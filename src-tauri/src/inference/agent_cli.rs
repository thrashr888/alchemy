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

/// A CLI that *banners* — one speaking a structured event stream — and has
/// printed NOTHING for this long is wedged at startup; a healthy one banners
/// within seconds (codex emits thread.started before its model runs). Observed
/// live: the Morning Brief burned a whole 600-second budget on codex's MCP
/// handshake into a CPU-pinned process, before any model call happened.
///
/// This only detects a wedge for CLIs that announce themselves. See
/// [`AgentKind::banners_at_startup`].
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
/// Mid-run silence cap: ten minutes of nothing after output began is a
/// hang, not thinking.
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// Ceiling for an actively-streaming run — agentic briefs tool-loop and
/// legitimately run long.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(1800);
/// How often the wait is re-examined while a CLI is silent. One second keeps
/// the countdown honest without costing anything — the read future is held
/// across ticks, so a tick is a `sleep`, not a re-read.
const COUNTDOWN_TICK: Duration = Duration::from_secs(1);
/// Grace period before silence becomes visible. Under this, a quiet CLI is
/// just latency and a countdown would be noise; past it, a run that is going
/// to take minutes should say so rather than sit behind a spinner.
const COUNTDOWN_AFTER: Duration = Duration::from_secs(10);

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

    /// Does this CLI print something before the model answers?
    ///
    /// The event-protocol CLIs do — a session/thread banner lands within
    /// seconds — so silence past [`STARTUP_TIMEOUT`] means a wedged handshake.
    /// The plain-text one-shot CLIs (bob, gemini, copilot, hermes) print
    /// NOTHING until the answer itself begins: their pre-answer silence covers
    /// MCP discovery, extension loading, and the whole model call, and is
    /// indistinguishable from thinking. Holding them to the startup deadline
    /// killed healthy slow runs and then blamed whatever boot warning happened
    /// to be last on stderr — Paul's two "bob produced no output 120s" reports,
    /// captioned with an unrelated gpg-agent path and a stray `}`.
    pub fn banners_at_startup(&self) -> bool {
        match self {
            AgentKind::Claude
            | AgentKind::Codex
            | AgentKind::Cursor
            | AgentKind::Opencode
            | AgentKind::Prime
            | AgentKind::Pi => true,
            AgentKind::Gemini | AgentKind::Bob | AgentKind::Copilot | AgentKind::Hermes => false,
        }
    }

    /// The reasoning-effort levels this CLI accepts, cheapest first. Empty
    /// means the CLI has no such control and the UI hides it rather than
    /// offering a setting that does nothing.
    ///
    /// Read off each CLI's own `--help` (2026-08-18), which is also why the
    /// ladders differ in length: codex tops out where OpenAI's
    /// `reasoning_effort` does, while claude and pi carry two rungs above it.
    pub fn efforts(&self) -> &'static [&'static str] {
        match self {
            // `--effort <level>`: low, medium, high, xhigh, max.
            AgentKind::Claude => &["low", "medium", "high", "xhigh", "max"],
            // `-c model_reasoning_effort="<level>"` — OpenAI's own ladder.
            AgentKind::Codex => &["minimal", "low", "medium", "high"],
            // `--thinking <level>`: off, minimal, low, medium, high, xhigh,
            // max. "off" is left out — Default already means "don't ask".
            AgentKind::Pi | AgentKind::Prime => {
                &["minimal", "low", "medium", "high", "xhigh", "max"]
            }
            // `--variant <level>`, documented as provider-specific with
            // "high, max, minimal" named: offer only those three.
            AgentKind::Opencode => &["minimal", "high", "max"],
            AgentKind::Gemini
            | AgentKind::Cursor
            | AgentKind::Copilot
            | AgentKind::Hermes
            | AgentKind::Bob => &[],
        }
    }

    /// Argv that makes this CLI print the models it can reach, when it has
    /// such a command. `None` means "no catalogue" — the CLI still accepts
    /// `--model`, we just can't enumerate for it, so the picker offers Default
    /// plus a free-text entry rather than a list we made up.
    ///
    /// Bob is deliberately `None`: bobshell has no list command, and its only
    /// catalogue is the IBM inference API, which refuses third-party clients
    /// outright (Cloudflare 403 — IBM policy, not a bug to route around).
    pub fn list_models_args(&self) -> Option<&'static [&'static str]> {
        match self {
            AgentKind::Opencode => Some(&["models"]),
            AgentKind::Pi => Some(&["--list-models"]),
            AgentKind::Prime => Some(&["model", "list"]),
            AgentKind::Cursor => Some(&["--list-models"]),
            AgentKind::Claude
            | AgentKind::Codex
            | AgentKind::Gemini
            | AgentKind::Copilot
            | AgentKind::Hermes
            | AgentKind::Bob => None,
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
pub(crate) fn load_shell_env() -> HashMap<String, String> {
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

pub(crate) fn find_binary_cached(name: &str) -> Option<PathBuf> {
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

/// A vendor gateway rejecting the model ("The requested model is not
/// supported" / `model_not_supported`) means a retired name is being asked
/// for — seen live with GitHub Copilot and codex. Point at whoever chose it:
/// Alchemy, when the provider entry names a model, and the CLI's own saved
/// default otherwise. Sending someone to the CLI's /model command to fix a
/// name that lives in Alchemy's own settings is a dead end.
fn model_fix_hint(kind: AgentKind, model: Option<&str>, error_text: &str) -> Option<String> {
    let t = error_text.to_lowercase();
    // Three live phrasings: copilot's old gateway JSON (model_not_supported),
    // codex's prose ("model is not supported"), and copilot 1.x's flag
    // rejection ("Model \"x\" from --model flag is not available").
    let modelish = t.contains("model_not_supported")
        || t.contains("model is not supported")
        || (t.contains("model") && t.contains("not available"));
    if !modelish {
        return None;
    }
    let bin = kind.binary_name();
    Some(match model {
        Some(m) => format!(
            "Fix: “{m}” is the model set for {} in Settings → Models — clear it \
             to use the CLI's own default, or name one {bin} still offers.",
            kind.label(),
        ),
        None => format!(
            "Fix: the {} CLI is likely outdated, or pinned to a model that has \
             been retired — update it (`{}`) and retry here. If that doesn't \
             clear it, run `{bin}` and pick a current model (usually the \
             /model command), or name one in Settings → Models.",
            kind.label(),
            kind.install_hint()
        ),
    })
}

/// copilot's end-of-run stats footer, printed to stderr after the answer:
/// `Changes    +N -M` and `AI Credits X.XX (Ys)`. Matched by shape so a real
/// answer line has essentially no way to collide (verified live on CLI
/// 1.0.80).
fn is_copilot_footer(line: &str) -> bool {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("Changes") {
        let rest = rest.trim();
        if let Some((add, del)) = rest.split_once(' ') {
            return add
                .strip_prefix('+')
                .is_some_and(|n| n.parse::<u64>().is_ok())
                && del
                    .trim()
                    .strip_prefix('-')
                    .is_some_and(|n| n.parse::<u64>().is_ok());
        }
        return false;
    }
    if let Some(rest) = t.strip_prefix("AI Credits") {
        let rest = rest.trim();
        return rest.ends_with(')')
            && rest.contains('(')
            && rest.chars().next().is_some_and(|c| c.is_ascii_digit());
    }
    false
}

/// Did a plain-text CLI print an error transcript instead of an answer?
///
/// Old copilot builds put their whole failure on stdout — five "× Model call
/// failed" retries and a final "× Execution failed" — which the plain-text
/// passthrough would otherwise return as a successful reply. Only when EVERY
/// non-blank line is error-shaped is the text reclassified as a failure, so
/// an answer that merely discusses an error can never be swallowed. Returns
/// the first and last lines, the same shape the stderr tail keeps.
fn plain_text_error_transcript(kind: AgentKind, text: &str) -> Option<String> {
    if !matches!(
        kind,
        AgentKind::Gemini
            | AgentKind::Bob
            | AgentKind::Cursor
            | AgentKind::Copilot
            | AgentKind::Hermes
    ) {
        return None;
    }
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let error_shaped = |l: &str| {
        l.starts_with('\u{00d7}') // ×
            || l.starts_with('\u{2717}') // ✗
            || l.starts_with("Error:")
            || l.starts_with("Execution failed")
    };
    if !lines.iter().all(|l| error_shaped(l)) {
        return None;
    }
    let (first, last) = (lines[0], lines[lines.len() - 1]);
    Some(if first == last {
        first.to_string()
    } else {
        format!("{first} \u{2014} {last}")
    })
}

/// Is this stderr line worth quoting back to the user as the cause?
///
/// CLIs that pretty-print JSON to stderr leave structural crumbs — a bare `}`,
/// `],`, a lone quote — that say nothing on their own. Keeping the last
/// *non-empty* line is how Paul's bug report ended up captioned "— }". Require
/// a few actual word characters before a line is allowed to speak for a failure.
fn is_meaningful_stderr(line: &str) -> bool {
    line.chars().filter(|c| c.is_alphanumeric()).count() >= 3
}

/// The live line shown while a CLI has said nothing for a while: what we are
/// waiting on, and how long before we stop waiting. Phrased as a wait rather
/// than a warning — most of these finish.
fn waiting_label(kind: AgentKind, saw_output: bool, left: Duration) -> String {
    let secs = left.as_secs();
    let remaining = match secs {
        0..=59 => format!("{secs}s"),
        _ => format!("{}m {:02}s", secs / 60, secs % 60),
    };
    match saw_output {
        false => format!("Waiting for {} — {remaining} left", kind.label()),
        true => format!("{} has gone quiet — {remaining} left", kind.label()),
    }
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

/// What a CLI will use when we pass no `--model`, read from its own config so
/// the composer can show a name instead of the bare word "Default".
///
/// Only where the location and key are documented and stable: codex writes
/// `model = "…"` at the top level of `~/.codex/config.toml`. The others either
/// keep it behind an authenticated command (`claude config get model` fails on
/// an expired session) or don't record one at all, and a guess shown as fact
/// would be worse than saying nothing — those return `None`, and the UI falls
/// back to "Default".
fn cli_default_model(kind: AgentKind) -> Option<String> {
    // Copilot's default is what Alchemy passes, not what the CLI saved:
    // blank model = `--model auto` (see the invocation arm), so "auto" is the
    // truth the picker should show.
    if kind == AgentKind::Copilot {
        return Some("auto".to_string());
    }
    let home = std::env::var("HOME").ok()?;
    let (path, key) = match kind {
        AgentKind::Codex => (format!("{home}/.codex/config.toml"), "model"),
        _ => return None,
    };
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .map(str::trim)
        // Top-level only: a `model` under some `[profiles.x]` table is not the
        // default. Stop at the first table header.
        .take_while(|l| !l.starts_with('['))
        .find_map(|l| l.strip_prefix(key)?.trim_start().strip_prefix('='))
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
}

/// The readable sentence inside an opencode error event.
///
/// Shape is `{name, data: {message, statusCode, ...}}`, and `data.message`
/// often embeds the provider's own JSON body verbatim
/// (`Payment Required: {"error":{"message":"You have exceeded your monthly
/// quota"}}`). Splice the inner message in place of the blob so the user
/// reads one sentence instead of a wire dump.
fn opencode_error_message(err: &serde_json::Value) -> String {
    let raw = err["data"]["message"]
        .as_str()
        .or_else(|| err["message"].as_str())
        .or_else(|| err["name"].as_str())
        .unwrap_or("opencode reported an error");
    let Some(brace) = raw.find('{') else {
        return raw.to_string();
    };
    let (prefix, body) = raw.split_at(brace);
    let inner = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .or_else(|| v["message"].as_str())
                .map(str::to_string)
        });
    match inner {
        Some(msg) => {
            let prefix = prefix.trim_end_matches([':', ' ']);
            if prefix.is_empty() {
                msg
            } else {
                format!("{prefix}: {msg}")
            }
        }
        None => raw.to_string(),
    }
}

/// Drop ANSI colour/cursor escapes so a CLI's decorated output parses. These
/// CLIs redraw a "Loading models…" spinner before printing, and the control
/// bytes otherwise glue themselves to the first model name.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // CSI/OSC run: skip to the terminating byte.
        for c in chars.by_ref() {
            if c.is_ascii_alphabetic() || c == '~' {
                break;
            }
        }
    }
    out
}

/// Parse a CLI's model listing into ids we can hand back to `--model`.
///
/// Two shapes in the wild, told apart by their own header rather than by which
/// CLI printed them:
///   * a `provider  model  context …` table (pi, prime-agent) — the pair joins
///     into the `provider/id` form both accept back;
///   * one id per line (opencode's `provider/model`).
///
/// Anything else on the line — spinners, "No models available for this
/// account.", totals — has the wrong shape and is dropped rather than offered
/// to the user as a model name.
fn parse_model_list(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut table = false;
    for raw in text.lines() {
        let line = strip_ansi(raw);
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.first() == Some(&"provider") && cols.get(1) == Some(&"model") {
            table = true;
            continue;
        }
        let id = if table {
            match cols.len() >= 2 {
                true => format!("{}/{}", cols[0], cols[1]),
                false => continue,
            }
        } else {
            match cols.as_slice() {
                [only] => only.to_string(),
                _ => continue,
            }
        };
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// How long a model listing may take before we give up and fall back to
/// Default-plus-free-text. Listing spawns the CLI, which boots like any other
/// run; it is still a menu opening, so it cannot hang behind a spinner.
const LIST_MODELS_TIMEOUT: Duration = Duration::from_secs(45);

/// Process-lifetime cache of model listings, keyed by CLI id. Spawning these
/// costs seconds; the picker opens on a click.
fn model_cache() -> &'static std::sync::Mutex<HashMap<&'static str, Vec<String>>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<&'static str, Vec<String>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// The model this CLI falls back to with no `--model`, when it is knowable.
pub fn agent_default_model(kind: AgentKind) -> Option<String> {
    cli_default_model(kind)
}

/// The models `kind` says it can reach. Empty when the CLI has no catalogue
/// command, is not installed, or fails — every caller treats "no list" and "a
/// list we could not fetch" the same way, so neither is an error.
pub async fn list_agent_models(kind: AgentKind) -> Vec<String> {
    if let Some(hit) = model_cache().lock().unwrap().get(kind.id()) {
        return hit.clone();
    }
    let Some(args) = kind.list_models_args() else {
        return Vec::new();
    };
    let Some(bin) = find_binary_cached(kind.binary_name()) else {
        return Vec::new();
    };
    let env = tokio::task::spawn_blocking(load_shell_env)
        .await
        .unwrap_or_default();
    let mut cmd = tokio::process::Command::new(bin);
    cmd.env_clear()
        .envs(&env)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let models = match tokio::time::timeout(LIST_MODELS_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) => parse_model_list(&String::from_utf8_lossy(&out.stdout)),
        _ => Vec::new(),
    };
    // Cache even an empty result: a CLI without a catalogue should not be
    // re-spawned every time the menu opens.
    model_cache()
        .lock()
        .unwrap()
        .insert(kind.id(), models.clone());
    models
}

#[derive(Clone)]
pub struct AgentCli {
    kind: AgentKind,
    binary: Option<PathBuf>,
    /// Model override from the provider entry, empty = the CLI's own default.
    /// Without this, a CLI pinned to a retired model ("The requested model is
    /// not supported" / `model_not_supported`, seen live on codex and copilot)
    /// could only be fixed outside Alchemy — the provider's model field was
    /// collected and then dropped on the floor for family B.
    model: Option<String>,
    /// Reasoning effort, empty = the CLI's own default. Ignored for CLIs whose
    /// [`AgentKind::efforts`] ladder is empty.
    effort: Option<String>,
}

impl AgentCli {
    pub fn configured(kind: AgentKind, model: &str, effort: &str) -> Self {
        let model = model.trim();
        let effort = effort.trim();
        Self {
            kind,
            model: (!model.is_empty()).then(|| model.to_string()),
            // A level this CLI does not offer is dropped rather than passed
            // through to be rejected — provider ladders differ in length, and
            // switching providers must not poison the next run.
            effort: kind.efforts().contains(&effort).then(|| effort.to_string()),
            binary: find_binary_cached(kind.binary_name()),
        }
    }

    pub fn kind(&self) -> AgentKind {
        self.kind
    }

    /// Point this engine at an arbitrary executable instead of the discovered
    /// CLI. Test-only: it lets a shell script stand in for a vendor CLI, so
    /// the REAL spawn → stream-parse → classify → hint pipeline runs against
    /// scripted stdout/stderr instead of a live subscription.
    #[cfg(test)]
    pub(crate) fn with_binary_for_test(kind: AgentKind, binary: PathBuf) -> Self {
        Self {
            kind,
            model: None,
            effort: None,
            binary: Some(binary),
        }
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
    ///
    /// Retry parity with the gateway's backoff, scoped to what is safe for
    /// a subprocess that may run tools: ONE retry, and only when the first
    /// attempt died quickly without producing any output — a spawn failure
    /// or an instant crash, the transient class. A run that streamed
    /// anything is never blindly repeated (duplicate tokens, repeated tool
    /// side effects), and timeouts are not retried — doubling a 120s
    /// silence hides a wedged CLI instead of surfacing it.
    pub async fn chat_stream_steps<F, S>(
        &self,
        messages: &[ChatTurn],
        mut on_token: F,
        mut on_step: S,
    ) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
        S: FnMut(super::Step<'_>),
    {
        // A missing binary is deterministic — never worth a retry.
        self.binary.as_ref().ok_or_else(|| {
            anyhow!(
                "{} CLI not found. {}",
                self.kind.binary_name(),
                self.kind.install_hint()
            )
        })?;
        const QUICK_FAILURE: std::time::Duration = std::time::Duration::from_secs(20);
        let mut saw_any = false;
        let started = std::time::Instant::now();
        match self
            .chat_stream_once(messages, &mut on_token, &mut on_step, &mut saw_any)
            .await
        {
            Err(err) if !saw_any && started.elapsed() < QUICK_FAILURE => {
                crate::note!(
                    "{} failed before any output ({err:#}); retrying once",
                    self.kind.binary_name()
                );
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                self.chat_stream_once(messages, &mut on_token, &mut on_step, &mut saw_any)
                    .await
            }
            outcome => outcome,
        }
    }

    async fn chat_stream_once<F, S>(
        &self,
        messages: &[ChatTurn],
        on_token: &mut F,
        on_step: &mut S,
        saw_any: &mut bool,
    ) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
        S: FnMut(super::Step<'_>),
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
        // Every CLI in the roster spells this `--model` (verified against
        // --help for the eight installed here; copilot per its docs). It must
        // go on before the positional-prompt arms below append theirs.
        let model = self.model.clone();
        let effort = self.effort.clone();
        let kind = self.kind;
        // Each CLI spells effort differently; the ladder is validated in
        // `configured`, so by here the level is one this CLI accepts.
        let set_model = |cmd: &mut tokio::process::Command| {
            if let Some(m) = &model {
                cmd.args(["--model", m]);
            }
            let Some(e) = &effort else { return };
            match kind {
                AgentKind::Claude => cmd.args(["--effort", e]),
                // A codex config override is TOML: the value needs its quotes.
                AgentKind::Codex => cmd.args(["-c", &format!("model_reasoning_effort={e:?}")]),
                AgentKind::Pi | AgentKind::Prime => cmd.args(["--thinking", e]),
                AgentKind::Opencode => cmd.args(["--variant", e]),
                AgentKind::Gemini
                | AgentKind::Cursor
                | AgentKind::Copilot
                | AgentKind::Hermes
                | AgentKind::Bob => cmd,
            };
        };
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
                set_model(&mut cmd);
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
                cmd.args(["exec", "--json", "--skip-git-repo-check"]);
                set_model(&mut cmd);
                cmd.arg(&full);
            }
            AgentKind::Cursor => {
                // cursor-agent print mode speaks claude-shaped stream-json;
                // the lenient parser treats non-JSON lines as raw text so
                // plain-text builds still work. No system flag — folded
                // into the prompt below. Prompt over stdin.
                // --trust: headless runs inherit the app's cwd, which the
                // CLI has never trusted; without it every run dies on the
                // interactive Workspace Trust prompt.
                cmd.args(["-p", "--output-format", "stream-json", "--trust"]);
                set_model(&mut cmd);
            }
            AgentKind::Gemini => {
                // Plain-text CLI reading the prompt from stdin; stdout
                // chunks stream through as tokens. No system flag — folded.
                set_model(&mut cmd);
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
                cmd.args(["run", "--format", "json"]);
                set_model(&mut cmd);
                cmd.arg(&full);
            }
            AgentKind::Copilot => {
                // Verified live (CLI 1.0.80): `copilot -p <prompt>` answers on
                // stdout. Plain-text output parse.
                let full = fold_system(&system, &prompt);
                if full.len() > 150_000 {
                    return Err(anyhow!("context too large for copilot's argv-based prompt"));
                }
                cmd.args(["-p", &full]);
                set_model(&mut cmd);
                // No model configured: pass `auto` rather than nothing. The
                // CLI's own saved model goes stale when GitHub retires it —
                // non-interactively it then burns five retries and ~100s
                // before failing with model_not_supported (Paul's live
                // report, twice). `auto` is copilot's documented ask-the-
                // service alias ("use 'auto' to let Copilot pick
                // automatically"), so it can never name a retired model.
                if self.model.is_none() {
                    cmd.args(["--model", "auto"]);
                }
            }
            AgentKind::Hermes => {
                // Verified live: `hermes -z <prompt>` prints the reply as
                // plain text.
                let full = fold_system(&system, &prompt);
                if full.len() > 150_000 {
                    return Err(anyhow!("context too large for hermes's argv-based prompt"));
                }
                cmd.args(["-z", &full]);
                set_model(&mut cmd);
            }
            AgentKind::Bob => {
                // The prompt goes in positionally — bobshell's own --help now
                // marks `-p` deprecated ("Use the positional prompt instead.
                // This flag will be removed in a future version"), and the
                // positional form is one-shot by default. Argv-based either
                // way, so guard oversized stuffed contexts against ARG_MAX.
                let full = fold_system(&system, &prompt);
                if full.len() > 150_000 {
                    return Err(anyhow!(
                        "context too large for bob's argv-based prompt — \
                         trim source selection or use another provider"
                    ));
                }
                set_model(&mut cmd);
                cmd.arg(&full);
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
                set_model(&mut cmd);
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
        let err_kind = self.kind;
        let err_tail = tokio::spawn(async move {
            // Vendors put the cause on the first line and decoration after
            // ("must specify GEMINI_API_KEY" … "Update your environment…"),
            // so keep first + last meaningful lines, not just the last.
            let mut first = String::new();
            let mut last = String::new();
            if let Some(e) = stderr {
                let mut lines = BufReader::new(e).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    if !is_meaningful_stderr(&l) {
                        continue;
                    }
                    // copilot ends every run with a stats footer on stderr
                    // ("Changes    +0 -0", "AI Credits 3.4 (4s)") — session
                    // accounting that would otherwise be kept as the "last
                    // meaningful line" and quoted as a failure's cause.
                    if err_kind == AgentKind::Copilot && is_copilot_footer(&l) {
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
            let on_token = on_token;
            let on_step = on_step;
            // Consecutive-duplicate guard: event streams repeat themselves
            // (several deltas per tool call), and the step trail shouldn't.
            // Transient lines skip the guard — their whole job is to change.
            let mut last_step = String::new();
            let mut emit_step = move |label: String, transient: bool| {
                if transient {
                    on_step(super::Step::transient(&label));
                } else if label != last_step {
                    on_step(super::Step::new(&label));
                    last_step = label;
                }
            };
            // Immediate first line — the CLI takes seconds just to boot.
            emit_step(format!("Asking {}", kind.label()), false);
            let mut text = String::new();
            let mut errored: Option<String> = None;
            let mut cost_usd: Option<f64> = None;
            let mut in_thinking = false;
            // Staged deadlines, not one flat cap: a silent START is a wedged
            // handshake and fails fast; once streaming, the run gets real
            // room, bounded by a mid-run silence cap and a total ceiling.
            let started = std::time::Instant::now();
            let mut saw_output = false;
            // A CLI that never banners gets the idle budget from the start:
            // there is no handshake signal to time out on, only thinking.
            let startup = if kind.banners_at_startup() {
                STARTUP_TIMEOUT
            } else {
                IDLE_TIMEOUT
            };
            while let Some(line) = {
                let stage = if saw_output { IDLE_TIMEOUT } else { startup };
                let quiet_since = std::time::Instant::now();
                // The wait is sliced so a long silence can say so out loud.
                // The read future is held across ticks rather than re-created,
                // so nothing buffered is lost to a cancelled `next_line`.
                let next = lines.next_line();
                tokio::pin!(next);
                loop {
                    let stage_left = stage.saturating_sub(quiet_since.elapsed());
                    let total_left = TOTAL_TIMEOUT.saturating_sub(started.elapsed());
                    let left = stage_left.min(total_left);
                    if left.is_zero() {
                        let (what, secs) = if !saw_output {
                            ("produced no output", startup.as_secs())
                        } else if total_left.is_zero() {
                            ("exceeded the run ceiling of", TOTAL_TIMEOUT.as_secs())
                        } else {
                            ("went silent mid-run for", IDLE_TIMEOUT.as_secs())
                        };
                        return Err(anyhow!("{} {what} {secs}s", kind.binary_name()));
                    }
                    tokio::select! {
                        line = &mut next => break line?,
                        _ = tokio::time::sleep(COUNTDOWN_TICK.min(left)) => {
                            // Short silences are just latency; past the grace
                            // period the wait becomes visible, with the deadline
                            // it is counting toward, so a slow run reads as slow
                            // rather than frozen.
                            if quiet_since.elapsed() >= COUNTDOWN_AFTER {
                                emit_step(waiting_label(kind, saw_output, left), true);
                            }
                        }
                    }
                }
            } {
                saw_output = true;
                *saw_any = true;
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
                                emit_step(format!("Using {name}"), false);
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
                            Some("step_start") => emit_step("Thinking".into(), false),
                            Some("tool") => {
                                let name = v["part"]["tool"].as_str().unwrap_or("a tool");
                                emit_step(tool_step_label(name), false);
                            }
                            // opencode reports provider failures as an event
                            // on STDOUT and then exits 0-ish with no text.
                            // Dropping it turned "You have exceeded your
                            // monthly quota" into "agent produced no output"
                            // — the cause was on the pipe the whole time.
                            Some("error") => {
                                errored = Some(opencode_error_message(&v["error"]));
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
                                emit_step(tool_step_label(name), false);
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
                                        emit_step(tool_step_label(name), false);
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
                                emit_step(tool_step_label(name), false);
                            }
                            Some("auto_retry_start") => emit_step("Retrying".into(), false),
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
                            emit_step(label, false);
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
                _ => match plain_text_error_transcript(kind, &text) {
                    // A plain-text CLI prints its failures to stdout, where
                    // the passthrough would deliver them as the ANSWER — the
                    // user then reads a wall of "× Model call failed" with no
                    // guidance, because none of the fix-hint machinery ever
                    // saw an error (Paul's second copilot report, verbatim).
                    Some(msg) => Err(anyhow!("{msg}")),
                    None => Ok(ChatOutcome {
                        text,
                        stats: None,
                        cost_usd,
                    }),
                },
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
                // Label it as stderr rather than splicing it in as the cause:
                // a CLI's boot warnings (a missing gpg-agent.conf, an MCP
                // server it could not reach) are noise that happened to be on
                // the pipe, and reading "produced no output 120s: Path could
                // not be resolved…" sent Paul hunting the wrong bug.
                let base = if tail.is_empty() {
                    format!("{e:#}")
                } else {
                    format!("{e:#} (stderr: {tail})")
                };
                let hint = auth_fix_hint(self.kind, &base)
                    .or_else(|| model_fix_hint(self.kind, self.model.as_deref(), &base));
                match hint {
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
        let cli = AgentCli::configured(AgentKind::Codex, "", "");
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
        let cli = AgentCli::configured(AgentKind::Claude, "", "");
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

    /// codex's real config shape. A `model` under a profile table is not the
    /// default and must not be mistaken for one.
    #[test]
    fn a_clis_default_model_comes_from_its_own_top_level_config() {
        let parse = |text: &str| -> Option<String> {
            text.lines()
                .map(str::trim)
                .take_while(|l| !l.starts_with('['))
                .find_map(|l| l.strip_prefix("model")?.trim_start().strip_prefix('='))
                .map(|v| v.trim().trim_matches('"').to_string())
                .filter(|v| !v.is_empty())
        };
        let real = "model = \"gpt-5.6-sol\"\nmodel_reasoning_effort = \"medium\"\n";
        assert_eq!(parse(real), Some("gpt-5.6-sol".to_string()));

        let profiled = "approval_policy = \"on-request\"\n\n[profiles.fast]\nmodel = \"mini\"\n";
        assert_eq!(
            parse(profiled),
            None,
            "a profile's model is not the default"
        );

        // Every CLI without a documented location says so rather than guessing.
        assert!(cli_default_model(AgentKind::Bob).is_none());
        assert!(cli_default_model(AgentKind::Gemini).is_none());
    }

    /// Two listing shapes, told apart by their own header. Fixtures are the
    /// real thing: `pi --list-models` and `opencode models`, run live.
    #[test]
    fn model_listings_parse_into_ids_we_can_hand_back() {
        let pi = "provider  model                         context  max-out  thinking  images\n\
                  ollama    gpt-oss:120b                  128K     16.4K    no        no\n\
                  ollama    qwen3.6:35b-mlx               128K     16.4K    no        no\n";
        assert_eq!(
            parse_model_list(pi),
            vec!["ollama/gpt-oss:120b", "ollama/qwen3.6:35b-mlx"]
        );

        let opencode = "opencode/big-pickle\ngithub-copilot/claude-sonnet-5\n";
        assert_eq!(
            parse_model_list(opencode),
            vec!["opencode/big-pickle", "github-copilot/claude-sonnet-5"]
        );
    }

    /// A CLI with nothing to offer must yield an empty list, not a menu entry
    /// reading "No models available for this account." — cursor-agent's real
    /// output on an account with none, spinner escapes and all.
    #[test]
    fn status_chatter_is_never_offered_as_a_model() {
        let cursor = "\u{1b}[2K\u{1b}[GLoading models…\n\
                      \u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[GNo models available for this account.\n";
        assert!(parse_model_list(cursor).is_empty());
        assert_eq!(
            strip_ansi("\u{1b}[2K\u{1b}[Gopencode/big-pickle"),
            "opencode/big-pickle"
        );
    }

    /// Only the CLIs that really have a catalogue command get spawned for one.
    #[test]
    fn only_cataloguing_clis_are_listed() {
        for kind in AgentKind::ALL {
            let expected = matches!(
                kind,
                AgentKind::Opencode | AgentKind::Pi | AgentKind::Prime | AgentKind::Cursor
            );
            assert_eq!(kind.list_models_args().is_some(), expected, "{kind:?}");
        }
    }

    /// Write an executable script that plays a vendor CLI: prints the given
    /// stdout and stderr, exits with the given code. The fixture for every
    /// exit-code-blind pipeline test below.
    fn fake_cli(stdout: &str, stderr: &str, exit: i32) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nbl-fakecli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk fake cli dir");
        let path = dir.join("cli.sh");
        let script = format!(
            "#!/bin/sh\ncat <<'NBL_OUT'\n{stdout}\nNBL_OUT\ncat <<'NBL_ERR' >&2\n{stderr}\nNBL_ERR\nexit {exit}\n"
        );
        std::fs::write(&path, script).expect("write fake cli");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    /// The situation end to end, not just the classifier: an old copilot
    /// EXITS SUCCESSFULLY while printing its whole failure to stdout (Paul's
    /// live transcript, footer on stderr) — the app must report an error, and
    /// that error must carry the fix hint, not the stderr footer.
    #[tokio::test]
    async fn a_cli_that_prints_errors_and_exits_zero_is_reported_as_an_error() {
        let stdout = "\u{00d7} Model call failed: {\"message\":\"The requested model is not supported.\",\"code\":\"model_not_supported\",\"param\":\"model\",\"type\":\"invalid_request_error\"}\n\
             \n\
             \u{00d7} Execution failed: Failed to get response from the AI model; retried 5 times";
        let bin = fake_cli(stdout, "Changes    +0 -0\nAI Credits 3.4 (4s)", 0);
        let cli = AgentCli::with_binary_for_test(AgentKind::Copilot, bin);
        let err = cli
            .chat(&[ChatTurn::user("hi")])
            .await
            .err()
            .expect("an all-error transcript must surface as a failure");
        let msg = format!("{err:#}");
        assert!(msg.contains("model_not_supported"), "{msg}");
        assert!(msg.contains("Fix:"), "no fix hint attached: {msg}");
        assert!(msg.contains("outdated"), "{msg}");
        assert!(
            !msg.contains("AI Credits") && !msg.contains("Changes"),
            "stderr footer quoted as a cause: {msg}"
        );
    }

    /// The false-positive guard, through the same real pipeline: an answer
    /// that QUOTES an error line among normal prose must come back as a
    /// successful answer, verbatim, never reclassified.
    #[tokio::test]
    async fn an_answer_discussing_an_error_is_never_reclassified() {
        let stdout = "The failure you pasted was:\n\
             \u{00d7} Model call failed: model_not_supported\n\
             It means the CLI asked for a retired model.";
        let bin = fake_cli(stdout, "AI Credits 1.2 (2s)", 0);
        let cli = AgentCli::with_binary_for_test(AgentKind::Copilot, bin);
        let out = cli
            .chat(&[ChatTurn::user("what was that error?")])
            .await
            .expect("a real answer must not be swallowed");
        assert!(out
            .text
            .contains("It means the CLI asked for a retired model."));
        assert!(
            out.text.contains("\u{00d7} Model call failed"),
            "quoted line survives"
        );
        assert!(
            !out.text.contains("AI Credits"),
            "footer stays out of the answer"
        );
    }

    /// The modern-copilot shape: empty stdout, a clean stderr error, exit 1.
    /// The error must quote the stderr cause and carry the hint — and the
    /// stats footer around it must not be mistaken for the cause.
    #[tokio::test]
    async fn a_clean_stderr_failure_carries_cause_and_hint_not_footer() {
        let bin = fake_cli(
            "",
            "Error: Model \"fake-model-9000\" from --model flag is not available.\nChanges    +0 -0\nAI Credits 0.1 (1s)",
            1,
        );
        let cli = AgentCli::with_binary_for_test(AgentKind::Copilot, bin);
        let err = cli
            .chat(&[ChatTurn::user("hi")])
            .await
            .err()
            .expect("empty stdout is a failure");
        let msg = format!("{err:#}");
        assert!(msg.contains("not available"), "stderr cause missing: {msg}");
        assert!(msg.contains("Fix:"), "{msg}");
        assert!(!msg.contains("AI Credits"), "footer quoted as cause: {msg}");
    }

    /// Paul's second copilot report, verbatim: an OLD copilot printed its
    /// whole failure to stdout, so the passthrough delivered five retry
    /// errors as the ANSWER and no fix hint ever fired.
    #[test]
    fn an_all_error_stdout_transcript_is_a_failure_not_an_answer() {
        let transcript = "\u{00d7} Model call failed: {\"message\":\"The requested model is not supported.\",\"code\":\"model_not_supported\",\"param\":\"model\",\"type\":\"invalid_request_error\"}\n\
             \n\
             \u{00d7} Model call failed: {\"message\":\"The requested model is not supported.\",\"code\":\"model_not_supported\",\"param\":\"model\",\"type\":\"invalid_request_error\"}\n\
             \u{00d7} Execution failed: Failed to get response from the AI model; retried 5 times";
        let msg = plain_text_error_transcript(AgentKind::Copilot, transcript)
            .expect("an all-error transcript is a failure");
        // The reclassified message must still trip the model fix hint.
        assert!(
            model_fix_hint(AgentKind::Copilot, None, &msg).is_some(),
            "{msg}"
        );

        // An answer that merely DISCUSSES an error is never swallowed.
        let discussing = "The error you saw was:\n\u{00d7} Model call failed: model_not_supported";
        assert!(plain_text_error_transcript(AgentKind::Copilot, discussing).is_none());

        // Event-protocol CLIs are exempt — their text came out of JSON fields.
        assert!(plain_text_error_transcript(AgentKind::Codex, transcript).is_none());
    }

    /// copilot's stderr stats footer must never be quoted as a failure's
    /// cause (shapes verified live on CLI 1.0.80).
    #[test]
    fn copilot_footer_lines_are_recognised() {
        assert!(is_copilot_footer("Changes    +0 -0"));
        assert!(is_copilot_footer("AI Credits 3.4 (4s)"));
        assert!(is_copilot_footer("AI Credits 5.98 (4s)"));
        assert!(!is_copilot_footer("Changes to the API are breaking"));
        assert!(!is_copilot_footer("AI Credits are a billing concept"));
        assert!(!is_copilot_footer("Error: something real"));
    }

    /// The countdown says what it is waiting on and how long is left, in a
    /// shape that reads as a wait rather than a warning.
    #[test]
    fn the_countdown_names_the_wait_and_the_deadline() {
        let before = waiting_label(AgentKind::Bob, false, Duration::from_secs(605));
        assert_eq!(before, "Waiting for Bob Shell — 10m 05s left");

        let after = waiting_label(AgentKind::Bob, true, Duration::from_secs(42));
        assert_eq!(after, "Bob Shell has gone quiet — 42s left");
    }

    /// The startup deadline is a wedge detector, and it only detects anything
    /// for a CLI that announces itself before the model runs. Verified against
    /// each CLI's real behaviour: bob's first stdout line is its own
    /// `<thinking>` block, i.e. the answer already starting.
    #[test]
    fn only_bannering_clis_get_the_startup_deadline() {
        for kind in AgentKind::ALL {
            let expected = !matches!(
                kind,
                AgentKind::Gemini | AgentKind::Bob | AgentKind::Copilot | AgentKind::Hermes
            );
            assert_eq!(
                kind.banners_at_startup(),
                expected,
                "{} classified wrong",
                kind.binary_name()
            );
        }
    }

    /// Paul's report was captioned "… — }": the last non-empty stderr line was
    /// a closing brace from a pretty-printed JSON error.
    #[test]
    fn structural_stderr_crumbs_never_speak_for_a_failure() {
        assert!(!is_meaningful_stderr("}"));
        assert!(!is_meaningful_stderr("  },"));
        assert!(!is_meaningful_stderr("]"));
        assert!(!is_meaningful_stderr(""));
        assert!(is_meaningful_stderr(
            r#"× Model call failed: {"code":"model_not_supported"}"#
        ));
        assert!(is_meaningful_stderr(
            "Error: ENOENT: no such file or directory"
        ));
    }

    /// codex's live phrasing differs from copilot's `model_not_supported`
    /// code, and the fix depends on who picked the name.
    #[test]
    fn a_retired_model_names_whoever_chose_it() {
        let copilot = r#"× Model call failed: {"code":"model_not_supported"}"#;
        let codex = "The 'gpt-5.1-codex-max' model is not supported when using \
                     Codex with a ChatGPT account.";
        for text in [copilot, codex] {
            let ours = model_fix_hint(AgentKind::Codex, Some("gpt-5.1-codex-max"), text)
                .expect("recognised as a model error");
            assert!(ours.contains("Settings → Models"), "{ours}");
            assert!(ours.contains("gpt-5.1-codex-max"), "{ours}");

            // Nothing set on our side: the CLI itself is the suspect, and
            // updating it is the fix that actually worked in the field.
            let theirs =
                model_fix_hint(AgentKind::Codex, None, text).expect("recognised as a model error");
            assert!(theirs.contains("outdated"), "{theirs}");
            assert!(theirs.contains("update it"), "{theirs}");
        }

        // copilot 1.x's phrasing for a bad --model value (verified live).
        let flag = r#"Error: Model "fake-model-9000" from --model flag is not available."#;
        assert!(model_fix_hint(AgentKind::Copilot, Some("fake-model-9000"), flag).is_some());

        // An unrelated failure is not a model failure.
        assert!(model_fix_hint(AgentKind::Codex, None, "connection reset").is_none());
    }

    /// Effort ladders differ in length by CLI, and switching providers must
    /// not carry a level the new one has never heard of into its argv.
    #[test]
    fn an_effort_the_cli_does_not_offer_is_dropped() {
        // codex tops out where OpenAI's reasoning_effort does.
        assert_eq!(
            AgentCli::configured(AgentKind::Codex, "", "high").effort,
            Some("high".to_string())
        );
        assert!(AgentCli::configured(AgentKind::Codex, "", "max")
            .effort
            .is_none());
        // claude carries two rungs above it.
        assert_eq!(
            AgentCli::configured(AgentKind::Claude, "", "max").effort,
            Some("max".to_string())
        );
        // And a CLI with no effort control never gets one.
        assert!(AgentKind::Bob.efforts().is_empty());
        assert!(AgentCli::configured(AgentKind::Bob, "", "high")
            .effort
            .is_none());
    }

    /// Every advertised level must be one the CLI's own flag accepts — the
    /// ladders are read off `--help`, so a typo here is a broken run.
    #[test]
    fn advertised_efforts_are_ordered_cheapest_first() {
        const ORDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
        for kind in AgentKind::ALL {
            let ranks: Vec<usize> = kind
                .efforts()
                .iter()
                .map(|e| ORDER.iter().position(|o| o == e).expect("known level"))
                .collect();
            assert!(
                ranks.windows(2).all(|w| w[0] < w[1]),
                "{} ladder out of order: {:?}",
                kind.binary_name(),
                kind.efforts()
            );
        }
    }

    /// The provider's model field reaches the CLI, and blank still means "the
    /// CLI's own default" — the state every existing entry is saved in.
    #[test]
    fn opencode_error_events_surface_the_provider_sentence() {
        // Real 402 shape from `opencode run --format json` with a spent
        // Copilot quota: the useful sentence is buried in an embedded body.
        let ev = serde_json::json!({
            "name": "APIError",
            "data": {
                "message": "Payment Required: {\"error\":{\"message\":\"You have exceeded your monthly quota\",\"code\":\"quota_exceeded\"}}",
                "statusCode": 402
            }
        });
        assert_eq!(
            opencode_error_message(&ev),
            "Payment Required: You have exceeded your monthly quota"
        );
        // No embedded body: passed through untouched.
        let plain = serde_json::json!({ "data": { "message": "session expired" } });
        assert_eq!(opencode_error_message(&plain), "session expired");
        // Nothing usable at all still names the source rather than panicking.
        let empty = serde_json::json!({});
        assert!(!opencode_error_message(&empty).is_empty());
    }

    #[test]
    fn a_blank_model_stays_unset() {
        assert!(AgentCli::configured(AgentKind::Codex, "", "")
            .model
            .is_none());
        assert!(AgentCli::configured(AgentKind::Codex, "   ", "")
            .model
            .is_none());
        assert_eq!(
            AgentCli::configured(AgentKind::Codex, " gpt-5.1-codex ", "").model,
            Some("gpt-5.1-codex".to_string())
        );
    }
}

#[cfg(test)]
mod live_smokes {
    use super::*;

    /// The positional-prompt shape (bobshell deprecated `-p`) against the real
    /// CLI, plus the no-banner deadline: bob prints nothing until its answer.
    ///   cargo test agent_cli_bob_smoke -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn agent_cli_bob_smoke() {
        let cli = AgentCli::configured(AgentKind::Bob, "", "");
        let out = cli
            .chat(&[ChatTurn::user("What is 2+2? Reply with only the number.")])
            .await
            .expect("bob chat failed");
        assert!(out.text.contains('4'), "unexpected: {}", out.text);
    }

    /// Effort reaches each CLI through its own flag, and the run still
    /// succeeds — the flags differ enough (`--effort`, a TOML `-c` override,
    /// `--thinking`, `--variant`) that only a live run proves the spelling.
    ///   cargo test agent_cli_effort_smoke -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn agent_cli_effort_smoke() {
        for (kind, effort) in [
            (AgentKind::Codex, "low"),
            (AgentKind::Claude, "low"),
            (AgentKind::Pi, "low"),
        ] {
            let cli = AgentCli::configured(kind, "", effort);
            let out = cli
                .chat(&[ChatTurn::user("What is 2+2? Reply with only the number.")])
                .await
                .unwrap_or_else(|e| panic!("{} at effort {effort}: {e:#}", kind.binary_name()));
            assert!(
                out.text.contains('4'),
                "{}: unexpected {}",
                kind.binary_name(),
                out.text
            );
        }
    }

    /// The full copilot path — `-p` prompt, `--model auto` default, footer
    /// on stderr never in the answer.
    ///   cargo test agent_cli_copilot_smoke -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn agent_cli_copilot_smoke() {
        let cli = AgentCli::configured(AgentKind::Copilot, "", "");
        let out = cli
            .chat(&[ChatTurn::user("What is 2+2? Reply with only the number.")])
            .await
            .expect("copilot chat failed");
        assert!(out.text.contains('4'), "unexpected: {}", out.text);
        assert!(
            !out.text.contains("AI Credits") && !out.text.contains("Changes"),
            "footer leaked into the answer: {}",
            out.text
        );
    }

    /// cargo test agent_cli_opencode_smoke -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn agent_cli_opencode_smoke() {
        let cli = AgentCli::configured(AgentKind::Opencode, "", "");
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
        let cli = AgentCli::configured(AgentKind::Pi, "", "");
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
        let cli = AgentCli::configured(AgentKind::Prime, "", "");
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
        let cli = AgentCli::configured(AgentKind::Hermes, "", "");
        let out = cli
            .chat(&[ChatTurn::user("What is 2+2? Reply with only the number.")])
            .await
            .expect("hermes chat failed");
        assert!(out.text.contains('4'), "unexpected: {}", out.text);
    }
}
