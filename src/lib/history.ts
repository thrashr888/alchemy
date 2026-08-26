// Session undo history (docs/RFC-professional-grade.md Pillar 5).
//
// The undo toasts already built the hard part — snapshot-and-restore
// closures for every recoverable delete (DESIGN.md §9: undo beats confirm).
// What they lacked was memory: the closure died with the toast, so a user
// who looked away lost the only way back. This is that memory.
//
// Pure stack mechanics live here so they can be tested without a store.
// The store owns the state and the wiring; see `pushHistory` in store.ts.

/** One reversible mutation. `label` is a verb phrase ("Delete Source") that
 *  the Edit menu renders as "Undo Delete Source", so it must read correctly
 *  after that prefix. */
export interface HistoryEntry {
  id: string;
  label: string;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
}

/** Deep enough to cover a working session's mistakes, shallow enough that
 *  the retained snapshots (deleted note bodies, source text) can't grow into
 *  a leak. Session-scoped: nothing here survives a relaunch. */
export const HISTORY_LIMIT = 50;

let seq = 0;

/** Entries are identified rather than compared by value so a toast can undo
 *  its own mutation out of order — see `dropEntry`. */
export function makeEntry(
  label: string,
  undo: () => Promise<void>,
  redo: () => Promise<void>,
): HistoryEntry {
  return { id: `history-${++seq}`, label, undo, redo };
}

/** Push onto the undo stack, discarding the oldest entry past the limit. */
export function pushEntry(
  stack: HistoryEntry[],
  entry: HistoryEntry,
): HistoryEntry[] {
  return [...stack, entry].slice(-HISTORY_LIMIT);
}

/** Remove one entry wherever it sits. Toast-clicked undo is inherently
 *  out of order — delete A, delete B, then click A's toast — so that path
 *  drops its entry instead of popping the top. The cost is that an
 *  out-of-order undo is not redoable, which is deliberate: rebuilding a
 *  coherent redo from a hole in the middle of the stack would guess at an
 *  intent the user never expressed. */
export function dropEntry(
  stack: HistoryEntry[],
  id: string,
): HistoryEntry[] {
  return stack.filter((e) => e.id !== id);
}

/** The Edit-menu label for the next undo, or null when the stack is empty
 *  (the item greys out). */
export function undoLabel(stack: HistoryEntry[]): string | null {
  const top = stack[stack.length - 1];
  return top ? `Undo ${top.label}` : null;
}

export function redoLabel(stack: HistoryEntry[]): string | null {
  const top = stack[stack.length - 1];
  return top ? `Redo ${top.label}` : null;
}
