import { describe, expect, it, vi } from "vitest";
import { EMPTY_PEEK, createUrlPeeker, peekIsEmpty } from "./urlPeek";
import type { UrlPeek } from "./types";

const peek = (title: string): UrlPeek => ({
  title,
  description: "",
  imageUrl: "",
  site: "ex.com",
});

/** A fetch whose resolution the test controls. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("createUrlPeeker", () => {
  it("fetches once per URL and answers from cache after that", async () => {
    const fetch = vi.fn(async (url: string) => peek(`T:${url}`));
    const peeker = createUrlPeeker(fetch);
    const first = vi.fn();
    peeker.watch("https://a", first);
    await flush();
    expect(first).toHaveBeenCalledWith(peek("T:https://a"));
    expect(peeker.cached("https://a")).toEqual(peek("T:https://a"));

    // Second hover: synchronous callback, no second fetch.
    const second = vi.fn();
    peeker.watch("https://a", second);
    expect(second).toHaveBeenCalledWith(peek("T:https://a"));
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("drops a result that lands after the pointer moved on", async () => {
    const slow = deferred<UrlPeek>();
    const fetch = vi.fn((url: string) =>
      url === "https://slow" ? slow.promise : Promise.resolve(peek("fast")),
    );
    const peeker = createUrlPeeker(fetch);
    const onSlow = vi.fn();
    const onFast = vi.fn();
    peeker.watch("https://slow", onSlow);
    peeker.unwatch("https://slow");
    peeker.watch("https://fast", onFast);
    await flush();
    slow.resolve(peek("late"));
    await flush();
    expect(onFast).toHaveBeenCalledWith(peek("fast"));
    // The stale answer never reaches the card, but it did fill the cache.
    expect(onSlow).not.toHaveBeenCalled();
    expect(peeker.cached("https://slow")).toEqual(peek("late"));
  });

  it("unwatch of another URL does not cancel the current one", async () => {
    const fetch = vi.fn(async () => peek("here"));
    const peeker = createUrlPeeker(fetch);
    const ready = vi.fn();
    peeker.watch("https://a", ready);
    peeker.unwatch("https://b"); // a leave from a row we already left
    await flush();
    expect(ready).toHaveBeenCalledWith(peek("here"));
  });

  it("keeps one fetch in flight per URL across repeated hovers", async () => {
    const d = deferred<UrlPeek>();
    const fetch = vi.fn(() => d.promise);
    const peeker = createUrlPeeker(fetch);
    const a = vi.fn();
    const b = vi.fn();
    peeker.watch("https://a", a);
    peeker.unwatch("https://a");
    peeker.watch("https://a", b);
    expect(fetch).toHaveBeenCalledTimes(1);
    d.resolve(peek("once"));
    await flush();
    expect(a).not.toHaveBeenCalled(); // superseded by the re-entry
    expect(b).toHaveBeenCalledTimes(1);
  });

  it("caches a failed fetch as the empty peek", async () => {
    const fetch = vi.fn(async () => {
      throw new Error("ipc down");
    });
    const peeker = createUrlPeeker(fetch);
    const ready = vi.fn();
    peeker.watch("https://a", ready);
    await flush();
    expect(ready).toHaveBeenCalledWith(EMPTY_PEEK);
    expect(peekIsEmpty(peeker.cached("https://a")!)).toBe(true);
    peeker.watch("https://a", ready);
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});
