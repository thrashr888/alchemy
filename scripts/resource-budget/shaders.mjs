import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const { chromium } = createRequire(require.resolve('@eraserlabs/diagrams/package.json'))('playwright-core');
const browser = await chromium.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' });
try {
  const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('http://127.0.0.1:8791/');
  await page.waitForFunction(() => document.documentElement.dataset.status === 'ok');
  await page.screenshot({ path: '/tmp/alchemy-resource-shader-sheet.png', fullPage: true });
  await page.goto('http://127.0.0.1:8791/?mode=mist&theme=dracula');
  await page.waitForFunction(() => document.documentElement.dataset.status === 'ok');
  await page.waitForTimeout(400);
  const before = await page.screenshot();
  await page.waitForTimeout(600);
  const after = await page.screenshot();
  assert(!before.equals(after), 'shader motion changes rendered pixels');
  assert.deepEqual(errors, []);
  console.log('All shader modes compile; contact sheet captured; motion changes pixels.');
} finally { await browser.close(); }
