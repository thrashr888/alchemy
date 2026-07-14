//! Agentic (RLM-inspired) retrieval loop. Instead of one-shot top-k retrieval,
//! the model plans a sequence of searches/reads over the notebook, accumulates
//! evidence, then writes a single grounded answer. Progress is streamed to the
//! UI via `chat://step` events and the final answer via `chat://token`.

use std::collections::HashSet;

use anyhow::Result;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::ai::{Ai, ChatTurn};
use crate::db::Db;
use crate::models::Citation;
use crate::rag;

const MAX_STEPS: usize = 5;
/// Results kept per search step, after reranking.
const SEARCH_K: usize = 5;
/// Hybrid-retrieval pool handed to the reranker.
const SEARCH_POOL: usize = 20;

/// Total budget (chars, ~4 chars/token) for `read` actions across the whole
/// loop. This is the input handed to the distillation sub-call, so it is
/// bounded by what one model call can absorb: local models have small
/// contexts; gateway models can take far more. Also used by artifact
/// generation to cap the input of its truncation-rescue distills.
pub(crate) const READ_CHARS_LOCAL: usize = 12_000;
pub(crate) const READ_CHARS_GATEWAY: usize = 120_000;
/// Fallback excerpt size when the distiller fails — a raw head beats nothing.
const READ_GIST_CHARS: usize = 1_500;
/// Cap on a distilled read (the prompt asks for ~500 words; this guards
/// runaway outputs). Distillates are re-sent in the planner transcript and
/// persisted as the citation snippet, so they must stay small.
const DISTILL_MAX_CHARS: usize = 4_000;

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
/// `source_ids` restricts the loop to those sources; None means all.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    app: &AppHandle,
    db: &Db,
    ollama: &Ai,
    notebook_id: &str,
    question: &str,
    history: &[ChatTurn],
    extra_system: &str,
    source_ids: Option<&[String]>,
) -> Result<(String, Vec<Citation>, Option<crate::ai::GenStats>)> {
    let mut read_remaining = if ollama.config().is_gateway() {
        READ_CHARS_GATEWAY
    } else {
        READ_CHARS_LOCAL
    };
    // Deselected sources are invisible to the planner: they never appear in
    // the source list (so no reads) and are filtered out of every search.
    let mut sources = db.list_sources(notebook_id).await?;
    if let Some(ids) = source_ids {
        sources.retain(|s| ids.contains(&s.id));
    }
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
                let mut hits = db
                    .search_chunks(notebook_id, qvec, &query, SEARCH_POOL, source_ids)
                    .await?;
                // Retrieve wide, then let the model pick the few passages that
                // actually answer — recall from hybrid search, precision from
                // the rerank.
                if hits.len() > SEARCH_K {
                    emit_step(app, "Ranking results".into());
                    hits = rerank(ollama, &query, hits).await;
                }
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
                // Later reads always get at least the gist even with the
                // budget spent, so a read step is never a silent no-op.
                let budget = read_remaining.max(READ_GIST_CHARS);
                let content = truncate(&db.source_content(&source_id).await?, budget);
                read_remaining = read_remaining.saturating_sub(content.chars().count());
                // RLM-style sub-read: a separate model call distills the
                // document against the question into verbatim quotes, so a
                // read contributes evidence — not bulk — to every later
                // prompt. One distillate serves the planner transcript, the
                // writer excerpt, and the persisted citation alike.
                emit_step(app, format!("Distilling: {title}"));
                let evidence = distill(ollama, question, &title, &content).await;
                transcript.push_str(&format!("READ \"{title}\":\n{evidence}\n\n"));
                let read_id = format!("read:{source_id}");
                if seen.insert(read_id.clone()) {
                    gathered.push(Citation {
                        chunk_id: read_id,
                        source_id: source_id.clone(),
                        source_title: title,
                        note_id: String::new(),
                        ordinal: 0,
                        snippet: evidence,
                        distance: 0.0,
                    });
                }
            }
            Some(Action::Stop) | None => break,
        }
    }

    // Safety net: if the planner never searched, fall back to a direct query so
    // the final answer is still grounded.
    if gathered.is_empty() {
        emit_step(app, "Searching".into());
        let qvec = ollama.embed_one(question).await?;
        gathered = db
            .search_chunks(notebook_id, qvec, question, 8, source_ids)
            .await?;
    }

    emit_step(app, "Writing answer".into());
    let source_manifest: Vec<(String, String)> = sources
        .iter()
        .map(|s| (s.title.clone(), s.url.clone()))
        .collect();
    let persona = rag::persona_block(&ollama.config().profile);
    let messages = rag::build_chat_messages(
        history,
        question,
        &gathered,
        &source_manifest,
        extra_system,
        &persona,
    );
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

/// Rerank a wide retrieval pool down to the SEARCH_K most relevant hits via
/// one model call. Any failure (model error, unparseable output, bogus
/// indices) falls back to the fusion order.
pub(crate) async fn rerank(ai: &Ai, question: &str, hits: Vec<Citation>) -> Vec<Citation> {
    let top = |hits: Vec<Citation>| hits.into_iter().take(SEARCH_K).collect::<Vec<_>>();

    let snippets: Vec<(String, String)> = hits
        .iter()
        .map(|h| (h.source_title.clone(), truncate(&h.snippet, 300)))
        .collect();
    let messages = rag::build_rerank_messages(question, &snippets, SEARCH_K);
    let raw = match ai.chat(&messages).await {
        Ok(out) => out.text,
        Err(_) => return top(hits),
    };
    let Some(json) = extract_json(&raw) else {
        return top(hits);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return top(hits);
    };
    let indices: Vec<usize> = value
        .get("keep")
        .and_then(|k| k.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_u64())
                .map(|x| x as usize)
                .collect()
        })
        .unwrap_or_default();

    let mut used = HashSet::new();
    let picked: Vec<Citation> = indices
        .into_iter()
        .take(SEARCH_K)
        .filter(|&i| i < hits.len() && used.insert(i))
        .map(|i| hits[i].clone())
        .collect();
    if picked.is_empty() {
        top(hits)
    } else {
        picked
    }
}

/// Distill one document against the question into verbatim quotes via a
/// sub-call. On failure (model error, empty output) fall back to a raw head
/// excerpt — a degraded read still beats an empty one. Shared with artifact
/// generation, which distills content that won't fit its corpus budget.
pub(crate) async fn distill(ai: &Ai, question: &str, title: &str, content: &str) -> String {
    let messages = rag::build_distill_messages(question, title, content);
    match ai.chat(&messages).await {
        Ok(out) if !out.text.trim().is_empty() => truncate(out.text.trim(), DISTILL_MAX_CHARS),
        _ => truncate(content, READ_GIST_CHARS),
    }
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
pub(crate) fn extract_json(raw: &str) -> Option<String> {
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
