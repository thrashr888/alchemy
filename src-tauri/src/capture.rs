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
/// Completeness pass: at most this many viewport-sized scroll steps (so
/// infinite feeds terminate), pausing between steps for lazy-loaders.
const SCROLL_STEPS_MAX: u32 = 5;
const SCROLL_PAUSE: Duration = Duration::from_millis(400);
/// Short re-settle after the scroll pass — the page is already hydrated,
/// we only wait out the stragglers the scroll just triggered.
const RESETTLE_CAP: Duration = Duration::from_secs(2);
/// A domain-memory entry older than this is re-probed via the fast path —
/// sites change, and a stale "rendered" marker would tax every add.
const MEMORY_TTL_SECS: u64 = 30 * 24 * 60 * 60;

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

/// Completeness pass, part 2 (RFC §3): after the scroll steps, wake lazy
/// images (`data-src`/`data-srcset`), drop full-viewport fixed/sticky
/// overlays (consent scrims sit over the article and readability sometimes
/// keeps them), and park scroll back at the top for a clean extraction.
const FINALIZE_JS: &str = r#"
(() => {
  try {
    let unlazy = 0, overlays = 0;
    for (const img of Array.from(document.querySelectorAll('img')).slice(0, 500)) {
      const src = img.getAttribute('data-src') || img.getAttribute('data-lazy-src');
      const srcset = img.getAttribute('data-srcset');
      if (src && !img.src) { img.src = src; unlazy++; }
      if (srcset && !img.getAttribute('srcset')) { img.setAttribute('srcset', srcset); unlazy++; }
      if (img.loading === 'lazy') img.loading = 'eager';
    }
    const vw = window.innerWidth || 1280, vh = window.innerHeight || 900;
    let seen = 0;
    for (const el of document.querySelectorAll('body *')) {
      if (++seen > 2500) break;
      const cs = getComputedStyle(el);
      if (cs.position !== 'fixed' && cs.position !== 'sticky') continue;
      const r = el.getBoundingClientRect();
      if (r.width * r.height > vw * vh * 0.55) { el.remove(); overlays++; }
    }
    window.scrollTo(0, 0);
    return JSON.stringify({ unlazy, overlays });
  } catch (e) { return '{}'; }
})();
"#;

