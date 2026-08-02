# RFC: The Brief — one ranked arrival point, with an audio edition

## Summary

One synthesized document per cadence that tells the user what happened, what
changed, and what needs them — ranked by *needs a decision*, not chronology —
with a two-voice audio edition rendered on-device. The Brief is the second
move of [RFC-v12-steward.md](RFC-v12-steward.md), riding directly on
[Night Shift v1](RFC-night-shift.md): its v1 synthesizes only what the
resident scheduler already produces. **The ranking function is the product**;
everything else reuses shipped machinery.

V8's `AwayDigest` lists activity since your last visit; a steward triages.
The difference between a feed and a chief of staff is the ordering.

## Background — everything needed already exists

- **Assembly:** `global_meta_route` (commands.rs:7373) already does
  cross-notebook map-reduce over gist rows for corpus-wide answers.
- **Standing documents:** `run_report` threads the prior run
  (`collapse_report_notes`, commands/reports.rs:88) so each brief can open
  with what changed since the last one.
- **Kinds:** `ARTIFACT_KINDS` (rag.rs:450) + `resolve_report_kind`
  (commands.rs:3934) — a `brief` kind slots into the registry every other
  surface (Studio tiles, MCP `generate`, schedules) reads.
- **Audio:** `tts.rs` — Kokoro-82M two-voice synthesis, download-and-verify,
  fully local. The podcast pipeline needs a script; a brief is a script.
- **Arrival surfaces:** `HomeReportsFeed.tsx` with unread state,
  `AwayDigest`/`useHomeActivity.ts`, desktop notifications (Rust-side after
  Night Shift v1), and the tray's top block reserved by RFC-night-shift §3.
- **Agent access:** briefs are notes in a notebook — `get_note`/`list_notes`/
  search over MCP work with zero new code, including windowless (the MCP
  server is resident after Night Shift v1).

## Proposal

### 1. A brief is a scheduled report with corpus scope

A new `brief` report kind. A brief schedule differs from a report schedule in
scope only: it reads across **all notebooks** (v1) rather than one. Default:
one daily brief, morning-scheduled, created on first run of Night Shift v1 —
smart defaults on; delete it like any schedule if unwanted.

Runs land as `brief`-kind notes in a system notebook (**"Briefs"**, created
lazily) — an ordinary notebook by design, so Reader, search, OKF export, and
MCP all work for free and agents can read this morning's brief with
`list_notes` on one well-known notebook.

### 2. Assembly: collect, rank, then write

Per run, over the window since the previous brief (whose note is the prior-run
input, so the window travels with the document):

1. **Collect** (plain queries, no model): report notes created and their
   unread state; sources whose `updated_at` moved, grouped by notebook;
   sources in error state (OCR gate failures, dead URLs, failed refresh);
   schedule runs that errored. Later stages add Ledger obligations, Weave
   verdicts, and watcher diffs to this list as those pillars land — the
   collector grows, the shape doesn't.
2. **Rank** (the pillar): *needs a decision* (errors, rescan asks, anything
   with an action) → *changed* (source and report deltas, most-active
   notebook first) → *for the record* (completed runs, quiet notebooks). Hard
   deadlines (future Tickler rows) always notify immediately rather than
   waiting for the brief.
3. **Write** (one generation call): the ranked collection becomes the
   generation context — the same waterfill/distill path every artifact uses —
   with a fixed section shape: decision items with one-line "what happens if
   you ignore this," then changes, then the record. Citations anchor to the
   underlying notes and sources so clicking through opens the real thing.
4. **Voice** (background, non-blocking): the brief script renders through the
   existing two-host pipeline; the toolbar shows "Play audio · m:ss" when
   ready. Audio failure never blocks the note — degrade to text, silently.

### 3. Arrival

- **Home → Brief section** (RFC-v12-steward UI §2): current brief with inline
  audio, archive below, `AwayDigest` folded in as the between-briefs view.
- **Brief card + unread dot** above the reports feed; the tray's top block
  gains its 2–3 headline items (the slot RFC-night-shift reserved).
- **One notification** on arrival ("Your brief is ready · 2 decisions"),
  respecting the migrated notifications setting. Never one per item.

### 4. Temperament, staged

v1 ships one global brief. Temperament arrives as a per-notebook setting
(cadence + ranking flavor: decision-ranked / due-date-ranked /
delight-ranked) once the Ledger gives the ranker real material — that's when
the Sunday household brief and the Saturday curiosity brief split off, each
just another `brief` schedule with a notebook filter. The mechanism doesn't
change; the filter and the ranking weights do.

## Non-goals

- **Not a feed.** No infinite scroll, no per-item cards competing for
  attention, no engagement mechanics. One document, sections in rank order,
  done.
- **No new tables, no new pipeline.** A brief is a report kind + a system
  notebook. If the report machinery can't express something the brief needs,
  fix the report machinery.
- **No per-item notification fan-out**, ever. The brief exists to *absorb*
  interruptions.
- **Background work off = no briefs** — the master switch
  (RFC-night-shift.md) governs brief schedules like everything else.

## Verification

- Schedule a brief, close the window overnight (Night Shift v1 running):
  morning brief note exists in Briefs, one notification arrived, unread dot
  on Home; audio plays with a real duration.
- Ranking honesty: seed one failing source + one changed source + one
  completed report → the failing source leads the brief.
- Prior-run threading: second brief opens with "since Tuesday's brief" and
  doesn't repeat absorbed items.
- Agent path: `list_notes` on Briefs over MCP returns the brief with no
  window open.
- Gates: `cargo fmt -- --check && cargo clippy -- -D warnings && cargo test`.
