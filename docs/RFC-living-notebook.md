# RFC: The Living Notebook — scale, and a corpus that grows itself

Status: draft, for review
Origin: "consider how Alchemy might scale its ability to handle many more
files within a single notebook. It's a context size and UX problem, and
potentially a new feature to proactively scan, find and organize new
related content." Companion idea: an auto-expanding personal wiki that
crawls your world 24/7 toward 1M+ primary sources.

## Problem

Two of the three problems in the prompt are already solved or scoped:

- **Context size** is RFC-infinite-context (shipped, all five phases):
  gists, distilled embeddings, scale-adaptive evidence assembly, lazy
  map-reduce for global questions, model-tiered packets. Retrieval
  quality is designed to stay flat as a notebook grows to 10M+ chars.
- **Retrieval routing** is RFC-retrieval-maturity (shipped): hybrid
  search, semantic router, deep-search, traces.

What is *not* solved:

1. **UX collapses before retrieval does.** The Sources panel is a flat
   scrolling list with per-row chrome; at 500+ sources it is unusable for
   navigation, selection, and hygiene. Import queues, folder scans, and
   gallery views all assume tens, not thousands.
2. **Growth is manual.** Every source arrives because Paul dragged it in.
   A notebook about a topic never says "these three links inside your own
   sources cover the gap you keep asking about" — even though the app
   already knows the citations that come back empty.
3. **Organization is manual.** Tags, titles, and archive state exist, but
   nothing curates at scale: no clustering, no duplicate collapse, no
   "this source never retrieves — mute it?".

The wiki idea is these three pillars running unattended: a notebook that
scans, fetches, files, and prunes on its own schedule. It should be an
Alchemy capability, not a second app — everything it needs (ingest
pipeline, embeddings, router, gists, night shift scheduling, MCP for
agent-driven imports, registry for entities) already lives here.

## What already exists to build on

| System | What it gives the living notebook |
| --- | --- |
| Folder sources + watch | unattended local ingest, placeholder hydration |
| Reports / night shift | scheduled background work with a cost dial |
| Registry (V12) | entities auto-attach; names propose themselves |
| Router + gists | cheap per-source understanding for clustering |
| Retrieval traces | ground truth on which sources earn their keep |
| MCP server | agents can add/organize sources programmatically |
| Background-refresh collapse guard | refuses gutted refetches |

## Prior art & outside ingredients (researched)

### WikiSkill (arXiv:2608.27454) — the loop, validated

WikiSkill separates raw execution experience, accumulated knowledge, and
executable skills, continuously consolidating experience into a wiki;
ablations show the persistent wiki is what makes skill evolution work,
and it lets small models beat much larger ones. That is this RFC's loop
with benchmarks behind it: retrieval traces are the raw experience,
standing queries and gists the accumulated knowledge, and Pillar 3's
generated wiki pages the consolidated layer the next retrieval builds
on. It argues for consolidation being *continuous* (night shift), not
on-demand only.

### Firecrawl — the frontier fetcher we don't have to build

Verified against docs.firecrawl.dev and the pricing page:

- **Free tier: 1,000 credits/month, no card.** Search, scrape, and
  interact work **keyless** at low rate limits (2 concurrent); an API
  key only raises limits. Costs: scrape/crawl/map = 1 credit/page,
  search = 2 credits per 10 results (+1/result to also return scraped
  markdown).
- **`POST /v2/search`** takes a query plus `categories` — including
  **Research** (academic sites), **PDF**, and **Developer** (repos +
  docs) — `includeDomains`/`excludeDomains`, and time filters, and can
  return each hit's content as markdown in the same call. That output
  drops straight into the existing ingest pipeline.
- **Research Index** (`GET /search/research/papers[...]`): ~43M
  abstracts (PubMed, bioRxiv, medRxiv, arXiv) with passage reading and
  citation-network navigation (`/similar` with citers/references
  modes) — real primary sources for research notebooks, and the
  citation graph is a frontier in itself.

Budget sketch at nightly cadence, all keyless: 2 standing queries ×
2 credits + 5 accepted scrapes ≈ 9 credits/night ≈ 270/month — inside
the free tier with 3× headroom. The growth agenda should meter itself
against this (per-notebook credit ledger, stop at a soft cap) the same
way night shift meters model work.

### Atlas for Mac — the scale-UX bar

