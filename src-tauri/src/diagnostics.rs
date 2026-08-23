//! Error and crash capture (docs/RFC-diagnostics.md).
//!
//! Alchemy is a GUI app: launched from Finder its stdout and stderr go to
//! `/dev/null`, so the 170-odd `eprintln!`s in this crate are invisible in
//! the installed build, and a failure only ever surfaced as a toast the user
//! then had to describe from memory. This module gives every failure one
//! durable home:
//!
//! - **`~/Library/Logs/com.thrashr888.alchemy/alchemy.log`** — JSONL, one
//!   record per event, rotated at 2 MB. Under `~/Library/Logs` so Console.app
//!   lists it beside the crash reports, and so it survives an app-data reset.
//! - **the unified log** — errors and fatals mirror to `os_log` under the
//!   `com.thrashr888.alchemy` subsystem, so `log stream` and Console.app show
//!   them live, interleaved with the system events around a real crash.
//! - **`recent_errors`** — the same records back out over IPC and MCP, so the
//!   UI and an agent can read what just went wrong without knowing the path.
//!
//! Two rules hold everywhere in here. **Recording must never fail loudly**:
//! every path swallows its own errors, because a logger that panics turns a
//! recoverable bug into a crash. And **fatal must be recoverable**: anything
//! that leaves the app unable to continue emits `app://fatal`, which the
//! front-end turns into a banner with a Restart button rather than a hang.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

/// Rotate at 2 MB keeping one previous generation. Errors are rare by
/// definition — at a few hundred bytes each this is a long history, and the
/// cap matters more than the depth: a render loop can write fast.
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const FILE: &str = "alchemy.log";
const ROTATED: &str = "alchemy.1.log";
const SUBSYSTEM: &str = "com.thrashr888.alchemy";

/// How many records `recent_errors` will hand back at most, however large a
/// limit is asked for — the reader is a chat context or a UI list, not an
/// archive tool.
const MAX_RECENT: usize = 200;

// ---- Record shape ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Not a failure — session markers that make the log readable later.
    Info,
    /// Something went wrong but the app carried on unaffected.
    Warn,
    /// An operation failed. The user probably saw an error.
    Error,
    /// The app (or one window) can't continue without a restart.
    Fatal,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
            Level::Fatal => "fatal",
        }
    }

    /// os_log type: warnings are `default` (kept in memory), errors and
    /// fatals are `error`/`fault` so they persist to disk without the user
    /// having enabled anything.
    fn os_log_type(self) -> u8 {
        match self {
            Level::Info => 0x01,  // OS_LOG_TYPE_INFO
            Level::Warn => 0x00,  // OS_LOG_TYPE_DEFAULT
            Level::Error => 0x10, // OS_LOG_TYPE_ERROR
            Level::Fatal => 0x11, // OS_LOG_TYPE_FAULT
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "info" => Some(Level::Info),
            "warn" => Some(Level::Warn),
            "error" => Some(Level::Error),
            "fatal" => Some(Level::Fatal),
            _ => None,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Level::Info => 0,
            Level::Warn => 1,
            Level::Error => 2,
            Level::Fatal => 3,
        }
    }
}

/// One thing that went wrong. `kind` is a free-form short tag ("panic",
/// "ipc", "render", "unhandled-rejection", "startup") that makes records
/// greppable by failure shape; `context` carries whatever structured extras
/// the call site has.
pub struct Event {
    pub level: Level,
    pub origin: &'static str,
    pub kind: String,
    pub message: String,
    pub detail: Option<String>,
    pub context: Option<serde_json::Value>,
}

impl Event {
    pub fn new(level: Level, origin: &'static str, kind: impl Into<String>) -> Self {
        Event {
            level,
            origin,
            kind: kind.into(),
            message: String::new(),
            detail: None,
            context: None,
        }
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        if !detail.is_empty() {
            self.detail = Some(detail);
        }
        self
    }

    pub fn context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(context);
        self
    }
}

