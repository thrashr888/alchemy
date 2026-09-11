#!/usr/bin/env node
// With Vite running, compare the same workload before/after a renderer change:
// node scripts/diagram-harness/memory.mjs http://127.0.0.1:8794
// Chromium heap/DOM measurements are not the installed WKWebView app footprint.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const { chromium } = createRequire(require.resolve("@eraserlabs/diagrams/package.json"))("playwright-core");
const browser = await chromium.launch({
  executablePath: process.env.CHROMIUM_PATH ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
});
const page = await browser.newPage({ viewport: { width: 1400, height: 900 }, deviceScaleFactor: 2 });
const cdp = await page.context().newCDPSession(page);
const samples = [];
const snapshot = async (label) => {
  await cdp.send("HeapProfiler.collectGarbage");
  samples.push({ label, heap: await cdp.send("Runtime.getHeapUsage"), dom: await cdp.send("Memory.getDOMCounters") });
};
try {
  await page.goto(`${process.argv[2] ?? "http://127.0.0.1:8792"}/scripts/diagram-harness/memory.html`);
  await page.waitForFunction(() => window.diagramMemory);
  await snapshot("before");
  // Cancelled reader items must not boot a frame or render their scenes.
  const cancelled = await page.evaluate(() => window.diagramMemory.cancelledBurst());
  assert(cancelled.every((status) => status === "AbortError"));
  assert.equal(await page.locator("iframe").count(), 0);
  await page.route("**/diagram-frame.html", (route) => route.fulfill({ contentType: "text/html", body: "<!doctype html><html></html>" }));
  const failed = await page.evaluate(() => window.diagramMemory.draw(0).then(() => "unexpected success", (error) => error.message));
  assert.match(failed, /did not boot/);
  assert.equal(await page.locator("iframe").count(), 0, "failed boot must release its frame");
  await page.unroute("**/diagram-frame.html");
  const timings = [];
  for (let i = 0; i < 20; i++) {
    timings.push(await page.evaluate((i) => window.diagramMemory.draw(i), i));
    assert.equal(await page.locator("iframe").count(), 1);
    assert.equal(await page.evaluate(() => document.querySelector("iframe").contentDocument.querySelectorAll("#eraser-scene").length), 0);
    if (i === 0 || i === 9 || i === 19) await snapshot(`draw-${i + 1}`);
  }
  // Releasing the measurement frame must not change the visible document.
  const before = await page.locator("#view").screenshot();
  await page.waitForFunction(() => !document.querySelector("iframe"), undefined, { timeout: 8000 });
  assert.deepEqual(await page.locator("#view").screenshot(), before);
  await page.evaluate(() => window.diagramMemory.clearScene());
  await snapshot("idle");
  assert.equal(samples.at(-1).dom.documents, 1);
  // A new diagram after idle boots successfully and still cleans up.
  await page.evaluate(() => window.diagramMemory.draw(0));
  assert.equal(await page.locator("iframe").count(), 1);
  const whileWarm = await page.evaluate(() => window.diagramMemory.cancelledBurst());
  assert(whileWarm.every((status) => status === "AbortError"));
  await page.evaluate(() => window.diagramMemory.draw(1));
  await page.waitForFunction(() => !document.querySelector("iframe"), undefined, { timeout: 8000 });
  console.log(JSON.stringify({ assertions: "passed", samples, timings }, null, 2));
} finally {
  await browser.close();
}
