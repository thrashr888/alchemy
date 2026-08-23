//! The Night Shift's resident scheduler (docs/RFC-night-shift.md): the
//! 60-second tick that used to live in the main window's webview, moved into
//! Rust so scheduled reports and source resyncs run with no window open.
//! Due-ness is derived from persisted state each pass — no job queue, nothing
//! to recover after a crash or sleep: a Mac asleep past a due time runs the
//! report on the first tick after wake, because the filter is wall-clock.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::commands::{self, AppState};

/// Set by the explicit quit paths (⌘Q, the app menu's Quit, tray Quit) so
/// `ExitRequested` can tell "the user said quit" from "a window closed."
pub static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Epoch ms until which report runs are snoozed (tray "Pause until morning").
/// Source resync never pauses — it's cheap table/mtime work and pausing it
/// would let a foregrounded window go stale.
static PAUSED_UNTIL: AtomicI64 = AtomicI64::new(0);

/// Epoch ms of the last database maintenance pass. Seeded at start() because
/// launch runs its own pass (lib.rs); the periodic one is for installs that
/// stay resident for days, where Lance versions otherwise pile up between
/// launches.
static LAST_MAINTAIN: AtomicI64 = AtomicI64::new(0);
const MAINTAIN_EVERY_MS: i64 = 6 * 60 * 60 * 1000;

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
        PAUSED_UNTIL.store(next_local_hour_ms(6), Ordering::Relaxed);
        true
    }
}

/// The next occurrence of a local wall-clock hour (6 → pause auto-clear,
/// 7 → the default brief's morning alignment).
pub(crate) fn next_local_hour_ms(hour: u32) -> i64 {
    let now = chrono::Local::now();
    let at = chrono::NaiveTime::from_hms_opt(hour, 0, 0).expect("valid time");
    let mut day = now.date_naive();
    if now.time() >= at {
        day = day.checked_add_days(chrono::Days::new(1)).unwrap_or(day);
    }
    day.and_time(at)
        .and_local_timezone(chrono::Local)
        .earliest()
        .map(|dt| dt.timestamp_millis())
        // DST edge: fall back to eight hours from now.
        .unwrap_or_else(|| now_ms() + 8 * 60 * 60 * 1000)
}

/// Is any Alchemy window focused? Frontmost means the user is already
/// looking — a desktop banner would be noise. Hidden and background windows
/// report unfocused, so tray-resident operation notifies as before.
pub fn app_is_frontmost(app: &AppHandle) -> bool {
    app.webview_windows()
        .values()
        .any(|w| w.is_focused().unwrap_or(false))
}

