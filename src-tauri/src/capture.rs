//! Rendered page capture (docs/RFC-page-capture.md, phase 1).
//!
//! When the reqwest fast path in `ingest::extract_url` comes back empty or
//! looking like a bot wall / JS shell, load the same URL in a hidden
//! zero-capability webview, wait for the page to settle, and run the
//! rendered DOM through the same readability extraction saved pages use.
//! Strictly local: no external service ever sees the URL.
//!
//! Security model matches the reader's live view (commands.rs): the capture
//! window's label (`capture-*`) matches no capability pattern, so the remote
//! page can invoke nothing — it is a plain browser surface outside the
//! app's IPC boundary. Results come back through the native WKWebView
//! `evaluateJavaScript` completion handler, not Tauri IPC.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use tauri::AppHandle;
use tokio::sync::Semaphore;

use crate::ingest::{self, Extracted};

/// WKWebView's default UA lacks the `Version/x Safari/x` suffix real Safari
/// sends, and some bot checks key on exactly that. Claim the full Safari UA —
/// it is barely a lie, we *are* that engine (RFC §1).
const SAFARI_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15";

/// Poll cadence for the settle detector.
const POLL: Duration = Duration::from_millis(250);
/// Network + DOM must both be quiet this long to count as settled.
const QUIET_MS: i64 = 600;
/// Never accept "settled" before this much wall clock, even when the page
/// reports quiet immediately — cheap insurance against a too-eager settle.
const MIN_DWELL: Duration = Duration::from_millis(800);
/// When the init script never took (CSP blocked the `WKUserScript`, exotic
/// document), we have no mutation/network signal — only readyState, which
/// goes `complete` on the empty SPA shell before hydration. Give hydration a
/// fixed budget before extracting rather than settling instantly (the 253 ms
/// 0-char captures this replaced).
const DEGRADED_DWELL: Duration = Duration::from_millis(3500);
/// Give up waiting and extract whatever exists after this long (RFC §3).
const SETTLE_CAP: Duration = Duration::from_secs(10);
/// Hard ceiling on one whole capture, teardown included.
const TOTAL_CAP: Duration = Duration::from_secs(30);

/// Injected before any page script runs (WKUserScript at document start, so
/// it re-arms across redirects). Counts in-flight fetch/XHR for a network-
/// quiet signal — no CDP needed — and timestamps DOM mutations.
const INIT_JS: &str = r#"
(() => {
  if (window.__alcapState) return;
  const s = { loaded: false, inflight: 0, lastNet: Date.now(), lastMut: Date.now() };
  window.__alcapState = s;
  try {
    const of = window.fetch;
    if (of) window.fetch = function (...a) {
      s.inflight++; s.lastNet = Date.now();
      return of.apply(this, a).finally(() => {
        s.inflight = Math.max(0, s.inflight - 1); s.lastNet = Date.now();
      });
    };
  } catch (e) {}
  try {
    const send = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.send = function (...a) {
      s.inflight++; s.lastNet = Date.now();
      this.addEventListener('loadend', () => {
        s.inflight = Math.max(0, s.inflight - 1); s.lastNet = Date.now();
      }, { once: true });
      return send.apply(this, a);
    };
  } catch (e) {}
  const arm = () => {
    try {
      new MutationObserver(() => { s.lastMut = Date.now(); }).observe(
        document.documentElement,
        { subtree: true, childList: true, characterData: true, attributes: true }
      );
    } catch (e) {}
  };
  if (document.documentElement) arm();
  else document.addEventListener('DOMContentLoaded', arm, { once: true });
  if (document.readyState === 'complete') s.loaded = true;
  else window.addEventListener('load', () => { s.loaded = true; }, { once: true });
})();
"#;

/// Settle probe. Degrades gracefully when the init script didn't take
/// (about:blank, exotic documents): readyState alone then decides.
const POLL_JS: &str = r#"
(() => {
  try {
    const s = window.__alcapState || null;
    return JSON.stringify({
      hs: !!s,
      rs: document.readyState,
      loaded: !!((s && s.loaded) || document.readyState === 'complete'),
      inflight: s ? s.inflight : 0,
      mq: s && s.lastMut ? Date.now() - s.lastMut : 99999,
      nq: s && s.lastNet ? Date.now() - s.lastNet : 99999
    });
  } catch (e) { return '{}'; }
})();
"#;

