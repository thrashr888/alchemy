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

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::models::MetaCitation;

use super::{
    add_text_from_home, add_url_sources, add_urls_from_home, delete_home_thread, host_of,
    meta_step_to, new_notebook, open_notebook_outcome, rename_home_thread, retrieve_everything,
    save_home_note, shared_tool_reply, AppState, MetaEffect, SharedAction, StyleTarget,
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
    /// The user said No to a write. The answer is that, and nothing the
    /// model went on to do after it: a declined write ends the turn.
    pub declined: bool,
    /// The model stopped asking for tools of its own accord, rather than
    /// being cut off by the budget.
    pub settled: bool,
    /// Why gathering ended: "settled" (the model was done), "sufficient"
    /// (`EVIDENCE_CAP` reached), "budget" (rounds ran out), "error" (a
    /// provider failure), "cancelled", "declined" (the user said No to a
    /// write), or "skipped" (no tool-capable engine).
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

/// Every tool the loop advertises.
///
/// The `core` seven are the ones a common Home turn needs before it can do
/// anything at all, and they are few because a catalog in a prompt is a bill
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
            name: "create_notebook",
            // Core, not behind tool_search: a local model asked to make a
            // notebook searched three times and stopped, never looking for
            // a tool it couldn't see (live run, 2026-10-03).
            core: true,
            description: "Create a new notebook when the user asks for one, optionally starting it with web pages. Give it a short title. urls may be pages the user wrote, or well-known public pages you are confident exist for the topic (for company filings, SEC EDGAR submissions JSON: https://data.sec.gov/submissions/CIK##########.json with the 10-digit CIK). Each page you propose is fetched first and only readable ones are added.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "The notebook's title." },
                        "urls": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Pages to start it with (optional, at most 12)."
                        }
                    },
                    "required": ["title"]
                })
            },
        },
        ToolSpec {
            name: "save_text",
            core: false,
            description: "Save text from the user's message (a pasted article, notes, a quote) as a source. Alchemy picks the notebook. Asks the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "A short title for the source." },
                        "text": { "type": "string", "description": "The text to save, as the user gave it." }
                    },
                    "required": ["text"]
                })
            },
        },
        ToolSpec {
            name: "rename_chat",
            core: false,
            description: "Rename THIS conversation.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "title": { "type": "string", "description": "The new name." } },
                    "required": ["title"]
                })
            },
        },
        ToolSpec {
            name: "delete_chat",
            core: false,
            description: "Delete THIS conversation and everything in it. Asks the user first.",
            params: || json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "settings",
            core: false,
            description: "Read or change Alchemy's settings. op: get (current AI settings, redacted), models (installed models and provider readiness), test (probe a provider or model; field = its name, or empty for the chat provider), setup (the next setup step), set (field = chatProvider|studioProvider|chatModel|effort|baseUrl|smallModel|embedder|profile.name|profile.profession|profile.instructions|profile.assistantName, value = new value), style (field = answer style, value = default|shorter|longer), theme (field = theme name, \"random\", or empty to list), pull (field = Ollama model; staged for the user to run, never run), connect (field = agent client, or empty to list). API keys can never be read or set. Changes ask the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "op": { "type": "string", "enum": ["get", "models", "test", "setup", "set", "style", "theme", "pull", "connect"] },
                        "field": { "type": "string" },
                        "value": { "type": "string" }
                    },
                    "required": ["op"]
                })
            },
        },
        ToolSpec {
            name: "night_shift",
            core: false,
            description: "The Night Shift (overnight report runs): status reports what is queued; pause and resume stop or restart it. Pause and resume ask the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "op": { "type": "string", "enum": ["status", "pause", "resume"] } },
                    "required": ["op"]
                })
            },
        },
        ToolSpec {
            name: "tool_search",
            core: true,
            description: "Find tools beyond the ones listed here. Returns matching tool definitions and makes them callable for the rest of this turn. Use it when you need to create a notebook, save text, change a setting, manage this chat or the Night Shift, read a notebook's sources or notes, or inspect the app's own activity.",
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
    /// The user said No to this write. The turn ends on their word.
    declined: bool,
}

