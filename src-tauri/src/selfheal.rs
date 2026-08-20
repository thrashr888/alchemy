//! Self-resolve (docs/RFC-self-resolve.md), phases 2 and 3.
//!
//! Phase 3 — the `settings` tool: get/set over a strict allowlist of
//! `AiConfig` fields, reachable from the chat tool router, the MCP server,
//! and phase-2 fix buttons. Secrets never pass in either direction: key-ish
//! fields are refused on write and reads redact anything key-shaped.
//!
//! Phase 2 — diagnose-and-suggest: when the phase-1 deterministic classifier
//! doesn't recognize an error, one Small-role call turns the raw error plus
//! a REDACTED config snapshot into a two-sentence diagnosis and fixes chosen
//! from a fixed action vocabulary. Parse-or-skip: an unparseable diagnosis
//! is dropped, never shown. The diagnosing model is never the failing engine.

use crate::ai::{Ai, AiConfig, ChatTurn, ProviderEntry};

// ---- Redaction --------------------------------------------------------------

/// Strip userinfo credentials from a URL: `scheme://user:pass@host/x` →
/// `scheme://host/x`. Anything else passes through unchanged.
pub(crate) fn redact_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{}{}", &url[..scheme_end + 3], &rest[at + 1..]),
        None => url.to_string(),
    }
}

/// Does this token look like an API key / bearer token? Conservative: known
/// key prefixes, or a long unbroken alphanumeric run mixing letters and
/// digits (dots exclude hostnames and model names like `gpt-oss:20b`).
pub(crate) fn looks_key_shaped(token: &str) -> bool {
    let t = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
    if t.is_empty() {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    const PREFIXES: [&str; 8] = [
        "sk-", "ntn_", "ghp_", "gho_", "xoxb-", "xoxp-", "eyj", "bearer:",
    ];
    if t.len() >= 12 && PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    t.len() >= 28
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().any(|c| c.is_ascii_alphabetic())
}

/// Replace key-shaped tokens with `•••`, preserving all other text and
/// whitespace. Belt-and-suspenders behind the structural redaction: applied
/// to error text and to anything the settings tool prints.
pub(crate) fn redact_key_shaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if !token.is_empty() {
            if looks_key_shaped(token) {
                out.push_str("•••");
            } else {
                out.push_str(token);
            }
            token.clear();
        }
    };
    for c in text.chars() {
        if c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')' | ',' | ';') {
            flush(&mut token, &mut out);
            out.push(c);
        } else {
            token.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}

/// Scrub an error string before it reaches a model prompt: exact occurrences
/// of every configured secret, URL userinfo, then key-shaped tokens.
pub(crate) fn redact_error(raw: &str, config: &AiConfig) -> String {
    let mut out = raw.to_string();
    let mut secrets: Vec<&str> = config
        .providers
        .iter()
        .map(|p| p.api_key.as_str())
        .collect();
    secrets.push(&config.openai_api_key);
    secrets.push(&config.notion_token);
    for secret in secrets {
        if secret.len() >= 4 {
            out = out.replace(secret, "•••");
        }
    }
    // URL userinfo: rewrite each whitespace-delimited token that parses as a
    // URL, then the generic key-shape pass.
    out = out
        .split(' ')
        .map(|tok| {
            if tok.contains("://") && tok.contains('@') {
                redact_url(tok)
            } else {
                tok.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    redact_key_shaped(&out)
}

// ---- Phase 3: the settings tool ---------------------------------------------

/// Fields that can never travel through this tool, in either direction.
pub(crate) fn is_secret_field(field: &str) -> bool {
    let l = field.to_ascii_lowercase();
    ["key", "token", "secret", "password", "credential"]
        .iter()
        .any(|k| l.contains(k))
}

fn provider_display(p: &ProviderEntry) -> String {
    if p.chat_model.trim().is_empty() {
        p.label.clone()
    } else {
        format!("{} · {}", p.label, p.chat_model.trim())
    }
}

fn find_provider<'a>(config: &'a AiConfig, needle: &str) -> Option<&'a ProviderEntry> {
    let n = needle.trim();
    config
        .providers
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(n) || p.label.eq_ignore_ascii_case(n))
        .or_else(|| {
            // Friendly aliases for the on-device model.
            let l = n.to_ascii_lowercase();
            if [
                "apple intelligence",
                "apple",
                "on-device",
                "foundation models",
            ]
            .contains(&l.as_str())
            {
                config.providers.iter().find(|p| p.kind == "fm")
            } else {
                None
            }
        })
}