/// Serialize the rendered DOM plus the bits readability can't recover.
const EXTRACT_JS: &str = r#"
(() => {
  try {
    return JSON.stringify({
      ok: true,
      title: document.title || '',
      html: document.documentElement ? document.documentElement.outerHTML : ''
    });
  } catch (e) {
    return JSON.stringify({ ok: false, error: String((e && e.message) || e) });
  }
})();
"#;

#[derive(serde::Deserialize, Default)]
struct SettleState {
    /// Was our init script present in this frame? False ⇒ degraded probe
    /// (readyState only), which the dwell floor compensates for.
    #[serde(default)]
    hs: bool,
    #[serde(default)]
    rs: String,
    #[serde(default)]
    loaded: bool,
    #[serde(default)]
    inflight: i64,
    #[serde(default)]
    mq: i64,
    #[serde(default)]
    nq: i64,
}

impl SettleState {
    fn settled(&self) -> bool {
        self.loaded && self.inflight <= 0 && self.mq >= QUIET_MS && self.nq >= QUIET_MS
    }
}

#[derive(serde::Deserialize)]
struct ExtractPayload {
    ok: bool,
    #[serde(default)]
    title: String,
    #[serde(default)]
    html: String,
    #[serde(default)]
    error: String,
}

struct Ctx {
    app: AppHandle,
    trace_dir: PathBuf,
}

static CTX: OnceLock<Ctx> = OnceLock::new();
static SEQ: AtomicU64 = AtomicU64::new(0);
/// One capture at a time — a re-sync sweep must not open a window per source.
static SLOT: Semaphore = Semaphore::const_new(1);

/// Called once from setup. Until then (and in tests) escalation is silently
/// unavailable and `extract_url_rescued` behaves exactly like `extract_url`.
pub fn init(app: AppHandle, trace_dir: PathBuf) {
    let _ = CTX.set(Ctx { app, trace_dir });
}

/// Below this many extracted chars a URL "success" is treated as suspect and
/// escalated anyway. Sits above `looks_blocked`'s 200-char floor on purpose:
/// real pages this small are almost always a pre-hydration shell (Apple's
/// HIG pages fast-fetch to ~260 chars of chrome), and the strictly-better
/// acceptance rule below makes a wasted render harmless.
const THIN_CHARS: usize = 500;

/// `ingest::extract_url` with the rendered-capture rescue. Escalates on the
/// existing `looks_blocked` heuristic plus a thin-content trigger; a rescue
/// is accepted only when the rendered text passes `looks_blocked` AND beats
/// the fast path's length, so a failed rescue never makes a source worse
/// than the fast path left it.
pub async fn extract_url_rescued(url: &str) -> Result<Extracted> {
    let fast = ingest::extract_url(url).await;
    // Google export endpoints are authoritative plain text — rendering
    // docs.google.com without login can't beat them (assisted capture is
    // phase 3).
    if ingest::is_google_doc_url(url) {
        return fast;
    }
    let fast_chars = fast.as_ref().map(|ex| ex.text.chars().count()).unwrap_or(0);
    let trigger = match &fast {
        Ok(ex) => match ingest::looks_blocked(&ex.text) {
            Some(reason) => reason,
            None if fast_chars < THIN_CHARS => {
                format!("Only {fast_chars} characters extracted — thin for a real page.")
            }
            None => return fast,
        },
        Err(err) => format!("{err:#}"),
    };
    if CTX.get().is_none() {
        return fast;
    }
    let t0 = Instant::now();
    match tokio::time::timeout(TOTAL_CAP, rendered_capture(url)).await {
        Ok(Ok(rendered)) => {
            let chars = rendered.extracted.text.chars().count();
            if ingest::looks_blocked(&rendered.extracted.text).is_some() {
                // Still a wall after rendering — keep the fast result (same
                // warning semantics as before this module existed).
                log_capture(url, "still_blocked", &trigger, &rendered, t0, chars);
                fast
            } else if chars <= fast_chars {
                // Unblocked but no more content than the fast path found —
                // the page really is that small. Keep the fast result.
                log_capture(url, "not_better", &trigger, &rendered, t0, chars);
                fast
            } else {
                log_capture(url, "rescued", &trigger, &rendered, t0, chars);
                Ok(rendered.extracted)
            }
        }
        Ok(Err(err)) => {
            log_failure(url, &trigger, &format!("{err:#}"), t0);
            fast
        }
        Err(_) => {
            log_failure(url, &trigger, "capture timed out", t0);
            fast
        }
    }
}

