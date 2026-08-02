//! Source ingest and read tools.

use super::*;
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddSourceReq {
    /// Notebook to add the source to.
    notebook_id: String,
    /// Web page URL to fetch and extract, or a cider:// Mac-item origin
    /// (e.g. cider://reminders/list/Shopping) to connect as a living source
    /// (exactly one of url / text / file_path).
    #[serde(default)]
    url: Option<String>,
    /// Raw text/markdown content to store as a source.
    #[serde(default)]
    text: Option<String>,
    /// Absolute path to a local file (pdf, md, txt, csv, xlsx, docx, images…).
    #[serde(default)]
    file_path: Option<String>,
    /// Title for `text` sources and optional label for cider:// sources
    /// (ignored for web url/file, which derive their own).
    #[serde(default)]
    title: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SourceIdReq {
    /// Source id (from list_sources).
    source_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ActivityReq {
    /// Look-back window in hours (default 24, capped to the 30-day event
    /// window the table keeps).
    #[serde(default)]
    hours: Option<u32>,
}

#[tool_router(router = sources_router, vis = "pub(super)")]
impl AlchemyMcp {
    // -- Sources --

    #[tool(
        description = "Recent source-change events across ALL notebooks (what the resident scheduler's resyncs observed): each event has notebook_id, source_id, source_title, kind, a short detail, and a millisecond timestamp, newest first. The same signal the Morning Brief's \"what changed\" reads. Default window 24 hours."
    )]
    async fn list_source_events(
        &self,
        Parameters(ActivityReq { hours }): Parameters<ActivityReq>,
    ) -> Result<CallToolResult, McpError> {
        let hours = i64::from(hours.unwrap_or(24));
        let since = commands::now() - hours * 60 * 60 * 1000;
        let events = self
            .state()
            .db
            .source_events_since(since)
            .await
            .map_err(|e| invalid(format!("{e:#}")))?;
        json_result(&events)
    }

    #[tool(
        description = "List a notebook's sources (id, title, type, url, status, char/chunk counts). status \"error\" means the import failed — see the error field."
    )]
    async fn list_sources(
        &self,
        Parameters(NotebookIdReq { notebook_id }): Parameters<NotebookIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let sources: Vec<Source> = self
            .state()
            .db
            .list_sources(&notebook_id)
            .await
            .map_err(internal)?
            .into_iter()
            .map(slim)
            .collect();
        json_result(&sources)
    }

    #[tool(
        description = "Add a source to a notebook. Provide exactly one of: url (fetched + article-extracted), text (pasted content; give a title), or file_path (local pdf/md/txt/csv/xlsx/docx/image — images and scanned PDFs are OCR'd when a vision model is configured). url also accepts a cider:// origin to connect a Mac item as a living, auto-syncing source: cider://reminders/list/<list name>, cider://calendar/upcoming/<days>, cider://notes/note/<note id>, or cider://stocks/watchlist/<name>. Content is chunked and embedded automatically. Duplicate content or an already-added URL is rejected with an error naming the existing source — treat that as already done."
    )]
    async fn add_source(
        &self,
        Parameters(AddSourceReq {
            notebook_id,
            url,
            text,
            file_path,
            title,
        }): Parameters<AddSourceReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let provided = [url.is_some(), text.is_some(), file_path.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        if provided != 1 {
            return Err(invalid("provide exactly one of url, text, or file_path"));
        }
        let source = if let Some(url) = url {
            if crate::mac::is_mac_uri(&url) {
                // Mac items connect as living, auto-syncing sources — never
                // through the web fetcher, which can't reach cider:// origins.
                commands::ingest_mac(&state, &notebook_id, &url, title.as_deref().unwrap_or(""))
                    .await
                    .map_err(internal)?
            } else {
                commands::ingest_url(&state, &notebook_id, &url, None)
                    .await
                    .map_err(internal)?
            }
        } else if let Some(text) = text {
            let title = title.unwrap_or_else(|| "Untitled source".into());
            let extracted = crate::ingest::extract_pasted(&title, &text).map_err(internal)?;
            commands::store_extracted(&state, &notebook_id, extracted)
                .await
                .map_err(internal)?
        } else {
            let path = file_path.unwrap();
            let mut extracted = commands::extract_any_file(&state, &path)
                .await
                .map_err(internal)?;
            commands::friendly_title(&state, &mut extracted).await;
            commands::store_extracted(&state, &notebook_id, extracted)
                .await
                .map_err(internal)?
        };
        self.changed("sources", Some(&notebook_id));
        json_result(&source)
    }

    #[tool(description = "Read a source's metadata and full extracted text content.")]
    async fn get_source(
        &self,
        Parameters(SourceIdReq { source_id }): Parameters<SourceIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let source = state
            .db
            .get_source(&source_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| invalid(format!("no source with id {source_id}")))?;
        let content = state
            .db
            .source_content(&source_id)
            .await
            .map_err(internal)?;
        json_result(&Source { content, ..source })
    }

    #[tool(description = "Delete a source and its chunks from a notebook.")]
    async fn delete_source(
        &self,
        Parameters(SourceIdReq { source_id }): Parameters<SourceIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let notebook_id = state
            .db
            .get_source(&source_id)
            .await
            .map_err(internal)?
            .map(|s| s.notebook_id);
        state.db.delete_source(&source_id).await.map_err(internal)?;
        self.changed("sources", notebook_id.as_deref());
        json_result(&serde_json::json!({ "ok": true }))
    }
}
