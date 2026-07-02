//! Tauri command surface — the entire IPC API the React frontend calls.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use chrono::Utc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::ai::{AiConfig, GenStats, Ollama};
use crate::db::Db;
use crate::models::{
    Message, ModelHealth, ModelStat, ModelStatus, Note, Notebook, ReportSchedule, Source,
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
    pub ai: tokio::sync::RwLock<Ollama>,
    pub config_path: PathBuf,
    pub stats_path: PathBuf,
    pub model_stats: Mutex<HashMap<String, ModelStatAcc>>,
}

impl AppState {
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

fn now() -> i64 {
    Utc::now().timestamp_millis()
}

fn new_id() -> String {
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
fn classify(source_type: &str, text: &str) -> (String, String) {
    if source_type == "url" {
        if let Some(reason) = ingest::looks_blocked(text) {
            return ("error".to_string(), reason);
        }
    }
    ("ready".to_string(), String::new())
}

async fn store_extracted(
    state: &AppState,
    notebook_id: &str,
    extracted: ingest::Extracted,
) -> anyhow::Result<Source> {
    let chunks = ingest::chunk_text(&extracted.text);
    let embeddings = {
        let ai = state.ai.read().await;
        ai.embed(&chunks).await?
    };

    let chunk_tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(i, text)| (new_id(), i as i32, text.clone()))
        .collect();

    let (status, error) = classify(&extracted.source_type, &extracted.text);
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

#[tauri::command]
pub async fn add_source_file(
    state: State<'_, AppState>,
    notebook_id: String,
    path: String,
) -> Result<Source, String> {
    let extracted = if ingest::is_image(&path) {
        e(extract_image(&state, &path).await)?
    } else if ingest::is_pdf(&path) {
        // Try fast text extraction; fall back to per-page OCR for scanned PDFs.
        match ingest::extract_file(&path) {
            Ok(ex) => ex,
            Err(text_err) => e(extract_pdf_ocr(&state, &path)
                .await
                .map_err(|ocr_err| anyhow::anyhow!("{text_err} OCR fallback failed: {ocr_err}")))?,
        }
    } else {
        e(ingest::extract_file(&path))?
    };
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
async fn ingest_url(state: &AppState, notebook_id: &str, url: &str) -> anyhow::Result<Source> {
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
    let chunks = ingest::chunk_text(&extracted.text);
    let embeddings = {
        let ai = state.ai.read().await;
        ai.embed(&chunks).await?
    };
    let chunk_tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(i, text)| (new_id(), i as i32, text.clone()))
        .collect();

    let (status, error) = classify(&existing.source_type, &extracted.text);
    let updated = Source {
        id: existing.id.clone(),
        notebook_id: existing.notebook_id.clone(),
        title: extracted.title,
        source_type: existing.source_type.clone(),
        url: extracted.url,
        content: extracted.text.clone(),
        char_count: extracted.text.chars().count() as i64,
        chunk_count: chunk_tuples.len() as i64,
        created_at: existing.created_at,
        status,
        error,
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

#[tauri::command]
pub async fn refresh_source_url(
    state: State<'_, AppState>,
    source_id: String,
) -> Result<Source, String> {
    let existing =
        e(state.db.get_source(&source_id).await)?.ok_or_else(|| "Source not found".to_string())?;
    if existing.url.is_empty() {
        return Err("This source has no URL to refresh".into());
    }
    match ingest::extract_url(&existing.url).await {
        Ok(extracted) => e(reingest(&state, &existing, extracted).await),
        Err(err) => e(mark_source_failed(&state, &existing, err.to_string()).await),
    }
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
    e(state.db.delete_source(&source_id).await)
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
        let chunks = ingest::chunk_text(content);
        if chunks.is_empty() {
            continue;
        }
        let embeddings = e(ai.embed(&chunks).await)?;
        let tuples: Vec<(String, i32, String)> = chunks
            .iter()
            .enumerate()
            .map(|(j, text)| (new_id(), j as i32, text.clone()))
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
        let t = token.trim_matches(|c: char| "()[]{}<>,\"'`|".contains(c));
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
    let has_kw = ["add", "import", "ingest", "save", "include", "load", "grab", "attach", "pull in"]
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
        created_at: now(),
    };
    e(state.db.add_message(&msg).await)?;
    e(state.db.touch_notebook(notebook_id, now()).await)?;
    let _ = app.emit("chat://done", &msg);
    Ok(msg)
}

/// If the message is an "add these URLs" request, ingest them and return a
/// summary reply. Returns None to fall through to the normal chat flow.
async fn try_add_url_sources(
    app: &AppHandle,
    state: &AppState,
    notebook_id: &str,
    content: &str,
) -> Option<String> {
    let urls = extract_urls(content);
    if urls.is_empty() || !wants_add_sources(content, &urls) {
        return None;
    }

    let mut added: Vec<Source> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    for url in &urls {
        let _ = app.emit("chat://step", StepEvent { label: format!("Adding source: {}", host_of(url)) });
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
        out.push_str(&format!(
            "{} couldn't be added:\n",
            failed.len()
        ));
        for (url, err) in &failed {
            out.push_str(&format!("- {} — {}\n", host_of(url), err));
        }
    }
    Some(out)
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
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
        created_at: now(),
    };
    e(state.db.add_message(&user_msg).await)?;

    // Tool: if the user asked to add URLs as sources, do that instead of chat.
    if let Some(reply) = try_add_url_sources(&app, &state, &notebook_id, &content).await {
        return finish_tool_reply(&app, &state, &notebook_id, reply).await;
    }

    // Retrieve relevant chunks.
    let query_vec = {
        let ai = state.ai.read().await;
        e(ai.embed_one(&content).await)?
    };
    let citations = e(state.db.search_chunks(&notebook_id, query_vec, 8).await)?;

    // Build prompt with short history (exclude the just-added user msg from window).
    let history = e(state.db.list_messages(&notebook_id).await)?;
    let history_turns: Vec<crate::ai::ChatTurn> = history
        .iter()
        .filter(|m| m.id != user_msg.id)
        .map(|m| crate::ai::ChatTurn {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();
    let messages = rag::build_chat_messages(&history_turns, &content, &citations, &extra);

    // Stream the answer, emitting tokens to the frontend.
    let app_for_cb = app.clone();
    let (answer, stats, model) = {
        let ai = state.ai.read().await;
        let out = e(ai
            .chat_stream(&messages, |tok| {
                let _ = app_for_cb.emit(
                    "chat://token",
                    TokenEvent {
                        content: tok.to_string(),
                    },
                );
            })
            .await)?;
        (out.text, out.stats, ai.config().chat_model.clone())
    };
    state.record_chat_stats(&model, stats);

    let assistant_msg = Message {
        id: new_id(),
        notebook_id: notebook_id.clone(),
        role: "assistant".into(),
        content: answer,
        citations,
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
        created_at: now(),
    };
    e(state.db.add_message(&user_msg).await)?;

    // Tool: add-URL requests are handled the same in deep-research mode.
    if let Some(reply) = try_add_url_sources(&app, &state, &notebook_id, &content).await {
        return finish_tool_reply(&app, &state, &notebook_id, reply).await;
    }

    let history = e(state.db.list_messages(&notebook_id).await)?;
    let history_turns: Vec<crate::ai::ChatTurn> = history
        .iter()
        .filter(|m| m.id != user_msg.id)
        .map(|m| crate::ai::ChatTurn {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();

    let (answer, citations, stats, model) = {
        let ai = state.ai.read().await;
        let (answer, citations, stats) =
            e(crate::agent::run(
                &app,
                &state.db,
                &ai,
                &notebook_id,
                &content,
                &history_turns,
                &extra,
            )
            .await)?;
        (answer, citations, stats, ai.config().chat_model.clone())
    };
    state.record_chat_stats(&model, stats);

    let assistant_msg = Message {
        id: new_id(),
        notebook_id: notebook_id.clone(),
        role: "assistant".into(),
        content: answer,
        citations,
        created_at: now(),
    };
    e(state.db.add_message(&assistant_msg).await)?;
    e(state.db.touch_notebook(&notebook_id, now()).await)?;
    let _ = app.emit("chat://done", &assistant_msg);
    Ok(assistant_msg)
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
pub async fn delete_note(state: State<'_, AppState>, id: String) -> Result<(), String> {
    e(state.db.delete_note(&id).await)
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
/// a notebook's source text. Returns (title, content).
async fn generate_content(
    state: &AppState,
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
    let mut corpus = String::new();
    for s in &sources {
        let full = state.db.source_content(&s.id).await?;
        corpus.push_str(&format!("## {}\n\n{}\n\n", s.title, full));
    }
    let messages = rag::build_artifact_messages(&instruction, &corpus);
    let (content, stats, model) = {
        let ai = state.ai.read().await;
        let out = ai.chat(&messages).await?;
        (out.text, out.stats, ai.config().chat_model.clone())
    };
    state.record_chat_stats(&model, stats);
    Ok((title.to_string(), content))
}

#[tauri::command]
pub async fn generate_artifact(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: String,
    kind: String,
    prompt: Option<String>,
) -> Result<Note, String> {
    let prompt = prompt.unwrap_or_default();
    let (title, content) = e(generate_content(&state, &notebook_id, &kind, &prompt).await)?;

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
    e(state.db.add_note(&note).await)?;
    let _ = app.emit("generate://done", &note);
    Ok(note)
}

#[tauri::command]
pub async fn rebuild_note(
    app: AppHandle,
    state: State<'_, AppState>,
    note_id: String,
    notebook_id: String,
    kind: String,
    prompt: String,
) -> Result<Note, String> {
    let (title, content) = e(generate_content(&state, &notebook_id, &kind, &prompt).await)?;
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
    let start = history.len().saturating_sub(4);
    let mut convo = String::new();
    for m in &history[start..] {
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

// ---- Settings / health ---------------------------------------------------

/// Verify the configured chat + embedding models are installed and (for embed)
/// actually responding. Used to surface a clear status instead of a hang.
#[tauri::command]
pub async fn check_models(state: State<'_, AppState>) -> Result<ModelHealth, String> {
    let ai = state.ai.read().await;
    let cfg = ai.config().clone();
    let norm = |m: &str| m.trim_end_matches(":latest").to_string();

    let installed = match ai.list_models().await {
        Ok(list) => list,
        Err(_) => {
            // Ollama unreachable — report both as unknown.
            let unknown = |name: String| ModelStatus {
                name,
                installed: false,
                working: false,
                detail: "Ollama not reachable".into(),
            };
            return Ok(ModelHealth {
                reachable: false,
                chat: unknown(cfg.chat_model.clone()),
                embed: unknown(cfg.embed_model.clone()),
            });
        }
    };
    let has = |m: &str| installed.iter().any(|x| norm(x) == norm(m));

    let chat_installed = has(&cfg.chat_model);
    let chat = ModelStatus {
        name: cfg.chat_model.clone(),
        installed: chat_installed,
        working: chat_installed,
        detail: if chat_installed {
            "Installed".into()
        } else {
            format!("Not installed — run `ollama pull {}`", cfg.chat_model)
        },
    };

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
    let embed = ModelStatus {
        name: cfg.embed_model.clone(),
        installed: embed_installed,
        working: embed_working,
        detail: embed_detail,
    };

    Ok(ModelHealth {
        reachable: true,
        chat,
        embed,
    })
}

#[tauri::command]
pub async fn get_ai_config(state: State<'_, AppState>) -> Result<AiConfig, String> {
    let ai = state.ai.read().await;
    Ok(ai.config().clone())
}

#[tauri::command]
pub async fn set_ai_config(state: State<'_, AppState>, config: AiConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&state.config_path, json).map_err(|e| e.to_string())?;
    let mut ai = state.ai.write().await;
    *ai = Ollama::new(config);
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
