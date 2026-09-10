import path from "node:path";

/**
 * Vite alias that swaps `ajv` for the precompiled diagram validators
 * (docs/RFC-diagrams.md, "Content Security Policy"). `@eraserlabs/resolve`
 * compiles its JSON schemas with Ajv, and Ajv compiles with `new Function`,
 * which the release CSP forbids; `src/lib/diagramValidators.ts` serves the
 * validators `scripts/diagram-validators.mjs` compiled at build time
 * instead. The bare specifier only — nothing in the app's graph other than
 * the resolver imports ajv, and nothing imports its subpaths.
 *
 * Every Vite config that can load the resolver (the app, the diagram
 * harness, vitest) uses this, so dev, release, and the tests run the same
 * code.
 */
export function ajvShimAlias(root: string) {
  return { find: /^ajv$/, replacement: path.resolve(root, "src/lib/diagramValidators.ts") };
}
