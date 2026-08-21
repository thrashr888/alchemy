# RFC: Activity View — your research, measured

Status: accepted

## Summary

An **Activity** tab in Settings that shows how you actually use Alchemy: a
year-long heatmap of daily activity, streaks, totals, your most-used models and
most-active notebooks, and one fun corpus-scale comparison. The inspiration is
the activity/profile views in tools like Claude Code and GitHub — small,
glanceable, quietly delightful.

The key insight: **almost everything is already tracked.** Every notebook,
source, note, and message row carries `created_at`; retrieval traces carry
`ts`. No new writes, no new tables, no counters to keep consistent — one
read-time aggregation over data the app has recorded since day one. That also
means the view is retroactive: it lights up with full history the first time
it opens.

## What we can measure today

| Signal | Where it lives | Notes |
| --- | --- | --- |
| Messages (user + assistant turns) | `messages` table | `role`, `kind` ("tool" rows excluded), `model` caption, `created_at`, content length |
| Sources imported | `sources` table | `source_type`, `created_at`, `char_count` |
| Notes created | `notes` table | `created_at` |
| Notebooks | `notebooks` table | titles for the "most active" join |
| Retrievals | `traces/retrieval.jsonl` (+ rotated `.1`) | `ts` per record; rotation caps history at ~5 MB (~months), so this one is "recent", not lifetime |
| Words written by models | assistant message content | measured word count, not estimated tokens |
| Corpus size | `sources.char_count` | powers the book comparison |

Deliberately **not** in v1: token counts (providers report them inconsistently
and we never persisted them — an estimate would violate the "claim only
measured numbers" rule), session counts (Alchemy has no session concept), and
cost totals (already surfaced per-message in chat captions).

## Design

### Derived metrics

- **Per-day series** — messages, sources, notes, retrievals bucketed by local
  calendar day. One series, all-time; every range view derives from it.
- **Active days / current streak / longest streak** — a day is active if any
  bucket is non-zero. Current streak anchors on today *or yesterday* (opening
  the app at 9am shouldn't show a broken streak before you've done anything).
- **Peak hour** — mode of the message-hour histogram, local time.
- **Favorite model** — top assistant `model` caption with the ` · $0.04` cost
  suffix stripped; ties break toward the more recent.
- **Most active notebooks** — message count per notebook, joined to titles.
  Deleted notebooks aggregate as "(deleted)" and drop to the bottom.
- **Source types** — import counts by `source_type`.
- **The words line** — corpus chars ÷ 6 ≈ words: "Your sources hold ~2.1M
  words." (A book-scale comparison was tried and cut — one whimsy line was
  enough without it.)

### Backend

- `activity.rs` — pure aggregation: takes row-level metadata + trace day
  counts, returns `ActivityStats`. Unit-tested date math (streaks, local-day
  bucketing, hour histogram).
- `db.rs` — three projected scans (`collect_cols`), none of which drag source
  content through Arrow: message meta (notebook, role, kind, model, ts,
  content chars), note timestamps, source meta (type, ts, chars).
- `commands::activity_stats` — the IPC surface; reads trace files under
  `state.trace_dir` for retrieval counts.
- MCP tool `activity_stats` in the settings router — same aggregation, JSON
  out, so agents can answer "how much have I been using Alchemy?" (features
  ship agent-reachable, per convention).

### Frontend

`Settings → Activity`, between Shortcuts and About:

- **Range toggle** (All · 30d · 7d) — client-side slicing of the day series;
  totals like notebooks/corpus stay all-time.
- **Stat tiles** — hairline-bordered grid (no tonal fills): Messages, Sources,
  Notes, Active days, Current streak, Longest streak, Peak hour, Favorite
  model.
- **Heatmap** — 13 weeks × 7 days, GitHub-grammar (a full year overflowed
  the pane; a quarter fits without scrollbars). Cells are `--primary` at
  stepped opacity (0 → border-only). Intensity levels scale to the user's own
  p90, not a fixed scale, so light users still see texture. Month labels on
  top, `title` tooltips per cell. Color carries meaning here (quantity), so it
  complies with DESIGN.md's "color only when it means something".
- **Lists** — Most-used models and Most-active notebooks as quiet two-column
  rows (label left, count right), mirroring the Claude Code plugin list.
- **The book line** — one caption under the heatmap.

### Performance

One command call per tab open. The scans are projected; the biggest is the
messages table (chat text — a heavy user is single-digit MB). No caching in
v1; if it ever shows up in a profile, cache by `(messages.len, sources.len)`.

## Future

- Persist real token/cost usage per turn (schema append on `messages` —
  bundle with a release per the shared dev/prod store policy) and add a
  models-over-time stacked bar like Claude Code's Models view.
- Weekly/cumulative heatmap toggles.
- Surface a mini streak/heatmap on the Home view once it earns it.
