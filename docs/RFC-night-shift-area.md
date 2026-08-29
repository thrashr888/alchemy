# RFC: Night Shift, the area

## Summary

Promote Night Shift from infrastructure to a third top-level area beside
Notebooks and Registry. [RFC-night-shift.md](RFC-night-shift.md) built the
resident staff; this RFC builds the desk you leave instructions on. Three
views — **Tonight** (one-off commissions), **Standing orders** (the
cross-notebook index of recurring work), **The record** (a receipt per run)
— plus chat-first administration through the existing tool router, and a
consolidated **Background Work** settings page that gathers today's
scattered toggles and the mechanical housekeeping chores under one roof.

The organizing claim: Notebooks organize the corpus by document, Registry
by thing, Night Shift by **time** — the work you have decided should happen
without you. The shipped Staff sidebar is the staff's timesheet; the area
is where work gets commissioned, standing orders get authored, and trust
gets built through receipts. Mocks: [v12-mockups](v12-mockups/README.md)
screens 10–12. Public-copy draft: [PRFAQ-night-shift.md](PRFAQ-night-shift.md).

## Background: what already shipped

More of the substrate exists than the original RFC's staging predicted:

- **The resident scheduler** (`scheduler.rs`) runs the full pass with no
  window: db maintenance every 6h (`db.maintain`, Lance version pruning),
  source resync, the gist sweep, the hygiene sweep, and due reports off-tick
  with a `REPORTS_RUNNING` guard. Pause-until-morning, tray status, and
  notification gating are done.
- **Standing questions shipped early.** `ReportSchedule.trigger` is already
  `"interval" | "change"` (models.rs:302), and change-triggered schedules
  fire off `source_events` rows with the interval as throttle floor
  (scheduler.rs:205). The `source_events` table is a rolling window that
  prunes old rows (models.rs:277) — the precedent receipts reuse below.
- **Chat tools exist.** A cheap keyword gate (`tool_gate`,
  commands.rs:5757) routes imperative messages to one JSON routing call on
  the Small role, then dispatches to existing commands — and `schedule_report` /
  `update_report` are already arms. Scheduling recurring work by chat works
  today; this RFC extends the arm list, it does not invent the mechanism.
- **The V12 pillars grew commands**: `commands/brief.rs`, `ledger.rs`,
  `weave.rs`, `second_look.rs`, `registry.rs` — several of which are
  exactly the slow jobs worth commissioning overnight.
- **Settings are scattered.** Background-related flags live across three
  Settings sections today: `background_enabled` (the Night Shift master
  switch), `tray_enabled`, `show_notifications`, `quiet_when_focused`,
  `curator_consolidate`, `source_gists`, `source_hygiene` +
  `hygiene_refresh_days`, `git_sync_minutes` (ai/mod.rs:86–171). Launch at
  login is still pending from the original RFC.

## Proposal

### 1. Commissions: the missing verb

A commission is a one-off job handed to the night. Mechanically it is a
`ReportSchedule` — no new table, no queue:

- `trigger` gains `"once"` (additive, serde-default like `"change"` was).
- New field `not_before: i64` (epoch ms, `#[serde(default)]`, 0 = now) —
  zero-migration by construction, same as every additive schedule field.
- The due filter in `run_pass` honors it: a `"once"` schedule is due when
  `now >= not_before && last_run_at == 0`; on success it sets
  `enabled = false` and stays as its own history. Crash-safety remains
  re-derivation on wake — a commission is persisted state, not a queue
  entry.
- "Tonight" defaults `not_before` to the next 2:00 AM local
  (`next_local_hour_ms(2)`, scheduler.rs:58); "now" is `0`. No richer
  grammar.
- `kind` reuses the existing vocabulary: any generator kind, `"custom"`
  with a prompt, and the pillar commands (a Second Look pass, a deep-read
  rebuild, a re-gist) each get a kind that dispatches to the command that
  already exists. One commission = one `run_report_inner`-shaped unit with
  the same notification, threading, and failure paths reports have today.

The overnight budget from RFC-night-shift §4.3 attaches here: `"once"` runs
get the relaxed caps (steps, wall-clock, spend) instead of `MAX_STEPS = 5`;
recurring schedules keep today's limits. A nightly metered-spend cap lives
in `AiConfig`; at the cap, work degrades to local roles rather than
stopping — the Seal is enforced at the router as always, so a sealed
notebook's commission never routes to a gateway regardless of budget.

