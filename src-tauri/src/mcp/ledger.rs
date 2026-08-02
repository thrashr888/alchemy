//! Ledger tools: typed, anchored memory rows agents can write and revisit —
//! the same rows the user sees in the notebook's Ledger tab. Agent parity is
//! the point: anything captured here appears live in the app, and vice versa.

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

use super::*;
use crate::models::{LedgerAnchor, LedgerEntry};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddLedgerReq {
    /// Notebook the entry belongs to.
    notebook_id: String,
    /// "assertion" (asserted→corroborated|contradicted|stale), "fact"
    /// (current→superseded), "decision" (decided→superseded), "question"
    /// (open→answered), or "log" (a dated line, no lifecycle).
    kind: String,
    /// The entry itself — one claim, fact, decision, question, or log line.
    text: String,
    /// The because: rationale for decisions, context otherwise.
    #[serde(default)]
    why: Option<String>,
    /// Verbatim quotes pinning the entry to sources; each anchor's quote
    /// should be copied exactly from the source so it can be found again.
    #[serde(default)]
    anchors: Option<Vec<AnchorReq>>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AnchorReq {
    /// Source id (from list_sources / search results).
    source_id: String,
    /// Exact text from that source backing the entry.
    #[serde(default)]
    quote: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ListLedgerReq {
    /// Notebook to read.
    notebook_id: String,
    /// Filter to one kind (assertion|fact|decision|question|log).
    #[serde(default)]
    kind: Option<String>,
    /// Filter to one status (e.g. "contradicted", "open").
    #[serde(default)]
    status: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SetLedgerStatusReq {
    /// Entry id (from list_ledger / add_ledger_entry).
    entry_id: String,
    /// New status, from the entry's kind lifecycle.
    status: String,
    /// Optionally update the why while you're here (e.g. what contradicted
    /// an assertion, what answered a question).
    #[serde(default)]
    why: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct LedgerIdReq {
    /// Entry id.
    entry_id: String,
}

#[tool_router(router = ledger_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "Record a typed ledger entry in a notebook: an assertion, fact, decision (with its why), open question, or log line — durable memory with a lifecycle, anchored to sources by verbatim quotes. Prefer this over a note when the thing recorded should be revisited, corroborated, or superseded later."
    )]
    async fn add_ledger_entry(
        &self,
        Parameters(AddLedgerReq {
            notebook_id,
            kind,
            text,
            why,
            anchors,
        }): Parameters<AddLedgerReq>,
    ) -> Result<CallToolResult, McpError> {
        commands::validate_kind(&kind).map_err(invalid)?;
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err(invalid("text is empty — an entry is its text"));
        }
        let ts = commands::now();
        let entry = LedgerEntry {
            id: commands::new_id(),
            notebook_id: notebook_id.clone(),
            status: commands::initial_status(&kind).to_string(),
            kind,
            text,
            why: why.unwrap_or_default().trim().to_string(),
            anchors: anchors
                .unwrap_or_default()
                .into_iter()
                .map(|a| LedgerAnchor {
                    source_id: a.source_id,
                    quote: a.quote.unwrap_or_default(),
                })
                .collect(),
            created_at: ts,
            updated_at: ts,
        };
        self.state()
            .db
            .add_ledger_entry(&entry)
            .await
            .map_err(internal)?;
        self.changed("ledger", Some(&notebook_id));
        json_result(&entry)
    }

    #[tool(
        description = "List a notebook's ledger entries (newest first), optionally filtered by kind or status. Each entry: id, kind, text, why, status, anchors, timestamps. \"status: contradicted\" surfaces what needs re-examination; \"kind: question, status: open\" lists what's unresolved."
    )]
    async fn list_ledger(
        &self,
        Parameters(ListLedgerReq {
            notebook_id,
            kind,
            status,
        }): Parameters<ListLedgerReq>,
    ) -> Result<CallToolResult, McpError> {
        let mut entries = self
            .state()
            .db
            .list_ledger(&notebook_id)
            .await
            .map_err(internal)?;
        if let Some(kind) = kind {
            entries.retain(|e| e.kind == kind);
        }
        if let Some(status) = status {
            entries.retain(|e| e.status == status);
        }
        json_result(&entries)
    }

    #[tool(
        description = "Move a ledger entry through its lifecycle (e.g. an assertion to corroborated/contradicted/stale, a question to answered, a fact or decision to superseded), optionally updating its why with what changed."
    )]
    async fn set_ledger_status(
        &self,
        Parameters(SetLedgerStatusReq {
            entry_id,
            status,
            why,
        }): Parameters<SetLedgerStatusReq>,
    ) -> Result<CallToolResult, McpError> {
        let db = &self.state().db;
        let Some(mut entry) = db.get_ledger_entry(&entry_id).await.map_err(internal)? else {
            return Err(invalid("no ledger entry with that id"));
        };
        let allowed = commands::statuses_for(&entry.kind);
        if !allowed.contains(&status.as_str()) {
            return Err(invalid(format!(
                "a {} can be: {}",
                entry.kind,
                allowed.join(", ")
            )));
        }
        entry.status = status;
        if let Some(why) = why {
            entry.why = why.trim().to_string();
        }
        entry.updated_at = commands::now();
        db.update_ledger_entry(&entry).await.map_err(internal)?;
        self.changed("ledger", Some(&entry.notebook_id));
        json_result(&entry)
    }

    #[tool(description = "Delete a ledger entry permanently.")]
    async fn delete_ledger_entry(
        &self,
        Parameters(LedgerIdReq { entry_id }): Parameters<LedgerIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let entry = self
            .state()
            .db
            .get_ledger_entry(&entry_id)
            .await
            .map_err(internal)?;
        self.state()
            .db
            .delete_ledger_entry(&entry_id)
            .await
            .map_err(internal)?;
        if let Some(entry) = entry {
            self.changed("ledger", Some(&entry.notebook_id));
        }
        json_result(&serde_json::json!({ "deleted": true }))
    }
}
