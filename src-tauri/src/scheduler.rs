//! The Night Shift's resident scheduler (docs/RFC-night-shift.md): the
//! 60-second tick that used to live in the main window's webview, moved into
//! Rust so scheduled reports and source resyncs run with no window open.
//! Due-ness is derived from persisted state each pass — no job queue, nothing
//! to recover after a crash or sleep: a Mac asleep past a due time runs the
//! report on the first tick after wake, because the filter is wall-clock.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::commands::{self, AppState};

/// Set by the explicit quit paths (⌘Q, the app menu's Quit, tray Quit) so
/// `ExitRequested` can tell "the user said quit" from "a window closed."
pub static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Epoch ms until which report runs are snoozed (tray "Pause until morning").
/// Source resync never pauses — it's cheap table/mtime work and pausing it
/// would let a foregrounded window go stale.
static PAUSED_UNTIL: AtomicI64 = AtomicI64::new(0);

/// Quit for real: mark the exit as intentional, then exit.
pub fn request_quit(app: &AppHandle) {
    QUIT_REQUESTED.store(true, Ordering::Relaxed);
    app.exit(0);
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn is_paused() -> bool {
    now_ms() < PAUSED_UNTIL.load(Ordering::Relaxed)
}

/// Toggle the overnight snooze; returns the new paused state.
pub fn toggle_pause() -> bool {
    if is_paused() {
        PAUSED_UNTIL.store(0, Ordering::Relaxed);
        false
    } else {
        PAUSED_UNTIL.store(next_six_am_ms(), Ordering::Relaxed);
        true
    }
}

/// The next 6:00 AM local, when a pause auto-clears.
fn next_six_am_ms() -> i64 {
    let now = chrono::Local::now();
    let six = chrono::NaiveTime::from_hms_opt(6, 0, 0).expect("valid time");
    let mut day = now.date_naive();
    if now.time() >= six {
        day = day.checked_add_days(chrono::Days::new(1)).unwrap_or(day);
    }
    day.and_time(six)
        .and_local_timezone(chrono::Local)
        .earliest()
        .map(|dt| dt.timestamp_millis())
        // DST edge: fall back to eight hours from now.
        .unwrap_or_else(|| now_ms() + 8 * 60 * 60 * 1000)
}

/// One-time close-to-tray notice, so the first hidden window isn't a mystery.
/// Deliberately ignores the notifications setting — it explains where the
/// app went, once, and never fires again (marker file in the data dir).
pub fn first_close_notice(app: &AppHandle) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let marker = dir.join("residency-notice-seen");
    if marker.exists() {
        return;
    }
    let _ = std::fs::write(&marker, b"1");
    let _ = app
        .notification()
        .builder()
        .title("Alchemy is still working")
        .body("Reports and syncs keep running from the menu bar. Quit from the menu bar icon or \u{2318}Q.")
        .show();
}

/// Spawn the resident loop. Same shape as the MCP and clip-receiver spawns
/// in setup(); the first tick fires immediately, matching the old frontend
/// loop's leading `void tick()`.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            run_pass(&app).await;
        }
    });
}

/// One pass: resync sources, then run due reports sequentially — exactly the
/// work the two frontend ticks did, minus the window requirement. A pass
/// longer than the interval delays the next tick rather than stacking.
async fn run_pass(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (background, notify) = {
        let ai = state.ai.read().await;
        let config = ai.config();
        (config.background_enabled, config.show_notifications)
    };
    if !background {
        crate::integrations::set_tray_status(app, "Background work is off");
        return;
    }

    let _ = commands::resync_sources_inner(app, &state).await;

    let mut ran = 0u32;
    if !is_paused() {
        let schedules = match state.db.all_report_schedules().await {
            Ok(schedules) => schedules,
            Err(err) => {
                eprintln!("night shift: schedule read failed: {err:#}");
                Vec::new()
            }
        };
        let due: Vec<_> = schedules
            .into_iter()
            .filter(|s| s.enabled && now_ms() - s.last_run_at >= s.interval_secs * 1000)
            .collect();
        for schedule in due {
            match commands::run_report_inner(app, &state, &schedule.id).await {
                Ok(_) => {
                    ran += 1;
                    if notify {
                        let _ = app
                            .notification()
                            .builder()
                            .title("Report ready")
                            .body(format!("\u{201c}{}\u{201d} was generated.", schedule.name))
                            .show();
                    }
                }
                Err(err) => eprintln!("night shift: report {} failed: {err}", schedule.name),
            }
        }
    }

    let stamp = chrono::Local::now().format("%-I:%M %p").to_string();
    let status = if is_paused() {
        format!("Paused until morning \u{00b7} synced {stamp}")
    } else if ran > 0 {
        let plural = if ran == 1 { "" } else { "s" };
        format!("{ran} report{plural} ready \u{00b7} synced {stamp}")
    } else {
        format!("Synced {stamp}")
    };
    crate::integrations::set_tray_status(app, &status);
}
