//! Hosted agents over the Agent Client Protocol (docs/RFC-acp-agents.md).
//!
//! Spawns the user's installed coding agent (opencode, Claude Code, Codex)
//! as an ACP subprocess, hands it Alchemy's own MCP endpoint at
//! session/new, and streams its turns to the UI as Tauri events. One hosted
//! session per notebook at a time; the agent's own login is the credential,
//! same as the headless CLI providers.
//!
//! Events (payloads carry `notebookId` — self-filter in multi-window):
//! - `acp://state`      lifecycle: starting → ready → turn → idle | error | stopped
//! - `acp://update`     one session/update notification, schema JSON passed through
//! - `acp://permission` an agent permission request awaiting `acp_permission`

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, HttpHeader, InitializeRequest, LoadSessionRequest, McpServer,
    McpServerHttp, NewSessionRequest, PermissionOptionKind, PromptRequest,
    RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome, SessionId,
    SessionModeState, SessionNotification, SetSessionModeRequest, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo, Responder};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};

use crate::inference::{find_binary_cached, load_shell_env};

// ---- Known agents -----------------------------------------------------------

/// The ACP-capable subset of the agent CLIs we know how to launch. Claude Code
/// speaks ACP through Zed's adapter (run via npx so nothing is installed);
/// the rest ship a native entrypoint.
///
/// Gemini CLI was here until 2026-08-22. Google retired it for individual
/// accounts on 2026-06-18 — it still starts and still has `--acp`, but
/// `session/new` dies with "This client is no longer supported for Gemini
/// Code Assist for individuals", so offering it only produced an agent that
/// could never answer. Its successor, Antigravity, is absent for a different
/// reason: the `agy` CLI has no ACP mode (google-antigravity/antigravity-cli#31
/// is open), and Google's separate `agy_acp_server` binary — the one the ACP
/// registry lists — is a download we'd have to fetch and verify ourselves,
/// which is the registry-driven picker we haven't built. Until then Alchemy
/// reaches Antigravity over MCP, via the connector in connectors.rs.
const AGENTS: [AcpAgentKind; 3] = [
    AcpAgentKind::Opencode,
    AcpAgentKind::ClaudeCode,
    AcpAgentKind::Codex,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum AcpAgentKind {
    Opencode,
    ClaudeCode,
    Codex,
}

impl AcpAgentKind {
    fn id(self) -> &'static str {
        match self {
            AcpAgentKind::Opencode => "opencode",
            AcpAgentKind::ClaudeCode => "claude-code",
            AcpAgentKind::Codex => "codex",
        }
    }

    fn label(self) -> &'static str {
        match self {
            AcpAgentKind::Opencode => "opencode",
            AcpAgentKind::ClaudeCode => "Claude Code",
            AcpAgentKind::Codex => "Codex",
        }
    }

    /// The session mode in which this agent asks the client before running
    /// tools, so Alchemy — not the agent — decides what may change a notebook
    /// (docs/RFC-unified-chat.md §6). Without it each agent applies its own
    /// rule: Claude Code inherits the user's `defaultMode` (often `auto`,
    /// which approves tools itself), and Codex starts in "Auto review".
    ///
    /// opencode has no such mode — its modes pick an agent, and asking is
    /// governed by opencode's own config — so it keeps its own rules. That
    /// is the one gap in the "same rule for every brain" promise.
    fn ask_mode(self) -> Option<&'static str> {
        match self {
            AcpAgentKind::ClaudeCode => Some("default"),
            AcpAgentKind::Codex => Some("workspace-write"),
            AcpAgentKind::Opencode => None,
        }
    }

    /// May this agent be Home's brain? Only one Alchemy can hold to the
    /// app's rule about what may change a notebook — one with an asking mode
    /// (see `ask_mode`). opencode has none, so it would decide for itself, and
    /// Home keeps the loop for it instead. The notebook Agent pane still
    /// hosts it (decided 2026-10-02).
    fn can_answer_home(self) -> bool {
        self.ask_mode().is_some()
    }

    fn from_id(id: &str) -> Option<Self> {
        AGENTS.into_iter().find(|k| k.id() == id)
    }

    /// The terminal command that signs this agent in, for the "Open Terminal"
    /// fix on an auth failure. Every entry must already be on
    /// `commands::terminal_command_allowed`'s allowlist — that list, not this
    /// one, is the security boundary.
    fn login_command(self) -> &'static str {
        match self {
            // `claude` alone: /login is a slash command inside the session.
            AcpAgentKind::ClaudeCode => "claude",
            AcpAgentKind::Opencode => "opencode auth login",
            AcpAgentKind::Codex => "codex login",
        }
    }

    /// How to install this agent, for the blank slate when none is present.
    /// Lives here rather than the UI so it stays next to the binary name it
    /// has to match.
    fn install_hint(self) -> &'static str {
        match self {
            AcpAgentKind::Opencode => "brew install sst/tap/opencode",
            AcpAgentKind::ClaudeCode => "npm install -g @anthropic-ai/claude-code",
            AcpAgentKind::Codex => "npm install -g @openai/codex",
        }
    }

    /// Launch config without environment, or None when the required binaries
    /// aren't installed. Kept free of `load_shell_env` on purpose: that spawns
    /// a login shell, and discovery asks this for every agent — paying for one
    /// login shell per agent blew past the IPC timeout before the picker could
    /// render. The env is attached in `launch`, once, only for the agent we
    /// actually start.
    fn command(self) -> Option<AcpAgentConfig> {
        Some(match self {
            AcpAgentKind::Opencode => {
                AcpAgentConfig::new(find_binary_cached("opencode")?).arg("acp")
            }
            // Both adapters moved out of the @zed-industries scope in 2026 and
            // the old names are deprecated — pinned to the scope, not the
            // vendor, because that is where updates land now.
            AcpAgentKind::ClaudeCode => {
                // The adapter drives the user's `claude` install; require it so
                // we don't offer an agent that can't authenticate.
                find_binary_cached("claude")?;
                AcpAgentConfig::new(find_binary_cached("npx")?)
                    .arg("-y")
                    .arg("@agentclientprotocol/claude-agent-acp")
            }
            AcpAgentKind::Codex => {
                // Codex has no ACP entrypoint of its own: `codex acp` was never
                // a subcommand in 0.149, so it fell through to the interactive
                // TUI and died on "stdin is not a terminal". The adapter drives
                // Codex's app-server protocol instead.
                find_binary_cached("codex")?;
                AcpAgentConfig::new(find_binary_cached("npx")?)
                    .arg("-y")
                    .arg("@agentclientprotocol/codex-acp")
            }
        })
    }

    /// Full launch config. The child inherits the login-shell env (GUI apps
    /// don't get dotfile PATH/auth), with provider API keys stripped so the
    /// CLI's own login is the credential — same scar as the headless
    /// providers. Blocking: callers run it off the async runtime.
    ///
    /// The SDK spawns with the app's own env underneath these additions and
    /// offers no env_clear, so vars the app itself inherited leak through.
    /// The one that bites: a dev build launched from a Claude Code terminal
    /// carries CLAUDECODE, and the claude CLI refuses to nest ("cannot be
    /// launched inside another Claude Code session") — the adapter then dies
    /// at session/new with a generic wire error. Empty string reads as unset
    /// to that guard.
    fn launch(self) -> Option<AcpAgentConfig> {
        Some(self.command()?.envs(load_shell_env()).env("CLAUDECODE", ""))
    }
}

