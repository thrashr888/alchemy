import { describe, expect, it } from "vitest";
import {
  kindCounts,
  liveFacet,
  missingSourceIds,
  sourceKind,
  tagCounts,
} from "./sourceFacets";
import type { Source } from "./types";

const s = (sourceType: Source["sourceType"], tags = "") => ({
  sourceType,
  tags,
});

describe("sourceKind", () => {
  it("buckets folder-like containers together", () => {
    expect(sourceKind(s("folder"))).toBe("folders");
    expect(sourceKind(s("git"))).toBe("folders");
    expect(sourceKind(s("feed"))).toBe("folders");
    expect(sourceKind(s("url"))).toBe("web");
    expect(sourceKind(s("html"))).toBe("web");
    expect(sourceKind(s("image"))).toBe("images");
    expect(sourceKind(s("mac"))).toBe("apple");
    expect(sourceKind(s("pdf"))).toBe("files");
  });
});

describe("kindCounts", () => {
  it("counts the list it is given", () => {
    const counts = kindCounts([s("url"), s("url"), s("pdf")]);
    expect(counts.get("web")).toBe(2);
    expect(counts.get("files")).toBe(1);
  });

  it("drops a kind once its last source is removed", () => {
    const before = [s("url"), s("pdf")];
    const after = before.filter((x) => x.sourceType !== "url");
    expect(kindCounts(before).has("web")).toBe(true);
    expect(kindCounts(after).has("web")).toBe(false);
  });
});

describe("tagCounts", () => {
  it("ranks by count, then alphabetically", () => {
    const counts = tagCounts([s("pdf", "b a"), s("pdf", "a"), s("pdf", "a c")]);
    expect(counts).toEqual([
      ["a", 3],
      ["b", 1],
      ["c", 1],
    ]);
  });

  it("keeps the filtering tag on screen past the cap", () => {
    const sources = [
      s("pdf", "t1 t2 t3 t4 t5 t6"),
      s("pdf", "t1 t2 t3 t4 t5 t6"),
      s("pdf", "rare"),
    ];
    expect(tagCounts(sources).map(([t]) => t)).not.toContain("rare");
    expect(tagCounts(sources, "rare").map(([t]) => t)).toContain("rare");
  });

  it("does not resurrect a tag no source carries any more", () => {
    expect(tagCounts([s("pdf", "a")], "gone").map(([t]) => t)).toEqual(["a"]);
  });
});

describe("liveFacet", () => {
  it("holds a selection the list still offers", () => {
    expect(liveFacet("web", kindCounts([s("url")]))).toBe("web");
  });

  it("clears a selection whose last source was removed", () => {
    expect(liveFacet("web", kindCounts([s("pdf")]))).toBeNull();
    expect(liveFacet("rare", new Set(["common"]))).toBeNull();
  });

  it("leaves an unset facet unset", () => {
    expect(liveFacet(null, kindCounts([s("url")]))).toBeNull();
  });
});

describe("missingSourceIds", () => {
  const src = (id: string, status: Source["status"], remote = false) => ({
    id,
    status,
    remote,
  });

  it("collects cloud stubs and files the sweep can no longer find", () => {
    const sources = [
      src("a", "placeholder"),
      src("b", "ready"),
      src("c", "ready"),
    ];
    const ids = missingSourceIds(sources, [
      { sourceId: "c", bucket: "missing-file" },
      { sourceId: "b", bucket: "duplicate" },
    ]);
    expect([...ids].sort()).toEqual(["a", "c"]);
  });

  it("ignores hygiene rows for sources that are already gone", () => {
    const ids = missingSourceIds(
      [src("a", "ready")],
      [{ sourceId: "removed", bucket: "missing-file" }],
    );
    expect(ids.size).toBe(0);
  });

  it("leaves out sources whose file lives on another Mac", () => {
    // The notebook came through a shared folder; the text is here and the
    // path is somebody else's drive (RFC-okf-live §5.8). Nothing to find,
    // so nothing to list under Missing.
    const ids = missingSourceIds(
      [src("here", "ready"), src("away", "ready", true)],
      [
        { sourceId: "here", bucket: "missing-file" },
        { sourceId: "away", bucket: "missing-file" },
      ],
    );
    expect([...ids]).toEqual(["here"]);
  });
});
