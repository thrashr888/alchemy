import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  // The production bundle has many Shiki language chunks. Gzip-size
  // reporting is useful for release analysis but adds avoidable work to every
  // local and CI build.
  build: {
    reportCompressedSize: false,
    rollupOptions: {
      input: {
        main: path.resolve(__dirname, "index.html"),
        // The eraser render frame (docs/RFC-diagrams.md): its own page so
        // the engine's global stylesheet never reaches the app document.
        diagramFrame: path.resolve(__dirname, "diagram-frame.html"),
      },
    },
  },

  // @eraserlabs/layout reads process.env unguarded (dev warnings, debug
  // dumps); an empty object keeps those reads inert in the WebView.
  define: {
    "process.env": "{}",
  },

  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