/// `eprintln!` for a GUI app: prints, and never panics doing it.
///
/// `eprintln!` unwraps the write and panics with "failed printing to stderr"
/// when it fails. In a bundled Mac app stderr is whatever the launcher left
/// behind, and when a `pnpm tauri dev` parent terminal exits it becomes a
/// broken pipe — at which point the next print panics from inside whatever
/// thread or Objective-C completion block it happened to run on. That has
/// already aborted Alchemy in the field: a `spotlight::reindex` completion
/// block printing its result took the whole app down with SIGABRT.
///
/// This writes through `writeln!`, which returns the error instead, and
/// drops it. Progress chatter is never worth a crash.
#[macro_export]
macro_rules! note {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

/// Shorthand for the common backend case: an operation failed, the app lives.
pub fn error(kind: &str, message: impl Into<String>) {
    record(Event::new(Level::Error, "rust", kind).message(message));
}

// ---- Destination -----------------------------------------------------------

static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The directory the log lives in. Resolved from the app handle at startup;
/// before that (and in tests) it is derived from `$HOME` so the panic hook
/// installed on the first line of `run()` already has somewhere to write —
/// a panic during setup is exactly the one we can least afford to lose.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = LOG_DIR.get() {
        return dir.clone();
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join("Library/Logs").join(SUBSYSTEM)
}

/// Full path to the current log file — what the UI reveals in Finder and what
/// we ask a user to send.
pub fn log_path() -> PathBuf {
    log_dir().join(FILE)
}

// ---- Writing ---------------------------------------------------------------

/// Suppression state for one (kind, message) pair, so a component that throws
/// on every render writes a handful of lines instead of filling the disk.
struct Throttle {
    /// (kind + message) -> (window start ms, count in window)
    seen: std::collections::HashMap<String, (i64, u32)>,
    /// Records written in the current global window, and when it started.
    window_start: i64,
    window_count: u32,
}

const DUP_WINDOW_MS: i64 = 60_000;
const DUP_LIMIT: u32 = 3;
const GLOBAL_LIMIT: u32 = 120;

static THROTTLE: OnceLock<Mutex<Throttle>> = OnceLock::new();

/// Decide whether this record gets written. Returns the number of records
/// suppressed since the last write of the same key, so the one that does get
/// through can say how much it stands for.
fn admit(key: &str, now: i64) -> Option<u32> {
    let cell = THROTTLE.get_or_init(|| Mutex::new(Throttle::new(now)));
    // A poisoned throttle must not silence logging — that would lose the very
    // records we care most about. Fall back to admitting the write.
    let Ok(mut t) = cell.lock() else {
        return Some(0);
    };
    t.admit(key, now)
}

impl Throttle {
    fn new(now: i64) -> Self {
        Throttle {
            seen: std::collections::HashMap::new(),
            window_start: now,
            window_count: 0,
        }
    }

    fn admit(&mut self, key: &str, now: i64) -> Option<u32> {
        if now - self.window_start > DUP_WINDOW_MS {
            self.window_start = now;
            self.window_count = 0;
            self.seen.clear();
        }
        self.window_count += 1;
        if self.window_count > GLOBAL_LIMIT {
            return None;
        }

        let entry = self.seen.entry(key.to_string()).or_insert((now, 0));
        entry.1 += 1;
        if entry.1 <= DUP_LIMIT {
            Some(0)
        } else if entry.1.is_multiple_of(100) {
            // Every hundredth repeat gets through carrying the running count,
            // so a runaway loop is visible in the log without being the whole
            // log.
            Some(entry.1 - 1)
        } else {
            None
        }
    }
}

