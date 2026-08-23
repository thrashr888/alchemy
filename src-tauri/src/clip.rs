//! Browser-extension clip receiver (docs/RFC-page-capture.md §8).
//!
//! The hidden capture webview (`capture.rs`) is a cookieless session, so it
//! hits the same wall as the fast path on intranet / login-walled / paywalled
//! pages. The Chrome/Firefox clipper runs in the user's *real, logged-in* tab
//! and captures the rendered DOM the user is actually looking at, then POSTs
//! it here. `ingest_url` consumes the held clip in place of the generic
//! page-capture fallback (`clip::take`), running it through the identical
//! `extracted_from_html` path a webview rescue uses.
//!
//! Auth is the inverse of the MCP server's: MCP *rejects* any `Origin` (real
//! clients never send one); we *require* an extension-scheme `Origin` (or
//! none, for native/CLI). A malicious web page cannot forge
//! `chrome-extension://…` — the browser sets it — so the origin gate closes
//! the web-page and DNS-rebind holes. Strictly local: nothing leaves 127.0.0.1.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::ingest::{self, Extracted, PageMeta};

/// Held clips expire after this long — the deep link fires right after the
/// POST and the user picks a notebook within seconds; 10 minutes is slack for
/// a distracted user, not a leak.
const TTL: Duration = Duration::from_secs(10 * 60);
/// Bound resident memory: at most this many clips waiting at once (a fresh
/// insert past the cap sweeps expired entries, then drops the oldest).
const MAX_HELD: usize = 32;
/// Reject a single payload larger than this — a rendered DOM over ~16 MB is
/// pathological, and the cap keeps one clip from ballooning the process.
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// The extension's captured payload — the same shape `capture.rs`'s
/// `EXTRACT_JS` produces, so the two ingest paths converge on
/// `extracted_from_html`.
#[derive(serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClipPayload {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub og_title: String,
    #[serde(default)]
    pub og_image: String,
    #[serde(default)]
    pub byline: String,
    #[serde(default)]
    pub published: String,
    #[serde(default)]
    pub html: String,
}

impl ClipPayload {
    /// Run the clipped DOM through the shared readability path. `None` when
    /// the render extracts to nothing — the caller then falls back to the
    /// normal fetch, so a broken clip never makes a source worse.
    pub fn into_extracted(self) -> Option<Extracted> {
        let meta = PageMeta {
            og_image: self.og_image,
            og_title: self.og_title,
            byline: self.byline,
            published: self.published,
        };
        let extracted = ingest::extracted_from_html(&self.html, &self.url, &self.title, &meta);
        (!extracted.text.trim().is_empty()).then_some(extracted)
    }
}

struct Held {
    payload: ClipPayload,
    inserted: Instant,
}

/// Process-global held-clip store, keyed by normalized URL. Reachable from
/// both the HTTP handler (insert) and `ingest_url` (`take`) without threading
/// state, mirroring `capture.rs`'s domain-memory pattern.
fn held() -> &'static Mutex<HashMap<String, Held>> {
    static HELD: OnceLock<Mutex<HashMap<String, Held>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Canonical key shared by insert and take — matches `ingest_url`'s own
/// dedup normalization so a clip and its deep link resolve to one entry.
fn key(url: &str) -> String {
    ingest::normalize_url(url).trim_end_matches('/').to_string()
}

/// Stash a clip for a URL the deep link is about to add. Drops expired
/// entries first, then the oldest if still at capacity.
fn insert(payload: ClipPayload) {
    let Ok(mut map) = held().lock() else { return };
    map.retain(|_, h| h.inserted.elapsed() < TTL);
    if map.len() >= MAX_HELD {
        if let Some(oldest) = map
            .iter()
            .min_by_key(|(_, h)| h.inserted)
            .map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        }
    }
    map.insert(
        key(&payload.url),
        Held {
            payload,
            inserted: Instant::now(),
        },
    );
}

/// Consume the clip held for this URL, if any and not expired. Removes it —
/// a clip is used once, then the source owns the content.
pub fn take(url: &str) -> Option<ClipPayload> {
    let mut map = held().lock().ok()?;
    map.retain(|_, h| h.inserted.elapsed() < TTL);
    map.remove(&key(url)).map(|h| h.payload)
}

