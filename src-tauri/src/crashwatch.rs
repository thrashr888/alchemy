//! Crashes that leave no panic behind (docs/RFC-diagnostics.md, "As built").
//!
//! `diagnostics.rs` catches everything that goes through Rust's unwind path:
//! panics, failed commands, front-end throws. It cannot catch the two ways
//! Alchemy actually disappears from under a user.
//!
//! - **A native crash.** WKWebView, the Lance/ONNX FFI, or anything else that
//!   raises SIGSEGV/SIGABRT/SIGBUS kills the process outright. Rust's panic
//!   hook never runs. The only record is a `.ips` report macOS writes to
//!   `~/Library/Logs/DiagnosticReports`, which nobody reads.
//! - **A plain exit.** An `exit(3)` from a dependency, a kill, a power loss.
//!   Nothing at all is written anywhere.
//!
//! Both are caught the same way — after the fact, at the next launch:
//!
//! 1. **The running stamp.** Startup writes `running.json` (pid, version,
//!    start time) into app-data and clean shutdown deletes it. A stamp still
//!    present at the next launch means the previous run never reached its
//!    shutdown path. That records at `error` with the previous version, how
//!    long that run lasted, and the last ten log lines it wrote — which is
//!    usually the whole diagnosis.
//! 2. **The crash-report scan.** Off the main thread, once the window is up,
//!    `~/Library/Logs/DiagnosticReports` is read for `.ips` reports newer
//!    than a watermark in app-data whose header names this app. Each becomes
//!    one `fatal` record carrying the exception, the termination reason, and
//!    the crashed thread's top frames. The reports themselves are never
//!    touched — they belong to macOS, and to whatever else reads them.
//!
//! **On signal handlers.** A SIGSEGV/SIGABRT handler writing a last-gasp
//! marker was considered and deliberately skipped. To be correct it may only
//! `write(2)` to a pre-opened fd — no allocation, no formatting, no logging
//! macro — and must re-raise the default action, and it competes with the
//! handlers WKWebView and the system crash reporter install for the same
//! signals. It would buy one bit ("we died by signal") that the `.ips` scan
//! already reports with the exception type and a stack. The stamp covers the
//! signal-free exits. Neither needs a handler, so there isn't one.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::diagnostics::{self, Event, Level};

/// Written at startup, deleted at clean shutdown. Its presence at the next
/// launch is the whole unclean-exit signal.
const STAMP: &str = "running.json";
/// How far the `.ips` scan has already looked, so a report is read once.
const WATERMARK: &str = "crash-scan.json";
/// A first-ever scan looks back this far. Without a floor, a fresh install on
/// an old Mac would replay every crash the machine has ever recorded; with
/// it, the crash that made someone reinstall is still caught.
const FIRST_SCAN_LOOKBACK_MS: i64 = 24 * 60 * 60 * 1000;
/// Log lines carried with an unclean exit. Enough for the failing operation
/// and what led to it, short enough to read inside the record itself.
const TAIL_LINES: usize = 10;
/// Frames kept from the crashed thread. The top of the stack is where the
/// fault is; the rest is the runtime that called into it.
const TOP_FRAMES: usize = 12;

// ---- The running stamp -----------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stamp {
    pid: u32,
    version: String,
    /// Unix ms at the moment the run reached `setup`.
    started: i64,
}

/// What a leftover stamp says about the run that wrote it.
#[derive(Debug, Clone, PartialEq)]
pub struct Unclean {
    pub version: String,
    pub pid: u32,
    pub started: i64,
    /// Milliseconds from that run's start to the last thing it logged, or to
    /// this launch when it logged nothing. Approximate by construction: a
    /// process that vanishes cannot record the moment it stopped.
    pub uptime_ms: i64,
}

