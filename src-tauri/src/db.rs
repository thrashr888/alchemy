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
use lancedb::index::scalar::{FtsIndexBuilder, FullTextSearchQuery};
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::Connection;

use crate::models::{Citation, Message, Note, NoteUsage, Notebook, ReportSchedule, Source};

const T_NOTEBOOKS: &str = "notebooks";
const T_SOURCES: &str = "sources";
const T_CHUNKS: &str = "chunks";
const T_MESSAGES: &str = "messages";
const T_NOTES: &str = "notes";
const T_NOTE_USAGE: &str = "note_usage";
const T_REPORTS: &str = "report_schedules";
const T_ROUTES: &str = "routes";
/// Note chunks share the chunks table with source chunks, stored under
/// `source_id = "note:<note_id>"` — real source ids are UUIDs, so the prefix
/// can't collide, and every existing notebook/source filter and delete
/// predicate keeps working on old databases with no schema migration. The
/// prefix is decoded back into `Citation::note_id` at the read boundary;
/// nothing outside this module sees it.
pub const NOTE_CHUNK_PREFIX: &str = "note:";
pub const NOTEBOOK_PALETTE: [&str; 8] = [
    "#eb5757", "#e8a33d", "#4cb782", "#5e9bd2", "#9b87f5", "#e274b6", "#4fc1c9", "#98a562",
];

/// One hybrid search with the working shown: what each stage saw and any
/// degradation the production path hides (see `search_chunks_trace`).
/// `fused_hits` is the full RRF-ordered pool; `final_hits` the top-k slice
/// production callers get.
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchTrace {
    pub vector_hits: Vec<Citation>,
    pub fts_hits: Vec<Citation>,
    pub fused_hits: Vec<Citation>,
    pub final_hits: Vec<Citation>,
    pub warnings: Vec<String>,
}

/// One semantic-router entry (docs/RFC-retrieval-maturity.md Phase 4): a
/// notebook summary embedded so corpus-wide questions can be routed to the
/// most likely notebooks before chunk search. `kind` is "notebook" today;
/// the schema leaves room for per-source routes later.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub id: String,
    pub kind: String,
    pub notebook_id: String,
    pub summary: String,
}

/// Post-fusion shaping for corpus-wide retrieval. Zero means "no cap";
/// `SearchOptions::default()` reproduces the flat search exactly.
#[derive(Clone, Copy, Default)]
pub struct SearchOptions {
    /// Candidate pool per retrieval side = k * this (0 → 3, the flat default).
    pub pool_multiplier: usize,
    /// Max chunks kept per source or note (0 → unlimited).
    pub max_per_source: usize,
    /// Max chunks kept per notebook (0 → unlimited).
    pub max_per_notebook: usize,
    /// Max note chunks kept in total (0 → unlimited).
    pub max_notes: usize,
}

