/**
 * One bit of memory about the search index: a rebuild that started and never
 * reported finishing.
 *
 * Changing the search model invalidates every vector, so Settings drops the
 * whole chunk index and rebuilds it (`reembed_all`). Quit the app mid-rebuild,
 * or let it fail, and the library is left part-indexed with nothing on screen
 * saying so — sources that never got their turn simply stop turning up in
 * search and citations.
 *
 * The stamp below is written before a rebuild starts and cleared when it
 * returns, so a stamp that survives a relaunch is exactly that failure. It
 * records what this app did, not what the store holds — the store itself
 * keeps no "was this indexed" flag to read (`chunk_count` on a source is
 * written at ingest and is not zeroed by a rebuild, so it cannot answer the
 * question).
 */

const KEY = "reindexPending";
/** Fired on this window whenever the stamp changes. */
const EVENT = "nb:reindex-pending";

/** Record that a rebuild is under way, naming the model it is rebuilding to. */
export function markReindexStarted(model: string) {
  try {
    localStorage.setItem(KEY, model);
  } catch {
    /* private mode, or a full store — the banner is the only thing lost */
  }
  window.dispatchEvent(new Event(EVENT));
}

/** Record that the index is whole again. */
export function clearReindexPending() {
  try {
    localStorage.removeItem(KEY);
  } catch {
    /* see above */
  }
  window.dispatchEvent(new Event(EVENT));
}

/** The model an unfinished rebuild was heading for, or null when there is
 *  nothing outstanding. */
export function reindexPending(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

export function subscribeReindexPending(onChange: () => void): () => void {
  window.addEventListener(EVENT, onChange);
  // Another window rebuilding clears it for this one too.
  window.addEventListener("storage", onChange);
  return () => {
    window.removeEventListener(EVENT, onChange);
    window.removeEventListener("storage", onChange);
  };
}
