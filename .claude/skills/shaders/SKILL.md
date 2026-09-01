---
name: shaders
description: Use when editing, adding, or reviewing a WebGL shader mode in src/components/DitherBackground.tsx (the notebook backdrop, one GLSL program with 17 theme-driven modes) or src/components/settings/TileShader.tsx (Activity tile washes), or when a theme in src/lib/themes.ts gets a new `shader` variant. Renders the real GLSL in a browser harness so the change is seen before it ships.
---

# Shaders — render before you ship

Both shader components are GLSL ES 1.0 inside `FRAG` template literals,
compiled by WebGL1 at runtime. That choice is deliberate (WKWebView on every
macOS, no WebGPU) — keep the WebGL1 constraints: no dynamic array indexing,
constant-bound loops, no `${}` in the literals (the harness refuses them).

Two facts drive the workflow:

- **One program serves every theme.** A GLSL error in the mode you're adding
  kills the backdrop for all 27 themes. Compile is the first gate.
- **Aesthetic misses are confident.** Writing a "bokeh city lights" field
  from the words alone produced an abstract plexus. The only reliable check
  is the rendered pixels next to a reference image.

## The loop

```bash
python3 scripts/shader-harness.py --serve    # http://127.0.0.1:8791/
```

or `preview_start` with the `shaders` launch config, which runs the same
command. The page is written once at start — after each `FRAG` edit rerun
`python3 scripts/shader-harness.py` (no flags) to regenerate it in place,
then reload.

1. **Ask for a reference image** when the request names a style. It carries
   more than the name does.
2. **Edit** the field function in the `.tsx`. New backdrop mode: add the
   `ShaderVariant` union member in `themes.ts`, the `SHADER_MODE` index, a
   `<name>Field(uv, glow)` function, and its branch in `main()`. Keep the
   shared dither, central glow, and transmutation ring — they are what make
   every mode read as the same element.
3. **Regenerate, reload, read status.** `<html data-status>` is `ok` or
   `fail`; the red banner (and `console.error`) carries the GLSL log with
   line numbers relative to the `FRAG` literal.
4. **Look.** The contact sheet (`/`) shows every mode with the theme that
   uses it. `?mode=<name>&theme=<id>` fills the viewport with one;
   `&t=<seconds>` freezes time for a deterministic screenshot;
   `&density=` / `&gain=` are the component's `density` / `intensity`
   props; `?shader=tile&mode=ember|sky|spark` (`&hour=`, `&series=a,b,c`)
   covers the tiles. Screenshot, compare with the reference, iterate.
5. **Check motion too** — drop `t` and watch a few seconds; the field should
   breathe, not scroll. Then check the app itself (`pnpm tauri dev`), since
   ANGLE-on-Metal in WKWebView is what ships.

## Harness internals worth knowing

- It regex-extracts `VERT`/`FRAG`/`SHADER_MODE` from the two components and
  each theme's `id`/`shader`/`background`/`primary` from `themes.ts`, so it
  needs no build step and no dependencies beyond Python 3.
- The sheet renders 22 cells through two shared offscreen WebGL contexts
  (opaque backdrop, alpha tiles) blitted into 2D canvases — browsers evict
  contexts past ~16, which is what a per-cell design hits.
- The Browser pane pauses `requestAnimationFrame` while hidden; front the
  tab before reading pixels or status.
