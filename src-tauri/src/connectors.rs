//! Agent connectors — one-click registration of Alchemy's MCP server (and
//! skill) with the agent clients installed on this machine.
//!
//! Each target declares how to detect it, how its config is written (careful
//! read-modify-write JSON merge, TOML section append, or manual snippet when
//! we shouldn't touch its config), and where its skills live. The Settings →
//! Agents tab renders one row per target from `list_agent_connectors`.

use serde::Serialize;
use tauri::{AppHandle, Manager};

const SKILL_MD: &str = include_str!("../../skills/alchemy/SKILL.md");

/// The standard skill payload: one SKILL.md, the de-facto cross-client format.
static STD_SKILL: &[(&str, &str)] = &[("SKILL.md", SKILL_MD)];

/// pi's skill: the alchemy_* tools come from the extension (below), so the
/// SKILL.md teaches those names instead of raw MCP tools.
static PI_SKILL: &[(&str, &str)] =
    &[("SKILL.md", include_str!("../../skills/alchemy-pi/SKILL.md"))];

/// pi has no MCP client by design — the bridge is a TypeScript extension
/// speaking streamable HTTP to our server with plain fetch (zero npm deps),
/// registering alchemy_* tools natively (RFC-mcp-server.md).
const PI_EXTENSION_TS: &str = include_str!("../../skills/alchemy-pi/alchemy.ts");

/// Prime Agent's skill is a Python package (its MCP integrations are
/// kernel-imported modules, not client-side tool lists — see prime-agent's
/// docs/mcp-integrations.md). Same SKILL.md idea, plus the two files that
/// make it installable into the kernel venv.
static PRIME_SKILL: &[(&str, &str)] = &[
    (
        "SKILL.md",
        include_str!("../../skills/alchemy-prime/SKILL.md"),
    ),
    (
        "pyproject.toml",
        include_str!("../../skills/alchemy-prime/pyproject.toml"),
    ),
    (
        "src/alchemy/__init__.py",
        include_str!("../../skills/alchemy-prime/__init__.py"),
    ),
];

fn server_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

fn auth_header(token: &str) -> String {
    format!("Bearer {token}")
}

fn http_headers(token: &str) -> serde_json::Value {
    serde_json::json!({ "Authorization": auth_header(token) })
}

// ---- Target registry ---------------------------------------------------------

enum Strategy {
    /// Merge `{ <pointer...>: { "alchemy": entry(port, token) } }` into JSON,
    /// preserving everything else. Creates the file if missing.
    JsonMerge {
        path: &'static str,
        pointer: &'static [&'static str],
        entry: fn(u16, &str) -> serde_json::Value,
    },
    /// Append a `[section]` block if the file doesn't already have one.
    TomlAppend {
        path: &'static str,
        section: fn(u16, &str) -> String,
    },
    /// Don't write their config — the user pastes the snippet themselves.
    /// `configured` when the file at `path` contains `needle`.
    Manual {
        path: &'static str,
        needle: &'static str,
    },
    /// Write a whole file we own (pi's extension bridge — the client has no
    /// MCP config to merge into). Re-connect overwrites it, which is how
    /// port changes propagate. `configured` when the file exists.
    WriteFile {
        path: &'static str,
        content: fn(u16, &str) -> String,
    },
}

struct Target {
    id: &'static str,
    name: &'static str,
    /// Home-relative paths whose existence marks the client installed.
    detect: &'static [&'static str],
    /// Applied in order on connect; `configured` when any matches.
    strategies: &'static [Strategy],
    /// Home-relative skills dirs that load `<dir>/alchemy/SKILL.md`.
    skills_dirs: &'static [&'static str],
    /// Files written under `<skills_dir>/alchemy/` on connect (path relative
    /// to that dir → content). `STD_SKILL` for everyone except Prime Agent,
    /// whose skill is a Python package.
    skill_files: &'static [(&'static str, &'static str)],
    /// Shown to the user: CLI one-liner or config snippet for manual setup.
    snippet: fn(u16, &str) -> String,
}

fn json_snippet(key: &str, entry: &serde_json::Value) -> String {
    serde_json::to_string_pretty(&serde_json::json!({ key: { "alchemy": entry } }))
        .unwrap_or_default()
}