fn provider_roster(config: &AiConfig) -> String {
    config
        .providers
        .iter()
        .map(|p| format!("\"{}\" ({})", p.id, p.label))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Read-only settings snapshot for chat/MCP, fully redacted: API keys are
/// omitted entirely, base URLs lose any credentials, and the whole output
/// takes a final key-shape pass so nothing key-ish can leak through labels
/// or model names either.
pub(crate) fn settings_get(config: &AiConfig) -> String {
    let mut out = String::from("Current AI settings:\n");
    let describe = |id: &str| {
        config
            .provider_by_id(id)
            .map(provider_display)
            .unwrap_or_else(|| id.to_string())
    };
    out.push_str(&format!("- Chat: {}\n", describe(&config.chat_provider)));
    out.push_str(&format!(
        "- Studio: {}\n",
        describe(&config.studio_provider)
    ));
    out.push_str(&format!(
        "- Small model: {}\n",
        if config.small_model.trim().is_empty() {
            "auto (Apple on-device when available)"
        } else {
            config.small_model.trim()
        }
    ));
    out.push_str(&format!(
        "- Embedder: {} ({})\n",
        config.embedder, config.embed_model
    ));
    out.push_str("\nProviders:\n");
    for p in &config.providers {
        let mut line = format!("- {} — id \"{}\", kind {}", p.label, p.id, p.kind);
        if !p.chat_model.trim().is_empty() {
            line.push_str(&format!(", model {}", p.chat_model.trim()));
        }
        if !p.effort.trim().is_empty() {
            line.push_str(&format!(", effort {}", p.effort.trim()));
        }
        if !p.base_url.trim().is_empty() {
            line.push_str(&format!(", url {}", redact_url(p.base_url.trim())));
        }
        if !p.api_key.is_empty() {
            line.push_str(", key set (hidden)");
        }
        out.push_str(&line);
        out.push('\n');
    }
    let unset = |s: &str| {
        if s.trim().is_empty() {
            "(not set)".to_string()
        } else {
            s.trim().to_string()
        }
    };
    out.push_str(&format!(
        "\nProfile:\n- Name: {}\n- Profession: {}\n- Standing instructions: {}\n",
        unset(&config.profile.name),
        unset(&config.profile.profession),
        unset(&config.profile.instructions)
    ));
    out.push_str(
        "\nAPI keys are never shown here — manage them in Settings → Models. \
         Say \"switch chat to <provider>\" to change providers.",
    );
    redact_key_shaped(&out)
}

const SETTABLE_FIELDS: &str = "chatProvider, studioProvider, chatModel, effort, baseUrl, \
     smallModel, embedder, provider.<id>.chatModel, provider.<id>.effort, provider.<id>.baseUrl, \
     profile.name, profile.profession, profile.instructions";

const EFFORTS: [&str; 5] = ["", "minimal", "low", "medium", "high"];

/// Apply one allowlisted settings change to `config`, returning the
/// transcript echo line ("Switched chat to Ollama · gemma3"). Err carries a
/// polite refusal for the transcript. The caller persists the config (and
/// rebuilds Ai) only on Ok — this function never touches disk.
pub(crate) fn settings_set(
    config: &mut AiConfig,
    field: &str,
    value: &str,
) -> Result<String, String> {
    let field = field.trim();
    if is_secret_field(field) {
        return Err(
            "API keys and tokens can't be read or changed through this tool — \
             set them in Settings → Models."
                .to_string(),
        );
    }
    let value = value.trim();
    if looks_key_shaped(value) {
        return Err(
            "That value looks like an API key — keys can't be set through this \
             tool. Paste it in Settings → Models instead."
                .to_string(),
        );
    }

    // `provider.<id>.<sub>` targets a specific entry; the bare field names
    // target the active chat provider's entry.
    let (target_id, sub) = match field.strip_prefix("provider.") {
        Some(rest) => match rest.rsplit_once('.') {
            Some((id, sub)) => (Some(id.to_string()), sub.to_string()),
            None => {
                return Err(format!(
                    "\"{field}\" isn't a setting I can change — the fields are: {SETTABLE_FIELDS}."
                ))
            }
        },
        None => (None, field.to_string()),
    };

    match sub.as_str() {
        "chatProvider" | "chat" if target_id.is_none() => {
            let p = find_provider(config, value).ok_or_else(|| {
                format!(
                    "No provider matches \"{value}\" — configured providers: {}.",
                    provider_roster(config)
                )
            })?;
            let echo = format!("Switched chat to {}", provider_display(p));
            config.chat_provider = p.id.clone();
            Ok(echo)
        }
        "studioProvider" | "studio" if target_id.is_none() => {
            let p = find_provider(config, value).ok_or_else(|| {
                format!(
                    "No provider matches \"{value}\" — configured providers: {}.",
                    provider_roster(config)
                )
            })?;
            let echo = format!("Switched studio to {}", provider_display(p));
            config.studio_provider = p.id.clone();
            Ok(echo)
        }
        "smallModel" if target_id.is_none() => {
            config.small_model = value.to_string();
            Ok(if value.is_empty() {
                "Reset the Small model to automatic (Apple on-device when available).".to_string()
            } else {
                format!("Set the Small model to {value}")
            })
        }
        "embedder" if target_id.is_none() => match value {
            "ollama" | "builtin" => {
                config.embedder = value.to_string();
                Ok(format!("Switched the embedder to {value}"))
            }
            _ => Err("The embedder can be \"ollama\" or \"builtin\".".to_string()),
        },
        // Personalization (RFC-conversational-setup §2): free text, but the
        // key-shape refusal above still applies — "remember this token for
        // me" can never smuggle a secret into the prompt-borne profile.
        "profile.name" | "profile.profession" | "profile.instructions" if target_id.is_none() => {
            let cap = if sub == "profile.instructions" {
                500
            } else {
                80
            };
            if value.chars().count() > cap {
                return Err(format!(
                    "That's too long for {sub} — keep it under {cap} characters."
                ));
            }
            match sub.as_str() {
                "profile.name" => {
                    config.profile.name = value.to_string();
                    Ok(if value.is_empty() {
                        "Cleared your name from the profile.".to_string()
                    } else {
                        format!("Got it — I'll call you {value}.")
                    })
                }
                "profile.profession" => {
                    config.profile.profession = value.to_string();
                    Ok(if value.is_empty() {
                        "Cleared your profession from the profile.".to_string()
                    } else {
                        format!("Noted your profession: {value}.")
                    })
                }
                _ => {
                    config.profile.instructions = value.to_string();
                    Ok(if value.is_empty() {
                        "Cleared your standing instructions.".to_string()
                    } else {
                        format!("Standing instructions saved: “{value}”")
                    })
                }
            }
        }
        "chatModel" | "model" | "effort" | "baseUrl" => {
            let id = match &target_id {
                Some(id) => id.clone(),
                None => config.chat_provider.clone(),
            };
            let roster = provider_roster(config);
            let Some(entry) = config
                .providers
                .iter_mut()
                .find(|p| p.id.eq_ignore_ascii_case(&id) || p.label.eq_ignore_ascii_case(&id))
            else {
                return Err(format!(
                    "No provider matches \"{id}\" — configured providers: {roster}."
                ));
            };
            let label = entry.label.clone();
            match sub.as_str() {
                "chatModel" | "model" => {
                    if value.len() > 128 || value.chars().any(char::is_whitespace) {
                        return Err("That doesn't look like a model name.".to_string());
                    }
                    entry.chat_model = value.to_string();
                    Ok(if value.is_empty() {
                        format!("Reset {label} to its default model")
                    } else {
                        format!("Set the {label} model to {value}")
                    })
                }
                "effort" => {
                    if !EFFORTS.contains(&value) {
                        return Err(
                            "Reasoning effort can be minimal, low, medium, high, or empty \
                             for the provider default."
                                .to_string(),
                        );
                    }
                    entry.effort = value.to_string();
                    Ok(if value.is_empty() {
                        format!("Reset {label} reasoning effort to the default")
                    } else {
                        format!("Set {label} reasoning effort to {value}")
                    })
                }
                _ => {
                    // baseUrl. Credentialed URLs are secrets by another name.
                    if !(value.starts_with("http://") || value.starts_with("https://")) {
                        return Err("Base URLs must start with http:// or https://.".to_string());
                    }
                    if redact_url(value) != value {
                        return Err(
                            "That URL embeds credentials — set keys in Settings → Models, \
                             and use a bare base URL here."
                                .to_string(),
                        );
                    }
                    entry.base_url = value.to_string();
                    Ok(format!("Set the {label} base URL to {value}"))
                }
            }
        }
        _ => Err(format!(
            "\"{field}\" isn't a setting I can change — the fields are: {SETTABLE_FIELDS}."
        )),
    }
}

// ---- Phase 2: diagnose-and-suggest ------------------------------------------

/// Fixed action vocabulary the diagnosing model chooses from. It picks
/// verbs; it never authors shell or free-form config.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FixAction {
    OpenSettings { tab: String },
    SwitchProvider { role: String, provider_id: String },
    Retry,
    Terminal { command: String },
}

pub(crate) struct Diagnosis {
    pub text: String,
    pub actions: Vec<FixAction>,
}

const SETTINGS_TABS: [&str; 5] = ["models", "general", "sources", "studio", "agents"];
const MAX_DIAGNOSIS_CHARS: usize = 400;
const MAX_ACTIONS: usize = 3;

/// Parse-or-skip: one JSON object with a non-empty `diagnosis` string and
/// actions drawn strictly from the vocabulary. Anything else — no JSON, an
/// empty diagnosis, an unknown action, a command outside the terminal
/// allowlist, a provider not in the config — is dropped (per-action or
/// wholesale), never shown.
pub(crate) fn parse_diagnosis(raw: &str, config: &AiConfig) -> Option<Diagnosis> {
    let json = crate::agent::extract_json(raw)?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let text: String = v
        .get("diagnosis")?
        .as_str()?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return None;
    }
    let text: String = text.chars().take(MAX_DIAGNOSIS_CHARS).collect();
    // The model saw only redacted input, but its output goes to the user —
    // scrub once more so a hallucinated key-shape can't render.
    let text = redact_key_shaped(&text);

    let mut actions: Vec<FixAction> = Vec::new();
    for a in v
        .get("actions")
        .and_then(|x| x.as_array())
        .map(|x| x.as_slice())
        .unwrap_or_default()
    {
        let s = |k: &str| {
            a.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let parsed = match a.get("action").and_then(|x| x.as_str()).unwrap_or("") {
            "open_settings" => {
                let tab = s("tab").to_ascii_lowercase();
                SETTINGS_TABS
                    .contains(&tab.as_str())
                    .then_some(FixAction::OpenSettings { tab })
            }
            "switch_provider" => {
                let role = s("role");
                let ok_role = role == "chat" || role == "studio";
                let provider = find_provider(config, &s("provider"));
                match (ok_role, provider) {
                    (true, Some(p)) => Some(FixAction::SwitchProvider {
                        role,
                        provider_id: p.id.clone(),
                    }),
                    _ => None,
                }
            }
            "retry" => Some(FixAction::Retry),
            "terminal" => {
                let command = s("command");
                crate::commands::terminal_command_allowed(&command)
                    .then_some(FixAction::Terminal { command })
            }
            _ => None,
        };
        if let Some(p) = parsed {
            if !actions.contains(&p) {
                actions.push(p);
            }
        }
        if actions.len() >= MAX_ACTIONS {
            break;
        }
    }
    Some(Diagnosis { text, actions })
}

/// Render a diagnosis into the grammar the error row parses: the phase-1
/// literal grammars (`Fix: open Terminal, run `cmd``, `Settings → <Tab>`)
/// plus phase 3's `Fix: switch <role> to provider `<id>``. Retry renders
/// nothing — the error row already carries a Retry button.
pub(crate) fn render_diagnosis(config: &AiConfig, d: &Diagnosis) -> String {
    let mut out = format!("\n\nDiagnosis: {}", d.text);
    for a in &d.actions {
        match a {
            FixAction::Terminal { command } => {
                out.push_str(&format!(
                    "\nFix: open Terminal, run `{command}`, then retry here."
                ));
            }
            FixAction::OpenSettings { tab } => {
                let mut label = tab.clone();
                if let Some(first) = label.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                out.push_str(&format!("\nCheck Settings → {label}."));
            }
            FixAction::SwitchProvider { role, provider_id } => {
                let label = config
                    .provider_by_id(provider_id)
                    .map(|p| p.label.clone())
                    .unwrap_or_else(|| provider_id.clone());
                out.push_str(&format!(
                    "\nFix: switch {role} to provider `{provider_id}` ({label})."
                ));
            }
            FixAction::Retry => {}
        }
    }
    out
}

