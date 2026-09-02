# RFC: Events — feeds, watchers, arrivals, and live cards

Status: implementing on `cld/events` (2026-09-02) — phases 1–4 and 6 landed
and live-verified; phase 5 (cider deltas) waits on the cider 0.5 release.
Tracking: `bd show alchemy-release-jcd`.
Origin: "explore bringing in realtime or event-based data." Companion to
[RFC-night-shift.md](RFC-night-shift.md) (the scheduler and `source_events`),
[RFC-living-notebook.md](RFC-living-notebook.md) (growth, consent tiers), and
[RFC-v12-steward.md](RFC-v12-steward.md) (watchers as a pillar).

## Summary

Alchemy has the *consumer* half of an event system and almost none of the
*producer* half. The resident scheduler, the `source_events` table,
change-triggered standing questions, the Weave, the Brief, and the MCP
`list_source_events` tool all wait for events — and today the only event
kind that exists is `updated`, written only when a page or file the user
already imported gets re-fetched. The steward machinery is starved of input.

This RFC adds producers and the surfaces that make them worth having:

1. **Feed sources** (RSS/Atom/JSON Feed, plus feed-shaped hosts: GitHub
   releases, Wikipedia page history, arXiv queries, YouTube channels), with
   autodiscovery from pages the user already imported.
2. **A real event vocabulary** — `added`, `removed`, `unreachable`,
   `completed`, `moved` beside `updated` — and a per-source watch cadence
   for URL sources.
3. **Cheaper detection**: FSEvents for the open notebook's folders, a slower
   sweep for everything nobody is looking at, and item-level Mac events
   through cider's stable IDs and `modified_at`.
4. **Three surfaces**: event *filters* on change-triggered reports, an
   **Arrivals** strip, and **live answer cards** that re-render from data
   without re-asking the model.
5. **An agent stream**: a bearer-gated SSE endpoint beside `/mcp`, richer
   `list_source_events` filters, and `alchemy events --follow` in the CLI.

Under one rule that governs all of it: **an event never calls a model.**
Events mark work; the budgets and gates that already exist decide whether
that work happens. "Realtime" in this app means a card re-rendering from
fresh data, never a model re-answering.

## What exists

| System | Where | What it gives this RFC |
| --- | --- | --- |
| Resident 60-second tick | `scheduler.rs` | The clock every producer hangs off; already windowless |
| `source_events` table + `SourceEvent` | `db.rs`, `models.rs:289` | The event row: kind, detail, capped diff, rolling window |
| Change-triggered schedules | `scheduler.rs::is_due`, `ReportSchedule.trigger` | Standing questions already fire on events; they only lack filters |
| Folder sources | `commands.rs::rescan_one_folder` | Parent row + child sources, mtime-reconciled — the shape a feed reuses |
| Living Mac sources | `mac.rs`, `cider_lib` | `cider://` origins, content hash in `mtime`, resynced per tick |
| Content stamp | `mac.rs::content_stamp` | Change signal for content with no file mtime — reused for ETags |
| Growth proposals | `growth.rs` | The consent tray a discovered feed lands in |
| Freshness budget | `freshness.rs` | The nightly ceiling every model call charges against |
| Weave | `commands/weave.rs` | Judgment on arrival: cosine floor, 3 in flight, 4 pairs — the model gate |
| Hygiene sweep | `hygiene.rs` | URL re-fetch with a per-pass budget of 3 and the collapse guard |
| Renderer pattern | `MindMap.tsx`, `Flashcards.tsx` | Rigid text spec → native component, plain-Markdown fallback |
| MCP server | `mcp/mod.rs` (axum + rmcp 3.1) | The router the SSE endpoint mounts beside |
| Clip receiver | `clip.rs` | Loopback axum precedent, origin/bearer gating |
| Home steward strip | `HomeReportsFeed.tsx::AwayDigest`, `useHomeActivity.ts` | Where Arrivals lands on Home |

## The survey

Measured against the running app on 2026-09-01, through MCP, then each URL
and domain probed for autodiscovery (`<link rel="alternate">`, nine
well-known paths, host rules):

| | count |
| --- | --- |
| URL sources in active notebooks | 285 |
| Distinct domains | 132 |
| Pages advertising a feed in their HTML | 31 |
| Domains with a feed at a well-known path | 15 |
| Fetches refused (bot walls, login-walled banks) | 20 |

