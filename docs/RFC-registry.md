# RFC: The Registry — a confirmed cast, and the documents that follow it

## Summary

A closed, user-confirmed cast of entities and threads — assets, people,
policies, providers, projects, dependencies — each a living card that
aggregates its documents, its key facts, and its dates. This is pillar 4 of
[RFC-v12-steward.md](RFC-v12-steward.md), and the first one that is
corpus-scoped rather than notebook-scoped: your questions arrive by *thing*,
and things don't respect notebook boundaries.

**Auto-attach is a literal match on a distinctive identifier, never a
similarity score.** A document joins a card without asking only when it
contains that card's VIN, policy number, or serial — otherwise it is a
proposal. That single rule is the pillar's safety story, and it is
`gate_tags`' grounding idiom (gist.rs:718-745) applied to a higher-stakes
edge.

V8 organizes by notebook and filename. The gallery made a corpus browsable by
*picture*; the Registry makes it browsable by *thing*. A filename is a handle
for a document; a card is a handle for the thing the documents are about.

## Background — the gallery already built the surface

The source gallery ([RFC-source-gallery.md](RFC-source-gallery.md)) shipped
since the Steward RFC was written, and it built most of what a card grid
needs. The Registry is its second consumer, not a new surface.

- **The card grid exists.** `GalleryPane.tsx` — the card shell class string
  (:626, :726), `CardAction` (ui.tsx:278-297, the full-card sibling button
  that lets a menu live inside a clickable card), `CardMenu` + `RowMenu`
  (:589-603), and shortest-column-first packing over JS-bucketed flex columns
  (:233-260) with the WKWebView-multicol warning that earned it (:210-217).
- **The filter bar exists, as inline JSX.** Two independent axes — type
  groups computed from what's present (:156-162) and tag chips ordered
  count-desc with an alpha tiebreak (:166-172), both `aria-pressed`, both
  falling back rather than rendering an empty grid (:163, :173-174). It has
  no component boundary, because it has only ever had one consumer. The
  Registry is the second; that is the moment to extract it.
- **Covers come for free.** `Source.imageUrl` and `source_thumbnail` already
  produce a picture for pages, PDFs, and images, with a typographic-card
  branch when there is none (`wantsSnippet`, :93-97). A card's cover is its
  primary document's cover — no new pipeline.
- **The precision-bar precedent is `gate_tags`** (gist.rs:718-745): a
  generated tag is discarded unless the document literally contains it,
  because "an invented tag is worse than no tag: it is a filter that lies."
  A wrong attachment is that failure with a bigger blast radius.
- **The storage precedent is the Ledger, and it diverged from this RFC's
  parent.** RFC-v12-steward pillar 2 promised ledger rows as a `note:`-style
  corpus species; what shipped is `T_LEDGER` (db.rs:34), one flat table, with
  the variable-cardinality part (`anchors`) as a JSON blob in a single `Utf8`
  column (db.rs:2809-2822). Columns are added with `add_string_column`
  (db.rs:254-269), never drop-and-recreate.
- **Sweep discipline is settled.** `weave.rs` clones the gist gates by name:
  `IN_FLIGHT`/`MAX_IN_FLIGHT`, `COSINE_FLOOR`, `MAX_PAIRS`, `TEXT_CAP`,
  strict parse-or-skip, and the rule that a sweep "must opt IN to acting."
  It hangs off two choke points — source insert (commands.rs:761, top-level
  only) and reingest (commands.rs:1595, on a non-empty diff).
- **Backlinks are weaker than the parent RFC assumed.** `source_backlinks`
  (commands.rs:6848) is a per-notebook content scan for URL and filename
  needles, not a link index. It cannot be the card's document list: a card
  spans notebooks, and a card is not text that contains a URL.
- **Reader primitives to reuse:** `DocRails` (ReaderPane.tsx:1035-1170)
  already generalizes docked-rail-vs-popover by measured width, and
  `DocProperties`' fact grid (:1587-1596) with `MetaEditable` (:1474-1532) is
  an editable key/value table that exists and works.