/// Record one event: JSONL to the log file, a mirror to the unified log, a
/// line on stderr for `pnpm tauri dev`, and — for fatals — an `app://fatal`
/// event so the UI can offer a restart instead of sitting there broken.
///
/// Infallible by contract. Every failure inside is swallowed.
pub fn record(event: Event) {
    let now = chrono::Utc::now();
    let ts = now.timestamp_millis();
    let key = format!("{}\u{1}{}", event.kind, event.message);
    let Some(suppressed) = admit(&key, ts) else {
        return;
    };

    let mut json = serde_json::json!({
        "ts": ts,
        "time": now.to_rfc3339(),
        "level": event.level.as_str(),
        "origin": event.origin,
        "kind": event.kind,
        "message": event.message,
        "version": env!("CARGO_PKG_VERSION"),
    });
    if let Some(map) = json.as_object_mut() {
        if let Some(detail) = &event.detail {
            map.insert("detail".into(), serde_json::json!(truncate(detail, 8_000)));
        }
        if let Some(context) = &event.context {
            map.insert("context".into(), context.clone());
        }
        if suppressed > 0 {
            map.insert("repeated".into(), serde_json::json!(suppressed));
        }
    }

    // Disk first. Both destinations below can fail in ways that end the
    // process — stderr can be a broken pipe, and the FFI runs on the panic
    // path — so the durable copy lands before either is touched.
    let _ = append(&log_dir(), &json);

    // The terminal, for dev builds and `tauri-browser logs`. Through `note!`,
    // never `eprintln!`: see the macro's own note on why that distinction has
    // teeth.
    crate::note!(
        "[{}] {}: {}",
        event.level.as_str(),
        event.kind,
        event.message
    );

    // Then the system console.
    mirror_to_os_log(event.level, &event.kind, &event.message);

    // And, in dev builds, the debug bridge's ring — so `tauri-browser errors`
    // shows Alchemy's own failures next to the JS errors and panics it
    // captures itself, and an agent driving the app has one place to look.
    #[cfg(feature = "debug")]
    mirror_to_bridge(event.level, &event.kind, &event.message, &event.detail);

    if event.level == Level::Fatal {
        announce_fatal(&json);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}… (truncated)")
}

fn append(dir: &Path, record: &serde_json::Value) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE);
    if std::fs::metadata(&path)
        .map(|m| m.len() > MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join(ROTATED));
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{record}")
}

// ---- Fatal broadcast -------------------------------------------------------

static FATAL: Mutex<Option<serde_json::Value>> = Mutex::new(None);
static FATAL_COUNT: AtomicU64 = AtomicU64::new(0);

/// The last fatal, if one has happened. A window created after the fact (or
/// reloaded past the event) asks for this on mount, so the restart banner
/// survives a reload rather than vanishing with the event that raised it.
pub fn last_fatal() -> Option<serde_json::Value> {
    FATAL.lock().ok().and_then(|f| f.clone())
}

fn announce_fatal(record: &serde_json::Value) {
    FATAL_COUNT.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut slot) = FATAL.lock() {
        *slot = Some(record.clone());
    }
    if let Some(app) = crate::commands::app_handle() {
        let _ = app.emit("app://fatal", record.clone());
    }
}

// ---- Panic hook ------------------------------------------------------------

static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// Install the process-wide panic hook. Called on the first line of `run()`,
/// before the Tauri builder exists, so a panic in `setup` — the class that
/// used to kill the app before it drew a window — still gets recorded.
///
/// A Rust panic is not usually fatal to Alchemy: `panic = unwind` means a
/// panic inside a `#[tauri::command]` unwinds into an `Err` string that the
/// front-end shows. So panics record at `error`, and only the ones that leave
/// the app unusable (startup, the main thread) get raised to `fatal` by their
/// call site.
pub fn install_panic_hook() {
    if HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // A panic raised *inside* a panic hook aborts the process, which
        // would turn every logged panic into a hard crash. Nothing in here
        // should panic, but the guard makes that guarantee cheap.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let message = panic_message(info);
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
            let thread = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_string();
            let backtrace = trim_backtrace(&std::backtrace::Backtrace::force_capture().to_string());
            let count = PANIC_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            record(
                Event::new(Level::Error, "rust", "panic")
                    .message(message.clone())
                    .detail(backtrace)
                    .context(serde_json::json!({
                        "location": location,
                        "thread": thread,
                        "panicsThisSession": count,
                    })),
            );
            escalate_if_wedged(&message, count);
        }));
        previous(info);
    }));
}

