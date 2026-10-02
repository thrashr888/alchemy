//! The Home chat tool loop (docs/RFC-unified-chat.md §1).
//!
//! Home chat used to be a classifier: one gate, one LLM call into a closed
//! enum of seven verbs, one dispatch, done. Nothing chained, and no tool
//! result ever went back to the model. This module is the loop that was
//! missing — model → tool → result → model, until the model stops asking or
//! the round budget is spent.
//!
//! **It gathers; it does not answer.** Rounds are non-streaming, and the
//! evidence they collect (`LoopEvidence`) is folded into the prompt the
//! existing synthesis streams from. Citations, cancellation and the
//! `meta://token` protocol are therefore untouched: the loop replaces the
//! *retrieval* half of `ask_everything`, not the answering half. A loop that
//! gathers nothing falls through to plain corpus retrieval, so the worst case
//! is exactly today's behavior plus one model call.
//!
//! Phase 1 exposes read tools plus the writes Home already had. The catalog
//! is not yet read off the MCP routers: rmcp's `Peer::new` is `pub(crate)`,
//! so a `ToolRouter` cannot be called without a live MCP session, and the
//! tool bodies that derive an OKF by-line take a `RequestContext` a chat turn
//! has no way to produce. `catalog_is_covered` pins every advertised tool to
//! a dispatch arm in the meantime; phase 2 extracts the tool bodies so both
//! callers share one implementation.

use std::collections::HashSet;

use serde_json::{json, Value};
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::models::MetaCitation;

use super::{
    add_urls_from_home, contains_any, host_of, meta_step_to, open_notebook_outcome,
    retrieve_everything, save_home_note, AppState, MetaEffect, GLOBAL_TOOL_VERBS,
};

/// Rounds a loop may spend before it has to settle for what it has.
///
/// Gateways get more because their rounds cost seconds, not minutes. These
/// are starting points, not findings: every run writes `rounds_used`,
/// `wall_ms` and `settled` to `traces/chatloop.jsonl` precisely so the evals
/// can say whether the ceiling or the clock is the real constraint before we
/// trade the round budget for a time budget.
const ROUNDS_LOCAL: usize = 8;
const ROUNDS_GATEWAY: usize = 12;

/// Tool results are model input, not a transcript: a list of 400 sources
/// spends the window that the evidence needs.
const RESULT_CAP: usize = 4_000;

/// Enough evidence to answer from. Gathering stops once the loop holds this
/// many distinct passages, and synthesis never sees more.
///
/// This, not the round budget, is what bounds a turn in practice. The first
/// live run without it asked a question whose answer is not in the corpus:
/// the model searched 15 times over all 8 rounds (6.2 minutes), handed
/// synthesis 71 excerpts, and the local model timed out before its first
/// token — six minutes of research and no answer. A model hunting for
/// something absent never runs out of reasons to look again; a ceiling on
/// what it may carry back does. 24 is the single-shot path's 16 passages
/// plus headroom for what a second, reworded search adds.
const EVIDENCE_CAP: usize = 24;

/// What the loop collected, for the synthesis that follows it.
#[derive(Default)]
pub(crate) struct LoopEvidence {
    /// Deduped by (kind, id) in arrival order — the synthesis numbers these.
    pub citations: Vec<MetaCitation>,
    /// Tool output that is not a passage (a notebook listing, a timeline).
    /// Folded into the prompt as context the answer may use but cannot cite.
    pub facts: Vec<String>,
    /// Confirmations from tools that changed something, shown verbatim above
    /// the answer so a write is never something the user has to infer.
    pub replies: Vec<String>,
    pub effect: Option<MetaEffect>,
    pub rounds_used: usize,
    /// The model stopped asking for tools of its own accord, rather than
    /// being cut off by the budget.
    pub settled: bool,
    /// Why gathering ended: "settled" (the model was done), "sufficient"
    /// (`EVIDENCE_CAP` reached), "budget" (rounds ran out), "error" (a
    /// provider failure), "cancelled", or "skipped" (no tool-capable engine).
    /// Traced, because the evals need to tell a model that knew when to stop
    /// from one that was stopped.
    pub stop: &'static str,
    pub cancelled: bool,
}

