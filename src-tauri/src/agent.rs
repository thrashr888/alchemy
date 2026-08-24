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

/// Which model role runs the loop's internal calls — planning, reranking,
/// and per-source distillation.
///
/// Small by default. Every one of these produces short structured output
/// (`SEARCH "..."`, a list of passage numbers, a ~500-word evidence note),
/// which is what the Small role exists for; paying full chat-model latency
/// up to MAX_STEPS times before the user sees a single answer token is the
/// waste that dominates deep research's time to first token. `chat_role`
/// falls through to the chat engine when no small model is configured, so
/// this is a no-op for anyone who has not opted into a small tier.
///
/// The final answer always streams from the chat model — the quality that
/// matters most is the one the user reads.
///
/// `ALCHEMY_PLANNER_ROLE=chat` restores the old behaviour.
///
/// Measured 2026-08-23, muse-glimmer:30b-mlx (chat) vs
/// digitsflow/bonsai-8b (small), 4 probes each:
///
/// | call    | role  | quality      | avg latency |
/// |---------|-------|--------------|-------------|
/// | planner | Small | 4/4 on-target|     1,475ms |
/// | planner | Chat  | 4/4 on-target|    24,214ms |
/// | distill | Small | 4/4 facts kept|      810ms |
/// | distill | Chat  | 4/4 facts kept|   36,428ms |
///
/// No quality difference on these probes, 16-45x the speed. Re-run with
/// `cargo test --lib eval_agent_planner_roles eval_agent_distill_roles --
/// --ignored --nocapture` after changing the prompts or the default pair.
/// Both probe sets are small and cover the first action / fact retention —
/// they show no regression, which is not the same as proving parity.
pub(crate) fn loop_role() -> crate::inference::Role {
    match std::env::var("ALCHEMY_PLANNER_ROLE").as_deref() {
        Ok("chat") => crate::inference::Role::Chat,
        _ => crate::inference::Role::Small,
    }
}
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

#[derive(Debug)]
pub(crate) enum Action {
    Search(String),
    /// One planner step can read several sources; they distill in parallel.
    Read(Vec<String>),
    Stop,
}

/// Cap one read action's batch — the planner shouldn't drain the whole
/// notebook in a step, and local model servers queue past a few in flight.
const READ_BATCH_CAP: usize = 4;

