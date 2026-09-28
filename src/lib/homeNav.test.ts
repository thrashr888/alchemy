import { describe, expect, it } from "vitest";
import {
  HOME_PLACES,
  homePlaceById,
  homePlaceByKey,
  sameNavEntry,
} from "./homeNav";
import type { NavEntry } from "./storeTypes";

const shelf = (over: Partial<NavEntry> = {}): NavEntry => ({
  nb: null,
  mode: "chat",
  section: "notebooks",
  scope: "all",
  tag: null,
  ...over,
});

describe("Home's places", () => {
  it("gives ⌘1–⌘9 to nine of the ten, in sidebar order", () => {
    expect(HOME_PLACES.map((p) => p.key)).toEqual([
      1, 2, 3, 4, 5, 6, 7, 8, 9, undefined,
    ]);
  });

  it("names every id menu.rs registers", () => {
    expect(HOME_PLACES.map((p) => p.id)).toEqual([
      "menu-home-notebooks",
      "menu-home-chats",
      "menu-home-shared",
      "menu-home-reports",
      "menu-home-archived",
      "menu-home-staff",
      "menu-home-cards",
      "menu-home-suggested",
      "menu-home-brief",
      "menu-home-timeline",
    ]);
  });

  it("scopes the three shelf rows and nothing else", () => {
    const shelves = HOME_PLACES.filter((p) => p.section === "notebooks");
    expect(shelves.map((p) => p.scope)).toEqual(["all", "shared", "archived"]);
    expect(
      HOME_PLACES.filter((p) => p.section !== "notebooks" && p.scope),
    ).toEqual([]);
  });

  it("clears the tag filter only on the way to the whole shelf", () => {
    expect(HOME_PLACES.filter((p) => p.clearTag).map((p) => p.id)).toEqual([
      "menu-home-notebooks",
    ]);
  });

  it("looks a place up by menu id and by digit", () => {
    expect(homePlaceById("menu-home-archived")?.scope).toBe("archived");
    expect(homePlaceByKey(6)?.section).toBe("staff");
    expect(homePlaceById("menu-toggle-sources")).toBeUndefined();
    expect(homePlaceByKey(0)).toBeUndefined();
  });
});

describe("sameNavEntry", () => {
  it("holds for an entry against itself", () => {
    expect(sameNavEntry(shelf(), shelf())).toBe(true);
  });

  it("has nothing to compare against an empty stack", () => {
    expect(sameNavEntry(undefined, shelf())).toBe(false);
  });

  it("tells a narrowed shelf from the whole library", () => {
    expect(sameNavEntry(shelf(), shelf({ tag: "invoices" }))).toBe(false);
    expect(sameNavEntry(shelf(), shelf({ scope: "archived" }))).toBe(false);
  });

  it("tells two conversations apart, and two cards", () => {
    const chat = (thread: string): NavEntry => ({
      nb: null,
      mode: "chat",
      section: "chat",
      thread,
    });
    expect(sameNavEntry(chat("t1"), chat("t2"))).toBe(false);
    expect(sameNavEntry(chat("t1"), chat("t1"))).toBe(true);
    const card = (id: string | null): NavEntry => ({
      nb: null,
      mode: "chat",
      section: "registry",
      card: id,
    });
    expect(sameNavEntry(card(null), card("acme"))).toBe(false);
  });

  it("tells one notebook's reader doc from another's", () => {
    const at = (nb: string, id: string): NavEntry => ({
      nb,
      mode: "reader",
      doc: { type: "source", id },
    });
    expect(sameNavEntry(at("a", "s1"), at("a", "s1"))).toBe(true);
    expect(sameNavEntry(at("a", "s1"), at("a", "s2"))).toBe(false);
    expect(sameNavEntry(at("a", "s1"), at("b", "s1"))).toBe(false);
  });

  it("ignores a highlight, which NavEntry never carries", () => {
    // The reader's citation highlight is an event, not a place: recordNav
    // strips it, so two jumps into the same doc are one entry.
    const doc = { type: "note" as const, id: "n1" };
    expect(
      sameNavEntry({ nb: "a", mode: "reader", doc }, { nb: "a", mode: "reader", doc }),
    ).toBe(true);
  });
});
