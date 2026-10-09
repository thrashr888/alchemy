import { describe, expect, it } from "vitest";
import {
  CONFIDENT_AT,
  confidentItems,
  inboxKind,
  inboxTitle,
  percent,
  verbForKey,
} from "./inbox";
import type { InboxItem } from "./types";

const item = (over: Partial<InboxItem> = {}): InboxItem => ({
  id: "i1",
  createdAt: 0,
  url: "",
  text: "",
  title: "",
  files: [],
  excerpt: "",
  suggestedNotebookId: "nb1",
  suggestedTitle: "Research",
  isNew: false,
  probability: 0.9,
  auto: true,
  alternatives: [],
  judge: "j",
  suggested: true,
  ...over,
});

const alt = (n: number) => ({
  notebookId: `a${n}`,
  title: `Alt ${n}`,
  probability: 0.1,
});

describe("inboxTitle", () => {
  it("prefers the title, then file name, domain, first line", () => {
    expect(inboxTitle(item({ title: " A page " }))).toBe("A page");
    expect(inboxTitle(item({ files: ["/Users/p/Docs/notes.pdf"] }))).toBe(
      "notes.pdf",
    );
    expect(inboxTitle(item({ files: ["/a/x.pdf", "/a/y.pdf", "/a/z.pdf"] }))).toBe(
      "x.pdf and 2 more",
    );
    expect(inboxTitle(item({ url: "https://www.example.com/a/b" }))).toBe(
      "example.com/a/b",
    );
    expect(inboxTitle(item({ url: "https://example.com/" }))).toBe(
      "example.com",
    );
    expect(inboxTitle(item({ text: "\n\n  first line\nsecond" }))).toBe(
      "first line",
    );
    expect(inboxTitle(item())).toBe("Untitled capture");
  });
});

describe("inboxKind", () => {
  it("is a file before a url before text", () => {
    expect(inboxKind(item({ files: ["/a"], url: "https://x.y" }))).toBe("file");
    expect(inboxKind(item({ url: "https://x.y" }))).toBe("url");
    expect(inboxKind(item({ text: "hi" }))).toBe("text");
  });
});

describe("confidentItems", () => {
  it("takes confident existing-notebook picks and nothing else", () => {
    const rows = [
      item({ id: "yes", probability: CONFIDENT_AT }),
      item({ id: "low", probability: 0.69 }),
      item({ id: "new", isNew: true, probability: 0.99 }),
      item({ id: "router", probability: null }),
      item({ id: "none", suggestedNotebookId: "", probability: 0.95 }),
      // Confident on paper, from a judge whose margin is not trusted.
      item({ id: "untrusted", auto: false, probability: 0.95 }),
    ];
    expect(confidentItems(rows).map((r) => r.id)).toEqual(["yes"]);
  });
});

describe("verbForKey", () => {
  const withAlts = item({ alternatives: [alt(1), alt(2)] });

  it("moves with the arrows even on an empty list", () => {
    expect(verbForKey("ArrowDown", undefined)).toEqual({
      type: "move",
      delta: 1,
    });
    expect(verbForKey("ArrowUp", withAlts)).toEqual({ type: "move", delta: -1 });
    expect(verbForKey("Enter", undefined)).toBeNull();
  });

  it("files, dismisses and starts a new notebook", () => {
    expect(verbForKey("Enter", withAlts)).toEqual({ type: "accept" });
    expect(verbForKey("Backspace", withAlts)).toEqual({ type: "dismiss" });
    expect(verbForKey("Delete", withAlts)).toEqual({ type: "dismiss" });
    expect(verbForKey("n", withAlts)).toEqual({ type: "new" });
    expect(verbForKey("N", withAlts)).toEqual({ type: "new" });
  });

  it("maps 1-4 to an alternative only while it exists", () => {
    expect(verbForKey("1", withAlts)).toEqual({
      type: "alternative",
      index: 0,
    });
    expect(verbForKey("2", withAlts)).toEqual({
      type: "alternative",
      index: 1,
    });
    expect(verbForKey("3", withAlts)).toBeNull();
    expect(verbForKey("5", withAlts)).toBeNull();
    expect(verbForKey("x", withAlts)).toBeNull();
  });
});

describe("percent", () => {
  it("rounds to a whole number", () => {
    expect(percent(0.923)).toBe("92%");
    expect(percent(1)).toBe("100%");
  });
});
