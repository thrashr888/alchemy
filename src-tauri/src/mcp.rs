//! Embedded MCP server — agent access to notebooks, sources, notes, and
//! hybrid search over localhost streamable HTTP (see docs/RFC-mcp-server.md).
//!
//! One process owns everything: tools run against the same `AppState` the UI
//! commands use, and every mutation emits `mcp://changed` so open windows
//! refresh live while an agent works.

use rmcp::{
    handler::server::wrapper::Parameters,
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData as McpError, ServerHandler,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{self, AppState};
use crate::db::NOTEBOOK_PALETTE;
use crate::models::{Note, Notebook, Source};

// ---- Server lifecycle ------------------------------------------------------

/// Managed handle to the running server, if any. Settings toggles it.
#[derive(Default)]
pub struct McpState {
    running: std::sync::Mutex<Option<Running>>,
}

struct Running {
    port: u16,
    shutdown: tokio_util::sync::CancellationToken,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub running: bool,
    pub port: u16,
    pub url: String,
}

/// Start the server if the config wants it running (app launch + settings
/// save). Stops first when the port changed or it was disabled.
pub async fn apply_config(app: &AppHandle, enabled: bool, port: u16) {
    let mcp = app.state::<McpState>();
    {
        let mut running = mcp.running.lock().unwrap();
        match running.as_ref() {
            Some(r) if !enabled || r.port != port => {
                r.shutdown.cancel();
                *running = None;
                remove_port_file(app);
            }
            Some(_) => return, // already running on the right port
            None => {}
        }
        if !enabled {
            return;
        }
    }
    match start_server(app.clone(), port).await {
        Ok(shutdown) => {
            *mcp.running.lock().unwrap() = Some(Running { port, shutdown });
            write_port_file(app, port);
        }
        Err(err) => eprintln!("mcp: failed to start on 127.0.0.1:{port}: {err:#}"),
    }
}

pub fn status(app: &AppHandle) -> McpStatus {
    let mcp = app.state::<McpState>();
    let running = mcp.running.lock().unwrap();
    let port = running.as_ref().map(|r| r.port).unwrap_or(0);
    McpStatus {
        running: running.is_some(),
        port,
        url: format!("http://127.0.0.1:{port}/mcp"),
    }
}

/// Reject anything that looks like it came from a browser page. Browsers
/// always attach `Origin` to cross-origin requests, so this closes the
/// malicious-webpage → localhost hole; real MCP clients never send one.
async fn reject_browser_origins(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    if req.headers().contains_key(axum::http::header::ORIGIN) {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }
    Ok(next.run(req).await)
}

async fn start_server(
    app: AppHandle,
    port: u16,
) -> anyhow::Result<tokio_util::sync::CancellationToken> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let shutdown = tokio_util::sync::CancellationToken::new();

    let service = StreamableHttpService::new(
        move || Ok(AlchemyMcp::new(app.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn(reject_browser_origins));

    let ct = shutdown.clone();
    tauri::async_runtime::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { ct.cancelled().await })
            .await;
    });
    Ok(shutdown)
}

/// Discovery file so tooling can find the server without hardcoding the port.
fn write_port_file(app: &AppHandle, port: u16) {
    if let Ok(dir) = app.path().app_data_dir() {
        let info = serde_json::json!({
            "port": port,
            "url": format!("http://127.0.0.1:{port}/mcp"),
            "pid": std::process::id(),
        });
        let _ = std::fs::write(dir.join("mcp.json"), info.to_string());
    }
}

fn remove_port_file(app: &AppHandle) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::remove_file(dir.join("mcp.json"));
    }
}

// ---- Tauri commands (Settings UI) -----------------------------------------

#[tauri::command]
pub fn mcp_status(app: AppHandle) -> McpStatus {
    status(&app)
}

