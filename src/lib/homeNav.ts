// Where Home can be told to go, and what makes two of those places the
// same one.
//
// Home used to keep four fold-able cards, and the ⌘1–4 that addressed them
// had to be handed a live toggle by whichever HomeView was mounted. The
// cards are gone: every place is now a section the store holds, so the
// table is static and the dispatch is a plain store write
// (`goHomePlace` in store.ts). Pure data and one pure predicate, so both
// can be read — and tested — without a store.

import type { HomeScope, HomeSection, NavEntry } from "./storeTypes";

/** One row of Home's sidebar, as something to navigate to. */
export interface HomePlace {
  /** The native menu id (menu.rs's registry) — the key both sides share. */
  id: string;
  section: HomeSection;
  /** Which notebooks the shelf shows, for `section: "notebooks"`. Shared
   *  and Archived are the same shelf narrowed, not shelves of their own. */
  scope?: HomeScope;
  /** ⌘-digit, 1–9. The Timeline has none: the digits ran out. */
  key?: number;
  /** Going here means the whole shelf — a tag left running would narrow a
   *  place the user just asked for in full. */
  clearTag?: boolean;
}

/** Home's places in sidebar order: the Library block, the Registry block,
 *  then the Brief and the Timeline. menu.rs's View menu is built from the
 *  same order and carries the same digits — keep the two tables together. */
export const HOME_PLACES: HomePlace[] = [
  {
    id: "menu-home-notebooks",
    section: "notebooks",
    scope: "all",
    key: 1,
    clearTag: true,
  },
  { id: "menu-home-chats", section: "chat", key: 2 },
  { id: "menu-home-shared", section: "notebooks", scope: "shared", key: 3 },
  { id: "menu-home-reports", section: "reports", key: 4 },
  { id: "menu-home-archived", section: "notebooks", scope: "archived", key: 5 },
  { id: "menu-home-staff", section: "staff", key: 6 },
  { id: "menu-home-cards", section: "registry", key: 7 },
  { id: "menu-home-suggested", section: "suggested", key: 8 },
  { id: "menu-home-brief", section: "brief", key: 9 },
  { id: "menu-home-timeline", section: "timeline" },
];

/** The place a native menu id names, or undefined for an id that isn't
 *  Home's. */
export function homePlaceById(id: string): HomePlace | undefined {
  return HOME_PLACES.find((p) => p.id === id);
}

/** The place ⌘<digit> means on Home, or undefined for a digit nothing
 *  claims. In a notebook the same digits mean its panels — the keydown
 *  handler reads the view before it asks either table. */
export function homePlaceByKey(digit: number): HomePlace | undefined {
  return HOME_PLACES.find((p) => p.key === digit);
}

/** Whether two history entries are the same PLACE — the test that keeps
 *  back/forward from stacking an entry for somewhere the user already is.
 *
 *  Every field of an entry counts, because every one of them is part of
 *  where you were: a filtered shelf is not the unfiltered one, and the
 *  Registry with a card open is not the cast. The reader's highlight is
 *  deliberately not in `NavEntry` at all — a citation jump is an event. */
export function sameNavEntry(
  a: NavEntry | undefined,
  b: NavEntry | undefined,
): boolean {
  if (!a || !b) return false;
  return (
    a.nb === b.nb &&
    a.mode === b.mode &&
    a.doc?.type === b.doc?.type &&
    a.doc?.id === b.doc?.id &&
    a.section === b.section &&
    a.thread === b.thread &&
    a.card === b.card &&
    a.scope === b.scope &&
    a.tag === b.tag
  );
}
