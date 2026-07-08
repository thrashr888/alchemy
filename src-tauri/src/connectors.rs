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

fn server_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

// ---- Target registry ---------------------------------------------------------

enum Strategy {
    /// Merge `{ <pointer...>: { "alchemy": entry(port) } }` into a JSON file,
    /// preserving everything else. Creates the file if missing.
    JsonMerge {
        path: &'static str,
        pointer: &'static [&'static str],
        entry: fn(u16) -> serde_json::Value,
    },
    /// Append a `[section]` block if the file doesn't already have one.
    TomlAppend {
        path: &'static str,
        section: fn(u16) -> String,
    },
    /// Don't write their config — the user pastes the snippet themselves.
    /// `configured` when the file at `path` contains `needle`.
    Manual {
        path: &'static str,
        needle: &'static str,
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
    /// Shown to the user: CLI one-liner or config snippet for manual setup.
    snippet: fn(u16) -> String,
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
            entry: |port| serde_json::json!({ "type": "http", "url": server_url(port) }),
        }],
        skills_dirs: &[".claude/skills"],
        snippet: |port| {
            format!(
                "claude mcp add --transport http --scope user alchemy {}",
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
            section: |port| format!("\n[mcp_servers.alchemy]\nurl = \"{}\"\n", server_url(port)),
        }],
        skills_dirs: &[".codex/skills"],
        snippet: |port| format!("codex mcp add alchemy --url {}", server_url(port)),
    },
    Target {
        id: "opencode",
        name: "OpenCode",
        detect: &[".config/opencode", ".local/share/opencode"],
        strategies: &[Strategy::JsonMerge {
            path: ".config/opencode/opencode.json",
            pointer: &["mcp"],
            entry: |port| serde_json::json!({ "type": "remote", "url": server_url(port), "enabled": true }),
        }],
        skills_dirs: &[".config/opencode/skills"],
        snippet: |port| {
            json_snippet(
                "mcp",
                &serde_json::json!({ "type": "remote", "url": server_url(port), "enabled": true }),
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
            entry: |port| serde_json::json!({ "httpUrl": server_url(port) }),
        }],
        skills_dirs: &[".gemini/skills"],
        snippet: |port| {
            format!(
                "gemini mcp add --transport http alchemy {}",
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
                entry: |port| serde_json::json!({ "serverUrl": server_url(port) }),
            },
            Strategy::JsonMerge {
                path: ".gemini/antigravity/mcp_config.json",
                pointer: &["mcpServers"],
                entry: |port| serde_json::json!({ "serverUrl": server_url(port) }),
            },
        ],
        skills_dirs: &[".gemini/skills"],
        snippet: |port| {
            json_snippet(
                "mcpServers",
                &serde_json::json!({ "serverUrl": server_url(port) }),
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
        snippet: |port| format!("hermes mcp add alchemy --url {}", server_url(port)),
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
            entry: |port| serde_json::json!({ "url": server_url(port), "disabled": false }),
        }],
        skills_dirs: &[".kiro/skills"],
        snippet: |port| {
            json_snippet(
                "mcpServers",
                &serde_json::json!({ "url": server_url(port), "disabled": false }),
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
                entry: |port| serde_json::json!({ "type": "streamable-http", "url": server_url(port) }),
            },
            Strategy::JsonMerge {
                path: ".bob/mcp_settings.json",
                pointer: &["mcpServers"],
                entry: |port| serde_json::json!({ "type": "streamable-http", "url": server_url(port) }),
            },
        ],
        skills_dirs: &[".bob/skills"],
        snippet: |port| {
            json_snippet(
                "mcpServers",
                &serde_json::json!({ "type": "streamable-http", "url": server_url(port) }),
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
            entry: |port| serde_json::json!({ "type": "http", "url": server_url(port), "disabled": false }),
        }],
        skills_dirs: &[".factory/skills"],
        snippet: |port| format!("droid mcp add alchemy {} --type http", server_url(port)),
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
            entry: |port| serde_json::json!({ "type": "http", "url": server_url(port), "tools": ["*"] }),
        }],
        skills_dirs: &[".copilot/skills"],
        snippet: |port| {
            format!(
                "copilot mcp add --transport http alchemy {}",
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
            entry: |port| serde_json::json!({ "type": "http", "url": server_url(port) }),
        }],
        skills_dirs: &[".copilot/skills"],
        snippet: |port| {
            format!(
                "code --add-mcp '{{\"name\":\"alchemy\",\"type\":\"http\",\"url\":\"{}\"}}'",
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

fn strategy_path(s: &Strategy) -> Option<&'static str> {
    match s {
        Strategy::JsonMerge { path, .. } => Some(path),
        Strategy::TomlAppend { path, .. } => Some(path),
        Strategy::Manual { path, .. } => Some(path),
    }
}

fn strategy_configured(home: &std::path::Path, s: &Strategy) -> bool {
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
                match node.get(key) {
                    Some(n) => node = n,
                    None => return false,
                }
            }
            node.get("alchemy").is_some()
        }
        Strategy::TomlAppend { path, .. } => std::fs::read_to_string(resolve(home, path))
            .map(|t| t.contains("[mcp_servers.alchemy]"))
            .unwrap_or(false),
        Strategy::Manual { path, needle } => std::fs::read_to_string(resolve(home, path))
            .map(|t| t.contains(needle))
            .unwrap_or(false),
    }
}

