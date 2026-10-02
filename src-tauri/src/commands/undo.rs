//! Undo for what an agent changed (docs/RFC-unified-chat.md, phase 2).
//!
//! Alchemy decides what an agent may change: every write to a note or source
//! stops at a permission prompt first. That moment is also the one place where
//! the change is known exactly — which session asked, which tool, with which
//! arguments — before it happens. So when the user says Yes to a destructive
//! write, Alchemy keeps the note's or source's current state, then lets the
//! agent go ahead. Undo puts it back.
//!
//! Nothing kept the old content before this: `update_note` replaces a note's
//! whole title and body and drops the old row, and the deletion receipts hold
//! ids only. A "Yes" to a bad rewrite was data the user had lost.
//!
//! Critical scope for the first release: updates and deletes of notes and
//! sources — where data can be lost. What an agent *creates* the user can
//! remove by hand, and the tool loop only ever creates, so neither is
//! journaled yet.
//!
//! Undo never overwrites the user. An update is restored only while the
//! entity still holds exactly what the agent wrote; anything edited since is
//! skipped and named. A deleted note or source comes back as a new item with
//! the old content — the old id is held by the deletion receipts that stop a
//! sync from resurrecting it, so reusing it would fight the sync.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};

use crate::models::{Note, Source};

use super::{
    add_note_indexed, add_url_sources, app_data_dir, index_note, new_id, now, store_extracted,
    AppState,
};

/// Changes kept per session. A session that has run long enough to pass this
/// has left its early turns well behind the thread the user is looking at.
const KEEP: usize = 200;

/// What an entity looked like before the agent touched it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum Before {
    Note { note: Note },
    Source { source: Source },
}

/// One destructive write an agent was allowed to make.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Change {
    pub id: String,
    /// When the user allowed it (ms). Home's Undo finds a turn's changes by
    /// this falling between the question and the answer.
    pub at: i64,
    /// Our tool name: update_note, delete_note, update_source, delete_source.
    pub tool: String,
    /// Who asked ("Claude Code").
    pub agent: String,
    pub before: Before,
    /// For an update, a hash of exactly what the agent will write. Undo
    /// restores only while the entity still matches it. None for a delete.
    pub after_hash: Option<String>,
    #[serde(default)]
    pub undone: bool,
}

fn hash(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    format!("{:x}", h.finalize())
}

fn note_hash(title: &str, content: &str) -> String {
    hash(&[title.trim(), content])
}

fn journal_path(data_dir: &Path, key: &str) -> PathBuf {
    // Session keys are notebook ids or `home-<thread id>`: path-safe already,
    // but nothing from outside should be able to name a path.
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    data_dir.join("agent-undo").join(format!("{safe}.json"))
}

