//! LanceDB persistence layer. Everything lives in one embedded Lance database:
//! notebooks, sources, chunks (with vectors), messages, and notes — each its own
//! Lance table. We filter by `notebook_id` instead of joining.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use arrow_array::types::Float32Type;
use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Int32Array, Int64Array, RecordBatch, RecordBatchIterator,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::Connection;

use crate::models::{Citation, Message, Note, Notebook, ReportSchedule, Source};

const T_NOTEBOOKS: &str = "notebooks";
const T_SOURCES: &str = "sources";
const T_CHUNKS: &str = "chunks";
const T_MESSAGES: &str = "messages";
const T_NOTES: &str = "notes";
const T_REPORTS: &str = "report_schedules";

pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (creating if needed) the Lance database at `dir` and ensure the
    /// fixed-schema tables exist. The chunks table is created lazily once we
    /// know the embedding dimensionality.
    pub async fn open(dir: &std::path::Path) -> Result<Self> {
        std::fs::create_dir_all(dir).context("failed to create data dir")?;
        let uri = dir.to_string_lossy().to_string();
        let conn = lancedb::connect(&uri)
            .execute()
            .await
            .context("failed to open LanceDB")?;
        let db = Self { conn };
        db.ensure_table(T_NOTEBOOKS, notebooks_schema()).await?;
        db.ensure_table(T_SOURCES, sources_schema()).await?;
        db.migrate_sources().await?;
        db.ensure_table(T_MESSAGES, messages_schema()).await?;
        db.ensure_table(T_NOTES, notes_schema()).await?;
        db.migrate_notes().await?;
        db.ensure_table(T_REPORTS, reports_schema()).await?;
        Ok(db)
    }

    /// Backfill the `prompt` column on pre-existing `notes` tables.
    async fn migrate_notes(&self) -> Result<()> {
        if !self.table_exists(T_NOTES).await? {
            return Ok(());
        }
        let schema = self.conn.open_table(T_NOTES).execute().await?.schema().await?;
        if schema.field_with_name("prompt").is_ok() {
            return Ok(());
        }
        let batches = self.collect(T_NOTES, None).await?;
        let mut notes = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let title = str_col(b, "title")?;
            let content = str_col(b, "content")?;
            let kind = str_col(b, "kind")?;
            let created = i64_col(b, "created_at")?;
            let updated = i64_col(b, "updated_at")?;
            for i in 0..b.num_rows() {
                notes.push(Note {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    title: title.value(i).to_string(),
                    content: content.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    prompt: String::new(),
                    created_at: created.value(i),
                    updated_at: updated.value(i),
                });
            }
        }
        self.conn.drop_table(T_NOTES, &[]).await?;
        self.ensure_table(T_NOTES, notes_schema()).await?;
        if !notes.is_empty() {
            let schema = notes_schema();
            let batch = note_batch(&schema, &notes)?;
            self.add_batch(T_NOTES, schema, batch).await?;
        }
        Ok(())
    }

    /// Bring a pre-existing `sources` table up to the current schema by
    /// rebuilding it, backfilling any missing columns (`url`, `status`,
    /// `error`) with defaults. No-op once all columns are present.
    async fn migrate_sources(&self) -> Result<()> {
        if !self.table_exists(T_SOURCES).await? {
            return Ok(());
        }
        let schema = self.conn.open_table(T_SOURCES).execute().await?.schema().await?;
        let has = |n: &str| schema.field_with_name(n).is_ok();
        if has("url") && has("status") && has("error") {
            return Ok(());
        }

        // Read whatever columns exist; optional ones get defaults.
        let batches = self.collect(T_SOURCES, None).await?;
        let mut sources = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let title = str_col(b, "title")?;
            let stype = str_col(b, "source_type")?;
            let content = str_col(b, "content")?;
            let cc = i64_col(b, "char_count")?;
            let ck = i64_col(b, "chunk_count")?;
            let ca = i64_col(b, "created_at")?;
            let url = opt_str_col(b, "url");
            let status = opt_str_col(b, "status");
            let error = opt_str_col(b, "error");
            for i in 0..b.num_rows() {
                sources.push(Source {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    title: title.value(i).to_string(),
                    source_type: stype.value(i).to_string(),
                    url: url.map(|a| a.value(i).to_string()).unwrap_or_default(),
                    content: content.value(i).to_string(),
                    char_count: cc.value(i),
                    chunk_count: ck.value(i),
                    created_at: ca.value(i),
                    status: status
                        .map(|a| a.value(i).to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "ready".to_string()),
                    error: error.map(|a| a.value(i).to_string()).unwrap_or_default(),
                });
            }
        }

        self.conn.drop_table(T_SOURCES, &[]).await?;
        self.ensure_table(T_SOURCES, sources_schema()).await?;
        if !sources.is_empty() {
            let schema = sources_schema();
            let batch = source_batch(&schema, &sources)?;
            self.add_batch(T_SOURCES, schema, batch).await?;
        }
        Ok(())
    }

    async fn table_exists(&self, name: &str) -> Result<bool> {
        Ok(self.conn.table_names().execute().await?.iter().any(|t| t == name))
    }

    async fn ensure_table(&self, name: &str, schema: SchemaRef) -> Result<()> {
        if !self.table_exists(name).await? {
            self.conn
                .create_empty_table(name, schema)
                .execute()
                .await
                .with_context(|| format!("failed to create table {name}"))?;
        }
        Ok(())
    }

    async fn add_batch(&self, table: &str, schema: SchemaRef, batch: RecordBatch) -> Result<()> {
        let tbl = self.conn.open_table(table).execute().await?;
        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let boxed: Box<dyn arrow_array::RecordBatchReader + Send> = Box::new(reader);
        tbl.add(boxed).execute().await?;
        Ok(())
    }

    async fn collect(&self, table: &str, filter: Option<&str>) -> Result<Vec<RecordBatch>> {
        if !self.table_exists(table).await? {
            return Ok(vec![]);
        }
        let tbl = self.conn.open_table(table).execute().await?;
        let mut q = tbl.query();
        if let Some(f) = filter {
            q = q.only_if(f);
        }
        let batches = q.execute().await?.try_collect::<Vec<_>>().await?;
        Ok(batches)
    }

    async fn delete_where(&self, table: &str, predicate: &str) -> Result<()> {
        if self.table_exists(table).await? {
            let tbl = self.conn.open_table(table).execute().await?;
            tbl.delete(predicate).await?;
        }
        Ok(())
    }

    // ---- Notebooks -------------------------------------------------------

    pub async fn list_notebooks(&self) -> Result<Vec<Notebook>> {
        let batches = self.collect(T_NOTEBOOKS, None).await?;
        let mut notebooks = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let title = str_col(b, "title")?;
            let created = i64_col(b, "created_at")?;
            let updated = i64_col(b, "updated_at")?;
            for i in 0..b.num_rows() {
                notebooks.push(Notebook {
                    id: id.value(i).to_string(),
                    title: title.value(i).to_string(),
                    created_at: created.value(i),
                    updated_at: updated.value(i),
                    source_count: 0,
                });
            }
        }

        // Count sources per notebook in one pass.
        let mut counts: HashMap<String, i64> = HashMap::new();
        for b in &self.collect(T_SOURCES, None).await? {
            let nb = str_col(b, "notebook_id")?;
            for i in 0..b.num_rows() {
                *counts.entry(nb.value(i).to_string()).or_insert(0) += 1;
            }
        }
        for n in &mut notebooks {
            n.source_count = counts.get(&n.id).copied().unwrap_or(0);
        }
        notebooks.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(notebooks)
    }

    pub async fn create_notebook(&self, notebook: &Notebook) -> Result<()> {
        let schema = notebooks_schema();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![notebook.id.clone()])),
                Arc::new(StringArray::from(vec![notebook.title.clone()])),
                Arc::new(Int64Array::from(vec![notebook.created_at])),
                Arc::new(Int64Array::from(vec![notebook.updated_at])),
            ],
        )?;
        self.add_batch(T_NOTEBOOKS, schema, batch).await
    }

    pub async fn rename_notebook(&self, id: &str, title: &str, updated_at: i64) -> Result<()> {
        let tbl = self.conn.open_table(T_NOTEBOOKS).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("title", format!("'{}'", esc(title)))
            .column("updated_at", updated_at.to_string())
            .execute()
            .await?;
        Ok(())
    }

    pub async fn touch_notebook(&self, id: &str, updated_at: i64) -> Result<()> {
        let tbl = self.conn.open_table(T_NOTEBOOKS).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("updated_at", updated_at.to_string())
            .execute()
            .await?;
        Ok(())
    }

    pub async fn delete_notebook(&self, id: &str) -> Result<()> {
        let pred = format!("notebook_id = '{}'", esc(id));
        self.delete_where(T_SOURCES, &pred).await?;
        self.delete_where(T_CHUNKS, &pred).await?;
        self.delete_where(T_MESSAGES, &pred).await?;
        self.delete_where(T_NOTES, &pred).await?;
        self.delete_where(T_NOTEBOOKS, &format!("id = '{}'", esc(id))).await?;
        Ok(())
    }

    // ---- Sources & chunks ------------------------------------------------

    pub async fn list_sources(&self, notebook_id: &str) -> Result<Vec<Source>> {
        let filter = format!("notebook_id = '{}'", esc(notebook_id));
        let batches = self.collect(T_SOURCES, Some(&filter)).await?;
        let mut sources = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let title = str_col(b, "title")?;
            let stype = str_col(b, "source_type")?;
            let url = str_col(b, "url")?;
            let char_count = i64_col(b, "char_count")?;
            let chunk_count = i64_col(b, "chunk_count")?;
            let created = i64_col(b, "created_at")?;
            let status = str_col(b, "status")?;
            let error = str_col(b, "error")?;
            for i in 0..b.num_rows() {
                sources.push(Source {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    title: title.value(i).to_string(),
                    source_type: stype.value(i).to_string(),
                    url: url.value(i).to_string(),
                    content: String::new(), // omitted from list payloads
                    char_count: char_count.value(i),
                    chunk_count: chunk_count.value(i),
                    created_at: created.value(i),
                    status: status.value(i).to_string(),
                    error: error.value(i).to_string(),
                });
            }
        }
        sources.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(sources)
    }

    /// Insert a source row plus all of its embedded chunks atomically-ish.
    pub async fn insert_source(
        &self,
        source: &Source,
        chunks: &[(String, i32, String)],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        // Source row.
        let schema = sources_schema();
        let batch = source_batch(&schema, std::slice::from_ref(source))?;
        self.add_batch(T_SOURCES, schema, batch).await?;
        self.add_chunks(&source.notebook_id, &source.id, chunks, embeddings).await
    }

    /// Append chunk rows (with embeddings) for a source. Creates the chunks
    /// table on first use, sizing the vector column to the embedding dimension.
    pub async fn add_chunks(
        &self,
        notebook_id: &str,
        source_id: &str,
        chunks: &[(String, i32, String)],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let dim = embeddings
            .first()
            .map(|v| v.len())
            .ok_or_else(|| anyhow!("no embeddings for chunks"))? as i32;
        self.ensure_table(T_CHUNKS, chunks_schema(dim)).await?;

        let schema = chunks_schema(dim);
        let ids: Vec<String> = chunks.iter().map(|c| c.0.clone()).collect();
        let nbs: Vec<String> = chunks.iter().map(|_| notebook_id.to_string()).collect();
        let sids: Vec<String> = chunks.iter().map(|_| source_id.to_string()).collect();
        let ords: Vec<i32> = chunks.iter().map(|c| c.1).collect();
        let texts: Vec<String> = chunks.iter().map(|c| c.2.clone()).collect();
        let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            embeddings
                .iter()
                .map(|v| Some(v.iter().map(|f| Some(*f)).collect::<Vec<_>>())),
            dim,
        );
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)),
                Arc::new(StringArray::from(nbs)),
                Arc::new(StringArray::from(sids)),
                Arc::new(Int32Array::from(ords)),
                Arc::new(StringArray::from(texts)),
                Arc::new(vectors),
            ],
        )?;
        self.add_batch(T_CHUNKS, schema, batch).await?;
        Ok(())
    }

    /// Delete every chunk owned by a source or note id.
    pub async fn delete_chunks_for(&self, owner_id: &str) -> Result<()> {
        self.delete_where(T_CHUNKS, &format!("source_id = '{}'", esc(owner_id))).await
    }

    /// Replace all chunks for an owner (source or note) in place.
    pub async fn set_chunks(
        &self,
        notebook_id: &str,
        owner_id: &str,
        chunks: &[(String, i32, String)],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        self.delete_chunks_for(owner_id).await?;
        self.add_chunks(notebook_id, owner_id, chunks, embeddings).await
    }

    /// All sources across every notebook, with full content (for re-embedding).
    pub async fn all_sources(&self) -> Result<Vec<Source>> {
        let batches = self.collect(T_SOURCES, None).await?;
        let mut sources = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let title = str_col(b, "title")?;
            let stype = str_col(b, "source_type")?;
            let url = str_col(b, "url")?;
            let content = str_col(b, "content")?;
            let cc = i64_col(b, "char_count")?;
            let ck = i64_col(b, "chunk_count")?;
            let ca = i64_col(b, "created_at")?;
            let status = str_col(b, "status")?;
            let error = str_col(b, "error")?;
            for i in 0..b.num_rows() {
                sources.push(Source {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    title: title.value(i).to_string(),
                    source_type: stype.value(i).to_string(),
                    url: url.value(i).to_string(),
                    content: content.value(i).to_string(),
                    char_count: cc.value(i),
                    chunk_count: ck.value(i),
                    created_at: ca.value(i),
                    status: status.value(i).to_string(),
                    error: error.value(i).to_string(),
                });
            }
        }
        Ok(sources)
    }

    /// Drop the entire chunk index. It is recreated (with the current embedding
    /// dimension) on the next `add_chunks`.
    pub async fn clear_all_chunks(&self) -> Result<()> {
        if self.table_exists(T_CHUNKS).await? {
            self.conn.drop_table(T_CHUNKS, &[]).await?;
        }
        Ok(())
    }

    /// Fetch the full extracted text for a single source.
    pub async fn source_content(&self, source_id: &str) -> Result<String> {
        let filter = format!("id = '{}'", esc(source_id));
        let batches = self.collect(T_SOURCES, Some(&filter)).await?;
        for b in &batches {
            let content = str_col(b, "content")?;
            if b.num_rows() > 0 {
                return Ok(content.value(0).to_string());
            }
        }
        Ok(String::new())
    }

    pub async fn delete_source(&self, source_id: &str) -> Result<()> {
        let pred = format!("source_id = '{}'", esc(source_id));
        self.delete_where(T_CHUNKS, &pred).await?;
        self.delete_where(T_SOURCES, &format!("id = '{}'", esc(source_id))).await?;
        Ok(())
    }

    /// Fetch a single source with its full content (None if not found).
    pub async fn get_source(&self, source_id: &str) -> Result<Option<Source>> {
        let filter = format!("id = '{}'", esc(source_id));
        let batches = self.collect(T_SOURCES, Some(&filter)).await?;
        for b in &batches {
            if b.num_rows() == 0 {
                continue;
            }
            return Ok(Some(Source {
                id: str_col(b, "id")?.value(0).to_string(),
                notebook_id: str_col(b, "notebook_id")?.value(0).to_string(),
                title: str_col(b, "title")?.value(0).to_string(),
                source_type: str_col(b, "source_type")?.value(0).to_string(),
                url: str_col(b, "url")?.value(0).to_string(),
                content: str_col(b, "content")?.value(0).to_string(),
                char_count: i64_col(b, "char_count")?.value(0),
                chunk_count: i64_col(b, "chunk_count")?.value(0),
                created_at: i64_col(b, "created_at")?.value(0),
                status: str_col(b, "status")?.value(0).to_string(),
                error: str_col(b, "error")?.value(0).to_string(),
            }));
        }
        Ok(None)
    }

    /// Replace a source's row and all its chunks in place (same id), used when
    /// a source is edited or refreshed and must be re-embedded.
    pub async fn replace_source(
        &self,
        source: &Source,
        chunks: &[(String, i32, String)],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        self.delete_where(T_CHUNKS, &format!("source_id = '{}'", esc(&source.id)))
            .await?;
        self.delete_where(T_SOURCES, &format!("id = '{}'", esc(&source.id)))
            .await?;
        self.insert_source(source, chunks, embeddings).await?;
        Ok(())
    }

    /// Vector-search chunks within a notebook, returning citations.
    pub async fn search_chunks(
        &self,
        notebook_id: &str,
        query_vec: Vec<f32>,
        k: usize,
    ) -> Result<Vec<Citation>> {
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(vec![]);
        }
        // Map owner_id -> title for citation labels (sources and notes both
        // live in the chunk index, keyed by their id in `source_id`).
        let mut titles: HashMap<String, String> = HashMap::new();
        for s in self.list_sources(notebook_id).await? {
            titles.insert(s.id, s.title);
        }
        for n in self.list_notes(notebook_id).await? {
            titles.insert(n.id, format!("Note: {}", n.title));
        }

        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        let batches = tbl
            .query()
            .only_if(format!("notebook_id = '{}'", esc(notebook_id)))
            .nearest_to(query_vec)?
            .limit(k)
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;

        let mut citations = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let sid = str_col(b, "source_id")?;
            let ord = i32_col(b, "ordinal")?;
            let text = str_col(b, "text")?;
            let dist = b
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::Float32Array>().cloned());
            for i in 0..b.num_rows() {
                let source_id = sid.value(i).to_string();
                citations.push(Citation {
                    chunk_id: id.value(i).to_string(),
                    source_title: titles.get(&source_id).cloned().unwrap_or_default(),
                    source_id,
                    ordinal: ord.value(i),
                    snippet: text.value(i).to_string(),
                    distance: dist.as_ref().map(|d| d.value(i)).unwrap_or(0.0),
                });
            }
        }
        citations.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
        Ok(citations)
    }

    // ---- Messages --------------------------------------------------------

    pub async fn list_messages(&self, notebook_id: &str) -> Result<Vec<Message>> {
        let filter = format!("notebook_id = '{}'", esc(notebook_id));
        let batches = self.collect(T_MESSAGES, Some(&filter)).await?;
        let mut messages = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let role = str_col(b, "role")?;
            let content = str_col(b, "content")?;
            let citations = str_col(b, "citations")?;
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                let cites: Vec<Citation> =
                    serde_json::from_str(citations.value(i)).unwrap_or_default();
                messages.push(Message {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    role: role.value(i).to_string(),
                    content: content.value(i).to_string(),
                    citations: cites,
                    created_at: created.value(i),
                });
            }
        }
        messages.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(messages)
    }

    pub async fn add_message(&self, msg: &Message) -> Result<()> {
        let schema = messages_schema();
        let citations = serde_json::to_string(&msg.citations).unwrap_or_else(|_| "[]".into());
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![msg.id.clone()])),
                Arc::new(StringArray::from(vec![msg.notebook_id.clone()])),
                Arc::new(StringArray::from(vec![msg.role.clone()])),
                Arc::new(StringArray::from(vec![msg.content.clone()])),
                Arc::new(StringArray::from(vec![citations])),
                Arc::new(Int64Array::from(vec![msg.created_at])),
            ],
        )?;
        self.add_batch(T_MESSAGES, schema, batch).await
    }

    pub async fn clear_messages(&self, notebook_id: &str) -> Result<()> {
        self.delete_where(T_MESSAGES, &format!("notebook_id = '{}'", esc(notebook_id))).await
    }

    // ---- Notes -----------------------------------------------------------

    pub async fn list_notes(&self, notebook_id: &str) -> Result<Vec<Note>> {
        let filter = format!("notebook_id = '{}'", esc(notebook_id));
        let batches = self.collect(T_NOTES, Some(&filter)).await?;
        let mut notes = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let title = str_col(b, "title")?;
            let content = str_col(b, "content")?;
            let kind = str_col(b, "kind")?;
            let created = i64_col(b, "created_at")?;
            let updated = i64_col(b, "updated_at")?;
            let prompt = str_col(b, "prompt")?;
            for i in 0..b.num_rows() {
                notes.push(Note {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    title: title.value(i).to_string(),
                    content: content.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    prompt: prompt.value(i).to_string(),
                    created_at: created.value(i),
                    updated_at: updated.value(i),
                });
            }
        }
        notes.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(notes)
    }

    /// All notes across every notebook (for re-embedding into RAG).
    pub async fn all_notes(&self) -> Result<Vec<Note>> {
        let batches = self.collect(T_NOTES, None).await?;
        let mut notes = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let title = str_col(b, "title")?;
            let content = str_col(b, "content")?;
            let kind = str_col(b, "kind")?;
            let created = i64_col(b, "created_at")?;
            let updated = i64_col(b, "updated_at")?;
            let prompt = str_col(b, "prompt")?;
            for i in 0..b.num_rows() {
                notes.push(Note {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    title: title.value(i).to_string(),
                    content: content.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    prompt: prompt.value(i).to_string(),
                    created_at: created.value(i),
                    updated_at: updated.value(i),
                });
            }
        }
        Ok(notes)
    }

    /// Fetch a single note by id (None if not found).
    pub async fn get_note(&self, id: &str) -> Result<Option<Note>> {
        let filter = format!("id = '{}'", esc(id));
        let batches = self.collect(T_NOTES, Some(&filter)).await?;
        for b in &batches {
            if b.num_rows() == 0 {
                continue;
            }
            return Ok(Some(Note {
                id: str_col(b, "id")?.value(0).to_string(),
                notebook_id: str_col(b, "notebook_id")?.value(0).to_string(),
                title: str_col(b, "title")?.value(0).to_string(),
                content: str_col(b, "content")?.value(0).to_string(),
                kind: str_col(b, "kind")?.value(0).to_string(),
                prompt: str_col(b, "prompt")?.value(0).to_string(),
                created_at: i64_col(b, "created_at")?.value(0),
                updated_at: i64_col(b, "updated_at")?.value(0),
            }));
        }
        Ok(None)
    }

    pub async fn add_note(&self, note: &Note) -> Result<()> {
        let schema = notes_schema();
        let batch = note_batch(&schema, std::slice::from_ref(note))?;
        self.add_batch(T_NOTES, schema, batch).await
    }

    pub async fn update_note(&self, id: &str, title: &str, content: &str, updated_at: i64) -> Result<()> {
        let tbl = self.conn.open_table(T_NOTES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("title", format!("'{}'", esc(title)))
            .column("content", format!("'{}'", esc(content)))
            .column("updated_at", updated_at.to_string())
            .execute()
            .await?;
        Ok(())
    }

    pub async fn delete_note(&self, id: &str) -> Result<()> {
        self.delete_where(T_NOTES, &format!("id = '{}'", esc(id))).await
    }

    // ---- Report schedules -------------------------------------------------

    async fn query_reports(&self, filter: Option<&str>) -> Result<Vec<ReportSchedule>> {
        let batches = self.collect(T_REPORTS, filter).await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let name = str_col(b, "name")?;
            let kind = str_col(b, "kind")?;
            let prompt = str_col(b, "prompt")?;
            let interval = i64_col(b, "interval_secs")?;
            let enabled = i64_col(b, "enabled")?;
            let last = i64_col(b, "last_run_at")?;
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                out.push(ReportSchedule {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    name: name.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    prompt: prompt.value(i).to_string(),
                    interval_secs: interval.value(i),
                    enabled: enabled.value(i) != 0,
                    last_run_at: last.value(i),
                    created_at: created.value(i),
                });
            }
        }
        Ok(out)
    }

    pub async fn list_report_schedules(&self, notebook_id: &str) -> Result<Vec<ReportSchedule>> {
        self.query_reports(Some(&format!("notebook_id = '{}'", esc(notebook_id)))).await
    }

    pub async fn all_report_schedules(&self) -> Result<Vec<ReportSchedule>> {
        self.query_reports(None).await
    }

    pub async fn get_report_schedule(&self, id: &str) -> Result<Option<ReportSchedule>> {
        Ok(self
            .query_reports(Some(&format!("id = '{}'", esc(id))))
            .await?
            .into_iter()
            .next())
    }

    pub async fn add_report_schedule(&self, r: &ReportSchedule) -> Result<()> {
        let schema = reports_schema();
        let batch = report_batch(&schema, r)?;
        self.add_batch(T_REPORTS, schema, batch).await
    }

    pub async fn update_report_schedule(
        &self,
        id: &str,
        name: &str,
        kind: &str,
        prompt: &str,
        interval_secs: i64,
        enabled: bool,
    ) -> Result<()> {
        let tbl = self.conn.open_table(T_REPORTS).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("name", format!("'{}'", esc(name)))
            .column("kind", format!("'{}'", esc(kind)))
            .column("prompt", format!("'{}'", esc(prompt)))
            .column("interval_secs", interval_secs.to_string())
            .column("enabled", i64::from(enabled).to_string())
            .execute()
            .await?;
        Ok(())
    }

    pub async fn set_report_last_run(&self, id: &str, ts: i64) -> Result<()> {
        let tbl = self.conn.open_table(T_REPORTS).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("last_run_at", ts.to_string())
            .execute()
            .await?;
        Ok(())
    }

    pub async fn delete_report_schedule(&self, id: &str) -> Result<()> {
        self.delete_where(T_REPORTS, &format!("id = '{}'", esc(id))).await
    }
}

