import type { NavEntry, ReaderDoc } from "./storeTypes";

export interface NavSnapshot {
  currentId: string | null;
  ledgerOpen: boolean;
  galleryOpen: boolean;
  readerOpen: boolean;
  readerDoc?: ReaderDoc;
  homeSection: "notebooks" | "chat" | "registry";
  openCardId: string | null;
}

/** Canonicalize live store state into one browser-history location. Reader
 * highlights are events, not locations; Home sections and open Registry cards
 * are locations because Back/Forward must visibly restore them. */
export function navEntryFromSnapshot(snapshot: NavSnapshot): NavEntry {
  const mode = snapshot.galleryOpen
    ? ("gallery" as const)
    : snapshot.ledgerOpen
      ? ("ledger" as const)
      : snapshot.readerOpen
        ? ("reader" as const)
        : ("chat" as const);
  const entry: NavEntry = {
    nb: snapshot.currentId,
    mode,
    ...(snapshot.readerOpen && snapshot.readerDoc
      ? {
          doc: {
            type: snapshot.readerDoc.type,
            id: snapshot.readerDoc.id,
          },
        }
      : {}),
  };
  if (snapshot.currentId === null) {
    entry.homeSection = snapshot.homeSection;
    if (snapshot.homeSection === "registry" && snapshot.openCardId)
      entry.openCardId = snapshot.openCardId;
  }
  return entry;
}

export function sameNavEntry(a: NavEntry | undefined, b: NavEntry): boolean {
  return (
    !!a &&
    a.nb === b.nb &&
    a.mode === b.mode &&
    a.doc?.type === b.doc?.type &&
    a.doc?.id === b.doc?.id &&
    a.homeSection === b.homeSection &&
    a.openCardId === b.openCardId
  );
}