static TARGETS: &[Target] = &[
    Target {
        id: "claude",
        name: "Claude Code",
        detect: &[".claude"],
        strategies: &[Strategy::JsonMerge {
            path: ".claude.json",
            pointer: &["mcpServers"],
            entry: |port, token| {
                serde_json::json!({
                    "type": "http", "url": server_url(port), "headers": http_headers(token)
                })
            },
        }],
        skills_dirs: &[".claude/skills"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            format!(
                "Use Connect, or run claude mcp add with --header Authorization:Bearer… at {}",
                server_url(port)
            )
        },
    },
    Target {
        id: "codex",
        name: "OpenAI Codex",
        detect: &[".codex"],
        strategies: &[Strategy::TomlAppend {
            path: ".codex/config.toml",
            section: |port, token| {
                format!(
                "\n[mcp_servers.alchemy]\nurl = \"{}\"\nhttp_headers = {{ Authorization = \"{}\" }}\n",
                server_url(port),
                auth_header(token)
            )
            },
        }],
        skills_dirs: &[".codex/skills"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            format!("Use Connect to authenticate Codex at {}", server_url(port))
        },
    },
    Target {
        id: "opencode",
        name: "OpenCode",
        detect: &[".config/opencode", ".local/share/opencode"],
        strategies: &[Strategy::JsonMerge {
            path: ".config/opencode/opencode.json",
            pointer: &["mcp"],
            entry: |port, token| {
                serde_json::json!({
                    "type": "remote", "url": server_url(port), "enabled": true,
                    "headers": http_headers(token)
                })
            },
        }],
        skills_dirs: &[".config/opencode/skills"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            json_snippet(
                "mcp",
                &serde_json::json!({
                    "type": "remote", "url": server_url(port), "enabled": true,
                    "headers": { "Authorization": "Bearer <private token from Alchemy>" }
                }),
            )
        },
    },
    Target {
        id: "gemini",
        name: "Gemini CLI",
        detect: &[".gemini/settings.json"],
        strategies: &[Strategy::JsonMerge {
            path: ".gemini/settings.json",
            pointer: &["mcpServers"],
            entry: |port, token| {
                serde_json::json!({
                    "httpUrl": server_url(port), "headers": http_headers(token)
                })
            },
        }],
        skills_dirs: &[".gemini/skills"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            format!(
                "Use Connect, or run gemini mcp add with --header Authorization:Bearer… at {}",
                server_url(port)
            )
        },
    },
    Target {
        id: "antigravity",
        name: "Google Antigravity",
        detect: &[
            ".gemini/antigravity",
            ".gemini/antigravity-cli",
            "/Applications/Antigravity.app",
        ],
        // Antigravity 2.x reads the unified config; the original IDE reads the
        // legacy path. Write both — extra entries in an unused file are inert.
        strategies: &[
            Strategy::JsonMerge {
                path: ".gemini/config/mcp_config.json",
                pointer: &["mcpServers"],
                entry: |port, token| {
                    serde_json::json!({
                        "serverUrl": server_url(port), "headers": http_headers(token)
                    })
                },
            },
            Strategy::JsonMerge {
                path: ".gemini/antigravity/mcp_config.json",
                pointer: &["mcpServers"],
                entry: |port, token| {
                    serde_json::json!({
                        "serverUrl": server_url(port), "headers": http_headers(token)
                    })
                },
            },
        ],
        skills_dirs: &[".gemini/skills"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            json_snippet(
                "mcpServers",
                &serde_json::json!({
                    "serverUrl": server_url(port),
                    "headers": { "Authorization": "Bearer <private token from Alchemy>" }
                }),
            )
        },
    },
    Target {
        id: "hermes",
        name: "Hermes Agent",
        detect: &[".hermes"],
        // ~/.hermes/config.yaml is YAML we won't machine-edit; its CLI does
        // the registration properly (OAuth probe, validation) in one line.
        strategies: &[Strategy::Manual {
            path: ".hermes/config.yaml",
            needle: "alchemy",
        }],
        skills_dirs: &[".hermes/skills/research"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            format!(
                "hermes mcp add alchemy --url {} --auth header",
                server_url(port)
            )
        },
    },
    Target {
        id: "kiro",
        name: "AWS Kiro",
        detect: &[".kiro", "/Applications/Kiro.app"],
        // No `type` field: Kiro auto-negotiates streamable HTTP (SSE fallback)
        // and the bare-url shape is the one both IDE and CLI accept.
        strategies: &[Strategy::JsonMerge {
            path: ".kiro/settings/mcp.json",
            pointer: &["mcpServers"],
            entry: |port, token| {
                serde_json::json!({
                    "url": server_url(port), "disabled": false, "headers": http_headers(token)
                })
            },
        }],
        skills_dirs: &[".kiro/skills"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            json_snippet(
                "mcpServers",
                &serde_json::json!({
                    "url": server_url(port), "disabled": false,
                    "headers": { "Authorization": "Bearer <private token from Alchemy>" }
                }),
            )
        },
    },
    Target {
        id: "bob",
        name: "IBM Bob",
        detect: &[".bob"],
        // The explicit type is load-bearing: a bare `url` makes Bob speak
        // legacy SSE at our streamable-HTTP endpoint. The IDE reads mcp.json,
        // Bob Shell reads mcp_settings.json — write both, same shape.
        strategies: &[
            Strategy::JsonMerge {
                path: ".bob/mcp.json",
                pointer: &["mcpServers"],
                entry: |port, token| {
                    serde_json::json!({
                        "type": "streamable-http", "url": server_url(port),
                        "headers": http_headers(token)
                    })
                },
            },
            Strategy::JsonMerge {
                path: ".bob/mcp_settings.json",
                pointer: &["mcpServers"],
                entry: |port, token| {
                    serde_json::json!({
                        "type": "streamable-http", "url": server_url(port),
                        "headers": http_headers(token)
                    })
                },
            },
        ],
        skills_dirs: &[".bob/skills"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            json_snippet(
                "mcpServers",
                &serde_json::json!({
                    "type": "streamable-http", "url": server_url(port),
                    "headers": { "Authorization": "Bearer <private token from Alchemy>" }
                }),
            )
        },
    },
    Target {
        id: "droid",
        name: "Factory Droid",
        detect: &[".factory"],
        strategies: &[Strategy::JsonMerge {
            path: ".factory/mcp.json",
            pointer: &["mcpServers"],
            entry: |port, token| {
                serde_json::json!({
                    "type": "http", "url": server_url(port), "disabled": false,
                    "headers": http_headers(token)
                })
            },
        }],
        skills_dirs: &[".factory/skills"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            format!("Use Connect to authenticate Droid at {}", server_url(port))
        },
    },
    Target {
        id: "copilot",
        name: "GitHub Copilot CLI",
        // ~/.copilot can exist without the CLI (VS Code shares its skills
        // dir) — a false "detected" only offers a harmless Connect.
        detect: &[".copilot"],
        strategies: &[Strategy::JsonMerge {
            path: ".copilot/mcp-config.json",
            pointer: &["mcpServers"],
            entry: |port, token| {
                serde_json::json!({
                    "type": "http", "url": server_url(port), "tools": ["*"],
                    "headers": http_headers(token)
                })
            },
        }],
        skills_dirs: &[".copilot/skills"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            format!(
                "Use Connect to authenticate Copilot at {}",
                server_url(port)
            )
        },
    },
    Target {
        id: "vscode",
        name: "VS Code",
        detect: &[
            "/Applications/Visual Studio Code.app",
            "Library/Application Support/Code/User",
        ],
        // VS Code's top-level key is `servers`, unlike everyone else's
        // `mcpServers`. Skills: it reads ~/.copilot/skills natively.
        strategies: &[Strategy::JsonMerge {
            path: "Library/Application Support/Code/User/mcp.json",
            pointer: &["servers"],
            entry: |port, token| {
                serde_json::json!({
                    "type": "http", "url": server_url(port), "headers": http_headers(token)
                })
            },
        }],
        skills_dirs: &[".copilot/skills"],
        skill_files: STD_SKILL,
        snippet: |port, _token| {
            format!(
                "Use Connect to authenticate VS Code at {}",
                server_url(port)
            )
        },
    },
    Target {
        id: "prime",
        name: "Prime Agent",
        detect: &[".prime/agent"],
        // Prime's integration reads the private discovery token at runtime;
        // headers are also present for clients that honor the settings entry.
        strategies: &[Strategy::JsonMerge {
            path: ".prime/agent/settings.json",
            pointer: &["mcpServers"],
            entry: |port, token| {
                serde_json::json!({
                    "type": "http", "url": server_url(port), "headers": http_headers(token)
                })
            },
        }],
        skills_dirs: &[".prime/agent/skills"],
        skill_files: PRIME_SKILL,
        snippet: |port, _token| {
            json_snippet(
                "mcpServers",
                &serde_json::json!({
                    "type": "http", "url": server_url(port),
                    "headers": { "Authorization": "Bearer <private token from Alchemy>" }
                }),
            )
        },
    },
    Target {
        id: "pi",
        name: "Pi",
        detect: &[".pi"],
        strategies: &[Strategy::WriteFile {
            path: ".pi/agent/extensions/alchemy.ts",
            content: |port, token| {
                PI_EXTENSION_TS
                    .replace("__ALCHEMY_MCP_URL__", &server_url(port))
                    .replace("__ALCHEMY_MCP_TOKEN__", token)
            },
        }],
        skills_dirs: &[".pi/agent/skills"],
        skill_files: PI_SKILL,
        snippet: |port, _token| {
            format!(
                "Connect writes ~/.pi/agent/extensions/alchemy.ts \
                 (alchemy_* tools → {}); run /reload in pi to pick it up",
                server_url(port)
            )
        },
    },
];

