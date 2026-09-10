import { defineConfig } from "vitest/config";
import path from "node:path";
import { ajvShimAlias } from "./scripts/diagram-csp-alias";

// Pure-logic suite (contrast math, store logic) — no DOM needed, so the
// default "node" environment is fine and keeps this config minimal.
export default defineConfig({
  resolve: {
    alias: [
      { find: "@", replacement: path.resolve(__dirname, "./src") },
      // src/lib/diagramCsp.test.ts runs the eraser resolver with `Function`
      // and `eval` stubbed out, the way the release CSP behaves. The alias
      // only reaches the resolver's `ajv` import when Vite transforms the
      // package instead of Node loading it as-is — hence the inline below.
      ajvShimAlias(__dirname),
    ],
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
    server: {
      deps: {
        // The whole family: @eraserlabs/diagrams imports the resolver too,
        // and one native copy of it would compile with the real Ajv.
        inline: [/@eraserlabs\//],
      },
    },
  },
});