- **The Brief plugs in with one loop.** `collect()` (brief.rs:75-200) reads
  sources, notes, ledger, and source events into three ranked buckets.
- **Home is holding the slot,** in two literal comments: "Registry joins when
  its pillar exists" (HomeView.tsx:80-82, HomeSections.tsx:29-32).
- **What this supersedes:** the `key-entities` default template
  (templates.rs:135-142) — a one-shot LLM listing of people, companies, and
  products, with no persistence and no attachment. It stays a template. The
  Registry is what you get when that answer is allowed to last.

## Proposal

### 1. A card is a corpus-scoped row, not a note

RFC-v12-steward's pillar-4 seam line guesses "cards as `note:` species," on
the theory that Reader, backlinks, and ⌘F come free. They don't: notes are
notebook-scoped, backlinks are a URL scan, and a card's structure (kind,
identifiers, attachment receipts) would have to survive as parsed markdown
frontmatter. The Ledger faced the same fork and took the table. So does this.

One new Lance table, `T_REGISTRY` — the one this pillar is allotted by
RFC-v12-steward's house rules:

- `RegistryCard { id, kind, name, identifiers, facts, attachments,
  created_at, updated_at }`
- **No `notebook_id`.** Cards are the first corpus-scoped entity besides
  notebooks themselves. A card's "home" notebook is *derived* from where its
  documents live, never stored — the Ducati card holds the manual in
  Vehicles and the ferry booking in Japan 2027, and neither owns it.
- `kind` ∈ `asset | person | policy | provider | project | dependency`, a
  vocabulary in `commands/registry.rs` beside `LEDGER_KINDS` (ledger.rs:10),
  validated by membership only. Kind is immutable after creation, like the
  Ledger's.
- `identifiers` — space-separated normalized tokens through the existing
  `normalize_tags` (commands.rs:1659-1672): the VIN, the policy number, the
  serial, the model number. This is the auto-attach key and nothing else.
- `facts` — a JSON blob of ordered `{label, value}` pairs. The card's key
  facts, rendered by the `DocProperties` grid and edited by `MetaEditable`.
- `attachments` — a JSON blob of
  `{ source_id, notebook_id, status, matched, at }`, the `LedgerAnchor`
  precedent exactly (db.rs:2809-2822): variable cardinality in one `Utf8`
  column, no join table.
  - `status` ∈ `confirmed | proposed | rejected`. **`rejected` is kept, not
    deleted** — it is the refusal memory that stops a sweep re-proposing the
    same pair forever, the `gist.rs` idiom.
  - `matched` is the receipt: the identifier string that matched, or
    `"name"`, or `"manual"`. It renders verbatim in the UI. A machine
    that attaches without showing its reason is a machine you stop trusting
    on the first mistake.

Cards have no lifecycle and no status. Attachments do. A card is a thing;
things don't get superseded, their documents do.

**Naming:** "registry" already means `rag::ARTIFACT_KINDS` in this codebase
("one registry, every surface" — Reports.tsx:59). The module header says so
once, the way RFC-night-shift disambiguated "paused."

### 2. Attach: identifiers act, everything else proposes

Two paths in, and only one of them writes without asking.

1. **Identifier match — auto-attach.** On arrival, a source's text is scanned
   for every card's identifiers. A hit attaches at `confirmed` with
   `matched` set to the identifier. The gate is `haystack.contains(...)` on
   normalized text — the same shape as `gate_tags`, for the same reason.
   Identifiers must be ≥6 characters and contain at least one digit; a
   4-character alphabetic "serial" is a false-positive machine. Identifiers
   are user-entered or user-confirmed, **never inferred and written** — the
   Clerk's header extraction may *propose* one, and a proposed identifier
   attaches nothing until it is confirmed.