static PANIC_COUNT: AtomicU64 = AtomicU64::new(0);

/// Decide whether a panic left the app in a state a restart is the only exit
/// from, and raise the fatal banner if so.
///
/// Most panics are survivable: `panic = unwind` turns one inside a command
/// into an `Err` string, the user sees a message, and the next action works.
/// Two shapes are not survivable:
///
/// - **A poisoned lock.** Alchemy takes `.lock().unwrap()` in 30-odd places.
///   Once a panic poisons one of those mutexes, every later lock of it panics
///   too — for the rest of the process. The feature is dead until restart and
///   nothing the user does will bring it back.
/// - **Panics that keep coming.** A handful of distinct failures in one
///   session means the app is not recovering between them, whatever the
///   individual messages say.
fn escalate_if_wedged(message: &str, count: u64) {
    let poisoned = message.contains("PoisonError") || message.contains("poisoned");
    if !poisoned && count < REPEAT_PANIC_LIMIT {
        return;
    }
    // Announce once. A wedged app produces panics in bursts, and a banner
    // that re-raises on each one can never be read.
    if ESCALATED.swap(true, Ordering::SeqCst) {
        return;
    }
    let reason = if poisoned {
        "Alchemy's internal state was left inconsistent by an earlier error."
    } else {
        "Alchemy has hit several errors in a row and isn't recovering."
    };
    record(
        Event::new(Level::Fatal, "rust", "wedged")
            .message(format!("{reason} Restart to clear it."))
            .context(serde_json::json!({
                "trigger": if poisoned { "poisoned-lock" } else { "repeat-panics" },
                "panicsThisSession": count,
                "lastPanic": message,
            })),
    );
}

static ESCALATED: AtomicBool = AtomicBool::new(false);
const REPEAT_PANIC_LIMIT: u64 = 5;

/// Drop the frames between the capture and the panic itself — this hook,
/// `catch_unwind`, and the panic runtime. They are the same dozen lines every
/// time and they push the frame that actually failed off the top of the
/// record, which is the one thing a reader is looking for.
fn trim_backtrace(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let start = lines
        .iter()
        .rposition(|l| l.contains("rust_begin_unwind") || l.contains("panic_fmt"))
        .map(|i| i + 1)
        .unwrap_or(0);
    // A trim that leaves nothing means the shape wasn't what we expected —
    // an unhelpful backtrace beats a missing one.
    let trimmed = lines[start..].join("\n");
    if trimmed.trim().is_empty() {
        raw.to_string()
    } else {
        trimmed
    }
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic with a non-string payload".to_string()
    }
}

// ---- Startup ---------------------------------------------------------------

/// Point the log at Tauri's own log directory and open the session with a
/// line naming the build. The startup line is what makes "which run was
/// this?" answerable when reading the file weeks later.
pub fn init(app: &tauri::AppHandle) {
    if let Ok(dir) = app.path().app_log_dir() {
        let _ = LOG_DIR.set(dir);
    }
    record(
        Event::new(Level::Info, "rust", "startup")
            .message(format!("Alchemy {} started", env!("CARGO_PKG_VERSION")))
            .context(serde_json::json!({
                "debug": cfg!(debug_assertions),
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
            })),
    );
}

/// Record a startup failure, tell the user in the only way that still works
/// this early — a native dialog — and exit. Without this, a failure before
/// the first window is a bounce in the Dock and nothing else.
pub fn fatal_startup(what: &str, err: &dyn std::fmt::Display) -> ! {
    let message = format!("{what}: {err}");
    record(
        Event::new(Level::Fatal, "rust", "startup")
            .message(message.clone())
            .context(serde_json::json!({ "stage": what })),
    );
    #[cfg(target_os = "macos")]
    native_alert(
        "Alchemy can't start",
        &format!(
            "{message}\n\nThe details were written to:\n{}",
            log_path().display()
        ),
    );
    std::process::exit(1);
}

