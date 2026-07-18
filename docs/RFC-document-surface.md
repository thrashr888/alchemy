# RFC: The reader — closing the read gap, then ambient connections

> Rev 2 (2026-07-17): reading is NOT fine. v1 of this RFC underweighted it.
> Navigation is open-modal → close → open-next ceremony; sources render as
> flattened plain text regardless of origin (markdown, docx, HTML); there is
> no wiki-like jumping between documents. The sources/chat/studio three-pane
> is an AAA-grade UX; the modals stacked on top of it are not. This revision
> puts the reader first.
>
> Rev 3 (2026-07-18): phases 1-4 are SHIPPED (commits effcd56 → 0ebcd02):
> reader pane, article-markdown extraction + document graph, ambient rail
> (editing), seamless edit-in-place with autosave + tables, plus the live
> web view. Per-section status notes below; the Remaining section at the
> end is the honest delta.

## Research: what the best current tools do

Inspiration pulled 2026-07-16 into the Alchemy Development notebook
(sources: OpenKnowledge, Cabinet, Hubble.md).

- **OpenKnowledge** (openknowledge.ai) — "AI-native markdown editor."
  Notion-grade visual editing that is *just markdown under the hood*, with
  an explicit **Visual ⇄ Markdown toggle**; block components (callout,
  accordion, tabs, Mermaid, embeds); a **title + tags metadata header**; a
  footer word/char/**token** count; an **agent edit timeline**
  ("claude-code · 18 min ago"). Agents are visible co-authors.
- **Cabinet** (runcabinet.com) — markdown on disk, git-backed, BYO-model.
  The document is the product, not the app chrome.
- **Hubble.md** (hubble.md) — file-path affordances (copy link/path,
  rename) and **agent-built live views** over the corpus.
- **Reading-first products** (the bar for this revision): Obsidian's
  persistent workspace with wikilinks, backlinks, and hover previews;
  Notion's peek-then-page navigation with breadcrumbs and back/forward;
  Readwise Reader's j/k keyboard flow through a reading queue; Wikipedia's
  hover page-previews; NotebookLM's in-panel source view (the source opens
  *inside* the layout — never a modal — with a guide up top).

## The read gap (current state)

- **Navigation**: every document is a modal. Moving between two sources
  means open → close → open; there is no prev/next, no history, no
  breadcrumb, and the reading position of the last document is lost.
- **Rendering**: `readable_text` extracts URLs/HTML with
  `TextMode::Formatted` — plain text; headings, bold, lists, tables, and
  **every link** are destroyed at ingest. Markdown files keep their source
  markdown but the viewer renders `whitespace-pre-wrap` plain text anyway.
  docx flattens to text. Nothing reads like its origin.
- **No document graph**: links died at extraction, so there is nothing to
  jump through; no backlinks ("what cites this?"), no hover previews.
- **Quality bar**: the three-pane workspace is a faithful copy of an AAA
  product; the reading/writing layer on top of it is not at that level.

## Design

### A. The reader pane (kill the modals)

The center column becomes a two-mode stage: **Chat ⇄ Reader**.

- Clicking a source or note opens it in the Reader **in place** — the
  sources rail stays put and becomes the navigator. Click another row: the
  document swaps instantly. Esc (or the Chat tab) returns to chat.
- Browser-grade navigation: per-notebook **history with back/forward**
  (⌘[ / ⌘]), **j/k / ↑↓ prev-next** through the rail order, and a
  breadcrumb (`notebook › document`). Scroll position is remembered per
  document within the session.
- The chat is never far: select-text-to-ask works in the reader and flips
  to chat with the passage attached (existing behavior, new home).
- **Every note kind renders in the pane** — markdown notes, mind maps,
  slide decks, quizzes, flashcards, audio overviews all use their native
  renderers inside the reader (Present mode still goes fullscreen). The
  modals retire entirely; the pop-out note window stays for multi-window
  work. SourceViewer's find/highlight/citation-jump logic moves into the
  reader.

### B. Faithful rendering per origin

Render documents like the thing they came from:

- **Markdown sources**: render through the existing `<Markdown>` component
  — free today, the content is already markdown.
- **URLs / HTML**: upgrade extraction to produce **article markdown**
  instead of flattened text — `dom_smoothie` already isolates the article
  node; convert its HTML to markdown (htmd or equivalent) so headings,
  emphasis, lists, tables, and **links survive**. Heading-aware chunking
  gets *better* under this change. Existing sources upgrade on Refresh; no
  migration.
- **docx**: map core styles at extraction (Heading 1-3, bold/italic,
  lists, tables) → markdown.
- **CSV/XLSX**: already row-shaped — render as real tables.
- **PDF**: text view now; pdfium page rendering (side-by-side pages) is a
  later phase — the dylib already ships.
- **Live web view** (read-it-later pattern, SHIPPED): URL sources get a
  **Cached ⇄ Live** toggle — Cached is the extracted article (fast,
  private, offline, default); Live embeds the actual page *inside the
  reader pane* via a Tauri child webview (`Window::add_child` behind the
  `unstable` feature) positioned over the pane and resized with it, so
  dashboards and JS-heavy pages never bounce to an external browser. The
  child webview gets no IPC access — its label matches no capability
  pattern — so it is a plain browser surface outside the app's boundary.
  Bounds track the reader body via ResizeObserver; a MutationObserver
  hides the child while any `role="dialog"` overlay is up (a native view
  would paint over the DOM); it closes on doc switch or leaving Live.
  Known caveats: once the user clicks into the page the child owns key
  focus, so app shortcuts pause until they click app chrome again
  (standard embedded-browser behavior; we hand focus back on open), and
  the tauri-browser debug bridge cannot enumerate windows while a child
  webview exists (`get_webview_window` returns nothing for multi-webview
  windows — needs a fix in tauri-browser itself).
- Find-in-source and citation-highlight walk the **rendered** DOM (the
  passage-locate logic keys on text content, which survives rendering);
  plain-text view stays available as a fallback toggle.

### C. The document graph (wiki jumping)

- **In-corpus links**: a rendered link whose URL matches another source
  opens *that source in the reader* (history records the hop). External
  links open the browser as today.
- **Hover previews**: in-corpus links show a Wikipedia-style preview card
  (title, favicon, first lines) on hover.
- **Backlinks**: SHIPPED as a per-open scan (`source_backlinks` greps the
  notebook's contents for the source's URL / filename) instead of an
  ingest-time column — no schema change, instant at notebook scale. The
  reader footer shows "← linked from N", each entry jumpable. Revisit the
  column only if notebooks grow past hundreds of sources.
- Later: linkify note mentions of source titles; a notebook-level graph
  view once the link data exists.

### D. Ambient connections rail (reminder P3)

A quiet right-hand rail inside the reader/editor surfaces the top 2-3
related passages for **where you are**:

- Editing: 700ms-debounced embed of the current paragraph → hybrid
  `search_chunks` against the notebook (excluding the active note); passage
  cards show source title + snippet.
- Reading: the visible section drives the same query — long documents
  cross-reference the rest of the corpus while you scroll.
- Click opens the passage in the reader (with highlight); an insert button
  drops a reference at the cursor when editing. Never demands attention.
- New `related_passages` command wrapping the existing embedder +
  `db.search_chunks`; cancellable, cached per paragraph hash. Results may
  include notes (badged), not just sources.

> Shipped for WRITING: 800ms debounce on the paragraph being edited
> (consecutive-state diff), sources ranked ahead of notes, self excluded,
> floating over the right gutter when the pane is ≥1060px. Still open:
> the READING variant (visible-section queries while scrolling), the
> insert-reference-at-cursor button, and per-paragraph result caching.

### E. Document chrome (match the AAA panel language)

Header: inline-editable title (notes), origin badge + favicon, tags, copy
`alchemy://` deep link, Refresh/Sync, Open original. Rail: TOC extracted
from headings, scroll-synced, on long documents. Footer: word · char ·
~token count (chars/4, honestly labeled). All styled in the existing panel
idiom — same 11px uppercase labels, same borders, same quiet grays.

> Shipped: inline title, origin actions (open original / Finder /
> sync), single-line counts footer with the token estimate, responsive
> toolbar with overflow. Still open: favicon in the header, tags, copy
> `alchemy://` deep link, and the scroll-synced TOC rail.

### F. Editing upgrades (after the reader lands)

Edit-in-place on the reader surface (no Save/Cancel modal): TipTap grows
tables, task lists, images, callouts; **Visual ⇄ Markdown toggle**;
autosave on idle. The ambient rail stays up while writing.

> Shipped (first pass): prose notes ARE the editor — the reading surface
> is a bare TipTap over the pane (no boxes, inline transparent title,
> reading-width column, reading prefs honored), autosaving 1.2s after the
> last keystroke with a Saved whisper in the counts footer, flushing on
> doc switch. Save/Cancel are gone for prose; artifact kinds keep their
> raw-markdown form behind the toolbar pencil. The ambient rail floats
> over the right gutter (translucent, below the title) and only
> materializes when the pane is wide enough (≥1060px) that it cannot
> overlap the text column. Tables are in (TableKit: GFM tables round-trip
> losslessly, contextual row/column controls in the toolbar); links follow
> on plain click through the shared doc-link router (⌘-click to edit);
> generated reports are read-only records with deliberate editing behind
> the pencil. Still open: task lists, callouts, images, and the
> Visual ⇄ Markdown toggle.

## Phasing

1. ✅ **Reader pane** (effcd56): Chat ⇄ Reader in the window toolbar,
   rail-click routing, history + j/k, all note kinds in-pane, modals gone.
2. ✅ **Extraction upgrade** (fe307c9): URL/HTML → article markdown with
   links; in-corpus link resolution + hover previews + backlinks (scan,
   not column). docx style mapping did NOT ship — see Remaining.
3. ✅ **Ambient rail** (f95e52c): editing variant; reading variant open.
4. ✅ **Editing upgrades** (0ebcd02): seamless edit-in-place, autosave
   with real dirty-checking, tables, clickable links, read-only reports.
   Bonus: ✅ live web view (e8927c8).
5. Later: PDF page view, agent edit timeline, agent-built live views,
   Mermaid blocks, graph view.

## Remaining (the honest delta, roughly in value order)

- **Reading-mode ambient rail** — the visible section drives
  `related_passages` while scrolling long documents. Cheap: the command
  and the rail both exist; needs a scroll-position → section mapper.
- **TOC rail + per-document scroll memory** — the two reading-comfort
  items from A/E that didn't ship; both frontend-only.
- **Visual ⇄ Markdown toggle** — the agent-era affordance from the
  OpenKnowledge research; cheap (textarea over the same markdown).
- **docx style mapping** — headings/bold/lists/tables → markdown at
  extraction; the last flattened-origin format that matters.
- **Find/highlight on the RENDERED view** — today an active find or
  citation highlight drops markdown sources to the plain-text view
  (exact-match honesty); walking the rendered DOM keeps typography.
- **Ambient insert-reference button** — drop a link at the cursor.
- Header extras: favicon, tags, copy `alchemy://` deep link.

## Open questions — resolved

- Reader replaces chat in the same center column, two tabs in the window
  toolbar (HIG: visible-but-disabled before first use). ✓
- Note editing shipped WITH the reader as a plain form first, then went
  seamless in phase 4 — shipping the interim editor twice was fine.
- Old URL sources upgrade silently on refresh/report runs. ✓
