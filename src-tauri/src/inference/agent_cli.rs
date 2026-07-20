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

/// Whole-response cap. Agent answers stream; a silent ten minutes is a hung
/// CLI, not a slow model.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
    Gemini,
    Cursor,
    /// IBM Bob via its own bobshell CLI — the sanctioned client itself, not a
    /// session workaround (Paul's call, 2026-07-20; API-key/session mimicry
    /// stays out per policy). Known wart: `bob -p` prints its thinking
    /// before the answer, and v1 passes output through as-is.
    Bob,
}

impl AgentKind {
    pub const ALL: [AgentKind; 5] = [
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Gemini,
        AgentKind::Cursor,
        AgentKind::Bob,
    ];

    pub fn binary_name(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Gemini => "gemini",
            AgentKind::Cursor => "cursor-agent",
            AgentKind::Bob => "bob",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::Gemini => "gemini-cli",
            AgentKind::Cursor => "cursor-cli",
            AgentKind::Bob => "bob-shell",
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
            AgentKind::Bob => "Bob Shell",
        }
    }

    pub fn install_hint(&self) -> &'static str {
        match self {
            AgentKind::Claude => "npm install -g @anthropic-ai/claude-code",
            AgentKind::Codex => "npm install -g @openai/codex",
            AgentKind::Gemini => "npm install -g @google/gemini-cli",
            AgentKind::Cursor => "curl https://cursor.com/install -fsS | bash",
            AgentKind::Bob => "curl -fsSL https://bob.ibm.com/download/bobshell.sh | sh",
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
    env
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
pub fn agent_status(kind: AgentKind) -> (bool, String) {
    match find_binary(kind.binary_name()) {
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

fn fold_system(system: &str, prompt: &str) -> String {
    if system.is_empty() {
        prompt.to_string()
    } else {
        format!("{system}\n\n---\n\n{prompt}")
    }
}

pub struct AgentCli {
    kind: AgentKind,
    binary: Option<PathBuf>,
}

impl AgentCli {
    pub fn new(kind: AgentKind) -> Self {
        Self {
            kind,
            binary: find_binary(kind.binary_name()),
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
        }
        let stdin_payload = match self.kind {
            AgentKind::Claude => Some(prompt.clone()),
            AgentKind::Cursor | AgentKind::Gemini => Some(fold_system(&system, &prompt)),
            AgentKind::Codex | AgentKind::Bob => None,
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
            let mut tail = String::new();
            if let Some(e) = stderr {
                let mut lines = BufReader::new(e).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    tail = l;
                }
            }
            tail
        });

        let kind = self.kind;
        let run = async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut on_token = on_token;
            let mut text = String::new();
            let mut errored: Option<String> = None;
            while let Some(line) = lines.next_line().await? {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    // Plain-text CLIs (gemini, bob) and stream-json builds
                    // that print bare text: pass the line through verbatim.
                    if matches!(kind, AgentKind::Gemini | AgentKind::Bob | AgentKind::Cursor) {
                        if !text.is_empty() {
                            text.push('\n');
                            on_token("\n");
                        }
                        text.push_str(&line);
                        on_token(&line);
                    }
                    continue;
                };
                match kind {
                    AgentKind::Gemini | AgentKind::Bob => {
                        // JSON on stdout from a plain-text CLI is unexpected;
                        // stringify it into the transcript rather than drop.
                        let t = v.to_string();
                        text.push_str(&t);
                        on_token(&t);
                    }
                    AgentKind::Claude | AgentKind::Cursor => match v["type"].as_str() {
                        // Per-token deltas from --include-partial-messages.
                        Some("stream_event") => {
                            if let Some(delta) = v["event"]["delta"]["text"].as_str() {
                                text.push_str(delta);
                                on_token(delta);
                            }
                        }
                        // Full assistant turns; authoritative when partial
                        // events were absent (older CLI versions).
                        Some("assistant") => {
                            if text.is_empty() {
                                if let Some(blocks) = v["message"]["content"].as_array() {
                                    for b in blocks {
                                        if let Some(t) = b["text"].as_str() {
                                            text.push_str(t);
                                            on_token(t);
                                        }
                                    }
                                }
                            }
                        }
                        Some("result") => {
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
                    AgentKind::Codex => {
                        // codex exec --json: items complete whole; the
                        // agent_message item carries the reply text.
                        if v["type"].as_str() == Some("item.completed")
                            && v["item"]["type"].as_str() == Some("agent_message")
                        {
                            if let Some(t) = v["item"]["text"].as_str() {
                                text.push_str(t);
                                on_token(t);
                            }
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
                _ => Ok(ChatOutcome { text, stats: None }),
            }
        };

        let outcome = tokio::time::timeout(REQUEST_TIMEOUT, run).await;
        let _ = child.start_kill();
        match outcome {
            Err(_) => Err(anyhow!(
                "{} timed out after {}s",
                self.kind.binary_name(),
                REQUEST_TIMEOUT.as_secs()
            )),
            Ok(Err(e)) => {
                let tail = err_tail.await.unwrap_or_default();
                if tail.is_empty() {
                    Err(e)
                } else {
                    Err(anyhow!("{e:#}: {tail}"))
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
