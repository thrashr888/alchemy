# RFC: The Night Shift — resident scheduler + tray residency

## Summary

Move the two 60-second tick loops out of the frontend and into a resident Rust
scheduler, and keep the app alive in the tray when the last window closes.
This is the keystone of [RFC-v12-steward.md](RFC-v12-steward.md): every
nocturnal capability (watchers, standing questions, the Brief, overnight
budgets) stands on a process that exists while no window does.

It also fixes a real defect today: **scheduled reports only fire while the
main window is open.** The backend does all the work — `run_report`
(commands/reports.rs:113) refreshes URL sources, threads the prior run,
generates, persists, and stamps `last_run_at` — but the *tick* lives in
`startReportScheduler` (store.ts:1459), a `setInterval` in the main window's
webview. Close the window, and a "daily" report silently isn't.

## Background

Two frontend loops, both 60s `setInterval`s gated to the main window's
webview, both thin wrappers over backend commands:

- `startSourceSync` (store.ts:824) → `api.resyncSources()` — folder scans,
  Mac-app resyncs, git pulls, iCloud hydration. The backend serializes scans;
  changed notebooks announce via `sources://changed`.
- `startReportScheduler` (store.ts:1459) → `list_all_report_schedules`,
  filters `enabled && now - lastRunAt >= intervalSecs * 1000`, runs each due
  schedule sequentially via `api.runReport`, posts a notification, refreshes
  the open notebook's notes.

Everything else the loops touch is already backend-resident and
window-independent:

- `run_report` needs only `AppHandle` + `AppState`; it already emits
  `report://step` and `generate://done` events that any window *may* observe.
  Due-ness is derived from the persisted `last_run_at`
  (`set_report_last_run`), so there is no job queue to corrupt — the schedule
  table self-heals on wake, the same idiom as `gist::ensure_gists`.
- `setup()` already spawns resident background services on the async runtime:
  the MCP server (`mcp::apply_config`) and the clip receiver
  (`clip::apply_config`). The scheduler is the third.
- The tray exists (`integrations.rs:260`, id `alchemy-tray`), with recents
  mutated in place. But it dies with the last window: nothing handles
  `CloseRequested` or `ExitRequested`, so the tray is a *launcher*, not
  residency.
- Idle tracking exists: `touch_activity()` (commands.rs:2854) is stamped by
  command traffic and `idle_ms()` (commands.rs:2858) already gates the
  curator's consolidation pass.

## Proposal

### 1. `scheduler.rs`: one resident task, timestamp-driven

A new `src-tauri/src/scheduler.rs` spawned in `setup()` after state exists
(same shape as the MCP/clip spawns):

```rust
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            resync_sources_inner(&app).await;        // cheap; never paused
            if !scheduler_paused(&app) {
                run_due_reports(&app).await;         // sequential, one at a time
            }
        }
    });
}
```

One piece of enabling work this implies: `resync_sources` and `run_report`
are single `#[tauri::command]` functions today with their bodies inline —
each gets its body extracted into an `_inner` function the scheduler calls
directly (house precedent: `rescan_one_folder_inner`, commands.rs:2248; the
command wrappers keep their signatures). `gist::spawn_sweep` (gist.rs:591) is
the ownership model to copy — it takes owned `Arc<Db>` + `Ai` so the resident
task needs no window, and its `SWEEPING` atomic guard (gist.rs:595) is the
one-pass-at-a-time pattern the scheduler reuses.

Design points, all inherited from what the frontend loop already does right:

- **Timestamp-derived due-ness, no queue.** `run_due_reports` re-reads
  `all_report_schedules` each tick and filters by `last_run_at` — identical
  logic to store.ts:1470. Sleep/wake needs no special handling: a Mac asleep
  past a due time runs the report on the first tick after wake, because the
  filter is on wall-clock, not tick count. `MissedTickBehavior::Delay` keeps
  wake from bursting a backlog of ticks.
- **Sequential and overlap-proof.** One loop, `await`ed serially, exactly like
  today's `for (const s of due)`. A run longer than 60s delays the next tick
  rather than stacking (the guard the frontend got implicitly from `await`).
- **Notifications move to Rust — and so does their preference.** The
  scheduler posts "Report ready" through `tauri_plugin_notification`'s Rust
  API (registered at lib.rs:50; currently only the JS side is used) — it must
  work with zero windows. Today the "Show notifications" setting lives in
  localStorage (`notify.ts:12`), which Rust cannot read; it migrates into
  `AiConfig` beside `tray_enabled`, and `notify.ts` reads it from there so
  the setting stays one toggle. Open windows keep updating live exactly as
  they do now, via the events `run_report` already emits; the
  `generate://done` listener replaces the scheduler-loop's manual `listNotes`
  refresh.
- **Command-surface unchanged.** `resync_sources` and `run_report` stay
  invocable (Run Now buttons, MCP `schedule_report` flows). The scheduler
  calls their inner functions, not the Tauri command wrappers.

The frontend deletes `startReportScheduler`, `startSourceSync`, both
module-level guards, their boot calls, and their declarations in
`storeTypes.ts:163/170` — a net negative diff.

### 2. Residency: close-to-tray, explicit quit

- **`CloseRequested` on the main window** (when the tray is enabled): hide the
  window and `api.prevent_close()`. Child windows (notes, mind maps) keep
  their default close behavior.
- **`RunEvent::ExitRequested`**: `api.prevent_exit()` unless quitting was
  explicit — ⌘Q, the app menu's Quit, or a new **"Quit Alchemy"** item at the
  bottom of the tray menu (which currently has no way to quit at all).
- **Reopen paths already exist:** `focus_main` (integrations.rs:41) handles
  the tray's Open, deep links, Services, and Spotlight; add
  `RunEvent::Reopen` (Dock click) → `focus_main`. Hidden → shown is exactly
  the summon path every intent already funnels through.