2. **Name match — propose only.** A card's name is ordinary language, so a
   document containing it is a candidate and nothing more: it queues a
   `proposed` attachment with `matched: "name"`. Names must be ≥4
   characters, and a card stops accruing past `MAX_PENDING_PER_CARD` — a
   queue nobody can face is the same as no queue.

   *This is where the built version diverged from the first draft, which
   called for a cosine pass against embedded card summaries with the
   `weave.rs` gates.* Cosine needs card vectors — a new column or a
   route-table reuse — plus a Small-role call per arrival, and it buys a
   **worse** signal than the name: entity resolution keys on distinctive
   strings, which is what a name is. Dropping it makes the whole sweep
   literal string work with no model call, so it can run on every arrival
   including folder children, with nothing to budget and nothing to gate.
   The thesis is unchanged and in fact sharper — graded literal matching,
   distinctive strings act, ordinary ones propose. Cosine can come back if
   name matching proves too narrow in practice.

Both hang off the choke points the Weave already uses — source insert
(commands.rs:761) and reingest (commands.rs:1595). Registry matching and the
Weave become siblings at one seam, both fire-and-forget in the `spawn_sweep`
shape, both silent on failure. Nothing is owed; the next change retries. Two
differences from the Weave, both because matching costs nothing: it does
**not** skip folder children (a folder of scanned documents is exactly where
auto-filing earns its keep), and on reingest it re-scans the whole updated
document rather than the diff, so a card minted after a source landed still
picks it up. Already-attached pairs skip in any status, so re-running is
free and idempotent — which is what makes `rematch_registry` safe to fire
whenever a card's identifiers change.

The third path is the fastest and seeds the cast: **"File under a card…"**
in the gallery's card menu and the sources panel's `RowMenu`, opening a
picker that also mints a card from what you type. Most of a household's cast
gets built this way in an afternoon, and every sweep after that is a bonus.

### 3. The suggester: the cast populates itself, by proposing

An empty registry is an opt-in gate, and this project doesn't ship those —
intelligent behavior is default-ON and the toggle is cost control, not a
safety rite. So the cast fills itself. **It proposes; it never mints.**

Once per notebook per app run, after the gist sweep has settled (so the
material exists), one `Small` call reads up to 40 of that notebook's gists —
one call for the whole notebook, not one per source — and names the things
that recur. Each proposal lands as a card with `origin: "auto"`, which does
nothing at all: it is not in the grid, it holds no documents, and it appears
only in a **Suggested** strip above the cast, with **Keep** and a dismiss.

- Gated exactly like `gate_tags`: the kind must be in the vocabulary, the
  name must be 4–60 characters, and **the name must appear verbatim in the
  material**. An invented card is worse than no card — it is a thing in your
  registry that was never in your life.
- Anything already in the cast is skipped, in any origin. Dismissed cards
  are kept as `origin: "dismissed"` and never render: the row *is* the
  refusal memory, so a guess you turned down never comes back.
- **Keep** flips origin to "" and kicks off a corpus-wide rematch in the
  background, so a card you accept immediately acquires its documents rather
  than waiting for the next import.

App-run scope for the once-per-notebook marker is deliberate (the gallery's
image backfill uses the same): the pass converges immediately so the gist
sweep's loop can terminate, and a new launch reconsiders a corpus that has
since grown.

This is the `ensure_tags` bargain at card scale — fill what is empty, never
touch what a human has curated. It does not weaken the closed cast: nothing
enters the registry proper without a human click. It only removes the blank
page.

### 4. The grid is the gallery's grid, on Home

**Extract first.** Before a third consumer, lift out of `GalleryPane.tsx`:
a `FilterBar` (two axes, group buttons + chips, count-desc chip ordering,
fallback-not-empty behavior) and the card shell — `CardAction`, the hover
border/surface classes, `CardMenu`. Two call sites duplicate the shell string
today (GalleryPane.tsx:626, HomeView.tsx:515); a third makes it a primitive.

The Registry grid is that grid with the axes re-aimed: **kind groups**
(asset / person / policy / …, computed from what exists) on the left of the
separator, **notebook chips** on the right, ordered count-desc with an alpha
tiebreak. Uniform cards, so `HomeView.tsx`'s
`grid-cols-[repeat(auto-fill,minmax(220px,1fr))]` rather than the masonry
packer — the packer earns its complexity on natural image ratios, and cards
don't have one.

