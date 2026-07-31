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

#[tool_router(router = notebooks_router, vis = "pub(super)")]
impl AlchemyMcp {
    // -- Notebooks --

    #[tool(
        description = "List all notebooks with ids, titles, timestamps, and source counts. Start here to find or pick a notebook."
    )]
    async fn list_notebooks(&self) -> Result<CallToolResult, McpError> {
        let nbs: Vec<Notebook> = self.state().db.list_notebooks().await.map_err(internal)?;
        json_result(&nbs)
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
        let nb = Notebook {
            id: commands::new_id(),
            title,
            created_at: ts,
            updated_at: ts,
            color: NOTEBOOK_PALETTE[0].to_string(),
            source_count: 0,
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
        description = "Delete a notebook and everything in it (sources, chunks, chat, notes). Irreversible — confirm with the user before deleting anything they didn't explicitly ask to remove."
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