// ---- Home: the agent as the corpus-wide brain -------------------------------

/// Home threads have no notebook, so their sessions are keyed by this prefix
/// and the thread id (docs/RFC-unified-chat.md §6). The session map, the
/// working directory and the events all take any string as their key; only
/// the preamble has to know a Home session from a notebook one.
pub const HOME_SCOPE: &str = "home-";

fn is_home(key: &str) -> bool {
    key.starts_with(HOME_SCOPE)
}

/// Who answers Home's questions (docs/RFC-unified-chat.md §6).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeBrain {
    /// "agent": Home turns run in a thread-scoped ACP session.
    /// "loop": they go through `ask_everything` as before.
    pub kind: &'static str,
    /// The ACP agent id, for "agent"; empty otherwise.
    pub agent_id: String,
    /// What the turn is captioned with ("Claude Code").
    pub label: String,
    /// How to sign the agent in ("claude"), so a turn that fails on an
    /// expired login can say what to run instead of just "Authentication
    /// required".
    pub login_command: String,
}

impl HomeBrain {
    fn local() -> Self {
        Self {
            kind: "loop",
            agent_id: String::new(),
            label: String::new(),
            login_command: String::new(),
        }
    }
}

/// Decide, per ask, whether Home's brain is the user's agent.
///
/// The agent answers when the chat provider IS an agent that speaks ACP. That
/// is the case the tool loop can never serve — `supports_tools()` is false for
/// agent CLIs, because headless they are one-shot answerers over excerpts
/// Alchemy already retrieved. Hosted over ACP the same agent is a loop of its
/// own with all of Alchemy's tools, so Home hands it the turn instead of
/// running a second loop around it. The chat provider's kind and the ACP
/// agent id are the same string (`claude-code`, `codex`, `opencode`), so this
/// is a lookup, not a mapping table.
///
/// Two conditions fall back to the loop rather than fail the ask: the agent's
/// binary isn't installed, and the MCP server isn't running. Without MCP the
/// agent would arrive with no way into the user's notebooks, which for a
/// corpus-wide question is worse than the excerpt answer it replaces.
#[tauri::command]
pub async fn home_brain(
    app: AppHandle,
    state: tauri::State<'_, crate::commands::AppState>,
) -> Result<HomeBrain, String> {
    let provider_kind = {
        let ai = state.ai.read().await;
        let config = ai.config();
        config
            .provider_by_id(&config.chat_provider)
            .map(|p| p.kind.clone())
            .unwrap_or_default()
    };
    let Some(kind) = AcpAgentKind::from_id(&provider_kind).filter(|k| k.can_answer_home()) else {
        return Ok(HomeBrain::local());
    };
    if !crate::mcp::status(&app).running {
        return Ok(HomeBrain::local());
    }
    // The binary probe can fall back to a login-shell `which`; same reason
    // `acp_agents` keeps it off the IPC thread.
    let installed = tauri::async_runtime::spawn_blocking(move || kind.command().is_some())
        .await
        .unwrap_or(false);
    if !installed {
        return Ok(HomeBrain::local());
    }
    Ok(HomeBrain {
        kind: "agent",
        agent_id: kind.id().to_string(),
        label: kind.label().to_string(),
        login_command: kind.login_command().to_string(),
    })
}

/// Answer a permission request on the user's behalf, or leave it to them.
///
/// Alchemy's own read-only tools are allowed without asking — the same rule
/// the tool loop follows, so a search never interrupts the user whichever
/// brain runs it. Anything else (a write, a delete, a shell command, a tool
/// from another server, a name we can't read) goes to the user.
///
/// `names` are every spelling of the tool the request offers, best first:
/// Claude Code's `_meta.claudeCode.toolName`, the request title, and for
/// Codex — whose MCP permission request carries only the call id — the title
/// its earlier `tool_call` update gave that id. Only `allow_once` is ever
/// chosen: "always" would outlive this decision.
fn auto_allow(
    names: &[Option<String>],
    options: &[(String, PermissionOptionKind)],
) -> Option<String> {
    let tool = names
        .iter()
        .flatten()
        .find_map(|n| crate::mcp::access::alchemy_tool(n))?;
    if crate::mcp::access::access(tool) != crate::mcp::access::Access::Read {
        return None;
    }
    options
        .iter()
        .find(|(_, kind)| *kind == PermissionOptionKind::AllowOnce)
        .map(|(id, _)| id.clone())
}

/// The answers the user is offered: this call only.
///
/// Agents also offer "allow always" ("Yes, and don't ask again for Create
/// Note commands"), which writes a rule into the AGENT's own settings — after
/// which that agent stops asking Alchemy at all, while every other brain
/// still asks. The rule about what may change a notebook is the app's, so a
/// remembered answer would have to be remembered by the app; until it is,
/// only per-call answers are shown. If an agent ever offers nothing else,
/// its options are kept as they are rather than leaving no way to say yes.
fn per_call_options(
    options: &[agent_client_protocol::schema::v1::PermissionOption],
) -> Vec<&agent_client_protocol::schema::v1::PermissionOption> {
    let once: Vec<_> = options
        .iter()
        .filter(|o| {
            !matches!(
                o.kind,
                PermissionOptionKind::AllowAlways | PermissionOptionKind::RejectAlways
            )
        })
        .collect();
    if once
        .iter()
        .any(|o| o.kind == PermissionOptionKind::AllowOnce)
    {
        once
    } else {
        options.iter().collect()
    }
}

