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

use crate::models::{
    Citation, LedgerEntry, Message, MetaThread, MetaTurn, Note, NoteUsage, Notebook, RegistryCard,
    ReportSchedule, RunReceipt, Source, SourceEvent,
};

const T_NOTEBOOKS: &str = "notebooks";
const T_SOURCES: &str = "sources";
const T_CHUNKS: &str = "chunks";
const T_MESSAGES: &str = "messages";
const T_NOTES: &str = "notes";
const T_NOTE_USAGE: &str = "note_usage";
const T_REPORTS: &str = "report_schedules";
const T_ROUTES: &str = "routes";
const T_SOURCE_EVENTS: &str = "source_events";
const T_RECEIPTS: &str = "run_receipts";
/// Small app state that used to live in JSON sidecars and localStorage:
/// feed poll state, discovered feeds, robots.txt caches, the Arrivals
/// watermark. One row per key, JSON or a scalar in `value`. The database
/// is single-tenant by design; a second store beside it is a second
/// source of truth.
const T_KV: &str = "app_state";
const T_LEDGER: &str = "ledger";
/// The Registry's cast (docs/RFC-registry.md). Corpus-scoped: no
/// notebook_id column, unlike every other entity table here.
const T_REGISTRY: &str = "registry";
/// Home's corpus-wide conversations (docs/RFC-meta-chat.md). Also
/// corpus-scoped: a turn belongs to a thread, and the thread is about
/// everything. Threads are derived from these rows, not stored.
const T_META_TURNS: &str = "meta_turns";
/// The `kind` of the one row in a Home thread that is not a turn: the name
/// the small model gave the conversation. It rides `meta_turns` so a thread
/// stays exactly what it always was — the rows that share a `thread_id` —
/// and is filtered out of every read that means "the conversation".
pub const META_TITLE_KIND: &str = "title";
/// Source events prune past this window — a rolling record, not an archive.
const SOURCE_EVENT_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Receipts are a recent record, not an archive (docs/RFC-night-shift-area.md
/// §2) — the durable artifacts are the notes the runs wrote.
const RECEIPT_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// Note chunks share the chunks table with source chunks, stored under
/// `source_id = "note:<note_id>"` — real source ids are UUIDs, so the prefix
/// can't collide, and every existing notebook/source filter and delete
/// predicate keeps working on old databases with no schema migration. The
/// prefix is decoded back into `Citation::note_id` at the read boundary;
/// nothing outside this module sees it.
pub const NOTE_CHUNK_PREFIX: &str = "note:";

/// `source_id = "gist:<source_id>"` marks a source-gist row
/// (docs/RFC-infinite-context.md Phase 1): one distilled overview per
/// source, stored in the chunks table so it rides the same vector + FTS
/// index. Its `ordinal` column carries the i32 content hash of the source
/// text it was distilled from — the staleness signal for the gist sweep —
/// not a position.
pub const GIST_CHUNK_PREFIX: &str = "gist:";

/// `source_id = "section:<source_id>"` marks a section-summary row
/// (docs/RFC-outline-index.md Phase 2): one short summary per top-level
/// section of a long structured source, embedded and BM25-indexed like a
/// gist. Its chunk id is `section:<source_id>:<start>-<end>`, the ordinal
/// span of the passages it stands for; a hit on the row is swapped for
/// those passages before results leave `search_chunks_trace`.
pub const SECTION_CHUNK_PREFIX: &str = "section:";

/// One section summary to store (see `add_section_rows`).
#[derive(Clone, Debug)]
pub struct SectionGist {
    /// First and last chunk ordinal of the section, inclusive.
    pub start: i32,
    pub end: i32,
    /// "title › heading" — the chain, stored as the row's context.
    pub chain: String,
    pub summary: String,
}

/// One line of a notebook's outline (see `notebook_outline`).
#[derive(Clone, Debug)]
pub struct OutlineEntry {
    pub source_id: String,
    /// "title › heading" — the context column of the section row.
    pub chain: String,
    pub summary: String,
    pub start: i32,
    pub end: i32,
}

/// The passage span a section row stands for, from its chunk id.
pub fn section_range(chunk_id: &str) -> Option<(i32, i32)> {
    let rest = chunk_id.strip_prefix(SECTION_CHUNK_PREFIX)?;
    let (_, span) = rest.rsplit_once(':')?;
    let (a, b) = span.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// How many passages replace one section hit. The summary says "the answer
/// is in this section"; a mini hybrid search over that span says where.
/// Two, not the whole section: dumping a section in document order pushed
/// the wanted passage down and let wrong-section hits crowd the top ten
/// (topic MRR 0.50 → 0.44 measured that way).
const SECTION_EXPAND_MAX: usize = 2;

/// `SECTION_EXPAND_MAX`, overridable for eval sweeps.
fn section_expand_max() -> usize {
    std::env::var("ALCHEMY_SECTION_EXPAND")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(SECTION_EXPAND_MAX)
}

/// `source_id = "snote:<source_id>"` marks the user's own annotation on a
/// source (docs/RFC-source-tags.md): one editable note per source, indexed
/// in the chunks table so "why did I save this" is retrievable. Unlike
/// gists these rows are user ground truth, so they stay IN the per-notebook
/// search path (no exclusion filter) and need no confabulation gate.
pub const SNOTE_CHUNK_PREFIX: &str = "snote:";
pub const NOTEBOOK_PALETTE: [&str; 11] = [
    "#eb5757", "#e8a33d", "#d9b13b", "#4cb782", "#4fc1c9", "#5e9bd2", "#9b87f5", "#e274b6",
    "#c1765a", "#98a562", "#8a93a6",
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
    /// Max gist rows kept in total (0 → unlimited). Gists also count toward
    /// their source's `max_per_source` budget — a gist is evidence about the
    /// source, not a bonus slot for it.
    pub max_gists: usize,
}

/// Walk the fused pool in score order keeping hits that fit the caps, then
/// backfill remaining slots from the skipped candidates (still in score
/// order) so caps trade duplication for breadth, never for count.
fn apply_diversity(
    ranked: Vec<(String, Citation)>,
    k: usize,
    opts: SearchOptions,
) -> Vec<(String, Citation)> {
    let uncapped = opts.max_per_source == 0
        && opts.max_per_notebook == 0
        && opts.max_notes == 0
        && opts.max_gists == 0;
    if uncapped {
        return ranked.into_iter().take(k).collect();
    }
    let mut per_owner: HashMap<String, usize> = HashMap::new();
    let mut per_notebook: HashMap<String, usize> = HashMap::new();
    let mut notes = 0usize;
    let mut gists = 0usize;
    let mut kept: Vec<(String, Citation)> = Vec::with_capacity(k);
    let mut skipped: Vec<(String, Citation)> = Vec::new();
    for hit in ranked {
        if kept.len() >= k {
            break;
        }
        let (nb, c) = &hit;
        let is_note = !c.note_id.is_empty();
        // Annotation rows (snote) reach here with source_id resolved, so
        // they share their source's per-owner budget — the annotation is
        // evidence about the source, not a bonus slot for it. They are not
        // gists, so `max_gists` never suppresses them.
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
        let gists_full = opts.max_gists > 0 && c.gist && gists >= opts.max_gists;
        if owner_full || nb_full || notes_full || gists_full {
            skipped.push(hit);
            continue;
        }
        *per_owner.entry(owner).or_default() += 1;
        *per_notebook.entry(nb.clone()).or_default() += 1;
        if is_note {
            notes += 1;
        }
        if c.gist {
            gists += 1;
        }
        kept.push(hit);
    }
    // Backfill keeps the count guarantee (shaped search never returns fewer
    // hits than flat), but gists rejoin last: a skipped chunk is a lost
    // near-duplicate, while a skipped gist is redundant by construction —
    // its source's verbatim chunks are in the pool. Without this two-tier
    // order, a gist-heavy pool walks straight past `max_gists` on backfill.
    let (skipped_gists, skipped_rest): (Vec<_>, Vec<_>) =
        skipped.into_iter().partition(|(_, c)| c.gist);
    for hit in skipped_rest.into_iter().chain(skipped_gists) {
        if kept.len() >= k {
            break;
        }
        kept.push(hit);
    }
    kept
}

pub struct Db {
    conn: Connection,
    /// Serializes chunk BM25-index rebuilds. `add_chunks` rebuilds the whole
    /// full-text index on every write; the background gist/enrichment sweep
    /// (RFC-infinite-context) made those writes concurrent with foreground
    /// imports, and two overlapping Lance `CreateIndex` transactions preempt
    /// each other (a retryable commit conflict). One rebuild at a time — plus
    /// the bounded retry in `rebuild_chunks_fts` — removes the race.
    fts_lock: tokio::sync::Mutex<()>,
    /// Bulk-write mode: rebuilding the full BM25 index after EVERY insert is
    /// O(n²) across a folder import or eval seeding (a 48-file folder paid 48
    /// full rebuilds; the 10M-char eval corpus ran 40+ minutes on a rebuild
    /// pattern whose search takes milliseconds). While deferred, `add_chunks`
    /// only marks the index dirty; `flush_fts` rebuilds once at the end.
    fts_deferred: std::sync::atomic::AtomicBool,
    fts_dirty: std::sync::atomic::AtomicBool,
    /// The chunks table has been seen with its context column (Phase 1.5);
    /// see `has_chunk_context`.
    chunk_context_seen: std::sync::atomic::AtomicBool,
    /// Nudged on every non-deferred chunk write; the debounced flusher task
    /// (lib.rs) listens and rebuilds once per burst.
    fts_notify: tokio::sync::Notify,
    /// The vector leg's RRF weight (f32 bits; BM25 is fixed at 1.0). Set by
    /// whoever installs an Ai — BEIR sweeps showed the built-in embedder's
    /// leg earns 0.25 while nomic-class embedders earn full weight
    /// (beir_eval.rs, measured 2026-08-09 across three domains).
    fusion_vector_weight: std::sync::atomic::AtomicU32,
    fusion_rrf_k: std::sync::atomic::AtomicU32,
    /// Where the tables live — the counts cache is persisted beside them.
    dir: std::path::PathBuf,
    /// Per-notebook document counts, keyed by the table versions they were
    /// computed from. See `NotebookCounts`.
    counts_cache: tokio::sync::RwLock<Option<NotebookCounts>>,
}

/// Cached source/note/report counts per notebook.
///
/// Computing these means scanning the sources and notes tables end to end,
/// and `list_notebooks` runs on every refresh — every `mcp://changed`, every
/// window focus, and once during boot, where on a large library it grew
/// slow enough to threaten the frontend's IPC timeout.
///
/// The cache is keyed by the two tables' Lance versions rather than
/// invalidated by hand at each write site. A version is one manifest read
/// where a recount is a full scan, and keying on it cannot miss a mutation
/// — including one made by another process against the same store, which
/// hand-placed invalidation would never see. It persists beside the tables
/// so a cold start pays the version check instead of the scan.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct NotebookCounts {
    sources_version: u64,
    notes_version: u64,
    /// notebook id -> (sources, notes excluding reports, reports)
    counts: HashMap<String, (i64, i64, i64)>,
}

/// One stored source-gist row (docs/RFC-infinite-context.md Phase 1).
/// `hash` is the i32 content hash of the source text the gist was distilled
/// from (stored in the chunk row's `ordinal` column) — the staleness signal
/// the gist sweep diffs against, router-style.
#[derive(Clone, Debug)]
pub struct GistRow {
    pub source_id: String,
    pub hash: i32,
    pub text: String,
}

/// Where the counts cache lives. Beside the tables, so a store copied or
/// restored elsewhere carries its cache — and its version keys — with it.
fn counts_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("notebook-counts.json")
}

