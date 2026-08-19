//! Write-back to Apple Notes / Reminders through cider sources.

use super::*;
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct UpdateMacNoteReq {
    /// Source id of an Apple Notes source (from list_sources).
    source_id: String,
    /// Full replacement note text; first line is the note's title.
    body: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddReminderReq {
    /// Source id of a Reminders-list source (from list_sources).
    source_id: String,
    /// Reminder title.
    title: String,
    /// Optional notes attached to the reminder.
    notes: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CompleteReminderReq {
    /// Source id of a Reminders-list source (from list_sources).
    source_id: String,
    /// The reminder's id, as printed after its title in the source text.
    /// Always use the id: reminder titles are not unique, and there is no
    /// way to name one by title when two of them match.
    reminder_id: String,
}

#[tool_router(router = mac_router, vis = "pub(super)")]
impl AlchemyMcp {
    // -- Mac source write-back (Apple Notes / Reminders via cider) --

    #[tool(
        description = "Replace the body of an Apple Notes source (a source whose url starts with cider://notes/note/). Writes to the actual note in Apple Notes, then re-syncs and re-embeds the source. The first line of the body is the note's title — keep it there. Read the current text with get_source first and preserve the user's content."
    )]
    async fn update_mac_note(
        &self,
        Parameters(UpdateMacNoteReq { source_id, body }): Parameters<UpdateMacNoteReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let source = state
            .db
            .get_source(&source_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| invalid(format!("no source with id {source_id}")))?;
        crate::mac::update_note(&source.url, &body)
            .await
            .map_err(|e| invalid(format!("{e:#}")))?;
        let notebook_id = source.notebook_id.clone();
        let updated = commands::resync_mac_source(&state, source)
            .await
            .map_err(internal)?;
        self.changed("sources", Some(&notebook_id));
        json_result(&updated)
    }

    #[tool(
        description = "Add a reminder to the Apple Reminders list behind a Reminders source (a source whose url starts with cider://reminders/list/). Writes to Apple Reminders, then re-syncs the source. Only lists the user has connected as sources are reachable."
    )]
    async fn add_reminder(
        &self,
        Parameters(AddReminderReq {
            source_id,
            title,
            notes,
        }): Parameters<AddReminderReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let source = state
            .db
            .get_source(&source_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| invalid(format!("no source with id {source_id}")))?;
        crate::mac::add_reminder(&source.url, &title, notes.as_deref())
            .await
            .map_err(|e| invalid(format!("{e:#}")))?;
        let notebook_id = source.notebook_id.clone();
        let updated = commands::resync_mac_source(&state, source)
            .await
            .map_err(internal)?;
        self.changed("sources", Some(&notebook_id));
        json_result(&updated)
    }

    #[tool(
        description = "Check off a reminder in the Apple Reminders list behind a Reminders source (a source whose url starts with cider://reminders/list/). Takes the reminder's id, which the source text prints after each title — titles repeat, so an id is the only way to name one exactly. Writes to Apple Reminders, then re-syncs the source."
    )]
    async fn complete_reminder(
        &self,
        Parameters(CompleteReminderReq {
            source_id,
            reminder_id,
        }): Parameters<CompleteReminderReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let source = state
            .db
            .get_source(&source_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| invalid(format!("no source with id {source_id}")))?;
        crate::mac::complete_reminder(&source.url, &reminder_id)
            .await
            .map_err(|e| invalid(format!("{e:#}")))?;
        let notebook_id = source.notebook_id.clone();
        let updated = commands::resync_mac_source(&state, source)
            .await
            .map_err(internal)?;
        self.changed("sources", Some(&notebook_id));
        json_result(&updated)
    }
}
