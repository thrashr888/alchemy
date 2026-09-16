#!/usr/bin/env node
// Rasterize the Lucide icons that menu rows use into PNGs for native
// menus (src/components/ui.tsx, RowMenu). Run once, commit the output;
// rerun only when a menu gains an icon the set lacks:
//
//   node scripts/menu-icons.mjs            # scans src/components for `icon: <Name`
//   node scripts/menu-icons.mjs Pencil Rss # just these
//
// 32×32 (16 pt at 2×), the stroke in the menu's ink for each appearance —
// NSMenu gets no template flag through Tauri's menu API, so each icon is
// drawn twice. Needs rsvg-convert (brew install librsvg).
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import * as Lucide from "lucide-react";
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, writeFileSync, mkdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "src/assets/menu-icons");
const INKS = { light: "#000000", dark: "#ffffff" };
const SIZE = 32;

function scanNames() {
  const dir = join(ROOT, "src/components");
  const names = new Set();
  for (const f of readdirSync(dir)) {
    if (!f.endsWith(".tsx")) continue;
    const src = readFileSync(join(dir, f), "utf8");
    for (const m of src.matchAll(/icon:\s*<([A-Z][A-Za-z0-9]+)/g)) names.add(m[1]);
  }
  return [...names].sort();
}

const wanted = process.argv.length > 2 ? process.argv.slice(2) : scanNames();
mkdirSync(OUT, { recursive: true });
let done = 0;
const skipped = [];
for (const name of wanted) {
  const Component = Lucide[name];
  if (!Component || typeof Component === "function" && !Component.displayName) {
    skipped.push(name);
    continue;
  }
  for (const [appearance, ink] of Object.entries(INKS)) {
    let svg;
    try {
      svg = renderToStaticMarkup(
        createElement(Component, {
          size: SIZE,
          color: ink,
          strokeWidth: 2,
          strokeOpacity: 0.85,
          xmlns: "http://www.w3.org/2000/svg",
        }),
      );
    } catch {
      // `Icon` itself and other non-icon exports need props we don't have.
      skipped.push(name);
      break;
    }
    if (skipped[skipped.length - 1] === name) break;
    const png = execFileSync(
      "rsvg-convert",
      ["-w", String(SIZE), "-h", String(SIZE), "-f", "png"],
      { input: svg },
    );
    writeFileSync(join(OUT, `${name}-${appearance}.png`), png);
  }
  if (skipped[skipped.length - 1] !== name) done += 1;
}
console.log(`${done} icons × 2 appearances → ${OUT}`);
if (skipped.length) console.log(`skipped (not a Lucide export): ${skipped.join(", ")}`);