/// Best-effort read. A missing, truncated, or stale-format file just means
/// the next `list_notebooks` recounts, which is always correct.
fn load_counts(dir: &std::path::Path) -> Option<NotebookCounts> {
    let raw = std::fs::read(counts_path(dir)).ok()?;
    serde_json::from_slice(&raw).ok()
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
        let db = Self {
            conn,
            fts_lock: tokio::sync::Mutex::new(()),
            fts_deferred: std::sync::atomic::AtomicBool::new(false),
            fts_dirty: std::sync::atomic::AtomicBool::new(false),
            chunk_context_seen: std::sync::atomic::AtomicBool::new(false),
            fts_notify: tokio::sync::Notify::new(),
            fusion_vector_weight: std::sync::atomic::AtomicU32::new(1.0f32.to_bits()),
            fusion_rrf_k: std::sync::atomic::AtomicU32::new(60.0f32.to_bits()),
            dir: dir.to_path_buf(),
            counts_cache: tokio::sync::RwLock::new(load_counts(dir)),
        };
        db.ensure_table(T_NOTEBOOKS, notebooks_schema()).await?;
        db.migrate_notebooks().await?;
        db.migrate_notebook_status().await?;
        db.ensure_table(T_SOURCES, sources_schema()).await?;
        db.migrate_sources().await?;
        db.migrate_source_image().await?;
        db.migrate_source_tags_note().await?;
        db.migrate_source_fetch().await?;
        db.backfill_blank_titles().await?;
        db.ensure_table(T_MESSAGES, messages_schema()).await?;
        db.migrate_messages().await?;
        db.ensure_table(T_NOTES, notes_schema()).await?;
        db.migrate_notes().await?;
        db.ensure_table(T_REPORTS, reports_schema()).await?;
        db.migrate_reports().await?;
        db.ensure_table(T_SOURCE_EVENTS, source_events_schema())
            .await?;
        db.migrate_source_events().await?;
        db.ensure_table(T_RECEIPTS, receipts_schema()).await?;
        db.migrate_receipts().await?;
        db.ensure_table(T_LEDGER, ledger_schema()).await?;
        db.migrate_ledger().await?;
        db.ensure_table(T_REGISTRY, registry_schema()).await?;
        db.migrate_registry().await?;
        db.ensure_table(T_META_TURNS, meta_turns_schema()).await?;
        db.migrate_meta_turns().await?;
        Ok(db)
    }

    /// Add a missing string column in place with a constant default.
    ///
    /// This replaces the old collect → drop_table → recreate → refill
    /// migration idiom, which had a fatal window: a kill between the drop
    /// and the refill (dev-watcher restarts, quits, crashes) destroyed the
    /// whole table. Lance's add_columns commits atomically — the table is
    /// never not-there.
    async fn add_string_column(&self, table: &str, column: &str, default: &str) -> Result<()> {
        let tbl = self.conn.open_table(table).execute().await?;
        if tbl.schema().await?.field_with_name(column).is_ok() {
            return Ok(());
        }
        tbl.add_columns()
            .transform(lancedb::table::NewColumnTransform::SqlExpressions(vec![(
                column.to_string(),
                format!("'{}'", esc(default)),
            )]))
            .execute()
            .await
            .with_context(|| format!("failed to add {table}.{column}"))?;
        Ok(())
    }

    /// Add a missing Int64 column in place, backfilled by a SQL expression
    /// evaluated per row — a constant (`"0"`) or an existing column
    /// (`"created_at"`). Same atomic add_columns idiom as
    /// `add_string_column`.
    async fn add_i64_column(&self, table: &str, column: &str, expr: &str) -> Result<()> {
        let tbl = self.conn.open_table(table).execute().await?;
        if tbl.schema().await?.field_with_name(column).is_ok() {
            return Ok(());
        }
        tbl.add_columns()
            .transform(lancedb::table::NewColumnTransform::SqlExpressions(vec![(
                column.to_string(),
                format!("CAST({expr} AS BIGINT)"),
            )]))
            .execute()
            .await
            .with_context(|| format!("failed to add {table}.{column}"))?;
        Ok(())
    }

    /// Flip one notebook's web-search opt-in in place.
    pub async fn set_notebook_growth_web(&self, id: &str, on: bool) -> Result<()> {
        let tbl = self.conn.open_table(T_NOTEBOOKS).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("growth_web", if on { "1" } else { "0" })
            .execute()
            .await?;
        Ok(())
    }

    /// Add the `origin` and `triage` columns ("") to pre-existing registry
    /// tables.
    async fn migrate_registry(&self) -> Result<()> {
        self.add_string_column(T_REGISTRY, "origin", "").await?;
        self.add_string_column(T_REGISTRY, "triage", "").await
    }

    /// Add the `origin` column ("") to pre-existing ledger tables.
    async fn migrate_ledger(&self) -> Result<()> {
        self.add_string_column(T_LEDGER, "origin", "").await
    }

    /// Add the `trigger` column ("interval") to pre-existing schedule tables.
    async fn migrate_reports(&self) -> Result<()> {
        self.add_string_column(T_REPORTS, "trigger", "interval")
            .await?;
        // Commissions (docs/RFC-night-shift-area.md §1). Zero means "the next
        // pass", which is the right reading for every schedule that predates
        // the column.
        self.add_i64_column(T_REPORTS, "not_before", "0").await?;
        // Standing-question filters (docs/RFC-events.md §5). Empty means
        // "any", which is exactly what every pre-filter schedule meant.
        self.add_string_column(T_REPORTS, "watch_sources", "")
            .await?;
        self.add_string_column(T_REPORTS, "watch_kinds", "").await
    }

    /// Lateness came after receipts did; 0 reads as "not recorded", which is
    /// what an older receipt honestly is.
    async fn migrate_receipts(&self) -> Result<()> {
        self.add_i64_column(T_RECEIPTS, "due_at", "0").await
    }

    /// Add the `diff` column ("") to pre-existing event tables.
    async fn migrate_source_events(&self) -> Result<()> {
        self.add_string_column(T_SOURCE_EVENTS, "diff", "").await
    }

    /// Backfill the `color` column on pre-existing `notebooks` tables,
    /// and the `growth_web` opt-in (0) on tables from before Grow's web
    /// tier moved off its JSON sidecar.
    async fn migrate_notebooks(&self) -> Result<()> {
        if !self.table_exists(T_NOTEBOOKS).await? {
            return Ok(());
        }
        self.add_i64_column(T_NOTEBOOKS, "growth_web", "0").await?;
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
                    icon: String::new(),
                    status: String::new(),
                    growth_web: false,
                    source_count: 0,
                    note_count: 0,
                    report_count: 0,
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

    /// Add the `status` ("") and `icon` ("") columns to pre-existing
    /// notebook tables.
    async fn migrate_notebook_status(&self) -> Result<()> {
        if !self.table_exists(T_NOTEBOOKS).await? {
            return Ok(());
        }
        self.add_string_column(T_NOTEBOOKS, "status", "").await?;
        self.add_string_column(T_NOTEBOOKS, "icon", "").await
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
    /// One-time backfill: any source persisted with a blank title (a page
    /// capture with no `<title>`, from before `presentable_title` guarded the
    /// ingest funnel) gets its origin host, else "Untitled" — so the list
    /// never shows an unlabeled row the user can't act on. A filtered query,
    /// so it's a no-op scan once there are none left.
    async fn backfill_blank_titles(&self) -> Result<()> {
        if !self.table_exists(T_SOURCES).await? {
            return Ok(());
        }
        // Scan all rows and test the trimmed title in Rust: a `title = ''`
        // SQL filter missed whitespace-only titles (a page <title> of spaces
        // or newlines), which still render as an unlabeled, menu-less row.
        let batches = self.collect(T_SOURCES, None).await?;
        for b in &batches {
            let id = str_col(b, "id")?;
            let url = str_col(b, "url")?;
            let title = str_col(b, "title")?;
            for i in 0..b.num_rows() {
                // Not `.trim().is_empty()`: a zero-width/BOM title isn't
                // whitespace, so trim keeps it while the row renders blank.
                if !crate::commands::is_blank_title(title.value(i)) {
                    continue;
                }
                let host = url
                    .value(i)
                    .split("://")
                    .nth(1)
                    .and_then(|rest| rest.split('/').next())
                    .unwrap_or("")
                    .trim_start_matches("www.");
                let new_title = if host.is_empty() {
                    "Untitled source"
                } else {
                    host
                };
                let tbl = self.conn.open_table(T_SOURCES).execute().await?;
                tbl.update()
                    .only_if(format!("id = '{}'", esc(id.value(i))))
                    .column("title", format!("'{}'", esc(new_title)))
                    .execute()
                    .await?;
            }
        }
        Ok(())
    }

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
        if has("url")
            && has("status")
            && has("error")
            && has("parent_id")
            && has("mtime")
            && has("author")
        {
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
                    author: String::new(),
                    image_url: String::new(),
                    tags: String::new(),
                    note: String::new(),
                    fetched_at: ca.value(i),
                    fetch_failures: 0,
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

    /// Add the `image_url` column ("") to pre-existing source tables.
    async fn migrate_source_image(&self) -> Result<()> {
        if !self.table_exists(T_SOURCES).await? {
            return Ok(());
        }
        self.add_string_column(T_SOURCES, "image_url", "").await
    }

    /// Add the `tags` / `note` columns ("") to pre-existing source tables
    /// (docs/RFC-source-tags.md).
    async fn migrate_source_tags_note(&self) -> Result<()> {
        if !self.table_exists(T_SOURCES).await? {
            return Ok(());
        }
        self.add_string_column(T_SOURCES, "tags", "").await?;
        self.add_string_column(T_SOURCES, "note", "").await
    }

    /// Add the `fetched_at` / `fetch_failures` columns to pre-existing
    /// source tables (docs/RFC-source-hygiene.md). `fetched_at` backfills
    /// from `created_at` — the honest floor: the content is at least that
    /// fresh, and hygiene ages it from there.
    async fn migrate_source_fetch(&self) -> Result<()> {
        if !self.table_exists(T_SOURCES).await? {
            return Ok(());
        }
        self.add_i64_column(T_SOURCES, "fetched_at", "created_at")
            .await?;
        self.add_i64_column(T_SOURCES, "fetch_failures", "0").await
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

    /// Width of a table's `vector` column as it exists on disk, or None when
    /// the table isn't there (or has no vector column). The vector tables are
    /// created lazily at whatever dimensionality the embedder of the day
    /// emitted, so this is the only honest answer to "what will Lance accept
    /// as a query vector here" — a query of any other width fails the search
    /// with "No vector column found to match with the query vector dimension".
    pub async fn vector_dim(&self, table: &str) -> Result<Option<usize>> {
        if !self.table_exists(table).await? {
            return Ok(None);
        }
        let tbl = self.conn.open_table(table).execute().await?;
        let schema = tbl.schema().await?;
        Ok(schema
            .field_with_name("vector")
            .ok()
            .and_then(|f| match f.data_type() {
                DataType::FixedSizeList(_, dim) => Some(*dim as usize),
                _ => None,
            }))
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

    /// Append a batch, conformed to the LIVE table schema first. The dev
    /// build and the installed app share one store, so a newer binary may
    /// have migrated columns this binary doesn't know about. Reads already
    /// tolerate that; appends must too — Lance rejects a batch whose fields
    /// don't match the table ("Append with different schema … missing=[…]"),
    /// which bricked every insert in the installed app the first time dev
    /// added a column. Unknown columns are filled with defaults ("", 0);
    /// anything unsynthesizable falls through so the original error surfaces.
    async fn add_batch(&self, table: &str, schema: SchemaRef, batch: RecordBatch) -> Result<()> {
        let tbl = self.conn.open_table(table).execute().await?;
        let (schema, batch) = match tbl.schema().await {
            Ok(live) => conform_to_live(&live, schema, batch),
            Err(_) => (schema, batch),
        };
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

    /// `collect`, but reading only the named columns — the difference
    /// between a count and dragging every source's full text through Arrow.
    async fn collect_cols(
        &self,
        table: &str,
        filter: Option<&str>,
        cols: &[&str],
    ) -> Result<Vec<RecordBatch>> {
        if !self.table_exists(table).await? {
            return Ok(vec![]);
        }
        let tbl = self.conn.open_table(table).execute().await?;
        let mut q = tbl.query().select(lancedb::query::Select::columns(cols));
        if let Some(f) = filter {
            q = q.only_if(f);
        }
        Ok(q.execute().await?.try_collect::<Vec<_>>().await?)
    }

    /// Full extracted text for a batch of sources in ONE projected scan.
    /// The gallery's snippet path used to make up to 400 single-id scans of
    /// the same table for this.
    pub async fn source_contents(&self, source_ids: &[String]) -> Result<HashMap<String, String>> {
        if source_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids = source_ids
            .iter()
            .map(|id| format!("'{}'", esc(id)))
            .collect::<Vec<_>>()
            .join(", ");
        let filter = format!("id IN ({ids})");
        let batches = self
            .collect_cols(T_SOURCES, Some(&filter), &["id", "content"])
            .await?;
        let mut out = HashMap::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let content = str_col(b, "content")?;
            for i in 0..b.num_rows() {
                out.insert(id.value(i).to_string(), content.value(i).to_string());
            }
        }
        Ok(out)
    }

    /// `source_contents` plus each source's title, for surfaces that need to
    /// tell the body apart from its own heading (gallery snippets).
    pub async fn source_titled_contents(
        &self,
        source_ids: &[String],
    ) -> Result<HashMap<String, (String, String)>> {
        if source_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids = source_ids
            .iter()
            .map(|id| format!("'{}'", esc(id)))
            .collect::<Vec<_>>()
            .join(", ");
        let filter = format!("id IN ({ids})");
        let batches = self
            .collect_cols(T_SOURCES, Some(&filter), &["id", "title", "content"])
            .await?;
        let mut out = HashMap::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let title = str_col(b, "title")?;
            let content = str_col(b, "content")?;
            for i in 0..b.num_rows() {
                out.insert(
                    id.value(i).to_string(),
                    (title.value(i).to_string(), content.value(i).to_string()),
                );
            }
        }
        Ok(out)
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
            let icon = opt_str_col(b, "icon");
            let status = opt_str_col(b, "status");
            let growth_web = i64_col(b, "growth_web")?;
            for i in 0..b.num_rows() {
                notebooks.push(Notebook {
                    id: id.value(i).to_string(),
                    title: title.value(i).to_string(),
                    created_at: created.value(i),
                    updated_at: updated.value(i),
                    color: color.map(|c| c.value(i).to_string()).unwrap_or_default(),
                    icon: icon.map(|c| c.value(i).to_string()).unwrap_or_default(),
                    status: status.map(|s| s.value(i).to_string()).unwrap_or_default(),
                    growth_web: growth_web.value(i) != 0,
                    source_count: 0,
                    note_count: 0,
                    report_count: 0,
                });
            }
        }

        // One row per id, whatever the table holds: a double import once
        // appended the same notebook twice (see `dedupe_notebook_rows`), and
        // a listing must never show one notebook as two. The active row wins
        // over an archived one, then the most recently updated.
        dedupe_notebooks(&mut notebooks);

        // Counting means scanning sources and notes end to end, so it is
        // done only when one of those tables has actually moved. See
        // `NotebookCounts`.
        let sources_version = self.table_version(T_SOURCES).await;
        let notes_version = self.table_version(T_NOTES).await;
        if let (Some(sv), Some(nv)) = (sources_version, notes_version) {
            let cached = self.counts_cache.read().await.clone();
            if let Some(c) = cached {
                if c.sources_version == sv && c.notes_version == nv {
                    for n in &mut notebooks {
                        let (s, no, r) = c.counts.get(&n.id).copied().unwrap_or((0, 0, 0));
                        n.source_count = s;
                        n.note_count = no;
                        n.report_count = r;
                    }
                    notebooks.sort_by_key(|n| std::cmp::Reverse(n.updated_at));
                    return Ok(notebooks);
                }
            }
        }

        // Count sources per notebook in one pass — projected to the one
        // column a count needs. Unprojected it dragged the whole corpus's
        // content through Arrow each time.
        let mut counts: HashMap<String, i64> = HashMap::new();
        for b in &self.collect_cols(T_SOURCES, None, &["notebook_id"]).await? {
            let nb = str_col(b, "notebook_id")?;
            for i in 0..b.num_rows() {
                *counts.entry(nb.value(i).to_string()).or_insert(0) += 1;
            }
        }
        // Notes and reports, same one-pass shape. Reports are a note kind, so
        // they're counted out of the note total rather than added to it —
        // "12 notes, 3 reports" reading as 15 documents would be a lie.
        let mut note_counts: HashMap<String, i64> = HashMap::new();
        let mut report_counts: HashMap<String, i64> = HashMap::new();
        if self.table_exists(T_NOTES).await? {
            let note_batches = match self
                .collect_cols(T_NOTES, None, &["notebook_id", "kind"])
                .await
            {
                Ok(b) => b,
                // "kind" postdates the notes table — degrade on old stores.
                Err(_) => self.collect(T_NOTES, None).await?,
            };
            for b in &note_batches {
                let nb = str_col(b, "notebook_id")?;
                let kind = opt_str_col(b, "kind");
                for i in 0..b.num_rows() {
                    let is_report = kind.as_ref().map(|k| k.value(i)) == Some("report");
                    let map = if is_report {
                        &mut report_counts
                    } else {
                        &mut note_counts
                    };
                    *map.entry(nb.value(i).to_string()).or_insert(0) += 1;
                }
            }
        }
        for n in &mut notebooks {
            n.source_count = counts.get(&n.id).copied().unwrap_or(0);
            n.note_count = note_counts.get(&n.id).copied().unwrap_or(0);
            n.report_count = report_counts.get(&n.id).copied().unwrap_or(0);
        }
        // Store under the versions read BEFORE the scan: a write that landed
        // mid-scan leaves the cache keyed to the older version, so the next
        // call recounts rather than serving a number it never computed.
        if let (Some(sv), Some(nv)) = (sources_version, notes_version) {
            let fresh = NotebookCounts {
                sources_version: sv,
                notes_version: nv,
                counts: notebooks
                    .iter()
                    .map(|n| (n.id.clone(), (n.source_count, n.note_count, n.report_count)))
                    .collect(),
            };
            self.save_counts(&fresh).await;
        }
        notebooks.sort_by_key(|n| std::cmp::Reverse(n.updated_at));
        Ok(notebooks)
    }

    /// A table's Lance version, or None if it does not exist yet. One
    /// manifest read — the whole point of keying the counts cache on it.
    async fn table_version(&self, table: &str) -> Option<u64> {
        let tbl = self.conn.open_table(table).execute().await.ok()?;
        tbl.version().await.ok()
    }

    /// Publish freshly counted totals to memory and disk. Failing to write
    /// the file costs a recount next boot and nothing else, so it is not an
    /// error worth propagating out of a read.
    async fn save_counts(&self, fresh: &NotebookCounts) {
        *self.counts_cache.write().await = Some(fresh.clone());
        match serde_json::to_vec(fresh) {
            Ok(bytes) => {
                if let Err(err) = std::fs::write(counts_path(&self.dir), bytes) {
                    crate::note!("notebook counts cache write failed: {err}");
                }
            }
            Err(err) => crate::note!("notebook counts cache encode failed: {err}"),
        }
    }

    /// Insert a notebook row. An id that already exists is refused rather
    /// than appended: Lance has no unique constraint, and the one time two
    /// imports raced on a reused id the store carried the same notebook
    /// twice. Callers that meant "open the existing notebook" look it up
    /// first; this makes the other case an error, not a duplicate.
    pub async fn create_notebook(&self, notebook: &Notebook) -> Result<()> {
        let pred = format!("id = '{}'", esc(&notebook.id));
        let existing = self.collect(T_NOTEBOOKS, Some(&pred)).await?;
        if existing.iter().any(|b| b.num_rows() > 0) {
            anyhow::bail!("notebook {} already exists", notebook.id);
        }
        let schema = notebooks_schema();
        let batch = notebook_batch(&schema, std::slice::from_ref(notebook))?;
        self.add_batch(T_NOTEBOOKS, schema, batch).await
    }

    /// The raw insert, without the duplicate-id guard, for the one caller
    /// that has already removed every row for the id (`dedupe_notebook_rows`).
    async fn insert_notebook_row(&self, notebook: &Notebook) -> Result<()> {
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
        // "This notebook's contents changed" is exactly when a bound notebook
        // owes its bundle a write (docs/RFC-okf-live.md §5.2). Every source
        // mutation already lands here, so one hook covers add, delete,
        // rename, refresh, and retag. Cheap and reentrancy-free: unbound
        // notebooks return on a file read, and a bound one only records a
        // deadline for a task that runs two seconds from now.
        crate::okf::schedule_write(id);
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

    pub async fn set_notebook_icon(&self, id: &str, icon: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_NOTEBOOKS).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("icon", format!("'{}'", esc(icon)))
            .execute()
            .await?;
        Ok(())
    }

    /// Ids of archived notebooks — the background gate: scheduled reports
    /// and source resyncs skip these until unarchive (nothing is mutated,
    /// so unarchiving resumes them automatically).
    pub async fn archived_notebook_ids(&self) -> Result<std::collections::HashSet<String>> {
        let batches = self.collect(T_NOTEBOOKS, None).await?;
        let mut out = std::collections::HashSet::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let Some(status) = opt_str_col(b, "status") else {
                continue;
            };
            for i in 0..b.num_rows() {
                if status.value(i) == "archived" {
                    out.insert(id.value(i).to_string());
                }
            }
        }
        Ok(out)
    }

    /// Set the notebook's lifecycle status: "" (active) or "archived".
    pub async fn set_notebook_status(&self, id: &str, status: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_NOTEBOOKS).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("status", format!("'{}'", esc(status)))
            .execute()
            .await?;
        Ok(())
    }

    /// Collapse notebooks that exist as more than one row with the same id.
    /// The 0.55.0 root watcher raced its own tick and imported one bundle
    /// twice; because an import reuses the bundle's notebook id when it is
    /// free, the second import appended a second row for the same id, and
    /// the sources under that id showed up under both. The rows are deleted
    /// together (a predicate on the id matches every one) and the keeper is
    /// written back. Returns the titles that were collapsed.
    pub async fn dedupe_notebook_rows(&self) -> Result<Vec<String>> {
        let batches = self.collect(T_NOTEBOOKS, None).await?;
        let mut rows: Vec<Notebook> = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let title = str_col(b, "title")?;
            let created = i64_col(b, "created_at")?;
            let updated = i64_col(b, "updated_at")?;
            let color = opt_str_col(b, "color");
            let icon = opt_str_col(b, "icon");
            let status = opt_str_col(b, "status");
            let growth_web = i64_col(b, "growth_web")?;
            for i in 0..b.num_rows() {
                rows.push(Notebook {
                    id: id.value(i).to_string(),
                    title: title.value(i).to_string(),
                    created_at: created.value(i),
                    updated_at: updated.value(i),
                    color: color.map(|c| c.value(i).to_string()).unwrap_or_default(),
                    icon: icon.map(|c| c.value(i).to_string()).unwrap_or_default(),
                    status: status.map(|s| s.value(i).to_string()).unwrap_or_default(),
                    growth_web: growth_web.value(i) != 0,
                    source_count: 0,
                    note_count: 0,
                    report_count: 0,
                });
            }
        }
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for r in &rows {
            *counts.entry(r.id.as_str()).or_insert(0) += 1;
        }
        let dup_ids: Vec<String> = counts
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(id, _)| id.to_string())
            .collect();
        let mut collapsed = Vec::new();
        for id in dup_ids {
            let mut group: Vec<Notebook> = rows.iter().filter(|r| r.id == id).cloned().collect();
            dedupe_notebooks(&mut group);
            let Some(keeper) = group.into_iter().next() else {
                continue;
            };
            self.delete_where(T_NOTEBOOKS, &format!("id = '{}'", esc(&id)))
                .await?;
            self.insert_notebook_row(&keeper).await?;
            collapsed.push(keeper.title);
        }
        Ok(collapsed)
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
        // Metadata-only listings project content away at the query — the
        // old shape read every source's full text and then discarded it,
        // on every list_sources call in the app.
        let meta_cols: &[&str] = &[
            "id",
            "notebook_id",
            "title",
            "source_type",
            "url",
            "char_count",
            "chunk_count",
            "created_at",
            "status",
            "error",
            "parent_id",
            "mtime",
            "author",
            "image_url",
            "tags",
            "note",
            "fetched_at",
            "fetch_failures",
        ];
        let batches = if with_content {
            self.collect(T_SOURCES, filter).await?
        } else {
            match self.collect_cols(T_SOURCES, filter, meta_cols).await {
                Ok(b) => b,
                // A store from an older version may predate one of these
                // columns (that's what opt_str_col is for) — degrade to the
                // full read rather than failing the list.
                Err(_) => self.collect(T_SOURCES, filter).await?,
            }
        };
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
            let author = str_col(b, "author")?;
            let image = str_col(b, "image_url")?;
            let tags = str_col(b, "tags")?;
            let note = str_col(b, "note")?;
            let fetched = i64_col(b, "fetched_at")?;
            let failures = i64_col(b, "fetch_failures")?;
            for i in 0..b.num_rows() {
                sources.push(Source {
                    author: author.value(i).to_string(),
                    image_url: image.value(i).to_string(),
                    tags: tags.value(i).to_string(),
                    note: note.value(i).to_string(),
                    fetched_at: fetched.value(i),
                    fetch_failures: failures.value(i),
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

    /// Every source in a notebook WITH its text, in one scan.
    ///
    /// `source_content` filter-scans the whole sources table per call, which
    /// is fine for opening one document and quietly disastrous for anything
    /// that wants them all: the link graph called it once per source and
    /// spent seconds doing hundreds of sequential scans of the same table.
    /// One scan, all the rows.
    pub async fn sources_with_content(&self, notebook_id: &str) -> Result<Vec<Source>> {
        let filter = format!("notebook_id = '{}'", esc(notebook_id));
        let mut sources = self.query_sources(Some(&filter), true).await?;
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
        out.extend(
            self.query_sources(Some("source_type = 'obsidian'"), false)
                .await?,
        );
        out.extend(
            self.query_sources(Some("source_type = 'okf'"), false)
                .await?,
        );
        Ok(out)
    }

    /// Living Mac sources — `cider://` origins (mac.rs) of every status: the
    /// store watch re-fetches these by provider, and an errored one (a
    /// permission denied at add time) deserves the retry a change brings.
    pub async fn all_mac_sources(&self) -> Result<Vec<Source>> {
        self.query_sources(Some("source_type = 'mac'"), false).await
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

    /// Stamp a source's freshness pair (docs/RFC-source-hygiene.md) without
    /// touching content or chunks: a background probe failure keeps the
    /// last-good `fetched_at` and bumps `fetch_failures`; a "Keep" from the
    /// hygiene review resets the count.
    pub async fn set_source_fetch(
        &self,
        source_id: &str,
        fetched_at: i64,
        fetch_failures: i64,
    ) -> Result<()> {
        let tbl = self.conn.open_table(T_SOURCES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(source_id)))
            .column("fetched_at", fetched_at.to_string())
            .column("fetch_failures", fetch_failures.to_string())
            .execute()
            .await?;
        Ok(())
    }

    /// Every web-url source across all notebooks (metadata only) — the
    /// hygiene sweep's worklist (docs/RFC-source-hygiene.md).
    pub async fn all_url_sources(&self) -> Result<Vec<Source>> {
        self.query_sources(Some("source_type = 'url'"), false).await
    }

    /// Feed parents (docs/RFC-events.md §2) — the poller's work list.
    pub async fn all_feed_sources(&self) -> Result<Vec<Source>> {
        self.query_sources(Some("source_type = 'feed'"), false)
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

    /// Rename a source in place — the background retitle's write. The row is
    /// the whole rename: chunks carry no title, and citations join
    /// `source_title` from this table at read time (`search_chunks_trace`).
    /// (v0.34.0 also updated a `source_title` column on chunks that doesn't
    /// exist, which errored the whole call and silently dropped every
    /// retitle.)
    pub async fn set_source_title(&self, source_id: &str, title: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_SOURCES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(source_id)))
            .column("title", format!("'{}'", esc(title)))
            .execute()
            .await?;
        Ok(())
    }

    /// Close out an import's background stage (RFC-import-pipeline §2):
    /// stamp status, error, and the chunk count the stage produced, without
    /// touching content or chunks.
    pub async fn finish_processing(
        &self,
        source_id: &str,
        chunk_count: i64,
        status: &str,
        error: &str,
    ) -> Result<()> {
        let tbl = self.conn.open_table(T_SOURCES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(source_id)))
            .column("status", format!("'{}'", esc(status)))
            .column("error", format!("'{}'", esc(error)))
            .column("chunk_count", chunk_count.to_string())
            .execute()
            .await?;
        Ok(())
    }

    /// Stamp a source's gallery lead image ("" unknown, "-" checked-none)
    /// without touching content or chunks.
    pub async fn set_source_image(&self, source_id: &str, image_url: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_SOURCES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(source_id)))
            .column("image_url", format!("'{}'", esc(image_url)))
            .execute()
            .await?;
        Ok(())
    }

    /// Stamp a source's normalized tag string (docs/RFC-source-tags.md)
    /// without touching content or chunks. Routes pick the change up on the
    /// next self-healing sweep (the summary string diff re-embeds).
    /// Point a source at a new on-disk origin (the file moved) and clear
    /// its failure count so hygiene stops flagging it. Content untouched —
    /// the caller re-ingests through the normal refresh path.
    pub async fn set_source_path(&self, source_id: &str, path: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_SOURCES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(source_id)))
            .column("url", format!("'{}'", esc(path)))
            .column("fetch_failures", "0".to_string())
            .execute()
            .await?;
        Ok(())
    }

    pub async fn set_source_tags(&self, source_id: &str, tags: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_SOURCES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(source_id)))
            .column("tags", format!("'{}'", esc(tags)))
            .execute()
            .await?;
        Ok(())
    }

    /// Store the user's annotation on a source (docs/RFC-source-tags.md)
    /// without touching content or chunks. The caller (re)indexes the
    /// matching `snote:<id>` chunk rows.
    pub async fn set_source_note(&self, source_id: &str, note: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_SOURCES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(source_id)))
            .column("note", format!("'{}'", esc(note)))
            .execute()
            .await?;
        Ok(())
    }

    /// Drop a source's chunk rows — the reingest stage's swap. Annotation
    /// (`snote:`) and gist (`gist:`) rows carry prefixed owner ids, so the
    /// plain-id predicate leaves them alone.
    pub async fn delete_source_chunks(&self, source_id: &str) -> Result<()> {
        // Section rows name passage spans by ordinal; a re-chunk invalidates
        // them, so they go with the chunks (the gist row, keyed by content
        // hash, survives and re-diffs on its own).
        let pred = format!(
            "source_id = '{0}' OR source_id = '{SECTION_CHUNK_PREFIX}{0}'",
            esc(source_id)
        );
        self.delete_where(T_CHUNKS, &pred).await
    }

    /// Swap a source's ROW without touching its chunks — the async reingest
    /// writes the row first, so the old chunks keep serving retrieval until
    /// the background stage replaces them.
    pub async fn replace_source_row(&self, source: &Source) -> Result<()> {
        self.delete_where(T_SOURCES, &format!("id = '{}'", esc(&source.id)))
            .await?;
        let schema = sources_schema();
        let batch = source_batch(&schema, std::slice::from_ref(source))?;
        self.add_batch(T_SOURCES, schema, batch).await
    }

    /// Drop the indexed chunk rows for a source's annotation (owner
    /// `snote:<source_id>`), ahead of a re-index or when the note is cleared.
    pub async fn delete_snote_chunks(&self, source_id: &str) -> Result<()> {
        let pred = format!("source_id = '{SNOTE_CHUNK_PREFIX}{}'", esc(source_id));
        self.delete_where(T_CHUNKS, &pred).await
    }

    /// Flip a source's `source_type` in place (no child/chunk disturbance) —
    /// used to upgrade a plain folder to an Obsidian vault when `.obsidian/`
    /// is detected on rescan.
    pub async fn set_source_type(&self, source_id: &str, source_type: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_SOURCES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(source_id)))
            .column("source_type", format!("'{}'", esc(source_type)))
            .execute()
            .await?;
        Ok(())
    }

    /// Bulk-append source rows in one Lance commit. Fixture seeding (and any
    /// future bulk importer) must not pay one commit per row — at thousands
    /// of rows that is both slow and a fragment storm for later scans.
    pub async fn insert_sources_bulk(&self, sources: &[Source]) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }
        let schema = sources_schema();
        let batch = source_batch(&schema, sources)?;
        self.add_batch(T_SOURCES, schema, batch).await
    }

    /// Insert a source row plus all of its embedded chunks atomically-ish.
    pub async fn insert_source(
        &self,
        source: &Source,
        chunks: &[(String, i32, String)],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        self.insert_source_ctx(source, chunks, &[], embeddings)
            .await
    }

    /// `insert_source` with each chunk's context ("title › chain") stored
    /// beside it; an empty `contexts` stores blanks.
    pub async fn insert_source_ctx(
        &self,
        source: &Source,
        chunks: &[(String, i32, String)],
        contexts: &[String],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        // Source row.
        let schema = sources_schema();
        let batch = source_batch(&schema, std::slice::from_ref(source))?;
        self.add_batch(T_SOURCES, schema, batch).await?;
        self.add_chunks_ctx(
            &source.notebook_id,
            &source.id,
            chunks,
            contexts,
            embeddings,
        )
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
        self.add_chunks_ctx(notebook_id, source_id, chunks, &[], embeddings)
            .await
    }

    /// Does the live chunks table carry the context column? Tables from
    /// before Phase 1.5 don't until `ensure_chunk_context_column` runs.
    /// Remembered once seen: a column never goes away, and every search
    /// asks — a schema read per query would be a needless round trip.
    async fn has_chunk_context(&self) -> Result<bool> {
        if self
            .chunk_context_seen
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(true);
        }
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(false);
        }
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        let has = tbl
            .schema()
            .await?
            .fields()
            .iter()
            .any(|f| f.name() == CHUNK_CONTEXT_COL);
        if has {
            self.chunk_context_seen
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(has)
    }

    /// Tests only: rebuild the chunks FTS index from scratch, so a seeding
    /// that appended rows after an earlier build measures against one
    /// index of everything rather than an incrementally optimized one.
    #[cfg(test)]
    pub async fn rebuild_fts_from_scratch(&self) -> Result<()> {
        let _guard = self.fts_lock.lock().await;
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(());
        }
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        let col = if self.has_chunk_context().await? {
            CHUNK_BM25_COL
        } else {
            "text"
        };
        tbl.create_index(&[col], Index::FTS(FtsIndexBuilder::default()))
            .replace(true)
            .execute()
            .await?;
        self.fts_dirty
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    /// Tests only: one source's section rows as (chain, summary), in
    /// section order.
    #[cfg(test)]
    pub async fn section_rows(&self, source_id: &str) -> Result<Vec<(String, String)>> {
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(vec![]);
        }
        let filter = format!("source_id = '{SECTION_CHUNK_PREFIX}{}'", esc(source_id));
        let batches = self
            .collect_cols(
                T_CHUNKS,
                Some(&filter),
                &["ordinal", "text", CHUNK_CONTEXT_COL],
            )
            .await?;
        let mut rows: Vec<(i32, String, String)> = Vec::new();
        for b in &batches {
            let ord = i32_col(b, "ordinal")?;
            let text = str_col(b, "text")?;
            let ctx = str_col(b, CHUNK_CONTEXT_COL)?;
            for i in 0..b.num_rows() {
                rows.push((
                    ord.value(i),
                    ctx.value(i).to_string(),
                    text.value(i).to_string(),
                ));
            }
        }
        rows.sort_by_key(|r| r.0);
        Ok(rows.into_iter().map(|(_, c, t)| (c, t)).collect())
    }

    /// Probe for tests: (chunk rows, rows whose context is non-empty).
    #[cfg(test)]
    pub async fn context_fill(&self) -> Result<(usize, usize)> {
        if !self.table_exists(T_CHUNKS).await? {
            return Ok((0, 0));
        }
        let batches = self
            .collect_cols(T_CHUNKS, None, &["id", CHUNK_CONTEXT_COL])
            .await?;
        let mut rows = 0usize;
        let mut filled = 0usize;
        for b in &batches {
            rows += b.num_rows();
            if let Some(col) = b
                .column_by_name(CHUNK_CONTEXT_COL)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            {
                filled += (0..b.num_rows())
                    .filter(|&i| !col.value(i).is_empty())
                    .count();
            }
        }
        Ok((rows, filled))
    }

    /// One-time schema evolution: give an older chunks table the context
    /// column, blank for every existing row. Older builds sharing the store
    /// keep working — their appends conform to the live schema with "".
    async fn ensure_chunk_context_column(&self) -> Result<()> {
        if self.has_chunk_context().await? {
            return Ok(());
        }
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        // Existing rows: no chain to give them, but their BM25 document is
        // still their text — the index must not lose them.
        tbl.add_columns()
            .transform(lancedb::table::NewColumnTransform::SqlExpressions(vec![
                (CHUNK_CONTEXT_COL.to_string(), "''".to_string()),
                (CHUNK_BM25_COL.to_string(), "text".to_string()),
            ]))
            .execute()
            .await?;
        crate::note!("chunks: added the context and bm25 columns");
        Ok(())
    }

    /// `add_chunks` with contexts. `contexts[i]` belongs to `chunks[i]`;
    /// a short or empty slice stores "" for the rest.
    pub async fn add_chunks_ctx(
        &self,
        notebook_id: &str,
        source_id: &str,
        chunks: &[(String, i32, String)],
        contexts: &[String],
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
        self.ensure_chunk_context_column().await?;

        let schema = chunks_schema(dim);
        let ids: Vec<String> = chunks.iter().map(|c| c.0.clone()).collect();
        let nbs: Vec<String> = chunks.iter().map(|_| notebook_id.to_string()).collect();
        let sids: Vec<String> = chunks.iter().map(|_| source_id.to_string()).collect();
        let ords: Vec<i32> = chunks.iter().map(|c| c.1).collect();
        let texts: Vec<String> = chunks.iter().map(|c| c.2.clone()).collect();
        let ctxs: Vec<String> = (0..chunks.len())
            .map(|i| contexts.get(i).cloned().unwrap_or_default())
            .collect();
        let docs: Vec<String> = ctxs
            .iter()
            .zip(chunks.iter())
            .map(|(c, ch)| bm25_doc(c, &ch.2))
            .collect();
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
                Arc::new(StringArray::from(ctxs)),
                Arc::new(StringArray::from(docs)),
            ],
        )?;
        self.add_batch(T_CHUNKS, schema, batch).await?;

        // The BM25 side of hybrid search stays NEAR-current, never inline: a
        // full Tantivy rebuild per write meant every note edit, Mac-item
        // sync, and single-file refresh paid a whole-corpus index build.
        // Mark dirty; bulk writers (folder import, eval seeding) defer and
        // flush themselves, everyone else nudges the debounced flusher
        // (lib.rs), which lands one rebuild ~2s after the burst. In the gap
        // the vector leg still finds the new chunks — a stale index only
        // costs BM25 rank, never the search (it degrades with a warning).
        self.fts_dirty
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if !self.fts_deferred.load(std::sync::atomic::Ordering::SeqCst) {
            self.fts_notify.notify_one();
        }
        Ok(())
    }

    /// Block until some writer nudges the FTS flusher — the debounce loop's
    /// wait (lib.rs). A nudge that lands before the wait is stored, not lost.
    pub async fn fts_write_notified(&self) {
        self.fts_notify.notified().await;
    }

    /// Bulk chunk append across MANY sources in one Lance commit — corpus
    /// seeding's path (the BEIR eval's 5k documents would otherwise be 5k
    /// commits). Rows are (source_id, chunk_id, ordinal, text); embeddings
    /// align by index. Marks the FTS index dirty like every chunk write.
    // Test-only today (the BEIR eval); the bulk-import path that should
    // adopt it is RFC-import-pipeline follow-up work.
    #[cfg_attr(not(test), allow(dead_code))]
    pub async fn add_chunk_rows(
        &self,
        notebook_id: &str,
        rows: &[(String, String, i32, String)],
        contexts: &[String],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let dim = embeddings
            .first()
            .map(|v| v.len())
            .ok_or_else(|| anyhow!("no embeddings for chunk rows"))? as i32;
        self.ensure_table(T_CHUNKS, chunks_schema(dim)).await?;
        self.ensure_chunk_context_column().await?;
        let schema = chunks_schema(dim);
        let ids: Vec<String> = rows.iter().map(|r| r.1.clone()).collect();
        let nbs: Vec<String> = rows.iter().map(|_| notebook_id.to_string()).collect();
        let sids: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
        let ords: Vec<i32> = rows.iter().map(|r| r.2).collect();
        let texts: Vec<String> = rows.iter().map(|r| r.3.clone()).collect();
        let ctxs: Vec<String> = (0..rows.len())
            .map(|i| contexts.get(i).cloned().unwrap_or_default())
            .collect();
        let docs: Vec<String> = ctxs
            .iter()
            .zip(rows.iter())
            .map(|(c, r)| bm25_doc(c, &r.3))
            .collect();
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
                Arc::new(StringArray::from(ctxs)),
                Arc::new(StringArray::from(docs)),
            ],
        )?;
        self.add_batch(T_CHUNKS, schema, batch).await?;
        self.fts_dirty
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if !self.fts_deferred.load(std::sync::atomic::Ordering::SeqCst) {
            self.fts_notify.notify_one();
        }
        Ok(())
    }

    /// The current RRF fusion parameters (vector weight, k); BM25's
    /// weight is fixed at 1.0.
    fn fusion(&self) -> (f32, f32) {
        (
            f32::from_bits(
                self.fusion_vector_weight
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            f32::from_bits(self.fusion_rrf_k.load(std::sync::atomic::Ordering::Relaxed)),
        )
    }

    /// Stamp the RRF fusion parameters — called when an Ai is installed,
    /// so fusion always matches the embedder tier that filled the index.
    pub fn set_fusion(&self, (w, k): (f32, f32)) {
        self.fusion_vector_weight
            .store(w.to_bits(), std::sync::atomic::Ordering::Relaxed);
        self.fusion_rrf_k
            .store(k.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Enter/leave bulk-write mode (see `fts_deferred`). Leaving does NOT
    /// rebuild — call `flush_fts` after, so error paths can still flush.
    pub fn defer_fts(&self, on: bool) {
        self.fts_deferred
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// Reclaim disk and scan speed: compact fragmented tables and prune old
    /// dataset versions. Lance is additive — every write, and every FTS
    /// rebuild, leaves the prior version (index and all) on disk forever
    /// unless pruned, and nothing prunes by default. An install observed in
    /// the wild held 69 MB of live chunk data inside a 9.8 GB table: 11,605
    /// versions, 8.6 GB of dead FTS indices, and 4,085 fragments for every
    /// scan to plan over. Best-effort per table; an hour of retained history
    /// is generous when this process is the only writer. Returns (bytes
    /// reclaimed, versions removed).
    pub async fn maintain(&self) -> Result<(u64, u64)> {
        use lancedb::table::OptimizeAction;
        let mut bytes = 0u64;
        let mut versions = 0u64;
        for name in self.conn.table_names().execute().await? {
            let Ok(tbl) = self.conn.open_table(&name).execute().await else {
                continue;
            };
            // Compact first so the prune that follows can also drop the
            // pre-compaction fragments it just superseded.
            let _ = tbl
                .optimize(OptimizeAction::Compact {
                    options: Default::default(),
                    remap_options: None,
                })
                .await;
            if let Ok(stats) = tbl
                .optimize(OptimizeAction::Prune {
                    older_than: Some(lancedb::table::optimize::Duration::hours(1)),
                    delete_unverified: None,
                    error_if_tagged_old_versions: Some(false),
                })
                .await
            {
                if let Some(p) = stats.prune {
                    bytes += p.bytes_removed;
                    versions += p.old_versions;
                }
            }
        }
        Ok((bytes, versions))
    }

    /// Rebuild the chunks FTS index if any deferred write dirtied it.
    pub async fn flush_fts(&self) -> Result<()> {
        if self
            .fts_dirty
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.rebuild_chunks_fts().await?;
        }
        Ok(())
    }

    /// Bring the chunks full-text index up to date, serialized process-wide
    /// and retried on Lance's retryable commit conflicts. INCREMENTAL when
    /// the index exists: rows written since the last pass join as a delta
    /// index and merge into the newest one (`OptimizeOptions` default), so
    /// a burst of writes costs a delta, not a whole-corpus Tantivy rebuild.
    /// The only full build is the first (fresh store, or after
    /// `clear_all_chunks`). Rows deleted since (reingest, source deletion)
    /// leave stale index entries that Lance filters at query time; the
    /// merge keeps the delta count bounded.
    async fn rebuild_chunks_fts(&self) -> Result<()> {
        let _guard = self.fts_lock.lock().await;
        let mut attempt = 0u32;
        loop {
            let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
            let indices = tbl.list_indices().await?;
            // The BM25 document column (context + text) carries the index
            // once it exists; a table from before it keeps indexing text.
            let fts_col = if self.has_chunk_context().await? {
                CHUNK_BM25_COL
            } else {
                "text"
            };
            let has_fts = indices.iter().any(|i| i.columns == [fts_col]);
            // lance-index 7.0's inverted builder PANICS (not errors) with an
            // index-out-of-bounds (builder.rs:856) when deletes + compaction
            // remap leave the token set inconsistent (TokenSet::next_id >
            // tokens.len(); lance-format/lance#8310, fixed in lance-index
            // >= 8). Concurrent appends alone never trigger it — a repro
            // confirmed that, so appends are NOT frozen during builds.
            // Spawned task: a panic becomes a retryable JoinError instead of
            // killing whichever caller awaited the flush — still worthwhile
            // after the upstream fix, since an older binary sharing the
            // store can leave a remapped index behind.
            let t = tbl.clone();
            let result: std::result::Result<(), String> = tokio::spawn(async move {
                if has_fts {
                    t.optimize(lancedb::table::OptimizeAction::Index(
                        lancedb::table::OptimizeOptions::default(),
                    ))
                    .await
                    .map(|_| ())
                } else {
                    t.create_index(&[fts_col], Index::FTS(FtsIndexBuilder::default()))
                        .replace(true)
                        .execute()
                        .await
                }
                .map_err(|e| e.to_string())
            })
            .await
            .map_err(|join| format!("Retryable: index build panicked: {join}"))
            .and_then(|r| r);
            match result {
                Ok(()) => {
                    // Each pass leaves its predecessor behind as a dead
                    // version — prune promptly. Deltas are small, but a
                    // chatty session still accumulates.
                    let _ = tbl
                        .optimize(lancedb::table::OptimizeAction::Prune {
                            older_than: Some(lancedb::table::optimize::Duration::minutes(10)),
                            delete_unverified: None,
                            error_if_tagged_old_versions: Some(false),
                        })
                        .await;
                    return Ok(());
                }
                Err(msg) => {
                    let retryable = msg.contains("Retryable")
                        || msg.contains("commit conflict")
                        || msg.contains("preempted");
                    if retryable && attempt < 5 {
                        attempt += 1;
                        tokio::time::sleep(std::time::Duration::from_millis(
                            40 * u64::from(attempt),
                        ))
                        .await;
                        continue;
                    }
                    return Err(anyhow::anyhow!(msg))
                        .context("failed to update full-text index on chunks");
                }
            }
        }
    }

    /// All sources across every notebook, with full content (for re-embedding).
    pub async fn all_sources(&self) -> Result<Vec<Source>> {
        self.query_sources(None, true).await
    }

    /// All sources, metadata only — one projected scan, no content.
    pub async fn all_sources_lean(&self) -> Result<Vec<Source>> {
        self.query_sources(None, false).await
    }

    /// Sources still mid-embed — the startup resume's query. Content rides
    /// along because the resumed stage chunks from it, and the filter keeps
    /// that cost to the stranded few instead of the whole corpus.
    pub async fn processing_sources(&self) -> Result<Vec<Source>> {
        self.query_sources(Some("status = 'processing'"), true)
            .await
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
        // Chunks, the source's gist row, and its annotation rows go together
        // (the gist sweep would also catch a stray gist later, but immediate
        // is cleaner).
        let pred = format!(
            "source_id = '{0}' OR source_id = '{GIST_CHUNK_PREFIX}{0}' \
             OR source_id = '{SECTION_CHUNK_PREFIX}{0}' \
             OR source_id = '{SNOTE_CHUNK_PREFIX}{0}'",
            esc(source_id)
        );
        self.delete_where(T_CHUNKS, &pred).await?;
        self.delete_where(T_SOURCES, &format!("id = '{}'", esc(source_id)))
            .await?;
        Ok(())
    }

    /// Bulk-delete an arbitrary set of sources (docs/RFC-multi-select.md):
    /// two predicate deletes total, whatever the selection size — the
    /// delete_source_tree lesson applied to multi-select. `ids` are the
    /// selected rows (any of which may be folder-like parents whose children
    /// go too — one `parent_id IN` arm covers them all); `child_ids` are
    /// those children, passed separately because their chunk owners must be
    /// enumerated.
    pub async fn delete_sources(&self, ids: &[String], child_ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        // Every owner id whose chunks (verbatim + gist + annotation rows)
        // must go: each selected source plus each child.
        let owners: Vec<&String> = ids.iter().chain(child_ids.iter()).collect();
        let quoted = |prefix: &str| {
            owners
                .iter()
                .map(|id| format!("'{prefix}{}'", esc(id)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let pred = format!(
            "source_id IN ({}) OR source_id IN ({}) OR source_id IN ({}) OR source_id IN ({})",
            quoted(""),
            quoted(GIST_CHUNK_PREFIX),
            quoted(SECTION_CHUNK_PREFIX),
            quoted(SNOTE_CHUNK_PREFIX)
        );
        self.delete_where(T_CHUNKS, &pred).await?;
        // One delete for the selected rows and every row parented to one.
        let id_list = ids
            .iter()
            .map(|id| format!("'{}'", esc(id)))
            .collect::<Vec<_>>()
            .join(", ");
        self.delete_where(
            T_SOURCES,
            &format!("id IN ({id_list}) OR parent_id IN ({id_list})"),
        )
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
        // their prefixed form, matching what the chunk rows store), plus
        // owner recency for the fusion tie-break.
        let mut titles: HashMap<String, String> = HashMap::new();
        let mut recency: HashMap<String, i64> = HashMap::new();
        // Owner id -> on-disk path, for Citation.source_path. Only local
        // paths qualify — web/mac origins stay empty rather than leaking a
        // non-openable URL into a field agents treat as a filesystem handle.
        let mut paths: HashMap<String, String> = HashMap::new();
        for s in self.list_sources(notebook_id).await? {
            recency.insert(s.id.clone(), s.created_at);
            if s.url.starts_with('/') {
                paths.insert(s.id.clone(), s.url.clone());
            }
            // Annotation rows (`snote:<id>`) display their source's title.
            titles.insert(format!("{SNOTE_CHUNK_PREFIX}{}", s.id), s.title.clone());
            titles.insert(s.id, s.title);
        }
        // Titles only — list_notes would drag every note body (reports run
        // tens of KB) through this per-query join.
        for (id, title, created_at) in self.list_note_meta(Some(notebook_id)).await? {
            titles.insert(format!("{NOTE_CHUNK_PREFIX}{id}"), title);
            recency.insert(format!("{NOTE_CHUNK_PREFIX}{id}"), created_at);
        }

        // Gist rows are corpus-wide evidence (meta-chat, MCP search); the
        // per-notebook chat path stays verbatim-passages-only until the
        // citation reader can render a gist hit (RFC-infinite-context §1).
        // Annotation rows (`snote:%`) are deliberately NOT excluded: they're
        // the user's own words, not a machine distillate (RFC-source-tags).
        let mut filter = format!(
            "notebook_id = '{}' AND source_id NOT LIKE '{GIST_CHUNK_PREFIX}%'",
            esc(notebook_id)
        );
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

        let query_vec_for_sections = query_vec.clone();
        let vec_batches = tbl
            .query()
            .only_if(filter.clone())
            .nearest_to(query_vec)?
            .limit(pool)
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        let mut vec_hits = citations_from_batches(&vec_batches, &titles, &paths)?;
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
            // The BM25 document is context + text where the column exists
            // (docs/RFC-outline-index.md Phase 1.5); older tables index text.
            let fts_col = if self.has_chunk_context().await? {
                CHUNK_BM25_COL
            } else {
                "text"
            };
            let text_query = FullTextSearchQuery::new(query_text.to_string())
                .with_column(fts_col.to_string())
                .map_err(|e| anyhow!("fts query: {e}"))?;
            match tbl
                .query()
                .only_if(filter)
                .full_text_search(text_query)
                .limit(pool)
                .execute()
                .await
            {
                Ok(stream) => match stream.try_collect::<Vec<_>>().await {
                    Ok(batches) => citations_from_batches(&batches, &titles, &paths)?,
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

        // Reciprocal rank fusion: score = Σ w/(k + rank) over the lists.
        // Exact score ties are common (e.g. a vector-only and an FTS-only
        // hit at the same rank), and HashMap iteration order is randomized,
        // so break ties by chunk id to keep results stable across runs.
        let (w_vec, rrf_k) = self.fusion();
        let mut fused: HashMap<String, (Citation, f32)> = HashMap::new();
        for (hits, w) in [(&vec_hits, w_vec), (&fts_hits, 1.0)] {
            for (rank, c) in hits.iter().enumerate() {
                fused
                    .entry(c.chunk_id.clone())
                    .or_insert((c.clone(), 0.0))
                    .1 += w / (rrf_k + rank as f32);
            }
        }
        let mut merged: Vec<(Citation, f32)> = fused.into_values().collect();
        let at = |c: &Citation| owner_recency(&recency, c);
        merged.sort_by(|a, b| {
            fused_cmp(
                (a.1, at(&a.0), &a.0.chunk_id),
                (b.1, at(&b.0), &b.0.chunk_id),
            )
        });
        let fused_hits: Vec<Citation> = merged.into_iter().map(|(c, _)| c).collect();
        let fused_hits = self
            .expand_section_hits(
                fused_hits,
                &query_vec_for_sections,
                query_text,
                &titles,
                &paths,
            )
            .await?;
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

    /// The newest page of a notebook's transcript. The metadata pass keeps
    /// large message bodies out of the scan used to choose the page; only the
    /// selected rows are hydrated in the second query.
    pub async fn message_page(
        &self,
        notebook_id: &str,
        before_at: Option<i64>,
        before_id: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<Message>, bool)> {
        let limit = limit.clamp(1, 200);
        let mut filter = format!("notebook_id = '{}'", esc(notebook_id));
        if let Some(ts) = before_at {
            if let Some(id) = before_id {
                filter.push_str(&format!(
                    " AND (created_at < {ts} OR (created_at = {ts} AND id < '{}'))",
                    esc(id)
                ));
            } else {
                filter.push_str(&format!(" AND created_at < {ts}"));
            }
        }
        let batches = self
            .collect_cols(T_MESSAGES, Some(&filter), &["id", "created_at"])
            .await?;
        let mut ids = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                ids.push((id.value(i).to_string(), created.value(i)));
            }
        }
        ids.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
        let has_more = ids.len() > limit;
        ids.truncate(limit);
        if ids.is_empty() {
            return Ok((Vec::new(), false));
        }

        let selected = ids
            .iter()
            .map(|(id, _)| format!("'{}'", esc(id)))
            .collect::<Vec<_>>()
            .join(", ");
        let page_filter = format!(
            "notebook_id = '{}' AND id IN ({selected})",
            esc(notebook_id)
        );
        let batches = self.collect(T_MESSAGES, Some(&page_filter)).await?;
        let mut messages = messages_from_batches(&batches)?;
        messages.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok((messages, has_more))
    }

    pub async fn list_messages(&self, notebook_id: &str) -> Result<Vec<Message>> {
        let filter = format!("notebook_id = '{}'", esc(notebook_id));
        let batches = self.collect(T_MESSAGES, Some(&filter)).await?;
        let mut messages = messages_from_batches(&batches)?;
        messages.sort_by_key(|m| m.created_at);
        Ok(messages)
    }

    /// Replace one message's content in place — the verify-and-repair pass
    /// (RFC-judged-evals §5) swaps a repaired answer under the same id, so
    /// citations, ordering, and history references all survive.
    pub async fn update_message_content(&self, id: &str, content: &str) -> Result<()> {
        let tbl = self.conn.open_table(T_MESSAGES).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("content", format!("'{}'", esc(content)))
            .execute()
            .await?;
        Ok(())
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

    // ---- Home chat (docs/RFC-meta-chat.md) --------------------------------

    /// Add `model` to a `meta_turns` table written before answers recorded
    /// which model wrote them. Additive and atomic (Lance `add_columns`), so
    /// a store the installed app is also reading is never left half-migrated
    /// — the rows it already holds simply carry an empty model, which the UI
    /// renders as nothing.
    async fn migrate_meta_turns(&self) -> Result<()> {
        self.add_string_column(T_META_TURNS, "model", "").await
    }

    pub async fn add_meta_turn(&self, turn: &MetaTurn) -> Result<()> {
        let schema = meta_turns_schema();
        let citations = serde_json::to_string(&turn.citations).unwrap_or_else(|_| "[]".into());
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![turn.id.clone()])),
                Arc::new(StringArray::from(vec![turn.thread_id.clone()])),
                Arc::new(StringArray::from(vec![turn.role.clone()])),
                Arc::new(StringArray::from(vec![turn.content.clone()])),
                Arc::new(StringArray::from(vec![citations])),
                Arc::new(StringArray::from(vec![turn.kind.clone()])),
                Arc::new(StringArray::from(vec![turn.model.clone()])),
                Arc::new(Int64Array::from(vec![turn.created_at])),
            ],
        )?;
        self.add_batch(T_META_TURNS, schema, batch).await
    }

    /// One thread's turns, oldest first — the conversation as it was read.
    ///
    /// The generated-name row (`kind = "title"`) is not a turn: it never
    /// enters the transcript, the model's history, or a turn count.
    pub async fn list_meta_turns(&self, thread_id: &str) -> Result<Vec<MetaTurn>> {
        let filter = format!(
            "thread_id = '{}' AND kind != '{}'",
            esc(thread_id),
            META_TITLE_KIND
        );
        let batches = self.collect(T_META_TURNS, Some(&filter)).await?;
        let mut turns = meta_turns_from_batches(&batches)?;
        turns.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(turns)
    }

    /// Every Home conversation, most recently used first. Threads are derived
    /// from their turns: the metadata pass reads five small columns, and only
    /// the opening question and the generated name of each thread are
    /// hydrated — the answers are the long rows, and none of them is a title.
    ///
    /// A thread is named by its `kind = "title"` row when one exists (the
    /// small model's five words, written once the first exchange settles) and
    /// by its opening question until then. That row is bookkeeping, not
    /// conversation: it counts for nothing and never moves `updated_at`.
    pub async fn list_meta_threads(&self) -> Result<Vec<MetaThread>> {
        let batches = self
            .collect_cols(
                T_META_TURNS,
                None,
                &["id", "thread_id", "role", "kind", "created_at"],
            )
            .await?;
        let mut acc: HashMap<String, ThreadAcc> = HashMap::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let thread = str_col(b, "thread_id")?;
            let role = str_col(b, "role")?;
            let kind = str_col(b, "kind")?;
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                let at = created.value(i);
                let named = kind.value(i) == META_TITLE_KIND;
                let t = acc
                    .entry(thread.value(i).to_string())
                    .or_insert_with(|| ThreadAcc {
                        first_user: None,
                        named: None,
                        count: 0,
                        created_at: at,
                        updated_at: at,
                    });
                if named {
                    // Last writer wins: a re-title supersedes its predecessor
                    // even if the old row outlived the delete.
                    let later = t.named.as_ref().is_none_or(|(_, seen)| at >= *seen);
                    if later {
                        t.named = Some((id.value(i).to_string(), at));
                    }
                    continue;
                }
                t.count += 1;
                t.created_at = t.created_at.min(at);
                t.updated_at = t.updated_at.max(at);
                let earlier = match &t.first_user {
                    None => true,
                    Some((_, seen)) => at < *seen,
                };
                if role.value(i) == "user" && earlier {
                    t.first_user = Some((id.value(i).to_string(), at));
                }
            }
        }
        // A thread that is nothing but a name isn't a conversation — a titling
        // task that outlived its deleted thread must not resurrect it.
        acc.retain(|_, t| t.count > 0);
        if acc.is_empty() {
            return Ok(Vec::new());
        }

        let title_ids = acc
            .values()
            .flat_map(|t| {
                [t.first_user.as_ref(), t.named.as_ref()]
                    .into_iter()
                    .flatten()
                    .map(|(id, _)| format!("'{}'", esc(id)))
            })
            .collect::<Vec<_>>();
        let mut titles: HashMap<String, String> = HashMap::new();
        if !title_ids.is_empty() {
            let filter = format!("id IN ({})", title_ids.join(", "));
            let batches = self
                .collect_cols(T_META_TURNS, Some(&filter), &["id", "content"])
                .await?;
            for b in &batches {
                let id = str_col(b, "id")?;
                let content = str_col(b, "content")?;
                for i in 0..b.num_rows() {
                    titles.insert(id.value(i).to_string(), content.value(i).to_string());
                }
            }
        }

        let mut threads = acc
            .into_iter()
            .map(|(id, t)| {
                let content_of = |slot: &Option<(String, i64)>| {
                    slot.as_ref()
                        .and_then(|(id, _)| titles.get(id))
                        .map(String::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                let question = thread_title(&content_of(&t.first_user));
                let named = content_of(&t.named);
                let named = named.trim();
                MetaThread {
                    id,
                    title: if named.is_empty() {
                        question.clone()
                    } else {
                        named.to_string()
                    },
                    question,
                    turn_count: t.count,
                    created_at: t.created_at,
                    updated_at: t.updated_at,
                }
            })
            .collect::<Vec<_>>();
        threads.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(threads)
    }

    /// The generated name for a thread, if one has been written yet.
    pub async fn meta_thread_title(&self, thread_id: &str) -> Result<Option<String>> {
        let filter = format!(
            "thread_id = '{}' AND kind = '{}'",
            esc(thread_id),
            META_TITLE_KIND
        );
        let batches = self
            .collect_cols(T_META_TURNS, Some(&filter), &["content", "created_at"])
            .await?;
        let mut best: Option<(i64, String)> = None;
        for b in &batches {
            let content = str_col(b, "content")?;
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                let at = created.value(i);
                if best.as_ref().is_none_or(|(seen, _)| at >= *seen) {
                    best = Some((at, content.value(i).trim().to_string()));
                }
            }
        }
        Ok(best.map(|(_, t)| t).filter(|t| !t.is_empty()))
    }

    /// Name a thread. Stored as one more row in the thread's own turns —
    /// no second table, no column migration, and deleting the conversation
    /// takes its name with it.
    pub async fn set_meta_thread_title(&self, thread_id: &str, title: &str) -> Result<()> {
        self.delete_where(
            T_META_TURNS,
            &format!(
                "thread_id = '{}' AND kind = '{}'",
                esc(thread_id),
                META_TITLE_KIND
            ),
        )
        .await?;
        self.add_meta_turn(&MetaTurn {
            id: uuid::Uuid::new_v4().to_string(),
            thread_id: thread_id.to_string(),
            role: "assistant".into(),
            content: title.trim().to_string(),
            citations: Vec::new(),
            kind: META_TITLE_KIND.into(),
            model: String::new(),
            created_at: chrono::Utc::now().timestamp_millis(),
        })
        .await
    }

    pub async fn delete_meta_thread(&self, thread_id: &str) -> Result<()> {
        self.delete_where(T_META_TURNS, &format!("thread_id = '{}'", esc(thread_id)))
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

    /// (id, title, created_at) for notes WITHOUT their bodies — the search
    /// path joins note titles on every query, and generated reports make
    /// note bodies big. `None` = corpus-wide.
    pub async fn list_note_meta(
        &self,
        notebook_id: Option<&str>,
    ) -> Result<Vec<(String, String, i64)>> {
        let filter = notebook_id.map(|nb| format!("notebook_id = '{}'", esc(nb)));
        let batches = match self
            .collect_cols(T_NOTES, filter.as_deref(), &["id", "title", "created_at"])
            .await
        {
            Ok(b) => b,
            Err(_) => self.collect(T_NOTES, filter.as_deref()).await?,
        };
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let title = str_col(b, "title")?;
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                out.push((
                    id.value(i).to_string(),
                    title.value(i).to_string(),
                    created.value(i),
                ));
            }
        }
        Ok(out)
    }

    /// (id, notebook_id, title, created_at) for every source — lightweight
    /// lookups without dragging full content across.
    pub async fn all_source_meta(&self) -> Result<Vec<(String, String, String, i64)>> {
        let batches = self
            .collect_cols(
                T_SOURCES,
                None,
                &["id", "notebook_id", "title", "created_at"],
            )
            .await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let title = str_col(b, "title")?;
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                out.push((
                    id.value(i).to_string(),
                    nb.value(i).to_string(),
                    title.value(i).to_string(),
                    created.value(i),
                ));
            }
        }
        Ok(out)
    }

    /// (notebook_id, role, model, created_at, content words) for every chat
    /// message — tool confirmations excluded. Feeds the Activity view's
    /// read-time aggregation (activity.rs); content is reduced to a word
    /// count here so only counts leave the scan.
    pub async fn message_activity(&self) -> Result<Vec<(String, String, String, i64, i64)>> {
        let batches = self
            .collect_cols(
                T_MESSAGES,
                Some("kind != 'tool'"),
                &["notebook_id", "role", "model", "content", "created_at"],
            )
            .await?;
        let mut out = Vec::new();
        for b in &batches {
            let nb = str_col(b, "notebook_id")?;
            let role = str_col(b, "role")?;
            let model = str_col(b, "model")?;
            let content = str_col(b, "content")?;
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                out.push((
                    nb.value(i).to_string(),
                    role.value(i).to_string(),
                    model.value(i).to_string(),
                    created.value(i),
                    content.value(i).split_whitespace().count() as i64,
                ));
            }
        }
        Ok(out)
    }

    /// created_at for every note — the Activity view only needs the when.
    pub async fn note_activity(&self) -> Result<Vec<i64>> {
        let batches = self.collect_cols(T_NOTES, None, &["created_at"]).await?;
        let mut out = Vec::new();
        for b in &batches {
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                out.push(created.value(i));
            }
        }
        Ok(out)
    }

    /// (source_type, created_at, char_count) for every source — the Activity
    /// view's import series and corpus-size line, without dragging content.
    pub async fn source_activity(&self) -> Result<Vec<(String, i64, i64)>> {
        let batches = self
            .collect_cols(
                T_SOURCES,
                None,
                &["source_type", "created_at", "char_count"],
            )
            .await?;
        let mut out = Vec::new();
        for b in &batches {
            let ty = str_col(b, "source_type")?;
            let created = i64_col(b, "created_at")?;
            let chars = i64_col(b, "char_count")?;
            for i in 0..b.num_rows() {
                out.push((ty.value(i).to_string(), created.value(i), chars.value(i)));
            }
        }
        Ok(out)
    }

    /// Home's activity snapshot in one pass per table. Previously Home read
    /// the full notes table for recent notes, again for reports, and a third
    /// time just to count it.
    pub async fn home_activity(
        &self,
        recent_limit: usize,
        report_limit: usize,
    ) -> Result<(Vec<Note>, Vec<Note>, i64, i64, i64, i64)> {
        let (source_batches, note_batches, ledger_batches) = tokio::try_join!(
            self.collect_cols(T_SOURCES, None, &["char_count"]),
            self.collect(T_NOTES, None),
            self.collect_cols(T_LEDGER, None, &["id"]),
        )?;

        let (mut source_count, mut chars) = (0i64, 0i64);
        for b in &source_batches {
            let cc = i64_col(b, "char_count")?;
            source_count += b.num_rows() as i64;
            for i in 0..b.num_rows() {
                chars += cc.value(i);
            }
        }

        let mut notes = notes_from_batches(&note_batches)?;
        let note_count = notes.len() as i64;
        notes.sort_by_key(|n| std::cmp::Reverse(n.updated_at));
        let reports = notes
            .iter()
            .filter(|n| n.kind == "report")
            .take(report_limit)
            .cloned()
            .collect();
        notes.truncate(recent_limit);
        let ledger_count = ledger_batches.iter().map(|b| b.num_rows() as i64).sum();
        Ok((
            notes,
            reports,
            source_count,
            chars,
            note_count,
            ledger_count,
        ))
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
        let (titles, recency) = self.corpus_meta().await?;
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
        let (w_vec, rrf_k) = self.fusion();
        let mut fused: HashMap<String, ((String, Citation), f32)> = HashMap::new();
        for (hits, w) in [(vec_hits, w_vec), (fts_hits, 1.0)] {
            for (rank, hit) in hits.into_iter().enumerate() {
                fused.entry(hit.1.chunk_id.clone()).or_insert((hit, 0.0)).1 +=
                    w / (rrf_k + rank as f32);
            }
        }
        let mut merged: Vec<((String, Citation), f32)> = fused.into_values().collect();
        let at = |c: &Citation| owner_recency(&recency, c);
        merged.sort_by(|a, b| {
            fused_cmp(
                (a.1, at(&a.0 .1), &a.0 .1.chunk_id),
                (b.1, at(&b.0 .1), &b.0 .1.chunk_id),
            )
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
        // Same uniform title pass as the hybrid path — this previously
        // returned note chunks with empty titles (the known gap from
        // RFC-retrieval-maturity Phase 2).
        let (titles, _) = self.corpus_meta().await?;
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        let fts_col = if self.has_chunk_context().await? {
            CHUNK_BM25_COL
        } else {
            "text"
        };
        let Ok(query) =
            FullTextSearchQuery::new(query_text.to_string()).with_column(fts_col.to_string())
        else {
            return Ok(vec![]);
        };
        let batches = match tbl.query().full_text_search(query).limit(k).execute().await {
            Ok(stream) => stream.try_collect::<Vec<_>>().await.unwrap_or_default(),
            Err(_) => return Ok(vec![]),
        };
        nb_citations_from_batches(&batches, &titles)
    }

    /// Vector search over the gist rows only (docs/RFC-infinite-context.md
    /// Phase 4): the standing distilled-overview layer, retrieved corpus-wide
    /// to seed the global answer route. Restricted to `gist:%` owners, decoded
    /// through the shared title pass (so each hit is titled after its source),
    /// then capped to `MAX_GISTS_PER_NOTEBOOK` per notebook — walked in score
    /// order like `apply_diversity`, kept local and simple — so one chatty
    /// notebook can't own the whole fan-out. Returns (notebook_id, citation).
    pub async fn search_gists(
        &self,
        query_vec: Vec<f32>,
        k: usize,
    ) -> Result<Vec<(String, Citation)>> {
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(vec![]);
        }
        let (titles, _) = self.corpus_meta().await?;
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        let batches = tbl
            .query()
            .only_if(format!("source_id LIKE '{GIST_CHUNK_PREFIX}%'"))
            .nearest_to(query_vec)?
            .limit(k.max(1) * 3)
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        let mut hits = nb_citations_from_batches(&batches, &titles)?;
        hits.sort_by(|a, b| {
            a.1.distance
                .partial_cmp(&b.1.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        const MAX_GISTS_PER_NOTEBOOK: usize = 3;
        let mut per_notebook: HashMap<String, usize> = HashMap::new();
        let mut out: Vec<(String, Citation)> = Vec::with_capacity(k);
        for hit in hits {
            if out.len() >= k {
                break;
            }
            let n = per_notebook.entry(hit.0.clone()).or_default();
            if *n >= MAX_GISTS_PER_NOTEBOOK {
                continue;
            }
            *n += 1;
            out.push(hit);
        }
        Ok(out)
    }

    /// Corpus-wide owner metadata in one sources+notes scan: stored-owner-id
    /// → display title (the uniform title-filling pass shared by all
    /// corpus-wide reads; gist rows display their source's title), plus the
    /// recency map fusion uses as a tie-break. Recency keys: plain source id
    /// (gists resolve to their source before lookup) and `note:<id>`.
    async fn corpus_meta(&self) -> Result<(HashMap<String, String>, HashMap<String, i64>)> {
        let mut titles: HashMap<String, String> = HashMap::new();
        let mut recency: HashMap<String, i64> = HashMap::new();
        for (id, _nb, title, created_at) in self.all_source_meta().await? {
            titles.insert(format!("{GIST_CHUNK_PREFIX}{id}"), title.clone());
            titles.insert(format!("{SNOTE_CHUNK_PREFIX}{id}"), title.clone());
            recency.insert(id.clone(), created_at);
            titles.insert(id, title);
        }
        for (id, title, created_at) in self.list_note_meta(None).await? {
            titles.insert(format!("{NOTE_CHUNK_PREFIX}{id}"), title);
            recency.insert(format!("{NOTE_CHUNK_PREFIX}{id}"), created_at);
        }
        Ok((titles, recency))
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

    /// Drop the whole router index. Routes are derived data — every row is
    /// recomputed from sources, notes, tags and gists by
    /// `router::ensure_router` — so throwing them away costs one re-embed
    /// sweep and never loses anything the corpus doesn't still hold. That is
    /// what makes the dimension self-heal below safe here and NOT safe for
    /// `chunks`, whose text is the only copy of a source's chunking.
    pub async fn clear_all_routes(&self) -> Result<()> {
        if self.table_exists(T_ROUTES).await? {
            self.conn.drop_table(T_ROUTES, &[]).await?;
        }
        Ok(())
    }

    /// The router index's vector width, or None when it hasn't been built.
    pub async fn routes_vector_dim(&self) -> Result<Option<usize>> {
        self.vector_dim(T_ROUTES).await
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
        let Some(stored_dim) = self.routes_vector_dim().await? else {
            return Ok(vec![]);
        };
        // Dimension self-heal. The query vector is the current embedder's
        // width by construction, so comparing it against the stored column is
        // a free, exact drift test — and drift is real: switching embedders
        // and running Re-embed All rebuilt `chunks` at the new width and left
        // `routes` at the old one, after which every routed search died on
        // "No vector column found to match with the query vector dimension".
        // Routes are derived, so the repair is to throw the stale index away;
        // the next `ensure_router` rebuilds it at the current width. Callers
        // treat an empty result as "no index yet" and search flat meanwhile.
        if stored_dim != query_vec.len() {
            crate::note!(
                "router index is {stored_dim}-d but the embedder emits {}-d; \
                 dropping the stale index (it rebuilds on the next sweep)",
                query_vec.len()
            );
            self.clear_all_routes().await?;
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

    /// Bulk-delete notes (docs/RFC-multi-select.md): three predicate deletes
    /// total — indexed chunks, usage rows, note rows — whatever the
    /// selection size, mirroring `delete_sources`.
    pub async fn delete_notes(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let quoted = |prefix: &str| {
            ids.iter()
                .map(|id| format!("'{prefix}{}'", esc(id)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        self.delete_where(
            T_CHUNKS,
            &format!("source_id IN ({})", quoted(NOTE_CHUNK_PREFIX)),
        )
        .await?;
        self.delete_where(T_NOTE_USAGE, &format!("note_id IN ({})", quoted("")))
            .await?;
        self.delete_where(T_NOTES, &format!("id IN ({})", quoted("")))
            .await
    }

    /// Drop a note's chunks from the retrieval index (no-op if unindexed).
    pub async fn delete_note_chunks(&self, note_id: &str) -> Result<()> {
        let pred = format!("source_id = '{NOTE_CHUNK_PREFIX}{}'", esc(note_id));
        self.delete_where(T_CHUNKS, &pred).await
    }

    /// All stored source-gist rows — the staleness baseline for
    /// `gist::ensure_gists`'s diff (mirrors `list_routes` for the router).
    pub async fn list_gists(&self) -> Result<Vec<GistRow>> {
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(vec![]);
        }
        let filter = format!("source_id LIKE '{GIST_CHUNK_PREFIX}%'");
        let batches = self.collect(T_CHUNKS, Some(&filter)).await?;
        let mut out = Vec::new();
        for b in &batches {
            let sid = str_col(b, "source_id")?;
            let ord = i32_col(b, "ordinal")?;
            let text = str_col(b, "text")?;
            for i in 0..b.num_rows() {
                let Some(source_id) = sid.value(i).strip_prefix(GIST_CHUNK_PREFIX) else {
                    continue;
                };
                out.push(GistRow {
                    source_id: source_id.to_string(),
                    hash: ord.value(i),
                    text: text.value(i).to_string(),
                });
            }
        }
        Ok(out)
    }

    /// Replace one source's section-summary rows (RFC-outline-index Phase
    /// 2). `embeddings[i]` embeds `sections[i]`; the summary is the row's
    /// text (its snippet), the chain its context, so BM25 sees both.
    pub async fn replace_section_rows(
        &self,
        notebook_id: &str,
        source_id: &str,
        sections: &[SectionGist],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        self.delete_section_rows(source_id).await?;
        if sections.is_empty() {
            return Ok(());
        }
        let owner = format!("{SECTION_CHUNK_PREFIX}{source_id}");
        let rows: Vec<(String, i32, String)> = sections
            .iter()
            .enumerate()
            .map(|(i, s)| {
                (
                    format!("{owner}:{}-{}", s.start, s.end),
                    i as i32,
                    s.summary.clone(),
                )
            })
            .collect();
        let contexts: Vec<String> = sections.iter().map(|s| s.chain.clone()).collect();
        self.add_chunks_ctx(notebook_id, &owner, &rows, &contexts, embeddings)
            .await
    }

    /// Every section summary in a notebook — the outline the escalation
    /// reads (RFC-outline-index Phase 3): (source_id, chain, summary, start,
    /// end), in source then section order.
    pub async fn notebook_outline(&self, notebook_id: &str) -> Result<Vec<OutlineEntry>> {
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(vec![]);
        }
        let filter = format!(
            "notebook_id = '{}' AND source_id LIKE '{SECTION_CHUNK_PREFIX}%'",
            esc(notebook_id)
        );
        let batches = self
            .collect_cols(
                T_CHUNKS,
                Some(&filter),
                &["id", "source_id", "ordinal", "text", CHUNK_CONTEXT_COL],
            )
            .await?;
        let mut out: Vec<(String, i32, OutlineEntry)> = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let sid = str_col(b, "source_id")?;
            let ord = i32_col(b, "ordinal")?;
            let text = str_col(b, "text")?;
            let ctx = str_col(b, CHUNK_CONTEXT_COL)?;
            for i in 0..b.num_rows() {
                let Some(source_id) = sid.value(i).strip_prefix(SECTION_CHUNK_PREFIX) else {
                    continue;
                };
                let Some((start, end)) = section_range(id.value(i)) else {
                    continue;
                };
                out.push((
                    source_id.to_string(),
                    ord.value(i),
                    OutlineEntry {
                        source_id: source_id.to_string(),
                        chain: ctx.value(i).to_string(),
                        summary: text.value(i).to_string(),
                        start,
                        end,
                    },
                ));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        Ok(out.into_iter().map(|(_, _, e)| e).collect())
    }

    /// The best passages of one section for a question — the escalation's
    /// read after the model picks a section (RFC-outline-index Phase 3).
    pub async fn section_passages(
        &self,
        notebook_id: &str,
        query_vec: &[f32],
        query_text: &str,
        span: (&str, i32, i32),
        n: usize,
    ) -> Result<Vec<Citation>> {
        let mut titles: HashMap<String, String> = HashMap::new();
        let mut paths: HashMap<String, String> = HashMap::new();
        for s in self.list_sources(notebook_id).await? {
            if s.url.starts_with('/') {
                paths.insert(s.id.clone(), s.url.clone());
            }
            titles.insert(s.id, s.title);
        }
        let ranked = self
            .rank_within_section(query_vec, query_text, span, (&titles, &paths))
            .await?;
        Ok(ranked.into_iter().take(n).collect())
    }

    /// Drop one source's section-summary rows (no-op if it has none).
    pub async fn delete_section_rows(&self, source_id: &str) -> Result<()> {
        let pred = format!("source_id = '{SECTION_CHUNK_PREFIX}{}'", esc(source_id));
        self.delete_where(T_CHUNKS, &pred).await
    }

    /// The passages of one section ranked against the question: the same
    /// two legs as the main search, filtered to the section's span, fused
    /// by RRF. Cheap — a span is a handful of rows — and it turns "the
    /// answer is in this section" into "this passage".
    async fn rank_within_section(
        &self,
        query_vec: &[f32],
        query_text: &str,
        span: (&str, i32, i32),
        labels: (&HashMap<String, String>, &HashMap<String, String>),
    ) -> Result<Vec<Citation>> {
        let (source_id, start, end) = span;
        let (titles, paths) = labels;
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(vec![]);
        }
        let filter = format!(
            "source_id = '{}' AND ordinal >= {start} AND ordinal <= {end}",
            esc(source_id)
        );
        let tbl = self.conn.open_table(T_CHUNKS).execute().await?;
        let span = (end - start + 1).max(1) as usize;
        let vec_batches = tbl
            .query()
            .only_if(filter.clone())
            .nearest_to(query_vec.to_vec())?
            .limit(span)
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        let vec_hits = citations_from_batches(&vec_batches, titles, paths)?;
        #[cfg(test)]
        if std::env::var("ALCHEMY_TRACE_SECTIONS").is_ok() {
            eprintln!(
                "    span vector leg: {:?}",
                vec_hits.iter().map(|p| p.ordinal).collect::<Vec<_>>()
            );
        }
        let fts_hits = if query_text.trim().is_empty() {
            vec![]
        } else {
            let col = if self.has_chunk_context().await? {
                CHUNK_BM25_COL
            } else {
                "text"
            };
            match FullTextSearchQuery::new(query_text.to_string()).with_column(col.to_string()) {
                Ok(q) => match tbl
                    .query()
                    .only_if(filter)
                    .full_text_search(q)
                    .limit(span)
                    .execute()
                    .await
                {
                    Ok(stream) => citations_from_batches(
                        &stream.try_collect::<Vec<_>>().await.unwrap_or_default(),
                        titles,
                        paths,
                    )?,
                    Err(_) => vec![],
                },
                Err(_) => vec![],
            }
        };
        let (w_vec, rrf_k) = self.fusion();
        let mut fused: HashMap<String, (Citation, f32)> = HashMap::new();
        for (hits, w) in [(&vec_hits, w_vec), (&fts_hits, 1.0)] {
            for (rank, c) in hits.iter().enumerate() {
                fused
                    .entry(c.chunk_id.clone())
                    .or_insert((c.clone(), 0.0))
                    .1 += w / (rrf_k + rank as f32);
            }
        }
        let mut merged: Vec<(Citation, f32)> = fused.into_values().collect();
        merged.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.ordinal.cmp(&b.0.ordinal))
        });
        Ok(merged.into_iter().map(|(c, _)| c).collect())
    }

    /// Swap each section-summary hit for the passages it stands for, in
    /// place, best-matching first (RFC-outline-index Phase 2). The summary
    /// found the section; the reader and the prompt want the verbatim
    /// text, which is also what click-to-highlight and the retrieval evals
    /// match. A passage the section vouches for takes the section's rank
    /// even if the flat search had it further down — that promotion is the
    /// whole point (leaving it "at its own rank" kept the wanted passage at
    /// 25 behind a summary at 1). A passage already placed higher stays.
    async fn expand_section_hits(
        &self,
        hits: Vec<Citation>,
        query_vec: &[f32],
        query_text: &str,
        titles: &HashMap<String, String>,
        paths: &HashMap<String, String>,
    ) -> Result<Vec<Citation>> {
        if !hits.iter().any(|c| section_range(&c.chunk_id).is_some()) {
            return Ok(hits);
        }
        // Ids already emitted (in either capacity); a later flat hit for a
        // promoted passage is skipped, not duplicated.
        let mut emitted: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out: Vec<Citation> = Vec::with_capacity(hits.len());
        // Every section hit expands, wherever it sits: gating on the row's
        // rank (top 2, 3, 5, 10) measured no better than none.
        for hit in hits {
            let Some((start, end)) = section_range(&hit.chunk_id) else {
                if emitted.insert(hit.chunk_id.clone()) {
                    out.push(hit);
                }
                continue;
            };
            let passages = self
                .rank_within_section(
                    query_vec,
                    query_text,
                    (&hit.source_id, start, end),
                    (titles, paths),
                )
                .await?;
            #[cfg(test)]
            if std::env::var("ALCHEMY_TRACE_SECTIONS").is_ok() {
                eprintln!(
                    "  section {}..={} of {} → {:?}",
                    start,
                    end,
                    hit.section,
                    passages.iter().map(|p| p.ordinal).collect::<Vec<_>>()
                );
            }
            for mut p in passages.into_iter().take(section_expand_max()) {
                if emitted.insert(p.chunk_id.clone()) {
                    // The summary's rank is the passage's rank.
                    p.distance = hit.distance;
                    out.push(p);
                }
            }
        }
        Ok(out)
    }

    /// Drop one source's gist row (no-op if it has none).
    pub async fn delete_gist_row(&self, source_id: &str) -> Result<()> {
        let pred = format!("source_id = '{GIST_CHUNK_PREFIX}{}'", esc(source_id));
        self.delete_where(T_CHUNKS, &pred).await
    }

    /// One source's stored chunk rows as (chunk_id, ordinal, text), in ordinal
    /// order — the enrichment pass (RFC-infinite-context §2) reads these to
    /// re-embed with identical ids, ordinals, and verbatim text. The equality
    /// filter can't match the source's `gist:`/`note:` rows (those owner ids
    /// are prefixed), so only the plain source chunks come back.
    pub async fn source_chunk_rows(&self, source_id: &str) -> Result<Vec<(String, i32, String)>> {
        if !self.table_exists(T_CHUNKS).await? {
            return Ok(vec![]);
        }
        let filter = format!("source_id = '{}'", esc(source_id));
        let batches = self.collect(T_CHUNKS, Some(&filter)).await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let ord = i32_col(b, "ordinal")?;
            let text = str_col(b, "text")?;
            for i in 0..b.num_rows() {
                out.push((
                    id.value(i).to_string(),
                    ord.value(i),
                    text.value(i).to_string(),
                ));
            }
        }
        out.sort_by_key(|(_, ord, _)| *ord);
        Ok(out)
    }

    /// Replace the vectors of one source's chunks in place: same ids, ordinals,
    /// and verbatim text — only the embeddings change (RFC-infinite-context §2
    /// enrichment). LanceDB has no vector-cell update, so this is
    /// delete-then-add of the exact same rows; the FTS `text` is unchanged, so
    /// the rebuilt index is identical. Only the plain source chunks are
    /// touched — the `gist:` row (prefixed owner id) is left in place.
    pub async fn reembed_source_chunks(
        &self,
        notebook_id: &str,
        source_id: &str,
        chunks: &[(String, i32, String)],
        contexts: &[String],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        self.delete_where(T_CHUNKS, &format!("source_id = '{}'", esc(source_id)))
            .await?;
        self.add_chunks_ctx(notebook_id, source_id, chunks, contexts, embeddings)
            .await
    }

    /// Post-rank neighbor expansion (RFC-infinite-context §3): for each
    /// cited source chunk, widen the PROMPT excerpt to include its ordinal
    /// ±1 neighbors so a section split by chunking reaches the model whole.
    /// Returns chunk_id → expanded text for the citations that grew;
    /// persisted citations keep their verbatim snippet (click-to-highlight
    /// depends on it) — only prompt assembly reads this map. Higher-ranked
    /// citations claim neighbors first, and an ordinal already cited (or
    /// claimed) is never included twice.
    pub async fn expand_neighbor_excerpts(
        &self,
        citations: &[Citation],
    ) -> Result<HashMap<String, String>> {
        let mut claimed: HashMap<&str, std::collections::HashSet<i32>> = HashMap::new();
        for c in citations {
            if c.note_id.is_empty() && !c.gist && !c.source_id.is_empty() {
                claimed.entry(&c.source_id).or_default().insert(c.ordinal);
            }
        }
        let mut out = HashMap::new();
        for c in citations {
            // Notes and gists have no meaningful neighbors; grep/read
            // pseudo-citations carry ordinals that aren't chunk positions.
            if !c.note_id.is_empty() || c.gist || c.source_id.is_empty() {
                continue;
            }
            if c.chunk_id.starts_with("grep:") || c.chunk_id.starts_with("read:") {
                continue;
            }
            let taken = claimed.entry(&c.source_id).or_default();
            let want: Vec<i32> = [c.ordinal - 1, c.ordinal + 1]
                .into_iter()
                .filter(|o| *o >= 0 && !taken.contains(o))
                .collect();
            if want.is_empty() {
                continue;
            }
            let texts = self.chunk_texts_by_ordinal(&c.source_id, &want).await?;
            if texts.is_empty() {
                continue;
            }
            for o in texts.keys() {
                taken.insert(*o);
            }
            let mut parts: Vec<&str> = Vec::with_capacity(3);
            if let Some(prev) = texts.get(&(c.ordinal - 1)) {
                parts.push(prev);
            }
            parts.push(&c.snippet);
            if let Some(next) = texts.get(&(c.ordinal + 1)) {
                parts.push(next);
            }
            if parts.len() > 1 {
                out.insert(c.chunk_id.clone(), parts.join("\n\n"));
            }
        }
        Ok(out)
    }

    /// Verbatim chunk texts for specific ordinals of one source — the
    /// neighbor-expansion fetch. Gist rows can't collide: their stored
    /// owner id is prefixed, so the equality filter never matches them.
    async fn chunk_texts_by_ordinal(
        &self,
        source_id: &str,
        ordinals: &[i32],
    ) -> Result<HashMap<i32, String>> {
        if ordinals.is_empty() || !self.table_exists(T_CHUNKS).await? {
            return Ok(HashMap::new());
        }
        let list = ordinals
            .iter()
            .map(|o| o.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let filter = format!("source_id = '{}' AND ordinal IN ({list})", esc(source_id));
        let batches = self.collect(T_CHUNKS, Some(&filter)).await?;
        let mut out = HashMap::new();
        for b in &batches {
            let ord = i32_col(b, "ordinal")?;
            let text = str_col(b, "text")?;
            for i in 0..b.num_rows() {
                out.insert(ord.value(i), text.value(i).to_string());
            }
        }
        Ok(out)
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
            let trigger = opt_str_col(b, "trigger");
            let not_before = opt_i64_col(b, "not_before");
            let interval = i64_col(b, "interval_secs")?;
            let enabled = i64_col(b, "enabled")?;
            let last = i64_col(b, "last_run_at")?;
            let created = i64_col(b, "created_at")?;
            let watch_sources = opt_str_col(b, "watch_sources");
            let watch_kinds = opt_str_col(b, "watch_kinds");
            let opt = |col: Option<&StringArray>, i: usize| {
                col.map(|c| c.value(i).to_string()).unwrap_or_default()
            };
            for i in 0..b.num_rows() {
                out.push(ReportSchedule {
                    watch_sources: opt(watch_sources, i),
                    watch_kinds: opt(watch_kinds, i),
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    name: name.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    prompt: prompt.value(i).to_string(),
                    trigger: trigger
                        .as_ref()
                        .map(|c| c.value(i).to_string())
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| "interval".to_string()),
                    not_before: not_before.as_ref().map(|c| c.value(i)).unwrap_or(0),
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

    pub async fn add_source_event(&self, event: &SourceEvent) -> Result<()> {
        let schema = source_events_schema();
        let batch = source_event_batch(&schema, event)?;
        self.add_batch(T_SOURCE_EVENTS, schema, batch).await?;
        // The one write point doubles as the live stream's fan-out
        // (docs/RFC-events.md §8): agents tailing /events see it now.
        crate::events::publish(event);
        Ok(())
    }

    /// Events newer than `since`, newest first. Prunes the rolling window on
    /// the way in — callers are periodic (the Brief, agents), so the table
    /// stays bounded without a dedicated sweep.
    pub async fn source_events_since(&self, since: i64) -> Result<Vec<SourceEvent>> {
        let cutoff = crate::commands::now() - SOURCE_EVENT_WINDOW_MS;
        self.delete_where(T_SOURCE_EVENTS, &format!("at < {cutoff}"))
            .await?;
        let batches = self
            .collect(T_SOURCE_EVENTS, Some(&format!("at > {since}")))
            .await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let sid = str_col(b, "source_id")?;
            let title = str_col(b, "source_title")?;
            let kind = str_col(b, "kind")?;
            let detail = str_col(b, "detail")?;
            let diff = str_col(b, "diff")?;
            let at = i64_col(b, "at")?;
            for i in 0..b.num_rows() {
                out.push(SourceEvent {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    source_id: sid.value(i).to_string(),
                    source_title: title.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    detail: detail.value(i).to_string(),
                    diff: diff.value(i).to_string(),
                    at: at.value(i),
                });
            }
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.at));
        Ok(out)
    }

    // ---- App state (key → value) -------------------------------------------

    /// One value by key; `None` when unset (or before the table exists).
    pub async fn kv_get(&self, key: &str) -> Result<Option<String>> {
        if !self.table_exists(T_KV).await? {
            return Ok(None);
        }
        let batches = self
            .collect(T_KV, Some(&format!("key = '{}'", esc(key))))
            .await?;
        for b in &batches {
            if b.num_rows() > 0 {
                let value = str_col(b, "value")?;
                return Ok(Some(value.value(0).to_string()));
            }
        }
        Ok(None)
    }

    /// Set (replace) one value. Delete-then-append: the table is tiny and
    /// a key is written far less often than it is read.
    pub async fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        // Created on first write: unlike the entity tables, nothing needs
        // this one at startup.
        self.ensure_table(T_KV, kv_schema()).await?;
        self.delete_where(T_KV, &format!("key = '{}'", esc(key)))
            .await?;
        let schema = kv_schema();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![key.to_string()])),
                Arc::new(StringArray::from(vec![value.to_string()])),
                Arc::new(Int64Array::from(vec![crate::commands::now()])),
            ],
        )?;
        self.add_batch(T_KV, schema, batch).await
    }

    /// Record one run. Best-effort by contract: a receipt that fails to
    /// write must never fail the run it describes, so callers log and move on.
    pub async fn add_receipt(&self, receipt: &RunReceipt) -> Result<()> {
        let schema = receipts_schema();
        let batch = receipt_batch(&schema, receipt)?;
        self.add_batch(T_RECEIPTS, schema, batch).await
    }

    /// Receipts newest first, pruning the rolling window on the way in — the
    /// same self-bounding idiom as `source_events_since`, so the table needs
    /// no dedicated sweep. `since` is a wall-clock floor; `limit` caps the
    /// read for the UI's "last night" grouping.
    pub async fn list_receipts(&self, since: i64, limit: usize) -> Result<Vec<RunReceipt>> {
        let cutoff = crate::commands::now() - RECEIPT_WINDOW_MS;
        self.delete_where(T_RECEIPTS, &format!("ended_at < {cutoff}"))
            .await?;
        let batches = self
            .collect(T_RECEIPTS, Some(&format!("ended_at >= {since}")))
            .await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let sched = str_col(b, "schedule_id")?;
            let nb = str_col(b, "notebook_id")?;
            let name = str_col(b, "name")?;
            let kind = str_col(b, "kind")?;
            let trigger = str_col(b, "trigger")?;
            let status = str_col(b, "status")?;
            let detail = str_col(b, "detail")?;
            let error = str_col(b, "error")?;
            let note_id = str_col(b, "note_id")?;
            let provider = str_col(b, "provider")?;
            let model = str_col(b, "model")?;
            let cost = i64_col(b, "cost_micros")?;
            // Optional: receipts written before lateness was recorded.
            let due = opt_i64_col(b, "due_at");
            let started = i64_col(b, "started_at")?;
            let ended = i64_col(b, "ended_at")?;
            for i in 0..b.num_rows() {
                out.push(RunReceipt {
                    id: id.value(i).to_string(),
                    schedule_id: sched.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    name: name.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    trigger: trigger.value(i).to_string(),
                    status: status.value(i).to_string(),
                    detail: detail.value(i).to_string(),
                    error: error.value(i).to_string(),
                    note_id: note_id.value(i).to_string(),
                    provider: provider.value(i).to_string(),
                    model: model.value(i).to_string(),
                    cost_micros: cost.value(i),
                    due_at: due.as_ref().map(|c| c.value(i)).unwrap_or(0),
                    started_at: started.value(i),
                    ended_at: ended.value(i),
                });
            }
        }
        out.sort_by_key(|r| std::cmp::Reverse(r.ended_at));
        out.truncate(limit);
        Ok(out)
    }

    /// Every receipt for one schedule, newest first — the run history behind
    /// a standing order.
    pub async fn receipts_for_schedule(
        &self,
        schedule_id: &str,
        limit: usize,
    ) -> Result<Vec<RunReceipt>> {
        let all = self.list_receipts(0, usize::MAX).await?;
        Ok(all
            .into_iter()
            .filter(|r| r.schedule_id == schedule_id)
            .take(limit)
            .collect())
    }

    pub async fn add_ledger_entry(&self, entry: &LedgerEntry) -> Result<()> {
        let schema = ledger_schema();
        let batch = ledger_batch(&schema, entry)?;
        self.add_batch(T_LEDGER, schema, batch).await
    }

    /// How many ledger rows the Weave moved to a contradicted or superseded
    /// state since `since` (freshness.rs). Counted from persisted status
    /// rather than instrumented at the call site, the same way due-ness is
    /// derived rather than queued.
    pub async fn ledger_upsets_since(&self, since: i64) -> Result<u32> {
        let batches = self
            .collect(T_LEDGER, Some(&format!("updated_at >= {since}")))
            .await?;
        let mut count = 0;
        for b in &batches {
            let status = str_col(b, "status")?;
            let updated = i64_col(b, "updated_at")?;
            let created = i64_col(b, "created_at")?;
            for i in 0..b.num_rows() {
                // A row that arrived already contradicted was minted that
                // way; only a row the night CHANGED is a finding.
                if updated.value(i) == created.value(i) {
                    continue;
                }
                if matches!(status.value(i), "contradicted" | "superseded") {
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    pub async fn list_ledger(&self, notebook_id: &str) -> Result<Vec<LedgerEntry>> {
        let batches = self
            .collect(
                T_LEDGER,
                Some(&format!("notebook_id = '{}'", esc(notebook_id))),
            )
            .await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let nb = str_col(b, "notebook_id")?;
            let kind = str_col(b, "kind")?;
            let text = str_col(b, "text")?;
            let why = str_col(b, "why")?;
            let status = str_col(b, "status")?;
            let origin = opt_str_col(b, "origin");
            let anchors = str_col(b, "anchors")?;
            let created = i64_col(b, "created_at")?;
            let updated = i64_col(b, "updated_at")?;
            for i in 0..b.num_rows() {
                out.push(LedgerEntry {
                    id: id.value(i).to_string(),
                    notebook_id: nb.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    text: text.value(i).to_string(),
                    why: why.value(i).to_string(),
                    status: status.value(i).to_string(),
                    origin: origin
                        .as_ref()
                        .map(|c| c.value(i).to_string())
                        .unwrap_or_default(),
                    anchors: serde_json::from_str(anchors.value(i)).unwrap_or_default(),
                    created_at: created.value(i),
                    updated_at: updated.value(i),
                });
            }
        }
        // Newest first — the ledger reads as a record, latest on top.
        out.sort_by_key(|e| std::cmp::Reverse(e.created_at));
        Ok(out)
    }

    pub async fn get_ledger_entry(&self, id: &str) -> Result<Option<LedgerEntry>> {
        let batches = self
            .collect(T_LEDGER, Some(&format!("id = '{}'", esc(id))))
            .await?;
        for b in &batches {
            if b.num_rows() > 0 {
                let anchors = str_col(b, "anchors")?;
                return Ok(Some(LedgerEntry {
                    id: str_col(b, "id")?.value(0).to_string(),
                    notebook_id: str_col(b, "notebook_id")?.value(0).to_string(),
                    kind: str_col(b, "kind")?.value(0).to_string(),
                    text: str_col(b, "text")?.value(0).to_string(),
                    why: str_col(b, "why")?.value(0).to_string(),
                    status: str_col(b, "status")?.value(0).to_string(),
                    origin: opt_str_col(b, "origin")
                        .as_ref()
                        .map(|c| c.value(0).to_string())
                        .unwrap_or_default(),
                    anchors: serde_json::from_str(anchors.value(0)).unwrap_or_default(),
                    created_at: i64_col(b, "created_at")?.value(0),
                    updated_at: i64_col(b, "updated_at")?.value(0),
                }));
            }
        }
        Ok(None)
    }

    /// Update text/why/status in place; anchors travel whole as JSON.
    pub async fn update_ledger_entry(&self, entry: &LedgerEntry) -> Result<()> {
        let tbl = self.conn.open_table(T_LEDGER).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(&entry.id)))
            .column("text", format!("'{}'", esc(&entry.text)))
            .column("why", format!("'{}'", esc(&entry.why)))
            .column("status", format!("'{}'", esc(&entry.status)))
            .column(
                "anchors",
                format!(
                    "'{}'",
                    esc(&serde_json::to_string(&entry.anchors).unwrap_or_default())
                ),
            )
            .column("updated_at", entry.updated_at.to_string())
            .execute()
            .await?;
        Ok(())
    }

    pub async fn delete_ledger_entry(&self, id: &str) -> Result<()> {
        self.delete_where(T_LEDGER, &format!("id = '{}'", esc(id)))
            .await
    }

    // ---- Registry (docs/RFC-registry.md) ----

    pub async fn add_registry_card(&self, card: &RegistryCard) -> Result<()> {
        let schema = registry_schema();
        let batch = registry_batch(&schema, card)?;
        self.add_batch(T_REGISTRY, schema, batch).await
    }

    /// The whole cast. Corpus-scoped by construction — there is no notebook
    /// filter to pass, and callers that want one derive it from
    /// `attachments`.
    pub async fn list_registry(&self) -> Result<Vec<RegistryCard>> {
        let batches = self.collect(T_REGISTRY, None).await?;
        let mut out = Vec::new();
        for b in &batches {
            let id = str_col(b, "id")?;
            let kind = str_col(b, "kind")?;
            let name = str_col(b, "name")?;
            let origin = opt_str_col(b, "origin");
            let triage = opt_str_col(b, "triage");
            let identifiers = str_col(b, "identifiers")?;
            let note = str_col(b, "note")?;
            let facts = str_col(b, "facts")?;
            let attachments = str_col(b, "attachments")?;
            let created = i64_col(b, "created_at")?;
            let updated = i64_col(b, "updated_at")?;
            for i in 0..b.num_rows() {
                out.push(RegistryCard {
                    id: id.value(i).to_string(),
                    kind: kind.value(i).to_string(),
                    name: name.value(i).to_string(),
                    origin: origin
                        .as_ref()
                        .map(|c| c.value(i).to_string())
                        .unwrap_or_default(),
                    triage: triage
                        .as_ref()
                        .map(|c| c.value(i).to_string())
                        .unwrap_or_default(),
                    identifiers: identifiers.value(i).to_string(),
                    note: note.value(i).to_string(),
                    facts: serde_json::from_str(facts.value(i)).unwrap_or_default(),
                    attachments: serde_json::from_str(attachments.value(i)).unwrap_or_default(),
                    created_at: created.value(i),
                    updated_at: updated.value(i),
                });
            }
        }
        // Alphabetical — a cast is a list of names, not a feed.
        out.sort_by_key(|c| c.name.to_lowercase());
        Ok(out)
    }

    pub async fn get_registry_card(&self, id: &str) -> Result<Option<RegistryCard>> {
        let batches = self
            .collect(T_REGISTRY, Some(&format!("id = '{}'", esc(id))))
            .await?;
        for b in &batches {
            if b.num_rows() > 0 {
                return Ok(Some(RegistryCard {
                    id: str_col(b, "id")?.value(0).to_string(),
                    kind: str_col(b, "kind")?.value(0).to_string(),
                    name: str_col(b, "name")?.value(0).to_string(),
                    origin: opt_str_col(b, "origin")
                        .as_ref()
                        .map(|c| c.value(0).to_string())
                        .unwrap_or_default(),
                    triage: opt_str_col(b, "triage")
                        .as_ref()
                        .map(|c| c.value(0).to_string())
                        .unwrap_or_default(),
                    identifiers: str_col(b, "identifiers")?.value(0).to_string(),
                    note: str_col(b, "note")?.value(0).to_string(),
                    facts: serde_json::from_str(str_col(b, "facts")?.value(0)).unwrap_or_default(),
                    attachments: serde_json::from_str(str_col(b, "attachments")?.value(0))
                        .unwrap_or_default(),
                    created_at: i64_col(b, "created_at")?.value(0),
                    updated_at: i64_col(b, "updated_at")?.value(0),
                }));
            }
        }
        Ok(None)
    }

    /// Update everything mutable in place; facts and attachments travel
    /// whole as JSON. `kind` is immutable by construction — a card that
    /// changes kind is a different thing.
    pub async fn update_registry_card(&self, card: &RegistryCard) -> Result<()> {
        let tbl = self.conn.open_table(T_REGISTRY).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(&card.id)))
            .column("name", format!("'{}'", esc(&card.name)))
            .column("origin", format!("'{}'", esc(&card.origin)))
            .column("triage", format!("'{}'", esc(&card.triage)))
            .column("identifiers", format!("'{}'", esc(&card.identifiers)))
            .column("note", format!("'{}'", esc(&card.note)))
            .column(
                "facts",
                format!(
                    "'{}'",
                    esc(&serde_json::to_string(&card.facts).unwrap_or_default())
                ),
            )
            .column(
                "attachments",
                format!(
                    "'{}'",
                    esc(&serde_json::to_string(&card.attachments).unwrap_or_default())
                ),
            )
            .column("updated_at", card.updated_at.to_string())
            .execute()
            .await?;
        Ok(())
    }

    pub async fn delete_registry_card(&self, id: &str) -> Result<()> {
        self.delete_where(T_REGISTRY, &format!("id = '{}'", esc(id)))
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_report_schedule(
        &self,
        id: &str,
        name: &str,
        kind: &str,
        prompt: &str,
        trigger: &str,
        interval_secs: i64,
        enabled: bool,
        watch_sources: &str,
        watch_kinds: &str,
    ) -> Result<()> {
        let tbl = self.conn.open_table(T_REPORTS).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
            .column("name", format!("'{}'", esc(name)))
            .column("kind", format!("'{}'", esc(kind)))
            .column("prompt", format!("'{}'", esc(prompt)))
            .column("trigger", format!("'{}'", esc(trigger)))
            .column("interval_secs", interval_secs.to_string())
            .column("enabled", i64::from(enabled).to_string())
            .column("watch_sources", format!("'{}'", esc(watch_sources)))
            .column("watch_kinds", format!("'{}'", esc(watch_kinds)))
            .execute()
            .await?;
        Ok(())
    }

    /// Retire a commission after it runs (docs/RFC-night-shift-area.md §1).
    /// The row stays as its own history — the receipt points at it — but it
    /// will never come due again.
    pub async fn set_report_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let tbl = self.conn.open_table(T_REPORTS).execute().await?;
        tbl.update()
            .only_if(format!("id = '{}'", esc(id)))
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

/// Decode message-table batches into transcript rows.
fn messages_from_batches(batches: &[RecordBatch]) -> Result<Vec<Message>> {
    let mut messages = Vec::new();
    for b in batches {
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
            messages.push(Message {
                id: id.value(i).to_string(),
                notebook_id: nb.value(i).to_string(),
                role: role.value(i).to_string(),
                content: content.value(i).to_string(),
                citations: serde_json::from_str(citations.value(i)).unwrap_or_default(),
                kind: kind.value(i).to_string(),
                model: model.map(|m| m.value(i).to_string()).unwrap_or_default(),
                created_at: created.value(i),
            });
        }
    }
    Ok(messages)
}

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
        // Meta-search spans every notebook; corpus_meta carries no paths,
        // so cross-notebook hits leave source_path empty for now.
        let citations = citations_from_batches(std::slice::from_ref(b), titles, &HashMap::new())?;
        for (i, c) in citations.into_iter().enumerate() {
            out.push((nb.value(i).to_string(), c));
        }
    }
    Ok(out)
}

