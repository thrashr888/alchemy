//! Serde data models shared across the Tauri command boundary.
//! Field names are camelCased so they land naturally in the TS frontend.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notebook {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub color: String,
    /// Lucide icon name ("" → the default book). Auto-picked from the title
    /// at creation; user-set from the rename dialog thereafter.
    #[serde(default)]
    pub icon: String,
    /// "" (active) | "archived" | "system". Archived notebooks are hidden
    /// from the main grid but keep all their data and can be unarchived;
    /// system notebooks (Briefs) never appear on the shelf at all.
    #[serde(default)]
    pub status: String,
    /// Populated on list queries; not stored on the row.
    #[serde(default)]
    pub source_count: i64,
    /// Deliberate notes, excluding reports — see `report_count`.
    #[serde(default)]
    pub note_count: i64,
    /// Report-kind notes (scheduled runs, briefs).
    #[serde(default)]
    pub report_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub notebook_id: String,
    pub title: String,
    /// "pdf" | "text" | "markdown" | "html" | "url" | "image" | "folder" | "mac"
    pub source_type: String,
    /// Origin of the content: the URL for `url` sources, the local file path
    /// for file imports, empty for pasted text. Retained so sources can be
    /// refreshed from their origin and agents can crawl/expand them later.
    #[serde(default)]
    pub url: String,
    /// Full extracted text. Kept so we can re-chunk or show the original.
    #[serde(default)]
    pub content: String,
    pub char_count: i64,
    pub chunk_count: i64,
    pub created_at: i64,
    /// "ready" | "error" | "placeholder". Placeholder = a cloud-sync file
    /// (OneDrive/Dropbox/Drive/iCloud) that exists in the folder but isn't
    /// downloaded locally — listed, labeled, and skipped by embedding until
    /// it materializes.
    #[serde(default = "default_status")]
    pub status: String,
    /// Human-readable failure reason when `status == "error"`.
    #[serde(default)]
    pub error: String,
    /// Id of the folder source this file belongs to; empty for top-level
    /// sources. Folder children are regular sources grouped under a parent.
    #[serde(default)]
    pub parent_id: String,
    /// File modification time (unix millis) recorded at ingest for folder
    /// children; 0 otherwise. Folder rescans compare it to detect changes.
    #[serde(default)]
    pub mtime: i64,
    /// Embedded document authorship (PDF /Author, Office dc:creator, EXIF
    /// Artist), captured at ingest; empty when the format carries none.
    #[serde(default)]
    pub author: String,
    /// Lead image for the gallery: the page's og:image / twitter:image for
    /// `url` sources. "" = unknown, "-" = checked and the page has none.
    #[serde(default)]
    pub image_url: String,
    /// User-assigned tags: space-separated normalized tokens (lowercase, no
    /// `#`, deduped — see `commands::normalize_tags`). Ground truth from the
    /// user, folded into routes and the chat manifest
    /// (docs/RFC-source-tags.md).
    #[serde(default)]
    pub tags: String,
    /// The user's one editable annotation on this source ("why I saved
    /// this"). Indexed as a chunk row under `snote:<source_id>` so retrieval
    /// can surface it (docs/RFC-source-tags.md).
    #[serde(default)]
    pub note: String,
}

/// Tally of what a folder rescan changed across the scanned folder sources.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderScan {
    pub added: u32,
    pub updated: u32,
    pub removed: u32,
    pub failed: u32,
}

impl FolderScan {
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed + self.failed > 0
    }

    pub fn absorb(&mut self, other: FolderScan) {
        self.added += other.added;
        self.updated += other.updated;
        self.removed += other.removed;
        self.failed += other.failed;
    }
}

fn default_trigger() -> String {
    "interval".into()
}

