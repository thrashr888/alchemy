# RFC: Architecture diagrams with eraser-diagrams

Studio draws one kind of diagram today: `uml`, Mermaid source the model
writes and the WebView renders. Mermaid is right for class, sequence,
state, and ER diagrams and wrong for the diagram people actually want from
a corpus about a system — boxes with the technology's icon on them, grouped
by tier or host, with labeled lines. This RFC evaluates
[eraser-diagrams](https://github.com/eraserlabs/eraser-diagrams) (MIT) for
that job and ships a prototype: a new `architecture` artifact.

**Verdict:** yes, in the WebView, with one piece of our own. eraser's
resolve and render packages run in WKWebView with no Chromium and no
network; the template library is data. What eraser does not do is place
anything — entity coordinates are input — so the generator writes topology
and a small deterministic layout of ours (`src/lib/diagramLayout.ts`) puts
it on the page.

![Alchemy's own architecture, generated from a 19-entity document](images/diagrams-3-alchemy.png)

## Findings

**Rendering without Chromium.** The monorepo's published packages split
cleanly: `@eraserlabs/protocol` (types), `@eraserlabs/resolve` (validate,
sanitize, inline icons — pure, "runs in the browser playground"),
`@eraserlabs/layout` (line routing), `@eraserlabs/render` (an in-page
fill → mount → measure → route → apply engine exposed as
`window.__eraser`), and `@eraserlabs/diagrams`, the "impure conductor" that
owns playwright-core and the stock library. The library is importable
from `@eraserlabs/diagrams/library` as data — 72 KB of template HTML/CSS
and JSON schemas — without touching Chromium; only the package root pulls
playwright. The three stock fonts (Shantell Sans, Inter, JetBrains Mono,
OFL) are 492 KB of woff2, vendored. So a document renders from JSON alone
inside the app. Two adaptations were needed: `@eraserlabs/layout` reads
`process.env` unguarded (a Vite `define` stubs it), and the render engine's
base stylesheet is global (`*{box-sizing}` and a rule on every
`svg[stroke='currentColor']` that would restyle every lucide icon), so it
runs in a same-origin iframe (`diagram-frame.html`, a second Vite entry —
the CSP's `script-src 'self'` rules out srcdoc) and the serialized scene is
copied into a shadow root in the viewer.

**Icons.** Names resolve against Eraser's hosted catalog: 3,865 SVGs,
4.8 MB, on a public GCS bucket with no CORS headers — so the WebView could
not fetch them even if we wanted the network call. The prototype vendors a
curated 178-icon set (179 KB raw, `src/assets/diagrams/icons`) covering
clouds, stores, runtimes, agents, and the generic glyphs an architecture
needs; the prompt carries the same list and a test holds the two in
lockstep. Unknown names draw eraser's placeholder glyph plus a warning the
viewer shows. A Rust fetch-and-cache path for the long tail (icon name in,
SVG out, `~/Library/Application Support` cache) is the obvious follow-up
if the curated set proves too small.

**The document schema.** MDP 0.1: `{ entities, connections }` of tagged
objects. Stock entity tags are `Shape` (12 shapes, text runs, optional
icon), `Icon` (icon + caption), `Group`/`Lane`/`Pool` (containers, joined
via `containerId`), `Textbox`, BPMN `Activity`/`Event`/`Gateway`,
`DatabaseTable`, `Legend`, `Divider`; connections default to `Relationship`
(`from`, `to`, `label`, arrowheads, ports, line style). Colors are eight
palette tokens or any CSS color. The compact contract in the prompt
(`rag.rs`, `architecture_instruction`) is four entity forms, the connection
form, the palette, and the icon list — about 1.6 KB plus icons.

**Placement.** eraser's README is explicit: "treats entity placement as
input." Its own examples hand-author `x`/`y`. A model guessing pixels is
the failure mode Mermaid spares us, so the `architecture` document carries
no coordinates (any the model volunteers are dropped) and
`diagramLayout.ts` places it: containers laid out recursively and sized
around their members, siblings ranked by longest path over the lifted
connections (cycles cut in DFS order), ranks stacked as bands along a
`direction`, swimlanes (`Lane`/`Pool`) running their steps across the flow
with columns shared between sibling lanes. The engine renders twice —
once on estimated sizes to learn what each component measures, once on the
measurements — in tens of milliseconds. Placement is pure and unit tested.

**What it does that Mermaid does not.** Technology icons on components
(the reason to want it); nested groups with titles, tints, and icons;
swimlane processes with real lanes; routed orthogonal lines with placed
labels, dashes, and arrowheads; a hand-drawn (`rough`) typeface option.
What Mermaid does that it does not: sequence, class, state, and ER
diagrams, Gantt, git graphs. Those stay `uml`.

![Cloud architecture](images/diagrams-1-cloud.png) ![Swimlane process](images/diagrams-2-swimlane.png)

**Export.** The serialized scene is ordinary positioned HTML, so the
existing print pipeline (docs/RFC-note-export.md) covers PDF and PNG:
`PrintArchitecture` scales the scene to the sheet width on a white ground
and signals when it has settled, `export.rs` rasterizes it at diagram
width like `uml`. No canvas, no foreignObject, no Chromium.

## The kind

- `architecture` joins `ARTIFACT_KINDS` (`rag.rs`), so Studio, the chat
  router, report schedules, and the MCP `generate` tool all take it with
  no further wiring; label "Architecture diagram", learning family, beside
  UML in the picker.
- The prompt asks for the JSON object only: 6–16 entities in 1–5 groups,
  components named as the sources name them, no invented parts, every
  connection labeled in four words or fewer, icons from the list or none.
- `ArchitectureDiagram.tsx`: title chip, warning count, Source toggle,
  pan/zoom canvas (`PanCanvas`); a document that will not resolve shows
  the JSON with the resolver's message, which names the element and
  suggests the fix (did-you-mean on tags, icons, enums).
- The note stores the document verbatim, so it is also a valid input to
  the upstream CLI — portable, per the sovereignty invariant.
- Harness: `scripts/diagram-harness/` renders three hand-written samples
  through the real pipeline (`pnpm exec vite --config
  scripts/diagram-harness/vite.config.ts`, then
  `node scripts/diagram-harness/snap.mjs` to refresh `docs/images/`).

## Risks

- **Bundle.** Three packages plus vendored assets: the lazy diagram chunk
  (resolve + library + render + icons) is loaded on first render only; the
  fonts are 492 KB of assets fetched on first use. `@eraserlabs/diagrams`
  installs playwright-core (not bundled — we import its data subpaths
  only). Asking upstream for a `@eraserlabs/library` package, or vendoring
  `templates.gen.js`, would drop that install-time weight.
- **Model JSON validity.** Same shape of risk as Mermaid, better tooling:
  the resolver's errors are structured and specific. Not yet measured on
  local models; a judged eval (docs/RFC-judged-evals.md) over a few
  notebooks is the next step before default-on.
- **Label placement.** eraser places connection labels; on tight ranks
  they can land on a group title. Wider gaps help; this is theirs to
  improve and the layout's to avoid.
- **Theme.** eraser paints for paper. The scene sits on a white card in
  every theme, like a Mermaid SVG carries its own colors. A dark palette
  is data (`palette` in the library) and could follow.
- **Version.** 0.1.0 across the board; "a change to the 0.x minor version
  may be breaking." Pin exact versions.
- **License.** MIT for the code, OFL for the fonts, Eraser's icon catalog
  served from their public bucket without a stated license — the vendored
  subset should be confirmed with Eraser before release.

## Not in this RFC

BPMN tags (`Activity`/`Event`/`Gateway`), `DatabaseTable` (an ER-shaped
kind eraser could also draw), the fetch-and-cache icon fallback, a dark
palette, global step alignment across nested lanes, and letting a user
edit the document in place with a re-render.