// ---- Status + operations -----------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorStatus {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub configured: bool,
    /// False = manual-only: show the snippet, no Connect button.
    pub can_auto: bool,
    pub supports_skill: bool,
    pub skill_installed: bool,
    /// CLI one-liner or JSON snippet for manual setup / verification.
    pub snippet: String,
    /// Human-readable config location ("~/.codex/config.toml").
    pub config_path: String,
}

fn home(app: &AppHandle) -> std::path::PathBuf {
    app.path().home_dir().unwrap_or_default()
}

/// Resolve a registry path: absolute stays as-is, otherwise home-relative.
fn resolve(home: &std::path::Path, p: &str) -> std::path::PathBuf {
    if p.starts_with('/') {
        std::path::PathBuf::from(p)
    } else {
        home.join(p)
    }
}

fn display_path(p: &str) -> String {
    if p.starts_with('/') {
        p.to_string()
    } else {
        format!("~/{p}")
    }
}

fn json_contains(actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
    match expected {
        serde_json::Value::Object(expected_fields) => expected_fields.iter().all(|(key, value)| {
            actual
                .get(key)
                .is_some_and(|candidate| json_contains(candidate, value))
        }),
        _ => actual == expected,
    }
}

/// Connector config now carries the local bearer token. Keep the whole client
/// config owner-only after updating it; otherwise adding authentication would
/// merely move the cross-account disclosure into a different file.
fn write_connector_config(path: &std::path::Path, contents: &str) -> anyhow::Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn replace_toml_section(existing: &str, header: &str, replacement: &str) -> String {
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let Some(start) = lines.iter().position(|line| line.trim() == header) else {
        return format!("{existing}{replacement}");
    };
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.trim_start().starts_with('['))
        .map(|offset| start + 1 + offset)
        .unwrap_or(lines.len());
    let mut updated = lines[..start].concat();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(replacement.trim_start_matches('\n'));
    if !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&lines[end..].concat());
    updated
}