/// A dialog with no app handle and no event loop assumptions — osascript is
/// the one alerting path that works this early in startup, and a failure to
/// show it must not stop the exit.
#[cfg(target_os = "macos")]
fn native_alert(title: &str, body: &str) {
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display alert \"{}\" message \"{}\" as critical",
        escape(title),
        escape(body)
    );
    let _ = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .status();
}

// ---- Reading back ----------------------------------------------------------

/// The most recent records, newest first. Reads the rotated generation too so
/// a rotation mid-session doesn't hide the error someone is asking about.
pub fn recent(limit: usize, min_level: Option<Level>) -> Vec<serde_json::Value> {
    let dir = log_dir();
    let mut out: Vec<serde_json::Value> = Vec::new();
    // Newest file first; stop as soon as we have enough.
    for file in [FILE, ROTATED] {
        let Ok(text) = std::fs::read_to_string(dir.join(file)) else {
            continue;
        };
        for line in text.lines().rev() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if let Some(min) = min_level {
                let level = value
                    .get("level")
                    .and_then(|l| l.as_str())
                    .and_then(Level::parse);
                match level {
                    Some(l) if l.rank() >= min.rank() => {}
                    _ => continue,
                }
            }
            out.push(value);
            if out.len() >= limit.min(MAX_RECENT) {
                return out;
            }
        }
    }
    out
}

/// Counts by level over the whole retained log — the one-line health summary
/// the UI and the MCP tool lead with.
pub fn summary() -> serde_json::Value {
    let records = recent(MAX_RECENT, None);
    let count = |want: &str| {
        records
            .iter()
            .filter(|r| r.get("level").and_then(|l| l.as_str()) == Some(want))
            .count()
    };
    serde_json::json!({
        "path": log_path().to_string_lossy(),
        "retained": records.len(),
        "fatal": count("fatal"),
        "error": count("error"),
        "warn": count("warn"),
        "info": count("info"),
        "fatalsThisSession": FATAL_COUNT.load(Ordering::Relaxed),
    })
}

// ---- Unified log mirror ----------------------------------------------------

/// Mirror one line into the macOS unified log under our own subsystem, so
/// `log stream --predicate 'subsystem == "com.thrashr888.alchemy"'` and
/// Console.app show Alchemy's errors live and in system context.
///
/// The FFI here is fixed-shape on purpose — one format string, one public
/// string argument, always the same buffer layout — because this runs on the
/// panic path and an encoding bug would be a crash inside a crash. The
/// message is written to disk before this is ever called.
#[cfg(target_os = "macos")]
fn mirror_to_os_log(level: Level, kind: &str, message: &str) {
    use std::ffi::{c_char, c_int, c_void, CString};

    #[link(name = "System", kind = "dylib")]
    extern "C" {
        fn os_log_create(subsystem: *const c_char, category: *const c_char) -> *mut c_void;
        fn _os_log_impl(
            dso: *const c_void,
            log: *mut c_void,
            ty: u8,
            format: *const c_char,
            buf: *mut u8,
            size: u32,
        );
        fn dladdr(addr: *const c_void, info: *mut DlInfo) -> c_int;
    }

    #[repr(C)]
    struct DlInfo {
        dli_fname: *const c_char,
        dli_fbase: *mut c_void,
        dli_sname: *const c_char,
        dli_saddr: *mut c_void,
    }

    // The format string must live in __TEXT so the log decoder can find it by
    // (image uuid, offset) — a string in Rust's default static storage lands
    // in __DATA_CONST and every message renders as "<compose failure>".
    #[link_section = "__TEXT,__cstring"]
    static FORMAT: [u8; 11] = *b"%{public}s\0";

    // One os_log handle per process, plus the mach header of the image that
    // owns FORMAT. Resolved once; if either fails we simply stop mirroring.
    struct Handle(*mut c_void, *const c_void);
    // Safe: os_log_t is documented as thread-safe, and the dso pointer is the
    // image's immutable mach header.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}
    static HANDLE: OnceLock<Option<Handle>> = OnceLock::new();

    let handle = HANDLE.get_or_init(|| unsafe {
        let mut info: DlInfo = std::mem::zeroed();
        if dladdr(FORMAT.as_ptr() as *const c_void, &mut info) == 0 || info.dli_fbase.is_null() {
            return None;
        }
        let subsystem = CString::new(SUBSYSTEM).ok()?;
        let category = CString::new("diagnostics").ok()?;
        let log = os_log_create(subsystem.as_ptr(), category.as_ptr());
        if log.is_null() {
            return None;
        }
        Some(Handle(log, info.dli_fbase))
    });
    let Some(Handle(log, dso)) = handle else {
        return;
    };

    // Unified-log entries are capped; keep the line short and let the JSONL
    // file carry backtraces and context.
    let line = truncate(&format!("[{kind}] {message}"), 900);
    let Ok(text) = CString::new(line.replace('\0', " ")) else {
        return;
    };

    // Argument buffer for a single %{public}s: summary flags, argument count,
    // then (descriptor, size, value) — descriptor 0x22 = public | string.
    let mut buf = [0u8; 12];
    buf[0] = 0x02;
    buf[1] = 0x01;
    buf[2] = 0x22;
    buf[3] = 0x08;
    buf[4..12].copy_from_slice(&(text.as_ptr() as u64).to_ne_bytes());
    unsafe {
        _os_log_impl(
            *dso,
            *log,
            level.os_log_type(),
            FORMAT.as_ptr() as *const c_char,
            buf.as_mut_ptr(),
            buf.len() as u32,
        );
    }
    // `text` must outlive the call — os_log copies the string synchronously.
    drop(text);
}

