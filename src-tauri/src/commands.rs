//! Tauri command surface — the entire IPC API the React frontend calls.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::ai::{AiConfig, Ollama};
use crate::db::Db;
use crate::models::{Message, Note, Notebook, Source};
use crate::{ingest, rag};

pub struct AppState {
    pub db: Arc<Db>,
    pub ai: tokio::sync::RwLock<Ollama>,
    pub config_path: PathBuf,
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
    };
    state.db.insert_source(&source, &chunk_tuples, &embeddings).await?;
    state.db.touch_notebook(notebook_id, now()).await?;

    // Don't ship the full content back in the list payload.
    Ok(Source { content: String::new(), ..source })
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
    let extracted = e(ingest::extract_url(&url).await)?;
    e(store_extracted(&state, &notebook_id, extracted).await)
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
    };
    state.db.replace_source(&updated, &chunk_tuples, &embeddings).await?;
    state.db.touch_notebook(&existing.notebook_id, now()).await?;
    Ok(Source { content: String::new(), ..updated })
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
    let extracted = e(ingest::extract_url(&existing.url).await)?;
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
    e(state.db.delete_source(&source_id).await)
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
    let answer = {
        let ai = state.ai.read().await;
        e(ai
            .chat_stream(&messages, |tok| {
                let _ = app_for_cb.emit("chat://token", TokenEvent { content: tok.to_string() });
            })
            .await)?
    };

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

    let (answer, citations) = {
        let ai = state.ai.read().await;
        e(crate::agent::run(&app, &state.db, &ai, &notebook_id, &content, &history_turns).await)?
    };

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
        title: if title.trim().is_empty() { "Untitled note".into() } else { title.trim().to_string() },
        content,
        kind: "note".into(),
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
    e(state.db.update_note(&id, title.trim(), &content, now()).await)
}

#[tauri::command]
pub async fn delete_note(state: State<'_, AppState>, id: String) -> Result<(), String> {
    e(state.db.delete_note(&id).await)
}

#[tauri::command]
pub async fn generate_artifact(
    state: State<'_, AppState>,
    notebook_id: String,
    kind: String,
) -> Result<Note, String> {
    let (title, instruction) =
        rag::artifact_spec(&kind).ok_or_else(|| format!("Unknown artifact kind: {kind}"))?;

    // Concatenate all source content for this notebook.
    let sources = e(state.db.list_sources(&notebook_id).await)?;
    if sources.is_empty() {
        return Err("Add at least one source before generating.".into());
    }
    let mut corpus = String::new();
    for s in &sources {
        let full = e(state.db.source_content(&s.id).await)?;
        corpus.push_str(&format!("## {}\n\n{}\n\n", s.title, full));
    }

    let messages = rag::build_artifact_messages(instruction, &corpus);
    let content = {
        let ai = state.ai.read().await;
        e(ai.chat(&messages).await)?
    };

    let ts = now();
    let note = Note {
        id: new_id(),
        notebook_id,
        title: title.to_string(),
        content,
        kind,
        created_at: ts,
        updated_at: ts,
    };
    e(state.db.add_note(&note).await)?;
    Ok(note)
}

// ---- Settings / health ---------------------------------------------------

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