/// Fused-pool ordering (RFC-infinite-context §3): RRF score descending,
/// then owner recency descending — when two hits score identically (the
/// classic case: a vector-only and an FTS-only hit at the same rank), the
/// newer owner answers — then chunk id ascending as the deterministic
/// floor. Extracted so the rule is unit-testable; both fusion paths use it.
fn fused_cmp(a: (f32, i64, &str), b: (f32, i64, &str)) -> std::cmp::Ordering {
    b.0.partial_cmp(&a.0)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| b.1.cmp(&a.1))
        .then_with(|| a.2.cmp(b.2))
}

/// A hit's owner recency for the fusion tie-break. Notes key by their
/// prefixed form; gists were resolved to their source id at decode time, so
/// they inherit the source's timestamp. Unknown owners sort oldest.
fn owner_recency(recency: &HashMap<String, i64>, c: &Citation) -> i64 {
    if c.note_id.is_empty() {
        recency.get(&c.source_id).copied().unwrap_or(0)
    } else {
        recency
            .get(&format!("{NOTE_CHUNK_PREFIX}{}", c.note_id))
            .copied()
            .unwrap_or(0)
    }
}

/// Decode a stored chunk owner id into (source_id, note_id, is_gist,
/// is_snote). Note rows set `note_id`; gist and snote rows resolve to their
/// source id with the matching flag set; plain rows are just the source id.
fn split_owner(stored: &str) -> (String, String, bool, bool) {
    if let Some(source_id) = stored.strip_prefix(SNOTE_CHUNK_PREFIX) {
        return (source_id.to_string(), String::new(), false, true);
    }
    if let Some(note_id) = stored.strip_prefix(NOTE_CHUNK_PREFIX) {
        return (String::new(), note_id.to_string(), false, false);
    }
    if let Some(source_id) = stored.strip_prefix(GIST_CHUNK_PREFIX) {
        return (source_id.to_string(), String::new(), true, false);
    }
    // Section rows are gists of a part: same caps, same "distillate, not
    // passage" treatment everywhere a gist gets it.
    if let Some(source_id) = stored.strip_prefix(SECTION_CHUNK_PREFIX) {
        return (source_id.to_string(), String::new(), true, false);
    }
    (stored.to_string(), String::new(), false, false)
}