A card's cover is its primary confirmed document's `imageUrl` or thumbnail;
with none, the typographic branch the gallery already has. A card with
pending proposals carries the small `bg-primary` dot the notebook cards use
for unread reports (HomeView.tsx:541) — the same signal, the same pixel.

**Where it lives: the Home center column, behind a Notebooks | Registry
switch in the heading row** (HomeView.tsx:~435), not a fifth notebook center
mode. Two reasons. Cards are corpus-scoped, and putting a corpus-wide cast
behind a notebook is a lie about what it is. And the center-mode union is
hardcoded in four places (store.ts:1658-1666, :1683-1689, :1740-1746,
:374-377) plus every mode-setter's obligation to clear the other flags — a
chore worth paying for a notebook view and not for this one. The Staff and
Brief sidebars are untouched; this replaces the notebook grid in place, and
the ask box stays across both.

Clicking a card opens the **card detail** in the same column, with a back
control: the fact grid (`DocProperties` + `MetaEditable`, editable in place),
the document thread, and this card's proposal queue. Project cards order
their thread by document date — quote → contract → permit → invoices —
everything else newest-first. Cards are not notes, so they do not open in the
Reader and ⌘F does not come free; that is the honest cost of the table, and
it is small.

### 5. The card in the notebook: the right rail

`CardRail` renders **above `AmbientRail` inside the existing right rail**,
in both its docked and popover forms — a sibling in that column rather than
a third rail with its own toggle and fit math. It inherits `DocRails`'
measured docked-vs-popover logic (ReaderPane.tsx:1035-1170) for free, and it
returns null when the document is filed nowhere, so it costs no space in the
common case. `excludeSourceId` already carried the id it needs.

The rail shows each card's name and kind, its first key facts, and the
attachment receipt — the identifier that matched, "name matched", or "filed
by hand" — with **Confirm** and **Not this** on anything still proposed.

That last part is the point: a proposal resolves where its evidence is on
screen. The confirm queue on Home is the sweep-up, not the primary surface.

Boundary microcopy, per RFC-v12-steward's UI rule: "Attaching only files this
document under the card. It changes nothing in the document."

### 6. Reach: MCP, the Brief, OKF

- **MCP** — `src-tauri/src/mcp/registry.rs`, the three mechanical steps
  (`mod` in the alphabetized block, `#[tool_router(router = registry_router)]`,
  `+ Self::registry_router()` in the summed `#[tool_handler]`,
  mcp/mod.rs:241-247). Verb-shaped like the Ledger's: `list_registry`,
  `get_registry_card`, `add_registry_card`, `update_registry_card`,
  `attach_source`, `set_attachment_status`, `delete_registry_card`. Every
  mutation calls `self.changed("registry", None)`, with a matching
  `registryBump` case in the store's listener (store.ts:459-460).
  Validation imports from `commands::registry` — the vocabulary lives in one
  place, as the Ledger's does.
- **The Brief** — one more loop in `collect()` (brief.rs:75-200): pending
  proposals rank as *needs a decision* (they are the one thing here that
  waits on a human); cards whose documents changed rank as *changed*. The
  collector grows, the shape doesn't.
- **OKF** — `export_notebook_okf` (commands.rs:7349) exports cards with at
  least one confirmed attachment in that notebook, as documents carrying
  their facts and their in-notebook thread. A card that spans notebooks
  travels partially, and the export says so rather than pretending the bundle
  is the whole card.

### 7. Reprise, staged

Reprise — opening a notebook kin to dormant ones triggers a carry-forward
brief of what still holds and what went stale — is in the same pillar but not
in v1. It reads `router.rs`'s per-notebook summary vectors for kinship and
`global_meta_route` (commands.rs:8177) scoped to kin notebooks, and its
verdicts (*still holds / went stale / left open*) are **Ledger** rows, not
Registry rows. It needs a cast worth reprising and a ledger with history.
Cards ship first; Reprise lands on top of both, and its seam is a brief
kind with a notebook-open trigger, not new storage.

