//! Agentic (RLM-inspired) retrieval loop. Instead of one-shot top-k retrieval,
//! the model plans a sequence of searches/reads over the notebook, accumulates
//! evidence, then writes a single grounded answer. Progress is streamed to the
//! UI via `chat://step` events and the final answer via `chat://token`.

use std::collections::HashSet;

use anyhow::Result;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::ai::{ChatTurn, Ollama};
use crate::db::Db;
use crate::models::Citation;
use crate::rag;

const MAX_STEPS: usize = 5;
const SEARCH_K: usize = 5;

#[derive(Serialize, Clone)]
struct StepEvent {
    label: String,
}

#[derive(Serialize, Clone)]
struct TokenEvent {
    content: String,
}

enum Action {
    Search(String),
    Read(String),
    Stop,
}

/// Run the loop and return the final answer plus the citations actually gathered.
pub async fn run(
    app: &AppHandle,
    db: &Db,
    ollama: &Ollama,
    notebook_id: &str,
    question: &str,
    history: &[ChatTurn],
    extra_system: &str,
) -> Result<(String, Vec<Citation>, Option<crate::ai::GenStats>)> {
    let sources = db.list_sources(notebook_id).await?;
    let source_list = sources
        .iter()
        .map(|s| format!("- {} (id: {}, {} chunks)", s.title, s.id, s.chunk_count))
        .collect::<Vec<_>>()
        .join("\n");

    let mut gathered: Vec<Citation> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut transcript = String::new();

    for _ in 0..MAX_STEPS {
        let messages =
            rag::build_agent_decision(question, &source_list, &transcript, gathered.len());
        let raw = ollama.chat(&messages).await?.text;
        match parse_action(&raw) {
            Some(Action::Search(query)) => {
                emit_step(app, format!("Searching: {query}"));
                let qvec = ollama.embed_one(&query).await?;
                let hits = db.search_chunks(notebook_id, qvec, SEARCH_K).await?;
                transcript.push_str(&format!("SEARCH \"{query}\":\n"));
                for h in &hits {
                    if seen.insert(h.chunk_id.clone()) {
                        gathered.push(h.clone());
                    }
                    transcript.push_str(&format!(
                        "  - ({}) {}\n",
                        h.source_title,
                        truncate(&h.snippet, 180)
                    ));
                }
                transcript.push('\n');
            }
            Some(Action::Read(source_id)) => {
                let title = sources
                    .iter()
                    .find(|s| s.id == source_id)
                    .map(|s| s.title.clone())
                    .unwrap_or_else(|| "source".into());
                emit_step(app, format!("Reading: {title}"));
                let content = db.source_content(&source_id).await?;
                transcript.push_str(&format!(
                    "READ \"{title}\":\n{}\n\n",
                    truncate(&content, 1500)
                ));
            }
            Some(Action::Stop) | None => break,
        }
    }

    // Safety net: if the planner never searched, fall back to a direct query so
    // the final answer is still grounded.
    if gathered.is_empty() {
        emit_step(app, "Searching".into());
        let qvec = ollama.embed_one(question).await?;
        gathered = db.search_chunks(notebook_id, qvec, 8).await?;
    }

    emit_step(app, "Writing answer".into());
    let messages = rag::build_chat_messages(history, question, &gathered, extra_system);
    let app_cb = app.clone();
    let outcome = ollama
        .chat_stream(&messages, |tok| {
            let _ = app_cb.emit(
                "chat://token",
                TokenEvent {
                    content: tok.to_string(),
                },
            );
        })
        .await?;

    Ok((outcome.text, gathered, outcome.stats))
}

fn emit_step(app: &AppHandle, label: String) {
    let _ = app.emit("chat://step", StepEvent { label });
}

/// Parse the planner's JSON action, tolerating surrounding prose/code fences.
fn parse_action(raw: &str) -> Option<Action> {
    let json = extract_json(raw)?;
    let value: serde_json::Value = serde_json::from_str(&json).ok()?;
    match value.get("action").and_then(|a| a.as_str())? {
        "search" => {
            let q = value.get("query").and_then(|q| q.as_str())?.trim();
            if q.is_empty() {
                None
            } else {
                Some(Action::Search(q.to_string()))
            }
        }
        "read" => value
            .get("sourceId")
            .and_then(|s| s.as_str())
            .map(|s| Action::Read(s.to_string())),
        "answer" => Some(Action::Stop),
        _ => None,
    }
}

/// Extract the first balanced `{...}` object from arbitrary model output.
fn extract_json(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in raw[start..].char_indices() {
        match c {
            '"' if !escaped => in_str = !in_str,
            '\\' if in_str => {
                escaped = !escaped;
                continue;
            }
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(raw[start..start + i + 1].to_string());
                }
            }
            _ => {}
        }
        escaped = false;
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_fenced_prose() {
        let raw = "Plan:\n```json\n{\"action\":\"search\",\"query\":\"x\"}\n```";
        assert_eq!(
            extract_json(raw).as_deref(),
            Some("{\"action\":\"search\",\"query\":\"x\"}")
        );
    }

    #[test]
    fn extracts_json_with_braces_in_strings() {
        let raw = "{\"action\":\"search\",\"query\":\"what is {x}?\",\"m\":{\"k\":1}}";
        assert_eq!(extract_json(raw).as_deref(), Some(raw));
    }

    #[test]
    fn returns_none_without_json() {
        assert!(extract_json("no json here").is_none());
    }

    #[test]
    fn parses_each_action() {
        assert!(matches!(
            parse_action("{\"action\":\"search\",\"query\":\"q\"}"),
            Some(Action::Search(q)) if q == "q"
        ));
        assert!(matches!(
            parse_action("```{\"action\":\"read\",\"sourceId\":\"abc\"}```"),
            Some(Action::Read(id)) if id == "abc"
        ));
        assert!(matches!(
            parse_action("{\"action\":\"answer\"}"),
            Some(Action::Stop)
        ));
        assert!(parse_action("garbage").is_none());
        assert!(parse_action("{\"action\":\"search\",\"query\":\"\"}").is_none());
    }
}
