import type { MetaCitation, MetaTurn } from "./types";
import type { HomeRun } from "./storeTypes";

/**
 * The rules a Home (corpus-wide) conversation is run by, as plain functions
 * — the store wires them to Tauri, and these are the parts worth pinning
 * down on their own.
 *
 * The organising idea: a run belongs to the CONVERSATION it was asked in, not
 * to the view that started it. Everything here follows from that.
 */

/** What the backend sees as prior context: completed exchanges only. A
 *  provider failure leaves a dangling question that would only teach the
 *  model that answers can be error messages. */
export function historyOf(
  turns: MetaTurn[],
): { role: string; content: string }[] {
  const out: { role: string; content: string }[] = [];
  for (let i = 0; i + 1 < turns.length; i++) {
    const q = turns[i];
    const a = turns[i + 1];
    if (q.role === "user" && a.role === "assistant" && a.kind !== "error") {
      out.push(
        { role: "user", content: q.content },
        { role: "assistant", content: a.content },
      );
    }
  }
  return out;
}

/** How much of the live run the conversation on screen is entitled to see.
 *  An answer being written into another thread is that thread's business:
 *  it keeps running, but it doesn't appear under someone else's question. */
export function runForThread(
  run: HomeRun | null,
  threadId: string | null,
): HomeRun | null {
  return run && threadId && run.threadId === threadId ? run : null;
}

/** Which slot unsent composer text is kept under. Per conversation inside the
 *  Chat tab; the ask box over the notebook grid has its own, because a
 *  question typed there is a fresh subject, not a follow-up. */
export function homeDraftKey(
  chatOpen: boolean,
  threadId: string | null,
): string {
  return chatOpen ? `t:${threadId ?? "new"}` : "shelf";
}

/** The notebooks an answer drew from, in the order it first cited them — the
 *  palette's notebook chips. A citation into the Registry names no notebook,
 *  so the cast isn't a chip. */
export function citedNotebooks(
  citations: MetaCitation[],
): [id: string, title: string][] {
  const seen = new Map<string, string>();
  for (const c of citations)
    if (c.notebookId && !seen.has(c.notebookId))
      seen.set(c.notebookId, c.notebookTitle);
  return [...seen.entries()];
}

/** Turns just fetched for a thread, merged with what is already on screen for
 *  it. An answer that settled while the fetch was in flight is newer than
 *  what came back; overwriting would blink it away and then bring it back. */
export function mergeLoadedTurns(
  fetched: MetaTurn[],
  onScreen: MetaTurn[],
): MetaTurn[] {
  const known = new Set(fetched.map((t) => t.id));
  return [...fetched, ...onScreen.filter((t) => !known.has(t.id))];
}