/// Where the stamp lives, remembered so shutdown doesn't need the app handle
/// (the exit path runs after state teardown has begun).
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Open the session: report a previous run that never shut down, then leave
/// our own stamp behind. Called from `setup` once app-data is resolved.
pub fn open_session(data_dir: &Path) {
    let _ = DATA_DIR.set(data_dir.to_path_buf());
    let stamp_path = data_dir.join(STAMP);
    let now = chrono::Utc::now().timestamp_millis();

    if let Some(previous) = read_stamp(&stamp_path) {
        if is_running(previous.pid) {
            // A second Alchemy owns that stamp. Not a crash — and not ours to
            // overwrite either, so the file is left alone and this is said
            // once, because it explains any oddity that follows.
            diagnostics::record(
                Event::new(Level::Info, "rust", "session")
                    .message(format!(
                        "Another Alchemy (pid {}) is already running.",
                        previous.pid
                    ))
                    .context(serde_json::json!({ "otherPid": previous.pid })),
            );
            return;
        }
        let unclean = classify(&previous, last_log_ms(), now);
        report_unclean(&unclean);
    }

    write_stamp(
        &stamp_path,
        &Stamp {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            started: now,
        },
    );
}

/// Close the session cleanly. Anything that reaches this leaves no stamp, so
/// the next launch stays quiet.
pub fn close_session() {
    if let Some(dir) = DATA_DIR.get() {
        clear_stamp(dir);
    }
}

/// Turn a leftover stamp plus the log's last timestamp into the shape we
/// report. Split out from the IO so the arithmetic is testable.
fn classify(previous: &Stamp, last_log: Option<i64>, now: i64) -> Unclean {
    // The last line that run wrote is the closest thing to a time of death.
    // A line older than the stamp's own start belongs to an earlier run, and
    // a run that logged nothing at all falls back to this launch — an upper
    // bound, which is the safe direction for "how long did it survive".
    let end = match last_log {
        Some(ts) if ts >= previous.started => ts,
        _ => now,
    };
    Unclean {
        version: previous.version.clone(),
        pid: previous.pid,
        started: previous.started,
        uptime_ms: (end - previous.started).max(0),
    }
}

fn report_unclean(unclean: &Unclean) {
    let tail = log_tail(TAIL_LINES);
    diagnostics::record(
        Event::new(Level::Error, "rust", "unclean-exit")
            .message(format!(
                "Alchemy {} quit without shutting down cleanly after {}.",
                unclean.version,
                human_duration(unclean.uptime_ms)
            ))
            .detail(tail)
            .context(serde_json::json!({
                "previousVersion": unclean.version,
                "previousPid": unclean.pid,
                "startedAt": chrono::DateTime::from_timestamp_millis(unclean.started)
                    .map(|t| t.to_rfc3339()),
                "uptimeMs": unclean.uptime_ms,
            })),
    );
}

fn human_duration(ms: i64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn read_stamp(path: &Path) -> Option<Stamp> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_stamp(path: &Path, stamp: &Stamp) {
    if let Ok(text) = serde_json::to_string(stamp) {
        let _ = std::fs::write(path, text);
    }
}

fn clear_stamp(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(STAMP));
}

/// Is that pid still alive? `kill(pid, 0)` asks without sending anything.
fn is_running(pid: u32) -> bool {
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    // SAFETY: signal 0 performs the existence and permission check only —
    // nothing is delivered to the process.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

// ---- Log tail --------------------------------------------------------------

/// Timestamp of the newest record written before this launch, if any.
fn last_log_ms() -> Option<i64> {
    previous_run_records(1)
        .first()
        .and_then(|r| r.get("ts").and_then(|t| t.as_i64()))
}

/// The last records from before this launch, newest first.
fn previous_run_records(limit: usize) -> Vec<serde_json::Value> {
    let boundary = diagnostics::session_start_ms();
    diagnostics::recent(diagnostics::MAX_RECENT, None)
        .into_iter()
        .filter(|r| {
            r.get("ts")
                .and_then(|t| t.as_i64())
                .is_some_and(|ts| ts < boundary)
        })
        .take(limit)
        .collect()
}

/// The tail as prose, oldest first — the order someone reads a log in.
fn log_tail(limit: usize) -> String {
    let mut lines: Vec<String> = previous_run_records(limit)
        .iter()
        .map(format_record)
        .collect();
    lines.reverse();
    lines.join("\n")
}

fn format_record(record: &serde_json::Value) -> String {
    let field = |name: &str| record.get(name).and_then(|v| v.as_str()).unwrap_or("");
    format!(
        "{} [{}] {}: {}",
        field("time"),
        field("level"),
        field("kind"),
        field("message")
    )
}

// ---- Crash reports ---------------------------------------------------------

/// One parsed `.ips` report, reduced to what a reader needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrashReport {
    pub file: String,
    pub app_version: String,
    pub os_version: String,
    pub time: String,
    /// "EXC_BAD_ACCESS (SIGSEGV)", or the termination indicator when the
    /// report carries no exception block.
    pub cause: String,
    pub termination: String,
    pub faulting_thread: Option<i64>,
    /// Top frames of the crashed thread, one per line.
    pub frames: Vec<String>,
}