impl ToolReply {
    fn say(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

/// A write's confirmation, shown to the user as well as told to the model.
fn write_reply(reply: String) -> ToolReply {
    ToolReply {
        text: reply.clone(),
        reply: Some(reply),
        ..Default::default()
    }
}

/// The user said No to a write. Said to them, not just the model, so the
/// answer is "you declined", never a search result misreading why nothing
/// happened.
fn declined_reply(reply: &str) -> ToolReply {
    ToolReply {
        text: reply.to_string(),
        reply: Some(reply.to_string()),
        declined: true,
        ..Default::default()
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

// ---- Where a turn runs ------------------------------------------------------

/// Which chat a loop turn belongs to. Home acts across the library from a
/// thread; a notebook's chat acts inside that one notebook (RFC 6c), where
/// the notebook's own retrieval pipeline answers questions and the loop's
/// job is the actions.
#[derive(Clone, Copy)]
pub(crate) enum Surface<'a> {
    Home { thread_id: &'a str },
    Notebook { notebook_id: &'a str },
}

impl Surface<'_> {
    /// The thread or notebook a prompt belongs to, as the front end matches it.
    fn scope_id(&self) -> &str {
        match self {
            Surface::Home { thread_id } => thread_id,
            Surface::Notebook { notebook_id } => notebook_id,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Surface::Home { .. } => "home",
            Surface::Notebook { .. } => "notebook",
        }
    }

    /// The undo journal key: Home threads share their agent turns' key
    /// (`home-<threadId>`); a notebook shares its Agent pane's (its id).
    fn undo_key(&self) -> String {
        match self {
            Surface::Home { thread_id } => format!("home-{thread_id}"),
            Surface::Notebook { notebook_id } => notebook_id.to_string(),
        }
    }
}

/// A progress line on the surface's own step trail: Home's is window-scoped
/// `meta://step`; a notebook chat's is `chat://step`.
fn step(app: &AppHandle, window_label: &str, surface: Surface<'_>, label: &str, transient: bool) {
    match surface {
        Surface::Home { .. } => meta_step_to(Some((app, window_label)), label, transient),
        Surface::Notebook { .. } => {
            let _ = app.emit(
                "chat://step",
                super::StepEvent {
                    label: label.to_string(),
                    transient,
                },
            );
        }
    }
}

// ---- Asking the user (RFC-unified-chat phase 6a) --------------------------

/// How long a prompt waits before it counts as No. Long enough to read a
/// list of URLs and decide; short enough that a prompt left behind doesn't
/// hold a turn open for the rest of the afternoon.
const PROMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Prompts waiting on the person: request id → (window, answer channel).
/// The window is kept so closing it can answer No.
type Pending = HashMap<String, (String, oneshot::Sender<bool>)>;
static PENDING: LazyLock<Mutex<Pending>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// One Yes/No the loop raises. The same shape the agent's permission
/// request takes on the wire, so the front end draws one prompt for both
/// brains; `detail` lists what exactly would change (the URLs).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LoopPermissionEvent {
    window: String,
    /// "home" or "notebook": which chat's prompt this is.
    surface: String,
    /// The Home thread or notebook the prompt belongs to.
    thread_id: String,
    request_id: String,
    tool_title: String,
    action: String,
    detail: Vec<String>,
    options: Vec<Value>,
}

/// Where a prompt goes and what can end it early.
#[derive(Clone, Copy)]
pub(crate) struct Asker<'a> {
    pub app: &'a AppHandle,
    pub window_label: &'a str,
    pub surface: Surface<'a>,
    pub cancel: &'a CancellationToken,
}

impl Asker<'_> {
    /// Ask the person before a write and wait for the answer. Anything but
    /// a Yes is a No: Stop, a newer question in this window (both cancel the
    /// turn's token), closing the window, or no answer within
    /// `PROMPT_TIMEOUT`. The wait is raised inside the cancel race, since
    /// nothing has changed yet, and stands the turn aside from the
    /// foreground queue so background work isn't held up by a person.
    ///
    /// A Yes to a write Undo can take back snapshots what it will change
    /// before returning, the way an agent's approved write does, under the
    /// same journal key that surface's agent turns use, so its Undo covers
    /// both brains.
    async fn approve(
        &self,
        state: &AppState,
        tool: &str,
        action: String,
        detail: Vec<String>,
        args: &Value,
    ) -> bool {
        let request_id = super::new_id();
        let (tx, rx) = oneshot::channel();
        PENDING
            .lock()
            .unwrap()
            .insert(request_id.clone(), (self.window_label.to_string(), tx));
        let _ = self.app.emit(
            "chat://permission",
            LoopPermissionEvent {
                window: self.window_label.to_string(),
                surface: self.surface.name().to_string(),
                thread_id: self.surface.scope_id().to_string(),
                request_id: request_id.clone(),
                tool_title: crate::mcp::access::human_title(tool)
                    .or_else(|| crate::mcp::access::human_title(&format!("mcp__alchemy__{tool}")))
                    .unwrap_or_else(|| tool.to_string()),
                action,
                detail,
                options: vec![
                    json!({ "id": "allow", "name": "Yes", "kind": "allow_once" }),
                    json!({ "id": "reject", "name": "No", "kind": "reject_once" }),
                ],
            },
        );
        let allowed = {
            let _aside = crate::foreground::stand_aside();
            tokio::select! {
                answer = rx => answer.unwrap_or(false),
                _ = self.cancel.cancelled() => false,
                _ = tokio::time::sleep(PROMPT_TIMEOUT) => false,
            }
        };
        PENDING.lock().unwrap().remove(&request_id);
        // Clears the prompt in a window that didn't answer it (timeout,
        // Stop), and is harmless in one that did.
        let _ = self.app.emit(
            "chat://permission-settled",
            json!({ "window": self.window_label, "requestId": request_id }),
        );
        if allowed && super::undo::journals(tool) {
            super::undo::capture(state, &self.surface.undo_key(), "Alchemy", tool, args).await;
        }
        allowed
    }
}

/// Answer a prompt the loop is waiting on. An unknown id is a prompt that
/// already settled (timed out, or Stop got there first) and is not an error
/// worth showing.
#[tauri::command]
pub fn loop_permission(request_id: String, allow: bool) {
    if let Some((_, tx)) = PENDING.lock().unwrap().remove(&request_id) {
        let _ = tx.send(allow);
    }
}

/// A closed window can't answer: every prompt it was showing is a No.
pub(crate) fn decline_window_prompts(window_label: &str) {
    let mut pending = PENDING.lock().unwrap();
    let ids: Vec<String> = pending
        .iter()
        .filter(|(_, (w, _))| w == window_label)
        .map(|(id, _)| id.clone())
        .collect();
    for id in ids {
        if let Some((_, tx)) = pending.remove(&id) {
            let _ = tx.send(false);
        }
    }
}

/// Run one tool. Errors are returned to the MODEL as text, not propagated:
/// a bad argument is something it can correct on the next round, and killing
/// the turn over one teaches it nothing.
async fn dispatch(
    asker: Asker<'_>,
    state: &AppState,
    question: &str,
    name: &str,
    args: &Value,
) -> ToolReply {
    let Asker {
        app,
        window_label,
        surface,
        ..
    } = asker;
    let thread_id = match surface {
        Surface::Home { thread_id } => thread_id,
        Surface::Notebook { notebook_id } => {
            return dispatch_notebook(asker, state, notebook_id, question, name, args).await;
        }
    };
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
            // The user's own URLs are their scope and go straight in. Any
            // the model came up with are a write the user approves, listed.
            let (written, proposed) = split_written(urls, &asked);
            let mut declined = Vec::new();
            let mut urls = written;
            if !proposed.is_empty() {
                let action = format!(
                    "add {} page{} you didn't link",
                    proposed.len(),
                    if proposed.len() == 1 { "" } else { "s" }
                );
                if asker
                    .approve(state, name, action, proposed.clone(), args)
                    .await
                {
                    urls.extend(proposed);
                } else {
                    declined = proposed;
                }
            }
            if urls.is_empty() {
                // Said to the user, not just the model: the answer has to
                // be "you said No", not a search result that missed.
                let reply = format!(
                    "Didn't add {} — you said No.",
                    declined
                        .iter()
                        .map(|u| host_of(u))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                return declined_reply(&reply);
            }
            let mut reply = add_urls_from_home(app, state, &urls).await;
            if !declined.is_empty() {
                reply.push_str(&format!(
                    "\n\nNot added (declined): {}",
                    declined
                        .iter()
                        .map(|u| host_of(u))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
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
            let action = format!("save the previous answer as a note, “{title}”");
            if !asker.approve(state, name, action, Vec::new(), args).await {
                return declined_reply("Didn't save the note — you said No.");
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
        "create_notebook" => {
            let title = arg(args, "title");
            if title.is_empty() {
                return ToolReply::say("error: title is required");
            }
            let urls: Vec<String> = args["urls"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|u| u.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            match create_notebook_with_pages(app, asker, state, title, urls, &asked, args).await {
                Err(err) => match err.strip_prefix("DECLINED:") {
                    Some(said) => declined_reply(said),
                    None => ToolReply::say(format!("error: {err}")),
                },
                Ok((reply, effect)) => ToolReply {
                    text: reply.clone(),
                    reply: Some(reply),
                    effect: Some(effect),
                    ..Default::default()
                },
            }
        }
        "save_text" => {
            let text = arg(args, "text");
            if text.is_empty() {
                return ToolReply::say("error: text is required");
            }
            let title = arg(args, "title");
            let action = if title.is_empty() {
                "save text from your message as a source".to_string()
            } else {
                format!("save “{title}” as a source")
            };
            if !asker.approve(state, name, action, Vec::new(), args).await {
                return declined_reply("Didn't save it — you said No.");
            }
            write_reply(add_text_from_home(app, state, title, text).await)
        }
        "rename_chat" => {
            // Local and reversible, like navigation: it runs without asking.
            let title = arg(args, "title");
            if title.is_empty() {
                return ToolReply::say("error: title is required");
            }
            write_reply(rename_home_thread(state, thread_id, title).await)
        }
        "delete_chat" => {
            let action = "delete this conversation and everything in it".to_string();
            if !asker.approve(state, name, action, Vec::new(), args).await {
                return declined_reply("Kept this conversation — you said No.");
            }
            let outcome = delete_home_thread(state, thread_id).await;
            ToolReply {
                text: outcome.reply.clone(),
                reply: Some(outcome.reply),
                effect: outcome.effect,
                ..Default::default()
            }
        }
        "settings" => {
            let op = arg(args, "op").to_string();
            let field = arg(args, "field").to_string();
            let value = arg(args, "value").to_string();
            // Reads run. A change asks, except `pull` (staged for the user
            // to run, never run) and `connect` (its own confirm click is
            // already the user's word before anything is written).
            let changes = matches!(op.as_str(), "set" | "style" | "theme");
            if changes {
                let action = match op.as_str() {
                    "set" => format!("set {field} to “{value}”"),
                    "style" => format!("change the answer style to {field} {value}")
                        .trim()
                        .to_string(),
                    _ if field.is_empty() || field == "random" => {
                        "switch to a random theme".to_string()
                    }
                    _ => format!("switch the theme to {field}"),
                };
                if !asker.approve(state, name, action, Vec::new(), args).await {
                    return declined_reply("Left the settings as they were — you said No.");
                }
            }
            let reply = shared_tool_reply(
                app,
                state,
                SharedAction::Settings { op, field, value },
                StyleTarget::Home,
            )
            .await;
            if changes {
                write_reply(reply)
            } else {
                ToolReply::say(cap(reply))
            }
        }
        "night_shift" => {
            let op = arg(args, "op").to_string();
            let changes = matches!(op.as_str(), "pause" | "resume");
            if changes
                && !asker
                    .approve(
                        state,
                        name,
                        format!("{op} the Night Shift"),
                        Vec::new(),
                        args,
                    )
                    .await
            {
                return declined_reply("Left the Night Shift as it was — you said No.");
            }
            let reply = shared_tool_reply(
                app,
                state,
                SharedAction::NightShift { op },
                StyleTarget::Home,
            )
            .await;
            if changes {
                write_reply(reply)
            } else {
                ToolReply::say(cap(reply))
            }
        }
        "tool_search" => tool_search_reply(catalog(), arg(args, "query")),
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

/// Pages a chat-made notebook may start with. Each one the model proposed
/// costs a fetch before it is added, so this also bounds the wait.
const MAX_STARTING_PAGES: usize = 12;

/// Make the notebook, then fill it. Pages the user wrote go straight in;
/// pages the model proposed are fetched first and only readable ones are
/// added, so a guessed address never becomes an errored row in the user's
/// new notebook — the reply names what was skipped instead.
///
/// Shared by the tool loop and the Home router, so a notebook can be made
/// from chat on every engine, tool-calling or not. `Err` is a message for
/// the model (or the user) to act on; nothing was created.
async fn create_notebook_with_pages(
    app: &AppHandle,
    asker: Asker<'_>,
    state: &AppState,
    title: &str,
    urls: Vec<String>,
    asked: &str,
    args: &Value,
) -> Result<(String, MetaEffect), String> {
    let target = Some((app, asker.window_label));
    let existing = state
        .db
        .list_notebooks()
        .await
        .map_err(|_| "couldn't read the notebook list".to_string())?;
    if let Some(nb) = existing
        .iter()
        .find(|n| n.title.eq_ignore_ascii_case(title.trim()))
    {
        return Err(format!(
            "a notebook called “{}” already exists (id: {}). Pick another title, or add to that one.",
            nb.title, nb.id
        ));
    }
    let urls: Vec<String> = urls
        .into_iter()
        .map(|u| u.trim().to_string())
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
        .take(MAX_STARTING_PAGES)
        .collect();
    let (written, proposed) = split_written(urls, asked);
    if !proposed.is_empty() {
        meta_step_to(
            target,
            format!("Checking {} suggested pages", proposed.len()),
            false,
        );
    }
    let checked = futures::future::join_all(
        proposed
            .iter()
            .map(|u| async move { (u.clone(), crate::growth::probe_readable(u).await) }),
    )
    .await;
    let skipped: Vec<String> = checked
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(u, _)| u.clone())
        .collect();
    // One prompt for the whole write: the notebook, and any readable pages
    // the model proposed, listed. The user's own pages ride along unlisted;
    // they were the user's words already.
    let readable: Vec<String> = checked
        .into_iter()
        .filter(|(_, ok)| *ok)
        .map(|(u, _)| u)
        .collect();
    let mut action = format!("create the notebook “{}”", title.trim());
    if !readable.is_empty() {
        action.push_str(&format!(
            " and start it with {} suggested page{}",
            readable.len(),
            if readable.len() == 1 { "" } else { "s" }
        ));
    }
    if !asker
        .approve(state, "create_notebook", action, readable.clone(), args)
        .await
    {
        return Err(format!(
            "DECLINED:Didn't create “{}” — you said No.",
            title.trim()
        ));
    }
    let mut pages = written;
    pages.extend(readable);
    let nb = new_notebook(state, title.to_string())
        .await
        .map_err(|err| format!("couldn't create the notebook: {err}"))?;
    let dest = format!("**{}**", nb.title);
    let mut reply = if pages.is_empty() {
        format!("Created the notebook {dest}.")
    } else {
        format!(
            "Created the notebook {dest}.\n\n{}",
            add_url_sources(app, state, &nb.id, &pages, "meta://step", &dest).await
        )
    };
    if !skipped.is_empty() {
        reply.push_str(&format!(
            "\n\nLeft out {} suggested page{} that didn't load as readable content:\n{}",
            skipped.len(),
            if skipped.len() == 1 { "" } else { "s" },
            skipped
                .iter()
                .map(|u| format!("- {u}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok((
        reply.trim_end().to_string(),
        MetaEffect {
            kind: "openNotebook".into(),
            notebook_id: nb.id,
        },
    ))
}

// ---- The notebook loop (RFC-unified-chat phase 6c) --------------------------

/// What a notebook's chat can do. Search is not here: the notebook's own
/// pipeline (hybrid search, gap retrieval, outline escalation, rerank)
/// answers questions better than a tool round could, so this loop acts and
/// the pipeline answers. A question costs one round with no tool calls.
fn notebook_catalog() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "add_source",
            core: true,
            description: "Add web pages to this notebook by URL. Pages the user didn't link are shown to them first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "urls": { "type": "array", "items": { "type": "string" }, "description": "Full URLs." }
                    },
                    "required": ["urls"]
                })
            },
        },
        ToolSpec {
            name: "generate",
            core: true,
            description: "Generate a document from this notebook's sources (a study guide, briefing, FAQ, timeline, and so on). It is saved as a note. Asks the user first.",
            params: || {
                let mut kinds: Vec<&str> = crate::rag::ARTIFACT_KINDS.to_vec();
                kinds.push("custom");
                json!({
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": kinds },
                        "prompt": { "type": "string", "description": "Extra instructions, or empty." }
                    },
                    "required": ["kind"]
                })
            },
        },
        ToolSpec {
            name: "save_note",
            core: true,
            description: "Save the previous answer in this chat as a note. Asks the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "title": { "type": "string", "description": "Title for the note, or empty." } }
                })
            },
        },
        ToolSpec {
            name: "remove_source",
            core: true,
            description: "Remove one source from this notebook, named by part of its title or its site. Asks the user first, and can be undone.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "name": { "type": "string" } },
                    "required": ["name"]
                })
            },
        },
        ToolSpec {
            name: "save_text",
            core: false,
            description: "Save text from the user's message as a source in this notebook. Asks the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "title": { "type": "string" }, "text": { "type": "string" } },
                    "required": ["text"]
                })
            },
        },
        ToolSpec {
            name: "refresh_sources",
            core: false,
            description: "Re-fetch URL sources in this notebook: those matching a name fragment, or all of them when it is empty. Asks the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "name": { "type": "string" } }
                })
            },
        },
        ToolSpec {
            name: "schedule_report",
            core: false,
            description: "Create a recurring report for this notebook (kind: an artifact kind, brief, custom, or a template name; interval: hourly, daily or weekly). Asks the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string" },
                        "interval": { "type": "string", "enum": ["hourly", "daily", "weekly"] },
                        "name": { "type": "string" },
                        "prompt": { "type": "string", "description": "What it should cover, for kind custom." }
                    },
                    "required": ["kind", "interval"]
                })
            },
        },
        ToolSpec {
            name: "update_report",
            core: false,
            description: "Change an existing recurring report, named by part of its name. Empty fields stay as they are. Asks the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "new_name": { "type": "string" },
                        "kind": { "type": "string" },
                        "interval": { "type": "string" },
                        "prompt": { "type": "string" },
                        "enabled": { "type": "string", "enum": ["", "true", "false"] }
                    },
                    "required": ["name"]
                })
            },
        },
        ToolSpec {
            name: "commission",
            core: false,
            description: "Hand one job to the Night Shift instead of running it now (when: tonight, or now only if the user says so). Asks the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string" },
                        "name": { "type": "string" },
                        "prompt": { "type": "string" },
                        "when": { "type": "string", "enum": ["tonight", "now"] }
                    },
                    "required": ["kind"]
                })
            },
        },
        ToolSpec {
            name: "create_template",
            core: false,
            description: "Save a reusable custom generator the user can run from Studio later. Compose its prompt from what they asked it to do. Asks the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "prompt": { "type": "string" }
                    },
                    "required": ["name", "prompt"]
                })
            },
        },
        ToolSpec {
            name: "settings",
            core: false,
            description: "Read or change Alchemy's settings; style changes apply to this notebook. Same ops as Home: get, models, test, setup, set, style, theme, pull, connect. Changes ask the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": {
                        "op": { "type": "string", "enum": ["get", "models", "test", "setup", "set", "style", "theme", "pull", "connect"] },
                        "field": { "type": "string" },
                        "value": { "type": "string" }
                    },
                    "required": ["op"]
                })
            },
        },
        ToolSpec {
            name: "night_shift",
            core: false,
            description: "The Night Shift: status, pause, or resume. Pause and resume ask the user first.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "op": { "type": "string", "enum": ["status", "pause", "resume"] } },
                    "required": ["op"]
                })
            },
        },
        ToolSpec {
            name: "list_sources",
            core: false,
            description: "List this notebook's sources: id, title, type.",
            params: || json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "tool_search",
            core: true,
            description: "Find tools beyond the ones listed here: save text, refresh sources, schedule or change reports, hand work to the Night Shift, save a template, change settings, list sources.",
            params: || {
                json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                })
            },
        },
    ]
}

