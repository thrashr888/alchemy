import { api } from "./api";
import type { SourceEvent } from "./types";

/**
 * Arrivals (docs/RFC-events.md §6): what the watchers saw since the reader
 * last looked. Pure helpers over `SourceEvent` rows — the strip in the
 * sources panel and the Home digest both tally with these, and neither
 * calls a model.
 *
 * The seen watermark is per notebook, in the database (`app_state`): the
 * app is single-tenant by design, so UI state belongs there too, not in a
 * webview's localStorage that another window or a reinstall forgets.
 */

/** Epoch ms the notebook's arrivals were last dismissed; 0 = never. */
export async function loadSeenAt(notebookId: string): Promise<number> {
  try {
    return await api.arrivalsSeenAt(notebookId);
  } catch {
    return 0;
  }
}

export function saveSeenAt(notebookId: string, at: number) {
  void api.markArrivalsSeen(notebookId, at).catch(() => {
    /* best-effort — the strip just shows again next time */
  });
}

/** How many items an event stands for. Folder scans coalesce a pass into
 *  one row ("3 new files", "12 files gone") so a sync tool dropping 400
 *  files never writes 400 rows; the count rides at the front of `detail`.
 *  Everything else is one item. */
export function eventCount(e: SourceEvent): number {
  if (e.kind !== "added" && e.kind !== "removed") return 1;
  const m = /^(\d+)\s/.exec(e.detail);
  const n = m ? Number(m[1]) : 1;
  return n > 0 ? n : 1;
}

const plural = (n: number, one: string, many: string) =>
  `${n} ${n === 1 ? one : many}`;

/** Group added/removed by the source they landed in: "3 new from Tauri
 *  blog". Past three sources the list collapses to a count. */
function perSource(events: SourceEvent[], verb: string): string[] {
  const bySource = new Map<string, number>();
  for (const e of events)
    bySource.set(e.sourceTitle, (bySource.get(e.sourceTitle) ?? 0) + eventCount(e));
  const total = [...bySource.values()].reduce((a, b) => a + b, 0);
  if (bySource.size === 0) return [];
  if (bySource.size > 3)
    return [`${total} ${verb} from ${plural(bySource.size, "source", "sources")}`];
  return [...bySource.entries()].map(([title, n]) => `${n} ${verb} from ${title}`);
}

/** The strip's one line: tallies grouped by kind, e.g.
 *  "3 new from Tauri blog · 1 page changed · 2 reminders done". Empty when
 *  there is nothing to say. Order follows the RFC's vocabulary table. */
export function tallyEvents(events: SourceEvent[]): string[] {
  const of = (kind: string) => events.filter((e) => e.kind === kind);
  const parts: string[] = [...perSource(of("added"), "new")];
  const updated = of("updated");
  if (updated.length === 1) parts.push(`${updated[0].sourceTitle} changed`);
  else if (updated.length > 1) parts.push(`${updated.length} changed`);
  parts.push(...perSource(of("removed"), "gone"));
  const unreachable = of("unreachable");
  if (unreachable.length) parts.push(`${unreachable.length} unreachable`);
  const completed = of("completed");
  if (completed.length)
    parts.push(plural(completed.length, "reminder done", "reminders done"));
  const moved = of("moved");
  if (moved.length) parts.push(`${moved.length} rescheduled`);
  return parts;
}

/** Events the reader has not dismissed, newest first (the backend already
 *  sorts; this only filters). */
export function unseenEvents(events: SourceEvent[], seenAt: number): SourceEvent[] {
  return events.filter((e) => e.at > seenAt);
}

/** Verb for one event row in the expanded list. */
export const EVENT_VERB: Record<string, string> = {
  added: "new",
  updated: "changed",
  removed: "gone",
  unreachable: "unreachable",
  completed: "done",
  moved: "rescheduled",
};
