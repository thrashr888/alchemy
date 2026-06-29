//! LanceDB persistence layer. Everything lives in one embedded Lance database:
//! notebooks, sources, chunks (with vectors), messages, and notes — each its own
//! Lance table. We filter by `notebook_id` instead of joining.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use arrow_array::types::Float32Type;
use arrow_array::{
    Array, FixedSizeListArray, Int32Array, Int64Array, RecordBatch, RecordBatchIterator,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::Connection;

use crate::models::{Citation, Message, Note, Notebook, Source};

const T_NOTEBOOKS: &str = "notebooks";
const T_SOURCES: &str = "sources";
const T_CHUNKS: &str = "chunks";
const T_MESSAGES: &str = "messages";
const T_NOTES: &str = "notes";

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
        db.migrate_sources_url().await?;
        db.ensure_table(T_MESSAGES, messages_schema()).await?;
        db.ensure_table(T_NOTES, notes_schema()).await?;
        Ok(db)
    }

    /// Add the `url` column to pre-existing `sources` tables by rebuilding them.
    /// No-op once the column is present. Keeps existing data.
    async fn migrate_sources_url(&self) -> Result<()> {
        if !self.table_exists(T_SOURCES).await? {
            return Ok(());
        }
        let tbl = self.conn.open_table(T_SOURCES).execute().await?;
        if tbl.schema().await?.field_with_name("url").is_ok() {
            return Ok(());
        }

        // Read existing rows under the old (url-less) schema.
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
            for i in 0..b.num_rows() {
                sources.push(Source {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    title: title.value(i).to_string(),
                    source_type: stype.value(i).to_string(),
                    url: String::new(),
                    content: content.value(i).to_string(),
                    char_count: cc.value(i),
                    chunk_count: ck.value(i),
                    created_at: ca.value(i),
                });
            }
        }

        self.conn.drop_table(T_SOURCES, &[]).await?;
        self.ensure_table(T_SOURCES, sources_schema()).await?;
        if !sources.is_empty() {
            let schema = sources_schema();
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(StringArray::from(sources.iter().map(|s| s.id.clone()).collect::<Vec<_>>())),
                    Arc::new(StringArray::from(sources.iter().map(|s| s.notebook_id.clone()).collect::<Vec<_>>())),
                    Arc::new(StringArray::from(sources.iter().map(|s| s.title.clone()).collect::<Vec<_>>())),
                    Arc::new(StringArray::from(sources.iter().map(|s| s.source_type.clone()).collect::<Vec<_>>())),
                    Arc::new(StringArray::from(sources.iter().map(|s| s.url.clone()).collect::<Vec<_>>())),
                    Arc::new(StringArray::from(sources.iter().map(|s| s.content.clone()).collect::<Vec<_>>())),
                    Arc::new(Int64Array::from(sources.iter().map(|s| s.char_count).collect::<Vec<_>>())),
                    Arc::new(Int64Array::from(sources.iter().map(|s| s.chunk_count).collect::<Vec<_>>())),
                    Arc::new(Int64Array::from(sources.iter().map(|s| s.created_at).collect::<Vec<_>>())),
                ],
            )?;
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
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![source.id.clone()])),
                Arc::new(StringArray::from(vec![source.notebook_id.clone()])),
                Arc::new(StringArray::from(vec![source.title.clone()])),
                Arc::new(StringArray::from(vec![source.source_type.clone()])),
                Arc::new(StringArray::from(vec![source.url.clone()])),
                Arc::new(StringArray::from(vec![source.content.clone()])),
                Arc::new(Int64Array::from(vec![source.char_count])),
                Arc::new(Int64Array::from(vec![source.chunk_count])),
                Arc::new(Int64Array::from(vec![source.created_at])),
            ],
        )?;
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
        // Map source_id -> title for citation labels.
        let mut titles: HashMap<String, String> = HashMap::new();
        for s in self.list_sources(notebook_id).await? {
            titles.insert(s.id, s.title);
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
            for i in 0..b.num_rows() {
                notes.push(Note {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    title: title.value(i).to_string(),
                    content: content.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    created_at: created.value(i),
                    updated_at: updated.value(i),
                });
            }
        }
        notes.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(notes)
    }

    pub async fn add_note(&self, note: &Note) -> Result<()> {
        let schema = notes_schema();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![note.id.clone()])),
                Arc::new(StringArray::from(vec![note.notebook_id.clone()])),
                Arc::new(StringArray::from(vec![note.title.clone()])),
                Arc::new(StringArray::from(vec![note.content.clone()])),
                Arc::new(StringArray::from(vec![note.kind.clone()])),
                Arc::new(Int64Array::from(vec![note.created_at])),
                Arc::new(Int64Array::from(vec![note.updated_at])),
            ],
        )?;
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
}

// ---- Arrow column helpers ------------------------------------------------

fn str_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("missing/invalid string column `{name}`"))
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
    ]))
}
