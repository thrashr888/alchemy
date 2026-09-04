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

/// The one startup clock for this process. Tauri builds the webview before it
/// enters setup(), so the frontend can paint while setup is still running;
/// retaining this clock lets its first committed frame close the same trace.
static ACTIVE_STARTUP: std::sync::OnceLock<Startup> = std::sync::OnceLock::new();

/// Boot-phase stamps in `startup.jsonl` (docs/RFC-professional-grade.md
/// Pillar 2): one line per phase, so a cold-start regression between releases
/// is a `jq` one-liner instead of a stopwatch.
///
/// The clock starts at the first instruction in [`crate::run`], before Tauri's
/// plugin registration and WKWebView construction. It still cannot see the
/// LaunchServices/dyld interval before Rust's entrypoint; an external launch
/// harness must include that last piece. `window_interactive` comes from the
/// frontend after its first committed frame, so the trace now covers every
/// in-process phase instead of stopping at backend setup.
#[derive(Clone)]
pub struct StartupStart {
    t0: std::time::Instant,
    boot: String,
}

impl StartupStart {
    pub fn begin() -> Self {
        Self {
            t0: std::time::Instant::now(),
            boot: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Attach the process clock once Tauri has resolved the app-data path.
    /// The first record is backdated by the already-elapsed duration so its
    /// timestamp represents the entrypoint rather than setup().
    pub fn attach(self, dir: std::path::PathBuf) -> Startup {
        let started = Startup {
            dir,
            t0: self.t0,
            boot: self.boot,
            interactive_reported: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let elapsed = started.t0.elapsed().as_millis() as i64;
        started.log_stamp(
            "process_start",
            chrono::Utc::now()
                .timestamp_millis()
                .saturating_sub(elapsed),
            0,
        );
        started.stamp("setup_start");
        let _ = ACTIVE_STARTUP.set(started.clone());
        started
    }
}

#[derive(Clone)]
pub struct Startup {
    dir: std::path::PathBuf,
    t0: std::time::Instant,
    /// Groups one boot's lines together — the log interleaves runs.
    boot: String,
    interactive_reported: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Startup {
    /// Test and utility convenience. Production begins before the Tauri
    /// builder through [`StartupStart`] so it includes pre-setup work.
    #[cfg(test)]
    pub fn begin(dir: std::path::PathBuf) -> Self {
        StartupStart::begin().attach(dir)
    }

    /// Stamp one phase with its elapsed milliseconds since `begin`.
    /// Infallible like every other trace write — see module docs.
    pub fn stamp(&self, phase: &str) {
        self.log_stamp(
            phase,
            chrono::Utc::now().timestamp_millis(),
            self.t0.elapsed().as_millis() as u64,
        );
    }

    fn log_stamp(&self, phase: &str, ts: i64, ms: u64) {
        log_file(
            &self.dir,
            STARTUP_FILE,
            serde_json::json!({
                "ts": ts,
                "version": env!("CARGO_PKG_VERSION"),
                "boot": self.boot,
                "phase": phase,
                "ms": ms,
            }),
        );
    }

    fn stamp_interactive_once(&self) {
        if !self
            .interactive_reported
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            self.stamp("window_interactive");
        }
    }
}

/// Close the startup trace after the main view has initialized, restored its
/// notebook collections, and painted. The frontend owns that readiness gate;
/// recording remains infallible and idempotent for the lifetime of this boot.
pub fn stamp_startup_interactive() {
    if let Some(startup) = ACTIVE_STARTUP.get() {
        startup.stamp_interactive_once();
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
        startup.stamp_interactive_once();
        startup.stamp_interactive_once();

        let text = std::fs::read_to_string(dir.join(super::STARTUP_FILE)).expect("trace written");
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).expect("valid json"))
            .collect();
        let phases: Vec<&str> = lines.iter().map(|l| l["phase"].as_str().unwrap()).collect();
        assert_eq!(
            phases,
            [
                "process_start",
                "setup_start",
                "db_open",
                "setup_done",
                "window_interactive"
            ]
        );
        assert!(lines
            .windows(2)
            .all(|w| w[0]["ms"].as_u64() <= w[1]["ms"].as_u64()));
        assert!(lines.iter().all(|l| l["boot"] == lines[0]["boot"]));
        assert_eq!(lines[0]["version"], env!("CARGO_PKG_VERSION"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