The fits are exactly the ones the steward vision names. **Earnings Reports**
holds investor-relations press-release feeds for seven of its companies —
the literal "tell me when the 10-K drops." **Alchemy Development** has the
Tauri and Ollama blogs plus GitHub release feeds for lancedb and
whisper.cpp. **Ferrari** has Bring a Trailer; **River House** has SFist;
**Wildfire** has Wikipedia page-history feeds for the FAIR Plan article.
The 29 arXiv sources have no per-paper feed, but category and search-query
feeds exist, which is the better subscription for a research notebook.
**Benefits & Memberships** is banks and airlines behind logins: feeds are
useless there and the clipper is the right tool. Feeds are an input for
roughly half the notebooks, not all of them, and the design should not
pretend otherwise.

## Proposal

### 1. The event vocabulary

`SourceEvent.kind` grows from one value to six. No schema change: `kind`
is already a string, `detail` the human line, `diff` the capped excerpt.

| kind | producer | `detail` example | `diff` carries |
| --- | --- | --- | --- |
| `updated` | URL re-fetch, folder file, git, Mac list, feed entry edited | "page re-fetched · +12 −3 lines" | ± diff (today) |
| `added` | folder scan, git pull, feed poll, Mac item created | "new entry · *Q2 results*" | entry excerpt |
| `removed` | folder scan, git pull, Mac item deleted | "file gone · report.pdf" | — |
| `unreachable` | hygiene strikes | "3 strikes · 404 since Aug 30" | last error |
| `completed` | Reminders | "✓ *Call the insurer*" | — |
| `moved` | Calendar | "Inspection · Thu 2 PM → Fri 10 AM" | — |

Every producer that already reconciles state (`rescan_one_folder`,
`git::sync_remote`, the Mac resync, `hygiene.rs`) writes the row at the
point it already knows what changed. The only new writer is the feed poller.
Events stay a rolling window; the durable artifacts are the sources and
notes they lead to.

### 2. Feed sources

**Shape.** A feed is a folder source whose root is a URL: a parent row with
`source_type: "feed"` and `url` = the feed URL, children per entry as
ordinary `url` sources with `parent_id` set and `mtime` = the entry's
published time. Retrieval, gallery, tags, archive, and hygiene all work
unchanged because children are just sources. The parent's `mtime` holds
`content_stamp(etag ‖ last-modified ‖ latest entry id)` so a 304 or an
unchanged document is a no-op at the same cost as a Mac resync today.

The parent is not empty. Its text is a **rolling index**: the feed's
description plus one line per kept entry (`- YYYY-MM-DD — [Title](link)`),
rewritten and re-embedded whenever an entry lands. "What's new in the
Tauri blog" retrieves the parent; a specific claim retrieves the child.
This is the same lesson the wiki fold and living reports taught: a
synthesized index over its own history gives the next judgement enough
context to be a judgement, and the `updated` event's diff keeps the
previous version in reach without a snapshot table.

**Entries.** When the feed carries full content (`content:encoded`, Atom
`content`), that is the child's text — no second fetch. When it carries a
summary only, the entry link goes through `extract_url` like any pasted
URL, including the page-capture fallback and the collapse guard. The
poller ingests at most **5 new entries per feed per pass** and **20 per
pass overall**; the rest wait. A feed keeps its **last 50 entries**; older
children are proposed for archive by the retirement pass in
RFC-living-notebook, never deleted silently.

**Cadence.** Per-feed, derived rather than configured: the median gap
between the feed's own timestamps, clamped to **30 minutes … 24 hours**,
doubled on each failure, reset on success. Conditional requests
(`If-None-Match`, `If-Modified-Since`) always. A feed whose host answers
429 or 5xx backs off exactly like a hygiene strike.

**Autodiscovery, three tiers, all cheap:**

1. *In hand.* `extract_url` already holds the page HTML; `<link
   rel="alternate" type="application/rss+xml|atom+xml|feed+json">` costs
   nothing extra. Found feeds land as a growth proposal of kind `feed`
   ("*Tauri blog* has a feed — follow it?"), never auto-subscribed.
2. *Well-known paths.* `/feed`, `/rss`, `/rss.xml`, `/atom.xml`,
   `/feed.xml`, `/index.xml`, `/feed.json`. One probe per domain, cached
   in a `feed_hosts.json` beside `git_hosts.json`, run only from the
   explicit **Follow updates…** menu item on a source — not from the sweep.