### 2. Receipts: one row per run

Every run — scheduled, change-triggered, commissioned, and each mechanical
chore batch — writes a receipt when it ends, success or failure:

- A `receipt:` row species on the chunks-table prefix pattern
  (db.rs:42–62; `note:` / `gist:` / `snote:` precedent), body = compact
  JSON: schedule id, kind, started/ended, sources read, notes written (ids),
  flags raised, per-role engine, metered cost, gateway/agent-CLI call
  counts, sealed flag, failure reason if any. The raw material is already
  produced (trace.rs JSONL, `cost_usd` from agent CLIs, model_stats) —
  receipts are aggregation at the run boundary, not new instrumentation.
- **Excluded from hybrid search** (same read-boundary filtering the other
  species get) so receipts never pollute retrieval; read through commands
  and MCP verbs instead.
- **Rolling window**, pruned like `source_events` — the record is a recent
  ledger, not an archive. The permanent artifacts are the notes and reports
  the runs produced, which already persist.
- Agent parity in the same change: `list_receipts` / `get_receipt` MCP
  tools beside the existing schedule tools in `mcp/studio.rs`.

Receipts land first in the build order because they instrument what
already runs — the Staff sidebar and tray status get exact numbers
immediately, before any new surface exists.

### 3. The area: Notebooks | Registry | Night Shift

Home's center switch grows a third section, three views on the left rail
of the section (mocks 10–12):

- **Tonight** — the plan: commissions queued (with estimates and Remove),
  recurring work due in the window, and the composer (§4). Header carries
  "Pause until morning"; the rail shows the budget (wall-clock, steps,
  spend cap, "At cap: degrades to local") and sealed notebooks.
- **Standing orders** — every enabled schedule across notebooks in one
  list, grouped Reports / Watchers / Questions (watchers = the hygiene
  sweep's per-source cadence surfaced as objects; questions = `"change"`
  schedules). Selected order's rail: last runs from its receipts, what it
  produces, Run now / Pause / Edit. This is `all_report_schedules` plus
  grouping — the authoring surface the change-trigger machinery never got.
- **The record** — receipts grouped by night; selected receipt in full in
  the rail (engine, egress, authority, trace pointer). Morning-after
  review; nothing on this screen updates live.

The Staff sidebar stays as the Notebooks-section summary and becomes
click-through into the area. The Brief is untouched — it is the output
arrival point; Night Shift is the input side. The boundary line renders
verbatim wherever commissioning happens: **"Night Shift writes notes and
reports. It will not act outward."**

### 4. Chat-first administration

The Tonight composer *is* the chat tool router with a night-shift bias —
one parser, two mouths. Extensions to the shipped mechanism:

- `tool_gate` nouns gain "night shift", "tonight", "overnight", "watcher",
  "standing", "commission", "receipt" (the verb list already covers
  schedule/pause/resume/show).
- New dispatch arms: `commission` (kind, prompt, notebook, when:
  tonight|now), `night_shift` (status | pause | resume), and receipts
  queries answered from the receipt rows deterministically — "what ran
  last night" should be exact, not retrieved.
- Anything that spends echoes the parsed plan and waits for confirmation,
  same contract as `schedule_report` today. Nothing destructive (deleting
  standing orders) executes from a single message.
- Every arm ships its MCP twin in the same change (house rule) — an agent
  on this Mac can leave work for tonight and read the receipt tomorrow.

### 5. The Background Work settings page

One page replaces the scatter. It has two jobs: the genuine cost controls,
and honest documentation of what runs — a settings page the user can read
to learn what their Mac does at night. Every hint is one clipped line per
WRITING.md; no per-chore knobs beyond real cost control.

**Night Shift** (top): the master switch (`background_enabled`, existing) —
off means today's on-demand behavior, nothing below runs unattended.

**Residency**: menu bar icon (`tray_enabled`, existing; keeps its coupling
to close-to-tray), launch at login (the pending `tauri-plugin-autostart`
item from RFC-night-shift, default on), and "Wake for Night Shift" — not a
toggle but a disclosure showing the copyable
`sudo pmset repeat wakeorpoweron MTWRFSU 01:55:00` command with one plain
sentence about what it does. No privileged helper (§non-goals).

**Notifications**: `show_notifications` and `quiet_when_focused` move here
from app preferences — they exist for background work and read better
beside it.

