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
use crate::models::RunReceipt;

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

/// Epoch ms of the last snapshot attempt. Checked hourly; the job itself is
/// idempotent within a calendar day, so a machine that wakes at odd hours
/// still gets exactly one snapshot per day.
static LAST_SNAPSHOT: AtomicI64 = AtomicI64::new(0);
const SNAPSHOT_EVERY_MS: i64 = 60 * 60 * 1000;

/// Quit for real: mark the exit as intentional, then exit.
pub fn request_quit(app: &AppHandle) {
    QUIT_REQUESTED.store(true, Ordering::Relaxed);
    app.exit(0);
}

/// Record what a run did. Best-effort by contract (docs/RFC-night-shift-area.md
/// §2): a receipt is a description of work, so failing to write one must
/// never fail — or hide — the work itself. Errors are noted and dropped.
pub(crate) async fn write_receipt(state: &AppState, receipt: RunReceipt) {
    if let Err(err) = state.db.add_receipt(&receipt).await {
        crate::diagnostics::error("night-shift", format!("receipt write failed: {err:#}"));
    }
}

/// The provider and model that answered a run, for the receipt's egress
/// line. Read at write time rather than run time: role resolution is stable
/// across a pass, and this keeps the run path untouched.
pub(crate) async fn engine_attribution(state: &AppState) -> (String, String) {
    let ai = state.ai.read().await;
    (
        ai.chat_engine_id(crate::inference::Role::Generate)
            .to_string(),
        ai.active_chat_model(),
    )
}

/// Build a receipt for one scheduled run. `note` is the artifact it wrote,
/// when it wrote one; `error` is the user-facing reason when it did not.
pub(crate) async fn schedule_receipt(
    state: &AppState,
    schedule: &crate::models::ReportSchedule,
    started_at: i64,
    note: Option<&crate::models::Note>,
    error: Option<&str>,
) -> RunReceipt {
    let (provider, model) = engine_attribution(state).await;
    let detail = match (note, error) {
        (Some(n), _) => format!("Wrote \u{201c}{}\u{201d}", n.title),
        (None, Some(_)) => String::new(),
        (None, None) => String::new(),
    };
    RunReceipt {
        id: commands::new_id(),
        schedule_id: schedule.id.clone(),
        notebook_id: schedule.notebook_id.clone(),
        name: schedule.name.clone(),
        kind: schedule.kind.clone(),
        trigger: schedule.trigger.clone(),
        status: if error.is_some() { "failed" } else { "ok" }.into(),
        detail,
        error: error.unwrap_or_default().to_string(),
        note_id: note.map(|n| n.id.clone()).unwrap_or_default(),
        provider,
        model,
        // Only agent CLIs report a price today; local runs are genuinely
        // free. Left at zero rather than estimated - an invented number on a
        // receipt is worse than no number.
        cost_micros: 0,
        started_at,
        ended_at: now_ms(),
    }
}

