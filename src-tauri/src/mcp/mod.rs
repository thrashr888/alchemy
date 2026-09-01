//! Embedded MCP server — agent access to notebooks, sources, notes, and
//! hybrid search over localhost streamable HTTP (see docs/RFC-mcp-server.md).
//!
//! One process owns everything: tools run against the same `AppState` the UI
//! commands use, and every mutation emits `mcp://changed` so open windows
//! refresh live while an agent works.

use rmcp::{
    model::*,
    schemars, tool_handler,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData as McpError, ServerHandler,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{self, AppState};
use crate::db::NOTEBOOK_PALETTE;
use crate::models::{Note, Notebook, Source};

mod diagnostics;
mod homechat;
mod ledger;
mod mac;
mod notebooks;
mod notes;
mod registry;
mod search;
mod settings;
mod sources;
mod studio;

// ---- Server lifecycle ------------------------------------------------------

/// Managed handle to the running server, if any. Settings toggles it.
#[derive(Default)]
pub struct McpState {
    running: std::sync::Mutex<Option<Running>>,
}

struct Running {
    port: u16,
    shutdown: tokio_util::sync::CancellationToken,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub running: bool,
    pub port: u16,
    pub url: String,
}

/// Dev builds bind one port above the configured one: a dev instance and
/// the installed app share config + data dir, so without the offset their
/// MCP servers collide on the same port (first one wins, the other dies
/// with a bind error at launch).
fn effective_port(configured: u16) -> u16 {
    if cfg!(debug_assertions) {
        configured.saturating_add(1)
    } else {
        configured
    }
}

/// Start the server if the config wants it running (app launch + settings
/// save). Stops first when the port changed or it was disabled.
pub async fn apply_config(app: &AppHandle, enabled: bool, port: u16) {
    let port = effective_port(port);
    let token = match auth_token(app) {
        Ok(token) => token,
        Err(err) => {
            crate::diagnostics::error(
                "mcp",
                format!("could not initialize local authentication: {err:#}"),
            );
            return;
        }
    };
    let mcp = app.state::<McpState>();
    {
        let mut running = mcp.running.lock().unwrap();
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
    match start_server(app.clone(), port, token.clone()).await {
        Ok(shutdown) => {
            *mcp.running.lock().unwrap() = Some(Running { port, shutdown });
            if let Err(err) = write_port_file(app, port, &token) {
                crate::diagnostics::error(
                    "mcp",
                    format!("could not write private discovery file: {err:#}"),
                );
            }
        }
        Err(err) => crate::diagnostics::error(
            "mcp",
            format!("failed to start on 127.0.0.1:{port}: {err:#}"),
        ),
    }
}

pub fn status(app: &AppHandle) -> McpStatus {
    let mcp = app.state::<McpState>();
    let running = mcp.running.lock().unwrap();
    let port = running.as_ref().map(|r| r.port).unwrap_or(0);
    McpStatus {
        running: running.is_some(),
        port,
        url: format!("http://127.0.0.1:{port}/mcp"),
    }
}

const TOKEN_FILE: &str = "mcp-token";
static TOKEN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Read the stable per-installation bearer token, creating it with 256 bits of
/// randomness when this installation has not served MCP before. Keeping it in
/// a separate private file lets discovery JSON be replaced atomically without
/// rotating every configured agent connector.
pub(crate) fn auth_token(app: &AppHandle) -> anyhow::Result<String> {
    let _guard = TOKEN_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("authentication token lock was poisoned"))?;
    let dir = app.path().app_data_dir()?;
    crate::secure_fs::ensure_private_dir(&dir)?;
    let path = dir.join(TOKEN_FILE);
    crate::secure_fs::ensure_private_file(&path)?;
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let token = existing.trim();
        if token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(token.to_string());
        }
    }

    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    crate::secure_fs::write_private_file(&path, token.as_bytes())?;
    Ok(token)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (&a, &b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

fn bearer_authorized(headers: &axum::http::HeaderMap, expected: &str) -> bool {
    let Some(value) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(token.as_bytes(), expected.as_bytes())
}

/// Authenticate every request before rmcp allocates a session. The Origin
/// rejection remains useful defense in depth against a token ever being
/// exposed to renderer content.
async fn authorize_request(
    axum::extract::State(expected): axum::extract::State<String>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    if req.headers().contains_key(axum::http::header::ORIGIN) {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }
    if !bearer_authorized(req.headers(), &expected) {
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(req).await)
}

async fn start_server(
    app: AppHandle,
    port: u16,
    token: String,
) -> anyhow::Result<tokio_util::sync::CancellationToken> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let shutdown = tokio_util::sync::CancellationToken::new();

    // Sessions only exist for legacy-protocol clients now: under MCP
    // 2026-07-28 the transport is stateless (rmcp 3 default keeps
    // `legacy_session_mode` on for older clients). For those legacy sessions,
    // the keep-alive kills one after that much time without a single session
    // event — and a batch of parallel add_source imports (scanned PDFs OCR'ing
    // page by page) can sit quiet far longer than the 5-minute default,
    // terminating the session under every still-running call ("Session
    // service terminated", seen live with 6 parallel PDF imports). Long
    // imports also heartbeat progress (see sources.rs), but the ceiling must
    // outlast the slowest legitimate call: the 20-minute generation watchdog,
    // plus margin.
    let mut sessions = LocalSessionManager::default();
    sessions.session_config.keep_alive = Some(std::time::Duration::from_secs(30 * 60));
    let service = StreamableHttpService::new(
        move || Ok(AlchemyMcp::new(app.clone())),
        sessions.into(),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service("/mcp", service).layer(
        axum::middleware::from_fn_with_state(token, authorize_request),
    );

    let ct = shutdown.clone();
    tauri::async_runtime::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { ct.cancelled().await })
            .await;
    });
    Ok(shutdown)
}