## Non-goals

- **No open-world entity extraction that MINTS.** The suggester (§3) reads
  the corpus and proposes, which is a deliberate revision of this RFC's first
  draft — that draft said "nothing mints a card unasked" and meant it as a
  ban on extraction, when the thing actually worth banning is extraction that
  *lands in the cast*. Nothing enters the registry without a human click; the
  `key-entities` template (templates.rs:135-142) stays exactly what it is.
  RFC-v12-steward pillar 4 cut open-world dossiers and was right to: "one
  authority" means the sweep proposes and you decide, always.
- **No card-to-card graph.** No edges between cards, no graph view, no
  evidence-board canvas (cut by name, RFC-v12-steward non-goals). A card
  aggregates documents. That's the whole data model.
- **No automatic merge or dedupe.** Two cards for one dishwasher is a
  one-click fix for a human and a poisoning risk for a sweep. Merge is a
  manual verb; nothing guesses that two cards are the same thing.
- **No connectors, no credentials, no logins.** The Argos boundary
  (RFC-v12-steward pillar 4, "Prior art, in-house") stands: Argos does
  accounts and scraping, Alchemy is documents-in. An Argos export lands here
  as sources like anything else.
- **Not an asset register or a CRM.** No custody fields, no depreciation, no
  contact history, no reminders of its own — dated facts become Tickler
  obligations through the Ledger, and the Tickler proposes to Apple Reminders
  and stops.
- **Background work off = no proposals.** The master switch governs the
  matching sweep like every other background family. Manual attach and the
  whole grid keep working — off means today's behavior, not a dead surface.

## Risks

- **A wrong auto-attach is the failure that matters.** It files a document
  under the wrong thing and then answers questions from it. Mitigation is the
  whole of §2: literal identifier match only, ≥6 characters with a digit,
  user-owned identifiers, and a visible `matched` receipt on every attachment
  so a mistake is legible at a glance instead of silently structural.
- **Identifier collision.** A short or wordlike serial matches unrelated
  documents. The length-and-digit floor handles the common case; a card whose
  identifier produces an implausible burst of attachments in one sweep stops
  auto-attaching and proposes instead.
- **Proposal fatigue.** A cast of forty cards against a busy import could
  queue hundreds. Per-card and per-sweep caps, `rejected` remembered forever,
  and proposals surfacing in the reader rail where they're cheap to resolve.
- **A second grid to maintain.** Real, and the reason §3 leads with the
  extraction. If the Registry grid and the gallery grid drift, the extraction
  didn't go far enough.

## Verification

- Create an asset card with a VIN, import a PDF containing that VIN: it
  attaches at `confirmed`, and the reader rail shows the VIN as the receipt.
- Import a document naming the vehicle but *without* the VIN: it appears as
  a proposal, never as a confirmed attachment; **Not this** in the rail marks
  it `rejected`, and re-importing the same document does not re-propose it.
- Identifier floor: a card with identifier `ab12` attaches nothing, and a
  card named "Cat" proposes nothing.
- Adding a VIN to a card that already had documents re-files them on the
  spot (`rematch_registry`), not on the next import.
- Cross-notebook: one card holds documents from two notebooks; the grid shows
  both notebook chips, and neither notebook claims the card.
- Home: Notebooks | Registry switches in place, kind groups and notebook
  chips filter, a card with a pending proposal shows the dot, and the ask box
  is present in both.
- Brief: a pending proposal appears in the next brief's needs-a-decision
  section.
- Agent path: `list_registry` and `attach_source` over MCP with no window
  open; a card created by an agent appears in the grid on the next bump.
- Background work off: no proposals and no suggestions accrue; manual attach
  and hand-made cards still work.
- Suggester: a notebook with gists offers a short Suggested strip; **Keep**
  moves a card into the grid and it gains its documents; dismissing removes
  it and it is not re-suggested on the next launch.
- Gates: `cargo fmt -- --check && cargo clippy -- -D warnings && cargo test`,
  plus the frontend typecheck (`pnpm build`).
