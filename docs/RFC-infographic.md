# RFC: Infographic — a native HTML/CSS poster from a rigid markdown spec

## Summary

A new `infographic` artifact kind: the generator distills the corpus into a
strict, plain-markdown shape — a title, a hook, and 4-7 sections whose bodies
are stat lines, a small numeric table, bullets, a quote, or a paragraph — and
a native React renderer turns that shape into a polished vertical poster:
stat-tile grids, proportional CSS bars, fact cards, callouts. This is the
"HTML version" of the long-deferred image-generation infographic: the model
never emits HTML, SVG, or chart code; it emits numbers and labels, and the
renderer does all the visual work. Parsing failures fall back to plain
`<Markdown>`, so an infographic never arrives broken.

## Why

- The original infographic idea waited on local image generation that never
  materialized. But the essence of an infographic — big numbers, short
  labels, one comparison chart, a pull quote — needs no raster image at all;
  it needs layout and type, which HTML/CSS does better than a diffusion
  model anyway (and it stays selectable, themable, and printable).
- The house pattern (RFC-artifact-renderers) is proven four times over:
  model-emitted Mermaid/HTML/SVG breaks constantly; a rigid markdown spec
  plus a native renderer almost never does, even from small local models.
  Mind maps, flashcards, quizzes, and slide decks all ship on it.
- Everything visual is inference, not declaration: the model writes content
  shapes (`**73%** — label`, a 2-column table, bullets) and the renderer
  infers the block type — mirroring `slideLayout()`, which keeps small
  models reliable because there is no layout vocabulary to get wrong.

## Design

### Markdown spec (the generator contract)

Layout is NEVER declared; the renderer infers each block from its shape:

- Line 1: `# <title>` — punchy, under 8 words.
- One optional hook before the first section: a single short paragraph or a
  `> quote` stating the big takeaway.
- 4-7 `## <section>` blocks. Each section body is exactly ONE shape:
  - **Stat tiles** — consecutive lines `**<value>** — <label>`
    (`**73%** — of sources are PDFs`) → a responsive grid of big-number
    tiles.
  - **Bar chart** — a 2-column GFM table whose second column is numeric →
    horizontal proportional bars (the renderer computes the max and draws
    pure-CSS widths; the raw cell text stays as the value label).
  - **Fact cards** — a `- ` bullet list → one hairline card per fact.
  - **Callout** — a `> ` blockquote (plus an optional `—` attribution
    line) → a centered quote card.
  - **Narrative** — a plain paragraph → prose interlude (at most one per
    piece).

The prompt (in `rag::artifact_spec`) pins this shape with a worked example,
demands concrete numbers pulled from the corpus, short labels, and forbids
invented data — if the sources lack numbers, the model is told to use fact
cards rather than manufacture figures.

### Renderer (`src/components/Infographic.tsx`)

`parseInfographic(md)` returns `null` unless the content parses into a title
plus at least two recognizable sections; the component then falls back to
`<Markdown>` — never render broken (the `MindMap`/`SlideDeck` discipline).

The rendered poster is a single centered column (~680px) that scrolls
vertically: title and hook up top, then sections as hairline-separated bands
with `text-micro` uppercase kickers. All colors come from the app's semantic
tokens (`--foreground`, `--muted-foreground`, `--border`, `--primary`), so
the poster follows all 23 themes for free. Per DESIGN.md, the one accent is
the theme accent and it appears only where it means something — bar fills —
while tiles, cards, and callouts stay hairline-bordered, never tonally
filled, and no colored left-border accents anywhere.

Bars are plain divs with percentage widths, value labels in a right-aligned
tabular-nums column — no chart library, nothing to break.

PDF export reuses `usePrintExport` + `PrintPortal` (portrait, `size: auto`
with margins). The print sheet renders with fixed paper ink (near-black on
white, gray bars) exactly as the flashcards study sheet does — print is a
document, not chrome, so it deliberately does not inherit the screen theme
(a dark theme would print light-on-white). The visible-overlay print rules
in `index.css` (WKWebView paints hidden content as blank pages) apply
unchanged; sections are `print-card` so pages never break inside one.

### Wiring

- `rag.rs`: `"infographic"` joins `ARTIFACT_KINDS` and `artifact_spec` (the
  `artifact_kinds_match_specs` drift test keeps them locked together);
  report scheduling, chat tools, and the MCP `generate` tool all read that
  list, so agents get the kind for free.
- `types.ts::NoteKind` gains `"infographic"`; `studioArtifacts.tsx`
  registers it in the documents family (`BarChart3` icon), which exposes it
  in Studio and the command menu.
- Kind-switches route it to the renderer on all three surfaces:
  `StudioNoteViewer` (modal, default prose width — the poster is a column),
  `NoteWindow`, and `ReaderPane`. In the reader it counts as an artifact
  (read-only surface, raw-markdown editing behind the toolbar pencil, same
  as `slide_deck`); it scrolls rather than filling the pane, so it keeps
  `DocProperties` and the counts footer.

> Rev 2 (2026-08-02): three more inferred shapes, taxonomy borrowed from the
> most-installed infographic skill on skills.sh (timelines, funnels, and
> comparisons are what people actually reach for): **timeline** (a bullet run
> where every item starts with a date), **funnel** (a 2-column table whose
> first header cell is `Stage` — an explicit marker, because "decreasing
> values" would misread ranking tables), and **comparison** (exactly two
> `###` subheadings with bullets, rendered as vs-cards). Layout stays
> inferred, never declared.

## Out of scope (v1)

- Image generation (still no local imagegen worth shipping) and any raster
  export beyond the PDF.
- More chart types (donut/line/sparkline), multi-series bars, or declared
  layouts — the eight inferred shapes cover what small models can reliably
  produce.
- Themed poster palettes à la slide decks (`theme:` front-matter); the app
  theme is the poster theme in v1.

## Alternatives considered

- **Image generation** (the original plan): deferred — there is no local
  image model in the stack, and a text-heavy artifact rendered as pixels
  loses selection, search, theming, and accessibility. Revisit only if
  on-device imagegen lands.
- **Model-emitted raw HTML in a sandboxed iframe**: rejected on the
  RFC-artifact-renderers reliability precedent — asking models for
  HTML/SVG breaks constantly, every generation is a new one-off layout, and
  a sandbox both isolates it from the design tokens and adds an attack
  surface. Rigid spec + native renderer is the pattern that has never
  regressed.
- **Reusing `slide_deck` with a "poster" layout**: an infographic is a
  scrolling document, not a fixed 960×540 canvas; wedging vertical flow
  into the deck's scale-to-fit machinery would complicate both.
