import { expect, it } from "vitest";
import { queuePdfPage } from "./pdfPageQueue";
it("limits rasterization and skips pages abandoned while queued", async () => {
  const finish: Array<(value: string) => void> = [];
  let calls = 0;
  const render = () => { calls++; return new Promise<string>((resolve) => finish.push(resolve)); };
  const first = queuePdfPage(render, new AbortController().signal);
  const second = queuePdfPage(render, new AbortController().signal);
  const stale = new AbortController();
  const third = queuePdfPage(render, stale.signal).catch((e: Error) => e.name);
  const fourth = queuePdfPage(render, new AbortController().signal);
  await Promise.resolve(); expect(calls).toBe(2);
  stale.abort(); expect(await third).toBe("AbortError");
  finish[0]("one"); expect(await first).toBe("one");
  await Promise.resolve(); await Promise.resolve(); expect(calls).toBe(3);
  finish[1]("two"); finish[2]("four");
  expect(await Promise.all([second, fourth])).toEqual(["two", "four"]);
});
it("recovers slots after failure and rejects pre-aborted work", async () => {
  await expect(queuePdfPage(() => Promise.reject(new Error("bad page")), new AbortController().signal)).rejects.toThrow("bad page");
  const abort = new AbortController(); abort.abort();
  await expect(queuePdfPage(() => Promise.resolve("unexpected"), abort.signal)).rejects.toMatchObject({ name: "AbortError" });
  expect(await queuePdfPage(() => Promise.resolve("next"), new AbortController().signal)).toBe("next");
});

it("skips a page abandoned just before its render starts", async () => {
  const abort = new AbortController();
  let called = false;
  const page = queuePdfPage(() => { called = true; return Promise.resolve("stale"); }, abort.signal);
  abort.abort();
  await expect(page).rejects.toMatchObject({ name: "AbortError" });
  expect(called).toBe(false);
});
