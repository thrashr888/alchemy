//! Agent-CLI chat engines (RFC-inference-providers §5, family B): the
//! vendor's own CLI carries the subscription — claude (Max) and codex
//! (ChatGPT Pro) run headless, one process per message, speaking their
//! structured event streams. Never a terminal.
//!
//! The bootstrap mechanics are ported from Paul's shipped wrappers (audited
//! 2026-07-20): binary discovery + login-shell env from Argos
//! (crates/argos-core/src/claude_cli.rs) with its zombie gap fixed
//! (`kill_on_drop`), event handling shaped like tradr's
//! (app/src-tauri/src/commands/agent.rs) — stderr drained, errors don't
//! terminate the read loop early, deltas stream as they arrive.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::{ChatOutcome, ChatTurn};

/// A CLI that has printed NOTHING for this long is wedged at startup — a
/// healthy one banners within seconds (codex emits thread.started before
/// its model runs). Observed live: the Morning Brief burned a whole
/// 600-second budget on codex's MCP handshake into a CPU-pinned process,
/// before any model call happened.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
/// Mid-run silence cap: ten minutes of nothing after output began is a
/// hang, not thinking.
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// Ceiling for an actively-streaming run — agentic briefs tool-loop and
/// legitimately run long.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(1800);
/// Backstop on the whole future, over and above the per-read deadlines —
/// only a bug in the deadline logic could ever reach it.
const RUN_BACKSTOP: Duration = Duration::from_secs(1860);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
    Gemini,
    Cursor,
    Opencode,
    Copilot,
    Hermes,
    /// IBM Bob via its own bobshell CLI — the sanctioned client itself, not a
    /// session workaround (Paul's call, 2026-07-20; API-key/session mimicry
    /// stays out per policy). Known wart: `bob -p` prints its thinking
    /// before the answer, and v1 passes output through as-is.
    Bob,
    /// Prime Intellect's prime-agent. Flags per its docs (json.md/usage.md,
    /// read 2026-08-09); verified live same day against local Ollama.
    Prime,
    /// Mario Zechner's pi (earendil-works/pi) — prime-agent's upstream.
    /// Identical JSONL event protocol (--mode json), so the two share an
    /// invocation and parse arm.
    Pi,
}

impl AgentKind {
    pub const ALL: [AgentKind; 10] = [
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Gemini,
        AgentKind::Cursor,
        AgentKind::Opencode,
        AgentKind::Copilot,
        AgentKind::Hermes,
        AgentKind::Bob,
        AgentKind::Prime,
        AgentKind::Pi,
    ];

    pub fn binary_name(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Gemini => "gemini",
            AgentKind::Cursor => "cursor-agent",
            AgentKind::Opencode => "opencode",
            AgentKind::Copilot => "copilot",
            AgentKind::Hermes => "hermes",
            AgentKind::Bob => "bob",
            AgentKind::Prime => "prime-agent",
            AgentKind::Pi => "pi",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::Gemini => "gemini-cli",
            AgentKind::Cursor => "cursor-cli",
            AgentKind::Opencode => "opencode",
            AgentKind::Copilot => "copilot",
            AgentKind::Hermes => "hermes",
            AgentKind::Bob => "bob-shell",
            AgentKind::Prime => "prime-agent",
            AgentKind::Pi => "pi",
        }
    }

    pub fn from_id(id: &str) -> Option<AgentKind> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn label(&self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::Gemini => "Gemini CLI",
            AgentKind::Cursor => "Cursor CLI",
            AgentKind::Opencode => "OpenCode",
            AgentKind::Copilot => "GitHub Copilot",
            AgentKind::Hermes => "Hermes",
            AgentKind::Bob => "Bob Shell",
            AgentKind::Prime => "Prime Agent",
            AgentKind::Pi => "Pi",
        }
    }

    pub fn install_hint(&self) -> &'static str {
        match self {
            AgentKind::Claude => "npm install -g @anthropic-ai/claude-code",
            AgentKind::Codex => "npm install -g @openai/codex",
            AgentKind::Gemini => "npm install -g @google/gemini-cli",
            AgentKind::Cursor => "curl https://cursor.com/install -fsS | bash",
            AgentKind::Opencode => "brew install sst/tap/opencode",
            AgentKind::Copilot => "npm install -g @github/copilot",
            AgentKind::Hermes => "pipx install hermes-agent",
            AgentKind::Bob => "curl -fsSL https://bob.ibm.com/download/bobshell.sh | sh",
            AgentKind::Prime => {
                "curl -fsSL https://app.primeintellect.ai/prime-agent/install.sh | sh"
            }
            AgentKind::Pi => "npm install -g --ignore-scripts @earendil-works/pi-coding-agent",
        }
    }
}