/// Distills in flight at once. Gateways parallelize; a local single-GPU
/// server just queues, so past ~3 there's nothing left to win.
const DISTILL_CONCURRENCY: usize = 3;

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
    // Timed by the caller so deep research reports the same
    // send-to-first-token wait the direct chat path does — the loop's tool
    // rounds run before this stream, and that delay is part of it.
    ttft: &crate::commands::TtftClock,
    // Where the pre-answer time went, for the timing trace.
    phases: &crate::commands::AgentPhases,
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
        let planned = std::time::Instant::now();
        let raw = ollama.chat_role(loop_role(), &messages).await?.text;
        phases.planner(planned.elapsed().as_millis() as u64);
        match parse_action(&raw) {
            Some(Action::Search(query)) => {
                let searched = std::time::Instant::now();
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
                phases.search(searched.elapsed().as_millis() as u64);
            }
            Some(Action::Read(source_ids)) => {
                let readed = std::time::Instant::now();
                // Fetches stay sequential (DB reads are cheap and the char
                // budget is a running total), then every distill — the model
                // call that dominates a read's wall-clock — runs concurrently.
                // `buffered` (not unordered) keeps transcript order matching
                // the planner's requested order, so runs stay reproducible.
                let mut fetched: Vec<(String, String, String)> = Vec::new();
                for source_id in source_ids {
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
                    fetched.push((source_id, title, content));
                }
                if fetched.len() > 1 {
                    emit_step(app, format!("Reading {} sources", fetched.len()));
                } else if let Some((_, title, _)) = fetched.first() {
                    emit_step(app, format!("Reading: {title}"));
                }
                // RLM-style sub-read: a separate model call distills each
                // document against the question into verbatim quotes, so a
                // read contributes evidence — not bulk — to every later
                // prompt. One distillate serves the planner transcript, the
                // writer excerpt, and the persisted citation alike.
                use futures::stream::StreamExt;
                let distilled: Vec<(String, String, String)> =
                    futures::stream::iter(fetched.into_iter().map(
                        |(source_id, title, content)| async move {
                            let evidence = distill(ollama, question, &title, &content).await;
                            (source_id, title, evidence)
                        },
                    ))
                    .buffered(DISTILL_CONCURRENCY)
                    .collect()
                    .await;
                for (source_id, title, evidence) in distilled {
                    transcript.push_str(&format!("READ \"{title}\":\n{evidence}\n\n"));
                    let read_id = format!("read:{source_id}");
                    if seen.insert(read_id.clone()) {
                        gathered.push(Citation {
                            chunk_id: read_id,
                            source_id,
                            source_title: title,
                            source_path: String::new(),
                            note_id: String::new(),
                            gist: false,
                            snote: false,
                            ordinal: 0,
                            snippet: evidence,
                            distance: 0.0,
                        });
                    }
                }
                phases.read(readed.elapsed().as_millis() as u64);
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
    let source_manifest: Vec<(String, String, String)> = sources
        .iter()
        .map(|s| (s.title.clone(), s.url.clone(), s.tags.clone()))
        .collect();
    let persona = rag::persona_block(&ollama.config().profile);
    // The agentic loop always drives a full-size model; the default profile's
    // budgets are the right shape here.
    // Agentic reads are already whole-document distillates — no neighbor
    // expansion to apply.
    let no_expansion = std::collections::HashMap::new();
    let messages = rag::build_chat_messages(
        history,
        question,
        rag::Excerpts {
            citations: &gathered,
            expanded: &no_expansion,
        },
        &source_manifest,
        extra_system,
        &persona,
        &crate::inference::ContextProfile::default(),
    );
    let app_cb = app.clone();
    let ttft_cb = ttft.clone();
    let outcome = ollama
        .chat_stream(&messages, |tok| {
            ttft_cb.mark();
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

/// Ask the model which `keep` of the (title, snippet) candidates actually
/// answer the question. Returns the kept indices in the model's preference
/// order, or None on any failure (model error, unparseable output, bogus
/// indices) — callers fall back to fusion order. Shared by the agentic
/// search loop and the meta-chat deep-search profile.
pub(crate) async fn rerank_indices(
    ai: &Ai,
    question: &str,
    snippets: &[(String, String)],
    keep: usize,
) -> Option<Vec<usize>> {
    let messages = rag::build_rerank_messages(question, snippets, keep);
    let raw = ai.chat_role(loop_role(), &messages).await.ok()?.text;
    let json = extract_json(&raw)?;
    let value: serde_json::Value = serde_json::from_str(&json).ok()?;
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
    let picked: Vec<usize> = indices
        .into_iter()
        .take(keep)
        .filter(|&i| i < snippets.len() && used.insert(i))
        .collect();
    if picked.is_empty() {
        None
    } else {
        Some(picked)
    }
}

/// Rerank a wide retrieval pool down to the SEARCH_K most relevant hits via
/// one model call, falling back to fusion order on failure.
pub(crate) async fn rerank(ai: &Ai, question: &str, hits: Vec<Citation>) -> Vec<Citation> {
    let snippets: Vec<(String, String)> = hits
        .iter()
        .map(|h| (h.source_title.clone(), truncate(&h.snippet, 300)))
        .collect();
    match rerank_indices(ai, question, &snippets, SEARCH_K).await {
        Some(picked) => picked.into_iter().map(|i| hits[i].clone()).collect(),
        None => hits.into_iter().take(SEARCH_K).collect(),
    }
}

/// Distill one document against the question into verbatim quotes via a
/// sub-call. On failure (model error, empty output) fall back to a raw head
/// excerpt — a degraded read still beats an empty one. Shared with artifact
/// generation, which distills content that won't fit its corpus budget.
pub(crate) async fn distill(ai: &Ai, question: &str, title: &str, content: &str) -> String {
    distill_with(ai, loop_role(), question, title, content).await
}

/// `distill` with the role named explicitly, so an eval can A/B the tiers
/// without mutating process-global state.
pub(crate) async fn distill_with(
    ai: &Ai,
    role: crate::inference::Role,
    question: &str,
    title: &str,
    content: &str,
) -> String {
    let messages = rag::build_distill_messages(question, title, content);
    match ai.chat_role(role, &messages).await {
        Ok(out) if !out.text.trim().is_empty() => truncate(out.text.trim(), DISTILL_MAX_CHARS),
        _ => truncate(content, READ_GIST_CHARS),
    }
}

fn emit_step(app: &AppHandle, label: String) {
    let _ = app.emit("chat://step", StepEvent { label });
}

/// Parse the planner's JSON action, tolerating surrounding prose/code fences.
pub(crate) fn parse_action(raw: &str) -> Option<Action> {
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
        "read" => {
            // Both grammars parse: the batched `sourceIds` array the prompt
            // advertises, and the legacy singular `sourceId` smaller models
            // keep producing from prior habits.
            let mut ids: Vec<String> = value
                .get("sourceIds")
                .and_then(|s| s.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            if ids.is_empty() {
                if let Some(one) = value.get("sourceId").and_then(|s| s.as_str()) {
                    ids.push(one.to_string());
                }
            }
            let mut seen = HashSet::new();
            ids.retain(|id| seen.insert(id.clone()));
            ids.truncate(READ_BATCH_CAP);
            if ids.is_empty() {
                None
            } else {
                Some(Action::Read(ids))
            }
        }
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
        // Legacy singular grammar still parses (smaller models keep emitting
        // it from habit) — it just becomes a batch of one.
        assert!(matches!(
            parse_action("```{\"action\":\"read\",\"sourceId\":\"abc\"}```"),
            Some(Action::Read(ids)) if ids == ["abc"]
        ));
        // The batched grammar the prompt advertises, capped and deduped.
        assert!(matches!(
            parse_action("{\"action\":\"read\",\"sourceIds\":[\"a\",\"a\",\"b\"]}"),
            Some(Action::Read(ids)) if ids == ["a", "b"]
        ));
        assert!(matches!(
            parse_action(
                "{\"action\":\"read\",\"sourceIds\":[\"a\",\"b\",\"c\",\"d\",\"e\",\"f\"]}"
            ),
            Some(Action::Read(ids)) if ids.len() == 4
        ));
        assert!(parse_action("{\"action\":\"read\",\"sourceIds\":[]}").is_none());
        assert!(matches!(
            parse_action("{\"action\":\"answer\"}"),
            Some(Action::Stop)
        ));
        assert!(parse_action("garbage").is_none());
        assert!(parse_action("{\"action\":\"search\",\"query\":\"\"}").is_none());
    }
}