/// The notebook loop's instructions: act on requests, leave questions to
/// the pipeline.
fn notebook_loop_system() -> String {
    let base = "You work in one notebook of the user's research library. If the user asks you \
     to DO something here (add a page, generate a document, save a note, remove or refresh \
     a source, schedule a report, change a setting), do it with the matching tool; use \
     tool_search to find one you don't see. Changes are shown to the user, who approves \
     them, so act rather than describe. If the user is asking a QUESTION, call no tools \
     at all: the notebook's own search answers questions, better than you can from here.";
    match crate::fieldnotes::prompt_block() {
        Some(notes) => format!("{base}\n\n{notes}"),
        None => base.to_string(),
    }
}

/// `tool_search` over a catalog: the non-core tools whose name or
/// description shares a word with the query, made callable for the turn.
fn tool_search_reply(catalog: Vec<ToolSpec>, query: &str) -> ToolReply {
    let query = query.to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    let matches: Vec<ToolSpec> = catalog
        .into_iter()
        .filter(|t| !t.core)
        .filter(|t| {
            let hay = format!("{} {}", t.name, t.description).to_lowercase();
            words.iter().any(|w| w.len() > 2 && hay.contains(w))
        })
        .collect();
    if matches.is_empty() {
        return ToolReply::say("No other tool matches that. Answer from what you already have.");
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

/// Run one notebook tool. Each write asks first, then lands in the same
/// `run_tool_action` the notebook router uses, so a verb behaves the same
/// whichever picked it.
async fn dispatch_notebook(
    asker: Asker<'_>,
    state: &AppState,
    notebook_id: &str,
    question: &str,
    name: &str,
    args: &Value,
) -> ToolReply {
    use super::ToolAction;
    let app = asker.app;
    let asked = question.to_lowercase();
    let s = |key: &str| arg(args, key).to_string();
    let sources = match state.db.list_sources(notebook_id).await {
        Ok(sources) => sources,
        Err(err) => return ToolReply::say(format!("error: couldn't read the sources: {err}")),
    };
    // Every write but add_source (tiered) and remove_source (named) asks
    // with a sentence of its own, then runs as the router would.
    let (action, ask): (ToolAction, Option<String>) = match name {
        "tool_search" => return tool_search_reply(notebook_catalog(), arg(args, "query")),
        "list_sources" => {
            if sources.is_empty() {
                return ToolReply::say("This notebook has no sources.");
            }
            return ToolReply::say(cap(sources
                .iter()
                .map(|s| format!("{} ({}) — id: {}", s.title, s.source_type, s.id))
                .collect::<Vec<_>>()
                .join("\n")));
        }
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
            let (mut urls, proposed) = split_written(urls, &asked);
            if !proposed.is_empty() {
                let action = format!(
                    "add {} page{} you didn't link to this notebook",
                    proposed.len(),
                    if proposed.len() == 1 { "" } else { "s" }
                );
                if asker
                    .approve(state, name, action, proposed.clone(), args)
                    .await
                {
                    urls.extend(proposed);
                } else if urls.is_empty() {
                    let hosts: Vec<String> = proposed.iter().map(|u| host_of(u)).collect();
                    return declined_reply(&format!(
                        "Didn't add {} — you said No.",
                        hosts.join(", ")
                    ));
                }
            }
            return write_reply(
                super::add_url_sources(
                    app,
                    state,
                    notebook_id,
                    &urls,
                    "chat://step",
                    "this notebook",
                )
                .await,
            );
        }
        "remove_source" => {
            let needle = arg(args, "name").to_lowercase();
            if needle.is_empty() {
                return ToolReply::say("error: name is required");
            }
            let matches: Vec<_> = sources
                .iter()
                .filter(|s| {
                    s.title.to_lowercase().contains(&needle)
                        || (!s.url.is_empty() && host_of(&s.url).to_lowercase().contains(&needle))
                })
                .collect();
            // Ambiguity and no-match are answers, not prompts: the router's
            // own arm says so, and nothing is asked until there is one target.
            if let [one] = matches.as_slice() {
                // Journaled as the MCP tool it is, so Undo can put it back.
                let journal = json!({ "source_id": one.id });
                let action = format!("remove “{}” from this notebook", one.title);
                if !asker
                    .approve(state, "delete_source", action, Vec::new(), &journal)
                    .await
                {
                    return declined_reply(&format!("Kept “{}” — you said No.", one.title));
                }
            }
            (ToolAction::RemoveSource(s("name")), None)
        }
        "generate" => {
            let kind = s("kind");
            let label = crate::rag::artifact_spec(&kind)
                .map(|(t, _)| t.to_string())
                .unwrap_or_else(|| "document".into());
            let ask = format!("generate a {label} from this notebook");
            (
                ToolAction::Generate {
                    kind,
                    prompt: s("prompt"),
                },
                Some(ask),
            )
        }
        "save_note" => (
            ToolAction::SaveNote(s("title")),
            Some("save the previous answer as a note".to_string()),
        ),
        "save_text" => {
            if arg(args, "text").is_empty() {
                return ToolReply::say("error: text is required");
            }
            let ask = match arg(args, "title") {
                "" => "save text from your message as a source".to_string(),
                t => format!("save “{t}” as a source"),
            };
            (
                ToolAction::AddText {
                    title: s("title"),
                    text: s("text"),
                },
                Some(ask),
            )
        }
        "refresh_sources" => {
            let ask = match arg(args, "name") {
                "" => "re-fetch every URL source in this notebook".to_string(),
                n => format!("re-fetch the sources matching “{n}”"),
            };
            (ToolAction::RefreshSources(s("name")), Some(ask))
        }
        "schedule_report" => {
            let ask = format!("schedule a {} {} report", s("interval"), s("kind"));
            (
                ToolAction::ScheduleReport {
                    kind: s("kind"),
                    interval: s("interval"),
                    name: s("name"),
                    prompt: s("prompt"),
                },
                Some(ask),
            )
        }
        "update_report" => {
            let ask = format!("change the report “{}”", s("name"));
            (
                ToolAction::UpdateReport {
                    name: s("name"),
                    new_name: s("new_name"),
                    kind: s("kind"),
                    interval: s("interval"),
                    prompt: s("prompt"),
                    enabled: s("enabled"),
                },
                Some(ask),
            )
        }
        "commission" => {
            let when = if arg(args, "when") == "now" {
                "now"
            } else {
                "tonight"
            };
            let ask = format!("hand a {} job to the Night Shift ({when})", s("kind"));
            (
                ToolAction::Commission {
                    kind: s("kind"),
                    name: s("name"),
                    prompt: s("prompt"),
                    when: when.to_string(),
                },
                Some(ask),
            )
        }
        "create_template" => {
            let ask = format!("save a template, “{}”", s("name"));
            (
                ToolAction::CreateTemplate {
                    name: s("name"),
                    description: s("description"),
                    prompt: s("prompt"),
                },
                Some(ask),
            )
        }
        "settings" => {
            let (op, field, value) = (s("op"), s("field"), s("value"));
            let ask = match op.as_str() {
                "set" => Some(format!("set {field} to “{value}”")),
                "style" => Some(
                    format!("change this notebook's answer style to {field} {value}")
                        .trim()
                        .to_string(),
                ),
                "theme" if field.is_empty() || field == "random" => {
                    Some("switch to a random theme".to_string())
                }
                "theme" => Some(format!("switch the theme to {field}")),
                _ => None,
            };
            (
                ToolAction::Shared(SharedAction::Settings { op, field, value }),
                ask,
            )
        }
        "night_shift" => {
            let op = s("op");
            let ask =
                matches!(op.as_str(), "pause" | "resume").then(|| format!("{op} the Night Shift"));
            (ToolAction::Shared(SharedAction::NightShift { op }), ask)
        }
        other => return ToolReply::say(format!("error: no tool named {other}")),
    };
    let is_write = ask.is_some() || name == "remove_source";
    if let Some(ask) = ask {
        if !asker.approve(state, name, ask, Vec::new(), args).await {
            return declined_reply("Left it as it was — you said No.");
        }
    }
    let reply = super::run_tool_action(app, state, notebook_id, question, &sources, action)
        .await
        .unwrap_or_default();
    if is_write {
        write_reply(reply)
    } else {
        ToolReply::say(cap(reply))
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

/// Split URLs into the ones the user wrote (their host appears in the
/// message, the rule the classifier route has always applied) and the ones
/// the model came up with. The user's own go straight in; the model's are
/// a write the user approves.
fn split_written(urls: Vec<String>, asked: &str) -> (Vec<String>, Vec<String>) {
    urls.into_iter().partition(|u| {
        let host = host_of(u).to_lowercase();
        !host.is_empty() && asked.contains(&host)
    })
}

/// What the model is told the loop is for.
///
/// Deliberately short. It is prepended to a prompt that already carries the
/// persona and the conversation, and every extra line is paid for on every
/// round by a model that may have 8k of room.
fn loop_system() -> String {
    let base = "You work in the user's research library. If the user asks you to DO something \
     (make a notebook, add or save something, change a setting, rename or delete this \
     chat), do it with the matching tool first; use tool_search to find one you don't \
     see. Changes are shown to the user, who approves them, so act rather than describe. \
     If the user asks a QUESTION, gather evidence: search more than once if the first \
     result is thin, and when two or three searches keep missing, stop. \
     Do NOT write the final answer: reply with no tool calls when you are done and the \
     answer will be written from what you gathered.";
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
    surface: Surface<'_>,
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

    let system = match surface {
        Surface::Home { .. } => loop_system(),
        Surface::Notebook { .. } => notebook_loop_system(),
    };
    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": system })];
    let catalog = || match surface {
        Surface::Home { .. } => catalog(),
        Surface::Notebook { .. } => notebook_catalog(),
    };
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
        step(
            app,
            window_label,
            surface,
            match (surface, round) {
                (Surface::Notebook { .. }, 0) => "Reading your request",
                (_, 0) => "Deciding what to look for",
                _ => "Looking further",
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
            let asker = Asker {
                app,
                window_label,
                surface,
                cancel,
            };
            let reply = dispatch(asker, state, question, &c.name, &c.arguments).await;
            // A rejected call is a mistake worth remembering across sessions.
            if let Some(why) = reply.text.strip_prefix("error:") {
                crate::fieldnotes::record(&c.name, why);
            }
            ev.push_citations(reply.citations);
            if let Some(r) = reply.reply {
                ev.replies.push(r);
            } else if !reply.text.is_empty() && c.name != "tool_search" {
                // tool_search's list of tools is the loop's bookkeeping, not
                // something the answer can use: as a fact it sent a pure
                // action turn ("rename this chat") on to synthesis, which
                // then reported finding nothing about renaming.
                ev.facts.push(format!("{}: {}", c.name, reply.text));
            }
            if reply.effect.is_some() {
                ev.effect = reply.effect;
            }
            for name in reply.enable {
                enabled.insert(name);
            }
            messages.push(tool_row(&c.id, &c.name, &reply.text));
            if reply.declined {
                ev.declined = true;
                break;
            }
        }
        if ev.declined {
            ev.stop = "declined";
            break;
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
                "surface": surface.name(),
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
            "create_notebook",
            "save_text",
            "rename_chat",
            "delete_chat",
            "settings",
            "night_shift",
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

    /// The core set is a prompt-tax decision, not a drive-by. The seventh,
    /// `create_notebook`, was one: asked for a notebook, a local model never
    /// went to `tool_search` for a tool it couldn't see.
    #[test]
    fn core_set_stays_seven() {
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
                "create_notebook",
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

    /// The URL trust boundary, pinned: the user's own URLs go straight in,
    /// and anything the model came up with goes to the user first. Every
    /// other write asks (`Asker::approve`).
    /// A Yes reaches the waiting turn; anything else is a No, and an
    /// answer to a prompt that already settled is ignored, not an error.
    #[tokio::test]
    async fn a_prompt_settles_once_and_closing_its_window_says_no() {
        let (tx, rx) = oneshot::channel();
        PENDING
            .lock()
            .unwrap()
            .insert("p1".into(), ("main".into(), tx));
        loop_permission("p1".into(), true);
        assert!(rx.await.unwrap());
        loop_permission("p1".into(), true); // already settled: no panic

        let (tx, rx) = oneshot::channel();
        PENDING
            .lock()
            .unwrap()
            .insert("p2".into(), ("other".into(), tx));
        decline_window_prompts("main");
        assert!(PENDING.lock().unwrap().contains_key("p2"));
        decline_window_prompts("other");
        assert!(!rx.await.unwrap());
    }

    #[test]
    fn writes_need_the_users_words() {
        // URLs: hosts the user typed go straight in. An invented one never
        // does, even riding alongside a real one: it goes to the user.
        let asked = "add https://example.com/post please".to_lowercase();
        let (written, proposed) = split_written(
            vec![
                "https://example.com/post".into(),
                "https://attacker.test/payload".into(),
            ],
            &asked,
        );
        assert_eq!(written, vec!["https://example.com/post".to_string()]);
        assert_eq!(proposed, vec!["https://attacker.test/payload".to_string()]);
        let (written, _) =
            split_written(vec!["https://made-up.test/".into()], "what's in my notes");
        assert!(written.is_empty());
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