// ---- Tool parameter shapes -------------------------------------------------

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TitleReq {
    /// Notebook title.
    title: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct NotebookIdReq {
    /// Notebook id (from list_notebooks).
    notebook_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct RenameNotebookReq {
    /// Notebook id.
    id: String,
    /// New title.
    title: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddSourceReq {
    /// Notebook to add the source to.
    notebook_id: String,
    /// Web page URL to fetch and extract (exactly one of url / text / file_path).
    #[serde(default)]
    url: Option<String>,
    /// Raw text/markdown content to store as a source.
    #[serde(default)]
    text: Option<String>,
    /// Absolute path to a local file (pdf, md, txt, csv, xlsx, docx, images…).
    #[serde(default)]
    file_path: Option<String>,
    /// Title for `text` sources (ignored for url/file, which derive their own).
    #[serde(default)]
    title: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SourceIdReq {
    /// Source id (from list_sources).
    source_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AskEverythingReq {
    /// The question to retrieve corpus-wide passages for.
    question: String,
}

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
struct SearchReq {
    /// Notebook to search.
    notebook_id: String,
    /// Natural-language query; hybrid vector + keyword search.
    query: String,
    /// Max passages to return (default 6, max 20).
    #[serde(default)]
    max_results: Option<u32>,
}

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

// ---- The MCP service --------------------------------------------------------

#[derive(Clone)]
pub struct AlchemyMcp {
    app: AppHandle,
}

fn internal(err: impl std::fmt::Display) -> McpError {
    McpError::internal_error(format!("{err:#}"), None)
}

fn invalid(msg: impl Into<String>) -> McpError {
    McpError::invalid_params(msg.into(), None)
}

fn json_result(value: &impl Serialize) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(value).map_err(internal)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Strip full content from a source for list payloads (same as the UI does).
fn slim(s: Source) -> Source {
    Source {
        content: String::new(),
        ..s
    }
}

#[tool_router]
impl AlchemyMcp {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn state(&self) -> tauri::State<'_, AppState> {
        self.app.state::<AppState>()
    }

    /// Tell open windows something changed so lists refresh live.
    fn changed(&self, scope: &str, notebook_id: Option<&str>) {
        #[derive(Serialize, Clone)]
        #[serde(rename_all = "camelCase")]
        struct Changed<'a> {
            scope: &'a str,
            notebook_id: Option<&'a str>,
        }
        let _ = self
            .app
            .emit("mcp://changed", Changed { scope, notebook_id });
    }

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

    // -- Sources --

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
        description = "Add a source to a notebook. Provide exactly one of: url (fetched + article-extracted), text (pasted content; give a title), or file_path (local pdf/md/txt/csv/xlsx/docx/image — images and scanned PDFs are OCR'd when a vision model is configured). Content is chunked and embedded automatically. Duplicate content or an already-added URL is rejected with an error naming the existing source — treat that as already done."
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
            commands::ingest_url(&state, &notebook_id, &url)
                .await
                .map_err(internal)?
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

    #[tool(
        description = "Retrieve passages for a question across ALL notebooks at once (hybrid vector + keyword, rank-fused, plus matching notes). Each passage names its notebook — use this to answer 'which notebook has…' questions or to ground corpus-wide answers. Synthesize the answer yourself from the passages."
    )]
    async fn ask_everything(
        &self,
        Parameters(AskEverythingReq { question }): Parameters<AskEverythingReq>,
    ) -> Result<CallToolResult, McpError> {
        let question = question.trim().to_string();
        if question.is_empty() {
            return Err(invalid("question is empty"));
        }
        let state = self.state();
        let passages = commands::retrieve_everything(&state, &question, 16)
            .await
            .map_err(internal)?;
        json_result(&passages)
    }

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

    // -- Search --

    #[tool(
        description = "Hybrid search (vector similarity + BM25 keyword, rank-fused) over a notebook's source chunks. Runs on the local embedder — cheap, call freely. Returns passages with sourceId/sourceTitle/snippet/distance; use get_source for a passage's full document. Synthesize answers yourself from the passages."
    )]
    async fn search(
        &self,
        Parameters(SearchReq {
            notebook_id,
            query,
            max_results,
        }): Parameters<SearchReq>,
    ) -> Result<CallToolResult, McpError> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Err(invalid("query is empty"));
        }
        let k = max_results.unwrap_or(6).clamp(1, 20) as usize;
        let state = self.state();
        let query_vec = {
            let ai = state.ai.read().await;
            ai.embed_one(&query).await.map_err(internal)?
        };
        let citations = state
            .db
            .search_chunks(&notebook_id, query_vec, &query, k, None)
            .await
            .map_err(internal)?;
        json_result(&citations)
    }

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
        json_result(&note)
    }

    #[tool(description = "Create a markdown note in a notebook and return it.")]
    async fn create_note(
        &self,
        Parameters(CreateNoteReq {
            notebook_id,
            title,
            content,
        }): Parameters<CreateNoteReq>,
    ) -> Result<CallToolResult, McpError> {
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
            kind: "note".into(),
            prompt: String::new(),
            created_at: ts,
            updated_at: ts,
        };
        self.state().db.add_note(&note).await.map_err(internal)?;
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
        self.changed("notes", Some(&note.notebook_id));
        json_result(&serde_json::json!({ "ok": true }))
    }

    #[tool(description = "Delete a note.")]
    async fn delete_note(
        &self,
        Parameters(NoteIdReq { note_id }): Parameters<NoteIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let notebook_id = state
            .db
            .get_note(&note_id)
            .await
            .map_err(internal)?
            .map(|n| n.notebook_id);
        // An Audio Overview's episode file lives outside the DB — remove it too.
        if let Some(path) = commands::audio_path(&self.app, &note_id) {
            let _ = std::fs::remove_file(path);
        }
        state.db.delete_note(&note_id).await.map_err(internal)?;
        self.changed("notes", notebook_id.as_deref());
        json_result(&serde_json::json!({ "ok": true }))
    }
}

#[tool_handler]
impl ServerHandler for AlchemyMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("alchemy", env!("CARGO_PKG_VERSION")).with_title("Alchemy"),
            )
            .with_instructions(
                "Alchemy is the user's local-first research notebook: notebooks hold sources \
                 (documents, web pages, pasted text) and notes. Typical flow: list_notebooks \
                 (or create_notebook) → add_source for each URL/file/text → search to find \
                 relevant passages → write findings with create_note. Everything runs on the \
                 user's machine; search is cheap, call it freely.",
            )
    }
}