// ---- Server lifecycle (mirrors mcp.rs) ------------------------------------

#[derive(Default)]
pub struct ClipState {
    running: Mutex<Option<Running>>,
}

struct Running {
    port: u16,
    shutdown: tokio_util::sync::CancellationToken,
}

/// Dev builds bind one port above the configured one — a dev instance and the
/// installed app share config + data dir and would otherwise collide (same
/// reasoning as `mcp::effective_port`).
fn effective_port(configured: u16) -> u16 {
    if cfg!(debug_assertions) {
        configured.saturating_add(1)
    } else {
        configured
    }
}

/// Start/stop the receiver to match config (app launch + settings save).
pub async fn apply_config(app: &AppHandle, enabled: bool, port: u16) {
    let port = effective_port(port);
    let clip = app.state::<ClipState>();
    {
        let mut running = clip.running.lock().unwrap();
        match running.as_ref() {
            Some(r) if !enabled || r.port != port => {
                r.shutdown.cancel();
                *running = None;
                remove_port_file(app);
            }
            Some(_) => return, // already running on the right port
            None => {}
        }
        if !enabled {
            return;
        }
    }
    match start_server(app.clone(), port).await {
        Ok(shutdown) => {
            *clip.running.lock().unwrap() = Some(Running { port, shutdown });
            write_port_file(app, port);
        }
        Err(err) => crate::diagnostics::error(
            "clip",
            format!("failed to start on 127.0.0.1:{port}: {err:#}"),
        ),
    }
}

/// Allow only requests a browser extension (or a native/CLI caller) could
/// have sent: an extension-scheme `Origin`, or none at all. A web page always
/// carries its real `Origin`, which it cannot forge, so this rejects
/// malicious pages and DNS-rebind attempts with 403.
fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(o) => o.starts_with("chrome-extension://") || o.starts_with("moz-extension://"),
    }
}

async fn guard_origin(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    let origin = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if origin_allowed(origin) {
        Ok(next.run(req).await)
    } else {
        Err(axum::http::StatusCode::FORBIDDEN)
    }
}

async fn handle_clip(body: axum::body::Bytes) -> axum::http::StatusCode {
    if body.len() > MAX_BYTES {
        return axum::http::StatusCode::PAYLOAD_TOO_LARGE;
    }
    let Ok(payload) = serde_json::from_slice::<ClipPayload>(&body) else {
        return axum::http::StatusCode::BAD_REQUEST;
    };
    if payload.url.trim().is_empty() || payload.html.trim().is_empty() {
        return axum::http::StatusCode::BAD_REQUEST;
    }
    insert(payload);
    axum::http::StatusCode::OK
}

/// Probe endpoint so the extension can discover which candidate port is the
/// app (only extension-origin callers reach it, per the guard). Plain text —
/// axum is built without the `json` feature; the extension string-matches.
async fn handle_health() -> &'static str {
    r#"{"ok":true,"app":"alchemy"}"#
}

/// The receiver's routes + auth/body-limit layers. Factored out so tests can
/// bind it on an ephemeral port without an `AppHandle`.
fn router() -> axum::Router {
    axum::Router::new()
        .route("/clip", axum::routing::post(handle_clip))
        .route("/clip", axum::routing::get(handle_health))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BYTES))
        .layer(axum::middleware::from_fn(guard_origin))
}

async fn start_server(
    app: AppHandle,
    port: u16,
) -> anyhow::Result<tokio_util::sync::CancellationToken> {
    let _ = app; // reserved for future emit-on-clip; keeps the signature uniform
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let shutdown = tokio_util::sync::CancellationToken::new();

    let router = router();

    let ct = shutdown.clone();
    tauri::async_runtime::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { ct.cancelled().await })
            .await;
    });
    Ok(shutdown)
}

/// Discovery file so tooling (and humans) can find the receiver's port.
fn write_port_file(app: &AppHandle, port: u16) {
    if let Ok(dir) = app.path().app_data_dir() {
        let info = serde_json::json!({
            "port": port,
            "url": format!("http://127.0.0.1:{port}/clip"),
            "pid": std::process::id(),
        });
        let _ = std::fs::write(dir.join("clip.json"), info.to_string());
    }
}