fn citations_from_batches(
    batches: &[RecordBatch],
    titles: &HashMap<String, String>,
    paths: &HashMap<String, String>,
) -> Result<Vec<Citation>> {
    let mut citations = Vec::new();
    for b in batches {
        let id = str_col(b, "id")?;
        let sid = str_col(b, "source_id")?;
        let ord = i32_col(b, "ordinal")?;
        let text = str_col(b, "text")?;
        let ctx = b
            .column_by_name(CHUNK_CONTEXT_COL)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let dist = b.column_by_name("_distance").and_then(|c| {
            c.as_any()
                .downcast_ref::<arrow_array::Float32Array>()
                .cloned()
        });
        for i in 0..b.num_rows() {
            let stored = sid.value(i).to_string();
            let (source_id, note_id, gist, snote) = split_owner(&stored);
            citations.push(Citation {
                chunk_id: id.value(i).to_string(),
                source_title: titles.get(&stored).cloned().unwrap_or_default(),
                // Keyed by the bare source id: note/gist-prefixed owners have
                // no file on disk, so their lookups miss and stay empty.
                source_path: paths.get(&source_id).cloned().unwrap_or_default(),
                source_id,
                note_id,
                gist,
                snote,
                ordinal: ord.value(i),
                snippet: text.value(i).to_string(),
                distance: dist.as_ref().map(|d| d.value(i)).unwrap_or(0.0),
                section: ctx.map(|c| c.value(i).to_string()).unwrap_or_default(),
            });
        }
    }
    Ok(citations)
}