"Zoom out and see a thousand things at once, zoom in and look at one
properly." Atlas handles thousands of visual items with three views
(dense grid / spatial canvas / drift), nested collections, and
connected external folders browsed "at library speed." Concrete takes
for Pillar 1: a density-zoomable grid on the existing gallery (zoom
level = information per row), group rows as first-class objects, and
treating watched folders as browse-in-place rather than import-then-
browse.

### OpenKnowledge — the wiki's format, not a competitor

OK is markdown/MDX + git as source of truth, a WYSIWYG editor over it,
and an MCP server + skills so agents search, edit, and reorganize the
same files humans do; hierarchical RAG and wiki-link graph views sit on
top. Alchemy already speaks the relevant dialect: OKF export/import
ships today (`export_notebook_okf`/`_zip`, `probe_okf`,
`import_notebook_okf`), and the reader already renders Obsidian-style
`[[wikilinks]]`. So Pillar 3's wiki view is not a new surface: generate
cluster/entity index pages as ordinary notes with wikilinks, and the
whole wiki round-trips through OKF for free — portable, git-syncable,
editable by OK's own agents or ours over MCP.

## Design

### Pillar 1 — UX at scale (the prerequisite)

- **Virtualize the Sources list** (windowed rendering — the panel maps
  every row today; @tanstack/react-virtual is the current standard);
  target 10k rows.
- **Search-first navigation**: the filter box becomes the primary way in;
  add type/tag/freshness facets. Cmd+P-style jump already exists for
  notebooks; extend to sources.
- **Rollups**: collapse folders/domains/tags into group rows with counts;
  a notebook shows dozens of groups, not thousands of rows.
- **Hygiene affordances**: bulk archive/mute from the group row; a
  "never retrieved" filter fed by traces.

Gate: a 5k-source fixture notebook scrolls at 60fps, filters under
100ms, and imports without wedging the queue UI.

### Pillar 2 — proactive growth (scan & find)

A per-notebook **growth agenda**, default-on but budgeted like reports:

- **Gap detection**: retrieval traces record questions whose evidence was
  thin (low scores, empty citations). Those become standing queries.
- **Frontier expansion**: existing sources already contain the frontier —
  outbound links in web sources, references in PDFs, siblings in watched
  folders, backlinks in Notes/Obsidian. Rank frontier items against the
  standing queries with the existing embedder; propose the top few.
- **Local tier via Spotlight and cider**: standing queries also run
  through `filesearch.rs` (mdfind, already ranked and junk-filtered) and
  the cider integrations, so the frontier includes files already on this
  Mac and notes/reminders in Apple apps — the sources that need no
  network consent at all. These proposals rank above web ones: the
  cheapest fetch is the one that never leaves the machine.
- **Open-web tier via Firecrawl**: standing queries run through
  `/v2/search` (Research/PDF/Developer categories, domain filters) and
  the Research Index for academic notebooks; accepted proposals scrape
  to markdown through the same API. Keyless free tier first; an API key
  in Settings only raises limits.
- **Consent tiers**: local files and already-subscribed feeds import
  automatically; anything that fetches a *new* network origin lands in a
  proposal tray ("Found 6 related pages — add?") rather than importing
  silently. The collapse guard applies to every unattended fetch.
- Runs inside night shift's budget dial, plus a per-notebook Firecrawl
  credit ledger with a soft cap, so a month of nightly sweeps stays
  inside the 1,000-credit free tier.

Gate: on a seeded topic notebook, the tray proposes ≥5 relevant new
sources within one night cycle, with zero silent network imports.

### Pillar 3 — self-organization (curate)

- **Cluster pass** (Small role, over gists): propose tag groups and
  merge/duplicate candidates; one-click apply, like registry proposals.
- **Retirement pass**: sources with zero retrievals over N cycles get
  proposed for archive — the corpus stays sharp as it grows.
- **Wiki view** (the north star, last): a generated index note per
  cluster — entity pages from the registry, linked citations — rendered
  as a browsable start page for the notebook. This is the "personal
  wiki" reading surface; the crawler underneath is Pillar 2.

Gate: curation never mutates without a proposal step; every apply is
undoable; eval fixtures keep retrieval metrics flat after a curation
pass.

## Phasing

1. ~~Pillar 1 (UX)~~ — shipped: virtualized panel, facets, rollups,
   uncited filter, 5k fixture (12ms filter keystrokes, 21–35 mounted
   rows at any scroll position).
