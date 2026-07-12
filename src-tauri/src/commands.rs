//! Tauri command surface — the entire IPC API the React frontend calls.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use chrono::Utc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::ai::{Ai, AiConfig, GenStats};
use crate::db::Db;
use crate::models::{
    FolderScan, Message, ModelHealth, ModelStat, ModelStatus, Note, Notebook, ReportSchedule,
    Source,
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
    pub model_stats: Mutex<HashMap<String, ModelStatAcc>>,
    /// Cancellation tokens for in-flight generations, one per scope ("chat",
    /// "artifact", …) so stopping a chat doesn't kill a running document.
    pub cancel: Mutex<HashMap<String, tokio_util::sync::CancellationToken>>,
    /// Serializes folder scans: the periodic rescan tick skips while a manual
    /// folder add/refresh holds it, so the same file is never ingested twice.
    pub folder_scan_lock: tokio::sync::Mutex<()>,
}

impl AppState {
    /// Start a fresh cancellation scope for a new generation, returning its
    /// token. Supersedes any previous token in the same scope.
    pub fn begin_generation(&self, scope: &str) -> tokio_util::sync::CancellationToken {
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
pub fn ai_runtime(app: AppHandle, data_dir: std::path::PathBuf) -> crate::ai::AiRuntime {
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
        source_count: 0,
    };
    e(state.db.create_notebook(&nb).await)?;
    Ok(nb)
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
    store_new_source(state, notebook_id, extracted, "", mtime).await
}

/// Chunk, embed, classify, and persist a new source row. `parent_id` is set
/// for folder children (which dedup by path, not content); `mtime` for any
/// file-backed source.
async fn store_new_source(
    state: &AppState,
    notebook_id: &str,
    extracted: ingest::Extracted,
    parent_id: &str,
    mtime: i64,
) -> anyhow::Result<Source> {
    let chunks = ingest::chunk_text(&extracted.title, &extracted.text);
    let embed_inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let embeddings = {
        let ai = state.ai.read().await;
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
        title: extracted.title,
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

/// OCR an image file into an Extracted source using the vision model.
async fn extract_image(state: &AppState, path: &str) -> anyhow::Result<ingest::Extracted> {
    use base64::Engine;
    let bytes = std::fs::read(path).with_context(|| format!("failed to read {path}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let text = {
        let ai = state.ai.read().await;
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
            let ai = state.ai.read().await;
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
        let ai = state.ai.read().await;
        ai.chat(&messages).await
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

#[tauri::command]
pub async fn add_source_url(
    state: State<'_, AppState>,
    notebook_id: String,
    url: String,
) -> Result<Source, String> {
    e(ingest_url(&state, &notebook_id, &url).await)
}

/// Fetch a URL into a source. Hard failures (network / HTTP / empty) still
/// produce an errored source row so the user sees it and can retry.
pub(crate) async fn ingest_url(
    state: &AppState,
    notebook_id: &str,
    url: &str,
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
    match ingest::extract_url(url).await {
        Ok(extracted) => store_extracted(state, notebook_id, extracted).await,
        Err(err) => store_failed_url(state, notebook_id, url.trim(), err.to_string()).await,
    }
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

/// Re-chunk, re-embed, and replace a source's content in place (edit / refresh).
async fn reingest(
    state: &AppState,
    existing: &Source,
    extracted: ingest::Extracted,
) -> anyhow::Result<Source> {
    let chunks = ingest::chunk_text(&extracted.title, &extracted.text);
    let embed_inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let embeddings = {
        let ai = state.ai.read().await;
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
        title: extracted.title,
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
    e(reingest(&state, &existing, extracted).await)
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
    if existing.source_type == "folder" {
        let _guard = state.folder_scan_lock.lock().await;
        e(rescan_one_folder(&app, &state, &existing).await)?;
        let folder = e(state.db.get_source(&source_id).await)?
            .ok_or_else(|| "Source not found".to_string())?;
        return Ok(Source {
            content: String::new(),
            ..folder
        });
    }
    if is_web_url(&existing.url) {
        return match ingest::extract_url(&existing.url).await {
            Ok(extracted) => e(reingest(&state, &existing, extracted).await),
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
    e(reingest(&state, &existing, extracted).await)
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
    // Deleting a folder removes its children (and their chunks) with it.
    if let Some(src) = e(state.db.get_source(&source_id).await)? {
        if src.source_type == "folder" {
            for child in e(state.db.list_sources(&src.notebook_id).await)? {
                if child.parent_id == source_id {
                    e(state.db.delete_source(&child.id).await)?;
                }
            }
        }
    }
    e(state.db.delete_source(&source_id).await)
}

// ---- Folder sources --------------------------------------------------------

/// Extensions worth ingesting from a folder scan (mirrors the frontend's
/// SUPPORTED_EXTENSIONS in src/lib/utils.ts).
const FOLDER_EXTENSIONS: &[&str] = &[
    "pdf", "txt", "text", "md", "markdown", "docx", "pptx", "xlsx", "xls", "xlsm", "ods", "csv",
    "tsv", "gdoc", "gsheet", "gslides", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff",
    "heic",
];

/// How deep a folder scan descends. Research folders are shallow; the cap only
/// guards against pathological trees.
const FOLDER_MAX_DEPTH: usize = 6;

fn folder_ingestable(path: &std::path::Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    FOLDER_EXTENSIONS.contains(&ext.as_str())
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

/// Recursively collect ingestable files under `root`, sorted by path. Skips
/// hidden entries and symlinks (cycle safety). Cloud-evicted files come back
/// as placeholders rather than being dropped.
fn scan_folder(root: &std::path::Path) -> Vec<ScanEntry> {
    fn walk(dir: &std::path::Path, depth: usize, out: &mut Vec<ScanEntry>) {
        if depth > FOLDER_MAX_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            if name.starts_with('.') {
                // iCloud Drive evicts files by replacing them with a hidden
                // `.name.icloud` stub — surface it under the real filename so
                // it upgrades in place once downloaded.
                if let Some(real) = name
                    .strip_prefix('.')
                    .and_then(|n| n.strip_suffix(".icloud"))
                    .filter(|n| !n.is_empty())
                {
                    let real_path = dir.join(real);
                    if folder_ingestable(&real_path) && !real_path.exists() {
                        out.push(ScanEntry {
                            path: real_path.to_string_lossy().to_string(),
                            mtime: file_mtime(&path),
                            placeholder: true,
                        });
                    }
                }
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                walk(&path, depth + 1, out);
            } else if folder_ingestable(&path) {
                let Ok(meta) = entry.metadata() else { continue };
                out.push(ScanEntry {
                    path: path.to_string_lossy().to_string(),
                    mtime: file_mtime(&path),
                    placeholder: is_evicted(&meta),
                });
            }
        }
    }
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Source type for a file we haven't read yet (placeholder rows), so the list
/// shows the right icon.
fn source_type_for_path(path: &str) -> &'static str {
    if ingest::is_pdf(path) {
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

/// Reconcile one folder source with the directory on disk: ingest new files,
/// re-ingest changed ones (by mtime), and drop children whose file is gone.
async fn rescan_one_folder(
    app: &AppHandle,
    state: &AppState,
    folder: &Source,
) -> anyhow::Result<FolderScan> {
    let mut scan = FolderScan::default();
    let root = std::path::Path::new(&folder.url);
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

    let children: Vec<Source> = state
        .db
        .list_sources(&folder.notebook_id)
        .await?
        .into_iter()
        .filter(|s| s.parent_id == folder.id)
        .collect();
    let on_disk = scan_folder(root);
    let by_path: HashMap<&str, &Source> = children.iter().map(|c| (c.url.as_str(), c)).collect();

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
        let _ = app.emit(
            "folder://progress",
            FolderProgress {
                done: done as u32,
                total,
                title: ingest::file_title(path),
            },
        );
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
                    store_new_source(state, &folder.notebook_id, extracted, &folder.id, mtime)
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
                    reingest(state, &existing, extracted).await?;
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
        let _ = app.emit(
            "folder://progress",
            FolderProgress {
                done: total,
                total,
                title: String::new(),
            },
        );
    }

    // Files that disappeared from disk take their sources with them.
    let disk_paths: HashSet<&str> = on_disk.iter().map(|e| e.path.as_str()).collect();
    for child in &children {
        if !disk_paths.contains(child.url.as_str()) {
            state.db.delete_source(&child.id).await?;
            scan.removed += 1;
        }
    }

    if scan.changed() {
        state.db.touch_notebook(&folder.notebook_id, now()).await?;
    }
    Ok(scan)
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
        if s.source_type == "folder" && s.url == path {
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
    let folder = Source {
        id: new_id(),
        notebook_id: notebook_id.clone(),
        title,
        source_type: "folder".to_string(),
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
    e(rescan_one_folder(&app, &state, &folder).await)?;
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
    // A manual folder add/refresh is already scanning — skip this tick rather
    // than queue behind it and ingest the same files twice.
    let Ok(_guard) = state.folder_scan_lock.try_lock() else {
        return Ok(FolderScan::default());
    };
    let mut total = FolderScan::default();
    let mut per_notebook: HashMap<String, FolderScan> = HashMap::new();
    for folder in e(state.db.all_folder_sources().await)? {
        match rescan_one_folder(&app, &state, &folder).await {
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
    for src in e(state.db.all_loose_sources().await)? {
        if src.url.is_empty() || is_web_url(&src.url) {
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
                match reingest(&state, &existing, extracted).await {
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
    let owners: Vec<(String, String, String, String)> = sources
        .iter()
        .map(|s| {
            (
                s.notebook_id.clone(),
                s.id.clone(),
                s.content.clone(),
                s.title.clone(),
            )
        })
        .collect();
    let total = owners.len() as u32;

    // Drop the old index first so the new (possibly differently-sized) vectors
    // can recreate the table cleanly.
    e(state.db.clear_all_chunks().await)?;

    let ai = state.ai.read().await;
    for (i, (notebook_id, owner_id, content, title)) in owners.iter().enumerate() {
        let _ = app.emit(
            "migrate://progress",
            MigrateProgress {
                done: i as u32,
                total,
                title: title.clone(),
            },
        );
        let chunks = ingest::chunk_text(title, content);
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
    for m in history.iter().rev().filter(|m| m.kind != "tool").take(6) {
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
- {\"action\":\"generate\",\"kind\":\"summary|faq|study_guide|briefing|timeline|problems|prd|prfaq|rfc|skill|custom\",\"prompt\":\"<extra instructions or empty>\"} — generate a document from the sources.\n\
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
        let ai = state.ai.read().await;
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
            match generate_content(state, None, notebook_id, &kind, &prompt).await {
                Ok((title, body)) => {
                    let ts = now();
                    let note = Note {
                        id: new_id(),
                        notebook_id: notebook_id.to_string(),
                        title: title.clone(),
                        content: body,
                        kind,
                        prompt,
                        created_at: ts,
                        updated_at: ts,
                    };
                    if let Err(err) = state.db.add_note(&note).await {
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
                    let extracted = ingest::extract_url(&existing.url).await?;
                    reingest(state, &existing, extracted).await
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
                .find(|m| m.role == "assistant" && m.kind != "tool")
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
                created_at: ts,
                updated_at: ts,
            };
            match state.db.add_note(&note).await {
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
        match ingest_url(state, notebook_id, url).await {
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
        created_at: now(),
    };
    e(state.db.add_message(&user_msg).await)?;

    // Tool: if the user asked to add URLs as sources, do that instead of chat.
    if let Some(reply) = try_tool_route(&app, &state, &notebook_id, &content, true).await {
        return finish_tool_reply(&app, &state, &notebook_id, reply).await;
    }

    // Retrieve relevant chunks.
    let query_vec = {
        let ai = state.ai.read().await;
        e(ai.embed_one(&content).await)?
    };
    let citations = e(state
        .db
        .search_chunks(&notebook_id, query_vec, &content, 8)
        .await)?;

    // Full source manifest (title + url) so corpus-level questions are
    // answerable regardless of which chunks the top-k search happened to
    // surface, and the model can propose new addable URLs.
    let source_manifest: Vec<(String, String)> = e(state.db.list_sources(&notebook_id).await)?
        .into_iter()
        .map(|s| (s.title, s.url))
        .collect();

    // Build prompt with short history (exclude the just-added user msg from window).
    let history = e(state.db.list_messages(&notebook_id).await)?;
    let history_turns: Vec<crate::ai::ChatTurn> = history
        .iter()
        .filter(|m| m.id != user_msg.id && m.kind != "tool")
        .map(|m| crate::ai::ChatTurn {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();
    let persona = {
        let ai = state.ai.read().await;
        rag::persona_block(&ai.config().profile)
    };
    let messages = rag::build_chat_messages(
        &history_turns,
        &content,
        &citations,
        &source_manifest,
        &extra,
        &persona,
    );

    // Stream the answer, emitting tokens to the frontend. Race against the
    // cancellation token so a Stop click aborts the request; on cancel we keep
    // whatever partial text streamed so far.
    let app_for_cb = app.clone();
    let cancel = state.begin_generation(&format!("chat:{}", window.label()));
    let partial = Arc::new(Mutex::new(String::new()));
    let partial_cb = partial.clone();
    let (answer, stats, model) = {
        let ai = state.ai.read().await;
        let model = ai.active_chat_model();
        let streamed = tokio::select! {
            out = ai.chat_stream(&messages, |tok| {
                partial_cb.lock().unwrap().push_str(tok);
                let _ = app_for_cb.emit(
                    "chat://token",
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

    let assistant_msg = Message {
        id: new_id(),
        notebook_id: notebook_id.clone(),
        role: "assistant".into(),
        content: answer,
        citations,
        kind: "chat".into(),
        created_at: now(),
    };
    e(state.db.add_message(&assistant_msg).await)?;
    e(state.db.touch_notebook(&notebook_id, now()).await)?;
    let _ = app.emit("chat://done", &assistant_msg);
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
        .filter(|m| m.id != user_msg.id && m.kind != "tool")
        .map(|m| crate::ai::ChatTurn {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();

    let cancel = state.begin_generation(&format!("chat:{}", window.label()));
    let (answer, citations, stats, model) = {
        let ai = state.ai.read().await;
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
            ) => Some(e(r)?),
            _ = cancel.cancelled() => None,
        };
        match out {
            Some((answer, citations, stats)) => (answer, citations, stats, model),
            None => ("_(Stopped.)_".to_string(), vec![], None, model),
        }
    };
    state.record_chat_stats(&model, stats);

    let assistant_msg = Message {
        id: new_id(),
        notebook_id: notebook_id.clone(),
        role: "assistant".into(),
        content: answer,
        citations,
        kind: "chat".into(),
        created_at: now(),
    };
    e(state.db.add_message(&assistant_msg).await)?;
    e(state.db.touch_notebook(&notebook_id, now()).await)?;
    let _ = app.emit("chat://done", &assistant_msg);
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
        created_at: ts,
        updated_at: ts,
    };
    e(state.db.add_note(&note).await)?;
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
        .await)
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
/// tokens stream to the UI as `artifact://token` events.
async fn generate_content(
    state: &AppState,
    app: Option<&AppHandle>,
    notebook_id: &str,
    kind: &str,
    prompt: &str,
) -> anyhow::Result<(String, String)> {
    // Known kinds use their spec (+ optional extra prompt); "custom"/unknown
    // kinds use the prompt itself as the instruction.
    let (title, instruction) = match rag::artifact_spec(kind) {
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

    let sources = state.db.list_sources(notebook_id).await?;
    if sources.is_empty() {
        anyhow::bail!("Add at least one source before generating.");
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
            let ai = state.ai.read().await;
            crate::agent::distill(&ai, &instruction, heading, &tail).await
        };
        corpus.push_str(&format!(
            "{heading}\n\n{clipped}\n…[source truncated to fit context; key passages from the \
             remainder:]\n{rescued}\n\n"
        ));
    }
    let persona = {
        let ai = state.ai.read().await;
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
        let ai = state.ai.read().await;
        let out = match app {
            Some(app) => {
                let app = app.clone();
                ai.chat_stream(messages, move |tok| {
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
) -> Result<Note, String> {
    let prompt = prompt.unwrap_or_default();
    let cancel = state.begin_generation(&format!("artifact:{}", window.label()));
    let produced = tokio::select! {
        r = generate_content(&state, Some(&app), &notebook_id, &kind, &prompt) => Some(e(r)?),
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
        created_at: ts,
        updated_at: ts,
    };
    // Audio overviews synthesize the episode before the note is saved, so a
    // failed or stopped synthesis never leaves a half-built artifact behind.
    if note.kind == "audio_overview" {
        e(synthesize_audio(&app, &note.id, &note.content, &cancel).await)?;
    }
    e(state.db.add_note(&note).await)?;
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
        r = generate_content(&state, Some(&app), &notebook_id, &kind, &prompt) => Some(e(r)?),
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
        created_at: ts,
        updated_at: ts,
    };
    let _ = app.emit("generate://done", &note);
    Ok(note)
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
    let chat_only: Vec<&Message> = history.iter().filter(|m| m.kind != "tool").collect();
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
        let ai = state.ai.read().await;
        e(ai.chat(&messages).await)?.text
    };
    let mut qs = parse_string_array(&out);
    qs.truncate(3);
    Ok(qs)
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
    )
    .await)?;
    Ok(content)
}

// ---- Periodic reports ----------------------------------------------------

#[tauri::command]
pub async fn list_report_schedules(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Vec<ReportSchedule>, String> {
    e(state.db.list_report_schedules(&notebook_id).await)
}

#[tauri::command]
pub async fn list_all_report_schedules(
    state: State<'_, AppState>,
) -> Result<Vec<ReportSchedule>, String> {
    e(state.db.all_report_schedules().await)
}

#[tauri::command]
pub async fn create_report_schedule(
    state: State<'_, AppState>,
    notebook_id: String,
    name: String,
    kind: String,
    prompt: String,
    interval_secs: i64,
) -> Result<ReportSchedule, String> {
    let schedule = ReportSchedule {
        id: new_id(),
        notebook_id,
        name: name.trim().to_string(),
        kind,
        prompt,
        interval_secs,
        enabled: true,
        last_run_at: 0,
        created_at: now(),
    };
    e(state.db.add_report_schedule(&schedule).await)?;
    Ok(schedule)
}

#[tauri::command]
pub async fn update_report_schedule(
    state: State<'_, AppState>,
    id: String,
    name: String,
    kind: String,
    prompt: String,
    interval_secs: i64,
    enabled: bool,
) -> Result<(), String> {
    e(state
        .db
        .update_report_schedule(&id, name.trim(), &kind, &prompt, interval_secs, enabled)
        .await)
}

#[tauri::command]
pub async fn delete_report_schedule(state: State<'_, AppState>, id: String) -> Result<(), String> {
    e(state.db.delete_report_schedule(&id).await)
}

/// Refresh every URL source in a notebook (best-effort), emitting progress.
async fn refresh_notebook_urls(app: &AppHandle, state: &AppState, notebook_id: &str) {
    let sources = state.db.list_sources(notebook_id).await.unwrap_or_default();
    for s in sources
        .iter()
        .filter(|s| s.source_type == "url" && !s.url.is_empty())
    {
        let _ = app.emit("report://step", format!("Refreshing: {}", s.title));
        if let Ok(Some(existing)) = state.db.get_source(&s.id).await {
            if let Ok(extracted) = ingest::extract_url(&existing.url).await {
                let _ = reingest(state, &existing, extracted).await;
            }
        }
    }
}

/// Run a report now: refresh URL sources, generate, save a timestamped note.
#[tauri::command]
pub async fn run_report(
    app: AppHandle,
    state: State<'_, AppState>,
    schedule_id: String,
) -> Result<Note, String> {
    let schedule = e(state.db.get_report_schedule(&schedule_id).await)?
        .ok_or_else(|| "Report schedule not found".to_string())?;

    refresh_notebook_urls(&app, &state, &schedule.notebook_id).await;

    let _ = app.emit("report://step", "Generating report".to_string());
    let (_t, content) = e(generate_content(
        &state,
        None,
        &schedule.notebook_id,
        &schedule.kind,
        &schedule.prompt,
    )
    .await)?;

    let ts = now();
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let note = Note {
        id: new_id(),
        notebook_id: schedule.notebook_id.clone(),
        title: format!("{} — {stamp}", schedule.name),
        content,
        kind: "report".into(),
        prompt: schedule.prompt.clone(),
        created_at: ts,
        updated_at: ts,
    };
    e(state.db.add_note(&note).await)?;
    e(state.db.set_report_last_run(&schedule_id, ts).await)?;
    e(state.db.touch_notebook(&schedule.notebook_id, ts).await)?;
    let _ = app.emit("generate://done", &note);
    Ok(note)
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
) -> Result<(), String> {
    let recents: Vec<(String, String)> = e(state.db.list_notebooks().await)?
        .into_iter()
        .map(|n| (n.id, n.title))
        .collect();
    crate::menu::fill_recents(&app, &recent.0, &recents).map_err(|err| err.to_string())
}

// ---- Home page: activity, stats, global search ----------------------------

#[tauri::command]
pub async fn list_recent_notes(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<Note>, String> {
    e(state.db.recent_notes(limit.unwrap_or(6)).await)
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
        .map(|(id, nb, title)| (id.as_str(), (nb.as_str(), title.as_str())))
        .collect();

    let mut hits = Vec::new();
    for (id, nb, title) in &meta {
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

    let mut note_hits = 0;
    for n in e(state.db.recent_notes(usize::MAX).await)? {
        if note_hits >= 4 {
            break;
        }
        if n.title.to_lowercase().contains(&q) || n.content.to_lowercase().contains(&q) {
            note_hits += 1;
            hits.push(SearchHit {
                kind: "note".into(),
                notebook_id: n.notebook_id,
                id: n.id,
                title: n.title,
                snippet: n.content.chars().take(120).collect(),
            });
        }
    }

    for (nb, c) in e(state.db.search_chunks_fts_all(query.trim(), 6).await)? {
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
    let ai = state.ai.read().await;
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
    let ai = state.ai.read().await;
    Ok(ai.config().clone())
}

#[tauri::command]
pub async fn set_ai_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: AiConfig,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&state.config_path, json).map_err(|e| e.to_string())?;
    let (mcp_enabled, mcp_port) = (config.mcp_enabled, config.mcp_port);
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
    let ai = state.ai.read().await;
    e(ai.list_models().await)
}

#[tauri::command]
pub async fn check_ollama(state: State<'_, AppState>) -> Result<bool, String> {
    let ai = state.ai.read().await;
    Ok(ai.list_models().await.is_ok())
}

#[cfg(test)]
mod tool_tests {
    use super::*;

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
