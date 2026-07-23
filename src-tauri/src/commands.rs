//! Tauri command surface — the entire IPC API the React frontend calls.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use chrono::Utc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

mod reports;
pub use reports::*;

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

/// Launch Terminal.app running one of the known agent sign-in commands (the
/// "Fix:" hints on error rows). Strictly allowlisted: the command string
/// travels through model-adjacent error text, so nothing outside this fixed
/// set may ever reach a shell.
#[tauri::command]
pub fn open_in_terminal(command: String) -> Result<(), String> {
    const ALLOWED: [&str; 8] = [
        "claude",
        "codex login",
        "gemini",
        "cursor-agent login",
        "opencode auth login",
        "copilot",
        "hermes",
        "bob",
    ];
    if !ALLOWED.contains(&command.as_str()) {
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
    r.map_err(|err| format!("{err:#}"))
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
        title,
        created_at: ts,
        updated_at: ts,
        color: color.to_string(),
        source_count: 0,
    };
    e(state.db.create_notebook(&nb).await)?;
    Ok(nb)
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
        // Only ready sources count — error and placeholder rows have empty
        // content and would false-match each other.
        if s.char_count == char_count
            && s.status == "ready"
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
/// falling back to the origin's host and then "Untitled".
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
        "Untitled".to_string()
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

/// Chunk, embed, classify, and persist a new source row. `parent_id` is set
/// for folder children (which dedup by path, not content); `mtime` for any
/// file-backed source; `code_ctx` is the "repo › path" retrieval context for
/// code chunks when the caller knows it.
async fn store_new_source(
    state: &AppState,
    notebook_id: &str,
    extracted: ingest::Extracted,
    parent_id: &str,
    mtime: i64,
    code_ctx: Option<&str>,
    embed: bool,
) -> anyhow::Result<Source> {
    // Repository-tier code children store their content but skip embedding —
    // the ripgrep leg reaches them at query time (RFC-git-sources §4).
    let chunks = if embed {
        ingest::chunk_source(&extracted, code_ctx)
    } else {
        Vec::new()
    };
    let embed_inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let embeddings = {
        let ai = state.ai.read().await.clone();
        ai.embed(&embed_inputs).await?
    };

    let chunk_tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (new_id(), i as i32, c.text.clone()))
        .collect();

    let (status, error) = classify(&extracted.source_type, &extracted.url, &extracted.text);
    let source = Source {
        id: new_id(),
        notebook_id: notebook_id.to_string(),
        title: presentable_title(&extracted.title, &extracted.url),
        source_type: extracted.source_type,
        url: extracted.url,
        content: extracted.text.clone(),
        char_count: extracted.text.chars().count() as i64,
        chunk_count: chunk_tuples.len() as i64,
        created_at: now(),
        status,
        error,
        parent_id: parent_id.to_string(),
        mtime,
    };
    state
        .db
        .insert_source(&source, &chunk_tuples, &embeddings)
        .await?;
    state.db.touch_notebook(notebook_id, now()).await?;

    // Kick the gist sweep (RFC-infinite-context §1) — fire-and-forget, so
    // the import returns before any distillation happens.
    if embed {
        crate::gist::spawn_sweep(state.db.clone(), state.ai.read().await.clone());
    }

    // Don't ship the full content back in the list payload.
    Ok(Source {
        content: String::new(),
        ..source
    })
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
        title: ingest::file_title(path),
        source_type: "pdf".to_string(),
        url: String::new(),
        text,
    })
}

/// Filenames, slugs, and arXiv-style IDs make poor display titles. Markdown
/// gets its first heading; everything else asks the chat model for a short
/// title. Best-effort — any failure keeps the filename, titling must never
/// break an import.
pub(crate) async fn friendly_title(state: &AppState, extracted: &mut ingest::Extracted) {
    // Code files are their own best titles (db.rs IS the name) — and a repo
    // add would otherwise fire one model call per file.
    if extracted.source_type == "code" {
        return;
    }
    // A title containing spaces is usually already human-written.
    if extracted.title.contains(char::is_whitespace) {
        return;
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
            return;
        }
    }
    let excerpt: String = extracted.text.chars().take(1500).collect();
    let messages = vec![
        crate::ai::ChatTurn::system(
            "You title documents. Reply with ONLY a short descriptive title (3-8 words) for the \
             document excerpt — no quotes, no trailing punctuation, nothing else.",
        ),
        crate::ai::ChatTurn::user(format!(
            "Filename: {}\n\nExcerpt:\n{excerpt}\n\nTitle:",
            extracted.title
        )),
    ];
    let out = {
        let ai = state.ai.read().await.clone();
        ai.chat_role(crate::inference::Role::Small, &messages).await
    };
    if let Ok(out) = out {
        let t = out
            .text
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .trim_matches(['"', '“', '”', '*', '#'])
            .trim();
        if !t.is_empty() && t.chars().count() <= 100 {
            extracted.title = t.to_string();
        }
    }
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
            Err(text_err) => extract_pdf_ocr(state, path)
                .await
                .map_err(|ocr_err| anyhow::anyhow!("{text_err} OCR fallback failed: {ocr_err}"))?,
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
    friendly_title(&state, &mut extracted).await;
    e(store_extracted(&state, &notebook_id, extracted).await)
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
    match crate::capture::extract_url_rescued(url).await {
        Ok(extracted) => store_extracted(state, notebook_id, extracted).await,
        Err(err) => store_failed_url(state, notebook_id, url.trim(), err.to_string()).await,
    }
}