// ---- Arrow column helpers ------------------------------------------------

fn str_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("missing/invalid string column `{name}`"))
}

/// Build a `sources` RecordBatch from rows (column order matches `sources_schema`).
fn source_batch(schema: &SchemaRef, sources: &[Source]) -> Result<RecordBatch> {
    let s = |f: fn(&Source) -> String| {
        Arc::new(StringArray::from(sources.iter().map(f).collect::<Vec<_>>())) as ArrayRef
    };
    let i = |f: fn(&Source) -> i64| {
        Arc::new(Int64Array::from(sources.iter().map(f).collect::<Vec<_>>())) as ArrayRef
    };
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            s(|x| x.id.clone()),
            s(|x| x.notebook_id.clone()),
            s(|x| x.title.clone()),
            s(|x| x.source_type.clone()),
            s(|x| x.url.clone()),
            s(|x| x.content.clone()),
            i(|x| x.char_count),
            i(|x| x.chunk_count),
            i(|x| x.created_at),
            s(|x| x.status.clone()),
            s(|x| x.error.clone()),
        ],
    )?)
}

/// Like `str_col` but returns None if the column is absent (used by migrations
/// that read tables predating a column).
fn opt_str_col<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
}

fn i64_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int64Array> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| anyhow!("missing/invalid i64 column `{name}`"))
}

