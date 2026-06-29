//! Tauri command surface — the entire IPC API the React frontend calls.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::ai::{AiConfig, GenStats, Ollama};
use crate::db::Db;
use crate::models::{Message, ModelHealth, ModelStat, ModelStatus, Note, Notebook, Source};
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
    let title = if title.trim().is_empty() { "Untitled notebook".into() } else { title.trim().to_string() };
    let nb = Notebook { id: new_id(), title, created_at: ts, updated_at: ts, source_count: 0 };
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
    state.db.insert_source(&source, &chunk_tuples, &embeddings).await?;
    state.db.touch_notebook(notebook_id, now()).await?;

    // Don't ship the full content back in the list payload.
    Ok(Source { content: String::new(), ..source })
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

#[tauri::command]
pub async fn add_source_file(
    state: State<'_, AppState>,
    notebook_id: String,
    path: String,
) -> Result<Source, String> {
    let extracted = e(ingest::extract_file(&path))?;
    e(store_extracted(&state, &notebook_id, extracted).await)
}

#[tauri::command]
pub async fn add_source_url(
    state: State<'_, AppState>,
    notebook_id: String,
    url: String,
) -> Result<Source, String> {
    // Hard failures (network / HTTP / empty) still produce a source row marked
    // as errored, so the user sees it and can retry rather than it vanishing.
    match ingest::extract_url(&url).await {
        Ok(extracted) => e(store_extracted(&state, &notebook_id, extracted).await),
        Err(err) => e(store_failed_url(&state, &notebook_id, url.trim(), err.to_string()).await),
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
    state.db.replace_source(&updated, &chunk_tuples, &embeddings).await?;
    state.db.touch_notebook(&existing.notebook_id, now()).await?;
    Ok(Source { content: String::new(), ..updated })
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
    state.db.touch_notebook(&existing.notebook_id, now()).await?;
    Ok(failed)
}

#[tauri::command]
pub async fn update_source_text(
    state: State<'_, AppState>,
    source_id: String,
    title: String,
    text: String,
) -> Result<Source, String> {
    let existing = e(state.db.get_source(&source_id).await)?
        .ok_or_else(|| "Source not found".to_string())?;
    let extracted = e(ingest::extract_pasted(&title, &text))?;
    e(reingest(&state, &existing, extracted).await)
}

#[tauri::command]
pub async fn refresh_source_url(
    state: State<'_, AppState>,
    source_id: String,
) -> Result<Source, String> {
    let existing = e(state.db.get_source(&source_id).await)?
        .ok_or_else(|| "Source not found".to_string())?;
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
    let notes = e(state.db.all_notes().await)?;
    // Notes with content also live in the chunk index.
    let owners: Vec<(String, String, String, String)> = sources
        .iter()
        .map(|s| (s.notebook_id.clone(), s.id.clone(), s.content.clone(), s.title.clone()))
        .chain(
            notes
                .iter()
                .map(|n| (n.notebook_id.clone(), n.id.clone(), n.content.clone(), format!("Note: {}", n.title))),
        )
        .collect();
    let total = owners.len() as u32;

    // Drop the old index first so the new (possibly differently-sized) vectors
    // can recreate the table cleanly.
    e(state.db.clear_all_chunks().await)?;

    let ai = state.ai.read().await;
    for (i, (notebook_id, owner_id, content, title)) in owners.iter().enumerate() {
        let _ = app.emit(
            "migrate://progress",
            MigrateProgress { done: i as u32, total, title: title.clone() },
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
        e(state.db.add_chunks(notebook_id, owner_id, &tuples, &embeddings).await)?;
    }

    let _ = app.emit(
        "migrate://progress",
        MigrateProgress { done: total, total, title: "Done".into() },
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

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: String,
    content: String,
) -> Result<Message, String> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err("Message is empty".into());
    }

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
        .map(|m| crate::ai::ChatTurn { role: m.role.clone(), content: m.content.clone() })
        .collect();
    let messages = rag::build_chat_messages(&history_turns, &content, &citations);

    // Stream the answer, emitting tokens to the frontend.
    let app_for_cb = app.clone();
    let (answer, stats, model) = {
        let ai = state.ai.read().await;
        let out = e(ai
            .chat_stream(&messages, |tok| {
                let _ = app_for_cb.emit("chat://token", TokenEvent { content: tok.to_string() });
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
) -> Result<Message, String> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err("Message is empty".into());
    }

    let user_msg = Message {
        id: new_id(),
        notebook_id: notebook_id.clone(),
        role: "user".into(),
        content: content.clone(),
        citations: vec![],
        created_at: now(),
    };
    e(state.db.add_message(&user_msg).await)?;

    let history = e(state.db.list_messages(&notebook_id).await)?;
    let history_turns: Vec<crate::ai::ChatTurn> = history
        .iter()
        .filter(|m| m.id != user_msg.id)
        .map(|m| crate::ai::ChatTurn { role: m.role.clone(), content: m.content.clone() })
        .collect();

    let (answer, citations, stats, model) = {
        let ai = state.ai.read().await;
        let (answer, citations, stats) =
            e(crate::agent::run(&app, &state.db, &ai, &notebook_id, &content, &history_turns).await)?;
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

/// Embed a note's content into the chunk index so chat retrieval can cite it.
/// Best-effort: a failure here doesn't block the note operation.
async fn index_note(state: &AppState, notebook_id: &str, note_id: &str, content: &str) {
    let chunks = ingest::chunk_text(content);
    if chunks.is_empty() {
        let _ = state.db.set_chunks(notebook_id, note_id, &[], &[]).await;
        return;
    }
    let embeddings = {
        let ai = state.ai.read().await;
        match ai.embed(&chunks).await {
            Ok(e) => e,
            Err(e) => {
                eprintln!("note index: embedding failed: {e}");
                return;
            }
        }
    };
    let tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(i, text)| (new_id(), i as i32, text.clone()))
        .collect();
    if let Err(e) = state.db.set_chunks(notebook_id, note_id, &tuples, &embeddings).await {
        eprintln!("note index: write failed: {e}");
    }
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
        title: if title.trim().is_empty() { "Untitled note".into() } else { title.trim().to_string() },
        content,
        kind: "note".into(),
        prompt: String::new(),
        created_at: ts,
        updated_at: ts,
    };
    e(state.db.add_note(&note).await)?;
    index_note(&state, &note.notebook_id, &note.id, &note.content).await;
    Ok(note)
}

#[tauri::command]
pub async fn update_note(
    state: State<'_, AppState>,
    id: String,
    notebook_id: String,
    title: String,
    content: String,
) -> Result<(), String> {
    e(state.db.update_note(&id, title.trim(), &content, now()).await)?;
    index_note(&state, &notebook_id, &id, &content).await;
    Ok(())
}

#[tauri::command]
pub async fn delete_note(state: State<'_, AppState>, id: String) -> Result<(), String> {
    e(state.db.delete_note(&id).await)?;
    e(state.db.delete_chunks_for(&id).await)
}

/// Generate artifact content for a kind (+ optional custom prompt) over all of
/// a notebook's source text. Returns (title, content).
async fn generate_content(
    state: &AppState,
    notebook_id: &str,
    kind: &str,
    prompt: &str,
) -> anyhow::Result<(String, String)> {
    let (title, base) = rag::artifact_spec(kind)
        .ok_or_else(|| anyhow::anyhow!("Unknown artifact kind: {kind}"))?;

    let sources = state.db.list_sources(notebook_id).await?;
    if sources.is_empty() {
        anyhow::bail!("Add at least one source before generating.");
    }
    let mut corpus = String::new();
    for s in &sources {
        let full = state.db.source_content(&s.id).await?;
        corpus.push_str(&format!("## {}\n\n{}\n\n", s.title, full));
    }

    let instruction = if prompt.trim().is_empty() {
        base.to_string()
    } else {
        format!("{base}\n\nAdditional instructions from the user (follow these):\n{}", prompt.trim())
    };
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
    index_note(&state, &note.notebook_id, &note.id, &note.content).await;
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
    index_note(&state, &notebook_id, &note_id, &content).await;

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
        (false, format!("Not installed — run `ollama pull {}`", cfg.embed_model))
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

    Ok(ModelHealth { reachable: true, chat, embed })
}

#[tauri::command]
pub async fn get_ai_config(state: State<'_, AppState>) -> Result<AiConfig, String> {
    let ai = state.ai.read().await;
    Ok(ai.config().clone())
}

#[tauri::command]
pub async fn set_ai_config(
    state: State<'_, AppState>,
    config: AiConfig,
) -> Result<(), String> {
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
