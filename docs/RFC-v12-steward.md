# RFC: Alchemy V12 — The Steward

## Summary

Alchemy today is the "V8 prosumer": grounded chat with citations, deep-research
agent mode, eval-fenced retrieval to 10M chars, nineteen studio generators plus
templates and scheduled reports, OKF sharing, a 24-tool MCP server, on-device
podcasts, deep Mac integration. This RFC is the answer to "what is the V12 —
the high-end professional version — still single-user, explicitly not
enterprise?"

**V12 turns Alchemy from a notebook you consult into a steward that works
while you're away: it remembers what you concluded, notices what changed and
judges it against what you hold, keeps the long documents current, and meets
you at your own cadence — writing notes and proposing, never acting outward.**

An earlier draft of this vision ("The Instrument of Record") centered
high-stakes professionals — journalists, litigators, scholars — and optimized
for defensibility: prove every sentence to a hostile third party. That was the
wrong center. The actual user is a PM/Designer/Engineer who also runs a
personal life in Alchemy — home, finance, vehicles, family, activities,
projects, health. The [lineage section](#lineage-what-happened-to-the-instrument-of-record)
records what survived the reframe, what mutated, and what died. The mockups in
[v12-mockups/](v12-mockups/README.md) visualize three moments of this vision
under the earlier draft's names (the Audit → the Second Look, the Claim
Ledger → the Ledger, the Morning Desk → the Brief); their layouts and design
invariants carry forward unchanged.

## The one idea

The persona clusters — work, household, curiosity — are not three markets.
They are one person's one life: the same person writes the PRD at 9:00, scans
the insurance EOB at noon, and checks the motorcycle's valve clearances on
Saturday, and all three lives fail the same way. Assumptions rot, renewals
lapse, threads plateau. Not because information is missing, but because
**continuity leaks** — nobody's job is to hold the thread while attention is
elsewhere.

V12 makes that Alchemy's job. Work gets a chief of staff, the household gets a
family office, curiosity gets a research companion — one instrument, not three
modes, because it is one mechanism applied everywhere: *durable typed memory +
resident attention + judgment on arrival*. What varies per notebook is
**temperament** — ranking function, cadence, row types — settings of one
machine. V8 answers questions; V12 holds the thread: never lose the decision,
never re-derive, never miss the date, never be surprised.

Every pillar below is a promotion of a seam that already exists in the engine
— usually from "background convenience" or "frontend timer" to product spine.
No new storage engine, no new process model, no timeline theater.

## The nine pillars

### 1. The Night Shift *(keystone — see [RFC-night-shift.md](RFC-night-shift.md))*
The report scheduler moves into Rust and the app becomes tray-resident; every
living source's existing resync (git, folders, URLs, Mac apps) unifies into
**watchers** that keep a snapshot and make change a first-class diff event;
standing questions let an event pull a trigger instead of a clock; overnight
runs get a budget — steps, wall-clock, dollars — instead of `MAX_STEPS = 5`.
- **Work:** LanceDB tags a release overnight; by 7:40 the deprecated call is
  matched against your three call sites.
- **Life:** the household doesn't open the app for two weeks — the
  registration renewal still fires. Today's frontend tick would have missed it.
- **Curiosity:** the track-day org's page changed; the diff plus a calendar
  check waits in Saturday's brief.
- **Why V12:** residency changes the ontology from tool to staff. "No window
  open, nothing fires" (`startReportScheduler`, store.ts:1459) is a tolerable
  bug for a notebook and a disqualifying one for a steward.
- **Seam:** `run_report` + `report_schedules` (backend complete; only the tick
  is frontend); the tray in `integrations.rs`; `idle_ms`/`touch_activity`
  gating; per-scope cancellation; `agent.rs:17`; an overnight `ContextProfile`
  tier.

