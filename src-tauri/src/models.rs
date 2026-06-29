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
    /// "pdf" | "text" | "markdown" | "url"
    pub source_type: String,
    /// Original URL for `url` sources (empty otherwise). Retained so an agent
    /// can crawl/expand the source later.
    #[serde(default)]
    pub url: String,
    /// Full extracted text. Kept so we can re-chunk or show the original.
    #[serde(default)]
    pub content: String,
    pub char_count: i64,
    pub chunk_count: i64,
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
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub notebook_id: String,
    pub title: String,
    pub content: String,
    /// "note" | "summary" | "faq" | "study_guide" | "briefing"
    #[serde(default = "default_note_kind")]
    pub kind: String,
    pub created_at: i64,
    pub updated_at: i64,
}

fn default_note_kind() -> String {
    "note".to_string()
}
