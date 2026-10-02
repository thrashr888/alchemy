//! Field notes (docs/RFC-unified-chat.md, "Self-improvement: field notes,
//! and nothing else"): when a model's tool call is rejected for a bad
//! argument, one deduplicated line is appended to `<app-data>/field-notes.md`
//! and the newest lines ride every future prompt, so a new session does not
//! repeat the mistake an old one made.
//!
//! Deterministic by design: no model call, no scoring. The file is plain
//! markdown a person can read and edit. It is re-read on every call with no
//! cache, so a line the user deletes stays deleted. Recording is infallible
//! from the caller's side — a field note must never break a tool call.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FILE: &str = "field-notes.md";
const MAX_LINES: usize = 40;
const MAX_ERROR_CHARS: usize = 200;

static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Serializes read-modify-write cycles: the Home loop and the MCP server
/// can reject calls at the same moment, and an unguarded pair would drop one.
static WRITE: Mutex<()> = Mutex::new(());

/// Set the app data directory once at startup, as `trace::set_dir` does.
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

/// Record a rejected tool call. Never panics, never reports failure.
pub fn record(tool: &str, error: &str) {
    if let Some(dir) = DIR.get() {
        record_in(dir, tool, error);
    }
}

/// The `<field-notes>` block for a prompt, or `None` when there is nothing.
pub fn prompt_block() -> Option<String> {
    DIR.get().and_then(|dir| prompt_block_in(dir))
}

/// Collapse an error to one line and cap it, so a stack of newlines or a
/// pasted payload cannot turn one note into a paragraph.
fn normalize(error: &str) -> String {
    let one_line = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > MAX_ERROR_CHARS {
        let cut: String = one_line.chars().take(MAX_ERROR_CHARS).collect();
        format!("{}…", cut.trim_end())
    } else {
        one_line
    }
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .filter(|l| l.starts_with("- "))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn record_in(dir: &Path, tool: &str, error: &str) {
    let line = format!("- {}: {}", tool.trim(), normalize(error));
    // A poisoned lock only means another writer panicked; the file is
    // rewritten whole each time, so carrying on is safe.
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let path = dir.join(FILE);
    let mut lines = read_lines(&path);
    lines.retain(|l| *l != line);
    lines.push(line);
    if lines.len() > MAX_LINES {
        lines.drain(..lines.len() - MAX_LINES);
    }
    if let Err(err) = write_atomic(dir, &path, &lines) {
        crate::note!("field note write failed: {err}");
    }
}

fn write_atomic(dir: &Path, path: &Path, lines: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, format!("{}\n", lines.join("\n")))?;
    std::fs::rename(&tmp, path)
}

fn prompt_block_in(dir: &Path) -> Option<String> {
    let lines = read_lines(&dir.join(FILE));
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "<field-notes>\nMistakes earlier sessions made with these tools — avoid repeating them:\n{}\n</field-notes>",
        lines.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alchemy-fieldnotes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn duplicate_moves_to_newest() {
        let dir = temp();
        record_in(&dir, "a", "one");
        record_in(&dir, "b", "two");
        record_in(&dir, "a", "one");
        assert_eq!(read_lines(&dir.join(FILE)), vec!["- b: two", "- a: one"]);
    }

    #[test]
    fn keeps_newest_forty() {
        let dir = temp();
        for i in 0..50 {
            record_in(&dir, "t", &format!("e{i}"));
        }
        let lines = read_lines(&dir.join(FILE));
        assert_eq!(lines.len(), 40);
        assert_eq!(lines[0], "- t: e10");
        assert_eq!(lines[39], "- t: e49");
    }

    #[test]
    fn long_and_multiline_errors_are_normalized() {
        assert_eq!(normalize("bad\n  arg\tshape"), "bad arg shape");
        let long = normalize(&"x".repeat(500));
        assert_eq!(long.chars().count(), 201);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn empty_has_no_block_and_filled_lists_lines() {
        let dir = temp();
        assert!(prompt_block_in(&dir).is_none());
        record_in(&dir, "search", "unknown notebook id");
        let block = prompt_block_in(&dir).unwrap();
        assert!(block.starts_with("<field-notes>"));
        assert!(block.contains("- search: unknown notebook id"));
        assert!(block.ends_with("</field-notes>"));
    }

    #[test]
    fn deleted_lines_stay_deleted() {
        let dir = temp();
        record_in(&dir, "a", "one");
        std::fs::write(dir.join(FILE), "").unwrap();
        assert!(prompt_block_in(&dir).is_none());
    }

    #[test]
    fn unwritable_dir_does_not_panic() {
        let dir = temp();
        let blocker = dir.join("file");
        std::fs::write(&blocker, "x").unwrap();
        record_in(&blocker.join("sub"), "a", "one");
    }
}
