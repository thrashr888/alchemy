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

## Prior art & outside ingredients

- **Firecrawl Research Index** (docs.firecrawl.dev/features/research) —
  keyless academic search API (~43M abstracts: PubMed, bioRxiv, medRxiv,
  arXiv) with paper search, passage reading, and citation-network
  navigation; no API key to start (advertised ~1000 credits/month
  keyless). A ready-made frontier for Pillar 2's open-web tier: standing
  queries → paper search → proposal tray, without running a crawler of
  our own. Research-flavored notebooks get real primary sources.
- **Atlas for Mac** (atlasformac.com) — the scale-UX bar for Pillar 1:
  "zoom out and see a thousand things at once, zoom in and look at one
  properly." Its grid/canvas/infinity triad and connected-folders-at-
  library-speed are the right instincts for a 5k-source panel: density
  zoom on the existing gallery, spatial grouping as a curation surface.
- **OpenKnowledge** (openknowledge.ai) — markdown + git knowledge base
  with hierarchical RAG and native MCP, built for humans and agents in
  one loop. Alchemy already imports/exports OKF; the Pillar 3 wiki view
  should emit OKF-compatible markdown pages so the generated wiki is
  portable and agent-editable rather than a bespoke render.

## Design

### Pillar 1 — UX at scale (the prerequisite)

- **Virtualize the Sources list** (windowed rendering); target 10k rows.
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
- **Consent tiers**: local files and already-subscribed feeds import
  automatically; anything that fetches a *new* network origin lands in a
  proposal tray ("Found 6 related pages — add?") rather than importing
  silently. The collapse guard applies to every unattended fetch.
- Runs inside night shift's budget dial; nothing new to configure.

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

1. Pillar 1 (UX) — pure frontend, unblocks everything, no model cost.
2. Pillar 2 gap detection + frontier from *existing* sources (no new
   origins), proposal tray.
3. Pillar 3 cluster + retirement passes on night shift.
4. Wiki view; open-web frontier behind explicit per-notebook opt-in.

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

- Does the proposal tray live in the Sources panel or the Brief?
- Should standing queries be visible/editable (a "what this notebook is
  hungry for" list), or stay implicit from traces?
- Frontier ranking: embedder-only, or is a Small-role rerank worth it?
