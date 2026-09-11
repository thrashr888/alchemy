/** Wheel events have no portable finger-up/momentum phase. Treat the whole
 * burst as one gesture, including vertical events and irregular inertia. */
const GESTURE_IDLE_MS = 350;
const DIRECTION_THRESHOLD = 12;
const NAVIGATION_THRESHOLD = 90;

export function createReaderSwipeTracker() {
  let lastAt = -Infinity;
  let x = 0;
  let y = 0;
  let consumed = false;

  return (
    event: Pick<WheelEvent, "deltaX" | "deltaY" | "timeStamp">,
    contentOwnsGesture: boolean,
  ): 1 | -1 | null => {
    if (event.timeStamp - lastAt > GESTURE_IDLE_MS) {
      x = 0;
      y = 0;
      consumed = false;
    }
    // Every event extends the burst, even after navigating or while a child
    // handles scrolling. Crossing out of a canvas cannot turn its tail into
    // a page swipe, and changing items must not reset this state.
    lastAt = event.timeStamp;
    consumed ||= contentOwnsGesture;
    if (consumed) return null;

    x += event.deltaX;
    y += event.deltaY;
    if (Math.max(Math.abs(x), Math.abs(y)) < DIRECTION_THRESHOLD) return null;
    if (Math.abs(x) <= Math.abs(y) * 1.5) {
      consumed = true; // Vertical/diagonal scrolling owns the whole gesture.
      return null;
    }
    if (Math.abs(x) < NAVIGATION_THRESHOLD) return null;
    consumed = true;
    return x > 0 ? 1 : -1;
  };
}

function contentOwnsWheel(event: WheelEvent, root: HTMLElement): boolean {
  if (
    event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey
  ) {
    return true;
  }
  for (const target of event.composedPath()) {
    if (!(target instanceof Element)) continue;
    if (target.matches(
      "[data-reader-scroll], input, textarea, select, [contenteditable]:not([contenteditable='false'])",
    )) {
      return true;
    }
    // Keep native horizontal scroll areas in charge even at either edge.
    // Testing available scroll distance would turn an overscroll into nav.
    if (target.scrollWidth > target.clientWidth + 1) {
      const overflow = getComputedStyle(target).overflowX;
      if (overflow === "auto" || overflow === "scroll") return true;
    }
    if (target === root) break;
  }
  return false;
}

/** Install once on the persistent reader, rather than on each keyed item. */
export function listenForReaderSwipes(
  root: HTMLElement,
  step: (direction: 1 | -1) => void,
) {
  const track = createReaderSwipeTracker();
  const onWheel = (event: WheelEvent) => {
    const unitX = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? root.clientWidth : 1;
    const unitY = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? root.clientHeight : 1;
    const direction = track(
      {
        deltaX: event.deltaX * unitX,
        deltaY: event.deltaY * unitY,
        timeStamp: event.timeStamp,
      },
      contentOwnsWheel(event, root),
    );
    if (direction !== null) step(direction);
  };
  root.addEventListener("wheel", onWheel, { passive: true });
  return () => root.removeEventListener("wheel", onWheel);
}