impl CrashReport {
    fn summary(&self) -> String {
        let cause = if self.cause.is_empty() {
            "an unknown fault".to_string()
        } else {
            self.cause.clone()
        };
        let version = if self.app_version.is_empty() {
            String::new()
        } else {
            format!(" {}", self.app_version)
        };
        format!("Alchemy{version} crashed: {cause}")
    }
}

/// The notice this launch owes the user, if any. Set by the scan, read by the
/// front-end. Deliberately not persisted: the point is to mention a crash on
/// the launch after it, not forever.
static NOTICE: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// The one-line notice for the banner, or `None` when this launch found no
/// new crash reports.
pub fn notice() -> Option<serde_json::Value> {
    NOTICE.lock().ok().and_then(|n| n.clone())
}

/// Scan for crash reports written since the last scan and record each one.
/// Runs off the main thread after the window is up: it reads a directory that
/// can hold hundreds of files, and nothing here is worth a slower launch.
pub fn scan_reports(data_dir: &Path) {
    let Some(dir) = reports_dir() else {
        return;
    };
    let now = chrono::Utc::now().timestamp_millis();
    let watermark_path = data_dir.join(WATERMARK);
    let since = read_watermark(&watermark_path).unwrap_or(now - FIRST_SCAN_LOOKBACK_MS);

    let mut found = 0usize;
    let mut latest = String::new();
    for path in candidates(&dir, since) {
        match path.extension().and_then(|e| e.to_str()) {
            Some("ips") => {
                let Some(report) = parse_ips_file(&path) else {
                    continue;
                };
                record_crash(&report);
                found += 1;
                latest = report.summary();
            }
            // The pre-Monterey text format. Parsing it means a second parser
            // for a shape this app has almost certainly never crashed in;
            // saying it exists, with its path, is enough to go read it.
            Some("crash") => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if !name.to_lowercase().starts_with("alchemy") {
                    continue;
                }
                diagnostics::record(
                    Event::new(Level::Fatal, "rust", "crash-report")
                        .message(format!("Alchemy crashed: see {name}"))
                        .context(serde_json::json!({
                            "file": path.to_string_lossy(),
                            "format": "legacy-crash",
                        }))
                        .quiet(),
                );
                found += 1;
                latest = "Alchemy crashed".to_string();
            }
            _ => {}
        }
    }

    write_watermark(&watermark_path, now);

    if found > 0 {
        if let Ok(mut slot) = NOTICE.lock() {
            *slot = Some(serde_json::json!({
                "count": found,
                "summary": latest,
                "logPath": diagnostics::log_path().to_string_lossy(),
            }));
        }
    }
}

/// `~/Library/Logs/DiagnosticReports`, where macOS files crash reports for
/// the logged-in user.
fn reports_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join("Library/Logs/DiagnosticReports");
    dir.is_dir().then_some(dir)
}

/// Reports modified since the watermark, oldest first. Modification time is
/// the filter because it needs no read; the header decides whether the file
/// is ours.
fn candidates(dir: &Path, since: i64) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(i64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str())?;
            if ext != "ips" && ext != "crash" {
                return None;
            }
            let modified = entry
                .metadata()
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_millis() as i64;
            (modified > since).then_some((modified, path))
        })
        .collect();
    out.sort_by_key(|(ms, _)| *ms);
    out.into_iter().map(|(_, path)| path).collect()
}

fn read_watermark(path: &Path) -> Option<i64> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("scanned")
        .and_then(|v| v.as_i64())
}

