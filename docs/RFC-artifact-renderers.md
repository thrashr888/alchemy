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

## Addendum: UML diagrams (`uml`)

The UML kind breaks the rule at the top of this RFC on purpose, and it is
worth writing down why.

The rule is: don't ask a model for Mermaid, ask for a rigid text format and
lay it out natively. That holds when the native layout is tractable — a mind
map is a tree, and `MindMap.tsx` is 400 lines. UML is five different
grammars with five different layout algorithms (class boxes with typed
edges, sequence lifelines with activation bars, state machines, ER
cardinalities, component graphs). Writing those is a project, not a
renderer, and mermaid — already a dependency, already sanitized and
theme-mapped in `lib/mermaid.ts` — implements all five.

So the generator emits Mermaid source and `UmlDiagram.tsx` renders it. The
contract this RFC actually cares about — *an artifact never arrives broken* —
is kept a different way: a diagram that will not parse shows its source with
mermaid's own error message, so a near-miss model is a two-word fix rather
than a blank box and a Rebuild. The source view is always one click away
even when the diagram is fine.

- Content is bare Mermaid (`classDiagram` / `sequenceDiagram` /
  `stateDiagram-v2` / `erDiagram` / `flowchart TD`); `umlSource()` tolerates
  a stray ```mermaid fence or a prose preamble, since that is the failure
  models actually make.
- The diagram fills the pane on a shared `PanCanvas` (exported from
  `MindMap.tsx`) — UML grows wider than any reading column.
- Export follows the mind-map path: PNG primary via the print pipeline,
  rasterized at 2200px so class-box labels stay crisp.