### 2. The Ledger
Memory the machine can act on: typed, dated, hash-anchored rows as corpus row
species beside `note:` and `gist:` — **assertions** (asserted → corroborated →
contradicted → stale), **facts** (current → superseded), **decisions** (with
alternatives-rejected and because), **open questions** (open → possibly
answered), **log entries**. Dated rows become standing obligations with lead
times — the **Tickler** — fired by the Night Shift; one confirmed click pushes
to Apple Reminders through the existing narrow write-back, nothing more.
- **Work:** "we decided X because Y" captured mid-chat, anchored to the exact
  interview passages; assumption A3 flips to *contradicted* when interview 14
  lands.
- **Life:** LDL 118 → 124 → 131 trended across three years of lab PDFs; the
  registration date becomes a 21-day-lead obligation.
- **Curiosity:** "start 1°C hotter on lighter roasts" is a nine-word log row;
  an open question gets marked *possibly answered* by a new paper.
- **Why V12:** V8's memory is chat history and notes that dim. Typed rows with
  lifecycles are the substrate every other pillar reads.
- **Seam:** `db.rs:37/45` row-species prefixes (zero-migration by
  construction); `note_usage` lifecycle counters; `spawn_auto_evidence`
  (commands.rs:4799) as the minting precedent; the tool router gains capture
  verbs; `mac.rs` `add_reminder`.

### 3. The Weave
Judgment on arrival. When chunks land — new source, refresh, scan, highlight —
they're cosine-matched against the Ledger and old highlights/notes/gists, and
the Small role judges each pair: *extends / complicates / echoes / contradicts
/ supersedes*. Three specialized forms ride the same sweep: **year-over-year
diffs** on recurring documents, **absence detection** (the expected arrival
that didn't come), and **impact matching** (a dependency release matched via
AST + grep against your own repos — a triage note, never a patch).
- **Work:** a competitor's changelog contradicts the PR/FAQ assertion —
  flagged with diff and anchored passage; three retros touching the same
  subsystem get linked by recurrence.
