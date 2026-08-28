import { describe, expect, it } from "vitest";
import { navEntryFromSnapshot, sameNavEntry } from "./appNavigation";

const base = {
  currentId: null,
  ledgerOpen: false,
  galleryOpen: false,
  readerOpen: false,
  homeSection: "notebooks" as const,
  openCardId: null,
};

describe("app navigation locations", () => {
  it("treats each Home section as a distinct Back/Forward destination", () => {
    const notebooks = navEntryFromSnapshot(base);
    const chat = navEntryFromSnapshot({ ...base, homeSection: "chat" });
    const registry = navEntryFromSnapshot({ ...base, homeSection: "registry" });

    expect(notebooks).toEqual({
      nb: null,
      mode: "chat",
      homeSection: "notebooks",
    });
    expect(sameNavEntry(notebooks, chat)).toBe(false);
    expect(sameNavEntry(chat, registry)).toBe(false);
  });

  it("records an open Registry card but drops Home fields inside notebooks", () => {
    const card = navEntryFromSnapshot({
      ...base,
      homeSection: "registry",
      openCardId: "card-1",
    });
    expect(card.openCardId).toBe("card-1");

    expect(
      navEntryFromSnapshot({
        ...base,
        currentId: "nb-1",
        homeSection: "chat",
        openCardId: "card-1",
      }),
    ).toEqual({ nb: "nb-1", mode: "chat" });
  });

  it("drops one-time reader highlights from the history identity", () => {
    const reader = navEntryFromSnapshot({
      ...base,
      currentId: "nb-1",
      readerOpen: true,
      readerDoc: { type: "source", id: "source-1", highlight: "needle" },
    });
    expect(reader).toEqual({
      nb: "nb-1",
      mode: "reader",
      doc: { type: "source", id: "source-1" },
    });
  });
});