**Overnight housekeeping** (mechanical, no AI spend — runs even seem
routine, so the page lists them; new chores marked •):

- • Nightly snapshot — the data-trust job (§7). Row shows the last
  snapshot time and total size; the one knob is retention.
- Database optimize — `db.maintain` exists; gains an FTS optimize leg and
  moves its long pass into the night window. Row reads "Runs
  automatically"; no knob.
- Source health — the existing hygiene cadence select
  (`hygiene_refresh_days`), relocated.
- Repository sync — `git_sync_minutes`, relocated.
- • Orphan cleanup — chunks whose source is gone, stranded markers
  (generalizing gist.rs:553). No knob; listed.

**Background intelligence** (AI spend, each a cost-control gate, all
default on per house rules): source summaries (`source_gists`), note
consolidation (`curator_consolidate`), and • the nightly spend cap (§1) —
a dollar field with the degrade-to-local note. Cortex consolidation
(RFC-foundation-learnings pillar 4b) joins this group when it lands;
overnight re-index after an embedder change arrives as an auto-proposed
commission, not a toggle.

Mechanical chores gate on `background_enabled` but never on the spend cap;
db maintenance keeps its deliberate position ahead of the gate
(scheduler.rs:156) since disk hygiene must run regardless.

### 6. Sleep, power, and the ladder

The default stands: due-ness is wall-clock, so everything runs on the
first tick after wake — late, not lost. The area's copy says so plainly
("Runs when your Mac is awake.").