/// Build a receipt for a housekeeping chore (docs/RFC-night-shift-area.md §7):
/// mechanical, never metered, and attributed to no model.
pub(crate) fn chore_receipt(
    name: &str,
    kind: &str,
    started_at: i64,
    detail: String,
    error: Option<String>,
) -> RunReceipt {
    RunReceipt {
        id: commands::new_id(),
        schedule_id: String::new(),
        notebook_id: String::new(),
        name: name.to_string(),
        kind: kind.to_string(),
        trigger: "chore".into(),
        status: if error.is_some() { "failed" } else { "ok" }.into(),
        detail,
        error: error.unwrap_or_default(),
        note_id: String::new(),
        provider: "local".into(),
        model: String::new(),
        cost_micros: 0,
        started_at,
        ended_at: now_ms(),
    }
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

/// Is this schedule due right now? Pure so the cases that matter — a
/// commission that has already run, one whose hour has not arrived, an
/// archived notebook, a standing question with nothing to answer — are
/// testable without a database or a clock.
///
/// Due-ness stays derived from persisted state (docs/RFC-night-shift.md):
/// a Mac asleep past a due time runs on the first pass after wake, because
/// every comparison here is against wall-clock, not tick count.
pub(crate) fn is_due(
    s: &crate::models::ReportSchedule,
    now: i64,
    archived: &std::collections::HashSet<String>,
    events: &[crate::models::SourceEvent],
) -> bool {
    if !s.enabled || archived.contains(&s.notebook_id) {
        return false;
    }
    match s.trigger.as_str() {
        // A commission runs once, when its hour arrives. `last_run_at` is
        // the guard against a second run: the success path disables the row,
        // but a crash between running and disabling must not re-run the job.
        "once" => s.last_run_at == 0 && now >= s.not_before,
        // A standing question needs both its throttle floor and something
        // to answer.
        "change" => {
            now - s.last_run_at >= s.interval_secs * 1000
                && events
                    .iter()
                    .any(|e| e.notebook_id == s.notebook_id && e.at > s.last_run_at)
        }
        _ => now - s.last_run_at >= s.interval_secs * 1000,
    }
}

/// Take the day's snapshot if it hasn't been taken, and leave a receipt
/// either way. Runs on the pass thread: an APFS clone is a metadata
/// operation, and the fallback copy only happens on volumes that cannot
/// clone at all.
async fn run_snapshot(app: &AppHandle, state: &AppState) {
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };
    // Idempotent per day, so a machine that wakes hourly still snapshots once.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    if let Some((path, _, _)) = crate::backup::latest_snapshot(&data_dir) {
        if path.file_name().map(|n| n.to_string_lossy().to_string()) == Some(today) {
            return;
        }
    }
    let started_at = now_ms();
    let receipt =
        match tokio::task::spawn_blocking(move || crate::backup::snapshot(&data_dir)).await {
            Ok(Ok(out)) => {
                let how = if out.cloned { "cloned" } else { "copied" };
                let mb = out.bytes / (1024 * 1024);
                crate::note!("nightly snapshot {how}: {mb} MB at {}", out.path.display());
                chore_receipt(
                    "Nightly snapshot",
                    "snapshot",
                    started_at,
                    format!("Store {how} \u{00b7} {mb} MB"),
                    None,
                )
            }
            Ok(Err(err)) => {
                crate::diagnostics::error("backup", format!("snapshot failed: {err:#}"));
                chore_receipt(
                    "Nightly snapshot",
                    "snapshot",
                    started_at,
                    String::new(),
                    Some(format!("{err}")),
                )
            }
            Err(err) => chore_receipt(
                "Nightly snapshot",
                "snapshot",
                started_at,
                String::new(),
                Some(format!("{err}")),
            ),
        };
    write_receipt(state, receipt).await;
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
    // The nightly snapshot (docs/RFC-night-shift-area.md §7). Like database
    // maintenance this sits ahead of the background gate on purpose: an APFS
    // clone costs almost nothing, and losing the library is the one failure
    // no other feature can undo.
    if now_ms() - LAST_SNAPSHOT.load(Ordering::Relaxed) >= SNAPSHOT_EVERY_MS {
        LAST_SNAPSHOT.store(now_ms(), Ordering::Relaxed);
        run_snapshot(app, &state).await;
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

    // Source hygiene (docs/RFC-source-hygiene.md): re-fetch aging urls,
    // count strikes on unreachable ones. Single-flight and budgeted like the
    // gist sweep; its own config gate lives inside.
    crate::hygiene::spawn_sweep(app);

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
        let now = now_ms();
        let due: Vec<_> = schedules
            .into_iter()
            .filter(|s| is_due(s, now, &archived, &events))
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
                    let started_at = now_ms();
                    match commands::run_report_inner(&app, &state, &schedule.id).await {
                        Ok(note) => {
                            finished += 1;
                            let receipt =
                                schedule_receipt(&state, &schedule, started_at, Some(&note), None)
                                    .await;
                            write_receipt(&state, receipt).await;
                            // A commission is done when it has run once. The
                            // row stays as its own history; it just never
                            // comes due again.
                            if schedule.trigger == "once" {
                                if let Err(err) =
                                    state.db.set_report_enabled(&schedule.id, false).await
                                {
                                    crate::diagnostics::error(
                                        "night-shift",
                                        format!("could not retire commission: {err:#}"),
                                    );
                                }
                            }
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
                        Err(err) => {
                            crate::diagnostics::error(
                                "night-shift",
                                format!("report {} failed: {err}", schedule.name),
                            );
                            let receipt =
                                schedule_receipt(&state, &schedule, started_at, None, Some(&err))
                                    .await;
                            write_receipt(&state, receipt).await;
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ReportSchedule, SourceEvent};
    use std::collections::HashSet;

    const HOUR: i64 = 3_600_000;

    fn schedule(trigger: &str) -> ReportSchedule {
        ReportSchedule {
            id: "s1".into(),
            notebook_id: "nb".into(),
            name: "Test order".into(),
            kind: "briefing".into(),
            prompt: String::new(),
            trigger: trigger.into(),
            not_before: 0,
            interval_secs: 3_600,
            enabled: true,
            last_run_at: 0,
            created_at: 0,
        }
    }

    fn event(at: i64) -> SourceEvent {
        SourceEvent {
            id: "e1".into(),
            notebook_id: "nb".into(),
            source_id: "src".into(),
            source_title: "A page".into(),
            kind: "updated".into(),
            detail: String::new(),
            diff: String::new(),
            at,
        }
    }

    #[test]
    fn commissions_run_once_when_their_hour_arrives() {
        let none = HashSet::new();
        let now = 10 * HOUR;

        let mut queued = schedule("once");
        queued.not_before = now + HOUR;
        assert!(
            !is_due(&queued, now, &none, &[]),
            "a commission for later tonight is not due yet"
        );

        queued.not_before = now - 1;
        assert!(is_due(&queued, now, &none, &[]), "its hour has arrived");

        // "now" commissions carry no floor at all.
        let immediate = schedule("once");
        assert!(
            is_due(&immediate, now, &none, &[]),
            "run-now starts next pass"
        );

        // Having run once is the guard, so a crash between running and
        // retiring the row cannot produce a second run.
        let mut ran = schedule("once");
        ran.last_run_at = now - HOUR;
        assert!(!is_due(&ran, now, &none, &[]), "never runs twice");

        let mut retired = schedule("once");
        retired.enabled = false;
        assert!(!is_due(&retired, now, &none, &[]));
    }

    #[test]
    fn interval_orders_wait_out_their_interval() {
        let none = HashSet::new();
        let now = 10 * HOUR;

        let fresh = schedule("interval");
        assert!(is_due(&fresh, now, &none, &[]), "never run means due");

        let mut just_ran = schedule("interval");
        just_ran.last_run_at = now - 60_000;
        assert!(!is_due(&just_ran, now, &none, &[]), "inside the interval");

        let mut overdue = schedule("interval");
        overdue.last_run_at = now - 2 * HOUR;
        assert!(is_due(&overdue, now, &none, &[]), "past the interval");

        // The wall-clock comparison is what makes sleep safe: a machine that
        // slept through several intervals runs once on wake, not once per
        // missed tick.
        let mut slept = schedule("interval");
        slept.last_run_at = now - 48 * HOUR;
        assert!(
            is_due(&slept, now, &none, &[]),
            "runs on the pass after wake"
        );
    }

    #[test]
    fn standing_questions_need_something_to_answer() {
        let none = HashSet::new();
        let now = 10 * HOUR;

        let mut question = schedule("change");
        question.last_run_at = now - 2 * HOUR;
        assert!(
            !is_due(&question, now, &none, &[]),
            "no source changed, so nothing to say"
        );
        assert!(
            is_due(&question, now, &none, &[event(now - HOUR)]),
            "a change since the last run pulls the trigger"
        );
        assert!(
            !is_due(&question, now, &none, &[event(now - 3 * HOUR)]),
            "a change it already reported on does not re-fire"
        );

        // The interval is the throttle floor even when changes keep landing.
        let mut throttled = schedule("change");
        throttled.last_run_at = now - 60_000;
        assert!(
            !is_due(&throttled, now, &none, &[event(now - 1_000)]),
            "one run per interval at most"
        );
    }

    #[test]
    fn archived_notebooks_stay_quiet() {
        let now = 10 * HOUR;
        let archived: HashSet<String> = ["nb".to_string()].into_iter().collect();
        assert!(!is_due(&schedule("interval"), now, &archived, &[]));
        assert!(!is_due(&schedule("once"), now, &archived, &[]));
        assert!(!is_due(&schedule("change"), now, &archived, &[event(now)]));
    }
}