fn write_watermark(path: &Path, now: i64) {
    let _ = std::fs::write(path, serde_json::json!({ "scanned": now }).to_string());
}

fn record_crash(report: &CrashReport) {
    diagnostics::record(
        Event::new(Level::Fatal, "rust", "crash-report")
            .message(report.summary())
            .detail(report.frames.join("\n"))
            .context(serde_json::json!({
                "file": report.file,
                "appVersion": report.app_version,
                "osVersion": report.os_version,
                "crashedAt": report.time,
                "cause": report.cause,
                "termination": report.termination,
                "faultingThread": report.faulting_thread,
            }))
            // Quiet on purpose: this is a fatal that already happened, to a
            // process that is gone. Raising the restart screen over it would
            // ask the user to fix a run that ended yesterday.
            .quiet(),
    );
}

/// Read a `.ips` only as far as it takes to know it is ours: the header is
/// the first line, and reports from other apps are most of that directory.
fn parse_ips_file(path: &Path) -> Option<CrashReport> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut header = String::new();
    reader.read_line(&mut header).ok()?;
    if !is_ours(&header) {
        return None;
    }
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut body).ok()?;
    // Lossy on purpose: a byte we can't decode must not lose a crash we do
    // want, and the header has already said this file is ours.
    let mut report = parse_ips(&header, &String::from_utf8_lossy(&body))?;
    report.file = path.to_string_lossy().to_string();
    Some(report)
}

fn is_ours(header_line: &str) -> bool {
    let Ok(header) = serde_json::from_str::<serde_json::Value>(header_line) else {
        return false;
    };
    let field = |name: &str| {
        header
            .get(name)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
    };
    field("bundleID") == "com.thrashr888.alchemy"
        || field("app_name").eq_ignore_ascii_case("alchemy")
        || field("coalitionName") == "com.thrashr888.alchemy"
}

/// Parse the two halves of an `.ips`: a one-line JSON header, then the report
/// body as one more JSON object. Every field is optional — the format has
/// changed between macOS releases, and a partial summary beats none.
pub fn parse_ips(header_line: &str, body_text: &str) -> Option<CrashReport> {
    let header: serde_json::Value = serde_json::from_str(header_line).ok()?;
    let body: serde_json::Value = serde_json::from_str(body_text.trim()).unwrap_or_default();

    let str_at = |value: &serde_json::Value, path: &[&str]| -> String {
        let mut cursor = value;
        for key in path {
            match cursor.get(key) {
                Some(next) => cursor = next,
                None => return String::new(),
            }
        }
        cursor.as_str().unwrap_or_default().to_string()
    };

    let exception_type = str_at(&body, &["exception", "type"]);
    let signal = str_at(&body, &["exception", "signal"]);
    let indicator = str_at(&body, &["termination", "indicator"]);
    let cause = match (exception_type.is_empty(), signal.is_empty()) {
        (false, false) => format!("{exception_type} ({signal})"),
        (false, true) => exception_type,
        (true, false) => signal,
        (true, true) => indicator.clone(),
    };
    let termination = {
        let namespace = str_at(&body, &["termination", "namespace"]);
        let by = str_at(&body, &["termination", "byProc"]);
        [namespace, indicator, by]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    };

    let faulting = body.get("faultingThread").and_then(|t| t.as_i64());
    Some(CrashReport {
        file: String::new(),
        app_version: str_at(&header, &["app_version"]),
        os_version: str_at(&header, &["os_version"]),
        time: str_at(&header, &["timestamp"]),
        cause,
        termination,
        faulting_thread: faulting,
        frames: top_frames(&body, faulting),
    })
}

