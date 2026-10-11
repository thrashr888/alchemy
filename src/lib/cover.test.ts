import { describe, expect, it, vi } from "vitest";

// The renderer's imports reach the store and the shader, which need a DOM;
// the choice logic needs neither.
vi.mock("@/components/DitherBackground", () => ({ buildProgram: () => null }));
vi.mock("@/lib/api", () => ({ api: {} }));

import { NO_COVER, coverChoice, parseCover } from "./cover";

describe("cover choice", () => {
  it("parses a stored style, none, and automatic", () => {
    expect(parseCover("dither:nb-2")).toEqual({ style: "dither", seed: "nb-2" });
    expect(parseCover("none")).toBe(NO_COVER);
    expect(parseCover("")).toBeNull();
    expect(parseCover(undefined)).toBeNull();
    expect(parseCover("photo:nb")).toBeNull();
    expect(parseCover("mist:")).toBeNull();
  });

  it("resolves none to no cover and automatic to the id's own picture", () => {
    expect(coverChoice("nb", "none")).toBeNull();
    expect(coverChoice("nb", "ascii:nb-3")).toEqual({ style: "ascii", seed: "nb-3" });
    expect(coverChoice("nb", "")?.seed).toBe("nb");
    expect(coverChoice("nb").seed).toBe("nb");
  });
});
