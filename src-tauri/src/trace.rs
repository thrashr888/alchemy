//! Local retrieval trace export (docs/RFC-retrieval-maturity.md Phase 6):
//! one JSONL line per retrieval — query, scope, per-stage counts, final
//! citations, warnings — so a bad answer can be replayed from what search
//! actually saw, and future tuning (query planning, rerank thresholds, a
//! small routing model) has real data to learn from.
//!
//! Strictly local: the file lives in the app data dir and nothing ships it
//! anywhere. Tracing must never break retrieval, so every failure here is
//! swallowed after a stderr note.

use std::io::Write;
use std::path::Path;

/// Rotate at ~5 MB, keeping one previous generation. At a few hundred bytes
/// per retrieval that is months of history without unbounded growth.
const MAX_BYTES: u64 = 5 * 1024 * 1024;
const FILE: &str = "retrieval.jsonl";

/// Append one retrieval trace record. Infallible by design — see module docs.
pub fn log(dir: &Path, record: serde_json::Value) {
    log_file(dir, FILE, record);
}

/// Append one record to an arbitrary JSONL trace file in `dir` — same
/// rotation rules and swallow-after-stderr contract as retrieval traces.
/// Page capture telemetry (capture.rs) writes `capture.jsonl` through this.
pub fn log_file(dir: &Path, file: &str, record: serde_json::Value) {
    if let Err(err) = try_log(dir, file, &record) {
        eprintln!("{file} trace write failed: {err}");
    }
}

fn try_log(dir: &Path, file: &str, record: &serde_json::Value) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file);
    if std::fs::metadata(&path)
        .map(|m| m.len() > MAX_BYTES)
        .unwrap_or(false)
    {
        let rotated = file.replace(".jsonl", ".1.jsonl");
        let _ = std::fs::rename(&path, dir.join(rotated));
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{record}")
}

/// Compact citation list for a trace record: enough to identify every hit
/// without duplicating chunk text into the log.
pub fn cite_summaries(citations: &[crate::models::Citation]) -> Vec<serde_json::Value> {
    citations
        .iter()
        .enumerate()
        .map(|(rank, c)| {
            serde_json::json!({
                "rank": rank + 1,
                "chunkId": c.chunk_id,
                "sourceId": c.source_id,
                "noteId": c.note_id,
                "title": c.source_title,
            })
        })
        .collect()
}
