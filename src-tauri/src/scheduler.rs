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

/// Epoch ms of the last nightly Weave pass, and the window it judges. Hourly
/// rather than per-tick: judging is the expensive stage, and a source that
/// changed two minutes ago is no more urgent than one that changed fifty.
static LAST_WEAVE: AtomicI64 = AtomicI64::new(0);
const WEAVE_EVERY_MS: i64 = 60 * 60 * 1000;

/// Epoch ms of the last tick-driven catch-up sweep (gists, tags, cards,
/// hygiene). Imports and edits kick the same sweep the moment they land —
/// that is the event path, and it is where almost all the work happens.
/// The tick's copy exists for what those miss (a restart mid-sweep, a
/// change made by another process), and once an hour is plenty for that.
/// Every minute was a fan-spinning idle: a sweep that has converged still
/// scans the corpus and asks the small model about anything new it finds,
/// and the model it wakes stays resident for half an hour after.
static LAST_SWEEP: AtomicI64 = AtomicI64::new(0);
const SWEEP_EVERY_MS: i64 = 60 * 60 * 1000;

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
    due_at: i64,
    started_at: i64,
    note: Option<&crate::models::Note>,
    error: Option<&str>,
    cost_micros: i64,
) -> RunReceipt {
    let (provider, model) = engine_attribution(state).await;
    let mut detail = match (note, error) {
        (Some(n), _) => format!("Wrote \u{201c}{}\u{201d}", n.title),
        (None, Some(_)) => String::new(),
        (None, None) => String::new(),
    };
    // Say it on the receipt, not just in the notification: the record is
    // where someone goes to work out why a brief arrived at lunchtime.
    if let Some(late) = lateness(due_at, started_at) {
        detail = if detail.is_empty() {
            late
        } else {
            format!("{detail} \u{00b7} {late}")
        };
    }
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
        // Measured for this run alone (crate::freshness::metered_run), never
        // estimated: only engines that report a price move it, so a local
        // run is genuinely 0 and says so.
        cost_micros,
        due_at,
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
        // Chores have no appointment to be late for.
        due_at: 0,
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

/// When this run *should* have started. Pure, for the same reason `is_due`
/// is: lateness is a claim the app makes to the user, so it has to be
/// derived from persisted state rather than guessed at run time.
///
/// A Mac that slept through 8 AM runs the brief on wake, which is the right
/// behaviour — but showing up at 11:00 with no explanation reads like the
/// schedule is broken. Recording the due time is what lets the app say
/// "this was due at 8:00" instead.
pub(crate) fn due_at(
    s: &crate::models::ReportSchedule,
    events: &[crate::models::SourceEvent],
) -> i64 {
    match s.trigger.as_str() {
        // A commission is due at its floor; "run now" commissions (floor 0)
        // are due from the moment they were created.
        "once" => {
            if s.not_before > 0 {
                s.not_before
            } else {
                s.created_at
            }
        }
        // A standing question became due when the change it answers landed,
        // not when its interval elapsed — that is the moment the user would
        // have wanted to know.
        "change" => events
            .iter()
            .filter(|e| e.notebook_id == s.notebook_id && e.at > s.last_run_at)
            .map(|e| e.at)
            .min()
            .unwrap_or(s.last_run_at),
        // A never-run schedule is due from creation; otherwise one interval
        // past the last run.
        _ => {
            if s.last_run_at == 0 {
                s.created_at
            } else {
                s.last_run_at + s.interval_secs * 1000
            }
        }
    }
}

/// How late is late enough to mention? Under this, the delay is just the
/// pass interval doing its job and saying so would be noise.
pub(crate) const LATE_THRESHOLD_MS: i64 = 15 * 60 * 1000;

/// "3 hours late" / "25 minutes late", or None when it ran on time. Rounded
/// to the unit a person would use — a brief that is 187 minutes late is
/// three hours late.
pub(crate) fn lateness(due_at: i64, started_at: i64) -> Option<String> {
    if due_at <= 0 {
        return None;
    }
    let late_ms = started_at - due_at;
    if late_ms < LATE_THRESHOLD_MS {
        return None;
    }
    let minutes = late_ms / 60_000;
    if minutes < 90 {
        return Some(format!("{minutes} minutes late"));
    }
    let hours = (minutes as f64 / 60.0).round() as i64;
    if hours < 36 {
        return Some(format!("{hours} hours late"));
    }
    let days = (hours as f64 / 24.0).round() as i64;
    Some(format!("{days} days late"))
}

/// Put the Weave's stamp back when a pass could not do its work, so the
/// changes it skipped come round again on the next window instead of being
/// silently written off.
pub(crate) fn rewind_weave_stamp(to: i64) {
    LAST_WEAVE.store(to, Ordering::Relaxed);
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

    // The nightly freshness queue (docs/RFC-night-shift-area.md, freshness.rs).
    // One notch of user control; the priority order is the app's job:
    // keep the corpus current, then judge what arrived, then tidy up. Each
    // stage checks the budget before starting rather than mid-run — stopping
    // a generation half-written wastes what it already spent.
    let budget = {
        let ai = state.ai.read().await;
        ai.config().background_budget.clone()
    };

    // 1. Freshness. Resync is cheap table/mtime work and always runs: a
    //    foregrounded window going stale is worse than a few tokens.
    let _ = commands::resync_sources_inner(app, &state, None).await;

    if crate::freshness::has_budget(&budget) {
        // Distillation, tags, and card suggestions converge even when no
        // fresh import kicks them — a restart mid-sweep used to strand
        // untagged sources until the next import happened to arrive. Hourly
        // from here (imports kick it immediately; see LAST_SWEEP), and the
        // first tick after launch counts, so a restart still catches up.
        if now_ms() - LAST_SWEEP.load(Ordering::Relaxed) >= SWEEP_EVERY_MS {
            LAST_SWEEP.store(now_ms(), Ordering::Relaxed);
            crate::gist::spawn_sweep(state.db.clone(), state.ai.read().await.clone());

            // Source hygiene (docs/RFC-source-hygiene.md): re-fetch aging
            // urls, count strikes on unreachable ones. Its own config gate
            // lives inside; the cadence it works to is days, so hourly is
            // already generous.
            crate::hygiene::spawn_sweep(app);
        }
        // 2. Verification. The Weave already judges a source the moment it
        //    arrives; this catches the case that matters more — a watched
        //    page changed at 3 AM, and the conclusion it undermines was
        //    written in March. Its own stamp, so a Mac that stays awake does
        //    not re-judge the same changes every minute.
        let last_weave = LAST_WEAVE.load(Ordering::Relaxed);
        if now_ms() - last_weave >= WEAVE_EVERY_MS {
            LAST_WEAVE.store(now_ms(), Ordering::Relaxed);
            // Never run is a fresh install, not a licence to judge the whole
            // corpus: start from the last hour, not from the epoch.
            let since = if last_weave == 0 {
                now_ms() - WEAVE_EVERY_MS
            } else {
                last_weave
            };
            crate::commands::weave::spawn_nightly(
                state.db.clone(),
                state.ai.read().await.clone(),
                since,
                budget.clone(),
            );
        }
    } else {
        crate::note!(
            "freshness: nightly budget spent ({} tokens), holding until morning",
            crate::freshness::spent_tonight()
        );
    }

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
        // Pair each run with the time it should have started, computed here
        // while the triggering events are still in hand.
        let due: Vec<_> = schedules
            .into_iter()
            .filter(|s| is_due(s, now, &archived, &events))
            .map(|s| {
                let due_at = due_at(&s, &events);
                (s, due_at)
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
                for (schedule, due_at) in due {
                    let started_at = now_ms();
                    // Meter the run as a unit so its receipt can state what
                    // it cost rather than leaving a column that only ever
                    // reads zero.
                    let (outcome, cost_micros) = crate::freshness::metered_run(
                        commands::run_report_inner(&app, &state, &schedule.id),
                    )
                    .await;
                    match outcome {
                        Ok(note) => {
                            finished += 1;
                            let receipt = schedule_receipt(
                                &state,
                                &schedule,
                                due_at,
                                started_at,
                                Some(&note),
                                None,
                                cost_micros,
                            )
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
                                // A run that slept past its hour says so.
                                // Arriving at lunchtime with no explanation
                                // reads like the schedule is broken; naming
                                // the delay is what keeps "late, not lost"
                                // true from the user's side too.
                                let late = lateness(due_at, started_at);
                                let (title, body) = match (schedule.kind.as_str(), &late) {
                                    ("brief", Some(late)) => (
                                        "Your brief is ready",
                                        format!(
                                            "\u{201c}{}\u{201d} was due while your Mac was asleep \u{2014} {late}.",
                                            schedule.name
                                        ),
                                    ),
                                    ("brief", None) => (
                                        "Your brief is ready",
                                        format!(
                                            "\u{201c}{}\u{201d} has the rundown.",
                                            schedule.name
                                        ),
                                    ),
                                    (_, Some(late)) => (
                                        "Report ready",
                                        format!(
                                            "\u{201c}{}\u{201d} was due while your Mac was asleep \u{2014} {late}.",
                                            schedule.name
                                        ),
                                    ),
                                    (_, None) => (
                                        "Report ready",
                                        format!("\u{201c}{}\u{201d} was generated.", schedule.name),
                                    ),
                                };
                                let _ = app.notification().builder().title(title).body(body).show();
                            }
                        }
                        Err(err) => {
                            crate::diagnostics::error(
                                "night-shift",
                                format!("report {} failed: {err}", schedule.name),
                            );
                            let receipt = schedule_receipt(
                                &state,
                                &schedule,
                                due_at,
                                started_at,
                                None,
                                Some(&err),
                                cost_micros,
                            )
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
    fn a_slept_through_run_reports_when_it_was_due() {
        // The case that prompted this: an 8 AM brief on a Mac that woke at
        // 11. It runs (is_due says so), and the receipt has to carry the
        // hour it was meant for, or the user just sees a brief at lunchtime.
        let eight_am = 8 * HOUR;
        let mut brief = schedule("interval");
        brief.interval_secs = 86_400;
        brief.last_run_at = eight_am - 24 * HOUR;
        assert_eq!(due_at(&brief, &[]), eight_am, "due one interval on");

        let woke_at = 11 * HOUR;
        assert_eq!(lateness(eight_am, woke_at).as_deref(), Some("3 hours late"));

        // A never-run schedule is due from creation, not from epoch zero.
        let mut fresh = schedule("interval");
        fresh.created_at = 5 * HOUR;
        fresh.last_run_at = 0;
        assert_eq!(due_at(&fresh, &[]), 5 * HOUR);
    }

    #[test]
    fn lateness_stays_quiet_about_ordinary_delay() {
        let due = 8 * HOUR;
        // The pass runs once a minute; that is not news.
        assert_eq!(lateness(due, due + 60_000), None);
        assert_eq!(lateness(due, due + LATE_THRESHOLD_MS - 1), None);
        // Nor is a run that beat its due time.
        assert_eq!(lateness(due, due - HOUR), None);
        // An unrecorded due time makes no claim either way.
        assert_eq!(lateness(0, due), None);

        // Past the threshold it speaks, in the unit a person would use.
        assert_eq!(
            lateness(due, due + 25 * 60_000).as_deref(),
            Some("25 minutes late")
        );
        assert_eq!(
            lateness(due, due + 3 * HOUR).as_deref(),
            Some("3 hours late")
        );
        assert_eq!(
            lateness(due, due + 50 * HOUR).as_deref(),
            Some("2 days late"),
            "a long weekend away is stated in days"
        );
    }

    #[test]
    fn a_standing_question_is_due_when_the_change_landed() {
        // Not when its interval elapsed: the moment the user would have
        // wanted to know is the moment the source changed.
        let mut question = schedule("change");
        question.last_run_at = 4 * HOUR;
        let events = [event(9 * HOUR), event(6 * HOUR), event(2 * HOUR)];
        assert_eq!(
            due_at(&question, &events),
            6 * HOUR,
            "the earliest change it has not answered yet"
        );

        // Commissions are due at their floor, or at creation when run-now.
        let mut tonight = schedule("once");
        tonight.not_before = 2 * HOUR;
        assert_eq!(due_at(&tonight, &[]), 2 * HOUR);
        let mut now_job = schedule("once");
        now_job.not_before = 0;
        now_job.created_at = 7 * HOUR;
        assert_eq!(due_at(&now_job, &[]), 7 * HOUR);
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
