# RFC: Canvas — sources arranged by hand

Status: draft, awaiting review (2026-09-15).
Origin: Reminders item "Canvas view for sources. For image based
notebooks it can be nice to arrange them on a canvas view…" plus its note
on cover images, and the bezel.gallery look (Reminders 2026-09-15: gpui-
only, but the *document* it reads is the thing to borrow). Builds on
[RFC-source-gallery.md](RFC-source-gallery.md) (lead images, thumbnails)
and [RFC-okf-live.md](RFC-okf-live.md) (the bundle).

## Summary

A fifth center mode beside Chat, Reader, Gallery and Grow: a pannable,
zoomable surface where the notebook's sources sit where the user put
them. Drag a source in from the sidebar, move it, resize it, bring it to
front, drop a text label beside it. The arrangement is a file in the
bundle — a **JSON Canvas 1.0** document, the format Obsidian's canvas
reads and writes — so it syncs like everything else, opens in Obsidian,
and survives the app.

Nothing on the canvas is a new kind of thing: a node is a source, a note,
a text label, or a group. The canvas is a view of the corpus, not a
second corpus.

## Why JSON Canvas

Obsidian's spec (jsoncanvas.org, 1.0) is small — nodes with `id`, `type`
(`text` | `file` | `link` | `group`), `x y width height`, optional
`color`; edges with `fromNode`/`toNode` and optional sides and labels —
and it is already the interchange format for hand-arranged boards.
bezel's canvas component confirmed the shape works for an app that owns
richer node kinds: it keys a renderer by `type`, keeps unknown fields
verbatim so another app's file survives a save, and models every edit
as a `Change` (Add, Move, Reparent, Detach, Remove, Update) that a filter
can refuse or rewrite. We take the format and that edit model; the
rendering is ours (WebView, not gpui).

Mapping:

| Canvas node | JSON Canvas | Alchemy field |
| --- | --- | --- |
| Source | `file`, `file: "sources/<slug>.md"` | resolves to the source by the bundle path; `alchemy.sourceId` carried for speed |
| Note | `file`, `file: "notes/<slug>.md"` | same |
| Text label | `text` | plain Markdown, not a source, not a note |
| Group | `group` with `label` | a titled frame that moves its children |
| Link (v2) | `link`, `url` | a URL that is not (yet) a source; "Add as source" on it |

The file lives at `<bundle>/canvas/<name>.canvas` (Obsidian's extension),
one notebook can have several. Frontmatter is not a thing in JSON; the
canvas's own title is the file name. Alchemy-specific fields ride under
an `alchemy` key on nodes and never break another reader.

## The surface

- **Pan and zoom** by trackpad (two-finger scroll pans, pinch zooms) and
  by buttons (−, 100%, +, Fit) in a small bar top-right; ⌘0 fits, ⌘= / ⌘−
  step. Space-drag pans, as in every design tool.
- **Nodes** render as the gallery's cards: the lead image or thumbnail
  edge to edge, title beneath, the hairline frame. A note node shows its
  first lines. A text label is just text on the surface with no frame
  until selected. A group is a titled rounded rect behind its children.
- **Drag in** from the sources sidebar or the Studio notes list (both
  already start real drags — the marquee and Finder-drag machinery) —
  drops at the pointer at the current zoom. Drag from Finder or a
  browser adds the file/URL as a source first (the existing add path),
  then places it.
- **Move, resize, front/back**: drag to move, corner handles to resize
  (images keep aspect), ⌘] / ⌘[ for z-order, arrow keys nudge. Marquee
  select, ⌘A, shift-click; multi-move.
- **Open**: double-click a source or note opens it in the Reader (the
  canvas stays in the back-stack); Enter on a text label edits it in
  place.
- **Right-click** on a node: Open, Open original, Bring to front, Send to
  back, Remove from canvas (never deletes the source), Color. The stock
  menu never shows (2026-09-15 rule).
- **Edges** are v2. Arranging is the ask; connecting is the mind map's
  job today (GraphView), and edges between file nodes would duplicate
  what citations already express.

The mode has a **Canvas** tab in `CenterModeTabs` and a store flag like
the others (`canvasOpen`, wins above Gallery). ⌘5 is free again since the
Ledger left; it goes to Canvas.

## Cover images, in bulk

The reminder's note is the half of this that pays off everywhere: a
canvas of imageless sources is a grid of grey rectangles, and so is the
Gallery. Store today: 1,121 URL sources, 173 without a lead image; every
markdown, text, code and PDF source (2,229) has none, and only PDFs get a
rendered thumbnail. Two changes, both outside the canvas:

1. **Pick harder.** URL ingest stops at `og:image` / `twitter:image`.
   Fall through to the first `<img>` in the article body over 400 px
   wide (readability keeps the article's images; the extractor sees them
   before the strip), then to the site's `apple-touch-icon`. Markdown and
   HTML sources take their first image reference; a PDF keeps its first
   page. Applies to new imports and to the existing backfill sweep, which
   already stamps `"-"` for "checked, none" — clear the sentinel once so
   the sweep runs again with the deeper pick.
2. **Fix them by hand, in bulk.** A **Cover images** sheet from the
   Gallery's ⋯ (and the Canvas's): one row per imageless source — title,
   type, and the candidate images found in its content (every `<img>`
   from the fetched HTML, every image reference in the Markdown, the first
   three PDF pages) as thumbnails; click one to set it, or paste a URL,
   or drop a file. Sources with no candidates show a "none in content"
   note and the paste/drop targets. `set_source_image` exists; the sheet
   is a loop over it, with a `source_image_candidates(source_id)` command
   that returns the candidates without re-ingesting. Agent-reachable as
   the same op on the `update_source` family.

