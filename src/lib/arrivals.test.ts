import { describe, expect, it } from "vitest";
import { eventCount, tallyEvents, unseenEvents } from "./arrivals";
import type { SourceEvent } from "./types";

const ev = (kind: string, sourceTitle: string, detail = "", at = 10): SourceEvent => ({
  id: `${kind}-${sourceTitle}-${at}`,
  notebookId: "nb",
  sourceId: `src-${sourceTitle}`,
  sourceTitle,
  kind,
  detail,
  diff: "",
  at,
});

describe("eventCount", () => {
  it("reads the coalesced count off folder-scan rows and defaults to one", () => {
    expect(eventCount(ev("added", "Docs", "3 new files"))).toBe(3);
    expect(eventCount(ev("removed", "Docs", "12 files gone"))).toBe(12);
    expect(eventCount(ev("added", "Docs", "new file · report.pdf"))).toBe(1);
    // Only arrivals and departures coalesce; a diff's "+12 −3" is not a count.
    expect(eventCount(ev("updated", "Docs", "12 lines changed"))).toBe(1);
  });
});

describe("tallyEvents", () => {
  it("groups by kind and source in the RFC's order", () => {
    const parts = tallyEvents([
      ev("added", "Tauri blog"),
      ev("added", "Tauri blog"),
      ev("added", "Tauri blog"),
      ev("updated", "SFist"),
      ev("completed", "Home"),
      ev("completed", "Home"),
      ev("moved", "Calendar"),
      ev("unreachable", "Old site"),
      ev("removed", "Docs", "2 files gone"),
    ]);
    expect(parts).toEqual([
      "3 new from Tauri blog",
      "SFist changed",
      "2 gone from Docs",
      "1 unreachable",
      "2 reminders done",
      "1 rescheduled",
    ]);
  });
  it("collapses arrivals past three sources and counts plural changes", () => {
    const parts = tallyEvents([
      ev("added", "A", "2 new files"),
      ev("added", "B"),
      ev("added", "C"),
      ev("added", "D"),
      ev("updated", "X"),
      ev("updated", "Y"),
    ]);
    expect(parts).toEqual(["5 new from 4 sources", "2 changed"]);
  });
  it("says nothing for nothing", () => {
    expect(tallyEvents([])).toEqual([]);
  });
});

describe("unseenEvents", () => {
  it("keeps only rows newer than the watermark", () => {
    const rows = [ev("added", "A", "", 30), ev("added", "B", "", 20), ev("added", "C", "", 10)];
    expect(unseenEvents(rows, 20).map((e) => e.sourceTitle)).toEqual(["A"]);
    expect(unseenEvents(rows, 0)).toHaveLength(3);
  });
});