fn strategy_path(s: &Strategy) -> Option<&'static str> {
    match s {
        Strategy::JsonMerge { path, .. } => Some(path),
        Strategy::TomlAppend { path, .. } => Some(path),
        Strategy::Manual { path, .. } => Some(path),
        Strategy::WriteFile { path, .. } => Some(path),
    }
}

/// Does this config already contain an Alchemy entry from an earlier release?
/// This intentionally ignores URL/token freshness and is used only to migrate
/// connectors Alchemy previously installed, never to opt a new client in.
fn strategy_present(home: &std::path::Path, s: &Strategy) -> bool {
    match s {
        Strategy::JsonMerge { path, pointer, .. } => {
            let Ok(text) = std::fs::read_to_string(resolve(home, path)) else {
                return false;
            };
            let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) else {
                return false;
            };
            let mut node = &root;
            for key in *pointer {
                let Some(next) = node.get(key) else {
                    return false;
                };
                node = next;
            }
            node.get("alchemy").is_some()
        }
        Strategy::TomlAppend { path, .. } => std::fs::read_to_string(resolve(home, path))
            .is_ok_and(|text| {
                text.lines()
                    .any(|line| line.trim() == "[mcp_servers.alchemy]")
            }),
        Strategy::Manual { .. } => false,
        Strategy::WriteFile { path, .. } => resolve(home, path).exists(),
    }
}

