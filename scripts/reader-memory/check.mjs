import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const { chromium } = createRequire(require.resolve('@eraserlabs/diagrams/package.json'))('playwright-core');
const browser = await chromium.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' });
const page = await browser.newPage({ viewport: { width: 1000, height: 900 } });
const cdp = await page.context().newCDPSession(page);
const errors = [];
page.on('pageerror', (error) => errors.push(error.message));
const read = async (label) => {
  await cdp.send('HeapProfiler.collectGarbage');
  return { label, heap: await cdp.send('Runtime.getHeapUsage'),
    images: await page.locator('img').count(), ...await page.evaluate(() => ({
      calls: window.readerMemory.calls.length, peak: window.readerMemory.peak,
      encodedBytes: [...document.images].reduce((sum, img) => sum + img.src.length, 0),
      decodedPixelBytes: [...document.images].reduce((sum, img) => sum + img.naturalWidth * img.naturalHeight * 4, 0),
    })) };
};
try {
  await page.goto(process.argv[2] ?? 'http://127.0.0.1:8794/scripts/reader-memory/index.html');
  await page.waitForFunction(() => document.images.length > 0 && [...document.images].every((i) => i.complete));
  const samples = [await read('opened')];
  const scroller = page.locator('.overflow-y-auto');
  for (let i = 0; i < 40; i++) {
    await scroller.evaluate((el, i) => { el.scrollTop = i * 970; }, i);
    await page.waitForTimeout(180);
  }
  await page.waitForTimeout(400);
  samples.push(await read('after-40-pages'));
  const baseline = process.argv.includes('--baseline');
  if (!baseline) {
    assert(samples.at(-1).images <= 4, 'only nearby bitmaps stay mounted');
    assert(samples.at(-1).peak <= 2, 'at most two rasterizations');
    await scroller.evaluate((el) => { el.scrollTop = 0; });
    await page.waitForFunction(() => document.querySelector('img[alt="first.pdf — page 1"]')?.complete);
    await page.screenshot({ path: 'docs/images/pdf-memory-bounded.png' });
    await page.evaluate(() => window.readerMemory.open('second.pdf'));
    await page.waitForFunction(() => document.querySelector('img[alt="second.pdf — page 1"]')?.complete);
    assert.equal(await page.locator('img[alt^="first.pdf"]').count(), 0);
    // Resizing regenerates visible pages at the new width.
    await page.locator('#root > div').evaluate((el) => { el.style.width = '600px'; });
    await page.waitForFunction(() => window.readerMemory.calls.some((call) => call.path === 'second.pdf' && call.width < 600));
    await page.evaluate(() => window.readerMemory.close());
    await page.waitForTimeout(300);
    const count = await page.evaluate(() => window.readerMemory.calls.length);
    await page.waitForTimeout(400);
    assert.equal(await page.evaluate(() => window.readerMemory.calls.length), count);
    assert.equal(await page.evaluate(() => window.readerMemory.observers), 0, 'all observers disconnect');
    samples.push(await read('closed'));
    assert.equal(samples.at(-1).images, 0);
  }
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ assertions: 'passed', samples }, null, 2));
} finally { await browser.close(); }