/// Keep one notebook per id: an active row beats an archived one, then the
/// later `updated_at`. Order of the survivors is the order they arrived in.
fn dedupe_notebooks(notebooks: &mut Vec<Notebook>) {
    let mut best: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, n) in notebooks.iter().enumerate() {
        match best.get(&n.id) {
            None => {
                best.insert(n.id.clone(), i);
            }
            Some(&j) => {
                let cur = &notebooks[j];
                let cur_active = cur.status != "archived";
                let new_active = n.status != "archived";
                let wins = match (cur_active, new_active) {
                    (false, true) => true,
                    (true, false) => false,
                    _ => n.updated_at > cur.updated_at,
                };
                if wins {
                    best.insert(n.id.clone(), i);
                }
            }
        }
    }
    let mut keep: Vec<usize> = best.into_values().collect();
    keep.sort_unstable();
    let mut idx = 0;
    notebooks.retain(|_| {
        let k = keep.binary_search(&idx).is_ok();
        idx += 1;
        k
    });
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
            s(|x| x.icon.clone()),
            s(|x| x.status.clone()),
            i(|x| x.growth_web as i64),
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
            s(|x| x.author.clone()),
            s(|x| x.image_url.clone()),
            s(|x| x.tags.clone()),
            s(|x| x.note.clone()),
            i(|x| x.fetched_at),
            i(|x| x.fetch_failures),
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

