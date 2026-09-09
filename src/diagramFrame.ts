/**
 * The render frame for architecture diagrams (docs/RFC-diagrams.md).
 *
 * `@eraserlabs/render/browser` fills templates, mounts them in the page
 * body, measures what the layout engine produced, routes the lines, and
 * applies the result — all against `document`. Its base stylesheet is
 * global (`*{box-sizing}`, `svg[stroke='currentColor']{stroke-width:…}`),
 * which would restyle every lucide icon in the app, so the engine runs in
 * this same-origin iframe instead. The parent (`lib/eraserDiagram.ts`)
 * drives `window.__eraser` directly across the frame boundary and copies
 * the serialized scene out; this document only registers the stock fonts
 * so the measurements match the paint.
 */
import "@eraserlabs/render/browser";
import { DIAGRAM_FONTS, diagramFontsCss } from "./lib/diagramFonts";

window.__alchemyDiagramFrame = {
  ready: window.__eraser.registerFonts({
    css: diagramFontsCss(),
    faces: [],
    urlFaces: DIAGRAM_FONTS.map((face) => ({ family: face.family })),
  }),
};