fn load(data_dir: &Path, key: &str) -> Vec<Change> {
    std::fs::read_to_string(journal_path(data_dir, key))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(data_dir: &Path, key: &str, changes: &[Change]) -> anyhow::Result<()> {
    let path = journal_path(data_dir, key);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(changes)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The ids a note or source tool was called with: `note_id` / `note_ids`.
fn ids(args: &Value, one: &str, many: &str) -> Vec<String> {
    let mut out: Vec<String> = args[many]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if let Some(id) = args[one].as_str().filter(|s| !s.is_empty()) {
        out.push(id.to_string());
    }
    out.dedup();
    out
}

/// Will this tool's write be kept for undo? Callers check before reaching
/// the database, so an ordinary permission costs nothing.
pub(crate) fn journals(tool: &str) -> bool {
    matches!(
        tool,
        "update_note" | "delete_note" | "update_source" | "delete_source"
    )
}

/// Keep what `tool` is about to change, before the agent is allowed to change
/// it. Called with the user's Yes in hand and the agent still waiting on it,
/// so the snapshot is never late. Failure to keep a snapshot is logged, not
/// fatal: the user already said Yes, and refusing now would surprise them
/// more than an Undo that has nothing to offer.
pub(crate) async fn capture(state: &AppState, key: &str, agent: &str, tool: &str, args: &Value) {
    if !journals(tool) {
        return;
    }
    let at = now();
    let mut new = Vec::new();
    match tool {
        "update_note" => {
            let id = args["note_id"].as_str().unwrap_or_default();
            if let Ok(Some(note)) = state.db.get_note(id).await {
                let after = note_hash(
                    args["title"].as_str().unwrap_or_default(),
                    args["content"].as_str().unwrap_or_default(),
                );
                new.push((Before::Note { note }, Some(after)));
            }
        }
        "delete_note" => {
            for id in ids(args, "note_id", "note_ids") {
                if let Ok(Some(note)) = state.db.get_note(&id).await {
                    new.push((Before::Note { note }, None));
                }
            }
        }
        "update_source" => {
            let id = args["source_id"].as_str().unwrap_or_default();
            if let Ok(Some(source)) = state.db.get_source(id).await {
                // What the source will hold is the pasted text as ingest
                // normalizes it, not the raw argument.
                let stored = crate::ingest::extract_pasted(
                    args["title"].as_str().unwrap_or_default(),
                    args["text"].as_str().unwrap_or_default(),
                )
                .map(|e| e.text)
                .unwrap_or_default();
                new.push((Before::Source { source }, Some(hash(&[&stored]))));
            }
        }
        "delete_source" => {
            for id in ids(args, "source_id", "source_ids") {
                if let Ok(Some(source)) = state.db.get_source(&id).await {
                    new.push((Before::Source { source }, None));
                }
            }
        }
        _ => {}
    }
    if new.is_empty() {
        return;
    }
    let dir = app_data_dir(state);
    let mut changes = load(&dir, key);
    for (before, after_hash) in new {
        changes.push(Change {
            id: new_id(),
            at,
            tool: tool.to_string(),
            agent: agent.to_string(),
            before,
            after_hash,
            undone: false,
        });
    }
    if changes.len() > KEEP {
        changes.drain(..changes.len() - KEEP);
    }
    if let Err(err) = save(&dir, key, &changes) {
        crate::note!("undo: couldn't keep {tool} for {key}: {err:#}");
    }
}

/// One change as Home lists it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary {
    pub id: String,
    /// When it was allowed (ms) — Home windows a thread's changes into turns.
    pub at: i64,
    pub tool: String,
    pub agent: String,
    pub title: String,
    pub undone: bool,
}

fn summary(c: &Change) -> ChangeSummary {
    let title = match &c.before {
        Before::Note { note } => note.title.clone(),
        Before::Source { source } => source.title.clone(),
    };
    ChangeSummary {
        id: c.id.clone(),
        at: c.at,
        tool: c.tool.clone(),
        agent: c.agent.clone(),
        title,
        undone: c.undone,
    }
}

fn in_window(c: &Change, from_ms: i64, to_ms: i64) -> bool {
    c.at >= from_ms && c.at <= to_ms
}

/// What an agent changed in a session between two moments — a Home turn is
/// the span from its question to its answer.
#[tauri::command]
pub async fn agent_changes(
    state: State<'_, AppState>,
    key: String,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<ChangeSummary>, String> {
    Ok(load(&app_data_dir(&state), &key)
        .iter()
        .filter(|c| in_window(c, from_ms, to_ms))
        .map(summary)
        .collect())
}

/// What an undo did, item by item, so nothing it declined is silent.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UndoReport {
    pub restored: Vec<String>,
    pub skipped: Vec<String>,
}

/// Put back what the agent changed in this window, newest first.
#[tauri::command]
pub async fn undo_agent_changes(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    from_ms: i64,
    to_ms: i64,
) -> Result<UndoReport, String> {
    let dir = app_data_dir(&state);
    let mut changes = load(&dir, &key);
    let mut report = UndoReport::default();
    let mut touched_notebooks: Vec<String> = Vec::new();
    for change in changes.iter_mut().rev() {
        if change.undone || !in_window(change, from_ms, to_ms) {
            continue;
        }
        let outcome = restore(&app, &state, change).await;
        match outcome {
            Ok((line, notebook)) => {
                change.undone = true;
                report.restored.push(line);
                if !touched_notebooks.contains(&notebook) {
                    touched_notebooks.push(notebook);
                }
            }
            Err(why) => report.skipped.push(why),
        }
    }
    save(&dir, &key, &changes).map_err(|e| format!("{e:#}"))?;
    for nb in touched_notebooks {
        let _ = app.emit(
            "mcp://changed",
            serde_json::json!({ "scope": "notes", "notebookId": nb }),
        );
        let _ = app.emit(
            "mcp://changed",
            serde_json::json!({ "scope": "sources", "notebookId": nb }),
        );
    }
    Ok(report)
}

/// Restore one change. Ok carries a line for the report and the notebook it
/// touched; Err says, in the user's words, why it was left alone.
async fn restore(
    app: &AppHandle,
    state: &AppState,
    change: &Change,
) -> Result<(String, String), String> {
    let human = crate::okf::okf_human();
    let dir = app_data_dir(state);
    match (&change.before, change.tool.as_str()) {
        (Before::Note { note }, "update_note") => {
            let current = state
                .db
                .get_note(&note.id)
                .await
                .map_err(|e| format!("“{}”: {e:#}", note.title))?
                .ok_or_else(|| {
                    format!(
                        "“{}” was deleted since, so there's nothing to put back",
                        note.title
                    )
                })?;
            if Some(note_hash(&current.title, &current.content)) != change.after_hash {
                return Err(format!(
                    "“{}” changed after {} edited it, so it was left as it is",
                    note.title, change.agent
                ));
            }
            state
                .db
                .update_note(&note.id, &note.title, &note.content, now())
                .await
                .map_err(|e| format!("“{}”: {e:#}", note.title))?;
            let _ = state.db.set_note_origin(&note.id, &note.origin).await;
            crate::okf::note_okf_edit(&dir, &note.notebook_id, &note.id, &human);
            if let Ok(Some(restored)) = state.db.get_note(&note.id).await {
                index_note(state, &restored).await;
            }
            Ok((
                format!("Restored “{}”", note.title),
                note.notebook_id.clone(),
            ))
        }
        (Before::Note { note }, "delete_note") => {
            let copy = Note {
                id: new_id(),
                updated_at: now(),
                ..note.clone()
            };
            add_note_indexed(state, &copy)
                .await
                .map_err(|e| format!("“{}”: {e:#}", note.title))?;
            crate::okf::note_okf_edit(&dir, &copy.notebook_id, &copy.id, &human);
            Ok((
                format!("Brought back “{}”", note.title),
                note.notebook_id.clone(),
            ))
        }
        (Before::Source { source }, "update_source") => {
            let current = state
                .db
                .get_source(&source.id)
                .await
                .map_err(|e| format!("“{}”: {e:#}", source.title))?
                .ok_or_else(|| {
                    format!(
                        "“{}” was deleted since, so there's nothing to put back",
                        source.title
                    )
                })?;
            if Some(hash(&[&current.content])) != change.after_hash {
                return Err(format!(
                    "“{}” changed after {} edited it, so it was left as it is",
                    source.title, change.agent
                ));
            }
            let extracted = crate::ingest::extract_pasted(&source.title, &source.content)
                .map_err(|e| format!("“{}”: {e:#}", source.title))?;
            crate::okf::note_okf_edit(&dir, &source.notebook_id, &source.id, &human);
            super::reingest(state, &current, extracted, None, true)
                .await
                .map_err(|e| format!("“{}”: {e:#}", source.title))?;
            Ok((
                format!("Restored “{}”", source.title),
                source.notebook_id.clone(),
            ))
        }
        (Before::Source { source }, "delete_source") => {
            // A web page comes back as the page — still a URL source, still
            // refreshable — rather than as a paste of what it said.
            if source.source_type == "url" && !source.url.is_empty() {
                add_url_sources(
                    app,
                    state,
                    &source.notebook_id,
                    std::slice::from_ref(&source.url),
                    "meta://step",
                    "",
                )
                .await;
            } else {
                let extracted = crate::ingest::extract_pasted(&source.title, &source.content)
                    .map_err(|e| format!("“{}”: {e:#}", source.title))?;
                store_extracted(state, &source.notebook_id, extracted)
                    .await
                    .map_err(|e| format!("“{}”: {e:#}", source.title))?;
            }
            Ok((
                format!("Brought back “{}”", source.title),
                source.notebook_id.clone(),
            ))
        }
        _ => Err(format!("{} can't be undone yet", change.tool)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, title: &str, content: &str) -> Note {
        Note {
            id: id.into(),
            notebook_id: "nb".into(),
            title: title.into(),
            content: content.into(),
            kind: "note".into(),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: 1,
            updated_at: 1,
        }
    }

    /// Only the writes that can lose data are kept.
    #[test]
    fn only_destructive_writes_are_journaled() {
        for t in [
            "update_note",
            "delete_note",
            "update_source",
            "delete_source",
        ] {
            assert!(journals(t), "{t}");
        }
        for t in ["create_note", "add_source", "search", "set_source_tags"] {
            assert!(!journals(t), "{t}");
        }
    }

    /// Both spellings of a delete's target are honored, once each.
    #[test]
    fn delete_targets_come_from_either_argument() {
        let a = serde_json::json!({ "note_id": "x" });
        let b = serde_json::json!({ "note_ids": ["x", "y"] });
        let c = serde_json::json!({ "note_ids": ["x"], "note_id": "x" });
        assert_eq!(ids(&a, "note_id", "note_ids"), vec!["x"]);
        assert_eq!(ids(&b, "note_id", "note_ids"), vec!["x", "y"]);
        assert_eq!(ids(&c, "note_id", "note_ids"), vec!["x"]);
    }

    /// The after-hash matches what update_note stores — the trimmed title —
    /// so an untouched rewrite is recognized and an edited one isn't.
    #[test]
    fn after_hash_recognizes_the_agents_write() {
        let written = note_hash("  Closing date ", "October 2025");
        assert_eq!(written, note_hash("Closing date", "October 2025"));
        assert_ne!(
            written,
            note_hash("Closing date", "October 2025, confirmed")
        );
    }

    /// The journal round-trips, keeps the newest entries, and a key from
    /// outside can't name a path.
    #[test]
    fn journal_round_trips_and_stays_inside_its_folder() {
        let dir = std::env::temp_dir().join(format!("alchemy-undo-{}", new_id()));
        let change = |i: i64| Change {
            id: format!("c{i}"),
            at: i,
            tool: "delete_note".into(),
            agent: "Claude Code".into(),
            before: Before::Note {
                note: note("n", "T", "body"),
            },
            after_hash: None,
            undone: false,
        };
        let all: Vec<Change> = (0..3).map(change).collect();
        save(&dir, "home-thread-1", &all).unwrap();
        let back = load(&dir, "home-thread-1");
        assert_eq!(back.len(), 3);
        assert_eq!(back[2].id, "c2");
        let escaped = journal_path(&dir, "../../etc/passwd");
        assert!(escaped.starts_with(dir.join("agent-undo")));
        assert!(load(&dir, "never-written").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A turn's changes are the ones allowed between its question and answer.
    #[test]
    fn a_turn_owns_its_window() {
        let c = Change {
            id: "c".into(),
            at: 150,
            tool: "update_note".into(),
            agent: "Codex".into(),
            before: Before::Note {
                note: note("n", "T", "b"),
            },
            after_hash: None,
            undone: false,
        };
        assert!(in_window(&c, 100, 200));
        assert!(!in_window(&c, 151, 200));
        assert!(!in_window(&c, 0, 149));
    }
}
