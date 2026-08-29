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
3. **Answer mode** — the palette body becomes a lightweight chat: each
   question sits above its answer, which streams in with **notebook chips**
   for every notebook it drew from and inline citations.
   Esc returns to results (query preserved); Enter on a citation jumps to
   it. A follow-up input at the bottom continues the thread.

   **A palette ask is a Home conversation** (see the persistence note under
   Non-goals). Asking from the launcher opens a fresh thread — a question
   typed there is a new subject, not a follow-up to whatever Home had open —
   and follow-ups continue it, so the palette's history and a thread's
   history are the same thing. The turns persist as they settle: the answer
   survives the palette closing (it keeps streaming into its thread, and the
   Chats card says "Answering…"), and **Open in Chat** in the palette's
   footer walks over to it. Esc stops a live answer exactly as Home's Stop
   does — cancelled, partial kept, filed under its own thread.

   One owner, one channel: the palette calls `askHome`/`stopHome` like every
   other Home surface and renders `homeRun` from the store. It has no
   `meta://` listeners and no `cancelGeneration("meta")` of its own —
   otherwise a palette ask over a live Home answer put two sets of listeners
   on one token stream and two owners on one cancel scope.
4. **From anywhere** — ⌥Space already summons the palette, so meta-chat is
   automatically the system-wide "ask my research" surface (the
   ethertext-recall gesture, answered by the corpus instead of a memory
   store).

The window stays palette-sized (no modal-in-modal); this is a glanceable
answer surface, not a second chat app — glanceable in what it *shows*, not
in what it keeps: the conversation behind the glance is the same durable
thread the Chat tab reads.

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

- ~~Persisting meta-chat threads (ephemeral; a rerun is cheap and the corpus
  moved anyway).~~ **Superseded.** A rerun is cheap; getting back to the
  conversation was impossible. Home's Chat tab keeps threads in a
  corpus-scoped `meta_turns` table — one row per turn, citations serialized
  beside it, and a thread is nothing but the turns that share a `thread_id`
  (so a conversation nobody asked into never exists). The tab lists past
  threads, back/forward lands on one, and a relaunch reopens the one that
  was on screen. **⌘K asks are threads too**: the palette mints one per ask
  session (`newHomeThread`) and asks into it, so a question asked at the
  launcher is findable an hour later instead of dying with the overlay. Agents read them over MCP with `list_home_chats` and
  `get_home_chat`. Still open: saving an answer as a note in a chosen
  notebook.

  Threads name themselves. Once a conversation's first exchange settles,
  `add_meta_turn` fires a background task that asks the **small** role for a
  name of at most five words and stores it as one more row in `meta_turns`
  — `kind = "title"`, same `thread_id` — so a thread stays exactly what it
  always was (the rows that share an id), with no second table and no column
  migration. That row is bookkeeping, not conversation: every read that
  means "the transcript" filters it out, it counts for no turn, it never
  moves `updated_at`, and deleting the thread takes it along. The list falls
  back to the opening question until a name exists and keeps the question
  beside the name either way (the row's tooltip). Naming is best-effort in
  the `spawn_retitle` mould — no small model, a refusal, or a runaway answer
  leaves the question-derived title in place and the next settled answer
  tries again; nothing here can block a chat. The finished name reaches open
  windows as `mcp://changed` with scope `homechat`.

  Turns also record which model wrote them (`meta_turns.model`, added in
  place), so a Home answer is captioned exactly as a notebook answer is.
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
