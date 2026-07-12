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
    /// Populated on list queries; not stored on the row.
    #[serde(default)]
    pub source_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub notebook_id: String,
    pub title: String,
    /// "pdf" | "text" | "markdown" | "url" | "image" | "folder" | "mac"
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
    pub source_id: String,
    pub source_title: String,
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
    /// "data_table" | "problems" |
    /// "prd" | "prfaq" | "rfc" | "skill" | "report" | "template"
    #[serde(default = "default_note_kind")]
    pub kind: String,
    /// Optional custom instructions used to generate this note, retained so it
    /// can be rebuilt with fresh context.
    #[serde(default)]
    pub prompt: String,
    pub created_at: i64,
    pub updated_at: i64,
}

fn default_note_kind() -> String {
    "note".to_string()
}
