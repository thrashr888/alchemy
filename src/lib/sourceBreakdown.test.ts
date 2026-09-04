import { describe, expect, it } from "vitest";
import {
  breakdownKey,
  formatShare,
  sourceBreakdown,
} from "./sourceBreakdown";
import type { Source } from "./types";

const s = (
  sourceType: Source["sourceType"],
  url = "",
  charCount = 0,
): { sourceType: Source["sourceType"]; url: string; charCount: number } => ({
  sourceType,
  url,
  charCount,
});

const many = (
  n: number,
  sourceType: Source["sourceType"],
  charCount = 0,
) => Array.from({ length: n }, () => s(sourceType, "", charCount));

describe("breakdownKey", () => {
  it("folds url and html into web", () => {
    expect(breakdownKey(s("url", "https://example.com"))).toBe("web");
    expect(breakdownKey(s("html", "https://example.com"))).toBe("web");
  });

  it("recovers office files from the path of a flattened text source", () => {
    expect(breakdownKey(s("text", "/docs/plan.docx"))).toBe("office");
    expect(breakdownKey(s("text", "/docs/deck.pptx"))).toBe("office");
    expect(breakdownKey(s("text", "/docs/rows.csv"))).toBe("office");
    expect(breakdownKey(s("text", "/docs/notes.txt"))).toBe("text");
  });

  it("leaves a web URL alone even when its path ends in an ext", () => {
    expect(breakdownKey(s("url", "https://example.com/a.csv"))).toBe("web");
  });

  it("keeps every other type as itself", () => {
    expect(breakdownKey(s("pdf", "/a.pdf"))).toBe("pdf");
    expect(breakdownKey(s("folder", "/repo"))).toBe("folder");
    expect(breakdownKey(s("mac", "cider://notes/1"))).toBe("mac");
  });
});

describe("sourceBreakdown", () => {
  it("has nothing to say about an empty notebook", () => {
    expect(sourceBreakdown([])).toEqual([]);
  });

  it("ranks by count and shares sum to 100", () => {
    const out = sourceBreakdown([
      ...many(6, "url"),
      ...many(3, "pdf"),
      ...many(1, "markdown"),
    ]);
    expect(out.map((x) => x.key)).toEqual(["web", "pdf", "markdown"]);
    expect(out[0].share).toBeCloseTo(60);
    expect(out.reduce((n, x) => n + x.share, 0)).toBeCloseTo(100);
  });

  it("counts sources, not characters, so empty folders still show", () => {
    const out = sourceBreakdown([
      ...many(1, "pdf", 500_000),
      ...many(1, "folder", 0),
    ]);
    expect(out.map((x) => [x.key, x.share])).toEqual([
      ["folder", 50],
      ["pdf", 50],
    ]);
    expect(out.find((x) => x.key === "pdf")!.chars).toBe(500_000);
  });

  it("folds a rare tail into Other and sorts it last", () => {
    const out = sourceBreakdown([
      ...many(91, "url"),
      ...many(4, "pdf"),
      ...many(3, "markdown"),
      ...many(1, "image"),
      ...many(1, "mac"),
    ]);
    expect(out.map((x) => x.key)).toEqual([
      "web",
      "pdf",
      "markdown",
      "other",
    ]);
    const other = out[out.length - 1];
    expect(other.count).toBe(2);
    expect(other.share).toBeCloseTo(2);
  });

  it("names a one-type tail rather than calling it Other", () => {
    const out = sourceBreakdown([...many(99, "url"), s("mac")]);
    expect(out.map((x) => x.key)).toEqual(["web", "mac"]);
  });

  it("caps the legend at the limit", () => {
    const out = sourceBreakdown(
      [
        ...many(5, "url"),
        ...many(5, "pdf"),
        ...many(5, "markdown"),
        ...many(5, "image"),
        ...many(5, "mac"),
      ],
      { limit: 3 },
    );
    expect(out).toHaveLength(4);
    expect(out[out.length - 1].key).toBe("other");
    expect(out[out.length - 1].count).toBe(10);
  });

  it("labels slices for the legend", () => {
    const out = sourceBreakdown([s("okf"), s("url")]);
    expect(out.map((x) => x.label).sort()).toEqual(["OpenKnowledge", "Web"]);
  });
});

describe("formatShare", () => {
  it("rounds to one decimal and keeps the trailing zero", () => {
    expect(formatShare(50)).toBe("50.0%");
    expect(formatShare(12.34)).toBe("12.3%");
    expect(formatShare(100)).toBe("100.0%");
  });

  it("floors a nonzero sliver instead of showing 0.0%", () => {
    expect(formatShare(0.01)).toBe("<0.1%");
  });
});