impl LoopEvidence {
    fn push_citations(&mut self, found: Vec<MetaCitation>) {
        for c in found {
            if !self
                .citations
                .iter()
                .any(|u| u.kind == c.kind && u.id == c.id)
            {
                self.citations.push(c);
            }
        }
    }

    /// Did the loop come back with anything worth synthesizing from? A loop
    /// that answered "hello" with no tool calls has not, and the caller
    /// should retrieve the ordinary way.
    pub(crate) fn is_empty(&self) -> bool {
        self.citations.is_empty() && self.facts.is_empty() && self.replies.is_empty()
    }
}

/// One tool the loop can offer the model.
struct ToolSpec {
    name: &'static str,
    /// In the prompt from the first round, rather than behind `tool_search`.
    core: bool,
    description: &'static str,
    /// JSON Schema for the arguments object.
    params: fn() -> Value,
}

/// Every tool phase 1 advertises.
///
/// The `core` six are the ones a common Home turn needs before it can do
/// anything at all, and they are six because a catalog in a prompt is a bill
/// this codebase has already measured: `agent_cli.rs` records copilot loading
/// 105 tools for ~42k tool-definition tokens, and the fix was to cut it to
/// 17. A local model has far less room than copilot had. Everything else is
/// one `tool_search` round away.
fn catalog() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "search_corpus",
            core: true,
            description: "Search every notebook for passages relevant to a query. Returns numbered excerpts with their notebook and source. This is how you find evidence; call it more than once with different wordings when the first pass is thin.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "What to look for, in natural language." }
                    },
                    "required": ["query"]
                })
            },
        },
        ToolSpec {
            name: "list_notebooks",
            core: true,
            description: "List every notebook with its id, title, and how many sources and notes it holds. Call this before any tool that takes a notebook_id.",
            params: || json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "add_source",
            core: true,
            description: "Add web pages to the library by URL. Alchemy picks the notebook. Only pass URLs the user actually wrote.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "urls": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Full URLs, exactly as the user wrote them."
                        }
                    },
                    "required": ["urls"]
                })
            },
        },
        ToolSpec {
            name: "save_note",
            core: true,
            description: "Save the previous answer in this conversation as a note, filed in the notebook the answer itself belongs to.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Title for the note." }
                    },
                    "required": ["title"]
                })
            },
        },
        ToolSpec {
            name: "open_notebook",
            core: true,
            description: "Open a notebook in the app by name, when the user asks to go somewhere.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "The notebook's title, or part of it." }
                    },
                    "required": ["name"]
                })
            },
        },
        ToolSpec {
            name: "tool_search",
            core: true,
            description: "Find tools beyond the ones listed here. Returns matching tool definitions and makes them callable for the rest of this turn. Use it when you need to read a notebook's sources or notes, or inspect the app's own activity.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "What you want to do, e.g. \"read a note\" or \"list sources\"." }
                    },
                    "required": ["query"]
                })
            },
        },
        ToolSpec {
            name: "list_sources",
            core: false,
            description: "List a notebook's sources: id, title, type, size.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "notebook_id": { "type": "string" } },
                    "required": ["notebook_id"]
                })
            },
        },
        ToolSpec {
            name: "list_notes",
            core: false,
            description: "List a notebook's notes: id, title, kind.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "notebook_id": { "type": "string" } },
                    "required": ["notebook_id"]
                })
            },
        },
        ToolSpec {
            name: "get_note",
            core: false,
            description: "Read one note in full by id.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "note_id": { "type": "string" } },
                    "required": ["note_id"]
                })
            },
        },
        ToolSpec {
            name: "get_source",
            core: false,
            description: "Read one source's metadata by id: title, type, URL, size.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "source_id": { "type": "string" } },
                    "required": ["source_id"]
                })
            },
        },
        ToolSpec {
            name: "recent_errors",
            core: false,
            description: "Recent errors and failures the app recorded, newest first. Use when the user asks what went wrong.",
            params: || json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "list_receipts",
            core: false,
            description: "Recent background run receipts (scheduled reports, Night Shift chores): what ran, when, and whether it worked.",
            params: || json!({ "type": "object", "properties": {} }),
        },
    ]
}