/// Put the agent in its asking mode, if it has one and offers it here.
async fn set_ask_mode(
    connection: &ConnectionTo<Agent>,
    kind: Option<AcpAgentKind>,
    session_id: &SessionId,
    modes: Option<&SessionModeState>,
    agent_label: &str,
) {
    let Some(mode) = kind.and_then(AcpAgentKind::ask_mode) else {
        return;
    };
    // An adapter release could rename its modes. Setting one it doesn't
    // offer is an error at best; say so instead, and leave the session in
    // whatever mode it chose.
    let offered = modes.is_some_and(|m| m.available_modes.iter().any(|am| &*am.id.0 == mode));
    if !offered {
        crate::note!(
            "acp: {agent_label} doesn't offer an asking mode ({mode}); its own rules apply"
        );
        return;
    }
    if let Err(err) = connection
        .send_request(SetSessionModeRequest::new(session_id.clone(), mode))
        .block_task()
        .await
    {
        crate::note!("acp: couldn't put {agent_label} in {mode} mode: {err}");
    }
}

/// Alchemy's own MCP server, as the agent should connect to it.
///
/// The server has required its per-installation bearer token since the local
/// security hardening (67207aa, 2026-08-31); this handoff predates that and
/// passed only the URL. Every hosted agent — the notebook Agent pane as much
/// as Home — then met a 401 on its first call, and since "attached" was
/// decided from the server running rather than from the agent being able to
/// get in, nothing said so: the agent was told its tools were there and found
/// them locked. Found driving Home's agent brain live, where Codex reported
/// `mcp__alchemy__startup (failed)` and went looking for the port itself.
///
/// No token means no attachment, so the preamble never points the agent at
/// tools it can't open.
fn alchemy_mcp_server(app: &AppHandle, url: &str) -> Option<McpServer> {
    match crate::mcp::auth_token(app) {
        Ok(token) => Some(McpServer::Http(McpServerHttp::new("alchemy", url).headers(
            vec![HttpHeader::new("Authorization", format!("Bearer {token}"))],
        ))),
        Err(err) => {
            crate::note!(
                "acp: no MCP token for the agent, attaching without Alchemy's tools: {err:#}"
            );
            None
        }
    }
}

/// The Home counterpart of `session_preamble`: no notebook to name, so it
/// points the agent at the whole library and at the corpus-wide tools.
fn home_preamble() -> String {
    "<context>You are running inside Alchemy, the user's local research notebook app, \
     answering in its library-wide chat: questions here can span every notebook the user \
     has. Their sources and notes are reachable through the connected `alchemy` MCP tools: \
     start with `ask_everything` (passages from across all notebooks, each naming its \
     notebook), use `list_notebooks` to see what exists and `search` with a notebook_id to \
     go deeper in one, and `get_source`/`get_note` to read in full. Ground your answer in \
     what those tools return and name the notebook each fact came from. Only add, change or \
     delete anything when the user asks you to.</context>"
        .to_string()
}

// ---- State ------------------------------------------------------------------

#[derive(Default)]
pub struct AcpState {
    sessions: Mutex<HashMap<String, SessionHandle>>,
}

struct SessionHandle {
    agent_id: String,
    tx: mpsc::UnboundedSender<HostCmd>,
    /// Permission requests awaiting a UI answer, keyed by the id we emitted.
    permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
    /// Clock for the turn in flight, so the agent pane reports the same
    /// send-to-first-token wait the chat surfaces do. Taken by the first
    /// answer chunk of the turn; None between turns.
    ttft: Arc<Mutex<Option<crate::commands::TtftClock>>>,
}

struct PendingPermission {
    responder: Responder<RequestPermissionResponse>,
}

enum HostCmd {
    Prompt(String),
    Cancel,
    Stop,
}

// ---- Event payloads ---------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StateEvent {
    notebook_id: String,
    agent_id: String,
    state: &'static str,
    /// Stop reason on idle, error message on error, agent info JSON on ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<serde_json::Value>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UpdateEvent {
    notebook_id: String,
    update: serde_json::Value,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PermissionEvent {
    notebook_id: String,
    request_id: String,
    tool_title: String,
    /// For one of Alchemy's own tools, what it does in the user's words
    /// ("create a note"), so the prompt can read "Claude Code wants to create
    /// a note" instead of naming `mcp__alchemy__create_note`.
    action: Option<String>,
    options: Vec<PermissionOptionInfo>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PermissionOptionInfo {
    id: String,
    name: String,
    kind: String,
}

fn emit_state(
    app: &AppHandle,
    notebook_id: &str,
    agent_id: &str,
    state: &'static str,
    detail: Option<serde_json::Value>,
) {
    let _ = app.emit(
        "acp://state",
        StateEvent {
            notebook_id: notebook_id.to_string(),
            agent_id: agent_id.to_string(),
            state,
            detail,
        },
    );
}

// ---- Commands ---------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentInfo {
    pub id: String,
    pub label: String,
    pub available: bool,
    /// Terminal command that signs this agent in — offered as a fix when a
    /// session dies on open because the agent isn't authenticated.
    pub login_command: String,
    /// Shell one-liner that installs it, shown when nothing is installed.
    pub install_hint: String,
}