/// Should this raw error go to the diagnosis loop? Only when nothing cheaper
/// already answered: phase 1's classifier didn't match, no upstream layer
/// attached its own fix, and it isn't the schema-skew arm.
pub(crate) fn needs_diagnosis(raw: &str) -> bool {
    !raw.contains("Fix:")
        && !raw.contains("Settings → Models")
        && !raw.contains("Append with different schema")
        && crate::commands::classify_model_error(raw).is_none()
}

const DIAGNOSIS_SYSTEM: &str = "You diagnose AI-provider errors inside Alchemy, a local-first \
research notebook app, and suggest fixes a user can click. \
Respond with EXACTLY ONE JSON object, nothing else:\n\
{\"diagnosis\":\"<at most two short plain-language sentences>\",\"actions\":[...]}\n\n\
Allowed actions (choose at most 3 that would actually help; the list may be empty):\n\
- {\"action\":\"open_settings\",\"tab\":\"models\"} — open a Settings tab (models or general).\n\
- {\"action\":\"switch_provider\",\"role\":\"chat\",\"provider\":\"<a provider id from the list>\"} — suggest answering with a different configured provider (role: chat or studio).\n\
- {\"action\":\"retry\"} — the failure looks transient.\n\
- {\"action\":\"terminal\",\"command\":\"ollama serve\"} — ONLY the commands `ollama serve` or `ollama pull <model>`.\n\n\
Never invent shell commands, URLs, or config values. Never include keys or tokens. \
If unsure, suggest open_settings.";

/// Redacted config snapshot for the diagnosis prompt: provider kinds,
/// labels, ids, and model names — never keys, never URLs with credentials.
pub(crate) fn config_snapshot(config: &AiConfig) -> String {
    let mut out = String::new();
    for p in &config.providers {
        let mut line = format!("- id \"{}\", kind {}, label \"{}\"", p.id, p.kind, p.label);
        if !p.chat_model.trim().is_empty() {
            line.push_str(&format!(", model {}", p.chat_model.trim()));
        }
        if !p.base_url.trim().is_empty() {
            line.push_str(&format!(", url {}", redact_url(p.base_url.trim())));
        }
        if p.id == config.chat_provider {
            line.push_str(" [chat]");
        }
        if p.id == config.studio_provider {
            line.push_str(" [studio]");
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&format!(
        "embedder: {} ({})\n",
        config.embedder, config.embed_model
    ));
    redact_key_shaped(&out)
}

/// One capped diagnosis call (RFC-self-resolve phase 2). Returns the text to
/// append to the error row, or None to show the phase-1 output untouched:
/// the toggle is off, the shape was already classified, no engine other than
/// the failing one is alive, the call failed, or the output didn't parse.
pub(crate) async fn diagnose(ai: &Ai, raw: &str) -> Option<String> {
    let config = ai.config();
    if !config.self_diagnose || !needs_diagnosis(raw) {
        return None;
    }
    let failing_id = ai.chat_engine_id(crate::ai::Role::Chat);
    let engine = ai.diagnosis_engine(failing_id).await?;
    let failing_label = config
        .provider_by_id(&config.chat_provider)
        .map(|p| p.label.clone())
        .unwrap_or_else(|| config.chat_provider.clone());
    let redacted: String = redact_error(raw, config).chars().take(1500).collect();
    let messages = vec![
        ChatTurn::system(DIAGNOSIS_SYSTEM.to_string()),
        ChatTurn::user(format!(
            "Configured providers (redacted):\n{}\nThe chat provider \"{failing_label}\" \
             just failed with this error:\n{redacted}\n\nOne JSON object:",
            config_snapshot(config)
        )),
    ];
    let out = tokio::time::timeout(std::time::Duration::from_secs(30), engine.chat(&messages))
        .await
        .ok()?
        .ok()?;
    let d = parse_diagnosis(&out.text, config)?;
    Some(render_diagnosis(config, &d))
}

// ---- Model verbs (RFC-conversational-setup phase 1) -------------------------
//
// `models`, `test`, and `pull` grow the settings tool into onboarding. The
// pure halves live here (target resolution, rendering, the pull command's
// charset gate); the live probes stay in commands.rs beside the readiness
// machinery they reuse.

/// What a `test <target>` invocation resolved to. Provider targets probe the
/// configured entry's engine; a bare model name probes Ollama with that
/// model — the pre-commitment "is it any good" check.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TestTarget {
    /// A configured provider, by id.
    Provider(String),
    /// An installed (or about-to-be-tested) Ollama model name.
    OllamaModel(String),
}

/// Resolve a `test` target: empty means the active chat provider, otherwise
/// a provider id/label/alias, otherwise an Ollama model name — which must
/// pass the same charset gate as `pull` (the name may travel onward into a
/// terminal affordance) and must never be key-shaped.
pub(crate) fn resolve_test_target(config: &AiConfig, target: &str) -> Result<TestTarget, String> {
    let target = target.trim();
    if looks_key_shaped(target) {
        return Err(
            "That looks like an API key, not a provider or model — keys never pass \
             through this tool."
                .to_string(),
        );
    }
    if target.is_empty() {
        return Ok(TestTarget::Provider(config.chat_provider.clone()));
    }
    if let Some(p) = find_provider(config, target) {
        return Ok(TestTarget::Provider(p.id.clone()));
    }
    if crate::commands::is_safe_model_name(target) {
        return Ok(TestTarget::OllamaModel(target.to_string()));
    }
    Err(format!(
        "\"{target}\" isn't a configured provider ({}) or a valid model name.",
        provider_roster(config)
    ))
}

/// The validated `ollama pull` command for a model name — the ONLY thing
/// either surface ever hands onward, and only as text: the app never shells
/// out on its own. Charset-gated here and re-gated at execution
/// (`terminal_command_allowed`), so both ends refuse anything shell-shaped.
pub(crate) fn pull_command(model: &str) -> Result<String, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("Which model? Say e.g. \"pull gemma3\".".to_string());
    }
    if !crate::commands::is_safe_model_name(model) {
        return Err(format!(
            "\"{}\" isn't a valid Ollama model name (letters, digits, and ._:/- only, \
             at most 64 characters).",
            model.chars().take(80).collect::<String>()
        ));
    }
    Ok(format!("ollama pull {model}"))
}

/// Chat-side `pull` reply: stages the command through the same literal
/// `` Fix: open Terminal, run `cmd` `` grammar the error rows use, which the
/// transcript renders as a one-click Terminal launch. Never executed here.
pub(crate) fn settings_pull(model: &str) -> Result<String, String> {
    let command = pull_command(model)?;
    let model = model.trim();
    Ok(format!(
        "Ready to download “{model}”. Fix: open Terminal, run `{command}`, then come \
         back — Alchemy stages the command but never runs it. When the download \
         finishes, say “test {model}”."
    ))
}

/// One provider row of the `models` roster, precomputed by the live side.
pub(crate) struct ProviderStatus {
    pub label: String,
    pub model: String,
    pub ready: bool,
    pub detail: String,
    pub is_chat: bool,
    pub is_studio: bool,
}

