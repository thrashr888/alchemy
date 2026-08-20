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
    out.push_str(
        "\nAPI keys are never shown here — manage them in Settings → Models. \
         Say \"switch chat to <provider>\" to change providers.",
    );
    redact_key_shaped(&out)
}

const SETTABLE_FIELDS: &str = "chatProvider, studioProvider, chatModel, effort, baseUrl, \
     smallModel, embedder, provider.<id>.chatModel, provider.<id>.effort, provider.<id>.baseUrl";

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
        for field in ["notionThing", "mcpPort", "provider.ollama", "profile.name"] {
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
