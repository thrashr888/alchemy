//! Notebook lifecycle tools.

use super::*;
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TitleReq {
    /// Notebook title.
    title: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct RenameNotebookReq {
    /// Notebook id.
    id: String,
    /// New title.
    title: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ArchiveNotebookReq {
    /// Notebook id.
    notebook_id: String,
    /// true to archive, false to restore.
    archived: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SuggestNotebookReq {
    /// Title of the incoming source, if known.
    #[serde(default)]
    title: String,
    /// The source's text. Leave empty when passing a `url` instead.
    #[serde(default)]
    text: String,
    /// A URL to file. Fetched and extracted when `text` is empty.
    #[serde(default)]
    url: String,
}

#[tool_router(router = notebooks_router, vis = "pub(super)")]
impl AlchemyMcp {
    // -- Notebooks --

    #[tool(
        description = "List all notebooks with ids, titles, timestamps, source counts, and status (\"archived\" = hidden from the main grid). Start here to find or pick a notebook."
    )]
    async fn list_notebooks(&self) -> Result<CallToolResult, McpError> {
        let nbs: Vec<Notebook> = self.state().db.list_notebooks().await.map_err(internal)?;
        json_result(&nbs)
    }

    #[tool(
        description = "Ask where an unfiled source belongs before adding it. Returns {notebookId, title, isNew}: an existing notebook to file into, or isNew=true with a proposed title when nothing fits (create it, then add). Pass the text, or just a url and it will be fetched."
    )]
    async fn suggest_notebook(
        &self,
        Parameters(SuggestNotebookReq { title, text, url }): Parameters<SuggestNotebookReq>,
    ) -> Result<CallToolResult, McpError> {
        let (title, text) = if text.trim().is_empty() && !url.trim().is_empty() {
            match crate::ingest::extract_url(&url).await {
                Ok(ex) => (
                    if title.trim().is_empty() {
                        ex.title
                    } else {
                        title
                    },
                    ex.text,
                ),
                Err(_) => (if title.is_empty() { url.clone() } else { title }, url),
            }
        } else {
            (title, text)
        };
        // Snapshot the Ai under a momentary read guard, never across an await.
        let ai = self.state().ai.read().await.clone();
        let suggestion = crate::router::suggest_notebook(&self.state().db, &ai, &title, &text)
            .await
            .map_err(internal)?;
        json_result(&suggestion)
    }

    #[tool(description = "Create a new notebook and return it (including its id).")]
    async fn create_notebook(
        &self,
        Parameters(TitleReq { title }): Parameters<TitleReq>,
    ) -> Result<CallToolResult, McpError> {
        let ts = commands::now();
        let title = if title.trim().is_empty() {
            "Untitled notebook".into()
        } else {
            title.trim().to_string()
        };
        let icon = commands::auto_notebook_icon(&title);
        let nb = Notebook {
            id: commands::new_id(),
            title,
            created_at: ts,
            updated_at: ts,
            color: NOTEBOOK_PALETTE[0].to_string(),
            icon,
            status: String::new(),
            source_count: 0,
            note_count: 0,
            report_count: 0,
        };
        self.state()
            .db
            .create_notebook(&nb)
            .await
            .map_err(internal)?;
        self.changed("notebooks", Some(&nb.id));
        json_result(&nb)
    }

    #[tool(description = "Rename a notebook.")]
    async fn rename_notebook(
        &self,
        Parameters(RenameNotebookReq { id, title }): Parameters<RenameNotebookReq>,
    ) -> Result<CallToolResult, McpError> {
        self.state()
            .db
            .rename_notebook(&id, title.trim(), commands::now())
            .await
            .map_err(internal)?;
        self.changed("notebooks", Some(&id));
        json_result(&serde_json::json!({ "ok": true }))
    }

    #[tool(
        description = "Archive a notebook (archived: true) or restore it (archived: false). Archiving hides the notebook from the main grid but keeps all data — prefer this over delete_notebook unless the user explicitly wants data gone."
    )]
    async fn archive_notebook(
        &self,
        Parameters(ArchiveNotebookReq {
            notebook_id,
            archived,
        }): Parameters<ArchiveNotebookReq>,
    ) -> Result<CallToolResult, McpError> {
        let status = if archived { "archived" } else { "" };
        self.state()
            .db
            .set_notebook_status(&notebook_id, status)
            .await
            .map_err(internal)?;
        self.changed("notebooks", Some(&notebook_id));
        json_result(&serde_json::json!({ "ok": true, "status": status }))
    }

    #[tool(
        description = "Delete a notebook and everything in it (sources, chunks, chat, notes). Irreversible — confirm with the user before deleting anything they didn't explicitly ask to remove; prefer archive_notebook when in doubt."
    )]
    async fn delete_notebook(
        &self,
        Parameters(NotebookIdReq { notebook_id }): Parameters<NotebookIdReq>,
    ) -> Result<CallToolResult, McpError> {
        self.state()
            .db
            .delete_notebook(&notebook_id)
            .await
            .map_err(internal)?;
        self.changed("notebooks", Some(&notebook_id));
        json_result(&serde_json::json!({ "ok": true }))
    }
}
