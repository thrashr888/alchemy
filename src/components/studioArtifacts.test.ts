import { describe, expect, it } from "vitest";
import { ARTIFACTS, SHELF_TOP, studioArtifacts } from "./studioArtifacts";

/** The Studio shelves fold, so the rule that decides which generators a
 *  person sees without opening anything is worth pinning down: counts first,
 *  then the primary set, then the shelf's own order. */
describe("studioArtifacts", () => {
  it("keeps every generator reachable across top and folded", () => {
    const shelves = studioArtifacts(false);
    const listed = shelves.flatMap((s) => [...s.top, ...s.folded].map((a) => a.kind));
    expect(new Set(listed)).toEqual(new Set(ARTIFACTS.map((a) => a.kind)));
    expect(listed).toHaveLength(ARTIFACTS.length);
  });

  it("shows each shelf its own number of rows", () => {
    for (const shelf of studioArtifacts(false)) {
      expect(shelf.top).toHaveLength(SHELF_TOP[shelf.id]);
    }
  });

  it("merges Learn and Visualize into one shelf", () => {
    const shelves = studioArtifacts(false);
    expect(shelves.map((s) => s.id)).toEqual(["understand", "learn", "write"]);
    expect(shelves[1].label).toBe("Learn and visualize");
    const learn = [...shelves[1].top, ...shelves[1].folded].map((a) => a.kind);
    expect(learn).toContain("study_guide");
    expect(learn).toContain("mind_map");
  });

  it("floats the primary kinds up when no notes exist yet", () => {
    const shelves = studioArtifacts(false);
    expect(shelves[0].top.map((a) => a.kind)).toEqual(["summary", "briefing", "faq"]);
    // process and slide_deck are the visual kinds people actually make.
    expect(shelves[1].top.map((a) => a.kind)).toEqual(["process", "slide_deck"]);
  });

  it("lets a notebook's own counts outrank the primary order", () => {
    const shelves = studioArtifacts(false, { evidence: 9, round_table: 4 });
    expect(shelves[0].top.map((a) => a.kind)).toEqual([
      "evidence",
      "round_table",
      "summary",
    ]);
  });

  it("offers Audio Overview on Understand once its voice model is ready", () => {
    const off = studioArtifacts(false);
    expect([...off[0].top, ...off[0].folded].map((a) => a.kind)).not.toContain(
      "audio_overview",
    );
    const on = studioArtifacts(true);
    expect(on[0].top[0].kind).toBe("audio_overview");
  });
});