fn default_status() -> String {
    "ready".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub name: String,
    pub installed: bool,
    pub working: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelHealth {
    pub reachable: bool,
    pub chat: ModelStatus,
    pub embed: ModelStatus,
    /// Optional — only needed for image / scanned-PDF OCR.
    pub vision: ModelStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStat {
    pub name: String,
    pub last_tokens_per_sec: f64,
    pub avg_tokens_per_sec: f64,
    pub samples: u64,
}

/// One anchor pinning a ledger entry to verbatim source text. The quote is
/// the anchor — it survives re-chunking and drives find-in-source
/// highlighting, the same contract citations already use.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerAnchor {
    pub source_id: String,
    #[serde(default)]
    pub quote: String,
}

/// One typed ledger row (RFC-v12-steward pillar 2): memory the machine can
/// act on. Kinds and their lifecycles:
///   assertion: asserted → corroborated | contradicted | stale
///   fact:      current → superseded
///   decision:  decided → superseded
///   question:  open → answered
///   log:       logged (terminal — a log line is what happened)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub id: String,
    pub notebook_id: String,
    /// "assertion" | "fact" | "decision" | "question" | "log"
    pub kind: String,
    pub text: String,
    /// The because: rationale for decisions, context for others. Optional.
    #[serde(default)]
    pub why: String,
    pub status: String,
    /// "" for user- and agent-written rows, "auto" for rows the chat
    /// post-pass minted on its own (same contract as auto notes).
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub anchors: Vec<LedgerAnchor>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One key fact on a registry card — an ordered label/value pair, the same
/// shape the reader's document-properties grid already renders.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardFact {
    pub label: String,
    #[serde(default)]
    pub value: String,
}

/// One document filed under a registry card.
///
/// `matched` is the receipt and it is never empty: the identifier string
/// that matched, "name", or "manual". A machine that attaches without
/// showing its reason is a machine you stop trusting on the first mistake.
/// `rejected` rows are kept, not deleted — they are the refusal memory that
/// stops the sweep re-proposing the same pair forever (the `gist.rs` idiom).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardAttachment {
    pub source_id: String,
    /// Denormalized so the grid can group by notebook without loading every
    /// source in the corpus.
    #[serde(default)]
    pub notebook_id: String,
    /// "confirmed" | "proposed" | "rejected"
    pub status: String,
    pub matched: String,
    pub at: i64,
}

/// One registry card (docs/RFC-registry.md): a confirmed cast member —
/// asset, person, policy, provider, project, or dependency — aggregating
/// the documents that follow it.
///
/// Deliberately has no `notebook_id`: cards are the first corpus-scoped
/// entity besides notebooks themselves. A card's "home" notebook is derived
/// from where its documents live, never stored. Cards have no lifecycle;
/// their attachments do.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryCard {
    pub id: String,
    /// "asset" | "person" | "policy" | "provider" | "project" | "dependency"
    pub kind: String,
    pub name: String,
    /// "" for cards you made or confirmed, "auto" for one the suggester
    /// proposed and you haven't ruled on, "dismissed" for one you turned
    /// down — kept, because the row IS the refusal memory that stops the
    /// suggester proposing it again (the `gist.rs` idiom).
    #[serde(default)]
    pub origin: String,
    /// The triage verdict on a still-suggested card ("auto" origin only):
    /// "" = not yet triaged, "recommended" = the triage pass thinks this one
    /// matters, "routine" = triaged and not singled out. Cleared when the
    /// card is ruled on — it is queue metadata, not a property of the thing.
    #[serde(default)]
    pub triage: String,
    /// Space-separated normalized tokens (the `normalize_tags` form): VIN,
    /// policy number, serial, model number. The auto-attach key, and the
    /// only thing that ever attaches a document without asking.
    #[serde(default)]
    pub identifiers: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub facts: Vec<CardFact>,
    #[serde(default)]
    pub attachments: Vec<CardAttachment>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One observed source change (docs/RFC-night-shift.md §"Watchers"): change
/// becomes a first-class event instead of a silent overwrite. Written by the
/// resync/refresh paths; read by the Brief's collector and agents. The
/// events table is a rolling window, not an archive — old rows prune.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceEvent {
    pub id: String,
    pub notebook_id: String,
    pub source_id: String,
    pub source_title: String,
    /// "updated" (more kinds as watcher classes land).
    pub kind: String,
    /// Short human line ("page re-fetched · +12 −3 lines", …).
    #[serde(default)]
    pub detail: String,
    /// Capped diff excerpt (± prefixed lines) computed at refresh time — the
    /// old content is in hand at the reingest choke point, so no snapshot
    /// table is needed. Empty when nothing textual changed (e.g. re-embeds).
    #[serde(default)]
    pub diff: String,
    pub at: i64,
}

/// A periodic report definition. On its interval, the app refreshes the
/// notebook's URL sources, then generates a timestamped note.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSchedule {
    pub id: String,
    pub notebook_id: String,
    pub name: String,
    /// Generator kind (e.g. "briefing") or "custom".
    pub kind: String,
    /// Custom instruction when `kind == "custom"`.
    #[serde(default)]
    pub prompt: String,
    /// "interval" (the clock fires it) or "change" (a standing question —
    /// source events in its notebook pull the trigger, with `interval_secs`
    /// as the throttle floor between runs). RFC-night-shift §Staged.
    #[serde(default = "default_trigger")]
    pub trigger: String,
    pub interval_secs: i64,
    pub enabled: bool,
    /// Unix millis of the last successful run; 0 = never run.
    pub last_run_at: i64,
    pub created_at: i64,
}