fn i32_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int32Array> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
        .ok_or_else(|| anyhow!("missing/invalid i32 column `{name}`"))
}

/// Escape single quotes for inline SQL predicates. Ids are UUIDs, but titles
/// are user-supplied, so this matters for update/rename paths.
fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

// ---- Schemas -------------------------------------------------------------

fn notebooks_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
    ]))
}

fn sources_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("notebook_id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("source_type", DataType::Utf8, false),
        Field::new("url", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("char_count", DataType::Int64, false),
        Field::new("chunk_count", DataType::Int64, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("error", DataType::Utf8, false),
    ]))
}

fn chunks_schema(dim: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("notebook_id", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, false),
        Field::new("ordinal", DataType::Int32, false),
        Field::new("text", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), dim),
            true,
        ),
    ]))
}

fn messages_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("notebook_id", DataType::Utf8, false),
        Field::new("role", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("citations", DataType::Utf8, false),
        Field::new("created_at", DataType::Int64, false),
    ]))
}

fn notes_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("notebook_id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
        Field::new("prompt", DataType::Utf8, false),
    ]))
}

fn reports_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("notebook_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("prompt", DataType::Utf8, false),
        Field::new("interval_secs", DataType::Int64, false),
        Field::new("enabled", DataType::Int64, false),
        Field::new("last_run_at", DataType::Int64, false),
        Field::new("created_at", DataType::Int64, false),
    ]))
}

