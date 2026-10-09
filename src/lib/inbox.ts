// The Inbox's pure rules: what a row is called, what a key means on it, and
// which rows "Accept all confident" may file. No store, no DOM, so each can
// be read and tested alone (inbox.test.ts).

import type { InboxItem } from "./types";

/** The line above which the judge's pick may be filed without a second look.
 *  The same 0.7 the registry's triage and Second Look read as "confident". */
export const CONFIDENT_AT = 0.7;

export type InboxKind = "file" | "url" | "text";

export function inboxKind(item: InboxItem): InboxKind {
  if (item.files.length > 0) return "file";
  if (item.url) return "url";
  return "text";
}

function baseName(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

function hostAndPath(url: string): string {
  try {
    const u = new URL(url);
    const path = u.pathname === "/" ? "" : u.pathname;
    return `${u.hostname.replace(/^www\./, "")}${path}`;
  } catch {
    return url;
  }
}

/** What to call a row: the title when there is one, else the domain, the
 *  file's name, or the first line of what was pasted. */
export function inboxTitle(item: InboxItem): string {
  const title = item.title.trim();
  if (title) return title;
  if (item.files.length > 0) {
    const first = baseName(item.files[0]);
    return item.files.length > 1
      ? `${first} and ${item.files.length - 1} more`
      : first;
  }
  if (item.url) return hostAndPath(item.url);
  const line = item.text.split("\n").find((l) => l.trim());
  return line?.trim().slice(0, 80) || "Untitled capture";
}

/** The rows "Accept all confident" files: a real notebook (never a new one)
 *  that a trusted judge was at least `CONFIDENT_AT` sure of. A row with no
 *  probability was chosen by the router alone and is never confident, and
 *  neither is one from a judge whose margins were never measured to mean
 *  anything (`auto` is false for those). */
export function confidentItems(items: InboxItem[]): InboxItem[] {
  return items.filter(
    (i) =>
      i.auto &&
      !i.isNew &&
      i.suggestedNotebookId !== "" &&
      i.probability !== null &&
      i.probability >= CONFIDENT_AT,
  );
}

export function percent(p: number): string {
  return `${Math.round(p * 100)}%`;
}

export type InboxVerb =
  | { type: "accept" }
  | { type: "alternative"; index: number }
  | { type: "new" }
  | { type: "dismiss" }
  | { type: "move"; delta: 1 | -1 };

/** What a key does on the selected row. Enter files the suggestion; 1–4 file
 *  into that alternative (only while it exists); N starts a new notebook;
 *  Backspace/Delete dismiss; the arrows move. Anything else is not ours. */
export function verbForKey(
  key: string,
  item: InboxItem | undefined,
): InboxVerb | null {
  if (key === "ArrowDown") return { type: "move", delta: 1 };
  if (key === "ArrowUp") return { type: "move", delta: -1 };
  if (!item) return null;
  if (key === "Enter") return { type: "accept" };
  if (key === "Backspace" || key === "Delete") return { type: "dismiss" };
  if (key === "n" || key === "N") return { type: "new" };
  if (/^[1-4]$/.test(key)) {
    const index = Number(key) - 1;
    return index < item.alternatives.length
      ? { type: "alternative", index }
      : null;
  }
  return null;
}