3. *Host rules.* GitHub repo → `releases.atom` and `commits.atom`;
   Wikipedia article → page-history Atom; YouTube channel → the channel's
   `videos.xml`; arXiv abs/pdf → an `export.arxiv.org/api/query` feed built
   from the notebook's standing queries (`growth::standing_queries`),
   never a category — a category is the whole field, and the point is to
   narrow the notebook, not widen it; Substack → `/feed`; Reddit → `.rss`. Rules are a
   table, not code paths.

The living-notebook consent rule holds: a feed on an origin already in the
notebook is still a *proposal*, one click to accept. Pasting a feed URL
directly into Add Source is consent by itself and imports immediately;
`ingest` sniffs `<rss`, `<feed`, and JSON Feed's `version` before treating
the URL as a page.

### 3. Watch cadence for URL sources

Today a URL refreshes when a report runs or when `hygiene.rs` judges it
aging (`fetched_at`, budget 3 per pass). That stays the default. A source
gains an optional **Watch** setting — hourly, daily, weekly — stored in a
new additive `watch_secs` column (0 = default behaviour; `add_batch`
conforms older writers, per the shared dev/prod store rule). The hygiene
sweep treats a watched source as aging once its cadence elapses, so the
re-fetch path, the collapse guard, and the `updated` event are all the
existing code. The Source menu shows "Watch · daily" the way it shows tags.

### 4. Detection cost

**FSEvents, scoped.** FSEvents is the kernel pushing to us at zero idle
cost; the *expensive* mechanism is the current one — a stat sweep over
every folder source every 60 seconds whether anyone is looking. So:

- Windows report their open notebook ids; the scheduler subscribes
  (`notify` crate, FSEvents backend) to folder sources of **open notebooks
  only**, debounced 2 seconds into the existing `rescan_one_folder` for
  that parent. Placeholder hydration and iCloud eviction ride the same
  path they do today.
- Closed notebooks' folders move from a 60-second to a **10-minute** sweep
  (`CLOSED_SWEEP_MS`, a constant, not a setting). Mac and feed polls stay
  on their own cadences regardless, because they are hash checks, not
  directory walks.
- Opening a notebook triggers one immediate scoped resync, which
  `resync_sources_inner(only_notebook)` already does.

Debounce matters more than latency: a folder sync tool writing 400 files
must land as one rescan and one coalesced batch of events, or it recreates
the Lance scan storms.

**Mac events via cider.** cider reads the Reminders and Calendar SQLite
stores directly and already returns stable IDs and `modified_at`. Upstream
(`cider_lib`, fix-there-then-bump per house rule), each list call gains
`since: Option<DateTime>`; Alchemy keeps the last high-water mark per Mac
source and asks for the delta, producing `added`/`completed`/`moved`/
`updated` per item instead of one whole-list diff. A later `cider watch`
that FSEvents the store files and yields a change stream slots in behind
the same call, so Alchemy's side does not change twice. Mail stays out as
a source, as RFC-cider-tools decided; nothing here reopens that.

### 5. Triggers: filters on change-triggered reports

`ReportSchedule` gains two additive fields (serde default, older binaries
unaffected):

