/**
 * The stock eraser faces (Shantell Sans, Inter, JetBrains Mono — all SIL
 * OFL 1.1, licenses beside the files), vendored under src/assets/diagrams
 * so a diagram renders with no network and the same metrics every time.
 * The render frame registers them for measurement; the viewer declares them
 * in the app document so the copied scene paints with the same fonts.
 */
import shantellUrl from "../assets/diagrams/fonts/ShantellSans.var.woff2?url";
import interUrl from "../assets/diagrams/fonts/Inter.var.woff2?url";
import monoUrl from "../assets/diagrams/fonts/JetBrainsMono-Regular.woff2?url";

export interface DiagramFontFace {
  family: string;
  url: string;
  weight?: string;
}

export const DIAGRAM_FONTS: readonly DiagramFontFace[] = [
  // Variable faces must declare their weight range, or bold text matches the
  // face and renders at the default axis position (no synthesis).
  { family: "ShantellSans", url: shantellUrl, weight: "300 800" },
  { family: "Inter", url: interUrl, weight: "100 900" },
  { family: "JetBrainsMono", url: monoUrl },
];

const ROLES: Record<string, [family: string, fallback: string]> = {
  rough: ["ShantellSans", "sans-serif"],
  clean: ["Inter", "sans-serif"],
  mono: ["JetBrainsMono", "monospace"],
};

/** The `#eraser-fonts` stylesheet: role variables plus one `@font-face` per face. */
export function diagramFontsCss(): string {
  const vars = Object.entries(ROLES)
    .map(([role, [family, fallback]]) => `--font-${role}:'${family}',${fallback}`)
    .join(";");
  const faces = DIAGRAM_FONTS.map(
    (face) =>
      `@font-face{font-family:'${face.family}';${face.weight ? `font-weight:${face.weight};` : ""}src:url('${face.url}') format('woff2')}`,
  ).join("");
  return `:root{${vars}}${faces}`;
}

const DOCUMENT_STYLE_ID = "alchemy-diagram-fonts";

/**
 * Declare the faces in the app document once. A scene copied into a shadow
 * root inherits the document's `@font-face` set — a shadow tree's own
 * declarations do not load in WebKit — so without this the viewer would
 * paint the fonts the frame measured with as plain sans-serif.
 */
export function ensureDocumentDiagramFonts(): void {
  if (document.getElementById(DOCUMENT_STYLE_ID)) return;
  const style = document.createElement("style");
  style.id = DOCUMENT_STYLE_ID;
  style.textContent = diagramFontsCss();
  document.head.appendChild(style);
}
