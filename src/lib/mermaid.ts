import DOMPurify from "dompurify";

/**
 * Mermaid setup, kept out of the component on purpose: these are pure
 * functions over the DOM's CSS variables, with no React and no store, so a
 * render harness can import exactly what ships without dragging in Tauri.
 * See src/components/MermaidBlock.tsx for the rendering itself.
 */

/** Read a theme token off :root, resolved to a real color string. */
function token(styles: CSSStyleDeclaration, name: string, fallback: string) {
  const value = styles.getPropertyValue(name).trim();
  return value || fallback;
}

/** Parse `#rgb`, `#rrggbb`, `rgb()` and `rgba()` into channels + alpha.
 *  Returns null for anything else (`color-mix()`, `oklch()`, a keyword). */
function parseColor(
  input: string,
): { r: number; g: number; b: number; a: number } | null {
  const s = input.trim();
  const hex = s.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (hex) {
    const h = hex[1];
    const full =
      h.length === 3
        ? h
            .split("")
            .map((c) => c + c)
            .join("")
        : h;
    return {
      r: parseInt(full.slice(0, 2), 16),
      g: parseInt(full.slice(2, 4), 16),
      b: parseInt(full.slice(4, 6), 16),
      a: 1,
    };
  }
  const rgb = s.match(
    /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)(?:[\s,/]+([\d.%]+))?\s*\)$/i,
  );
  if (!rgb) return null;
  const alpha = rgb[4]
    ? rgb[4].endsWith("%")
      ? parseFloat(rgb[4]) / 100
      : parseFloat(rgb[4])
    : 1;
  return {
    r: Number(rgb[1]),
    g: Number(rgb[2]),
    b: Number(rgb[3]),
    a: Number.isFinite(alpha) ? alpha : 1,
  };
}

/**
 * Flatten a possibly-translucent token onto a backdrop, returning solid hex.
 *
 * This matters more than it looks. Alchemy's hairline borders are rgba white
 * at low alpha (`--border-strong: rgba(255,255,255,0.12)`), and mermaid's
 * "base" theme does not merely paint the values it is given — it derives
 * neighbouring colors from them with its own color math. Hand that math an
 * `rgba()` string and it gives back black, which is how a diagram ends up as
 * solid black shapes with black labels on a dark theme. Solid hex only.
 *
 * Anything unparseable (`color-mix()`, `oklch()`) falls back rather than
 * reaching mermaid — a slightly-off border beats an unreadable diagram.
 */
function opaque(color: string, backdrop: string, fallback: string): string {
  const fg = parseColor(color);
  const bg = parseColor(backdrop);
  if (!fg || !bg) return fallback;
  const mix = (f: number, b: number) => Math.round(f * fg.a + b * (1 - fg.a));
  const hex = (n: number) => Math.max(0, Math.min(255, n)).toString(16).padStart(2, "0");
  return `#${hex(mix(fg.r, bg.r))}${hex(mix(fg.g, bg.g))}${hex(mix(fg.b, bg.b))}`;
}

/** Map the app's design tokens onto the theme variables mermaid understands.
 *  Mermaid's "base" theme is the only one that honors all of these.
 *  Exported so a harness can verify exactly what ships. */
export function themeVariables() {
  const styles = getComputedStyle(document.documentElement);
  const background = opaque(
    token(styles, "--background", "#08090a"),
    "#000000",
    "#08090a",
  );
  const fg = opaque(token(styles, "--foreground", "#eceef1"), background, "#eceef1");
  const muted = opaque(
    token(styles, "--muted-foreground", "#8a8f98"),
    background,
    "#8a8f98",
  );
  const surface = opaque(token(styles, "--surface-2", "#141517"), background, "#141517");
  // Hairline borders are translucent white — flattened over the surface they
  // sit on, not the page, or they come out too dark to see.
  const border = opaque(token(styles, "--border-strong", "#3a3a3a"), surface, "#3a3a3a");
  const primary = opaque(token(styles, "--primary", "#5e6ad2"), background, "#5e6ad2");
  return {
    // Nodes read as the app's cards do: hairline border, surface fill.
    primaryColor: surface,
    primaryTextColor: fg,
    primaryBorderColor: border,
    secondaryColor: surface,
    secondaryTextColor: fg,
    secondaryBorderColor: border,
    tertiaryColor: background,
    tertiaryTextColor: fg,
    tertiaryBorderColor: border,
    background,
    mainBkg: surface,
    nodeBorder: border,
    nodeTextColor: fg,
    // Edges carry the one bit of color, the way citations do.
    lineColor: muted,
    textColor: fg,
    titleColor: fg,
    edgeLabelBackground: background,
    clusterBkg: background,
    clusterBorder: border,
    // Sequence and state diagrams reach for these directly.
    actorBkg: surface,
    actorBorder: border,
    actorTextColor: fg,
    signalColor: fg,
    signalTextColor: fg,
    labelBoxBkgColor: surface,
    labelBoxBorderColor: border,
    labelTextColor: fg,
    loopTextColor: fg,
    noteBkgColor: background,
    noteTextColor: fg,
    noteBorderColor: border,
    activationBkgColor: primary,
    // The token is a multi-line stack; mermaid writes it into an SVG style
    // attribute, where the newlines are noise at best.
    fontFamily: token(styles, "--font-sans", "ui-sans-serif, system-ui").replace(
      /\s+/g,
      " ",
    ),
    fontSize: "13px",
  };
}

/** The full initialize config. One definition, used by the component and by
 *  the render harness — a second hand-written copy drifts and then "verified"
 *  stops meaning anything. */
export function mermaidConfig() {
  return {
    startOnLoad: false,
    // Untrusted input: sanitize labels, refuse click bindings.
    securityLevel: "strict" as const,
    theme: "base" as const,
    // Root-level, NOT `flowchart.htmlLabels` — that key is deprecated in
    // mermaid 11 and silently loses to this one. With HTML labels on,
    // mermaid wraps node text in <foreignObject>, which the SVG sanitizer
    // below strips: diagrams rendered as empty shapes. Off, labels are real
    // SVG <text> and survive.
    htmlLabels: false,
    themeVariables: themeVariables(),
  };
}

/** Sanitize mermaid's SVG before it is injected. See the component note. */
export function sanitizeSvg(svg: string) {
  return DOMPurify.sanitize(svg, {
    USE_PROFILES: { svg: true, svgFilters: true },
    // `style` MUST survive. Mermaid emits its entire theme as a <style>
    // block inside the SVG and sets almost nothing via presentation
    // attributes — strip it and every shape falls back to SVG's default
    // black fill, with black text on top. The diagram still "renders", the
    // labels are still in the DOM, and it is completely unreadable. The CSS
    // is mermaid's own, generated from the config above; label text reaches
    // the document as escaped <text>, never as CSS.
    //
    // foreignObject stays forbidden — it is the route from an innocent
    // diagram back to arbitrary HTML, and htmlLabels:false means mermaid
    // never needs it.
    FORBID_TAGS: ["foreignObject", "script", "iframe", "object", "embed"],
    FORBID_ATTR: ["href", "xlink:href", "onload", "onerror", "onclick"],
  });
}