#[cfg(not(target_os = "macos"))]
fn mirror_to_os_log(_level: Level, _kind: &str, _message: &str) {}

/// Push one record into `tauri-plugin-debug-bridge`, which keeps it in its
/// own ring and mirrors it to `/tmp/tauri-debug-bridge/`. Dev builds only —
/// the bridge is feature-gated off in releases, where `alchemy.log` is the
/// record that matters.
#[cfg(feature = "debug")]
fn mirror_to_bridge(level: Level, kind: &str, message: &str, detail: &Option<String>) {
    use tauri_plugin_debug_bridge::Level as BridgeLevel;
    let bridge_level = match level {
        Level::Info => BridgeLevel::Info,
        Level::Warn => BridgeLevel::Warn,
        Level::Error | Level::Fatal => BridgeLevel::Error,
    };
    match detail {
        Some(detail) => {
            tauri_plugin_debug_bridge::record_detailed(bridge_level, kind, message, detail)
        }
        None => tauri_plugin_debug_bridge::record(bridge_level, kind, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point the log somewhere disposable for a test that writes.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alchemy-diag-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn appends_and_reads_back_newest_first() {
        let dir = temp_dir("readback");
        for i in 0..3 {
            append(
                &dir,
                &serde_json::json!({ "ts": i, "level": "error", "message": format!("m{i}") }),
            )
            .unwrap();
        }
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[2].contains("m2"));
    }

    #[test]
    fn rotates_past_the_cap() {
        let dir = temp_dir("rotate");
        let big = "x".repeat(MAX_BYTES as usize + 1);
        std::fs::write(dir.join(FILE), &big).unwrap();
        append(&dir, &serde_json::json!({ "message": "after" })).unwrap();
        assert!(dir.join(ROTATED).exists(), "previous generation kept");
        let current = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(current.contains("after"));
        assert!(current.len() < 1_000, "current file starts fresh");
    }

    #[test]
    fn duplicate_floods_are_throttled() {
        // A local throttle, not the process-wide one: that is shared with
        // every other test running in parallel, and its window resets under
        // them. Same key a hundred times — the first few write, then one
        // every hundredth carrying the count it stands for.
        let mut t = Throttle::new(0);
        let admitted: Vec<u32> = (0..100).filter_map(|_| t.admit("flood-key", 0)).collect();
        assert_eq!(
            admitted.len(),
            DUP_LIMIT as usize + 1,
            "a repeating error must not fill the log"
        );
        assert_eq!(
            admitted.last(),
            Some(&99),
            "the record that gets through says how many it stands for"
        );
    }

    #[test]
    fn a_new_window_forgives_earlier_repeats() {
        let mut t = Throttle::new(0);
        for _ in 0..10 {
            t.admit("k", 0);
        }
        assert!(
            t.admit("k", DUP_WINDOW_MS + 1).is_some(),
            "an error that recurs a minute later is news again"
        );
    }

    #[test]
    fn the_global_ceiling_holds_against_many_distinct_errors() {
        let mut t = Throttle::new(0);
        let admitted = (0..GLOBAL_LIMIT + 50)
            .filter(|i| t.admit(&format!("k{i}"), 0).is_some())
            .count();
        assert_eq!(admitted, GLOBAL_LIMIT as usize);
    }

    #[test]
    fn distinct_keys_are_not_throttled() {
        let mut t = Throttle::new(0);
        for i in 0..10 {
            assert!(t.admit(&format!("distinct-{i}"), 0).is_some());
        }
    }

    #[test]
    fn levels_round_trip() {
        for level in [Level::Info, Level::Warn, Level::Error, Level::Fatal] {
            assert_eq!(Level::parse(level.as_str()), Some(level));
        }
        assert!(Level::Fatal.rank() > Level::Error.rank());
        assert!(Level::Error.rank() > Level::Warn.rank());
        assert!(Level::Warn.rank() > Level::Info.rank());
    }

    #[test]
    fn truncate_keeps_a_marker() {
        let long = "a".repeat(50);
        let cut = truncate(&long, 10);
        assert!(cut.starts_with("aaaaaaaaaa"));
        assert!(cut.ends_with("(truncated)"));
        assert_eq!(truncate("short", 10), "short");
    }

    #[test]
    fn poisoned_locks_and_repeats_are_the_escalation_rule() {
        // The classifier, isolated from the announce side effect: a poison
        // panic escalates on its first occurrence, an ordinary one only once
        // the session has stopped recovering.
        let wedged = |message: &str, count: u64| {
            message.contains("PoisonError")
                || message.contains("poisoned")
                || count >= REPEAT_PANIC_LIMIT
        };
        assert!(wedged(
            "called `Result::unwrap()` on an `Err` value: PoisonError { .. }",
            1
        ));
        assert!(wedged("mutex is poisoned", 1));
        assert!(!wedged("index out of bounds", 1));
        assert!(!wedged("index out of bounds", REPEAT_PANIC_LIMIT - 1));
        assert!(wedged("index out of bounds", REPEAT_PANIC_LIMIT));
    }

    #[test]
    fn backtrace_trim_starts_at_the_real_frame() {
        let raw = [
            "   0: <std::backtrace::Backtrace>::create",
            "   1: alchemy_lib::diagnostics::install_panic_hook",
            "   2: __rustc::rust_begin_unwind",
            "   3: core::panicking::panic_fmt",
            "   4: alchemy_lib::commands::the_guilty_one",
            "   5: main",
        ]
        .join("\n");
        let trimmed = trim_backtrace(&raw);
        assert!(trimmed.starts_with("   4: alchemy_lib::commands::the_guilty_one"));
        assert!(!trimmed.contains("install_panic_hook"));
    }

    #[test]
    fn backtrace_trim_keeps_an_unfamiliar_shape() {
        let raw = "   0: something_else\n   1: main";
        assert_eq!(trim_backtrace(raw), raw);
        assert_eq!(trim_backtrace(""), "");
    }

    /// The FFI mirror is the one piece that could take the process down. It
    /// must survive every shape of input we could hand it.
    #[test]
    fn os_log_mirror_survives_hostile_input() {
        mirror_to_os_log(Level::Error, "test", "plain");
        mirror_to_os_log(Level::Fatal, "test", "");
        mirror_to_os_log(Level::Warn, "test", &"x".repeat(10_000));
        mirror_to_os_log(Level::Error, "test", "with\0interior\0nuls");
        mirror_to_os_log(
            Level::Error,
            "test",
            "emoji 🧪 and \" quotes % formats %s %@",
        );
    }
}