fn strategy_apply(home: &std::path::Path, s: &Strategy, port: u16) -> anyhow::Result<()> {
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
            node["alchemy"] = entry(port);
            std::fs::write(&file, serde_json::to_string_pretty(&root)?)?;
            Ok(())
        }
        Strategy::TomlAppend { path, section } => {
            let file = resolve(home, path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let existing = std::fs::read_to_string(&file).unwrap_or_default();
            if existing.contains("[mcp_servers.alchemy]") {
                return Ok(());
            }
            std::fs::write(&file, format!("{existing}{}", section(port)))?;
            Ok(())
        }
        Strategy::Manual { .. } => Ok(()),
    }
}

fn skill_installed(home: &std::path::Path, target: &Target) -> bool {
    target
        .skills_dirs
        .iter()
        .any(|d| resolve(home, d).join("alchemy/SKILL.md").exists())
}

fn install_skill(home: &std::path::Path, target: &Target) -> anyhow::Result<()> {
    for d in target.skills_dirs {
        let dir = resolve(home, d).join("alchemy");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("SKILL.md"), SKILL_MD)?;
    }
    Ok(())
}

fn status_of(home: &std::path::Path, target: &Target, port: u16) -> ConnectorStatus {
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
            .any(|s| strategy_configured(home, s)),
        can_auto,
        supports_skill: !target.skills_dirs.is_empty(),
        skill_installed: skill_installed(home, target),
        snippet: (target.snippet)(port),
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
    let home = home(&app);
    Ok(TARGETS.iter().map(|t| status_of(&home, t, port)).collect())
}

/// Write the target's MCP config and install the skill where supported.
#[tauri::command]
pub async fn connect_agent(app: AppHandle, id: String) -> Result<ConnectorStatus, String> {
    let target = TARGETS
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("unknown agent target {id}"))?;
    let port = current_port(&app).await;
    let home = home(&app);
    for s in target.strategies {
        strategy_apply(&home, s, port).map_err(|e| format!("{e:#}"))?;
    }
    if !target.skills_dirs.is_empty() {
        install_skill(&home, target).map_err(|e| format!("{e:#}"))?;
    }
    Ok(status_of(&home, target, port))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!status_of(&home, t, 41414).configured);
        for s in t.strategies {
            strategy_apply(&home, s, 41414).unwrap();
        }

        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(root["theme"], "dark", "unrelated keys survive");
        assert_eq!(root["mcpServers"]["other"]["command"], "foo");
        assert_eq!(
            root["mcpServers"]["alchemy"]["httpUrl"],
            "http://127.0.0.1:41414/mcp"
        );
        assert!(status_of(&home, t, 41414).configured);
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
        assert!(strategy_apply(&home, &t.strategies[0], 41414).is_err());
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(home);
    }

    /// TOML append keeps existing content and is idempotent.
    #[test]
    fn toml_append_is_idempotent() {
        let home = tmp_home();
        let cfg = home.join(".codex/config.toml");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, "model = \"o5\"\n").unwrap();

        let t = target("codex");
        for _ in 0..2 {
            strategy_apply(&home, &t.strategies[0], 41414).unwrap();
        }
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.starts_with("model = \"o5\"\n"));
        assert_eq!(text.matches("[mcp_servers.alchemy]").count(), 1);
        assert!(text.contains("url = \"http://127.0.0.1:41414/mcp\""));
        assert!(status_of(&home, t, 41414).configured);
        let _ = std::fs::remove_dir_all(home);
    }

    /// Missing config files are created (with parents) rather than erroring.
    #[test]
    fn json_merge_creates_missing_file() {
        let home = tmp_home();
        let t = target("kiro");
        for s in t.strategies {
            strategy_apply(&home, s, 5150).unwrap();
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
            strategy_apply(&home, s, 41414).unwrap();
        }
        assert!(!home.join(".hermes/config.yaml").exists());
        assert!(!status_of(&home, t, 41414).configured);
        assert!(!status_of(&home, t, 41414).can_auto);

        std::fs::create_dir_all(home.join(".hermes")).unwrap();
        std::fs::write(
            home.join(".hermes/config.yaml"),
            "mcp_servers:\n  alchemy:\n    url: http://127.0.0.1:41414/mcp\n",
        )
        .unwrap();
        assert!(status_of(&home, t, 41414).configured);
        let _ = std::fs::remove_dir_all(home);
    }

    /// VS Code is the odd one out: `servers` top-level key and a config
    /// path containing spaces.
    #[test]
    fn vscode_uses_servers_key() {
        let home = tmp_home();
        let t = target("vscode");
        for s in t.strategies {
            strategy_apply(&home, s, 41414).unwrap();
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
        assert!(status_of(&home, t, 41414).configured);
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
