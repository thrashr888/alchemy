//! Hosted agents over the Agent Client Protocol (docs/RFC-acp-agents.md).
//!
//! Spawns the user's installed coding agent (opencode, Claude Code, Gemini,
//! Codex) as an ACP subprocess, hands it Alchemy's own MCP endpoint at
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
    CancelNotification, ContentBlock, InitializeRequest, McpServer, McpServerHttp,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, TextContent,
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
const AGENTS: [AcpAgentKind; 4] = [
    AcpAgentKind::Opencode,
    AcpAgentKind::ClaudeCode,
    AcpAgentKind::Gemini,
    AcpAgentKind::Codex,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum AcpAgentKind {
    Opencode,
    ClaudeCode,
    Gemini,
    Codex,
}

impl AcpAgentKind {
    fn id(self) -> &'static str {
        match self {
            AcpAgentKind::Opencode => "opencode",
            AcpAgentKind::ClaudeCode => "claude-code",
            AcpAgentKind::Gemini => "gemini-cli",
            AcpAgentKind::Codex => "codex",
        }
    }

    fn label(self) -> &'static str {
        match self {
            AcpAgentKind::Opencode => "opencode",
            AcpAgentKind::ClaudeCode => "Claude Code",
            AcpAgentKind::Gemini => "Gemini CLI",
            AcpAgentKind::Codex => "Codex",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        AGENTS.into_iter().find(|k| k.id() == id)
    }

    /// Launch config, or None when the required binaries aren't installed.
    /// The child inherits the login-shell env (GUI apps don't get dotfile
    /// PATH/auth), with provider API keys stripped so the CLI's own login is
    /// the credential — same scar as the headless providers.
    fn launch(self) -> Option<AcpAgentConfig> {
        let config = match self {
            AcpAgentKind::Opencode => {
                AcpAgentConfig::new(find_binary_cached("opencode")?).arg("acp")
            }
            AcpAgentKind::ClaudeCode => {
                // The adapter drives the user's `claude` install; require it so
                // we don't offer an agent that can't authenticate.
                find_binary_cached("claude")?;
                AcpAgentConfig::new(find_binary_cached("npx")?)
                    .arg("-y")
                    .arg("@zed-industries/claude-code-acp")
            }
            AcpAgentKind::Gemini => {
                AcpAgentConfig::new(find_binary_cached("gemini")?).arg("--experimental-acp")
            }
            AcpAgentKind::Codex => AcpAgentConfig::new(find_binary_cached("codex")?).arg("acp"),
        };
        Some(config.envs(load_shell_env()))
    }
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
}

/// Detected ACP agents for the picker. Availability is the cached binary
/// probe — cheap enough for every Settings/composer open.
#[tauri::command]
pub fn acp_agents() -> Vec<AcpAgentInfo> {
    AGENTS
        .into_iter()
        .map(|kind| AcpAgentInfo {
            id: kind.id().to_string(),
            label: kind.label().to_string(),
            available: kind.launch().is_some(),
        })
        .collect()
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
) -> Result<(), String> {
    let kind =
        AcpAgentKind::from_id(&agent_id).ok_or_else(|| format!("unknown agent: {agent_id}"))?;
    let config = kind
        .launch()
        .ok_or_else(|| format!("{} is not installed", kind.label()))?;

    let state = app.state::<AcpState>();
    let (tx, rx) = mpsc::unbounded_channel();
    let permissions: Arc<Mutex<HashMap<String, PendingPermission>>> = Arc::default();
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
            task_agent.clone(),
            config,
            rx,
            ready_tx,
            permissions,
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
    send_cmd(&app, &notebook_id, HostCmd::Prompt(text))
}

/// Cancel the in-flight turn (session/cancel); the session stays alive.
#[tauri::command]
pub fn acp_cancel(app: AppHandle, notebook_id: String) -> Result<(), String> {
    send_cmd(&app, &notebook_id, HostCmd::Cancel)
}

/// End the notebook's hosted session and reap the agent subprocess.
#[tauri::command]
pub fn acp_stop(app: AppHandle, notebook_id: String) -> Result<(), String> {
    send_cmd(&app, &notebook_id, HostCmd::Stop)
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

async fn run_session(
    app: AppHandle,
    notebook_id: String,
    agent_id: String,
    config: AcpAgentConfig,
    mut rx: mpsc::UnboundedReceiver<HostCmd>,
    ready_tx: oneshot::Sender<Result<(), String>>,
    permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
) -> Result<(), agent_client_protocol::Error> {
    let agent = AcpAgent::new(config);

    let update_app = app.clone();
    let update_notebook = notebook_id.clone();
    let perm_app = app.clone();
    let perm_notebook = notebook_id.clone();

    let mut ready_tx = Some(ready_tx);
    let cwd = session_cwd(&app, &notebook_id);
    let mcp = crate::mcp::status(&app);

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                let update =
                    serde_json::to_value(&notification.update).unwrap_or(serde_json::Value::Null);
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
                let request_id = uuid::Uuid::new_v4().to_string();
                let options = request
                    .options
                    .iter()
                    .map(|o| PermissionOptionInfo {
                        id: o.option_id.0.to_string(),
                        name: o.name.clone(),
                        kind: serde_json::to_value(o.kind)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_default(),
                    })
                    .collect();
                let tool_title = request.tool_call.fields.title.clone().unwrap_or_default();
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
                        let _ = tx.send(Err(format!("agent failed to initialize: {err}")));
                    }
                    return Err(err);
                }
            };

            let mut session_req = NewSessionRequest::new(cwd);
            if mcp.running && init.agent_capabilities.mcp_capabilities.http {
                session_req = session_req.mcp_servers(vec![McpServer::Http(McpServerHttp::new(
                    "alchemy",
                    mcp.url.clone(),
                ))]);
            }
            let session = match connection.send_request(session_req).block_task().await {
                Ok(s) => s,
                Err(err) => {
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(Err(format!("agent failed to open a session: {err}")));
                    }
                    return Err(err);
                }
            };
            let session_id = session.session_id.clone();

            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(Ok(()));
            }
            emit_state(
                &app,
                &notebook_id,
                &agent_id,
                "ready",
                serde_json::to_value(&init).ok(),
            );

            while let Some(cmd) = rx.recv().await {
                match cmd {
                    HostCmd::Stop => break,
                    HostCmd::Cancel => {} // nothing in flight
                    HostCmd::Prompt(text) => {
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