/// Walk the fused pool in score order keeping hits that fit the caps, then
/// backfill remaining slots from the skipped candidates (still in score
/// order) so caps trade duplication for breadth, never for count.
fn apply_diversity(
    ranked: Vec<(String, Citation)>,
    k: usize,
    opts: SearchOptions,
) -> Vec<(String, Citation)> {
    let uncapped = opts.max_per_source == 0 && opts.max_per_notebook == 0 && opts.max_notes == 0;
    if uncapped {
        return ranked.into_iter().take(k).collect();
    }
    let mut per_owner: HashMap<String, usize> = HashMap::new();
    let mut per_notebook: HashMap<String, usize> = HashMap::new();
    let mut notes = 0usize;
    let mut kept: Vec<(String, Citation)> = Vec::with_capacity(k);
    let mut skipped: Vec<(String, Citation)> = Vec::new();
    for hit in ranked {
        if kept.len() >= k {
            break;
        }
        let (nb, c) = &hit;
        let is_note = !c.note_id.is_empty();
        let owner = if is_note {
            format!("{NOTE_CHUNK_PREFIX}{}", c.note_id)
        } else {
            c.source_id.clone()
        };
        let owner_full = opts.max_per_source > 0
            && per_owner.get(&owner).copied().unwrap_or(0) >= opts.max_per_source;
        let nb_full = opts.max_per_notebook > 0
            && per_notebook.get(nb).copied().unwrap_or(0) >= opts.max_per_notebook;
        let notes_full = opts.max_notes > 0 && is_note && notes >= opts.max_notes;
        if owner_full || nb_full || notes_full {
            skipped.push(hit);
            continue;
        }
        *per_owner.entry(owner).or_default() += 1;
        *per_notebook.entry(nb.clone()).or_default() += 1;
        if is_note {
            notes += 1;
        }
        kept.push(hit);
    }
    for hit in skipped {
        if kept.len() >= k {
            break;
        }
        kept.push(hit);
    }
    kept
}

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
        db.migrate_notebooks().await?;
        db.ensure_table(T_SOURCES, sources_schema()).await?;
        db.migrate_sources().await?;
        db.ensure_table(T_MESSAGES, messages_schema()).await?;
        db.migrate_messages().await?;
        db.ensure_table(T_NOTES, notes_schema()).await?;
        db.migrate_notes().await?;
        db.ensure_table(T_REPORTS, reports_schema()).await?;
        Ok(db)
    }

    /// Backfill the `color` column on pre-existing `notebooks` tables.
    async fn migrate_notebooks(&self) -> Result<()> {
        if !self.table_exists(T_NOTEBOOKS).await? {
            return Ok(());
        }
        let schema = self
            .conn
            .open_table(T_NOTEBOOKS)
            .execute()
            .await?
            .schema()
            .await?;
        if schema.field_with_name("color").is_ok() {
            return Ok(());
        }

        let batches = self.collect(T_NOTEBOOKS, None).await?;
        let mut notebooks = Vec::new();
        let mut idx = 0usize;
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
                    color: NOTEBOOK_PALETTE[idx % NOTEBOOK_PALETTE.len()].to_string(),
                    source_count: 0,
                });
                idx += 1;
            }
        }

        self.conn.drop_table(T_NOTEBOOKS, &[]).await?;
        self.ensure_table(T_NOTEBOOKS, notebooks_schema()).await?;
        if !notebooks.is_empty() {
            let schema = notebooks_schema();
            let batch = notebook_batch(&schema, &notebooks)?;
            self.add_batch(T_NOTEBOOKS, schema, batch).await?;
        }
        Ok(())
    }

    /// Backfill the `kind` column ("chat") on pre-existing `messages` tables.
    async fn migrate_messages(&self) -> Result<()> {
        if !self.table_exists(T_MESSAGES).await? {
            return Ok(());
        }
        let schema = self
            .conn
            .open_table(T_MESSAGES)
            .execute()
            .await?
            .schema()
            .await?;
        if schema.field_with_name("kind").is_ok() && schema.field_with_name("model").is_ok() {
            return Ok(());
        }
        let has_kind = schema.field_with_name("kind").is_ok();
        let batches = self.collect(T_MESSAGES, None).await?;
        let mut messages = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let role = str_col(b, "role")?;
            let content = str_col(b, "content")?;
            let citations = str_col(b, "citations")?;
            let kind = has_kind.then(|| str_col(b, "kind")).transpose()?;
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                messages.push(Message {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    role: role.value(i).to_string(),
                    content: content.value(i).to_string(),
                    citations: serde_json::from_str(citations.value(i)).unwrap_or_default(),
                    kind: kind
                        .map(|k| k.value(i).to_string())
                        .unwrap_or_else(|| "chat".to_string()),
                    model: String::new(),
                    created_at: created.value(i),
                });
            }
        }
        self.conn.drop_table(T_MESSAGES, &[]).await?;
        self.ensure_table(T_MESSAGES, messages_schema()).await?;
        for msg in &messages {
            self.add_message(msg).await?;
        }
        Ok(())
    }

    /// Backfill the `prompt` column on pre-existing `notes` tables.
    async fn migrate_notes(&self) -> Result<()> {
        if !self.table_exists(T_NOTES).await? {
            return Ok(());
        }
        let schema = self
            .conn
            .open_table(T_NOTES)
            .execute()
            .await?
            .schema()
            .await?;
        let has = |n: &str| schema.field_with_name(n).is_ok();
        if has("prompt") && has("origin") && has("status") {
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
            let prompt = if has("prompt") {
                Some(str_col(b, "prompt")?)
            } else {
                None
            };
            let origin = if has("origin") {
                Some(str_col(b, "origin")?)
            } else {
                None
            };
            for i in 0..b.num_rows() {
                notes.push(Note {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    title: title.value(i).to_string(),
                    content: content.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    prompt: prompt.map(|p| p.value(i).to_string()).unwrap_or_default(),
                    // Notes from before the origin column are all deliberate.
                    origin: origin.map(|o| o.value(i).to_string()).unwrap_or_default(),
                    status: String::new(),
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
    /// `error`, `parent_id`, `mtime`) with defaults. No-op once all columns
    /// are present.
    async fn migrate_sources(&self) -> Result<()> {
        if !self.table_exists(T_SOURCES).await? {
            return Ok(());
        }
        let schema = self
            .conn
            .open_table(T_SOURCES)
            .execute()
            .await?
            .schema()
            .await?;
        let has = |n: &str| schema.field_with_name(n).is_ok();
        if has("url") && has("status") && has("error") && has("parent_id") && has("mtime") {
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
            let parent = opt_str_col(b, "parent_id");
            let mtime = opt_i64_col(b, "mtime");
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
                    parent_id: parent.map(|a| a.value(i).to_string()).unwrap_or_default(),
                    mtime: mtime.map(|a| a.value(i)).unwrap_or(0),
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
        Ok(self
            .conn
            .table_names()
            .execute()
            .await?
            .iter()
            .any(|t| t == name))
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
            let color = opt_str_col(b, "color");
            for i in 0..b.num_rows() {
                notebooks.push(Notebook {
                    id: id.value(i).to_string(),
                    title: title.value(i).to_string(),
                    created_at: created.value(i),
                    updated_at: updated.value(i),
                    color: color.map(|c| c.value(i).to_string()).unwrap_or_default(),
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
        notebooks.sort_by_key(|n| std::cmp::Reverse(n.updated_at));
        Ok(notebooks)
    }

    pub async fn create_notebook(&self, notebook: &Notebook) -> Result<()> {
        let schema = notebooks_schema();
        let batch = notebook_batch(&schema, std::slice::from_ref(notebook))?;
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

    pub async fn set_notebook_color(&self, id: &str, color: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_NOTEBOOKS).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("color", format!("'{}'", esc(color)))
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
        self.delete_where(T_NOTEBOOKS, &format!("id = '{}'", esc(id)))
            .await?;
        Ok(())
    }

    // ---- Sources & chunks ------------------------------------------------

    /// Decode source rows matching `filter`. Content is the expensive column —
    /// callers that only list skip it with `with_content = false`.
    async fn query_sources(&self, filter: Option<&str>, with_content: bool) -> Result<Vec<Source>> {
        let batches = self.collect(T_SOURCES, filter).await?;
        let mut sources = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let title = str_col(b, "title")?;
            let stype = str_col(b, "source_type")?;
            let url = str_col(b, "url")?;
            let content = with_content.then(|| str_col(b, "content")).transpose()?;
            let char_count = i64_col(b, "char_count")?;
            let chunk_count = i64_col(b, "chunk_count")?;
            let created = i64_col(b, "created_at")?;
            let status = str_col(b, "status")?;
            let error = str_col(b, "error")?;
            let parent = str_col(b, "parent_id")?;
            let mtime = i64_col(b, "mtime")?;
            for i in 0..b.num_rows() {
                sources.push(Source {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    title: title.value(i).to_string(),
                    source_type: stype.value(i).to_string(),
                    url: url.value(i).to_string(),
                    content: content.map(|c| c.value(i).to_string()).unwrap_or_default(),
                    char_count: char_count.value(i),
                    chunk_count: chunk_count.value(i),
                    created_at: created.value(i),
                    status: status.value(i).to_string(),
                    error: error.value(i).to_string(),
                    parent_id: parent.value(i).to_string(),
                    mtime: mtime.value(i),
                });
            }
        }
        Ok(sources)
    }

    pub async fn list_sources(&self, notebook_id: &str) -> Result<Vec<Source>> {
        let filter = format!("notebook_id = '{}'", esc(notebook_id));
        let mut sources = self.query_sources(Some(&filter), false).await?;
        sources.sort_by_key(|s| s.created_at);
        Ok(sources)
    }

    /// Every folder source across all notebooks (cheap — folders carry no
    /// content). Drives the periodic auto-refresh rescan.
    pub async fn all_folder_sources(&self) -> Result<Vec<Source>> {
        // Two queries, not one OR predicate: the disjunction scan missed a
        // freshly `update()`d git row that matched either arm alone —
        // sidestep the pushdown rather than debug it at notebook scale.
        let mut out = self
            .query_sources(Some("source_type = 'folder'"), false)
            .await?;
        out.extend(
            self.query_sources(Some("source_type = 'git'"), false)
                .await?,
        );
        out.extend(
            self.query_sources(Some("source_type = 'notion'"), false)
                .await?,
        );
        Ok(out)
    }

    /// Top-level ready sources that aren't folder-like parents (folders and
    /// git repos sweep via rescan) — the resync sweep filters these down to
    /// file- or git-backed ones and re-embeds any whose backing changed.
    pub async fn all_loose_sources(&self) -> Result<Vec<Source>> {
        self.query_sources(
            Some(
                "parent_id = '' AND source_type != 'folder' AND source_type != 'git' \
                 AND status = 'ready'",
            ),
            false,
        )
        .await
    }

    /// Update a source's recorded file mtime without touching its chunks.
    pub async fn set_source_mtime(&self, source_id: &str, mtime: i64) -> Result<()> {
        let tbl = self.conn.open_table(T_SOURCES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(source_id)))
            .column("mtime", mtime.to_string())
            .execute()
            .await?;
        Ok(())
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
        self.add_chunks(&source.notebook_id, &source.id, chunks, embeddings)
            .await
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

        // Keep the BM25 side of hybrid search current. Rebuilding on every
        // write is fine at personal-corpus scale (thousands of rows, ms-level).
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        tbl.create_index(&["text"], Index::FTS(FtsIndexBuilder::default()))
            .replace(true)
            .execute()
            .await
            .context("failed to build full-text index on chunks")?;
        Ok(())
    }

    /// All sources across every notebook, with full content (for re-embedding).
    pub async fn all_sources(&self) -> Result<Vec<Source>> {
        self.query_sources(None, true).await
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
        self.delete_where(T_SOURCES, &format!("id = '{}'", esc(source_id)))
            .await?;
        Ok(())
    }

    /// Fetch a single source with its full content (None if not found).
    pub async fn get_source(&self, source_id: &str) -> Result<Option<Source>> {
        let filter = format!("id = '{}'", esc(source_id));
        Ok(self
            .query_sources(Some(&filter), true)
            .await?
            .into_iter()
            .next())
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
    /// Hybrid search: vector similarity and BM25 full-text, fused with
    /// reciprocal rank fusion. Embeddings find paraphrases; BM25 finds exact
    /// identifiers (names, codes, numbers) that vectors reliably miss.
    /// `source_ids` narrows retrieval to those sources; None searches all.
    pub async fn search_chunks(
        &self,
        notebook_id: &str,
        query_vec: Vec<f32>,
        query_text: &str,
        k: usize,
        source_ids: Option<&[String]>,
    ) -> Result<Vec<Citation>> {
        Ok(self
            .search_chunks_trace(notebook_id, query_vec, query_text, k, source_ids)
            .await?
            .final_hits)
    }

    /// `search_chunks` with the working shown: per-stage hits plus warnings
    /// the production path deliberately swallows (an FTS failure degrades to
    /// vector-only silently for the UI, but debugging and evals need to see
    /// it). `final_hits` is exactly what `search_chunks` returns.
    pub async fn search_chunks_trace(
        &self,
        notebook_id: &str,
        query_vec: Vec<f32>,
        query_text: &str,
        k: usize,
        source_ids: Option<&[String]>,
    ) -> Result<SearchTrace> {
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(SearchTrace::default());
        }
        // Map stored owner id -> title for citation labels (notes keyed by
        // their prefixed form, matching what the chunk rows store).
        let mut titles: HashMap<String, String> = HashMap::new();
        for s in self.list_sources(notebook_id).await? {
            titles.insert(s.id, s.title);
        }
        for n in self.list_notes(notebook_id).await? {
            titles.insert(format!("{NOTE_CHUNK_PREFIX}{}", n.id), n.title);
        }

        let mut filter = format!("notebook_id = '{}'", esc(notebook_id));
        if let Some(ids) = source_ids {
            // Some(&[]) matches nothing — '' is never a real source id.
            let list = if ids.is_empty() {
                "''".to_string()
            } else {
                ids.iter()
                    .map(|id| format!("'{}'", esc(id)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            filter.push_str(&format!(" AND source_id IN ({list})"));
        }
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        // Fetch a wider pool from each side than we return, so fusion has
        // something to work with.
        let pool = k.max(1) * 3;

        let vec_batches = tbl
            .query()
            .only_if(filter.clone())
            .nearest_to(query_vec)?
            .limit(pool)
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        let mut vec_hits = citations_from_batches(&vec_batches, &titles)?;
        vec_hits.sort_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // BM25 side is best-effort: a database from before the FTS index
        // existed (or an exotic query string) degrades to vector-only. The
        // trace records why instead of hiding it.
        let mut warnings: Vec<String> = Vec::new();
        let fts_hits = if query_text.trim().is_empty() {
            vec![]
        } else {
            match tbl
                .query()
                .only_if(filter)
                .full_text_search(FullTextSearchQuery::new(query_text.to_string()))
                .limit(pool)
                .execute()
                .await
            {
                Ok(stream) => match stream.try_collect::<Vec<_>>().await {
                    Ok(batches) => citations_from_batches(&batches, &titles)?,
                    Err(err) => {
                        warnings.push(format!("fts collect failed: {err:#}"));
                        vec![]
                    }
                },
                Err(err) => {
                    warnings.push(format!("fts query failed: {err:#}"));
                    vec![]
                }
            }
        };

        // Reciprocal rank fusion: score = Σ 1/(60 + rank) over both lists.
        // Exact score ties are common (e.g. a vector-only and an FTS-only
        // hit at the same rank), and HashMap iteration order is randomized,
        // so break ties by chunk id to keep results stable across runs.
        let mut fused: HashMap<String, (Citation, f32)> = HashMap::new();
        for hits in [&vec_hits, &fts_hits] {
            for (rank, c) in hits.iter().enumerate() {
                fused
                    .entry(c.chunk_id.clone())
                    .or_insert((c.clone(), 0.0))
                    .1 += 1.0 / (60.0 + rank as f32);
            }
        }
        let mut merged: Vec<(Citation, f32)> = fused.into_values().collect();
        merged.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.chunk_id.cmp(&b.0.chunk_id))
        });
        let fused_hits: Vec<Citation> = merged.into_iter().map(|(c, _)| c).collect();
        let final_hits = fused_hits.iter().take(k).cloned().collect();
        Ok(SearchTrace {
            vector_hits: vec_hits,
            fts_hits,
            fused_hits,
            final_hits,
            warnings,
        })
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
            let kind = str_col(b, "kind")?;
            let model = b
                .column_by_name("model")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
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
                    kind: kind.value(i).to_string(),
                    model: model.map(|m| m.value(i).to_string()).unwrap_or_default(),
                    created_at: created.value(i),
                });
            }
        }
        messages.sort_by_key(|m| m.created_at);
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
                Arc::new(StringArray::from(vec![msg.kind.clone()])),
                Arc::new(StringArray::from(vec![msg.model.clone()])),
                Arc::new(Int64Array::from(vec![msg.created_at])),
            ],
        )?;
        self.add_batch(T_MESSAGES, schema, batch).await
    }

    pub async fn clear_messages(&self, notebook_id: &str) -> Result<()> {
        self.delete_where(T_MESSAGES, &format!("notebook_id = '{}'", esc(notebook_id)))
            .await
    }

    // ---- Notes -----------------------------------------------------------

    pub async fn list_notes(&self, notebook_id: &str) -> Result<Vec<Note>> {
        let filter = format!("notebook_id = '{}'", esc(notebook_id));
        let batches = self.collect(T_NOTES, Some(&filter)).await?;
        let mut notes = notes_from_batches(&batches)?;
        notes.sort_by_key(|n| std::cmp::Reverse(n.updated_at));
        Ok(notes)
    }

    /// The most recently updated notes across every notebook (home activity).
    pub async fn recent_notes(&self, limit: usize) -> Result<Vec<Note>> {
        let batches = self.collect(T_NOTES, None).await?;
        let mut notes = notes_from_batches(&batches)?;
        notes.sort_by_key(|n| std::cmp::Reverse(n.updated_at));
        notes.truncate(limit);
        Ok(notes)
    }

    /// The most recently updated report notes across every notebook, full
    /// content included — the home page reads them in place.
    pub async fn recent_reports(&self, limit: usize) -> Result<Vec<Note>> {
        let batches = self.collect(T_NOTES, Some("kind = 'report'")).await?;
        let mut notes = notes_from_batches(&batches)?;
        notes.sort_by_key(|n| std::cmp::Reverse(n.updated_at));
        notes.truncate(limit);
        Ok(notes)
    }

    /// (id, notebook_id, title) for every source — lightweight lookups without
    /// dragging full content across.
    pub async fn all_source_meta(&self) -> Result<Vec<(String, String, String)>> {
        let batches = self.collect(T_SOURCES, None).await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let title = str_col(b, "title")?;
            for i in 0..b.num_rows() {
                out.push((
                    id.value(i).to_string(),
                    nb.value(i).to_string(),
                    title.value(i).to_string(),
                ));
            }
        }
        Ok(out)
    }

    /// Aggregate (source count, total chars) across every notebook.
    pub async fn corpus_stats(&self) -> Result<(i64, i64)> {
        let batches = self.collect(T_SOURCES, None).await?;
        let (mut count, mut chars) = (0i64, 0i64);
        for b in &batches {
            let cc = i64_col(b, "char_count")?;
            for i in 0..b.num_rows() {
                count += 1;
                chars += cc.value(i);
            }
        }
        Ok((count, chars))
    }

    /// BM25-only search across every notebook — no embedding round-trip, so
    /// it's fast enough for as-you-type global search. Returns
    /// Corpus-wide hybrid search — `search_chunks` without the notebook
    /// filter; `SearchOptions::default()` and no routing give the flat
    /// baseline. Returns (notebook_id, citation), rank-fused across the
    /// vector and BM25 sides exactly like the per-notebook path.
    ///
    /// `route_notebooks` is a relevance hint, not a boundary: it narrows the
    /// VECTOR side to the routed notebooks while BM25 stays corpus-wide, so
    /// an exact identifier the router couldn't see (titles carry no error
    /// codes) still escapes a routing mistake. Diversity caps stop one
    /// chatty source or notebook from filling the whole top-k with
    /// near-duplicates; skipped candidates backfill in score order, so this
    /// never returns fewer hits than the uncapped search would.
    pub async fn search_chunks_all_opts(
        &self,
        query_vec: Vec<f32>,
        query_text: &str,
        k: usize,
        route_notebooks: Option<&[String]>,
        opts: SearchOptions,
    ) -> Result<Vec<(String, Citation)>> {
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(vec![]);
        }
        let mut titles: HashMap<String, String> = self
            .all_source_meta()
            .await?
            .into_iter()
            .map(|(id, _nb, title)| (id, title))
            .collect();
        for n in self.recent_notes(usize::MAX).await? {
            titles.insert(format!("{NOTE_CHUNK_PREFIX}{}", n.id), n.title);
        }
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        let pool = k.max(1) * opts.pool_multiplier.max(3);

        let nb_filter = route_notebooks.map(|ids| {
            // Some(&[]) matches nothing — '' is never a real notebook id.
            let list = if ids.is_empty() {
                "''".to_string()
            } else {
                ids.iter()
                    .map(|id| format!("'{}'", esc(id)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!("notebook_id IN ({list})")
        });

        let mut vec_query = tbl.query();
        if let Some(f) = &nb_filter {
            vec_query = vec_query.only_if(f.clone());
        }
        let vec_batches = vec_query
            .nearest_to(query_vec)?
            .limit(pool)
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        let mut vec_hits = nb_citations_from_batches(&vec_batches, &titles)?;
        vec_hits.sort_by(|a, b| {
            a.1.distance
                .partial_cmp(&b.1.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Deliberately unrouted: BM25 stays corpus-wide so exact identifiers
        // survive a bad route (see the method docs).
        let fts_hits = if query_text.trim().is_empty() {
            vec![]
        } else {
            match tbl
                .query()
                .full_text_search(FullTextSearchQuery::new(query_text.to_string()))
                .limit(pool)
                .execute()
                .await
            {
                Ok(stream) => match stream.try_collect::<Vec<_>>().await {
                    Ok(batches) => nb_citations_from_batches(&batches, &titles)?,
                    Err(_) => vec![],
                },
                Err(_) => vec![],
            }
        };

        // Same tie-break-by-chunk-id as search_chunks: RRF score ties are
        // common and HashMap order is randomized.
        let mut fused: HashMap<String, ((String, Citation), f32)> = HashMap::new();
        for hits in [vec_hits, fts_hits] {
            for (rank, hit) in hits.into_iter().enumerate() {
                fused.entry(hit.1.chunk_id.clone()).or_insert((hit, 0.0)).1 +=
                    1.0 / (60.0 + rank as f32);
            }
        }
        let mut merged: Vec<((String, Citation), f32)> = fused.into_values().collect();
        merged.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0 .1.chunk_id.cmp(&b.0 .1.chunk_id))
        });
        let ranked: Vec<(String, Citation)> = merged.into_iter().map(|(hit, _)| hit).collect();
        Ok(apply_diversity(ranked, k, opts))
    }

    pub async fn search_chunks_fts_all(
        &self,
        query_text: &str,
        k: usize,
    ) -> Result<Vec<(String, Citation)>> {
        if query_text.trim().is_empty() || !self.table_exists(T_CHUNKS).await? {
            return Ok(vec![]);
        }
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        let batches = match tbl
            .query()
            .full_text_search(FullTextSearchQuery::new(query_text.to_string()))
            .limit(k)
            .execute()
            .await
        {
            Ok(stream) => stream.try_collect::<Vec<_>>().await.unwrap_or_default(),
            Err(_) => return Ok(vec![]),
        };
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let sid = str_col(b, "source_id")?;
            let ord = i32_col(b, "ordinal")?;
            let text = str_col(b, "text")?;
            for i in 0..b.num_rows() {
                let (source_id, note_id) = split_owner(sid.value(i));
                out.push((
                    nb.value(i).to_string(),
                    Citation {
                        chunk_id: id.value(i).to_string(),
                        source_id,
                        source_title: String::new(),
                        note_id,
                        ordinal: ord.value(i),
                        snippet: text.value(i).to_string(),
                        distance: 0.0,
                    },
                ));
            }
        }
        Ok(out)
    }

    // ---- Semantic router ---------------------------------------------------

    /// All stored router entries (without vectors) — the staleness baseline
    /// for `router::ensure_router`'s diff.
    pub async fn list_routes(&self) -> Result<Vec<Route>> {
        let batches = self.collect(T_ROUTES, None).await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let kind = str_col(b, "kind")?;
            let nb = str_col(b, "notebook_id")?;
            let summary = str_col(b, "summary")?;
            for i in 0..b.num_rows() {
                out.push(Route {
                    id: id.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    summary: summary.value(i).to_string(),
                });
            }
        }
        Ok(out)
    }

    /// Insert-or-replace router entries (embeddings parallel to `routes`).
    /// Creates the routes table on first use with the embedding dimension.
    pub async fn upsert_routes(&self, routes: &[Route], embeddings: &[Vec<f32>]) -> Result<()> {
        if routes.is_empty() {
            return Ok(());
        }
        let dim = embeddings
            .first()
            .map(|v| v.len())
            .ok_or_else(|| anyhow!("no embeddings for routes"))? as i32;
        self.ensure_table(T_ROUTES, routes_schema(dim)).await?;
        self.delete_routes(&routes.iter().map(|r| r.id.clone()).collect::<Vec<_>>())
            .await?;
        let schema = routes_schema(dim);
        let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            embeddings
                .iter()
                .map(|v| Some(v.iter().map(|f| Some(*f)).collect::<Vec<_>>())),
            dim,
        );
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(
                    routes.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    routes.iter().map(|r| r.kind.clone()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    routes
                        .iter()
                        .map(|r| r.notebook_id.clone())
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    routes.iter().map(|r| r.summary.clone()).collect::<Vec<_>>(),
                )),
                Arc::new(vectors),
            ],
        )?;
        self.add_batch(T_ROUTES, schema, batch).await
    }

    pub async fn delete_routes(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let list = ids
            .iter()
            .map(|id| format!("'{}'", esc(id)))
            .collect::<Vec<_>>()
            .join(", ");
        self.delete_where(T_ROUTES, &format!("id IN ({list})"))
            .await
    }

    /// Nearest router entries to the query, best first, with the vector
    /// distance (lower = closer). `kind` filters to one entry kind.
    pub async fn route_search(
        &self,
        query_vec: Vec<f32>,
        kind: Option<&str>,
        k: usize,
    ) -> Result<Vec<(Route, f32)>> {
        if !self.table_exists(T_ROUTES).await? {
            return Ok(vec![]);
        }
        let tbl = self.conn.open_table(T_ROUTES).execute().await?;
        let mut q = tbl.query();
        if let Some(kind) = kind {
            q = q.only_if(format!("kind = '{}'", esc(kind)));
        }
        let batches = q
            .nearest_to(query_vec)?
            .limit(k.max(1))
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let rkind = str_col(b, "kind")?;
            let nb = str_col(b, "notebook_id")?;
            let summary = str_col(b, "summary")?;
            let dist = b.column_by_name("_distance").and_then(|c| {
                c.as_any()
                    .downcast_ref::<arrow_array::Float32Array>()
                    .cloned()
            });
            for i in 0..b.num_rows() {
                out.push((
                    Route {
                        id: id.value(i).to_string(),
                        kind: rkind.value(i).to_string(),
                        notebook_id: nb.value(i).to_string(),
                        summary: summary.value(i).to_string(),
                    },
                    dist.as_ref().map(|d| d.value(i)).unwrap_or(0.0),
                ));
            }
        }
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(out)
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
                origin: str_col(b, "origin")?.value(0).to_string(),
                status: str_col(b, "status")?.value(0).to_string(),
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

    pub async fn update_note(
        &self,
        id: &str,
        title: &str,
        content: &str,
        updated_at: i64,
    ) -> Result<()> {
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

    /// Set a note's origin. Used to flip "auto" → "" when a human or agent
    /// deliberately edits an auto-created note: ownership is the pin — the
    /// curator never touches owned notes (docs/RFC-note-curator.md).
    pub async fn set_note_origin(&self, id: &str, origin: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_NOTES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("origin", format!("'{}'", esc(origin)))
            .execute()
            .await?;
        Ok(())
    }

    /// Set a note's curator status: "" (active) | "stale" | "archived".
    pub async fn set_note_status(&self, id: &str, status: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_NOTES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("status", format!("'{}'", esc(status)))
            .execute()
            .await?;
        Ok(())
    }

    /// Remove one chat message (retry flow: the failed answer and its
    /// question are deleted before the resend).
    pub async fn delete_message(&self, id: &str) -> Result<()> {
        self.delete_where(T_MESSAGES, &format!("id = '{}'", esc(id)))
            .await
    }

    pub async fn delete_note(&self, id: &str) -> Result<()> {
        self.delete_note_chunks(id).await?;
        self.delete_where(T_NOTE_USAGE, &format!("note_id = '{}'", esc(id)))
            .await?;
        self.delete_where(T_NOTES, &format!("id = '{}'", esc(id)))
            .await
    }

    /// Drop a note's chunks from the retrieval index (no-op if unindexed).
    pub async fn delete_note_chunks(&self, note_id: &str) -> Result<()> {
        let pred = format!("source_id = '{NOTE_CHUNK_PREFIX}{}'", esc(note_id));
        self.delete_where(T_CHUNKS, &pred).await
    }

    /// Bump a usage counter for the given notes (deduped — one answer citing
    /// three passages of a note counts once) and stamp `last_used_at`.
    /// `field` is one of "reads" | "retrieval_hits" | "cited". This is the
    /// curator's ground truth (docs/RFC-note-curator.md, phase 2): staleness
    /// decisions come from these counters, not vibes.
    pub async fn bump_note_usage(&self, note_ids: &[String], field: &str, ts: i64) -> Result<()> {
        if !matches!(field, "reads" | "retrieval_hits" | "cited") {
            return Err(anyhow!("unknown note usage field {field}"));
        }
        let ids: std::collections::HashSet<&String> = note_ids.iter().collect();
        if ids.is_empty() {
            return Ok(());
        }
        self.ensure_table(T_NOTE_USAGE, note_usage_schema()).await?;
        let tbl = self.conn.open_table(T_NOTE_USAGE).execute().await?;
        for id in ids {
            let filter = format!("note_id = '{}'", esc(id));
            let existing = self.collect(T_NOTE_USAGE, Some(&filter)).await?;
            if existing.iter().any(|b| b.num_rows() > 0) {
                tbl.update()
                    .only_if(filter)
                    .column(field, format!("{field} + 1"))
                    .column("last_used_at", ts.to_string())
                    .execute()
                    .await?;
            } else {
                let usage = NoteUsage {
                    note_id: id.clone(),
                    reads: (field == "reads") as i64,
                    retrieval_hits: (field == "retrieval_hits") as i64,
                    cited: (field == "cited") as i64,
                    last_used_at: ts,
                };
                let schema = note_usage_schema();
                let batch = note_usage_batch(&schema, std::slice::from_ref(&usage))?;
                self.add_batch(T_NOTE_USAGE, schema, batch).await?;
            }
        }
        Ok(())
    }

    /// Every note's usage counters (notes never used have no row).
    pub async fn note_usage(&self) -> Result<Vec<NoteUsage>> {
        if !self.table_exists(T_NOTE_USAGE).await? {
            return Ok(vec![]);
        }
        let batches = self.collect(T_NOTE_USAGE, None).await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "note_id")?;
            let reads = i64_col(b, "reads")?;
            let hits = i64_col(b, "retrieval_hits")?;
            let cited = i64_col(b, "cited")?;
            let used = i64_col(b, "last_used_at")?;
            for i in 0..b.num_rows() {
                out.push(NoteUsage {
                    note_id: id.value(i).to_string(),
                    reads: reads.value(i),
                    retrieval_hits: hits.value(i),
                    cited: cited.value(i),
                    last_used_at: used.value(i),
                });
            }
        }
        Ok(out)
    }

    /// Note ids that currently have chunks in the retrieval index. Used by
    /// the startup backfill to find notes written before notes were indexed.
    pub async fn indexed_note_ids(&self) -> Result<std::collections::HashSet<String>> {
        let mut out = std::collections::HashSet::new();
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(out);
        }
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        let batches = tbl
            .query()
            .only_if(format!("source_id LIKE '{NOTE_CHUNK_PREFIX}%'"))
            .select(lancedb::query::Select::columns(&["source_id"]))
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        for b in &batches {
            let sid = str_col(b, "source_id")?;
            for i in 0..b.num_rows() {
                if let Some(id) = sid.value(i).strip_prefix(NOTE_CHUNK_PREFIX) {
                    out.insert(id.to_string());
                }
            }
        }
        Ok(out)
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
        self.query_reports(Some(&format!("notebook_id = '{}'", esc(notebook_id))))
            .await
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
        self.delete_where(T_REPORTS, &format!("id = '{}'", esc(id)))
            .await
    }
}

