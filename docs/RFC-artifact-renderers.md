# RFC: Native artifact renderers — quiz, flashcards, slide deck

## Summary

Mind maps set the pattern: the generator emits a strict, plain-markdown spec
(reliable even from small local models), and a native component does the
visual work — falling back to plain Markdown whenever parsing fails, so an
artifact never arrives broken. This RFC extends that pattern to the three
artifacts that are still walls of text: **flashcards** become a flippable
deck, **quizzes** become answerable with scoring, and a new **slide deck**
kind renders as actual slides instead of raw Marp-style markdown.

## Why renderers, not smarter generation

- Asking models for Mermaid/HTML/SVG breaks constantly; asking for a rigid
  text format almost never does (see `MindMap.tsx`). The existing flashcard
  and quiz specs in `rag::artifact_spec` are already rigid enough to parse —
  every existing note lights up retroactively, no migration.
- Interactivity (flip, answer, navigate) is UI state, not content. It belongs
  in the renderer, where it survives Rebuild and Edit round-trips untouched.

## Design

### Markdown specs (the generator contract)

- **Flashcards** (existing spec, unchanged): `**Front:** …` / `**Back:** …`
  pairs separated by `---` lines.
- **Quiz** (existing spec, unchanged): `## Questions` with numbered items,
  options `A)`–`D)` one per line, then `## Answer Key` with
  `<n>. <letter> — <explanation>` entries.
- **Slide deck** (new kind `slide_deck`): Marp-style — slides separated by
  `---` lines. First slide is `# <title>` plus a one-line subtitle; body
  slides are `## <heading>` plus up to ~5 tight bullets (or a short quote /
   table); last slide is takeaways. No code fences around the deck, no
  speaker notes in v1.

### Renderers (`src/components/`)

Each parses its spec and falls back to `<Markdown>` when it can't
(mirroring `MindMap`): parse errors must degrade to what users see today.

- `Flashcards.tsx` — one card at a time: click/Space flips front→back,
  ←/→ browse, progress counter. After a flip, pass/fail grading ("Missed
  it" / "Got it", keys 1/2) drives Leitner-style spaced repetition: each
  card carries a box 0-4 with review intervals of now/1/3/7/21 days,
  persisted in localStorage per note id + a hash of the card front (so
  regenerating a deck keeps the schedule of unchanged cards). Sessions
  order due cards first, end with a summary, and offer a missed-cards-only
  review pass. This is the effective core of spaced repetition — active
  recall, self-grading, expanding intervals — without imported-algorithm
  ceremony (full SM-2 ease factors add little at flashcard-deck scale).
- `QuizView.tsx` — all questions listed; clicking an option grades it
  immediately against the answer key (correct/incorrect coloring plus the
  key's explanation), running score at the top, Reset to retake.
- `SlideDeck.tsx` — slides are laid out at a fixed 960×540 design
  resolution and scaled (CSS transform) to fit any box — modal, note
  window, fullscreen Present mode, or print page — so aspect is always
  16:9 and nothing scrolls; over-long content autofits down
  PowerPoint-style (zoom, floor 0.5). Layouts are inferred from content,
  never declared, so small models stay reliable: `# h1` → centered title;
  a lone `## h2` → section divider; blockquote-only → big quote; one short
  paragraph → statement slide; tables get width; everything else is
  heading-plus-bullets. The generator prompt asks for a mix of these
  shapes and substantive 40-80-word slides. Decks are styled by
  front-matter (`theme:` — any of the app's UI themes, palette derived
  from its tokens; `font:` — sans/serif/mono/rounded system stacks),
  chosen by the generator to fit the topic and switchable from the deck
  controls; switches persist by rewriting the note's front-matter. The
  note modal grows to near-window width for decks.
- PDF export (decks and flashcards) is one click: native save dialog, then
  a silent `NSPrintSaveJob` writes the file and reveals it in Finder. The
  `print_webview` command drives the public
  `printOperationWithPrintInfo:` itself — wry's `print()` uses WKWebView's
  private selector and produces blank pages — with three hard-won rules:
  set the operation view's frame (or pages print blank), run the
  sheet-modal variant (the blocking `runOperation` nests a modal run loop
  inside tao's event handler and spins a core at 100% forever; completion
  is observed by polling the output file to a stable size), and render the
  print-only DOM as a visible overlay (WKWebView paints hidden/off-screen
  content as blank). Deck pages are true 16:9 (custom 792×445.5pt paper,
  zero margins, full-bleed theme); flashcards print as a margined portrait
  study sheet.

### Wiring

- `rag.rs::artifact_spec` gains `slide_deck`; `types.ts::NoteKind`,
  `studioArtifacts.tsx` (documents family, `Presentation` icon) expose it in
  Studio and the command menu.
- `StudioNoteViewer.tsx` and `NoteWindow.tsx` kind-switches route
  `flashcards`, `quiz`, and `slide_deck` to the new renderers. Raw markdown
  stays reachable via Edit and Copy, and streaming Rebuild still shows text.

## Out of scope (v1)

- Speaker notes, slide themes, export to PPTX/PDF (print CSS can come later).
- Full SM-2 ease factors and cross-device sync of flashcard review state
  (localStorage is per-machine); quiz attempt history.
- Retrofitting other kinds (timeline, data_table already read fine as prose).