## Data and sync

- The canvas file is a bundle file, so the OKF reconciler carries it
  between Macs and to a shared notebook like any other file (RFC-okf-
  live §5.6). Two people moving nodes at once produce a conflict copy,
  as two edits to one note do; the newer wins and the older becomes a
  `.canvas` conflict file the user can open. Fine for v1: canvases are
  arranged, not co-edited in real time.
- Notebooks not kept on disk keep their canvases in the app data dir
  under `canvases/<notebook-id>/`, exported into the bundle when the
  notebook is bound (same rule as originals).
- Deleting a source removes its node from every canvas of the notebook
  (a dangling `file` node otherwise renders as a broken card; Obsidian
  does the same removal).
- No Lance table. The canvas is not retrieved; a text label is not
  indexed. If labels turn out to carry real content, they become notes
  by an explicit "Convert to note", not by indexing the canvas.

## MCP

`list_canvases(notebook_id)`, `get_canvas(notebook_id, name)` returning
the JSON Canvas document, `set_canvas(notebook_id, name, document)`
writing it whole (validated: every `file` node resolves), and
`arrange_canvas(notebook_id, name, layout: "grid" | "by-tag" |
"by-date")` — the one machine-judgment verb, and it writes a *new*
canvas rather than rearranging the user's (the invariant: proposals when
machine judgment would change the user's work). An agent that groups a
notebook's sources by theme and lays them out is a real use; it does it
on its own board.

## Not in v1

Edges and labels on them; freehand drawing; embedding a web page live in
a node; per-canvas chat scope ("ask about what's on this board" — worth
doing, it is the @mention override over the node set, but after the
board itself is good); infinite-canvas performance work beyond ~500
nodes (a notebook of 2,400 sources is the Gallery's problem, not the
canvas's — the canvas is curated by hand).

## Phasing

1. **Cover images.** The deeper pick + the bulk sheet. Pays off in the
   Gallery on day one, and every canvas after.
2. **The board.** JSON Canvas read/write, the tab, pan/zoom, nodes for
   sources/notes/text, drag in, move/resize/z-order, right-click, open.
   Golden-file tests: an Obsidian-written `.canvas` round-trips byte-
   stable except for the nodes we touched.
3. **Groups, colors, Fit, keyboard.** The design-tool polish.
4. **MCP** verbs, including `arrange_canvas`.

## Open questions for Paul

- **Obsidian fidelity vs. our fields.** Keep `alchemy.*` fields on nodes
  (fast lookups, our colors) or stay strictly to the spec and resolve by
  path every time? Proposed: keep them; the spec says unknown fields are
  preserved, and Obsidian does preserve them.
- **Where the Canvas tab sits.** Between Gallery and Grow, or at the
  end? Proposed: after Gallery — both are "look at the sources" modes.
- **Should the canvas be the Gallery?** A gallery is a canvas with an
  automatic grid layout. Merging them (one tab, a "Grid / Free" toggle)
  is tempting and removes a mode. I have not proposed it because the
  Gallery's multi-select and bulk verbs are list-shaped and would have to
  be rebuilt on the board; worth deciding before phase 2.