- `watch_sources: String` — space-separated source ids; empty = any source
  in the notebook (today's behaviour).
- `watch_kinds: String` — space-separated event kinds; empty = any.

`is_due` and `due_at` filter `events` by both before the existing checks;
the interval stays the throttle floor. The editor in `Reports.tsx` grows one
row under "On change": a source picker (the multi-select from
RFC-multi-select) and kind chips. "When a new entry arrives in these three
IR feeds, summarize what changed and whether it touches my thesis" is now a
schedule. No cron grammar, as RFC-night-shift decided.

### 6. Arrivals

One strip, two places, reading `source_events_since` and nothing else:

- **In a notebook**, above the sources list when there are unseen events:
  "3 new from *Tauri blog* · 1 page changed · 2 reminders done". Expands to
  the events, each linking to the entry, the diff, or the item. Source rows
  carry a new-dot until the strip is dismissed. The seen watermark is a
  per-notebook `seen_events_at` in `localStorage` (mind the StrictMode
  restore gotcha), not a table — it is a UI convenience.
- **On Home**, `AwayDigest` gains the same tallies beside reports, in the
  steward sidebar RFC-v12-steward §2 reserved.

Notifications do not change: only standing questions notify, and only when
they produce something. An arrival is not news; what it changed might be.

### 7. Live answer cards

When a chat reply cites a **live source** — `mac`, `feed`, or a URL with
`watch_secs` set — the reply renders a native card beneath the prose:

- Stocks → a quote table (symbol, price, change, as-of).
- Calendar → the cited events with times.
- Reminders → the cited items with due dates and state.
- Feed → the cited entries with published times.

The card is a **renderer over the source's stored text**, exactly the
MindMap/Flashcards contract: the Mac and feed writers already emit rigid,
line-structured text (cider's JSON rendered as fixed columns; feed entries
as `title / published / link / excerpt` blocks); the card parses it and
falls back to nothing when it cannot. Each card shows **fetched-at** and a
refresh affordance that calls `refresh_source`. When `sources://changed`
names a cited source, the card re-reads the source and re-renders. The
prose above it is not touched, and no model runs. A card that is stale says
so instead of pretending.

Cards are not a new message kind and are not persisted: `Message.citations`
already carries the `source_id`; the card is derived at render time, so
every existing transcript lights up retroactively and nothing migrates.

### 8. The agent stream

Agents get events three ways, cheapest first:

- **`list_source_events`** gains `since`, `kinds`, `source_ids`, and
  `notebook_id` filters, and returns a `cursor` (the last `at`) so a
  polling agent asks for exactly the delta.
- **`GET /events`** on the MCP axum router, bearer-gated like `/mcp`,
  Server-Sent Events, `?since=<ms>` replays from the rolling window then
  tails. One `tokio::sync::broadcast` fed at `add_source_event`; a
  disconnected client costs nothing. The CLI RFC's `alchemy events
  --follow` is this endpoint with a pretty-printer.
- **Hosted agents (ACP)**: the pane's `session/new` already hands the agent
  Alchemy's MCP endpoint; the events URL goes in the same config block.
  Whether Claude Code acts on MCP `notifications/resources/updated` is
  unverified, so MCP-native notifications are deferred until a client is
  shown to consume them. SSE is the safe bet and needs no client support.

"When the 10-K drops, run *my* analysis" then has two implementations: a
change-triggered report whose Generate role is an agent CLI (works today,
gated below), or the user's own agent tailing `/events` and deciding.

## Cost rules

Written here so every phase inherits them:

1. **Events never call a model.** A producer writes a row and stops. Model
   work happens only through gates that already exist: the Weave (cosine
   floor 0.45, 3 in flight, 4 pairs, then the hourly nightly pass), standing
   questions (interval floor, one run per tick, off-tick execution), and
   the freshness ceiling (`freshness::has_budget`). Feeds add no new gate
   and get no exemption.
2. **Ingest is the only per-event cost**, and it is local embedding.
   Caps: 5 entries per feed per pass, 20 per pass overall, 50 kept per
   feed. Entries batch through the existing chunk/embed path in one
   `add_batch`, and `flush_fts` runs once per pass, not per entry.
3. **Detection is free or it is wrong.** FSEvents over polling; conditional
   HTTP over re-fetch; `since` deltas over whole-list diffs; a 10-minute
   sweep for notebooks nobody has open. If a producer shows up in `top`,
   the producer is the bug.
4. **Cards cost zero inference**, by construction: they parse stored text.
5. **Agent runs are the priciest thing in the app.** A change-triggered
   schedule whose Generate role is an agent CLI gets a hard **4 runs per
   day** cap and waits for `idle_ms()` like a deep run; the receipt says
   when a run was skipped for the cap.
6. **Background work must settle.** Every new loop is single-flight, holds
   no `Ai` guard across an await, and shows up in `list_receipts` when it
   spends. Diagnose with `lsof -i :11434` and `ollama` server-log
   per-minute counts before blaming a model.

## Agent reach

Same change, same release, per the house convention:

| Surface | Verb |
| --- | --- |
| Feeds | `add_source` accepts feed URLs (sniffed); `discover_feeds(source_id)` returns the three tiers' candidates; `follow_feed(url, notebook_id)` |
| Watch | `update_source` gains `watch_secs` |
| Triggers | `schedule_report` gains `watch_sources`, `watch_kinds` |
| Events | `list_source_events` filters + cursor; `GET /events` SSE |
| Cards | none — derived in the UI; agents read the same source text |

## Staging

Each phase is its own change with its own gate, in dependency order:

1. **Vocabulary + producers** — `added`/`removed`/`unreachable` from the
   existing reconcilers, trigger filters, `list_source_events` filters.
   *Gate:* a folder add of 3 files writes **one** `added` event naming the
   three files (never one row per file — cost rule 3) and one
   `sources://changed`; a filtered standing question ignores events outside
   its filter (unit tests beside `is_due`).
2. **Feeds** — parent/child shape, poller, sniffing, tier-1 discovery into
   the proposal tray, Follow updates… with tiers 2–3.
   *Gate:* subscribe the seven Earnings IR feeds; over one week the poller
   makes ≤ 7 × 48 conditional requests, a new press release becomes a
   source within one cadence, and `list_receipts` shows **zero** model
   spend attributable to arrival.
3. **Arrivals + cards** — strip, new-dots, Home tallies, four card
   renderers with fallback.
   *Gate:* a question over the Stocks watchlist renders a quote card; a
   manual `refresh_source` re-renders it with a new as-of and no new
   message; a corrupted source text renders prose only.
4. **Detection** — FSEvents for open notebooks, 10-minute closed sweep.
   *Gate:* with 5 folder notebooks closed, the minute-tick's folder walk
   disappears from a 10-minute `sample`; a file saved in the open
   notebook's folder is a source within 5 seconds.
5. **cider deltas** — `since` upstream, release, bump; item-level Mac
   events. *Gate:* completing a reminder writes one `completed` row, not an
   `updated` diff of the list.
6. **Agent stream** — `/events` SSE, CLI `events --follow`, ACP config.
   *Gate:* `curl -N` tails a live `added` event within a second of the
   poller writing it. (Landed: `events.rs`, `alchemy events [--follow]`,
   `events_url` in the discovery file. The ACP `session/new` hand-off is
   still open.)

Phases 1–3 are the product; 4–6 are cost and reach. Nothing here is a
timeline.

## What this deliberately does not do

- **No websocket or streaming market data.** Stocks is a poll of the
  Apple Stocks watchlist through cider, as today. A quote card says
  "as of 4:02 PM", which is the truth.
- **No daemon, no cron grammar, no job queue** — RFC-night-shift's refusals
  stand. Due-ness stays derived from persisted state; the SSE broadcast is
  in-memory and lossy by design (the table is the replay).
- **No Mail source.** Mail *events* are attractive and stay out for the
  same privacy reason RFC-cider-tools gave.
- **No auto-subscribe.** Discovery proposes; the user follows.
- **No per-event summaries by default.** The Small role summarizing every
  arrival is exactly the model-per-event pattern rule 1 forbids. The Brief
  already summarizes the night's events once, under budget.

## Risks

- **Feed corpus growth.** 50 entries × N feeds is real weight in a notebook
  that was 40 sources. The retirement pass and the gallery's "feed"
  grouping (children collapse under the parent, as folders do) keep the
  sources panel usable; RFC-living-notebook's scale work is the backstop.
- **Entry pages behind bot walls.** Summary-only feeds whose links refuse
  the fast path fall to the page-capture webview, which is the most
  expensive fetch in the app. Cap: capture at most 3 entry pages per pass;
  the rest keep the feed's summary as their text and are marked for
  hygiene's next pass.
- **FSEvents on cloud-synced folders** fires for every placeholder
  materialization. The 2-second debounce plus the existing placeholder
  status handle it, but this is the phase-4 gate to watch.
- **Card parse drift.** cider output changing shape breaks the stocks card
  silently into "no card". Fallback is prose, so nothing is lost, and a
  fixture test per card pins the format.

## Decisions (2026-09-01)

Three questions the draft left open, settled in review:

- **Feed parents hold a rolling index** of their kept entries and re-embed
  on change (§2). Reports and the wiki fold got measurably better once they
  could see indexes and prior versions of themselves; feeds start there.
- **Arrivals watermark is per notebook**, in `localStorage`. Notebooks are
  the isolation boundary the user reaches for, while the registry, staff
  history, activity, and global chat all draw their value from everything
  living in one database — so the watermark is UI state, not a table, and
  nothing about events becomes notebook-partitioned storage.
- **arXiv follows a query, never a category.** A category feed is the
  whole field and would widen a notebook's focus into noise. The host rule
  builds an `export.arxiv.org/api/query` feed from the notebook's standing
  queries and offers *that*, as a proposal like every other discovered feed.