/// Rebuild `batch` in the live table's shape when they differ: columns the
/// table has but the batch lacks get default values, matching-name columns
/// carry over in the table's order. Falls back to the original pair when a
/// missing column's type can't be synthesized (e.g. vectors) or when
/// anything about the rebuild fails — the plain append then reports the real
/// mismatch. See `add_batch` for why this exists (shared dev/prod store).
fn conform_to_live(
    live: &SchemaRef,
    ours: SchemaRef,
    batch: RecordBatch,
) -> (SchemaRef, RecordBatch) {
    let same = live.fields().len() == ours.fields().len()
        && live
            .fields()
            .iter()
            .zip(ours.fields().iter())
            .all(|(a, b)| a.name() == b.name());
    if same {
        return (ours, batch);
    }
    let rows = batch.num_rows();
    let mut cols: Vec<ArrayRef> = Vec::with_capacity(live.fields().len());
    for field in live.fields() {
        if let Some((idx, our_field)) = ours.column_with_name(field.name()) {
            if our_field.data_type() != field.data_type() {
                return (ours, batch); // type drift — let the append report it
            }
            cols.push(batch.column(idx).clone());
            continue;
        }
        let filler: ArrayRef = match field.data_type() {
            DataType::Utf8 => Arc::new(StringArray::from(vec![""; rows])),
            DataType::Int64 => Arc::new(Int64Array::from(vec![0i64; rows])),
            DataType::Int32 => Arc::new(Int32Array::from(vec![0i32; rows])),
            DataType::Float32 => Arc::new(arrow_array::Float32Array::from(vec![0f32; rows])),
            DataType::Boolean => Arc::new(arrow_array::BooleanArray::from(vec![false; rows])),
            _ => return (ours, batch), // vectors etc. — nothing sane to invent
        };
        cols.push(filler);
    }
    match RecordBatch::try_new(live.clone(), cols) {
        Ok(conformed) => (live.clone(), conformed),
        Err(_) => (ours, batch),
    }
}