/// The one notification gate, asked at send time by every desktop
/// notification path: the "Show notifications" preference, plus the
/// quiet-while-focused rule (suppress while a window is focused; the
/// Settings toggle turns that rule off, not on). `first_close_notice`
/// stays exempt on purpose — it explains a disappearance, once.
pub async fn notifications_wanted(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let (on, quiet) = {
        let ai = state.ai.read().await;
        let config = ai.config();
        (config.show_notifications, config.quiet_when_focused)
    };
    on && !(quiet && app_is_frontmost(app))
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
    LAST_MAINTAIN.store(now_ms(), Ordering::Relaxed);
    tauri::async_runtime::spawn(async move {
        // Smart defaults: the daily Morning Brief exists unless the user
        // deleted it (docs/RFC-brief.md) — offered exactly once, ever.
        {
            let state = app.state::<AppState>();
            commands::ensure_default_brief(&state).await;
            // First-run example notebooks, same once-ever marker contract.
            // A first-ever launch may embed ~56 sources here; delaying the
            // opening tick by that much is harmless (it's all background
            // work), and every later launch is a marker stat.
            if crate::examples::ensure_example_notebooks(&state).await {
                // Same event the MCP mutations use, so an already-open home
                // screen shows the new notebooks without a restart.
                let _ = app.emit(
                    "mcp://changed",
                    serde_json::json!({ "scope": "notebooks", "notebookId": null }),
                );
            }
        }
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
    // Database housekeeping every few hours, ahead of the background gate on
    // purpose: pruning dead Lance versions is disk hygiene, not AI spend,
    // and it must run even when background intelligence is switched off.
    if now_ms() - LAST_MAINTAIN.load(Ordering::Relaxed) >= MAINTAIN_EVERY_MS {
        LAST_MAINTAIN.store(now_ms(), Ordering::Relaxed);
        match state.db.maintain().await {
            Ok((bytes, versions)) if versions > 0 => crate::note!(
                "db maintenance: pruned {versions} old versions, reclaimed {} MB",
                bytes / (1024 * 1024)
            ),
            Ok(_) => {}
            Err(err) => {
                crate::diagnostics::error("maintenance", format!("db maintenance failed: {err:#}"))
            }
        }
    }
    let background = {
        let ai = state.ai.read().await;
        ai.config().background_enabled
    };
    if !background {
        crate::integrations::set_tray_status(app, "Background work is off");
        return;
    }

    let _ = commands::resync_sources_inner(app, &state, None).await;

    // Distillation, tags, and card suggestions converge even when no fresh
    // import kicks them — a restart mid-sweep used to strand untagged
    // sources until the next import happened to arrive. Self-gating
    // (SWEEPING) and budgeted, so a converged corpus makes this a cheap
    // no-op pass.
    crate::gist::spawn_sweep(state.db.clone(), state.ai.read().await.clone());

    // Reports now run off-tick (below), so this pass never counts them —
    // the spawned batch stamps the tray itself when it lands.
    let ran = 0u32;
    if !is_paused() {
        let schedules = match state.db.all_report_schedules().await {
            Ok(schedules) => schedules,
            Err(err) => {
                crate::diagnostics::error("night-shift", format!("schedule read failed: {err:#}"));
                Vec::new()
            }
        };
        // Standing questions (trigger "change") fire when a source event
        // landed in their notebook since the last run, with the interval as
        // the throttle floor; one events read serves them all this pass.
        let change_floor = schedules
            .iter()
            .filter(|s| s.enabled && s.trigger == "change")
            .map(|s| s.last_run_at)
            .min();
        let events = match change_floor {
            Some(floor) => state
                .db
                .source_events_since(floor)
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        };
        // Archived notebooks' reports stay quiet — nothing is mutated, so
        // unarchiving resumes the schedule where it left off.
        let archived = state.db.archived_notebook_ids().await.unwrap_or_default();
        let due: Vec<_> = schedules
            .into_iter()
            .filter(|s| {
                if !s.enabled
                    || archived.contains(&s.notebook_id)
                    || now_ms() - s.last_run_at < s.interval_secs * 1000
                {
                    return false;
                }
                match s.trigger.as_str() {
                    "change" => events
                        .iter()
                        .any(|e| e.notebook_id == s.notebook_id && e.at > s.last_run_at),
                    _ => true,
                }
            })
            .collect();
        // Off the tick: a slow report (an agent CLI can legitimately run
        // many minutes, or hang to its deadline) used to stall the entire
        // pass — resyncs and maintenance queued behind one wedged brief.
        // One batch at a time; a tick that lands mid-batch just skips.
        static REPORTS_RUNNING: AtomicBool = AtomicBool::new(false);
        if !due.is_empty() && !REPORTS_RUNNING.swap(true, Ordering::SeqCst) {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<AppState>();
                let mut finished = 0u32;
                for schedule in due {
                    match commands::run_report_inner(&app, &state, &schedule.id).await {
                        Ok(_) => {
                            finished += 1;
                            // At send time, not pass time: a report can run
                            // for minutes, and focus may have changed.
                            if notifications_wanted(&app).await {
                                let (title, body) = if schedule.kind == "brief" {
                                    (
                                        "Your brief is ready",
                                        format!(
                                            "\u{201c}{}\u{201d} has the rundown.",
                                            schedule.name
                                        ),
                                    )
                                } else {
                                    (
                                        "Report ready",
                                        format!("\u{201c}{}\u{201d} was generated.", schedule.name),
                                    )
                                };
                                let _ = app.notification().builder().title(title).body(body).show();
                            }
                        }
                        Err(err) => crate::diagnostics::error(
                            "night-shift",
                            format!("report {} failed: {err}", schedule.name),
                        ),
                    }
                }
                REPORTS_RUNNING.store(false, Ordering::SeqCst);
                if finished > 0 {
                    let stamp = chrono::Local::now().format("%-I:%M %p").to_string();
                    let plural = if finished == 1 { "" } else { "s" };
                    crate::integrations::set_tray_status(
                        &app,
                        &format!("{finished} report{plural} ready \u{00b7} {stamp}"),
                    );
                }
            });
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
