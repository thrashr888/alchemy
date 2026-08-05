//! Source ingest and read tools.

use super::*;
use rmcp::{
    handler::server::wrapper::Parameters, service::RequestContext, tool, tool_router,
    ErrorData as McpError, RoleServer,
};

/// Concurrent add_source calls queue here instead of all running at once.
/// Imports are heavy — PDFium rasterization (globally mutexed), per-page OCR,
/// embedding — and a burst of parallel calls mostly serializes on those locks
/// anyway, stretching every call's wall clock past the clients' idle
/// timeouts. Two at a time lets a small text add slip past a big scanned PDF
/// without letting a batch starve them all.
static IMPORT_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// Progress heartbeat for one long import. MCP clients abandon a call that
/// stays silent too long (Claude Code: 300s), so while an import waits on the
/// gate or OCRs a scanned PDF, a beat every 20 seconds keeps the call — and
/// the rmcp session's own keep-alive — visibly alive. Clients that sent no
/// progress token get no beats (the spec forbids inventing one); the raised
/// session keep-alive in mod.rs still covers them server-side.
struct Heartbeat(Option<tokio::task::JoinHandle<()>>);

impl Heartbeat {
    fn start(ctx: &RequestContext<RoleServer>, message: String) -> Self {
        let Some(token) = ctx.meta.get_progress_token() else {
            return Self(None);
        };
        let peer = ctx.peer.clone();
        Self(Some(tokio::spawn(async move {
            for beat in 1u64.. {
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                let progress = ProgressNotificationParam::new(token.clone(), beat as f64)
                    .with_message(message.clone());
                if peer.notify_progress(progress).await.is_err() {
                    return; // peer gone — nothing left to keep alive
                }
            }
        })))
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

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
struct UpdateSourceReq {
    /// Source id (from list_sources).
    source_id: String,
    /// New title; empty keeps the current title.
    #[serde(default)]
    title: String,
    /// The full replacement text.
    text: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SetTagsReq {
    /// Source id (from list_sources).
    source_id: String,
    /// Tags as free text — `#` prefixes, commas, and mixed case are all
    /// accepted; stored normalized (lowercase, deduped, space-separated).
    /// Empty clears all tags.
    tags: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SetNoteReq {
    /// Source id (from list_sources).
    source_id: String,
    /// The annotation text ("why this source matters"). Empty clears it.
    note: String,
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
        description = "List a notebook's sources (id, title, type, url, status, char/chunk counts, tags and note — the user's own labels and annotation — and image_url, the page's lead image for url sources; \"-\" means checked and none). status \"error\" means the import failed — see the error field; \"processing\" means the content is stored and readable but still being indexed — search reaches it shortly, no action needed."
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let provided = [url.is_some(), text.is_some(), file_path.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        if provided != 1 {
            return Err(invalid("provide exactly one of url, text, or file_path"));
        }
        let what = url
            .clone()
            .or_else(|| file_path.clone())
            .or_else(|| title.clone())
            .unwrap_or_else(|| "pasted text".into());
        let _heartbeat = Heartbeat::start(&ctx, format!("importing {what}"));
        let _permit = IMPORT_GATE
            .acquire()
            .await
            .expect("import gate never closes");
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
            let settled = commands::friendly_title_fast(&mut extracted);
            let src = commands::store_extracted(&state, &notebook_id, extracted)
                .await
                .map_err(internal)?;
            if !settled {
                commands::spawn_retitle(&state, &src).await;
            }
            src
        };
        self.changed("sources", Some(&notebook_id));
        json_result(&source)
    }

    #[tool(
        description = "Read a source's metadata (including its user tags and annotation note) and full extracted text content."
    )]
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

    #[tool(
        description = "Replace an editable source's title and full text (pasted/text/markdown/file-extracted sources — not url or mac mirrors, which refresh from their origin). The new text is re-chunked and re-embedded. Returns the updated source."
    )]
    async fn update_source(
        &self,
        Parameters(UpdateSourceReq {
            source_id,
            title,
            text,
        }): Parameters<UpdateSourceReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let existing = state
            .db
            .get_source(&source_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| invalid(format!("no source with id {source_id}")))?;
        if matches!(
            existing.source_type.as_str(),
            "url" | "mac" | "folder" | "git" | "notion" | "obsidian"
        ) {
            return Err(invalid(format!(
                "{} sources mirror an origin and can't be edited — refresh them instead",
                existing.source_type
            )));
        }
        let title = if title.trim().is_empty() {
            existing.title.clone()
        } else {
            title.trim().to_string()
        };
        let extracted = crate::ingest::extract_pasted(&title, &text).map_err(internal)?;
        let source = commands::reingest(&state, &existing, extracted, None, true)
            .await
            .map_err(internal)?;
        self.changed("sources", Some(&source.notebook_id));
        json_result(&slim(source))
    }

    #[tool(
        description = "Set a source's tags (user organization that retrieval also uses: tags join the source's route summary and the chat manifest). Free-form input — '#' prefixes, commas, mixed case all accepted; stored normalized (lowercase, deduped, space-separated). Empty tags clears them. Returns the updated source."
    )]
    async fn set_source_tags(
        &self,
        Parameters(SetTagsReq { source_id, tags }): Parameters<SetTagsReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let source = commands::set_source_tags_impl(&state, &source_id, &tags)
            .await
            .map_err(internal)?;
        self.changed("sources", Some(&source.notebook_id));
        json_result(&source)
    }

    #[tool(
        description = "Set the user annotation on a source (one editable note per source: \"why this was saved\"). The note is indexed for retrieval and surfaces in chat labeled as the user's own judgment. Empty note clears it. Returns the updated source."
    )]
    async fn set_source_note(
        &self,
        Parameters(SetNoteReq { source_id, note }): Parameters<SetNoteReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let source = commands::set_source_note_impl(&state, &source_id, &note)
            .await
            .map_err(internal)?;
        self.changed("sources", Some(&source.notebook_id));
        json_result(&source)
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