**And the app says when it was late.** "Late, not lost" is only true from
the user's side if the lateness is disclosed: an 8:00 brief that appears at
11:00 with a bare "Report ready" reads like a broken schedule, not a
sleeping Mac. So every receipt records `due_at` — the hour the run was
meant for, derived from persisted state exactly as due-ness is — and past a
15-minute threshold (under that, the delay is just the pass interval) three
surfaces say so: the notification names it ("was due while your Mac was
asleep — 3 hours late"), the receipt carries it, and Tonight marks an order
whose hour has passed as overdue rather than merely "not yet run". A
standing question's due time is when the change landed, not when its
interval elapsed — that is the moment the user would have wanted to know.

Above the default:

1. **Keep-awake during runs**: hold a `PreventUserIdleSystemSleep` power
   assertion while a commission or report batch is active on AC power, so
   a long run isn't cut off by idle sleep. Ships with commissions.
2. **Run on power-connect**: plugging in during the evening starts the
   Tonight plan early if anything is queued. Observable event, no
   permissions.
3. **Manual**: "Start tonight's plan now" on Tonight, in the tray, and as
   a chat verb.
4. **Scheduled wake**: the copyable `pmset` command in Settings (§5).
   Wakes a lid-closed Mac on AC; combined with (1) the plan completes.

### 7. Data trust: the snapshot is the point

The nightly snapshot is the highest-value thing the Night Shift can do, and
it is the one job whose absence is unrecoverable. Adopted from Pillar 1 of
[RFC-professional-grade.md](RFC-professional-grade.md), which names the
Night Shift scheduler as its natural home; that RFC's pillar is satisfied by
this section rather than duplicated.

**The snapshot job** (`backup.rs`, registered with the scheduler):

- **Mechanism:** APFS `clonefile(2)` per file over the LanceDB directory
  into `<app-data>/backups/store/<YYYY-MM-DD>/`, falling back to a plain
  copy on non-APFS volumes. Clones are near-instant and near-free until
  blocks diverge, which is what makes a multi-GB store snapshottable every
  night without the user noticing.
- **Quiesce:** taken between passes with the write gate held — the
  single-flight idiom the gist sweep already uses — so no Lance commit is
  mid-flight.
- **Retention:** 7 nightly + 4 weekly, pruned after each success.
- **Escape hatch alongside:** the same job loops `export_notebook_okf`
  (commands.rs) over every notebook into `backups/okf/latest/`. That copy
  is human-readable and survives a Lance-format problem entirely. It
  currently covers sources and notes; widening OKF to chat turns and ledger
  rows is a follow-up, not a blocker.

**Migration safety** — the failure this codebase has actually hit (the
shared dev/prod store policy exists because a column append bricks older
binaries):

- **`store_version` stamp** in the store directory, written by `Db::open`
  after `ensure_table` and any column appends complete. An older binary
  finding a newer stamp records `fatal` and gets the designed restart
  screen — "This library was upgraded by Alchemy vX" — instead of a Lance
  panic.
- **Pre-migration snapshot:** when `open()` is about to append columns to
  an existing table (the `field_with_name` miss branch), it clones the
  store into `backups/pre-migrate/<version>/` first. Upgrades become
  reversible.
- **Downgrade tests:** minimal fixture stores written by the last two
  releases, checked in; `cargo test` opens each and asserts every table
  reads. This is the test that would have caught both historical brickings.

**Integrity on open:** `Db::open` gains a cheap validation pass — every
expected table opens, row counts read, chunks-table dimensionality matches
config. On failure it records `fatal` and the restart screen offers
**Restore from last snapshot**, which renames the bad store aside (never
deletes) and clones the snapshot back.

Snapshots and integrity checks are mechanical: they run whenever the app
runs, gated only by `background_enabled`, never by the spend cap, and they
are the one part of the Night Shift that stays on even for a user who wants
nothing intelligent happening at night.

## What this deliberately does not do

- **No job queue and no cron grammar** — commissions are schedules with a
  `"once"` trigger and a `not_before`; "tonight" and "now" are the whole
  vocabulary.
- **No privileged helper.** The `pmset` wake is a command the user runs
  once, shown with its explanation; Alchemy never escalates.
- **No live operations view.** Nothing in the area updates while you
  watch; you look at it before bed and the morning after. Progress during
  a foregrounded run stays where it lives today (the reports feed).
- **No per-chore knob wall.** The settings page documents; it only
  exposes the knobs that are genuine cost control (cap, cadences,
  snapshot count).
- **No receipt archive.** Rolling window; the durable record is the notes
  the runs produced.
- **No outward action**, restated because the surface grows: commissions
  write notes and reports, propose, and stop.

## Risks

- **Expectation vs. physics.** A commission on a lid-closed battery Mac
  runs at 8 AM, not 2 AM. Mitigation: the copy commitment above, the
  receipt showing actual run time, and the ladder for users who want true
  overnight.
- **The dashboard invariant.** Standing orders is one admin-console step
  from violating "no dashboards." Guardrails: flat rows, no charts, no
  live state, and the sidebar remains the default summary surface.
- **Tool-router misfires.** "pause" + "night shift" is unambiguous;
  "commission" parsing is not. The echo-and-confirm contract carries the
  risk; a misparse costs one correction message, never a wrong run.
- **Receipt volume.** A busy install writes dozens of rows nightly; the
  rolling window and search exclusion keep them out of retrieval and
  storage growth bounded.
- **Two surfaces, one parser.** The Tonight composer and chat share the
  router; a routing regression breaks both. The router's dispatch tests
  extend to the new arms in the same change.

## Verification

- Unit: due filter over `"once"` + `not_before` (queued, due now, already
  ran, disabled); receipt written on success, failure, and cap-degrade;
  snapshot rotation keeps exactly N; spend-cap arithmetic.
- Router: gate + dispatch tests for the new arms, including the
  echo-and-confirm path and a deliberately ambiguous commission.
- Manual, the area's acceptance test: commission a deep read at 11 PM →
  close the laptop on battery → open at 7 AM → run starts, finishes, and
  the receipt shows the true times; same run on AC completes overnight
  with the keep-awake assertion held.
- Chat: "what ran last night" answers from receipts with exact counts;
  "pause night shift until morning" flips the same flag as the tray.
- Gates: `cargo fmt -- --check && cargo clippy --all-targets -- -D
  warnings && cargo test`, plus `pnpm build` for the new surfaces.

## Build order

Dependency order, each stage useful alone, no timelines:

1. **Receipts** — instrument the runs that already happen; Staff sidebar
   and tray status read from them.
2. **Data trust** (§7) — snapshot job, `store_version`, pre-migration
   clone, integrity check, restore path. First real work because it is the
   only irreversible failure on the list.
3. **Background Work settings page** — consolidation plus the remaining
   mechanical chores (FTS optimize leg, orphan cleanup).
4. **Commissions** — `"once"` + `not_before` + budget caps + keep-awake.
5. **The area** — the three views over the data the first three stages
   produced.
6. **Chat and MCP verbs** — can land with 3; complete with 4.
7. **Power ladder extras** — run-on-power-connect, the `pmset` disclosure.