struct Rendered {
    extracted: Extracted,
    settled: bool,
    settle_ms: u128,
    /// Did the init script take on any poll? Feeds telemetry so a fleet of
    /// `saw_state:false` rescues points straight at CSP-blocked injection.
    saw_state: bool,
    /// Last observed `document.readyState` — diagnostic context for misses.
    ready_state: String,
}

/// Drive one hidden-webview capture: create the window, wait for settle,
/// extract, tear down. Serialized app-wide by `SLOT`.
async fn rendered_capture(url: &str) -> Result<Rendered> {
    let ctx = CTX
        .get()
        .ok_or_else(|| anyhow!("capture not initialized"))?;
    let parsed: tauri::Url = url.parse().with_context(|| format!("bad URL {url}"))?;
    anyhow::ensure!(
        parsed.scheme() == "https" || parsed.scheme() == "http",
        "only web pages can be captured"
    );
    let _permit = SLOT.acquire().await?;
    let label = format!("capture-{}", SEQ.fetch_add(1, Ordering::Relaxed));
    // Hidden and never focused; a realistic viewport so responsive layouts
    // render the desktop article, not a collapsed mobile shell.
    let window =
        tauri::WebviewWindowBuilder::new(&ctx.app, &label, tauri::WebviewUrl::External(parsed))
            .title("Alchemy page capture")
            .visible(false)
            .focused(false)
            .inner_size(1280.0, 900.0)
            .user_agent(SAFARI_UA)
            .initialization_script(INIT_JS)
            .build()
            .context("could not create capture window")?;

    let result = drive(&window, url).await;
    // Destroy, not close — no close-requested round trip for a window
    // nothing is watching.
    let _ = window.destroy();
    result
}

async fn drive(window: &tauri::WebviewWindow, url: &str) -> Result<Rendered> {
    let started = Instant::now();
    let mut settled = false;
    let mut saw_state = false;
    let mut ready_state = String::new();
    while started.elapsed() < SETTLE_CAP {
        tokio::time::sleep(POLL).await;
        // Eval failures mid-navigation are normal — keep polling; the cap
        // bounds the worst case.
        let Ok(raw) = eval_string(window, POLL_JS).await else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<SettleState>(&raw) else {
            continue;
        };
        saw_state |= state.hs;
        ready_state = state.rs.clone();
        // With the init script present, `settled()` already can't fire before
        // QUIET_MS of quiet; the floor is just insurance. Without it, the
        // probe is readyState-only and would settle on the pre-hydration
        // shell — hold for DEGRADED_DWELL so client rendering can run.
        let floor = if saw_state { MIN_DWELL } else { DEGRADED_DWELL };
        if started.elapsed() >= floor && state.settled() {
            settled = true;
            break;
        }
    }
    let settle_ms = started.elapsed().as_millis();

    let raw = eval_string(window, EXTRACT_JS)
        .await
        .context("extraction script failed")?;
    let payload: ExtractPayload =
        serde_json::from_str(&raw).context("unexpected extraction payload")?;
    anyhow::ensure!(payload.ok, "extraction failed in page: {}", payload.error);
    anyhow::ensure!(!payload.html.trim().is_empty(), "rendered DOM was empty");
    Ok(Rendered {
        extracted: ingest::extracted_from_html(&payload.html, url, &payload.title),
        settled,
        settle_ms,
        saw_state,
        ready_state,
    })
}

/// Evaluate JS in the capture webview and return its string result via the
/// native WKWebView completion handler (same objc recipe as print/spotlight).
async fn eval_string(window: &tauri::WebviewWindow, js: &'static str) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
        window
            .with_webview(move |wv| unsafe { mac_eval(wv.inner().cast(), js, tx) })
            .map_err(|e| anyhow!("webview gone: {e}"))?;
        tauri::async_runtime::spawn_blocking(move || {
            rx.recv_timeout(Duration::from_secs(15))
                .unwrap_or_else(|e| Err(e.to_string()))
        })
        .await
        .context("eval task failed")?
        .map_err(|e| anyhow!(e))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, js);
        Err(anyhow!("rendered capture is macOS-only for now"))
    }
}