/// App data dir (`config_path`'s parent) — capture memory, git host memory,
/// and git cache checkouts live here.
fn app_data_dir(state: &AppState) -> std::path::PathBuf {
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
                eprintln!("git: failed to adopt cache for {}: {err:#}", src.id);
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
        return Err("Enter a pattern to grep for.".to_string());
    }
    let files = repo_backed_files(&state, &notebook_id, None).await;
    if files.is_empty() {
        return Err("This notebook has no repo- or folder-backed files to grep.".to_string());
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
            let (_, id, title) = &files[h.file_index];
            Citation {
                chunk_id: format!("grep:{}:{}", id, h.first_line),
                source_id: id.clone(),
                source_title: title.clone(),
                note_id: String::new(),
                gist: false,
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

/// Re-chunk, re-embed, and replace a source's content in place (edit /
/// refresh). `code_ctx` as in `store_new_source`.
async fn reingest(
    state: &AppState,
    existing: &Source,
    extracted: ingest::Extracted,
    code_ctx: Option<&str>,
    embed: bool,
) -> anyhow::Result<Source> {
    // Repository-tier code children store their content but skip embedding —
    // the ripgrep leg reaches them at query time (RFC-git-sources §4).
    let chunks = if embed {
        ingest::chunk_source(&extracted, code_ctx)
    } else {
        Vec::new()
    };
    let embed_inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let embeddings = {
        let ai = state.ai.read().await.clone();
        ai.embed(&embed_inputs).await?
    };
    let chunk_tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (new_id(), i as i32, c.text.clone()))
        .collect();

    // Classify against the stored URL: text edits arrive via extract_pasted
    // with an empty extracted.url, which would drop the Google-doc exemption.
    let (status, error) = classify(&existing.source_type, &existing.url, &extracted.text);
    // An empty extracted.url means the text came from an edit or paste, not a
    // re-fetch — keep the stored origin (URL or file path) so refresh keeps
    // working after edits.
    let url = if extracted.url.is_empty() {
        existing.url.clone()
    } else {
        extracted.url
    };
    let updated = Source {
        id: existing.id.clone(),
        notebook_id: existing.notebook_id.clone(),
        title: presentable_title(&extracted.title, &url),
        source_type: existing.source_type.clone(),
        url,
        content: extracted.text.clone(),
        char_count: extracted.text.chars().count() as i64,
        chunk_count: chunk_tuples.len() as i64,
        created_at: existing.created_at,
        status,
        error,
        // Folder membership and change-tracking travel with the row; a rescan
        // that re-ingests a changed file passes `existing` with a fresh mtime.
        parent_id: existing.parent_id.clone(),
        mtime: existing.mtime,
    };
    state
        .db
        .replace_source(&updated, &chunk_tuples, &embeddings)
        .await?;
    state
        .db
        .touch_notebook(&existing.notebook_id, now())
        .await?;
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

/// Does this source origin point at the web (vs. a local file path)?
fn is_web_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

#[tauri::command]
pub async fn refresh_source_url(
    app: AppHandle,
    state: State<'_, AppState>,
    source_id: String,
) -> Result<Source, String> {
    let existing =
        e(state.db.get_source(&source_id).await)?.ok_or_else(|| "Source not found".to_string())?;
    if existing.url.is_empty() {
        return Err("This source has no URL or file path to refresh from".into());
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
                let dir = crate::notion::cache_dir(&app_data_dir(&state), &existing.id);
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
                    Err(err) => return Err(format!("Notion refresh failed: {err:#}")),
                }
            }
        }
        // Git parents force a remote sync first so the rescan sees fresh
        // files; local folders scan the disk as-is.
        if existing.source_type == "git" {
            let dir = crate::git::cache_dir(&app_data_dir(&state), &existing.id);
            match crate::git::sync_remote(&dir).await {
                Ok(Some(sha)) => {
                    let stamp = crate::mac::content_stamp(&sha);
                    e(state.db.set_source_mtime(&existing.id, stamp).await)?;
                }
                Ok(None) => {}
                Err(err) => return Err(format!("git sync failed: {err:#}")),
            }
        }
        let _guard = state.folder_scan_lock.lock().await;
        e(rescan_one_folder(Some(&app), &state, &existing, true).await)?;
        let folder = e(state.db.get_source(&source_id).await)?
            .ok_or_else(|| "Source not found".to_string())?;
        return Ok(Source {
            content: String::new(),
            ..folder
        });
    }
    if crate::mac::is_mac_uri(&existing.url) {
        // Mac item — re-fetch through cider and re-embed. Like files, a
        // failed fetch (permission prompt pending, app closed) must not wipe
        // the working source.
        let (_, text) = e(crate::mac::fetch(&existing.url).await)?;
        let mut existing = existing;
        existing.mtime = crate::mac::content_stamp(&text);
        let extracted = ingest::Extracted {
            title: existing.title.clone(),
            source_type: "mac".to_string(),
            url: existing.url.clone(),
            text,
        };
        return e(reingest(&state, &existing, extracted, None, true).await);
    }
    // Git-backed singles (README/blob) refresh from their cache clone — the
    // cache dir is the definitive marker; page captures of github.com URLs
    // parse git-shaped too but have no clone.
    let git_dir = crate::git::cache_dir(&app_data_dir(&state), &existing.id);
    if git_dir.exists() {
        if let Err(err) = crate::git::sync_remote(&git_dir).await {
            return Err(format!("git sync failed: {err:#}"));
        }
        let sha = crate::git::detect_repo(&git_dir)
            .await
            .map(|r| r.sha)
            .unwrap_or_default();
        return e(reextract_git_single(&state, &existing, &sha).await);
    }
    if is_web_url(&existing.url) {
        return match crate::capture::extract_url_rescued(&existing.url).await {
            Ok(extracted) => e(reingest(&state, &existing, extracted, None, true).await),
            Err(err) => e(mark_source_failed(&state, &existing, err.to_string()).await),
        };
    }
    // File-backed source. Unlike a dead URL (where the errored row is the
    // retry affordance), a failed re-read must NOT wipe the working source —
    // the extracted text and chunks are still perfectly usable. Surface the
    // failure and leave the source untouched.
    if !std::path::Path::new(&existing.url).exists() {
        // iCloud eviction leaves only a hidden `.name.icloud` stub, which we
        // can't hydrate by reading — the user has to download it in Finder.
        let p = std::path::Path::new(&existing.url);
        let stub = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| p.with_file_name(format!(".{n}.icloud")));
        if stub.is_some_and(|s| s.exists()) {
            return Err(
                "This file is online-only in iCloud — download it in Finder first".to_string(),
            );
        }
        return Err(format!(
            "Original file no longer exists at {}",
            existing.url
        ));
    }
    let mut extracted = e(extract_any_file(&state, &existing.url).await)?;
    let mut existing = existing;
    if existing.status == "placeholder" {
        // First real read of an evicted file (reading it just hydrated it) —
        // give it a real title like any fresh import.
        friendly_title(&state, &mut extracted).await;
    } else {
        // Keep the existing title — the file's content changed, its name
        // didn't, and the stored title may be friendlier than the file stem.
        extracted.title = existing.title.clone();
    }
    // Stamp the on-disk mtime, or the next folder rescan would see a mismatch
    // and re-embed this file a second time.
    existing.mtime = file_mtime(std::path::Path::new(&existing.url));
    e(reingest(&state, &existing, extracted, None, true).await)
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
            let data_dir = app_data_dir(&state);
            // Parent and children can each own a git or notion cache dir; the
            // bulk delete_source_tree drops their rows in one shot.
            for id in child_ids.iter().chain(std::iter::once(&source_id)) {
                crate::git::remove_cache(&data_dir, id);
                let nc = crate::notion::cache_dir(&data_dir, id);
                if nc.exists() {
                    let _ = std::fs::remove_dir_all(&nc);
                }
            }
            e(state.db.delete_source_tree(&source_id, &child_ids).await)?;
            return Ok(());
        }
    }
    e(state.db.delete_source(&source_id).await)?;
    // Git and Notion sources leave cache dirs behind — no-ops otherwise.
    crate::git::remove_cache(&app_data_dir(&state), &source_id);
    let notion_cache = crate::notion::cache_dir(&app_data_dir(&state), &source_id);
    if notion_cache.exists() {
        let _ = std::fs::remove_dir_all(&notion_cache);
    }
    Ok(())
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

