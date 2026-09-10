#!/usr/bin/env node
// Fail when the built front-end would evaluate code at runtime (docs/RFC-diagrams.md,
// "Content Security Policy"). The release CSP is `script-src 'self'`; a chunk
// that reaches `new Function` or `eval` works in `pnpm tauri dev` (Vite's page
// carries no CSP) and dies in the signed bundle — which is how v0.59.0 shipped
// with diagrams that never rendered. Ajv's compiler is the known offender and
// is kept out by src/lib/diagramValidators.ts; this catches its return, and
// any other dependency that brings a compiler along.
//
//   pnpm build && pnpm run diagram:check-csp
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const assets = path.join(root, "dist/assets");

const PATTERNS = [
  { name: "Ajv schema compiler", re: /Error compiling schema/ },
  { name: "new Function", re: /\bnew Function\s*\(/ },
  { name: "direct eval", re: /(^|[^\w$.])eval\s*\(/ },
];

let chunks;
try {
  chunks = readdirSync(assets).filter((f) => f.endsWith(".js"));
} catch {
  console.error(`diagram:check-csp: no ${path.relative(root, assets)} — run pnpm build first`);
  process.exit(2);
}

const hits = [];
for (const chunk of chunks) {
  const text = readFileSync(path.join(assets, chunk), "utf8");
  for (const { name, re } of PATTERNS) {
    if (re.test(text)) hits.push(`${chunk}: ${name}`);
  }
}

if (hits.length > 0) {
  console.error("diagram:check-csp: the bundle evaluates code at runtime, which script-src 'self' forbids:");
  for (const hit of hits) console.error(`  ${hit}`);
  process.exit(1);
}
console.log(`diagram:check-csp: ${chunks.length} chunks, none evaluate code at runtime`);
