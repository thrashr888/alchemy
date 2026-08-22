# RFC: Source Hygiene — check and clean outdated sources

Status: implemented (answers the backlog item "Data hygiene feature that checks and cleans outdated sources"). Built as designed with three v1 trims, each noted inline: duplicates are same-URL only, "Keep" suppression is frontend-local, and the report loop got a one-hour floor rather than the full cadence check.

## Summary

A source that was true when imported drifts: the page changes upstream, the file moves, the link dies, the same URL gets added twice. Today nothing notices — `Source` carries no freshness timestamp at all, and the only URL re-fetch loop in the app (`refresh_notebook_urls` in `commands/reports.rs`) blindly re-fetches every URL source before a report, with no cadence and no failure memory.

The proposal splits hygiene by reversibility, exactly like the note curator does:

- **Refreshing is reversible → automatic.** Stale URLs re-fetch on a cadence, edited local files re-ingest, all budgeted through the scheduler like the gist sweep. Default-ON; the Settings toggle is cost control.
- **Removing is not → proposed, never automatic.** Dead links, missing files, duplicates, and errored husks get flagged with a badge and a one-click review. Hygiene never deletes on its own — the curator's "archive, never delete" lesson applied to sources.

The clean-up verbs (refresh N, remove N) are the batch operations from [RFC-multi-select.md](RFC-multi-select.md); hygiene is the feature that decides *which* N to suggest.

## What exists today

- **No freshness signal.** `Source` (`models.rs`) has `created_at` only; `mtime` is overloaded (file mtime for folder children, content stamp for mac/git). `source_events` records refreshes but prunes to 30 days — a source refreshed 60 days ago and one never refreshed look identical.
- **The sweep pattern is shipped three times.** `gist.rs` (`content_hash`, `SWEEP_BUDGET`, single-flight bool), `router.rs` (`ensure_router`'s desired/stored diff), and the scheduler's 60s pass with per-concern throttles. Hygiene is the fourth tenant of the same shape, not new machinery.
- **A per-source time throttle already exists**: `git::remote_probe_due(id, minutes)` gates git/Notion remote probes in `resync_sources_inner`. That's the hook a URL re-fetch cadence wants.
- **Folder rescan already handles its children** (mtime-reconciled). The gap is standalone sources: individual URLs and single imported files are never revisited.
- **Refresh failure is destructive.** `refresh_source_url`'s web path calls `mark_source_failed` on error — wipes content, flips `status: "error"`. Acceptable when the user clicked Refresh; unacceptable for a background sweep hitting a transient timeout.
- **The note curator** (`curate_notes`) is the staleness-lifecycle precedent: app-open-day thresholds, any-use revival, archive-not-delete, silent badges (`stale` + `opacity-60` in StudioPanel).

## Design

### Data

Two columns on `sources`: `fetched_at` (i64 ms, set by `reingest` on every successful ingest/refresh; backfilled from `created_at`) and `fetch_failures` (i32, consecutive background-probe failures; reset to 0 on success).

> Operational note: schema append. Per the shared dev/prod store policy, land at the start of a release cycle and release promptly — older binaries brick on appends.

### The check (classification)

`hygiene.rs` scans a notebook and buckets sources:

| Bucket | Signal | Disposition |
|---|---|---|
| `stale-url` | `source_type == "url"` and `fetched_at` older than cadence (default 30 days) | auto-refresh |
| `stale-file` | standalone file whose disk mtime > ingested `mtime` | auto-reingest |
| `unreachable` | `fetch_failures >= 3` (distinct sweep passes) | propose remove |
| `missing-file` | local file path no longer exists (after iCloud hydration check) | propose remove |
| `duplicate` | same normalized URL in the notebook (content-hash dupes deferred — gist hashes cover only distilled sources, so they'd flag unevenly) | propose remove (keep oldest) |
| `husk` | `status == "error"` with no content, older than 7 days | propose remove |

### The clean

**Automatic half** — a `hygiene::spawn_sweep` sibling of the gist sweep, called from the scheduler pass: budget of 3 refreshes per pass, single-flight, skips archived notebooks, honors `background_enabled`. Two behavior changes to the refresh path it reuses:

1. **Non-destructive failure.** Background refresh keeps last-good content on error, increments `fetch_failures`, and does *not* flip `status`. Only user-initiated refresh keeps today's hard-fail semantics.
2. Unchanged content (same extract hash) updates `fetched_at` without re-embedding — `reingest` already diffs; this just short-circuits earlier.

**Proposed half** — surfaced, never executed:

- Source rows in the affected buckets get a quiet `Badge` ("unreachable", "duplicate", "missing") + `opacity-60`, the exact stale-note idiom.
- The sources panel header shows a one-line affordance when proposals exist ("3 sources need attention") opening a review modal: each item shows the reason and diff context, with per-item **Remove** / **Keep** (Keep suppresses the flag until the signal changes). Removal goes through the batch `delete_sources` command from RFC-multi-select.
- "Keep" for `unreachable` resets `fetch_failures` (real backend state — the retry cadence restarts); for the other buckets it's a per-notebook localStorage suppression. Deliberate: a kept duplicate is a viewing preference, the signal itself stays true, and the MCP report keeps showing it to agents.

### UI

- **SourcesPanel**: bucket badges on rows; "needs attention" header line + review modal (app modal, never `window.confirm`).
- **Settings → Sources**: one cadence select (week / month / 3 months / off), beside the git auto-sync cadence it mirrors.
- **MCP**: `source_hygiene` tool returning the classification report (agent decides what to do with it via existing `delete_source` / new `refresh_source` tools) — agent-reachable per house convention.

## Out of scope (v1)

- HTTP conditional GET (ETag / If-Modified-Since). The fetch layer surfaces no headers today; time-cadence + extract-hash diffing gets most of the value. Natural v2 stored alongside `fetched_at`.
- Cross-notebook duplicate detection; content-hash duplicates; per-source custom cadences; auto-archive of stale sources.
- A gallery "needs attention" filter chip (the panel affordance covers review; the FilterBar is value-faceted and a status chip would be its first special case).
- Fixing `refresh_notebook_urls`'s blind loop beyond a one-hour `fetched_at` floor (stops back-to-back schedule storms re-fetching the same pages).

## Alternatives considered

- **Derive freshness from `source_events`** — rejected: 30-day retention makes old-vs-never indistinguishable; a real column is honest.
- **Auto-delete dead links** — rejected: a 404 is often a moved page or an outage; the note curator's archive-not-delete stance has proven right.
- **Full LLM review of source content for "outdatedness"** — rejected for v1: expensive, confabulation-prone, and the mechanical signals (age, reachability, duplication) cover the actual complaint.
- **A separate hygiene daemon** — rejected: the scheduler's pass + budget pattern already exists; hygiene is a tenant, not a service.

## Recommendation

Build small: two columns in one migration, `hygiene.rs` (classify + budgeted sweep), non-destructive background refresh, row badges + review modal, one MCP tool. Ride the batch verbs from RFC-multi-select for the clean-up actions.