/// Post-write resync: fetch the item's current state and re-embed it.
pub(crate) async fn resync_mac_source(
    state: &AppState,
    mut existing: Source,
) -> Result<Source, String> {
    let (_, text) = e(crate::mac::fetch(&existing.url).await)?;
    existing.mtime = crate::mac::content_stamp(&text);
    let extracted = ingest::Extracted {
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
    "pdf", "txt", "text", "md", "markdown", "html", "htm", "xhtml", "docx", "pptx", "epub",
    "boxnote", "xlsx", "xls", "xlsm", "ods", "csv", "tsv", "gdoc", "gsheet", "gslides", "png",
    "jpg", "jpeg", "jpe", "webp", "gif", "bmp", "tif", "tiff", "heic", "heif", "avif", "ico",
    "jp2",
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
        eprintln!("folder scan: FTS flush failed: {err:#}");
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
                    friendly_title(state, &mut extracted).await;
                    let ctx = code_context(&folder.title, root, path);
                    store_new_source(
                        state,
                        &folder.notebook_id,
                        extracted,
                        &folder.id,
                        mtime,
                        ctx.as_deref(),
                        embed_file(path),
                    )
                    .await?;
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
                    if existing.status == "placeholder" {
                        // First real read of this file — give it a real title.
                        friendly_title(state, &mut extracted).await;
                    } else {
                        // Keep the stored title: the content changed, not the
                        // file. (A failed child keeps its filename title.)
                        extracted.title = existing.title.clone();
                    }
                    let ctx = code_context(&folder.title, root, path);
                    reingest(
                        state,
                        &existing,
                        extracted,
                        ctx.as_deref(),
                        embed_file(path),
                    )
                    .await?;
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
                    eprintln!("folder rescan: failed to re-read {path}: {err:#}");
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
            eprintln!("note backfill: listing failed: {err:#}");
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
            eprintln!("report collapse: listing schedules failed: {err:#}");
            return;
        }
    };
    for s in schedules {
        if let Err(err) = collapse_report_notes(state, &s.notebook_id, &s.name).await {
            eprintln!("report collapse for \"{}\" failed: {err:#}", s.name);
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
                title: format!("\"{title}\" absorbed \"{}\"", loser.title),
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
            Err(err) => eprintln!("note curator failed: {err:#}"),
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
            Err(err) => eprintln!("note consolidation failed: {err:#}"),
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
                        title: "Curator report".into(),
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
            eprintln!("curator report for {notebook_id} failed: {err:#}");
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
    eprintln!("note curator: {} action(s)", actions.len());
}

/// Rescan every folder source and re-embed loose file sources whose on-disk
/// file changed (the frontend ticks this once a minute from the main window,
/// and on notebook open). Emits `sources://changed` per notebook that
/// actually changed. Missing files never remove a loose source — uploads are
/// snapshots; the origin path is only a refresh hint.
#[tauri::command]
pub async fn resync_sources(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<FolderScan, String> {
    // The Spotlight index rides the same tick (internally ~10-min throttled).
    #[cfg(target_os = "macos")]
    crate::spotlight::refresh_if_due(&state).await;
    // One-shot per app run: index notes written before notes joined the
    // retrieval index (or whose indexing failed at write time).
    backfill_note_index(&state).await;
    // One-shot per app run: collapse timestamped report piles from before
    // reports became living notes (one note per schedule, newest wins).
    collapse_old_report_piles(&state).await;
    // Curator: track app-open days; runs its pass at most weekly.
    note_curator_tick(&app, &state).await;
    // A manual folder add/refresh is already scanning — skip this tick rather
    // than queue behind it and ingest the same files twice.
    let Ok(_guard) = state.folder_scan_lock.try_lock() else {
        return Ok(FolderScan::default());
    };
    let mut total = FolderScan::default();
    let mut per_notebook: HashMap<String, FolderScan> = HashMap::new();
    // The auto-sync cadence setting: minutes between remote probes, 0 = off
    // (manual Refresh still syncs).
    let sync_minutes = { state.ai.read().await.config().git_sync_minutes };
    for folder in e(state.db.all_folder_sources().await)? {
        // Remote repos: one cheap ls-remote per cadence tick; a moved branch
        // refetches the cache so the ordinary rescan below sees fresh
        // mtimes. Never runs against user repos — only our own clones.
        if folder.source_type == "git"
            && sync_minutes > 0
            && crate::git::remote_probe_due(&folder.id, sync_minutes)
        {
            let dir = crate::git::cache_dir(&app_data_dir(&state), &folder.id);
            match crate::git::sync_remote(&dir).await {
                Ok(Some(sha)) => {
                    let stamp = crate::mac::content_stamp(&sha);
                    let _ = state.db.set_source_mtime(&folder.id, stamp).await;
                }
                Ok(None) => {}
                Err(err) => eprintln!("git resync: {} failed: {err:#}", folder.url),
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
                let dir = crate::notion::cache_dir(&app_data_dir(&state), &folder.id);
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
                    Err(err) => eprintln!("notion resync: {} failed: {err:#}", folder.url),
                }
            }
        }
        match rescan_one_folder(Some(&app), &state, &folder, false).await {
            Ok(scan) => {
                per_notebook
                    .entry(folder.notebook_id.clone())
                    .or_default()
                    .absorb(scan);
                total.absorb(scan);
            }
            Err(err) => {
                eprintln!("folder rescan: {} failed: {err:#}", folder.url);
                total.failed += 1;
            }
        }
    }

    // Loose file sources (added or dropped individually) re-embed when their
    // file changes. Deleted files leave the source untouched; cloud-evicted
    // files aren't read (that would force a download).
    let data_dir = app_data_dir(&state);
    for src in e(state.db.all_loose_sources().await)? {
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
                    match reextract_git_single(&state, &src, &sha).await {
                        Ok(_) => {
                            scan.updated += 1;
                            total.updated += 1;
                        }
                        Err(err) => {
                            eprintln!("git resync: failed to re-embed {}: {err:#}", src.url);
                            scan.failed += 1;
                            total.failed += 1;
                        }
                    }
                }
                Ok(None) => {}
                Err(err) => eprintln!("git resync: {} failed: {err:#}", src.url),
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
                        title: existing.title.clone(),
                        source_type: "mac".to_string(),
                        url: existing.url.clone(),
                        text,
                    };
                    let scan = per_notebook.entry(src.notebook_id.clone()).or_default();
                    match reingest(&state, &existing, extracted, None, true).await {
                        Ok(_) => {
                            scan.updated += 1;
                            total.updated += 1;
                        }
                        Err(err) => {
                            eprintln!("mac resync: failed to re-embed {}: {err:#}", src.url);
                            scan.failed += 1;
                            total.failed += 1;
                        }
                    }
                }
                Err(err) => {
                    // Keep the working text; permission prompts and closed
                    // apps are transient. The cadence gate throttles retries.
                    eprintln!("mac resync: failed to fetch {}: {err:#}", src.url);
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
        match extract_any_file(&state, &src.url).await {
            Ok(mut extracted) => {
                let mut existing = src.clone();
                existing.mtime = mtime;
                // Content changed, not the file's name — keep the stored title.
                extracted.title = existing.title.clone();
                match reingest(&state, &existing, extracted, None, true).await {
                    Ok(_) => {
                        scan.updated += 1;
                        total.updated += 1;
                    }
                    Err(err) => {
                        eprintln!("file resync: failed to re-embed {}: {err:#}", src.url);
                        scan.failed += 1;
                        total.failed += 1;
                    }
                }
            }
            Err(err) => {
                // Keep the working text; bump the mtime so a broken file isn't
                // re-attempted every minute.
                e(state.db.set_source_mtime(&src.id, mtime).await)?;
                eprintln!("file resync: failed to re-read {}: {err:#}", src.url);
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
struct StepEvent {
    label: String,
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
        _ => {}
    }
    match cfg.length.as_str() {
        "longer" => parts.push("Give thorough, detailed answers with examples.".into()),
        "shorter" => parts.push("Be concise — answer in just a few sentences.".into()),
        _ => {}
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
        "disable", "resume",
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
    ScheduleReport {
        kind: String,
        interval: String,
        name: String,
        prompt: String,
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
    Chat,
}

const TOOL_ROUTER_SYSTEM: &str = "You route a user's chat message in a research-notebook app. \
Decide if the message is a COMMAND to perform one of the tools below, or an ordinary question. \
Respond with EXACTLY ONE JSON object, nothing else.\n\n\
Tools:\n\
- {\"action\":\"add_urls\",\"urls\":[\"https://…\"]} — add the given URL(s) as sources.\n\
- {\"action\":\"add_text\",\"title\":\"<short title>\",\"text\":\"<the text to add>\"} — save text from the message as a source.\n\
- {\"action\":\"generate\",\"kind\":\"summary|faq|study_guide|briefing|timeline|problems|evidence|prd|prfaq|rfc|skill|custom\",\"prompt\":\"<extra instructions or empty>\"} — generate a document from the sources.\n\
- {\"action\":\"remove_source\",\"name\":\"<source name fragment>\"} — remove a source.\n\
- {\"action\":\"refresh_sources\",\"name\":\"<name fragment, or empty for all URL sources>\"} — re-fetch URL sources.\n\
- {\"action\":\"save_note\",\"title\":\"<title or empty>\"} — save the assistant's previous answer as a note.\n\
- {\"action\":\"schedule_report\",\"kind\":\"summary|briefing|timeline|faq|custom\",\"interval\":\"hourly|daily|weekly\",\"name\":\"<report name>\",\"prompt\":\"<what the report should cover, for kind custom; else empty>\"} — create a recurring report (echo the user's cadence word in \"interval\" even if unsupported).\n\
- {\"action\":\"update_report\",\"name\":\"<existing report name fragment>\",\"new_name\":\"\",\"kind\":\"\",\"interval\":\"\",\"prompt\":\"\",\"enabled\":\"true|false or empty\"} — change an existing recurring report; leave fields empty to keep them.\n\
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
    let messages = vec![
        crate::ai::ChatTurn::system(TOOL_ROUTER_SYSTEM),
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
        "schedule_report" => {
            // Keep the raw interval; dispatch validates it and refuses politely
            // on unsupported cadences instead of silently coercing.
            let kind = match s("kind").as_str() {
                k @ ("summary" | "briefing" | "timeline" | "faq" | "custom") => k.to_string(),
                _ => "briefing".to_string(),
            };
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
        _ => ToolAction::Chat,
    }
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
                },
            );
            match generate_content(state, None, notebook_id, &kind, &prompt, None, None).await {
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
            let interval_secs = match interval.as_str() {
                "hourly" => 3_600,
                "daily" => 86_400,
                "weekly" => 604_800,
                other => {
                    return Some(format!(
                        "I can schedule reports **hourly**, **daily**, or **weekly** — “{other}” isn't supported yet, so I haven't created anything. Rephrase with one of those cadences?"
                    ));
                }
            };
            let schedule = ReportSchedule {
                id: new_id(),
                notebook_id: notebook_id.to_string(),
                name: name.trim().to_string(),
                kind,
                prompt,
                interval_secs,
                enabled: true,
                last_run_at: 0,
                created_at: now(),
            };
            match state.db.add_report_schedule(&schedule).await {
                Ok(()) => Some(format!(
                    "Scheduled **{name}** to run {interval} — it refreshes your URL sources, then writes a timestamped note (first run starts shortly). Manage it under Studio → Reports."
                )),
                Err(err) => Some(format!("Couldn't create the schedule: {err:#}")),
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

#[tauri::command]
pub async fn send_message(
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
    let (query_vec, profile) = {
        let ai = state.ai.read().await.clone();
        (
            e(ai.embed_one(&content).await)?,
            ai.profile(crate::inference::Role::Chat),
        )
    };
    let selected_sources: Vec<Source> = e(state.db.list_sources(&notebook_id).await)?
        .into_iter()
        .filter(|s| source_ids.as_ref().is_none_or(|ids| ids.contains(&s.id)))
        .collect();
    let notebook_chars: i64 = selected_sources.iter().map(|s| s.char_count).sum();
    let k = profile.retrieve_k_for(notebook_chars);
    let search = e(state
        .db
        .search_chunks_trace(&notebook_id, query_vec, &content, k, source_ids.as_deref())
        .await)?;
    // The ripgrep leg (RFC-git-sources §6): code-shaped queries also
    // exact-match over the notebook's repo-backed files, and the windows
    // join the fusion as ordinary citations.
    let grep_hits = grep_leg(&state, &notebook_id, &content, source_ids.as_deref()).await;
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
            "warnings": search.warnings,
            "citations": crate::trace::cite_summaries(&search.final_hits),
        }),
    );
    let citations = fuse_grep_hits(search.final_hits, grep_hits, k);
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

    // Full source manifest (title + url) so corpus-level questions are
    // answerable regardless of which chunks the top-k search happened to
    // surface, and the model can propose new addable URLs. Respects the
    // source selection so deselected sources stay out of the prompt.
    let source_manifest: Vec<(String, String)> = selected_sources
        .into_iter()
        .map(|s| (s.title, s.url))
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

    // Stream the answer, emitting tokens to the frontend. Race against the
    // cancellation token so a Stop click aborts the request; on cancel we keep
    // whatever partial text streamed so far.
    let app_for_cb = app.clone();
    let cancel = state.begin_generation(&format!("chat:{}", window.label()));
    let partial = Arc::new(Mutex::new(String::new()));
    let partial_cb = partial.clone();
    let (answer, kind, stats, cost_usd, model) = {
        let ai = state.ai.read().await.clone();
        let model = ai.active_chat_model();
        let streamed = tokio::select! {
            out = ai.chat_stream(&messages, |tok| {
                partial_cb.lock().unwrap().push_str(tok);
                let _ = app_for_cb.emit(
                    "chat://token",
                    TokenEvent { content: tok.to_string() },
                );
            }) => Some(out),
            _ = cancel.cancelled() => None,
        };
        match streamed {
            Some(Ok(out)) => (out.text, "chat", out.stats, out.cost_usd, model),
            // A provider failure becomes a durable transcript row instead of
            // a vanishing toast: the stored user turn would otherwise sit
            // unanswered in history with no trace of why.
            Some(Err(err)) => (format!("{err:#}"), "error", None, None, model),
            None => (partial.lock().unwrap().clone(), "chat", None, None, model),
        }
    };
    state.record_chat_stats(&model, stats);

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
    let (answer, kind, citations, stats, model) = {
        let ai = state.ai.read().await.clone();
        let model = ai.active_chat_model();
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
            ) => Some(r),
            _ = cancel.cancelled() => None,
        };
        match out {
            Some(Ok((answer, citations, stats))) => (answer, "chat", citations, stats, model),
            // Durable transcript row for a failed run — same contract as the
            // direct chat path.
            Some(Err(err)) => (format!("{err:#}"), "error", vec![], None, model),
            None => ("_(Stopped.)_".to_string(), "chat", vec![], None, model),
        }
    };
    state.record_chat_stats(&model, stats);

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
        eprintln!(
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
            eprintln!("auto evidence pass failed: {err:#}");
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
        eprintln!(
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
            eprintln!("auto evidence: merge declined for \"{}\"", prior.title);
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
        eprintln!("auto evidence: created \"{}\"", note.title);
        note
    };

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
        eprintln!("note usage bump ({field}) failed: {err:#}");
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
        eprintln!("indexing note {} failed: {err:#}", note.id);
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
                "Your podcast voices are ready.",
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
        "The podcast voices aren't set up — download them in Settings → Models."
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
async fn generate_content(
    state: &AppState,
    app: Option<&AppHandle>,
    notebook_id: &str,
    kind: &str,
    prompt: &str,
    source_ids: Option<&[String]>,
    prior_report: Option<&str>,
) -> anyhow::Result<(String, String)> {
    // Known kinds use their spec (+ optional extra prompt); "custom"/unknown
    // kinds use the prompt itself as the instruction.
    let (title, mut instruction) = match rag::artifact_spec(kind) {
        Some((t, base)) => {
            let instr = if prompt.trim().is_empty() {
                base.to_string()
            } else {
                format!(
                    "{base}\n\nAdditional instructions from the user (follow these):\n{}",
                    prompt.trim()
                )
            };
            (t.to_string(), instr)
        }
        None => {
            if prompt.trim().is_empty() {
                anyhow::bail!("No instructions provided for this generation.");
            }
            ("Report".to_string(), prompt.trim().to_string())
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
    let is_gateway = { state.ai.read().await.config().is_gateway() };
    let budget: usize = if is_gateway { 150_000 } else { 24_000 };

    let mut contents = Vec::with_capacity(sources.len());
    for s in &sources {
        let full = state.db.source_content(&s.id).await?;
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
    // informs the "what changed" framing but must never crowd out sources.
    if let Some(prior) = prior_report {
        let cap = if is_gateway { 40_000 } else { 8_000 };
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
    let mut content = run_generation_chat(state, app, &messages).await?;

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
            let more = run_generation_chat(state, app, &messages).await?;
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
) -> anyhow::Result<String> {
    let (text, stats, model) = {
        let ai = state.ai.read().await.clone();
        let out = match app {
            Some(app) => {
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
            None => ai.chat(messages).await?,
        };
        (out.text, out.stats, ai.active_chat_model())
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
        r = generate_content(&state, Some(&app), &notebook_id, &kind, &prompt, source_ids.as_deref(), None) => Some(e(r)?),
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
        r = generate_content(&state, Some(&app), &notebook_id, &kind, &prompt, None, None) => Some(e(r)?),
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
             Respond with ONLY one original aphorism of 8-14 words about research, \
             knowledge, or transformation, in the voice of an alchemist's notebook, \
             tinted by the given mood. No quotation marks, no attribution, no preamble.",
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
    for s in e(state.db.list_sources(&target.notebook_id).await)? {
        if s.id == target.id || matches!(s.source_type.as_str(), "folder" | "obsidian") {
            continue;
        }
        let content = e(state.db.source_content(&s.id).await)?;
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
    let label = format!("win-{}", new_id());
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
}

#[tauri::command]
pub async fn corpus_stats(state: State<'_, AppState>) -> Result<CorpusStats, String> {
    let (sources, chars) = e(state.db.corpus_stats().await)?;
    Ok(CorpusStats { sources, chars })
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
    Err("Not an OKF bundle — expected index.md with sources/ and notes/ folders".into())
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
                .unwrap_or("Imported notebook")
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
            let nb = Notebook {
                id: new_id(),
                title,
                created_at: ts,
                updated_at: ts,
                color: NOTEBOOK_PALETTE[count.len() % NOTEBOOK_PALETTE.len()].to_string(),
                source_count: 0,
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
                    .unwrap_or("Untitled")
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
                    .unwrap_or("Untitled")
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
    /// "source" (chunk passage) | "note".
    pub kind: String,
    pub notebook_id: String,
    pub notebook_title: String,
    /// Source id for source passages; note id for notes.
    pub id: String,
    pub title: String,
    pub snippet: String,
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
        let ai = state.ai.read().await.clone();
        // Piggyback the gist sweep on the same self-heal moment the router
        // uses — catches sources imported before gisting existed (or while
        // the app was quitting mid-backfill).
        crate::gist::spawn_sweep(state.db.clone(), ai.clone());
        if let Err(err) = crate::router::ensure_router(&state.db, &ai).await {
            eprintln!("router refresh failed (falling back to flat): {err:#}");
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
                eprintln!("notebook routing failed (falling back to flat): {err:#}");
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
    // Deep search retrieves a wider pool for the reranker to pick from.
    let fetch_k = if deep { k * 3 } else { k };
    let mut out: Vec<MetaCitation> = e(state
        .db
        .search_chunks_all_opts(query_vec, question, fetch_k, routed.as_deref(), opts)
        .await)?
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
    for (id, nb, title, _) in e(state.db.all_source_meta().await)? {
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
    for n in e(state.db.recent_notes(usize::MAX).await)? {
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
            eprintln!("note usage bump (retrieval_hits) failed: {err:#}");
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
    for (i, (nb_id, gist)) in selected.iter().enumerate() {
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
        match global_meta_route(&state, &question).await {
            Ok(g) => g,
            Err(err) => {
                eprintln!("meta-global route failed, falling back to pointed: {err:#}");
                None
            }
        }
    } else {
        None
    };

    // References are per SOURCE, not per chunk: several excerpts from one
    // source share a number, and the citation list the UI shows is deduped —
    // otherwise a source that contributed five chunks shows up five times.
    let (citations, passages) = if let Some(g) = global {
        g
    } else {
        let passages_raw = retrieve_everything(&state, &question, 16, deep).await?;
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

    // Same stream/cancel dance as notebook chat, under its own scope so a
    // palette Esc never kills a notebook stream (or vice versa).
    let app_for_cb = app.clone();
    let cancel = state.begin_generation(&format!("meta:{}", window.label()));
    let partial = Arc::new(Mutex::new(String::new()));
    let partial_cb = partial.clone();
    let (answer, stats, model) = {
        let ai = state.ai.read().await.clone();
        let model = ai.active_chat_model();
        let streamed = tokio::select! {
            out = ai.chat_stream(&messages, |tok| {
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

#[tauri::command]
pub async fn check_models(state: State<'_, AppState>) -> Result<ModelHealth, String> {
    let ai = state.ai.read().await.clone();
    let cfg = ai.config().clone();
    let norm = |m: &str| m.trim_end_matches(":latest").to_string();

    // Chat status comes from the configured provider; embeddings and vision
    // remain Ollama-backed below.
    let gateway_chat = if cfg.provider == "openai" {
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

#[tauri::command]
pub async fn get_ai_config(state: State<'_, AppState>) -> Result<AiConfig, String> {
    let ai = state.ai.read().await.clone();
    Ok(ai.config().clone())
}

#[tauri::command]
pub async fn set_ai_config(
    app: AppHandle,
    state: State<'_, AppState>,
    mut config: AiConfig,
) -> Result<(), String> {
    // Keep the provider list and flat legacy fields coherent on every save.
    config.normalize();
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&state.config_path, json).map_err(|e| e.to_string())?;
    let (mcp_enabled, mcp_port) = (config.mcp_enabled, config.mcp_port);
    crate::integrations::set_tray_visible(&app, config.tray_enabled);
    {
        let mut ai = state.ai.write().await;
        let data_dir = state
            .config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        *ai = Ai::new(config, ai_runtime(app.clone(), data_dir));
    }
    crate::mcp::apply_config(&app, mcp_enabled, mcp_port).await;
    Ok(())
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
        assert_eq!(presentable_title("   ", ""), "Untitled");
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
        // Unsupported cadence survives parsing; dispatch refuses it politely.
        match parse_tool_action(
            r#"{"action":"schedule_report","kind":"podcast","interval":"monthly","name":"X"}"#,
        ) {
            ToolAction::ScheduleReport { kind, interval, .. } => {
                assert_eq!(kind, "briefing"); // unknown kinds coerce to a known one
                assert_eq!(interval, "monthly"); // preserved for the refusal reply
            }
            _ => panic!("expected schedule"),
        }
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
