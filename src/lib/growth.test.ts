import { describe, expect, it } from "vitest";
import {
  growthAttention,
  growthSourceRevision,
  visibleGrowthProposals,
} from "./growth";
import type { GrowthProposal, HygieneIssue } from "./types";

const proposal = (url: string): GrowthProposal => ({
  url,
  kind: "web",
  anchor: url,
  mentions: 1,
  sourceCount: 1,
  matchedQuery: "",
  score: 1,
});
const issue = (
  sourceId: string,
  bucket: HygieneIssue["bucket"],
  kind: HygieneIssue["kind"] = "source",
): HygieneIssue => ({
  sourceId,
  bucket,
  kind,
  title: sourceId,
  detail: "",
  keeperId: "",
});

describe("Grow review eligibility", () => {
  it("removes imported and dismissed proposals from both review surfaces", () => {
    const proposals = [
      proposal("imported"),
      proposal("dismissed"),
      proposal("new"),
    ];
    const imported = new Set(["imported"]);
    const dismissed = { dismissed: 1 };
    expect(visibleGrowthProposals(proposals, imported, dismissed)).toEqual([
      proposals[2],
    ]);
    expect(
      visibleGrowthProposals(proposals, imported, { ...dismissed, new: 2 }),
    ).toEqual([]);
  });

  it("counts each flagged object once, including notes with multiple flags", () => {
    const rows = [
      issue("note", "duplicate", "note"),
      issue("note", "empty-note", "note"),
      issue("source", "missing-file"),
      issue("source", "duplicate"),
    ];
    expect(growthAttention(rows, {})).toEqual([rows[0], rows[2]]);
  });

  it("does not advertise kept or stale flags that the pane hides", () => {
    const rows = [
      issue("source", "stale"),
      issue("note", "stale", "note"),
      issue("kept", "husk"),
    ];
    expect(growthAttention(rows, { "kept:husk": true })).toEqual([]);
  });

  it("keeps another flag reviewable when only one flag on the object was kept", () => {
    const rows = [issue("source", "missing-file"), issue("source", "duplicate")];
    expect(growthAttention(rows, { "source:missing-file": true })).toEqual([rows[1]]);
  });

  it("does not conflate a note and a source with the same id", () => {
    const rows = [issue("same", "duplicate"), issue("same", "duplicate", "note")];
    expect(growthAttention(rows, {})).toEqual(rows);
  });
});

describe("Grow proposal refresh", () => {
  const source = {
    id: "a",
    status: "ready" as const,
    url: "https://a.test",
    fetchedAt: 1,
  };

  it("refreshes when replacing a source without changing the ready count", () => {
    expect(growthSourceRevision([source])).not.toBe(
      growthSourceRevision([{ ...source, id: "b" }]),
    );
  });

  it("refreshes after the source content is fetched again", () => {
    expect(growthSourceRevision([source])).not.toBe(
      growthSourceRevision([{ ...source, fetchedAt: 2 }]),
    );
  });

  it("is stable for an unchanged metadata reload", () => {
    expect(growthSourceRevision([source])).toBe(
      growthSourceRevision([{ ...source }]),
    );
  });
});