/// The user's login-shell environment. macOS GUI apps don't inherit dotfile
/// exports, so PATH additions and auth land only in a login shell — the
/// Argos/tradr pattern, copied verbatim in spirit.
fn load_shell_env() -> HashMap<String, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let out = std::process::Command::new(&shell)
        .args(["-l", "-c", "env"])
        .output();
    let mut env: HashMap<String, String> = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        _ => std::env::vars().collect(),
    };
    // Both repos agree, with scars: a stray API key makes the CLI bill the
    // key instead of the subscription (or conflict with its OAuth session).
    // The CLI's own login is the credential — always.
    env.remove("ANTHROPIC_API_KEY");
    env.remove("OPENAI_API_KEY");
    env.remove("GEMINI_API_KEY");
    env.remove("GOOGLE_API_KEY");
    env.remove("CURSOR_API_KEY");
    env.remove("BOBSHELL_API_KEY");
    env.remove("PRIME_API_KEY");
    env
}

/// Process-lifetime discovery cache. Engine construction happens under the
/// config write lock on every save; the login-shell `which` fallback costs
/// seconds per missing CLI, so construction reads this cache and only the
/// explicit status probe (`agent_status`) pays for a fresh look.
fn binary_cache() -> &'static std::sync::Mutex<HashMap<String, Option<PathBuf>>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Option<PathBuf>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn find_binary_cached(name: &str) -> Option<PathBuf> {
    if let Some(hit) = binary_cache().lock().unwrap().get(name) {
        return hit.clone();
    }
    let found = find_binary(name);
    binary_cache()
        .lock()
        .unwrap()
        .insert(name.to_string(), found.clone());
    found
}