// ---- Schemas -------------------------------------------------------------

fn notebooks_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
        Field::new("color", DataType::Utf8, false),
        Field::new("icon", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("growth_web", DataType::Int64, false),
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
        Field::new("author", DataType::Utf8, false),
        Field::new("image_url", DataType::Utf8, false),
        Field::new("tags", DataType::Utf8, false),
        Field::new("note", DataType::Utf8, false),
        Field::new("fetched_at", DataType::Int64, false),
        Field::new("fetch_failures", DataType::Int64, false),
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
        // "title › chapter › section" — the chunk's place in its document
        // (docs/RFC-outline-index.md Phase 1.5), read back as the citation's
        // `section`. Nullable and after `vector`, so a table from before it
        // exists gains it by `add_columns` and older builds' appends
        // conform ("").
        Field::new(CHUNK_CONTEXT_COL, DataType::Utf8, true),
        // What BM25 indexes: the context and the text as ONE document, so a
        // chunk matching the question in both scores above one matching a
        // heading word alone. A separate context leg gave a context-only
        // match a full rank-one vote and cost exact-identifier queries
        // their top rank (hard-exact MRR 1.00 → 0.78); one document per
        // chunk is the field-weighting BM25 was built for.
        Field::new(CHUNK_BM25_COL, DataType::Utf8, true),
    ]))
}

/// The chunks table's context column (see `chunks_schema`).
pub const CHUNK_CONTEXT_COL: &str = "context";
/// The chunks table's BM25 document column: "{context}\n{text}".
pub const CHUNK_BM25_COL: &str = "bm25";

/// The BM25 document for one chunk.
fn bm25_doc(context: &str, text: &str) -> String {
    if context.is_empty() {
        text.to_string()
    } else {
        format!("{context}\n{text}")
    }
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

/// What a scan of the turn table knows about one thread before its title is
/// fetched: which turn opened it, and how big and how recent it is.
struct ThreadAcc {
    /// (turn id, created_at) of the earliest user turn.
    first_user: Option<(String, i64)>,
    /// (row id, created_at) of the newest generated-name row, if any.
    named: Option<(String, i64)>,
    count: i64,
    created_at: i64,
    updated_at: i64,
}

/// The opening question as a one-line title.
fn thread_title(question: &str) -> String {
    let line = question.trim().lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return "Untitled".into();
    }
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(119).collect::<String>())
    } else {
        line.to_string()
    }
}

fn meta_turns_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("thread_id", DataType::Utf8, false),
        Field::new("role", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("citations", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("model", DataType::Utf8, false),
        Field::new("created_at", DataType::Int64, false),
    ]))
}

/// Decode meta-turn batches into rows.
fn meta_turns_from_batches(batches: &[RecordBatch]) -> Result<Vec<MetaTurn>> {
    let mut turns = Vec::new();
    for b in batches {
        let id = str_col(b, "id")?;
        let thread = str_col(b, "thread_id")?;
        let role = str_col(b, "role")?;
        let content = str_col(b, "content")?;
        let citations = str_col(b, "citations")?;
        let kind = str_col(b, "kind")?;
        // Written by a build that recorded which model answered; absent on
        // rows older than that column, which render without a model name.
        let model = opt_str_col(b, "model");
        let created = i64_col(b, "created_at")?;
        for i in 0..b.num_rows() {
            turns.push(MetaTurn {
                id: id.value(i).to_string(),
                thread_id: thread.value(i).to_string(),
                role: role.value(i).to_string(),
                content: content.value(i).to_string(),
                citations: serde_json::from_str(citations.value(i)).unwrap_or_default(),
                kind: kind.value(i).to_string(),
                model: model.map(|m| m.value(i).to_string()).unwrap_or_default(),
                created_at: created.value(i),
            });
        }
    }
    Ok(turns)
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

fn kv_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("key", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, false),
        Field::new("updated_at", DataType::Int64, false),
    ]))
}

fn receipts_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("schedule_id", DataType::Utf8, false),
        Field::new("notebook_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("trigger", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("detail", DataType::Utf8, false),
        Field::new("error", DataType::Utf8, false),
        Field::new("note_id", DataType::Utf8, false),
        Field::new("provider", DataType::Utf8, false),
        Field::new("model", DataType::Utf8, false),
        Field::new("cost_micros", DataType::Int64, false),
        Field::new("due_at", DataType::Int64, false),
        Field::new("started_at", DataType::Int64, false),
        Field::new("ended_at", DataType::Int64, false),
    ]))
}

fn receipt_batch(schema: &SchemaRef, r: &RunReceipt) -> Result<RecordBatch> {
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![r.id.clone()])),
            Arc::new(StringArray::from(vec![r.schedule_id.clone()])),
            Arc::new(StringArray::from(vec![r.notebook_id.clone()])),
            Arc::new(StringArray::from(vec![r.name.clone()])),
            Arc::new(StringArray::from(vec![r.kind.clone()])),
            Arc::new(StringArray::from(vec![r.trigger.clone()])),
            Arc::new(StringArray::from(vec![r.status.clone()])),
            Arc::new(StringArray::from(vec![r.detail.clone()])),
            Arc::new(StringArray::from(vec![r.error.clone()])),
            Arc::new(StringArray::from(vec![r.note_id.clone()])),
            Arc::new(StringArray::from(vec![r.provider.clone()])),
            Arc::new(StringArray::from(vec![r.model.clone()])),
            Arc::new(Int64Array::from(vec![r.cost_micros])),
            Arc::new(Int64Array::from(vec![r.due_at])),
            Arc::new(Int64Array::from(vec![r.started_at])),
            Arc::new(Int64Array::from(vec![r.ended_at])),
        ],
    )?)
}

fn source_events_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("notebook_id", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, false),
        Field::new("source_title", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("detail", DataType::Utf8, false),
        Field::new("diff", DataType::Utf8, false),
        Field::new("at", DataType::Int64, false),
    ]))
}

fn source_event_batch(schema: &SchemaRef, e: &SourceEvent) -> Result<RecordBatch> {
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![e.id.clone()])),
            Arc::new(StringArray::from(vec![e.notebook_id.clone()])),
            Arc::new(StringArray::from(vec![e.source_id.clone()])),
            Arc::new(StringArray::from(vec![e.source_title.clone()])),
            Arc::new(StringArray::from(vec![e.kind.clone()])),
            Arc::new(StringArray::from(vec![e.detail.clone()])),
            Arc::new(StringArray::from(vec![e.diff.clone()])),
            Arc::new(Int64Array::from(vec![e.at])),
        ],
    )?)
}

fn ledger_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("notebook_id", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("why", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("origin", DataType::Utf8, false),
        Field::new("anchors", DataType::Utf8, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
    ]))
}

fn ledger_batch(schema: &SchemaRef, e: &LedgerEntry) -> Result<RecordBatch> {
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![e.id.clone()])),
            Arc::new(StringArray::from(vec![e.notebook_id.clone()])),
            Arc::new(StringArray::from(vec![e.kind.clone()])),
            Arc::new(StringArray::from(vec![e.text.clone()])),
            Arc::new(StringArray::from(vec![e.why.clone()])),
            Arc::new(StringArray::from(vec![e.status.clone()])),
            Arc::new(StringArray::from(vec![e.origin.clone()])),
            Arc::new(StringArray::from(vec![
                serde_json::to_string(&e.anchors).unwrap_or_default()
            ])),
            Arc::new(Int64Array::from(vec![e.created_at])),
            Arc::new(Int64Array::from(vec![e.updated_at])),
        ],
    )?)
}

fn registry_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("origin", DataType::Utf8, false),
        Field::new("triage", DataType::Utf8, false),
        Field::new("identifiers", DataType::Utf8, false),
        Field::new("note", DataType::Utf8, false),
        Field::new("facts", DataType::Utf8, false),
        Field::new("attachments", DataType::Utf8, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
    ]))
}

fn registry_batch(schema: &SchemaRef, c: &RegistryCard) -> Result<RecordBatch> {
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![c.id.clone()])),
            Arc::new(StringArray::from(vec![c.kind.clone()])),
            Arc::new(StringArray::from(vec![c.name.clone()])),
            Arc::new(StringArray::from(vec![c.origin.clone()])),
            Arc::new(StringArray::from(vec![c.triage.clone()])),
            Arc::new(StringArray::from(vec![c.identifiers.clone()])),
            Arc::new(StringArray::from(vec![c.note.clone()])),
            Arc::new(StringArray::from(vec![
                serde_json::to_string(&c.facts).unwrap_or_default()
            ])),
            Arc::new(StringArray::from(vec![serde_json::to_string(
                &c.attachments,
            )
            .unwrap_or_default()])),
            Arc::new(Int64Array::from(vec![c.created_at])),
            Arc::new(Int64Array::from(vec![c.updated_at])),
        ],
    )?)
}

