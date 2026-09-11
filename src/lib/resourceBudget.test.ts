import { afterEach, describe, expect, it, vi } from "vitest";
import { BoundedCache } from "./boundedCache";
import { coalescedRefresh } from "./coalescedRefresh";
import { toNoteSummary } from "./noteSummary";
import { visibleAnimation } from "./visibleAnimation";
import type { Note } from "./types";

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe("graph retention budgets", () => {
  it("evicts cold entries, including their keys, and rejects oversized values", () => {
    const cache = new BoundedCache<string>(3, 16, (key, value) => key.length + value.length);
    cache.set("a", "1111"); cache.set("b", "2222"); cache.set("c", "3333");
    cache.get("a"); cache.set("d", "4444");
    expect(cache.get("b")).toBeUndefined();
    expect(cache.get("a")).toBe("1111");
    cache.set("a", "x".repeat(20));
    expect(cache.get("a")).toBeUndefined();
    for (let i = 0; i < 1000; i++) cache.set(String(i), "1234");
    expect(cache.size).toBeLessThanOrEqual(3);
    expect(cache.totalWeight).toBeLessThanOrEqual(16);
  });
});

describe("note collection ownership", () => {
  it("strips bodies and prompts from generated notes without changing metadata", () => {
    const full: Note = { id: "n", notebookId: "nb", title: "Large report", kind: "report",
      content: "全文🙂".repeat(100000), prompt: "instructions".repeat(10000),
      status: "", origin: "auto", createdAt: 1, updatedAt: 2 };
    const row = toNoteSummary(full);
    expect(row).not.toHaveProperty("content");
    expect(row).not.toHaveProperty("prompt");
    expect(row.title).toBe(full.title);
    expect(toNoteSummary(row)).toBe(row);
    expect(full.content.length).toBeGreaterThan(100000);
    expect(JSON.stringify(row).length).toBeLessThan(200);
  });
});

describe("event refresh coalescing", () => {
  it("turns 100 events into one read", async () => {
    vi.useFakeTimers();
    const read = vi.fn(async () => {});
    const refresh = coalescedRefresh(read, vi.fn());
    for (let i = 0; i < 100; i++) refresh.request();
    await vi.advanceTimersByTimeAsync(150);
    expect(read).toHaveBeenCalledTimes(1);
    refresh.dispose();
  });
  it("drops an outdated response and performs one trailing read without overlap", async () => {
    vi.useFakeTimers();
    let resolve: () => void = () => {};
    let count = 0;
    const published: number[] = [];
    const read = vi.fn(async (current: () => boolean) => {
      const value = ++count;
      if (value === 1) await new Promise<void>((r) => { resolve = r; });
      if (current()) published.push(value);
    });
    const refresh = coalescedRefresh(read, vi.fn());
    refresh.request(); await vi.advanceTimersByTimeAsync(150);
    for (let i = 0; i < 100; i++) refresh.request();
    await vi.advanceTimersByTimeAsync(1500);
    expect(read).toHaveBeenCalledTimes(1);
    resolve(); await vi.advanceTimersByTimeAsync(150);
    expect(read).toHaveBeenCalledTimes(2);
    expect(published).toEqual([2]);
    refresh.dispose();
  });
  it("recovers after errors and discards work after disposal", async () => {
    vi.useFakeTimers();
    const errors = vi.fn();
    const read = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue(undefined);
    const refresh = coalescedRefresh(read, errors);
    refresh.request(); await vi.advanceTimersByTimeAsync(150);
    expect(errors).toHaveBeenCalledTimes(1);
    refresh.request(); await vi.advanceTimersByTimeAsync(150);
    expect(read).toHaveBeenCalledTimes(2);
    refresh.request(); refresh.dispose(); await vi.runAllTimersAsync();
    expect(read).toHaveBeenCalledTimes(2);
  });
});

describe("visible canvas ownership", () => {
  function setup(isStatic = false) {
    let intersection: (entries: { isIntersecting: boolean }[]) => void = () => {};
    let resizeObserved: () => void = () => {};
    let visibility: () => void = () => {};
    let frame: FrameRequestCallback | undefined;
    let cancelled = 0;
    const disconnected = vi.fn();
    const doc = { visibilityState: "visible", addEventListener: (_: string, fn: () => void) => { visibility = fn; }, removeEventListener: vi.fn() };
    vi.stubGlobal("document", doc);
    vi.stubGlobal("IntersectionObserver", class { constructor(cb: typeof intersection) { intersection = cb; } observe() {} disconnect = disconnected; });
    vi.stubGlobal("ResizeObserver", class { constructor(cb: typeof resizeObserved) { resizeObserved = cb; } observe() {} disconnect = disconnected; });
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => { frame = cb; return 1; });
    vi.stubGlobal("cancelAnimationFrame", () => { frame = undefined; cancelled++; });
    const resize = vi.fn(); const draw = vi.fn();
    const stop = visibleAnimation({} as HTMLCanvasElement, resize, draw, isStatic);
    return { draw, resize, stop, disconnected, doc,
      show: (yes: boolean) => intersection([{ isIntersecting: yes }]),
      tick: (now: number) => { const cb = frame; frame = undefined; cb?.(now); },
      resized: () => resizeObserved(), changed: () => visibility(),
      pending: () => !!frame, cancelled: () => cancelled };
  }
  it("has zero hidden frames, resumes after visibility, and avoids per-frame layout", () => {
    const x = setup();
    expect(x.pending()).toBe(false);
    x.show(true); x.tick(0); x.tick(40); x.tick(80);
    expect(x.draw).toHaveBeenCalledTimes(3);
    expect(x.resize).toHaveBeenCalledTimes(1);
    x.show(false); x.tick(120);
    expect(x.draw).toHaveBeenCalledTimes(3);
    expect(x.pending()).toBe(false);
    x.show(true); x.tick(160);
    x.doc.visibilityState = "hidden"; x.changed();
    expect(x.pending()).toBe(false);
    x.doc.visibilityState = "visible"; x.changed(); x.tick(200);
    expect(x.draw).toHaveBeenCalledTimes(5);
    x.stop(); x.resized(); x.show(true);
    expect(x.pending()).toBe(false);
    expect(x.disconnected).toHaveBeenCalledTimes(2);
  });
  it("redraws static/reduced-motion content only when visibility or size changes", () => {
    const x = setup(true);
    x.show(true); x.tick(0); x.tick(40);
    expect(x.draw).toHaveBeenCalledTimes(1);
    expect(x.pending()).toBe(false);
    x.resized(); x.tick(80);
    expect(x.draw).toHaveBeenCalledTimes(2);
    x.stop();
  });
});