/// Detected ACP agents for the picker. The binary probe falls back to a login
/// shell `which` on a cache miss, so this runs on the blocking pool rather
/// than stalling the IPC thread on first open.
#[tauri::command]
pub async fn acp_agents() -> Vec<AcpAgentInfo> {
    tauri::async_runtime::spawn_blocking(|| {
        AGENTS
            .into_iter()
            .map(|kind| AcpAgentInfo {
                id: kind.id().to_string(),
                label: kind.label().to_string(),
                available: kind.command().is_some(),
                login_command: kind.login_command().to_string(),
                install_hint: kind.install_hint().to_string(),
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Can this agent actually open a session right now? An installed binary says
/// nothing about being signed in, and the wire error for "not signed in" is
/// generic enough to look like a crash — so the only honest check is a real
/// `initialize` + `session/new`. Runs with no MCP server and a throwaway cwd,
/// then drops the connection; the SDK's `ChildGuard` reaps the process group.
/// Seconds, and it can prompt the agent's own device flow, so Settings asks
/// for it per agent on click rather than probing every agent on open.
#[tauri::command]
pub async fn acp_check(agent_id: String) -> Result<(), String> {
    let kind =
        AcpAgentKind::from_id(&agent_id).ok_or_else(|| format!("unknown agent: {agent_id}"))?;
    let config = tauri::async_runtime::spawn_blocking(move || kind.launch())
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{} is not installed", kind.label()))?;
    let label = kind.label();
    let cwd = std::env::temp_dir();

    // The closure's Ok type carries the verdict: returning `Ok(Err(msg))`
    // rather than `Err(..)` lets the connection close cleanly while still
    // handing back wording the user can act on.
    let probe = agent_client_protocol::Client
        .builder()
        .connect_with(
            AcpAgent::new(config),
            |connection: ConnectionTo<Agent>| async move {
                let init = match connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await
                {
                    Ok(init) => init,
                    Err(err) => return Ok(Err(format!("{label} didn't start. {err}"))),
                };
                match connection
                    .send_request(NewSessionRequest::new(cwd))
                    .block_task()
                    .await
                {
                    Ok(_) => Ok(Ok(())),
                    Err(err) => Ok(Err(session_open_error(
                        label,
                        &init.auth_methods,
                        &err.to_string(),
                    ))),
                }
            },
        )
        .await;

    match probe {
        Ok(verdict) => verdict,
        Err(err) => Err(format!("{label} didn't start. {err}")),
    }
}

/// The active session's agent id for a notebook, if one is running — lets a
/// remounted view re-sync without waiting for the next event.
#[tauri::command]
pub fn acp_status(app: AppHandle, notebook_id: String) -> Option<String> {
    let state = app.state::<AcpState>();
    let sessions = state.sessions.lock().unwrap();
    sessions.get(&notebook_id).map(|h| h.agent_id.clone())
}

/// Start a hosted session for a notebook. Resolves once initialize +
/// session/new have completed (so failures surface in the caller), then
/// updates stream as events. Replaces any existing session for the notebook.
#[tauri::command]
pub async fn acp_start(
    app: AppHandle,
    notebook_id: String,
    agent_id: String,
    resume: Option<String>,
) -> Result<(), String> {
    let kind =
        AcpAgentKind::from_id(&agent_id).ok_or_else(|| format!("unknown agent: {agent_id}"))?;
    // Building the config reads the login-shell environment — seconds of
    // blocking work, so keep it off the async runtime.
    let config = tauri::async_runtime::spawn_blocking(move || kind.launch())
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{} is not installed", kind.label()))?;

    // The preamble names the notebook, so look the title up front; an empty
    // title (notebook gone mid-start) degrades to id-only wording. A Home
    // session has no notebook and needs no lookup.
    let notebook_title = if is_home(&notebook_id) {
        String::new()
    } else {
        let db = app.state::<crate::commands::AppState>().db.clone();
        db.list_notebooks()
            .await
            .ok()
            .and_then(|nbs| nbs.into_iter().find(|n| n.id == notebook_id))
            .map(|n| n.title)
            .unwrap_or_default()
    };

    let state = app.state::<AcpState>();
    let (tx, rx) = mpsc::unbounded_channel();
    let permissions: Arc<Mutex<HashMap<String, PendingPermission>>> = Arc::default();
    let ttft: Arc<Mutex<Option<crate::commands::TtftClock>>> = Arc::default();
    {
        let mut sessions = state.sessions.lock().unwrap();
        if let Some(old) = sessions.remove(&notebook_id) {
            let _ = old.tx.send(HostCmd::Stop);
        }
        sessions.insert(
            notebook_id.clone(),
            SessionHandle {
                agent_id: agent_id.clone(),
                tx,
                permissions: permissions.clone(),
                ttft: ttft.clone(),
            },
        );
    }

    let (ready_tx, ready_rx) = oneshot::channel();
    let task_app = app.clone();
    let task_notebook = notebook_id.clone();
    let task_agent = agent_id.clone();
    let task_permissions = permissions.clone();
    tauri::async_runtime::spawn(async move {
        emit_state(&task_app, &task_notebook, &task_agent, "starting", None);
        let result = run_session(
            task_app.clone(),
            task_notebook.clone(),
            notebook_title,
            task_agent.clone(),
            config,
            resume,
            rx,
            ready_tx,
            permissions,
            ttft,
        )
        .await;
        // Clear the slot — but only if this session still owns it. A restart
        // may have replaced the handle while this one was shutting down, and
        // its events shouldn't stomp the replacement's.
        let acp = task_app.state::<AcpState>();
        let owned = {
            let mut sessions = acp.sessions.lock().unwrap();
            let owned = sessions
                .get(&task_notebook)
                .is_some_and(|h| Arc::ptr_eq(&h.permissions, &task_permissions));
            if owned {
                sessions.remove(&task_notebook);
            }
            owned
        };
        if owned {
            match result {
                Ok(()) => emit_state(&task_app, &task_notebook, &task_agent, "stopped", None),
                Err(err) => emit_state(
                    &task_app,
                    &task_notebook,
                    &task_agent,
                    "error",
                    Some(serde_json::Value::String(format!("{err:#}"))),
                ),
            }
        }
    });

    ready_rx
        .await
        .map_err(|_| "agent exited before the session was ready".to_string())?
}

/// Send a user prompt into the notebook's hosted session.
#[tauri::command]
pub fn acp_prompt(app: AppHandle, notebook_id: String, text: String) -> Result<(), String> {
    // Start the turn clock before the prompt goes out, so the agent pane's
    // time to first token covers the same span the chat surfaces time.
    if let Some(handle) = app
        .state::<AcpState>()
        .sessions
        .lock()
        .unwrap()
        .get(&notebook_id)
    {
        *handle.ttft.lock().unwrap() = Some(crate::commands::TtftClock::start());
    }
    send_cmd(&app, &notebook_id, HostCmd::Prompt(text))
}

/// Cancel the in-flight turn (session/cancel); the session stays alive.
///
/// Idempotent: see `send_cmd_if_running`.
#[tauri::command]
pub fn acp_cancel(app: AppHandle, notebook_id: String) -> Result<(), String> {
    send_cmd_if_running(&app, &notebook_id, HostCmd::Cancel)
}

/// End the notebook's hosted session and reap the agent subprocess.
///
/// Idempotent: see `send_cmd_if_running`.
#[tauri::command]
pub fn acp_stop(app: AppHandle, notebook_id: String) -> Result<(), String> {
    send_cmd_if_running(&app, &notebook_id, HostCmd::Stop)
}

/// Answer a pending permission request. `option_id: None` cancels it.
#[tauri::command]
pub fn acp_permission(
    app: AppHandle,
    notebook_id: String,
    request_id: String,
    option_id: Option<String>,
) -> Result<(), String> {
    let state = app.state::<AcpState>();
    let permissions = {
        let sessions = state.sessions.lock().unwrap();
        let handle = sessions
            .get(&notebook_id)
            .ok_or_else(|| "no agent session for this notebook".to_string())?;
        handle.permissions.clone()
    };
    let pending = permissions
        .lock()
        .unwrap()
        .remove(&request_id)
        .ok_or_else(|| "permission request already answered".to_string())?;
    let outcome = match option_id {
        Some(id) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
        None => RequestPermissionOutcome::Cancelled,
    };
    pending
        .responder
        .respond(RequestPermissionResponse::new(outcome))
        .map_err(|e| e.to_string())
}

/// Send a command whose goal is the *absence* of something - stop, cancel -
/// treating "there was no session" as already done rather than as an error.
///
/// Both callers ask for a postcondition: no session running, no turn in
/// flight. When there is no session, that postcondition already holds, so
/// reporting failure describes the world incorrectly. It also cost real
/// signal: every `#[tauri::command]` error is recorded at `error` level, and
/// the agent pane calls stop on unmount, so an ordinary session of closing
/// notebooks wrote 118 of the 132 errors in a day's log - enough noise to
/// bury an actual crash, which is exactly what it did during a pre-release
/// sweep. The front end had already decided this was nothing and swallowed
/// it (`api.acpStop(id).catch(() => {})`); only the log disagreed.
///
/// A hung-up channel is treated the same way: the host loop being gone is
/// the state the caller was asking for.
fn send_cmd_if_running(app: &AppHandle, notebook_id: &str, cmd: HostCmd) -> Result<(), String> {
    let state = app.state::<AcpState>();
    let sessions = state.sessions.lock().unwrap();
    let Some(handle) = sessions.get(notebook_id) else {
        return Ok(());
    };
    let _ = handle.tx.send(cmd);
    Ok(())
}

/// Send a command that genuinely needs a live session - a prompt, a
/// permission answer - where "no session" is a real failure worth surfacing.
fn send_cmd(app: &AppHandle, notebook_id: &str, cmd: HostCmd) -> Result<(), String> {
    let state = app.state::<AcpState>();
    let sessions = state.sessions.lock().unwrap();
    let handle = sessions
        .get(notebook_id)
        .ok_or_else(|| "no agent session for this notebook".to_string())?;
    handle
        .tx
        .send(cmd)
        .map_err(|_| "agent session has ended".to_string())
}

// ---- Session task -----------------------------------------------------------

/// Names of the agent's advertised sign-in methods. Read through JSON rather
/// than matching the schema enum: `AuthMethod` is `#[non_exhaustive]` and
/// gains variants between releases, while every variant carries a
/// human-readable name either way.
fn auth_method_names<T: Serialize>(methods: &[T]) -> Vec<String> {
    methods
        .iter()
        .filter_map(|m| serde_json::to_value(m).ok())
        .filter_map(|v| {
            v.get("name")
                .or_else(|| v.get("description"))
                .or_else(|| v.get("id"))
                .and_then(|n| n.as_str().map(str::to_string))
        })
        .collect()
}

/// The message for a session that died on open. The actionable sentence goes
/// first — the raw wire error is multi-line JSON that says nothing a user can
/// use, so it trails as flattened detail rather than leading.
fn session_open_error<T: Serialize>(label: &str, methods: &[T], wire: &str) -> String {
    let detail = wire.split_whitespace().collect::<Vec<_>>().join(" ");
    let names = auth_method_names(methods);
    if names.is_empty() {
        format!("{label} couldn't open a session. {detail}")
    } else {
        format!(
            "{label} couldn't open a session — it may need you to sign in first ({}). \
             Sign in from a terminal, then retry. ({detail})",
            names.join(", ")
        )
    }
}

/// The agent's working directory: a per-notebook scratch dir under app data.
/// Deliberately not the LanceDB data dir — the agent's file tools operate
/// here, and notebook content is reachable only through our MCP tools.
fn session_cwd(app: &AppHandle, notebook_id: &str) -> std::path::PathBuf {
    let base = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join("acp").join(notebook_id);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The agent arrives with its own system prompt and no idea where it is
/// running: it can see the alchemy MCP tools but not which notebook this
/// session belongs to, so "what's the cheapest one?" gets a coding
/// assistant's shrug instead of a search over the user's sources. Said once,
/// at the head of the session's first prompt — and only when the MCP server
/// actually attached, because pointing the agent at tools it doesn't have
/// would be worse than silence.
fn session_preamble(notebook_title: &str, notebook_id: &str) -> String {
    let name = if notebook_title.is_empty() {
        "this notebook".to_string()
    } else {
        format!("the notebook \"{notebook_title}\"")
    };
    format!(
        "<context>You are running inside Alchemy, the user's local research notebook app, \
         attached to {name} (notebook_id: {notebook_id}). The user's questions are usually \
         about this notebook's contents. Its sources are reachable through the connected \
         `alchemy` MCP tools: start with `search` (hybrid search over the notebook's sources \
         and notes; pass this notebook_id), and use `list_sources`/`get_source` to read full \
         documents. Ground answers about the notebook's subject in those sources.</context>"
    )
}

#[expect(clippy::too_many_arguments)]
async fn run_session(
    app: AppHandle,
    notebook_id: String,
    notebook_title: String,
    agent_id: String,
    config: AcpAgentConfig,
    resume: Option<String>,
    mut rx: mpsc::UnboundedReceiver<HostCmd>,
    ready_tx: oneshot::Sender<Result<(), String>>,
    permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
    ttft: Arc<Mutex<Option<crate::commands::TtftClock>>>,
) -> Result<(), agent_client_protocol::Error> {
    let agent = AcpAgent::new(config);
    // Errors name the agent the way the picker does ("Claude Code"), not by
    // its id — the id is ours, the label is what the user chose.
    let agent_label = AcpAgentKind::from_id(&agent_id).map_or("The agent", |k| k.label());

    let update_app = app.clone();
    let update_notebook = notebook_id.clone();
    let update_ttft = ttft.clone();
    // Ranked under the label the picker shows ("Claude Code"), not the id.
    let update_model = agent_label.to_string();
    let perm_app = app.clone();
    let perm_notebook = notebook_id.clone();
    // Codex's MCP permission request carries only the call id; the tool's
    // name arrived earlier, on the `tool_call` update for that id. Remember
    // it so the permission can be judged by what it is for.
    let tool_titles: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let update_titles = tool_titles.clone();
    let kind = AcpAgentKind::from_id(&agent_id);

    let mut ready_tx = Some(ready_tx);
    let cwd = session_cwd(&app, &notebook_id);
    let mcp = crate::mcp::status(&app);

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                let mut update =
                    serde_json::to_value(&notification.update).unwrap_or(serde_json::Value::Null);
                if let (Some(id), Some(title)) = (
                    update.get("toolCallId").and_then(|v| v.as_str()),
                    update.get("title").and_then(|v| v.as_str()),
                ) {
                    let mut titles = update_titles.lock().unwrap();
                    // A session can run for days; the names only matter
                    // until the permission that follows a call.
                    if titles.len() > 512 {
                        titles.clear();
                    }
                    titles.insert(id.to_string(), title.to_string());
                }
                // Our own tools in the user's words, in every trail that
                // shows them (Home and the Agent pane alike). After the cache
                // above, which keeps the wire name the permission check reads.
                if let Some(human) = update
                    .get("title")
                    .and_then(|v| v.as_str())
                    .and_then(crate::mcp::access::human_title)
                {
                    update["title"] = serde_json::Value::String(human);
                }
                // First answer chunk of the turn stops the clock — thoughts
                // and tool calls stream before it, but "first answer token"
                // is what the chat surfaces measure, so the agent pane is
                // ranked on the same basis.
                if update.get("sessionUpdate").and_then(|v| v.as_str())
                    == Some("agent_message_chunk")
                {
                    if let Some(clock) = update_ttft.lock().unwrap().take() {
                        clock.mark();
                        if let Some(state) = update_app.try_state::<crate::commands::AppState>() {
                            state.record_ttft(
                                &update_model,
                                "agent-pane",
                                &update_notebook,
                                &clock,
                                None,
                            );
                        }
                    }
                }
                let _ = update_app.emit(
                    "acp://update",
                    UpdateEvent {
                        notebook_id: update_notebook.clone(),
                        update,
                    },
                );
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::RequestPermissionRequest,
                        responder,
                        _cx| {
                let names = [
                    request
                        .tool_call
                        .meta
                        .as_ref()
                        .and_then(|m| m.get("claudeCode"))
                        .and_then(|c| c.get("toolName"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    request.tool_call.fields.title.clone(),
                    tool_titles
                        .lock()
                        .unwrap()
                        .get(&*request.tool_call.tool_call_id.0)
                        .cloned(),
                ];
                let kinds: Vec<(String, PermissionOptionKind)> = request
                    .options
                    .iter()
                    .map(|o| (o.option_id.0.to_string(), o.kind))
                    .collect();
                if let Some(id) = auto_allow(&names, &kinds) {
                    // One of Alchemy's reads: the user never sees it ask.
                    let _ = responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
                    ));
                    return Ok(());
                }
                let request_id = uuid::Uuid::new_v4().to_string();
                let options = per_call_options(&request.options)
                    .into_iter()
                    .map(|o| PermissionOptionInfo {
                        id: o.option_id.0.to_string(),
                        name: o.name.clone(),
                        kind: serde_json::to_value(o.kind)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_default(),
                    })
                    .collect();
                let ours = names
                    .iter()
                    .flatten()
                    .find_map(|n| crate::mcp::access::alchemy_tool(n));
                let action = ours
                    .and_then(crate::mcp::access::action)
                    .map(str::to_string);
                let tool_title = names
                    .iter()
                    .flatten()
                    .find_map(|n| crate::mcp::access::human_title(n))
                    .or_else(|| request.tool_call.fields.title.clone())
                    .unwrap_or_default();
                permissions
                    .lock()
                    .unwrap()
                    .insert(request_id.clone(), PendingPermission { responder });
                let _ = perm_app.emit(
                    "acp://permission",
                    PermissionEvent {
                        notebook_id: perm_notebook.clone(),
                        request_id,
                        tool_title,
                        action,
                        options,
                    },
                );
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            let init = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await;
            let init = match init {
                Ok(init) => init,
                Err(err) => {
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(Err(format!(
                            "{agent_label} didn't start. {}",
                            err.to_string()
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ")
                        )));
                    }
                    return Err(err);
                }
            };

            let mut session_req = NewSessionRequest::new(cwd.clone());
            // Notebook access is the entire point of hosting the agent here,
            // so a session that opens without it is worth saying out loud
            // rather than leaving the user to wonder why the agent can't find
            // any of their sources. Two ways to end up here: the MCP server is
            // switched off in Settings, or it failed to bind its port — which
            // a second dev build on the same machine will cause, since the
            // dev +1 offset only separates dev from the installed app.
            let alchemy = (mcp.running && init.agent_capabilities.mcp_capabilities.http)
                .then(|| alchemy_mcp_server(&app, &mcp.url))
                .flatten();
            let mcp_attached = alchemy.is_some();
            if let Some(server) = &alchemy {
                session_req = session_req.mcp_servers(vec![server.clone()]);
            }
            // Resume first when we have a session to resume and the agent can
            // do it. `session/load` replays the whole conversation back as
            // session/update notifications, so the transcript is rebuilt from
            // the agent's own memory rather than from ours — and, unlike our
            // stored copy, the resumed session can actually be prompted again.
            //
            // A stale id is ordinary, not exceptional: agents expire their own
            // sessions, and a machine can lose them entirely. Falling through
            // to a fresh session is the right answer, silently.
            let mut resumed = None;
            let mut modes: Option<SessionModeState> = None;
            if let Some(id) = resume.filter(|_| init.agent_capabilities.load_session) {
                let mut load_req = LoadSessionRequest::new(id.clone(), cwd.clone());
                if let Some(server) = &alchemy {
                    load_req = load_req.mcp_servers(vec![server.clone()]);
                }
                match connection.send_request(load_req).block_task().await {
                    Ok(resp) => {
                        modes = resp.modes;
                        resumed = Some(SessionId::from(id));
                    }
                    Err(err) => crate::note!("acp: could not resume {agent_label} session: {err}"),
                }
            }

            let was_resumed = resumed.is_some();
            let session_id = match resumed {
                Some(id) => id,
                None => match connection.send_request(session_req).block_task().await {
                    Ok(s) => {
                        modes = s.modes;
                        s.session_id
                    }
                    Err(err) => {
                        if let Some(tx) = ready_tx.take() {
                            // A session that dies on open is usually the agent
                            // not being signed in, and the wire error for that
                            // is unhelpfully generic ("Query closed before
                            // response received"). Lead with what the user can
                            // act on — the agent told us at initialize how it
                            // wants to be authenticated — and keep the wire
                            // text behind it.
                            let _ = tx.send(Err(session_open_error(
                                agent_label,
                                &init.auth_methods,
                                &err.to_string(),
                            )));
                        }
                        return Err(err);
                    }
                },
            };

            // Before "ready", so no prompt ever runs in the agent's own mode.
            // Every session, not only Home's: the rule is the app's, and the
            // notebook Agent pane is the same agent touching the same notes.
            set_ask_mode(&connection, kind, &session_id, modes.as_ref(), agent_label).await;

            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(Ok(()));
            }
            emit_state(
                &app,
                &notebook_id,
                &agent_id,
                "ready",
                Some(serde_json::json!({
                    "mcpAttached": mcp_attached,
                    "agent": serde_json::to_value(&init).ok(),
                    // Kept by the UI so this conversation can be resumed after
                    // a restart; `resumed` says whether the transcript on
                    // screen is about to be replaced by the agent's replay.
                    "sessionId": session_id.0.to_string(),
                    "resumed": was_resumed,
                })),
            );

            let mut first_prompt = true;
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    HostCmd::Stop => break,
                    HostCmd::Cancel => {} // nothing in flight
                    HostCmd::Prompt(text) => {
                        // Orientation rides the first prompt (see
                        // session_preamble); the transcript shows only what
                        // the user typed, since the UI echoes their text
                        // locally before it reaches us.
                        let text = if std::mem::take(&mut first_prompt) && mcp_attached {
                            let preamble = if is_home(&notebook_id) {
                                home_preamble()
                            } else {
                                session_preamble(&notebook_title, &notebook_id)
                            };
                            format!("{preamble}\n\n{text}")
                        } else {
                            text
                        };
                        emit_state(&app, &notebook_id, &agent_id, "turn", None);
                        let prompt = connection
                            .send_request(PromptRequest::new(
                                session_id.clone(),
                                vec![ContentBlock::Text(TextContent::new(text))],
                            ))
                            .block_task();
                        tokio::pin!(prompt);
                        let outcome = loop {
                            tokio::select! {
                                res = &mut prompt => break Some(res),
                                cmd = rx.recv() => match cmd {
                                    Some(HostCmd::Cancel) => {
                                        let _ = connection.send_notification(
                                            CancelNotification::new(session_id.clone()),
                                        );
                                    }
                                    Some(HostCmd::Stop) | None => break None,
                                    // One turn at a time; a prompt sent while
                                    // busy is dropped rather than queued.
                                    Some(HostCmd::Prompt(_)) => {}
                                },
                            }
                        };
                        match outcome {
                            None => break, // stopped mid-turn
                            Some(Err(err)) => {
                                emit_state(
                                    &app,
                                    &notebook_id,
                                    &agent_id,
                                    "error",
                                    Some(serde_json::Value::String(err.to_string())),
                                );
                            }
                            Some(Ok(resp)) => {
                                emit_state(
                                    &app,
                                    &notebook_id,
                                    &agent_id,
                                    "idle",
                                    serde_json::to_value(resp.stop_reason).ok(),
                                );
                            }
                        }
                    }
                }
            }
            Ok(())
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The "Open Terminal" fix can only run commands the allowlist accepts,
    /// so a login command that isn't on it is a dead button.
    #[test]
    fn every_login_command_is_allowlisted() {
        for kind in AGENTS {
            let cmd = kind.login_command();
            assert!(
                crate::commands::terminal_command_allowed(cmd),
                "{} login command {cmd:?} is not on the terminal allowlist",
                kind.label()
            );
        }
    }

    #[test]
    fn session_error_without_auth_methods_is_plain() {
        let none: [serde_json::Value; 0] = [];
        let msg = session_open_error("opencode", &none, "connection reset");
        assert_eq!(msg, "opencode couldn't open a session. connection reset");
    }

    #[test]
    fn session_error_leads_with_the_sign_in_hint() {
        let methods = [
            json!({"id": "claude-login", "name": "Log in with Claude Code"}),
            json!({"id": "api-key", "name": "Use an API key"}),
        ];
        let msg = session_open_error("Claude Code", &methods, "Query closed");
        // The actionable half comes before the wire text, which is what the
        // user actually reads first in the failure notice.
        assert!(
            msg.starts_with("Claude Code couldn't open a session"),
            "{msg}"
        );
        assert!(msg.contains("Log in with Claude Code"), "{msg}");
        assert!(msg.contains("Use an API key"), "{msg}");
        assert!(
            msg.find("sign in first").unwrap() < msg.find("Query closed").unwrap(),
            "wire detail should trail the hint: {msg}"
        );
    }

    /// The wire error is pretty-printed JSON; flattened it stays one readable
    /// line instead of sprawling down the notice.
    #[test]
    fn session_error_flattens_multiline_wire_text() {
        let none: [serde_json::Value; 0] = [];
        let msg = session_open_error(
            "Codex",
            &none,
            "Internal error: {\n  \"details\": \"Query closed\"\n}",
        );
        assert!(!msg.contains('\n'), "{msg}");
        assert!(
            msg.contains("Internal error: { \"details\": \"Query closed\" }"),
            "{msg}"
        );
    }

    #[test]
    fn auth_method_names_fall_back_to_description_then_id() {
        assert_eq!(
            auth_method_names(&[json!({"description": "Sign in via browser"})]),
            ["Sign in via browser"]
        );
        assert_eq!(
            auth_method_names(&[json!({"id": "opaque-method"})]),
            ["opaque-method"]
        );
        // Nothing human-readable at all: skipped, not rendered as "null".
        assert!(auth_method_names(&[json!({"type": "oauth"})]).is_empty());
    }
}

#[cfg(test)]
mod home_tests {
    use super::*;

    /// A Home key is recognized by its prefix and nothing else: a notebook
    /// whose id merely contains "home" must keep its notebook preamble.
    #[test]
    fn home_keys_are_prefixed() {
        assert!(is_home(&format!("{HOME_SCOPE}thread-1")));
        assert!(!is_home("nb-home-improvement"));
        assert!(!is_home(""));
    }

    /// The chat provider's kind IS the ACP agent id. If either side renames
    /// one, Home silently falls back to the loop for that agent; this pins
    /// the three that have to line up.
    #[test]
    fn provider_kinds_are_acp_ids() {
        for id in ["claude-code", "codex", "opencode"] {
            let kind = AcpAgentKind::from_id(id).expect(id);
            assert_eq!(kind.id(), id);
        }
        // Agent CLIs with no ACP adapter stay on the loop.
        assert!(AcpAgentKind::from_id("copilot").is_none());
        assert!(AcpAgentKind::from_id("gateway").is_none());
    }

    /// The rule, against the request shapes each adapter actually sends
    /// (claude-agent-acp 0.85.0, codex-acp 2.1.1): Alchemy's reads are
    /// allowed without asking; writes, other servers' tools and anything
    /// unnamed go to the user.
    #[test]
    fn alchemy_reads_are_allowed_writes_ask() {
        let claude_opts = vec![
            ("allow-once".to_string(), PermissionOptionKind::AllowOnce),
            (
                "allow-with-updates".to_string(),
                PermissionOptionKind::AllowAlways,
            ),
            ("reject".to_string(), PermissionOptionKind::RejectOnce),
        ];
        // Claude Code: the name in _meta and the title.
        let read = [
            Some("mcp__alchemy__search".into()),
            Some("mcp__alchemy__search".into()),
            None,
        ];
        assert_eq!(
            auto_allow(&read, &claude_opts).as_deref(),
            Some("allow-once")
        );
        let write = [Some("mcp__alchemy__create_note".into()), None, None];
        assert_eq!(auto_allow(&write, &claude_opts), None);
        // Codex: the request names nothing; the cached tool_call title does.
        let codex = [None, None, Some("mcp.alchemy.list_notebooks".into())];
        assert_eq!(
            auto_allow(&codex, &claude_opts).as_deref(),
            Some("allow-once")
        );
        // Another server's read-looking tool, a shell command, nothing at all.
        let other = [Some("mcp__github__search".into()), None, None];
        assert_eq!(auto_allow(&other, &claude_opts), None);
        assert_eq!(
            auto_allow(&[Some("Run command?".into()), None, None], &claude_opts),
            None
        );
        assert_eq!(auto_allow(&[None, None, None], &claude_opts), None);
        // Never "always": with no allow-once offered, the user decides.
        let always_only = vec![("a".to_string(), PermissionOptionKind::AllowAlways)];
        assert_eq!(auto_allow(&read, &always_only), None);
    }

    /// "Always" answers would move the decision into the agent's own
    /// settings, so the user is offered this call only — unless an agent
    /// offers nothing else, in which case its options stand.
    #[test]
    fn only_per_call_answers_are_offered() {
        use agent_client_protocol::schema::v1::PermissionOption;
        let opt = |id: &'static str, kind| PermissionOption::new(id, id, kind);
        // claude-agent-acp's set for create_note, as seen live.
        let claude = vec![
            opt("allow-once", PermissionOptionKind::AllowOnce),
            opt("allow-with-updates", PermissionOptionKind::AllowAlways),
            opt("reject", PermissionOptionKind::RejectOnce),
        ];
        let shown: Vec<_> = per_call_options(&claude)
            .iter()
            .map(|o| o.option_id.0.to_string())
            .collect();
        assert_eq!(shown, vec!["allow-once", "reject"]);
        let only_always = vec![opt("always", PermissionOptionKind::AllowAlways)];
        assert_eq!(per_call_options(&only_always).len(), 1);
    }

    /// The asking modes, by the ids each adapter advertises. opencode has
    /// none and keeps its own rules — pinned so that changes on purpose.
    #[test]
    fn asking_modes_per_agent() {
        assert_eq!(AcpAgentKind::ClaudeCode.ask_mode(), Some("default"));
        assert_eq!(AcpAgentKind::Codex.ask_mode(), Some("workspace-write"));
        assert_eq!(AcpAgentKind::Opencode.ask_mode(), None);
    }

    /// Home's brain is only an agent Alchemy can govern: opencode, with no
    /// asking mode, falls back to the loop.
    #[test]
    fn only_governable_agents_answer_home() {
        assert!(AcpAgentKind::ClaudeCode.can_answer_home());
        assert!(AcpAgentKind::Codex.can_answer_home());
        assert!(!AcpAgentKind::Opencode.can_answer_home());
    }

    /// The Home preamble points at the corpus-wide tools and holds writes to
    /// the user's word, mirroring the loop's license rule.
    #[test]
    fn home_preamble_is_corpus_wide() {
        let p = home_preamble();
        assert!(p.contains("ask_everything"));
        assert!(p.contains("list_notebooks"));
        assert!(p.contains("Only add, change or delete anything when the user asks"));
    }
}
