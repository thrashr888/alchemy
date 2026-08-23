//! Note CRUD tools.

use super::*;
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct NoteIdReq {
    /// Note id (from list_notes).
    note_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CreateNoteReq {
    /// Notebook to create the note in.
    notebook_id: String,
    /// Note title.
    title: String,
    /// Markdown body.
    content: String,
    /// "note" (default) or "evidence" — an evidence record documenting a
    /// claim or decision with its supporting passages.
    kind: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct UpdateNoteReq {
    /// Note id.
    note_id: String,
    /// New title.
    title: String,
    /// New markdown body (full replacement).
    content: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct DeleteNoteReq {
    /// Note id (provide this or note_ids).
    #[serde(default)]
    note_id: Option<String>,
    /// Several note ids to delete as one bulk operation — the multi-select
    /// batch shape (docs/RFC-multi-select.md).
    #[serde(default)]
    note_ids: Option<Vec<String>>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ExportNoteReq {
    /// Note id.
    note_id: String,
    /// "png" | "pdf" | "pptx" | "docx" | "xlsx" | "m4a".
    format: String,
    /// Absolute destination path (defaults to ~/Downloads/<title>.<ext>).
    path: Option<String>,
}

#[tool_router(router = notes_router, vis = "pub(super)")]
impl AlchemyMcp {
    // -- Notes --

    #[tool(
        description = "List a notebook's notes (id, title, kind, content, timestamps). Notes are the user's own writing plus generated artifacts."
    )]
    async fn list_notes(
        &self,
        Parameters(NotebookIdReq { notebook_id }): Parameters<NotebookIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let notes: Vec<Note> = self
            .state()
            .db
            .list_notes(&notebook_id)
            .await
            .map_err(internal)?;
        json_result(&notes)
    }

    #[tool(description = "Read a single note by id.")]
    async fn get_note(
        &self,
        Parameters(NoteIdReq { note_id }): Parameters<NoteIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let note = self
            .state()
            .db
            .get_note(&note_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| invalid(format!("no note with id {note_id}")))?;
        // An agent reading a note counts as a read for the curator.
        if let Err(err) = self
            .state()
            .db
            .bump_note_usage(std::slice::from_ref(&note.id), "reads", commands::now())
            .await
        {
            crate::note!("note usage bump (reads) failed: {err:#}");
        }
        json_result(&note)
    }

    #[tool(
        description = "Create a markdown note in a notebook and return it. When recording WHY you reached a conclusion — a claim, decision, or recommendation grounded in the sources — set kind:\"evidence\" and structure the body as: the claim, supporting passages (verbatim, each naming its source), search queries used, confidence (high/medium/low with why), counter-evidence or \"none found\", and open questions. Evidence notes make your reasoning auditable and let a later session pick up the thread."
    )]
    async fn create_note(
        &self,
        Parameters(CreateNoteReq {
            notebook_id,
            title,
            content,
            kind,
        }): Parameters<CreateNoteReq>,
    ) -> Result<CallToolResult, McpError> {
        let kind = match kind.as_deref().unwrap_or("note") {
            "" | "note" => "note",
            "evidence" => "evidence",
            other => {
                return Err(invalid(format!(
                    "unknown note kind \"{other}\" — use \"note\" or \"evidence\""
                )))
            }
        };
        let ts = commands::now();
        let note = Note {
            id: commands::new_id(),
            notebook_id: notebook_id.clone(),
            title: if title.trim().is_empty() {
                "Untitled note".into()
            } else {
                title.trim().to_string()
            },
            content,
            kind: kind.into(),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: ts,
            updated_at: ts,
        };
        commands::add_note_indexed(&self.state(), &note)
            .await
            .map_err(internal)?;
        self.changed("notes", Some(&notebook_id));
        json_result(&note)
    }

    #[tool(
        description = "Replace a note's title and content. Read the note first — this is a full replacement, and the user may have edited it since you last saw it."
    )]
    async fn update_note(
        &self,
        Parameters(UpdateNoteReq {
            note_id,
            title,
            content,
        }): Parameters<UpdateNoteReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let note = state
            .db
            .get_note(&note_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| invalid(format!("no note with id {note_id}")))?;
        state
            .db
            .update_note(&note_id, title.trim(), &content, commands::now())
            .await
            .map_err(internal)?;
        // An agent's deliberate edit takes ownership, same as a human's.
        state
            .db
            .set_note_origin(&note_id, "")
            .await
            .map_err(internal)?;
        if let Ok(Some(updated)) = state.db.get_note(&note_id).await {
            commands::index_note(&state, &updated).await;
        }
        self.changed("notes", Some(&note.notebook_id));
        json_result(&serde_json::json!({ "ok": true }))
    }

    #[tool(
        description = "Export a note to a file and return the written path. Formats match the note's shape: \"docx\" for prose, \"xlsx\" for notes with markdown tables, \"pptx\" for slide decks and flashcards, \"png\" for infographics and mind maps, \"m4a\" for Audio Overview episodes, and \"pdf\" for any kind (the note's own render, printed). Writes to `path` when given, otherwise ~/Downloads/<title>.<ext>."
    )]
    async fn export_note(
        &self,
        Parameters(ExportNoteReq {
            note_id,
            format,
            path,
        }): Parameters<ExportNoteReq>,
    ) -> Result<CallToolResult, McpError> {
        let written = crate::export::export_note_file(&self.app, &note_id, &format, path)
            .await
            .map_err(internal)?;
        json_result(&serde_json::json!({ "path": written }))
    }

    #[tool(
        description = "Delete notes. Accepts one note_id or a note_ids batch, removed in one bulk operation (docs/RFC-multi-select.md)."
    )]
    async fn delete_note(
        &self,
        Parameters(DeleteNoteReq { note_id, note_ids }): Parameters<DeleteNoteReq>,
    ) -> Result<CallToolResult, McpError> {
        let ids = match (note_ids, note_id) {
            (Some(ids), _) if !ids.is_empty() => ids,
            (_, Some(id)) => vec![id],
            _ => return Err(invalid("provide note_id or note_ids")),
        };
        let state = self.state();
        // Resolve every id first: a missing id used to slip through and
        // report success while deleting nothing, and a batch spanning
        // notebooks would emit `notes://changed` for whichever notebook the
        // first id happened to live in.
        let mut notebook_id: Option<String> = None;
        for id in &ids {
            let note = state
                .db
                .get_note(id)
                .await
                .map_err(internal)?
                .ok_or_else(|| invalid(format!("no note with id {id}")))?;
            match &notebook_id {
                None => notebook_id = Some(note.notebook_id),
                Some(first) if *first != note.notebook_id => {
                    return Err(invalid(
                        "note_ids span more than one notebook — delete them one notebook at a time",
                    ));
                }
                Some(_) => {}
            }
        }
        // An Audio Overview's episode file lives outside the DB — remove it too.
        for id in &ids {
            if let Some(path) = commands::audio_path(&self.app, id) {
                let _ = std::fs::remove_file(path);
            }
        }
        state.db.delete_notes(&ids).await.map_err(internal)?;
        self.changed("notes", notebook_id.as_deref());
        json_result(&serde_json::json!({ "ok": true, "deleted": ids.len() }))
    }
}