// ---- Arrow column helpers ------------------------------------------------

/// Decode note-table batches into Note rows.
fn notes_from_batches(batches: &[RecordBatch]) -> Result<Vec<Note>> {
    let mut notes = Vec::new();
    for b in batches {
        let id = str_col(b, "id")?;
        let nb = str_col(b, "notebook_id")?;
        let title = str_col(b, "title")?;
        let content = str_col(b, "content")?;
        let kind = str_col(b, "kind")?;
        let created = i64_col(b, "created_at")?;
        let updated = i64_col(b, "updated_at")?;
        let prompt = str_col(b, "prompt")?;
        let origin = str_col(b, "origin")?;
        let status = str_col(b, "status")?;
        for i in 0..b.num_rows() {
            notes.push(Note {
                id: id.value(i).to_string(),
                notebook_id: nb.value(i).to_string(),
                title: title.value(i).to_string(),
                content: content.value(i).to_string(),
                kind: kind.value(i).to_string(),
                prompt: prompt.value(i).to_string(),
                origin: origin.value(i).to_string(),
                status: status.value(i).to_string(),
                created_at: created.value(i),
                updated_at: updated.value(i),
            });
        }
    }
    Ok(notes)
}

/// Decode chunk-query result batches into citations. `_distance` is present
/// on vector results only; FTS hits leave it at 0.0.
/// Like `citations_from_batches`, but keeps each row's notebook_id — the
/// corpus-wide searches need to say where a passage lives.
fn nb_citations_from_batches(
    batches: &[RecordBatch],
    titles: &HashMap<String, String>,
) -> Result<Vec<(String, Citation)>> {
    let mut out = Vec::new();
    for b in batches {
        let nb = str_col(b, "notebook_id")?;
        let citations = citations_from_batches(std::slice::from_ref(b), titles)?;
        for (i, c) in citations.into_iter().enumerate() {
            out.push((nb.value(i).to_string(), c));
        }
    }
    Ok(out)
}

