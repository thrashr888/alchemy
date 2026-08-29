// Home's four sidebar cards, addressable from outside the view that owns
// them.
//
// Their open state is HomeView's own React state (each card writes its own
// localStorage key), and HomeView only mounts on Home — but the ⌘1–4 keydown
// handler lives in App.tsx, above both views, because the same four keys mean
// a notebook's panels when a notebook is open. So HomeView registers its
// toggles here on mount and stands down on unmount, the way RichEditor
// registers its undo history in textUndo.ts. Nothing registered means no Home
// on screen, and the shortcut is simply not ours to take.

/** The four cards in rail order: left rail top-to-bottom, then right rail.
 *  This is the View menu's order and ⌘1–4's order — keep all three the same. */
export const HOME_CARDS = ["chats", "staff", "brief", "reports"] as const;

export type HomeCard = (typeof HOME_CARDS)[number];

let toggle: ((card: HomeCard) => void) | null = null;

/** Called by HomeView on mount (with its toggles) and unmount (with null). */
export function registerHomeCards(fn: ((card: HomeCard) => void) | null): void {
  toggle = fn;
}

/** Show or hide one card. False when no Home is mounted to act on it, which
 *  is the caller's cue to leave the keystroke alone. */
export function toggleHomeCard(card: HomeCard): boolean {
  if (!toggle) return false;
  toggle(card);
  return true;
}
