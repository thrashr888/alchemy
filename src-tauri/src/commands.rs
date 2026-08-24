//! Tauri command surface — the entire IPC API the React frontend calls.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

mod brief;
mod diagnostics;
mod ledger;
mod registry;
mod reports;
mod second_look;
mod weave;
pub(crate) use brief::ensure_default_brief;
pub use diagnostics::*;
pub use ledger::*;
pub use registry::*;
pub use reports::*;
pub use second_look::*;

use crate::ai::{Ai, AiConfig, GenStats};
use crate::db::Db;
use crate::db::NOTEBOOK_PALETTE;
use crate::models::{
    Citation, FolderScan, Message, ModelHealth, ModelStat, ModelStatus, Note, Notebook,
    ReportSchedule, Source,
};
use crate::{ingest, rag};

/// Accumulated generation throughput for one model (persisted to disk).
#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelStatAcc {
    pub samples: u64,
    pub total_tokens: u64,
    pub total_seconds: f64,
    pub last_tps: f64,
    /// Time to first streamed token over the chat surface, wall-clock from
    /// the send to the first chat://token emit — retrieval included, because
    /// that is the wait the user actually feels. serde(default) so stats
    /// files from before the field deserialize.
    #[serde(default)]
    pub ttft_samples: u64,
    #[serde(default)]
    pub total_ttft_ms: u64,
    #[serde(default)]
    pub last_ttft_ms: u64,
}

/// Wall-clock time to first token for one streamed answer: started when the
/// user's turn is accepted (retrieval included, because that is the wait the
/// user actually feels) and stopped by the first token the engine emits.
///
/// Cloneable so a streaming callback can hold one — every path that emits a
/// token calls `mark`, and only the first call sticks.
#[derive(Clone)]
pub struct TtftClock {
    start: std::time::Instant,
    first_ms: Arc<std::sync::atomic::AtomicU64>,
}

impl TtftClock {
    pub fn start() -> Self {
        Self {
            start: std::time::Instant::now(),
            first_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Record this instant if no token has arrived yet. Clamped to 1ms so an
    /// instant first token still reads as measured rather than as "never".
    pub fn mark(&self) {
        let _ = self.first_ms.compare_exchange(
            0,
            (self.start.elapsed().as_millis() as u64).max(1),
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Milliseconds to the first token; 0 when none ever arrived.
    pub fn ttft_ms(&self) -> u64 {
        self.first_ms.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Chat surfaces whose time-to-first-token is a fair measure of the model's
/// own responsiveness, and so feed Activity's fastest-models ranking.
/// "deep-research" is deliberately absent — see `AppState::record_ttft`.
const RANKED_TTFT_PATHS: [&str; 3] = ["chat", "ask-everything", "agent-pane"];

/// Where a deep-research turn spent its time before the answer streamed.
/// Filled by `agent::run` as it works; read once for the timing trace.
/// Atomics rather than a lock: the loop touches these on every step and the
/// numbers are independent counters, never a consistent snapshot.
#[derive(Clone, Default)]
pub struct AgentPhases {
    inner: Arc<AgentPhasesInner>,
}

#[derive(Default)]
struct AgentPhasesInner {
    planner_ms: std::sync::atomic::AtomicU64,
    search_ms: std::sync::atomic::AtomicU64,
    read_ms: std::sync::atomic::AtomicU64,
    steps: std::sync::atomic::AtomicU64,
}

impl AgentPhases {
    fn add(slot: &std::sync::atomic::AtomicU64, ms: u64) {
        slot.fetch_add(ms, std::sync::atomic::Ordering::Relaxed);
    }
    /// One planner decision call — the model choosing the next action.
    pub fn planner(&self, ms: u64) {
        Self::add(&self.inner.planner_ms, ms);
        Self::add(&self.inner.steps, 1);
    }
    /// One search action: embed + hybrid search + any rerank.
    pub fn search(&self, ms: u64) {
        Self::add(&self.inner.search_ms, ms);
    }
    /// One read action: fetches plus the distill calls.
    pub fn read(&self, ms: u64) {
        Self::add(&self.inner.read_ms, ms);
    }
    pub fn as_json(&self) -> serde_json::Value {
        use std::sync::atomic::Ordering::Relaxed;
        serde_json::json!({
            "plannerMs": self.inner.planner_ms.load(Relaxed),
            "searchMs": self.inner.search_ms.load(Relaxed),
            "readMs": self.inner.read_ms.load(Relaxed),
            "steps": self.inner.steps.load(Relaxed),
        })
    }
}

pub struct AppState {
    pub db: Arc<Db>,
    pub ai: tokio::sync::RwLock<Ai>,
    pub config_path: PathBuf,
    pub stats_path: PathBuf,
    /// Local-only retrieval trace JSONL lives here (trace.rs).
    pub trace_dir: PathBuf,
    pub model_stats: Mutex<HashMap<String, ModelStatAcc>>,
    /// Cancellation tokens for in-flight generations, one per scope ("chat",
    /// "artifact", …) so stopping a chat doesn't kill a running document.
    pub cancel: Mutex<HashMap<String, tokio_util::sync::CancellationToken>>,
    /// Serializes folder scans: the periodic rescan tick skips while a manual
    /// folder add/refresh holds it, so the same file is never ingested twice.
    pub folder_scan_lock: tokio::sync::Mutex<()>,
    /// Last successfully applied glass state per window label
    /// (enabled, dark, pinned) — evicted on window destroy in lib.rs.
    pub glass_applied: Mutex<HashMap<String, (bool, bool, bool)>>,
}

/// Background sweeps outlive any one command and hold no Tauri handle (the
/// `gist::spawn_sweep` shape lets reingest spawn them), so the handle they
/// need to announce their work lives here — the `services.rs`/`spotlight.rs`
/// OnceLock idiom, once more for the staff.
static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

pub(crate) fn set_app_handle(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

/// The same handle, for the code that runs outside any command and may run
/// before setup — `diagnostics` broadcasts fatals through it, and must cope
/// with the handle not existing yet.
pub(crate) fn app_handle() -> Option<tauri::AppHandle> {
    APP.get().cloned()
}

/// Announce a background write on the same event the MCP mutations use, so
/// an open window refreshes live instead of waiting to be navigated away
/// from and back. Silent no-op before setup, and by design after a failure:
/// a missed refresh must never be worth more than the write it followed.
pub(crate) fn notify_changed(scope: &str, notebook_id: Option<&str>) {
    let Some(app) = APP.get() else { return };
    let _ = app.emit(
        "mcp://changed",
        serde_json::json!({ "scope": scope, "notebookId": notebook_id }),
    );
}

impl AppState {
    /// Start a fresh cancellation scope for a new generation, returning its
    /// token. Supersedes any previous token in the same scope.
    pub fn begin_generation(&self, scope: &str) -> tokio_util::sync::CancellationToken {
        // Every user-initiated generation flows through here — the curator's
        // idle gate reads this as "the user is around".
        touch_activity();
        let token = tokio_util::sync::CancellationToken::new();
        self.cancel
            .lock()
            .unwrap()
            .insert(scope.to_string(), token.clone());
        token
    }

    /// Cancel an in-flight generation. `None` cancels every scope.
    pub fn cancel_current(&self, scope: Option<&str>) {
        let map = self.cancel.lock().unwrap();
        match scope {
            Some(s) => {
                if let Some(t) = map.get(s) {
                    t.cancel();
                }
            }
            None => map.values().for_each(|t| t.cancel()),
        }
    }

    /// Fold a chat's throughput into the running per-model stats and persist.
    pub fn record_chat_stats(&self, model: &str, stats: Option<GenStats>) {
        let Some(s) = stats else { return };
        let tps = s.tokens_per_sec();
        if tps <= 0.0 {
            return;
        }
        let mut map = self.model_stats.lock().unwrap();
        let entry = map.entry(model.to_string()).or_default();
        entry.samples += 1;
        entry.total_tokens += s.eval_count;
        entry.total_seconds += s.eval_duration_ns as f64 / 1e9;
        entry.last_tps = tps;
        if let Ok(json) = serde_json::to_string_pretty(&*map) {
            let _ = std::fs::write(&self.stats_path, json);
        }
    }

    /// Fold one answer's time-to-first-token into the running stats and
    /// leave a timing trace line. `path` names which chat surface produced
    /// it.
    ///
    /// Only surfaces whose wait reflects how fast the MODEL answers feed the
    /// per-model ranking. Deep research is traced but not ranked: its time is
    /// dominated by up to MAX_STEPS planner round trips before a single
    /// answer token, so averaging it in would rank the mode rather than the
    /// model, and one research turn would bury a model's real chat latency.
    ///
    /// A clock that never saw a token records nothing: a failed or stopped
    /// turn has no first token to time.
    pub fn record_ttft(
        &self,
        model: &str,
        path: &str,
        notebook_id: &str,
        clock: &TtftClock,
        phases: Option<serde_json::Value>,
    ) {
        let ttft_ms = clock.ttft_ms();
        if ttft_ms == 0 {
            return;
        }
        // Traced always; ranked only where the number means "model speed".
        if RANKED_TTFT_PATHS.contains(&path) {
            let mut map = self.model_stats.lock().unwrap();
            let entry = map.entry(model.to_string()).or_default();
            entry.ttft_samples += 1;
            entry.total_ttft_ms += ttft_ms;
            entry.last_ttft_ms = ttft_ms;
            if let Ok(json) = serde_json::to_string_pretty(&*map) {
                let _ = std::fs::write(&self.stats_path, json);
            }
        }
        let mut record = serde_json::json!({
            "ts": now(),
            "surface": "chat-timing",
            "path": path,
            "notebookId": notebook_id,
            "model": model,
            "ttftMs": ttft_ms,
        });
        // Whatever split the surface can account for, merged in as-is.
        if let Some(serde_json::Value::Object(extra)) = phases {
            for (k, v) in extra {
                record[k] = v;
            }
        }
        crate::trace::log(&self.trace_dir, record);
    }

    pub fn model_stats_snapshot(&self) -> Vec<ModelStat> {
        let map = self.model_stats.lock().unwrap();
        map.iter()
            .map(|(name, a)| ModelStat {
                name: name.clone(),
                last_tokens_per_sec: a.last_tps,
                avg_tokens_per_sec: if a.total_seconds > 0.0 {
                    a.total_tokens as f64 / a.total_seconds
                } else {
                    0.0
                },
                samples: a.samples,
                ttft_samples: a.ttft_samples,
                last_ttft_ms: a.last_ttft_ms,
                avg_ttft_ms: if a.ttft_samples > 0 {
                    a.total_ttft_ms as f64 / a.ttft_samples as f64
                } else {
                    0.0
                },
            })
            .collect()
    }
}

/// Build the Ai runtime: app data dir + embedder download progress events
/// (`embedder://progress` with {label, done, total}).
/// Locate the alchemy-fm sidecar: bundled resource first (release), then
/// the in-repo Swift build (dev). None disables the Foundation Models rung.
fn find_fm_sidecar(app: &AppHandle) -> Option<std::path::PathBuf> {
    use tauri::path::BaseDirectory;
    use tauri::Manager;
    if let Ok(p) = app
        .path()
        .resolve("binaries/alchemy-fm", BaseDirectory::Resource)
    {
        if p.exists() {
            return Some(p);
        }
    }
    if cfg!(debug_assertions) {
        let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../sidecar/alchemy-fm/.build/release/alchemy-fm");
        if dev.exists() {
            return Some(dev);
        }
    }
    None
}

/// Agent-CLI availability for the provider tiles (claude, codex): probed
/// off the main thread — discovery may fall through to a login-shell which.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentCliStatus {
    pub id: String,
    pub installed: bool,
    pub detail: String,
}

#[tauri::command]
pub async fn agent_cli_status() -> Result<Vec<AgentCliStatus>, String> {
    tokio::task::spawn_blocking(|| {
        crate::inference::AgentKind::ALL
            .into_iter()
            .map(|kind| {
                let (installed, detail) = crate::inference::agent_status(kind);
                AgentCliStatus {
                    id: kind.id().to_string(),
                    installed,
                    detail: if installed {
                        format!("{} · {}", kind.label(), detail)
                    } else {
                        detail
                    },
                }
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

/// Live readiness for every configured provider row (the ready-list chips):
/// fm probes the sidecar, ollama pings its server, gateways report keyed
/// state, agent CLIs report install/version.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReadiness {
    pub id: String,
    pub ready: bool,
    pub detail: String,
}

/// Compute one provider row's live readiness. Shared by the batch
/// `provider_readiness` (ChatPanel's model pill) and the per-provider
/// `provider_readiness_one` (Settings → Models probes each row on its own so a
/// slow or unreachable provider never blocks a healthy one).
async fn readiness_for_entry(
    app: &AppHandle,
    entry: &crate::ai::ProviderEntry,
    config: &AiConfig,
) -> Result<(bool, String), String> {
    Ok(match entry.kind.as_str() {
        "fm" => match find_fm_sidecar(app) {
            Some(bin) => {
                let fm = crate::inference::FmEngine::new(bin);
                if fm.available().await {
                    (true, "Apple on-device · private, no setup".to_string())
                } else {
                    let detail = fm.probe_detail().await;
                    if detail.contains("modelNotReady") {
                        (
                            false,
                            "downloading — macOS is fetching the on-device model".to_string(),
                        )
                    } else {
                        (false, "needs macOS 26+ with Apple Intelligence".to_string())
                    }
                }
            }
            None => (false, "not available in this build".to_string()),
        },
        "gateway" => {
            if entry.api_key.is_empty() {
                (false, "no key yet".to_string())
            } else {
                let model = if entry.chat_model.is_empty() {
                    "model picked on first use".to_string()
                } else {
                    entry.chat_model.clone()
                };
                (true, format!("{model} · your key"))
            }
        }
        "ollama" => {
            let mut cfg = crate::inference::OllamaConfig {
                base_url: config.base_url.clone(),
                chat_model: config.chat_model.clone(),
                embed_model: config.embed_model.clone(),
                vision_model: config.vision_model.clone(),
                effort: String::new(),
            };
            if !entry.base_url.trim().is_empty() {
                cfg.base_url = entry.base_url.clone();
            }
            let model = if entry.chat_model.trim().is_empty() {
                cfg.chat_model.clone()
            } else {
                entry.chat_model.clone()
            };
            let ping = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                crate::inference::Ollama::new(cfg).list_models(),
            )
            .await;
            match ping {
                Ok(Ok(_)) => (true, format!("{model} · running")),
                _ => (false, "server not running".to_string()),
            }
        }
        kind => match crate::inference::AgentKind::from_id(kind) {
            Some(agent) => {
                let (installed, detail) =
                    tokio::task::spawn_blocking(move || crate::inference::agent_status(agent))
                        .await
                        .map_err(|e| e.to_string())?;
                if installed {
                    (true, format!("your subscription · {detail}"))
                } else {
                    (false, detail)
                }
            }
            None => (false, "unknown provider".to_string()),
        },
    })
}

#[tauri::command]
pub async fn provider_readiness(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<ProviderReadiness>, String> {
    let config = { state.ai.read().await.config().clone() };
    let mut out = Vec::new();
    for entry in &config.providers {
        let (ready, detail) = readiness_for_entry(&app, entry, &config).await?;
        out.push(ProviderReadiness {
            id: entry.id.clone(),
            ready,
            detail,
        });
    }
    Ok(out)
}

/// One provider's readiness, looked up by id. Settings → Models fires one of
/// these per row so each renders the instant its own probe resolves — a hung
/// ollama server or a slow agent-CLI `which` no longer gates every other row.
#[tauri::command]
pub async fn provider_readiness_one(
    app: AppHandle,
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<ProviderReadiness, String> {
    let config = { state.ai.read().await.config().clone() };
    let entry = config
        .provider_by_id(&provider_id)
        .ok_or_else(|| "unknown provider".to_string())?;
    let (ready, detail) = readiness_for_entry(&app, entry, &config).await?;
    Ok(ProviderReadiness {
        id: entry.id.clone(),
        ready,
        detail,
    })
}

pub fn ai_runtime(app: AppHandle, data_dir: std::path::PathBuf) -> crate::ai::AiRuntime {
    let fm_sidecar = find_fm_sidecar(&app);
    #[derive(serde::Serialize, Clone)]
    struct EmbedderProgressEvent {
        label: String,
        done: u64,
        total: u64,
    }
    let progress: crate::ai::EmbedderProgress = std::sync::Arc::new(move |label, done, total| {
        let _ = app.emit(
            "embedder://progress",
            EmbedderProgressEvent {
                label: label.to_string(),
                done,
                total,
            },
        );
    });
    crate::ai::AiRuntime {
        data_dir,
        embedder_progress: Some(progress),
        fm_sidecar,
    }
}

/// Retry support: drop a message row (the failed answer, then its question)
/// so the resend owns a clean slot in the transcript.
#[tauri::command]
pub async fn delete_message(state: State<'_, AppState>, message_id: String) -> Result<(), String> {
    e(state.db.delete_message(&message_id).await)
}

/// Which notebook should an incoming source be filed in? Backs the
/// "Add to which notebook?" picker's suggestion and the MCP tool of the same
/// name. Pass whatever is on hand — pasted text, a file path, or just a URL.
///
/// A bare URL is fetched and extracted first: the domain alone is thin signal
/// ("substack.com" says nothing), and the picker is worth the second or two.
/// A fetch failure is not fatal — the URL string still routes, just worse.
#[tauri::command]
pub async fn suggest_notebook(
    state: State<'_, AppState>,
    title: String,
    text: String,
    url: String,
) -> Result<crate::router::NotebookSuggestion, String> {
    let (title, text) = if text.trim().is_empty() && !url.trim().is_empty() {
        match ingest::extract_url(&url).await {
            Ok(ex) => (
                if title.trim().is_empty() {
                    ex.title
                } else {
                    title
                },
                ex.text,
            ),
            Err(_) => (if title.is_empty() { url.clone() } else { title }, url),
        }
    } else {
        (title, text)
    };
    // Snapshot the Ai under a momentary read guard — never held across the
    // awaits below.
    let ai = state.ai.read().await.clone();
    e(crate::router::suggest_notebook(&state.db, &ai, &title, &text).await)
}

/// How many pages a PDF has, for the reader's page view. Zero when the file
/// is unreadable — the view falls back to text rather than erroring.
#[tauri::command]
pub fn pdf_page_count(path: String) -> usize {
    crate::pdf::page_count(&path)
}

/// One rendered PDF page as a `data:` URL, 1-indexed. The reader asks for
/// pages as they scroll into view, so a long document costs only the pages
/// actually looked at.
/// Where a PDF's bytes live on disk for the reader's page view.
///
/// A PDF added from a file is already local. A PDF added from a URL is not:
/// ingest downloads it, extracts the text, and throws the bytes away — so
/// page view, which needs to rasterize real pages, had nothing to open and
/// was simply hidden for URL sources. That excluded exactly the case v0.32.0
/// taught Alchemy to import properly (arxiv.org/pdf/...).
///
/// So: resolve to a local path, downloading into a per-source cache the first
/// time. Cached rather than re-fetched per page because a 40-page paper would
/// otherwise be 40 downloads, and because the point of a local-first app is
/// that the second look works on a plane.
#[tauri::command]
pub async fn pdf_local_path(
    state: State<'_, AppState>,
    source_id: String,
) -> Result<String, String> {
    let Some(source) = e(state.db.get_source(&source_id).await)? else {
        return Err("source not found".into());
    };
    if source.url.is_empty() {
        return Err("this PDF has no file behind it".into());
    }
    if !source.url.starts_with("http://") && !source.url.starts_with("https://") {
        return Ok(source.url);
    }

    let cached = pdf_cache_path(&state, &source_id);
    if cached.is_file() {
        return Ok(cached.to_string_lossy().into_owned());
    }

    let client = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|err| err.to_string())?;
    let bytes = client
        .get(&source.url)
        .send()
        .await
        .map_err(|err| format!("could not fetch {}: {err}", source.url))?
        .bytes()
        .await
        .map_err(|err| format!("could not read {}: {err}", source.url))?;
    if !crate::pdf::looks_like_pdf(&bytes) {
        return Err(format!("{} no longer serves a PDF", source.url));
    }
    if let Some(dir) = cached.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    std::fs::write(&cached, &bytes).map_err(|err| err.to_string())?;
    Ok(cached.to_string_lossy().into_owned())
}

/// Downloaded PDF bytes for a URL-backed source, kept beside the og-image
/// thumbs. Keyed by source id so deleting the source can drop it.
fn pdf_cache_path(state: &AppState, source_id: &str) -> std::path::PathBuf {
    app_data_dir(state)
        .join("pdfs")
        .join(format!("{source_id}.pdf"))
}

#[tauri::command]
pub fn pdf_page_image(path: String, page: usize, width: u32) -> Result<String, String> {
    use base64::Engine;
    // Clamped: `width` arrives from the frontend's element measurement, and a
    // rogue value would ask PDFium for a multi-gigabyte bitmap.
    let width = width.clamp(200, 3000) as i32;
    let png = e(crate::pdf::render_page(&path, page, width))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    Ok(format!("data:image/png;base64,{b64}"))
}

/// Launch Terminal.app running one of the known fix commands (the "Fix:"
/// hints on error rows): agent sign-ins plus the Ollama fixes. Strictly
/// allowlisted (`terminal_command_allowed`): the command string travels
/// through model-adjacent error text, so nothing outside that set may ever
/// reach a shell.
#[tauri::command]
pub fn open_in_terminal(command: String) -> Result<(), String> {
    if !terminal_command_allowed(&command) {
        return Err("unsupported command".into());
    }
    let script =
        format!("tell application \"Terminal\"\nactivate\ndo script \"{command}\"\nend tell");
    std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Validate a Notion integration token against the API (Settings field's live
/// check). Returns the workspace/bot label on success; a human error string
/// on failure. Standalone — no app state needed.
#[tauri::command]
pub async fn notion_check(token: String) -> Result<String, String> {
    if token.trim().is_empty() {
        return Err("Paste a token first".into());
    }
    crate::notion::NotionClient::new(&token)
        .check_token()
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Message-footer attribution: which provider answered, with metered cost
/// when the engine reported one ("Claude Code · $0.04").
fn model_caption(model: &str, cost_usd: Option<f64>) -> String {
    match cost_usd {
        Some(c) if c > 0.0 => format!("{model} · ${c:.2}"),
        _ => model.to_string(),
    }
}

pub(crate) fn now() -> i64 {
    Utc::now().timestamp_millis()
}

pub(crate) fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// Map any error into a string so it crosses the IPC boundary cleanly.
fn e<T>(r: anyhow::Result<T>) -> Result<T, String> {
    r.map_err(|err| friendly_error(&format!("{err:#}")))
}

/// Errors cross IPC as strings the user actually reads — translate known
/// machine noise into something actionable and strip code locations.
pub(crate) fn friendly_error(raw: &str) -> String {
    // Schema skew on the shared store: a newer Alchemy (usually the dev
    // build) migrated a table this binary doesn't know yet. The raw Lance
    // dump ("Append with different schema … location: …/schema.rs:186")
    // is pure noise; say what to do instead.
    if raw.contains("Append with different schema") {
        return "Couldn't save: the notebook database was upgraded by a newer version \
                of Alchemy than this one. Update Alchemy to its latest version (or add \
                from the newer copy) and try again."
            .into();
    }
    if let Some(msg) = classify_model_error(raw) {
        return msg;
    }
    // Drop `, location: /path/to/file.rs:12:3` fragments and collapse the
    // duplicate sentence Lance nests inside its own context chain.
    let mut out = String::with_capacity(raw.len());
    for (i, piece) in raw.split(", location: ").enumerate() {
        if i == 0 {
            out.push_str(piece);
            continue;
        }
        // Skip the leading path token; keep anything after it.
        if let Some(rest) = piece.split_once(' ') {
            out.push(' ');
            out.push_str(rest.1);
        }
    }
    let out = out.trim().trim_end_matches(':').to_string();
    if let Some((head, tail)) = out.split_once(": ") {
        if tail.starts_with(head) {
            return tail.to_string();
        }
    }
    out
}

/// Deterministic first pass over provider/model failures (RFC-self-resolve
/// phase 1): recognize the shapes users actually hit and answer with the fix
/// instead of the transport noise. Two grammars in the output are load-bearing
/// because the frontend turns them into buttons: `` Fix: open Terminal, run
/// `cmd`, then retry here. `` becomes a Terminal launch (allowlisted in
/// `open_in_terminal`), and the literal phrase "Settings → Models" becomes a
/// jump to that Settings tab (chat error rows and error toasts both).
pub(crate) fn classify_model_error(raw: &str) -> Option<String> {
    // Already translated upstream — the gateway's status advice and the agent
    // CLIs' sign-in/model hints are more specific than anything matched here.
    if raw.contains("Fix:") || raw.contains("Settings → Models") {
        return None;
    }
    let lower = raw.to_lowercase();

    // Ollama 404 body: {"error":"model \"x\" not found, try pulling it first"}
    // — the model name is either a typo or simply not pulled yet.
    if lower.contains("not found, try pulling it first") {
        return Some(match ollama_missing_model(raw) {
            Some(m) => format!(
                "The model “{m}” isn't downloaded in Ollama. Fix: open Terminal, \
                 run `ollama pull {m}`, then retry here. (If the name looks wrong, \
                 pick an installed model in Settings → Models.)"
            ),
            None => "That model isn't downloaded in Ollama — pull it in Terminal \
                     (`ollama pull <model>`) or pick an installed model in \
                     Settings → Models."
                .into(),
        });
    }

    // Nothing listening on the Ollama port: the daemon isn't running.
    if lower.contains("connection refused") && (lower.contains("ollama") || lower.contains("11434"))
    {
        return Some(
            "Ollama isn't running — nothing answered at its address. Fix: open \
             Terminal, run `ollama serve` (or launch the Ollama app), then retry \
             here. Or switch to another provider in Settings → Models."
                .into(),
        );
    }

    // A model-shaped timeout: still loading into memory, or holding the GPU
    // for another job. Scoped to provider-ish errors so a slow source fetch
    // during import never gets model advice.
    if lower.contains("operation timed out")
        && (lower.contains("ollama")
            || lower.contains("model")
            || lower.contains("provider")
            || lower.contains("gateway"))
    {
        return Some(
            "The model took too long to answer — it may still be loading into \
             memory, or busy with another generation. Wait a moment and retry; \
             if it keeps happening, pick a smaller model in Settings → Models."
                .into(),
        );
    }

    // Key trouble outside the gateway's own status translation.
    if lower.contains("invalid api key")
        || lower.contains("incorrect api key")
        || lower.contains("invalid_api_key")
        || lower.contains("401 unauthorized")
    {
        return Some("This provider rejected the API key — check it in Settings → Models.".into());
    }

    None
}

/// Model name out of an Ollama "not found, try pulling it first" body. The
/// name travels into a `Fix: … run `ollama pull X`` hint that a button can
/// execute, so it is charset-validated here as well as at execution time —
/// error text must never smuggle shell.
fn ollama_missing_model(raw: &str) -> Option<String> {
    let head = &raw[..raw.find(" not found")?];
    let start = head.rfind("model ")? + "model ".len();
    let name = head[start..]
        .trim_matches(|c: char| matches!(c, '\\' | '"' | '\'' | '“' | '”'))
        .to_string();
    is_safe_model_name(&name).then_some(name)
}

/// The character set a model name may use to reach a terminal command.
pub(crate) fn is_safe_model_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/-".contains(c))
}

/// The full allowlist behind `open_in_terminal`: the fixed agent sign-in
/// commands, plus the Ollama fixes the error classifier can name. Pure and
/// separate from the command so tests can cover it without spawning Terminal.
pub(crate) fn terminal_command_allowed(command: &str) -> bool {
    const ALLOWED: [&str; 11] = [
        "claude",
        "codex login",
        "gemini",
        "cursor-agent login",
        "opencode auth login",
        "copilot",
        "hermes",
        "bob",
        "prime-agent",
        "pi",
        "ollama serve",
    ];
    if ALLOWED.contains(&command) {
        return true;
    }
    // `ollama pull <model>`: the name arrives via error text, so only the
    // strict model-name charset may pass — nothing that could escape the
    // AppleScript string or the shell.
    command
        .strip_prefix("ollama pull ")
        .is_some_and(is_safe_model_name)
}

// Keep this palette in sync with the Rust DB schema helper constant in
// `src-tauri/src/db.rs` and the frontend palette in HomeView.
fn is_valid_hex_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color
            .as_bytes()
            .get(1..)
            .is_some_and(|hex| hex.iter().all(|b| (*b as char).is_ascii_hexdigit()))
}

// ---- Notebooks -----------------------------------------------------------

#[tauri::command]
pub async fn list_notebooks(state: State<'_, AppState>) -> Result<Vec<Notebook>, String> {
    e(state.db.list_notebooks().await)
}

#[tauri::command]
pub async fn create_notebook(
    state: State<'_, AppState>,
    title: String,
) -> Result<Notebook, String> {
    let ts = now();
    let count = e(state.db.list_notebooks().await)?;
    let color = NOTEBOOK_PALETTE[count.len() % NOTEBOOK_PALETTE.len()];
    let title = if title.trim().is_empty() {
        "Untitled notebook".into()
    } else {
        title.trim().to_string()
    };
    let nb = Notebook {
        id: new_id(),
        icon: auto_notebook_icon(&title),
        title,
        created_at: ts,
        updated_at: ts,
        color: color.to_string(),
        status: String::new(),
        source_count: 0,
        note_count: 0,
        report_count: 0,
    };
    e(state.db.create_notebook(&nb).await)?;
    Ok(nb)
}

/// Pick a relevant icon for a fresh notebook from its title — instant and
/// deterministic (keyword table, no model call). "" means "no strong signal";
/// the frontend renders its default book for that. Names are lucide icon ids
/// and must exist in the frontend's `NOTEBOOK_ICONS` map.
pub(crate) fn auto_notebook_icon(title: &str) -> String {
    const TABLE: &[(&str, &[&str])] = &[
        ("plane", &["travel", "trip", "flight", "vacation", "abroad"]),
        (
            "briefcase",
            &["work", "job", "career", "interview", "hiring"],
        ),
        (
            "dollar-sign",
            &[
                "finance", "money", "budget", "invest", "tax", "stock", "crypto",
            ],
        ),
        (
            "home",
            &[
                "house",
                "home",
                "apartment",
                "mortgage",
                "real estate",
                "renovation",
            ],
        ),
        (
            "heart",
            &["health", "medical", "doctor", "therapy", "wellness"],
        ),
        (
            "dumbbell",
            &["gym", "fitness", "workout", "training", "running"],
        ),
        (
            "utensils",
            &["food", "recipe", "cooking", "restaurant", "meal", "diet"],
        ),
        (
            "music",
            &["music", "song", "album", "band", "guitar", "piano"],
        ),
        ("film", &["movie", "film", "tv", "show", "cinema"]),
        ("gamepad-2", &["game", "gaming"]),
        (
            "graduation-cap",
            &[
                "school", "course", "study", "class", "learning", "college", "exam",
            ],
        ),
        (
            "flask-conical",
            &[
                "science",
                "research",
                "experiment",
                "lab",
                "chemistry",
                "physics",
            ],
        ),
        (
            "code",
            &[
                "code",
                "coding",
                "software",
                "programming",
                "rust",
                "python",
                "javascript",
                "app",
            ],
        ),
        ("car", &["car", "auto", "vehicle", "motorcycle"]),
        (
            "trees",
            &[
                "garden", "plant", "nature", "outdoor", "camping", "hike", "hiking",
            ],
        ),
        ("baby", &["baby", "kids", "child", "parenting"]),
        ("dog", &["dog", "puppy", "pet"]),
        ("cat", &["cat", "kitten"]),
        (
            "wrench",
            &["diy", "repair", "build", "maintenance", "tools"],
        ),
        ("scale", &["legal", "law", "contract", "court"]),
        ("landmark", &["history", "politics", "government", "civic"]),
        (
            "globe",
            &[
                "world",
                "geography",
                "language",
                "spanish",
                "french",
                "japanese",
            ],
        ),
        ("palette", &["art", "design", "drawing", "painting"]),
        ("newspaper", &["news", "press", "media"]),
        ("rocket", &["startup", "launch", "space"]),
        (
            "calendar",
            &["plan", "planning", "schedule", "event", "wedding"],
        ),
        ("book", &["reading", "book", "novel", "literature"]),
        ("map", &["map", "places", "city", "neighborhood"]),
        ("shopping-cart", &["shopping", "gift", "wishlist"]),
        ("users", &["family", "team", "people", "friends"]),
        ("sailboat", &["boat", "sailing", "sail", "yacht", "marina"]),
        ("bug", &["bug", "debug", "insect"]),
        (
            "drama",
            &["theater", "theatre", "drama", "acting", "improv"],
        ),
        (
            "shirt",
            &["clothes", "clothing", "wardrobe", "fashion", "laundry"],
        ),
        ("command", &["mac", "macos", "apple"]),
        ("monitor", &["computer", "pc", "desktop", "hardware"]),
        ("flame", &["fire", "grill", "bbq", "barbecue", "smoker"]),
        ("wand-sparkles", &["magic", "wizard", "fantasy", "spell"]),
        ("clock", &["time", "clock", "hours", "timeline"]),
    ];
    let t = title.to_lowercase();
    // Whole-word matching, not substring — "autopick" must not hit "auto",
    // "carpet" must not hit "car". Plurals and long-keyword extensions
    // ("trips" → "trip", "camping trips" → "camping") still land.
    let words: Vec<&str> = t
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let hit = |kw: &str| {
        if kw.contains(' ') {
            return t.contains(kw); // multi-word keywords match as phrases
        }
        words.iter().any(|w| {
            *w == kw || w.strip_suffix('s') == Some(kw) || (kw.len() >= 5 && w.starts_with(kw))
        })
    };
    for (icon, kws) in TABLE {
        if kws.iter().any(|kw| hit(kw)) {
            return (*icon).to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod icon_tests {
    #[test]
    fn auto_icon_matches_whole_words() {
        use super::auto_notebook_icon as pick;
        assert_eq!(pick("Camping Trips 2027"), "plane"); // "trips" → trip
        assert_eq!(pick("Camping Gear"), "trees");
        assert_eq!(pick("Icon autopick test"), ""); // "autopick" must NOT hit "auto"
        assert_eq!(pick("Carpet samples"), ""); // "carpet" must NOT hit "car"
        assert_eq!(pick("Car maintenance"), "car");
        assert_eq!(pick("Household Budgeting"), "dollar-sign"); // budget-
        assert_eq!(pick("Rust projects"), "code");
        assert_eq!(pick("Real estate leads"), "home");
        assert_eq!(pick("Boat 2027"), "sailboat");
        assert_eq!(pick("Debugging the parser"), "bug"); // debug- prefix
        assert_eq!(pick("Mac setup"), "command");
        assert_eq!(pick("Fire pit ideas"), "flame");
        assert_eq!(pick("Untitled notebook"), "");
    }
}

/// Set (or clear — "") the notebook's icon. Names are constrained to a slug
/// shape rather than an allowlist so the frontend's curated set can grow
/// without a lockstep backend change.
#[tauri::command]
pub async fn set_notebook_icon(
    state: State<'_, AppState>,
    id: String,
    icon: String,
) -> Result<(), String> {
    let icon = icon.trim();
    if icon.len() > 40
        || !icon
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("icon must be a lucide icon slug".into());
    }
    e(state.db.set_notebook_icon(&id, icon).await)
}

#[tauri::command]
pub async fn set_notebook_color(
    state: State<'_, AppState>,
    id: String,
    color: String,
) -> Result<(), String> {
    let color = color.trim();
    if !is_valid_hex_color(color) {
        return Err("color must be in hex form (#rrggbb)".into());
    }
    e(state.db.set_notebook_color(&id, color).await)
}

#[tauri::command]
pub async fn rename_notebook(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> Result<(), String> {
    e(state.db.rename_notebook(&id, title.trim(), now()).await)
}

#[tauri::command]
pub async fn delete_notebook(state: State<'_, AppState>, id: String) -> Result<(), String> {
    e(state.db.delete_notebook(&id).await)
}

/// Archive ("archived") or restore ("") a notebook. Data is untouched —
/// archived notebooks just leave the main grid.
#[tauri::command]
pub async fn set_notebook_status(
    state: State<'_, AppState>,
    id: String,
    status: String,
) -> Result<(), String> {
    if status != "archived" && !status.is_empty() {
        return Err("status must be \"archived\" or empty".into());
    }
    e(state.db.set_notebook_status(&id, &status).await)
}

// ---- Sources -------------------------------------------------------------

#[tauri::command]
pub async fn list_sources(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Vec<Source>, String> {
    e(state.db.list_sources(&notebook_id).await)
}

/// Flag URL sources whose extracted text looks like a bot wall / login / JS shell.
/// Google export endpoints return authoritative plain text (not scraped HTML),
/// so a short public doc is not a blocked page — but an interstitial ("you
/// need access") can still come through, so the marker check stays.
fn classify(source_type: &str, url: &str, text: &str) -> (String, String) {
    if source_type == "url" {
        let reason = if ingest::is_google_doc_url(url) {
            ingest::blocked_marker(text)
        } else {
            ingest::looks_blocked(text)
        };
        if let Some(reason) = reason {
            return ("error".to_string(), reason);
        }
    }
    ("ready".to_string(), String::new())
}

/// Return the title of an existing source in the notebook with identical
/// content, if any. `char_count` prefilters so only same-length candidates
/// pay for a full-content read.
async fn find_duplicate(
    state: &AppState,
    notebook_id: &str,
    text: &str,
) -> anyhow::Result<Option<String>> {
    let char_count = text.chars().count() as i64;
    for s in state.db.list_sources(notebook_id).await? {
        // Only ready and still-indexing sources count — error and
        // placeholder rows have empty content and would false-match each
        // other. "processing" matters here: dropping the same file twice in
        // quick succession must catch the second copy while the first is
        // still in the embed queue.
        if s.char_count == char_count
            && (s.status == "ready" || s.status == "processing")
            && state.db.source_content(&s.id).await? == text
        {
            return Ok(Some(s.title));
        }
    }
    Ok(None)
}

/// True when a title carries no visible characters. `trim()` alone is not
/// enough: a page `<title>` can be a zero-width space or a BOM (U+200B, U+FEFF)
/// — not whitespace, so `trim()` keeps it, and it renders as an empty row that
/// evaded every earlier blank-title guard. Visible = at least one char that is
/// not whitespace, control, or zero-width formatting.
pub(crate) fn is_blank_title(s: &str) -> bool {
    s.chars().all(|c| {
        c.is_whitespace()
            || c.is_control()
            || matches!(
                c,
                '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}'
            )
    })
}

/// A source never persists a blank title — lists would render an unlabeled
/// row (seen live: pages with no <title>). Extractors already provide file
/// stems and readability titles; this is the last-resort funnel guard,
/// falling back to the origin's host and then "Untitled source".
fn presentable_title(title: &str, url: &str) -> String {
    let t = title.trim();
    if !is_blank_title(t) {
        return t.to_string();
    }
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
        .trim_start_matches("www.");
    if host.is_empty() {
        "Untitled source".to_string()
    } else {
        host.to_string()
    }
}

pub(crate) async fn store_extracted(
    state: &AppState,
    notebook_id: &str,
    extracted: ingest::Extracted,
) -> anyhow::Result<Source> {
    if let Some(title) = find_duplicate(state, notebook_id, &extracted.text).await? {
        anyhow::bail!("Already in this notebook as \"{title}\" — skipped duplicate");
    }
    // File-backed sources record the file's mtime so the auto-refresh sweep
    // can spot on-disk changes; web/pasted sources have nothing to track.
    let mtime = if !extracted.url.is_empty() && !is_web_url(&extracted.url) {
        file_mtime(std::path::Path::new(&extracted.url))
    } else {
        0
    };
    store_new_source(state, notebook_id, extracted, "", mtime, None, true).await
}

/// Classify and persist a new source row IMMEDIATELY, then hand chunking and
/// embedding to the background stage (docs/RFC-import-pipeline.md §2) — the
/// row lands as `"processing"` and flips to `"ready"` when its chunks do.
/// `parent_id` is set for folder children (which dedup by path, not
/// content); `mtime` for any file-backed source; `code_ctx` is the
/// "repo › path" retrieval context for code chunks when the caller knows it.
async fn store_new_source(
    state: &AppState,
    notebook_id: &str,
    extracted: ingest::Extracted,
    parent_id: &str,
    mtime: i64,
    code_ctx: Option<&str>,
    embed: bool,
) -> anyhow::Result<Source> {
    let (status, error) = classify(&extracted.source_type, &extracted.url, &extracted.text);
    // Repository-tier code children store their content but skip embedding —
    // the ripgrep leg reaches them at query time (RFC-git-sources §4). They
    // land "ready" with no chunks, exactly as before.
    let processing = embed && status == "ready";
    let source = Source {
        image_url: extracted.image_url.clone(),
        author: extracted.author.clone(),
        id: new_id(),
        notebook_id: notebook_id.to_string(),
        title: presentable_title(&extracted.title, &extracted.url),
        source_type: extracted.source_type.clone(),
        url: extracted.url.clone(),
        content: extracted.text.clone(),
        char_count: extracted.text.chars().count() as i64,
        chunk_count: 0,
        created_at: now(),
        status: if processing {
            "processing".to_string()
        } else {
            status
        },
        error,
        parent_id: parent_id.to_string(),
        mtime,
        tags: String::new(),
        note: String::new(),
        fetched_at: now(),
        fetch_failures: 0,
    };
    state.db.insert_source(&source, &[], &[]).await?;
    state.db.touch_notebook(notebook_id, now()).await?;

    if processing {
        spawn_embed_stage(
            state,
            &source,
            extracted,
            code_ctx.map(str::to_string),
            false,
        )
        .await;
    } else if !source.content.is_empty() {
        // Non-embedding and errored arrivals still file under any registry
        // card that claims them, as they always did — literal string work,
        // no model call to budget. Embedded arrivals file after their
        // chunks land, inside the stage.
        registry::spawn_registry_match(
            state.db.clone(),
            notebook_id.to_string(),
            source.id.clone(),
            source.content.clone(),
        );
    }

    // Don't ship the full content back in the list payload.
    Ok(Source {
        content: String::new(),
        ..source
    })
}

/// Imports waiting on (or inside) the background embed stage. The last one
/// out flushes the FTS rebuild that everyone deferred — a folder drop does
/// one rebuild, not one per file.
static EMBED_QUEUE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Stage two of an import OR a reingest (docs/RFC-import-pipeline.md §2):
/// chunk, embed, and index in the background, then flip the row from
/// "processing" to "ready" and kick the after-import intelligence. ONE
/// worker on purpose — a folder drop queues through in arrival order, so
/// neither the embedding model nor Lance ever sees a stampede. The row is
/// already on screen; retrieval joins when the chunks land.
/// `replace_chunks` is the reingest flavor: the source's previous chunks
/// keep serving retrieval until this stage swaps them.
pub(crate) async fn spawn_embed_stage(
    state: &AppState,
    source: &Source,
    extracted: ingest::Extracted,
    code_ctx: Option<String>,
    replace_chunks: bool,
) {
    static EMBED_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let db = state.db.clone();
    let ai = { state.ai.read().await.clone() };
    let source = source.clone();
    EMBED_QUEUE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    db.defer_fts(true);
    tauri::async_runtime::spawn(async move {
        let outcome: anyhow::Result<Option<usize>> = async {
            let _permit = EMBED_GATE.acquire().await?;
            let chunks = ingest::chunk_source(&extracted, code_ctx.as_deref());
            let inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
            let embeddings = ai.embed(&inputs).await?;
            // The queue wait plus the embed can outlive the row: a delete
            // drops it, another refresh replaces it. The claim is the
            // CONTENT, not just the status — two refreshes in flight both
            // see "processing", and only the stage whose text matches the
            // row's current text may write, so the newest refresh wins.
            match db.get_source(&source.id).await? {
                Some(current)
                    if current.status == "processing" && current.content == extracted.text => {}
                _ => return Ok(None),
            }
            if replace_chunks {
                db.delete_source_chunks(&source.id).await?;
            }
            let tuples: Vec<(String, i32, String)> = chunks
                .iter()
                .enumerate()
                .map(|(i, c)| (new_id(), i as i32, c.text.clone()))
                .collect();
            db.add_chunks(&source.notebook_id, &source.id, &tuples, &embeddings)
                .await?;
            Ok(Some(tuples.len()))
        }
        .await;
        match &outcome {
            Ok(Some(n)) => {
                let _ = db
                    .finish_processing(&source.id, *n as i64, "ready", "")
                    .await;
            }
            // The row was deleted or replaced mid-stage — nothing to stamp.
            Ok(None) => {}
            Err(err) => {
                // A failed stage is an errored row with the reason, retryable
                // via Refresh — never a silent disappearance.
                let _ = db
                    .finish_processing(&source.id, 0, "error", &format!("indexing failed: {err:#}"))
                    .await;
            }
        }
        // Last one out rebuilds BM25 once for the whole batch.
        if EMBED_QUEUE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            db.defer_fts(false);
            if let Err(err) = db.flush_fts().await {
                crate::note!("embed stage: FTS flush failed: {err:#}");
            }
        }
        notify_changed("sources", Some(&source.notebook_id));
        if !matches!(outcome, Ok(Some(_))) {
            return;
        }

        // The after-import intelligence, exactly what the synchronous path
        // used to kick — now downstream of the chunks it reads. The gist
        // sweep self-gates (SWEEPING), so per-source kicks don't stack.
        crate::gist::spawn_sweep(db.clone(), ai.clone());
        // Judgment on arrival for deliberate adds only — folder children
        // skip (a bulk import judging hundreds of files against the ledger
        // would be noise; their later CHANGES still weave via reingest).
        if source.parent_id.is_empty() {
            weave::spawn_weave(
                db.clone(),
                ai,
                source.notebook_id.clone(),
                source.title.clone(),
                extracted.text.chars().take(4_000).collect(),
            );
        }
        // File the arrival under any card that claims it — folder children
        // included: a folder of scanned documents is exactly where
        // auto-filing earns its keep.
        registry::spawn_registry_match(
            db,
            source.notebook_id.clone(),
            source.id.clone(),
            extracted.text,
        );
    });
}

/// Re-arm background work stranded by a quit or crash: embed stages for
/// "processing" rows (content is stored, so the stage restarts from the row
/// itself; the one loss is a code child's "repo › path" chunk context), and
/// retitles for file sources still wearing their filename — a restart
/// mid-title used to leave them that way forever. The claim check inside
/// `spawn_retitle` keeps the retitle leg idempotent and never touches a
/// name anyone (or any model) already improved.
pub(crate) fn resume_stranded_imports(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        // Content only for the stranded few — this used to read the whole
        // corpus with content at every launch.
        if let Ok(processing) = state.db.processing_sources().await {
            for s in processing {
                let extracted = ingest::Extracted {
                    image_url: s.image_url.clone(),
                    author: s.author.clone(),
                    title: s.title.clone(),
                    source_type: s.source_type.clone(),
                    url: s.url.clone(),
                    text: s.content.clone(),
                };
                // Replace-flavored: a stranded REINGEST still has its old
                // chunks in place (an import's delete simply matches
                // nothing).
                spawn_embed_stage(&state, &s, extracted, None, true).await;
            }
        }
        let Ok(sources) = state.db.all_sources_lean().await else {
            return;
        };
        for s in sources {
            if !matches!(s.status.as_str(), "ready" | "processing")
                || s.source_type == "code"
                || s.url.is_empty()
                || is_web_url(&s.url)
                || crate::mac::is_mac_uri(&s.url)
            {
                continue;
            }
            // Still titled exactly as the file is named, and not a name a
            // person would have written — the model never got its turn.
            if s.title == ingest::file_title(&s.url) && !title_reads_human(&s.title) {
                spawn_retitle(&state, &s).await;
            }
        }
    });
}

/// Persist a URL source that failed to import so it shows with an error badge
/// and can be retried (refreshed) later.
async fn store_failed_url(
    state: &AppState,
    notebook_id: &str,
    url: &str,
    reason: String,
) -> anyhow::Result<Source> {
    let source = Source {
        image_url: String::new(),
        author: String::new(),
        id: new_id(),
        notebook_id: notebook_id.to_string(),
        title: url.to_string(),
        source_type: "url".to_string(),
        url: url.to_string(),
        content: String::new(),
        char_count: 0,
        chunk_count: 0,
        created_at: now(),
        status: "error".to_string(),
        error: reason,
        parent_id: String::new(),
        mtime: 0,
        tags: String::new(),
        note: String::new(),
        fetched_at: now(),
        fetch_failures: 0,
    };
    state.db.insert_source(&source, &[], &[]).await?;
    state.db.touch_notebook(notebook_id, now()).await?;
    Ok(source)
}

/// Image bytes ready for the vision model. Formats its decoders rarely handle
/// (HEIC/HEIF, AVIF, JPEG 2000, ICO, TIFF) are converted to PNG first via
/// macOS's built-in `sips`; everything else is sent as-is.
fn image_bytes_for_ocr(path: &str) -> anyhow::Result<Vec<u8>> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let needs_png = matches!(
        ext.as_str(),
        "heic" | "heif" | "avif" | "ico" | "jp2" | "tif" | "tiff"
    );
    if !needs_png {
        return std::fs::read(path).with_context(|| format!("failed to read {path}"));
    }
    let tmp = std::env::temp_dir().join(format!("alchemy-ocr-{}.png", new_id()));
    let status = std::process::Command::new("sips")
        .args(["-s", "format", "png"])
        .arg(path)
        .arg("-o")
        .arg(&tmp)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("failed to run sips")?;
    anyhow::ensure!(status.success(), "sips could not convert {path} to PNG");
    let bytes = std::fs::read(&tmp).context("failed to read converted PNG")?;
    let _ = std::fs::remove_file(&tmp);
    Ok(bytes)
}

/// OCR an image file into an Extracted source using the vision model.
async fn extract_image(state: &AppState, path: &str) -> anyhow::Result<ingest::Extracted> {
    use base64::Engine;
    let bytes = image_bytes_for_ocr(path)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let text = {
        let ai = state.ai.read().await.clone();
        ai.ocr(&b64).await?
    };
    if text.trim().is_empty() {
        anyhow::bail!("no text found in image {path}");
    }
    Ok(ingest::Extracted {
        image_url: String::new(),
        author: String::new(),
        title: ingest::file_title(path),
        source_type: "image".to_string(),
        url: String::new(),
        text,
    })
}

/// OCR a scanned/image-only PDF by rasterizing each page and transcribing it.
async fn extract_pdf_ocr(state: &AppState, path: &str) -> anyhow::Result<ingest::Extracted> {
    use base64::Engine;
    const MAX_PAGES: usize = 30;
    let pages = crate::pdf::render_pdf_pages(path, MAX_PAGES, 1600)?;
    if pages.is_empty() {
        anyhow::bail!("no pages to OCR in {path}");
    }
    let mut text = String::new();
    for (i, png) in pages.iter().enumerate() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        let page_text = {
            let ai = state.ai.read().await.clone();
            ai.ocr(&b64).await?
        };
        let page_text = page_text.trim();
        if !page_text.is_empty() {
            text.push_str(&format!("## Page {}\n{}\n\n", i + 1, page_text));
        }
    }
    if text.trim().is_empty() {
        anyhow::bail!("OCR produced no text from {path}");
    }
    Ok(ingest::Extracted {
        image_url: String::new(),
        author: String::new(),
        title: ingest::file_title(path),
        source_type: "pdf".to_string(),
        url: String::new(),
        text,
    })
}

/// Filenames, slugs, and arXiv-style IDs make poor display titles. The cheap
/// legs run inline: code files and human-looking names keep themselves,
/// markdown takes its first heading. Returns true when the title is settled;
/// false means only a model could do better — the caller queues
/// `spawn_retitle` AFTER the source lands, because an import must never wait
/// on a model (a cold Ollama used to turn "add a file" into a long spinner
/// with nothing on screen).
pub(crate) fn friendly_title_fast(extracted: &mut ingest::Extracted) -> bool {
    // Code files are their own best titles (db.rs IS the name) — and a repo
    // add would otherwise fire one model call per file.
    if extracted.source_type == "code" {
        return true;
    }
    // A title containing spaces is usually already human-written.
    if title_reads_human(&extracted.title) {
        return true;
    }
    if extracted.source_type == "markdown" {
        let heading = extracted
            .text
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(str::trim)
            .filter(|l| l.starts_with('#'))
            .map(|l| l.trim_start_matches('#').trim().to_string());
        if let Some(h) = heading.filter(|h| !h.is_empty()) {
            extracted.title = h.chars().take(80).collect();
            return true;
        }
    }
    false
}

/// Whether a filename-derived title already reads as human-written. A copy
/// suffix — "PortfolioDownload (1)" — is the browser's, not the author's,
/// so its space doesn't count as the human touch a space usually signals.
fn title_reads_human(title: &str) -> bool {
    let stem = title
        .strip_suffix(')')
        .and_then(|t| t.rsplit_once(" ("))
        .filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .map(|(stem, _)| stem)
        .unwrap_or(title);
    stem.contains(char::is_whitespace)
}

#[cfg(test)]
mod title_tests {
    use super::title_reads_human;

    #[test]
    fn a_copy_suffix_is_not_the_human_touch() {
        // Observed live: "PortfolioDownload (1)" kept its filename forever
        // because the copy suffix's space read as a human-written title.
        assert!(!title_reads_human("PortfolioDownload"));
        assert!(!title_reads_human("PortfolioDownload (1)"));
        assert!(!title_reads_human("scan-2026-08 (12)"));
        assert!(title_reads_human("Quarterly Report"));
        assert!(title_reads_human("Quarterly Report (2)"));
        // A bare "(3)" or non-numeric parenthetical stays as-is.
        assert!(!title_reads_human("(3)"));
        assert!(title_reads_human("Notes (draft)"));
    }
}

/// The model leg of titling, in the background: ask Small for a short title
/// and rename the stored source when it answers. Best-effort — any failure
/// keeps the filename, titling must never break an import. Bounded so a
/// folder of fifty files trickles through the model instead of stampeding
/// it, and the rename is skipped if the title moved meanwhile (a refresh
/// raced us).
pub(crate) async fn spawn_retitle(state: &AppState, source: &Source) {
    static RETITLE_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    // "processing" rows count: imports now land before their embed stage
    // (RFC-import-pipeline §2), and the retitle runs alongside it. Only rows
    // with nothing worth titling — errors, placeholders — are skipped.
    if !matches!(source.status.as_str(), "ready" | "processing") {
        return;
    }
    let db = state.db.clone();
    // Momentary read guard, snapshot out — never hold the Ai lock across
    // the call itself.
    let ai = { state.ai.read().await.clone() };
    let id = source.id.clone();
    let placed_title = source.title.clone();
    let notebook_id = source.notebook_id.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(_permit) = RETITLE_GATE.acquire().await else {
            return;
        };
        let Ok(text) = db.source_content(&id).await else {
            return;
        };
        let excerpt: String = text.chars().take(1500).collect();
        let messages = vec![
            crate::ai::ChatTurn::system(
                "You title documents. Reply with ONLY a short descriptive title (3-8 words) for \
                 the document excerpt — no quotes, no trailing punctuation, nothing else.",
            ),
            crate::ai::ChatTurn::user(format!(
                "Filename: {placed_title}\n\nExcerpt:\n{excerpt}\n\nTitle:"
            )),
        ];
        let Ok(out) = ai.chat_role(crate::inference::Role::Small, &messages).await else {
            return;
        };
        let t = out
            .text
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .trim_matches(['"', '“', '”', '*', '#'])
            .trim()
            .to_string();
        if t.is_empty() || t.chars().count() > 100 || t == placed_title {
            return;
        }
        // A refresh may have replaced the row (or its title) while the model
        // thought — the filename title is the claim check.
        match db.get_source(&id).await {
            Ok(Some(s)) if s.title == placed_title => {}
            _ => return,
        }
        if db.set_source_title(&id, &t).await.is_ok() {
            notify_changed("sources", Some(&notebook_id));
        }
    });
}

/// Extract a local file through the full pipeline (Google placeholder fetch,
/// image OCR, scanned-PDF OCR fallback, plain extraction). File-backed results
/// record the originating path in `url` so the source can be refreshed from
/// disk later; Google placeholders keep their cloud URL instead.
pub(crate) async fn extract_any_file(
    state: &AppState,
    path: &str,
) -> anyhow::Result<ingest::Extracted> {
    let mut extracted = if let Some(url) = ingest::google_placeholder_url(path) {
        // Google Drive desktop placeholder — the content lives in the cloud;
        // fetch it through the same export path as a pasted docs.google.com URL.
        ingest::extract_url(&url).await?
    } else if let Some(url) = ingest::dropbox_paper_url(path) {
        // Dropbox Paper stub that carries a link to the online doc — fetch it
        // as a web page, the same way a .gdoc placeholder resolves.
        ingest::extract_url(&url).await?
    } else if ingest::is_image(path) {
        extract_image(state, path).await?
    } else if ingest::is_pdf(path) {
        // Try fast text extraction; fall back to per-page OCR for scanned PDFs.
        match ingest::extract_file(path) {
            Ok(ex) => ex,
            Err(text_err) => extract_pdf_ocr(state, path).await.map_err(|ocr_err| {
                anyhow::anyhow!(
                    "{text_err} OCR failed: {ocr_err}. A vision model in Settings → Models reads scanned PDFs."
                )
            })?,
        }
    } else {
        ingest::extract_file(path)?
    };
    if extracted.url.is_empty() {
        extracted.url = path.to_string();
    }
    Ok(extracted)
}

#[tauri::command]
pub async fn add_source_file(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: String,
    path: String,
) -> Result<Source, String> {
    // A dropped directory becomes a folder source (drag-and-drop parity with
    // the "Add folder" menu item).
    if std::path::Path::new(&path).is_dir() {
        return add_source_folder(app, state, notebook_id, path).await;
    }
    let mut extracted = e(extract_any_file(&state, &path).await)?;
    let settled = friendly_title_fast(&mut extracted);
    let src = e(store_extracted(&state, &notebook_id, extracted).await)?;
    if !settled {
        spawn_retitle(&state, &src).await;
    }
    Ok(src)
}

/// Live Spotlight search over the user's Mac, backing the Add Source →
/// "Search your Mac" step. Returns ranked file/folder hits; the rows route
/// back through `add_source_file`, so folders and OKF bundles behave exactly
/// as they do from a file drop. Empty query = empty results (no subprocess).
/// See `filesearch.rs`.
#[tauri::command]
pub async fn search_mac_files(
    query: String,
    limit: Option<usize>,
) -> Result<Vec<crate::filesearch::FileHit>, String> {
    Ok(crate::filesearch::search(&query, limit.unwrap_or(30)).await)
}

#[tauri::command]
pub async fn add_source_url(
    state: State<'_, AppState>,
    notebook_id: String,
    url: String,
    include: Option<String>,
) -> Result<Source, String> {
    e(ingest_url(&state, &notebook_id, &url, include.as_deref()).await)
}

/// Fetch a URL into a source. Hard failures (network / HTTP / empty) still
/// produce an errored source row so the user sees it and can retry.
pub(crate) async fn ingest_url(
    state: &AppState,
    notebook_id: &str,
    url: &str,
    include: Option<&str>,
) -> anyhow::Result<Source> {
    // Same URL twice is always a mistake — fail fast before fetching.
    let normalized = ingest::normalize_url(url);
    let normalized = normalized.trim_end_matches('/');
    for s in state.db.list_sources(notebook_id).await? {
        if !s.url.is_empty() && s.url.trim_end_matches('/') == normalized && s.status != "error" {
            anyhow::bail!(
                "Already in this notebook as \"{}\" — use Refresh to re-fetch it",
                s.title
            );
        }
    }
    // Git-shaped URLs become git sources (docs/RFC-git-sources.md): repo
    // homes as README, /blob files, /tree subtrees, clone URLs as whole
    // repos — always on; the smarter thing is the only thing. Detection is
    // URL shape plus one remembered host probe; when it says no, the URL
    // falls through to page capture. `include` is the add-modal ladder rung
    // ("readme" | "docs" | "full"); None = the URL shape's default.
    if let Some(target) = crate::git::detect_target(&app_data_dir(state), url).await {
        return match ingest_git(state, notebook_id, url, target, include).await {
            Ok(src) => Ok(src),
            Err(err) => store_failed_url(state, notebook_id, url.trim(), err.to_string()).await,
        };
    }
    // Notion pages (docs/RFC-obsidian-notion.md §4): with a token configured,
    // the page tree exports to a cache dir and ingests via the folder
    // machinery. Without one, public pages fall through to page capture.
    if let Some(page_id) = crate::notion::detect_page(url) {
        let token = { state.ai.read().await.config().notion_token.clone() };
        if !token.is_empty() {
            return match ingest_notion(state, notebook_id, url, &page_id, &token).await {
                Ok(src) => Ok(src),
                Err(err) => store_failed_url(state, notebook_id, url.trim(), err.to_string()).await,
            };
        }
    }
    // A browser-extension clip (docs/RFC-page-capture.md §8) supersedes the
    // generic page-capture fallback: the clipper saw the page as the
    // logged-in user, which the cookieless capture webview never can. It
    // sits *after* the git/notion detectors above — a clipped GitHub or
    // Notion URL still ingests with its specialized identity — and *before*
    // the fetch, so private pages never take the doomed round trip. A clip
    // that extracts to nothing falls through to the normal path below.
    if let Some(clip) = crate::clip::take(url) {
        if let Some(extracted) = clip.into_extracted() {
            return store_extracted(state, notebook_id, extracted).await;
        }
    }
    match crate::capture::extract_url_rescued(url).await {
        Ok(extracted) => store_extracted(state, notebook_id, extracted).await,
        Err(err) => store_failed_url(state, notebook_id, url.trim(), err.to_string()).await,
    }
}

/// App data dir (`config_path`'s parent) — capture memory, git host memory,
/// and git cache checkouts live here.
pub(crate) fn app_data_dir(state: &AppState) -> std::path::PathBuf {
    state
        .config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Ingest a remote git target. Singles (README default, blob URLs) land as
/// one source; subtrees and whole repos land as a `git` parent whose
/// children come from the shared folder rescan over a shallow cache
/// checkout under `<app-data>/git/<source-id>`.
pub(crate) async fn ingest_git(
    state: &AppState,
    notebook_id: &str,
    url: &str,
    target: crate::git::GitTarget,
    include: Option<&str>,
) -> anyhow::Result<Source> {
    let data_dir = app_data_dir(state);
    // Resolve the ladder rung: the URL shape's default unless the add modal
    // chose otherwise. A repo home widened past README clones like a whole
    // repo (the stored url stays the one the user pasted).
    let rung = include.unwrap_or(match &target {
        crate::git::GitTarget::RepoHome { .. } => "readme",
        _ => "full",
    });
    let target = match (&target, rung) {
        (crate::git::GitTarget::RepoHome { remote }, "docs" | "full") => {
            crate::git::GitTarget::CloneAll {
                remote: remote.clone(),
            }
        }
        _ => target,
    };
    let staged = crate::git::clone_target(&data_dir, &target).await?;
    let label = target.repo_label();
    let stored_url = url.trim().trim_end_matches('/').to_string();
    match &staged.kind {
        crate::git::StagedKind::Single { file_rel } => {
            let abs = staged.dir.join(file_rel);
            let mut extracted = ingest::extract_file(&abs.to_string_lossy())?;
            if matches!(target, crate::git::GitTarget::RepoHome { .. }) {
                // The repo is the identity, not the filename README.md.
                extracted.title = label.clone();
            }
            if let Some(line) = crate::git::provenance_header(&staged.dir).await {
                extracted.text = format!("{line}\n\n{}", extracted.text);
            }
            extracted.url = stored_url;
            let ctx = (extracted.source_type == "code").then(|| format!("{label} › {file_rel}"));
            let src = store_new_source(state, notebook_id, extracted, "", 0, ctx.as_deref(), true)
                .await?;
            if let Err(err) = crate::git::adopt_cache(&staged.dir, &data_dir, &src.id) {
                // The source still works; it just can't re-sync until re-added.
                crate::note!("git: failed to adopt cache for {}: {err:#}", src.id);
            }
            let stamp = crate::mac::content_stamp(&staged.sha);
            state.db.set_source_mtime(&src.id, stamp).await?;
            Ok(Source {
                mtime: stamp,
                ..src
            })
        }
        crate::git::StagedKind::Tree => {
            let parent = Source {
                image_url: String::new(),
                author: String::new(),
                id: new_id(),
                notebook_id: notebook_id.to_string(),
                title: label,
                source_type: "git".to_string(),
                url: stored_url,
                content: String::new(),
                char_count: 0,
                chunk_count: 0,
                created_at: now(),
                status: "ready".to_string(),
                error: String::new(),
                parent_id: String::new(),
                mtime: crate::mac::content_stamp(&staged.sha),
                tags: String::new(),
                note: String::new(),
                fetched_at: now(),
                fetch_failures: 0,
            };
            state.db.insert_source(&parent, &[], &[]).await?;
            crate::git::adopt_cache(&staged.dir, &data_dir, &parent.id)
                .map_err(|e| anyhow::anyhow!("failed to adopt git cache: {e}"))?;
            if rung == "docs" {
                // Recorded before the first rescan so the filter applies
                // from the very first scan.
                crate::git::record_include(&crate::git::cache_dir(&data_dir, &parent.id), "docs");
            }
            let _guard = state.folder_scan_lock.lock().await;
            rescan_one_folder(None, state, &parent, true).await?;
            state.db.touch_notebook(notebook_id, now()).await?;
            Ok(Source {
                content: String::new(),
                ..parent
            })
        }
    }
}

/// Notion page tree -> parent source + markdown cache dir + folder rescan
/// (docs/RFC-obsidian-notion.md §4). The exporter writes only changed pages,
/// so the rescan re-embeds only what moved.
pub(crate) async fn ingest_notion(
    state: &AppState,
    notebook_id: &str,
    url: &str,
    page_id: &str,
    token: &str,
) -> anyhow::Result<Source> {
    let data_dir = app_data_dir(state);
    let parent_id = new_id();
    let dir = crate::notion::cache_dir(&data_dir, &parent_id);
    let client = crate::notion::NotionClient::new(token);
    let stats = client.export_tree(page_id, &dir).await?;
    let parent = Source {
        image_url: String::new(),
        author: String::new(),
        id: parent_id,
        notebook_id: notebook_id.to_string(),
        title: stats.title.clone(),
        source_type: "notion".to_string(),
        url: url.trim().trim_end_matches('/').to_string(),
        content: String::new(),
        char_count: 0,
        chunk_count: 0,
        created_at: now(),
        status: "ready".to_string(),
        error: String::new(),
        parent_id: String::new(),
        mtime: stats.max_edited_ms,
        tags: String::new(),
        note: String::new(),
        fetched_at: now(),
        fetch_failures: 0,
    };
    state.db.insert_source(&parent, &[], &[]).await?;
    let _guard = state.folder_scan_lock.lock().await;
    rescan_one_folder(None, state, &parent, true).await?;
    state.db.touch_notebook(notebook_id, now()).await?;
    Ok(parent)
}

/// Re-read a git-backed single source (README/blob) from its cache checkout
/// and re-embed, stamping the given sha.
async fn reextract_git_single(
    state: &AppState,
    existing: &Source,
    sha: &str,
) -> anyhow::Result<Source> {
    let data_dir = app_data_dir(state);
    let file = crate::git::checkout_root(&data_dir, &existing.id);
    if !file.is_file() {
        anyhow::bail!(
            "git cache for \"{}\" is missing — remove and re-add the source",
            existing.title
        );
    }
    let dir = crate::git::cache_dir(&data_dir, &existing.id);
    let mut extracted = ingest::extract_file(&file.to_string_lossy())?;
    if let Some(line) = crate::git::provenance_header(&dir).await {
        extracted.text = format!("{line}\n\n{}", extracted.text);
    }
    extracted.title = existing.title.clone();
    extracted.url = existing.url.clone();
    let ctx = (extracted.source_type == "code")
        .then(|| crate::git::parse_git_url(&existing.url).map(|t| t.repo_label()))
        .flatten()
        .zip(file.strip_prefix(&dir).ok())
        .map(|(label, rel)| format!("{label} › {}", rel.to_string_lossy()));
    let mut ex = existing.clone();
    ex.mtime = crate::mac::content_stamp(sha);
    reingest(state, &ex, extracted, ctx.as_deref(), true).await
}

/// The exact-match retrieval leg (RFC-git-sources §6): when the query
/// carries code-shaped tokens, grep the notebook's repo-backed children
/// directly (no walking — the scan already chose the files) and return the
/// best line windows as ordinary citations pointing at the child sources.
/// The notebook's repo- and folder-backed child files as
/// (abs path, source id, title) — shared by the chat grep leg and the MCP
/// grep/ast tools. Capped; respects the source selection when given.
pub(crate) async fn repo_backed_files(
    state: &AppState,
    notebook_id: &str,
    selection: Option<&[String]>,
) -> Vec<(String, String, String)> {
    let Ok(sources) = state.db.list_sources(notebook_id).await else {
        return Vec::new();
    };
    let parents: HashSet<&str> = sources
        .iter()
        .filter(|s| {
            matches!(
                s.source_type.as_str(),
                "folder" | "obsidian" | "git" | "notion"
            ) && s.parent_id.is_empty()
        })
        .map(|s| s.id.as_str())
        .collect();
    let selected = |id: &str| selection.is_none_or(|ids| ids.iter().any(|x| x == id));
    sources
        .iter()
        .filter(|s| parents.contains(s.parent_id.as_str()))
        .filter(|s| s.status == "ready" && !s.url.is_empty())
        .filter(|s| selected(&s.id))
        .map(|s| (s.url.clone(), s.id.clone(), s.title.clone()))
        .take(800)
        .collect()
}

/// One exact-match window for the `/grep` composer command.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GrepHitOut {
    pub source_id: String,
    pub source_title: String,
    pub path: String,
    pub line: u64,
    pub window: String,
}

/// `/grep` in the chat composer: the same in-process ripgrep engine the MCP
/// `grep_sources` tool uses, exposed as a command so the chat UI can render
/// hits locally with no model call. Searches the notebook's repo- and
/// folder-backed files (whole working trees, not just embedded passages).
#[tauri::command]
pub async fn grep_sources(
    state: State<'_, AppState>,
    notebook_id: String,
    pattern: String,
    max_results: Option<u32>,
) -> Result<Vec<GrepHitOut>, String> {
    let pattern = pattern.trim().to_string();
    if pattern.is_empty() {
        return Err("Enter text to search for.".to_string());
    }
    let files = repo_backed_files(&state, &notebook_id, None).await;
    if files.is_empty() {
        return Err("This notebook has no files from repos or folders to search.".to_string());
    }
    let k = max_results.unwrap_or(8).clamp(1, 20) as usize;
    let paths: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
    let hits =
        tokio::task::spawn_blocking(move || crate::grepsearch::search_pattern(&pattern, &paths, k))
            .await
            .map_err(|err| err.to_string())??;
    Ok(hits
        .into_iter()
        .map(|h| {
            let (path, id, title) = &files[h.file_index];
            GrepHitOut {
                source_id: id.clone(),
                source_title: title.clone(),
                path: path.clone(),
                line: h.first_line,
                window: h.window,
            }
        })
        .collect())
}

/// Iterative retrieval's gap loop (RFC-judged-evals §4.3): show the small
/// tier the first-pass excerpts, ask what ONE search would find the
/// missing evidence, run it, and merge. Self-gating — NONE means the
/// first pass suffices and nothing else happens. Merging interleaves the
/// two pools so gap evidence reaches the prompt even on tiers with no
/// cross-encoder to reorder the union; tiers WITH one rerank the merged
/// pool anyway (that combination is where the measured +12pt multi-hop
/// win came from). Returns the gap query for the retrieval trace; any
/// failure leaves the pool untouched.
#[allow(clippy::too_many_arguments)]
async fn gap_retrieve(
    ai: &crate::ai::Ai,
    db: &crate::db::Db,
    notebook_id: &str,
    question: &str,
    pool: &mut Vec<Citation>,
    k: usize,
    fetch_k: usize,
    source_ids: Option<&[String]>,
) -> Option<String> {
    let preview: String = pool
        .iter()
        .take(k)
        .map(|c| {
            format!(
                "- {}: {}\n",
                c.source_title,
                c.snippet.chars().take(150).collect::<String>()
            )
        })
        .collect();
    let prompt = format!(
        "Question: {question}\n\nExcerpts found so far:\n{preview}\n\
         Does the question involve a second entity, comparison, or linked fact that \
         these excerpts do NOT cover? If everything needed is already here, or the \
         question has a single subject the excerpts address, reply NONE. Otherwise \
         reply with ONLY the search query text for the missing evidence — a search \
         that merely restates the question is useless; reply NONE instead."
    );
    let reply = ai
        .chat_role(
            crate::inference::Role::Small,
            &[crate::ai::ChatTurn::user(prompt)],
        )
        .await
        .ok()?;
    let gq = reply
        .text
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .trim_end_matches('.')
        .to_string();
    if gq.is_empty() || gq.len() > 200 || gq.to_ascii_lowercase().starts_with("none") {
        return None;
    }
    // Rephrase guard (found live, not in the harness): small models
    // sometimes restate the question instead of answering NONE — a second
    // search for the same thing buys nothing. Jaccard word similarity
    // catches the restatement (≈0.7) while sparing a targeted gap query,
    // which reuses only PART of the question's words (≈0.4 on the live
    // comparison case) — plain question-overlap would kill both.
    let words = |s: &str| -> std::collections::HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .map(str::to_string)
            .collect()
    };
    let (qw, gw) = (words(question), words(&gq));
    let overlap = qw.intersection(&gw).count();
    let union = qw.union(&gw).count();
    if union > 0 && overlap * 10 >= union * 6 {
        return None;
    }
    let qvec = ai.embed_one(&gq).await.ok()?;
    let extra = db
        .search_chunks_trace(notebook_id, qvec, &gq, fetch_k, source_ids)
        .await
        .ok()?
        .final_hits;
    let first = std::mem::take(pool);
    let mut seen: std::collections::HashSet<String> =
        first.iter().map(|c| c.chunk_id.clone()).collect();
    let gap: Vec<Citation> = extra
        .into_iter()
        .filter(|c| seen.insert(c.chunk_id.clone()))
        .collect();
    let mut merged = Vec::with_capacity(first.len() + gap.len());
    let mut gap_iter = gap.into_iter();
    for (i, c) in first.into_iter().enumerate() {
        merged.push(c);
        // Alternate after the top first-pass hits so both searches place
        // evidence inside any truncation window.
        if i >= 1 {
            if let Some(g) = gap_iter.next() {
                merged.push(g);
            }
        }
    }
    merged.extend(gap_iter);
    *pool = merged;
    Some(gq)
}

async fn grep_leg(
    state: &AppState,
    notebook_id: &str,
    query: &str,
    selection: Option<&[String]>,
) -> Vec<Citation> {
    let tokens = crate::grepsearch::code_tokens(query);
    if tokens.is_empty() {
        return Vec::new();
    }
    let files = repo_backed_files(state, notebook_id, selection).await;
    if files.is_empty() {
        return Vec::new();
    }
    let paths: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
    let hits =
        tokio::task::spawn_blocking(move || crate::grepsearch::search_files(&tokens, &paths, 4))
            .await
            .unwrap_or_default();
    hits.into_iter()
        .map(|h| {
            let (path, id, title) = &files[h.file_index];
            Citation {
                chunk_id: format!("grep:{}:{}", id, h.first_line),
                source_id: id.clone(),
                source_title: title.clone(),
                source_path: path.clone(),
                note_id: String::new(),
                gist: false,
                snote: false,
                ordinal: 0,
                snippet: h.window,
                // Not a vector hit — match count carried the ranking; the
                // field only feeds trace summaries.
                distance: 0.0,
            }
        })
        .collect()
}

/// Reciprocal-rank fusion of the hybrid citations with the grep windows —
/// same constant as the vector/BM25 fusion, deterministic tie-break, capped
/// grep contribution so a hot identifier can't flood the excerpt list.
fn fuse_grep_hits(db_hits: Vec<Citation>, grep_hits: Vec<Citation>, k: usize) -> Vec<Citation> {
    if grep_hits.is_empty() {
        return db_hits;
    }
    let mut scored: Vec<(f32, Citation)> = Vec::new();
    for (rank, c) in db_hits.into_iter().enumerate() {
        scored.push((1.0 / (60.0 + rank as f32), c));
    }
    for (rank, c) in grep_hits.into_iter().enumerate().take(4) {
        scored.push((1.0 / (60.0 + rank as f32), c));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.chunk_id.cmp(&b.1.chunk_id))
    });
    scored.into_iter().map(|(_, c)| c).take(k).collect()
}

#[tauri::command]
pub async fn add_source_text(
    state: State<'_, AppState>,
    notebook_id: String,
    title: String,
    text: String,
) -> Result<Source, String> {
    let extracted = e(ingest::extract_pasted(&title, &text))?;
    e(store_extracted(&state, &notebook_id, extracted).await)
}

/// Deterministic line-level diff for change events: `(stats, excerpt)`.
/// Multiset difference, not LCS — cheap at folder-sync scale and enough to
/// say what moved; the excerpt shows a few added/removed lines verbatim,
/// document-ordered, ± prefixed. Empty when nothing textual changed.
fn diff_excerpt(old: &str, new: &str) -> (String, String) {
    const EXCERPT_LINES: usize = 6;
    const EXCERPT_CHARS: usize = 1_500;
    const LINE_CHARS: usize = 160;
    if old == new || old.is_empty() {
        return (String::new(), String::new());
    }
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for line in old.lines() {
        *counts.entry(line).or_default() -= 1;
    }
    for line in new.lines() {
        *counts.entry(line).or_default() += 1;
    }
    let (mut added, mut removed) = (0i64, 0i64);
    for (line, c) in &counts {
        if line.trim().is_empty() {
            continue;
        }
        if *c > 0 {
            added += c;
        } else {
            removed -= c;
        }
    }
    if added == 0 && removed == 0 {
        return (String::new(), String::new());
    }
    let stats = format!("+{added} \u{2212}{removed} lines");
    let mut excerpt = String::new();
    let mut sample = |lines: std::str::Lines<'_>, sign: i64, prefix: char| {
        let mut shown = 0usize;
        for line in lines {
            if shown >= EXCERPT_LINES || excerpt.len() >= EXCERPT_CHARS {
                break;
            }
            if line.trim().is_empty() {
                continue;
            }
            if let Some(c) = counts.get_mut(line) {
                if *c * sign > 0 {
                    *c -= sign;
                    let clipped: String = line.chars().take(LINE_CHARS).collect();
                    excerpt.push_str(&format!("{prefix} {clipped}\n"));
                    shown += 1;
                }
            }
        }
    };
    sample(new.lines(), 1, '+');
    sample(old.lines(), -1, '\u{2212}');
    (stats, excerpt.trim_end().to_string())
}

/// Replace a source's content in place (edit / refresh). The ROW lands
/// immediately; chunking and embedding run in the shared background stage
/// (docs/RFC-import-pipeline.md §2), with the old chunks serving retrieval
/// until the new ones swap in — a refresh never opens a search gap and
/// never blocks its caller on a model. `code_ctx` as in `store_new_source`.
pub(crate) async fn reingest(
    state: &AppState,
    existing: &Source,
    extracted: ingest::Extracted,
    code_ctx: Option<&str>,
    embed: bool,
) -> anyhow::Result<Source> {
    // Classify against the stored URL: text edits arrive via extract_pasted
    // with an empty extracted.url, which would drop the Google-doc exemption.
    let (status, error) = classify(&existing.source_type, &existing.url, &extracted.text);
    // Repository-tier code children store their content but skip embedding —
    // the ripgrep leg reaches them at query time (RFC-git-sources §4).
    let processing = embed && status == "ready";
    // An empty extracted.url means the text came from an edit or paste, not a
    // re-fetch — keep the stored origin (URL or file path) so refresh keeps
    // working after edits.
    let url = if extracted.url.is_empty() {
        existing.url.clone()
    } else {
        extracted.url.clone()
    };
    let updated = Source {
        // A refresh may carry a new lead image; edits/pastes carry none —
        // keep the stored one then (same rule as author below).
        image_url: if extracted.image_url.is_empty() {
            existing.image_url.clone()
        } else {
            extracted.image_url.clone()
        },
        // A paste/edit re-extract carries no file authorship — keep what the
        // original ingest captured rather than blanking it. (Cloned:
        // `extracted` travels whole into the embed stage below.)
        author: if extracted.author.is_empty() {
            existing.author.clone()
        } else {
            extracted.author.clone()
        },
        id: existing.id.clone(),
        notebook_id: existing.notebook_id.clone(),
        title: presentable_title(&extracted.title, &url),
        source_type: existing.source_type.clone(),
        url,
        content: extracted.text.clone(),
        char_count: extracted.text.chars().count() as i64,
        // The old chunks keep serving until the stage swaps them — their
        // count stays honest in the meantime; the stage stamps the new one.
        chunk_count: if processing { existing.chunk_count } else { 0 },
        created_at: existing.created_at,
        status: if processing {
            "processing".to_string()
        } else {
            status
        },
        error,
        // Folder membership and change-tracking travel with the row; a rescan
        // that re-ingests a changed file passes `existing` with a fresh mtime.
        parent_id: existing.parent_id.clone(),
        mtime: existing.mtime,
        // User metadata survives every re-embed: tags and the annotation
        // describe why the source matters, not what its bytes say.
        tags: existing.tags.clone(),
        note: existing.note.clone(),
        // A successful ingest IS the freshness signal: stamp it and clear
        // any probe-failure streak (docs/RFC-source-hygiene.md).
        fetched_at: now(),
        fetch_failures: 0,
    };
    if processing {
        // Row first, chunks in the background — the stage's content claim
        // (spawn_embed_stage) settles racing refreshes in the newest one's
        // favor.
        state.db.replace_source_row(&updated).await?;
        spawn_embed_stage(
            state,
            &updated,
            extracted,
            code_ctx.map(str::to_string),
            true,
        )
        .await;
    } else {
        // No embedding to wait for (code children, errored extractions):
        // the full swap stays synchronous, old chunks dropped with it.
        state.db.replace_source(&updated, &[], &[]).await?;
    }
    // A refreshed PDF may have a new first page, a refreshed page a new
    // hero image — drop the stale caches so the gallery re-renders them.
    if existing.source_type == "pdf" {
        let _ = std::fs::remove_file(thumb_path(state, &existing.id));
        // A re-pointed PDF must not keep serving the old file's pages.
        let _ = std::fs::remove_file(pdf_cache_path(state, &existing.id));
    }
    if existing.source_type == "url" && updated.image_url != existing.image_url {
        let _ = std::fs::remove_file(og_cache_path(state, &existing.id));
    }
    state
        .db
        .touch_notebook(&existing.notebook_id, now())
        .await?;
    // Change is an event, not a silent overwrite (RFC-night-shift §Watchers):
    // every content refresh — file, folder child, Mac item, git, URL — lands
    // here, so this is the one write point, and the outgoing content is still
    // in hand, so the diff needs no snapshot table. Best-effort: an event
    // miss must never fail the reingest that produced it.
    let verb = match existing.source_type.as_str() {
        "url" => "page re-fetched",
        "mac" => "Mac item synced",
        "git" => "repository synced",
        _ => "file changed on disk",
    };
    let (stats, diff) = diff_excerpt(&existing.content, &updated.content);
    let detail = if stats.is_empty() {
        verb.to_string()
    } else {
        format!("{verb} \u{00b7} {stats}")
    };
    // Judgment on arrival (commands/weave.rs): the changed lines are weighed
    // against this notebook's ledger. Fire-and-forget, capped, gated.
    if !diff.is_empty() {
        weave::spawn_weave(
            state.db.clone(),
            state.ai.read().await.clone(),
            existing.notebook_id.clone(),
            updated.title.clone(),
            diff.clone(),
        );
    }
    // Re-file on change against the WHOLE updated document, not the diff: a
    // card minted after this source landed should still pick it up, and
    // already-attached pairs skip, so re-running costs nothing.
    if !updated.content.is_empty() {
        registry::spawn_registry_match(
            state.db.clone(),
            existing.notebook_id.clone(),
            existing.id.clone(),
            updated.content.clone(),
        );
    }
    let _ = state
        .db
        .add_source_event(&crate::models::SourceEvent {
            id: new_id(),
            notebook_id: existing.notebook_id.clone(),
            source_id: existing.id.clone(),
            source_title: updated.title.clone(),
            kind: "updated".into(),
            detail,
            diff,
            at: now(),
        })
        .await;
    // Refreshed content means a changed hash — let the sweep re-gist it.
    crate::gist::spawn_sweep(state.db.clone(), state.ai.read().await.clone());
    Ok(Source {
        content: String::new(),
        ..updated
    })
}

/// Mark an existing source as failed (used when a refresh/retry can't fetch).
async fn mark_source_failed(
    state: &AppState,
    existing: &Source,
    reason: String,
) -> anyhow::Result<Source> {
    let failed = Source {
        content: String::new(),
        char_count: 0,
        chunk_count: 0,
        status: "error".to_string(),
        error: reason,
        ..existing.clone()
    };
    state.db.replace_source(&failed, &[], &[]).await?;
    state
        .db
        .touch_notebook(&existing.notebook_id, now())
        .await?;
    Ok(failed)
}

#[tauri::command]
pub async fn update_source_text(
    state: State<'_, AppState>,
    source_id: String,
    title: String,
    text: String,
) -> Result<Source, String> {
    let existing =
        e(state.db.get_source(&source_id).await)?.ok_or_else(|| "Source not found".to_string())?;
    let extracted = e(ingest::extract_pasted(&title, &text))?;
    e(reingest(&state, &existing, extracted, None, true).await)
}

/// Normalize a raw tag string into the stored form (docs/RFC-source-tags.md):
/// split on whitespace and commas, strip a leading `#`, lowercase, drop
/// empties, dedupe preserving first-seen order, join with single spaces.
pub(crate) fn normalize_tags(raw: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for token in raw.split(|c: char| c.is_whitespace() || c == ',') {
        let t = token.trim_start_matches('#').to_lowercase();
        if !t.is_empty() && seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out.join(" ")
}

/// Stamp a source's tags (normalized on write) and return the updated row
/// in list-payload shape (content stripped). Shared by the Tauri command
/// and the MCP tool. Routes fold the tags in on the next self-healing sweep
/// — the summary string changes, so the diff re-embeds; no extra machinery.
pub(crate) async fn set_source_tags_impl(
    state: &AppState,
    source_id: &str,
    tags: &str,
) -> anyhow::Result<Source> {
    let existing = state
        .db
        .get_source(source_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Source not found"))?;
    let tags = normalize_tags(tags);
    state.db.set_source_tags(source_id, &tags).await?;
    state
        .db
        .touch_notebook(&existing.notebook_id, now())
        .await?;
    Ok(Source {
        tags,
        content: String::new(),
        ..existing
    })
}

/// Store the user's annotation on a source and (re)index it under
/// `snote:<source_id>` so retrieval can surface "why I saved this"
/// (docs/RFC-source-tags.md). Empty note = clear both row field and index.
/// The row is the truth; indexing is best-effort like `index_note` — a
/// failed embed logs and the next edit retries.
pub(crate) async fn set_source_note_impl(
    state: &AppState,
    source_id: &str,
    note: &str,
) -> anyhow::Result<Source> {
    let existing = state
        .db
        .get_source(source_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Source not found"))?;
    let note = note.trim().to_string();
    state.db.set_source_note(source_id, &note).await?;
    if let Err(err) = index_snote(state, &existing, &note).await {
        crate::note!("indexing note on source {source_id} failed: {err:#}");
    }
    state
        .db
        .touch_notebook(&existing.notebook_id, now())
        .await?;
    Ok(Source {
        note,
        content: String::new(),
        ..existing
    })
}

/// (Re)build the chunk rows for a source annotation — the `index_note`
/// pattern with the `snote:` owner prefix. No confabulation gate: the user
/// wrote it (RFC-source-tags §Per-source notes).
async fn index_snote(state: &AppState, source: &Source, note: &str) -> anyhow::Result<()> {
    state.db.delete_snote_chunks(&source.id).await?;
    if note.is_empty() {
        return Ok(());
    }
    let chunks = ingest::chunk_text(&source.title, note);
    if chunks.is_empty() {
        return Ok(());
    }
    let inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let embeddings = {
        let ai = state.ai.read().await.clone();
        ai.embed(&inputs).await?
    };
    let tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(j, c)| (new_id(), j as i32, c.text.clone()))
        .collect();
    state
        .db
        .add_chunks(
            &source.notebook_id,
            &format!("{}{}", crate::db::SNOTE_CHUNK_PREFIX, source.id),
            &tuples,
            &embeddings,
        )
        .await
}

#[tauri::command]
pub async fn set_source_tags(
    state: State<'_, AppState>,
    source_id: String,
    tags: String,
) -> Result<Source, String> {
    e(set_source_tags_impl(&state, &source_id, &tags).await)
}

#[tauri::command]
pub async fn set_source_note(
    state: State<'_, AppState>,
    source_id: String,
    note: String,
) -> Result<Source, String> {
    e(set_source_note_impl(&state, &source_id, &note).await)
}

/// Does this source origin point at the web (vs. a local file path)?
pub(crate) fn is_web_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Classify a notebook's sources into hygiene buckets
/// (docs/RFC-source-hygiene.md) — read-only; the review modal and row
/// badges render from this, and acting on it goes through the normal
/// refresh/delete commands.
#[tauri::command]
pub async fn source_hygiene(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Vec<crate::hygiene::HygieneIssue>, String> {
    let sources = e(state.db.list_sources(&notebook_id).await)?;
    let cadence = state.ai.read().await.config().hygiene_refresh_days;
    Ok(crate::hygiene::classify(&sources, cadence, now()))
}

/// "Keep" from the hygiene review: clear an unreachable source's strike
/// count and stamp it fresh, so the flag drops and the retry cadence
/// restarts from today.
#[tauri::command]
pub async fn hygiene_keep(state: State<'_, AppState>, source_id: String) -> Result<(), String> {
    e(state.db.set_source_fetch(&source_id, now(), 0).await)
}

#[tauri::command]
pub async fn refresh_source_url(
    app: AppHandle,
    state: State<'_, AppState>,
    source_id: String,
) -> Result<Source, String> {
    e(refresh_source_impl(&app, &state, &source_id).await)
}

/// The whole refresh dispatch (folder-like rescan, Mac re-fetch, git sync,
/// web re-extract, file re-read) behind the Refresh menu item — shared by
/// the single command above, the multi-select batch, and the MCP tool
/// (docs/RFC-multi-select.md).
pub(crate) async fn refresh_source_impl(
    app: &AppHandle,
    state: &AppState,
    source_id: &str,
) -> anyhow::Result<Source> {
    let existing = state
        .db
        .get_source(source_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Source not found"))?;
    if existing.url.is_empty() {
        anyhow::bail!("This source has no URL or file path to refresh from");
    }
    if matches!(
        existing.source_type.as_str(),
        "folder" | "obsidian" | "git" | "notion"
    ) {
        // Notion parents re-export changed pages before the rescan.
        if existing.source_type == "notion" {
            let token = { state.ai.read().await.config().notion_token.clone() };
            let page = crate::notion::detect_page(&existing.url);
            if let (Some(page_id), false) = (page, token.is_empty()) {
                let dir = crate::notion::cache_dir(&app_data_dir(state), &existing.id);
                match crate::notion::NotionClient::new(&token)
                    .export_tree(&page_id, &dir)
                    .await
                {
                    Ok(stats) => {
                        let _ = state
                            .db
                            .set_source_mtime(&existing.id, stats.max_edited_ms)
                            .await;
                    }
                    Err(err) => anyhow::bail!("Notion refresh failed: {err:#}"),
                }
            }
        }
        // Git parents force a remote sync first so the rescan sees fresh
        // files; local folders scan the disk as-is.
        if existing.source_type == "git" {
            let dir = crate::git::cache_dir(&app_data_dir(state), &existing.id);
            match crate::git::sync_remote(&dir).await {
                Ok(Some(sha)) => {
                    let stamp = crate::mac::content_stamp(&sha);
                    state.db.set_source_mtime(&existing.id, stamp).await?;
                }
                Ok(None) => {}
                Err(err) => anyhow::bail!("git sync failed: {err:#}"),
            }
        }
        let _guard = state.folder_scan_lock.lock().await;
        rescan_one_folder(Some(app), state, &existing, true).await?;
        let folder = state
            .db
            .get_source(source_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Source not found"))?;
        return Ok(Source {
            content: String::new(),
            ..folder
        });
    }
    if crate::mac::is_mac_uri(&existing.url) {
        // Mac item — re-fetch through cider and re-embed. Like files, a
        // failed fetch (permission prompt pending, app closed) must not wipe
        // the working source.
        let (_, text) = crate::mac::fetch(&existing.url).await?;
        let mut existing = existing;
        existing.mtime = crate::mac::content_stamp(&text);
        let extracted = ingest::Extracted {
            image_url: String::new(),
            author: String::new(),
            title: existing.title.clone(),
            source_type: "mac".to_string(),
            url: existing.url.clone(),
            text,
        };
        return reingest(state, &existing, extracted, None, true).await;
    }
    // Git-backed singles (README/blob) refresh from their cache clone — the
    // cache dir is the definitive marker; page captures of github.com URLs
    // parse git-shaped too but have no clone.
    let git_dir = crate::git::cache_dir(&app_data_dir(state), &existing.id);
    if git_dir.exists() {
        if let Err(err) = crate::git::sync_remote(&git_dir).await {
            anyhow::bail!("git sync failed: {err:#}");
        }
        let sha = crate::git::detect_repo(&git_dir)
            .await
            .map(|r| r.sha)
            .unwrap_or_default();
        return reextract_git_single(state, &existing, &sha).await;
    }
    if is_web_url(&existing.url) {
        return match crate::capture::extract_url_rescued(&existing.url).await {
            Ok(extracted) => reingest(state, &existing, extracted, None, true).await,
            Err(err) => mark_source_failed(state, &existing, err.to_string()).await,
        };
    }
    // File-backed source. Unlike a dead URL (where the errored row is the
    // retry affordance), a failed re-read must NOT wipe the working source —
    // the extracted text and chunks are still perfectly usable. Surface the
    // failure and leave the source untouched.
    if !std::path::Path::new(&existing.url).exists() {
        // iCloud eviction leaves only a hidden `.name.icloud` stub, which a
        // read can't hydrate (unlike File Provider mounts, where the extract
        // below is itself the download). Ask bird to fetch it, then wait —
        // bounded, because a refresh that hangs forever is worse than one
        // that says "still downloading".
        let p = std::path::Path::new(&existing.url).to_path_buf();
        let stub = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| p.with_file_name(format!(".{n}.icloud")));
        if stub.is_some_and(|s| s.exists()) {
            let target = p.clone();
            let hydrated = tokio::task::spawn_blocking(move || {
                let _ = std::process::Command::new("brctl")
                    .arg("download")
                    .arg(&target)
                    .status();
                // bird downloads in the background; the real file replaces
                // the stub when it lands. 90s covers all but huge files.
                for _ in 0..90 {
                    if target.exists() {
                        return true;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                false
            })
            .await
            .unwrap_or(false);
            if !hydrated {
                anyhow::bail!("iCloud is still downloading this file — try again in a moment");
            }
            // Fall through: the file is local now; extract like any refresh.
        } else {
            anyhow::bail!("Original file no longer exists at {}", existing.url);
        }
    }
    let mut extracted = extract_any_file(state, &existing.url).await?;
    let mut existing = existing;
    let mut retitle = false;
    if existing.status == "placeholder" {
        // First real read of an evicted file (reading it just hydrated it) —
        // give it a real title like any fresh import.
        retitle = !friendly_title_fast(&mut extracted);
    } else {
        // Keep the existing title — the file's content changed, its name
        // didn't, and the stored title may be friendlier than the file stem.
        extracted.title = existing.title.clone();
    }
    // Stamp the on-disk mtime, or the next folder rescan would see a mismatch
    // and re-embed this file a second time.
    existing.mtime = file_mtime(std::path::Path::new(&existing.url));
    let src = reingest(state, &existing, extracted, None, true).await?;
    if retitle {
        spawn_retitle(state, &src).await;
    }
    Ok(src)
}

/// Refresh several sources sequentially off the IPC thread
/// (docs/RFC-multi-select.md): the command returns immediately, each source
/// runs the same dispatch as a single Refresh, and one `sources://changed`
/// lands at the end with the tally — one re-list, one toast, however many
/// rows were selected. A per-item loop from the frontend would be N full
/// re-lists and N toasts, and a synchronous loop here would trip the IPC
/// timeout the way per-child folder deletes once did.
#[tauri::command]
pub async fn refresh_sources(
    app: AppHandle,
    notebook_id: String,
    source_ids: Vec<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let mut scan = FolderScan::default();
        for id in &source_ids {
            match refresh_source_impl(&app, &state, id).await {
                Ok(_) => scan.updated += 1,
                Err(err) => {
                    crate::note!("batch refresh: source {id}: {err:#}");
                    scan.failed += 1;
                }
            }
        }
        let _ = app.emit("sources://changed", SourcesChanged { notebook_id, scan });
    });
    Ok(())
}

#[tauri::command]
pub async fn get_source_content(
    state: State<'_, AppState>,
    source_id: String,
) -> Result<String, String> {
    e(state.db.source_content(&source_id).await)
}

#[tauri::command]
pub async fn delete_source(state: State<'_, AppState>, source_id: String) -> Result<(), String> {
    // Deleting a folder or repo removes its children (and their chunks) in
    // one bulk op — a per-child loop was slow enough to trip the IPC timeout.
    if let Some(src) = e(state.db.get_source(&source_id).await)? {
        if matches!(
            src.source_type.as_str(),
            "folder" | "obsidian" | "git" | "notion"
        ) {
            let child_ids: Vec<String> = e(state.db.list_sources(&src.notebook_id).await)?
                .into_iter()
                .filter(|c| c.parent_id == source_id)
                .map(|c| c.id)
                .collect();
            // Parent and children can each own a git or notion cache dir; the
            // bulk delete_source_tree drops their rows in one shot.
            for id in child_ids.iter().chain(std::iter::once(&source_id)) {
                cleanup_source_files(&state, id);
            }
            e(state.db.delete_source_tree(&source_id, &child_ids).await)?;
            return Ok(());
        }
    }
    e(state.db.delete_source(&source_id).await)?;
    cleanup_source_files(&state, &source_id);
    Ok(())
}

/// Remove a deleted source's on-disk leavings: git and Notion cache dirs
/// (no-ops for other types), the gallery thumbnail, and the og:image cache.
fn cleanup_source_files(state: &AppState, source_id: &str) {
    let data_dir = app_data_dir(state);
    crate::git::remove_cache(&data_dir, source_id);
    let notion_cache = crate::notion::cache_dir(&data_dir, source_id);
    if notion_cache.exists() {
        let _ = std::fs::remove_dir_all(&notion_cache);
    }
    let _ = std::fs::remove_file(thumb_path(state, source_id));
    let _ = std::fs::remove_file(og_cache_path(state, source_id));
}

/// Bulk-delete a selection (docs/RFC-multi-select.md): two Lance predicate
/// deletes total via `db.delete_sources`, however many rows are selected.
/// Selected folder-like parents take their children along, exactly like the
/// single delete. Shared by the Tauri command and the MCP tool.
pub(crate) async fn delete_sources_impl(
    state: &AppState,
    notebook_id: &str,
    source_ids: &[String],
) -> anyhow::Result<()> {
    if source_ids.is_empty() {
        return Ok(());
    }
    let all = state.db.list_sources(notebook_id).await?;
    // `db.delete_sources` deletes by id with no notebook predicate, so the
    // caller's list is checked against this notebook's rows before anything
    // is dropped: an id from somewhere else would otherwise delete a
    // stranger's source and its chunks. Refuse the whole batch rather than
    // silently deleting the subset that did match — a partial delete on a
    // mistaken selection is the harder outcome to explain, and to undo.
    let owned: std::collections::HashSet<&str> = all.iter().map(|s| s.id.as_str()).collect();
    if let Some(stray) = source_ids.iter().find(|id| !owned.contains(id.as_str())) {
        anyhow::bail!("source {stray} is not in notebook {notebook_id}");
    }
    let selected: std::collections::HashSet<&str> = source_ids.iter().map(String::as_str).collect();
    // Children of selected parents whose own row wasn't selected — their
    // chunks must be enumerated for the bulk delete's owner list.
    let child_ids: Vec<String> = all
        .iter()
        .filter(|s| selected.contains(s.parent_id.as_str()) && !selected.contains(s.id.as_str()))
        .map(|s| s.id.clone())
        .collect();
    for id in source_ids.iter().chain(child_ids.iter()) {
        cleanup_source_files(state, id);
    }
    state.db.delete_sources(source_ids, &child_ids).await?;
    state.db.touch_notebook(notebook_id, now()).await?;
    Ok(())
}

#[tauri::command]
pub async fn delete_sources(
    state: State<'_, AppState>,
    notebook_id: String,
    source_ids: Vec<String>,
) -> Result<(), String> {
    e(delete_sources_impl(&state, &notebook_id, &source_ids).await)
}

/// Apply one tag string to a whole selection (docs/RFC-multi-select.md) —
/// per-row updates server-side, one IPC call. Tag writes are cheap row
/// updates (no re-embed), so a loop here is fine where a delete loop wasn't.
#[tauri::command]
pub async fn set_sources_tags(
    state: State<'_, AppState>,
    source_ids: Vec<String>,
    tags: String,
) -> Result<(), String> {
    for id in &source_ids {
        e(set_source_tags_impl(&state, id, &tags).await)?;
    }
    Ok(())
}

/// On-disk cache for a source's gallery thumbnail (PDF first pages).
fn thumb_path(state: &AppState, source_id: &str) -> std::path::PathBuf {
    app_data_dir(state)
        .join("thumbs")
        .join(format!("{source_id}.png"))
}

/// On-disk cache for a URL source's downloaded og:image bytes.
fn og_cache_path(state: &AppState, source_id: &str) -> std::path::PathBuf {
    app_data_dir(state)
        .join("thumbs")
        .join(format!("{source_id}.img"))
}

/// Image mime from magic bytes — og caches store raw downloads, so the
/// extension carries no type.
fn sniff_image_mime(bytes: &[u8]) -> &'static str {
    match bytes {
        b if b.starts_with(b"\x89PNG") => "image/png",
        b if b.starts_with(b"\xff\xd8") => "image/jpeg",
        b if b.starts_with(b"GIF8") => "image/gif",
        b if b.len() > 11 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" => "image/webp",
        _ => "image/png",
    }
}

/// Data-URI thumbnail for a source's gallery card: PDFs render their first
/// page (cached on disk, rendered once ever); images return the original
/// file. Empty string when the source has no visual — the card falls back
/// to typography. Base64 over IPC sidesteps the asset:// WKWebView decode
/// caveat (see ImageView in ReaderPane.tsx).
#[tauri::command]
pub async fn source_thumbnail(
    state: State<'_, AppState>,
    source_id: String,
) -> Result<String, String> {
    use base64::Engine;
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let Some(src) = e(state.db.get_source(&source_id).await)? else {
        return Err("Source not found".into());
    };
    match src.source_type.as_str() {
        "pdf" => {
            let cache = thumb_path(&state, &source_id);
            if let Ok(bytes) = std::fs::read(&cache) {
                return Ok(format!("data:image/png;base64,{}", b64(&bytes)));
            }
            if src.url.is_empty() {
                return Ok(String::new());
            }
            // A PDF reaches us two ways: a file on disk, or a link straight to
            // one (arxiv.org/pdf/...). The second has no local bytes, so fetch
            // them — once; the render is disk-cached above either way.
            let png = if std::path::Path::new(&src.url).exists() {
                match crate::pdf::render_pdf_pages(&src.url, 1, 480) {
                    Ok(pages) => match pages.into_iter().next() {
                        Some(png) => png,
                        None => return Ok(String::new()),
                    },
                    Err(_) => return Ok(String::new()), // never fail the gallery
                }
            } else if is_web_url(&src.url) {
                const MAX_PDF_BYTES: usize = 64 * 1024 * 1024;
                let Some(bytes) = ingest::fetch_bytes(&src.url, MAX_PDF_BYTES).await else {
                    return Ok(String::new());
                };
                match crate::pdf::render_first_page_mem(&bytes, 480) {
                    Ok(png) => png,
                    Err(_) => return Ok(String::new()),
                }
            } else {
                return Ok(String::new());
            };
            if let Some(dir) = cache.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&cache, &png);
            Ok(format!("data:image/png;base64,{}", b64(&png)))
        }
        // URL sources: download the og:image once and serve it from disk —
        // reopening the gallery must not re-fetch a page's hero image.
        "url" => {
            let img = &src.image_url;
            if img.is_empty() || img == "-" || !is_web_url(img) {
                return Ok(String::new());
            }
            let cache = og_cache_path(&state, &source_id);
            if let Ok(bytes) = std::fs::read(&cache) {
                return Ok(format!(
                    "data:{};base64,{}",
                    sniff_image_mime(&bytes),
                    b64(&bytes)
                ));
            }
            let Some(bytes) = ingest::fetch_image_bytes(img).await else {
                return Ok(String::new());
            };
            if let Some(dir) = cache.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&cache, &bytes);
            Ok(format!(
                "data:{};base64,{}",
                sniff_image_mime(&bytes),
                b64(&bytes)
            ))
        }
        "image" => {
            if src.url.is_empty() {
                return Ok(String::new());
            }
            // A gallery card never needs the original bytes — a 12 MB scan
            // used to cross IPC as ~16 MB of base64 per card. Downscale once
            // through macOS's sips (the image_bytes_for_ocr precedent) into
            // the same disk cache PDF thumbnails use, and serve that.
            let cache = thumb_path(&state, &source_id);
            if let Ok(bytes) = std::fs::read(&cache) {
                return Ok(format!("data:image/png;base64,{}", b64(&bytes)));
            }
            if !std::path::Path::new(&src.url).exists() {
                return Ok(String::new());
            }
            if let Some(dir) = cache.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let (input, out) = (src.url.clone(), cache.clone());
            let made = tokio::task::spawn_blocking(move || {
                std::process::Command::new("sips")
                    .args(["-s", "format", "png", "-Z", "480"])
                    .arg(&input)
                    .arg("-o")
                    .arg(&out)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            })
            .await
            .unwrap_or(false);
            if made {
                if let Ok(bytes) = std::fs::read(&cache) {
                    return Ok(format!("data:image/png;base64,{}", b64(&bytes)));
                }
            }
            // sips couldn't read it — fall back to raw bytes under a cap:
            // the webview scales, but a RAW-sized file as base64 would
            // balloon the IPC message.
            const MAX_IMAGE_BYTES: u64 = 12 * 1024 * 1024;
            let ok_size = std::fs::metadata(&src.url)
                .map(|m| m.len() <= MAX_IMAGE_BYTES)
                .unwrap_or(false);
            if !ok_size {
                return Ok(String::new());
            }
            let mime = match std::path::Path::new(&src.url)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase()
                .as_str()
            {
                "jpg" | "jpeg" | "jpe" => "image/jpeg",
                "webp" => "image/webp",
                "gif" => "image/gif",
                "bmp" => "image/bmp",
                "tif" | "tiff" => "image/tiff",
                "heic" | "heif" => "image/heic",
                _ => "image/png",
            };
            match std::fs::read(&src.url) {
                Ok(bytes) => Ok(format!("data:{mime};base64,{}", b64(&bytes))),
                Err(_) => Ok(String::new()),
            }
        }
        _ => Ok(String::new()),
    }
}

/// Opening lines of a source's text for a gallery card: provenance
/// blockquotes, images, and blank lines dropped, capped by chars.
fn snippet_of(content: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("> ") || t.starts_with("![") {
            continue;
        }
        // Headings read fine as plain lines; just drop the marker.
        let t = t.trim_start_matches('#').trim_start();
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(t);
        if out.chars().count() >= max_chars {
            break;
        }
    }
    if out.chars().count() > max_chars {
        let cut = out
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(out.len());
        out.truncate(cut);
        out.push('…');
    }
    out
}

/// Batched card snippets for the gallery: one IPC per level instead of one
/// per card. Unknown or empty sources are simply absent from the map.
#[tauri::command]
pub async fn source_snippets(
    state: State<'_, AppState>,
    source_ids: Vec<String>,
    max_chars: Option<usize>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let cap = max_chars.unwrap_or(280).min(1000);
    let ids: Vec<String> = source_ids.into_iter().take(400).collect();
    // One projected scan for the whole level — this was 400 sequential
    // single-id scans of the sources table, the Graph N-scan bug reborn.
    let contents = e(state.db.source_contents(&ids).await)?;
    let mut out = std::collections::HashMap::new();
    for (id, content) in contents {
        let snip = snippet_of(&content, cap);
        if !snip.is_empty() {
            out.insert(id, snip);
        }
    }
    Ok(out)
}

/// Backfill `image_url` for a notebook's pre-gallery URL sources: fetch just
/// the HTML, parse the lead image, stamp the row — no re-chunk, no re-embed.
/// Sources that yield nothing are stamped "-" so the sweep never repeats
/// them. Returns how many sources gained an image.
#[tauri::command]
pub async fn backfill_source_images(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<u32, String> {
    use futures::StreamExt;
    let targets: Vec<Source> = e(state.db.list_sources(&notebook_id).await)?
        .into_iter()
        .filter(|s| s.source_type == "url" && s.image_url.is_empty() && is_web_url(&s.url))
        .collect();
    if targets.is_empty() {
        return Ok(0);
    }
    let results: Vec<(String, Option<String>)> = futures::stream::iter(targets)
        .map(|s| async move {
            let img = ingest::fetch_lead_image(&s.url).await;
            (s.id, img)
        })
        .buffer_unordered(4)
        .collect()
        .await;
    let mut found = 0u32;
    for (id, img) in results {
        let stamp = match img {
            Some(url) => {
                found += 1;
                url
            }
            None => "-".to_string(), // checked, none — don't re-sweep
        };
        e(state.db.set_source_image(&id, &stamp).await)?;
    }
    Ok(found)
}

// ---- Mac sources (cider) ---------------------------------------------------

/// Add a Mac item (Reminders list, Calendar window, Notes folder) as a
/// living source. See docs/RFC-cider-tools.md and src/mac.rs.
#[tauri::command]
pub async fn add_source_mac(
    state: State<'_, AppState>,
    notebook_id: String,
    provider: String,
    collection: String,
    label: String,
) -> Result<Source, String> {
    let uri = crate::mac::mac_uri(&provider, &collection);
    e(ingest_mac(&state, &notebook_id, &uri, &label).await)
}

/// Connect a cider:// origin as a living Mac source — shared by the
/// add-source modal (which builds the uri from its picker) and MCP
/// add_source (which accepts the uri raw from agents).
pub(crate) async fn ingest_mac(
    state: &AppState,
    notebook_id: &str,
    uri: &str,
    label: &str,
) -> anyhow::Result<Source> {
    for s in state.db.list_sources(notebook_id).await? {
        if s.url == uri && s.status != "error" {
            anyhow::bail!(
                "Already in this notebook as \"{}\" — it re-syncs automatically",
                s.title
            );
        }
    }
    // Fetching a nonexistent Reminders list "succeeds" with zero rows; catch
    // the typo here instead of connecting a permanently empty source.
    if let Some(list) = uri.strip_prefix("cider://reminders/list/") {
        if !crate::mac::reminders_list_exists(list).await? {
            anyhow::bail!("No Reminders list named \"{list}\" — check the name in Apple Reminders");
        }
    }
    let (default_title, text) = crate::mac::fetch(uri).await?;
    let title = if label.trim().is_empty() {
        default_title
    } else {
        label.to_string()
    };
    // Mac sources carry a content hash in `mtime` (there's no file mtime);
    // store_extracted stamps 0 for a nonexistent path, so set it after.
    let stamp = crate::mac::content_stamp(&text);
    let extracted = ingest::Extracted {
        image_url: String::new(),
        author: String::new(),
        title,
        source_type: "mac".to_string(),
        url: uri.to_string(),
        text,
    };
    let source = store_extracted(state, notebook_id, extracted).await?;
    state.db.set_source_mtime(&source.id, stamp).await?;
    Ok(source)
}

/// The raw note text behind an Apple Notes source, for the editor (first
/// line is the note's title — keep it there or Notes renames the note).
#[tauri::command]
pub async fn mac_note_body(
    state: State<'_, AppState>,
    source_id: String,
) -> Result<String, String> {
    let src =
        e(state.db.get_source(&source_id).await)?.ok_or_else(|| "Source not found".to_string())?;
    e(crate::mac::note_body(&src.url).await)
}

/// Write an edited body back to the Apple Note, then re-fetch and re-embed so
/// the source mirrors what Notes now has.
#[tauri::command]
pub async fn update_mac_note(
    state: State<'_, AppState>,
    source_id: String,
    body: String,
) -> Result<Source, String> {
    let existing =
        e(state.db.get_source(&source_id).await)?.ok_or_else(|| "Source not found".to_string())?;
    e(crate::mac::update_note(&existing.url, &body).await)?;
    resync_mac_source(&state, existing).await
}

/// Add a reminder to the list a Reminders source mirrors, then resync it.
#[tauri::command]
pub async fn add_mac_reminder(
    state: State<'_, AppState>,
    source_id: String,
    title: String,
    notes: Option<String>,
) -> Result<Source, String> {
    let existing =
        e(state.db.get_source(&source_id).await)?.ok_or_else(|| "Source not found".to_string())?;
    e(crate::mac::add_reminder(&existing.url, &title, notes.as_deref()).await)?;
    resync_mac_source(&state, existing).await
}

/// Check off a reminder in the list a Reminders source mirrors, then resync.
#[tauri::command]
pub async fn complete_mac_reminder(
    state: State<'_, AppState>,
    source_id: String,
    reminder_id: String,
) -> Result<Source, String> {
    let existing =
        e(state.db.get_source(&source_id).await)?.ok_or_else(|| "Source not found".to_string())?;
    e(crate::mac::complete_reminder(&existing.url, &reminder_id).await)?;
    resync_mac_source(&state, existing).await
}

/// Post-write resync: fetch the item's current state and re-embed it.
pub(crate) async fn resync_mac_source(
    state: &AppState,
    mut existing: Source,
) -> Result<Source, String> {
    let (_, text) = e(crate::mac::fetch(&existing.url).await)?;
    existing.mtime = crate::mac::content_stamp(&text);
    let extracted = ingest::Extracted {
        image_url: String::new(),
        author: String::new(),
        title: existing.title.clone(),
        source_type: "mac".to_string(),
        url: existing.url.clone(),
        text,
    };
    e(reingest(state, &existing, extracted, None, true).await)
}

// ---- Folder sources --------------------------------------------------------

/// Rich formats with dedicated extractors — PDF, Office, images, saved pages
/// (mirrors the frontend's SUPPORTED_EXTENSIONS in src/lib/utils.ts). Code
/// and unknown-but-textual files are admitted separately below.
/// `pub(crate)` so `filesearch` can score Spotlight hits against the same list.
pub(crate) const RICH_EXTENSIONS: &[&str] = &[
    "pdf", "txt", "text", "md", "markdown", "html", "htm", "xhtml", "docx", "docm", "doc", "rtf",
    "odt", "pptx", "pptm", "ppt", "odp", "epub", "boxnote", "xlsx", "xls", "xlsm", "xlsb", "ods",
    "csv", "tsv", "gdoc", "gsheet", "gslides", "png", "jpg", "jpeg", "jpe", "webp", "gif", "bmp",
    "tif", "tiff", "heic", "heif", "avif", "ico", "jp2",
];

/// How deep a folder scan descends. Repos nest deeper than research folders;
/// the walker's ignore rules do the real filtering — this only guards
/// pathological trees.
const FOLDER_MAX_DEPTH: usize = 12;

/// Per-file byte cap for code and sniffed text (rich types keep their own
/// extractors' behavior). Oversized files land in the map's skip list.
const TEXT_MAX_BYTES: u64 = 200 * 1024;

/// Above this many eligible files a scope is repository-tier: prose and the
/// map embed, code stores content only and is reached by the ripgrep leg
/// (RFC-git-sources §4). Below it, everything embeds — it's a document.
const REPO_TIER_FILES: usize = 50;

/// Bytes read to decide whether an unknown extension holds text.
const SNIFF_BYTES: usize = 8 * 1024;

/// Vendored/generated directories pruned even when a repo forgot to
/// gitignore them.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "third_party",
    "__snapshots__",
    "__pycache__",
];

/// Name-based skip rules: files that are technically text but poison
/// retrieval. The reason string lands in the folder map's skip list.
fn name_skip_reason(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    const LOCKFILES: &[&str] = &[
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lockb",
        "composer.lock",
        "go.sum",
        "flake.lock",
    ];
    if LOCKFILES.contains(&lower.as_str()) || lower.ends_with(".lock") {
        return Some("lockfile");
    }
    if lower.ends_with(".min.js") || lower.ends_with(".min.css") {
        return Some("minified");
    }
    if lower.ends_with(".map") {
        return Some("source map");
    }
    if lower.ends_with(".snap") {
        return Some("test snapshot");
    }
    if lower.ends_with(".svg") {
        return Some("vector asset");
    }
    None
}

fn rich_ingestable(path: &std::path::Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    RICH_EXTENSIONS.contains(&ext.as_str())
}

/// First-8KB sniff for unknown extensions: UTF-8 with no NUL byte. A
/// multibyte char split at the buffer boundary is fine; a decode error
/// mid-buffer means binary.
fn sniff_is_text(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; SNIFF_BYTES];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    if n == 0 {
        return false;
    }
    let buf = &buf[..n];
    if buf.contains(&0) {
        return false;
    }
    match std::str::from_utf8(buf) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    }
}

/// File mtime in unix millis (0 when unavailable).
fn file_mtime(path: &std::path::Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One file found by a folder scan. `placeholder` = the file exists in the
/// folder but its bytes aren't local (cloud-sync eviction) — list it, but
/// don't read it, or the File Provider would download it behind the user's
/// back.
struct ScanEntry {
    path: String,
    mtime: i64,
    placeholder: bool,
}

/// Is this file present in the directory but not downloaded? Covers OneDrive,
/// Dropbox, and Google Drive (streaming) on macOS — all File Provider mounts
/// mark evicted files SF_DATALESS (stat is safe; only reads hydrate) — plus
/// zero-byte stubs from older sync clients. iCloud's `.name.icloud` stubs are
/// handled separately in the walk.
#[cfg(target_os = "macos")]
fn is_evicted(meta: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const SF_DATALESS: u32 = 0x4000_0000;
    meta.st_flags() & SF_DATALESS != 0 || meta.len() == 0
}

#[cfg(not(target_os = "macos"))]
fn is_evicted(meta: &std::fs::Metadata) -> bool {
    meta.len() == 0
}

/// Everything a folder scan learned: ingestable files (sorted by path) plus
/// the files it deliberately left out, with reasons, for the folder map.
#[derive(Default)]
struct ScanOutcome {
    entries: Vec<ScanEntry>,
    /// (folder-relative path, reason)
    skipped: Vec<(String, String)>,
    /// iCloud `.name.icloud` eviction stubs the caller should kick off a
    /// background `brctl download` for, so they hydrate and a later resync
    /// ingests them. Capped per scan pass so one folder can't spawn hundreds.
    #[cfg(target_os = "macos")]
    icloud_stubs: Vec<String>,
}

/// Max iCloud stubs to request a download for in a single scan pass — bounds
/// the fire-and-forget `brctl download` fan-out on a freshly-added drive.
#[cfg(target_os = "macos")]
const ICLOUD_HYDRATE_CAP: usize = 32;

/// Case-insensitive extension test.
fn has_ext(path: &std::path::Path, want: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(want))
}

/// Collect ingestable files under `root` with ripgrep's walker — respects
/// .gitignore/.ignore inside repos, skips dot-entries (except iCloud eviction
/// stubs) and symlinks, prunes vendored dirs. Rich types route by extension,
/// code by `ingest::is_code_path`, and unknown extensions by a text sniff.
/// Cloud-evicted files come back as placeholders rather than being dropped —
/// except unknown ones, which can't be sniffed without forcing a download.
fn scan_folder(root: &std::path::Path) -> ScanOutcome {
    let pruned: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let pruned_rec = pruned.clone();
    let root_owned = root.to_path_buf();
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
        .max_depth(Some(FOLDER_MAX_DEPTH))
        .follow_links(false)
        .filter_entry(move |e| {
            let name = e.file_name().to_string_lossy().to_string();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if let Some(rest) = name.strip_prefix('.') {
                // Dot entries stay hidden (what hidden(true) would do), except
                // iCloud stubs — surfaced as placeholders in the loop below.
                return !is_dir && rest.ends_with(".icloud") && rest.len() > ".icloud".len();
            }
            if is_dir && SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
                if let (Ok(rel), Ok(mut rec)) =
                    (e.path().strip_prefix(&root_owned), pruned_rec.lock())
                {
                    rec.push(format!("{}/", rel.to_string_lossy()));
                }
                return false;
            }
            true
        });

    let mut out = ScanOutcome::default();
    // Per-pass budget for kicking off iCloud downloads (macOS only).
    #[cfg(target_os = "macos")]
    let mut hydrate_budget = ICLOUD_HYDRATE_CAP;
    for dent in builder.build() {
        let Ok(dent) = dent else { continue };
        if dent.depth() == 0 {
            continue;
        }
        let Some(ft) = dent.file_type() else { continue };
        if ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let path = dent.path();
        let name = dent.file_name().to_string_lossy().to_string();

        // iCloud Drive evicts files by replacing them with a hidden
        // `.name.icloud` stub — surface it under the real filename so it
        // upgrades in place once downloaded.
        if name.starts_with('.') {
            if let Some(real) = name
                .strip_prefix('.')
                .and_then(|n| n.strip_suffix(".icloud"))
                .filter(|n| !n.is_empty())
            {
                let Some(dir) = path.parent() else { continue };
                let real_path = dir.join(real);
                let real_str = real_path.to_string_lossy().to_string();
                if (rich_ingestable(&real_path) || ingest::is_code_path(&real_str))
                    && !real_path.exists()
                {
                    out.entries.push(ScanEntry {
                        path: real_str,
                        mtime: file_mtime(path),
                        placeholder: true,
                    });
                    // Nudge iCloud to hydrate the stub in the background so a
                    // later resync ingests it — unlike other File Provider
                    // mounts, iCloud never downloads on its own. Bounded.
                    #[cfg(target_os = "macos")]
                    if hydrate_budget > 0 {
                        out.icloud_stubs.push(path.to_string_lossy().into_owned());
                        hydrate_budget -= 1;
                    }
                }
            }
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .map(|r| r.to_string_lossy().to_string())
            .unwrap_or_else(|_| name.clone());
        if let Some(reason) = name_skip_reason(&name) {
            out.skipped.push((rel, reason.to_string()));
            continue;
        }
        let Ok(meta) = dent.metadata() else { continue };
        let evicted = is_evicted(&meta);
        let path_str = path.to_string_lossy().to_string();
        let too_large = meta.len() > TEXT_MAX_BYTES;
        // Dropbox Paper docs surface as `.paper` files. A stub that links to the
        // online doc is fetched like a page (extract_any_file); an opaque or
        // online-only one is skipped with a reason rather than dumping its
        // wrapper bytes into the index.
        if has_ext(path, "paper") {
            if evicted {
                out.skipped
                    .push((rel, "Dropbox Paper (online-only)".to_string()));
            } else if ingest::dropbox_paper_url(&path_str).is_some() {
                out.entries.push(ScanEntry {
                    path: path_str,
                    mtime: file_mtime(path),
                    placeholder: false,
                });
            } else {
                out.skipped
                    .push((rel, "Dropbox Paper (open on dropbox.com)".to_string()));
            }
            continue;
        }
        if rich_ingestable(path) {
            out.entries.push(ScanEntry {
                path: path_str,
                mtime: file_mtime(path),
                placeholder: evicted,
            });
        } else if ingest::is_code_path(&path_str) {
            if !evicted && too_large {
                out.skipped
                    .push((rel, format!("too large ({} KB)", meta.len() / 1024)));
            } else {
                out.entries.push(ScanEntry {
                    path: path_str,
                    mtime: file_mtime(path),
                    placeholder: evicted,
                });
            }
        } else if evicted {
            out.skipped.push((rel, "not downloaded".to_string()));
        } else if too_large {
            out.skipped
                .push((rel, format!("too large ({} KB)", meta.len() / 1024)));
        } else if sniff_is_text(path) {
            out.entries.push(ScanEntry {
                path: path_str,
                mtime: file_mtime(path),
                placeholder: false,
            });
        } else {
            out.skipped.push((rel, "binary".to_string()));
        }
    }

    if let Ok(mut rec) = pruned.lock() {
        for dir in rec.drain(..) {
            out.skipped.push((dir, "vendored directory".to_string()));
        }
    }
    out.entries.sort_by(|a, b| a.path.cmp(&b.path));
    out.skipped.sort();
    out
}

/// Source type for a file we haven't read yet (placeholder rows), so the list
/// shows the right icon.
fn source_type_for_path(path: &str) -> &'static str {
    if ingest::is_code_path(path) {
        "code"
    } else if ingest::is_pdf(path) {
        "pdf"
    } else if ingest::is_image(path) {
        "image"
    } else if std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
    {
        "markdown"
    } else {
        "text"
    }
}

/// Emitted per file while a folder scan ingests, so the UI can show progress.
#[derive(serde::Serialize, Clone)]
struct FolderProgress {
    done: u32,
    total: u32,
    title: String,
}

/// Persist a folder child whose extraction failed. Recording the mtime means
/// the file isn't retried (possibly through expensive OCR) every rescan —
/// only when it changes on disk again.
async fn store_failed_child(
    state: &AppState,
    folder: &Source,
    path: &str,
    mtime: i64,
    reason: String,
) -> anyhow::Result<()> {
    let source = Source {
        image_url: String::new(),
        author: String::new(),
        id: new_id(),
        notebook_id: folder.notebook_id.clone(),
        title: ingest::file_title(path),
        source_type: source_type_for_path(path).to_string(),
        url: path.to_string(),
        content: String::new(),
        char_count: 0,
        chunk_count: 0,
        created_at: now(),
        status: "error".to_string(),
        error: reason,
        parent_id: folder.id.clone(),
        mtime,
        tags: String::new(),
        note: String::new(),
        fetched_at: now(),
        fetch_failures: 0,
    };
    state.db.insert_source(&source, &[], &[]).await
}

/// Persist a cloud-evicted folder child: visible and labeled in the list, no
/// content or chunks. It upgrades to a real source the rescan after its bytes
/// arrive locally.
async fn store_placeholder_child(
    state: &AppState,
    folder: &Source,
    path: &str,
    mtime: i64,
) -> anyhow::Result<()> {
    let source = Source {
        image_url: String::new(),
        author: String::new(),
        id: new_id(),
        notebook_id: folder.notebook_id.clone(),
        title: ingest::file_title(path),
        source_type: source_type_for_path(path).to_string(),
        url: path.to_string(),
        content: String::new(),
        char_count: 0,
        chunk_count: 0,
        created_at: now(),
        status: "placeholder".to_string(),
        error: String::new(),
        parent_id: folder.id.clone(),
        mtime,
        tags: String::new(),
        note: String::new(),
        fetched_at: now(),
        fetch_failures: 0,
    };
    state.db.insert_source(&source, &[], &[]).await
}

/// "folder title › relative/path" — the retrieval context embedded into a
/// code child's chunks (None for non-code files).
/// Per-file promote/demote choices from the repo reader (RFC-git-sources
/// §4). Kept in app data — never inside a user's repo — keyed by parent
/// source id; rescans consult them before the tier rule.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct EmbedOverrides {
    pub embed: Vec<String>,
    pub unembed: Vec<String>,
}

fn embed_overrides_path(data_dir: &std::path::Path, parent_id: &str) -> std::path::PathBuf {
    data_dir
        .join("embed_overrides")
        .join(format!("{parent_id}.json"))
}

pub(crate) fn load_embed_overrides(data_dir: &std::path::Path, parent_id: &str) -> EmbedOverrides {
    std::fs::read_to_string(embed_overrides_path(data_dir, parent_id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_embed_overrides(data_dir: &std::path::Path, parent_id: &str, ov: &EmbedOverrides) {
    let path = embed_overrides_path(data_dir, parent_id);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(ov) {
        let _ = std::fs::write(path, json);
    }
}

/// Promote a repo child into the embedded tier or demote it to search-only,
/// persist the choice, and re-ingest the file to match.
#[tauri::command]
pub async fn set_child_embedded(
    state: State<'_, AppState>,
    source_id: String,
    embed: bool,
) -> Result<Source, String> {
    let child =
        e(state.db.get_source(&source_id).await)?.ok_or_else(|| "Source not found".to_string())?;
    if child.parent_id.is_empty() {
        return Err("Only files inside a folder or repo can be promoted".into());
    }
    let parent = e(state.db.get_source(&child.parent_id).await)?
        .ok_or_else(|| "Parent source not found".to_string())?;
    let data_dir = app_data_dir(&state);
    let root_buf = match parent.source_type.as_str() {
        "git" => crate::git::checkout_root(&data_dir, &parent.id),
        "notion" => crate::notion::cache_dir(&data_dir, &parent.id),
        _ => std::path::PathBuf::from(&parent.url),
    };
    let rel = std::path::Path::new(&child.url)
        .strip_prefix(&root_buf)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| child.url.clone());

    let mut ov = load_embed_overrides(&data_dir, &parent.id);
    ov.embed.retain(|r| r != &rel);
    ov.unembed.retain(|r| r != &rel);
    if embed {
        ov.embed.push(rel.clone());
    } else {
        ov.unembed.push(rel.clone());
    }
    save_embed_overrides(&data_dir, &parent.id, &ov);

    let mut extracted = e(extract_any_file(&state, &child.url).await)?;
    extracted.title = child.title.clone();
    let ctx = code_context(&parent.title, &root_buf, &child.url);
    let mut existing = child;
    existing.mtime = file_mtime(std::path::Path::new(&existing.url));
    e(reingest(&state, &existing, extracted, ctx.as_deref(), embed).await)
}

fn code_context(folder_title: &str, root: &std::path::Path, path: &str) -> Option<String> {
    if !ingest::is_code_path(path) {
        return None;
    }
    let rel = std::path::Path::new(path)
        .strip_prefix(root)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string());
    Some(format!("{folder_title} › {rel}"))
}

/// Reconcile one folder source with the directory on disk: ingest new files,
/// re-ingest changed ones (by mtime), drop children whose file is gone, and
/// keep the parent's folder/repo map current. `force_map` re-renders the map
/// even when the scan found no changes (manual refresh, first scan).
///
/// FTS rebuilds are deferred across the whole scan and flushed once at the
/// end (error paths included): per-child rebuilds made folder imports O(n²)
/// — a 48-file folder paid 48 full BM25 index rebuilds.
async fn rescan_one_folder(
    app: Option<&AppHandle>,
    state: &AppState,
    folder: &Source,
    force_map: bool,
) -> anyhow::Result<FolderScan> {
    state.db.defer_fts(true);
    let result = rescan_one_folder_inner(app, state, folder, force_map).await;
    state.db.defer_fts(false);
    if let Err(err) = state.db.flush_fts().await {
        crate::note!("folder scan: FTS flush failed: {err:#}");
    }
    result
}

async fn rescan_one_folder_inner(
    app: Option<&AppHandle>,
    state: &AppState,
    folder: &Source,
    force_map: bool,
) -> anyhow::Result<FolderScan> {
    let mut scan = FolderScan::default();
    // Git parents scan their cache checkout (plus sparse scope), Notion
    // parents their export dir; local folders scan the path in `url`.
    let root_buf = match folder.source_type.as_str() {
        "git" => crate::git::checkout_root(&app_data_dir(state), &folder.id),
        "notion" => crate::notion::cache_dir(&app_data_dir(state), &folder.id),
        _ => std::path::PathBuf::from(&folder.url),
    };
    let root = root_buf.as_path();
    // Upgrade a plain local folder to an Obsidian vault when `.obsidian/`
    // appears (covers folders added before vault detection existed). One
    // column flip; the rest of the scan is identical for both types.
    if folder.source_type == "folder" && root.join(".obsidian").is_dir() {
        let _ = state.db.set_source_type(&folder.id, "obsidian").await;
    }
    if !root.is_dir() {
        // Folder vanished (unmounted / renamed / not yet synced). Keep the
        // children — their text is still usable — but flag the folder row.
        if folder.status != "error" {
            let failed = Source {
                status: "error".to_string(),
                error: format!("Folder no longer exists at {}", folder.url),
                ..folder.clone()
            };
            state.db.replace_source(&failed, &[], &[]).await?;
        }
        return Ok(scan);
    }
    if folder.status == "error" {
        // The folder came back — clear the flag before reconciling.
        let ok = Source {
            status: "ready".to_string(),
            error: String::new(),
            ..folder.clone()
        };
        state.db.replace_source(&ok, &[], &[]).await?;
    }

    let all_sources = state.db.list_sources(&folder.notebook_id).await?;
    // A file already in the notebook some other way — added individually, or
    // owned by an overlapping folder source — is not this folder's to ingest.
    let claimed: HashSet<&str> = all_sources
        .iter()
        .filter(|s| s.parent_id != folder.id && s.id != folder.id && !s.url.is_empty())
        .map(|s| s.url.as_str())
        .collect();
    let children: Vec<&Source> = all_sources
        .iter()
        .filter(|s| s.parent_id == folder.id)
        .collect();
    let outcome = scan_folder(root);
    let mut on_disk = outcome.entries;
    let mut skipped = outcome.skipped;
    // Fire-and-forget: ask iCloud to download this pass's eviction stubs so the
    // next resync (60s) ingests them. `brctl` returns immediately — bird does
    // the transfer in the background — and we reap in a detached blocking task
    // so no zombies pile up across the app's lifetime.
    #[cfg(target_os = "macos")]
    if !outcome.icloud_stubs.is_empty() {
        let stubs = outcome.icloud_stubs;
        tokio::task::spawn_blocking(move || {
            for stub in stubs {
                let _ = std::process::Command::new("brctl")
                    .arg("download")
                    .arg(&stub)
                    .status();
            }
        });
    }
    on_disk.retain(|e| !claimed.contains(e.path.as_str()));
    // The include ladder (RFC-git-sources §1): a "Docs" source lists prose
    // only — code is out of scope entirely, not merely unembedded.
    if folder.source_type == "git"
        && crate::git::read_include(&app_data_dir(state), &folder.id).as_deref() == Some("docs")
    {
        on_disk.retain(|e| !ingest::is_code_path(&e.path));
    }
    let by_path: HashMap<&str, &Source> = children.iter().map(|c| (c.url.as_str(), *c)).collect();

    // The tier decision (RFC-git-sources §4): document-sized scopes embed
    // everything; repository-sized scopes embed the knowledge layer (prose,
    // the map) while code children store content only — the ripgrep leg
    // reaches them at query time, and at rest they cost nothing.
    let repo_tier = on_disk.len() > REPO_TIER_FILES;
    let rel_of = |p: &str| {
        std::path::Path::new(p)
            .strip_prefix(root)
            .map(|r| r.to_string_lossy().to_string())
            .unwrap_or_else(|_| p.to_string())
    };
    // Per-file promote/demote overrides (repo reader) beat the tier rule.
    let overrides = load_embed_overrides(&app_data_dir(state), &folder.id);
    let embed_file = |path: &str| {
        let rel = rel_of(path);
        if overrides.embed.iter().any(|r| r == &rel) {
            return true;
        }
        if overrides.unembed.iter().any(|r| r == &rel) {
            return false;
        }
        !repo_tier || !ingest::is_code_path(path)
    };
    // Repository-tier images are almost always assets (icons, logos) — OCR
    // noise, tree noise. Diagrams under docs/ keep their OCR value.
    if repo_tier {
        let mut kept = Vec::with_capacity(on_disk.len());
        for e in on_disk {
            let rel = rel_of(&e.path);
            if ingest::is_image(&e.path) && !rel.starts_with("docs/") && !rel.contains("/docs/") {
                skipped.push((rel, "image asset".to_string()));
            } else {
                kept.push(e);
            }
        }
        on_disk = kept;
        skipped.sort();
    }

    // Decide the work list up front so progress events get a meaningful total.
    // An evicted file next to a ready child is NOT work: the text we embedded
    // before eviction is still good, and reading the file would force a
    // download the user didn't ask for.
    let needs_action = |entry: &ScanEntry| match by_path.get(entry.path.as_str()) {
        None => true,
        Some(c) if c.status == "placeholder" => !entry.placeholder,
        Some(c) => !entry.placeholder && c.mtime != entry.mtime,
    };
    let work: Vec<&ScanEntry> = on_disk.iter().filter(|e| needs_action(e)).collect();
    let total = work.len() as u32;

    for (done, entry) in work.iter().enumerate() {
        let path = entry.path.as_str();
        let mtime = entry.mtime;
        if let Some(app) = app {
            let _ = app.emit(
                "folder://progress",
                FolderProgress {
                    done: done as u32,
                    total,
                    title: ingest::file_title(path),
                },
            );
        }
        match by_path.get(path) {
            // New but not downloaded — list it, label it, embed nothing.
            None if entry.placeholder => {
                store_placeholder_child(state, folder, path, mtime).await?;
                scan.added += 1;
            }
            // New file — full ingest as a child of this folder.
            None => match extract_any_file(state, path).await {
                Ok(mut extracted) => {
                    let settled = friendly_title_fast(&mut extracted);
                    let ctx = code_context(&folder.title, root, path);
                    let src = store_new_source(
                        state,
                        &folder.notebook_id,
                        extracted,
                        &folder.id,
                        mtime,
                        ctx.as_deref(),
                        embed_file(path),
                    )
                    .await?;
                    if !settled {
                        spawn_retitle(state, &src).await;
                    }
                    scan.added += 1;
                }
                Err(err) => {
                    store_failed_child(state, folder, path, mtime, err.to_string()).await?;
                    scan.failed += 1;
                }
            },
            // A placeholder's bytes arrived, or a real file changed — read and
            // (re-)embed in place.
            Some(child) => match extract_any_file(state, path).await {
                Ok(mut extracted) => {
                    let mut existing = (*child).clone();
                    existing.mtime = mtime;
                    let mut retitle = false;
                    if existing.status == "placeholder" {
                        // First real read of this file — give it a real title.
                        retitle = !friendly_title_fast(&mut extracted);
                    } else {
                        // Keep the stored title: the content changed, not the
                        // file. (A failed child keeps its filename title.)
                        extracted.title = existing.title.clone();
                    }
                    let ctx = code_context(&folder.title, root, path);
                    let src = reingest(
                        state,
                        &existing,
                        extracted,
                        ctx.as_deref(),
                        embed_file(path),
                    )
                    .await?;
                    if retitle {
                        spawn_retitle(state, &src).await;
                    }
                    scan.updated += 1;
                }
                Err(err) if child.status == "placeholder" => {
                    // The bytes arrived but extraction failed — there's no
                    // embedded text to protect, so show the real failure.
                    let failed = Source {
                        status: "error".to_string(),
                        error: err.to_string(),
                        mtime,
                        ..(*child).clone()
                    };
                    state.db.replace_source(&failed, &[], &[]).await?;
                    scan.failed += 1;
                }
                Err(err) => {
                    // Don't wipe the working text over a failed re-read; bump
                    // the mtime so the file isn't re-attempted every minute.
                    state.db.set_source_mtime(&child.id, mtime).await?;
                    crate::note!("folder rescan: failed to re-read {path}: {err:#}");
                    scan.failed += 1;
                }
            },
        }
    }

    if total > 0 {
        // Final tick so the UI can clear its progress indicator.
        if let Some(app) = app {
            let _ = app.emit(
                "folder://progress",
                FolderProgress {
                    done: total,
                    total,
                    title: String::new(),
                },
            );
        }
    }

    // Files that disappeared from disk take their sources with them.
    let disk_paths: HashSet<&str> = on_disk.iter().map(|e| e.path.as_str()).collect();
    for child in &children {
        if !disk_paths.contains(child.url.as_str()) {
            state.db.delete_source(&child.id).await?;
            scan.removed += 1;
        }
    }

    // The parent's content is a folder/repo map: git provenance (when the
    // root sits in a working tree), the file tree, and the skip list — so
    // nothing the scan left out is silently absent. Rendering is cheap; the
    // git subprocesses are gated to changes, first scans, manual refreshes,
    // and a 15-minute provenance probe.
    if scan.changed() || force_map || folder.char_count == 0 || crate::git::probe_due(&folder.id) {
        let repo = crate::git::detect_repo(root).await;
        let files: Vec<crate::git::MapFile> = on_disk
            .iter()
            .map(|e| crate::git::MapFile {
                rel: std::path::Path::new(&e.path)
                    .strip_prefix(root)
                    .map(|r| r.to_string_lossy().to_string())
                    .unwrap_or_else(|_| e.path.clone()),
                ingested: !e.placeholder,
                outline: String::new(),
            })
            .collect();
        // Symbol outlines (RFC-git-sources §5): parse code files with the
        // bundled tree-sitter grammars so definitions stay retrievable by
        // name through the embedded map — even for grep-tier files that
        // never embed themselves. Bounded, and off the async runtime.
        let files = {
            let root_owned = root.to_path_buf();
            tokio::task::spawn_blocking(move || {
                let mut files = files;
                let mut outlined = 0usize;
                for f in files.iter_mut() {
                    if outlined >= 300 || !ingest::is_code_path(&f.rel) {
                        continue;
                    }
                    let abs = root_owned.join(&f.rel);
                    if let Ok(src) = std::fs::read_to_string(&abs) {
                        f.outline = crate::outline::suffix(&crate::outline::outline(&f.rel, &src));
                        outlined += 1;
                    }
                }
                files
            })
            .await
            .unwrap_or_default()
        };
        let map = crate::git::render_map(
            &folder.title,
            repo.as_ref(),
            root,
            &files,
            &skipped,
            if repo_tier {
                on_disk
                    .iter()
                    .filter(|e| ingest::is_code_path(&e.path))
                    .count()
            } else {
                0
            },
        );
        let current = state
            .db
            .source_content(&folder.id)
            .await
            .unwrap_or_default();
        if map != current {
            let extracted = ingest::Extracted {
                image_url: String::new(),
                author: String::new(),
                title: folder.title.clone(),
                source_type: folder.source_type.clone(),
                url: folder.url.clone(),
                text: map,
            };
            let fresh = Source {
                status: "ready".to_string(),
                error: String::new(),
                ..folder.clone()
            };
            reingest(state, &fresh, extracted, None, true).await?;
        }
    }

    if scan.changed() {
        state.db.touch_notebook(&folder.notebook_id, now()).await?;
    }
    Ok(scan)
}

/// A cloud-storage sync root the user can pick a subfolder from. `provider` is
/// a stable machine key ("google_drive", "onedrive", "box", "dropbox",
/// "icloud"); `label` is the display name; `path` is the root on disk.
#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CloudFolder {
    provider: String,
    label: String,
    path: String,
}

/// Cloud-storage sync roots that exist on this machine — Google Drive,
/// OneDrive, Box, Dropbox, and iCloud Drive — so "Add folder" can open the
/// native picker already inside one and the user drills down to a subfolder
/// (never the whole drive). macOS mounts most providers under
/// ~/Library/CloudStorage (File Provider); older clients drop ~/Dropbox and
/// ~/Box (often symlinks into CloudStorage, deduped by canonical path); iCloud
/// lives under ~/Library/Mobile Documents.
#[tauri::command]
pub fn list_cloud_folders() -> Vec<CloudFolder> {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return Vec::new();
    };
    detect_cloud_folders(&home)
}

/// Pure detection over a home directory, so tests can drive it with a temp dir.
fn detect_cloud_folders(home: &std::path::Path) -> Vec<CloudFolder> {
    let mut out: Vec<CloudFolder> = Vec::new();
    let mut seen: HashSet<std::path::PathBuf> = HashSet::new();
    let mut add = |provider: &str, label: &str, path: std::path::PathBuf| {
        if !path.is_dir() {
            return;
        }
        // Dedupe symlinked/duplicate roots (e.g. ~/Dropbox -> CloudStorage).
        let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(canon) {
            return;
        }
        out.push(CloudFolder {
            provider: provider.to_string(),
            label: label.to_string(),
            path: path.to_string_lossy().into_owned(),
        });
    };

    // File Provider mounts (macOS 12+): one dir per connected account.
    let cloud = home.join("Library/CloudStorage");
    if let Ok(rd) = std::fs::read_dir(&cloud) {
        let mut names: Vec<String> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort(); // stable order across launches
        for name in names {
            let provider = if name.starts_with("GoogleDrive-") {
                Some(("google_drive", "Google Drive"))
            } else if name.starts_with("OneDrive") {
                Some(("onedrive", "OneDrive"))
            } else if name == "Box" || name.starts_with("Box-") {
                Some(("box", "Box"))
            } else if name.starts_with("Dropbox") {
                Some(("dropbox", "Dropbox"))
            } else {
                None
            };
            if let Some((key, label)) = provider {
                add(key, label, cloud.join(&name));
            }
        }
    }

    // Legacy top-level sync folders from older desktop clients.
    add("dropbox", "Dropbox", home.join("Dropbox"));
    add("box", "Box", home.join("Box"));
    // iCloud Drive.
    add(
        "icloud",
        "iCloud Drive",
        home.join("Library/Mobile Documents/com~apple~CloudDocs"),
    );

    out
}

#[tauri::command]
pub async fn add_source_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: String,
    path: String,
) -> Result<Source, String> {
    let root = std::path::Path::new(&path);
    if !root.is_dir() {
        return Err(format!("Not a folder: {path}"));
    }
    let _guard = state.folder_scan_lock.lock().await;
    for s in e(state.db.list_sources(&notebook_id).await)? {
        if matches!(s.source_type.as_str(), "folder" | "obsidian") && s.url == path {
            return Err(format!(
                "Folder already added as \"{}\" — it refreshes automatically",
                s.title
            ));
        }
    }
    let title = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Folder")
        .to_string();
    // An `.obsidian/` config dir marks the folder as an Obsidian vault
    // (RFC-obsidian-notion §3): same folder machinery, distinct identity, and
    // the reader renders its wikilinks as hops.
    let source_type = if root.join(".obsidian").is_dir() {
        "obsidian"
    } else {
        "folder"
    };
    let folder = Source {
        image_url: String::new(),
        author: String::new(),
        id: new_id(),
        notebook_id: notebook_id.clone(),
        title,
        source_type: source_type.to_string(),
        url: path,
        content: String::new(),
        char_count: 0,
        chunk_count: 0,
        created_at: now(),
        status: "ready".to_string(),
        error: String::new(),
        parent_id: String::new(),
        mtime: 0,
        tags: String::new(),
        note: String::new(),
        fetched_at: now(),
        fetch_failures: 0,
    };
    e(state.db.insert_source(&folder, &[], &[]).await)?;
    e(rescan_one_folder(Some(&app), &state, &folder, true).await)?;
    e(state.db.touch_notebook(&notebook_id, now()).await)?;
    Ok(folder)
}

/// Payload for `sources://changed` — a background rescan altered a notebook's
/// sources, so any window showing it should reload its list.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SourcesChanged {
    notebook_id: String,
    #[serde(flatten)]
    scan: FolderScan,
}

/// Index any notes missing from the retrieval index — notes from before
/// phase 1 of docs/RFC-note-curator.md, or whose write-time indexing failed.
/// Runs once per app launch, on the first minute tick.
async fn backfill_note_index(state: &AppState) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let (notes, indexed) = match tokio::try_join!(
        state.db.recent_notes(usize::MAX),
        state.db.indexed_note_ids()
    ) {
        Ok(pair) => pair,
        Err(err) => {
            crate::note!("note backfill: listing failed: {err:#}");
            return;
        }
    };
    for note in notes {
        if note.kind != "audio_overview" && note.status != "archived" && !indexed.contains(&note.id)
        {
            index_note(state, &note).await;
        }
    }
}

/// One-shot per launch: collapse each schedule's timestamped report notes
/// ("{name} — 2026-07-13 09:00", one per run, from before reports became
/// living notes) into a single stable-titled note. Newest content wins.
async fn collapse_old_report_piles(state: &AppState) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let schedules = match state.db.all_report_schedules().await {
        Ok(s) => s,
        Err(err) => {
            crate::note!("report collapse: listing schedules failed: {err:#}");
            return;
        }
    };
    for s in schedules {
        if let Err(err) = collapse_report_notes(state, &s.notebook_id, &s.name).await {
            crate::note!("report collapse for \"{}\" failed: {err:#}", s.name);
        }
    }
}

// ---- Note curator (docs/RFC-note-curator.md phase 4) -----------------------

/// Staleness thresholds in APP-OPEN days — days the app actually ran, not
/// wall days — so a month away from the machine doesn't archive everything.
const CURATOR_STALE_OPEN_DAYS: usize = 30;
const CURATOR_ARCHIVE_OPEN_DAYS: usize = 90;

/// One curator state change, for the report note and the caller's reindex.
pub struct CuratorAction {
    pub notebook_id: String,
    pub note_id: String,
    pub title: String,
    /// "stale" | "archived" | "revived"
    pub action: &'static str,
}

fn day_of(ms: i64) -> i64 {
    ms.div_euclid(86_400_000)
}

/// The deterministic curator pass: walk `origin: "auto"` notes, count the
/// app-open days since each was last used, and transition status — active →
/// stale → archived (chunks dropped from retrieval; the note itself is never
/// deleted), with any use reviving. No model calls; pure DB so tests can
/// drive it with a fabricated open-day history.
pub async fn curate_notes(db: &Db, open_days: &[i64]) -> anyhow::Result<Vec<CuratorAction>> {
    let usage: HashMap<String, i64> = db
        .note_usage()
        .await?
        .into_iter()
        .map(|u| (u.note_id, u.last_used_at))
        .collect();
    let mut actions = Vec::new();
    for note in db.recent_notes(usize::MAX).await? {
        if note.origin != "auto" {
            continue;
        }
        // A note's own update counts as use, so fresh notes start at zero.
        let last_use = usage
            .get(&note.id)
            .copied()
            .unwrap_or(0)
            .max(note.updated_at);
        let unused = open_days.iter().filter(|d| **d > day_of(last_use)).count();
        let action = match note.status.as_str() {
            "" | "stale" if unused >= CURATOR_ARCHIVE_OPEN_DAYS => "archived",
            "" if unused >= CURATOR_STALE_OPEN_DAYS => "stale",
            "stale" | "archived" if unused == 0 => "revived",
            _ => continue,
        };
        match action {
            "archived" => {
                db.set_note_status(&note.id, "archived").await?;
                db.delete_note_chunks(&note.id).await?;
            }
            "stale" => db.set_note_status(&note.id, "stale").await?,
            _ => db.set_note_status(&note.id, "").await?,
        }
        actions.push(CuratorAction {
            notebook_id: note.notebook_id.clone(),
            note_id: note.id.clone(),
            title: note.title.clone(),
            action,
        });
    }
    Ok(actions)
}

/// Last user-initiated action (chat, generation, opening a note). The
/// consolidation pass rewrites content and spends tokens, so it only runs
/// when the user has been away a while; the deterministic pass doesn't care.
static LAST_ACTIVITY_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub(crate) fn touch_activity() {
    LAST_ACTIVITY_MS.store(now(), std::sync::atomic::Ordering::Relaxed);
}

fn idle_ms() -> i64 {
    let last = LAST_ACTIVITY_MS.load(std::sync::atomic::Ordering::Relaxed);
    // No activity since launch = idle (nothing in flight to disturb).
    if last == 0 {
        i64::MAX
    } else {
        now() - last
    }
}

/// Cosine similarity; 0 for degenerate vectors.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Index pairs whose vectors clear `threshold`, most similar first, each
/// index used at most once — consolidation candidates.
fn similar_pairs(embeds: &[Vec<f32>], threshold: f32) -> Vec<(usize, usize)> {
    let mut scored = Vec::new();
    for i in 0..embeds.len() {
        for j in (i + 1)..embeds.len() {
            let s = cosine(&embeds[i], &embeds[j]);
            if s >= threshold {
                scored.push(((i, j), s));
            }
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut used = HashSet::new();
    let mut out = Vec::new();
    for ((i, j), _) in scored {
        if used.contains(&i) || used.contains(&j) {
            continue;
        }
        used.insert(i);
        used.insert(j);
        out.push((i, j));
    }
    out
}

/// The LLM consolidation pass (phase 5, off by default): auto evidence
/// records whose TITLES embed similarly are candidate duplicates; the chat
/// model judges each pair (KEEP is the instructed default) and writes the
/// merged record. The older note wins — stable id, existing citations keep
/// pointing at it — and the newer is archived, never deleted. At most 3
/// merges per notebook per run: a bad week stays small, and next week's run
/// catches the rest.
async fn consolidate_notes(state: &AppState) -> anyhow::Result<Vec<CuratorAction>> {
    let mut actions = Vec::new();
    for nb in state.db.list_notebooks().await? {
        let evid: Vec<Note> = state
            .db
            .list_notes(&nb.id)
            .await?
            .into_iter()
            .filter(|n| n.kind == "evidence" && n.origin == "auto" && n.status != "archived")
            .collect();
        if evid.len() < 2 {
            continue;
        }
        let titles: Vec<String> = evid.iter().map(|n| n.title.clone()).collect();
        let embeds = {
            let ai = state.ai.read().await.clone();
            ai.embed(&titles).await?
        };
        let mut pairs = similar_pairs(&embeds, 0.75);
        pairs.truncate(3);
        for (i, j) in pairs {
            let (a, b) = (&evid[i], &evid[j]);
            let out = {
                let messages =
                    rag::build_consolidate_messages(&a.title, &a.content, &b.title, &b.content);
                let ai = state.ai.read().await.clone();
                ai.chat(&messages).await?.text
            };
            let Some((title, body)) = rag::parse_auto_evidence(&out) else {
                continue; // KEEP — distinct claims
            };
            let (winner, loser) = if a.created_at <= b.created_at {
                (a, b)
            } else {
                (b, a)
            };
            state
                .db
                .update_note(&winner.id, &title, &body, now())
                .await?;
            state.db.set_note_status(&winner.id, "").await?;
            if let Some(n) = state.db.get_note(&winner.id).await? {
                index_note(state, &n).await;
            }
            state.db.set_note_status(&loser.id, "archived").await?;
            state.db.delete_note_chunks(&loser.id).await?;
            actions.push(CuratorAction {
                notebook_id: nb.id.clone(),
                note_id: winner.id.clone(),
                title: format!("\"{}\" merged into \"{title}\"", loser.title),
                action: "merged",
            });
        }
    }
    Ok(actions)
}

/// Curator bookkeeping, one JSON file next to the config:
/// `{"lastRunAt": ms, "lastConsolidateAt": ms, "openDays": [day numbers]}`.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct CuratorState {
    #[serde(default)]
    last_run_at: i64,
    #[serde(default)]
    last_consolidate_at: i64,
    #[serde(default)]
    open_days: Vec<i64>,
}

/// Rides the minute tick: records today as an app-open day, and at most
/// once a week runs the deterministic pass, reindexes revived notes, and
/// updates one living "Curator report" note per affected notebook.
async fn note_curator_tick(app: &AppHandle, state: &AppState) {
    use std::sync::atomic::{AtomicI64, Ordering};
    static LAST_DAY_SEEN: AtomicI64 = AtomicI64::new(0);
    let today = day_of(now());
    let path = state.config_path.with_file_name("curator.json");
    let mut cur: CuratorState = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    if LAST_DAY_SEEN.swap(today, Ordering::SeqCst) != today && !cur.open_days.contains(&today) {
        cur.open_days.push(today);
        // Only the archive window's worth of history matters.
        let keep = CURATOR_ARCHIVE_OPEN_DAYS * 2;
        if cur.open_days.len() > keep {
            let drop = cur.open_days.len() - keep;
            cur.open_days.drain(..drop);
        }
        let _ = std::fs::write(&path, serde_json::to_string(&cur).unwrap_or_default());
    }

    const WEEK_MS: i64 = 7 * 86_400_000;
    let mut actions: Vec<CuratorAction> = Vec::new();

    // Deterministic pass: free, so it never needs an idle gate.
    if now() - cur.last_run_at >= WEEK_MS {
        cur.last_run_at = now();
        let _ = std::fs::write(&path, serde_json::to_string(&cur).unwrap_or_default());
        match curate_notes(&state.db, &cur.open_days).await {
            Ok(a) => actions.extend(a),
            Err(err) => crate::note!("note curator failed: {err:#}"),
        }
        // Revived notes need their chunks back in the index.
        for a in actions.iter().filter(|a| a.action == "revived") {
            if let Ok(Some(note)) = state.db.get_note(&a.note_id).await {
                index_note(state, &note).await;
            }
        }
    }

    // LLM consolidation (phase 5): opt-in, and only when the user has been
    // away — it spends tokens and rewrites content. Its own weekly stamp, so
    // a busy week just defers it to the next quiet tick.
    const CONSOLIDATE_IDLE_MS: i64 = 30 * 60 * 1000;
    let consolidate_on = { state.ai.read().await.config().curator_consolidate };
    if consolidate_on
        && idle_ms() >= CONSOLIDATE_IDLE_MS
        && now() - cur.last_consolidate_at >= WEEK_MS
    {
        cur.last_consolidate_at = now();
        let _ = std::fs::write(&path, serde_json::to_string(&cur).unwrap_or_default());
        match consolidate_notes(state).await {
            Ok(a) => actions.extend(a),
            Err(err) => crate::note!("note consolidation failed: {err:#}"),
        }
    }

    if actions.is_empty() {
        return;
    }

    // One living report note per affected notebook, updated in place so the
    // curator never generates its own silt.
    let mut by_notebook: HashMap<&str, Vec<&CuratorAction>> = HashMap::new();
    for a in &actions {
        by_notebook.entry(&a.notebook_id).or_default().push(a);
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d").to_string();
    for (notebook_id, acts) in by_notebook {
        let mut body = format!(
            "# Curator report\n\n_Last run {stamp}. The curator manages auto-created \
             evidence notes only: unused for ~{CURATOR_STALE_OPEN_DAYS} app-open days → stale \
             (dimmed), ~{CURATOR_ARCHIVE_OPEN_DAYS} → archived (out of retrieval, never \
             deleted). Merged records absorb a same-claim sibling, which is archived. \
             Using or editing a note revives it._\n\n"
        );
        for a in &acts {
            body.push_str(&format!("- **{}**: {}\n", a.action, a.title));
        }
        let existing = state
            .db
            .list_notes(notebook_id)
            .await
            .ok()
            .and_then(|notes| notes.into_iter().find(|n| n.title == "Curator report"));
        let result = match existing {
            Some(n) => {
                state
                    .db
                    .update_note(&n.id, "Curator report", &body, now())
                    .await
            }
            None => {
                let ts = now();
                state
                    .db
                    .add_note(&Note {
                        id: new_id(),
                        notebook_id: notebook_id.to_string(),
                        title: "Notebook housekeeping".into(),
                        content: body,
                        kind: "note".into(),
                        prompt: String::new(),
                        origin: String::new(),
                        status: String::new(),
                        created_at: ts,
                        updated_at: ts,
                    })
                    .await
            }
        };
        if let Err(err) = result {
            crate::note!("curator report for {notebook_id} failed: {err:#}");
        }
        #[derive(serde::Serialize, Clone)]
        #[serde(rename_all = "camelCase")]
        struct Changed<'a> {
            scope: &'a str,
            notebook_id: Option<&'a str>,
        }
        let _ = app.emit(
            "mcp://changed",
            Changed {
                scope: "notes",
                notebook_id: Some(notebook_id),
            },
        );
    }
    crate::note!("note curator: {} action(s)", actions.len());
}

/// Rescan every folder source and re-embed loose file sources whose on-disk
/// file changed (the resident scheduler ticks this once a minute —
/// scheduler.rs — and the frontend calls it on notebook open). Emits
/// `sources://changed` per notebook that actually changed. Missing files
/// never remove a loose source — uploads are snapshots; the origin path is
/// only a refresh hint.
#[tauri::command]
pub async fn resync_sources(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: Option<String>,
) -> Result<FolderScan, String> {
    resync_sources_inner(&app, &state, notebook_id.as_deref()).await
}

/// The command's body, callable from the resident scheduler (scheduler.rs)
/// with no Tauri `State` wrapper in sight. `only_notebook` scopes the sweep
/// to one notebook's sources — the notebook-open catch-up used to rescan
/// the entire corpus right as a notebook was loading, in a race the
/// 60-second scheduler tick was about to run anyway.
pub(crate) async fn resync_sources_inner(
    app: &AppHandle,
    state: &AppState,
    only_notebook: Option<&str>,
) -> Result<FolderScan, String> {
    let app = app.clone();
    // The Spotlight index rides the same tick (internally ~10-min throttled).
    #[cfg(target_os = "macos")]
    crate::spotlight::refresh_if_due(state).await;
    // One-shot per app run: index notes written before notes joined the
    // retrieval index (or whose indexing failed at write time).
    backfill_note_index(state).await;
    // One-shot per app run: collapse timestamped report piles from before
    // reports became living notes (one note per schedule, newest wins).
    collapse_old_report_piles(state).await;
    // Curator: track app-open days; runs its pass at most weekly.
    note_curator_tick(&app, state).await;
    // A manual folder add/refresh is already scanning — skip this tick rather
    // than queue behind it and ingest the same files twice.
    let Ok(_guard) = state.folder_scan_lock.try_lock() else {
        return Ok(FolderScan::default());
    };
    let mut total = FolderScan::default();
    let mut per_notebook: HashMap<String, FolderScan> = HashMap::new();
    // Archived notebooks sit out background refreshes entirely (their
    // sources stay frozen); unarchiving resumes them on the next tick.
    let archived = state.db.archived_notebook_ids().await.unwrap_or_default();
    // The auto-sync cadence setting: minutes between remote probes, 0 = off
    // (manual Refresh still syncs).
    let sync_minutes = { state.ai.read().await.config().git_sync_minutes };
    for folder in e(state.db.all_folder_sources().await)? {
        if archived.contains(&folder.notebook_id) {
            continue;
        }
        if only_notebook.is_some_and(|nb| nb != folder.notebook_id) {
            continue;
        }
        // Remote repos: one cheap ls-remote per cadence tick; a moved branch
        // refetches the cache so the ordinary rescan below sees fresh
        // mtimes. Never runs against user repos — only our own clones.
        if folder.source_type == "git"
            && sync_minutes > 0
            && crate::git::remote_probe_due(&folder.id, sync_minutes)
        {
            let dir = crate::git::cache_dir(&app_data_dir(state), &folder.id);
            match crate::git::sync_remote(&dir).await {
                Ok(Some(sha)) => {
                    let stamp = crate::mac::content_stamp(&sha);
                    let _ = state.db.set_source_mtime(&folder.id, stamp).await;
                }
                Ok(None) => {}
                Err(err) => crate::note!("git resync: {} failed: {err:#}", folder.url),
            }
        }
        // Notion parents: re-export changed pages per cadence tick; the
        // rescan below re-embeds only rewritten files. remote_probe_due is
        // a generic per-source-id throttle, shared with git.
        if folder.source_type == "notion"
            && sync_minutes > 0
            && crate::git::remote_probe_due(&folder.id, sync_minutes)
        {
            let token = { state.ai.read().await.config().notion_token.clone() };
            if let (Some(page_id), false) =
                (crate::notion::detect_page(&folder.url), token.is_empty())
            {
                let dir = crate::notion::cache_dir(&app_data_dir(state), &folder.id);
                match crate::notion::NotionClient::new(&token)
                    .export_tree(&page_id, &dir)
                    .await
                {
                    Ok(stats) if stats.pages > 0 => {
                        let _ = state
                            .db
                            .set_source_mtime(&folder.id, stats.max_edited_ms)
                            .await;
                    }
                    Ok(_) => {}
                    Err(err) => crate::note!("notion resync: {} failed: {err:#}", folder.url),
                }
            }
        }
        match rescan_one_folder(Some(&app), state, &folder, false).await {
            Ok(scan) => {
                per_notebook
                    .entry(folder.notebook_id.clone())
                    .or_default()
                    .absorb(scan);
                total.absorb(scan);
            }
            Err(err) => {
                crate::note!("folder rescan: {} failed: {err:#}", folder.url);
                total.failed += 1;
            }
        }
    }

    // Loose file sources (added or dropped individually) re-embed when their
    // file changes. Deleted files leave the source untouched; cloud-evicted
    // files aren't read (that would force a download).
    let data_dir = app_data_dir(state);
    for src in e(state.db.all_loose_sources().await)? {
        if archived.contains(&src.notebook_id) {
            continue;
        }
        if only_notebook.is_some_and(|nb| nb != src.notebook_id) {
            continue;
        }
        // Git-backed singles (README/blob) sync hourly from their cache
        // clone. The cache dir is the definitive marker — plain page
        // captures of github.com URLs parse git-shaped too, but have none.
        if crate::git::cache_dir(&data_dir, &src.id).exists() {
            if sync_minutes == 0 || !crate::git::remote_probe_due(&src.id, sync_minutes) {
                continue;
            }
            let dir = crate::git::cache_dir(&data_dir, &src.id);
            match crate::git::sync_remote(&dir).await {
                Ok(Some(sha)) => {
                    let scan = per_notebook.entry(src.notebook_id.clone()).or_default();
                    match reextract_git_single(state, &src, &sha).await {
                        Ok(_) => {
                            scan.updated += 1;
                            total.updated += 1;
                        }
                        Err(err) => {
                            crate::note!("git resync: failed to re-embed {}: {err:#}", src.url);
                            scan.failed += 1;
                            total.failed += 1;
                        }
                    }
                }
                Ok(None) => {}
                Err(err) => crate::note!("git resync: {} failed: {err:#}", src.url),
            }
            continue;
        }
        if src.url.is_empty() || is_web_url(&src.url) {
            continue;
        }
        // Mac items re-fetch on their own gentler cadence (osascript-backed);
        // re-embed only when the content hash moved.
        if crate::mac::is_mac_uri(&src.url) {
            if !crate::mac::sweep_due(&src.id) {
                continue;
            }
            match crate::mac::fetch(&src.url).await {
                Ok((_, text)) => {
                    let stamp = crate::mac::content_stamp(&text);
                    if stamp == src.mtime {
                        continue;
                    }
                    let mut existing = src.clone();
                    existing.mtime = stamp;
                    let extracted = ingest::Extracted {
                        image_url: String::new(),
                        author: String::new(),
                        title: existing.title.clone(),
                        source_type: "mac".to_string(),
                        url: existing.url.clone(),
                        text,
                    };
                    let scan = per_notebook.entry(src.notebook_id.clone()).or_default();
                    match reingest(state, &existing, extracted, None, true).await {
                        Ok(_) => {
                            scan.updated += 1;
                            total.updated += 1;
                        }
                        Err(err) => {
                            crate::note!("mac resync: failed to re-embed {}: {err:#}", src.url);
                            scan.failed += 1;
                            total.failed += 1;
                        }
                    }
                }
                Err(err) => {
                    // Keep the working text; permission prompts and closed
                    // apps are transient. The cadence gate throttles retries.
                    crate::note!("mac resync: failed to fetch {}: {err:#}", src.url);
                }
            }
            continue;
        }
        let path = std::path::Path::new(&src.url);
        let Ok(meta) = std::fs::metadata(path) else {
            continue; // file gone — the snapshot stays
        };
        if is_evicted(&meta) {
            continue;
        }
        let mtime = file_mtime(path);
        if mtime == src.mtime {
            continue;
        }
        if src.mtime == 0 {
            // Source predates mtime tracking — adopt the current mtime quietly
            // instead of re-embedding the whole back catalog on first sweep.
            e(state.db.set_source_mtime(&src.id, mtime).await)?;
            continue;
        }
        let scan = per_notebook.entry(src.notebook_id.clone()).or_default();
        match extract_any_file(state, &src.url).await {
            Ok(mut extracted) => {
                let mut existing = src.clone();
                existing.mtime = mtime;
                // Content changed, not the file's name — keep the stored title.
                extracted.title = existing.title.clone();
                match reingest(state, &existing, extracted, None, true).await {
                    Ok(_) => {
                        scan.updated += 1;
                        total.updated += 1;
                    }
                    Err(err) => {
                        crate::note!("file resync: failed to re-embed {}: {err:#}", src.url);
                        scan.failed += 1;
                        total.failed += 1;
                    }
                }
            }
            Err(err) => {
                // Keep the working text; bump the mtime so a broken file isn't
                // re-attempted every minute.
                e(state.db.set_source_mtime(&src.id, mtime).await)?;
                crate::note!("file resync: failed to re-read {}: {err:#}", src.url);
                scan.failed += 1;
                total.failed += 1;
            }
        }
    }

    for (notebook_id, scan) in per_notebook {
        if scan.changed() {
            let _ = app.emit("sources://changed", SourcesChanged { notebook_id, scan });
        }
    }
    Ok(total)
}

#[derive(serde::Serialize, Clone)]
struct MigrateProgress {
    done: u32,
    total: u32,
    title: String,
}

/// Rebuild the entire chunk index using the currently-configured embedding
/// model. Called after switching embedding models (the new model may have a
/// different vector dimension). Emits `migrate://progress` per source.
#[tauri::command]
pub async fn reembed_all(app: AppHandle, state: State<'_, AppState>) -> Result<u32, String> {
    let sources = e(state.db.all_sources().await)?;
    // Carry source_type so re-embedding dispatches the same way ingest does —
    // otherwise code loses its code-aware chunking and vault markdown loses
    // frontmatter stripping (both would silently degrade on a re-embed).
    let owners: Vec<(String, String, ingest::Extracted)> = sources
        .iter()
        .map(|s| {
            (
                s.notebook_id.clone(),
                s.id.clone(),
                ingest::Extracted {
                    image_url: String::new(),
                    author: String::new(),
                    title: s.title.clone(),
                    source_type: s.source_type.clone(),
                    url: s.url.clone(),
                    text: s.content.clone(),
                },
            )
        })
        .collect();
    let total = owners.len() as u32;

    // Drop the old index first so the new (possibly differently-sized) vectors
    // can recreate the table cleanly.
    e(state.db.clear_all_chunks().await)?;

    let ai = state.ai.read().await.clone();
    for (i, (notebook_id, owner_id, extracted)) in owners.iter().enumerate() {
        let _ = app.emit(
            "migrate://progress",
            MigrateProgress {
                done: i as u32,
                total,
                title: extracted.title.clone(),
            },
        );
        // Child files of a folder/repo keep their "parent › path" code context
        // when it can be derived; top-level sources fall back to the title.
        let chunks = ingest::chunk_source(extracted, None);
        if chunks.is_empty() {
            continue;
        }
        let embed_inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
        let embeddings = e(ai.embed(&embed_inputs).await)?;
        let tuples: Vec<(String, i32, String)> = chunks
            .iter()
            .enumerate()
            .map(|(j, c)| (new_id(), j as i32, c.text.clone()))
            .collect();
        e(state
            .db
            .add_chunks(notebook_id, owner_id, &tuples, &embeddings)
            .await)?;
    }

    drop(ai);

    // Notes ride the same chunk table, so the rebuild must re-embed them too
    // (archived notes stay out — the curator dropped them from retrieval).
    for note in e(state.db.recent_notes(usize::MAX).await)? {
        if note.status != "archived" {
            index_note(&state, &note).await;
        }
    }

    let _ = app.emit(
        "migrate://progress",
        MigrateProgress {
            done: total,
            total,
            title: "Done".into(),
        },
    );
    Ok(total)
}

// ---- Chat ----------------------------------------------------------------

#[tauri::command]
pub async fn list_messages(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Vec<Message>, String> {
    e(state.db.list_messages(&notebook_id).await)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub messages: Vec<Message>,
    pub has_more: bool,
}

/// A bounded transcript page for the webview. Backend generation paths still
/// use the complete history; this command keeps notebook open and rendering
/// proportional to what is on screen.
#[tauri::command]
pub async fn list_messages_page(
    state: State<'_, AppState>,
    notebook_id: String,
    before_at: Option<i64>,
    before_id: Option<String>,
    limit: Option<usize>,
) -> Result<MessagePage, String> {
    let (messages, has_more) = e(state
        .db
        .message_page(
            &notebook_id,
            before_at,
            before_id.as_deref(),
            limit.unwrap_or(80),
        )
        .await)?;
    Ok(MessagePage { messages, has_more })
}

#[tauri::command]
pub async fn clear_chat(state: State<'_, AppState>, notebook_id: String) -> Result<(), String> {
    e(state.db.clear_messages(&notebook_id).await)
}

/// Copy a note into the chat as an assistant turn so the user can respond to
/// it and discuss it with the model (history turns reach the model context).
#[tauri::command]
pub async fn add_note_to_chat(
    state: State<'_, AppState>,
    note_id: String,
) -> Result<Message, String> {
    let note = e(state.db.get_note(&note_id).await)?.ok_or_else(|| "Note not found".to_string())?;
    let msg = Message {
        id: new_id(),
        notebook_id: note.notebook_id.clone(),
        role: "assistant".to_string(),
        content: format!("**{}**\n\n{}", note.title, note.content),
        citations: Vec::new(),
        kind: "chat".to_string(),
        model: String::new(),
        created_at: now(),
    };
    e(state.db.add_message(&msg).await)?;
    Ok(msg)
}

#[derive(serde::Serialize, Clone)]
struct TokenEvent {
    content: String,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StepEvent {
    label: String,
    /// A live status line that replaces the previous transient one rather than
    /// growing the trail (see `inference::Step`).
    transient: bool,
}

/// Per-notebook chat configuration sent from the frontend.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatConfig {
    pub style: String,
    pub custom_prompt: String,
    pub length: String,
}

/// Turn the chat config into extra system-prompt guidance.
fn chat_style_instruction(cfg: &ChatConfig) -> String {
    let mut parts: Vec<String> = Vec::new();
    match cfg.style.as_str() {
        "learning" => parts.push(
            "Act as a patient learning guide: explain step by step, define key terms, and build intuition.".into(),
        ),
        "custom" if !cfg.custom_prompt.trim().is_empty() => parts.push(cfg.custom_prompt.trim().into()),
        // Shared voice and writing-standard presets in rag::CHAT_STYLES.
        id => {
            if let Some(text) = rag::style_instructions(id) {
                parts.push(text.into());
            }
        }
    }
    match cfg.length.as_str() {
        // Keep the legacy ids so existing per-notebook localStorage selections
        // survive the clearer Concise / Balanced / Thorough labels.
        "longer" => parts.push(
            "Answer thoroughly. Lead with the conclusion, then explain how the cited evidence supports it, including relevant uncertainty, caveats, and source-supported examples. Do not pad, repeat, or add unsupported background."
                .into(),
        ),
        "shorter" => parts.push(
            "Answer directly. Aim for no more than three short paragraphs or five bullets, unless accuracy, completeness, or the user's request requires more. Include essential evidence, caveats, and citations."
                .into(),
        ),
        _ => {}
    }
    if !parts.is_empty() {
        parts.push(
            "This guidance controls presentation only; it must not change facts, uncertainty, citations, or a format the user explicitly requested."
                .into(),
        );
    }
    parts.join(" ")
}

/// Extract bare http(s) URLs from free text (no regex dependency).
fn extract_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for token in text.split_whitespace() {
        // Trim wrapper punctuation until stable — handles nesting like
        // "(`https://x.com`)," where brackets and sentence marks interleave.
        let mut t = token;
        loop {
            let trimmed = t
                .trim_matches(|c: char| "()[]{}<>,\"'`|".contains(c))
                .trim_end_matches(|c: char| ".,;:!?".contains(c));
            if trimmed == t {
                break;
            }
            t = trimmed;
        }
        if (t.starts_with("http://") || t.starts_with("https://")) && t.len() > 10 {
            urls.push(t.to_string());
        }
    }
    urls.dedup();
    urls
}

/// Heuristic: does this message want the URLs added as sources (vs. just
/// mentioning one in a question)?
fn wants_add_sources(content: &str, urls: &[String]) -> bool {
    let l = content.to_lowercase();
    let has_kw = [
        "add", "import", "ingest", "save", "include", "load", "grab", "attach", "pull in",
    ]
    .iter()
    .any(|k| l.contains(k));
    // Or the message is essentially just the URL(s).
    let mut rest = l.clone();
    for u in urls {
        rest = rest.replace(&u.to_lowercase(), " ");
    }
    let rest_words = rest.split_whitespace().count();
    has_kw || rest_words <= 2
}

/// "Add those/these URLs" — an add request whose URLs live in conversation
/// context (a previous answer or its citations) rather than in this message.
fn wants_add_context_urls(content: &str) -> bool {
    let l = content.to_lowercase();
    let verb = [
        "add", "import", "ingest", "save", "include", "grab", "attach",
    ]
    .iter()
    .any(|k| l.contains(k));
    let noun = [
        "url", "link", "source", "site", "page", "website", "address",
    ]
    .iter()
    .any(|k| l.contains(k));
    let anaphor = [
        "those",
        "these",
        "them",
        "that one",
        "above",
        "mentioned",
        "cited",
        "from the answer",
        "you found",
        "you listed",
    ]
    .iter()
    .any(|k| l.contains(k));
    verb && noun && anaphor
}

/// URLs mentioned in recent conversation — message text and citation snippets,
/// newest first — excluding ones already present as sources.
async fn recent_context_urls(state: &AppState, notebook_id: &str) -> Vec<String> {
    let Ok(history) = state.db.list_messages(notebook_id).await else {
        return vec![];
    };
    let existing: HashSet<String> = state
        .db
        .list_sources(notebook_id)
        .await
        .map(|sources| {
            sources
                .iter()
                .filter(|s| !s.url.is_empty())
                .map(|s| s.url.trim_end_matches('/').to_lowercase())
                .collect()
        })
        .unwrap_or_default();

    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for m in history
        .iter()
        .rev()
        .filter(|m| m.kind != "tool" && m.kind != "error")
        .take(6)
    {
        let texts = std::iter::once(m.content.as_str())
            .chain(m.citations.iter().map(|c| c.snippet.as_str()));
        for text in texts {
            for url in extract_urls(text) {
                let key = url.trim_end_matches('/').to_lowercase();
                if !existing.contains(&key) && seen.insert(key) {
                    urls.push(url);
                }
            }
        }
    }
    urls
}

fn host_of(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url)
        .trim_start_matches("www.")
        .to_string()
}

/// The `models` roster (RFC-conversational-setup phase 1): installed Ollama
/// models (the same `list_models` the OllamaModelPicker reads) plus every
/// configured provider's active model and live readiness. Read-only.
pub(crate) async fn settings_models_report(app: &AppHandle, state: &AppState) -> String {
    let ai = state.ai.read().await.clone();
    let config = ai.config().clone();
    let installed =
        match tokio::time::timeout(std::time::Duration::from_secs(5), ai.list_models()).await {
            Ok(Ok(models)) => Ok(models),
            Ok(Err(err)) => Err(format!("{err:#}")),
            Err(_) => Err("timed out".to_string()),
        };
    let mut providers = Vec::new();
    for entry in &config.providers {
        let (ready, detail) = readiness_for_entry(app, entry, &config)
            .await
            .unwrap_or((false, "probe failed".into()));
        providers.push(crate::selfheal::ProviderStatus {
            label: entry.label.clone(),
            model: entry.chat_model.clone(),
            ready,
            detail,
            is_chat: entry.id == config.chat_provider,
            is_studio: entry.id == config.studio_provider,
        });
    }
    crate::selfheal::format_models_report(&installed, &providers)
}

/// One live `test` probe (RFC-conversational-setup phase 1): readiness grown
/// into evidence. Hard-capped by construction at ONE tiny chat call plus at
/// most ONE embed call per invocation, each under a short timeout — an agent
/// looping `test` can never turn probing into open-ended spend. Never
/// mutates config.
pub(crate) async fn settings_test_report(state: &AppState, target: &str) -> String {
    use crate::selfheal::{ProbeResult, TestTarget};
    let ai = state.ai.read().await.clone();
    let config = ai.config().clone();
    let resolved = match crate::selfheal::resolve_test_target(&config, target) {
        Ok(t) => t,
        Err(msg) => return msg,
    };
    // Engine + display label + the Ollama config for the embed leg, which
    // applies only when the target reaches an Ollama server (the embed
    // model lives there); FM/gateway/agent targets get the chat leg only.
    let (engine, label, embed_cfg) = match &resolved {
        TestTarget::Provider(id) => match ai.engine_for_provider(id) {
            Ok((engine, model)) => {
                let entry = config.provider_by_id(id);
                let label = entry
                    .map(|p| {
                        if p.chat_model.trim().is_empty() {
                            p.label.clone()
                        } else {
                            format!("{} · {}", p.label, p.chat_model.trim())
                        }
                    })
                    .unwrap_or(model);
                let embed = entry.filter(|p| p.kind == "ollama").map(|p| {
                    let mut oc = crate::ai::ollama_config(&config);
                    if !p.base_url.trim().is_empty() {
                        oc.base_url = p.base_url.trim().to_string();
                    }
                    // Carry the effective chat model so a timeout can ask
                    // /api/ps whether THAT model is mid-load.
                    if !p.chat_model.trim().is_empty() {
                        oc.chat_model = p.chat_model.trim().to_string();
                    }
                    oc
                });
                (engine, label, embed)
            }
            Err(err) => return friendly_error(&format!("{err:#}")),
        },
        TestTarget::OllamaModel(name) => {
            let mut oc = crate::ai::ollama_config(&config);
            oc.chat_model = name.clone();
            (
                crate::inference::ChatEngine::Ollama(crate::ai::Ollama::new(oc.clone())),
                format!("Ollama · {name}"),
                Some(oc),
            )
        }
    };

    // Leg 1 of exactly 2: one tiny chat, streamed so the first token stamps
    // time-to-first separately from total.
    let messages = vec![crate::ai::ChatTurn::user(
        "Reply with only the word OK.".to_string(),
    )];
    let start = std::time::Instant::now();
    let mut first: Option<u128> = None;
    let chat = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        engine.chat_stream(&messages, |_tok| {
            if first.is_none() {
                first = Some(start.elapsed().as_millis());
            }
        }),
    )
    .await
    {
        // A timeout with the daemon still up is usually a cold model paging
        // in, not a broken setup — ask /api/ps and tell that story instead
        // of a bare "no answer" (the story fn covers daemon-down too).
        Err(_) => {
            let ctx = match &embed_cfg {
                Some(oc) => Some(ollama_timeout_context(oc, &oc.chat_model).await),
                None => None,
            };
            let model = embed_cfg
                .as_ref()
                .map(|oc| oc.chat_model.clone())
                .unwrap_or_else(|| label.clone());
            ProbeResult::Failed(crate::selfheal::timeout_story("answer", 30, &model, ctx))
        }
        Ok(Err(err)) => ProbeResult::Failed(format!("{err:#}")),
        Ok(Ok(_)) => {
            let total_ms = start.elapsed().as_millis();
            ProbeResult::Ok {
                first_ms: first.unwrap_or(total_ms),
                total_ms,
            }
        }
    };

    // Leg 2 of exactly 2: one embed, only where the target embeds.
    let embed = match embed_cfg {
        Some(oc) => {
            let embed_model = oc.embed_model.clone();
            let ollama = crate::ai::Ollama::new(oc.clone());
            let estart = std::time::Instant::now();
            let result =
                match tokio::time::timeout(std::time::Duration::from_secs(15), ollama.test_embed())
                    .await
                {
                    Err(_) => {
                        let ctx = ollama_timeout_context(&oc, &oc.embed_model).await;
                        ProbeResult::Failed(crate::selfheal::timeout_story(
                            "embedding",
                            15,
                            &oc.embed_model,
                            Some(ctx),
                        ))
                    }
                    Ok(Err(err)) => ProbeResult::Failed(format!("{err:#}")),
                    Ok(Ok(_dims)) => ProbeResult::Ok {
                        first_ms: 0,
                        total_ms: estart.elapsed().as_millis(),
                    },
                };
            Some((embed_model, result))
        }
        None => None,
    };
    crate::selfheal::format_test_report(&config, &label, &chat, embed.as_ref())
}

/// What is the Ollama daemon doing right after one of our probes timed out?
/// `/api/ps` distinguishes "model mid-load" from "daemon idle" from "daemon
/// gone"; an old daemon without /api/ps falls back to /api/tags for the
/// alive check. Short timeouts throughout — this runs inside a failure path.
async fn ollama_timeout_context(
    oc: &crate::ai::OllamaConfig,
    model: &str,
) -> crate::selfheal::OllamaTimeoutContext {
    use crate::selfheal::OllamaTimeoutContext as Ctx;
    let ollama = crate::ai::Ollama::new(oc.clone());
    match tokio::time::timeout(std::time::Duration::from_secs(3), ollama.ps()).await {
        Ok(Ok(loaded)) => {
            if loaded.iter().any(|n| n == model || n.starts_with(model)) {
                Ctx::Loading
            } else {
                Ctx::AliveIdle
            }
        }
        Ok(Err(_)) => {
            match tokio::time::timeout(std::time::Duration::from_secs(3), ollama.list_models())
                .await
            {
                Ok(Ok(_)) => Ctx::AliveIdle,
                _ => Ctx::DaemonUnreachable,
            }
        }
        Err(_) => Ctx::DaemonUnreachable,
    }
}

/// Apply a per-notebook chat style/length change (RFC-conversational-setup
/// §2). The per-notebook ChatConfig lives frontend-side, so the validated
/// change travels as a `settings://style` event every window applies to its
/// stored config; the returned echo is the transcript row.
pub(crate) async fn settings_style_apply(
    app: &AppHandle,
    state: &AppState,
    notebook_id: &str,
    style_in: &str,
    length_in: &str,
) -> String {
    let (style, length, echo) = match crate::selfheal::settings_style(style_in, length_in) {
        Ok(v) => v,
        Err(msg) => return msg,
    };
    let known = state
        .db
        .list_notebooks()
        .await
        .map(|nbs| nbs.iter().any(|n| n.id == notebook_id))
        .unwrap_or(false);
    if !known {
        return "I couldn't find that notebook — styles are per notebook, so tell me which \
                one (or ask from inside it)."
            .to_string();
    }
    let _ = app.emit(
        "settings://style",
        serde_json::json!({
            "notebookId": notebook_id,
            "style": style,
            "length": length,
        }),
    );
    echo
}

/// Switch the app theme (RFC-conversational-setup §3). The theme is
/// frontend state (localStorage + applyTheme), so the resolved id travels
/// as a `settings://theme` event; every window applies it through the same
/// setTheme path the Settings dialog uses.
pub(crate) fn settings_theme_apply(app: &AppHandle, query: &str) -> String {
    if query.trim().is_empty() {
        return crate::selfheal::theme_roster_text();
    }
    match crate::selfheal::resolve_theme(query) {
        Ok((id, label)) => {
            let _ = app.emit("settings://theme", serde_json::json!({ "theme": id }));
            format!("Switched the theme to {label}.")
        }
        Err(msg) => msg,
    }
}

/// The `connect` verb's read/confirm side (RFC-conversational-setup §4).
/// NEVER writes: an empty target lists the agent clients; a named target
/// answers with the confirm-click grammar — only that click (or the MCP
/// call with confirm: true) reaches `connect_agent`.
pub(crate) async fn settings_connect_report(app: &AppHandle, target: &str) -> String {
    let list = match crate::connectors::list_agent_connectors(app.clone()).await {
        Ok(l) => l,
        Err(err) => return format!("Couldn't read the agent clients: {err}"),
    };
    if target.trim().is_empty() {
        let rows = list
            .iter()
            .map(|c| {
                format!(
                    "- {} — {}{}",
                    c.name,
                    if !c.installed {
                        "not installed"
                    } else if c.configured {
                        "connected"
                    } else {
                        "installed, not connected"
                    },
                    if c.installed && !c.configured && c.can_auto {
                        format!(" (say “connect {}”)", c.name.to_lowercase())
                    } else {
                        String::new()
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        return format!("Agent clients:\n{rows}");
    }
    let t = target.trim().to_lowercase();
    let found = list
        .iter()
        .find(|c| c.id.to_lowercase() == t || c.name.to_lowercase() == t)
        .or_else(|| list.iter().find(|c| c.name.to_lowercase().contains(&t)));
    let Some(c) = found else {
        return format!(
            "No agent client matches “{}” — I know: {}.",
            target.trim(),
            list.iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    if !c.installed {
        return format!(
            "{} doesn't look installed on this Mac, so there's nothing to connect yet.",
            c.name
        );
    }
    if c.configured && (!c.supports_skill || c.skill_installed) {
        return format!("{} is already connected ({}).", c.name, c.config_path);
    }
    if !c.can_auto {
        return format!(
            "{} needs manual setup — paste this into {}:\n{}",
            c.name, c.config_path, c.snippet
        );
    }
    crate::selfheal::connect_confirm_text(&c.id, &c.name, &c.config_path)
}

/// The guided flow (RFC-conversational-setup §5): gather live state, then
/// let the pure `setup_next_step` pick and render the ONE next unmet step.
pub(crate) async fn settings_setup_report(app: &AppHandle, state: &AppState) -> String {
    let ai = state.ai.read().await.clone();
    let config = ai.config().clone();
    let chat_entry = config.provider_by_id(&config.chat_provider).cloned();
    let (chat_ready, chat_detail) = match &chat_entry {
        Some(entry) => readiness_for_entry(app, entry, &config)
            .await
            .unwrap_or((false, "probe failed".into())),
        None => (false, "no provider selected".into()),
    };
    let fm_ready = match config.providers.iter().find(|p| p.kind == "fm") {
        Some(entry) => readiness_for_entry(app, entry, &config)
            .await
            .map(|(ready, _)| ready)
            .unwrap_or(false),
        None => false,
    };
    let installed: Option<Vec<String>> =
        match tokio::time::timeout(std::time::Duration::from_secs(4), ai.list_models()).await {
            Ok(Ok(models)) => Some(models),
            _ => None,
        };
    let has_model = |name: &str| {
        installed.as_ref().is_some_and(|ms| {
            ms.iter()
                .any(|m| m == name || m.starts_with(&format!("{name}:")))
        })
    };
    let chat_model = chat_entry
        .as_ref()
        .map(|e| e.chat_model.trim())
        .filter(|m| !m.is_empty())
        .unwrap_or(config.chat_model.trim())
        .to_string();
    let connectors = crate::connectors::list_agent_connectors(app.clone())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|c| (c.id, c.name, c.installed, c.configured))
        .collect();
    crate::selfheal::setup_next_step(&crate::selfheal::SetupState {
        chat_label: chat_entry
            .as_ref()
            .map(|e| e.label.clone())
            .unwrap_or_else(|| config.chat_provider.clone()),
        chat_kind: chat_entry.map(|e| e.kind).unwrap_or_default(),
        chat_ready,
        chat_detail,
        chat_model_installed: has_model(&chat_model),
        chat_model,
        fm_ready,
        ollama_reachable: installed.is_some(),
        embedder: config.embedder.clone(),
        embed_model_installed: has_model(config.embed_model.trim()),
        embed_model: config.embed_model.trim().to_string(),
        profile_named: !config.profile.name.trim().is_empty(),
        connectors,
    })
}

/// Apply a confirmed connect from the transcript's confirm-click
/// (RFC-conversational-setup §4): the ONLY chat-side path that writes an
/// agent client's config, and the echo names the file it touched.
#[tauri::command]
pub async fn apply_connect_fix(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: String,
    client_id: String,
) -> Result<Message, String> {
    let status = crate::connectors::connect_agent(app.clone(), client_id).await?;
    let skill = if status.supports_skill && status.skill_installed {
        " and installed the alchemy skill"
    } else {
        ""
    };
    let echo = format!(
        "Connected {} — wrote {}{skill}. Restart {} to pick up the change.",
        status.name, status.config_path, status.name
    );
    finish_tool_reply(&app, &state, &notebook_id, echo).await
}

/// Persist a tool-produced assistant reply and finish the chat turn.
async fn finish_tool_reply(
    app: &AppHandle,
    state: &AppState,
    notebook_id: &str,
    content: String,
) -> Result<Message, String> {
    let msg = Message {
        id: new_id(),
        notebook_id: notebook_id.to_string(),
        role: "assistant".into(),
        content,
        citations: vec![],
        kind: "tool".into(),
        model: String::new(),
        created_at: now(),
    };
    e(state.db.add_message(&msg).await)?;
    e(state.db.touch_notebook(notebook_id, now()).await)?;
    let _ = app.emit("chat://done", &msg);
    Ok(msg)
}

// ---- Chat tools ------------------------------------------------------------
//
// Imperative chat messages ("add this url", "make a study guide", "delete the
// spec pdf") route to tools instead of RAG. A cheap keyword gate keeps normal
// questions on the zero-overhead path; gated messages get one small JSON
// routing call to the chat model, then dispatch to existing commands.

/// Cheap pre-filter: only messages with a URL or an imperative verb + tool
/// noun ever reach the LLM router.
fn tool_gate(content: &str) -> bool {
    if !extract_urls(content).is_empty() {
        return true;
    }
    let l = content.to_lowercase();
    let verb = [
        "add", "import", "ingest", "attach", "load", "grab", "pull in", "paste", "make", "create",
        "generate", "write", "build", "remove", "delete", "drop", "get rid", "refresh", "re-fetch",
        "refetch", "update", "save", "schedule", "edit", "rename", "change", "pause", "enable",
        "disable", "resume", "switch", "use", "show", "set", "pull", "test", "download", "list",
        "connect", "call me", "help",
    ]
    .iter()
    .any(|k| l.contains(k));
    let noun = [
        "source",
        "url",
        "link",
        "summary",
        "faq",
        "study guide",
        "briefing",
        "timeline",
        "problems",
        "prd",
        "prfaq",
        "pr/faq",
        "rfc",
        "skill",
        "note",
        "report",
        "document",
        "doc",
        "template",
        "generator",
        // The settings tool (RFC-self-resolve phase 3 +
        // RFC-conversational-setup).
        "provider",
        "model",
        "settings",
        "embedder",
        "effort",
        "ollama",
        "gateway",
        "apple intelligence",
        "theme",
        "style",
        "profile",
        "brief",
        "agent",
        "claude",
        "codex",
        "alchemy",
        "set up",
        "setup",
        // Night Shift administration (docs/RFC-night-shift-area.md §4).
        "night shift",
        "tonight",
        "overnight",
        "watcher",
        "standing order",
        "commission",
        "receipt",
    ]
    .iter()
    .any(|k| l.contains(k));
    verb && noun
}

enum ToolAction {
    AddUrls(Vec<String>),
    AddText {
        title: String,
        text: String,
    },
    Generate {
        kind: String,
        prompt: String,
    },
    RemoveSource(String),
    RefreshSources(String),
    SaveNote(String),
    CreateTemplate {
        name: String,
        description: String,
        prompt: String,
    },
    ScheduleReport {
        kind: String,
        interval: String,
        name: String,
        prompt: String,
    },
    /// One-off overnight work (docs/RFC-night-shift-area.md §1, §4).
    Commission {
        kind: String,
        name: String,
        prompt: String,
        /// "tonight" (default) or "now".
        when: String,
    },
    /// Ask about, pause, or resume the Night Shift.
    NightShift {
        /// "status" | "pause" | "resume"
        op: String,
    },
    UpdateReport {
        /// Name fragment identifying the existing schedule.
        name: String,
        /// Empty fields below mean "leave unchanged".
        new_name: String,
        kind: String,
        interval: String,
        prompt: String,
        enabled: String,
    },
    /// The settings tool (RFC-self-resolve phase 3, grown by
    /// RFC-conversational-setup phase 1). `op` is one of get | set |
    /// models | test | pull; `field` carries set's field, test's target,
    /// or pull's model name; `value` is set-only.
    Settings {
        op: String,
        field: String,
        value: String,
    },
    Chat,
}

const TOOL_ROUTER_SYSTEM: &str = "You route a user's chat message in a research-notebook app. \
Decide if the message is a COMMAND to perform one of the tools below, or an ordinary question. \
Respond with EXACTLY ONE JSON object, nothing else.\n\n\
Tools:\n\
- {\"action\":\"add_urls\",\"urls\":[\"https://…\"]} — add the given URL(s) as sources.\n\
- {\"action\":\"add_text\",\"title\":\"<short title>\",\"text\":\"<the text to add>\"} — save text from the message as a source.\n\
- {\"action\":\"generate\",\"kind\":\"<KINDS>|custom\",\"prompt\":\"<extra instructions or empty>\"} — generate a document from the sources.\n\
- {\"action\":\"remove_source\",\"name\":\"<source name fragment>\"} — remove a source.\n\
- {\"action\":\"refresh_sources\",\"name\":\"<name fragment, or empty for all URL sources>\"} — re-fetch URL sources.\n\
- {\"action\":\"save_note\",\"title\":\"<title or empty>\"} — save the assistant's previous answer as a note.\n\
- {\"action\":\"create_template\",\"name\":\"<short name>\",\"description\":\"<one line>\",\"prompt\":\"<the reusable generation instruction>\"} — save a reusable custom generator the user can run from Studio later. Compose \"prompt\" yourself from what they asked the generator to do.\n\
- {\"action\":\"schedule_report\",\"kind\":\"<KINDS>|brief|custom, or a template name from the list below\",\"interval\":\"hourly|daily|weekly\",\"name\":\"<report name>\",\"prompt\":\"<what the report should cover, for kind custom; else empty>\"} — create a recurring report (\"make a weekly brief of this notebook\" → kind brief, interval weekly; echo the user's cadence word in \"interval\" even if unsupported).\n\
- {\"action\":\"commission\",\"kind\":\"<KINDS>|custom, or a template name\",\"name\":\"<short job name>\",\"prompt\":\"<what to do, for kind custom>\",\"when\":\"tonight|now\"} — hand ONE job to the Night Shift instead of running it now (\"tonight, re-read the Japan sources and rebuild the summary\"). Default \"when\" is tonight; use \"now\" only when the user says so.\n\
- {\"action\":\"night_shift\",\"op\":\"status|pause|resume\"} — report what the Night Shift has queued, or pause/resume overnight report runs (\"pause the night shift until morning\").\n\
- {\"action\":\"update_report\",\"name\":\"<existing report name fragment>\",\"new_name\":\"\",\"kind\":\"\",\"interval\":\"\",\"prompt\":\"\",\"enabled\":\"true|false or empty\"} — change an existing recurring report; leave fields empty to keep them.\n\
- {\"action\":\"settings\",\"op\":\"get\"} — show the current AI provider/model settings (always redacted; API keys are never readable).\n\
- {\"action\":\"settings\",\"op\":\"set\",\"field\":\"chatProvider|studioProvider|chatModel|effort|baseUrl|smallModel|embedder|provider.<id>.chatModel|provider.<id>.effort|provider.<id>.baseUrl\",\"value\":\"<new value>\"} — change ONE AI setting (\"switch chat to ollama\" → field chatProvider, value ollama; bare chatModel/effort/baseUrl target the active chat provider). API keys can never be read or set through this tool.\n\
- {\"action\":\"settings\",\"op\":\"models\"} — list installed Ollama models plus every provider's active model and readiness (\"what models do I have\").\n\
- {\"action\":\"settings\",\"op\":\"test\",\"target\":\"<provider or model name, or empty for the active chat provider>\"} — live-probe one provider or model (one tiny chat + embed) and report latency (\"is ollama working\", \"test gemma3\").\n\
- {\"action\":\"settings\",\"op\":\"pull\",\"model\":\"<ollama model name>\"} — stage `ollama pull <model>` as a one-click Terminal command; it is never executed automatically (\"download gemma3\", \"pull qwen3:8b\").\n\
- {\"action\":\"settings\",\"op\":\"set\",\"field\":\"profile.name|profile.profession|profile.instructions\",\"value\":\"<free text>\"} — personalize (\"call me Paul\" → profile.name Paul; \"always answer briefly\" as a standing preference → profile.instructions).\n\
- {\"action\":\"settings\",\"op\":\"style\",\"style\":\"default|learning|friendly|professional|scientific|adhd|ste100|govuk|plain|gdev|custom or empty\",\"length\":\"default|shorter|longer or empty\"} — set THIS notebook's answer voice/length (\"use the Google style here\", \"shorter answers in this notebook\"). Empty keeps that half unchanged.\n\
- {\"action\":\"settings\",\"op\":\"theme\",\"theme\":\"<theme name, or empty to list them>\"} — switch the app theme (\"use the gruvbox theme\", \"something dark\").\n\
- {\"action\":\"settings\",\"op\":\"connect\",\"target\":\"<agent client name, or empty to list>\"} — connect Alchemy to an installed agent client (Claude Code, Codex, …). Always confirmed with a click before anything is written.\n\
- {\"action\":\"settings\",\"op\":\"setup\"} — guided setup: reports the next unmet setup step (\"help me get set up\").\n\
- {\"action\":\"chat\"} — not a command; answer normally.\n\n\
Prefer {\"action\":\"chat\"} when unsure. Questions ABOUT sources (\"what does the spec say\") are chat, \
not tools.";

/// Neutralize a source title before interpolating it into the router prompt:
/// strip braces/newlines (JSON-shaped injection) and cap the length so a
/// hostile ingested page can't smuggle instructions into the classifier.
fn sanitize_title(t: &str) -> String {
    let cleaned: String = t
        .chars()
        .filter(|c| !matches!(c, '{' | '}' | '\n' | '\r' | '"'))
        .collect();
    cleaned.trim().chars().take(80).collect()
}

/// One small LLM call to classify a gated message into a ToolAction.
async fn route_tool(state: &AppState, sources: &[Source], content: &str) -> ToolAction {
    let source_list = if sources.is_empty() {
        "(none)".to_string()
    } else {
        sources
            .iter()
            .map(|s| format!("- {} [{}]", sanitize_title(&s.title), s.source_type))
            .collect::<Vec<_>>()
            .join("\n")
    };
    // The router's kind lists come from the artifact registry (plus the
    // user's templates), so a new generator or template is routable the
    // moment it exists — no prompt edit to forget.
    let system = TOOL_ROUTER_SYSTEM.replace("<KINDS>", &rag::ARTIFACT_KINDS.join("|"));
    let template_list = crate::templates::list_templates()
        .unwrap_or_default()
        .iter()
        .map(|t| format!("- {}", sanitize_title(&t.name)))
        .collect::<Vec<_>>()
        .join("\n");
    let system = if template_list.is_empty() {
        system
    } else {
        format!("{system}\n\nUser templates (usable as schedule_report kinds):\n{template_list}")
    };
    let messages = vec![
        crate::ai::ChatTurn::system(system),
        crate::ai::ChatTurn::user(format!(
            "Current sources:\n{source_list}\n\nUser message:\n{content}\n\nOne JSON object:"
        )),
    ];
    let raw = {
        let ai = state.ai.read().await.clone();
        match ai.chat(&messages).await {
            Ok(o) => o.text,
            Err(_) => return ToolAction::Chat,
        }
    };
    parse_tool_action(&raw)
}

fn parse_tool_action(raw: &str) -> ToolAction {
    let Some(json) = crate::agent::extract_json(raw) else {
        return ToolAction::Chat;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else {
        return ToolAction::Chat;
    };
    let s = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    match v.get("action").and_then(|a| a.as_str()).unwrap_or("chat") {
        "add_urls" => {
            let urls: Vec<String> = v
                .get("urls")
                .and_then(|u| u.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .map(str::trim)
                        .filter_map(|u| {
                            if u.starts_with("http://") || u.starts_with("https://") {
                                Some(u.to_string())
                            } else if u.contains('.') && !u.contains(char::is_whitespace) {
                                // Scheme-less host like "example.com/page".
                                Some(format!("https://{u}"))
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            if urls.is_empty() {
                ToolAction::Chat
            } else {
                ToolAction::AddUrls(urls)
            }
        }
        "add_text" => {
            let text = s("text");
            if text.is_empty() {
                ToolAction::Chat
            } else {
                ToolAction::AddText {
                    title: s("title"),
                    text,
                }
            }
        }
        "generate" => {
            let kind = s("kind");
            if kind.is_empty() {
                ToolAction::Chat
            } else {
                ToolAction::Generate {
                    kind,
                    prompt: s("prompt"),
                }
            }
        }
        "remove_source" => {
            let name = s("name");
            if name.is_empty() {
                ToolAction::Chat
            } else {
                ToolAction::RemoveSource(name)
            }
        }
        "refresh_sources" => ToolAction::RefreshSources(s("name")),
        "save_note" => ToolAction::SaveNote(s("title")),
        "create_template" => {
            let prompt = s("prompt");
            if prompt.is_empty() {
                ToolAction::Chat
            } else {
                ToolAction::CreateTemplate {
                    name: s("name"),
                    description: s("description"),
                    prompt,
                }
            }
        }
        "commission" => {
            let name = s("name");
            if name.is_empty() {
                ToolAction::Chat
            } else {
                ToolAction::Commission {
                    kind: s("kind"),
                    name,
                    prompt: s("prompt"),
                    when: s("when"),
                }
            }
        }
        "night_shift" => ToolAction::NightShift { op: s("op") },
        "schedule_report" => {
            // Keep the raw kind and interval; dispatch validates both against
            // the live registry (artifact kinds + user templates) and refuses
            // politely instead of silently coercing to some other report.
            let kind = s("kind");
            let name = {
                let n = s("name");
                if n.is_empty() {
                    "Scheduled report".into()
                } else {
                    n
                }
            };
            ToolAction::ScheduleReport {
                kind,
                interval: s("interval"),
                name,
                prompt: s("prompt"),
            }
        }
        "update_report" => {
            let name = s("name");
            if name.is_empty() {
                ToolAction::Chat
            } else {
                ToolAction::UpdateReport {
                    name,
                    new_name: s("new_name"),
                    kind: s("kind"),
                    interval: s("interval"),
                    prompt: s("prompt"),
                    enabled: s("enabled"),
                }
            }
        }
        "settings" => match s("op").as_str() {
            "get" => ToolAction::Settings {
                op: "get".into(),
                field: String::new(),
                value: String::new(),
            },
            "set" => {
                let field = s("field");
                if field.is_empty() {
                    ToolAction::Chat
                } else {
                    ToolAction::Settings {
                        op: "set".into(),
                        field,
                        value: s("value"),
                    }
                }
            }
            // Model verbs (RFC-conversational-setup phase 1). `test` with no
            // target probes the active chat provider; `pull` without a model
            // can't do anything and falls through to chat.
            "models" => ToolAction::Settings {
                op: "models".into(),
                field: String::new(),
                value: String::new(),
            },
            "test" => ToolAction::Settings {
                op: "test".into(),
                field: s("target"),
                value: String::new(),
            },
            "pull" => {
                let model = s("model");
                if model.is_empty() {
                    ToolAction::Chat
                } else {
                    ToolAction::Settings {
                        op: "pull".into(),
                        field: model,
                        value: String::new(),
                    }
                }
            }
            // Phase-2/3/5 verbs (RFC-conversational-setup): style carries
            // (style, length) in (field, value); theme and connect carry
            // their target in `field`; setup takes nothing.
            "style" => ToolAction::Settings {
                op: "style".into(),
                field: s("style"),
                value: s("length"),
            },
            "theme" => ToolAction::Settings {
                op: "theme".into(),
                field: s("theme"),
                value: String::new(),
            },
            "connect" => ToolAction::Settings {
                op: "connect".into(),
                field: s("target"),
                value: String::new(),
            },
            "setup" => ToolAction::Settings {
                op: "setup".into(),
                field: String::new(),
                value: String::new(),
            },
            _ => ToolAction::Chat,
        },
        _ => ToolAction::Chat,
    }
}

/// Resolve a requested report kind against the live registry: registry
/// artifact kinds pass through, "template:<id>" and bare template names
/// resolve to the id form, "custom" requires a prompt. Err carries the
/// polite refusal message for the chat transcript.
pub(crate) fn resolve_report_kind(kind: &str, prompt: &str) -> Result<String, String> {
    let kind = kind.trim();
    // The cross-notebook brief (docs/RFC-brief.md) — distinct from the
    // per-notebook "briefing" generator. Reads across every notebook; its
    // runs land in the notebook the schedule lives in (best: "Briefs").
    if kind.eq_ignore_ascii_case(brief::BRIEF_KIND) {
        return Ok(brief::BRIEF_KIND.to_string());
    }
    if rag::ARTIFACT_KINDS.contains(&kind) {
        return Ok(kind.to_string());
    }
    if kind == "custom" || kind.is_empty() {
        if prompt.trim().is_empty() {
            return Err(
                "A custom report needs a prompt describing what it should cover — \
                 tell me what to track and I'll schedule it."
                    .to_string(),
            );
        }
        return Ok("custom".to_string());
    }
    let templates = crate::templates::list_templates().unwrap_or_default();
    if let Some(id) = kind.strip_prefix("template:") {
        if templates.iter().any(|t| t.id == id) {
            return Ok(kind.to_string());
        }
    }
    if let Some(t) = templates
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(kind) || t.id == kind)
    {
        return Ok(format!("template:{}", t.id));
    }
    Err(format!(
        "I don't know a “{kind}” report. I can schedule any generator ({}), one of your \
         templates{}, or a custom prompt — which would you like?",
        rag::ARTIFACT_KINDS.join(", "),
        if templates.is_empty() {
            String::new()
        } else {
            format!(
                " ({})",
                templates
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    ))
}

/// Deterministic settings fast-path gate (RFC-conversational-setup): tight
/// imperative shapes that reach the settings verbs in BOTH chat modes — deep
/// research skips the LLM router entirely, so without this a "switch chat to
/// ollama" ask came back as a cited research essay. Returns
/// `(op, field, value)` for the settings dispatcher, or None to fall through.
///
/// Tightness is the contract: only short messages (≤ 80 chars), only shapes
/// that START with the imperative, and never anything question-shaped —
/// "how do I switch chat providers in LM Studio?" must still reach research.
/// The one interrogative exception is a closed set of exact roster asks
/// ("what models do i have"). A gate hit that later fails to resolve (an
/// unknown provider, an unresolvable test target) falls through to normal
/// flow in the dispatcher — a false positive must never eat a research ask.
pub(crate) fn settings_gate(content: &str) -> Option<(String, String, String)> {
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.len() > 80 {
        return None;
    }
    let lower = trimmed.to_lowercase();
    let l = lower.trim_end_matches(['?', '!', '.', ' ']);
    let hit = |op: &str, field: &str, value: &str| {
        Some((op.to_string(), field.to_string(), value.to_string()))
    };

    // Closed set of exact roster asks — the only interrogatives allowed.
    const MODELS_ASKS: [&str; 9] = [
        "what models do i have",
        "what models do i have installed",
        "what models are installed",
        "which models do i have",
        "which models are installed",
        "list models",
        "list my models",
        "list installed models",
        "show my models",
    ];
    if MODELS_ASKS.contains(&l) {
        return hit("models", "", "");
    }

    // Anything else that opens like a question is research, not a command.
    const INTERROGATIVES: [&str; 13] = [
        "how ", "why ", "when ", "where ", "what ", "which ", "who ", "can ", "could ", "should ",
        "would ", "does ", "is ",
    ];
    if INTERROGATIVES.iter().any(|p| l.starts_with(p)) {
        return None;
    }

    const SETTINGS_ASKS: [&str; 5] = [
        "show my settings",
        "show settings",
        "get my settings",
        "show my model settings",
        "show my ai settings",
    ];
    if SETTINGS_ASKS.contains(&l) {
        return hit("get", "", "");
    }

    // The guided flow (RFC-conversational-setup §5) — exact imperative
    // shapes only; "how do I set up X" stayed research above.
    const SETUP_ASKS: [&str; 6] = [
        "help me get set up",
        "help me set up",
        "help me get set up with alchemy",
        "help me set up alchemy",
        "set up alchemy",
        "get me set up",
    ];
    if SETUP_ASKS.contains(&l) {
        return hit("setup", "", "");
    }

    // Theme: "switch/set/change [the] theme to X" and "use the X theme".
    for prefix in [
        "switch theme to ",
        "switch the theme to ",
        "set theme to ",
        "set the theme to ",
        "change theme to ",
        "change the theme to ",
    ] {
        if let Some(rest) = l.strip_prefix(prefix) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return hit("theme", rest, "");
            }
        }
    }
    if let Some(mid) = l
        .strip_prefix("use the ")
        .and_then(|r| r.strip_suffix(" theme"))
    {
        let mid = mid.trim();
        if !mid.is_empty() {
            return hit("theme", mid, "");
        }
    }

    // "call me <name>" — the day-one profile one-liner. Case is recovered
    // from the original text (the gate lowercases only for matching).
    if l.starts_with("call me ") {
        let orig: String = trimmed
            .trim_end_matches(['?', '!', '.', ' '])
            .chars()
            .skip("call me ".chars().count())
            .collect();
        let name = orig.trim();
        if !name.is_empty()
            && name.chars().count() <= 30
            && name.split_whitespace().count() <= 3
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '\'' | '.'))
        {
            return hit("set", "profile.name", name);
        }
        return None;
    }

    // switch chat|studio [provider] to <provider> — resolved (and refused,
    // falling through) against the live roster at dispatch time.
    for (prefix, field) in [
        ("switch chat to ", "chatProvider"),
        ("switch chat provider to ", "chatProvider"),
        ("switch studio to ", "studioProvider"),
        ("switch studio provider to ", "studioProvider"),
        ("switch the embedder to ", "embedder"),
        ("switch embedder to ", "embedder"),
    ] {
        if let Some(rest) = l.strip_prefix(prefix) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return hit("set", field, rest);
            }
        }
    }

    // test <provider|model>: at most three short charset-clean words (the
    // multi-word forms are provider aliases like "apple intelligence");
    // dispatch re-resolves the target and falls through when it's neither a
    // provider nor a model name — "test the hypothesis …" stays research.
    if let Some(rest) = l.strip_prefix("test ") {
        let rest = rest.trim();
        if !rest.is_empty()
            && rest.len() <= 40
            && rest.split_whitespace().count() <= 3
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || " ._:/-".contains(c))
        {
            return hit("test", rest, "");
        }
    }

    // pull <model> — only a bare name in the pull charset; the staged
    // command is re-gated at execution either way.
    for prefix in ["ollama pull ", "pull "] {
        if let Some(rest) = l.strip_prefix(prefix) {
            let rest = rest.trim();
            if is_safe_model_name(rest) {
                return hit("pull", rest, "");
            }
            return None;
        }
    }

    None
}

/// Dispatch a settings_gate hit through the settings verbs — shared by both
/// chat modes, no LLM router involved. None = fall through to normal flow
/// (research/chat): the gate hit didn't resolve cleanly, and a false
/// positive must never break a real question.
async fn try_settings_fast_path(
    app: &AppHandle,
    state: &AppState,
    content: &str,
) -> Option<String> {
    let (op, field, value) = settings_gate(content)?;
    match op.as_str() {
        "get" => {
            let config = { state.ai.read().await.config().clone() };
            Some(crate::selfheal::settings_get(&config))
        }
        "models" => Some(settings_models_report(app, state).await),
        "pull" => crate::selfheal::settings_pull(&field).ok(),
        // A gated theme ask is unambiguous as an ASK even when the name
        // isn't — the roster/ambiguity reply is the right answer, not
        // fallthrough. Same for setup: the phrase set is exact.
        "theme" => Some(settings_theme_apply(app, &field)),
        "setup" => Some(settings_setup_report(app, state).await),
        "test" => {
            let config = { state.ai.read().await.config().clone() };
            // Pre-resolve: an unresolvable target means this probably wasn't
            // a settings ask after all.
            crate::selfheal::resolve_test_target(&config, &field).ok()?;
            Some(settings_test_report(state, &field).await)
        }
        "set" => {
            let mut config = { state.ai.read().await.config().clone() };
            match crate::selfheal::settings_set(&mut config, &field, &value) {
                Ok(echo) => match apply_ai_config(app, state, config).await {
                    Ok(()) => {
                        notify_changed("settings", None);
                        Some(echo)
                    }
                    Err(err) => Some(format!("Couldn't apply that setting: {err}")),
                },
                // Unknown provider etc. — not clean, so not ours.
                Err(_) => None,
            }
        }
        _ => None,
    }
}

/// Create a report schedule and render the confirmation/refusal reply.
/// Shared by the LLM router's schedule_report action and the deterministic
/// schedule gate below, so the two can never validate differently.
///
/// Hand one job to the night from chat (docs/RFC-night-shift-area.md §4).
/// The Tonight composer and the chat box are the same parser, so this reply
/// is what both produce. Kinds are validated exactly as schedules are:
/// refusing beats coercing.
async fn commission_reply(
    state: &AppState,
    notebook_id: &str,
    kind: &str,
    name: &str,
    prompt: &str,
    when: &str,
) -> String {
    // Same validation as a recurring schedule, and the same refusal copy:
    // a commission that quietly runs the wrong generator wastes a night.
    let kind = match resolve_report_kind(kind, prompt) {
        Ok(kind) => kind,
        Err(refusal) => return refusal,
    };
    let tonight = when != "now";
    let not_before = if tonight {
        crate::scheduler::next_local_hour_ms(2)
    } else {
        0
    };
    let schedule = ReportSchedule {
        id: new_id(),
        notebook_id: notebook_id.to_string(),
        name: name.trim().to_string(),
        kind,
        prompt: prompt.to_string(),
        trigger: "once".into(),
        not_before,
        interval_secs: 86_400,
        enabled: true,
        last_run_at: 0,
        created_at: now(),
    };
    match state.db.add_report_schedule(&schedule).await {
        Ok(()) => {
            let clock = if tonight {
                "It starts at 2:00 AM"
            } else {
                "It starts on the next pass"
            };
            format!(
                "Commissioned **{name}**. {clock}, and the result waits for you as a note. It writes notes and reports; it will not act outward."
            )
        }
        Err(err) => format!("Couldn't queue that: {err:#}"),
    }
}

/// Answer "what is the Night Shift doing?" and flip the overnight pause from
/// chat. Deterministic on purpose: status should be exact, not retrieved.
async fn night_shift_reply(state: &AppState, op: &str) -> String {
    match op {
        "pause" | "resume" => {
            let paused = crate::scheduler::is_paused();
            let want_pause = op == "pause";
            if paused == want_pause {
                return if paused {
                    "The Night Shift is already paused until morning.".into()
                } else {
                    "The Night Shift is already running.".into()
                };
            }
            let now_paused = crate::scheduler::toggle_pause();
            if let Some(app) = app_handle() {
                crate::integrations::set_tray_pause_label(&app, now_paused);
            }
            if now_paused {
                "Paused until morning. Scheduled reports hold; source syncing and housekeeping continue.".into()
            } else {
                "Resumed. Anything that came due while paused runs on the next pass.".into()
            }
        }
        _ => {
            let background = state.ai.read().await.config().background_enabled;
            if !background {
                return "Background work is off, so nothing runs on its own. Turn it back on in Settings \u{2192} Background Work.".into();
            }
            let queued = state
                .db
                .all_report_schedules()
                .await
                .map(|all| {
                    all.into_iter()
                        .filter(|s| s.enabled && s.trigger == "once" && s.last_run_at == 0)
                        .count()
                })
                .unwrap_or(0);
            let paused = if crate::scheduler::is_paused() {
                " Reports are paused until morning."
            } else {
                ""
            };
            match queued {
                0 => {
                    format!("The Night Shift is on with nothing commissioned for tonight.{paused}")
                }
                1 => {
                    format!("The Night Shift is on with one commission queued for tonight.{paused}")
                }
                n => format!(
                    "The Night Shift is on with {n} commissions queued for tonight.{paused}"
                ),
            }
        }
    }
}

/// Validates the kind against the live registry: any artifact kind, any
/// existing template (by "template:<id>" or by name), the cross-notebook
/// brief, or "custom" with a prompt. Refusing beats coercing — a schedule
/// that quietly generates the wrong report erodes trust in all of them.
async fn create_schedule_reply(
    state: &AppState,
    notebook_id: &str,
    kind: &str,
    interval: &str,
    name: &str,
    prompt: &str,
) -> String {
    let kind = match resolve_report_kind(kind, prompt) {
        Ok(k) => k,
        Err(msg) => return msg,
    };
    let interval_secs = match interval {
        "hourly" => 3_600,
        "daily" => 86_400,
        "weekly" => 604_800,
        other => {
            return format!(
                "I can schedule reports **hourly**, **daily**, or **weekly** — “{other}” isn't supported yet, so I haven't created anything. Rephrase with one of those cadences?"
            );
        }
    };
    let schedule = ReportSchedule {
        id: new_id(),
        notebook_id: notebook_id.to_string(),
        name: name.trim().to_string(),
        kind,
        prompt: prompt.to_string(),
        trigger: "interval".into(),
        not_before: 0,
        interval_secs,
        enabled: true,
        last_run_at: 0,
        created_at: now(),
    };
    match state.db.add_report_schedule(&schedule).await {
        Ok(()) => format!(
            "Scheduled **{name}** to run {interval} — it refreshes your URL sources, then writes a timestamped note (first run starts shortly). Manage it under Studio → Reports."
        ),
        Err(err) => format!("Couldn't create the schedule: {err:#}"),
    }
}

/// Deterministic schedule gate (RFC-conversational-setup phase 4): the
/// "make a weekly brief of this notebook" shape is unambiguous and
/// imperative, so it schedules in BOTH chat modes without the LLM router.
/// Returns (kind, interval, name). Same tightness contract as
/// `settings_gate`: short, leading imperative, cadence + known kind, and
/// only filler words after — anything else falls through to normal flow.
pub(crate) fn schedule_gate(content: &str) -> Option<(String, String, String)> {
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.len() > 80 {
        return None;
    }
    let lower = trimmed.to_lowercase();
    let l = lower.trim_end_matches(['?', '!', '.', ' ']);
    let rest = [
        "make me a ",
        "make me an ",
        "make a ",
        "make an ",
        "create a ",
        "create an ",
        "schedule a ",
        "schedule an ",
    ]
    .iter()
    .find_map(|p| l.strip_prefix(p))?;
    let mut words = rest.split_whitespace();
    let interval = match words.next()? {
        i @ ("hourly" | "daily" | "weekly") => i,
        _ => return None,
    };
    let kind = match words.next()? {
        k @ ("brief" | "briefing" | "summary" | "timeline" | "faq") => k,
        _ => return None,
    };
    // Whatever follows must be pure filler ("of this notebook", "report
    // for my sources") — a real clause means a real request, not this shape.
    const FILLER: [&str; 9] = [
        "of", "for", "on", "this", "the", "my", "notebook", "report", "sources",
    ];
    if !words.all(|w| FILLER.contains(&w)) {
        return None;
    }
    let mut cadence: String = interval.to_string();
    if let Some(first) = cadence.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    Some((
        kind.to_string(),
        interval.to_string(),
        format!("{cadence} {kind}"),
    ))
}

/// Verbs that mean the URL in a message is a *target*, not something to add.
fn has_non_add_verb(content: &str) -> bool {
    let l = content.to_lowercase();
    [
        "remove", "delete", "drop", "get rid", "refresh", "re-fetch", "refetch",
    ]
    .iter()
    .any(|k| l.contains(k))
}

/// Gate → route → dispatch. Returns Some(reply markdown) if a tool handled the
/// message; None falls through to normal chat. With `allow_router` false only
/// the deterministic add-URL fast path runs (used in deep-research mode so
/// imperative research prompts still reach the agent loop).
async fn try_tool_route(
    app: &AppHandle,
    state: &AppState,
    notebook_id: &str,
    content: &str,
    allow_router: bool,
) -> Option<String> {
    // Settings fast path first, and OUTSIDE tool_gate: it carries its own,
    // much tighter gate, and runs in both chat modes — "test gemma3" has no
    // tool_gate noun, and deep research never reaches the LLM router.
    if let Some(reply) = try_settings_fast_path(app, state, content).await {
        return Some(reply);
    }
    // Same for the unambiguous schedule shape (RFC-conversational-setup
    // phase 4): "make a weekly brief of this notebook" schedules in both
    // modes; validation still runs through the shared creation path.
    if let Some((kind, interval, name)) = schedule_gate(content) {
        return Some(create_schedule_reply(state, notebook_id, &kind, &interval, &name, "").await);
    }
    if !tool_gate(content) {
        return None;
    }

    // Deterministic fast path: message with URLs that clearly asks to add them
    // skips the router entirely (previous behavior, zero extra latency).
    // A destructive/refresh verb disqualifies it — "delete https://x" must
    // reach the router, not re-ingest the URL.
    let urls = extract_urls(content);
    if !urls.is_empty() && wants_add_sources(content, &urls) && !has_non_add_verb(content) {
        return Some(add_url_sources(app, state, notebook_id, &urls).await);
    }
    // "Add those URLs" — resolve the referent from recent messages and
    // citation snippets. Deterministic, so it also works in deep-research mode.
    // No URLs in context ("find me sources for X")? Fall through to chat: the
    // model sees the sources' URLs and can propose concrete ones to add.
    if urls.is_empty() && wants_add_context_urls(content) && !has_non_add_verb(content) {
        let ctx = recent_context_urls(state, notebook_id).await;
        if !ctx.is_empty() {
            return Some(add_url_sources(app, state, notebook_id, &ctx).await);
        }
    }
    if !allow_router {
        return None;
    }

    let _ = app.emit(
        "chat://step",
        StepEvent {
            label: "Checking for commands".into(),
            transient: false,
        },
    );
    // Fetched once: the router prompt and the remove/refresh arms all use it.
    let sources = state.db.list_sources(notebook_id).await.ok()?;
    match route_tool(state, &sources, content).await {
        ToolAction::Chat => None,
        ToolAction::AddUrls(urls) => {
            // Trust boundary: only ingest URLs whose host actually appears in
            // the user's message — the router must not invent or rewrite them.
            let l = content.to_lowercase();
            let (mut urls, rejected): (Vec<String>, Vec<String>) = urls
                .into_iter()
                .partition(|u| l.contains(&host_of(u).to_lowercase()));
            if urls.is_empty() && !rejected.is_empty() {
                // The router may be echoing a URL the conversation mentioned
                // ("add the dealer site") — trust it only if that host really
                // appears in recent context.
                let ctx_hosts: HashSet<String> = recent_context_urls(state, notebook_id)
                    .await
                    .iter()
                    .map(|u| host_of(u).to_lowercase())
                    .collect();
                urls = rejected
                    .into_iter()
                    .filter(|u| ctx_hosts.contains(&host_of(u).to_lowercase()))
                    .collect();
            }
            if urls.is_empty() {
                Some("I couldn't find that URL in your message — paste the full address (e.g. https://example.com/page) and I'll add it.".to_string())
            } else {
                Some(add_url_sources(app, state, notebook_id, &urls).await)
            }
        }
        ToolAction::AddText { title, text } => {
            let title = if title.is_empty() {
                "Pasted from chat".into()
            } else {
                title
            };
            match ingest::extract_pasted(&title, &text) {
                Ok(ex) => match store_extracted(state, notebook_id, ex).await {
                    Ok(src) => Some(format!(
                        "Added **{}** as a source ({} chars).",
                        src.title, src.char_count
                    )),
                    Err(err) => Some(format!("Couldn't add that as a source: {err:#}")),
                },
                Err(err) => Some(format!("Couldn't add that as a source: {err:#}")),
            }
        }
        ToolAction::Generate { kind, prompt } => {
            let label = rag::artifact_spec(&kind)
                .map(|(t, _)| t.to_string())
                .unwrap_or_else(|| "document".into());
            let _ = app.emit(
                "chat://step",
                StepEvent {
                    label: format!("Generating {label}"),
                    transient: false,
                },
            );
            match generate_content(state, None, notebook_id, &kind, &prompt, None, None, None).await
            {
                Ok((title, body)) => {
                    let ts = now();
                    let note = Note {
                        id: new_id(),
                        notebook_id: notebook_id.to_string(),
                        title: title.clone(),
                        content: body,
                        kind,
                        prompt,
                        origin: String::new(),
                        status: String::new(),
                        created_at: ts,
                        updated_at: ts,
                    };
                    if let Err(err) = add_note_indexed(state, &note).await {
                        return Some(format!("Generation succeeded but saving failed: {err:#}"));
                    }
                    let _ = app.emit("generate://done", &note);
                    Some(format!(
                        "Generated **{title}** — it's in your Studio notes."
                    ))
                }
                Err(err) => Some(format!("Couldn't generate that: {err:#}")),
            }
        }
        ToolAction::RemoveSource(name) => {
            let needle = name.to_lowercase();
            let matches: Vec<&Source> = sources
                .iter()
                .filter(|s| {
                    s.title.to_lowercase().contains(&needle)
                        || (!s.url.is_empty() && host_of(&s.url).to_lowercase().contains(&needle))
                })
                .collect();
            match matches.as_slice() {
                [] => Some(format!("No source matches “{name}”.")),
                [one] => {
                    let title = one.title.clone();
                    match state.db.delete_source(&one.id).await {
                        Ok(()) => Some(format!("Removed **{title}** from this notebook.")),
                        Err(err) => Some(format!("Couldn't remove {title}: {err:#}")),
                    }
                }
                many => {
                    let list = many
                        .iter()
                        .map(|s| format!("- {}", s.title))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(format!(
                        "“{name}” matches {} sources — be more specific:\n{list}",
                        many.len()
                    ))
                }
            }
        }
        ToolAction::RefreshSources(name) => {
            let needle = name.to_lowercase();
            let targets: Vec<&Source> = sources
                .iter()
                .filter(|s| !s.url.is_empty())
                .filter(|s| {
                    needle.is_empty()
                        || s.title.to_lowercase().contains(&needle)
                        || host_of(&s.url).to_lowercase().contains(&needle)
                })
                .collect();
            if targets.is_empty() {
                return Some("No matching URL sources to refresh.".into());
            }
            let mut ok = 0u32;
            let mut failed: Vec<String> = Vec::new();
            for src in &targets {
                let _ = app.emit(
                    "chat://step",
                    StepEvent {
                        label: format!("Refreshing: {}", src.title),
                        transient: false,
                    },
                );
                let result = async {
                    let existing = state
                        .db
                        .get_source(&src.id)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("source vanished"))?;
                    let extracted = crate::capture::extract_url_rescued(&existing.url).await?;
                    reingest(state, &existing, extracted, None, true).await
                }
                .await;
                match result {
                    Ok(_) => ok += 1,
                    Err(err) => failed.push(format!("- {} — {err:#}", src.title)),
                }
            }
            let mut out = format!(
                "Refreshed {ok} of {} URL source{}.",
                targets.len(),
                if targets.len() == 1 { "" } else { "s" }
            );
            if !failed.is_empty() {
                out.push_str(&format!("\n\nFailed:\n{}", failed.join("\n")));
            }
            Some(out)
        }
        ToolAction::CreateTemplate {
            name,
            description,
            prompt,
        } => {
            let name = if name.is_empty() {
                "New template".to_string()
            } else {
                name
            };
            match crate::templates::save_template(None, name, description, prompt) {
                Ok(t) => Some(format!(
                    "Created the \"{}\" template — it's in Studio under More; right-click its tile to edit.",
                    t.name
                )),
                Err(err) => Some(format!("Couldn't save the template: {err}")),
            }
        }
        ToolAction::SaveNote(title) => {
            let history = match state.db.list_messages(notebook_id).await {
                Ok(h) => h,
                Err(err) => return Some(format!("Couldn't read the chat history: {err:#}")),
            };
            // Skip tool confirmations — "that" means the last real answer.
            let Some(last) = history
                .iter()
                .rev()
                .find(|m| m.role == "assistant" && m.kind != "tool" && m.kind != "error")
            else {
                return Some(
                    "There's no previous answer to save yet — ask something first.".to_string(),
                );
            };
            let title = if title.is_empty() {
                last.content
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| {
                        l.trim_start_matches('#')
                            .replace(['*', '`'], "")
                            .trim()
                            .chars()
                            .take(60)
                            .collect()
                    })
                    .unwrap_or_else(|| "Chat answer".to_string())
            } else {
                title
            };
            let ts = now();
            let note = Note {
                id: new_id(),
                notebook_id: notebook_id.to_string(),
                title: title.clone(),
                content: last.content.clone(),
                kind: "note".into(),
                prompt: String::new(),
                origin: String::new(),
                status: String::new(),
                created_at: ts,
                updated_at: ts,
            };
            match add_note_indexed(state, &note).await {
                Ok(()) => Some(format!("Saved the previous answer as note **{title}**.")),
                Err(err) => Some(format!("Couldn't save the note: {err:#}")),
            }
        }
        ToolAction::ScheduleReport {
            kind,
            interval,
            name,
            prompt,
        } => {
            Some(create_schedule_reply(state, notebook_id, &kind, &interval, &name, &prompt).await)
        }
        ToolAction::Commission {
            kind,
            name,
            prompt,
            when,
        } => Some(commission_reply(state, notebook_id, &kind, &name, &prompt, &when).await),
        ToolAction::NightShift { op } => Some(night_shift_reply(state, &op).await),
        ToolAction::Settings { op, field, value } => {
            // The settings tool (RFC-self-resolve phase 3, plus the model
            // verbs of RFC-conversational-setup phase 1). The reply comes
            // back as a tool row via finish_tool_reply, so every applied
            // change is a visible line in the transcript — the config never
            // moves silently. Secrets are refused inside the core fns.
            let mut config = { state.ai.read().await.config().clone() };
            match op.as_str() {
                "get" => Some(crate::selfheal::settings_get(&config)),
                "models" => Some(settings_models_report(app, state).await),
                "test" => Some(settings_test_report(state, &field).await),
                // `pull` stages the command as a one-click Terminal
                // affordance — it is never executed from here.
                "pull" => Some(match crate::selfheal::settings_pull(&field) {
                    Ok(text) | Err(text) => text,
                }),
                "style" => {
                    Some(settings_style_apply(app, state, notebook_id, &field, &value).await)
                }
                "theme" => Some(settings_theme_apply(app, &field)),
                // Read/confirm only — the write happens on the confirm click.
                "connect" => Some(settings_connect_report(app, &field).await),
                "setup" => Some(settings_setup_report(app, state).await),
                _ => match crate::selfheal::settings_set(&mut config, &field, &value) {
                    Ok(echo) => match apply_ai_config(app, state, config).await {
                        Ok(()) => {
                            notify_changed("settings", None);
                            Some(echo)
                        }
                        Err(err) => Some(format!("Couldn't apply that setting: {err}")),
                    },
                    Err(msg) => Some(msg),
                },
            }
        }
        ToolAction::UpdateReport {
            name,
            new_name,
            kind,
            interval,
            prompt,
            enabled,
        } => {
            let schedules = match state.db.list_report_schedules(notebook_id).await {
                Ok(s) => s,
                Err(err) => return Some(format!("Couldn't read report schedules: {err:#}")),
            };
            if schedules.is_empty() {
                return Some(
                    "There are no scheduled reports in this notebook yet — ask me to create one."
                        .to_string(),
                );
            }
            let needle = name.to_lowercase();
            let matches: Vec<_> = schedules
                .iter()
                .filter(|r| r.name.to_lowercase().contains(&needle))
                .collect();
            let mut schedule = match matches.as_slice() {
                [one] => (*one).clone(),
                [] => {
                    let names = schedules
                        .iter()
                        .map(|r| format!("- {}", r.name))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Some(format!(
                        "No report named “{name}” here. The notebook has:\n{names}"
                    ));
                }
                many => {
                    let names = many
                        .iter()
                        .map(|r| format!("- {}", r.name))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Some(format!(
                        "“{name}” matches more than one report:\n{names}\nWhich one did you mean?"
                    ));
                }
            };
            let mut changes = Vec::new();
            if !new_name.trim().is_empty() {
                schedule.name = new_name.trim().to_string();
                changes.push(format!("renamed to “{}”", schedule.name));
            }
            match kind.as_str() {
                "" => {}
                k @ ("summary" | "briefing" | "timeline" | "faq" | "custom") => {
                    schedule.kind = k.to_string();
                    changes.push(format!("generator → {k}"));
                }
                other => return Some(format!("“{other}” isn't a report kind I know — use summary, briefing, timeline, faq, or custom.")),
            }
            match interval.as_str() {
                "" => {}
                "hourly" => {
                    schedule.interval_secs = 3_600;
                    changes.push("cadence → hourly".into());
                }
                "daily" => {
                    schedule.interval_secs = 86_400;
                    changes.push("cadence → daily".into());
                }
                "weekly" => {
                    schedule.interval_secs = 604_800;
                    changes.push("cadence → weekly".into());
                }
                other => {
                    return Some(format!(
                        "I can run reports **hourly**, **daily**, or **weekly** — “{other}” isn't supported, so I haven't changed anything."
                    ));
                }
            }
            if !prompt.trim().is_empty() {
                schedule.prompt = prompt.trim().to_string();
                changes.push("prompt updated".into());
            }
            match enabled.as_str() {
                "" => {}
                "true" => {
                    schedule.enabled = true;
                    changes.push("enabled".into());
                }
                "false" => {
                    schedule.enabled = false;
                    changes.push("paused".into());
                }
                _ => {}
            }
            if changes.is_empty() {
                return Some(format!(
                    "I found **{}** but you didn't say what to change — its name, generator, cadence, prompt, or paused state.",
                    schedule.name
                ));
            }
            match state
                .db
                .update_report_schedule(
                    &schedule.id,
                    &schedule.name,
                    &schedule.kind,
                    &schedule.prompt,
                    &schedule.trigger,
                    schedule.interval_secs,
                    schedule.enabled,
                )
                .await
            {
                Ok(()) => Some(format!(
                    "Updated **{}**: {}.",
                    schedule.name,
                    changes.join(", ")
                )),
                Err(err) => Some(format!("Couldn't update the schedule: {err:#}")),
            }
        }
    }
}

/// Ingest a list of URLs as sources, returning a markdown summary reply.
async fn add_url_sources(
    app: &AppHandle,
    state: &AppState,
    notebook_id: &str,
    urls: &[String],
) -> String {
    let mut added: Vec<Source> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    for url in urls {
        let _ = app.emit(
            "chat://step",
            StepEvent {
                label: format!("Adding source: {}", host_of(url)),
                transient: false,
            },
        );
        let result = if crate::mac::is_mac_uri(url) {
            ingest_mac(state, notebook_id, url, "").await
        } else {
            ingest_url(state, notebook_id, url, None).await
        };
        match result {
            Ok(src) if src.status != "error" => added.push(src),
            Ok(src) => failed.push((url.clone(), src.error)),
            Err(err) => failed.push((url.clone(), format!("{err:#}"))),
        }
    }

    let mut out = String::new();
    if !added.is_empty() {
        out.push_str(&format!(
            "Added {} source{} to this notebook:\n",
            added.len(),
            if added.len() == 1 { "" } else { "s" }
        ));
        for src in &added {
            out.push_str(&format!("- **{}** — {}\n", src.title, host_of(&src.url)));
        }
    }
    if !failed.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{} couldn't be added:\n", failed.len()));
        for (url, err) in &failed {
            out.push_str(&format!("- {} — {}\n", host_of(url), err));
        }
    }
    out
}

// The IPC surface is a flat argument list; one more one-shot knob
// (provider_override, RFC-self-resolve phase 4) tips the count over
// clippy's default without making the call harder to read.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    notebook_id: String,
    content: String,
    config: Option<ChatConfig>,
    source_ids: Option<Vec<String>>,
    provider_override: Option<String>,
) -> Result<Message, String> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err("Message is empty".into());
    }
    let extra = chat_style_instruction(&config.unwrap_or_default());
    // Time-to-first-token clock: from the send to the first chat://token
    // emit. Phase marks land in the chat-timing trace line below.
    let ttft = TtftClock::start();
    let t0 = std::time::Instant::now();

    // Persist the user's turn first.
    let user_msg = Message {
        id: new_id(),
        notebook_id: notebook_id.clone(),
        role: "user".into(),
        content: content.clone(),
        citations: vec![],
        kind: "chat".into(),
        model: String::new(),
        created_at: now(),
    };
    e(state.db.add_message(&user_msg).await)?;

    // Tool: if the user asked to add URLs as sources, do that instead of chat.
    if let Some(reply) = try_tool_route(&app, &state, &notebook_id, &content, true).await {
        return finish_tool_reply(&app, &state, &notebook_id, reply).await;
    }

    // Retrieve relevant chunks. The selected sources are fetched first so
    // retrieval depth can scale with how much text is actually in play
    // (RFC-infinite-context §3) and the manifest reuses the same rows.
    let ai = state.ai.read().await.clone();
    // Embedding the question and listing sources are independent — overlap
    // them; every pre-stream millisecond is felt time-to-first-token.
    let (query_vec, sources_list) =
        tokio::join!(ai.embed_one(&content), state.db.list_sources(&notebook_id));
    let query_vec = e(query_vec)?;
    let embed_ms = t0.elapsed().as_millis() as u64;
    let profile = ai.profile(crate::inference::Role::Chat);
    let selected_sources: Vec<Source> = e(sources_list)?
        .into_iter()
        .filter(|s| source_ids.as_ref().is_none_or(|ids| ids.contains(&s.id)))
        .collect();
    let notebook_chars: i64 = selected_sources.iter().map(|s| s.char_count).sum();
    let k = profile.retrieve_k_for(notebook_chars);
    // Cross-encoder tiers retrieve a 3x pool for the reranker to order —
    // recall from hybrid search, precision from the cross-encoder
    // (BEIR-measured in beir_eval.rs; tier choice in Router::xenc_model).
    let fetch_k = if ai.has_xenc() { k * 3 } else { k };
    let search = e(state
        .db
        .search_chunks_trace(
            &notebook_id,
            query_vec,
            &content,
            fetch_k,
            source_ids.as_deref(),
        )
        .await)?;
    // The ripgrep leg (RFC-git-sources §6): code-shaped queries also
    // exact-match over the notebook's repo-backed files, and the windows
    // join the fusion as ordinary citations.
    let grep_hits = grep_leg(&state, &notebook_id, &content, source_ids.as_deref()).await;
    // Iterative retrieval (RFC-judged-evals §4.3, measured before shipped):
    // the small tier names the evidence still missing, one more search
    // fetches it, and the merged pool reranks. Self-gating — the model
    // answers NONE when the first pass suffices — and it lifted multi-hop
    // gold-evidence citation 48%→60% with zero single-hop regression.
    let mut pool = search.final_hits;
    let gap_query = gap_retrieve(
        &ai,
        &state.db,
        &notebook_id,
        &content,
        &mut pool,
        k,
        fetch_k,
        source_ids.as_deref(),
    )
    .await;
    crate::trace::log(
        &state.trace_dir,
        serde_json::json!({
            "ts": now(),
            "surface": "chat",
            "notebookId": notebook_id,
            "query": content,
            "vectorHits": search.vector_hits.len(),
            "ftsHits": search.fts_hits.len(),
            "fusedHits": search.fused_hits.len(),
            "grepHits": grep_hits.len(),
            "gapQuery": gap_query,
            "warnings": search.warnings,
            "citations": crate::trace::cite_summaries(&pool),
        }),
    );
    let pool_cap = pool.len() + grep_hits.len();
    let citations = fuse_grep_hits(pool, grep_hits, pool_cap);
    let citations = ai.rerank_hits(&content, citations, k).await;
    let retrieval_ms = (t0.elapsed().as_millis() as u64).saturating_sub(embed_ms);
    bump_note_usage(&state.db, &citations, "retrieval_hits").await;

    // Widen prompt excerpts to ordinal neighbors where the model's window
    // affords it; persisted citations stay verbatim.
    let expanded = if profile.neighbor_expansion {
        state
            .db
            .expand_neighbor_excerpts(&citations)
            .await
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    // Full source manifest (title + url + user tags) so corpus-level
    // questions are answerable regardless of which chunks the top-k search
    // happened to surface, and the model can propose new addable URLs.
    // Respects the source selection so deselected sources stay out of the
    // prompt.
    let source_manifest: Vec<(String, String, String)> = selected_sources
        .into_iter()
        .map(|s| (s.title, s.url, s.tags))
        .collect();

    // Build prompt with short history (exclude the just-added user msg from window).
    let history = e(state.db.list_messages(&notebook_id).await)?;
    let history_turns: Vec<crate::ai::ChatTurn> = history
        .iter()
        .filter(|m| m.id != user_msg.id && m.kind != "tool" && m.kind != "error")
        .map(|m| crate::ai::ChatTurn {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();
    let persona = {
        let ai = state.ai.read().await.clone();
        rag::persona_block(&ai.config().profile)
    };
    let messages = rag::build_chat_messages(
        &history_turns,
        &content,
        rag::Excerpts {
            citations: &citations,
            expanded: &expanded,
        },
        &source_manifest,
        &extra,
        &persona,
        &profile,
    );
    // One-shot provider override (RFC-self-resolve phase 4): the error row's
    // "Answer with Ollama / Apple Intelligence" rerun answers THIS question
    // on the named engine — config untouched, one send only.
    let override_id = provider_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    // `model` captions the transcript row; `metrics_key` additionally carries
    // the model and reasoning effort behind it, because those move latency and
    // a speed ranking that pools them measures nothing.
    let (override_engine, model, metrics_key) = {
        let ai = state.ai.read().await.clone();
        match override_id {
            Some(id) => {
                let (engine, model) = ai
                    .engine_for_provider(id)
                    .map_err(|err| friendly_error(&format!("{err:#}")))?;
                let key = ai.chat_metrics_key(Some(id));
                (Some(engine), model, key)
            }
            None => (None, ai.active_chat_model(), ai.chat_metrics_key(None)),
        }
    };
    // On-device model only: cap the prompt to its 8192-token window before
    // streaming (structure-aware — the system rules and the question survive,
    // retrieved excerpts and old history are trimmed). No-op for the
    // larger-window engines, whose prompts must not be shrunk.
    let messages = {
        let ai = state.ai.read().await.clone();
        let budget = match &override_engine {
            Some(crate::inference::ChatEngine::FoundationModels(_)) => {
                Some(crate::inference::budget::fm_input_budget_tokens())
            }
            Some(_) => None,
            None => ai.fm_input_budget(crate::inference::Role::Chat),
        };
        match budget {
            Some(budget) => crate::inference::budget::fit_messages(&messages, budget).into_owned(),
            None => messages,
        }
    };

    // Stream the answer, emitting tokens to the frontend. Race against the
    // cancellation token so a Stop click aborts the request; on cancel we keep
    // whatever partial text streamed so far.
    let app_for_cb = app.clone();
    let cancel = state.begin_generation(&format!("chat:{}", window.label()));
    let partial = Arc::new(Mutex::new(String::new()));
    let partial_cb = partial.clone();
    // 0 = no token yet; the first token stores max(elapsed, 1).
    let ttft_cb = ttft.clone();
    let (answer, kind, stats, cost_usd, model) = {
        let ai = state.ai.read().await.clone();
        let engine = override_engine
            .as_ref()
            .unwrap_or_else(|| ai.engine(crate::inference::Role::Chat));
        // Agent-CLI engines narrate their work (booting, tool calls) through
        // the same step trail the deep-research loop uses — a long silent
        // spinner otherwise reads as a hang.
        let app_for_steps = app.clone();
        let streamed = tokio::select! {
            out = engine.chat_stream_steps(&messages, |tok| {
                ttft_cb.mark();
                partial_cb.lock().unwrap().push_str(tok);
                let _ = app_for_cb.emit(
                    "chat://token",
                    TokenEvent { content: tok.to_string() },
                );
            }, |step: crate::inference::Step<'_>| {
                let _ = app_for_steps.emit(
                    "chat://step",
                    StepEvent {
                        label: step.label.to_string(),
                        transient: step.transient,
                    },
                );
            }) => Some(out),
            _ = cancel.cancelled() => None,
        };
        match streamed {
            Some(Ok(out)) => (out.text, "chat", out.stats, out.cost_usd, model),
            // A provider failure becomes a durable transcript row instead of
            // a vanishing toast: the stored user turn would otherwise sit
            // unanswered in history with no trace of why. friendly_error
            // turns the known failure shapes into the fix (RFC-self-resolve
            // phase 1), and unclassified shapes get one capped diagnosis
            // call (phase 2) — parse-or-skip, never the failing engine.
            Some(Err(err)) => {
                let raw = format!("{err:#}");
                let mut text = friendly_error(&raw);
                if override_id.is_none() {
                    if let Some(extra) = crate::selfheal::diagnose(&ai, &raw).await {
                        text.push_str(&extra);
                    }
                }
                (text, "error", None, None, model)
            }
            None => (partial.lock().unwrap().clone(), "chat", None, None, model),
        }
    };
    state.record_chat_stats(&metrics_key, stats);
    if kind == "chat" {
        state.record_ttft(
            &metrics_key,
            "chat",
            &notebook_id,
            &ttft,
            Some(serde_json::json!({
                "embedMs": embed_ms,
                "retrievalMs": retrieval_ms,
            })),
        );
    }

    let assistant_msg = Message {
        id: new_id(),
        notebook_id: notebook_id.clone(),
        role: "assistant".into(),
        content: answer,
        citations: if kind == "error" { vec![] } else { citations },
        kind: kind.into(),
        model: model_caption(&model, cost_usd),
        created_at: now(),
    };
    bump_note_usage(&state.db, &assistant_msg.citations, "cited").await;
    e(state.db.add_message(&assistant_msg).await)?;
    e(state.db.touch_notebook(&notebook_id, now()).await)?;
    let _ = app.emit("chat://done", &assistant_msg);
    if assistant_msg.kind != "error" {
        spawn_auto_evidence(
            &app,
            &notebook_id,
            &content,
            &assistant_msg.content,
            &assistant_msg.citations,
        );
        // Verify-and-repair closes behind the delivered answer
        // (RFC-judged-evals §5) — cited answers only; an abstention has
        // nothing to check.
        if !assistant_msg.citations.is_empty() {
            spawn_answer_verify(
                &app,
                assistant_msg.clone(),
                messages.clone(),
                state.trace_dir.clone(),
            );
        }
    }
    Ok(assistant_msg)
}

#[tauri::command]
pub async fn send_message_agentic(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    notebook_id: String,
    content: String,
    config: Option<ChatConfig>,
    source_ids: Option<Vec<String>>,
) -> Result<Message, String> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err("Message is empty".into());
    }
    let extra = chat_style_instruction(&config.unwrap_or_default());

    let user_msg = Message {
        id: new_id(),
        notebook_id: notebook_id.clone(),
        role: "user".into(),
        content: content.clone(),
        citations: vec![],
        kind: "chat".into(),
        model: String::new(),
        created_at: now(),
    };
    e(state.db.add_message(&user_msg).await)?;

    // Tool: add-URL requests are handled the same in deep-research mode.
    if let Some(reply) = try_tool_route(&app, &state, &notebook_id, &content, false).await {
        return finish_tool_reply(&app, &state, &notebook_id, reply).await;
    }

    let history = e(state.db.list_messages(&notebook_id).await)?;
    let history_turns: Vec<crate::ai::ChatTurn> = history
        .iter()
        .filter(|m| m.id != user_msg.id && m.kind != "tool" && m.kind != "error")
        .map(|m| crate::ai::ChatTurn {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();

    let cancel = state.begin_generation(&format!("chat:{}", window.label()));
    let ttft = TtftClock::start();
    let phases = AgentPhases::default();
    let (answer, kind, citations, stats, model, metrics_key) = {
        let ai = state.ai.read().await.clone();
        let model = ai.active_chat_model();
        let metrics_key = ai.chat_metrics_key(None);
        let out = tokio::select! {
            r = crate::agent::run(
                &app,
                &state.db,
                &ai,
                &notebook_id,
                &content,
                &history_turns,
                &extra,
                source_ids.as_deref(),
                &ttft,
                &phases,
            ) => Some(r),
            _ = cancel.cancelled() => None,
        };
        match out {
            Some(Ok((answer, citations, stats))) => {
                (answer, "chat", citations, stats, model, metrics_key)
            }
            // Durable transcript row for a failed run — same contract as the
            // direct chat path: fix-classified (phase 1), then one capped
            // diagnosis call for unclassified shapes (phase 2).
            Some(Err(err)) => {
                let raw = format!("{err:#}");
                let mut text = friendly_error(&raw);
                if let Some(extra) = crate::selfheal::diagnose(&ai, &raw).await {
                    text.push_str(&extra);
                }
                (text, "error", vec![], None, model, metrics_key)
            }
            None => (
                "_(Stopped.)_".to_string(),
                "chat",
                vec![],
                None,
                model,
                metrics_key,
            ),
        }
    };
    state.record_chat_stats(&metrics_key, stats);
    if kind == "chat" {
        state.record_ttft(
            &metrics_key,
            "deep-research",
            &notebook_id,
            &ttft,
            Some(phases.as_json()),
        );
    }

    let assistant_msg = Message {
        id: new_id(),
        notebook_id: notebook_id.clone(),
        role: "assistant".into(),
        content: answer,
        citations,
        kind: kind.into(),
        model: model_caption(&model, None),
        created_at: now(),
    };
    bump_note_usage(&state.db, &assistant_msg.citations, "cited").await;
    e(state.db.add_message(&assistant_msg).await)?;
    e(state.db.touch_notebook(&notebook_id, now()).await)?;
    let _ = app.emit("chat://done", &assistant_msg);
    if assistant_msg.kind != "error" {
        spawn_auto_evidence(
            &app,
            &notebook_id,
            &content,
            &assistant_msg.content,
            &assistant_msg.citations,
        );
    }
    Ok(assistant_msg)
}

/// Stop an in-flight generation. `scope` is "chat" or "artifact"; omitted
/// cancels everything (legacy behavior).
#[tauri::command]
pub fn cancel_generation(
    state: State<'_, AppState>,
    window: tauri::WebviewWindow,
    scope: Option<String>,
) {
    // Scopes are per-window so Stop in one window never kills another's stream.
    let scoped = scope.map(|s| format!("{s}:{}", window.label()));
    state.cancel_current(scoped.as_deref());
}

// ---- Notes & artifacts ---------------------------------------------------

#[tauri::command]
pub async fn list_notes(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Vec<Note>, String> {
    e(state.db.list_notes(&notebook_id).await)
}

/// Fire-and-forget post-pass after a chat answer (docs/RFC-note-curator.md
/// phase 3): when the answer synthesized across sources, one model call
/// decides whether the exchange produced a durable conclusion and saves it
/// as an `origin: "auto"` evidence note. Conservative by design — cheap
/// gates first, the model must opt IN, malformed output means skip, and a
/// failure is only ever a log line.
/// Post-stream verify-and-repair (RFC-judged-evals §5): check the finished
/// answer's citations and claim support with the on-device verifier; on a
/// caught defect, ONE repair pass rewrites it and the message row is
/// swapped in place (chat://revised tells windows to re-render). The
/// repaired answer must strictly reduce defects or the original stands —
/// repair can improve-or-hold, never churn. Runs off-thread: the user
/// already has their answer; this is the safety net closing behind it.
fn spawn_answer_verify(
    app: &AppHandle,
    message: Message,
    prompt: Vec<crate::ai::ChatTurn>,
    trace_dir: std::path::PathBuf,
) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let ai = state.ai.read().await.clone();
        let check = crate::verify::check_answer(
            ai.verifier(),
            &message.content,
            &message.citations,
            crate::verify::REPAIR_THRESHOLD,
        )
        .await;
        if check.defects() == 0 {
            crate::trace::log(
                &trace_dir,
                serde_json::json!({
                    "ts": now(),
                    "surface": "verify",
                    "notebookId": message.notebook_id,
                    "messageId": message.id,
                    "defects": 0,
                    "scored": check.scored,
                }),
            );
            return;
        }
        let repair = crate::verify::build_repair_messages(&prompt, &message.content, &check);
        let Ok(out) = ai.chat(&repair).await else {
            return;
        };
        let revised = out.text.trim().to_string();
        if revised.is_empty() {
            return;
        }
        let recheck = crate::verify::check_answer(
            ai.verifier(),
            &revised,
            &message.citations,
            crate::verify::REPAIR_THRESHOLD,
        )
        .await;
        crate::trace::log(
            &trace_dir,
            serde_json::json!({
                "ts": now(),
                "surface": "verify",
                "notebookId": message.notebook_id,
                "messageId": message.id,
                "defects": check.defects(),
                "scored": check.scored,
                "unsupported": check.unsupported,
                "invalidMarkers": check.invalid_markers,
                "repairedDefects": recheck.defects(),
                "applied": check.accepts(&recheck),
            }),
        );
        if !check.accepts(&recheck) {
            return;
        }
        if state
            .db
            .update_message_content(&message.id, &revised)
            .await
            .is_ok()
        {
            let mut updated = message;
            updated.content = revised;
            let _ = app.emit("chat://revised", &updated);
        }
    });
}

fn spawn_auto_evidence(
    app: &AppHandle,
    notebook_id: &str,
    question: &str,
    answer: &str,
    citations: &[Citation],
) {
    // Gate: a conclusion needs synthesis across 2+ distinct SOURCES. Note
    // passages don't count — evidence derived from prior conclusions would
    // be circular. Short answers are lookups, not synthesis.
    let sources: Vec<Citation> = citations
        .iter()
        .filter(|c| !c.source_id.is_empty())
        .cloned()
        .collect();
    let distinct: HashSet<&str> = sources.iter().map(|c| c.source_id.as_str()).collect();
    if distinct.len() < 2 || answer.chars().count() < 400 {
        crate::note!(
            "auto evidence: gate skipped ({} distinct sources, {} chars)",
            distinct.len(),
            answer.chars().count()
        );
        return;
    }
    let app = app.clone();
    let notebook_id = notebook_id.to_string();
    let question = question.to_string();
    let answer = answer.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = auto_evidence(&app, &notebook_id, &question, &answer, &sources).await {
            crate::note!("auto evidence pass failed: {err:#}");
        }
    });
}

/// Overlap coefficient of two titles' word sets (lowercased, alphanumeric,
/// stop-length words dropped) — the cheap same-claim test for deduping auto
/// evidence notes. Shared words over the SMALLER set, not Jaccard: a title
/// that restates another with extra qualifiers should still match.
fn title_overlap(a: &str, b: &str) -> f32 {
    let words = |s: &str| -> HashSet<String> {
        s.to_lowercase()
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .map(str::to_string)
            .collect()
    };
    let (wa, wb) = (words(a), words(b));
    if wa.is_empty() || wb.is_empty() {
        return 0.0;
    }
    let shared = wa.intersection(&wb).count() as f32;
    shared / wa.len().min(wb.len()) as f32
}

async fn auto_evidence(
    app: &AppHandle,
    notebook_id: &str,
    question: &str,
    answer: &str,
    sources: &[Citation],
) -> anyhow::Result<()> {
    use tauri::Manager;
    let state = app.state::<AppState>();

    let draft = {
        let messages = rag::build_auto_evidence_messages(question, answer, sources, None);
        let ai = state.ai.read().await.clone();
        ai.chat(&messages).await?.text
    };
    let Some((title, body)) = rag::parse_auto_evidence(&draft) else {
        // SKIP is the common, correct case — but say so in the terminal so
        // "nothing happened" is diagnosable from the dev console.
        crate::note!(
            "auto evidence: model declined ({} chars): {}",
            draft.len(),
            draft
                .trim()
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(80)
                .collect::<String>()
        );
        return Ok(());
    };

    // Same claim already on record? Merge into it instead of a sibling
    // (Hermes' patch-over-create). Only auto notes merge — owned notes are
    // the user's, and the pass never touches them.
    let existing = state
        .db
        .list_notes(notebook_id)
        .await?
        .into_iter()
        .filter(|n| n.kind == "evidence" && n.origin == "auto")
        .find(|n| title_overlap(&n.title, &title) >= 0.6);

    let note = if let Some(prior) = existing {
        let merged = {
            let messages = rag::build_auto_evidence_messages(
                question,
                answer,
                sources,
                Some((&prior.title, &prior.content)),
            );
            let ai = state.ai.read().await.clone();
            ai.chat(&messages).await?.text
        };
        let Some((title, body)) = rag::parse_auto_evidence(&merged) else {
            crate::note!("auto evidence: merge declined for \"{}\"", prior.title);
            return Ok(());
        };
        state
            .db
            .update_note(&prior.id, &title, &body, now())
            .await?;
        // update_note leaves origin untouched, so the record stays "auto"
        // and claims accumulate evidence instead of siblings. Fresh evidence
        // revives a stale/archived record.
        state.db.set_note_status(&prior.id, "").await?;
        match state.db.get_note(&prior.id).await? {
            Some(n) => {
                index_note(&state, &n).await;
                n
            }
            None => return Ok(()),
        }
    } else {
        let ts = now();
        let note = Note {
            id: new_id(),
            notebook_id: notebook_id.to_string(),
            title,
            content: body,
            kind: "evidence".into(),
            // The originating question, kept so the record can be rebuilt.
            prompt: question.to_string(),
            origin: "auto".into(),
            status: String::new(),
            created_at: ts,
            updated_at: ts,
        };
        add_note_indexed(&state, &note).await?;
        crate::note!("auto evidence: created \"{}\"", note.title);
        note
    };

    // The same conclusion lands on the ledger as an anchored assertion —
    // the passive fill (RFC-v12-steward pillar 2): ordinary chat use builds
    // the record, no ceremony. Same discipline as the note: dedup by title
    // overlap against AUTO assertions only, merge instead of siblings, and
    // a failure here never fails the pass.
    let claim = note.title.clone();
    let anchors: Vec<crate::models::LedgerAnchor> = {
        let mut seen = HashSet::new();
        sources
            .iter()
            // Gist rows are distilled, not verbatim — no anchor material.
            .filter(|c| !c.gist && seen.insert(c.source_id.clone()))
            .take(4)
            .map(|c| crate::models::LedgerAnchor {
                source_id: c.source_id.clone(),
                quote: c.snippet.chars().take(220).collect(),
            })
            .collect()
    };
    let prior_entry = state
        .db
        .list_ledger(notebook_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.kind == "assertion" && entry.origin == "auto")
        .find(|entry| title_overlap(&entry.text, &claim) >= 0.6);
    let ledger_result = match prior_entry {
        Some(mut prior) => {
            prior.text = claim;
            // Fresh evidence revives a stale row; a contradicted one stays
            // contradicted — only the user (or, later, the Weave) clears it.
            if prior.status == "stale" {
                prior.status = "asserted".into();
            }
            for anchor in anchors {
                if !prior
                    .anchors
                    .iter()
                    .any(|a| a.source_id == anchor.source_id)
                {
                    prior.anchors.push(anchor);
                }
            }
            prior.anchors.truncate(6);
            prior.updated_at = now();
            state.db.update_ledger_entry(&prior).await
        }
        None => {
            let ts = now();
            state
                .db
                .add_ledger_entry(&crate::models::LedgerEntry {
                    id: new_id(),
                    notebook_id: notebook_id.to_string(),
                    kind: "assertion".into(),
                    text: claim,
                    why: format!(
                        "From chat: {}",
                        question.chars().take(160).collect::<String>()
                    ),
                    status: "asserted".into(),
                    origin: "auto".into(),
                    anchors,
                    created_at: ts,
                    updated_at: ts,
                })
                .await
        }
    };
    if let Err(err) = ledger_result {
        crate::note!("auto evidence: ledger write failed: {err:#}");
    }

    // Same event the MCP server emits — open windows refresh their notes
    // list live, with the arrival chime announcing the new record.
    #[derive(serde::Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    struct Changed<'a> {
        scope: &'a str,
        notebook_id: Option<&'a str>,
    }
    let _ = app.emit(
        "mcp://changed",
        Changed {
            scope: "notes",
            notebook_id: Some(&note.notebook_id),
        },
    );
    let _ = app.emit(
        "mcp://changed",
        Changed {
            scope: "ledger",
            notebook_id: Some(&note.notebook_id),
        },
    );
    Ok(())
}

/// Bump a usage counter for every note among these citations (best-effort;
/// counters are advisory, never worth failing a chat over).
pub async fn bump_note_usage(db: &Db, citations: &[Citation], field: &str) {
    let ids: Vec<String> = citations
        .iter()
        .filter(|c| !c.note_id.is_empty())
        .map(|c| c.note_id.clone())
        .collect();
    if ids.is_empty() {
        return;
    }
    if let Err(err) = db.bump_note_usage(&ids, field, now()).await {
        crate::note!("note usage bump ({field}) failed: {err:#}");
    }
}

/// Persist a new note and index it for retrieval. Indexing is best-effort:
/// the note row is the truth, chunks are derived — a failed embed logs and
/// the startup backfill retries next launch.
pub async fn add_note_indexed(state: &AppState, note: &Note) -> anyhow::Result<()> {
    state.db.add_note(note).await?;
    index_note(state, note).await;
    Ok(())
}

/// (Re)build a note's chunks in the retrieval index so search and chat can
/// recall prior conclusions (docs/RFC-note-curator.md, phase 1). Chunks ride
/// the source chunk table under `source_id = "note:<id>"`.
pub async fn index_note(state: &AppState, note: &Note) {
    if let Err(err) = try_index_note(state, note).await {
        crate::note!("indexing note {} failed: {err:#}", note.id);
    }
}

async fn try_index_note(state: &AppState, note: &Note) -> anyhow::Result<()> {
    state.db.delete_note_chunks(&note.id).await?;
    // Audio Overview scripts are two-host podcast dialogue — retrieval noise.
    if note.kind == "audio_overview" {
        return Ok(());
    }
    let chunks = ingest::chunk_text(&note.title, &note.content);
    if chunks.is_empty() {
        return Ok(());
    }
    let inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let embeddings = {
        let ai = state.ai.read().await.clone();
        ai.embed(&inputs).await?
    };
    let tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(j, c)| (new_id(), j as i32, c.text.clone()))
        .collect();
    state
        .db
        .add_chunks(
            &note.notebook_id,
            &format!("{}{}", crate::db::NOTE_CHUNK_PREFIX, note.id),
            &tuples,
            &embeddings,
        )
        .await
}

#[tauri::command]
pub async fn create_note(
    state: State<'_, AppState>,
    notebook_id: String,
    title: String,
    content: String,
) -> Result<Note, String> {
    let ts = now();
    let note = Note {
        id: new_id(),
        notebook_id,
        title: if title.trim().is_empty() {
            "Untitled note".into()
        } else {
            title.trim().to_string()
        },
        content,
        kind: "note".into(),
        prompt: String::new(),
        origin: String::new(),
        status: String::new(),
        created_at: ts,
        updated_at: ts,
    };
    e(add_note_indexed(&state, &note).await)?;
    Ok(note)
}

/// The fields of a deleted note the undo toast carries back.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoredNote {
    pub notebook_id: String,
    pub title: String,
    pub content: String,
    pub kind: String,
    pub prompt: String,
    pub origin: String,
    pub status: String,
}

/// Re-insert a note deleted moments ago — the undo half of the note-delete
/// toast. Fresh id (the old row and its index entry are gone), everything
/// else verbatim, so studio artifacts keep their kind and viewer.
#[tauri::command]
pub async fn restore_note(state: State<'_, AppState>, note: RestoredNote) -> Result<Note, String> {
    let ts = now();
    let note = Note {
        id: new_id(),
        notebook_id: note.notebook_id,
        title: note.title,
        content: note.content,
        kind: note.kind,
        prompt: note.prompt,
        origin: note.origin,
        status: note.status,
        created_at: ts,
        updated_at: ts,
    };
    e(add_note_indexed(&state, &note).await)?;
    Ok(note)
}

#[tauri::command]
pub async fn update_note(
    state: State<'_, AppState>,
    id: String,
    title: String,
    content: String,
) -> Result<(), String> {
    e(state
        .db
        .update_note(&id, title.trim(), &content, now())
        .await)?;
    // A deliberate edit takes ownership and revives: the curator stops
    // managing it, and a stale/archived note comes back to life.
    e(state.db.set_note_origin(&id, "").await)?;
    e(state.db.set_note_status(&id, "").await)?;
    if let Some(note) = e(state.db.get_note(&id).await)? {
        index_note(&state, &note).await;
    }
    Ok(())
}

/// The frontend calls this when a note is actually opened (not on list
/// render) — the "reads" counter feeds the curator's staleness pass.
#[tauri::command]
pub async fn note_opened(state: State<'_, AppState>, id: String) -> Result<(), String> {
    touch_activity();
    e(state.db.bump_note_usage(&[id], "reads", now()).await)
}

#[tauri::command]
pub async fn delete_note(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    // An Audio Overview's episode file lives outside the DB — remove it too.
    if let Some(path) = audio_path(&app, &id) {
        let _ = std::fs::remove_file(path);
    }
    e(state.db.delete_note(&id).await)
}

/// Bulk note delete (docs/RFC-multi-select.md): one IPC call, three Lance
/// predicate deletes, plus each note's episode audio if it has one.
#[tauri::command]
pub async fn delete_notes(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<(), String> {
    for id in &ids {
        if let Some(path) = audio_path(&app, id) {
            let _ = std::fs::remove_file(path);
        }
    }
    e(state.db.delete_notes(&ids).await)
}

// ---- Audio overview ---------------------------------------------------------

/// Where a note's episode audio lives; None only if the data dir is unknown.
pub(crate) fn audio_path(app: &AppHandle, note_id: &str) -> Option<PathBuf> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().ok()?.join("audio");
    Some(dir.join(format!("{note_id}.m4a")))
}

/// The episode file for a note, if it has been synthesized (frontend player).
#[tauri::command]
pub fn get_audio_path(app: AppHandle, note_id: String) -> Option<String> {
    let path = audio_path(&app, &note_id)?;
    path.exists().then(|| path.display().to_string())
}

#[derive(serde::Serialize, Clone)]
struct AudioProgress {
    done: u32,
    total: u32,
}

fn kokoro_dir(app: &AppHandle) -> anyhow::Result<PathBuf> {
    use tauri::Manager;
    Ok(app.path().app_data_dir()?.join("kokoro"))
}

/// Marker written after a successful test synthesis — the Audio Overview
/// generator only appears in the UI once this exists.
fn kokoro_verified_marker(dir: &std::path::Path) -> PathBuf {
    dir.join(".verified")
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KokoroStatus {
    pub downloaded: bool,
    pub verified: bool,
}

fn kokoro_status_of(dir: &std::path::Path) -> KokoroStatus {
    let downloaded = crate::tts::kokoro_files_present(dir);
    KokoroStatus {
        downloaded,
        verified: downloaded && kokoro_verified_marker(dir).exists(),
    }
}

/// Where the podcast voice model stands: absent, downloaded, or verified.
#[tauri::command]
pub fn kokoro_status(app: AppHandle) -> Result<KokoroStatus, String> {
    Ok(kokoro_status_of(
        &kokoro_dir(&app).map_err(|e2| e2.to_string())?,
    ))
}

/// Download the Kokoro model if needed, then prove it works with a short
/// test synthesis. Drives the Settings → Models "Podcast voices" section;
/// progress streams as `tts://download`. Cancellable via scope "tts".
#[tauri::command]
pub async fn setup_kokoro(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<KokoroStatus, String> {
    #[derive(serde::Serialize, Clone)]
    struct TtsDownload {
        label: String,
        done: u64,
        total: u64,
    }
    let dir = e(kokoro_dir(&app))?;
    let cancel = state.begin_generation("tts");
    let emitter = app.clone();
    let progress: crate::tts::DownloadProgress = std::sync::Arc::new(move |label, done, total| {
        let _ = emitter.emit(
            "tts://download",
            TtsDownload {
                label: label.to_string(),
                done,
                total,
            },
        );
    });
    let result: anyhow::Result<()> = async {
        crate::tts::ensure_kokoro_files(&dir, Some(&progress), &cancel).await?;
        let engine = crate::tts::KokoroEngine::load(&dir).await?;
        let probe = std::env::temp_dir().join("alchemy-kokoro-verify.wav");
        engine
            .synth(
                crate::tts::Speaker::Host,
                "Your Audio Overview voices are ready.",
                &probe,
            )
            .await?;
        let _ = std::fs::remove_file(&probe);
        std::fs::write(kokoro_verified_marker(&dir), b"ok")?;
        Ok(())
    }
    .await;
    // Always clear the download overlay, even on failure.
    let _ = app.emit(
        "tts://download",
        TtsDownload {
            label: "done".into(),
            done: 1,
            total: 1,
        },
    );
    e(result)?;
    Ok(kokoro_status_of(&dir))
}

/// Delete the downloaded voice model (frees ~93 MB; the generator hides).
#[tauri::command]
pub fn remove_kokoro(app: AppHandle) -> Result<KokoroStatus, String> {
    let dir = kokoro_dir(&app).map_err(|e2| e2.to_string())?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e2| e2.to_string())?;
    }
    Ok(kokoro_status_of(&dir))
}

/// Copy a note's episode audio to a user-chosen destination (Save dialog).
#[tauri::command]
pub fn export_audio(app: AppHandle, note_id: String, dest: String) -> Result<(), String> {
    let src = audio_path(&app, &note_id).ok_or("could not resolve the app data dir")?;
    if !src.exists() {
        return Err("This note has no audio yet.".into());
    }
    std::fs::copy(&src, &dest).map_err(|e2| e2.to_string())?;
    Ok(())
}

/// Synthesize an Audio Overview script into `<data>/audio/<note_id>.m4a`,
/// emitting `audio://progress` per line. Cancellable between lines via the
/// artifact cancel token, so Stop works during the long synthesis tail.
async fn synthesize_audio(
    app: &AppHandle,
    note_id: &str,
    script: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    let lines = crate::tts::parse_script(script);
    anyhow::ensure!(
        !lines.is_empty(),
        "The script contained no HOST/GUEST lines to synthesize."
    );
    let out = audio_path(app, note_id).context("could not resolve the app data dir")?;
    std::fs::create_dir_all(out.parent().unwrap())?;
    // Rebuilds overwrite the previous episode.
    let _ = std::fs::remove_file(&out);

    // Kokoro is the only voice, and generation never kicks off a 93 MB
    // download behind the user's back — the model is set up (and verified)
    // from Settings → Models, and the generator is hidden until then.
    let dir = kokoro_dir(app)?;
    anyhow::ensure!(
        crate::tts::kokoro_files_present(&dir),
        "The Audio Overview voices aren't set up. Download them in Settings → Models."
    );
    let engine = crate::tts::KokoroEngine::load(&dir).await?;

    // Pause lengths between turns follow the dialogue: a beat after a
    // question, snappy for short interjections, a steady gap otherwise.
    let gaps: Vec<u32> = lines
        .windows(2)
        .map(|w| {
            if w[1].text.chars().count() < 25 || w[1].text.starts_with(['—', '-']) {
                180
            } else if w[0].text.ends_with('?') {
                420
            } else {
                300
            }
        })
        .collect();

    let scratch = std::env::temp_dir().join(format!("alchemy-audio-{note_id}"));
    std::fs::create_dir_all(&scratch)?;
    let total = lines.len() as u32;
    let mut wavs = Vec::with_capacity(lines.len());
    let result: anyhow::Result<()> = async {
        for (i, line) in lines.iter().enumerate() {
            anyhow::ensure!(!cancel.is_cancelled(), "Generation stopped.");
            let wav = scratch.join(format!("line-{i:04}.wav"));
            engine.synth(line.speaker, &line.text, &wav).await?;
            wavs.push(wav);
            let _ = app.emit(
                "audio://progress",
                AudioProgress {
                    done: (i + 1) as u32,
                    total,
                },
            );
        }
        crate::tts::assemble_episode(&wavs, &gaps, &out, crate::tts::KokoroEngine::SAMPLE_RATE)
            .await
    }
    .await;
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Turn a note into a standalone source (chunked/embedded), then remove the note.
#[tauri::command]
pub async fn convert_note_to_source(
    state: State<'_, AppState>,
    note_id: String,
) -> Result<Source, String> {
    let note = e(state.db.get_note(&note_id).await)?.ok_or_else(|| "Note not found".to_string())?;
    let extracted = ingest::Extracted {
        image_url: String::new(),
        author: String::new(),
        title: note.title.clone(),
        source_type: "text".to_string(),
        url: String::new(),
        text: note.content.clone(),
    };
    let source = e(store_extracted(&state, &note.notebook_id, extracted).await)?;
    // Remove the original note now that it lives as a source.
    e(state.db.delete_note(&note_id).await)?;
    Ok(source)
}

/// Generate artifact content for a kind (+ optional custom prompt) over all of
/// a notebook's source text. Returns (title, content). When `app` is given,
/// tokens stream to the UI as `artifact://token` events. `source_ids` limits
/// the corpus to those sources; None uses everything. `prior_report` is the
/// previous run's output for scheduled reports — included so the model can
/// report what changed since, instead of apologizing that it can't.
#[allow(clippy::too_many_arguments)]
async fn generate_content(
    state: &AppState,
    app: Option<&AppHandle>,
    notebook_id: &str,
    kind: &str,
    prompt: &str,
    source_ids: Option<&[String]>,
    prior_report: Option<&str>,
    provider: Option<&str>,
) -> anyhow::Result<(String, String)> {
    // Instruction base by kind precedence: "template:<id>" resolves the
    // template at RUN time (a schedule tracks the template's current body,
    // not a snapshot; deleted template = hard error, because a report
    // silently doing something else is worse), registry kinds use their
    // spec, and "custom"/unknown kinds use the prompt itself. A trailing
    // user prompt augments the first two the same way.
    let augment = |base: &str| {
        if prompt.trim().is_empty() {
            base.to_string()
        } else {
            format!(
                "{base}\n\nAdditional instructions from the user (follow these):\n{}",
                prompt.trim()
            )
        }
    };
    let (title, mut instruction) = if let Some(template_id) = kind.strip_prefix("template:") {
        let t = crate::templates::list_templates()
            .map_err(|e| anyhow::anyhow!(e))?
            .into_iter()
            .find(|t| t.id == template_id)
            .ok_or_else(|| {
                anyhow::anyhow!("template '{template_id}' no longer exists — edit this schedule")
            })?;
        let instr = augment(&t.prompt);
        (t.name, instr)
    } else {
        match rag::artifact_spec(kind) {
            Some((t, base)) => (t.to_string(), augment(base)),
            None => {
                if prompt.trim().is_empty() {
                    anyhow::bail!("No instructions provided for this generation.");
                }
                ("Report".to_string(), prompt.trim().to_string())
            }
        }
    };
    if prior_report.is_some() {
        instruction.push_str(
            "\n\nThe corpus ends with a \"Previous report run\" section holding this report's \
             last output (its first line carries the run timestamp). Use it ONLY to identify \
             what is new, changed, or gone since that run, and call those changes out — do not \
             treat it as a source of current facts.",
        );
    }

    let mut sources = state.db.list_sources(notebook_id).await?;
    if sources.is_empty() {
        anyhow::bail!("Add at least one source before generating.");
    }
    if let Some(ids) = source_ids {
        sources.retain(|s| ids.contains(&s.id));
        if sources.is_empty() {
            anyhow::bail!("No sources are selected. Select at least one source, then retry.");
        }
    }
    // Budget the corpus fairly across sources (waterfill): every source is
    // represented, small ones donate unused budget to large ones. A blunt
    // head-truncation previously dropped later sources entirely.
    // Sized to the engine that will read the prompt — the old gateway/local
    // binary handed the on-device tier ~6k tokens against a 4,096 window.
    let (is_gateway, budget) = {
        let ai = state.ai.read().await;
        (
            ai.config().is_gateway(),
            ai.corpus_chars(crate::inference::Role::Generate),
        )
    };

    // One projected batch read — this was one full table scan per source on
    // every Generate click and every scheduled report run.
    let ids: Vec<String> = sources.iter().map(|s| s.id.clone()).collect();
    let mut full_by_id = state.db.source_contents(&ids).await?;
    let mut contents = Vec::with_capacity(sources.len());
    for s in &sources {
        let full = full_by_id.remove(&s.id).unwrap_or_default();
        // URL sources get a "Source URL:" line under their heading so
        // generated notes can cite where each finding can be viewed. File
        // sources carry their on-disk path under a "Source file:" label.
        let heading = if s.url.is_empty() {
            format!("## {}", s.title)
        } else if is_web_url(&s.url) {
            format!("## {}\nSource URL: {}", s.title, s.url)
        } else {
            format!("## {}\nSource file: {}", s.title, s.url)
        };
        contents.push((heading, full));
    }
    // Waterfill: allocate smallest-first so leftovers flow to bigger sources.
    let mut order: Vec<usize> = (0..contents.len()).collect();
    order.sort_by_key(|&i| contents[i].1.chars().count());
    let mut remaining = budget;
    let mut alloc = vec![0usize; contents.len()];
    for (pos, &i) in order.iter().enumerate() {
        let share = remaining / (order.len() - pos);
        let want = contents[i].1.chars().count();
        alloc[i] = want.min(share);
        remaining -= alloc[i];
    }

    // The distiller can only absorb so much of an over-budget source's tail.
    let distill_cap = if is_gateway {
        crate::agent::READ_CHARS_GATEWAY
    } else {
        crate::agent::READ_CHARS_LOCAL
    };
    let mut corpus = String::new();
    for (i, (heading, full)) in contents.iter().enumerate() {
        let total = full.chars().count();
        if total <= alloc[i] {
            corpus.push_str(&format!("{heading}\n\n{full}\n\n"));
            continue;
        }
        // Over budget: keep the head that fits, then distill the part that
        // would have been dropped against the instruction, so a truncated
        // source still contributes its relevant passages instead of silently
        // losing everything past the cut.
        let clipped: String = full.chars().take(alloc[i]).collect();
        let tail: String = full.chars().skip(alloc[i]).take(distill_cap).collect();
        let rescued = {
            let ai = state.ai.read().await.clone();
            crate::agent::distill(&ai, &instruction, heading, &tail).await
        };
        corpus.push_str(&format!(
            "{heading}\n\n{clipped}\n…[source truncated to fit context; key passages from the \
             remainder:]\n{rescued}\n\n"
        ));
    }
    // The prior run rides outside the source budget with its own cap: it
    // informs the "what changed" framing but must never crowd out sources —
    // a third of the corpus budget, so a 4k-token window (on-device tier)
    // isn't eaten by last week's report.
    if let Some(prior) = prior_report {
        let cap = (budget / 3).min(if is_gateway { 40_000 } else { 8_000 });
        let clipped: String = prior.chars().take(cap).collect();
        corpus.push_str(&format!(
            "## Previous report run (for change tracking — not a source)\n\n{clipped}\n\n"
        ));
    }
    let persona = {
        let ai = state.ai.read().await.clone();
        rag::persona_block(&ai.config().profile)
    };
    let messages = rag::build_artifact_messages(&instruction, &corpus, &persona);
    let mut content = run_generation_chat(state, app, &messages, provider).await?;

    // A twenty-minute episode is ~3,000 words, and chat models routinely fade
    // early. Continue the episode (dropping any premature outro) until it's
    // within reach of the target or the model has nothing more to add.
    if kind == "audio_overview" {
        const TARGET_WORDS: usize = 3000;
        for _ in 0..3 {
            let words = content.split_whitespace().count();
            if words >= TARGET_WORDS * 8 / 10 {
                break;
            }
            let trimmed = strip_outro(&content);
            let messages = rag::build_audio_continuation(&instruction, &corpus, &persona, &trimmed);
            let more = run_generation_chat(state, app, &messages, provider).await?;
            // A tiny continuation means the model considers the episode done.
            if more.split_whitespace().count() < 100 {
                break;
            }
            content = format!("{}\n{}", trimmed.trim_end(), more.trim());
        }
    }
    Ok((title.to_string(), content))
}

/// One artifact-generation chat call: stream tokens to the UI when a window
/// is listening, and record model throughput either way.
async fn run_generation_chat(
    state: &AppState,
    app: Option<&AppHandle>,
    messages: &[crate::ai::ChatTurn],
    provider: Option<&str>,
) -> anyhow::Result<String> {
    let (text, stats, model) = {
        let ai = state.ai.read().await.clone();
        // A per-call provider override (the MCP generate tool's optional
        // field) resolves to one configured entry's engine and bypasses role
        // routing for THIS call only — host settings still own every default.
        let overridden = provider.map(|id| ai.engine_for_provider(id)).transpose()?;
        // On-device model only: cap the corpus prompt to its context window
        // before generating (the instruction survives at the head; the source
        // body is trimmed to fit). With an override, budget by the engine
        // that will actually answer; otherwise streaming runs the Generate
        // role, the non-streaming summary path the Chat role.
        let needs_fm_budget = match &overridden {
            Some((engine, _)) => {
                matches!(engine, crate::inference::ChatEngine::FoundationModels(_))
            }
            None => {
                let role = if app.is_some() {
                    crate::inference::Role::Generate
                } else {
                    crate::inference::Role::Chat
                };
                ai.fm_input_budget(role).is_some()
            }
        };
        let budgeted = needs_fm_budget.then(|| {
            crate::inference::budget::fit_messages(
                messages,
                crate::inference::budget::fm_input_budget_tokens(),
            )
            .into_owned()
        });
        let messages: &[crate::ai::ChatTurn] = budgeted.as_deref().unwrap_or(messages);
        let out = match (&overridden, app) {
            (Some((engine, _)), Some(app)) => {
                let app = app.clone();
                engine
                    .chat_stream(messages, move |tok| {
                        let _ = app.emit(
                            "artifact://token",
                            TokenEvent {
                                content: tok.to_string(),
                            },
                        );
                    })
                    .await?
            }
            (Some((engine, _)), None) => engine.chat(messages).await?,
            (None, Some(app)) => {
                let app = app.clone();
                ai.chat_role_stream(crate::inference::Role::Generate, messages, move |tok| {
                    let _ = app.emit(
                        "artifact://token",
                        TokenEvent {
                            content: tok.to_string(),
                        },
                    );
                })
                .await?
            }
            (None, None) => ai.chat(messages).await?,
        };
        let model = match overridden {
            Some((_, model)) => model,
            None => ai.active_chat_model(),
        };
        (out.text, out.stats, model)
    };
    state.record_chat_stats(&model, stats);
    Ok(text)
}

/// Drop a premature sign-off from the tail of a dialogue script so a
/// continuation can pick up mid-episode instead of talking past a goodbye.
pub(crate) fn strip_outro(script: &str) -> String {
    const MARKERS: [&str; 6] = [
        "thanks for listening",
        "thanks for tuning",
        "until next time",
        "that's a wrap",
        "see you next",
        "signing off",
    ];
    let lines: Vec<&str> = script.lines().collect();
    let mut end = lines.len();
    // Only the last few lines can be an outro; a mid-episode "thanks" is fine.
    for (i, line) in lines.iter().enumerate().skip(lines.len().saturating_sub(4)) {
        let l = line.to_lowercase();
        if MARKERS.iter().any(|m| l.contains(m)) {
            end = i;
            break;
        }
    }
    lines[..end].join("\n")
}

#[tauri::command]
pub async fn generate_artifact(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    notebook_id: String,
    kind: String,
    prompt: Option<String>,
    source_ids: Option<Vec<String>>,
) -> Result<Note, String> {
    let prompt = prompt.unwrap_or_default();
    let cancel = state.begin_generation(&format!("artifact:{}", window.label()));
    let produced = tokio::select! {
        r = generate_content(&state, Some(&app), &notebook_id, &kind, &prompt, source_ids.as_deref(), None, None) => Some(e(r)?),
        _ = cancel.cancelled() => None,
    };
    let (title, content) = match produced {
        Some(t) => t,
        None => return Err("Generation stopped.".into()),
    };

    let ts = now();
    let note = Note {
        id: new_id(),
        notebook_id,
        title,
        content,
        kind,
        prompt,
        origin: String::new(),
        status: String::new(),
        created_at: ts,
        updated_at: ts,
    };
    // Audio overviews synthesize the episode before the note is saved, so a
    // failed or stopped synthesis never leaves a half-built artifact behind.
    if note.kind == "audio_overview" {
        e(synthesize_audio(&app, &note.id, &note.content, &cancel).await)?;
    }
    e(add_note_indexed(&state, &note).await)?;
    let _ = app.emit("generate://done", &note);
    Ok(note)
}

/// Start a generation and return its placeholder note immediately; content
/// arrives in the background. Built for MCP's generate tool — MCP clients
/// time out long calls, so the agent gets the id now and polls get_note.
/// The placeholder carries status "generating"; completion clears it and
/// indexes the note, failure sets status "error" with the reason as content.
pub async fn start_generation_detached(
    app: &AppHandle,
    notebook_id: &str,
    kind: &str,
    prompt: &str,
    provider: Option<String>,
) -> anyhow::Result<Note> {
    // Fail fast on an unknown provider id: the whole point of the override is
    // an agent steering one call, and a typo should error at the tool call,
    // not as a dead "generating" note discovered by polling.
    if let Some(id) = provider.as_deref() {
        let state = app.state::<AppState>();
        let ai = state.ai.read().await.clone();
        ai.engine_for_provider(id)?;
    }
    // Fail fast on kinds the async path can't honor: audio synthesis needs
    // the window-side player anyway, and a wrong kind should error at the
    // tool call, not twenty seconds into a background task.
    if kind == "audio_overview" {
        anyhow::bail!("audio_overview can't be generated over MCP — use the app's Studio panel");
    }
    let state = app.state::<AppState>();
    let title = if let Some(id) = kind.strip_prefix("template:") {
        crate::templates::list_templates()
            .map_err(|e| anyhow::anyhow!(e))?
            .into_iter()
            .find(|t| t.id == id)
            .map(|t| t.name)
            .ok_or_else(|| anyhow::anyhow!("no template with id {id}"))?
    } else {
        match rag::artifact_spec(kind) {
            Some((t, _)) => format!("{t} (generating…)"),
            None if !prompt.trim().is_empty() => "Report (generating…)".to_string(),
            None => anyhow::bail!(
                "unknown kind \"{kind}\" — use one of {}, template:<id>, or \"custom\" with a prompt",
                rag::ARTIFACT_KINDS.join(", ")
            ),
        }
    };
    let ts = now();
    let note = Note {
        id: new_id(),
        notebook_id: notebook_id.to_string(),
        title,
        content: String::new(),
        kind: kind.to_string(),
        prompt: prompt.to_string(),
        origin: "mcp".to_string(),
        status: "generating".to_string(),
        created_at: ts,
        updated_at: ts,
    };
    // Stored but NOT indexed: an empty in-flight note has nothing for
    // retrieval yet; indexing happens on completion.
    state.db.add_note(&note).await?;

    // Hard deadline over the whole background generation. "generating" must
    // be a bounded state: a polling agent has no other signal, and live
    // verification found a slow headless-CLI provider holding a note in
    // "generating" past twenty minutes. Providers carry their own request
    // timeouts; this is the belt over corpus assembly + provider + retries.
    const DETACHED_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20 * 60);

    let app = app.clone();
    let spawned = note.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let result = match tokio::time::timeout(
            DETACHED_DEADLINE,
            generate_content(
                &state,
                None, // no window streaming — the poller reads the stored note
                &spawned.notebook_id,
                &spawned.kind,
                &spawned.prompt,
                None,
                None,
                provider.as_deref(),
            ),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(anyhow::anyhow!(
                "generation exceeded {} minutes — the model provider may be \
                 overloaded; try again or switch providers",
                DETACHED_DEADLINE.as_secs() / 60
            )),
        };
        let ts = now();
        let outcome = match result {
            Ok((title, content)) => {
                let title = if spawned.kind.starts_with("template:") {
                    spawned.title.clone()
                } else {
                    title
                };
                if let Err(err) = state
                    .db
                    .update_note(&spawned.id, &title, &content, ts)
                    .await
                {
                    crate::note!("mcp generate: persisting result failed: {err:#}");
                    return;
                }
                let _ = state.db.set_note_status(&spawned.id, "").await;
                if let Ok(Some(done)) = state.db.get_note(&spawned.id).await {
                    index_note(&state, &done).await;
                }
                "done"
            }
            Err(err) => {
                let msg = format!("Generation failed: {err:#}");
                let _ = state
                    .db
                    .update_note(&spawned.id, &spawned.title, &msg, ts)
                    .await;
                let _ = state.db.set_note_status(&spawned.id, "error").await;
                "error"
            }
        };
        let _ = app.emit(
            "mcp://changed",
            serde_json::json!({ "scope": "notes", "notebookId": spawned.notebook_id, "outcome": outcome }),
        );
    });
    Ok(note)
}

#[tauri::command]
pub async fn rebuild_note(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    note_id: String,
    notebook_id: String,
    kind: String,
    prompt: String,
) -> Result<Note, String> {
    let cancel = state.begin_generation(&format!("artifact:{}", window.label()));
    let produced = tokio::select! {
        r = generate_content(&state, Some(&app), &notebook_id, &kind, &prompt, None, None, None) => Some(e(r)?),
        _ = cancel.cancelled() => None,
    };
    let (title, content) = match produced {
        Some(t) => t,
        None => return Err("Generation stopped.".into()),
    };
    // Re-synthesize before touching the stored note, so a failed rebuild
    // keeps the old script/audio pair intact.
    if kind == "audio_overview" {
        e(synthesize_audio(&app, &note_id, &content, &cancel).await)?;
    }
    let ts = now();
    e(state.db.update_note(&note_id, &title, &content, ts).await)?;

    let note = Note {
        id: note_id,
        notebook_id,
        title,
        content,
        kind,
        prompt,
        origin: String::new(),
        status: String::new(),
        created_at: ts,
        updated_at: ts,
    };
    index_note(&state, &note).await;
    let _ = app.emit("generate://done", &note);
    Ok(note)
}

/// Which build a window belongs to — Settings → About. Dev and the
/// installed app share a data dir and look identical; this tells them apart.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    pub version: String,
    pub commit: String,
    /// "dev" (cargo debug/tauri dev) | "release" (installed app).
    pub profile: String,
}

#[tauri::command]
pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        commit: env!("ALCHEMY_GIT_SHA").into(),
        profile: if cfg!(debug_assertions) {
            "dev"
        } else {
            "release"
        }
        .into(),
    }
}

#[tauri::command]
pub fn get_model_stats(state: State<'_, AppState>) -> Vec<ModelStat> {
    state.model_stats_snapshot()
}

/// Extract a JSON array of strings from model output (tolerant of surrounding text).
fn parse_string_array(raw: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (raw.find('['), raw.rfind(']')) else {
        return vec![];
    };
    if end <= start {
        return vec![];
    }
    serde_json::from_str::<Vec<String>>(&raw[start..=end]).unwrap_or_default()
}

/// Suggest a few follow-up questions based on the recent conversation.
#[tauri::command]
pub async fn suggest_followups(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Vec<String>, String> {
    let history = e(state.db.list_messages(&notebook_id).await)?;
    if history.is_empty() {
        return Ok(vec![]);
    }
    let chat_only: Vec<&Message> = history
        .iter()
        .filter(|m| m.kind != "tool" && m.kind != "error")
        .collect();
    let start = chat_only.len().saturating_sub(4);
    let mut convo = String::new();
    for m in &chat_only[start..] {
        let c: String = m.content.chars().take(500).collect();
        convo.push_str(&format!("{}: {}\n", m.role, c));
    }
    let messages = vec![
        crate::ai::ChatTurn::system(
            "Suggest follow-up questions. Respond with ONLY a JSON array of exactly 3 short, \
             distinct questions the user might naturally ask next, as strings. No other text.",
        ),
        crate::ai::ChatTurn::user(format!("Conversation so far:\n{convo}\nJSON array:")),
    ];
    let out = {
        let ai = state.ai.read().await.clone();
        e(ai.chat(&messages).await)?.text
    };
    let mut qs = parse_string_array(&out);
    qs.truncate(3);
    Ok(qs)
}

/// One-line themed aphorism for the hero / blank states. Ornament, not
/// content: the frontend caches it daily and falls back to a curated list,
/// so this may fail freely when no chat model is available.
#[tauri::command]
pub async fn generate_epigraph(state: State<'_, AppState>, mood: String) -> Result<String, String> {
    let mood: String = mood.chars().take(120).collect();
    let messages = vec![
        crate::ai::ChatTurn::system(
            "You write epigraphs for Alchemy, a local-first research notebook. \
             Respond with ONLY one original aphorism of 5-14 words about research, \
             knowledge, or transformation, in the voice of an alchemist's notebook, \
             tinted by the given mood. Quiet and plain, not grand; it must not read \
             like a motivational post. Vary the sentence shape — do not write \
             'X is not A but B' or 'no X but Y; no Z but W' antitheses. Never use \
             the words corpus, distill, retrieval, or pipeline. No quotation marks, \
             no attribution, no preamble.",
        ),
        crate::ai::ChatTurn::user(format!("Mood: {mood}")),
    ];
    let out = {
        let ai = state.ai.read().await.clone();
        e(ai.chat(&messages).await)?.text
    };
    Ok(out.trim().to_string())
}

/// A short prose overview of what the notebook's sources cover (not persisted).
#[tauri::command]
pub async fn generate_notebook_summary(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<String, String> {
    let (_t, content) = e(generate_content(
        &state,
        None,
        &notebook_id,
        "custom",
        "Write a 2-4 sentence plain-prose overview of what these sources collectively cover. \
         No lists, headings, or preamble — just the overview.",
        None,
        None,
        None,
    )
    .await)?;
    Ok(content)
}

// ---- Windows ---------------------------------------------------------------

/// Put the macOS stoplights back where they belong. AppKit resets them to
/// their default spot whenever the webview reloads (dev HMR, navigation),
/// and tao only re-applies its inset when its own — webview-covered — view
/// redraws, so the frontend invokes this on every boot. Mirrors tao's
/// `inset_traffic_lights`; keep the inset in sync with tauri.conf.json.
#[tauri::command]
pub fn fix_traffic_lights(window: tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        const INSET_X: f64 = 20.0;
        const INSET_Y: f64 = 26.0;
        let Ok(ns_window_ptr) = window.ns_window() else {
            return;
        };
        let addr = ns_window_ptr as usize;
        let _ = window.run_on_main_thread(move || unsafe {
            use objc2_app_kit::{NSWindow, NSWindowButton};
            let ns_window = &*(addr as *const NSWindow);
            let (Some(close), Some(mini), Some(zoom)) = (
                ns_window.standardWindowButton(NSWindowButton::CloseButton),
                ns_window.standardWindowButton(NSWindowButton::MiniaturizeButton),
                ns_window.standardWindowButton(NSWindowButton::ZoomButton),
            ) else {
                return;
            };
            let Some(container) = close.superview().and_then(|v| v.superview()) else {
                return;
            };
            let close_rect = close.frame();
            let bar_height = close_rect.size.height + INSET_Y;
            let mut bar_rect = container.frame();
            bar_rect.size.height = bar_height;
            bar_rect.origin.y = ns_window.frame().size.height - bar_height;
            container.setFrame(bar_rect);
            let spacing = mini.frame().origin.x - close_rect.origin.x;
            for (i, button) in [&*close, &*mini, &*zoom].into_iter().enumerate() {
                let mut rect = button.frame();
                rect.origin.x = INSET_X + (i as f64 * spacing);
                button.setFrameOrigin(rect.origin);
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Ambient connections: passages related to what the user is writing right
/// now (docs/RFC-document-surface.md phase 3). Embed-only and quiet — no
/// chat model in the loop, so it is fast enough to run on a typing debounce.
#[tauri::command]
pub async fn related_passages(
    state: State<'_, AppState>,
    notebook_id: String,
    text: String,
    limit: Option<usize>,
) -> Result<Vec<Citation>, String> {
    let text = text.trim().to_string();
    // Under a couple dozen characters the paragraph has no retrievable
    // meaning yet — return quietly instead of surfacing noise.
    if text.chars().count() < 24 {
        return Ok(vec![]);
    }
    let vec = {
        let ai = state.ai.read().await.clone();
        e(ai.embed(std::slice::from_ref(&text)).await)?
    }
    .into_iter()
    .next()
    .unwrap_or_default();
    if vec.is_empty() {
        return Ok(vec![]);
    }
    e(state
        .db
        .search_chunks(&notebook_id, vec, &text, limit.unwrap_or(3).min(8), None)
        .await)
}

// ---- Live web view (reader pane) -------------------------------------------
//
// The reader's Cached ⇄ Live toggle: Live embeds the actual page in a child
// webview positioned over the reader body (read-it-later style), so
// JS-heavy pages never bounce to an external browser. The child's label
// matches no capability pattern ("main"/"win-*"), so it can invoke nothing —
// it is a plain browser surface outside the app's IPC boundary.

fn live_label(window: &tauri::Window) -> String {
    format!("live-{}", window.label())
}

fn live_child(window: &tauri::Window) -> Option<tauri::Webview> {
    window
        .webviews()
        .into_iter()
        .find(|w| w.label() == live_label(window))
}

#[tauri::command]
pub fn live_view_open(
    window: tauri::Window,
    url: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(), String> {
    let parsed: tauri::Url = url.parse().map_err(|e| format!("bad url: {e}"))?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err("Only web pages can open in the live view".into());
    }
    if let Some(existing) = live_child(&window) {
        existing.navigate(parsed).map_err(|e| e.to_string())?;
        existing
            .set_position(tauri::LogicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
        existing
            .set_size(tauri::LogicalSize::new(w, h))
            .map_err(|e| e.to_string())?;
        existing.show().map_err(|e| e.to_string())?;
        refocus_main(&window);
        return Ok(());
    }
    let builder = tauri::webview::WebviewBuilder::new(
        live_label(&window),
        tauri::WebviewUrl::External(parsed),
    );
    window
        .add_child(
            builder,
            tauri::LogicalPosition::new(x, y),
            tauri::LogicalSize::new(w, h),
        )
        .map_err(|e| e.to_string())?;
    refocus_main(&window);
    Ok(())
}

/// A freshly created/shown child webview grabs key focus, which would eat
/// the app's shortcuts (⌘K, Esc, j/k) — hand focus back to the app webview.
/// The user reclaims the page by clicking into it.
fn refocus_main(window: &tauri::Window) {
    if let Some(main) = window
        .webviews()
        .into_iter()
        .find(|w| w.label() == window.label())
    {
        let _ = main.set_focus();
    }
}

#[tauri::command]
pub fn live_view_bounds(
    window: tauri::Window,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(), String> {
    if let Some(child) = live_child(&window) {
        child
            .set_position(tauri::LogicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
        child
            .set_size(tauri::LogicalSize::new(w, h))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Hide while an in-app overlay (palette, modal, presentation) is up — a
/// native child webview would otherwise paint over it.
#[tauri::command]
pub fn live_view_visible(window: tauri::Window, visible: bool) -> Result<(), String> {
    if let Some(child) = live_child(&window) {
        if visible {
            child.show().map_err(|e| e.to_string())?;
        } else {
            child.hide().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn live_view_close(window: tauri::Window) -> Result<(), String> {
    if let Some(child) = live_child(&window) {
        child.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Documents in this notebook that link to the given source — the reader's
/// "Linked from" footer. Sources link via absolute URLs (article markdown
/// keeps them); file sources are also matched by filename, which is how
/// relative links in sibling documents refer to them. Notebooks are small,
/// so a content scan per open beats maintaining a link index.
#[tauri::command]
pub async fn source_backlinks(
    state: State<'_, AppState>,
    source_id: String,
) -> Result<Vec<Backlink>, String> {
    let Some(target) = e(state.db.get_source(&source_id).await)? else {
        return Ok(vec![]);
    };
    let mut needles: Vec<String> = Vec::new();
    if !target.url.is_empty() {
        needles.push(target.url.clone());
        if !target.url.starts_with("http") && !target.url.starts_with("cider://") {
            // A file path: relative links from siblings use the filename.
            if let Some(name) = target.url.rsplit('/').next() {
                if name.len() >= 6 {
                    needles.push(name.to_string());
                }
            }
        }
    }
    if needles.is_empty() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    // One projected scan for every sibling's content — this ran a scan per
    // sibling, on every reader open.
    let siblings: Vec<Source> = e(state.db.list_sources(&target.notebook_id).await)?
        .into_iter()
        .filter(|s| s.id != target.id && !matches!(s.source_type.as_str(), "folder" | "obsidian"))
        .collect();
    let ids: Vec<String> = siblings.iter().map(|s| s.id.clone()).collect();
    let contents = e(state.db.source_contents(&ids).await)?;
    for s in siblings {
        let Some(content) = contents.get(&s.id) else {
            continue;
        };
        if needles.iter().any(|n| content.contains(n.as_str())) {
            out.push(Backlink {
                kind: "source".into(),
                id: s.id,
                title: s.title,
            });
        }
    }
    for n in e(state.db.list_notes(&target.notebook_id).await)? {
        if needles.iter().any(|k| n.content.contains(k.as_str())) {
            out.push(Backlink {
                kind: "note".into(),
                id: n.id,
                title: n.title,
            });
        }
    }
    Ok(out)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Backlink {
    pub kind: String,
    pub id: String,
    pub title: String,
}

/// The whole notebook as a link graph — every source and note as a node,
/// every reference between them as an edge (docs/RFC-document-surface.md
/// phase 5). One pass over the notebook's content, unlike `source_backlinks`
/// which answers for a single document and would be quadratic run per node.
#[tauri::command]
pub async fn notebook_graph(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<crate::graph::NotebookGraph, String> {
    let mut docs: Vec<crate::graph::GraphDoc> = Vec::new();
    // One scan for every source AND its text. Fetching content per source
    // meant one full table scan each — hundreds of them, sequentially, which
    // is where the graph pane's multi-second open actually went.
    for s in e(state.db.sources_with_content(&notebook_id).await)? {
        docs.push(crate::graph::GraphDoc {
            id: s.id,
            kind: "source".into(),
            title: s.title,
            source_type: s.source_type,
            url: s.url,
            content: s.content,
        });
    }
    for n in e(state.db.list_notes(&notebook_id).await)? {
        docs.push(crate::graph::GraphDoc {
            id: n.id,
            kind: "note".into(),
            title: n.title,
            source_type: "note".into(),
            url: String::new(),
            content: n.content,
        });
    }
    Ok(crate::graph::build(&docs))
}

/// Glass chrome (experimental): apply or clear window vibrancy so the
/// translucent sidebar chrome shows the desktop blurring through, like
/// native macOS sidebars. The webview windows are configured opaque, so
/// the effect only reads once the frontend also lifts its backgrounds
/// (html.glass — see index.css).
#[tauri::command]
pub fn set_window_glass(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    enabled: bool,
    dark: bool,
    pinned: bool,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager;

        // Re-applying NSGlassEffectView to an already-glassed window stacks
        // a second glass view over the webview and blanks it (frontend
        // reloads re-run init) — no-op identical requests. Only SUCCESSFUL
        // applies are recorded (below), so a failed apply stays retryable.
        let key = (enabled, dark, pinned);
        if state.glass_applied.lock().unwrap().get(window.label()) == Some(&key) {
            return Ok(());
        }

        // Pin the native appearance to the app theme while glass is on so
        // the material matches the palette. Never pin for the System theme
        // (pinned=false): set_theme is app-global on macOS and would freeze
        // prefers-color-scheme, so System must keep following the OS.
        let _ = window.set_theme(if enabled && pinned {
            Some(if dark {
                tauri::Theme::Dark
            } else {
                tauri::Theme::Light
            })
        } else {
            None
        });
        use tauri_plugin_liquid_glass::{LiquidGlassConfig, LiquidGlassExt};
        use window_vibrancy::{apply_vibrancy, clear_vibrancy, NSVisualEffectMaterial};

        // Prefer the real Liquid Glass material (macOS 26+); the plugin
        // itself falls back to NSVisualEffectView on older systems. Light
        // palettes get a white tint — untinted glass goes smoky over dark
        // wallpapers, which reads wrong under a light UI.
        let tint = if dark {
            None
        } else {
            Some("#FFFFFF99".to_string())
        };
        let liquid = window
            .app_handle()
            .get_webview_window(window.label())
            .and_then(|webview| {
                window
                    .liquid_glass()
                    .set_effect(
                        &webview,
                        LiquidGlassConfig {
                            enabled,
                            tint_color: tint,
                            ..Default::default()
                        },
                    )
                    .ok()
            })
            .is_some();
        if !liquid {
            if enabled {
                apply_vibrancy(
                    &window,
                    NSVisualEffectMaterial::UnderWindowBackground,
                    None,
                    None,
                )
                .map_err(|e| e.to_string())?;
            } else {
                clear_vibrancy(&window).map_err(|e| e.to_string())?;
            }
        }
        state
            .glass_applied
            .lock()
            .unwrap()
            .insert(window.label().to_string(), key);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, state, enabled, dark, pinned);
    Ok(())
}

/// Export the calling window's print layout as a PDF — the local-first
/// export path for slide decks and flashcards. With `save_path` the PDF is
/// written silently to that file (NSPrintSaveJob); without it the native
/// print dialog opens. (WKWebView ignores JS window.print(), so the
/// frontend invokes this.)
///
/// Runs the PUBLIC `printOperationWithPrintInfo:` (macOS 11+) instead of
/// wry's `print()`, which drives WKWebView's private print selector and
/// yields correctly-paginated but BLANK pages. The two load-bearing details:
/// the operation's view must be given the webview's frame before running,
/// and the print info carries orientation (landscape for slide decks) and
/// margins so the print CSS controls the page.
#[tauri::command]
pub async fn print_webview(
    window: tauri::WebviewWindow,
    landscape: bool,
    save_path: Option<String>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        {
            let save_path = save_path.clone();
            window
                .with_webview(move |wv| {
                    let result =
                        unsafe { mac_print_webview(wv.inner().cast(), landscape, save_path) };
                    let _ = tx.send(result);
                })
                .map_err(|e| e.to_string())?;
        }
        tauri::async_runtime::spawn_blocking(move || {
            rx.recv().unwrap_or_else(|e| Err(e.to_string()))
        })
        .await
        .map_err(|e| e.to_string())??;
        // The operation runs asynchronously (sheet-modal); for save jobs the
        // finish signal is the file itself — wait until it exists with a
        // stable non-zero size. The frontend keeps the print DOM mounted
        // until this resolves.
        if let Some(path) = save_path {
            let mut last: u64 = 0;
            for _ in 0..300 {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                if size > 0 && size == last {
                    return Ok(());
                }
                last = size;
            }
            return Err("PDF export timed out".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (landscape, save_path);
        window.print().map_err(|e| e.to_string())
    }
}

/// The objc recipe for a working WKWebView print (runs on the main thread).
#[cfg(target_os = "macos")]
unsafe fn mac_print_webview(
    webview: *mut objc2::runtime::AnyObject,
    landscape: bool,
    save_path: Option<String>,
) -> Result<(), String> {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSRect, NSString, NSURL};

    let shared: *mut AnyObject = msg_send![objc2::class!(NSPrintInfo), sharedPrintInfo];
    if shared.is_null() {
        return Err("no print info".into());
    }
    // Work on a copy — sharedPrintInfo is app-global state, and margins or a
    // save disposition must not leak into the user's next real print.
    let print_info: Option<Retained<AnyObject>> = Retained::from_raw(msg_send![shared, copy]);
    let Some(print_info) = print_info else {
        return Err("could not copy print info".into());
    };
    let print_info: *mut AnyObject = Retained::as_ptr(&print_info) as *mut _;

    // NSPaperOrientationLandscape = 1, portrait = 0. Slide pages run
    // edge-to-edge (the print CSS owns the layout); card sheets keep a
    // 16mm-ish margin.
    let orientation: isize = if landscape { 1 } else { 0 };
    let _: () = msg_send![print_info, setOrientation: orientation];
    if landscape {
        // PDF-only jobs take any paper size: make the page exactly 16:9
        // (11in wide) so a slide fills it edge to edge with no white band.
        let size = objc2_foundation::NSSize {
            width: 792.0,
            height: 445.5,
        };
        let _: () = msg_send![print_info, setPaperSize: size];
    }
    let margin: f64 = if landscape { 0.0 } else { 45.0 };
    let _: () = msg_send![print_info, setTopMargin: margin];
    let _: () = msg_send![print_info, setBottomMargin: margin];
    let _: () = msg_send![print_info, setLeftMargin: margin];
    let _: () = msg_send![print_info, setRightMargin: margin];

    // Silent save-to-PDF: job disposition + target URL instead of a panel.
    if let Some(path) = &save_path {
        let disposition = NSString::from_str("NSPrintSaveJob");
        let _: () = msg_send![print_info, setJobDisposition: &*disposition];
        let dict: *mut AnyObject = msg_send![print_info, dictionary];
        let ns_path = NSString::from_str(path);
        let url = NSURL::fileURLWithPath(&ns_path);
        let key = NSString::from_str("NSJobSavingURL");
        let _: () = msg_send![dict, setObject: &*url, forKey: &*key];
    }

    let op: *mut AnyObject = msg_send![webview, printOperationWithPrintInfo: print_info];
    if op.is_null() {
        return Err("webview did not produce a print operation".into());
    }
    // Without a real frame on the operation's view, every page prints blank.
    let bounds: NSRect = msg_send![webview, bounds];
    let view: *mut AnyObject = msg_send![op, view];
    let _: () = msg_send![view, setFrame: bounds];

    let panel = save_path.is_none();
    let _: () = msg_send![op, setShowsPrintPanel: panel];
    let _: () = msg_send![op, setShowsProgressPanel: panel];
    // Sheet-modal (returns immediately), NOT the blocking runOperation: a
    // nested modal run loop inside tao's event handler sends its run-loop
    // observers into a permanent 100%-CPU spin. Completion is observed by
    // the caller (save jobs: the output file reaching a stable size).
    let ns_window: *mut AnyObject = msg_send![webview, window];
    if ns_window.is_null() {
        return Err("webview has no window".into());
    }
    let no_delegate: *mut AnyObject = std::ptr::null_mut();
    let no_selector: Option<objc2::runtime::Sel> = None;
    let no_context: *mut std::ffi::c_void = std::ptr::null_mut();
    let _: () = msg_send![
        op,
        runOperationModalForWindow: ns_window,
        delegate: no_delegate,
        didRunSelector: no_selector,
        contextInfo: no_context
    ];
    Ok(())
}

/// Open another app window — at the home screen, straight into a notebook,
/// or onto a single note (a document-sized reader window). The boot target
/// rides an init script (not the URL) so it works identically under the dev
/// server and the bundled custom protocol.
#[tauri::command]
pub async fn new_window(
    app: AppHandle,
    notebook_id: Option<String>,
    note_id: Option<String>,
) -> Result<(), String> {
    // Note readers get their own label prefix so window-state restores them
    // at reader size, not workspace size (both still match the win-* capability).
    let label = if note_id.is_some() {
        format!("win-note-{}", new_id())
    } else {
        format!("win-{}", new_id())
    };
    let mut boot = match notebook_id {
        Some(id) => format!("window.__ALCHEMY_NOTEBOOK__ = '{}';", id.replace('\'', "")),
        None => "window.__ALCHEMY_FRESH__ = true;".to_string(),
    };
    if let Some(nid) = &note_id {
        boot.push_str(&format!(
            "window.__ALCHEMY_NOTE__ = '{}';",
            nid.replace('\'', "")
        ));
    }
    // Note windows are readers, not workspaces — size them like a document.
    let (w, h, min_w, min_h) = if note_id.is_some() {
        (880.0, 780.0, 480.0, 400.0)
    } else {
        (1280.0, 820.0, 1040.0, 640.0)
    };
    let builder =
        tauri::WebviewWindowBuilder::new(&app, label, tauri::WebviewUrl::App("index.html".into()))
            .title("Alchemy")
            .inner_size(w, h)
            .min_inner_size(min_w, min_h)
            // Transparent like the main window so glass chrome (vibrancy)
            // works in pop-outs too; opaque themes paint over it anyway.
            .transparent(true)
            .initialization_script(&boot);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        // Keep in sync with tauri.conf.json: centers the stoplights in the
        // 48px custom titlebar row.
        .traffic_light_position(tauri::LogicalPosition::new(20.0, 26.0));
    builder.build().map_err(|e2| e2.to_string())?;
    Ok(())
}

/// Refresh Open Recent in place so it reflects the current notebook list.
/// The menu itself is never rebuilt — that would clear the native Window list.
#[tauri::command]
pub async fn rebuild_app_menu(
    app: AppHandle,
    state: State<'_, AppState>,
    recent: State<'_, crate::menu::RecentMenu>,
    tray_recent: State<'_, crate::integrations::TrayRecents>,
) -> Result<(), String> {
    let recents: Vec<(String, String)> = e(state.db.list_notebooks().await)?
        .into_iter()
        .map(|n| (n.id, n.title))
        .collect();
    crate::menu::fill_recents(&app, &recent.0, &recents).map_err(|err| err.to_string())?;
    // The tray's Recent Notebooks mirrors Open Recent.
    crate::menu::fill_recents(&app, &tray_recent.0, &recents).map_err(|err| err.to_string())
}

/// Fill the frontend-owned menu lists — View > Theme (themes.ts is the
/// authority on the 23 schemes) and Notebook > Generate (studioArtifacts.tsx
/// owns the roster). Called at startup and again when the theme changes so
/// the selection dot tracks. In-place mutation, like Open Recent.
#[tauri::command]
pub fn fill_menu_lists(
    app: AppHandle,
    themes_menu: State<'_, crate::menu::ThemeMenu>,
    generate_menu: State<'_, crate::menu::GenerateMenu>,
    themes: Vec<(String, String)>,
    generators: Vec<(String, String)>,
    current_theme: String,
) -> Result<(), String> {
    crate::menu::fill_themes(&app, &themes_menu.0, &themes, &current_theme)
        .map_err(|err| err.to_string())?;
    crate::menu::fill_generators(&app, &generate_menu.0, &generators).map_err(|err| err.to_string())
}

/// The Settings → Shortcuts rows, straight from the menu's command registry
/// — one source of truth (menu.rs::CMD) for both surfaces.
#[tauri::command]
pub fn list_shortcuts() -> Vec<crate::menu::ShortcutRow> {
    crate::menu::shortcut_rows()
}

// ---- Home page: activity, stats, global search ----------------------------

#[tauri::command]
pub async fn list_recent_notes(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<Note>, String> {
    e(state.db.recent_notes(limit.unwrap_or(6)).await)
}

/// The latest report notes across every notebook, newest first — the home
/// page's report reader pages through these.
/// Watcher activity for the Home Staff section (agents get the same signal
/// via the MCP list_source_events tool).
#[tauri::command]
pub async fn list_source_events(
    state: State<'_, AppState>,
    hours: Option<u32>,
) -> Result<Vec<crate::models::SourceEvent>, String> {
    let hours = i64::from(hours.unwrap_or(24));
    e(state
        .db
        .source_events_since(now() - hours * 3_600_000)
        .await)
}

/// Commission one-off overnight work (docs/RFC-night-shift-area.md §1).
/// Mechanically a schedule with a "once" trigger, so it rides every path
/// that already exists — due-ness, running, notification, receipt — and
/// retires itself afterwards. No queue, nothing to recover after a crash.
///
/// `when` is "tonight" (the next 2 AM local) or "now" (the next pass);
/// anything else is treated as tonight, since a commission the user meant
/// for the night should never surprise them by starting immediately.
#[tauri::command]
pub async fn commission_run(
    state: State<'_, AppState>,
    notebook_id: String,
    name: String,
    kind: String,
    prompt: String,
    when: Option<String>,
) -> Result<ReportSchedule, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A commission needs a name.".into());
    }
    let not_before = match when.as_deref() {
        Some("now") => 0,
        _ => crate::scheduler::next_local_hour_ms(2),
    };
    let schedule = ReportSchedule {
        id: new_id(),
        notebook_id,
        name: name.to_string(),
        kind: if kind.trim().is_empty() {
            "custom".into()
        } else {
            kind
        },
        prompt,
        trigger: "once".into(),
        not_before,
        // Unused by the "once" path, but a sane floor keeps the row honest
        // if a user later flips it to a recurring order.
        interval_secs: 86_400,
        enabled: true,
        last_run_at: 0,
        created_at: now(),
    };
    e(state.db.add_report_schedule(&schedule).await)?;
    Ok(schedule)
}

/// Everything queued for tonight: commissions that have not run, plus the
/// recurring orders whose next turn falls in the window. The Tonight view
/// and the "what's planned?" chat question read the same list.
#[tauri::command]
pub async fn tonight_plan(state: State<'_, AppState>) -> Result<Vec<ReportSchedule>, String> {
    let mut all = e(state.db.all_report_schedules().await)?;
    all.retain(|s| s.enabled);
    all.sort_by_key(|s| {
        // Commissions first, in the order they will run; recurring work after.
        if s.trigger == "once" {
            (0, s.not_before)
        } else {
            (1, s.last_run_at + s.interval_secs * 1000)
        }
    });
    Ok(all)
}

/// What the last snapshot did, for the Background Work settings page
/// (docs/RFC-night-shift-area.md §7).
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotStatus {
    /// Epoch ms of the most recent snapshot; 0 when none has been taken.
    pub taken_at: i64,
    pub bytes: u64,
    pub path: String,
    /// Store format version this build reads.
    pub store_version: u32,
}

#[tauri::command]
pub async fn snapshot_status(app: AppHandle) -> Result<SnapshotStatus, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(match crate::backup::latest_snapshot(&data_dir) {
        Some((path, taken_at, bytes)) => SnapshotStatus {
            taken_at,
            bytes,
            path: path.to_string_lossy().to_string(),
            store_version: crate::backup::STORE_VERSION,
        },
        None => SnapshotStatus {
            taken_at: 0,
            bytes: 0,
            path: String::new(),
            store_version: crate::backup::STORE_VERSION,
        },
    })
}

/// Snapshot now rather than waiting for tonight — the "Back up now" button,
/// and what an agent calls before doing something it wants to be able to undo.
#[tauri::command]
pub async fn snapshot_now(app: AppHandle) -> Result<SnapshotStatus, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let dir = data_dir.clone();
    let out = tokio::task::spawn_blocking(move || crate::backup::snapshot(&dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    Ok(SnapshotStatus {
        taken_at: now(),
        bytes: out.bytes,
        path: out.path.to_string_lossy().to_string(),
        store_version: crate::backup::STORE_VERSION,
    })
}

/// Put the newest snapshot back, moving the current store aside first. The
/// app must restart afterwards: the open LanceDB handle points at the store
/// this just replaced.
#[tauri::command]
pub async fn restore_snapshot(app: AppHandle) -> Result<String, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let dir = data_dir.clone();
    let aside = tokio::task::spawn_blocking(move || crate::backup::restore_latest(&dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    Ok(aside.to_string_lossy().to_string())
}

/// The Night Shift's run record (docs/RFC-night-shift-area.md §2). Agents get
/// the same signal via the MCP list_receipts tool.
#[tauri::command]
pub async fn list_receipts(
    state: State<'_, AppState>,
    hours: Option<u32>,
    limit: Option<usize>,
) -> Result<Vec<crate::models::RunReceipt>, String> {
    let hours = i64::from(hours.unwrap_or(24 * 7));
    e(state
        .db
        .list_receipts(now() - hours * 3_600_000, limit.unwrap_or(200))
        .await)
}

/// Run history for one standing order — what the rail shows when an order is
/// selected.
#[tauri::command]
pub async fn receipts_for_schedule(
    state: State<'_, AppState>,
    schedule_id: String,
    limit: Option<usize>,
) -> Result<Vec<crate::models::RunReceipt>, String> {
    e(state
        .db
        .receipts_for_schedule(&schedule_id, limit.unwrap_or(5))
        .await)
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NightShiftStatus {
    pub background_enabled: bool,
    pub paused: bool,
}

#[tauri::command]
pub async fn night_shift_status(state: State<'_, AppState>) -> Result<NightShiftStatus, String> {
    let background_enabled = state.ai.read().await.config().background_enabled;
    Ok(NightShiftStatus {
        background_enabled,
        paused: crate::scheduler::is_paused(),
    })
}

/// The tray's "Pause until morning", callable from the Staff section too;
/// returns the new paused state and keeps the tray label in step.
#[tauri::command]
pub async fn toggle_night_shift_pause(app: AppHandle) -> Result<bool, String> {
    let paused = crate::scheduler::toggle_pause();
    crate::integrations::set_tray_pause_label(&app, paused);
    Ok(paused)
}

#[tauri::command]
pub async fn list_recent_reports(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<Note>, String> {
    e(state.db.recent_reports(limit.unwrap_or(10)).await)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusStats {
    pub sources: i64,
    pub chars: i64,
    pub notes: i64,
    pub ledger: i64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeActivity {
    pub schedules: Vec<ReportSchedule>,
    pub recent_notes: Vec<Note>,
    pub reports: Vec<Note>,
    pub stats: CorpusStats,
}

/// One Home snapshot: one notes read supplies the recent list, report feed,
/// and note count instead of three overlapping corpus scans.
#[tauri::command]
pub async fn home_activity(state: State<'_, AppState>) -> Result<HomeActivity, String> {
    let db = state.db.clone();
    let (schedules, activity) = tokio::join!(db.all_report_schedules(), db.home_activity(5, 50));
    let schedules = e(schedules)?;
    let (recent_notes, reports, sources, chars, notes, ledger) = e(activity)?;
    Ok(HomeActivity {
        schedules,
        recent_notes,
        reports,
        stats: CorpusStats {
            sources,
            chars,
            notes,
            ledger,
        },
    })
}

#[tauri::command]
pub async fn corpus_stats(state: State<'_, AppState>) -> Result<CorpusStats, String> {
    let (sources, chars, notes, ledger) = e(state.db.corpus_stats().await)?;
    Ok(CorpusStats {
        sources,
        chars,
        notes,
        ledger,
    })
}

/// Everything Settings → Activity renders — see activity.rs and
/// docs/RFC-activity-view.md. Read-only; aggregated fresh per call.
#[tauri::command]
pub async fn activity_stats(
    state: State<'_, AppState>,
) -> Result<crate::models::ActivityStats, String> {
    let messages = e(state.db.message_activity().await)?;
    let notes = e(state.db.note_activity().await)?;
    let sources = e(state.db.source_activity().await)?;
    let all_notebooks = e(state.db.list_notebooks().await)?;
    // Archived and system notebooks stay out of the "most active" ranking:
    // both are already absent from the shelf, and neither is where the
    // user's attention currently goes. Their turns still count everywhere
    // else — totals, heatmap, peak hour — because they really happened.
    let ranked_out: std::collections::HashSet<String> = all_notebooks
        .iter()
        .filter(|n| n.status == "archived" || n.status == "system")
        .map(|n| n.id.clone())
        .collect();
    let titles: std::collections::HashMap<String, String> =
        all_notebooks.into_iter().map(|n| (n.id, n.title)).collect();
    let retrievals = crate::activity::trace_times(&state.trace_dir);
    Ok(crate::activity::aggregate(
        &messages,
        &notes,
        &sources,
        &titles,
        &ranked_out,
        &retrievals,
        chrono::Local::now().date_naive(),
    ))
}

// ---- OKF export ------------------------------------------------------------

/// Kebab-case a title into a filesystem/URL-safe slug.
pub(crate) fn okf_slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(60).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "untitled".into()
    } else {
        out
    }
}

/// Double-quote a string for YAML frontmatter.
fn yaml_str(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', " ")
    )
}

/// First ~140 chars of content, flattened, for `description:` and index lines.
pub(crate) fn okf_description(content: &str) -> String {
    let flat = content
        .replace(['#', '*', '`', '>', '|'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out: String = flat.chars().take(140).collect();
    if flat.chars().count() > 140 {
        out.push('…');
    }
    out
}

fn okf_timestamp(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// Titles go into markdown link text; keep them from breaking the link.
fn link_text(s: &str) -> String {
    s.replace(['[', ']'], " ").trim().to_string()
}

/// Export a notebook as an Open Knowledge Format bundle: a directory of
/// markdown concept files with YAML frontmatter (sources/ and notes/), plus
/// index.md listings and a log.md — per the OKF v0.1 spec.
#[tauri::command]
pub async fn export_notebook_okf(
    state: State<'_, AppState>,
    notebook_id: String,
    dest_dir: String,
) -> Result<String, String> {
    let notebook = e(state.db.list_notebooks().await)?
        .into_iter()
        .find(|n| n.id == notebook_id)
        .ok_or_else(|| "Notebook not found".to_string())?;
    let sources = e(state.db.list_sources(&notebook_id).await)?;
    let notes = e(state.db.list_notes(&notebook_id).await)?;

    // A fresh directory per export — never merge into (or clobber) one the
    // user already has.
    let base = std::path::Path::new(&dest_dir);
    let nb_slug = okf_slug(&notebook.title);
    let mut bundle = base.join(&nb_slug);
    let mut n = 2;
    while bundle.exists() {
        bundle = base.join(format!("{nb_slug}-{n}"));
        n += 1;
    }
    let write = |path: &std::path::Path, text: &str| -> Result<(), String> {
        std::fs::write(path, text).map_err(|err| format!("Failed to write {path:?}: {err}"))
    };

    // Concept files, with per-directory slug dedup.
    let mut used: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut claim = |dir: &str, title: &str| -> String {
        let s = okf_slug(title);
        let key = format!("{dir}/{s}");
        let count = used.entry(key).or_insert(0);
        *count += 1;
        if *count == 1 {
            s
        } else {
            format!("{s}-{count}")
        }
    };

    let mut source_entries = Vec::new(); // (slug, title, description)
    if !sources.is_empty() {
        let dir = bundle.join("sources");
        std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        for s in &sources {
            let content = e(state.db.source_content(&s.id).await)?;
            let slug = claim("sources", &s.title);
            let mut fm = String::from("---\ntype: Source\n");
            fm.push_str(&format!("title: {}\n", yaml_str(&s.title)));
            let desc = okf_description(&content);
            if !desc.is_empty() {
                fm.push_str(&format!("description: {}\n", yaml_str(&desc)));
            }
            if !s.url.is_empty() {
                let resource = if is_web_url(&s.url) {
                    s.url.clone()
                } else {
                    format!("file://{}", s.url)
                };
                fm.push_str(&format!("resource: {}\n", yaml_str(&resource)));
            }
            fm.push_str(&format!("tags: [{}]\n", s.source_type));
            fm.push_str(&format!(
                "timestamp: {}\n---\n\n",
                okf_timestamp(s.created_at)
            ));
            write(&dir.join(format!("{slug}.md")), &format!("{fm}{content}\n"))?;
            source_entries.push((slug, s.title.clone(), desc));
        }
        let listing = source_entries
            .iter()
            .map(|(slug, title, desc)| format!("- [{}]({slug}.md) — {desc}", link_text(title)))
            .collect::<Vec<_>>()
            .join("\n");
        write(&dir.join("index.md"), &format!("# Sources\n\n{listing}\n"))?;
    }

    let mut note_entries = Vec::new();
    if !notes.is_empty() {
        let dir = bundle.join("notes");
        std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        for note in &notes {
            let slug = claim("notes", &note.title);
            let type_label = match note.kind.as_str() {
                "note" => "Note",
                "report" => "Report",
                kind => rag::artifact_spec(kind).map(|(t, _)| t).unwrap_or("Note"),
            };
            let desc = okf_description(&note.content);
            let mut fm = format!("---\ntype: {type_label}\n");
            fm.push_str(&format!("title: {}\n", yaml_str(&note.title)));
            if !desc.is_empty() {
                fm.push_str(&format!("description: {}\n", yaml_str(&desc)));
            }
            fm.push_str(&format!(
                "timestamp: {}\n---\n\n",
                okf_timestamp(note.updated_at)
            ));
            write(
                &dir.join(format!("{slug}.md")),
                &format!("{fm}{}\n", note.content),
            )?;
            note_entries.push((slug, note.title.clone(), desc));
        }
        let listing = note_entries
            .iter()
            .map(|(slug, title, desc)| format!("- [{}]({slug}.md) — {desc}", link_text(title)))
            .collect::<Vec<_>>()
            .join("\n");
        write(&dir.join("index.md"), &format!("# Notes\n\n{listing}\n"))?;
    }

    // Root index.md: progressive-disclosure listing of the whole bundle.
    let mut index = format!("# {}\n\n", notebook.title);
    index.push_str(
        "A research notebook exported from Alchemy as an Open Knowledge Format bundle.\n",
    );
    if !source_entries.is_empty() {
        index.push_str("\n# Sources\n\n");
        for (slug, title, desc) in &source_entries {
            index.push_str(&format!(
                "- [{}](sources/{slug}.md) — {desc}\n",
                link_text(title)
            ));
        }
    }
    if !note_entries.is_empty() {
        index.push_str("\n# Notes\n\n");
        for (slug, title, desc) in &note_entries {
            index.push_str(&format!(
                "- [{}](notes/{slug}.md) — {desc}\n",
                link_text(title)
            ));
        }
    }
    write(&bundle.join("index.md"), &index)?;

    let today = chrono::Utc::now().format("%Y-%m-%d");
    write(
        &bundle.join("log.md"),
        &format!(
            "# {today}\n\nExported from Alchemy: {} sources, {} notes.\n",
            source_entries.len(),
            note_entries.len()
        ),
    )?;

    Ok(bundle.display().to_string())
}

/// Export the bundle and zip it into a single shareable `.okf.zip` file at
/// `dest_path` (the coworker / other-laptop case — one file to send, and
/// import_notebook_okf on the other side recreates the notebook).
#[tauri::command]
pub async fn export_notebook_okf_zip(
    state: State<'_, AppState>,
    notebook_id: String,
    dest_path: String,
) -> Result<String, String> {
    let staging = std::env::temp_dir().join(format!("alchemy-okf-export-{}", new_id()));
    std::fs::create_dir_all(&staging).map_err(|e2| e2.to_string())?;
    let bundle = export_notebook_okf(state, notebook_id, staging.display().to_string()).await?;
    let result = zip_dir(
        std::path::Path::new(&bundle),
        std::path::Path::new(&dest_path),
    );
    let _ = std::fs::remove_dir_all(&staging);
    result?;
    Ok(dest_path)
}

/// Zip a bundle directory (bundle-name-rooted entries, so unzipping yields
/// the folder, matching what the exporter writes on disk).
fn zip_dir(dir: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    use std::io::Write as _;
    let file = std::fs::File::create(dest).map_err(|e| format!("Failed to create zip: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = Default::default();
    let root_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("notebook")
        .to_string();
    fn walk(
        zip: &mut zip::ZipWriter<std::fs::File>,
        opts: zip::write::SimpleFileOptions,
        dir: &std::path::Path,
        prefix: &str,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            let entry_name = format!("{prefix}/{name}");
            if path.is_dir() {
                walk(zip, opts, &path, &entry_name)?;
            } else {
                zip.start_file(&entry_name, opts)
                    .map_err(|e| e.to_string())?;
                let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                zip.write_all(&bytes).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    walk(&mut zip, opts, dir, &root_name)?;
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

// ---- OKF import ------------------------------------------------------------

/// Parse the exporter's frontmatter subset (`key: "quoted"` or bare values).
fn parse_okf_doc(text: &str) -> (std::collections::HashMap<String, String>, String) {
    let mut fm = std::collections::HashMap::new();
    let Some(rest) = text.strip_prefix("---\n") else {
        return (fm, text.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        return (fm, text.to_string());
    };
    let head = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('\n');
    for line in head.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim();
            let v = if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
                v[1..v.len() - 1]
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\")
            } else {
                v.to_string()
            };
            fm.insert(k.trim().to_string(), v);
        }
    }
    (fm, body.to_string())
}

/// Map an exported note's `type:` label back to its kind.
fn note_kind_from_label(label: &str) -> String {
    if label.eq_ignore_ascii_case("report") {
        return "report".into();
    }
    const KINDS: &[&str] = &[
        "summary",
        "faq",
        "study_guide",
        "briefing",
        "timeline",
        "insights",
        "flashcards",
        "quiz",
        "mind_map",
        "data_table",
        "round_table",
        "problems",
        "prd",
        "prfaq",
        "rfc",
        "skill",
    ];
    for k in KINDS {
        if rag::artifact_spec(k).map(|(t, _)| t) == Some(label) {
            return (*k).to_string();
        }
    }
    "note".into()
}

/// Safely extract an .okf.zip into a scratch dir and return it.
fn extract_okf_zip(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Failed to open zip: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Not a readable zip: {e}"))?;
    let dest = std::env::temp_dir().join(format!("alchemy-okf-import-{}", new_id()));
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        // enclosed_name refuses absolute paths and `..` traversal.
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        }
    }
    Ok(dest)
}

/// An OKF bundle root holds index.md (and sources/ / notes/); a zip usually
/// nests it one directory down.
fn find_bundle_root(dir: std::path::PathBuf) -> Result<std::path::PathBuf, String> {
    let looks_like = |p: &std::path::Path| {
        p.join("index.md").exists() || p.join("sources").is_dir() || p.join("notes").is_dir()
    };
    if looks_like(&dir) {
        return Ok(dir);
    }
    let subdirs: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    if let [only] = subdirs.as_slice() {
        if looks_like(only) {
            return Ok(only.clone());
        }
    }
    Err("This isn't an Open Knowledge Format bundle: expected index.md with sources/ and notes/ folders".into())
}

/// Sorted markdown docs in a bundle subdirectory (index.md excluded).
fn okf_docs(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|x| x.to_str()) == Some("md")
                && p.file_name().and_then(|n| n.to_str()) != Some("index.md")
        })
        .collect();
    files.sort();
    files
}

/// Does this dropped path look like an OKF bundle (folder or zip)? Cheap
/// check so drag-and-drop routes bundles to import instead of trying to
/// ingest them as sources.
#[tauri::command]
pub fn probe_okf(path: String) -> bool {
    let p = std::path::Path::new(&path);
    if p.is_dir() {
        return find_bundle_root(p.to_path_buf()).is_ok();
    }
    if p.extension().and_then(|e| e.to_str()) != Some("zip") {
        return false;
    }
    let Ok(file) = std::fs::File::open(p) else {
        return false;
    };
    let Ok(archive) = zip::ZipArchive::new(file) else {
        return false;
    };
    // Bundle zips are name-rooted ("slug/index.md"), but accept flat too.
    // (Bound to a local: the tail expression would otherwise borrow
    // `archive` past its drop point — E0597.)
    let looks_like_bundle = archive.file_names().take(200).any(|name| {
        name == "index.md"
            || name.ends_with("/index.md")
            || name.starts_with("sources/")
            || name.starts_with("notes/")
            || name.contains("/sources/")
            || name.contains("/notes/")
    });
    looks_like_bundle
}

/// Import an OKF bundle (a folder or an .okf.zip) into a new notebook (None)
/// or an existing one. Sources re-chunk and re-embed locally; duplicates are
/// skipped quietly, so merging the same bundle twice is harmless.
#[tauri::command]
pub async fn import_notebook_okf(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    notebook_id: Option<String>,
) -> Result<Notebook, String> {
    let src = std::path::PathBuf::from(&path);
    let (scratch, root) = if src.is_dir() {
        (None, src)
    } else {
        let dest = extract_okf_zip(&src)?;
        (Some(dest.clone()), dest)
    };
    let result = import_bundle(&app, &state, root, notebook_id).await;
    if let Some(dir) = scratch {
        let _ = std::fs::remove_dir_all(dir);
    }
    result
}

async fn import_bundle(
    app: &AppHandle,
    state: &AppState,
    root: std::path::PathBuf,
    notebook_id: Option<String>,
) -> Result<Notebook, String> {
    let root = find_bundle_root(root)?;

    // Bundle title: index.md's H1, else the folder name.
    let title = std::fs::read_to_string(root.join("index.md"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("# "))
                .map(|l| l[2..].trim().to_string())
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            root.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Untitled notebook")
                .to_string()
        });

    // Destination: an existing notebook, or a fresh one named for the bundle.
    let notebook = match &notebook_id {
        Some(id) => e(state.db.list_notebooks().await)?
            .into_iter()
            .find(|n| &n.id == id)
            .ok_or_else(|| "Notebook not found".to_string())?,
        None => {
            let ts = now();
            let count = e(state.db.list_notebooks().await)?;
            let icon = auto_notebook_icon(&title);
            let nb = Notebook {
                id: new_id(),
                title,
                created_at: ts,
                updated_at: ts,
                color: NOTEBOOK_PALETTE[count.len() % NOTEBOOK_PALETTE.len()].to_string(),
                icon,
                status: String::new(),
                source_count: 0,
                note_count: 0,
                report_count: 0,
            };
            e(state.db.create_notebook(&nb).await)?;
            nb
        }
    };

    const SOURCE_TYPES: &[&str] = &["pdf", "text", "markdown", "html", "url", "image", "mac"];
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let source_docs = okf_docs(&root.join("sources"));
    let total = source_docs.len();
    for (i, doc) in source_docs.into_iter().enumerate() {
        let Ok(text) = std::fs::read_to_string(&doc) else {
            skipped += 1;
            continue;
        };
        let (fm, body) = parse_okf_doc(&text);
        // Folder container rows export with empty bodies — their children
        // are full documents of their own. Nothing to embed here.
        if body.trim().is_empty() {
            skipped += 1;
            continue;
        }
        let title = fm
            .get("title")
            .cloned()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| {
                doc.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled source")
                    .to_string()
            });
        let source_type = fm
            .get("tags")
            .map(|t| t.trim_matches(['[', ']']).trim().to_string())
            .filter(|t| SOURCE_TYPES.contains(&t.as_str()))
            .unwrap_or_else(|| "text".to_string());
        // The resource is where the source CAME from — on this machine it's
        // provenance, not a live path, except web URLs which stay refreshable.
        let url = match fm.get("resource") {
            Some(r) if is_web_url(r) => r.clone(),
            Some(r) => r.strip_prefix("file://").unwrap_or(r).to_string(),
            None => String::new(),
        };
        let extracted = ingest::Extracted {
            image_url: String::new(),
            author: String::new(),
            title,
            source_type,
            url,
            text: body,
        };
        let _ = app.emit(
            "import://progress",
            serde_json::json!({ "done": i, "total": total, "title": extracted.title }),
        );
        match store_extracted(state, &notebook.id, extracted).await {
            Ok(_) => imported += 1,
            // Duplicates (merging a bundle twice) are success, not failure.
            Err(_) => skipped += 1,
        }
    }

    // Note dedup mirrors source dedup: re-importing the same bundle must not
    // double every note. Same title + same body = already here.
    let existing_notes: Vec<(String, String)> = e(state.db.list_notes(&notebook.id).await)?
        .into_iter()
        .map(|n| (n.title, n.content))
        .collect();
    for doc in okf_docs(&root.join("notes")) {
        let Ok(text) = std::fs::read_to_string(&doc) else {
            continue;
        };
        let (fm, body) = parse_okf_doc(&text);
        if body.trim().is_empty() {
            continue;
        }
        let title_for_dup = fm.get("title").cloned().unwrap_or_default();
        if existing_notes
            .iter()
            .any(|(t, c)| t == &title_for_dup && c.trim() == body.trim())
        {
            continue;
        }
        let note = Note {
            id: new_id(),
            notebook_id: notebook.id.clone(),
            title: fm.get("title").cloned().unwrap_or_else(|| {
                doc.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled note")
                    .to_string()
            }),
            content: body,
            kind: note_kind_from_label(fm.get("type").map(String::as_str).unwrap_or("Note")),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: now(),
            updated_at: now(),
        };
        e(add_note_indexed(state, &note).await)?;
    }

    e(state.db.touch_notebook(&notebook.id, now()).await)?;
    let _ = app.emit(
        "import://done",
        serde_json::json!({ "imported": imported, "skipped": skipped }),
    );
    Ok(notebook)
}

/// One passage behind a meta-chat answer: what it is and where it lives.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaCitation {
    /// "source" (chunk passage) | "note" | "card" (registry card).
    pub kind: String,
    /// Empty for registry cards — they are corpus-scoped and open on Home.
    pub notebook_id: String,
    pub notebook_title: String,
    /// Source id for source passages; note id for notes; card id for cards.
    pub id: String,
    pub title: String,
    pub snippet: String,
}

/// Does a registry card answer this query? Two shapes share one matcher:
/// palette typing (the whole query is a fragment of the name/identifiers)
/// and ask-mode questions ("what's my policy number for the Bayside boat"),
/// where a card-name word, an identifier token, or a fact label/value
/// appearing IN the question is the signal. Word-level checks require some
/// length so "the"/"a" never match a card into every answer.
pub(crate) fn card_matches(card: &crate::models::RegistryCard, q_lower: &str) -> bool {
    let name = card.name.to_lowercase();
    if name.contains(q_lower) || card.identifiers.contains(q_lower) {
        return true;
    }
    let name_hit = name
        .split_whitespace()
        .any(|w| w.chars().count() >= 4 && q_lower.contains(w));
    // Identifiers are already normalized lowercase tokens (the auto-attach
    // key), so containment in the question is exact-token evidence.
    let id_hit = card
        .identifiers
        .split_whitespace()
        .any(|t| t.chars().count() >= 3 && q_lower.contains(t));
    let fact_hit = card.facts.iter().any(|f| {
        let label = f.label.to_lowercase();
        let value = f.value.to_lowercase();
        (label.chars().count() >= 4 && q_lower.contains(&label))
            || (value.chars().count() >= 3 && q_lower.contains(&value))
    });
    name_hit || id_hit || fact_hit
}

/// A registry card rendered as answer context: kind, identifiers, facts,
/// and the user's note — the whole point is that "what's my policy number"
/// can answer from the card's facts.
pub(crate) fn card_passage_text(card: &crate::models::RegistryCard) -> String {
    let mut out = format!("Registry card ({}).", card.kind);
    if !card.identifiers.trim().is_empty() {
        out.push_str(&format!(" Identifiers: {}.", card.identifiers.trim()));
    }
    if !card.facts.is_empty() {
        let facts = card
            .facts
            .iter()
            .map(|f| {
                if f.value.trim().is_empty() {
                    f.label.trim().to_string()
                } else {
                    format!("{}: {}", f.label.trim(), f.value.trim())
                }
            })
            .collect::<Vec<_>>()
            .join("; ");
        out.push_str(&format!(" Facts: {facts}."));
    }
    if !card.note.trim().is_empty() {
        out.push_str(&format!(" Note: {}.", card.note.trim()));
    }
    out
}

/// Registry cards matching a question, as meta citations (RFC-registry ×
/// meta-chat): matched cards ride into the ask-everything context so the
/// answer can come straight off a card's facts. Shared by the command and
/// the MCP ask_everything tool.
pub(crate) async fn registry_card_citations(
    state: &AppState,
    question: &str,
    cap: usize,
) -> Vec<MetaCitation> {
    let q = question.trim().to_lowercase();
    if q.len() < 2 {
        return Vec::new();
    }
    let cards = state.db.list_registry().await.unwrap_or_default();
    cards
        .iter()
        .filter(|c| c.origin != "dismissed" && card_matches(c, &q))
        .take(cap)
        .map(|c| MetaCitation {
            kind: "card".into(),
            notebook_id: String::new(),
            notebook_title: "Registry".into(),
            id: c.id.clone(),
            title: c.name.clone(),
            snippet: card_passage_text(c),
        })
        .collect()
}

/// Progress line for the palette's ask flow — the same StepEvent grammar the
/// chat step trail uses, on its own channel so a palette ask never writes
/// into a notebook's step trail (or vice versa). `None` (the MCP path) emits
/// nothing.
fn meta_step(app: Option<&AppHandle>, label: impl Into<String>, transient: bool) {
    if let Some(app) = app {
        let _ = app.emit(
            "meta://step",
            StepEvent {
                label: label.into(),
                transient,
            },
        );
    }
}

/// A corpus-wide answer (docs/RFC-meta-chat.md).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaAnswer {
    pub answer: String,
    pub citations: Vec<MetaCitation>,
}

/// Retrieve corpus-wide passages for a question: hybrid chunk search across
/// every notebook (capped, gateway and local alike) merged with note hits —
/// notes are often the answer (reports, briefs). Shared by the ask_everything
/// command and the MCP tool.
///
/// `deep` is the deep-search profile: retrieve a 3x candidate pool and let
/// the chat model rerank it down to k (recall from hybrid retrieval,
/// precision from the rerank). Any rerank failure falls back to fusion
/// order, so deep can only reorder-or-equal, never lose the flat result.
pub(crate) async fn retrieve_everything(
    state: &AppState,
    app: Option<&AppHandle>,
    question: &str,
    k: usize,
    deep: bool,
) -> Result<Vec<MetaCitation>, String> {
    let nb_titles: std::collections::HashMap<String, String> = e(state.db.list_notebooks().await)?
        .into_iter()
        .map(|n| (n.id, n.title))
        .collect();

    let (query_vec, profile) = {
        let ai = state.ai.read().await.clone();
        (
            e(ai.embed_one(question).await)?,
            ai.profile(crate::inference::Role::Chat),
        )
    };
    // Semantic routing: with enough notebooks, search the most likely ones
    // instead of the whole corpus. The index is self-healing and any
    // failure (or a small corpus) falls back to the flat search; routing
    // keeps ROUTE_TOP_K notebooks, so corpora at or below that size are
    // searched in full either way.
    let routed: Option<Vec<String>> = if nb_titles.len() > crate::router::MIN_NOTEBOOKS_TO_ROUTE {
        meta_step(
            app,
            format!("Searching {} notebooks", nb_titles.len()),
            false,
        );
        let ai = state.ai.read().await.clone();
        // Piggyback the gist sweep on the same self-heal moment the router
        // uses — catches sources imported before gisting existed (or while
        // the app was quitting mid-backfill).
        crate::gist::spawn_sweep(state.db.clone(), ai.clone());
        if let Err(err) = crate::router::ensure_router(&state.db, &ai).await {
            crate::note!("router refresh failed (falling back to flat): {err:#}");
        }
        match crate::router::route_notebooks(
            &state.db,
            query_vec.clone(),
            crate::router::ROUTE_TOP_K,
        )
        .await
        {
            Ok(ids) if !ids.is_empty() => Some(ids),
            Ok(_) => None,
            Err(err) => {
                crate::note!("notebook routing failed (falling back to flat): {err:#}");
                None
            }
        }
    } else {
        None
    };

    // Diversity caps keep one chatty notebook or source from filling the
    // whole answer with near-duplicates; skipped candidates backfill, so a
    // single-notebook corpus behaves exactly like the flat search.
    let opts = crate::db::SearchOptions {
        pool_multiplier: 4,
        max_per_source: 2,
        max_per_notebook: 3,
        max_notes: 4,
        // Gists are overview evidence: useful on synthesis questions, but a
        // small budget is plenty — verbatim passages carry the specifics. The
        // budget is model-tiered (RFC-infinite-context §1, §5): two by
        // default, one on the tight on-device window.
        max_gists: profile.max_gists,
    };
    meta_step(
        app,
        match &routed {
            Some(ids) => format!("Searching the {} most likely notebooks", ids.len()),
            None => match nb_titles.len() {
                1 => "Searching your notebook".to_string(),
                n => format!("Searching all {n} notebooks"),
            },
        },
        false,
    );
    // Deep search retrieves a wider pool for the reranker to pick from.
    let fetch_k = if deep { k * 3 } else { k };
    // The title-fallback passes below need the corpus source list and the
    // notes regardless of what the search returns — neither depends on it,
    // so all three queries run concurrently instead of in file order. Only
    // the per-hit source_content reads stay sequential (they depend on
    // which titles match).
    let retrieval_t = std::time::Instant::now();
    let (searched, source_meta, all_notes) = tokio::join!(
        state
            .db
            .search_chunks_all_opts(query_vec, question, fetch_k, routed.as_deref(), opts),
        state.db.all_source_meta(),
        state.db.recent_notes(usize::MAX),
    );
    // Real-corpus phase timing, opt-in: `ALCHEMY_TIMING=1 pnpm tauri dev`.
    // The synthetic-corpus percentiles live in eval_retrieval_latency; this
    // is how those numbers get checked against an actual library.
    if std::env::var_os("ALCHEMY_TIMING").is_some() {
        crate::note!(
            "timing meta-retrieval: {:.1}ms (deep={deep})",
            retrieval_t.elapsed().as_secs_f64() * 1000.0
        );
    }
    let source_meta = e(source_meta)?;
    let all_notes = e(all_notes)?;
    let mut out: Vec<MetaCitation> = e(searched)?
        .into_iter()
        .map(|(nb, c)| {
            // Note chunks come back with note_id set (they share the chunk
            // table); surface them as first-class note citations.
            let is_note = !c.note_id.is_empty();
            MetaCitation {
                kind: if is_note { "note" } else { "source" }.into(),
                notebook_title: nb_titles.get(&nb).cloned().unwrap_or_default(),
                notebook_id: nb,
                id: if is_note { c.note_id } else { c.source_id },
                title: c.source_title,
                snippet: c.snippet,
            }
        })
        .collect();

    // Deep search: one model call picks the k passages that actually answer
    // from the wide pool. Failure (model down, unparseable output) degrades
    // to the fusion-ordered top k — exactly the non-deep result.
    if deep && out.len() > k {
        meta_step(
            app,
            format!(
                "Picking the {k} passages that answer best (of {})",
                out.len()
            ),
            false,
        );
        let snippets: Vec<(String, String)> = out
            .iter()
            .map(|c| (c.title.clone(), c.snippet.chars().take(300).collect()))
            .collect();
        let ai = state.ai.read().await.clone();
        match crate::agent::rerank_indices(&ai, question, &snippets, k).await {
            Some(picked) => out = picked.into_iter().map(|i| out[i].clone()).collect(),
            None => out.truncate(k),
        }
    }

    // Title-match fallback passes: hybrid search covers bodies, but an
    // exact-title lookup ("the contractor agreement", "the Q3 report note")
    // can still miss the top k — substring over titles backstops it.
    let q = question.trim().to_lowercase();
    let already: std::collections::HashSet<String> = out.iter().map(|c| c.id.clone()).collect();

    // Sources: match when the question names the title (guarded against
    // tiny titles matching everything) or a short palette-style query is
    // contained in the title.
    let mut source_hits = 0;
    for (id, nb, title, _) in source_meta {
        if source_hits >= 3 {
            break;
        }
        if already.contains(&id) {
            continue;
        }
        let t = title.to_lowercase();
        if (t.chars().count() >= 8 && q.contains(&t)) || t.contains(&q) {
            let snippet: String = e(state.db.source_content(&id).await)?
                .chars()
                .take(400)
                .collect();
            source_hits += 1;
            out.push(MetaCitation {
                kind: "source".into(),
                notebook_title: nb_titles.get(&nb).cloned().unwrap_or_default(),
                notebook_id: nb,
                id,
                title,
                snippet,
            });
        }
    }

    let mut note_hits = 0;
    for n in all_notes {
        if note_hits >= 4 {
            break;
        }
        if already.contains(&n.id) {
            continue;
        }
        if n.title.to_lowercase().contains(&q) || n.content.to_lowercase().contains(&q) {
            note_hits += 1;
            out.push(MetaCitation {
                kind: "note".into(),
                notebook_title: nb_titles.get(&n.notebook_id).cloned().unwrap_or_default(),
                notebook_id: n.notebook_id,
                id: n.id,
                title: n.title,
                snippet: n.content.chars().take(400).collect(),
            });
        }
    }

    {
        let nb_count = out
            .iter()
            .map(|c| c.notebook_id.as_str())
            .collect::<HashSet<_>>()
            .len();
        meta_step(
            app,
            format!(
                "Found {} passage{} across {} notebook{}",
                out.len(),
                if out.len() == 1 { "" } else { "s" },
                nb_count,
                if nb_count == 1 { "" } else { "s" }
            ),
            false,
        );
    }

    let note_ids: Vec<String> = out
        .iter()
        .filter(|c| c.kind == "note")
        .map(|c| c.id.clone())
        .collect();
    if !note_ids.is_empty() {
        if let Err(err) = state
            .db
            .bump_note_usage(&note_ids, "retrieval_hits", now())
            .await
        {
            crate::note!("note usage bump (retrieval_hits) failed: {err:#}");
        }
    }

    crate::trace::log(
        &state.trace_dir,
        serde_json::json!({
            "ts": now(),
            "surface": "meta",
            "query": question,
            "deep": deep,
            "routedNotebooks": routed,
            "citations": out.iter().enumerate().map(|(rank, c)| serde_json::json!({
                "rank": rank + 1,
                "kind": c.kind,
                "id": c.id,
                "notebookId": c.notebook_id,
                "title": c.title,
            })).collect::<Vec<_>>(),
        }),
    );
    Ok(out)
}

/// One Small-role extract for the global route: pull only what answers the
/// question out of one source's content. Returns None on any failure, an
/// explicit SKIP, empty output, or output past the length bound — the caller
/// then falls back to that source's gist text, never dropping the source.
async fn global_extract(ai: &Ai, question: &str, content: &str) -> Option<String> {
    // Same head-cap convention as the gist prompt (gist.rs PROMPT_HEAD_CHARS).
    const HEAD_CHARS: usize = 10_000;
    const EXTRACT_MAX_CHARS: usize = 2_000;
    let head: String = content.chars().take(HEAD_CHARS).collect();
    let messages = [
        crate::ai::ChatTurn::system(
            "You extract only what is relevant. Reply with 2-5 tight bullet points, \
             or exactly SKIP if nothing applies.",
        ),
        crate::ai::ChatTurn::user(format!("Question: {question}\n\nSource:\n---\n{head}")),
    ];
    let text = ai
        .chat_role(crate::ai::Role::Small, &messages)
        .await
        .ok()?
        .text;
    let text = text.trim();
    let skipped = text
        .lines()
        .next()
        .is_none_or(|l| l.trim().eq_ignore_ascii_case("SKIP"));
    if skipped || text.chars().count() > EXTRACT_MAX_CHARS {
        return None;
    }
    Some(text.to_string())
}

/// The global answer route (docs/RFC-infinite-context.md Phase 4): a lazy
/// map-reduce over the standing gist layer. Retrieve the gist rows the
/// question touches, extract per source on the Small role (falling back to the
/// gist text on any per-source failure), and hand source-granular passages +
/// citations to the shared meta synthesis path. Returns None when the route
/// does not apply (no gists, nothing retrieved) or ANY step failed — the
/// caller then takes the pointed path unchanged.
async fn global_meta_route(
    state: &AppState,
    app: Option<&AppHandle>,
    question: &str,
) -> anyhow::Result<Option<(Vec<MetaCitation>, Vec<rag::MetaPassage>)>> {
    if state.db.list_gists().await?.is_empty() {
        return Ok(None);
    }
    let (query_vec, profile) = {
        let ai = state.ai.read().await.clone();
        (
            ai.embed_one(question).await?,
            ai.profile(crate::inference::Role::Chat),
        )
    };
    // Fan-out is model-tiered (RFC-infinite-context §4, §5): six Small-role
    // extracts by default, three on the on-device tier whose single-tenant
    // engine also runs the synthesis these extracts feed.
    let selected: Vec<(String, Citation)> = state
        .db
        .search_gists(query_vec, 12)
        .await?
        .into_iter()
        .take(profile.global_fan_out)
        .collect();
    if selected.is_empty() {
        return Ok(None);
    }

    let nb_titles: std::collections::HashMap<String, String> = state
        .db
        .list_notebooks()
        .await?
        .into_iter()
        .map(|n| (n.id, n.title))
        .collect();

    // One source → one passage → one citation, so numbers line up 1:1 (the
    // pointed path dedupes several chunks per source; here each source is
    // distinct already). Small-role calls run sequentially: local engines are
    // single-tenant.
    let ai = state.ai.read().await.clone();
    let mut citations: Vec<MetaCitation> = Vec::with_capacity(selected.len());
    let mut passages: Vec<rag::MetaPassage> = Vec::with_capacity(selected.len());
    let mut fallbacks: Vec<bool> = Vec::with_capacity(selected.len());
    meta_step(
        app,
        format!(
            "Reading {} source{} in depth",
            selected.len(),
            if selected.len() == 1 { "" } else { "s" }
        ),
        false,
    );
    for (i, (nb_id, gist)) in selected.iter().enumerate() {
        // Live per-source status, transient: each read replaces the last in
        // the trail — a fan-out of six must not become six log lines.
        meta_step(
            app,
            format!(
                "Reading {} ({} of {})",
                gist.source_title,
                i + 1,
                selected.len()
            ),
            true,
        );
        let notebook_title = nb_titles.get(nb_id).cloned().unwrap_or_default();
        // The gist row's snippet IS the distilled overview — the guaranteed
        // fallback for this source when the extract fails or SKIPs.
        let content = state.db.source_content(&gist.source_id).await?;
        let (snippet, fell_back) = match global_extract(&ai, question, &content).await {
            Some(extract) => (extract, false),
            None => (gist.snippet.clone(), true),
        };
        fallbacks.push(fell_back);
        passages.push(rag::MetaPassage {
            number: i + 1,
            kind: "source".into(),
            notebook_title: notebook_title.clone(),
            title: gist.source_title.clone(),
            snippet,
        });
        citations.push(MetaCitation {
            kind: "source".into(),
            notebook_title,
            notebook_id: nb_id.clone(),
            id: gist.source_id.clone(),
            title: gist.source_title.clone(),
            snippet: gist.snippet.clone(),
        });
    }

    crate::trace::log(
        &state.trace_dir,
        serde_json::json!({
            "ts": now(),
            "surface": "meta-global",
            "query": question,
            "fanOut": selected.len(),
            "fallbacks": fallbacks,
        }),
    );
    Ok(Some((citations, passages)))
}

/// Answer a question across the ENTIRE corpus, streaming tokens as
/// meta://token events. See docs/RFC-meta-chat.md.
#[tauri::command]
pub async fn ask_everything(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    question: String,
    history: Option<Vec<crate::ai::ChatTurn>>,
    deep: Option<bool>,
) -> Result<MetaAnswer, String> {
    touch_activity();
    let question = question.trim().to_string();
    if question.is_empty() {
        return Err("Question is empty".into());
    }

    // Deep search (wide pool + model rerank) defaults on for gateway models,
    // where the extra rerank call is fast and cheap; local models keep the
    // low-latency single-pass path unless the caller asks for deep.
    let deep = match deep {
        Some(d) => d,
        None => state.ai.read().await.config().is_gateway(),
    };
    // Global route (RFC-infinite-context §4): enumerative/comparative
    // questions want coverage of the gist layer, not a top-k of chunks. The
    // classifier is pure; ANY failure inside the route degrades to None, so
    // the pointed path below runs unchanged whenever the route doesn't fire.
    let global = if rag::is_global_query(&question) {
        match global_meta_route(&state, Some(&app), &question).await {
            Ok(g) => g,
            Err(err) => {
                crate::note!("meta-global route failed, falling back to pointed: {err:#}");
                None
            }
        }
    } else {
        None
    };

    // References are per SOURCE, not per chunk: several excerpts from one
    // source share a number, and the citation list the UI shows is deduped —
    // otherwise a source that contributed five chunks shows up five times.
    let (mut citations, mut passages) = if let Some(g) = global {
        g
    } else {
        let passages_raw = retrieve_everything(&state, Some(&app), &question, 16, deep).await?;
        let mut citations: Vec<MetaCitation> = Vec::new();
        let mut passages: Vec<rag::MetaPassage> = Vec::new();
        for c in &passages_raw {
            let number = match citations
                .iter()
                .position(|u| u.kind == c.kind && u.id == c.id)
            {
                Some(i) => i + 1,
                None => {
                    citations.push(c.clone());
                    citations.len()
                }
            };
            passages.push(rag::MetaPassage {
                number,
                kind: c.kind.clone(),
                notebook_title: c.notebook_title.clone(),
                title: c.title.clone(),
                snippet: c.snippet.clone(),
            });
        }
        (citations, passages)
    };

    // Registry cards join the context (RFC-registry × meta-chat): a question
    // that names a card, an identifier, or a fact answers straight off the
    // card — "what's my policy number" from the Bayside card's facts. Cards
    // append after the retrieved passages so citation numbers stay stable.
    let card_citations = registry_card_citations(&state, &question, 3).await;
    if !card_citations.is_empty() {
        meta_step(
            Some(&app),
            format!(
                "Reading {} registry card{}",
                card_citations.len(),
                if card_citations.len() == 1 { "" } else { "s" }
            ),
            false,
        );
        for c in card_citations {
            passages.push(rag::MetaPassage {
                number: citations.len() + 1,
                kind: c.kind.clone(),
                notebook_title: c.notebook_title.clone(),
                title: c.title.clone(),
                snippet: c.snippet.clone(),
            });
            citations.push(c);
        }
    }

    let (persona, ctx_profile) = {
        let ai = state.ai.read().await.clone();
        (
            rag::persona_block(&ai.config().profile),
            ai.profile(crate::inference::Role::Chat),
        )
    };
    let messages = rag::build_meta_messages(
        history.as_deref().unwrap_or(&[]),
        &question,
        &passages,
        &persona,
        ctx_profile.compact_excerpts,
    );
    // On-device model only: fit the prompt to its 8192-token window before
    // streaming; a no-op for larger-window engines (see notebook chat above).
    let messages = {
        let ai = state.ai.read().await.clone();
        match ai.fm_input_budget(crate::inference::Role::Chat) {
            Some(budget) => crate::inference::budget::fit_messages(&messages, budget).into_owned(),
            None => messages,
        }
    };

    // Same stream/cancel dance as notebook chat, under its own scope so a
    // palette Esc never kills a notebook stream (or vice versa).
    meta_step(
        Some(&app),
        format!(
            "Synthesizing from {} excerpt{}",
            passages.len(),
            if passages.len() == 1 { "" } else { "s" }
        ),
        false,
    );
    let app_for_cb = app.clone();
    let cancel = state.begin_generation(&format!("meta:{}", window.label()));
    let partial = Arc::new(Mutex::new(String::new()));
    let partial_cb = partial.clone();
    let ttft = TtftClock::start();
    let ttft_cb = ttft.clone();
    let (answer, stats, model) = {
        let ai = state.ai.read().await.clone();
        let model = ai.chat_metrics_key(None);
        let streamed = tokio::select! {
            out = ai.chat_stream(&messages, |tok| {
                ttft_cb.mark();
                partial_cb.lock().unwrap().push_str(tok);
                let _ = app_for_cb.emit(
                    "meta://token",
                    TokenEvent { content: tok.to_string() },
                );
            }) => Some(e(out)?),
            _ = cancel.cancelled() => None,
        };
        match streamed {
            Some(out) => (out.text, out.stats, model),
            None => (partial.lock().unwrap().clone(), None, model),
        }
    };
    state.record_chat_stats(&model, stats);
    state.record_ttft(&model, "ask-everything", "", &ttft, None);

    Ok(MetaAnswer { answer, citations })
}

/// One global-search result for the command menu.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// "source" (title match) | "note" (title/content match) | "content" (BM25 chunk hit)
    pub kind: String,
    pub notebook_id: String,
    /// Source id for source/content hits; note id for note hits.
    pub id: String,
    pub title: String,
    pub snippet: String,
}

/// Search source titles, note titles/content, and chunk text (BM25) across
/// every notebook. No embedding round-trip, so it's cheap enough to run
/// as-you-type from the command menu.
#[tauri::command]
pub async fn search_everything(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<SearchHit>, String> {
    let q = query.trim().to_lowercase();
    if q.len() < 2 {
        return Ok(vec![]);
    }
    let meta = e(state.db.all_source_meta().await)?;
    let title_of: std::collections::HashMap<&str, (&str, &str)> = meta
        .iter()
        .map(|(id, nb, title, _)| (id.as_str(), (nb.as_str(), title.as_str())))
        .collect();

    let mut hits = Vec::new();
    for (id, nb, title, _) in &meta {
        if title.to_lowercase().contains(&q) {
            hits.push(SearchHit {
                kind: "source".into(),
                notebook_id: nb.clone(),
                id: id.clone(),
                title: title.clone(),
                snippet: String::new(),
            });
        }
        if hits.len() >= 4 {
            break;
        }
    }

    // Registry cards: corpus-scoped, so they carry no notebook — the palette
    // opens them on Home rather than switching notebooks. Dismissed
    // suggestions are refusal memory, not results. The shared matcher also
    // covers fact labels/values, so "policy number" finds the Bayside card.
    for c in e(state.db.list_registry().await)?
        .iter()
        .filter(|c| c.origin != "dismissed" && card_matches(c, &q))
    {
        if hits.iter().filter(|h| h.kind == "card").count() >= 4 {
            break;
        }
        hits.push(SearchHit {
            kind: "card".into(),
            notebook_id: String::new(),
            id: c.id.clone(),
            title: c.name.clone(),
            snippet: format!(
                "{} \u{00b7} {} document{}",
                c.kind,
                c.attachments
                    .iter()
                    .filter(|a| a.status == "confirmed")
                    .count(),
                if c.attachments
                    .iter()
                    .filter(|a| a.status == "confirmed")
                    .count()
                    == 1
                {
                    ""
                } else {
                    "s"
                }
            ),
        });
    }

    // Ledger rows, across every notebook. The palette opens the notebook's
    // Ledger tab, which is where a row can actually be acted on.
    let mut ledger_hits = 0;
    for nb in e(state.db.list_notebooks().await)? {
        if ledger_hits >= 4 {
            break;
        }
        for entry in e(state.db.list_ledger(&nb.id).await)? {
            if ledger_hits >= 4 {
                break;
            }
            if entry.text.to_lowercase().contains(&q) || entry.why.to_lowercase().contains(&q) {
                ledger_hits += 1;
                hits.push(SearchHit {
                    kind: "ledger".into(),
                    notebook_id: nb.id.clone(),
                    id: entry.id.clone(),
                    title: entry.text.clone(),
                    snippet: format!("{} \u{00b7} {}", entry.kind, entry.status),
                });
            }
        }
    }

    let notes = e(state.db.recent_notes(usize::MAX).await)?;
    let mut note_hits = 0;
    for n in &notes {
        if note_hits >= 4 {
            break;
        }
        if n.title.to_lowercase().contains(&q) || n.content.to_lowercase().contains(&q) {
            note_hits += 1;
            hits.push(SearchHit {
                kind: "note".into(),
                notebook_id: n.notebook_id.clone(),
                id: n.id.clone(),
                title: n.title.clone(),
                snippet: n.content.chars().take(120).collect(),
            });
        }
    }

    let note_title_of: std::collections::HashMap<&str, &str> = notes
        .iter()
        .map(|n| (n.id.as_str(), n.title.as_str()))
        .collect();
    let listed: std::collections::HashSet<String> = hits.iter().map(|h| h.id.clone()).collect();
    for (nb, c) in e(state.db.search_chunks_fts_all(query.trim(), 6).await)? {
        // Note chunks surface as note hits (the palette opens notes by id);
        // skip ones the substring pass above already listed.
        if !c.note_id.is_empty() {
            if !listed.contains(&c.note_id) {
                hits.push(SearchHit {
                    kind: "note".into(),
                    notebook_id: nb,
                    title: note_title_of
                        .get(c.note_id.as_str())
                        .unwrap_or(&"")
                        .to_string(),
                    id: c.note_id,
                    snippet: c.snippet.chars().take(140).collect(),
                });
            }
            continue;
        }
        let title = title_of
            .get(c.source_id.as_str())
            .map(|(_, t)| t.to_string())
            .unwrap_or_default();
        hits.push(SearchHit {
            kind: "content".into(),
            notebook_id: nb,
            id: c.source_id,
            title,
            snippet: c.snippet.chars().take(140).collect(),
        });
    }
    hits.truncate(12);
    Ok(hits)
}

// ---- Settings / health ---------------------------------------------------

/// Verify the configured chat + embedding models are installed and (for embed)
/// actually responding. Used to surface a clear status instead of a hang.
/// List models from an OpenAI-compatible gateway using draft credentials
/// (before they're saved), so Settings can offer model chips.
#[tauri::command]
pub async fn list_gateway_models(base_url: String, api_key: String) -> Result<Vec<String>, String> {
    let client = crate::ai::OpenAiClient::new(&base_url, &api_key, "");
    e(client.list_models().await)
}

/// One provider's model choices for the composer's picker.
///
/// `supportsDefault` is the honest part: for a vendor CLI, leaving the model
/// blank means "whatever the CLI itself is set to", which is a real and usually
/// correct choice. A gateway has no such fallback — it needs a name — so its
/// picker offers no Default entry. `models` may be empty (no catalogue, or a
/// listing that failed); the picker still offers Default and a free-text entry,
/// so an empty list is a thinner menu, never a dead end.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModels {
    pub models: Vec<String>,
    pub supports_default: bool,
    /// Reasoning-effort levels this provider accepts, cheapest first. Empty
    /// means it has no such control — the composer hides the Effort pill
    /// rather than offering a setting that goes nowhere.
    pub efforts: Vec<String>,
    /// What "Default" actually resolves to, when that is knowable. Ollama's
    /// blank falls through to the app's main Ollama model, which is a real
    /// name the user deserves to see next to the word Default. A vendor CLI's
    /// default lives inside the CLI and it won't tell us, so it stays `None`
    /// rather than being guessed at.
    pub default_model: Option<String>,
}

#[tauri::command]
pub async fn provider_models(
    provider_id: String,
    state: State<'_, AppState>,
) -> Result<ProviderModels, String> {
    let ai = state.ai.read().await.clone();
    let cfg = ai.config().clone();
    let entry = cfg
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| format!("no provider {provider_id}"))?
        .clone();

    let ladder = |levels: &[&str]| levels.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    // Failures degrade to an empty list rather than an error: a menu that
    // cannot reach a gateway should still open, with Default and Custom…
    Ok(match entry.kind.as_str() {
        // The on-device model is the one model. Nothing to choose.
        "fm" => ProviderModels {
            models: Vec::new(),
            supports_default: false,
            efforts: Vec::new(),
            default_model: None,
        },
        "gateway" => ProviderModels {
            models: crate::ai::OpenAiClient::new(&entry.base_url, &entry.api_key, "")
                .list_models()
                .await
                .unwrap_or_default(),
            supports_default: false,
            efforts: ladder(crate::inference::BODY_PARAM_EFFORTS),
            default_model: None,
        },
        kind => match crate::inference::AgentKind::from_id(kind) {
            Some(agent) => ProviderModels {
                models: crate::inference::list_agent_models(agent).await,
                supports_default: true,
                efforts: ladder(agent.efforts()),
                default_model: crate::inference::agent_default_model(agent),
            },
            // Ollama (the catch-all): blank falls back to the app's main
            // Ollama model, so Default means something here too.
            None => {
                let mut oc = crate::ai::ollama_config(&cfg);
                if !entry.base_url.trim().is_empty() {
                    oc.base_url = entry.base_url.clone();
                }
                ProviderModels {
                    models: crate::inference::Ollama::new(oc)
                        .list_models()
                        .await
                        .unwrap_or_default(),
                    supports_default: true,
                    efforts: ladder(crate::inference::BODY_PARAM_EFFORTS),
                    // Blank here means "the app's main Ollama model" — a real
                    // name, so show it rather than leaving Default opaque.
                    default_model: Some(cfg.chat_model.clone()).filter(|m| !m.trim().is_empty()),
                }
            }
        },
    })
}

#[tauri::command]
pub async fn check_models(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ModelHealth, String> {
    let ai = state.ai.read().await.clone();
    let cfg = ai.config().clone();
    let norm = |m: &str| m.trim_end_matches(":latest").to_string();

    // Chat status comes from the ACTIVE chat provider. Only an Ollama-kind
    // provider is Ollama's problem — Apple FM, gateways, and agent CLIs
    // probe on their own, so a sleeping Ollama no longer flags a working
    // chat setup ("Chat model: Ollama not reachable" over an FM answer).
    let provider_chat = match cfg.provider_by_id(&cfg.chat_provider) {
        Some(entry) if entry.kind != "ollama" => {
            let (ready, detail) = readiness_for_entry(&app, entry, &cfg).await?;
            Some(ModelStatus {
                name: if entry.chat_model.trim().is_empty() {
                    entry.label.clone()
                } else {
                    entry.chat_model.clone()
                },
                installed: ready,
                working: ready,
                detail: format!("{} · {detail}", entry.label),
            })
        }
        _ => None,
    };
    // Legacy flat-config path: no provider entry resolved, but the flat
    // provider says gateway.
    let gateway_chat = if provider_chat.is_some() {
        provider_chat
    } else if cfg.provider == "openai" {
        let name = cfg.openai_chat_model.clone();
        Some(if name.trim().is_empty() {
            ModelStatus {
                name,
                installed: false,
                working: false,
                detail: "No gateway model set — enter one in Settings".into(),
            }
        } else {
            match ai.list_gateway_models().await {
                Ok(list) if list.is_empty() || list.iter().any(|m| m == &name) => ModelStatus {
                    name,
                    installed: true,
                    working: true,
                    detail: "Gateway connected".into(),
                },
                Ok(_) => ModelStatus {
                    name: name.clone(),
                    installed: false,
                    working: false,
                    detail: format!("`{name}` isn't in the gateway's model list"),
                },
                Err(e) => ModelStatus {
                    name,
                    installed: false,
                    working: false,
                    detail: format!("Gateway: {e:#}"),
                },
            }
        })
    } else {
        None
    };

    // Built-in embedder works with no Ollama at all — probe it directly.
    let builtin_embed = if cfg.embedder == "builtin" {
        Some(match ai.test_embed().await {
            Ok(dim) => ModelStatus {
                name: "potion-base-8M".into(),
                installed: true,
                working: true,
                detail: format!("Built-in · {dim}-dim · runs on CPU"),
            },
            Err(e) => ModelStatus {
                name: "potion-base-8M".into(),
                installed: false,
                working: false,
                detail: format!("Built-in embedder: {e:#}"),
            },
        })
    } else {
        None
    };

    let installed = match ai.list_models().await {
        Ok(list) => list,
        Err(_) => {
            // Ollama unreachable — report Ollama-backed rows as unknown.
            let unknown = |name: String, detail: &str| ModelStatus {
                name,
                installed: false,
                working: false,
                detail: detail.into(),
            };
            let chat = gateway_chat
                .unwrap_or_else(|| unknown(cfg.chat_model.clone(), "Ollama not reachable"));
            let embed = builtin_embed.unwrap_or_else(|| {
                unknown(
                    cfg.embed_model.clone(),
                    "Ollama not reachable (required for the Ollama embedder)",
                )
            });
            return Ok(ModelHealth {
                reachable: false,
                chat,
                embed,
                vision: unknown(cfg.vision_model.clone(), "Ollama not reachable"),
            });
        }
    };
    let has = |m: &str| installed.iter().any(|x| norm(x) == norm(m));

    let chat = gateway_chat.unwrap_or_else(|| {
        let chat_installed = has(&cfg.chat_model);
        ModelStatus {
            name: cfg.chat_model.clone(),
            installed: chat_installed,
            working: chat_installed,
            detail: if chat_installed {
                "Installed".into()
            } else {
                format!("Not installed — run `ollama pull {}`", cfg.chat_model)
            },
        }
    });

    let embed = match builtin_embed {
        Some(b) => b,
        None => {
            let embed_installed = has(&cfg.embed_model);
            // Embeddings are cheap, so actually probe them.
            let (embed_working, embed_detail) = if !embed_installed {
                (
                    false,
                    format!("Not installed — run `ollama pull {}`", cfg.embed_model),
                )
            } else {
                match ai.test_embed().await {
                    Ok(dim) => (true, format!("Working ({dim}-dim)")),
                    Err(e) => (false, format!("Not responding: {e}")),
                }
            };
            ModelStatus {
                name: cfg.embed_model.clone(),
                installed: embed_installed,
                working: embed_working,
                detail: embed_detail,
            }
        }
    };

    let vision = if cfg.provider == "openai" {
        let name = cfg.openai_vision_model.trim().to_string();
        if name.is_empty() {
            ModelStatus {
                name,
                installed: false,
                working: false,
                detail: "Not configured (optional — enables image & scanned-PDF OCR)".into(),
            }
        } else {
            ModelStatus {
                name: name.clone(),
                installed: true,
                working: true,
                detail: format!("Via gateway ({name})"),
            }
        }
    } else if cfg.vision_model.trim().is_empty() {
        ModelStatus {
            name: String::new(),
            installed: false,
            working: false,
            detail: "Not configured (optional — enables image & scanned-PDF OCR)".into(),
        }
    } else {
        let vision_installed = has(&cfg.vision_model);
        ModelStatus {
            name: cfg.vision_model.clone(),
            installed: vision_installed,
            working: vision_installed,
            detail: if vision_installed {
                "Installed".into()
            } else {
                format!("Not installed — run `ollama pull {}`", cfg.vision_model)
            },
        }
    };

    Ok(ModelHealth {
        reachable: true,
        chat,
        embed,
        vision,
    })
}

/// Desktop notification with the standard gates applied backend-side
/// ("Show notifications" plus the quiet-while-focused rule). The frontend's
/// notify() routes here so focus is measured across every window, in the
/// one place all notification paths share (scheduler::notifications_wanted).
#[tauri::command]
pub async fn send_notification(app: AppHandle, title: String, body: String) -> Result<(), String> {
    if crate::scheduler::notifications_wanted(&app).await {
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().builder().title(title).body(body).show();
    }
    Ok(())
}

#[tauri::command]
pub async fn get_ai_config(state: State<'_, AppState>) -> Result<AiConfig, String> {
    let ai = state.ai.read().await.clone();
    Ok(ai.config().clone())
}

#[tauri::command]
pub async fn set_ai_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: AiConfig,
) -> Result<(), String> {
    apply_ai_config(&app, &state, config).await
}

/// Persist a new AiConfig and rebuild the live Ai around it. Shared by the
/// Settings UI (`set_ai_config`), the chat/MCP `settings` tool, and the
/// error-row fix buttons — one write path, so nothing can half-apply.
pub(crate) async fn apply_ai_config(
    app: &AppHandle,
    state: &AppState,
    mut config: AiConfig,
) -> Result<(), String> {
    // Keep the provider list and flat legacy fields coherent on every save.
    config.normalize();
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&state.config_path, json).map_err(|e| e.to_string())?;
    let (mcp_enabled, mcp_port) = (config.mcp_enabled, config.mcp_port);
    let (clip_enabled, clip_port) = (config.clip_enabled, config.clip_port);
    crate::integrations::set_tray_visible(app, config.tray_enabled);
    {
        let mut ai = state.ai.write().await;
        let data_dir = state
            .config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        *ai = Ai::new(config, ai_runtime(app.clone(), data_dir));
        // Fusion follows the embedder tier (BEIR-measured; db.rs).
        state.db.set_fusion(ai.fusion_params());
    }
    crate::mcp::apply_config(app, mcp_enabled, mcp_port).await;
    crate::clip::apply_config(app, clip_enabled, clip_port).await;
    Ok(())
}

/// Apply one settings-tool change from an error-row fix button
/// (RFC-self-resolve phases 2+3): same allowlist and refusals as the chat
/// `settings` tool, and the applied change lands in the transcript as a
/// tool row — the config never moves silently.
#[tauri::command]
pub async fn apply_settings_fix(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: String,
    field: String,
    value: String,
) -> Result<Message, String> {
    let mut config = { state.ai.read().await.config().clone() };
    let echo = crate::selfheal::settings_set(&mut config, &field, &value)?;
    apply_ai_config(&app, &state, config).await?;
    notify_changed("settings", None);
    finish_tool_reply(&app, &state, &notebook_id, echo).await
}

#[tauri::command]
pub async fn list_models(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let ai = state.ai.read().await.clone();
    e(ai.list_models().await)
}

#[tauri::command]
pub async fn check_ollama(state: State<'_, AppState>) -> Result<bool, String> {
    let ai = state.ai.read().await.clone();
    Ok(ai.list_models().await.is_ok())
}

#[cfg(test)]
mod tool_tests {
    use super::*;

    #[test]
    fn chat_presets_compose_style_and_length_guidance() {
        let instruction = |style: &str, length: &str| {
            chat_style_instruction(&ChatConfig {
                style: style.into(),
                length: length.into(),
                ..Default::default()
            })
        };

        let friendly = instruction("friendly", "default");
        assert!(friendly.starts_with(rag::style_instructions("friendly").unwrap()));
        assert!(friendly.contains("presentation only"));

        let professional_concise = instruction("professional", "shorter");
        assert!(professional_concise.starts_with(rag::style_instructions("professional").unwrap()));
        assert!(
            professional_concise.contains("no more than three short paragraphs or five bullets")
        );
        assert!(professional_concise.contains("essential evidence, caveats, and citations"));

        let thorough = instruction("default", "longer");
        assert!(thorough.starts_with("Answer thoroughly. Lead with the conclusion"));
        assert!(thorough.contains("source-supported examples"));
        assert!(thorough.contains("Do not pad, repeat, or add unsupported background"));

        assert_eq!(instruction("default", "default"), "");
    }

    /// RFC-source-tags: `#foo` → `foo`, lowercase, dedupe (first-seen order
    /// kept), whitespace/commas both split, empties vanish.
    #[test]
    fn normalize_tags_strips_lowercases_dedupes() {
        assert_eq!(normalize_tags("#Rust  lance,RUST"), "rust lance");
        assert_eq!(normalize_tags("  "), "");
        assert_eq!(normalize_tags("#a, #B\n#a"), "a b");
        assert_eq!(normalize_tags("one-tag two_tag"), "one-tag two_tag");
        assert_eq!(normalize_tags("#,, ,#"), "");
        assert_eq!(normalize_tags("Ökologie"), "ökologie");
    }

    #[test]
    fn blank_title_catches_invisible_content() {
        // Real content is not blank.
        assert!(!is_blank_title("Architecture RFC"));
        assert!(!is_blank_title("  padded but real  "));
        // Ordinary whitespace/control — blank.
        assert!(is_blank_title(""));
        assert!(is_blank_title("   \n\t "));
        // The bug that evaded three trim()-based guards: zero-width space,
        // ZWNJ/ZWJ, word-joiner, BOM — not whitespace, so trim() kept them
        // and the row rendered empty.
        assert!(is_blank_title("\u{200b}"));
        assert!(is_blank_title("\u{feff}\u{200d}"));
        assert!(is_blank_title(" \u{200b}\u{2060} "));
        // But a real char alongside a zero-width space is still a real title.
        assert!(!is_blank_title("A\u{200b}"));
    }

    #[test]
    fn presentable_title_falls_back_past_invisible() {
        assert_eq!(
            presentable_title("Real Title", "https://x.com"),
            "Real Title"
        );
        assert_eq!(
            presentable_title("\u{200b}", "https://www.example.com/page"),
            "example.com"
        );
        assert_eq!(presentable_title("   ", ""), "Untitled source");
    }

    #[test]
    fn detect_cloud_folders_finds_and_labels_roots() {
        // A throwaway HOME mirroring the real macOS cloud-storage layout.
        let home = std::env::temp_dir().join(format!("nbl-cloud-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let cloud = home.join("Library/CloudStorage");
        for name in [
            "GoogleDrive-me@gmail.com",
            "OneDrive-Personal",
            "Box-Box",
            "Dropbox",
            "Photos-Ignored",
        ] {
            std::fs::create_dir_all(cloud.join(name)).unwrap();
        }
        std::fs::create_dir_all(home.join("Library/Mobile Documents/com~apple~CloudDocs")).unwrap();

        let found = detect_cloud_folders(&home);
        let label = |p: &str| {
            found
                .iter()
                .find(|c| c.provider == p)
                .map(|c| c.label.as_str())
        };
        assert_eq!(label("google_drive"), Some("Google Drive"));
        assert_eq!(label("onedrive"), Some("OneDrive"));
        assert_eq!(label("box"), Some("Box"));
        assert_eq!(label("dropbox"), Some("Dropbox"));
        assert_eq!(label("icloud"), Some("iCloud Drive"));
        // Unknown CloudStorage dirs aren't offered.
        assert!(found.iter().all(|c| !c.path.contains("Ignored")));
        // Every detected root actually exists.
        assert!(found.iter().all(|c| std::path::Path::new(&c.path).is_dir()));

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn detect_cloud_folders_dedupes_symlinked_legacy_root() {
        // ~/Dropbox as a symlink into CloudStorage/Dropbox must count once.
        let home = std::env::temp_dir().join(format!("nbl-cloud-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let real = home.join("Library/CloudStorage/Dropbox");
        std::fs::create_dir_all(&real).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, home.join("Dropbox")).unwrap();

        let found = detect_cloud_folders(&home);
        let dropboxes = found.iter().filter(|c| c.provider == "dropbox").count();
        assert_eq!(
            dropboxes, 1,
            "symlinked legacy root should dedupe: {found:?}"
        );

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn auto_evidence_parsing_is_conservative() {
        use crate::rag::parse_auto_evidence;
        // Explicit declines, any casing/decoration, with or without prose.
        assert!(parse_auto_evidence("SKIP").is_none());
        assert!(parse_auto_evidence("  skip — just a lookup").is_none());
        assert!(parse_auto_evidence("**SKIP**").is_none());
        assert!(parse_auto_evidence("Decision: KEEP — distinct claims").is_none());
        // Malformed output (no TITLE line) is a skip, not a bad note.
        assert!(parse_auto_evidence("Here's a note about deductibles...").is_none());
        assert!(parse_auto_evidence("TITLE: no body follows").is_none());
        assert!(parse_auto_evidence("").is_none());
        // The well-formed case round-trips.
        let (title, body) = parse_auto_evidence(
            "TITLE: The hail deductible is $2,500\n\n**Claim:** The deductible is $2,500.\n**Evidence:** \"…\" (Insurance Policy)",
        )
        .expect("parses");
        assert_eq!(title, "The hail deductible is $2,500");
        assert!(body.starts_with("**Claim:**"));
    }

    #[test]
    fn auto_evidence_parsing_survives_model_dialects() {
        use crate::rag::parse_auto_evidence;
        // Markdown-bold marker — the way chat models actually write it.
        let (title, _) = parse_auto_evidence(
            "**TITLE:** CNT tethers fall short today\n\n**Claim:** …\n**Evidence:** … (Carbon nanotube)",
        )
        .expect("bold marker parses");
        assert_eq!(title, "CNT tethers fall short today");
        // Lowercase marker.
        assert!(parse_auto_evidence("Title: x\n\nbody text").is_some());
        // Reasoning preamble (long lines) before the record must not kill it.
        let long_preamble = format!(
            "{}\n{}\nTITLE: The claim survives preambles\n\n**Claim:** …",
            "The user asked a cross-source question and the answer synthesized material.".repeat(3),
            "Weighing whether this is durable enough to record as evidence.",
        );
        let (title, _) = parse_auto_evidence(&long_preamble).expect("preamble tolerated");
        assert_eq!(title, "The claim survives preambles");
        // A title containing the word KEEPS is not a decline.
        let (title, _) =
            parse_auto_evidence("TITLE: The 458 keeps its value better\n\nbody").expect("parses");
        assert!(title.contains("keeps"));
        // Multibyte first characters must not panic the slicer.
        assert!(parse_auto_evidence("日本語のプレアンブル\nTITLE: works\n\nbody").is_some());
    }

    #[test]
    fn similar_pairs_greedy_and_thresholded() {
        // v0 ≈ v1 (near-duplicates), v2 orthogonal, v3 = v0 exactly.
        let embeds = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.95, 0.05, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![1.0, 0.0, 0.0],
        ];
        let pairs = similar_pairs(&embeds, 0.75);
        // (0,3) is a perfect match and wins first; 1 then has no free partner
        // above threshold left besides consumed ones, so exactly one pair…
        // unless (1, x) still clears 0.75 — v1·v0 ≈ 0.999 but 0 is consumed.
        assert_eq!(pairs, vec![(0, 3)], "greedy, no index reuse");
        // Orthogonal vectors never pair.
        assert!(similar_pairs(&[vec![1.0, 0.0], vec![0.0, 1.0]], 0.75).is_empty());
        // Degenerate vectors are safe.
        assert!(similar_pairs(&[vec![0.0, 0.0], vec![0.0, 0.0]], 0.75).is_empty());
    }

    #[test]
    fn title_overlap_finds_same_claim() {
        assert!(
            title_overlap(
                "The hail deductible is $2,500",
                "Hail deductible is 2500 dollars"
            ) >= 0.4
        );
        assert!(
            title_overlap(
                "The hail deductible is $2,500",
                "Router firmware updates monthly"
            ) < 0.2
        );
        assert_eq!(title_overlap("", "anything"), 0.0);
    }

    #[test]
    fn context_url_requests_are_detected() {
        assert!(wants_add_context_urls("please add those urls as sources"));
        assert!(wants_add_context_urls(
            "save the links you listed as sources"
        ));
        assert!(wants_add_context_urls("add the cited pages"));
        // No anaphor — plain add with explicit URL goes through the normal path.
        assert!(!wants_add_context_urls(
            "add https://example.com as a source"
        ));
        // No add verb — a question about links is not a command.
        assert!(!wants_add_context_urls("what are those links about?"));
    }

    #[test]
    fn urls_extracted_from_prose_and_markdown() {
        assert_eq!(
            extract_urls("see https://a.com/x. Also (https://b.com/y), and `https://c.com`!"),
            vec!["https://a.com/x", "https://b.com/y", "https://c.com"]
        );
        assert!(extract_urls("no links here").is_empty());
    }

    #[test]
    fn gate_passes_commands_and_blocks_questions() {
        assert!(tool_gate("add https://example.com please"));
        assert!(tool_gate("make a study guide"));
        assert!(tool_gate("delete the ferrari source"));
        assert!(tool_gate("refresh my urls and sources"));
        assert!(!tool_gate("what does the spec say about pricing?"));
        assert!(!tool_gate("compare the two cars"));
    }

    #[test]
    fn gate_reaches_night_shift_administration() {
        // The Tonight composer and the chat box are the same parser, so
        // these have to pass the cheap gate before the router ever sees them.
        assert!(tool_gate("pause the night shift until morning"));
        assert!(tool_gate("show me tonight's plan"));
        assert!(tool_gate("schedule a commission for the japan notebook"));
        // Ordinary questions still take the zero-overhead path.
        assert!(!tool_gate("what happened in the meeting last night?"));
    }

    #[test]
    fn parses_commission() {
        match parse_tool_action(
            r#"{"action":"commission","kind":"custom","name":"Deep read","prompt":"re-read every source","when":"tonight"}"#,
        ) {
            ToolAction::Commission {
                kind,
                name,
                prompt,
                when,
            } => {
                assert_eq!(kind, "custom");
                assert_eq!(name, "Deep read");
                assert_eq!(prompt, "re-read every source");
                assert_eq!(when, "tonight");
            }
            _ => panic!("expected commission"),
        }
        // A commission with no name is not actionable — fall back to chat
        // rather than queueing something the user cannot recognise later.
        assert!(matches!(
            parse_tool_action(r#"{"action":"commission","kind":"custom","name":""}"#),
            ToolAction::Chat
        ));
    }

    #[test]
    fn parses_night_shift_ops() {
        assert!(matches!(
            parse_tool_action(r#"{"action":"night_shift","op":"pause"}"#),
            ToolAction::NightShift { op } if op == "pause"
        ));
        assert!(matches!(
            parse_tool_action(r#"{"action":"night_shift","op":"status"}"#),
            ToolAction::NightShift { op } if op == "status"
        ));
    }

    #[test]
    fn parses_generate() {
        match parse_tool_action(
            r#"{"action":"generate","kind":"study_guide","prompt":"focus on ch 2"}"#,
        ) {
            ToolAction::Generate { kind, prompt } => {
                assert_eq!(kind, "study_guide");
                assert_eq!(prompt, "focus on ch 2");
            }
            _ => panic!("expected generate"),
        }
    }

    #[test]
    fn parses_remove_and_refresh() {
        assert!(matches!(
            parse_tool_action(r#"{"action":"remove_source","name":"ferrari"}"#),
            ToolAction::RemoveSource(n) if n == "ferrari"
        ));
        assert!(matches!(
            parse_tool_action(r#"{"action":"refresh_sources","name":""}"#),
            ToolAction::RefreshSources(n) if n.is_empty()
        ));
    }

    #[test]
    fn parses_schedule_intervals() {
        match parse_tool_action(
            r#"{"action":"schedule_report","kind":"briefing","interval":"weekly","name":"News"}"#,
        ) {
            ToolAction::ScheduleReport { interval, name, .. } => {
                assert_eq!(interval, "weekly");
                assert_eq!(name, "News");
            }
            _ => panic!("expected schedule"),
        }
        // Unknown kind and unsupported cadence both survive parsing verbatim;
        // dispatch validates against the live registry (which the parser can't
        // see) and refuses politely instead of coercing to a different report.
        match parse_tool_action(
            r#"{"action":"schedule_report","kind":"podcast","interval":"monthly","name":"X"}"#,
        ) {
            ToolAction::ScheduleReport { kind, interval, .. } => {
                assert_eq!(kind, "podcast");
                assert_eq!(interval, "monthly"); // preserved for the refusal reply
            }
            _ => panic!("expected schedule"),
        }
        // The dispatch-time validator: registry kinds pass, unknown kinds get
        // the refusal that names alternatives, custom demands a prompt.
        assert_eq!(resolve_report_kind("briefing", ""), Ok("briefing".into()));
        // The cross-notebook brief is its own kind, distinct from "briefing".
        assert_eq!(resolve_report_kind("brief", ""), Ok("brief".into()));
        assert_eq!(resolve_report_kind("Brief", ""), Ok("brief".into()));
        assert_eq!(
            resolve_report_kind("round_table", ""),
            Ok("round_table".into())
        );
        assert_eq!(
            resolve_report_kind("custom", "track prices"),
            Ok("custom".into())
        );
        assert!(resolve_report_kind("custom", " ").is_err());
        assert!(resolve_report_kind("", "").is_err());
        // Custom reports carry their prompt through.
        match parse_tool_action(
            r#"{"action":"schedule_report","kind":"custom","interval":"daily","name":"X","prompt":"track prices"}"#,
        ) {
            ToolAction::ScheduleReport { kind, prompt, .. } => {
                assert_eq!(kind, "custom");
                assert_eq!(prompt, "track prices");
            }
            _ => panic!("expected schedule"),
        }
    }

    #[test]
    fn parses_update_report() {
        match parse_tool_action(
            r#"{"action":"update_report","name":"price check","interval":"weekly","enabled":"false"}"#,
        ) {
            ToolAction::UpdateReport {
                name,
                interval,
                enabled,
                new_name,
                ..
            } => {
                assert_eq!(name, "price check");
                assert_eq!(interval, "weekly");
                assert_eq!(enabled, "false");
                assert!(new_name.is_empty());
            }
            _ => panic!("expected update"),
        }
        // A nameless update can't identify a schedule — falls through to chat.
        assert!(matches!(
            parse_tool_action(r#"{"action":"update_report","name":""}"#),
            ToolAction::Chat
        ));
    }

    #[test]
    fn fast_path_never_adds_on_destructive_verbs() {
        assert!(has_non_add_verb("delete https://example.com"));
        assert!(has_non_add_verb("refresh https://example.com"));
        assert!(!has_non_add_verb("add https://example.com"));
    }

    #[test]
    fn normalizes_schemeless_urls() {
        match parse_tool_action(
            r#"{"action":"add_urls","urls":["example.com/page","https://a.io"]}"#,
        ) {
            ToolAction::AddUrls(urls) => {
                assert_eq!(urls, vec!["https://example.com/page", "https://a.io"]);
            }
            _ => panic!("expected add_urls"),
        }
        // Junk without a dot is dropped; empty list collapses to Chat.
        assert!(matches!(
            parse_tool_action(r#"{"action":"add_urls","urls":["httpfoo"]}"#),
            ToolAction::Chat
        ));
    }

    #[test]
    fn parses_settings_tool() {
        assert!(matches!(
            parse_tool_action(r#"{"action":"settings","op":"get"}"#),
            ToolAction::Settings { op, .. } if op == "get"
        ));
        match parse_tool_action(
            r#"{"action":"settings","op":"set","field":"chatProvider","value":"ollama"}"#,
        ) {
            ToolAction::Settings { op, field, value } => {
                assert_eq!(op, "set");
                assert_eq!(field, "chatProvider");
                assert_eq!(value, "ollama");
            }
            _ => panic!("expected settings"),
        }
        // A set with no field can't do anything — falls through to chat,
        // as does an unknown op.
        assert!(matches!(
            parse_tool_action(r#"{"action":"settings","op":"set","value":"x"}"#),
            ToolAction::Chat
        ));
        assert!(matches!(
            parse_tool_action(r#"{"action":"settings","op":"delete"}"#),
            ToolAction::Chat
        ));
    }

    /// RFC-conversational-setup phase 1: the model verbs' router grammar.
    #[test]
    fn parses_model_verbs() {
        assert!(matches!(
            parse_tool_action(r#"{"action":"settings","op":"models"}"#),
            ToolAction::Settings { op, .. } if op == "models"
        ));
        // `test` carries its target in `field`; empty = active chat provider.
        match parse_tool_action(r#"{"action":"settings","op":"test","target":"gemma3"}"#) {
            ToolAction::Settings { op, field, .. } => {
                assert_eq!(op, "test");
                assert_eq!(field, "gemma3");
            }
            _ => panic!("expected settings test"),
        }
        assert!(matches!(
            parse_tool_action(r#"{"action":"settings","op":"test"}"#),
            ToolAction::Settings { op, field, .. } if op == "test" && field.is_empty()
        ));
        // `pull` needs a model name; without one it falls through to chat.
        match parse_tool_action(r#"{"action":"settings","op":"pull","model":"qwen3:8b"}"#) {
            ToolAction::Settings { op, field, .. } => {
                assert_eq!(op, "pull");
                assert_eq!(field, "qwen3:8b");
            }
            _ => panic!("expected settings pull"),
        }
        assert!(matches!(
            parse_tool_action(r#"{"action":"settings","op":"pull"}"#),
            ToolAction::Chat
        ));
        // Gate examples: the phrasings users actually type reach the router.
        assert!(tool_gate("list my installed models"));
        assert!(tool_gate("show me my models"));
        assert!(tool_gate("test ollama"));
        assert!(tool_gate("pull the qwen model"));
        assert!(tool_gate("download gemma3 in ollama"));
    }

    #[test]
    fn settings_requests_pass_the_gate() {
        assert!(tool_gate("switch chat to ollama"));
        assert!(tool_gate("use apple intelligence for chat"));
        assert!(tool_gate("show my model settings"));
        assert!(tool_gate("set the embedder to builtin"));
        // Plain questions still skip the router.
        assert!(!tool_gate("what does the spec say about latency?"));
    }

    /// The deterministic settings fast path runs in BOTH chat modes (deep
    /// research skips the LLM router), so its gate must be tight both ways:
    /// unambiguous imperatives route; anything question-shaped or long
    /// falls through to research untouched.
    #[test]
    fn settings_gate_routes_imperatives() {
        let gate = |s: &str| settings_gate(s).expect(s);
        assert_eq!(
            gate("switch chat to ollama"),
            ("set".into(), "chatProvider".into(), "ollama".into())
        );
        assert_eq!(
            gate("Switch studio to Apple Intelligence."),
            (
                "set".into(),
                "studioProvider".into(),
                "apple intelligence".into()
            )
        );
        assert_eq!(
            gate("switch the embedder to builtin"),
            ("set".into(), "embedder".into(), "builtin".into())
        );
        assert_eq!(gate("show my settings").0, "get");
        assert_eq!(gate("Get my settings?").0, "get");
        assert_eq!(gate("what models do I have?").0, "models");
        assert_eq!(gate("list my models").0, "models");
        assert_eq!(
            gate("test ollama"),
            ("test".into(), "ollama".into(), String::new())
        );
        assert_eq!(gate("Test gemma3:4b").1, "gemma3:4b");
        assert_eq!(gate("test apple intelligence").1, "apple intelligence");
        assert_eq!(
            gate("pull gemma3"),
            ("pull".into(), "gemma3".into(), String::new())
        );
        assert_eq!(gate("ollama pull qwen3:8b").1, "qwen3:8b");
    }

    /// RFC-conversational-setup phases 2 and 5: setup, theme, and "call me"
    /// join the deterministic gate.
    #[test]
    fn settings_gate_routes_setup_theme_and_profile() {
        let gate = |s: &str| settings_gate(s).expect(s);
        assert_eq!(gate("help me get set up").0, "setup");
        assert_eq!(gate("Set up Alchemy!").0, "setup");
        assert_eq!(
            gate("use the gruvbox theme"),
            ("theme".into(), "gruvbox".into(), String::new())
        );
        assert_eq!(gate("set the theme to nord").1, "nord");
        assert_eq!(gate("switch theme to tokyo night").1, "tokyo night");
        // Case survives for names — the gate lowercases only for matching.
        assert_eq!(
            gate("call me Paul"),
            ("set".into(), "profile.name".into(), "Paul".into())
        );
        assert_eq!(gate("Call me Dr. Thrash.").2, "Dr. Thrash");
        // Not-routed: clause-shaped or question-shaped stays research.
        assert!(settings_gate("call me when the report is done").is_none());
        assert!(settings_gate("how do I set up a home lab?").is_none());
        assert!(settings_gate("use the same theme as the website mockup").is_none());
    }

    /// RFC-conversational-setup phase 4: the unambiguous schedule shape
    /// routes deterministically in both modes; everything else falls through.
    #[test]
    fn schedule_gate_routes_the_brief_shape() {
        assert_eq!(
            schedule_gate("make a weekly brief of this notebook").unwrap(),
            ("brief".into(), "weekly".into(), "Weekly brief".into())
        );
        assert_eq!(
            schedule_gate("Create a daily summary.").unwrap(),
            ("summary".into(), "daily".into(), "Daily summary".into())
        );
        assert_eq!(
            schedule_gate("schedule an hourly faq for my sources")
                .unwrap()
                .0,
            "faq"
        );
        // Not-routed: real clauses, questions, unknown cadences/kinds, length.
        assert!(schedule_gate("make a weekly brief comparing rates and prices").is_none());
        assert!(schedule_gate("why make a weekly brief of this notebook").is_none());
        assert!(schedule_gate("make a monthly brief of this notebook").is_none());
        assert!(schedule_gate("make a weekly podcast of this notebook").is_none());
        assert!(schedule_gate("make a brief weekly of this notebook").is_none());
        let long = format!("make a weekly brief {}", "of this notebook ".repeat(5));
        assert!(schedule_gate(&long).is_none());
    }

    #[test]
    fn settings_gate_never_eats_research_questions() {
        // Paul's literal failure class: setup QUESTIONS are research.
        assert!(settings_gate("how do I set up ollama?").is_none());
        assert!(settings_gate("how do I switch chat providers in LM Studio?").is_none());
        assert!(settings_gate("why does my model keep timing out").is_none());
        assert!(settings_gate("can you switch chat to ollama").is_none());
        assert!(settings_gate("should I use the builtin embedder?").is_none());
        // Interrogative model questions outside the closed roster set.
        assert!(settings_gate("what models does llama.cpp support?").is_none());
        assert!(settings_gate("which models are best for summarization?").is_none());
        // "test …" with a research-shaped tail (too many words / bad charset).
        assert!(settings_gate("test the hypothesis that rates drive housing prices").is_none());
        assert!(settings_gate("test whether the spec covers latency, please").is_none());
        // "pull …" that isn't a bare model name.
        assert!(settings_gate("pull the latest research on transformers").is_none());
        assert!(settings_gate("pull gemma3; rm -rf /").is_none());
        // Long messages never gate, even with an imperative prefix.
        let long = format!(
            "switch chat to ollama {}",
            "because I read a very long argument about local inference".repeat(2)
        );
        assert!(settings_gate(&long).is_none());
        // Mid-sentence mentions don't gate — only leading imperatives.
        assert!(settings_gate("the paper says to switch chat to ollama").is_none());
        assert!(settings_gate("").is_none());
    }

    /// Registry cards as a search/ask leg: one matcher serves palette typing
    /// and question-shaped asks, and the passage text carries the facts.
    #[test]
    fn registry_cards_match_palette_and_question_shapes() {
        use crate::models::{CardFact, RegistryCard};
        let card = RegistryCard {
            id: "c1".into(),
            kind: "policy".into(),
            name: "Bayside Marina Policy".into(),
            origin: String::new(),
            triage: String::new(),
            identifiers: "bay-4471 hull-9921".into(),
            note: "Renews in September".into(),
            facts: vec![CardFact {
                label: "Policy number".into(),
                value: "BAY-4471".into(),
            }],
            attachments: vec![],
            created_at: 0,
            updated_at: 0,
        };
        // Palette typing: the query is a fragment of the name/identifiers.
        assert!(card_matches(&card, "bayside"));
        assert!(card_matches(&card, "bay-4471"));
        // Question shapes: a name word, an identifier token, or a fact
        // label appearing IN the question is the signal.
        assert!(card_matches(
            &card,
            "what's my policy number for the bayside boat"
        ));
        assert!(card_matches(
            &card,
            "when does hull-9921 come up for renewal?"
        ));
        assert!(card_matches(&card, "which marina am I insured with?"));
        // Unrelated questions never pull a card in; short words don't bridge.
        assert!(!card_matches(&card, "compare the q3 revenue projections"));
        assert!(!card_matches(&card, "the a of"));
        // The passage text is answer-grade: kind, identifiers, facts, note.
        let text = card_passage_text(&card);
        assert!(text.contains("Registry card (policy)"), "{text}");
        assert!(text.contains("Policy number: BAY-4471"), "{text}");
        assert!(text.contains("bay-4471 hull-9921"), "{text}");
        assert!(text.contains("Note: Renews in September"), "{text}");
    }

    #[test]
    fn falls_back_to_chat() {
        assert!(matches!(
            parse_tool_action("no json at all"),
            ToolAction::Chat
        ));
        assert!(matches!(
            parse_tool_action(r#"{"action":"chat"}"#),
            ToolAction::Chat
        ));
        assert!(matches!(
            parse_tool_action(r#"{"action":"add_urls","urls":[]}"#),
            ToolAction::Chat
        ));
        assert!(matches!(
            parse_tool_action(r#"{"action":"generate","kind":""}"#),
            ToolAction::Chat
        ));
    }
}