/// The `models` roster: installed Ollama models plus each configured
/// provider's active model and readiness. Pure formatting — the live side
/// gathers the inputs — with the usual key-shape scrub on the way out.
pub(crate) fn format_models_report(
    installed: &Result<Vec<String>, String>,
    providers: &[ProviderStatus],
) -> String {
    let mut out = String::new();
    match installed {
        Ok(models) if models.is_empty() => {
            out.push_str(
                "No models installed in Ollama yet — say \"pull gemma3\" (or any model \
                 from ollama.com/library) to stage a download.\n",
            );
        }
        Ok(models) => {
            out.push_str(&format!("Installed in Ollama ({}):\n", models.len()));
            for m in models {
                out.push_str(&format!("- {m}\n"));
            }
        }
        Err(_) => {
            out.push_str("Ollama isn't reachable, so its installed models can't be listed.\n");
        }
    }
    out.push_str("\nProviders:\n");
    for p in providers {
        let mut line = format!("- {}", p.label);
        if !p.model.trim().is_empty() {
            line.push_str(&format!(" · {}", p.model.trim()));
        }
        line.push_str(if p.ready {
            " — ready"
        } else {
            " — not ready"
        });
        if !p.detail.trim().is_empty() {
            line.push_str(&format!(" ({})", p.detail.trim()));
        }
        if p.is_chat {
            line.push_str(" [chat]");
        }
        if p.is_studio {
            line.push_str(" [studio]");
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("\nSay \"test <provider or model>\" for a live probe, or \"switch chat to <provider>\" to change.");
    redact_key_shaped(&out)
}

/// One leg of a `test` probe, as measured by the live side.
pub(crate) enum ProbeResult {
    Ok { first_ms: u128, total_ms: u128 },
    Failed(String),
}

/// The `test` transcript row. Pure formatting; errors are redacted before
/// rendering (the raw may quote a provider response body).
pub(crate) fn format_test_report(
    config: &AiConfig,
    label: &str,
    chat: &ProbeResult,
    embed: Option<&(String, ProbeResult)>,
) -> String {
    let mut out = match chat {
        ProbeResult::Ok { first_ms, total_ms } => {
            format!("Tested {label}: alive — first token in {first_ms} ms, total {total_ms} ms.")
        }
        ProbeResult::Failed(err) => {
            format!("Tested {label}: failed — {}", redact_error(err, config))
        }
    };
    if let Some((embed_model, result)) = embed {
        match result {
            ProbeResult::Ok { total_ms, .. } => {
                out.push_str(&format!(" Embedding ({embed_model}): ok in {total_ms} ms."));
            }
            ProbeResult::Failed(err) => {
                out.push_str(&format!(
                    " Embedding ({embed_model}): failed — {}",
                    redact_error(err, config)
                ));
            }
        }
    }
    redact_key_shaped(&out)
}

// ---- Timeout stories (test-verb refinement) ---------------------------------

/// What the Ollama daemon looked like right after a probe timed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OllamaTimeoutContext {
    /// The daemon itself stopped answering — that IS the problem now.
    DaemonUnreachable,
    /// Daemon up and `/api/ps` shows the model — mid-load, not broken.
    Loading,
    /// Daemon up, model not (yet) listed — likely still paging in.
    AliveIdle,
}

/// A probe timeout is usually a cold load, not a broken setup — a 26GB model
/// takes longer than the probe budget to page in. Tell that story instead of
/// a bare "no answer": daemon-down gets the serve affordance, daemon-alive
/// gets "still loading, retry in a minute". `what` is "answer" | "embedding".
pub(crate) fn timeout_story(
    what: &str,
    secs: u64,
    model: &str,
    ctx: Option<OllamaTimeoutContext>,
) -> String {
    match ctx {
        Some(OllamaTimeoutContext::Loading) => format!(
            "no {what} within {secs} seconds, but the Ollama server is up and “{model}” is \
             loading into memory right now. Large models can take a few minutes on first \
             load — try again in a minute."
        ),
        Some(OllamaTimeoutContext::AliveIdle) => format!(
            "no {what} within {secs} seconds — the Ollama server is up, so “{model}” is \
             probably still loading into memory (the first use of a large model takes a \
             while). Try again in a minute."
        ),
        Some(OllamaTimeoutContext::DaemonUnreachable) => format!(
            "no {what} within {secs} seconds, and the Ollama server stopped responding — \
             it may have gone down under the load. Fix: open Terminal, run `ollama serve`, \
             then retry here."
        ),
        None => format!("no {what} within {secs} seconds"),
    }
}

// ---- Personalization: style (RFC-conversational-setup §2) -------------------

/// The non-preset style ids that predate CHAT_STYLES; "default" is the
/// frontend ChatConfig's explicit default id.
const EXTRA_STYLES: [(&str, &str); 2] = [("learning", "Learning guide"), ("custom", "Custom")];

/// Resolve a style by id, label, or unique label fragment ("google" → the
/// Google Developer Docs style). "default"/"normal"/"none" clear to default.
pub(crate) fn resolve_style(input: &str) -> Option<(&'static str, &'static str)> {
    let q = input.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    if ["default", "normal", "none", "reset"].contains(&q.as_str()) {
        return Some(("default", "Default"));
    }
    let all: Vec<(&'static str, &'static str)> = crate::rag::CHAT_STYLES
        .iter()
        .map(|(id, label, ..)| (*id, *label))
        .chain(EXTRA_STYLES.iter().copied())
        .collect();
    if let Some(hit) = all
        .iter()
        .find(|(id, label)| *id == q || label.to_lowercase() == q)
    {
        return Some(*hit);
    }
    let fragment: Vec<_> = all
        .iter()
        .filter(|(id, label)| id.contains(&q) || label.to_lowercase().contains(&q))
        .collect();
    match fragment.as_slice() {
        [one] => Some(**one),
        _ => None,
    }
}

fn style_roster() -> String {
    crate::rag::CHAT_STYLES
        .iter()
        .map(|(_, label, ..)| *label)
        .chain(["Learning guide", "Default"])
        .collect::<Vec<_>>()
        .join(", ")
}

/// Validate a per-notebook style/length change. Returns (style, length,
/// echo): None = leave that half unchanged. Pure — the caller emits the
/// event the frontend applies to its per-notebook ChatConfig.
pub(crate) fn settings_style(
    style_in: &str,
    length_in: &str,
) -> Result<(Option<String>, Option<String>, String), String> {
    let style = match style_in.trim() {
        "" => None,
        q => match resolve_style(q) {
            Some((id, label)) => Some((id.to_string(), label.to_string())),
            None => {
                return Err(format!(
                    "I don't know a “{q}” style. This notebook can answer as: {}.",
                    style_roster()
                ))
            }
        },
    };
    let length = match length_in.trim().to_lowercase().as_str() {
        "" => None,
        "default" | "normal" | "medium" => Some(("default", "default")),
        "shorter" | "short" | "concise" | "brief" => Some(("shorter", "shorter")),
        "longer" | "long" | "detailed" | "thorough" => Some(("longer", "longer")),
        other => {
            return Err(format!(
                "“{other}” isn't a length I know — say shorter, longer, or default."
            ))
        }
    };
    if style.is_none() && length.is_none() {
        return Err(format!(
            "Which style? This notebook can answer as: {} — and shorter/longer/default \
             for length.",
            style_roster()
        ));
    }
    let mut parts = Vec::new();
    if let Some((_, label)) = &style {
        parts.push(format!("style → {label}"));
    }
    if let Some((_, l)) = &length {
        parts.push(format!("length → {l} answers"));
    }
    let echo = format!("Set this notebook's chat voice: {}.", parts.join(", "));
    Ok((
        style.map(|(id, _)| id),
        length.map(|(id, _)| id.to_string()),
        echo,
    ))
}

// ---- Appearance: theme (RFC-conversational-setup §3) ------------------------

/// Mirror of the theme roster in `src/lib/themes.ts` (id, label, dark).
/// Keep in sync when themes.ts changes — the tool refuses unknown ids, so
/// drift fails safe (a new theme is just not settable from chat until added).
pub(crate) const THEME_ROSTER: &[(&str, &str, bool)] = &[
    ("midnight", "Midnight", true),
    ("light", "Light", false),
    ("slate", "Slate", true),
    ("dracula", "Dracula", true),
    ("monokai", "Monokai", true),
    ("one-dark", "One Dark", true),
    ("nord", "Nord", true),
    ("gruvbox", "Gruvbox", true),
    ("github", "GitHub", true),
    ("github-light", "GitHub Light", false),
    ("solarized", "Solarized", true),
    ("solarized-light", "Solarized Light", false),
    ("tokyo-night", "Tokyo Night", true),
    ("matrix", "Matrix", true),
    ("synthwave", "Synthwave '84", true),
    ("claude", "Claude", true),
    ("openai", "OpenAI", true),
    ("latte", "Catppuccin Latte", false),
    ("rose-pine-dawn", "Rose Pine Dawn", false),
    ("carrera", "Carrera", false),
    ("italia", "Italia", true),
    ("panigale", "Panigale", true),
    ("sepia", "Sepia", false),
];

pub(crate) fn theme_roster_text() -> String {
    let list = THEME_ROSTER
        .iter()
        .map(|(_, label, dark)| format!("{label}{}", if *dark { "" } else { " (light)" }))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Themes: {list} — or System to follow macOS. Say “use the <name> theme”.")
}

/// Fuzzy theme resolution: exact id/label first, then every query word must
/// appear in the id or label — with "dark"/"light" acting as a variant
/// filter, so "the dark rust one" finds Gruvbox. Err carries the reply for
/// an ambiguous or unmatched ask.
pub(crate) fn resolve_theme(query: &str) -> Result<(&'static str, &'static str), String> {
    let folded: String = query
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' => 'e',
            '’' | '\'' => ' ',
            c => c,
        })
        .collect();
    if folded == "system" {
        return Ok(("system", "System"));
    }
    if let Some((id, label, _)) = THEME_ROSTER
        .iter()
        .find(|(id, label, _)| *id == folded || label.to_lowercase() == folded)
    {
        return Ok((id, label));
    }
    let mut want_dark: Option<bool> = None;
    let mut tokens: Vec<String> = Vec::new();
    for tok in folded.split_whitespace() {
        match tok {
            "the" | "a" | "one" | "theme" | "mode" | "please" => {}
            "dark" => want_dark = Some(true),
            "light" => want_dark = Some(false),
            // A couple of vibe aliases users actually reach for.
            "rust" | "rusty" => tokens.push("gruvbox".into()),
            t => tokens.push(t.to_string()),
        }
    }
    if tokens.is_empty() && want_dark.is_none() {
        return Err(theme_roster_text());
    }
    let matches: Vec<&(&str, &str, bool)> = THEME_ROSTER
        .iter()
        .filter(|(id, label, dark)| {
            want_dark.is_none_or(|w| w == *dark)
                && tokens
                    .iter()
                    .all(|t| id.contains(t.as_str()) || label.to_lowercase().contains(t.as_str()))
        })
        .collect();
    match matches.as_slice() {
        [(id, label, _)] => Ok((id, label)),
        [] => Err(format!(
            "No theme matches “{}”. {}",
            query.trim(),
            theme_roster_text()
        )),
        many => Err(format!(
            "“{}” matches several themes: {} — which one?",
            query.trim(),
            many.iter()
                .map(|(_, label, _)| *label)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

// ---- Agents: connect (RFC-conversational-setup §4) --------------------------

/// The confirm-click reply for a connect ask. Connecting writes ANOTHER
/// app's config, so it NEVER auto-applies from chat: this text renders the
/// `` Confirm: connect agent `<id>` (<Name>) `` grammar the transcript turns
/// into a button, and only that click writes anything.
pub(crate) fn connect_confirm_text(id: &str, name: &str, config_path: &str) -> String {
    let file = if config_path.trim().is_empty() {
        "its config file".to_string()
    } else {
        config_path.trim().to_string()
    };
    format!(
        "Connecting Alchemy to {name} registers the MCP server and the alchemy skill by \
         writing {file}. Nothing is written until you click. Confirm: connect agent \
         `{id}` ({name})."
    )
}

/// The MCP-side refusal when `confirm: true` is missing — the one verb that
/// never applies without it, on either surface.
pub(crate) fn connect_refusal(name: &str, config_path: &str) -> String {
    format!(
        "connecting writes {name}'s config at {} — pass confirm: true to proceed",
        if config_path.trim().is_empty() {
            "its config file"
        } else {
            config_path.trim()
        }
    )
}

// ---- The guided flow: setup (RFC-conversational-setup §5) -------------------

/// Everything `setup` inspects, gathered by the live side so this decision
/// stays pure and testable.
pub(crate) struct SetupState {
    pub chat_label: String,
    pub chat_kind: String,
    pub chat_ready: bool,
    pub chat_detail: String,
    /// The chat model an Ollama chat provider would run.
    pub chat_model: String,
    pub chat_model_installed: bool,
    pub fm_ready: bool,
    pub ollama_reachable: bool,
    pub embedder: String,
    pub embed_model: String,
    pub embed_model_installed: bool,
    pub profile_named: bool,
    /// (id, name, installed, configured) per known agent client.
    pub connectors: Vec<(String, String, bool, bool)>,
}

/// One next unmet step per invocation — never a checklist dump, never
/// auto-advancing. Each step renders through the existing button grammars
/// (Terminal fix, switch-provider, Settings tab, connect confirm), so the
/// user walks the whole path with clicks and re-asks.
pub(crate) fn setup_next_step(s: &SetupState) -> String {
    let fm_offer = if s.fm_ready && s.chat_kind != "fm" {
        "\nOr answer on-device instead: Fix: switch chat to provider `on-device` \
         (On this Mac)."
    } else {
        ""
    };
    // 1. A provider that answers.
    if !s.chat_ready {
        if s.chat_kind == "ollama" && !s.ollama_reachable {
            return format!(
                "Setup 1 of 5 — start your model engine. Chat is set to {}, but the Ollama \
                 server isn't running. Fix: open Terminal, run `ollama serve`, then say \
                 “help me get set up” again.{fm_offer}",
                s.chat_label
            );
        }
        return format!(
            "Setup 1 of 5 — get a provider answering. {} isn't ready ({}). Check \
             Settings → Models.{fm_offer}",
            s.chat_label,
            if s.chat_detail.trim().is_empty() {
                "not responding"
            } else {
                s.chat_detail.trim()
            }
        );
    }
    // 2. A chat model on disk (Ollama chat only — other kinds carry theirs).
    if s.chat_kind == "ollama" && !s.chat_model_installed {
        if let Ok(cmd) = pull_command(&s.chat_model) {
            return format!(
                "Setup 2 of 5 — download your chat model. “{}” isn't installed yet. Fix: \
                 open Terminal, run `{cmd}`, then say “test {}”.",
                s.chat_model, s.chat_model
            );
        }
        return format!(
            "Setup 2 of 5 — pick a chat model. The configured model “{}” doesn't look like \
             an Ollama model name — choose one in Settings → Models.",
            s.chat_model
        );
    }
    // 3. The embedder. Builtin needs nothing — that's the point of it.
    if s.embedder == "ollama" && !s.embed_model_installed {
        let fix = match pull_command(&s.embed_model) {
            Ok(cmd) => format!("Fix: open Terminal, run `{cmd}`"),
            Err(_) => "pick one in Settings → Models".to_string(),
        };
        return format!(
            "Setup 3 of 5 — the embedder. Retrieval embeds with “{}”, which isn't \
             installed. {fix} — or say “switch the embedder to builtin” to use the \
             built-in one (no download).",
            s.embed_model
        );
    }
    // 4. Who you are.
    if !s.profile_named {
        return "Setup 4 of 5 — make it yours. Tell me what to call you (“call me Paul”), \
                and add a profession or standing instructions any time (“my profession \
                is …”, or Settings → General)."
            .to_string();
    }
    // 5. Agents (optional).
    if let Some((id, name, _, _)) = s
        .connectors
        .iter()
        .find(|(_, _, installed, configured)| *installed && !configured)
    {
        return format!(
            "Setup 5 of 5 — connect your agents (optional). {name} is installed but not \
             connected to Alchemy yet. {} Or ignore this step — everything else is ready.",
            connect_confirm_text(id, name, "")
        );
    }
    format!(
        "You're set up: chat answers with {}, retrieval embeds with {}, and your profile \
         is in. Ask anything — or say “what models do I have” any time.",
        s.chat_label,
        if s.embedder == "builtin" {
            "the built-in embedder".to_string()
        } else {
            format!("{} ({})", s.embedder, s.embed_model)
        }
    )
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> AiConfig {
        let mut c = AiConfig {
            providers: vec![
                ProviderEntry {
                    id: "on-device".into(),
                    kind: "fm".into(),
                    label: "On this Mac".into(),
                    ..Default::default()
                },
                ProviderEntry {
                    id: "ollama".into(),
                    kind: "ollama".into(),
                    label: "Ollama".into(),
                    base_url: "http://localhost:11434".into(),
                    chat_model: "gpt-oss:20b".into(),
                    ..Default::default()
                },
                ProviderEntry {
                    id: "gateway".into(),
                    kind: "gateway".into(),
                    label: "Gateway".into(),
                    base_url: "https://api.example.com/v1".into(),
                    api_key: "sk-secret1234567890abcdef".into(),
                    chat_model: "big-model".into(),
                    ..Default::default()
                },
            ],
            chat_provider: "gateway".into(),
            studio_provider: "ollama".into(),
            ..Default::default()
        };
        c.openai_api_key = "sk-secret1234567890abcdef".into();
        c
    }

    // -- Secrets: neither readable nor writable ------------------------------

    #[test]
    fn secret_fields_are_refused_on_write() {
        let mut c = test_config();
        for field in [
            "apiKey",
            "provider.gateway.apiKey",
            "openaiApiKey",
            "notionToken",
            "api_key",
            "password",
        ] {
            let err = settings_set(&mut c, field, "sk-whatever").unwrap_err();
            assert!(err.contains("can't"), "{field}: {err}");
        }
        // And a key-shaped VALUE is refused regardless of field.
        let err = settings_set(&mut c, "chatModel", "sk-abcdef1234567890").unwrap_err();
        assert!(err.contains("API key"), "{err}");
    }

    #[test]
    fn reads_never_leak_keys() {
        let c = test_config();
        let out = settings_get(&c);
        assert!(!out.contains("sk-secret"), "{out}");
        assert!(!out.contains("secret1234567890"), "{out}");
        // The keyed provider is still described as keyed.
        assert!(out.contains("key set (hidden)"), "{out}");
    }

    #[test]
    fn reads_redact_credentialed_urls() {
        let mut c = test_config();
        c.providers[2].base_url = "https://user:hunter2@api.example.com/v1".into();
        let out = settings_get(&c);
        assert!(!out.contains("hunter2"), "{out}");
        assert!(out.contains("https://api.example.com/v1"), "{out}");
    }

    #[test]
    fn snapshot_for_the_prompt_is_redacted() {
        let mut c = test_config();
        c.providers[2].base_url = "https://user:hunter2@api.example.com/v1".into();
        let snap = config_snapshot(&c);
        assert!(!snap.contains("sk-secret"), "{snap}");
        assert!(!snap.contains("hunter2"), "{snap}");
        assert!(snap.contains("[chat]"), "{snap}");
        assert!(snap.contains("kind ollama"), "{snap}");
    }

    #[test]
    fn error_text_is_scrubbed_before_prompting() {
        let c = test_config();
        let raw = "401 from https://user:hunter2@api.example.com/v1/chat \
                   with key sk-secret1234567890abcdef (request id abc)";
        let red = redact_error(raw, &c);
        assert!(!red.contains("sk-secret"), "{red}");
        assert!(!red.contains("hunter2"), "{red}");
        assert!(red.contains("401"), "{red}");
    }

    #[test]
    fn key_shape_heuristic() {
        assert!(looks_key_shaped("sk-abcdef1234567890"));
        assert!(looks_key_shaped("ghp_16C7e42F292c6912E7710c838347Ae178B4a"));
        // Model names, hosts, and short words survive.
        assert!(!looks_key_shaped("gpt-oss:20b"));
        assert!(!looks_key_shaped("localhost:11434"));
        assert!(!looks_key_shaped("api.example.com"));
        assert!(!looks_key_shaped("connection"));
    }

    #[test]
    fn redact_url_strips_userinfo_only() {
        assert_eq!(redact_url("https://u:p@host.com/v1"), "https://host.com/v1");
        assert_eq!(
            redact_url("http://localhost:11434"),
            "http://localhost:11434"
        );
        assert_eq!(redact_url("not a url"), "not a url");
    }

    // -- The set allowlist ----------------------------------------------------

    #[test]
    fn switches_provider_by_id_label_or_alias() {
        let mut c = test_config();
        let echo = settings_set(&mut c, "chatProvider", "ollama").unwrap();
        assert_eq!(c.chat_provider, "ollama");
        assert!(
            echo.contains("Switched chat to Ollama · gpt-oss:20b"),
            "{echo}"
        );

        let echo = settings_set(&mut c, "chatProvider", "apple intelligence").unwrap();
        assert_eq!(c.chat_provider, "on-device");
        assert!(echo.contains("On this Mac"), "{echo}");

        let echo = settings_set(&mut c, "studioProvider", "Gateway").unwrap();
        assert_eq!(c.studio_provider, "gateway");
        assert!(
            echo.contains("Switched studio to Gateway · big-model"),
            "{echo}"
        );
    }

    #[test]
    fn unknown_provider_names_the_roster() {
        let mut c = test_config();
        let err = settings_set(&mut c, "chatProvider", "hal9000").unwrap_err();
        assert!(err.contains("\"ollama\""), "{err}");
        assert_eq!(c.chat_provider, "gateway"); // untouched
    }

    #[test]
    fn unknown_field_is_refused_with_the_allowlist() {
        let mut c = test_config();
        for field in [
            "notionThing",
            "mcpPort",
            "provider.ollama",
            "profile.avatar",
        ] {
            let err = settings_set(&mut c, field, "x").unwrap_err();
            assert!(err.contains("chatProvider"), "{field}: {err}");
        }
    }

    #[test]
    fn per_provider_fields_apply_to_named_or_active_entry() {
        let mut c = test_config();
        // Bare chatModel targets the active chat provider (gateway).
        settings_set(&mut c, "chatModel", "new-model").unwrap();
        assert_eq!(c.providers[2].chat_model, "new-model");
        // provider.<id>. targets that entry.
        let echo = settings_set(&mut c, "provider.ollama.chatModel", "gemma3").unwrap();
        assert_eq!(c.providers[1].chat_model, "gemma3");
        assert!(echo.contains("Ollama model to gemma3"), "{echo}");
        settings_set(&mut c, "provider.ollama.effort", "low").unwrap();
        assert_eq!(c.providers[1].effort, "low");
        let err = settings_set(&mut c, "provider.ollama.effort", "extreme").unwrap_err();
        assert!(err.contains("minimal"), "{err}");
    }

    #[test]
    fn base_url_rejects_credentials_and_non_http() {
        let mut c = test_config();
        settings_set(
            &mut c,
            "provider.ollama.baseUrl",
            "http://192.168.1.5:11434",
        )
        .unwrap();
        assert_eq!(c.providers[1].base_url, "http://192.168.1.5:11434");
        assert!(settings_set(&mut c, "baseUrl", "ftp://x").is_err());
        let err =
            settings_set(&mut c, "baseUrl", "https://me:pw12@api.example.com/v1").unwrap_err();
        assert!(err.contains("credentials"), "{err}");
    }

    #[test]
    fn small_model_and_embedder() {
        let mut c = test_config();
        settings_set(&mut c, "smallModel", "llama3.2:3b").unwrap();
        assert_eq!(c.small_model, "llama3.2:3b");
        let echo = settings_set(&mut c, "smallModel", "").unwrap();
        assert!(echo.contains("automatic"), "{echo}");
        settings_set(&mut c, "embedder", "builtin").unwrap();
        assert_eq!(c.embedder, "builtin");
        assert!(settings_set(&mut c, "embedder", "cloud").is_err());
    }

    // -- Diagnosis: parse-or-skip --------------------------------------------

    #[test]
    fn diagnosis_parses_valid_output() {
        let c = test_config();
        let raw = r#"Here you go: {"diagnosis":"The gateway rejected the request. Its key may have expired.","actions":[{"action":"open_settings","tab":"models"},{"action":"switch_provider","role":"chat","provider":"ollama"},{"action":"retry"}]}"#;
        let d = parse_diagnosis(raw, &c).expect("should parse");
        assert!(d.text.starts_with("The gateway rejected"));
        assert_eq!(d.actions.len(), 3);
        assert!(d.actions.contains(&FixAction::SwitchProvider {
            role: "chat".into(),
            provider_id: "ollama".into()
        }));
    }

    #[test]
    fn diagnosis_skips_garbage() {
        let c = test_config();
        assert!(parse_diagnosis("total nonsense, no json", &c).is_none());
        assert!(parse_diagnosis(r#"{"actions":[]}"#, &c).is_none());
        assert!(parse_diagnosis(r#"{"diagnosis":"   "}"#, &c).is_none());
        assert!(parse_diagnosis("[1,2,3]", &c).is_none());
    }

    #[test]
    fn diagnosis_drops_invalid_actions_but_keeps_the_text() {
        let c = test_config();
        let raw = r#"{"diagnosis":"Something failed.","actions":[
            {"action":"terminal","command":"rm -rf /"},
            {"action":"terminal","command":"ollama serve; curl evil"},
            {"action":"switch_provider","role":"chat","provider":"hal9000"},
            {"action":"switch_provider","role":"root","provider":"ollama"},
            {"action":"open_settings","tab":"kernel"},
            {"action":"reboot"},
            {"action":"terminal","command":"ollama serve"}
        ]}"#;
        let d = parse_diagnosis(raw, &c).expect("text is valid");
        assert_eq!(
            d.actions,
            vec![FixAction::Terminal {
                command: "ollama serve".into()
            }]
        );
    }

    #[test]
    fn diagnosis_caps_length_and_action_count() {
        let c = test_config();
        let long = "word ".repeat(500);
        let raw = format!(
            r#"{{"diagnosis":"{long}","actions":[
                {{"action":"retry"}},{{"action":"open_settings","tab":"models"}},
                {{"action":"open_settings","tab":"general"}},
                {{"action":"terminal","command":"ollama serve"}}]}}"#
        );
        let d = parse_diagnosis(&raw, &c).unwrap();
        assert!(d.text.chars().count() <= MAX_DIAGNOSIS_CHARS);
        assert_eq!(d.actions.len(), MAX_ACTIONS);
    }

    #[test]
    fn diagnosis_output_redacts_hallucinated_keys() {
        let c = test_config();
        let raw = r#"{"diagnosis":"Your key sk-abcdef1234567890 is invalid.","actions":[]}"#;
        let d = parse_diagnosis(raw, &c).unwrap();
        assert!(!d.text.contains("sk-abcdef"), "{}", d.text);
    }

    #[test]
    fn rendered_grammar_matches_the_error_row_buttons() {
        let c = test_config();
        let d = Diagnosis {
            text: "Ollama looks down.".into(),
            actions: vec![
                FixAction::Terminal {
                    command: "ollama serve".into(),
                },
                FixAction::SwitchProvider {
                    role: "chat".into(),
                    provider_id: "on-device".into(),
                },
                FixAction::OpenSettings {
                    tab: "models".into(),
                },
                FixAction::Retry,
            ],
        };
        let out = render_diagnosis(&c, &d);
        assert!(out.contains("Diagnosis: Ollama looks down."), "{out}");
        assert!(
            out.contains("Fix: open Terminal, run `ollama serve`, then retry here."),
            "{out}"
        );
        assert!(
            out.contains("Fix: switch chat to provider `on-device` (On this Mac)."),
            "{out}"
        );
        assert!(out.contains("Settings → Models"), "{out}");
        // Retry renders nothing extra — the row already has the button.
        assert!(!out.contains("retry the question"), "{out}");
    }

    // -- Model verbs (RFC-conversational-setup phase 1) ----------------------

    #[test]
    fn pull_gates_the_charset_at_staging_time() {
        // Valid names render the error-row grammar AND pass the execution
        // allowlist — the same command string survives both gates.
        for model in ["gemma3", "qwen3:8b", "hf.co/org/model:Q4_K-M"] {
            let cmd = pull_command(model).unwrap();
            assert_eq!(cmd, format!("ollama pull {model}"));
            assert!(
                crate::commands::terminal_command_allowed(&cmd),
                "{cmd} must pass the execution-side allowlist"
            );
            let staged = settings_pull(model).unwrap();
            assert!(
                staged.contains(&format!("Fix: open Terminal, run `{cmd}`")),
                "{staged}"
            );
            assert!(staged.contains("never runs it"), "{staged}");
        }
        // Shell-shaped, quoted, oversized, and empty names are refused —
        // nothing outside the charset can ever reach the affordance.
        for bad in [
            "gemma3; rm -rf /",
            "a`b`",
            "model && curl evil",
            "name with spaces",
            "$(whoami)",
            "",
            "        ",
        ] {
            assert!(pull_command(bad).is_err(), "{bad:?} should be refused");
            assert!(settings_pull(bad).is_err());
        }
        let long = "a".repeat(65);
        assert!(pull_command(&long).is_err());
    }

    #[test]
    fn test_target_resolves_provider_model_or_refuses() {
        let c = test_config();
        // Empty = the active chat provider.
        assert_eq!(
            resolve_test_target(&c, "").unwrap(),
            TestTarget::Provider("gateway".into())
        );
        // Provider by id, label, or alias.
        assert_eq!(
            resolve_test_target(&c, "Ollama").unwrap(),
            TestTarget::Provider("ollama".into())
        );
        assert_eq!(
            resolve_test_target(&c, "apple intelligence").unwrap(),
            TestTarget::Provider("on-device".into())
        );
        // Anything else that fits the model charset is an Ollama model.
        assert_eq!(
            resolve_test_target(&c, "gemma3:4b").unwrap(),
            TestTarget::OllamaModel("gemma3:4b".into())
        );
        // Shell-shaped targets are refused with the roster named.
        let err = resolve_test_target(&c, "x; rm -rf /").unwrap_err();
        assert!(err.contains("\"ollama\""), "{err}");
        // Key-shaped targets are refused outright — the inherited
        // secret discipline extends to every new verb.
        let err = resolve_test_target(&c, "sk-abcdef1234567890").unwrap_err();
        assert!(err.contains("key"), "{err}");
    }

    #[test]
    fn models_report_lists_and_redacts() {
        let installed = Ok(vec![
            "gpt-oss:20b".to_string(),
            "mxbai-embed-large".to_string(),
        ]);
        let providers = vec![
            ProviderStatus {
                label: "Ollama".into(),
                model: "gpt-oss:20b".into(),
                ready: true,
                detail: "gpt-oss:20b · running".into(),
                is_chat: true,
                is_studio: false,
            },
            ProviderStatus {
                label: "Gateway".into(),
                model: "big-model".into(),
                ready: false,
                // A hostile/leaky readiness detail must not survive the
                // formatting pass.
                detail: "rejected key sk-abcdef1234567890".into(),
                is_chat: false,
                is_studio: true,
            },
        ];
        let out = format_models_report(&installed, &providers);
        assert!(out.contains("Installed in Ollama (2):"), "{out}");
        assert!(out.contains("- gpt-oss:20b"), "{out}");
        assert!(out.contains("Ollama · gpt-oss:20b — ready"), "{out}");
        assert!(out.contains("[chat]"), "{out}");
        assert!(out.contains("Gateway · big-model — not ready"), "{out}");
        assert!(out.contains("[studio]"), "{out}");
        assert!(!out.contains("sk-abcdef"), "{out}");
        // Unreachable Ollama degrades to a sentence, not an error dump.
        let down = format_models_report(&Err("connection refused".into()), &providers);
        assert!(down.contains("Ollama isn't reachable"), "{down}");
    }

    #[test]
    fn test_report_formats_and_redacts() {
        let c = test_config();
        let ok = format_test_report(
            &c,
            "Ollama · gpt-oss:20b",
            &ProbeResult::Ok {
                first_ms: 412,
                total_ms: 1180,
            },
            Some(&(
                "mxbai-embed-large".to_string(),
                ProbeResult::Ok {
                    first_ms: 0,
                    total_ms: 89,
                },
            )),
        );
        assert!(
            ok.contains("alive — first token in 412 ms, total 1180 ms"),
            "{ok}"
        );
        assert!(
            ok.contains("Embedding (mxbai-embed-large): ok in 89 ms"),
            "{ok}"
        );
        // Failures carry the redacted error, never the configured key.
        let failed = format_test_report(
            &c,
            "Gateway · big-model",
            &ProbeResult::Failed("401 unauthorized for key sk-secret1234567890abcdef".to_string()),
            None,
        );
        assert!(
            failed.contains("Tested Gateway · big-model: failed"),
            "{failed}"
        );
        assert!(!failed.contains("sk-secret"), "{failed}");
    }

    // -- Phase 2: profile, style, theme --------------------------------------

    #[test]
    fn profile_fields_set_echo_and_refuse() {
        let mut c = test_config();
        let echo = settings_set(&mut c, "profile.name", "Paul").unwrap();
        assert_eq!(c.profile.name, "Paul");
        assert!(echo.contains("call you Paul"), "{echo}");
        settings_set(&mut c, "profile.profession", "software engineer").unwrap();
        assert_eq!(c.profile.profession, "software engineer");
        let echo = settings_set(&mut c, "profile.instructions", "keep answers short").unwrap();
        assert!(echo.contains("keep answers short"), "{echo}");
        // The inherited secret discipline: key-shaped free text is refused
        // even in profile fields — instructions ride every prompt.
        let err = settings_set(&mut c, "profile.instructions", "sk-abcdef1234567890").unwrap_err();
        assert!(err.contains("API key"), "{err}");
        assert_eq!(c.profile.instructions, "keep answers short"); // untouched
                                                                  // Length caps hold.
        let long = "x".repeat(501);
        assert!(settings_set(&mut c, "profile.instructions", &long).is_err());
        assert!(settings_set(&mut c, "profile.name", &"y".repeat(81)).is_err());
        // And the snapshot shows the profile.
        let out = settings_get(&c);
        assert!(out.contains("Name: Paul"), "{out}");
    }

    #[test]
    fn style_resolves_and_validates() {
        assert_eq!(resolve_style("gdev").unwrap().0, "gdev");
        assert_eq!(resolve_style("Google").unwrap().0, "gdev"); // unique fragment
        assert_eq!(resolve_style("default").unwrap().0, "default");
        assert_eq!(resolve_style("learning").unwrap().0, "learning");
        assert!(resolve_style("shakespearean").is_none());

        let (style, length, echo) = settings_style("govuk", "shorter").unwrap();
        assert_eq!(style.as_deref(), Some("govuk"));
        assert_eq!(length.as_deref(), Some("shorter"));
        assert!(echo.contains("GOV.UK"), "{echo}");
        assert!(echo.contains("shorter answers"), "{echo}");
        // Half-changes leave the other half untouched (None).
        let (style, length, _) = settings_style("", "longer").unwrap();
        assert!(style.is_none());
        assert_eq!(length.as_deref(), Some("longer"));
        // Unknown style names the roster; unknown length names the ladder;
        // an empty ask is a question back, not a change.
        assert!(settings_style("shakespearean", "")
            .unwrap_err()
            .contains("GOV.UK"));
        assert!(settings_style("", "verbose-ish")
            .unwrap_err()
            .contains("shorter"));
        assert!(settings_style("", "").is_err());
    }

    #[test]
    fn theme_fuzzy_resolution() {
        assert_eq!(resolve_theme("gruvbox").unwrap().0, "gruvbox");
        assert_eq!(resolve_theme("Tokyo Night").unwrap().0, "tokyo-night");
        assert_eq!(resolve_theme("tokyo").unwrap().0, "tokyo-night");
        // The RFC's own example: vibe words plus a dark filter.
        assert_eq!(resolve_theme("the dark rust one").unwrap().0, "gruvbox");
        // Exact ids win even when a longer sibling exists.
        assert_eq!(resolve_theme("github").unwrap().0, "github");
        assert_eq!(resolve_theme("system").unwrap().0, "system");
        // Ambiguity lists the candidates; nonsense lists the roster.
        let err = resolve_theme("git").unwrap_err();
        assert!(err.contains("GitHub Light"), "{err}");
        let err = resolve_theme("banana").unwrap_err();
        assert!(err.contains("Gruvbox"), "{err}");
    }

    // -- Phase 3: connect is confirm-gated -----------------------------------

    #[test]
    fn connect_grammar_and_refusal() {
        let text = connect_confirm_text("claude", "Claude Code", "~/.claude.json");
        // The literal grammar the transcript turns into the confirm button.
        assert!(
            text.contains("Confirm: connect agent `claude` (Claude Code)."),
            "{text}"
        );
        assert!(text.contains("~/.claude.json"), "{text}");
        assert!(
            text.contains("Nothing is written until you click"),
            "{text}"
        );
        // The MCP-side no-confirm refusal names the file and the flag.
        let refusal = connect_refusal("Claude Code", "~/.claude.json");
        assert!(refusal.contains("confirm: true"), "{refusal}");
        assert!(refusal.contains("~/.claude.json"), "{refusal}");
    }

    // -- test-verb timeout stories -------------------------------------------

    #[test]
    fn timeout_stories_distinguish_cold_load_from_daemon_down() {
        let loading = timeout_story(
            "answer",
            30,
            "laguna-s-2.1",
            Some(OllamaTimeoutContext::Loading),
        );
        assert!(loading.contains("loading into memory"), "{loading}");
        assert!(loading.contains("try again in a minute"), "{loading}");
        let idle = timeout_story(
            "answer",
            30,
            "laguna-s-2.1",
            Some(OllamaTimeoutContext::AliveIdle),
        );
        assert!(idle.contains("server is up"), "{idle}");
        assert!(idle.contains("Try again in a minute"), "{idle}");
        let down = timeout_story(
            "embedding",
            15,
            "nomic-embed-text",
            Some(OllamaTimeoutContext::DaemonUnreachable),
        );
        // Daemon-down renders the one-click serve affordance.
        assert!(
            down.contains("Fix: open Terminal, run `ollama serve`"),
            "{down}"
        );
        // Non-Ollama engines keep the plain sentence.
        assert_eq!(
            timeout_story("answer", 30, "x", None),
            "no answer within 30 seconds"
        );
    }

    // -- Phase 5: setup picks exactly one next step ---------------------------

    fn setup_state() -> SetupState {
        SetupState {
            chat_label: "Ollama".into(),
            chat_kind: "ollama".into(),
            chat_ready: true,
            chat_detail: "gpt-oss:20b · running".into(),
            chat_model: "gpt-oss:20b".into(),
            chat_model_installed: true,
            fm_ready: true,
            ollama_reachable: true,
            embedder: "ollama".into(),
            embed_model: "mxbai-embed-large".into(),
            embed_model_installed: true,
            profile_named: true,
            connectors: vec![("claude".into(), "Claude Code".into(), true, true)],
        }
    }

    #[test]
    fn setup_walks_one_step_at_a_time() {
        // 1: engine down → serve affordance + the on-device alternative.
        let mut s = setup_state();
        s.chat_ready = false;
        s.ollama_reachable = false;
        let step = setup_next_step(&s);
        assert!(step.contains("Setup 1 of 5"), "{step}");
        assert!(
            step.contains("Fix: open Terminal, run `ollama serve`"),
            "{step}"
        );
        assert!(
            step.contains("switch chat to provider `on-device`"),
            "{step}"
        );
        assert!(!step.contains("Setup 2"), "one step only: {step}");

        // 2: engine up, chat model missing → pull affordance.
        let mut s = setup_state();
        s.chat_model_installed = false;
        let step = setup_next_step(&s);
        assert!(step.contains("Setup 2 of 5"), "{step}");
        assert!(
            step.contains("Fix: open Terminal, run `ollama pull gpt-oss:20b`"),
            "{step}"
        );

        // 3: embed model missing → pull or builtin.
        let mut s = setup_state();
        s.embed_model_installed = false;
        let step = setup_next_step(&s);
        assert!(step.contains("Setup 3 of 5"), "{step}");
        assert!(step.contains("ollama pull mxbai-embed-large"), "{step}");
        assert!(step.contains("builtin"), "{step}");
        // …but the builtin embedder needs nothing, ever.
        s.embedder = "builtin".into();
        assert!(
            !setup_next_step(&s).contains("Setup 3"),
            "builtin skips step 3"
        );

        // 4: no name → the profile prompt-back.
        let mut s = setup_state();
        s.profile_named = false;
        let step = setup_next_step(&s);
        assert!(step.contains("Setup 4 of 5"), "{step}");
        assert!(step.contains("call me"), "{step}");

        // 5: an installed-but-unconnected client → the confirm grammar.
        let mut s = setup_state();
        s.connectors = vec![("claude".into(), "Claude Code".into(), true, false)];
        let step = setup_next_step(&s);
        assert!(step.contains("Setup 5 of 5"), "{step}");
        assert!(
            step.contains("Confirm: connect agent `claude` (Claude Code)."),
            "{step}"
        );
        assert!(step.contains("Or ignore this step"), "{step}");

        // All met → done summary, no steps.
        let done = setup_next_step(&setup_state());
        assert!(done.contains("You're set up"), "{done}");
        assert!(!done.contains("Setup"), "{done}");
    }

    #[test]
    fn needs_diagnosis_defers_to_cheaper_loops() {
        // Phase-1 shapes and pre-translated errors skip the model loop.
        assert!(!needs_diagnosis(
            "ollama: connection refused on localhost:11434"
        ));
        assert!(!needs_diagnosis("Fix: open Terminal, run `claude`."));
        assert!(!needs_diagnosis("check Settings → Models"));
        assert!(!needs_diagnosis("Append with different schema: x"));
        // Novel shapes do get diagnosed.
        assert!(needs_diagnosis("mystery error 0x51 from provider"));
    }
}
