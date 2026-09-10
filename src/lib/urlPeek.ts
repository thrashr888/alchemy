import type { UrlPeek } from "./types";

// The Grow pane's link previews: one fetch per URL per session, one in
// flight per URL however many times the pointer crosses the row, and a
// result that lands after the pointer has moved on is dropped rather than
// painted over whatever the card shows now.

export const EMPTY_PEEK: UrlPeek = {
  title: "",
  description: "",
  imageUrl: "",
  site: "",
};

/** True when the page gave us nothing to show but its host. */
export function peekIsEmpty(peek: UrlPeek): boolean {
  return !peek.title && !peek.description;
}

export interface UrlPeeker {
  /** The session's answer for `url`, if it has already landed. */
  cached(url: string): UrlPeek | undefined;
  /** The pointer is now on `url`: fetch it (once per session) and call
   *  `onReady` when it lands — but only if the pointer is still there. A
   *  cached answer calls back synchronously. */
  watch(url: string, onReady: (peek: UrlPeek) => void): void;
  /** The pointer left `url` (or, with no argument, left the list). A
   *  fetch already in flight keeps going and fills the cache; its result
   *  just doesn't reach anyone. */
  unwatch(url?: string): void;
}

/** Build a peeker over `fetch` — `api.peekUrl` in the app, a stub in tests.
 *  A rejected fetch caches as the empty peek: the backend already resolves
 *  failures to empty strings, so a rejection here is a transport hiccup,
 *  and re-fetching on every hover would only repeat it. */
export function createUrlPeeker(
  fetch: (url: string) => Promise<UrlPeek>,
): UrlPeeker {
  const cache = new Map<string, UrlPeek>();
  const inflight = new Map<string, Promise<UrlPeek>>();
  // The one watcher a result may reach. Identity, not URL: leaving and
  // re-entering the same row registers a new watcher, and only the newest
  // hears the answer — the first would paint a second time otherwise.
  let current: { url: string; onReady: (peek: UrlPeek) => void } | null =
    null;

  const load = (url: string): Promise<UrlPeek> => {
    const pending = inflight.get(url);
    if (pending) return pending;
    const p = fetch(url)
      .catch(() => EMPTY_PEEK)
      .then((peek) => {
        cache.set(url, peek);
        inflight.delete(url);
        return peek;
      });
    inflight.set(url, p);
    return p;
  };

  return {
    cached: (url) => cache.get(url),
    watch(url, onReady) {
      const watcher = { url, onReady };
      current = watcher;
      const hit = cache.get(url);
      if (hit) {
        onReady(hit);
        return;
      }
      void load(url).then((peek) => {
        if (current === watcher) {
          current = null;
          onReady(peek);
        }
      });
    },
    unwatch(url) {
      if (url === undefined || current?.url === url) current = null;
    },
  };
}
