import { afterEach, describe, expect, it, vi } from "vitest";
import { afterStartupPaint, initializeOnce } from "./startup";

afterEach(() => vi.unstubAllGlobals());

describe("startup initialization", () => {
  it("keeps repeated callers pending until the restored notebook finishes loading", async () => {
    let finish!: () => void;
    const load = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    const init = initializeOnce(load);
    const ready = vi.fn();
    const first = init();
    const second = init();
    void second.then(ready);
    await Promise.resolve();
    expect(first).toBe(second);
    expect(load).toHaveBeenCalledTimes(1);
    expect(ready).not.toHaveBeenCalled();
    finish();
    await first;
    expect(ready).toHaveBeenCalledTimes(1);
    expect(init()).toBe(first);
  });

  it("does not report a successful boot to another caller after initialization fails", async () => {
    const load = vi.fn(async () => { throw new Error("restore failed"); });
    const init = initializeOnce(load);
    await expect(init()).rejects.toThrow("restore failed");
    await expect(init()).rejects.toThrow("restore failed");
    expect(load).toHaveBeenCalledTimes(1);
  });
});

function frames() {
  const pending = new Map<number, FrameRequestCallback>();
  let id = 0;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    pending.set(++id, callback);
    return id;
  });
  vi.stubGlobal("cancelAnimationFrame", (handle: number) => pending.delete(handle));
  return () => {
    const callbacks = [...pending.values()];
    pending.clear();
    callbacks.forEach((callback) => callback(0));
  };
}

describe("startup ready view paint", () => {
  it("reports only after two frames of the committed ready view", () => {
    const paint = frames();
    const report = vi.fn();
    afterStartupPaint(report);
    expect(report).not.toHaveBeenCalled();
    paint();
    expect(report).not.toHaveBeenCalled();
    paint();
    expect(report).toHaveBeenCalledTimes(1);
  });

  it.each([0, 1])("does not report a view removed after %i frames", (elapsed) => {
    const paint = frames();
    const report = vi.fn();
    const cancel = afterStartupPaint(report);
    if (elapsed) paint();
    cancel();
    paint();
    paint();
    expect(report).not.toHaveBeenCalled();
  });
});
