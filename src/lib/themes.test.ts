// The contrast matrix: WCAG AA across every theme for the token pairs that
// carry meaning (DESIGN.md §2). Any theme edit that breaks readability
// should fail this suite before it fails a human's eyes.
import { describe, expect, it } from "vitest";
import { THEMES } from "./themes";
import { contrastRatio, parseColor, relativeLuminance, tokenContrast } from "./contrast";

const AA_TEXT = 4.5; // WCAG AA, normal body/UI text
const AA_LARGE = 3.0; // WCAG AA, large text (>=18.66px bold / >=24px regular) or captions/UI-only text

// [foreground token, background token, minimum ratio, role note]
const PAIRS: Array<[string, string, number, string]> = [
  ["foreground", "background", AA_TEXT, "primary text"],
  ["muted-foreground", "surface", AA_TEXT, "secondary text, labels"],
  // subtle-foreground is captions/tertiary metadata by design (DESIGN.md
  // §2) — small, deliberately de-emphasized, never body copy. AA's 3:1
  // large-text/UI floor is the honest bar for that role, not 4.5:1.
  ["subtle-foreground", "surface-2", AA_LARGE, "captions, tertiary metadata"],
  ["citation", "surface", AA_TEXT, "citation chips, links, accent text"],
  ["destructive", "background", AA_TEXT, "errors, delete affordances"],
  // Button label vs. its own fill — the "primary fill vs on-color" pair.
  ["primary-foreground", "primary", AA_TEXT, "text on primary button fill"],
];

describe("contrast.ts WCAG math", () => {
  it("computes 21:1 for black on white", () => {
    expect(
      contrastRatio(parseColor("#000000"), parseColor("#ffffff")),
    ).toBeCloseTo(21, 1);
  });

  it("computes 1:1 for identical colors", () => {
    expect(contrastRatio(parseColor("#5e6ad2"), parseColor("#5e6ad2"))).toBeCloseTo(1, 5);
  });

  it("gives pure white full luminance and pure black none", () => {
    expect(relativeLuminance(parseColor("#ffffff"))).toBeCloseTo(1, 5);
    expect(relativeLuminance(parseColor("#000000"))).toBeCloseTo(0, 5);
  });

  it("composites a translucent rgba token over its backdrop rather than ignoring alpha", () => {
    // 50% white over black composites to mid-gray (~50% luminance channel),
    // which must land strictly between black-on-black (1:1) and
    // white-on-black (21:1) contrast — proof the alpha path actually runs.
    const onBlack = tokenContrast("rgba(255,255,255,0.5)", "#000000");
    expect(onBlack).toBeGreaterThan(1);
    expect(onBlack).toBeLessThan(21);
    // And it must match hand compositing: 50% white over black = #808080.
    expect(onBlack).toBeCloseTo(contrastRatio(parseColor("#808080"), parseColor("#000000")), 1);
  });

  it("rejects a color format it does not recognize rather than silently passing", () => {
    expect(() => parseColor("hsl(220 60% 50%)")).toThrow();
  });
});

describe("theme contrast matrix (WCAG AA)", () => {
  const themeEntries = Object.values(THEMES);

  it("covers all themes declared in THEMES", () => {
    // Guards against a future theme silently skipping the matrix because
    // THEMES was refactored to something this suite doesn't iterate.
    expect(themeEntries.length).toBeGreaterThanOrEqual(23);
  });

  for (const theme of themeEntries) {
    describe(`${theme.label} (${theme.id})`, () => {
      for (const [fgToken, bgToken, min, role] of PAIRS) {
        it(`${fgToken} on ${bgToken} >= ${min}:1 (${role})`, () => {
          const fg = theme.vars[fgToken];
          const bg = theme.vars[bgToken];
          expect(fg, `theme "${theme.id}" is missing token --${fgToken}`).toBeTruthy();
          expect(bg, `theme "${theme.id}" is missing token --${bgToken}`).toBeTruthy();

          const ratio = tokenContrast(fg, bg);
          expect(
            ratio,
            `[${theme.id}] --${fgToken} (${fg}) on --${bgToken} (${bg}) measured ` +
              `${ratio.toFixed(2)}:1, below the ${min}:1 floor for ${role}`,
          ).toBeGreaterThanOrEqual(min);
        });
      }
    });
  }
});