- **Life:** this year's declarations page diffs against last year's ("premium
  up 13.8%"); the provider bill asks $137 while the EOB says $88; the 1099-B
  that didn't arrive this February is flagged by absence.
- **Curiosity:** a new paper *complicates* an assertion your synthesis has
  carried since 2024 — both passages pinned side by side, not silently merged.
- **Why V12:** V8 retrieval finds what you ask for. The Weave judges what
  arrives against what you hold, unasked — the difference between an archive
  and a steward.
- **Seam:** the curator's cosine-pair + consolidation loop (commands.rs:2869+)
  re-aimed from dedup to judgment; the gated hash-diffed sweep idiom of
  `gist.rs`; `AmbientRail.tsx`; `fuse_grep_hits` + the `outline.rs` AST leg;
  `collapse_report_notes` prior-run threading.

### 4. The Registry
A closed, user-confirmed cast of entities and threads — assets, people,
policies, providers, projects, dependencies — each a living card aggregating
its documents, key facts, and dates. Documents attach on ingest: automatic
above a precision bar (serial number, policy number, VIN), otherwise
propose-and-confirm. Projects thread documents in sequence (quote → contract →
permit → invoices). And **Reprise**: opening a notebook kin to dormant ones
triggers a carry-forward brief — decisions that still hold, facts gone stale,
notes you forgot you wrote.
- **Work:** a card per dependency and per project; the July RFC revives the
  March decision context instead of re-litigating it.
- **Life:** "dishwasher model number, still under warranty?" answered in three
  seconds; the deck project's bid comparison flags the silently dropped
  demolition line item.
- **Curiosity:** the Ducati card holds the manual and the logbook trend;
  opening Japan 2027 triggers the Reprise from Japan 2023.
- **Why V12:** V8 organizes by notebook and filename; your questions arrive by
  *thing* and *thread*. The closed cast with one authority (you) is the safe
  form of the entity resolution the first draft rightly cut open-world.
- **Prior art, in-house:** Argos (`../Argos`) — a personal context graph that
  aggregates Apple, Google, GitHub, and finance accounts into one records
  table with entity views. It proves the entity-centric frame works across a
  whole life, and it draws the boundary this pillar keeps: Argos does
  connectors, credentials, and account scraping; Alchemy stays documents-in.
  An Argos export can land in Alchemy as sources; the Registry never logs in
  anywhere.
- **Seam:** cards as `note:` species + Convert to source; reader backlinks as
  the card's document list; curator matching for doc→entity attach;
  `router.rs` per-notebook summary vectors as Reprise's kinship signal;
  `global_meta_route` (commands.rs:7373) scoped to kin notebooks.

### 5. The Clerk
Capture-to-durable in one motion, across every format: watched scan folders,
clipper, drag-drop, tray, ⌥Space, Services, MCP — then OCR'd, classified,
filed to notebook and entity, header facts extracted, before you sit down. An
**OCR quality gate** flags degenerate scans "rescan this" while the paper is
still on the desk. **Marginalia:** a highlighted passage mints an anchored
excerpt row with one tap. **Recordings become sources:** interviews, insurance
calls, lectures — transcribed fully on-device (`whisper-large-v3-turbo` 4-bit,
shared HF cache), timestamps as citation anchors.
- **Work:** interview 15's recording, dropped in after lunch, is a searchable
  citable source by mid-afternoon.
- **Life:** the phone-scanned EOB lands in the watched iCloud folder, is
  OCR'd, filed to Health, fact-extracted overnight — and matched against the
  provider's bill by morning.
- **Curiosity:** the shot log from the tray in nine words; three highlights
  kept during an afternoon read, one tripping a Weave badge.
- **Why V12:** the corpus only exists if capture is effortless, and a garbage
  OCR is a document that silently doesn't exist. V8 capture ends at "source
  added"; V12 ends at "filed, extracted, judged, durable."
- **Seam:** `clip.rs`/`capture.rs`; folder sources + iCloud hydration; the
  sips/PDFium/vision OCR pipeline; `gist.rs` gating reused verbatim as the
  quality gate; select-to-ask gains a "Keep" verb; Kokoro's download-and-verify
  precedent for the Whisper rung.

### 6. The Long Forms
Standing documents that rebuild themselves with an explicit since-last-time
delta and a preserved changelog: the **Growing Answer** per long-running
thread ("what do I currently understand"); the weekly **Shipping Record**
(commits, decisions, closed RFCs); interview **theme maps**; the household
**Binder** (accounts, policies, directives, where the titles are — exportable
as PDF/OKF for the spouse or executor); the **Tax-Year briefing** that
collects all year and flags the missing form. Rider: flashcard decks minted
from Marginalia, maintained by a curator-style sweep — the shipped Leitner
machinery finally gets its user.
- **Work:** the Friday Shipping Record drafts itself; the quarterly review
  becomes an edit, not an excavation.
- **Life:** the Binder rebuilds on schedule and is one export from being in
  the right hands on the worst day.
- **Curiosity:** the Growing Answer shows, in your own corpus, what you
  believed in 2024 and why you believe something sharper now.
- **Why V12:** V8 generates artifacts once. The report machinery already
  threads prior runs — V12 keeps the changelog instead of collapsing it, and
  makes standing synthesis the default past a size threshold. Reading only
  compounds if something durable absorbs it.
- **Seam:** `commands/reports.rs` (`run_report`, prior-run threading,
  `collapse_report_notes`); new kinds in `ARTIFACT_KINDS` (rag.rs:450) via
  `resolve_report_kind` (commands.rs:3934); `templates.rs`; native PDF export;
  `Flashcards.tsx` Leitner state.

### 7. The Brief
One synthesized arrival point per cadence, with an on-device audio edition —
and **the ranking function is the pillar**: per-notebook temperament, not
three products. Hard deadlines always notify immediately; everything else
waits for the brief.
- **Work:** the 7:40 weekday brief ranked by *needs a decision*; the two-voice
  audio standup over coffee.
- **Life:** the Sunday brief — arrived this week, due in 30/60/90, what
  changed. Weekly is the honest cadence; daily would train the household to
  ignore it.
- **Curiosity:** the Saturday brief ranked by *what would unblock or delight*
  — including what to skip, with the March highlight as the receipt.
- **Why V12:** V8's AwayDigest lists what happened; a steward triages. Ranking
  is how a chief of staff earns a salary; cadence-follows-life is what makes
  one instrument serve three lives.
- **Seam:** `global_meta_route` map-reduce over gist rows;
  `AwayDigest`/`HomeReportsFeed.tsx`; `tts.rs` unchanged.

### 8. The Second Look
Claim-by-claim verification of a draft before it ships, sends, or gets signed:
each substantive claim independently re-retrieved (fresh hybrid search, not
the author's citations) and judged *supported / weakly supported / unsupported
/ contradicted* by a **different engine than the one that helped write it**,
each verdict carrying its own `SearchTrace`. A design-crit mode accepts
screenshots through the vision role, critted against your own principles
notes. Deliberately the same build as the live-model eval harness
[RFC-infinite-context](RFC-infinite-context.md) names twice as its blocker —
shipped as a product feature, measured on the real corpus.
- **Work:** 22 claims in the RFC — 19 supported, 2 unsupported, 1 contradicted
  — fixed before anyone else sees it. Used weekly.
- **Life:** the insurance appeal checked sentence-by-sentence against the
  policy and EOBs before it's sent. Used rarely; the day it's needed it pays
  for the year.
- **Curiosity:** "so which is right?" arbitration between two colliding
  highlights. Deliberately light — nobody deposes your espresso notes.
- **Why V12:** the solo person has no reviewer; this is the review culture of
  a good team, on call — plus its second identity as the eval harness the
  whole retrieval program is blocked on.
- **Seam:** `evals.rs`/`retrieval_eval.rs`; `search_debug`'s full trace; the
  `agent.rs` search/read/judge loop; the chat/studio provider split for
  different-engine judging; the vision pipeline.

### 9. The Seal and the Meter
Per-notebook **"never leaves this Mac" pin enforced at the router** — no role,
chat through vision, may resolve to a gateway or agent CLI for a sealed
notebook — plus an **egress receipt**: a per-notebook view over the trace
showing exactly which providers touched it. Beside it the meter: every metered
call aggregates per night, per schedule, per notebook, with caps that
**degrade to local rather than stop**.
- **Work:** employer-confidential strategy pinned local; last night's staff
  work: $0.58 metered, the rest local.
- **Life:** Health and Money sealed; the receipt shows zero gateway calls —
  provable, not asserted. The precondition for putting labs and statements in
  at all.
- **Curiosity:** a monthly cap on hobby spend that degrades to local.
- **Why V12:** residency and overnight budgets make cost and egress governance
  mandatory. V8 captions a message "· $0.04"; V12 governs a staff.
- **Seam:** the `Role` enum + `Ai::chat_role` fallthrough (`inference/mod.rs`);
  `cost_usd` already reported by agent CLIs; `model_stats.json` already
  persisted — the meter is aggregation, not plumbing; `trace.rs` JSONL as the
  receipt's raw material.

## UI direction

Four surfaces carry the Steward, each extending a shipped pattern rather than
inventing a new one. The [v12-mockups/](v12-mockups/README.md) screens test
the reader-verdict, ledger-mode, and morning-brief moments; a second round of
mocks (before-stand-up / the-$49-difference / pick-up-the-thread) confirmed
the ledger-as-center-mode call across all three personas. The shared design
invariants (narrow navigator, unboxed center paper, floating contextual rail,
icon-plus-text status, no dashboards) apply to everything below.

### 1. The notebook switch: Chat | Reader | Ledger
The center-column toggle grows a third mode. **Ledger** shows the notebook's
typed rows — assertions, facts, decisions, open questions, log entries — as
dense rows with lifecycle chips, filterable (All / Corroborated /
Contradicted / Stale), each row anchored: clicking an anchor opens the Reader
at the exact passage. The steward's own documents — a brief, a Reprise
interstitial, a judged-arrival note — open in Reader like any note, with
their actions inline; the toolbar's right slot carries the mode-specific
affordance ("Play audio · 4:12" on a brief, "Clerk receipt" on a filed scan,
"Open old notebook" during a Reprise).

Two rail patterns ride along:

- **The right rail follows the subject.** It already swaps by context (TOC,
  related passages); it gains pillar-aware forms — a **Registry card** when
  the open document attaches to an entity (key facts, document thread,
  related notebooks, and the matching receipt: identifiers matched,
  attachment confidence, why it auto-attached), a **Current Decision** panel
  when a brief item touches a Ledger decision (the decision on file, what
  changed with its verdict chip, the anchors), and a **Meter & Seal** foot
  (last night's cost, local %, gateway calls, seal state — the egress receipt
  where the work happened, not buried in Settings).
- **The left rail shows the staff's presence.** Sources gains a **Watching**
  group with last-update badges, thread groupings when one is on screen
  ("Filed to claim 4821," "Related thread"), and a status foot — "Night Shift
  on · next 2:00 AM," "Watched scan folder · active."

**Boundary microcopy, every time.** Each proposal states its limit in one
line beside its buttons: "It will not change the repository." "It will not
call, pay, appeal, or submit anything." The propose-never-act rule lives as
copy at the point of action, not a settings page.

### 2. Home becomes a dashboard with title-bar sections
The notebook's center switch (§1, now three modes) is the precedent; Home
gets the same control in its title bar, four sections:

- **Notebooks** — today's home (grid, ask box, gauge, epigraph), unchanged,
  still the default.
- **Brief** — the arrival point: the current brief (per-cadence,
  temperament-ranked) with its audio edition playable inline, the archive of
  past briefs, and **AwayDigest folded in** (it is the brief's degenerate form
  today).
- **Registry** — the cast: cards in a grid echoing the notebook grid's visual
  language (title dot, frosted right-click menus), grouped by kind — assets,
  people, policies, projects, dependencies. A card opens via the Reader
  pattern (cards are `note:` species, so reader, backlinks, and ⌘F work for
  free). The propose-and-confirm attach queue lives here.
- **Staff** — the Night Shift's own ledger: last night's runs and what they
  produced, the upcoming schedule, watchers with their last diff, the Meter
  (spend per night / schedule / notebook, caps), seal status per notebook, and
  "pause until morning."

The unified ask box stays present across all four sections — it's the
signature element. Unread state mirrors the reports feed: a dot on **Brief**
when a new one arrives. ⌘1–⌘4 switch sections on Home (free there; they mean
Sources/Studio only inside a notebook).

### 3. Above the reports feed: the steward's strip
In the Notebooks section, above `HomeReportsFeed.tsx`, two compact elements:

- The **Brief card** — top three lines of the current brief, play button,
  jump-through to the Brief section.
- The **Needs-you strip** — the confirm queue in one row: Clerk filings
  awaiting confirmation, Weave flags (*contradicts* / *supersedes*), Tickler
  obligations inside their lead window. Each item resolves in one click or
  opens its section. This is the propose-never-act boundary made visible:
  everything in the strip is a proposal.

The reports feed itself stays; Long Form runs and scheduled reports keep
landing there with unread state.

### 4. The tray dropdown becomes the steward's face
Today's tray (Ask, Add Clipboard/URL/Text, Recent Notebooks —
`integrations.rs`) grows a top block:

- **Status line** — what the staff did while you were away ("Overnight: 3
  runs, 2 flags · $0.58"), one line, click → Staff section.
- **Top brief items** (2–3), click-through to the Brief.
- **Due soon** — the next Tickler obligations.
- **Log…** — quick capture for nine-word log rows: ⌥Space Ask's write twin.
- **Pause staff until morning.**

Native-menu constraints apply (text items and submenus, no custom widgets),
and dynamic items rebuild the **tray menu only**, never the app menu (the
AppKit Window-list gotcha — see `menu.rs`).

### Elsewhere in the notebook
The Weave surfaces as margin badges in the related-passages rail
(`AmbientRail.tsx`) and reader — its verdict vocabulary (*extends /
complicates / echoes / contradicts / supersedes*, plus *still holds / left
open / mismatch* in Reprise and Weave documents) renders as outlined chips,
icon plus text, never color alone. Temperament (cadence + ranking) is a
notebook setting beside its color. Per house style, state carries in chips,
dots, and washes — no left-border accents.

## Lineage: what happened to the Instrument of Record

The headline is a **posture inversion**: the first draft was defensive — prove
every sentence to a hostile third party. Every persona pass revealed the same
correction: the hostile reviewer is *yourself in six months*, and the scarce
resource is continuity, not defensibility. Same engine seams, opposite center
of gravity.

- **Survived intact:** the Night Shift (promoted to keystone), the Transcript
  Desk (re-motivated from depositions to interviews/calls/lectures, folded
  into the Clerk), the Morning Brief (ranking became per-notebook
  temperament).
- **Mutated:** Claim Ledger → **the Ledger** (atom pluralized:
  assertion/fact/decision/question/log; forensic bibliography export died).
  Cross-Examination → **the Weave** (adversarial contradiction-hunting became
  generative judgment with *contradicts* as one verdict of five; widened to
  drift, absence, impact). The Audit → **the Second Look** (defending
  published work became checking drafts pre-ship; demoted from spine to tool;
  eval-harness dual identity retained). The Dispatcher → **the Seal and the
  Meter** (the two load-bearing halves promoted; the policy matrix died).
  Dossiers — the first draft's cut was right open-world; **the Registry** is
  the safe closed-cast form.
- **Died:** the forensic/bibliographic export apparatus (no persona's
  deliverable is verified by strangers); the Librarian as a pillar (OCR gate
  and source health moved into the Clerk; gap detection became absence
  detection; incremental FTS and the archival tier remain load-bearing
  engineering chores, not product identity).
- **Born — absent from the first draft entirely:** the Registry and Reprise,
  the Growing Answer and Long Forms, Marginalia and log rows, the Tickler,
  one-motion capture, decision capture and retro recurrence, dependency impact
  matching, absence detection, practice cadence. The first draft's persona
  *consumed* documents; the real one ships things, runs a household, and comes
  back to threads after years.

## V8 → V12

| | V8 (today) | V12 (the Steward) |
|---|---|---|
| Posture | Answers questions when a window is open | Stewards threads whether or not you're there |
| Center of gravity | The document | The person — threads, entities, decisions, dates |
| Uptime | Frontend tick; no window, nothing fires | Tray-resident Rust scheduler; the idle Mac is the staff |
| Memory | Chat history + notes that dim | Typed ledger rows with lifecycles: corroborated, superseded, answered, stale |
| Change | Resync silently overwrites | Change is an event, diffed and judged against what you hold |
| Time | Dates are text inside documents | Dates are obligations with lead times; absence itself is a signal |
| Arrival | You open the app and look around | The Brief meets you at your cadence, ranked by your temperament |
| Organization | Notebooks and filenames | Plus a confirmed cast of things and threads documents attach to |
| Long documents | Generated once, rebuilt by hand | Self-rebuilding with deltas and preserved changelogs |
| Verification | Trust the citation | Second Look — per-claim verdicts by a different engine, before you ship or sign |
| Capture | Source added | Filed, extracted, quality-gated, judged, durable — including recordings |
| Dormancy | Cold threads die | Reprise revives them with what still holds and what went stale |
| Cost & privacy | A caption ("· $0.04") and a promise | A meter with caps that degrade to local; a seal enforced at the router, with a receipt |

## Non-goals and the boundary

**The boundary, stated once:** Alchemy is a research/knowledge instrument. It
reads, remembers, judges, and drafts; it does not *manage* and does not *act
outward*. Where an artifact needs acting on, it hands off at a confirmed
narrow seam — Apple Reminders, calendar, your editor, an OKF export — and
stops.

- **Structural (unchanged):** single-user, no enterprise — no teams, seats,
  SSO, sharing permissions, or admin consoles; the unit of collaboration is
  the exported OKF bundle handed to another human. No cloud execution — the
  idle Mac is the only datacenter; sync stays on its own RFC track
  ([RFC-sync-backend.md](RFC-sync-backend.md)). No second database — row
  species, notes, and sweeps in LanceDB. No autonomous outbound action —
  notes, notifications, and confirmed Reminders through the existing
  write-back; it never sends, posts, trades, files, or logs in. macOS-only
  stands.
- **Not a PIM/todo app:** no in-app task list, recurrence engine, or snooze —
  the Tickler proposes to Reminders and stops. No bank/insurer/DMV connectors,
  no credential handling — documents in, never logins. No budgeting ledger —
  Alchemy trends facts from documents; it is not Quicken.
- **Not a project tracker:** no issues, sprints, kanban, or status fields —
  the Shipping Record *reads* git and the Ledger. Dependency impact matching
  writes triage notes, never PRs. The career/promo packet stays a user
  template.
- **Not a read-later service:** no reading-queue surface, mobile app, or
  discovery feed — the corpus is what you brought in; watchers watch what you
  named. The skip verdict ("you read its substance in March," with receipt)
  lives in the Brief.
- **Cut by name:** an `alchemy` CLI + hooks folder (MCP's tools are the
  extension surface); a plugin protocol; a query DSL; an evidence-board
  canvas; reference-manager round-trips (CSL-grade export can ride the Ledger
  later if a real need appears); a lifetime-archival tier as product identity;
  video overviews, voice assistant, Windows/Linux — all existing scope
  decisions, unchanged.

## House rules

- **Solo-dev buildable:** every pillar is a promotion of a shipped seam —
  prefix predicates, sweeps cloned from `gist.rs`, new report kinds, at most
  one new Lance table (precedent: `report_schedules`).
- **Smart defaults ON — with one true off switch:** every background family
  ships default-ON behind its own cost-control toggle (the `source_gists`
  precedent), and Settings → General gains **"Background work"** — one master
  switch that stops all of it at once: the Night Shift, watchers, sweeps,
  gists, curation. Off means today's on-demand behavior; nothing runs unless
  you ask. Cost control, not safety theater — but it must exist, and it must
  be one switch. No user-facing retrieval knobs — the Deep-toggle deletion
  (`491065d`) stands.
- **Agent-legible by construction:** everything the steward produces is
  markdown notes and typed corpus rows — never UI-only state — so it is
  readable through the existing MCP tools (`get_note`, `list_notes`, search)
  and travels in OKF export. Every new surface (ledger rows, briefs, registry
  cards, watcher diffs, receipts) ships its MCP verbs in the same change.
  The steward's work product is useful to any agent on this Mac, not just
  Alchemy's own UI — and since the MCP server becomes resident with the Night
  Shift, an agent can read last night's brief without a window ever opening.
- **No timelines:** order comes from the dependency graph — the resident
  scheduler before everything nocturnal; the Ledger before the Weave before
  the Second Look; the Clerk's Whisper rung before theme maps. Each lands with
  eval deltas against the production baseline
  (`cargo test --lib retrieval_eval`).
- **Degrade to today:** every stage falls back to current behavior on failure;
  caps degrade to local, never to stopped.
- **Gated extraction discipline:** every new sweep clones the `gist.rs` gates
  (length bounds, identifier overlap, degeneracy, refusal memory, hash-diffed
  self-healing); extracted rows stay under the ~5% cap; auto-attach only above
  the precision bar, otherwise propose-and-confirm. A wrong merge poisons the
  Ledger worse than no row.

## First moves

1. **[RFC-night-shift.md](RFC-night-shift.md)** — the resident scheduler and
   tray residency. Smallest surface, unblocks everything nocturnal, and fixes
   a real V8 defect: scheduled reports don't fire without an open window.
2. **[RFC-brief.md](RFC-brief.md)** — the Brief, riding Night Shift v1. Its
   v1 synthesizes what the resident scheduler already produces (report runs,
   sync activity, source health) into one ranked arrival point with an audio
   edition; temperament ranking deepens as the Ledger and the Weave land.

The Ledger → Weave → Second Look chain follows; the Clerk's Whisper rung and
the Registry are independent tracks.
