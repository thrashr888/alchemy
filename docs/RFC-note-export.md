# RFC: Per-note export

Notes leave Alchemy in the format that matches their shape. One backend path
serves both the note card's context menu and agents (MCP), per the house rule
that user-facing features are agent-reachable.

## Format matching

Each kind offers a kind-true primary format plus a PDF of the note's own
render (audio stays audio — a PDF of a podcast is a transcript, and the
script is already in the note).

| Note kind                                          | Primary | Also | How                                                                |
| -------------------------------------------------- | ------- | ---- | ------------------------------------------------------------------ |
| infographic                                        | .png    | .pdf | Export window renders the poster print sheet → PDF → pdfium raster |
| mind_map                                           | .png    | .pdf | Same pipeline; fixed-ink SVG scaled to one page, rasterized wide   |
| slide_deck                                         | .pptx   | .pdf | Hand-rolled OOXML writer (below); PDF prints the 16:9 slide pages  |
| flashcards                                         | .pptx   | .pdf | Q/A slide pairs (below); PDF prints the study sheet                |
| data_table (or one-table markdown)                 | .xlsx   | .pdf | Markdown tables → `rust_xlsxwriter` worksheets                     |
| audio_overview (episode exists)                    | .m4a    | —    | Copy the synthesized episode (see "Audio format" below)            |
| everything else — note, summary, faq, study_guide, briefing, timeline, insights, quiz, round_table, problems, evidence, prd, prfaq, rfc, skill, report, template | .docx | .pdf | Markdown → `docx-rs` (headings, lists, tables, inline styles) |

Audit notes on the prose bucket:

- **timeline** renders as markdown prose (`- **when** — what` bullets), not a
  visual — docx stands. (The infographic parser's timeline rail is a block
  *inside* infographic notes, not this kind.)
- **quiz** renders as an interactive answer-and-check worksheet, not a
  presentation — docx gives the printable worksheet with its answer key;
  pptx would put every answer one slide after its question for no one.
- **briefing** and **round_table** never get TTS episodes (synthesis is
  gated to audio_overview plus the Brief pipeline), so no audio item.
  Brief **report** notes can carry an episode; their audio is exportable
  where the audio actually shows — the player's save button — and via MCP
  `export_note` with format "m4a" for any note that has one.

## Surfaces

- **UI**: the note row menu in the Studio panel gains flat `Export…` items
  matched to the kind (primary + PDF). Destination comes from the native
  save dialog (same `@tauri-apps/plugin-dialog` pattern as the audio player);
  an "Exporting…" toast marks the in-flight window and the saved file is
  revealed in Finder.
- **MCP**: `export_note { note_id, format, path? }` writes the file and
  returns the absolute path. `path` optional — defaults to
  `~/Downloads/<title>.<ext>`. "pdf" is accepted for any kind.

Both go through one command (`export::export_note`), so the UI and agents can
never drift. The command is async and every blocking stage — the docx/xlsx/
pptx builders, pdfium rasterizing, file copies — runs under
`spawn_blocking`, so an export never stalls the UI or an async worker.

## The print pipeline (PDF and PNG)

There is no DOM rasterizer in WKWebView we can trust (`foreignObject`
canvases are tainted), but the app already has proven pieces: per-kind
fixed-ink print sheets (poster, slide pages, flashcard study sheet, and a
markdown sheet for prose), `print_webview`'s silent `NSPrintSaveJob`, and
pdfium page rendering. Export composes them:

1. The backend opens a small `win-export-*` window booted onto the note with
   an export flag carrying a temp PDF path.
2. The window renders the kind's print sheet (`PrintExportView.tsx`) and
   silently prints itself to that PDF — landscape 16:9 for slide decks,
   portrait otherwise. Mind maps print as a fixed-ink SVG of the whole laid
   out tree scaled to one page, never a panned viewport crop.
3. PDF export ships that file as-is; PNG export rasterizes every page at
   poster width (wider for mind maps) and stitches them vertically.

Fully local, and the PDF is pixel-identical to what the in-app print buttons
produce.

## pptx: hand-rolled OOXML, deliberately minimal

No maintained Rust crate writes PowerPoint files, so `pptx.rs` writes the
OOXML package by hand — a zip of literal XML parts: `[Content_Types].xml`,
package rels, `presentation.xml`, one master + one blank layout + one theme,
and a plain text-box slide per entry. No placeholder inheritance, no
numbering XML, no images; slides are explicit text boxes, which is exactly
what PowerPoint and Keynote need and nothing more.

- **Slide decks** parse with the same rules as `SlideDeck.tsx` (`parseDeck`):
  skip the front-matter style block, split on `---`, first heading is the
  slide title, bullets keep their nesting.
- **Flashcards** parse with the same rules as `Flashcards.tsx` (`parseCards`)
  and become a **question slide then an answer slide** per card — the deck
  IS the self-test: face the prompt full-screen, advance to reveal, which is
  how the cards are used in the app.

Validated by tests (zip structure, every part well-formed XML, rels/content
types complete) and by importing into Keynote and macOS QuickLook.

## Audio format: m4a, not mp3

The episode is already an AAC `.m4a` (Kokoro WAVs stitched and encoded by
`afconvert`, tts.rs). macOS ships no MP3 encoder — `afconvert` cannot write
one — and the Rust options are LAME bindings (LGPL C, same class of licensing
we deliberately avoided with espeak). `.m4a` plays everywhere the user will
paste it, so export copies the episode as-is.

## Dependencies

- `pulldown-cmark` (no default features) — one CommonMark+tables parse feeds
  both document writers.
- `docx-rs` (no default features) — pure-Rust .docx writer; tiny tree, no
  image feature since notes are text.
- `rust_xlsxwriter` — the maintained xlsx writer (same author as Python's
  XlsxWriter).
- pptx: no dependency — see above. (`quick-xml`, already in the tree via
  docx-rs, appears as a dev-dependency for the well-formedness tests.)