/// Runs on the main thread via `with_webview`. The completion handler fires
/// on the main queue; the mpsc channel carries the result back.
#[cfg(target_os = "macos")]
unsafe fn mac_eval(
    webview: *mut objc2::runtime::AnyObject,
    js: &str,
    tx: std::sync::mpsc::Sender<Result<String, String>>,
) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSError, NSString};

    let script = NSString::from_str(js);
    let done = block2::RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
        let out = if !error.is_null() {
            Err((*error).localizedDescription().to_string())
        } else if result.is_null() {
            Ok(String::new())
        } else {
            // Our scripts return JS strings, which arrive as NSString;
            // `description` stringifies anything else safely.
            let desc: *mut NSString = msg_send![result, description];
            if desc.is_null() {
                Ok(String::new())
            } else {
                Ok((*desc).to_string())
            }
        };
        let _ = tx.send(out);
    });
    let _: () = msg_send![webview, evaluateJavaScript: &*script, completionHandler: &*done];
}

// ---- Telemetry (RFC §7 phase 1) --------------------------------------------
//
// One JSONL line per escalation in traces/capture.jsonl — enough to compute
// escalation rate, rescue rate, and settle percentiles offline. Local-only,
// same rules as retrieval traces.

fn log_capture(
    url: &str,
    outcome: &str,
    trigger: &str,
    rendered: &Rendered,
    t0: Instant,
    chars: usize,
) {
    let Some(ctx) = CTX.get() else { return };
    crate::trace::log_file(
        &ctx.trace_dir,
        "capture.jsonl",
        serde_json::json!({
            "ts": unix_now(),
            "url": url,
            "outcome": outcome,
            "trigger": trigger,
            "settled": rendered.settled,
            "sawState": rendered.saw_state,
            "readyState": rendered.ready_state,
            "settleMs": rendered.settle_ms,
            "totalMs": t0.elapsed().as_millis(),
            "chars": chars,
        }),
    );
}

fn log_failure(url: &str, trigger: &str, error: &str, t0: Instant) {
    let Some(ctx) = CTX.get() else { return };
    crate::trace::log_file(
        &ctx.trace_dir,
        "capture.jsonl",
        serde_json::json!({
            "ts": unix_now(),
            "url": url,
            "outcome": "capture_failed",
            "trigger": trigger,
            "error": error,
            "totalMs": t0.elapsed().as_millis(),
        }),
    );
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_state_parses_and_degrades() {
        // Full probe payload.
        let s: SettleState =
            serde_json::from_str(r#"{"loaded":true,"inflight":0,"mq":700,"nq":800}"#).unwrap();
        assert!(s.settled());
        // In-flight requests hold settle open.
        let s: SettleState =
            serde_json::from_str(r#"{"loaded":true,"inflight":2,"mq":700,"nq":800}"#).unwrap();
        assert!(!s.settled());
        // Recent DOM churn holds settle open.
        let s: SettleState =
            serde_json::from_str(r#"{"loaded":true,"inflight":0,"mq":100,"nq":800}"#).unwrap();
        assert!(!s.settled());
        // The `'{}'` degraded probe: never settles, the cap decides.
        let s: SettleState = serde_json::from_str("{}").unwrap();
        assert!(!s.settled());
    }

    #[test]
    fn extract_payload_parses_error_shape() {
        let p: ExtractPayload = serde_json::from_str(r#"{"ok":false,"error":"boom"}"#).unwrap();
        assert!(!p.ok);
        assert_eq!(p.error, "boom");
        let p: ExtractPayload =
            serde_json::from_str(r#"{"ok":true,"title":"T","html":"<html></html>"}"#).unwrap();
        assert!(p.ok && p.html.starts_with("<html"));
    }

    #[test]
    fn injected_scripts_are_iifes() {
        // A stray top-level `return` or unbalanced brace would make every
        // capture fail at runtime; keep the scripts shaped like expressions.
        for js in [INIT_JS, POLL_JS, EXTRACT_JS] {
            let trimmed = js.trim();
            assert!(
                trimmed.starts_with("(() => {"),
                "not an IIFE: {trimmed:.40}"
            );
            assert!(trimmed.ends_with("})();"), "unterminated IIFE");
            assert_eq!(
                trimmed.matches('{').count(),
                trimmed.matches('}').count(),
                "unbalanced braces"
            );
        }
    }
}
