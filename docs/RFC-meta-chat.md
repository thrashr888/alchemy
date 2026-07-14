# RFC: Meta-chat — ask questions across the entire corpus

## Problem

Alchemy answers questions *inside* a notebook, but users hold questions that
span the library: "which notebook did I save the SNDK stock data in?",
"what projects is Tiffany helping with?", "have I researched this before?"
Today the only cross-notebook surface is ⌘K's structured search — good at
finding a title, mute on questions. The user has to guess the notebook
first, which is exactly backwards: the question is how you find the
notebook.

## UX — the Raycast pattern, in our ⌘K

Raycast's launcher answers this well: structured results stay primary, an
"Ask AI" affordance is always one keystroke away, and choosing it flips the
same window into a chat view you can Esc back out of. We reuse our ⌘K
palette identically:

1. **Palette, unchanged** — typing shows today's structured hits (sources,
   notes, content passages).
2. **A persistent last row**: `✦ Ask across all notebooks: "<query>"` —
   reachable with **Tab**, or Enter when no result is selected. Shown for
   question-shaped queries and whenever structured results are thin.
3. **Answer mode** — the palette body becomes a lightweight chat: the
   question pins to the top, the answer streams below, with **notebook
   chips** for every notebook the answer drew from and inline citations.
   Esc returns to results (query preserved); Enter on a citation jumps to
   it. A follow-up input at the bottom continues the thread.
4. **From anywhere** — ⌥Space already summons the palette, so meta-chat is
   automatically the system-wide "ask my research" surface (the
   ethertext-recall gesture, answered by the corpus instead of a memory
   store).

The window stays palette-sized (no modal-in-modal); this is a glanceable
answer surface, not a second chat app.

## Retrieval and answering

All chunks already live in ONE LanceDB table with a `notebook_id` column —
cross-notebook retrieval is the per-notebook query minus the filter:

- `db.search_chunks_all(query_vec, query_text, k)`: the existing hybrid
  (vector + BM25, rank-fused) `search_chunks` generalized to take an
  optional notebook filter. `search_chunks_fts_all` already proves the
  shape.
- New command **`ask_everything(question)`**: embed the question → hybrid
  retrieve top ~16 passages corpus-wide → prompt the chat model with each
  passage tagged `[notebook: <name> · source: <title>]` → stream tokens over
  the existing `chat://token`-style events. The model is instructed to name
  notebooks explicitly ("The SNDK watchlist data is in **Stocks: Indexes**
  inside *Alchemy Development*").
- Metadata-shaped questions ("which notebook…", "where did I…") are mostly
  answered by retrieval alone — the citations ARE the answer; the model
  narrates. No special-casing needed in v1.
- Also search note titles/bodies (notes are often the answer — reports,
  briefs) by embedding notes alongside chunks or falling back to the FTS
  pass `search_everything` already runs. v1: merge `search_chunks_all`
  passages with the note-FTS hits before prompting.

## Citations that navigate

The answer's citations carry `notebookId` + (`sourceId` | `noteId`) +
snippet. Clicking routes through `handleIntegrationUrl` — the same
alchemy:// router deep links use — so a citation click = select notebook,
open source viewer at the passage (or note card). Nothing new to build; the
router shipped in v0.13.0.

## Agent parity

Expose the same capability over MCP as **`ask_everything`** (or extend
`search` with `notebook_id: null`) so agents get corpus-wide grounding too.
Agents mostly want the raw passages, not our synthesized answer — so the
MCP tool returns passages + notebook names, mirroring `search`.

## Non-goals (v1)

- Persisting meta-chat threads (ephemeral; a rerun is cheap and the corpus
  moved anyway). Revisit if users ask to save answers as notes — likely as
  a "Save as note" button that writes to a chosen notebook.
- A separate window or menu-bar popover. ⌥Space + palette is the surface.
- Source-selection scoping (all notebooks always; per-notebook chat already
  covers the scoped case).

## Phasing

1. `search_chunks_all` (generalize the notebook filter) + `ask_everything`
   command with streaming + tests.
2. Palette: Ask row, answer mode, notebook chips, citation routing,
   follow-ups.
3. MCP `ask_everything` passages tool.
4. Later: "Save answer as note", question history, Spotlight-style recent
   questions in the empty palette.

## Open questions

- Model budget: corpus-wide context can pull passages from a dozen
  notebooks; cap at ~16 passages and let follow-ups narrow, or scale k by
  gateway vs local model (`is_gateway()` already exists for this)?
- Should the Ask row appear for every query, or only question-shaped ones
  (contains a question word / ends in "?") plus thin-result queries?
  Leaning: always show it, dimmed until Tab — discoverability beats
  cleverness.
- Notebook chips: click = filter the answer's citations, or jump to the
  notebook? Leaning: jump; filtering is a v2 refinement.