fn strategy_configured(home: &std::path::Path, s: &Strategy, port: u16, token: &str) -> bool {
    match s {
        Strategy::JsonMerge {
            path,
            pointer,
            entry,
        } => {
            let Ok(text) = std::fs::read_to_string(resolve(home, path)) else {
                return false;
            };
            let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) else {
                return false;
            };
            let mut node = &root;
            for key in *pointer {
                match node.get(key) {
                    Some(n) => node = n,
                    None => return false,
                }
            }
            node.get("alchemy")
                .is_some_and(|actual| json_contains(actual, &entry(port, token)))
        }
        Strategy::TomlAppend { path, section } => std::fs::read_to_string(resolve(home, path))
            .is_ok_and(|text| {
                section(port, token)
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .all(|line| {
                        text.lines()
                            .any(|candidate| candidate.trim() == line.trim())
                    })
            }),
        Strategy::Manual { path, needle } => std::fs::read_to_string(resolve(home, path))
            .map(|t| t.contains(needle))
            .unwrap_or(false),
        Strategy::WriteFile { path, content } => std::fs::read_to_string(resolve(home, path))
            .is_ok_and(|existing| existing == content(port, token)),
    }
}

fn strategy_apply(
    home: &std::path::Path,
    s: &Strategy,
    port: u16,
    token: &str,
) -> anyhow::Result<()> {
    match s {
        Strategy::JsonMerge {
            path,
            pointer,
            entry,
        } => {
            let file = resolve(home, path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut root: serde_json::Value = match std::fs::read_to_string(&file) {
                Ok(text) if !text.trim().is_empty() => {
                    serde_json::from_str(&text).map_err(|e| {
                        anyhow::anyhow!(
                            "{} is not valid JSON ({e}); not touching it",
                            display_path(path)
                        )
                    })?
                }
                _ => serde_json::json!({}),
            };
            let mut node = &mut root;
            for key in *pointer {
                if !node.get(*key).map(|v| v.is_object()).unwrap_or(false) {
                    node[*key] = serde_json::json!({});
                }
                node = node.get_mut(*key).unwrap();
            }
            node["alchemy"] = entry(port, token);
            write_connector_config(&file, &serde_json::to_string_pretty(&root)?)?;
            Ok(())
        }
        Strategy::TomlAppend { path, section } => {
            let file = resolve(home, path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let existing = std::fs::read_to_string(&file).unwrap_or_default();
            let replacement = section(port, token);
            let updated = replace_toml_section(&existing, "[mcp_servers.alchemy]", &replacement);
            write_connector_config(&file, &updated)?;
            Ok(())
        }
        Strategy::Manual { .. } => Ok(()),
        Strategy::WriteFile { path, content } => {
            let file = resolve(home, path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_connector_config(&file, &content(port, token))?;
            Ok(())
        }
    }
}

fn skill_installed(home: &std::path::Path, target: &Target) -> bool {
    target
        .skills_dirs
        .iter()
        .any(|d| resolve(home, d).join("alchemy/SKILL.md").exists())
}

/// Whether every installed skill file matches what this build ships.
/// Compares content rather than a version stamp: the file is small, and a
/// byte comparison also catches a downgrade, a half-written file, and the
/// skills we shipped before any stamp existed.
fn skill_current(home: &std::path::Path, target: &Target) -> bool {
    target.skills_dirs.iter().all(|d| {
        let dir = resolve(home, d).join("alchemy");
        target.skill_files.iter().all(|(rel, content)| {
            std::fs::read_to_string(dir.join(rel)).is_ok_and(|on_disk| on_disk == *content)
        })
    })
}

/// Re-write already-installed skills that have drifted from this build's.
///
/// Connect used to be the only writer, so a skill froze at whichever version
/// was running the day the user clicked it — the Settings row kept reporting
/// "Connected + skill" while the file taught agents tool names and flows that
/// had since changed. Refreshing at launch keeps that green check honest
/// without asking the user to re-click anything.
///
/// Only touches directories that already hold an alchemy skill, so this never
/// installs into a client the user didn't connect, and never resurrects one
/// they deleted. Silent by design: nothing here is worth a toast, and a
/// read-only skills dir is the client's business, not an error we can fix.
pub fn refresh_installed_skills(app: &AppHandle) {
    let home = home(app);
    for target in TARGETS {
        if target.skills_dirs.is_empty() || !skill_installed(&home, target) {
            continue;
        }
        if skill_current(&home, target) {
            continue;
        }
        match install_skill(&home, target) {
            Ok(()) => crate::note!("connectors: refreshed the {} skill", target.name),
            // A stale skill leaves the agent working from last release's
            // instructions, and nobody is watching this sweep run.
            Err(e) => crate::diagnostics::error(
                "connectors",
                format!("could not refresh the {} skill: {e:#}", target.name),
            ),
        }
    }
}

/// Upgrade only connector entries Alchemy already owns to the current URL and
/// bearer token. Authentication would otherwise make pre-upgrade connections
/// fail closed until every user manually clicked Connect again.
pub fn refresh_installed_connectors(app: &AppHandle, port: u16) {
    let home = home(app);
    let token = match crate::mcp::auth_token(app) {
        Ok(token) => token,
        Err(err) => {
            crate::diagnostics::error(
                "connectors",
                format!("could not initialize connector authentication: {err:#}"),
            );
            return;
        }
    };
    for target in TARGETS {
        if !target
            .strategies
            .iter()
            .any(|strategy| strategy_present(&home, strategy))
        {
            continue;
        }
        for strategy in target.strategies {
            if matches!(strategy, Strategy::Manual { .. }) {
                continue;
            }
            // Rewrite only when stale: these are live client configs
            // (Claude Code rewrites ~/.claude.json constantly), and an
            // unconditional read-modify-write on every boot would race
            // their own writes for no gain.
            if strategy_configured(&home, strategy, port, &token) {
                continue;
            }
            if let Err(err) = strategy_apply(&home, strategy, port, &token) {
                crate::diagnostics::error(
                    "connectors",
                    format!("could not refresh the {} connection: {err:#}", target.name),
                );
                break;
            }
        }
    }
}

fn install_skill(home: &std::path::Path, target: &Target) -> anyhow::Result<()> {
    for d in target.skills_dirs {
        let dir = resolve(home, d).join("alchemy");
        for (rel, content) in target.skill_files {
            let file = dir.join(rel);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(file, content)?;
        }
    }
    Ok(())
}

fn status_of(home: &std::path::Path, target: &Target, port: u16, token: &str) -> ConnectorStatus {
    let can_auto = target
        .strategies
        .iter()
        .any(|s| !matches!(s, Strategy::Manual { .. }));
    ConnectorStatus {
        id: target.id.into(),
        name: target.name.into(),
        installed: target.detect.iter().any(|p| resolve(home, p).exists()),
        configured: target
            .strategies
            .iter()
            .any(|s| strategy_configured(home, s, port, token)),
        can_auto,
        supports_skill: !target.skills_dirs.is_empty(),
        skill_installed: skill_installed(home, target),
        snippet: (target.snippet)(port, token),
        config_path: target
            .strategies
            .iter()
            .find_map(strategy_path)
            .map(display_path)
            .unwrap_or_default(),
    }
}

async fn current_port(app: &AppHandle) -> u16 {
    let state = app.state::<crate::commands::AppState>();
    let ai = state.ai.read().await;
    ai.config().mcp_port
}

// ---- Commands ------------------------------------------------------------------

#[tauri::command]
pub async fn list_agent_connectors(app: AppHandle) -> Result<Vec<ConnectorStatus>, String> {
    let port = current_port(&app).await;
    let token = crate::mcp::auth_token(&app).map_err(|err| format!("{err:#}"))?;
    let home = home(&app);
    Ok(TARGETS
        .iter()
        .map(|t| status_of(&home, t, port, &token))
        .collect())
}

/// Write the target's MCP config and install the skill where supported.
#[tauri::command]
pub async fn connect_agent(app: AppHandle, id: String) -> Result<ConnectorStatus, String> {
    let target = TARGETS
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("unknown agent target {id}"))?;
    let port = current_port(&app).await;
    let token = crate::mcp::auth_token(&app).map_err(|err| format!("{err:#}"))?;
    let home = home(&app);
    for s in target.strategies {
        strategy_apply(&home, s, port, &token).map_err(|e| format!("{e:#}"))?;
    }
    if !target.skills_dirs.is_empty() {
        install_skill(&home, target).map_err(|e| format!("{e:#}"))?;
    }
    Ok(status_of(&home, target, port, &token))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TOKEN: &str = "test-token";

    fn tmp_home() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("alchemy-conn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn target(id: &str) -> &'static Target {
        TARGETS.iter().find(|t| t.id == id).unwrap()
    }

    /// JSON merge must add our entry without disturbing existing config —
    /// this is another tool's file; corrupting it is the worst failure mode.
    #[test]
    fn json_merge_preserves_existing_config() {
        let home = tmp_home();
        let cfg = home.join(".gemini/settings.json");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(
            &cfg,
            r#"{"theme":"dark","mcpServers":{"other":{"command":"foo"}}}"#,
        )
        .unwrap();

        let t = target("gemini");
        assert!(!status_of(&home, t, 41414, TEST_TOKEN).configured);
        for s in t.strategies {
            strategy_apply(&home, s, 41414, TEST_TOKEN).unwrap();
        }

        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(root["theme"], "dark", "unrelated keys survive");
        assert_eq!(root["mcpServers"]["other"]["command"], "foo");
        assert_eq!(
            root["mcpServers"]["alchemy"]["httpUrl"],
            "http://127.0.0.1:41414/mcp"
        );
        assert_eq!(
            root["mcpServers"]["alchemy"]["headers"]["Authorization"],
            "Bearer test-token"
        );
        assert!(status_of(&home, t, 41414, TEST_TOKEN).configured);
        let _ = std::fs::remove_dir_all(home);
    }

    /// A malformed config must be left alone, not clobbered.
    #[test]
    fn json_merge_refuses_invalid_json() {
        let home = tmp_home();
        let cfg = home.join(".gemini/settings.json");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, "{ not json").unwrap();

        let t = target("gemini");
        assert!(strategy_apply(&home, &t.strategies[0], 41414, TEST_TOKEN).is_err());
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(home);
    }

    /// TOML append keeps existing content and is idempotent.
    #[test]
    fn toml_append_is_idempotent() {
        let home = tmp_home();
        let cfg = home.join(".codex/config.toml");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(
            &cfg,
            "model = \"o5\"\n\n[mcp_servers.alchemy]\nurl = \"http://127.0.0.1:1/mcp\"\nlegacy = true\n\n[other]\nenabled = true\n",
        )
        .unwrap();

        let t = target("codex");
        assert!(strategy_present(&home, &t.strategies[0]));
        assert!(!status_of(&home, t, 41414, TEST_TOKEN).configured);
        for _ in 0..2 {
            strategy_apply(&home, &t.strategies[0], 41414, TEST_TOKEN).unwrap();
        }
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.starts_with("model = \"o5\"\n"));
        assert_eq!(text.matches("[mcp_servers.alchemy]").count(), 1);
        assert!(text.contains("url = \"http://127.0.0.1:41414/mcp\""));
        assert!(text.contains("http_headers = { Authorization = \"Bearer test-token\" }"));
        assert!(!text.contains("legacy = true"));
        assert!(text.contains("[other]\nenabled = true"));
        assert!(status_of(&home, t, 41414, TEST_TOKEN).configured);
        let _ = std::fs::remove_dir_all(home);
    }

    /// Missing config files are created (with parents) rather than erroring.
    #[test]
    fn json_merge_creates_missing_file() {
        let home = tmp_home();
        let t = target("kiro");
        for s in t.strategies {
            strategy_apply(&home, s, 5150, TEST_TOKEN).unwrap();
        }
        let root: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join(".kiro/settings/mcp.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            root["mcpServers"]["alchemy"]["url"],
            "http://127.0.0.1:5150/mcp"
        );
        let _ = std::fs::remove_dir_all(home);
    }

    /// Manual targets never write config; configured keys off their file.
    #[test]
    fn manual_target_detects_but_never_writes() {
        let home = tmp_home();
        let t = target("hermes");
        for s in t.strategies {
            strategy_apply(&home, s, 41414, TEST_TOKEN).unwrap();
        }
        assert!(!home.join(".hermes/config.yaml").exists());
        assert!(!status_of(&home, t, 41414, TEST_TOKEN).configured);
        assert!(!status_of(&home, t, 41414, TEST_TOKEN).can_auto);

        std::fs::create_dir_all(home.join(".hermes")).unwrap();
        std::fs::write(
            home.join(".hermes/config.yaml"),
            "mcp_servers:\n  alchemy:\n    url: http://127.0.0.1:41414/mcp\n",
        )
        .unwrap();
        assert!(status_of(&home, t, 41414, TEST_TOKEN).configured);
        let _ = std::fs::remove_dir_all(home);
    }

    /// VS Code is the odd one out: `servers` top-level key and a config
    /// path containing spaces.
    #[test]
    fn vscode_uses_servers_key() {
        let home = tmp_home();
        let t = target("vscode");
        for s in t.strategies {
            strategy_apply(&home, s, 41414, TEST_TOKEN).unwrap();
        }
        let root: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("Library/Application Support/Code/User/mcp.json"))
                .unwrap(),
        )
        .unwrap();
        assert!(root.get("mcpServers").is_none());
        assert_eq!(
            root["servers"]["alchemy"]["url"],
            "http://127.0.0.1:41414/mcp"
        );
        assert!(status_of(&home, t, 41414, TEST_TOKEN).configured);
        let _ = std::fs::remove_dir_all(home);
    }

    /// Prime Agent gets the full Python skill package (SKILL.md +
    /// pyproject.toml + module), and its settings.json merge preserves
    /// existing keys like defaultModel.
    #[test]
    fn prime_installs_python_package_and_merges_settings() {
        let home = tmp_home();
        let cfg = home.join(".prime/agent/settings.json");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, r#"{"defaultModel":"qwen3.6:35b-mlx"}"#).unwrap();

        let t = target("prime");
        for s in t.strategies {
            strategy_apply(&home, s, 41414, TEST_TOKEN).unwrap();
        }
        install_skill(&home, t).unwrap();

        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(root["defaultModel"], "qwen3.6:35b-mlx");
        assert_eq!(
            root["mcpServers"]["alchemy"]["url"],
            "http://127.0.0.1:41414/mcp"
        );
        let skill = home.join(".prime/agent/skills/alchemy");
        assert!(skill.join("SKILL.md").exists());
        assert!(skill.join("pyproject.toml").exists());
        assert!(skill.join("src/alchemy/__init__.py").exists());
        assert!(skill_installed(&home, t));
        assert!(status_of(&home, t, 41414, TEST_TOKEN).configured);
        let _ = std::fs::remove_dir_all(home);
    }

    /// The drift this exists to fix: Connect wrote the skill once, a later
    /// release changed the text, and the stale copy kept reporting itself as
    /// installed. `skill_current` is what separates the two.
    #[test]
    fn a_stale_skill_reads_as_installed_but_not_current() {
        let home = tmp_home();
        let t = target("claude");
        install_skill(&home, t).unwrap();
        assert!(skill_installed(&home, t));
        assert!(skill_current(&home, t));

        let file = home.join(".claude/skills/alchemy/SKILL.md");
        std::fs::write(&file, "# an older release's skill\n").unwrap();
        assert!(skill_installed(&home, t), "still present, just wrong");
        assert!(!skill_current(&home, t));

        install_skill(&home, t).unwrap();
        assert!(skill_current(&home, t));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), SKILL_MD);
        let _ = std::fs::remove_dir_all(home);
    }

    /// Refresh must never install into a client the user didn't connect —
    /// an absent skill stays absent.
    #[test]
    fn refresh_skips_targets_with_no_skill_installed() {
        let home = tmp_home();
        let t = target("claude");
        assert!(!skill_installed(&home, t));
        // `skill_current` is vacuously false for a missing file, so the
        // installed check is what has to hold the line.
        assert!(!skill_current(&home, t));
        let _ = std::fs::remove_dir_all(home);
    }

    /// pi's connect writes the extension bridge with the live port
    /// substituted, plus the skill; configured keys off the file existing.
    #[test]
    fn pi_writes_extension_bridge_with_port() {
        let home = tmp_home();
        let t = target("pi");
        assert!(!status_of(&home, t, 5150, TEST_TOKEN).configured);
        for s in t.strategies {
            strategy_apply(&home, s, 5150, TEST_TOKEN).unwrap();
        }
        install_skill(&home, t).unwrap();

        let ext = std::fs::read_to_string(home.join(".pi/agent/extensions/alchemy.ts")).unwrap();
        assert!(ext.contains("http://127.0.0.1:5150/mcp"));
        assert!(!ext.contains("__ALCHEMY_MCP_URL__"));
        assert!(ext.contains("const MCP_TOKEN = \"test-token\""));
        assert!(!ext.contains("__ALCHEMY_MCP_TOKEN__"));
        assert!(ext.contains("registerTool"));
        assert!(home.join(".pi/agent/skills/alchemy/SKILL.md").exists());
        assert!(status_of(&home, t, 5150, TEST_TOKEN).configured);
        assert!(status_of(&home, t, 5150, TEST_TOKEN).can_auto);
        let _ = std::fs::remove_dir_all(home);
    }

    /// Skill install lands SKILL.md in every declared dir.
    #[test]
    fn skill_installs_to_target_dirs() {
        let home = tmp_home();
        let t = target("bob");
        install_skill(&home, t).unwrap();
        assert!(home.join(".bob/skills/alchemy/SKILL.md").exists());
        assert!(skill_installed(&home, t));
        let _ = std::fs::remove_dir_all(home);
    }
}
