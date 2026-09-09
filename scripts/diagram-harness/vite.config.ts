import { defineConfig } from "vite";
import path from "node:path";

// The architecture-diagram harness (docs/RFC-diagrams.md): the real engine
// — placement, resolve, the eraser render frame — against hand-written
// documents, outside the app. Rooted at the repo so `/diagram-frame.html`,
// `/src/...`, and the vendored assets resolve exactly as they do in Vite's
// app build; only the port and the page differ.
//
//   pnpm exec vite --config scripts/diagram-harness/vite.config.ts
//   open http://127.0.0.1:8792/scripts/diagram-harness/
const root = path.resolve(__dirname, "../..");

export default defineConfig({
  root,
  resolve: {
    alias: [
      { find: "@", replacement: path.resolve(root, "./src") },
      {
        find: /^(path|fs|url|source-map-js)$/,
        replacement: path.resolve(root, "./src/lib/nodeShims.ts"),
      },
    ],
  },
  define: {
    "process.env": "{}",
  },
  server: {
    port: 8792,
    strictPort: true,
  },
});
