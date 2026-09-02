//! Resident watch over the Apple stores behind Mac sources
//! (docs/RFC-events.md §4, phase 5).
//!
//! cider 0.6's `watch` registers FSEvents on the directories Reminders,
//! Calendar, and Notes write to and folds each burst into one event per
//! store. This module runs one such watch for the life of the app and, on
//! an event, re-fetches every Mac source of that provider across notebooks
//! (`commands::resync_mac_provider`): one cider read and a hash compare per
//! source, a reingest only where the rendering changed, item-level events
//! (`mac::item_events`) where it did. Idle cost is the kernel's, which is
//! zero; the fifteen-minute Mac cadence in the minute sweep stays as belt
//! to these braces.
//!
//! FSEvents only (`watch`, never `watch_via`): cider's CLI-bridge branch and
//! its missing-store path print with `eprintln!`, which panics on a closed
//! stderr and has aborted this app before (diagnostics.rs). The stores are
//! filtered for presence here so the library never reaches that line, and
//! the watch is skipped outright when stderr is already gone. The upstream
//! fix — a quiet library path — is a cider follow-up.
//!
//! Best-effort throughout: a watch that cannot start is recorded once and
//! the sweep carries detection alone. Nothing here may take the app down.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cider::sources::watch::{self, WatchSource};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;

use crate::commands::{self, AppState};
use crate::fswatch::Debouncer;

/// The stores whose contents become sources (mac.rs). Contacts, Home, and
/// Shortcuts are not sources, so they are not watched.
const SOURCES: [WatchSource; 3] = [
    WatchSource::Reminders,
    WatchSource::Calendar,
    WatchSource::Notes,
];

/// cider's own window: raw file events inside it fold into one event per
/// store.
const STORE_DEBOUNCE: Duration = Duration::from_secs(2);

/// The watched stores that exist under `home`. `watch` prints a stderr note
/// for each missing one, and a store that is not there — no Full Disk
/// Access, an app never opened — has nothing to watch anyway.
pub fn present_sources(home: &Path) -> Vec<WatchSource> {
    SOURCES
        .into_iter()
        .filter(|s| watch::store_paths(s, home).iter().all(|p| p.is_dir()))
        .collect()
}

fn names(sources: &[WatchSource]) -> String {
    sources
        .iter()
        .map(|s| s.name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Spawn the watch and its resync loop. Called once from setup.
pub fn start(app: AppHandle) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        crate::note!("macwatch: HOME unset; the minute sweep carries Mac sources");
        return;
    };
    let sources = present_sources(&home);
    if sources.is_empty() {
        crate::note!("macwatch: no Apple stores readable; the minute sweep carries Mac sources");
        return;
    }
    // This line is the log entry and the probe in one: cider announces the
    // watch with `eprintln!`, so a stderr that refuses this write would
    // panic the watch task on its first line.
    if writeln!(std::io::stderr(), "macwatch: watching {}", names(&sources)).is_err() {
        return;
    }
    let (tx, rx) = mpsc::unbounded_channel::<&'static str>();
    tauri::async_runtime::spawn(run(app, rx));
    tauri::async_runtime::spawn(async move {
        let result = watch::watch(&sources, STORE_DEBOUNCE, move |event| {
            // A closed receiver means the loop is gone; nothing to do.
            let _ = tx.send(event.source.name());
        })
        .await;
        match result {
            Ok(()) => crate::note!("macwatch: watch ended; the minute sweep carries Mac sources"),
            Err(err) => crate::diagnostics::error(
                "macwatch",
                format!("watch failed; the minute sweep carries Mac sources: {err:#}"),
            ),
        }
    });
}

/// The resync loop: providers arrive from the watch, sit out a quiet period
/// (the app syncing a list over iCloud writes its store several times a few
/// seconds apart, and one re-fetch is enough), then re-fetch. Same trailing
/// window and hold ceiling as the folder watcher.
async fn run(app: AppHandle, mut rx: mpsc::UnboundedReceiver<&'static str>) {
    let mut deb = Debouncer::default();
    loop {
        let deadline = deb
            .next_deadline()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(24 * 3600));
        tokio::select! {
            ev = rx.recv() => match ev {
                Some(provider) => deb.touch(provider, Instant::now()),
                None => return,
            },
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {}
        }
        for provider in deb.due(Instant::now()) {
            let state = app.state::<AppState>();
            match commands::resync_mac_provider(&app, &state, &provider).await {
                Ok(scan) if scan.changed() => crate::note!(
                    "macwatch: {provider}: ~{} ({} failed)",
                    scan.updated,
                    scan.failed
                ),
                Ok(_) => {}
                Err(err) => {
                    crate::diagnostics::error("macwatch", format!("{provider}: resync: {err}"))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_stores_that_exist_are_watched() {
        let home = tempfile::tempdir().unwrap();
        assert!(present_sources(home.path()).is_empty());
        for path in watch::store_paths(&WatchSource::Reminders, home.path()) {
            std::fs::create_dir_all(path).unwrap();
        }
        assert_eq!(present_sources(home.path()), vec![WatchSource::Reminders]);
        // A store that is a file, not a directory, is not a store.
        for path in watch::store_paths(&WatchSource::Notes, home.path()) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        assert_eq!(present_sources(home.path()), vec![WatchSource::Reminders]);
        assert_eq!(names(&present_sources(home.path())), "reminders");
    }
}