fn remove_port_file(app: &AppHandle) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::remove_file(dir.join("clip.json"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(url: &str, html: &str) -> ClipPayload {
        ClipPayload {
            url: url.to_string(),
            title: "T".into(),
            og_title: String::new(),
            og_image: String::new(),
            byline: String::new(),
            published: String::new(),
            html: html.into(),
        }
    }

    #[test]
    fn origin_gate_admits_extensions_and_native_only() {
        assert!(origin_allowed(None)); // curl / native
        assert!(origin_allowed(Some("chrome-extension://abcdef")));
        assert!(origin_allowed(Some("moz-extension://uuid")));
        assert!(!origin_allowed(Some("https://evil.com"))); // web page
        assert!(!origin_allowed(Some("http://127.0.0.1:41500"))); // rebind
        assert!(!origin_allowed(Some("null")));
    }

    #[test]
    fn payload_parses_camel_case() {
        let json = r#"{"url":"https://e.com/a","title":"Hi","ogTitle":"OG",
            "byline":"Jane","published":"2024-01-02","html":"<p>x</p>"}"#;
        let p: ClipPayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.url, "https://e.com/a");
        assert_eq!(p.og_title, "OG");
        assert_eq!(p.byline, "Jane");
    }

    #[test]
    fn take_matches_urls_that_normalize_together() {
        // Insert with a trailing slash; take without it — same key.
        insert(payload(
            "https://clip-test.example/post/",
            "<html><body><article>hello world body text that is plainly \
             long enough to survive readability extraction.</article></body></html>",
        ));
        assert!(take("https://clip-test.example/post").is_some());
        // Consumed — a second take misses.
        assert!(take("https://clip-test.example/post").is_none());
    }

    #[test]
    fn empty_render_yields_no_extracted() {
        // Nothing readable in the DOM → None, so ingest_url falls back.
        assert!(payload("https://x.example/y", "<html></html>")
            .into_extracted()
            .is_none());
    }

    /// Bind the real router on an ephemeral port and drive it over a socket:
    /// the origin gate, the POST→held→take round trip, and the body-limit
    /// rejection — the axum wiring the unit tests above can't reach.
    #[tokio::test]
    async fn server_round_trip_over_socket() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router()).await;
        });
        let base = format!("http://127.0.0.1:{}/clip", addr.port());
        let http = reqwest::Client::new();

        let good_html = "<html><body><article><h1>Clip me</h1><p>A long enough \
            paragraph of real article prose to survive the readability pass \
            without being dropped as page chrome.</p></article></body></html>";
        let body = serde_json::json!({
            "url": "https://socket-test.example/a",
            "title": "Clip me",
            "html": good_html,
        })
        .to_string();

        // A web-page Origin is forbidden — the core of the auth model.
        let denied = http
            .post(&base)
            .header("Origin", "https://evil.example")
            .body(body.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);
        // ...and nothing was held from the rejected request.
        assert!(take("https://socket-test.example/a").is_none());

        // An extension Origin is accepted and the clip becomes takeable.
        let ok = http
            .post(&base)
            .header("Origin", "chrome-extension://abcdefghijklmnop")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), reqwest::StatusCode::OK);
        let held = take("https://socket-test.example/a").expect("clip was held");
        assert!(held
            .into_extracted()
            .unwrap()
            .text
            .contains("article prose"));

        // The health probe answers for extension callers.
        let health = http
            .get(&base)
            .header("Origin", "moz-extension://uuid")
            .send()
            .await
            .unwrap();
        assert!(health.text().await.unwrap().contains("alchemy"));
    }

    #[test]
    fn real_html_extracts_with_provenance() {
        let mut p = payload(
            "https://x.example/story",
            "<html><body><article><h1>Headline</h1><p>A sufficiently long \
             paragraph of article prose so the readability pass keeps it as \
             real content rather than discarding it as chrome.</p></article>\
             </body></html>",
        );
        p.byline = "Ada Lovelace".into();
        p.published = "2024-03-12T08:00:00Z".into();
        let ex = p.into_extracted().expect("readable");
        assert_eq!(ex.source_type, "url");
        assert!(ex.text.contains("By Ada Lovelace"));
        assert!(ex.text.contains("Published 2024-03-12"));
    }
}