fn reports_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("notebook_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("prompt", DataType::Utf8, false),
        Field::new("trigger", DataType::Utf8, false),
        Field::new("not_before", DataType::Int64, false),
        Field::new("interval_secs", DataType::Int64, false),
        Field::new("enabled", DataType::Int64, false),
        Field::new("last_run_at", DataType::Int64, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("watch_sources", DataType::Utf8, false),
        Field::new("watch_kinds", DataType::Utf8, false),
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
            Arc::new(StringArray::from(vec![r.trigger.clone()])),
            Arc::new(Int64Array::from(vec![r.not_before])),
            Arc::new(Int64Array::from(vec![r.interval_secs])),
            Arc::new(Int64Array::from(vec![i64::from(r.enabled)])),
            Arc::new(Int64Array::from(vec![r.last_run_at])),
            Arc::new(Int64Array::from(vec![r.created_at])),
            Arc::new(StringArray::from(vec![r.watch_sources.clone()])),
            Arc::new(StringArray::from(vec![r.watch_kinds.clone()])),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    fn meta_turn(thread: &str, id: &str, role: &str, content: &str, at: i64) -> MetaTurn {
        MetaTurn {
            id: id.into(),
            thread_id: thread.into(),
            role: role.into(),
            content: content.into(),
            citations: Vec::new(),
            kind: "chat".into(),
            model: String::new(),
            created_at: at,
        }
    }

    /// Two rows with one id are one notebook: the listing shows it once and
    /// the dedupe pass leaves one row, preferring the active copy.
    #[tokio::test]
    async fn duplicate_notebook_rows_collapse_to_one() {
        let dir = std::env::temp_dir().join(format!("nbl-dedupe-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");
        let mut nb = Notebook {
            id: "nb-twice".into(),
            title: "Meridian".into(),
            created_at: 1,
            updated_at: 5,
            color: String::new(),
            icon: String::new(),
            status: "archived".into(),
            growth_web: false,
            source_count: 0,
            note_count: 0,
            report_count: 0,
        };
        db.create_notebook(&nb).await.expect("first row");
        assert!(
            db.create_notebook(&nb).await.is_err(),
            "a second row for an existing id is refused"
        );
        nb.updated_at = 2;
        nb.status = String::new();
        // The damage an older build could do: the same id appended twice.
        db.insert_notebook_row(&nb).await.expect("second row");
        let other = Notebook {
            id: "nb-once".into(),
            title: "Rookery".into(),
            ..nb.clone()
        };
        db.create_notebook(&other).await.expect("third row");

        let listed = db.list_notebooks().await.expect("list");
        assert_eq!(listed.len(), 2, "the listing never shows one id twice");
        let twice = listed.iter().find(|n| n.id == "nb-twice").expect("kept");
        assert_eq!(
            twice.status, "",
            "the active row wins over the archived one"
        );

        let collapsed = db.dedupe_notebook_rows().await.expect("dedupe");
        assert_eq!(collapsed, vec!["Meridian".to_string()]);
        let raw = db.collect(T_NOTEBOOKS, None).await.expect("collect");
        let rows: usize = raw.iter().map(|b| b.num_rows()).sum();
        assert_eq!(rows, 2, "one row per id remains on disk");
        let again = db.dedupe_notebook_rows().await.expect("dedupe again");
        assert!(again.is_empty(), "a clean table is a no-op");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Home thread is nothing but its turns: the list is derived, titled by
    /// the opening question, and ordered by what was used last.
    #[tokio::test]
    async fn meta_threads_are_derived_from_their_turns() {
        let dir = std::env::temp_dir().join(format!("nbl-meta-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");

        for turn in [
            meta_turn(
                "t1",
                "a",
                "user",
                "Where is the SNDK data?\nsecond line",
                10,
            ),
            meta_turn("t1", "b", "assistant", "In the Storage notebook.", 11),
            meta_turn("t2", "c", "user", "What did I conclude?", 20),
        ] {
            db.add_meta_turn(&turn).await.expect("add turn");
        }

        let threads = db.list_meta_threads().await.expect("threads");
        assert_eq!(
            threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["t2", "t1"],
            "most recently used first"
        );
        let first = threads.iter().find(|t| t.id == "t1").expect("t1");
        assert_eq!(first.title, "Where is the SNDK data?");
        assert_eq!(first.question, "Where is the SNDK data?");
        assert_eq!(first.turn_count, 2);
        assert_eq!(first.created_at, 10);
        assert_eq!(first.updated_at, 11);

        let turns = db.list_meta_turns("t1").await.expect("turns");
        assert_eq!(
            turns.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"],
            "oldest first — the conversation as it was read"
        );

        // The small model names the thread: the name is preferred over the
        // opening question, the question is still reported alongside it, and
        // the row it rides on is not a turn — it counts for nothing, never
        // moves `updated_at`, and never enters the transcript.
        assert_eq!(db.meta_thread_title("t1").await.expect("title"), None);
        db.set_meta_thread_title("t1", "SNDK data location")
            .await
            .expect("name it");
        assert_eq!(
            db.meta_thread_title("t1").await.expect("title").as_deref(),
            Some("SNDK data location")
        );
        let threads = db.list_meta_threads().await.expect("threads");
        let first = threads.iter().find(|t| t.id == "t1").expect("t1");
        assert_eq!(first.title, "SNDK data location");
        assert_eq!(first.question, "Where is the SNDK data?");
        assert_eq!(first.turn_count, 2, "a name is not a turn");
        assert_eq!(first.updated_at, 11, "naming is not conversation");
        assert_eq!(
            db.list_meta_turns("t1")
                .await
                .expect("turns")
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"],
            "the name never appears in the transcript"
        );
        // Re-naming replaces, never accumulates.
        db.set_meta_thread_title("t1", "Where SNDK lives")
            .await
            .expect("rename");
        assert_eq!(
            db.meta_thread_title("t1").await.expect("title").as_deref(),
            Some("Where SNDK lives")
        );
        assert_eq!(db.list_meta_turns("t1").await.expect("turns").len(), 2);

        db.delete_meta_thread("t1").await.expect("delete");
        assert!(db.list_meta_turns("t1").await.expect("turns").is_empty());
        let threads = db.list_meta_threads().await.expect("threads");
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, "t2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn message_pages_are_bounded_newest_first_without_overlap() {
        let dir = std::env::temp_dir().join(format!("nbl-message-page-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");
        for i in 1..=5 {
            db.add_message(&Message {
                id: format!("m-{i}"),
                notebook_id: "nb".into(),
                role: if i % 2 == 0 { "assistant" } else { "user" }.into(),
                content: format!("message {i}"),
                citations: Vec::new(),
                kind: "chat".into(),
                model: String::new(),
                created_at: i,
            })
            .await
            .expect("add message");
        }

        let (latest, more) = db.message_page("nb", None, None, 2).await.expect("latest");
        assert!(more);
        assert_eq!(
            latest.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m-4", "m-5"]
        );

        let (older, more) = db
            .message_page("nb", Some(latest[0].created_at), Some(&latest[0].id), 2)
            .await
            .expect("older");
        assert!(more);
        assert_eq!(
            older.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m-2", "m-3"]
        );

        let (oldest, more) = db
            .message_page("nb", Some(older[0].created_at), Some(&older[0].id), 2)
            .await
            .expect("oldest");
        assert!(!more);
        assert_eq!(
            oldest.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m-1"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn message_page_cursor_breaks_timestamp_ties_by_id() {
        let dir = std::env::temp_dir().join(format!("nbl-message-tie-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");
        for id in ["a", "b", "c"] {
            db.add_message(&Message {
                id: id.into(),
                notebook_id: "nb".into(),
                role: "user".into(),
                content: id.into(),
                citations: Vec::new(),
                kind: "chat".into(),
                model: String::new(),
                created_at: 42,
            })
            .await
            .expect("add message");
        }

        let (latest, more) = db.message_page("nb", None, None, 1).await.expect("latest");
        assert!(more);
        assert_eq!(latest[0].id, "c");

        let (middle, more) = db
            .message_page("nb", Some(42), Some("c"), 1)
            .await
            .expect("middle");
        assert!(more);
        assert_eq!(middle[0].id, "b");

        let (oldest, more) = db
            .message_page("nb", Some(42), Some("b"), 1)
            .await
            .expect("oldest");
        assert!(!more);
        assert_eq!(oldest[0].id, "a");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The prod-bricking bug: dev migrated the shared store (added
    /// `image_url`), and the installed binary's appends — batches built
    /// from its older compiled schema — failed with "Append with different
    /// schema". `add_batch` must conform an old-shape batch to the live
    /// table, filling unknown columns with defaults.
    #[tokio::test]
    async fn append_from_older_schema_conforms_to_live_table() {
        let dir = std::env::temp_dir().join(format!("nbl-conform-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");

        // An "old binary" batch for notebooks: no `status` column.
        let old_schema: SchemaRef = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("created_at", DataType::Int64, false),
            Field::new("updated_at", DataType::Int64, false),
            Field::new("color", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            old_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["old-id"])),
                Arc::new(StringArray::from(vec!["From the old binary"])),
                Arc::new(Int64Array::from(vec![1i64])),
                Arc::new(Int64Array::from(vec![2i64])),
                Arc::new(StringArray::from(vec!["#eb5757"])),
            ],
        )
        .expect("old batch");

        db.add_batch(T_NOTEBOOKS, old_schema, batch)
            .await
            .expect("old-schema append must conform, not error");

        let nbs = db.list_notebooks().await.expect("list");
        let nb = nbs.iter().find(|n| n.id == "old-id").expect("row landed");
        assert_eq!(nb.title, "From the old binary");
        assert_eq!(nb.status, "", "unknown column filled with its default");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same shared-store guarantee for the tags/note columns
    /// (docs/RFC-source-tags.md): an installed binary compiled before the
    /// columns existed appends source batches without them, and those must
    /// conform to the migrated live table with "" defaults — never brick.
    #[tokio::test]
    async fn source_append_without_tags_note_conforms() {
        let dir = std::env::temp_dir().join(format!("nbl-conform-src-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");

        // The pre-tags sources schema (through image_url, no tags/note).
        let old_schema: SchemaRef = Arc::new(Schema::new(vec![
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
            Field::new("author", DataType::Utf8, false),
            Field::new("image_url", DataType::Utf8, false),
        ]));
        let s = |v: &str| Arc::new(StringArray::from(vec![v])) as ArrayRef;
        let i = |v: i64| Arc::new(Int64Array::from(vec![v])) as ArrayRef;
        let batch = RecordBatch::try_new(
            old_schema.clone(),
            vec![
                s("old-src"),
                s("nb-1"),
                s("From the old binary"),
                s("text"),
                s(""),
                s("hello"),
                i(5),
                i(1),
                i(42),
                s("ready"),
                s(""),
                s(""),
                i(0),
                s(""),
                s(""),
            ],
        )
        .expect("old batch");

        db.add_batch(T_SOURCES, old_schema, batch)
            .await
            .expect("pre-tags append must conform, not error");

        let src = db
            .get_source("old-src")
            .await
            .expect("get")
            .expect("row landed");
        assert_eq!(src.title, "From the old binary");
        assert_eq!(src.tags, "", "tags filled with its default");
        assert_eq!(src.note, "", "note filled with its default");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same guarantee on a table that was MIGRATED into those columns
    /// rather than created with them — which is every existing install.
    /// Lance keeps `add_columns` columns in their own fragment, so an append
    /// carrying our own schema can land the row while leaving the evolved
    /// columns at their defaults. That silently zeroed `fetched_at` on every
    /// newly added source in a migrated store, making the hygiene sweep
    /// treat brand-new sources as ancient and re-fetch them immediately.
    #[tokio::test]
    async fn insert_keeps_fetch_stamp_on_a_migrated_table() {
        let dir = std::env::temp_dir().join(format!("nbl-migrated-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");

        // Recreate the pre-hygiene table shape, then migrate it exactly the
        // way a real install upgrades.
        db.conn.drop_table(T_SOURCES, &[]).await.expect("drop");
        let old: SchemaRef = Arc::new(Schema::new(
            sources_schema()
                .fields()
                .iter()
                .filter(|f| f.name() != "fetched_at" && f.name() != "fetch_failures")
                .map(|f| f.as_ref().clone())
                .collect::<Vec<_>>(),
        ));
        db.conn
            .create_empty_table(T_SOURCES, old)
            .execute()
            .await
            .expect("create pre-hygiene table");
        db.migrate_source_fetch().await.expect("migrate");

        let src = Source {
            id: "mig-1".into(),
            notebook_id: "nb-1".into(),
            title: "Fresh on a migrated table".into(),
            source_type: "url".into(),
            url: "https://example.com/fresh".into(),
            content: "body".into(),
            char_count: 4,
            chunk_count: 0,
            created_at: 111,
            status: "ready".into(),
            error: String::new(),
            parent_id: String::new(),
            mtime: 0,
            author: String::new(),
            image_url: String::new(),
            tags: String::new(),
            note: String::new(),
            fetched_at: 1_700_000_000_000,
            fetch_failures: 2,
        };
        db.insert_source(&src, &[], &[]).await.expect("insert");

        let got = db.get_source("mig-1").await.expect("get").expect("row");
        assert_eq!(
            got.fetched_at, 1_700_000_000_000,
            "fetched_at must survive an append to a migrated table"
        );
        assert_eq!(got.fetch_failures, 2, "fetch_failures must survive");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A narrow column update must not disturb the columns it doesn't name —
    /// on a MIGRATED table, where the hygiene columns live in their own
    /// fragment. Every status flip, retitle, and mtime bump goes through one
    /// of these updates, so if they reset `fetched_at` the sweep would treat
    /// every touched source as ancient and re-fetch it forever.
    #[tokio::test]
    async fn column_updates_leave_the_fetch_stamp_alone() {
        let dir = std::env::temp_dir().join(format!("nbl-updstamp-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");
        db.conn.drop_table(T_SOURCES, &[]).await.expect("drop");
        let old: SchemaRef = Arc::new(Schema::new(
            sources_schema()
                .fields()
                .iter()
                .filter(|f| f.name() != "fetched_at" && f.name() != "fetch_failures")
                .map(|f| f.as_ref().clone())
                .collect::<Vec<_>>(),
        ));
        db.conn
            .create_empty_table(T_SOURCES, old)
            .execute()
            .await
            .expect("create pre-hygiene table");
        db.migrate_source_fetch().await.expect("migrate");

        let src = Source {
            id: "upd-1".into(),
            notebook_id: "nb-1".into(),
            title: "Touched".into(),
            source_type: "url".into(),
            url: "https://example.com/touched".into(),
            content: "body".into(),
            char_count: 4,
            chunk_count: 0,
            created_at: 111,
            status: "processing".into(),
            error: String::new(),
            parent_id: String::new(),
            mtime: 0,
            author: String::new(),
            image_url: String::new(),
            tags: String::new(),
            note: String::new(),
            fetched_at: 1_700_000_000_000,
            fetch_failures: 0,
        };
        db.insert_source(&src, &[], &[]).await.expect("insert");

        db.finish_processing("upd-1", 3, "ready", "")
            .await
            .expect("finish");
        let got = db.get_source("upd-1").await.expect("get").expect("row");
        assert_eq!(got.status, "ready");
        assert_eq!(
            got.fetched_at, 1_700_000_000_000,
            "finish_processing must not reset fetched_at"
        );

        db.set_source_mtime("upd-1", 42).await.expect("mtime");
        db.set_source_title("upd-1", "Retitled")
            .await
            .expect("title");
        let got = db.get_source("upd-1").await.expect("get").expect("row");
        assert_eq!(got.mtime, 42);
        assert_eq!(got.title, "Retitled");
        assert_eq!(
            got.fetched_at, 1_700_000_000_000,
            "narrow updates must not reset fetched_at"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A freshly inserted source keeps its freshness stamp
    /// (docs/RFC-source-hygiene.md). The insert path goes through
    /// `source_batch` + `add_batch`, which rebuilds batches to match the live
    /// table — a column dropped there would silently zero every new source's
    /// `fetched_at` and make the hygiene sweep treat it as ancient.
    #[tokio::test]
    async fn insert_source_keeps_its_fetch_stamp() {
        let dir = std::env::temp_dir().join(format!("nbl-fetchstamp-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");
        let src = Source {
            id: "fs-1".into(),
            notebook_id: "nb-1".into(),
            title: "Fresh".into(),
            source_type: "url".into(),
            url: "https://example.com/fresh".into(),
            content: "body".into(),
            char_count: 4,
            chunk_count: 0,
            created_at: 111,
            status: "ready".into(),
            error: String::new(),
            parent_id: String::new(),
            mtime: 0,
            author: String::new(),
            image_url: String::new(),
            tags: String::new(),
            note: String::new(),
            fetched_at: 1_700_000_000_000,
            fetch_failures: 2,
        };
        db.insert_source(&src, &[], &[]).await.expect("insert");

        let got = db.get_source("fs-1").await.expect("get").expect("row");
        assert_eq!(got.fetched_at, 1_700_000_000_000, "fetched_at must survive");
        assert_eq!(got.fetch_failures, 2, "fetch_failures must survive");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Tags and notes round-trip through their update helpers, and clearing
    /// the annotation deletes its `snote:` chunk rows.
    #[tokio::test]
    async fn source_tags_and_note_round_trip() {
        let dir = std::env::temp_dir().join(format!("nbl-tags-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");
        let src = Source {
            id: "s-1".into(),
            notebook_id: "nb".into(),
            title: "Doc".into(),
            source_type: "text".into(),
            url: String::new(),
            content: "body".into(),
            char_count: 4,
            chunk_count: 0,
            created_at: 1,
            status: "ready".into(),
            error: String::new(),
            parent_id: String::new(),
            mtime: 0,
            author: String::new(),
            image_url: String::new(),
            tags: String::new(),
            note: String::new(),
            fetched_at: 0,
            fetch_failures: 0,
        };
        db.insert_source(&src, &[], &[]).await.expect("insert");

        db.set_source_tags("s-1", "rust retrieval")
            .await
            .expect("tags");
        db.set_source_note("s-1", "why I saved it")
            .await
            .expect("note");
        let got = db.get_source("s-1").await.expect("get").expect("found");
        assert_eq!(got.tags, "rust retrieval");
        assert_eq!(got.note, "why I saved it");

        // An snote chunk row is owned by `snote:<id>` and dies with its text.
        db.add_chunks(
            "nb",
            &format!("{SNOTE_CHUNK_PREFIX}s-1"),
            &[("sc-1".into(), 0, "why I saved it".into())],
            &[vec![0.0; 4]],
        )
        .await
        .expect("index snote");
        db.delete_snote_chunks("s-1").await.expect("clear");
        let batches = db
            .collect(T_CHUNKS, Some("source_id = 'snote:s-1'"))
            .await
            .expect("collect");
        let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(rows, 0, "cleared annotation leaves no chunk rows");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conform_falls_back_on_type_drift_and_unknown_types() {
        let ours: SchemaRef = Arc::new(Schema::new(vec![Field::new("id", DataType::Utf8, false)]));
        let batch = RecordBatch::try_new(
            ours.clone(),
            vec![Arc::new(StringArray::from(vec!["x"])) as ArrayRef],
        )
        .unwrap();

        // Same-name column with a different type: hands back the original.
        let drift: SchemaRef =
            Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let (schema, _) = conform_to_live(&drift, ours.clone(), batch.clone());
        assert_eq!(schema, ours);

        // Missing column of a type we can't invent (vector): original too.
        let vec_live: SchemaRef = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "embedding",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 4),
                false,
            ),
        ]));
        let (schema, _) = conform_to_live(&vec_live, ours.clone(), batch);
        assert_eq!(schema, ours);
    }

    /// The RRF tie case that motivated the recency tie-break: a vector-only
    /// and an FTS-only hit at the same rank score identically. Newer owner
    /// wins the tie; id order decides only when recency ties too — and the
    /// fixtures make id order vote the other way, so a pass proves recency
    /// actually outranks it.
    #[test]
    fn fused_cmp_breaks_score_ties_by_recency_then_id() {
        let tie = 1.0 / 60.0;
        // Score dominates everything.
        assert_eq!(
            fused_cmp((0.9, 0, "z"), (0.1, 999, "a")),
            Ordering::Less,
            "higher score ranks first regardless of recency and id"
        );
        // Equal score → newer owner first, even though "a" < "z".
        assert_eq!(
            fused_cmp((tie, 2_000, "z"), (tie, 1_000, "a")),
            Ordering::Less
        );
        assert_eq!(
            fused_cmp((tie, 1_000, "a"), (tie, 2_000, "z")),
            Ordering::Greater
        );
        // Equal score and recency → id ascending keeps determinism.
        assert_eq!(fused_cmp((tie, 5, "a"), (tie, 5, "b")), Ordering::Less);
        assert_eq!(fused_cmp((tie, 5, "b"), (tie, 5, "a")), Ordering::Greater);
        assert_eq!(fused_cmp((tie, 5, "a"), (tie, 5, "a")), Ordering::Equal);
    }

    #[test]
    fn owner_recency_resolves_notes_and_unknowns() {
        let mut recency = HashMap::new();
        recency.insert("src-1".to_string(), 111i64);
        recency.insert(format!("{NOTE_CHUNK_PREFIX}n-1"), 222i64);
        let source_hit = Citation {
            chunk_id: "c1".into(),
            source_id: "src-1".into(),
            source_title: String::new(),
            source_path: String::new(),
            note_id: String::new(),
            gist: false,
            snote: false,
            ordinal: 0,
            snippet: String::new(),
            distance: 0.0,
            section: String::new(),
        };
        // Gist rows reach the comparator with source_id already resolved,
        // so they inherit the source's timestamp through the same key.
        let gist_hit = Citation {
            gist: true,
            ..source_hit.clone()
        };
        let note_hit = Citation {
            source_id: String::new(),
            note_id: "n-1".into(),
            ..source_hit.clone()
        };
        let unknown = Citation {
            source_id: "src-gone".into(),
            ..source_hit.clone()
        };
        assert_eq!(owner_recency(&recency, &source_hit), 111);
        assert_eq!(owner_recency(&recency, &gist_hit), 111);
        assert_eq!(owner_recency(&recency, &note_hit), 222);
        assert_eq!(
            owner_recency(&recency, &unknown),
            0,
            "unknown owners sort oldest"
        );
    }

    /// Neighbor expansion widens prompt excerpts with ordinal ±1 text,
    /// never double-includes an ordinal another citation already carries,
    /// and skips notes/gists. Fake 4-dim vectors — no embedder involved.
    #[tokio::test]
    async fn expand_neighbor_excerpts_widens_and_dedupes() {
        let dir = std::env::temp_dir().join(format!("nbl-db-expand-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");
        let rows: Vec<(String, i32, String)> = [
            "alpha section text",
            "bravo section text",
            "charlie section text",
        ]
        .iter()
        .enumerate()
        .map(|(i, t)| (format!("c{i}"), i as i32, t.to_string()))
        .collect();
        let vecs: Vec<Vec<f32>> = vec![vec![0.0; 4]; rows.len()];
        db.add_chunks("nb", "src-1", &rows, &vecs)
            .await
            .expect("add");

        let cite = |chunk_id: &str, ordinal: i32, snippet: &str| Citation {
            chunk_id: chunk_id.into(),
            source_id: "src-1".into(),
            source_title: "Doc".into(),
            source_path: String::new(),
            note_id: String::new(),
            gist: false,
            snote: false,
            ordinal,
            snippet: snippet.into(),
            distance: 0.0,
            section: String::new(),
        };
        // Both middle and last chunks are cited: the middle one may claim
        // only the uncited ordinal 0; the last one has no free neighbors
        // (1 is cited, 3 does not exist) so it must not expand.
        let citations = vec![
            cite("c1", 1, "bravo section text"),
            cite("c2", 2, "charlie section text"),
        ];
        let expanded = db
            .expand_neighbor_excerpts(&citations)
            .await
            .expect("expand");
        let widened = expanded.get("c1").expect("middle chunk widens");
        assert_eq!(widened, "alpha section text\n\nbravo section text");
        assert!(
            !expanded.contains_key("c2"),
            "no free neighbors means no expansion entry"
        );

        // Notes and gists never expand.
        let note_hit = Citation {
            note_id: "n1".into(),
            source_id: String::new(),
            ..cite("c9", 0, "note text")
        };
        let gist_hit = Citation {
            gist: true,
            ..cite("c1", 1, "gist text")
        };
        let none = db
            .expand_neighbor_excerpts(&[note_hit, gist_hit])
            .await
            .expect("expand");
        assert!(none.is_empty());
    }

    /// Switching embedders leaves the router index at the old vector width,
    /// which no summary diff can see. A routed search carries the current
    /// width by construction, so it is the free detector: it drops the stale
    /// index and answers empty (the caller's "no index yet" case) instead of
    /// failing the whole search with Lance's dimension error — and the table
    /// then rebuilds cleanly at the new width, which a stale one refuses.
    #[tokio::test]
    async fn route_search_heals_a_stale_vector_width() {
        let dir = std::env::temp_dir().join(format!("nbl-routedim-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");
        let routes = vec![Route {
            id: "src:1".into(),
            kind: "source".into(),
            notebook_id: "nb1".into(),
            summary: "Ferrari service history".into(),
        }];

        db.upsert_routes(&routes, &[vec![0.1, 0.2, 0.3, 0.4]])
            .await
            .expect("build index");
        assert_eq!(db.routes_vector_dim().await.expect("dim"), Some(4));
        assert_eq!(
            db.route_search(vec![0.1, 0.2, 0.3, 0.4], None, 4)
                .await
                .expect("search")
                .len(),
            1,
            "matching width searches normally"
        );

        let healed = db
            .route_search(vec![0.1, 0.2], None, 4)
            .await
            .expect("mismatched width must not error");
        assert!(healed.is_empty());
        assert_eq!(
            db.routes_vector_dim().await.expect("dim"),
            None,
            "the stale index is dropped, not left to fail every query"
        );
        assert!(db.list_routes().await.expect("routes").is_empty());

        db.upsert_routes(&routes, &[vec![0.5, 0.6]])
            .await
            .expect("rebuild at the new width");
        assert_eq!(db.routes_vector_dim().await.expect("dim"), Some(2));
        assert_eq!(
            db.route_search(vec![0.5, 0.6], None, 4)
                .await
                .expect("search")
                .len(),
            1
        );
    }
}
