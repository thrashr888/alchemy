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
struct BindOkfReq {
    /// Notebook id.
    notebook_id: String,
    /// Absolute path to the folder the notebook should keep itself in.
    path: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SharedPathReq {
    /// The bundle folder's absolute path, from list_shared_notebooks.
    path: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ResolveDeletionReq {
    /// Notebook id.
    notebook_id: String,
    /// The source or note id, from deletion_proposals.
    entity_id: String,
    /// True puts it back for both people; false accepts the deletion.
    restore: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct NotebookIdReq {
    /// Notebook id.
    notebook_id: String,
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
        description = "List all notebooks with ids, titles, timestamps, source counts, status (\"archived\" = hidden from the main grid), and okfPath — the folder a notebook is kept in as an OKF bundle, when it has one. A notebook with an okfPath can be edited as files: change a note by changing its markdown, and Alchemy reads it back."
    )]
    async fn list_notebooks(&self) -> Result<CallToolResult, McpError> {
        let nbs: Vec<Notebook> = self.state().db.list_notebooks().await.map_err(internal)?;
        // `okfPath` is machine-local (it lives in a sidecar, not a column), so
        // it rides alongside the row rather than inside it.
        let data_dir = crate::commands::app_data_dir(&self.state());
        let bindings = crate::okf::load_bindings_checked(&data_dir).map_err(internal)?;
        let rows: Vec<serde_json::Value> = nbs
            .iter()
            .map(|nb| {
                let mut row = serde_json::to_value(nb).unwrap_or_default();
                if let Some(obj) = row.as_object_mut() {
                    obj.insert(
                        "okfPath".into(),
                        bindings
                            .get(&nb.id)
                            .map(|b| serde_json::Value::String(b.path.clone()))
                            .unwrap_or(serde_json::Value::Null),
                    );
                }
                row
            })
            .collect();
        json_result(&rows)
    }

    #[tool(
        description = "Keep a notebook on disk as an Open Knowledge Format bundle at `path`, and keep it current: every change to the notebook's sources and notes lands in the folder within seconds. A folder that is not there yet is created; an empty one is seeded from the notebook; one that already holds a bundle is imported first, then bound. Once bound, the folder is the editing surface — edit a note by editing its markdown file."
    )]
    async fn bind_notebook_okf(
        &self,
        Parameters(BindOkfReq { notebook_id, path }): Parameters<BindOkfReq>,
    ) -> Result<CallToolResult, McpError> {
        let app = self.app.clone();
        let state = self.state();
        let bound = crate::okf::bind_impl(&app, &state, &notebook_id, &path)
            .await
            .map_err(internal)?;
        self.changed("notebooks", Some(&notebook_id));
        json_result(&serde_json::json!({ "notebookId": notebook_id, "okfPath": bound }))
    }

    #[tool(
        description = "Notebooks sitting at the root of iCloud Drive that this Mac hasn't opened — where a folder someone shared with the user lands (docs/RFC-shared-notebook.md). Each is a bundle folder with its title and, when its index carries one, its notebook id. Nothing here is opened on its own; the user (or you, on their say-so) opens one with open_shared_notebook."
    )]
    async fn list_shared_notebooks(&self) -> Result<CallToolResult, McpError> {
        let state = self.state();
        json_result(&crate::okf::shared_bundle_offers(&state).await)
    }

    #[tool(
        description = "Open a notebook found in iCloud Drive here: the same import-or-rebind path the Notebooks folder uses, for one folder. Only on the user's say-so — a shared folder is somebody's, and opening it binds this Mac to it."
    )]
    async fn open_shared_notebook(
        &self,
        Parameters(SharedPathReq { path }): Parameters<SharedPathReq>,
    ) -> Result<CallToolResult, McpError> {
        let app = self.app.clone();
        let state = self.state();
        let name = crate::okf::open_shared_bundle(&app, &state, &path)
            .await
            .map_err(invalid)?;
        self.changed("notebooks", None);
        json_result(&serde_json::json!({ "opened": name, "path": path }))
    }

    #[tool(
        description = "Put a notebook where another person can be invited into it (docs/RFC-shared-notebook.md) and mark it shared — which is what makes another person's deletions arrive as proposals rather than removals. A notebook in the app's container or a plain local folder moves into iCloud Drive/Alchemy Shared, keeping its files, its sync record and its history; one already in iCloud Drive, or in a Dropbox/Google Drive/OneDrive folder, is marked where it sits and never moved. Returns the folder's path, and shareFrom when the invitation belongs in that service rather than Finder. It cannot show the macOS share sheet, so tell the user where to make the invitation."
    )]
    async fn share_notebook(
        &self,
        Parameters(NotebookIdReq { notebook_id }): Parameters<NotebookIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let app = self.app.clone();
        let state = self.state();
        let (path, service) = crate::okf::share_notebook(&app, &state, &notebook_id)
            .await
            .map_err(invalid)?;
        self.changed("notebooks", None);
        json_result(&serde_json::json!({
            "notebookId": notebook_id,
            "sharedPath": path,
            // Set when the folder is somebody else's cloud and the invitation
            // is made there rather than in Finder.
            "shareFrom": service,
        }))
    }

    #[tool(
        description = "Deletions the other person in a shared notebook made that this Mac has not answered yet (docs/RFC-shared-notebook.md §3). In a shared folder another person's deletion record is a proposal, not an instruction: the source or note is still here and still readable until someone answers. Empty for a notebook that is not shared."
    )]
    async fn deletion_proposals(
        &self,
        Parameters(NotebookIdReq { notebook_id }): Parameters<NotebookIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let proposals = crate::okf::deletion_proposals(&state, &notebook_id)
            .await
            .map_err(invalid)?;
        json_result(&proposals)
    }

    #[tool(
        description = "Answer one of those proposals. restore=true puts the source or note back for both people (it goes out under a new sync id, so their deletion record cannot take it again); restore=false accepts the deletion and removes it here. Only on the user's say-so — this is their corpus and the other person's, not yours."
    )]
    async fn resolve_deletion_proposal(
        &self,
        Parameters(ResolveDeletionReq {
            notebook_id,
            entity_id,
            restore,
        }): Parameters<ResolveDeletionReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        crate::okf::resolve_deletion_proposal(&state, &notebook_id, &entity_id, restore)
            .await
            .map_err(invalid)?;
        self.changed("sources", Some(&notebook_id));
        json_result(&serde_json::json!({ "id": entity_id, "restored": restore }))
    }

    #[tool(
        description = "Stop keeping a notebook on disk. The bundle folder and everything in it stays exactly where it is; Alchemy simply stops writing to it and reading from it."
    )]
    async fn unbind_notebook_okf(
        &self,
        Parameters(NotebookIdReq { notebook_id }): Parameters<NotebookIdReq>,
    ) -> Result<CallToolResult, McpError> {
        // Through the same path the menu verb takes, so the answer is the
        // truth: a caller told `okfPath: null` while the binding was still
        // being rewritten by an in-flight write had no way to know.
        crate::okf::unbind_impl(&self.state(), &notebook_id)
            .await
            .map_err(internal)?;
        self.changed("notebooks", Some(&notebook_id));
        json_result(&serde_json::json!({ "notebookId": notebook_id, "okfPath": null }))
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
            growth_web: false,
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
