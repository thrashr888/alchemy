//! The Ledger (RFC-v12-steward pillar 2): typed, dated, anchored rows —
//! memory the machine can act on. Kinds carry their own lifecycles; anchors
//! pin entries to verbatim source text (the citation contract). Retrieval
//! indexing of entries is deliberately deferred to the Weave stage: a wrong
//! merge poisons the ledger worse than no row.

use super::*;
use crate::models::{LedgerAnchor, LedgerEntry};

pub(crate) const LEDGER_KINDS: &[&str] = &["assertion", "fact", "decision", "question", "log"];

/// Every kind's starting status.
pub(crate) fn initial_status(kind: &str) -> &'static str {
    match kind {
        "assertion" => "asserted",
        "fact" => "current",
        "decision" => "decided",
        "question" => "open",
        _ => "logged",
    }
}

/// The statuses a kind may hold (first = initial). Transitions are free
/// within a kind — the lifecycle is a vocabulary, not a state machine; the
/// user (or an agent) is the authority.
pub(crate) fn statuses_for(kind: &str) -> &'static [&'static str] {
    match kind {
        "assertion" => &["asserted", "corroborated", "contradicted", "stale"],
        "fact" => &["current", "superseded"],
        "decision" => &["decided", "superseded"],
        "question" => &["open", "answered"],
        _ => &["logged"],
    }
}

pub(crate) fn validate_kind(kind: &str) -> Result<(), String> {
    if LEDGER_KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(format!(
            "Unknown ledger kind \u{201c}{kind}\u{201d} — one of: {}",
            LEDGER_KINDS.join(", ")
        ))
    }
}

#[tauri::command]
pub async fn list_ledger(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Vec<LedgerEntry>, String> {
    e(state.db.list_ledger(&notebook_id).await)
}

#[tauri::command]
pub async fn add_ledger_entry(
    state: State<'_, AppState>,
    notebook_id: String,
    kind: String,
    text: String,
    why: Option<String>,
    anchors: Option<Vec<LedgerAnchor>>,
) -> Result<LedgerEntry, String> {
    validate_kind(&kind)?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("An entry is its text — write what you're recording.".into());
    }
    let ts = now();
    let entry = LedgerEntry {
        id: new_id(),
        notebook_id,
        status: initial_status(&kind).to_string(),
        kind,
        text,
        why: why.unwrap_or_default().trim().to_string(),
        anchors: anchors.unwrap_or_default(),
        created_at: ts,
        updated_at: ts,
    };
    e(state.db.add_ledger_entry(&entry).await)?;
    Ok(entry)
}

#[tauri::command]
pub async fn update_ledger_entry(
    state: State<'_, AppState>,
    id: String,
    text: Option<String>,
    why: Option<String>,
    status: Option<String>,
) -> Result<LedgerEntry, String> {
    let Some(mut entry) = e(state.db.get_ledger_entry(&id).await)? else {
        return Err("Ledger entry not found".into());
    };
    if let Some(text) = text {
        let text = text.trim().to_string();
        if !text.is_empty() {
            entry.text = text;
        }
    }
    if let Some(why) = why {
        entry.why = why.trim().to_string();
    }
    if let Some(status) = status {
        let allowed = statuses_for(&entry.kind);
        if !allowed.contains(&status.as_str()) {
            return Err(format!(
                "A {} can be: {}",
                entry.kind,
                allowed.join(" \u{2192} ")
            ));
        }
        entry.status = status;
    }
    entry.updated_at = now();
    e(state.db.update_ledger_entry(&entry).await)?;
    Ok(entry)
}

#[tauri::command]
pub async fn delete_ledger_entry(state: State<'_, AppState>, id: String) -> Result<(), String> {
    e(state.db.delete_ledger_entry(&id).await)
}