/// Find the CLI: well-known install dirs first, then the login shell's
/// `which` (slow path). Argos's discovery order.
fn find_binary(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    for dir in [
        format!("{home}/.local/bin"),
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
    ] {
        let p = PathBuf::from(dir).join(name);
        if p.exists() {
            return Some(p);
        }
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let out = std::process::Command::new(shell)
        .args(["-l", "-c", &format!("which {name}")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Availability for the settings tiles: (installed, version-or-hint).
/// Always a fresh look — a user who just installed a CLI should see it —
/// and the result refreshes the construction cache.
pub fn agent_status(kind: AgentKind) -> (bool, String) {
    let fresh = find_binary(kind.binary_name());
    binary_cache()
        .lock()
        .unwrap()
        .insert(kind.binary_name().to_string(), fresh.clone());
    match fresh {
        Some(bin) => {
            let version = std::process::Command::new(&bin)
                .arg("--version")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|| "installed".to_string());
            (true, version)
        }
        None => (false, format!("Install with: {}", kind.install_hint())),
    }
}

/// When an agent CLI fails in an auth-shaped way, append the one command
/// that fixes it — expired OAuth needs the vendor's own interactive login
/// (a browser round-trip we can't do headlessly), so the error row must be
/// the instruction sheet.
fn auth_fix_hint(kind: AgentKind, error_text: &str) -> Option<String> {
    let t = error_text.to_lowercase();
    let authish = [
        "oauth",
        "authenticat",
        "unauthorized",
        "signed out",
        "not logged in",
        "login",
        "log in",
        "credential",
        "api key",
        "session expired",
        "401",
    ]
    .iter()
    .any(|w| t.contains(w));
    if !authish {
        return None;
    }
    let fix = match kind {
        AgentKind::Claude => "run `claude` and let it refresh your sign-in",
        AgentKind::Codex => "run `codex login`",
        AgentKind::Gemini => "run `gemini` and choose “Log in with Google”",
        AgentKind::Cursor => "run `cursor-agent login`",
        AgentKind::Opencode => "run `opencode auth login`",
        AgentKind::Copilot => "run `copilot` and follow its sign-in",
        AgentKind::Hermes => "run `hermes` and follow its sign-in",
        AgentKind::Bob => "run `bob` and follow its sign-in",
        AgentKind::Prime => "run `prime-agent` and sign in with its /login command",
        AgentKind::Pi => "run `pi` and sign in with its /login command",
    };
    Some(format!("Fix: open Terminal, {fix}, then retry here."))
}

/// A vendor gateway rejecting the CLI's saved default model ("The requested
/// model is not supported" / `model_not_supported`) means the CLI is pinned
/// to a retired model — seen live with GitHub Copilot. The fix is the CLI's,
/// not ours: update it, or re-pick a model in its own UI.
fn model_fix_hint(kind: AgentKind, error_text: &str) -> Option<String> {
    let t = error_text.to_lowercase();
    if !(t.contains("model_not_supported") || t.contains("model is not supported")) {
        return None;
    }
    let bin = kind.binary_name();
    Some(format!(
        "Fix: the {} CLI's saved model has been retired — update the CLI \
         (`{}`), or run `{bin}` and choose a current model (usually the \
         /model command), then retry here.",
        kind.label(),
        kind.install_hint()
    ))
}

/// "mcp__alchemy__search_notebook" → "Using search notebook": the last
/// path segment of a namespaced tool id, de-snaked, as a progress line.
fn tool_step_label(name: &str) -> String {
    let last = name.rsplit("__").next().unwrap_or(name);
    format!("Using {}", last.replace('_', " "))
}

fn fold_system(system: &str, prompt: &str) -> String {
    if system.is_empty() {
        prompt.to_string()
    } else {
        format!("{system}\n\n---\n\n{prompt}")
    }
}

#[derive(Clone)]
pub struct AgentCli {
    kind: AgentKind,
    binary: Option<PathBuf>,
}

impl AgentCli {
    pub fn new(kind: AgentKind) -> Self {
        Self {
            kind,
            binary: find_binary_cached(kind.binary_name()),
        }
    }

    pub fn kind(&self) -> AgentKind {
        self.kind
    }

    /// v1 session stance (RFC §5): one process per message, context replayed
    /// in the prompt — the Argos lifecycle with streaming output.
    fn build_prompt(&self, messages: &[ChatTurn]) -> (String, String) {
        let system = messages
            .iter()
            .filter(|t| t.role == "system")
            .map(|t| t.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let convo: Vec<&ChatTurn> = messages.iter().filter(|t| t.role != "system").collect();
        let prompt = if convo.len() == 1 {
            convo[0].content.clone()
        } else {
            let mut p = String::new();
            for t in &convo {
                p.push_str(if t.role == "assistant" {
                    "Assistant: "
                } else {
                    "User: "
                });
                p.push_str(&t.content);
                p.push_str("\n\n");
            }
            p.push_str("Assistant:");
            p
        };
        (system, prompt)
    }

    pub async fn chat_stream<F>(&self, messages: &[ChatTurn], on_token: F) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
    {
        self.chat_stream_steps(messages, on_token, |_| {}).await
    }

    /// `chat_stream` plus progress: agent CLIs sit silent for long stretches
    /// while they plan and run tools, so their structured events double as
    /// in-progress status lines (`on_step`) instead of a mute spinner.
    pub async fn chat_stream_steps<F, S>(
        &self,
        messages: &[ChatTurn],
        on_token: F,
        on_step: S,
    ) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
        S: FnMut(&str),
    {
        let bin = self.binary.as_ref().ok_or_else(|| {
            anyhow!(
                "{} CLI not found. {}",
                self.kind.binary_name(),
                self.kind.install_hint()
            )
        })?;
        let (system, prompt) = self.build_prompt(messages);
        let env = tokio::task::spawn_blocking(load_shell_env)
            .await
            .unwrap_or_default();

        let mut cmd = tokio::process::Command::new(bin);
        cmd.env_clear().envs(&env);
        match self.kind {
            AgentKind::Claude => {
                // Streamed structured events; tools restricted to Alchemy's
                // own MCP server (the agent grounds itself in the notebook —
                // never the filesystem). --verbose is required for
                // stream-json; partial messages give per-token deltas.
                cmd.args([
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--include-partial-messages",
                    "--allowedTools",
                    "mcp__alchemy__*",
                ]);
                if !system.is_empty() {
                    cmd.args(["--append-system-prompt", &system]);
                }
                // Prompt over stdin, not argv: stuffed retrieval contexts
                // can exceed ARG_MAX.
            }
            AgentKind::Codex => {
                // codex exec has no system flag: fold instructions into the
                // prompt. JSON mode emits item-level events (no token
                // deltas) — text arrives in item.completed chunks.
                let full = if system.is_empty() {
                    prompt.clone()
                } else {
                    format!("{system}\n\n---\n\n{prompt}")
                };
                // --skip-git-repo-check: bundled apps run outside any repo
                // and codex refuses non-repo cwds without it.
                cmd.args(["exec", "--json", "--skip-git-repo-check", &full]);
            }
            AgentKind::Cursor => {
                // cursor-agent print mode speaks claude-shaped stream-json;
                // the lenient parser treats non-JSON lines as raw text so
                // plain-text builds still work. No system flag — folded
                // into the prompt below. Prompt over stdin.
                cmd.args(["-p", "--output-format", "stream-json"]);
            }
            AgentKind::Gemini => {
                // Plain-text CLI reading the prompt from stdin; stdout
                // chunks stream through as tokens. No system flag — folded.
            }
            AgentKind::Opencode => {
                // Verified live: `run --format json` emits step_start / text
                // / step_finish events; text parts carry the reply. Prompt
                // is positional (argv-guarded below).
                let full = fold_system(&system, &prompt);
                if full.len() > 150_000 {
                    return Err(anyhow!(
                        "context too large for opencode's argv-based prompt"
                    ));
                }
                cmd.args(["run", "--format", "json", &full]);
            }
            AgentKind::Copilot => {
                // Best-known programmatic flags; unverifiable until installed
                // (detection gates selection). Plain-text output parse.
                let full = fold_system(&system, &prompt);
                if full.len() > 150_000 {
                    return Err(anyhow!("context too large for copilot's argv-based prompt"));
                }
                cmd.args(["-p", &full]);
            }
            AgentKind::Hermes => {
                // Verified live: `hermes -z <prompt>` prints the reply as
                // plain text.
                let full = fold_system(&system, &prompt);
                if full.len() > 150_000 {
                    return Err(anyhow!("context too large for hermes's argv-based prompt"));
                }
                cmd.args(["-z", &full]);
            }
            AgentKind::Bob => {
                // bobshell takes -p <prompt> as argv (no stdin mode known);
                // guard oversized stuffed contexts against ARG_MAX.
                let full = fold_system(&system, &prompt);
                if full.len() > 150_000 {
                    return Err(anyhow!(
                        "context too large for bob's argv-based prompt — \
                         trim source selection or use another provider"
                    ));
                }
                cmd.args(["-p", &full]);
            }
            AgentKind::Prime | AgentKind::Pi => {
                // pi / prime-agent --mode json: the same structured JSONL
                // protocol (prime forked pi; both docs/json.md agree).
                // Prompt is positional (argv-guarded); --append-system-prompt
                // is a real flag in both.
                if system.len() + prompt.len() > 150_000 {
                    return Err(anyhow!(
                        "context too large for {}'s argv-based prompt",
                        self.kind.binary_name()
                    ));
                }
                cmd.args(["--mode", "json"]);
                if !system.is_empty() {
                    cmd.args(["--append-system-prompt", &system]);
                }
                cmd.arg(&prompt);
            }
        }
        let stdin_payload = match self.kind {
            AgentKind::Claude => Some(prompt.clone()),
            AgentKind::Cursor | AgentKind::Gemini => Some(fold_system(&system, &prompt)),
            AgentKind::Codex
            | AgentKind::Bob
            | AgentKind::Opencode
            | AgentKind::Copilot
            | AgentKind::Hermes
            | AgentKind::Prime
            | AgentKind::Pi => None,
        };
        cmd.stdin(if stdin_payload.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn {}", self.kind.binary_name()))?;
        if let Some(payload) = stdin_payload {
            let mut si = child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("no agent stdin"))?;
            si.write_all(payload.as_bytes()).await?;
            drop(si);
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("no agent stdout"))?;
        // Drain stderr on a task so a chatty CLI can't deadlock the pipe
        // (tradr's scar); keep the tail for error messages.
        let stderr = child.stderr.take();
        let err_tail = tokio::spawn(async move {
            // Vendors put the cause on the first line and decoration after
            // ("must specify GEMINI_API_KEY" … "Update your environment…"),
            // so keep first + last non-empty lines, not just the last.
            let mut first = String::new();
            let mut last = String::new();
            if let Some(e) = stderr {
                let mut lines = BufReader::new(e).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    if l.trim().is_empty() {
                        continue;
                    }
                    if first.is_empty() {
                        first = l.clone();
                    }
                    last = l;
                }
            }
            if last == first {
                first
            } else if first.is_empty() {
                last
            } else {
                format!("{first} — {last}")
            }
        });

        fn strip_thinking(line: &str, in_thinking: &mut bool) -> Option<String> {
            // Stateful across lines: `<thinking>` opens, `</thinking>` closes,
            // tags may share a line with kept text. Lines that were entirely
            // reasoning vanish; genuine blank lines in the answer survive.
            let mut out = String::new();
            let mut rest = line;
            loop {
                if *in_thinking {
                    match rest.find("</thinking>") {
                        Some(i) => {
                            rest = &rest[i + "</thinking>".len()..];
                            *in_thinking = false;
                        }
                        None => break,
                    }
                } else {
                    match rest.find("<thinking>") {
                        Some(i) => {
                            out.push_str(&rest[..i]);
                            rest = &rest[i + "<thinking>".len()..];
                            *in_thinking = true;
                        }
                        None => {
                            out.push_str(rest);
                            break;
                        }
                    }
                }
            }
            if out.trim().is_empty() && (*in_thinking || line != out) {
                None
            } else {
                Some(out)
            }
        }

        let kind = self.kind;
        let run = async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut on_token = on_token;
            let mut on_step = on_step;
            // Consecutive-duplicate guard: event streams repeat themselves
            // (several deltas per tool call), and the step trail shouldn't.
            let mut last_step = String::new();
            let mut emit_step = move |label: String| {
                if label != last_step {
                    on_step(&label);
                    last_step = label;
                }
            };
            // Immediate first line — the CLI takes seconds just to boot.
            emit_step(format!("Asking {}", kind.label()));
            let mut text = String::new();
            let mut errored: Option<String> = None;
            let mut cost_usd: Option<f64> = None;
            let mut in_thinking = false;
            // Staged deadlines, not one flat cap: a silent START is a wedged
            // handshake and fails fast; once streaming, the run gets real
            // room, bounded by a mid-run silence cap and a total ceiling.
            let started = std::time::Instant::now();
            let mut saw_output = false;
            while let Some(line) = {
                let stage = if saw_output {
                    IDLE_TIMEOUT
                } else {
                    STARTUP_TIMEOUT
                };
                let left = TOTAL_TIMEOUT
                    .saturating_sub(started.elapsed())
                    .min(stage)
                    .max(Duration::from_millis(1));
                match tokio::time::timeout(left, lines.next_line()).await {
                    Ok(l) => l?,
                    Err(_) => {
                        let (what, secs) = if !saw_output {
                            ("produced no output", STARTUP_TIMEOUT.as_secs())
                        } else if started.elapsed() >= TOTAL_TIMEOUT {
                            ("exceeded the run ceiling of", TOTAL_TIMEOUT.as_secs())
                        } else {
                            ("went silent mid-run for", IDLE_TIMEOUT.as_secs())
                        };
                        return Err(anyhow!("{} {what} {secs}s", kind.binary_name()));
                    }
                }
            } {
                saw_output = true;
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    // Plain-text CLIs (gemini, bob) and stream-json builds
                    // that print bare text: pass the line through verbatim.
                    if matches!(
                        kind,
                        AgentKind::Gemini
                            | AgentKind::Bob
                            | AgentKind::Cursor
                            | AgentKind::Copilot
                            | AgentKind::Hermes
                    ) {
                        // bobshell prints its reasoning as a <thinking> block
                        // and wraps the reply in tool-call scaffolding
                        // ("[using tool …]", "---output---" fences); all of
                        // that is process, not reply.
                        let kept = if kind == AgentKind::Bob {
                            let t = line.trim();
                            if let Some(tool) = t.strip_prefix("[using tool ") {
                                // Scaffolding, not reply — but it IS status.
                                let name = tool.trim_end_matches(']').trim();
                                emit_step(format!("Using {name}"));
                                continue;
                            }
                            if t == "---output---" {
                                continue;
                            }
                            match strip_thinking(&line, &mut in_thinking) {
                                Some(k) => k,
                                None => continue,
                            }
                        } else {
                            line
                        };
                        if !text.is_empty() {
                            text.push('\n');
                            on_token("\n");
                        }
                        text.push_str(&kept);
                        on_token(&kept);
                    }
                    continue;
                };
                match kind {
                    AgentKind::Opencode => {
                        // Verified live: text parts stream the reply; step
                        // and tool events narrate the in-between.
                        match v["type"].as_str() {
                            Some("text") => {
                                if let Some(t) = v["part"]["text"].as_str() {
                                    text.push_str(t);
                                    on_token(t);
                                }
                            }
                            Some("step_start") => emit_step("Thinking".into()),
                            Some("tool") => {
                                let name = v["part"]["tool"].as_str().unwrap_or("a tool");
                                emit_step(tool_step_label(name));
                            }
                            _ => {}
                        }
                    }
                    AgentKind::Gemini | AgentKind::Bob | AgentKind::Copilot | AgentKind::Hermes => {
                        // JSON on stdout from a plain-text CLI is unexpected;
                        // stringify it into the transcript rather than drop.
                        let t = v.to_string();
                        text.push_str(&t);
                        on_token(&t);
                    }
                    AgentKind::Claude | AgentKind::Cursor => match v["type"].as_str() {
                        // Per-token deltas from --include-partial-messages;
                        // tool-use block starts double as progress lines.
                        Some("stream_event") => {
                            if let Some(delta) = v["event"]["delta"]["text"].as_str() {
                                text.push_str(delta);
                                on_token(delta);
                            } else if v["event"]["content_block"]["type"].as_str()
                                == Some("tool_use")
                            {
                                let name = v["event"]["content_block"]["name"]
                                    .as_str()
                                    .unwrap_or("a tool");
                                emit_step(tool_step_label(name));
                            }
                        }
                        // Full assistant turns; authoritative when partial
                        // events were absent (older CLI versions).
                        Some("assistant") => {
                            // Text is authoritative only when no partial
                            // events streamed it already (older CLIs).
                            let streamed_already = !text.is_empty();
                            if let Some(blocks) = v["message"]["content"].as_array() {
                                for b in blocks {
                                    if b["type"].as_str() == Some("tool_use") {
                                        let name = b["name"].as_str().unwrap_or("a tool");
                                        emit_step(tool_step_label(name));
                                    } else if let Some(t) = b["text"].as_str() {
                                        if !streamed_already {
                                            text.push_str(t);
                                            on_token(t);
                                        }
                                    }
                                }
                            }
                        }
                        Some("result") => {
                            cost_usd = cost_usd.or_else(|| v["total_cost_usd"].as_f64());
                            if v["is_error"].as_bool() == Some(true) {
                                let msg = v["result"].as_str().unwrap_or("agent error");
                                errored = Some(msg.to_string());
                            } else if text.is_empty() {
                                if let Some(t) = v["result"].as_str() {
                                    text.push_str(t);
                                    on_token(t);
                                }
                            }
                        }
                        _ => {}
                    },
                    AgentKind::Prime | AgentKind::Pi => {
                        // pi / prime-agent --mode json (docs/json.md, same
                        // protocol): message_update streams text deltas,
                        // tool_execution_start narrates the work, message_end
                        // is authoritative only when no deltas arrived.
                        // Thinking never streams as text_delta, so it stays
                        // out of the transcript by construction.
                        match v["type"].as_str() {
                            Some("message_update") => {
                                let ev = &v["assistantMessageEvent"];
                                if ev["type"].as_str() == Some("text_delta") {
                                    if let Some(d) = ev["delta"].as_str() {
                                        text.push_str(d);
                                        on_token(d);
                                    }
                                }
                            }
                            Some("message_end") if text.is_empty() => {
                                if v["message"]["role"].as_str() == Some("assistant") {
                                    if let Some(blocks) = v["message"]["content"].as_array() {
                                        for b in blocks {
                                            if b["type"].as_str() == Some("text") {
                                                if let Some(t) = b["text"].as_str() {
                                                    text.push_str(t);
                                                    on_token(t);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Some("tool_execution_start") => {
                                let name = v["toolName"].as_str().unwrap_or("a tool");
                                emit_step(tool_step_label(name));
                            }
                            Some("auto_retry_start") => emit_step("Retrying".into()),
                            Some("auto_retry_end") if v["success"].as_bool() == Some(false) => {
                                if let Some(msg) = v["finalError"].as_str() {
                                    errored = Some(msg.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                    AgentKind::Codex => {
                        // codex exec --json: items complete whole; the
                        // agent_message item carries the reply text, and
                        // item.started events narrate the work in between.
                        if v["type"].as_str() == Some("item.completed")
                            && v["item"]["type"].as_str() == Some("agent_message")
                        {
                            if let Some(t) = v["item"]["text"].as_str() {
                                text.push_str(t);
                                on_token(t);
                            }
                        } else if v["type"].as_str() == Some("item.started") {
                            let label = match v["item"]["type"].as_str() {
                                Some("reasoning") => "Thinking".to_string(),
                                Some("command_execution") => "Running a command".to_string(),
                                Some("web_search") => "Searching the web".to_string(),
                                Some("mcp_tool_call") => {
                                    tool_step_label(v["item"]["tool"].as_str().unwrap_or("a tool"))
                                }
                                Some(other) => format!("Working: {}", other.replace('_', " ")),
                                None => "Working".to_string(),
                            };
                            emit_step(label);
                        } else if v["type"].as_str() == Some("error") {
                            errored =
                                Some(v["message"].as_str().unwrap_or("codex error").to_string());
                        }
                    }
                }
            }
            match errored {
                // tradr's scar: an error event may still be followed by more
                // lines — only decide after the stream closes.
                Some(msg) if text.is_empty() => Err(anyhow!("{msg}")),
                _ if text.is_empty() => Err(anyhow!("agent produced no output")),
                _ => Ok(ChatOutcome {
                    text,
                    stats: None,
                    cost_usd,
                }),
            }
        };

        let outcome = tokio::time::timeout(RUN_BACKSTOP, run).await;
        let _ = child.start_kill();
        match outcome {
            Err(_) => Err(anyhow!(
                "{} timed out after {}s",
                self.kind.binary_name(),
                RUN_BACKSTOP.as_secs()
            )),
            Ok(Err(e)) => {
                let tail = err_tail.await.unwrap_or_default();
                let base = if tail.is_empty() {
                    format!("{e:#}")
                } else {
                    format!("{e:#}: {tail}")
                };
                match auth_fix_hint(self.kind, &base).or_else(|| model_fix_hint(self.kind, &base)) {
                    Some(hint) => Err(anyhow!("{base} — {hint}")),
                    None => Err(anyhow!("{base}")),
                }
            }
            Ok(ok) => ok,
        }
    }

    pub async fn chat(&self, messages: &[ChatTurn]) -> Result<ChatOutcome> {
        self.chat_stream(messages, |_| {}).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live smoke test against the real codex CLI — run explicitly:
    ///   cargo test agent_cli_codex_smoke -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn agent_cli_codex_smoke() {
        let cli = AgentCli::new(AgentKind::Codex);
        let messages = vec![
            ChatTurn::system("Answer with exactly one word."),
            ChatTurn::user("What is 2+2? Reply with only the number."),
        ];
        let out = cli.chat(&messages).await.expect("codex chat failed");
        assert!(out.text.contains('4'), "unexpected: {}", out.text);
    }

    /// Live smoke test against the real claude CLI — run explicitly:
    ///   cargo test agent_cli_claude_smoke -- --ignored --nocapture
    /// Skips nothing: requires the CLI installed and signed in.
    #[tokio::test]
    #[ignore]
    async fn agent_cli_claude_smoke() {
        let cli = AgentCli::new(AgentKind::Claude);
        let messages = vec![
            ChatTurn::system("Answer with exactly one word."),
            ChatTurn::user("What is 2+2? Reply with only the number."),
        ];
        let mut streamed = String::new();
        let out = cli
            .chat_stream(&messages, |t| streamed.push_str(t))
            .await
            .expect("claude chat failed");
        assert!(out.text.contains('4'), "unexpected: {}", out.text);
        assert!(!streamed.is_empty(), "no tokens streamed");
    }
}

#[cfg(test)]
mod live_smokes {
    use super::*;

    /// cargo test agent_cli_opencode_smoke -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn agent_cli_opencode_smoke() {
        let cli = AgentCli::new(AgentKind::Opencode);
        let out = cli
            .chat(&[ChatTurn::user("What is 2+2? Reply with only the number.")])
            .await
            .expect("opencode chat failed");
        assert!(out.text.contains('4'), "unexpected: {}", out.text);
    }

    /// cargo test agent_cli_pi_smoke -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn agent_cli_pi_smoke() {
        let cli = AgentCli::new(AgentKind::Pi);
        let out = cli
            .chat(&[ChatTurn::user("What is 2+2? Reply with only the number.")])
            .await
            .expect("pi chat failed");
        assert!(out.text.contains('4'), "unexpected: {}", out.text);
    }

    /// cargo test agent_cli_prime_smoke -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn agent_cli_prime_smoke() {
        let cli = AgentCli::new(AgentKind::Prime);
        let out = cli
            .chat(&[ChatTurn::user("What is 2+2? Reply with only the number.")])
            .await
            .expect("prime-agent chat failed");
        assert!(out.text.contains('4'), "unexpected: {}", out.text);
    }

    /// cargo test agent_cli_hermes_smoke -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn agent_cli_hermes_smoke() {
        let cli = AgentCli::new(AgentKind::Hermes);
        let out = cli
            .chat(&[ChatTurn::user("What is 2+2? Reply with only the number.")])
            .await
            .expect("hermes chat failed");
        assert!(out.text.contains('4'), "unexpected: {}", out.text);
    }
}
