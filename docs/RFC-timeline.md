# RFC: Timeline — the corpus by when it arrived

Status: draft, awaiting review (2026-09-12).
Tracking: `bd show alchemy-release-vjm`. Origin: Reminders item "Add a new global timeline visualization of
sources and notes (to the right of Chat and Registry, similar to our graph
alternate view). sources may update too often so just track initial
additions maybe. worth testing!"

## Summary

A fourth Home section, **Timeline**, beside Notebooks · Chat · Registry:
every source and note in the corpus laid along one time axis by the moment
it was *added*, grouped into the batches it actually arrived in, colored by
notebook. Home already answers "what do I have" (Notebooks) and "what is it
about" (Registry); this answers "when did it come in, and with what."

It is a read-only picture over columns the app has recorded since day one
(`created_at`), so it lights up with full history on first open, never
calls a model, and needs no new writes.

## Why additions, not updates

The reminder's instinct is right and the data confirms it. Sources carry
`updated_at` and the `source_events` table, but both churn: a folder
source re-reads on every FSEvent, a URL re-fetches on its cadence, Mac
items resync each tick. Plotting those puts the same document on the axis
dozens of times and says nothing. `created_at` is written once, at import,
and is what a person means by "when I brought this in."

`source_events` stays out of the first version for the same reason: it is a
30-day rolling window of churn. The timeline reads the entity tables.

## What the data looks like

From the running app (`activity_stats`, 2026-09-12):

| | |
| --- | --- |
| Sources | 1,284 |
| Notes | 563 |
| Notebooks | 45 |
| Days with activity | 50 of ~72 |
| Biggest single day | 264 sources (2026-07-19), 250 sources + 225 notes (2026-09-03) |
| Days with fewer than 5 additions | 27 |

Two things follow. Additions are **bursty**: a folder import, a starter
notebook, a sync heal lands hundreds of rows within minutes. And they are
**sparse** between bursts: many days hold one note. A timeline that plots
every row as its own mark is a few dense smears and long empty runs. The
unit has to be the *batch*.

## Design

### 1. Batches

A batch is one notebook's additions within a quiet gap of one another —
rows whose `created_at` are within 30 minutes of the previous row in the
same notebook. Batches are computed at read time from a sorted scan; no
stored grouping. A batch carries: notebook, start/end timestamps, source
count, note count, up to 8 sample titles, and the dominant source type
(the `sourceGroups` palette the gallery and graph already use).

30 minutes is a guess to test against the real store (a folder import of
264 files spans seconds; a reading session spans an evening). Tune, don't
configure.

### 2. The axis

Horizontal, newest at the right, the way the Activity heatmap and a person's
sense of "recent" both run. Two zoom levels, chosen by the visible range:

- **Months** (default: the whole history) — one lane per notebook, batches
  as pills sized by log(count) so a 264-file import reads as big without
  drowning a two-note evening. Lanes ordered by most recent batch, so the
  live notebooks sit at the top and the dormant ones sink.
- **Days** (zoomed in past ~6 weeks) — batches unfold into rows: each
  source and note gets its own tick with title, in arrival order.

Wheel and pinch zoom on the axis, drag to pan, one **Fit** button — the
same three verbs the graph view has (`GraphView.tsx`'s Plus/Minus/Crosshair
controls) and the same `viewMemory` habit so leaving and returning keeps
your place.

### 3. Marks and color

Notebook color is the lane's color (`notebookColor` from the sidebar
palette, the way the gallery tints multi-notebook cards). Source vs. note is
shape, not color: sources are filled pills, notes are outlined. Type shows
only on hover and in the day zoom, where there is room for an icon.

No colored left-border accents, no tonal fills for lanes — hairline lane
separators, tokens only (`DESIGN.md`).

### 4. Hover, click, filter

- Hover a batch: a hover card (the `useHoverCard` primitive) with the
  notebook, the date span, "12 sources · 3 notes", and the sample titles.
- Click a batch in the months view: zoom to its day.
- Click a tick in the days view: open the document in the reader
  (`openInReader`), same as the graph. Return restores the view.
- The `FilterBar` above (shared with graph and gallery) filters by notebook
  and by source group; the title filter matches sample titles.

### 5. Data path

One new query, `timeline_rows()`, in `db.rs` — `(notebook_id, id, kind,
title, source_type, created_at)` for every source and note, the
`collect_cols` shape `source_activity()` already uses (no content column,
so it is a column scan: ~1,900 rows today, milliseconds). Batching happens
in Rust so the MCP tool and the UI see the same batches:

```
timeline(since?, until?, notebook_id?) -> { batches: [...], range }
```

exposed as a `#[tauri::command]` and as an MCP tool of the same name (the
"agent-reachable" convention; an agent asking "what came in last week"
gets batches, not 250 rows). Cached per app run like the graph, invalidated
by the `mcp://changed` note/source events the store already listens to.

### 6. Where it lives

`HomeSection` gains `"timeline"`; `HomeSectionTabs` gets a fourth tab
(icon: `History` or `GanttChart` from lucide). The section renders in the
same slot `RegistrySection` does, full width, its own heading ("Your
timeline — 1,284 sources and 563 notes since July 2"). Nothing in the
notebook-level workspace changes; a per-notebook timeline is one filter
away and does not need its own tab.

## Out of scope

- Events (`updated`, `unreachable`, …) on the axis. If a later version wants
  them, they are a toggle that adds a second, thinner mark — not the default.
- Editing from the timeline (moving a source between notebooks by drag).
  Read-only until the picture proves useful.
- Messages and chat turns. They belong to the Activity heatmap; here they
  would swamp the documents.
- A per-notebook timeline tab in the workspace.

## Open questions for review

1. **Batch gap** — 30 minutes, or something the data argues for once the
   first render is on screen?
2. **Lane order** — most-recent-first (proposed) or the sidebar's order?
3. **Note origin** — should auto-created evidence notes (`origin: "auto"`)
   show, dim, or hide? Proposed: shown outlined and dimmer, same as the
   Studio list treats stale ones.
4. **Density** — 45 lanes is a tall picture. Collapse notebooks with no
   batch in the visible range into one "quiet" lane at the bottom?

## Implementation notes

- `db.rs`: `timeline_rows()` beside `source_activity()`; the batching fn
  is pure and unit-tested against synthetic bursts (a 264-row import in
  20 s, a note every two hours, two notebooks interleaved).
- `commands.rs` + `mcp/settings.rs` (next to `activity_stats`): `timeline`.
- `src/lib/types.ts` mirrors `TimelineBatch`; `api.timeline()`.
- `src/components/TimelineSection.tsx`: SVG like the graph — a few hundred
  batches is nothing for the DOM, and it gets hit-testing and theme tokens
  free. Pan/zoom math borrows `GraphView`'s transform handling.
- Gate: `pnpm build`, `cargo test` for the batching, and a look at the real
  store in the dev app before calling it done.
