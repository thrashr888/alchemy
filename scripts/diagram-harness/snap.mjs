#!/usr/bin/env node
// Screenshot every harness sample into docs/images/diagrams-<name>.png.
// Needs the harness server (pnpm exec vite --config scripts/diagram-harness/vite.config.ts)
// and a local Chrome; playwright-core comes with @eraserlabs/diagrams.
//
//   node scripts/diagram-harness/snap.mjs [http://localhost:8792]
import { createRequire } from "node:module";
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../..");
const out = path.join(root, "docs/images");
const origin = process.argv[2] ?? "http://localhost:8792";
const chrome =
  process.env.CHROMIUM_PATH ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

// playwright-core is @eraserlabs/diagrams' dependency, not ours: resolve it
// from there rather than adding a Chromium driver to the app's own tree.
const fromDiagrams = createRequire(
  createRequire(import.meta.url).resolve("@eraserlabs/diagrams/package.json"),
);
const { chromium } = fromDiagrams("playwright-core");

await mkdir(out, { recursive: true });
const browser = await chromium.launch({ executablePath: chrome });
const page = await browser.newPage({
  viewport: { width: 2300, height: 1400 },
  deviceScaleFactor: 2,
});
await page.goto(`${origin}/scripts/diagram-harness/`);
await page.waitForSelector('html[data-status="ok"]', { timeout: 30_000 });
// Fonts register asynchronously in the render frame; give the copied
// scene a beat to paint with them before the capture.
await page.waitForTimeout(500);

// Direct children only: Playwright locators pierce shadow roots, and the
// rendered scenes have sections of their own.
for (const section of await page.locator("main > section").all()) {
  const name = await section.getAttribute("id");
  const frame = section.locator(".frame");
  const file = path.join(out, `diagrams-${name}.png`);
  await frame.screenshot({ path: file, type: "png" });
  console.log(`wrote ${path.relative(root, file)}`);
}
await browser.close();
