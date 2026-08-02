# RFC: Source Gallery

A visual way to explore a notebook's sources — a masonry grid of cards in the
center column, in the spirit of mymind / raindrop.io / are.na. Scraped pages
lead with the page's own image; PDFs lead with their first page; images lead
with themselves; everything else gets a quiet typographic card.

## Why

The Sources panel is a working list: dense rows, built for triage and
selection. It answers "what's here" but not "what does this collection look
like". For visual corpora — product research, car listings, clothing, design
references — recognition beats recall: the picture of the thing *is* the
fastest handle for the thing. mymind's whole product is this insight.

## What ships

### Lead images (`Source.image_url`)

- New nullable-in-spirit string column `image_url` on `sources` (default
  `""`), migrated in place with `add_string_column`. Flows to TS as
  `imageUrl` and through MCP `list_sources` automatically.
- URL ingest extracts the page's lead image from the raw HTML before
  readability strips it: `og:image` → `twitter:image` (first hit wins),
  resolved against the page URL. Stored on the source; never fetched at
  ingest time (the webview loads it lazily when the gallery renders).
- The live-DOM capture paths (`capture.rs` page capture, `clip.rs` web
  clipper) pick up `og:image` the same way via `PageMeta`.
- Refresh/reingest re-extracts and updates the stored image like it does
  the text (a refreshed page may have a new hero image).

### Backfill for existing sources

Existing URL sources predate the column. On gallery open, a fire-and-forget
`backfill_source_images(notebook_id)` command sweeps that notebook's URL
sources with an empty `image_url`, re-fetches just the HTML, and writes the
parsed image URL back — **no re-chunk, no re-embed**. Sources that yield no
image are stamped `"-"` (sentinel for "checked, none") so the sweep never
repeats them. Bounded concurrency (4); emits one `sources` refresh at the end.

### Thumbnails for local documents (`source_thumbnail`)

- `source_thumbnail(source_id)` command returns a base64 PNG:
  - `pdf` → first page via the existing `pdf::render_pdf_pages(path, 1, 480)`,
    cached at `<app-data>/thumbs/<source_id>.png` (rendered once, ever).
  - `image` → the original file bytes (the webview scales; skips the cache).
  - anything else → empty (the card falls back to typography).
- Base64 over IPC deliberately sidesteps the `asset://` WKWebView decode
  caveat documented in `ReaderPane.tsx` (`ImageView`).
- Cache eviction: the thumb file is deleted on `delete_source` and on
  refresh-driven reingest of the source.

### The gallery pane

- A center-column view like `LedgerPane` — the Sources panel header gains a
  `LayoutGrid` toggle, and the command palette gains "Browse source gallery".
  Esc returns to chat (existing reader/center rules).
- CSS-columns masonry (`columns`, `break-inside-avoid`) — natural image
  aspect ratios, no measuring, no library.
- Card anatomy (DESIGN.md: hairlines, no tonal fills, color = content only):
  - image cards: the image full-bleed on top, then a 2-line title and a
    favicon + domain caption row;
  - typographic cards: type icon + title + metadata (author or domain,
    added date) — text carries the hierarchy.
- Click opens the source in the Reader (same `openSourceViewer` path as the
  panel rows). Hover raises border/surface per the design system; cards are
  keyboard-operable via `cardButtonProps`.
- Folder-like parents (`folder | git | notion | obsidian`) are shown as
  typographic cards; their children appear as their own cards.

## Not in v1

- No screenshot fallback for pages without og:image (needs a headless
  webview pass — Watchtower territory).
- No content snippets on typographic cards (list payloads omit `content`;
  fetching full text per card is too heavy for a browse surface).
- No cross-notebook gallery. Per-notebook only, matching the panel.

## Touchpoints

Backend: `models.rs`, `db.rs` (schema/batch/decode/migrate + `set_source_image`),
`ingest.rs` (`Extracted.image_url`, `PageMeta.og_image`, meta parse),
`capture.rs`, `clip.rs`, `commands.rs` (`store_new_source`/`reingest` threading,
`source_thumbnail`, `backfill_source_images`, thumb cleanup), `lib.rs`,
`mcp/sources.rs` (tool description).
Frontend: `types.ts`, `api.ts`, `store.ts`/`storeTypes.ts` (`galleryOpen`),
`Workspace.tsx`, `GalleryPane.tsx` (new), `SourcesPanel.tsx`,
`CommandPalette.tsx`.
