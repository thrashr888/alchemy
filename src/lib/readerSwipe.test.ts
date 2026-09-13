import { describe, expect, it } from "vitest";
import { createReaderSwipeTracker } from "./readerSwipe";

const wheel = (timeStamp: number, deltaX: number, deltaY = 0) => ({ timeStamp, deltaX, deltaY });

describe("reader swipe gestures", () => {
  it("waits for a deliberate horizontal swipe and navigates in either direction", () => {
    const track = createReaderSwipeTracker();
    expect(track(wheel(0, 30), false)).toBeNull();
    expect(track(wheel(16, 60), false)).toBe(1);
    expect(track(wheel(500, -90), false)).toBe(-1);
  });

  it("never re-arms on momentum spikes, direction changes, or long tails", () => {
    const track = createReaderSwipeTracker();
    expect(track(wheel(0, 100), false)).toBe(1);
    for (const [index, delta] of [50, 12, 2, 60, 150, -120, 1, 100].entries()) {
      expect(track(wheel((index + 1) * 200, delta), false)).toBeNull();
    }
    expect(track(wheel(2000, 100), false)).toBe(1);
  });

  it("vertical momentum extends the navigation lock", () => {
    const track = createReaderSwipeTracker();
    expect(track(wheel(0, 100), false)).toBe(1);
    expect(track(wheel(250, 0, 10), false)).toBeNull();
    expect(track(wheel(500, 100), false)).toBeNull();
    expect(track(wheel(900, 100), false)).toBe(1);
  });

  it("keeps vertical or diagonal scrolling in the content for the entire burst", () => {
    for (const start of [wheel(0, 2, 40), wheel(0, 70, 60)]) {
      const track = createReaderSwipeTracker();
      expect(track(start, false)).toBeNull();
      expect(track(wheel(16, 140), false)).toBeNull();
      expect(track(wheel(500, 100), false)).toBe(1);
    }
  });

  it("does not turn a canvas, editor or modifier gesture into nav after leaving it", () => {
    const track = createReaderSwipeTracker();
    expect(track(wheel(0, 100), true)).toBeNull();
    expect(track(wheel(16, 150), false)).toBeNull();
    expect(track(wheel(250, 20), false)).toBeNull();
    expect(track(wheel(650, 100), false)).toBe(1);
  });

  it("discards accumulated navigation travel when content takes ownership", () => {
    const track = createReaderSwipeTracker();
    expect(track(wheel(0, 70), false)).toBeNull();
    expect(track(wheel(16, 30), true)).toBeNull();
    expect(track(wheel(32, 100), false)).toBeNull();
    expect(track(wheel(500, 30), false)).toBeNull();
  });
});