/// Decode a stored chunk owner id into (source_id, note_id) — exactly one
/// is non-empty.
fn split_owner(stored: &str) -> (String, String) {
    match stored.strip_prefix(NOTE_CHUNK_PREFIX) {
        Some(note_id) => (String::new(), note_id.to_string()),
        None => (stored.to_string(), String::new()),
    }
}

fn citations_from_batches(
    batches: &[RecordBatch],
    titles: &HashMap<String, String>,
) -> Result<Vec<Citation>> {
    let mut citations = Vec::new();
    for b in batches {
        let id = str_col(b, "id")?;
        let sid = str_col(b, "source_id")?;
        let ord = i32_col(b, "ordinal")?;
        let text = str_col(b, "text")?;
        let dist = b.column_by_name("_distance").and_then(|c| {
            c.as_any()
                .downcast_ref::<arrow_array::Float32Array>()
                .cloned()
        });
        for i in 0..b.num_rows() {
            let stored = sid.value(i).to_string();
            let (source_id, note_id) = split_owner(&stored);
            citations.push(Citation {
                chunk_id: id.value(i).to_string(),
                source_title: titles.get(&stored).cloned().unwrap_or_default(),
                source_id,
                note_id,
                ordinal: ord.value(i),
                snippet: text.value(i).to_string(),
                distance: dist.as_ref().map(|d| d.value(i)).unwrap_or(0.0),
            });
        }
    }
    Ok(citations)
}

