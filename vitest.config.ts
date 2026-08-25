import { defineConfig } from "vitest/config";
import path from "node:path";

// Pure-logic suite (contrast math, store logic) — no DOM needed, so the
// default "node" environment is fine and keeps this config minimal.
export default defineConfig({
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
