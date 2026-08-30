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
use std::path::{Path, PathBuf};

/// The traces directory, set once at startup (the same value
/// `AppState::trace_dir` carries) so background work spawned without a
/// `State` handle — the gist sweep's wiki-index refresh — can still read
/// the retrieval history.
static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

pub fn dir() -> Option<&'static PathBuf> {
    DIR.get()
}

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
        crate::note!("{file} trace write failed: {err}");
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

// ---- Startup ---------------------------------------------------------------

const STARTUP_FILE: &str = "startup.jsonl";

/// Boot-phase stamps in `startup.jsonl` (docs/RFC-professional-grade.md
/// Pillar 2): one line per phase, so a cold-start regression between releases
/// is a `jq` one-liner instead of a stopwatch.
///
/// The clock is honest about where it starts and stops. `t0` is the top of
/// `setup()`; the builder chain, plugin registration, and the config window
/// whose webview Tauri builds *before* it runs our hook all happen earlier and
/// are unreachable from there, so `ms` is elapsed-since-setup, never since
/// `exec`. The last stamp is `setup_done` — the backend is ready and the
/// webview has been loading alongside it. "Window interactive" would need a
/// beacon the front-end does not emit; a stamp here would time `setup` rather
/// than paint, so it is deliberately absent instead of wrong.
pub struct Startup {
    dir: std::path::PathBuf,
    t0: std::time::Instant,
    /// Groups one boot's lines together — the log interleaves runs.
    boot: String,
}

impl Startup {
    /// Start the clock and stamp `setup_start`. `dir` is the traces directory.
    pub fn begin(dir: std::path::PathBuf) -> Self {
        let started = Self {
            dir,
            t0: std::time::Instant::now(),
            boot: uuid::Uuid::new_v4().to_string(),
        };
        started.stamp("setup_start");
        started
    }

    /// Stamp one phase with its elapsed milliseconds since `begin`.
    /// Infallible like every other trace write — see module docs.
    pub fn stamp(&self, phase: &str) {
        log_file(
            &self.dir,
            STARTUP_FILE,
            serde_json::json!({
                "ts": chrono::Utc::now().timestamp_millis(),
                "version": env!("CARGO_PKG_VERSION"),
                "boot": self.boot,
                "phase": phase,
                "ms": self.t0.elapsed().as_millis() as u64,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    /// The startup trace is only ever exercised by a real launch, so pin its
    /// shape here: one line per phase, in order, each parseable and stamped
    /// with a monotonic elapsed time under a single boot id.
    #[test]
    fn startup_stamps_write_ordered_jsonl() {
        let dir = std::env::temp_dir().join(format!("alchemy-startup-{}", uuid::Uuid::new_v4()));
        let startup = super::Startup::begin(dir.clone());
        startup.stamp("db_open");
        startup.stamp("setup_done");

        let text = std::fs::read_to_string(dir.join(super::STARTUP_FILE)).expect("trace written");
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).expect("valid json"))
            .collect();
        let phases: Vec<&str> = lines.iter().map(|l| l["phase"].as_str().unwrap()).collect();
        assert_eq!(phases, ["setup_start", "db_open", "setup_done"]);
        assert!(lines
            .windows(2)
            .all(|w| w[0]["ms"].as_u64() <= w[1]["ms"].as_u64()));
        assert!(lines.iter().all(|l| l["boot"] == lines[0]["boot"]));
        assert_eq!(lines[0]["version"], env!("CARGO_PKG_VERSION"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