fn notebook_batch(schema: &SchemaRef, notebooks: &[Notebook]) -> Result<RecordBatch> {
    let s = |f: fn(&Notebook) -> String| {
        Arc::new(StringArray::from(
            notebooks.iter().map(f).collect::<Vec<_>>(),
        )) as ArrayRef
    };
    let i = |f: fn(&Notebook) -> i64| {
        Arc::new(Int64Array::from(
            notebooks.iter().map(f).collect::<Vec<_>>(),
        )) as ArrayRef
    };
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            s(|x| x.id.clone()),
            s(|x| x.title.clone()),
            i(|x| x.created_at),
            i(|x| x.updated_at),
            s(|x| x.color.clone()),
        ],
    )?)
}

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
            s(|x| x.parent_id.clone()),
            i(|x| x.mtime),
        ],
    )?)
}

/// Like `str_col` but returns None if the column is absent (used by migrations
/// that read tables predating a column).
fn opt_str_col<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
}

/// Like `i64_col` but returns None if the column is absent (migrations).
fn opt_i64_col<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Int64Array> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
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
        Field::new("color", DataType::Utf8, false),
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
        Field::new("parent_id", DataType::Utf8, false),
        Field::new("mtime", DataType::Int64, false),
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

fn routes_schema(dim: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("notebook_id", DataType::Utf8, false),
        Field::new("summary", DataType::Utf8, false),
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
        Field::new("kind", DataType::Utf8, false),
        Field::new("model", DataType::Utf8, false),
        Field::new("created_at", DataType::Int64, false),
    ]))
}

fn note_usage_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("note_id", DataType::Utf8, false),
        Field::new("reads", DataType::Int64, false),
        Field::new("retrieval_hits", DataType::Int64, false),
        Field::new("cited", DataType::Int64, false),
        Field::new("last_used_at", DataType::Int64, false),
    ]))
}

fn note_usage_batch(schema: &SchemaRef, rows: &[NoteUsage]) -> Result<RecordBatch> {
    let i = |f: fn(&NoteUsage) -> i64| {
        Arc::new(Int64Array::from(rows.iter().map(f).collect::<Vec<_>>())) as ArrayRef
    };
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(
                rows.iter().map(|x| x.note_id.clone()).collect::<Vec<_>>(),
            )) as ArrayRef,
            i(|x| x.reads),
            i(|x| x.retrieval_hits),
            i(|x| x.cited),
            i(|x| x.last_used_at),
        ],
    )?)
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
        Field::new("origin", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
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
            s(|x| x.origin.clone()),
            s(|x| x.status.clone()),
        ],
    )?)
}