2. ~~Pillar 2 free tiers + tray~~ — shipped: standing queries from thin
   retrievals, Spotlight tier (two-token name matches), mined-link tier,
   Sources-panel tray.
3. ~~Grow center pane + open-web tier~~ — shipped: a Grow mode beside
   Chat/Reader/Gallery/Ledger showing the hungry-for queries and all
   tiers; Firecrawl keyless search behind a per-notebook enable, metered
   in traces/growth.jsonl against an 800-credit soft cap (measured: 2
   credits per query).
4. ~~Pillar 3 v1~~ — shipped: the retirement pass (old + never-cited →
   Mute/Keep/Remove proposals in Grow's Tidy section; the cluster/tag
   half was already live as gist.rs's ensure_tags sweep), and the wiki
   index — a deterministic generated note grouping sources by tag with
   title links the reader resolves, refreshed in place, OKF-portable.
5. ~~Phase 5~~ — shipped: the wiki re-derives on every gist sweep
   (WikiSkill's continuous consolidation, write-skipping), and it grew
   beyond one note — a page per registry entity filed in the notebook
   (facts + documents, title-linked both ways), plus tag-merge
   proposals (plural/singular and separator variants) in the Grow
   pane's Organize section. Notes link notes by title now, so the
   wiki cross-references itself. Still open: nightly growth sweeps for
   web-enabled notebooks (the opt-in flag lives client-side today).
6. ~~Progressive Grow~~ — shipped (alchemy-release-hxl). The pane used to
   render nothing until one `growth_proposals` call had computed every
   free tier, so the slowest tier set the time-to-first-pixel for the
   whole surface. It is now one call per section — `growth_queries`,
   `growth_feeds`, `growth_links` beside the existing `growth_local`,
   `growth_retire`, `growth_tag_merges` and `source_hygiene` — fired
   concurrently, each rendering when it lands, with the last known result
   per notebook served from the store cache on return.
   `growth_proposals` stays as the one-call aggregator agents get over
   MCP, and a test asserts the per-section union is exactly what it
   returns (feeds subtract from links in `growth_links_impl`, so the
   sections are disjoint and the cross-tier precedence stays next to
   `canonical_key`).

   **Measured** (3,000 sources, ~21 MB of text, debug build): the slow
   tier is link mining — `growth::proposals` walks every source's text
   looking for URLs, 6.2 s — and it was the one holding the pane hostage.
   Feeds cost 32 ms, hygiene 20 ms, and the whole notebook's text now
   loads once instead of once per section: three unshared
   `sources_with_content` scans took 341 ms, against 77 ms for the first
   shared scan and 2 ms for the two behind it (db.rs `SourcesContent`,
   keyed on the sources table's Lance version). Each section logs its own
   `grow section <name>: <n> rows in <ms>ms` line, so a future slow tier
   names itself in the log.

At 1M+ sources the answer is "many notebooks + meta-chat", not one
table: the router already federates across notebooks, and per-notebook
budgets keep any single corpus healthy. Revisit only if a real corpus
proves otherwise.

## Risks

- **Creep toward a crawler.** The consent tiers are the design; open-web
  fetching stays opt-in per notebook, forever.
- **Proposal fatigue.** Cap tray size; decay stale proposals; batch to
  night-shift cadence rather than interrupting.
- **Cost.** All model work runs on the Small role over gists, inside the
  existing background dial; Pillar 1 costs nothing.

## Open questions

- ~~Does the proposal tray live in the Sources panel or the Brief?~~
  Decided: the tray row lives in the Sources panel, but review graduates
  from a modal to a center-pane "Grow" surface (a `CenterModeTabs` mode
  beside Chat/Reader/Gallery/Ledger) once local + web tiers land — a
  review workflow deserves real estate, and the center pane is where
  this app puts workflows.
- Should standing queries be visible/editable (a "what this notebook is
  hungry for" list), or stay implicit from traces?
- Frontier ranking: embedder-only first — Firecrawl's search already
  ranks, so our embedder only re-ranks against the standing query; add a
  Small-role rerank only if the eval fixture shows it earns its cost.
- Wiki pages as notes round-trip through OKF today; do we also want OK's
  own MCP registered as a peer (their agents editing our wiki), or is
  export enough?