/// The crashed thread's top frames, resolved against `usedImages` so each
/// line names the binary it came from — "which library faulted" is the first
/// question a report has to answer.
fn top_frames(body: &serde_json::Value, faulting: Option<i64>) -> Vec<String> {
    let images: Vec<&str> = body
        .get("usedImages")
        .and_then(|i| i.as_array())
        .map(|images| {
            images
                .iter()
                .map(|image| image.get("name").and_then(|n| n.as_str()).unwrap_or("?"))
                .collect()
        })
        .unwrap_or_default();

    let Some(threads) = body.get("threads").and_then(|t| t.as_array()) else {
        return Vec::new();
    };
    // Without a faultingThread index, fall back to a thread that claims to be
    // the triggering one, then to the first.
    let index = faulting
        .map(|i| i as usize)
        .or_else(|| threads.iter().position(|t| t.get("triggered").is_some()))
        .unwrap_or(0);
    let Some(frames) = threads
        .get(index)
        .and_then(|t| t.get("frames"))
        .and_then(|f| f.as_array())
    else {
        return Vec::new();
    };
    frames
        .iter()
        .take(TOP_FRAMES)
        .enumerate()
        .map(|(depth, frame)| {
            let image = frame
                .get("imageIndex")
                .and_then(|i| i.as_u64())
                .and_then(|i| images.get(i as usize).copied())
                .unwrap_or("?");
            let offset = frame
                .get("imageOffset")
                .and_then(|o| o.as_u64())
                .unwrap_or(0);
            match frame.get("symbol").and_then(|s| s.as_str()) {
                Some(symbol) => {
                    let at = frame
                        .get("symbolLocation")
                        .and_then(|s| s.as_u64())
                        .unwrap_or(0);
                    format!("#{depth} {image} {symbol} + {at}")
                }
                None => format!("#{depth} {image} +0x{offset:x}"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alchemy-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A minimal two-line report in the shape macOS writes: a JSON header
    /// line naming the app, then the body.
    fn fixture_ips() -> (String, String) {
        let header = serde_json::json!({
            "app_name": "Alchemy",
            "timestamp": "2026-09-03 22:10:41.0000 -0700",
            "app_version": "0.55.0",
            "bundleID": "com.thrashr888.alchemy",
            "os_version": "macOS 27.0 (27A123)",
            "incident_id": "AAAA-BBBB",
        })
        .to_string();
        let body = serde_json::json!({
            "exception": { "type": "EXC_BAD_ACCESS", "signal": "SIGSEGV",
                           "subtype": "KERN_INVALID_ADDRESS at 0x0000000000000010" },
            "termination": { "namespace": "SIGNAL", "indicator": "Segmentation fault: 11",
                             "byProc": "exc handler" },
            "faultingThread": 1,
            "usedImages": [ { "name": "Alchemy" }, { "name": "WebKit" } ],
            "threads": [
                { "frames": [ { "imageIndex": 0, "imageOffset": 16, "symbol": "not_this_one" } ] },
                { "frames": [
                    { "imageIndex": 1, "imageOffset": 4660, "symbol": "WebCore::paint",
                      "symbolLocation": 42 },
                    { "imageIndex": 0, "imageOffset": 291 }
                ] }
            ]
        })
        .to_string();
        (header, body)
    }

    #[test]
    fn parses_a_crash_report_down_to_the_faulting_frames() {
        let (header, body) = fixture_ips();
        let report = parse_ips(&header, &body).expect("the fixture parses");
        assert_eq!(report.cause, "EXC_BAD_ACCESS (SIGSEGV)");
        assert_eq!(report.app_version, "0.55.0");
        assert_eq!(report.faulting_thread, Some(1));
        assert!(report.termination.contains("Segmentation fault: 11"));
        // The faulting thread, not thread 0 — reporting the wrong stack is
        // worse than reporting none.
        assert_eq!(report.frames[0], "#0 WebKit WebCore::paint + 42");
        assert_eq!(report.frames[1], "#1 Alchemy +0x123");
        assert!(report
            .summary()
            .starts_with("Alchemy 0.55.0 crashed: EXC_BAD_ACCESS"));
    }

    #[test]
    fn a_report_from_another_app_is_not_ours() {
        let (header, _body) = fixture_ips();
        assert!(is_ours(&header));
        assert!(!is_ours(
            &serde_json::json!({ "app_name": "Safari", "bundleID": "com.apple.Safari" })
                .to_string()
        ));
        assert!(!is_ours("not json at all"));
        // Body damage must not lose the header's facts.
        let partial = parse_ips(&header, "{}").expect("the header alone still parses");
        assert_eq!(partial.app_version, "0.55.0");
        assert!(partial.frames.is_empty());
        assert!(partial.summary().contains("crashed"));
    }

    #[test]
    fn the_stamp_lifecycle_detects_only_the_run_that_never_finished() {
        let dir = temp_dir("stamp");
        let path = dir.join(STAMP);

        // Nothing yet: a first launch has nothing to report.
        assert!(read_stamp(&path).is_none());

        // A run that shut down cleanly leaves nothing behind.
        write_stamp(
            &path,
            &Stamp {
                pid: std::process::id(),
                version: "0.55.0".into(),
                started: 1_000,
            },
        );
        clear_stamp(&dir);
        assert!(read_stamp(&path).is_none(), "clean shutdown clears it");

        // A run that vanished leaves its stamp; the next launch reads it,
        // dates it from the last thing it logged, and replaces it with its
        // own — so the crash is reported once, not on every launch after.
        let previous = Stamp {
            pid: 424_242,
            version: "0.54.0".into(),
            started: 10_000,
        };
        write_stamp(&path, &previous);
        let found = read_stamp(&path).expect("the stamp survives a restart");
        let unclean = classify(&found, Some(70_000), 900_000);
        assert_eq!(unclean.version, "0.54.0");
        assert_eq!(unclean.uptime_ms, 60_000);

        write_stamp(
            &path,
            &Stamp {
                pid: std::process::id(),
                version: "0.55.0".into(),
                started: 900_000,
            },
        );
        let replaced = read_stamp(&path).unwrap();
        assert_eq!(replaced.version, "0.55.0");
        assert_eq!(replaced.pid, std::process::id());
    }

    #[test]
    fn uptime_falls_back_when_the_log_says_nothing_useful() {
        let previous = Stamp {
            pid: 1,
            version: "0.1.0".into(),
            started: 5_000,
        };
        // No log lines at all: bounded by this launch.
        assert_eq!(classify(&previous, None, 8_000).uptime_ms, 3_000);
        // A line older than that run belongs to an earlier one.
        assert_eq!(classify(&previous, Some(1_000), 8_000).uptime_ms, 3_000);
        // Clock skew must never produce a negative lifetime.
        assert_eq!(classify(&previous, None, 4_000).uptime_ms, 0);
    }

    #[test]
    fn the_watermark_admits_a_report_once() {
        let dir = temp_dir("watermark");
        let reports = dir.join("DiagnosticReports");
        std::fs::create_dir_all(&reports).unwrap();
        let path = reports.join("Alchemy-2026-09-03-221041.ips");
        let (header, body) = fixture_ips();
        std::fs::write(&path, format!("{header}\n{body}\n")).unwrap();

        let watermark = dir.join(WATERMARK);
        assert!(read_watermark(&watermark).is_none(), "no scan yet");

        let modified = std::fs::metadata(&path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        // Before the watermark it is new; after it, it is already reported.
        assert_eq!(candidates(&reports, modified - 1_000).len(), 1);
        assert!(candidates(&reports, modified + 1_000).is_empty());

        write_watermark(&watermark, modified + 1_000);
        assert_eq!(read_watermark(&watermark), Some(modified + 1_000));
        assert!(candidates(&reports, read_watermark(&watermark).unwrap()).is_empty());

        // Files belonging to other tools are never candidates.
        std::fs::write(reports.join("something.diag"), "x").unwrap();
        assert!(candidates(&reports, modified - 1_000)
            .iter()
            .all(|p| p.extension().unwrap() == "ips"));

        // And the file we did read parses end to end, with its path attached.
        let parsed = parse_ips_file(&path).expect("our own report");
        assert_eq!(parsed.file, path.to_string_lossy());
        assert_eq!(parsed.faulting_thread, Some(1));
    }

    #[test]
    fn durations_read_like_durations() {
        assert_eq!(human_duration(0), "0s");
        assert_eq!(human_duration(45_000), "45s");
        assert_eq!(human_duration(65_000), "1m 5s");
        assert_eq!(human_duration(3 * 3_600_000 + 4 * 60_000), "3h 4m");
    }
}