/// Serialize the rendered DOM plus the bits readability can't recover:
/// the live `document.title` and OpenGraph title for SPA title gaps, and
/// byline/date metadata (meta tags + JSON-LD) for the provenance line.
const EXTRACT_JS: &str = r#"
(() => {
  try {
    const pick = (sel, attr) => {
      const el = document.querySelector(sel);
      if (!el) return '';
      return ((attr ? el.getAttribute(attr) : el.textContent) || '').trim();
    };
    let byline = pick('meta[name="author"]', 'content') ||
                 pick('meta[property="article:author"]', 'content');
    let published = pick('meta[property="article:published_time"]', 'content') ||
                    pick('meta[name="date"]', 'content') ||
                    pick('time[datetime]', 'datetime');
    for (const s of Array.from(document.querySelectorAll('script[type="application/ld+json"]')).slice(0, 5)) {
      if (byline && published) break;
      try {
        const d = JSON.parse(s.textContent);
        const nodes = Array.isArray(d) ? d : (d['@graph'] || [d]);
        for (const n of nodes) {
          if (!n || typeof n !== 'object') continue;
          if (!published && n.datePublished) published = String(n.datePublished);
          const a = n.author;
          if (!byline && a) {
            byline = Array.isArray(a) ? a.map(x => x && x.name || '').filter(Boolean).join(', ')
                                      : String((a && a.name) || '');
          }
        }
      } catch (e) {}
    }
    return JSON.stringify({
      ok: true,
      title: document.title || '',
      ogTitle: pick('meta[property="og:title"]', 'content'),
      byline: byline || '',
      published: published || '',
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
    #[serde(default, rename = "ogTitle")]
    og_title: String,
    #[serde(default)]
    byline: String,
    #[serde(default)]
    published: String,
    #[serde(default)]
    html: String,
    #[serde(default)]
    error: String,
}

#[derive(serde::Deserialize, Default)]
struct FinalizeStats {
    #[serde(default)]
    unlazy: u32,
    #[serde(default)]
    overlays: u32,
}

/// One scroll step's page geometry — how far the scrollable content extends
/// versus the viewport, so we know when we've reached the bottom.
#[derive(serde::Deserialize, Default)]
struct ScrollProbe {
    #[serde(default)]
    sh: f64,
    #[serde(default)]
    ih: f64,
}

struct Ctx {
    app: AppHandle,
    trace_dir: PathBuf,
    memory_path: PathBuf,
}

static CTX: OnceLock<Ctx> = OnceLock::new();
static SEQ: AtomicU64 = AtomicU64::new(0);
/// One capture at a time — a re-sync sweep must not open a window per source.
static SLOT: Semaphore = Semaphore::const_new(1);
/// Capture memory (RFC §4): domain → unix-seconds of the last successful
/// rendered capture. Only rendered-winning domains are listed; everything
/// else defaults to the fast path. Loaded lazily, persisted best-effort.
static MEMORY: OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> = OnceLock::new();

/// Called once from setup. Until then (and in tests) escalation is silently
/// unavailable and `extract_url_rescued` behaves exactly like `extract_url`.
pub fn init(app: AppHandle, data_dir: PathBuf) {
    let _ = CTX.set(Ctx {
        app,
        trace_dir: data_dir.join("traces"),
        memory_path: data_dir.join("capture_domains.json"),
    });
}

fn memory() -> &'static std::sync::Mutex<std::collections::HashMap<String, u64>> {
    MEMORY.get_or_init(|| {
        let loaded = CTX
            .get()
            .and_then(|ctx| std::fs::read_to_string(&ctx.memory_path).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        std::sync::Mutex::new(loaded)
    })
}

/// Is this URL's domain remembered as webview-first (and not stale)?
fn remembered_rendered(url: &str) -> bool {
    let Some(domain) = domain_of(url) else {
        return false;
    };
    memory()
        .lock()
        .map(|m| {
            m.get(&domain)
                .is_some_and(|ts| unix_now().saturating_sub(*ts) < MEMORY_TTL_SECS)
        })
        .unwrap_or(false)
}

/// Record (or clear) a domain's rendered-first standing and persist.
fn remember(url: &str, rendered_won: bool) {
    let Some(domain) = domain_of(url) else { return };
    let Ok(mut m) = memory().lock() else { return };
    let changed = if rendered_won {
        // Insert or refresh — the timestamp is the TTL clock either way.
        m.insert(domain, unix_now());
        true
    } else {
        m.remove(&domain).is_some()
    };
    if !changed {
        return;
    }
    let snapshot = m.clone();
    drop(m);
    if let (Some(ctx), Ok(json)) = (CTX.get(), serde_json::to_string_pretty(&snapshot)) {
        if let Err(err) = std::fs::write(&ctx.memory_path, json) {
            eprintln!("capture memory write failed: {err}");
        }
    }
}

fn domain_of(url: &str) -> Option<String> {
    url.parse::<tauri::Url>()
        .ok()?
        .host_str()
        .map(|h| h.to_ascii_lowercase())
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
    // Google export endpoints are authoritative plain text — rendering
    // docs.google.com without login can't beat them (assisted capture is
    // phase 3).
    if ingest::is_google_doc_url(url) {
        return ingest::extract_url(url).await;
    }
    // Capture memory (RFC §4): a domain that last won via rendering skips
    // the doomed fast fetch and goes straight to the webview. A rendered
    // miss falls through to the fast path, and a fast win clears the marker.
    if CTX.get().is_some() && remembered_rendered(url) {
        let t0 = Instant::now();
        match tokio::time::timeout(TOTAL_CAP, rendered_capture(url)).await {
            Ok(Ok(rendered)) if ingest::looks_blocked(&rendered.extracted.text).is_none() => {
                let chars = rendered.extracted.text.chars().count();
                remember(url, true);
                log_capture(url, "rendered_first", "domain memory", &rendered, t0, chars);
                return Ok(rendered.extracted);
            }
            _ => {}
        }
    }
    let fast = ingest::extract_url(url).await;
    let fast_chars = fast.as_ref().map(|ex| ex.text.chars().count()).unwrap_or(0);
    let trigger = match &fast {
        Ok(ex) => match ingest::looks_blocked(&ex.text) {
            Some(reason) => reason,
            None if fast_chars < THIN_CHARS => {
                format!("Only {fast_chars} characters extracted — thin for a real page.")
            }
            None => {
                remember(url, false);
                return fast;
            }
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
                remember(url, true);
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
    /// Completeness pass: scroll steps taken, whether the page grew while
    /// scrolling (lazy content actually loaded), and finalize counts.
    scroll_steps: u32,
    grew: bool,
    unlazy: u32,
    overlays: u32,
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

    // Completeness pass (RFC §3): step-scroll toward the bottom so
    // IntersectionObserver lazy-loaders and infinite feeds fire, capped so
    // endless timelines terminate. Each step scrolls the dominant inner
    // container as well as the window — a bare `window.scrollTo` never moves
    // an `overflow:auto` feed, so its lazy content would otherwise stay dark.
    let mut scroll_steps: u32 = 0;
    let mut grew = false;
    let mut probe = scroll_step(window, 1).await.unwrap_or_default();
    let initial_sh = probe.sh;
    if probe.sh > probe.ih && probe.ih > 0.0 {
        for step in 2..=SCROLL_STEPS_MAX + 1 {
            scroll_steps = step - 1;
            let y = probe.ih * (step - 1) as f64;
            tokio::time::sleep(SCROLL_PAUSE).await;
            match scroll_step(window, step).await {
                Ok(next) => {
                    grew |= next.sh > initial_sh + 1.0;
                    let done = y >= next.sh;
                    probe = next;
                    if done {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
    let finalize: FinalizeStats = eval_string(window, FINALIZE_JS)
        .await
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    // Give the lazy content a short re-settle before serializing; the full
    // dwell machinery isn't needed — the page is already hydrated.
    let resettle_started = Instant::now();
    while resettle_started.elapsed() < RESETTLE_CAP {
        tokio::time::sleep(POLL).await;
        let Ok(raw) = eval_string(window, POLL_JS).await else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<SettleState>(&raw) else {
            continue;
        };
        if state.settled() {
            break;
        }
    }

    let raw = eval_string(window, EXTRACT_JS)
        .await
        .context("extraction script failed")?;
    let payload: ExtractPayload =
        serde_json::from_str(&raw).context("unexpected extraction payload")?;
    anyhow::ensure!(payload.ok, "extraction failed in page: {}", payload.error);
    anyhow::ensure!(!payload.html.trim().is_empty(), "rendered DOM was empty");
    let meta = ingest::PageMeta {
        og_title: payload.og_title,
        byline: payload.byline,
        published: payload.published,
    };
    Ok(Rendered {
        extracted: ingest::extracted_from_html(&payload.html, url, &payload.title, &meta),
        settled,
        settle_ms,
        saw_state,
        ready_state,
        scroll_steps,
        grew,
        unlazy: finalize.unlazy,
        overlays: finalize.overlays,
    })
}

/// Scroll to `step × viewport` — moving both the window and the dominant
/// inner scroll container (feeds and panes live in an `overflow:auto`
/// element a bare `window.scrollTo` never budges) — then report page
/// geometry so the caller knows when the bottom is reached.
async fn scroll_step(window: &tauri::WebviewWindow, step: u32) -> Result<ScrollProbe> {
    let js = format!(
        r#"(() => {{
  const ih = window.innerHeight || 900;
  // Dominant inner scroller: the tallest overflow-scroll element. Scanning
  // a bounded slice keeps this cheap on huge DOMs.
  let sc = null, best = 0, k = 0;
  for (const el of document.querySelectorAll('*')) {{
    if (++k > 4000) break;
    const over = el.scrollHeight - el.clientHeight;
    if (over < ih) continue;
    const oy = getComputedStyle(el).overflowY;
    if (oy !== 'auto' && oy !== 'scroll') continue;
    if (over > best) {{ best = over; sc = el; }}
  }}
  const y = {step} * ih;
  window.scrollTo(0, y);
  if (sc) sc.scrollTop = y;
  const sh = Math.max(
    document.body ? document.body.scrollHeight : 0,
    sc ? sc.scrollHeight : 0
  );
  return JSON.stringify({{ sh, ih }});
}})();"#
    );
    let raw = eval_string(window, js).await?;
    serde_json::from_str(&raw).context("bad scroll probe")
}

/// Evaluate JS in the capture webview and return its string result via the
/// native WKWebView completion handler (same objc recipe as print/spotlight).
async fn eval_string(window: &tauri::WebviewWindow, js: impl Into<String>) -> Result<String> {
    let js: String = js.into();
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
        window
            .with_webview(move |wv| unsafe { mac_eval(wv.inner().cast(), &js, tx) })
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
            "scrollSteps": rendered.scroll_steps,
            "grew": rendered.grew,
            "unlazy": rendered.unlazy,
            "overlays": rendered.overlays,
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
    fn domain_memory_round_trips() {
        // No CTX in tests: the map lives in memory only, which is exactly
        // what we want to exercise.
        assert!(!remembered_rendered("https://memtest-a.example/x"));
        remember("https://memtest-a.example/x", true);
        assert!(remembered_rendered("https://memtest-a.example/y"));
        remember("https://memtest-a.example/z", false);
        assert!(!remembered_rendered("https://memtest-a.example/x"));
        // Not a URL → never remembered, never panics.
        remember("not a url", true);
        assert!(!remembered_rendered("not a url"));
    }

    #[test]
    fn injected_scripts_are_iifes() {
        // A stray top-level `return` or unbalanced brace would make every
        // capture fail at runtime; keep the scripts shaped like expressions.
        for js in [INIT_JS, POLL_JS, EXTRACT_JS, FINALIZE_JS] {
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
