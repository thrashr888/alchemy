//! "Open in Claude…" (docs/RFC-desktop-apps.md, phase 1): hand a notebook
//! to a desktop AI app the person already uses. Alchemy cannot drive those
//! apps — they have no local API — so the handoff is the honest one: a
//! prompt that carries what the notebook holds goes to the clipboard, the
//! app comes to the front, and the person pastes. If the app is connected
//! to Alchemy's MCP server the prompt tells it so and names the notebook,
//! so it can search instead of working from the list.

use serde::Serialize;
use tauri::State;

use crate::commands::AppState;

struct DesktopAppDef {
    id: &'static str,
    label: &'static str,
    /// The bundle name under /Applications, without ".app".
    bundle: &'static str,
}

/// The desktop apps a handoff can target. Order is the menu's order.
const DESKTOP_APPS: [DesktopAppDef; 3] = [
    DesktopAppDef {
        id: "claude",
        label: "Claude",
        bundle: "Claude",
    },
    DesktopAppDef {
        id: "chatgpt",
        label: "ChatGPT",
        bundle: "ChatGPT",
    },
    DesktopAppDef {
        id: "copilot",
        label: "GitHub Copilot",
        bundle: "GitHub Copilot",
    },
];

/// One desktop AI app, and whether this Mac has it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopApp {
    pub id: String,
    pub label: String,
    pub installed: bool,
}

fn app_installed(bundle: &str) -> bool {
    let name = format!("{bundle}.app");
    let mut roots = vec![std::path::PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(std::path::PathBuf::from(home).join("Applications"));
    }
    roots.iter().any(|r| r.join(&name).exists())
}

fn def(id: &str) -> Result<&'static DesktopAppDef, String> {
    DESKTOP_APPS
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("Unknown desktop app: {id}"))
}

/// Which handoff targets exist on this Mac.
#[tauri::command]
pub fn desktop_apps() -> Vec<DesktopApp> {
    DESKTOP_APPS
        .iter()
        .map(|d| DesktopApp {
            id: d.id.into(),
            label: d.label.into(),
            installed: app_installed(d.bundle),
        })
        .collect()
}

/// Sources a prompt names before it folds the rest into a count.
const HANDOFF_SOURCE_CAP: usize = 25;
/// Characters of a source's gist carried per line.
const HANDOFF_GIST_CHARS: usize = 220;

/// The prompt itself, pure so it can be read in a test. `sources` are
/// (title, origin, gist) with origin a URL or a file name and gist possibly
/// empty.
pub(crate) fn build_handoff_prompt(
    notebook_title: &str,
    notebook_id: &str,
    app_label: &str,
    sources: &[(String, String, String)],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "I'm working in my Alchemy notebook \u{201c}{notebook_title}\u{201d} ({} source{}). \
         Here is what it holds, one line per source:\n\n",
        sources.len(),
        if sources.len() == 1 { "" } else { "s" }
    ));
    for (title, origin, gist) in sources.iter().take(HANDOFF_SOURCE_CAP) {
        out.push_str("- ");
        out.push_str(title.trim());
        if !origin.trim().is_empty() {
            out.push_str(" \u{2014} ");
            out.push_str(origin.trim());
        }
        let gist = gist.split_whitespace().collect::<Vec<_>>().join(" ");
        if !gist.is_empty() {
            let short: String = gist.chars().take(HANDOFF_GIST_CHARS).collect();
            out.push_str(": ");
            out.push_str(&short);
            if gist.chars().count() > HANDOFF_GIST_CHARS {
                out.push('\u{2026}');
            }
        }
        out.push('\n');
    }
    if sources.len() > HANDOFF_SOURCE_CAP {
        out.push_str(&format!(
            "- \u{2026} and {} more\n",
            sources.len() - HANDOFF_SOURCE_CAP
        ));
    }
    out.push_str(&format!(
        "\nIf you have the Alchemy connector, use it rather than this list: the notebook id is \
         `{notebook_id}`; `search` returns cited passages and `get_source` the full text. \
         If you don't, work from the list above and ask me to paste anything you need.\n\n\
         {app_label}, here's my question: "
    ));
    out
}

/// The prompt for one notebook, ready for the clipboard.
#[tauri::command]
pub async fn handoff_prompt(
    state: State<'_, AppState>,
    notebook_id: String,
    app: String,
) -> Result<String, String> {
    let d = def(&app)?;
    let notebooks = state.db.list_notebooks().await.map_err(|e| e.to_string())?;
    let title = notebooks
        .iter()
        .find(|n| n.id == notebook_id)
        .map(|n| n.title.clone())
        .ok_or_else(|| "Notebook not found".to_string())?;
    let sources = state
        .db
        .list_sources(&notebook_id)
        .await
        .map_err(|e| e.to_string())?;
    let gists: std::collections::HashMap<String, String> = state
        .db
        .list_gists()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|g| (g.source_id, g.text))
        .collect();
    let rows: Vec<(String, String, String)> = sources
        .iter()
        .filter(|s| s.source_type != "folder")
        .map(|s| {
            let origin = if s.url.starts_with("http") {
                s.url.clone()
            } else {
                std::path::Path::new(&s.url)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            };
            (
                s.title.clone(),
                origin,
                gists.get(&s.id).cloned().unwrap_or_default(),
            )
        })
        .collect();
    Ok(build_handoff_prompt(&title, &notebook_id, d.label, &rows))
}

/// Bring the app to the front. `open -a` launches it if it isn't running.
#[tauri::command]
pub fn open_desktop_app(app: String) -> Result<(), String> {
    let d = def(&app)?;
    std::process::Command::new("open")
        .arg("-a")
        .arg(d.bundle)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't open {}: {e}", d.label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_names_the_notebook_its_sources_and_the_connector() {
        let sources = vec![
            (
                "Service menu".into(),
                "https://shop.example/services".into(),
                "Brake service, alignment, and lift kits for Subarus.  Prices from $120.".into(),
            ),
            (
                "Build notes.md".into(),
                "Build notes.md".into(),
                String::new(),
            ),
        ];
        let p = build_handoff_prompt("Subaru Battlewagon", "nb-1", "Claude", &sources);
        assert!(p.contains("\u{201c}Subaru Battlewagon\u{201d} (2 sources)"));
        assert!(p.contains(
            "- Service menu \u{2014} https://shop.example/services: Brake service, alignment"
        ));
        assert!(p.contains("- Build notes.md \u{2014} Build notes.md\n"));
        assert!(p.contains("notebook id is `nb-1`"));
        assert!(p.ends_with("Claude, here's my question: "));
    }

    #[test]
    fn a_long_notebook_folds_past_the_cap() {
        let sources: Vec<(String, String, String)> = (0..30)
            .map(|i| (format!("Doc {i}"), String::new(), String::new()))
            .collect();
        let p = build_handoff_prompt("Big", "nb", "ChatGPT", &sources);
        assert!(p.contains("- Doc 24\n"));
        assert!(!p.contains("- Doc 25\n"));
        assert!(p.contains("and 5 more"));
    }
}
