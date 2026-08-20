# RFC: Per-note export

Notes leave Alchemy in the format that matches their shape. One backend path
serves both the note card's context menu and agents (MCP), per the house rule
that user-facing features are agent-reachable.

## Format matching

The export offered follows the note's kind — a spreadsheet for a table, a
poster image for an infographic — not a one-size-fits-all dump.

| Note shape                          | Format | How                                                                 |
| ----------------------------------- | ------ | ------------------------------------------------------------------- |
| Infographic                         | .png   | Hidden export window renders the print sheet → PDF → pdfium raster  |
| Audio Overview (episode exists)     | .m4a   | Copy the synthesized episode (see "Audio format" below)             |
| Data table (or one-table markdown)  | .xlsx  | Markdown tables → `rust_xlsxwriter` worksheets                      |
| Everything else (prose markdown)    | .docx  | Markdown → `docx-rs` (headings, lists, tables, inline styles)       |

## Surfaces

- **UI**: the note row menu in the Studio panel gains one or two `Export…`
  items matched to the kind. Destination comes from the native save dialog
  (same `@tauri-apps/plugin-dialog` pattern as the audio player and PDF
  export); the saved file is revealed in Finder.
- **MCP**: `export_note { note_id, format, path? }` writes the file and
  returns the absolute path. `path` optional — defaults to
  `~/Downloads/<title>.<ext>`.

Both go through one command (`export::export_note`), so the UI and agents can
never drift.

## PNG (infographic)

There is no DOM rasterizer in WKWebView we can trust (`foreignObject` canvases
are tainted), but the app already has two proven pieces: the infographic's
print sheet (fixed-ink portrait layout, `print_webview` → silent
`NSPrintSaveJob` PDF) and pdfium page rendering (`pdf::render_page`). PNG
export composes them:

1. The backend opens a small `win-export-*` window booted onto the note with
   an export flag carrying a temp PDF path.
2. The window renders the print sheet and silently prints itself to that PDF.
3. The backend rasterizes every page at 1600 px wide, stitches them
   vertically, writes the PNG, and closes the window.

Fully local, pixel-identical to the PDF the poster's own button produces.

## Audio format: m4a, not mp3

The episode is already an AAC `.m4a` (Kokoro WAVs stitched and encoded by
`afconvert`, tts.rs). macOS ships no MP3 encoder — `afconvert` cannot write
one — and the Rust options are LAME bindings (LGPL C, same class of licensing
we deliberately avoided with espeak). `.m4a` plays everywhere the user will
paste it, so export copies the episode as-is.

## Deferred: pptx

No maintained Rust crate writes PowerPoint files (the existing candidates are
abandoned or read-only). Slide decks keep their PDF export (paginated print
path), which every deck viewer imports. Revisit if a real crate appears.

## Dependencies

- `pulldown-cmark` (no default features) — one CommonMark+tables parse feeds
  both writers.
- `docx-rs` (no default features) — pure-Rust .docx writer; tiny tree, no
  image feature since notes are text.
- `rust_xlsxwriter` — the maintained xlsx writer (same author as Python's
  XlsxWriter).