fn report_batch(schema: &SchemaRef, r: &ReportSchedule) -> Result<RecordBatch> {
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![r.id.clone()])),
            Arc::new(StringArray::from(vec![r.notebook_id.clone()])),
            Arc::new(StringArray::from(vec![r.name.clone()])),
            Arc::new(StringArray::from(vec![r.kind.clone()])),
            Arc::new(StringArray::from(vec![r.prompt.clone()])),
            Arc::new(Int64Array::from(vec![r.interval_secs])),
            Arc::new(Int64Array::from(vec![i64::from(r.enabled)])),
            Arc::new(Int64Array::from(vec![r.last_run_at])),
            Arc::new(Int64Array::from(vec![r.created_at])),
        ],
    )?)
}

fn note_batch(schema: &SchemaRef, notes: &[Note]) -> Result<RecordBatch> {
    let s = |f: fn(&Note) -> String| {
        Arc::new(StringArray::from(notes.iter().map(f).collect::<Vec<_>>())) as ArrayRef
    };
    let i = |f: fn(&Note) -> i64| {
        Arc::new(Int64Array::from(notes.iter().map(f).collect::<Vec<_>>())) as ArrayRef
    };
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            s(|x| x.id.clone()),
            s(|x| x.notebook_id.clone()),
            s(|x| x.title.clone()),
            s(|x| x.content.clone()),
            s(|x| x.kind.clone()),
            i(|x| x.created_at),
            i(|x| x.updated_at),
            s(|x| x.prompt.clone()),
        ],
    )?)
}
