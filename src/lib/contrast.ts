// WCAG 2.1 contrast math for design-token pairs. Pure and dependency-free so
// it can run in `themes.test.ts` under plain Node — no DOM needed to compute
// relative luminance from a token's own color string.

export interface RGB {
  r: number;
  g: number;
  b: number;
}

const HEX_RE = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i;
const RGBA_RE =
  /^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+))?\s*\)$/i;

function hexToRgb(hex: string): RGB {
  let h = hex.slice(1);
  if (h.length === 3) {
    h = h
      .split("")
      .map((c) => c + c)
      .join("");
  }
  const num = parseInt(h, 16);
  return { r: (num >> 16) & 255, g: (num >> 8) & 255, b: num & 255 };
}

/**
 * Parse a theme token color — hex or `rgba(...)` are the two formats present
 * in `themes.ts`. An `rgba` value with alpha < 1 is composited over
 * `backdrop` (the solid color it will actually be painted on), matching how
 * the browser renders a translucent token — alpha is never dropped silently.
 */
export function parseColor(value: string, backdrop?: RGB): RGB {
  const trimmed = value.trim();
  if (HEX_RE.test(trimmed)) return hexToRgb(trimmed);

  const m = trimmed.match(RGBA_RE);
  if (m) {
    const r = Number(m[1]);
    const g = Number(m[2]);
    const b = Number(m[3]);
    const a = m[4] !== undefined ? Number(m[4]) : 1;
    if (a >= 1 || !backdrop) return { r, g, b };
    // Alpha compositing over an opaque backdrop (source-over).
    return {
      r: r * a + backdrop.r * (1 - a),
      g: g * a + backdrop.g * (1 - a),
      b: b * a + backdrop.b * (1 - a),
    };
  }

  throw new Error(`contrast.ts: unrecognized color format "${value}"`);
}

function srgbChannel(c: number): number {
  const cs = c / 255;
  return cs <= 0.03928 ? cs / 12.92 : Math.pow((cs + 0.055) / 1.055, 2.4);
}

/** WCAG 2.1 relative luminance (§1.4.3), 0 (black) to 1 (white). */
export function relativeLuminance({ r, g, b }: RGB): number {
  return 0.2126 * srgbChannel(r) + 0.7152 * srgbChannel(g) + 0.0722 * srgbChannel(b);
}

/** WCAG 2.1 contrast ratio (§1.4.3), 1:1 (no contrast) to 21:1 (black/white). */
export function contrastRatio(a: RGB, b: RGB): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const lighter = Math.max(la, lb);
  const darker = Math.min(la, lb);
  return (lighter + 0.05) / (darker + 0.05);
}

/**
 * Contrast ratio between two theme token values, e.g. `foreground` painted
 * on `background`. `fg` is composited over `bg` first if it carries alpha
 * (borders and some accents are `rgba(...)` in `themes.ts`); `bg` itself is
 * assumed opaque, which holds for every surface/background token today.
 */
export function tokenContrast(fg: string, bg: string): number {
  const bgRgb = parseColor(bg);
  const fgRgb = parseColor(fg, bgRgb);
  return contrastRatio(fgRgb, bgRgb);
}