/// Dev builds advertise themselves in their own file. They share the app
/// data dir with the installed app, and clobbering mcp.json pointed the CLI
/// and every discovering agent at a dead port once the dev instance quit —
/// the +1 port offset kept the servers apart but not their discovery.
const fn discovery_file_name() -> &'static str {
    if cfg!(debug_assertions) {
        "mcp.dev.json"
    } else {
        "mcp.json"
    }
}

/// Discovery file so tooling can find the server without hardcoding the port.
fn write_port_file(app: &AppHandle, port: u16, token: &str) -> anyhow::Result<()> {
    let dir = app.path().app_data_dir()?;
    let info = serde_json::json!({
        "port": port,
        "url": format!("http://127.0.0.1:{port}/mcp"),
        "pid": std::process::id(),
        "token": token,
    });
    crate::secure_fs::write_private_file(
        &dir.join(discovery_file_name()),
        serde_json::to_string(&info)?.as_bytes(),
    )?;
    Ok(())
}

fn remove_port_file(app: &AppHandle) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::remove_file(dir.join(discovery_file_name()));
    }
}

// ---- Tauri commands (Settings UI) -----------------------------------------

#[tauri::command]
pub fn mcp_status(app: AppHandle) -> McpStatus {
    status(&app)
}

// ---- Shared tool plumbing --------------------------------------------------

/// The one request shape several domains share; domain-specific shapes live
/// beside their tools.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub(super) struct NotebookIdReq {
    /// Notebook id (from list_notebooks).
    pub(super) notebook_id: String,
}

// ---- The MCP service --------------------------------------------------------

#[derive(Clone)]
pub struct AlchemyMcp {
    app: AppHandle,
}

fn internal(err: impl std::fmt::Display) -> McpError {
    McpError::internal_error(format!("{err:#}"), None)
}

fn invalid(msg: impl Into<String>) -> McpError {
    McpError::invalid_params(msg.into(), None)
}

fn json_result(value: &impl Serialize) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(value).map_err(internal)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Strip full content from a source for list payloads (same as the UI does).
fn slim(s: Source) -> Source {
    Source {
        content: String::new(),
        ..s
    }
}

impl AlchemyMcp {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn state(&self) -> tauri::State<'_, AppState> {
        self.app.state::<AppState>()
    }

    /// Tell open windows something changed so lists refresh live.
    fn changed(&self, scope: &str, notebook_id: Option<&str>) {
        #[derive(Serialize, Clone)]
        #[serde(rename_all = "camelCase")]
        struct Changed<'a> {
            scope: &'a str,
            notebook_id: Option<&'a str>,
        }
        let _ = self
            .app
            .emit("mcp://changed", Changed { scope, notebook_id });
    }
}

// One tool router per domain file; the handler serves their sum.
// Parenthesized: the macro splices this expression in front of `.call(...)`,
// and method calls bind tighter than `+`.
#[tool_handler(router = (Self::notebooks_router()
    + Self::sources_router()
    + Self::mac_router()
    + Self::search_router()
    + Self::notes_router()
    + Self::studio_router()
    + Self::ledger_router()
    + Self::registry_router()
    + Self::settings_router()
    + Self::homechat_router()
    + Self::diagnostics_router()))]
impl ServerHandler for AlchemyMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("alchemy", env!("CARGO_PKG_VERSION")).with_title("Alchemy"),
            )
            .with_instructions(
                "Alchemy is the user's local-first research notebook: notebooks hold sources \
                 (documents, web pages, pasted text) and notes. Typical flow: list_notebooks \
                 (or create_notebook) → add_source for each URL/file/text → search to find \
                 relevant passages → write findings with create_note. Everything runs on the \
                 user's machine; search is cheap, call it freely.",
            )
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;

    #[test]
    fn bearer_auth_requires_exact_scheme_and_token() {
        let expected = "0123456789abcdef";
        let mut headers = axum::http::HeaderMap::new();
        assert!(!bearer_authorized(&headers, expected));

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Basic 0123456789abcdef".parse().unwrap(),
        );
        assert!(!bearer_authorized(&headers, expected));

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer 0123456789abcdee".parse().unwrap(),
        );
        assert!(!bearer_authorized(&headers, expected));

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer 0123456789abcdef".parse().unwrap(),
        );
        assert!(bearer_authorized(&headers, expected));
    }

    #[tokio::test]
    async fn auth_middleware_rejects_missing_invalid_and_browser_requests() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let router = axum::Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                "secret".to_string(),
                authorize_request,
            ));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let url = format!("http://{address}/");
        let client = reqwest::Client::new();

        assert_eq!(
            client.get(&url).send().await.unwrap().status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(&url)
                .bearer_auth("wrong")
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(&url)
                .bearer_auth("secret")
                .header("Origin", "https://example.com")
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::FORBIDDEN
        );
        assert_eq!(
            client
                .get(&url)
                .bearer_auth("secret")
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::OK
        );
    }
}