/// The provider wire format for a tool definition. Ollama and the
/// OpenAI-compatible gateways take the same shape.
fn tool_schema(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": (spec.params)(),
        }
    })
}

/// What one dispatched tool produced.
#[derive(Default)]
struct ToolReply {
    /// What the model is told came back.
    text: String,
    citations: Vec<MetaCitation>,
    /// Shown to the user verbatim (a write happened).
    reply: Option<String>,
    effect: Option<MetaEffect>,
    /// Tool names to enable for the rest of the turn (`tool_search` only).
    enable: Vec<String>,
}

impl ToolReply {
    fn say(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

fn arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args[key].as_str().unwrap_or_default().trim()
}

/// Truncate a tool result to something a context window can afford, saying
/// so rather than silently cutting — a model that knows the list was clipped
/// asks a narrower question next round.
fn cap(mut text: String) -> String {
    if text.len() > RESULT_CAP {
        text.truncate(RESULT_CAP);
        text.push_str("\n… (truncated; narrow the query)");
    }
    text
}

/// Run one tool. Errors are returned to the MODEL as text, not propagated:
/// a bad argument is something it can correct on the next round, and killing
/// the turn over one teaches it nothing.
async fn dispatch(
    app: &AppHandle,
    state: &AppState,
    window_label: &str,
    thread_id: &str,
    question: &str,
    name: &str,
    args: &Value,
) -> ToolReply {
    let target = Some((app, window_label));
    // Writes and effects need the user's words behind them, not just the
    // model's judgment. The classifier route has always held URLs to this
    // ("only ingest URLs whose host actually appears in the user's message");
    // the loop holds every tool that changes something to the same rule.
    let asked = question.to_lowercase();
    match name {
        "search_corpus" => {
            let query = arg(args, "query");
            if query.is_empty() {
                return ToolReply::say("error: query is required");
            }
            meta_step_to(target, format!("Searching for “{query}”"), false);
            match retrieve_everything(state, target, query, 12, false).await {
                Err(err) => ToolReply::say(format!("error: search failed: {err}")),
                Ok(found) if found.is_empty() => {
                    ToolReply::say("No passages matched. Try different wording.")
                }
                Ok(found) => {
                    let text = found
                        .iter()
                        .map(|c| {
                            format!(
                                "[{} · {}] {}",
                                c.notebook_title,
                                c.title,
                                c.snippet.replace('\n', " ")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    ToolReply {
                        text: cap(text),
                        citations: found,
                        ..Default::default()
                    }
                }
            }
        }
        "list_notebooks" => match state.db.list_notebooks().await {
            Err(err) => ToolReply::say(format!("error: {err}")),
            Ok(notebooks) if notebooks.is_empty() => {
                ToolReply::say("The library has no notebooks yet.")
            }
            Ok(notebooks) => ToolReply::say(cap(notebooks
                .iter()
                .map(|n| format!("{} — id: {}", n.title, n.id))
                .collect::<Vec<_>>()
                .join("\n"))),
        },
        "add_source" => {
            let urls: Vec<String> = args["urls"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|u| u.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if urls.is_empty() {
                return ToolReply::say("error: urls is required and must be a non-empty array");
            }
            let urls = licensed_urls(urls, &asked);
            if urls.is_empty() {
                return ToolReply::say(
                    "error: only add URLs the user wrote in their message. None of these appear there.",
                );
            }
            let reply = add_urls_from_home(app, state, &urls).await;
            ToolReply {
                text: reply.clone(),
                reply: Some(reply),
                ..Default::default()
            }
        }
        "save_note" => {
            let title = arg(args, "title");
            if title.is_empty() {
                return ToolReply::say("error: title is required");
            }
            if !may_save(&asked) {
                return ToolReply::say(
                    "error: only save a note when the user asks to save or keep something.",
                );
            }
            let reply = save_home_note(app, state, thread_id, title).await;
            ToolReply {
                text: reply.clone(),
                reply: Some(reply),
                ..Default::default()
            }
        }
        "open_notebook" => {
            let name = arg(args, "name");
            if name.is_empty() {
                return ToolReply::say("error: name is required");
            }
            if !may_navigate(&asked) {
                return ToolReply::say(
                    "error: only open a notebook when the user asks to go somewhere. Answer the question instead.",
                );
            }
            let Ok(notebooks) = state.db.list_notebooks().await else {
                return ToolReply::say("error: couldn't read the notebook list");
            };
            let outcome = open_notebook_outcome(&notebooks, name);
            ToolReply {
                text: outcome.reply.clone(),
                reply: Some(outcome.reply),
                effect: outcome.effect,
                ..Default::default()
            }
        }
        "tool_search" => {
            let query = arg(args, "query").to_lowercase();
            let words: Vec<&str> = query.split_whitespace().collect();
            let matches: Vec<ToolSpec> = catalog()
                .into_iter()
                .filter(|t| !t.core)
                .filter(|t| {
                    let hay = format!("{} {}", t.name, t.description).to_lowercase();
                    words.iter().any(|w| w.len() > 2 && hay.contains(w))
                })
                .collect();
            if matches.is_empty() {
                return ToolReply::say(
                    "No other tool matches that. Answer from what you already have.",
                );
            }
            let text = matches
                .iter()
                .map(|t| format!("{} — {}", t.name, t.description))
                .collect::<Vec<_>>()
                .join("\n");
            ToolReply {
                text: format!("These tools are now callable:\n{text}"),
                enable: matches.iter().map(|t| t.name.to_string()).collect(),
                ..Default::default()
            }
        }
        "list_sources" => {
            let id = arg(args, "notebook_id");
            match state.db.list_sources(id).await {
                Err(err) => ToolReply::say(format!("error: {err}")),
                Ok(rows) if rows.is_empty() => ToolReply::say("That notebook has no sources."),
                Ok(rows) => ToolReply::say(cap(rows
                    .iter()
                    .map(|s| format!("{} ({}) — id: {}", s.title, s.source_type, s.id))
                    .collect::<Vec<_>>()
                    .join("\n"))),
            }
        }
        "list_notes" => {
            let id = arg(args, "notebook_id");
            match state.db.list_notes(id).await {
                Err(err) => ToolReply::say(format!("error: {err}")),
                Ok(rows) if rows.is_empty() => ToolReply::say("That notebook has no notes."),
                Ok(rows) => ToolReply::say(cap(rows
                    .iter()
                    .map(|n| format!("{} ({}) — id: {}", n.title, n.kind, n.id))
                    .collect::<Vec<_>>()
                    .join("\n"))),
            }
        }
        "get_note" => match state.db.get_note(arg(args, "note_id")).await {
            Err(err) => ToolReply::say(format!("error: {err}")),
            Ok(None) => ToolReply::say("error: no note with that id"),
            Ok(Some(note)) => ToolReply::say(cap(format!("# {}\n\n{}", note.title, note.content))),
        },
        "get_source" => match state.db.get_source_summary(arg(args, "source_id")).await {
            Err(err) => ToolReply::say(format!("error: {err}")),
            Ok(None) => ToolReply::say("error: no source with that id"),
            Ok(Some(s)) => ToolReply::say(cap(format!(
                "{} ({})\nurl: {}\nchars: {}",
                s.title, s.source_type, s.url, s.char_count
            ))),
        },
        "recent_errors" => {
            let rows = crate::diagnostics::recent(20, None);
            if rows.is_empty() {
                return ToolReply::say("No errors recorded.");
            }
            ToolReply::say(cap(rows
                .iter()
                .map(|r| {
                    format!(
                        "[{}] {}: {}",
                        r["level"].as_str().unwrap_or("?"),
                        r["scope"].as_str().unwrap_or("?"),
                        r["message"].as_str().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")))
        }
        "list_receipts" => match state.db.list_receipts(0, 20).await {
            Err(err) => ToolReply::say(format!("error: {err}")),
            Ok(rows) if rows.is_empty() => ToolReply::say("No background runs recorded yet."),
            Ok(rows) => ToolReply::say(cap(rows
                .iter()
                .map(|r| format!("{} — {} — {}", r.name, r.status, r.detail))
                .collect::<Vec<_>>()
                .join("\n"))),
        },
        // Unreachable while the catalog and this match stay in step, which
        // `catalog_is_covered` asserts. A model that hallucinates a name
        // lands here too, and is told so.
        other => ToolReply::say(format!("error: no tool named {other}")),
    }
}

/// The assistant row that records a round's tool calls, in the neutral form.
///
/// `arguments` stays an OBJECT. Ollama takes this as-is and rejects a
/// stringified one outright ("Value looks like object, but can't find closing
/// '}'"), which is what killed every second round until it was caught in the
/// app — round one has no prior calls to echo, so it hid there. Gateways want
/// the OpenAI dialect; `inference::to_openai_dialect` converts on the way out.
fn assistant_row(text: &str, calls: &[crate::inference::ToolCall]) -> Value {
    json!({
        "role": "assistant",
        "content": text,
        "tool_calls": calls.iter().map(|c| json!({
            "id": c.id,
            "type": "function",
            "function": { "name": c.name, "arguments": c.arguments },
        })).collect::<Vec<_>>(),
    })
}

/// A tool result row. Carries both links: Ollama reads `tool_name`, the
/// OpenAI dialect reads `tool_call_id` (and the conversion drops the other).
fn tool_row(id: &str, name: &str, content: &str) -> Value {
    json!({
        "role": "tool",
        "tool_call_id": id,
        "tool_name": name,
        "content": content,
    })
}

/// Words that license `save_note`. Deliberately not "note" on its own: "what
/// notes do I have on X" is a question, not an instruction to write one.
const SAVE_WORDS: [&str; 7] = [
    "save",
    "keep that",
    "keep this",
    "file that",
    "file this",
    "write that down",
    "write this down",
];

/// The URLs the user actually wrote. The model can never invent or rewrite
/// one into the library: a URL survives only if its host appears in the
/// user's own message, the rule the classifier route has always applied.
fn licensed_urls(urls: Vec<String>, asked: &str) -> Vec<String> {
    urls.into_iter()
        .filter(|u| {
            let host = host_of(u).to_lowercase();
            !host.is_empty() && asked.contains(&host)
        })
        .collect()
}

/// Did the user ask to keep something? `asked` is lowercased.
fn may_save(asked: &str) -> bool {
    contains_any(asked, &SAVE_WORDS)
}

/// Did the user ask to go somewhere? Moving the user's window is an action,
/// not research: the live run that found this reached for open_notebook
/// mid-search on a question that never asked to go anywhere.
fn may_navigate(asked: &str) -> bool {
    contains_any(asked, &GLOBAL_TOOL_VERBS)
}

/// What the model is told the loop is for.
///
/// Deliberately short. It is prepended to a prompt that already carries the
/// persona and the conversation, and every extra line is paid for on every
/// round by a model that may have 8k of room.
fn loop_system() -> String {
    let base =
        "You are gathering evidence to answer a question about the user's research library. \
     Use tools to find what you need — search more than once if the first result is thin. \
     Do NOT write the final answer: once you have enough, reply with no tool calls and \
     the answer will be written from what you gathered. \
     If two or three searches keep missing, the library probably doesn't have it: stop, \
     and the answer will say so. \
     Only act on what the user actually asked for; never invent a URL.";
    match crate::fieldnotes::prompt_block() {
        Some(notes) => format!("{base}\n\n{notes}"),
        None => base.to_string(),
    }
}

/// Run the loop. Returns what it gathered, for the caller to synthesize from.
///
/// `cancel` races the model call in every round, so Stop bites mid-loop
/// rather than only at the first token of the answer. Tool dispatch is
/// outside the race, matching the rule the classifier route already follows:
/// a mutation abandoned halfway is worse than one the user waits out.
pub(crate) async fn run(
    app: &AppHandle,
    state: &AppState,
    window_label: &str,
    thread_id: &str,
    question: &str,
    history: &[crate::ai::ChatTurn],
    cancel: &CancellationToken,
) -> LoopEvidence {
    let mut ev = LoopEvidence::default();
    let started = std::time::Instant::now();

    let (supports, is_gateway) = {
        let ai = state.ai.read().await.clone();
        (ai.chat_supports_tools(), ai.config().is_gateway())
    };
    if !supports {
        ev.stop = "skipped";
        return ev;
    }
    let budget = if is_gateway {
        ROUNDS_GATEWAY
    } else {
        ROUNDS_LOCAL
    };

    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": loop_system() })];
    for turn in history {
        messages.push(json!({ "role": turn.role, "content": turn.content }));
    }
    messages.push(json!({ "role": "user", "content": question }));

    let mut enabled: HashSet<String> = catalog()
        .iter()
        .filter(|t| t.core)
        .map(|t| t.name.to_string())
        .collect();
    let mut used: Vec<String> = Vec::new();

    ev.stop = "budget";
    for round in 0..budget {
        // Honest progress from the first second. Without this the user
        // watched the front end's "Searching every notebook…" placeholder
        // for the whole first round — up to a minute on a 30b model — while
        // nothing was searching yet: the model was still deciding what to
        // look for. Transient, so the list keeps only the real searches.
        meta_step_to(
            Some((app, window_label)),
            if round == 0 {
                "Deciding what to look for"
            } else {
                "Looking further"
            },
            true,
        );
        let tools: Vec<Value> = catalog()
            .iter()
            .filter(|t| enabled.contains(t.name))
            .map(tool_schema)
            .collect();

        let call = {
            let ai = state.ai.read().await.clone();
            tokio::select! {
                out = ai.chat_tools(&messages, &tools, round) => Some(out),
                _ = cancel.cancelled() => None,
            }
        };
        let Some(out) = call else {
            ev.cancelled = true;
            ev.stop = "cancelled";
            break;
        };
        let out = match out {
            Ok(out) => out,
            Err(err) => {
                // A provider that cannot do tools after all, or a transport
                // failure: the caller still has the ordinary retrieval path,
                // so this degrades rather than fails the turn.
                crate::note!("chat loop round {round} failed: {err:#}");
                ev.stop = "error";
                break;
            }
        };
        ev.rounds_used = round + 1;
        // Loop rounds are generations: they cost tokens and they set the pace
        // the user is waiting on. Leaving them out of the model's stats would
        // make a six-round turn look as fast as a one-shot answer.
        {
            let ai = state.ai.read().await.clone();
            state.record_chat_stats(&ai.chat_metrics_key(None), out.stats);
        }
        if out.calls.is_empty() {
            ev.settled = true;
            ev.stop = "settled";
            break;
        }

        messages.push(assistant_row(&out.text, &out.calls));

        for c in &out.calls {
            used.push(c.name.clone());
            let reply = dispatch(
                app,
                state,
                window_label,
                thread_id,
                question,
                &c.name,
                &c.arguments,
            )
            .await;
            // A rejected call is a mistake worth remembering across sessions.
            if let Some(why) = reply.text.strip_prefix("error:") {
                crate::fieldnotes::record(&c.name, why);
            }
            ev.push_citations(reply.citations);
            if let Some(r) = reply.reply {
                ev.replies.push(r);
            } else if !reply.text.is_empty() {
                ev.facts.push(format!("{}: {}", c.name, reply.text));
            }
            if reply.effect.is_some() {
                ev.effect = reply.effect;
            }
            for name in reply.enable {
                enabled.insert(name);
            }
            messages.push(tool_row(&c.id, &c.name, &reply.text));
        }
        if ev.citations.len() >= EVIDENCE_CAP {
            ev.stop = "sufficient";
            break;
        }
    }
    // One round of parallel searches can overshoot the cap; synthesis gets
    // the first EVIDENCE_CAP, which arrived first because they answered the
    // model's first, most direct queries.
    ev.citations.truncate(EVIDENCE_CAP);

    // A turn that ran out of rounds keeps everything it gathered. Shift's
    // evals found the opposite (a round-limited turn discarded whole) let the
    // next model invent an account of work it could not see; here the
    // evidence simply goes to synthesis with `settled` false beside it.
    if let Some(dir) = crate::trace::dir() {
        crate::trace::log_file(
            dir,
            "chatloop.jsonl",
            json!({
                "at": super::now(),
                "question": question.chars().take(200).collect::<String>(),
                "rounds_used": ev.rounds_used,
                "budget": budget,
                "settled": ev.settled,
                "stop": ev.stop,
                "empty": ev.is_empty(),
                "cancelled": ev.cancelled,
                "wall_ms": started.elapsed().as_millis() as u64,
                "tools": used,
                "citations": ev.citations.len(),
            }),
        );
    }
    ev
}

/// Read the loop's evidence back as the numbered passages the synthesis
/// prompt expects, deduping sources the way `ask_everything` does.
pub(crate) fn passages(citations: &[MetaCitation]) -> Vec<crate::rag::MetaPassage> {
    citations
        .iter()
        .enumerate()
        .map(|(i, c)| crate::rag::MetaPassage {
            number: i + 1,
            kind: c.kind.clone(),
            notebook_title: c.notebook_title.clone(),
            title: c.title.clone(),
            snippet: c.snippet.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every advertised tool has a dispatch arm.
    ///
    /// This is the seam the RFC calls out: until the tool bodies are shared
    /// with the MCP router, the catalog and the dispatcher are two lists, and
    /// two lists drift. A name here that `dispatch` does not know would reach
    /// the model as a callable tool and answer "no tool named …" every time.
    #[test]
    fn catalog_is_covered() {
        // The arms `dispatch` implements, kept beside the match it mirrors.
        let arms = [
            "search_corpus",
            "list_notebooks",
            "add_source",
            "save_note",
            "open_notebook",
            "tool_search",
            "list_sources",
            "list_notes",
            "get_note",
            "get_source",
            "recent_errors",
            "list_receipts",
        ];
        for spec in catalog() {
            assert!(
                arms.contains(&spec.name),
                "catalog advertises {} with no dispatch arm",
                spec.name
            );
        }
        assert_eq!(arms.len(), catalog().len(), "a dispatch arm went unlisted");
    }

    /// The six are six. A seventh is a prompt-tax decision, not a drive-by.
    #[test]
    fn core_set_stays_six() {
        let core: Vec<&str> = catalog()
            .iter()
            .filter(|t| t.core)
            .map(|t| t.name)
            .collect();
        assert_eq!(
            core,
            vec![
                "search_corpus",
                "list_notebooks",
                "add_source",
                "save_note",
                "open_notebook",
                "tool_search",
            ]
        );
    }

    /// Every schema is a well-formed JSON Schema object, because a provider
    /// rejects the whole request over one malformed tool.
    #[test]
    fn schemas_are_objects() {
        for spec in catalog() {
            let schema = tool_schema(&spec);
            assert_eq!(schema["type"], "function");
            assert_eq!(schema["function"]["name"], spec.name);
            assert_eq!(
                schema["function"]["parameters"]["type"], "object",
                "{} has a non-object parameter schema",
                spec.name
            );
        }
    }

    /// `tool_search` matches on intent words, and never offers a core tool
    /// (they are already in the prompt — re-offering them wastes a round).
    #[test]
    fn tool_search_matches_intent_not_core() {
        let hits: Vec<&str> = catalog()
            .iter()
            .filter(|t| !t.core)
            .filter(|t| {
                let hay = format!("{} {}", t.name, t.description).to_lowercase();
                hay.contains("note")
            })
            .map(|t| t.name)
            .collect();
        assert!(hits.contains(&"list_notes"));
        assert!(hits.contains(&"get_note"));
        assert!(!hits.contains(&"search_corpus"));
    }

    /// Oversized tool output is clipped AND says so.
    #[test]
    fn cap_marks_truncation() {
        let capped = cap("x".repeat(RESULT_CAP + 500));
        assert!(capped.len() < RESULT_CAP + 100);
        assert!(capped.ends_with("(truncated; narrow the query)"));
        let short = cap("fine".into());
        assert_eq!(short, "fine");
    }

    /// Citations dedupe by (kind, id): one source that contributed five
    /// passages is one citation, as it is in `ask_everything`.
    #[test]
    fn citations_dedupe() {
        let cite = |id: &str| MetaCitation {
            kind: "source".into(),
            notebook_id: "nb".into(),
            notebook_title: "NB".into(),
            id: id.into(),
            title: "T".into(),
            snippet: "s".into(),
        };
        let mut ev = LoopEvidence::default();
        ev.push_citations(vec![cite("a"), cite("b"), cite("a")]);
        ev.push_citations(vec![cite("b"), cite("c")]);
        assert_eq!(ev.citations.len(), 3);
        assert_eq!(passages(&ev.citations)[2].number, 3);
    }

    /// The bug the app found: the loop must record a call's arguments as an
    /// object, or Ollama 400s the next round. Pinned on the builder itself,
    /// not on a hand-made history, because the builder is where it broke.
    #[test]
    fn assistant_row_keeps_arguments_an_object() {
        let calls = vec![crate::inference::ToolCall {
            id: "call_0_0".into(),
            name: "search_corpus".into(),
            arguments: json!({ "query": "Guerneville house" }),
        }];
        let row = assistant_row("", &calls);
        let args = &row["tool_calls"][0]["function"]["arguments"];
        assert!(
            args.is_object(),
            "arguments must not be stringified: {args}"
        );
        assert_eq!(args["query"], "Guerneville house");

        let tool = tool_row("call_0_0", "search_corpus", "…");
        assert_eq!(tool["tool_name"], "search_corpus");
        assert_eq!(tool["tool_call_id"], "call_0_0");
    }

    /// The trust boundary, pinned. Every write the loop can make has to be
    /// licensed by the user's own words, not the model's judgment.
    #[test]
    fn writes_need_the_users_words() {
        // URLs: only hosts the user typed. An invented one is dropped even
        // when it rides alongside a real one.
        let asked = "add https://example.com/post please".to_lowercase();
        let kept = licensed_urls(
            vec![
                "https://example.com/post".into(),
                "https://attacker.test/payload".into(),
            ],
            &asked,
        );
        assert_eq!(kept, vec!["https://example.com/post".to_string()]);
        assert!(
            licensed_urls(vec!["https://made-up.test/".into()], "what's in my notes").is_empty()
        );

        // Save: an instruction, not the word "note" in a question.
        assert!(may_save("save that as a note"));
        assert!(may_save("please keep this"));
        assert!(!may_save("what notes do i have on the guerneville house?"));

        // Navigate: only when asked to go somewhere. The exact question from
        // the live run that reached for open_notebook must not license it.
        assert!(may_navigate("open the japan notebook"));
        assert!(may_navigate("take me to bayside"));
        assert!(!may_navigate(
            &"What did I conclude about the Guerneville house purchase date?".to_lowercase()
        ));
    }

    /// An empty loop is the signal to fall back to plain retrieval.
    #[test]
    fn empty_evidence_is_detected() {
        let mut ev = LoopEvidence::default();
        assert!(ev.is_empty());
        ev.facts.push("list_notebooks: one".into());
        assert!(!ev.is_empty());
    }
}