- **Tray-off means residency-off.** If the user hides the menu bar icon
  (Settings → General), window close quits the app as today — a background
  process with no visible affordance is a trap, not a feature. The Settings
  row explains the coupling in one line.
- **One switch stops everything.** Settings → General gains **"Background
  work"** (default on): off suspends the scheduler's work entirely — no
  resyncs, no report runs, and every future background family honors the same
  flag. The app reverts to on-demand behavior; Run Now and manual Refresh
  still work. Distinct from the tray toggle (presence) and from "Pause until
  morning" (an overnight snooze of report runs only).
- **Activation policy stays Regular.** Flipping to Accessory when windowless
  would remove the Dock icon but interacts badly with the build-once app menu
  (rebuilding clears AppKit's Window list — see menu.rs). Not worth it in v1.
- **Launch at login, default ON** (`tauri-plugin-autostart`, SMAppService):
  residency that ends at reboot is half a steward. macOS discloses login
  items itself in System Settings and notifies on registration, and the
  toggle sits beside "Show menu bar icon." Smart defaults ship on; the toggle
  is cost control.

### 3. The tray learns what the staff did

Static items today; two additions, both mutate-in-place on the existing tray
menu (never the app menu):

- **Status line** (disabled item, top): "Last night: 2 reports · sources
  synced 4 min ago", updated after each scheduler pass. Clicking the adjacent
  **"Open Reports"** focuses the main window on the reports feed.
- **"Pause until morning"** (toggles to "Resume"): sets a `scheduler_paused`
  flag checked each tick; auto-clears at the next 6 AM local. Named
  distinctly because "paused" already means a single schedule's disabled
  state in the agent-facing surface (commands.rs:4404). It gates report runs
  only — source resync is cheap table/mtime work and pausing it would let a
  foregrounded window go stale. For loud fans at the wrong moment, not for
  safety.

The fuller steward face (brief items, due-soon obligations, Log…) arrives
with the Brief and the Ledger; the menu structure this RFC adds is where they
land.

### 4. Staged next, on this foundation

Named here so the seams are cut in the right place; each is its own change
with its own eval delta, in dependency order:

1. **Watchers** — promote each resync class (URL, folder, git, Mac apps) to
   keep a content hash + prior text snapshot and write a `source_events` row
   on real change (the one new Lance table this program allows itself;
   precedent `report_schedules`). Diff summaries by the Small role, gated by
   the `gist.rs` discipline. URL sources gain a resync cadence outside
   `run_report` — today `resync_sources` explicitly skips web URLs
   (commands.rs:3239), so they refresh only when a report happens to run.
2. **Standing questions** — `ReportSchedule` gains an additive
   `trigger: "interval" | "change"` field; a change-triggered schedule fires
   when a `source_events` row lands in its notebook, debounced to one run per
   tick. "When the 10-K drops, tell me what changed" becomes a report whose
   clock is an event.
3. **The overnight budget** — an overnight `ContextProfile` tier in
   `inference/mod.rs`; when `idle_ms()` is long and the schedule allows, the
   agent loop's caps (`MAX_STEPS = 5`, `READ_CHARS_*` in agent.rs) become a
   budget of steps, wall-clock, and dollars. (`idle_ms` is private today —
   commands.rs:2858 — and gets `pub(crate)` when the scheduler grows an idle
   gate.) Deep runs gate on idle; ordinary scheduled reports never wait for
   idle — a due report is user intent.

## What this deliberately does not do

- **No launchd daemon, no helper process.** The app process in the tray *is*
  the resident staff. A second process means a second lifecycle, IPC, and an
  update story — the exact complexity this program refuses.
- **No job queue table.** Due-ness stays derived from persisted state;
  crash-safety is re-derivation on wake, not queue recovery.
- **No cron grammar.** Intervals (hourly/daily/weekly) plus change triggers
  cover every persona moment in the vision; a cron editor is knob theater.
- **No headless CLI.** Cut in the vision's non-goals; MCP remains the
  programmatic surface, and the MCP server is now resident too — an agent can
  talk to a windowless Alchemy, which is new and free.

## Risks

- **Two ticks in one release window.** The frontend loops are deleted in the
  same commit that spawns the scheduler; there is no configuration in which
  both run. `last_run_at` stamping makes an accidental overlap idempotent-ish
  (second runner sees a fresh timestamp), but we don't rely on it.
- **Webview-less event emits.** `run_report` emits to whoever listens; with
  zero windows that's a no-op by construction in Tauri. Verified in testing
  rather than assumed.
- **Battery.** The tick itself is a table read; real work (generation,
  embedding) is what it always was, now sometimes at night. The pause item
  and the report interval are the controls; deep-run budgets add idle-gating
  when they arrive.
- **User surprise at persistence.** First close-to-tray shows a one-time
  notification: "Alchemy keeps working in the menu bar — reports and syncs
  run on schedule. Quit from the menu bar icon." One sentence, once.

## Verification

- `cargo test` unit coverage for due-filtering (overdue, disabled, just-ran,
  clock-skew) extracted into a pure function.
- Manual, the Steward's acceptance test: schedule an hourly report → quit all
  windows (not the app) → report note exists and a notification arrived with
  no window open → reopen from tray, reports feed shows the run with unread
  state.
- Sleep/wake: schedule due during a forced sleep window fires on wake, once.
- ⌘Q, tray Quit, and Settings tray-off all genuinely exit; Activity Monitor
  confirms no orphan.
- Gates: `cargo fmt -- --check && cargo clippy -- -D warnings && cargo test`,
  plus the frontend typecheck for the store.ts deletions.
