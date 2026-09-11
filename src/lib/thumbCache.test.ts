import { describe, expect, it } from "vitest";
import { ThumbnailCache } from "./thumbCache";
describe("thumbnail memory budget", () => {
  it("evicts the least recently viewed image when bytes run out", () => {
    const cache = new ThumbnailCache(24, 8);
    cache.set("a", "1111"); cache.set("b", "2222"); cache.get("a"); cache.set("c", "3333");
    expect(cache.has("a")).toBe(true); expect(cache.has("b")).toBe(false);
    expect(cache.byteLength).toBe(20);
  });
  it("bounds empty results, replaces sizes, and refuses oversized images", () => {
    const cache = new ThumbnailCache(24, 2);
    cache.set("a", ""); cache.set("b", ""); cache.set("c", "");
    expect(cache.size).toBe(2); expect(cache.has("a")).toBe(false);
    cache.set("b", "123"); expect(cache.byteLength).toBe(10);
    cache.set("b", "x".repeat(40)); expect(cache.has("b")).toBe(false);
    cache.clear(); expect(cache.byteLength).toBe(0); expect(cache.size).toBe(0);
  });
});
