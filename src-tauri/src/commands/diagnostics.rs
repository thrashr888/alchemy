//! The IPC face of `diagnostics.rs` (docs/RFC-diagnostics.md): the front-end
//! reports what it caught, and anything — the UI, an agent over MCP, a
//! support conversation — reads back what has gone wrong lately.

use crate::diagnostics::{self, Event, Level};

/// Record a front-end failure: a render crash, an unhandled rejection, or a
/// command that came back an error. Deliberately infallible — a logger that
/// can fail gives the caller a second error to handle inside its error path,
/// which is how logging turns into a loop.
#[tauri::command]
pub fn log_client_error(
    level: String,
    kind: String,
    message: String,
    detail: Option<String>,
    context: Option<serde_json::Value>,
) {
    let level = match level.as_str() {
        "fatal" => Level::Fatal,
        "warn" => Level::Warn,
        "info" => Level::Info,
        _ => Level::Error,
    };
    let mut event = Event::new(level, "js", kind).message(message);
    if let Some(detail) = detail {
        event = event.detail(detail);
    }
    if let Some(context) = context {
        event = event.context(context);
    }
    diagnostics::record(event);
}

/// Recent records, newest first. `min_level` filters to warn/error/fatal;
/// omitted, everything retained comes back.
#[tauri::command]
pub fn recent_errors(limit: Option<usize>, min_level: Option<String>) -> serde_json::Value {
    let min = min_level.as_deref().and_then(|l| match l {
        "fatal" => Some(Level::Fatal),
        "error" => Some(Level::Error),
        "warn" => Some(Level::Warn),
        "info" => Some(Level::Info),
        _ => None,
    });
    serde_json::json!({
        "summary": diagnostics::summary(),
        "records": diagnostics::recent(limit.unwrap_or(50), min),
    })
}

/// The fatal the backend hit, if any — asked for on mount so a window that
/// reloaded past the `app://fatal` event still shows the restart banner.
#[tauri::command]
pub fn pending_fatal() -> Option<serde_json::Value> {
    diagnostics::last_fatal()
}

/// Show the log in Finder so a user can attach it to a bug report without
/// being talked through a hidden directory.
#[tauri::command]
pub fn reveal_log() -> Result<(), String> {
    let path = diagnostics::log_path();
    // Reveal the folder if the file doesn't exist yet (a clean install that
    // has never failed) rather than erroring at the user.
    let target = if path.exists() {
        path
    } else {
        diagnostics::log_dir()
    };
    std::fs::create_dir_all(diagnostics::log_dir())
        .map_err(|err| format!("could not create the log folder: {err}"))?;
    std::process::Command::new("/usr/bin/open")
        .arg("-R")
        .arg(&target)
        .status()
        .map_err(|err| format!("could not reveal {}: {err}", target.display()))?;
    Ok(())
}