/// Mirrors the `chunks` Lance table. Rows are written via tuples in `db.rs`;
/// this type documents the schema and is used when reading chunks back.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub id: String,
    pub notebook_id: String,
    pub source_id: String,
    pub ordinal: i32,
    pub text: String,
}

/// A retrieved chunk with its similarity distance and owning source title.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub chunk_id: String,
    /// Empty when the passage came from a note (see `note_id`).
    pub source_id: String,
    /// Title of the source — or of the note for note passages.
    pub source_title: String,
    /// On-disk path of the source's original file (`Source.url` when it is a
    /// local path). Empty for web/mac sources and note passages. Lets an
    /// agent reading the MCP search payload open the original without a
    /// second `get_source` round-trip.
    #[serde(default)]
    pub source_path: String,
    /// Non-empty when the passage came from a note rather than a source:
    /// the note's id. Notes are indexed alongside source chunks so agents
    /// and chat can recall prior conclusions (docs/RFC-note-curator.md).
    #[serde(default)]
    pub note_id: String,
    /// True when this row is a source gist — a distilled overview row
    /// (docs/RFC-infinite-context.md Phase 1) rather than a verbatim
    /// passage. `source_id` still names the gisted source; `ordinal` holds
    /// the content hash the gist was distilled from, not a position.
    #[serde(default)]
    pub gist: bool,
    /// True when this row is the user's own annotation on a source
    /// (docs/RFC-source-tags.md) — stored under `snote:<source_id>`.
    /// `source_id` still names the annotated source. Prompts label these
    /// `(your note on "…")` so the model knows it's the user's judgment,
    /// not corpus evidence.
    #[serde(default)]
    pub snote: bool,
    pub ordinal: i32,
    pub snippet: String,
    pub distance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub notebook_id: String,
    /// "user" | "assistant"
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub citations: Vec<Citation>,
    /// "chat" (an LLM answer / user turn) | "tool" (a tool confirmation).
    /// Tool messages are excluded from model context windows.
    #[serde(default = "default_message_kind")]
    pub kind: String,
    /// Which provider answered (assistant messages), e.g. "Claude Code ·
    /// $0.04" — display caption, empty for user/tool turns and old rows.
    #[serde(default)]
    pub model: String,
    pub created_at: i64,
}

fn default_message_kind() -> String {
    "chat".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub notebook_id: String,
    pub title: String,
    pub content: String,
    /// "note" | "summary" | "faq" | "study_guide" | "briefing" | "timeline" |
    /// "insights" | "flashcards" | "quiz" | "audio_overview" | "mind_map" |
    /// "data_table" | "round_table" | "problems" | "evidence" |
    /// "prd" | "prfaq" | "rfc" | "skill" | "report" | "template"
    #[serde(default = "default_note_kind")]
    pub kind: String,
    /// Optional custom instructions used to generate this note, retained so it
    /// can be rebuilt with fresh context.
    #[serde(default)]
    pub prompt: String,
    /// "" for deliberate notes (user-written, Studio-generated, agent-created)
    /// or "auto" for notes the chat post-pass created on its own
    /// (docs/RFC-note-curator.md phase 3). The curator only ever touches
    /// "auto" notes; editing an auto note flips it to "" (user-owned).
    #[serde(default)]
    pub origin: String,
    /// Curator state for "auto" notes: "" (active) | "stale" (unused ~30
    /// app-open days, dimmed) | "archived" (~90, dropped from retrieval).
    /// Usage or an edit revives; the curator never deletes.
    #[serde(default)]
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

fn default_note_kind() -> String {
    "note".to_string()
}

/// Usage counters for one note — the curator's ground truth for staleness
/// (docs/RFC-note-curator.md phase 2). Notes never used have no row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteUsage {
    pub note_id: String,
    /// Times a human or agent opened the note (UI card, MCP get_note).
    pub reads: i64,
    /// Times a search surfaced one of its passages (chat retrieval, MCP
    /// search, meta-chat). Palette as-you-type hits don't count.
    pub retrieval_hits: i64,
    /// Times a persisted or streamed answer carried it as a citation.
    pub cited: i64,
    pub last_used_at: i64,
}
