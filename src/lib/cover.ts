import { useCallback, useSyncExternalStore } from "react";
import { buildProgram } from "@/components/DitherBackground";
import { api } from "@/lib/api";

/**
 * Procedural covers for Home's notebook cards: the backdrop shader's mist
 * field, sampled at a window picked by the notebook id and washed in the
 * notebook's color. No network, no model, stable across launches.
 *
 * A shelf holds 30+ cards and WebKit evicts live WebGL contexts past ~16, so
 * the cards never own a canvas. One offscreen context renders each cover once
 * at thumb size, reads it back as a PNG with the field as ALPHA (the wash is
 * translucent, so the card's own surface, hover and selection tint still show
 * through), and caches the data URL by (id, color, scheme, dpr). Work is
 * queued and run in idle slices; until a raster lands the card keeps its
 * plain surface. Covers are stills, so reduced motion has nothing to stop.
 */

/** The thumb's box in CSS px (HomeView's `w-[212px]` x `h-[140px]`). */
export const COVER_W = 212;
export const COVER_H = 140;

/** Peak wash opacity per scheme, at the bottom edge. Chosen against the text
 *  tokens: even the strongest pixel in the zone where the name and titles sit
 *  (peak x fade, ~0.7 of it) costs the muted/subtle tokens under a point of
 *  contrast in every theme, and the dither averages far lower than that. A
 *  bright tint lifts a dark surface more than a dark tint dents a light one,
 *  so dark gets the smaller number. */
const PEAK_ALPHA = { dark: 0.3, light: 0.26 } as const;
/** The wash thins toward the top, where the name and source titles sit. */
const TOP_FADE = 0.35;
const CACHE_LIMIT = 160;
/** Card colors are stored hex; anything else falls back to this neutral. */
const FALLBACK_RGB: [number, number, number] = [138, 147, 166];

type Scheme = "dark" | "light";

/** How a cover is drawn. `mist` is the shader field. `dither` and `ascii`
 *  start from a stock photo seeded by the notebook (fetched by Rust) and
 *  reduce it to the notebook's color: an ordered 1-bit dither, or a grid of
 *  characters by brightness. TRIAL: `localStorage.coverStyle` picks one for
 *  the whole shelf, or `mix` (the default) spreads the three by id. */
export type CoverStyle = "mist" | "dither" | "ascii";
export const COVER_STYLES: readonly CoverStyle[] = ["mist", "dither", "ascii"];
export const COVER_STYLE_LABEL: Record<CoverStyle, string> = {
  mist: "Mist",
  dither: "Dither",
  ascii: "ASCII",
};

/** The style and picture for a notebook. A stored choice
 *  ("<style>:<seed>", `Notebook.cover`) wins; otherwise the style is spread
 *  across the shelf by id (or fixed by `localStorage.coverStyle`) and the
 *  picture is the id's own. */
export function coverChoice(
  id: string,
  cover?: string,
): { style: CoverStyle; seed: string } {
  const stored = parseCover(cover);
  if (stored) return stored;
  let pick = "mix";
  try {
    pick = localStorage.getItem("coverStyle") || "mix";
  } catch {
    // Private mode: the default still works.
  }
  const style = (COVER_STYLES as readonly string[]).includes(pick)
    ? (pick as CoverStyle)
    : COVER_STYLES[hash32(id + "|style") % COVER_STYLES.length];
  return { style, seed: id };
}

export function parseCover(
  cover: string | undefined,
): { style: CoverStyle; seed: string } | null {
  if (!cover) return null;
  const at = cover.indexOf(":");
  if (at <= 0) return null;
  const style = cover.slice(0, at);
  const seed = cover.slice(at + 1);
  if (!(COVER_STYLES as readonly string[]).includes(style) || !seed) return null;
  return { style: style as CoverStyle, seed };
}

interface Request {
  key: string;
  /** The picture's name: the notebook id, or a seed the picker chose. */
  id: string;
  color: string;
  scheme: Scheme;
  dpr: number;
  style: CoverStyle;
  /** The box in CSS px: the card thumb, or the notebook page's band. */
  w: number;
  h: number;
}

const cache = new Map<string, string>();
const waiting = new Map<string, Set<() => void>>();
const requests = new Map<string, Request>();
const queue: string[] = [];
let scheduled = false;

// ---- scheme ---------------------------------------------------------------

const schemeListeners = new Set<() => void>();
let schemeObserver: MutationObserver | null = null;

function subscribeScheme(cb: () => void) {
  schemeListeners.add(cb);
  if (!schemeObserver) {
    schemeObserver = new MutationObserver(() =>
      schemeListeners.forEach((l) => l()),
    );
    schemeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-scheme"],
    });
  }
  return () => {
    schemeListeners.delete(cb);
    if (schemeListeners.size === 0) {
      schemeObserver?.disconnect();
      schemeObserver = null;
    }
  };
}

const getScheme = (): Scheme =>
  document.documentElement.dataset.scheme === "light" ? "light" : "dark";

// ---- seed -----------------------------------------------------------------

/** FNV-1a: same id, same number, every launch. */
function hash32(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** A point at least 2.5 uv units from the origin, in a direction and distance
 *  taken from the id: far enough that the shader's central glow and ring (both
 *  inside r ~ 1.2) miss the card, leaving the bare mist. */
export function coverSeed(id: string): [number, number] {
  const h = hash32(id);
  const angle = ((h & 0xffff) / 0xffff) * Math.PI * 2;
  const radius = 2.5 + ((h >>> 16) / 0xffff) * 6;
  return [Math.cos(angle) * radius, Math.sin(angle) * radius];
}

function parseHex(hex: string): [number, number, number] {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return FALLBACK_RGB;
  const n = parseInt(m[1], 16);
  return [n >> 16, (n >> 8) & 255, n & 255];
}

// ---- renderer -------------------------------------------------------------

interface Gpu {
  canvas: HTMLCanvasElement;
  gl: WebGLRenderingContext;
  program: WebGLProgram;
  uRes: WebGLUniformLocation | null;
  uSeed: WebGLUniformLocation | null;
}

let gpu: Gpu | null = null;
let gpuFailed = false;
let out: HTMLCanvasElement | null = null;

function getGpu(): Gpu | null {
  if (gpu) return gpu;
  if (gpuFailed) return null;
  const canvas = document.createElement("canvas");
  const gl = canvas.getContext("webgl", {
    antialias: false,
    alpha: false,
    powerPreference: "low-power",
  }) as WebGLRenderingContext | null;
  const program = gl && buildProgram(gl);
  if (!gl || !program) {
    gpuFailed = true;
    return null;
  }
  gl.useProgram(program);
  gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
  gl.bufferData(
    gl.ARRAY_BUFFER,
    new Float32Array([-1, -1, 3, -1, -1, 3]),
    gl.STATIC_DRAW,
  );
  const loc = gl.getAttribLocation(program, "a_pos");
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  // Luminance out: black ground, white tint. Mist's dither only reaches the
  // 0.6 level, so gain 7.6 (q * 0.22 * 7.6 ~ 1.67 q) lands that on 1.0.
  gl.uniform1f(gl.getUniformLocation(program, "u_mode"), 0);
  gl.uniform1f(gl.getUniformLocation(program, "u_time"), 0);
  gl.uniform1f(gl.getUniformLocation(program, "u_gain"), 7.6);
  gl.uniform1f(gl.getUniformLocation(program, "u_density"), 0.5);
  gl.uniform3f(gl.getUniformLocation(program, "u_tint"), 1, 1, 1);
  gl.uniform3f(gl.getUniformLocation(program, "u_bg"), 0, 0, 0);
  // The browser may take the context back (memory pressure); rebuild lazily.
  canvas.addEventListener("webglcontextlost", (e) => {
    e.preventDefault();
    gpu = null;
  });
  gpu = {
    canvas,
    gl,
    program,
    uRes: gl.getUniformLocation(program, "u_res"),
    uSeed: gl.getUniformLocation(program, "u_seed"),
  };
  return gpu;
}

function render(req: Request): string | null {
  const g = getGpu();
  if (!g) return null;
  const { canvas, gl } = g;
  const w = Math.round(req.w * req.dpr);
  const h = Math.round(req.h * req.dpr);
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  gl.viewport(0, 0, w, h);
  gl.uniform2f(g.uRes, w, h);
  const [sx, sy] = coverSeed(req.id);
  gl.uniform2f(g.uSeed, sx, sy);
  gl.drawArrays(gl.TRIANGLES, 0, 3);
  const px = new Uint8Array(w * h * 4);
  gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, px);
  if (gl.isContextLost()) return null;

  out ??= document.createElement("canvas");
  out.width = w;
  out.height = h;
  const ctx = out.getContext("2d");
  if (!ctx) return null;
  const img = ctx.createImageData(w, h);
  const [r, gr, b] = parseHex(req.color);
  // Some windows of the field are sparse. Scale each so its 99th-percentile
  // pixel lands on the peak: every card gets the same presence, and the cap
  // above stays the true ceiling.
  const hist = new Uint32Array(256);
  for (let i = 0; i < px.length; i += 4) hist[px[i]]++;
  let seen = 0;
  let p99 = 255;
  for (let v = 255; v >= 0; v--) {
    seen += hist[v];
    if (seen > w * h * 0.01) {
      p99 = v;
      break;
    }
  }
  const ref = Math.max(p99, 96);
  const peak = PEAK_ALPHA[req.scheme] * 255;
  for (let y = 0; y < h; y++) {
    // GL rows run bottom-up; the fade runs top (thin) to bottom (full).
    const src = (h - 1 - y) * w * 4;
    const dst = y * w * 4;
    const fade = TOP_FADE + (1 - TOP_FADE) * (y / (h - 1));
    for (let x = 0; x < w; x++) {
      const o = dst + x * 4;
      img.data[o] = r;
      img.data[o + 1] = gr;
      img.data[o + 2] = b;
      img.data[o + 3] = Math.min(1, px[src + x * 4] / ref) * peak * fade;
    }
  }
  ctx.putImageData(img, 0, 0);
  return out.toDataURL("image/png");
}

// ---- photo styles ----------------------------------------------------------

/** Peak opacity of the drawn marks per style and scheme: a dot or a glyph
 *  is a hard edge where the mist was a haze, so it can afford to be dimmer
 *  per pixel and still read as a picture. The same top fade applies. */
const DOT_ALPHA = { dark: 0.55, light: 0.45 } as const;
const GLYPH_ALPHA = { dark: 0.75, light: 0.6 } as const;
/** 8x8 Bayer threshold matrix, the classic ordered dither. */
const BAYER8 = [
  [0, 32, 8, 40, 2, 34, 10, 42],
  [48, 16, 56, 24, 50, 18, 58, 26],
  [12, 44, 4, 36, 14, 46, 6, 38],
  [60, 28, 52, 20, 62, 30, 54, 22],
  [3, 35, 11, 43, 1, 33, 9, 41],
  [51, 19, 59, 27, 49, 17, 57, 25],
  [15, 47, 7, 39, 13, 45, 5, 37],
  [63, 31, 55, 23, 61, 29, 53, 21],
];
const ASCII_RAMP = " .:-=+*#%@";

const photos = new Map<string, Promise<HTMLImageElement | null>>();

/** The seeded photo for a notebook, decoded, or null when it cannot be
 *  fetched (offline) — the caller then falls back to the mist. */
function photoFor(id: string, w: number, h: number): Promise<HTMLImageElement | null> {
  const key = `${id}|${w}|${h}`;
  let p = photos.get(key);
  if (!p) {
    p = api
      .coverPhoto(id, w, h)
      .then(
        (url) =>
          new Promise<HTMLImageElement | null>((resolve) => {
            const img = new Image();
            img.onload = () => resolve(img);
            img.onerror = () => resolve(null);
            img.src = url;
          }),
      )
      .catch(() => null);
    photos.set(key, p);
  }
  return p;
}

/** Luminance of the photo at cover size, one byte per pixel, plus the
 *  canvas it was drawn on (reused for the output). */
function luminance(img: HTMLImageElement, w: number, h: number) {
  out ??= document.createElement("canvas");
  out.width = w;
  out.height = h;
  const ctx = out.getContext("2d");
  if (!ctx) return null;
  ctx.drawImage(img, 0, 0, w, h);
  const src = ctx.getImageData(0, 0, w, h).data;
  const lum = new Uint8ClampedArray(w * h);
  for (let i = 0, j = 0; i < src.length; i += 4, j++) {
    lum[j] = 0.2126 * src[i] + 0.7152 * src[i + 1] + 0.0722 * src[i + 2];
  }
  return { ctx, lum };
}

function renderDither(img: HTMLImageElement, req: Request): string | null {
  const w = Math.round(req.w * req.dpr);
  const h = Math.round(req.h * req.dpr);
  const l = luminance(img, w, h);
  if (!l) return null;
  const { ctx, lum } = l;
  const [r, g, b] = parseHex(req.color);
  const peak = DOT_ALPHA[req.scheme] * 255;
  // Dark surface: the light parts of the picture become dots of color.
  // Light surface: the dark parts do, so the picture still reads positive.
  const flip = req.scheme === "light";
  // Dots of 1.5 CSS px, not device pixels: at 2x a one-pixel dither reads
  // as a soft photo, and the point of the style is that it reads as dots.
  const c = Math.max(1, Math.round(1.5 * req.dpr));
  const outImg = ctx.createImageData(w, h);
  for (let cy = 0; cy * c < h; cy++) {
    const y0 = cy * c;
    const sy = Math.min(h - 1, y0 + (c >> 1));
    const fade = TOP_FADE + (1 - TOP_FADE) * (y0 / (h - 1));
    const row = BAYER8[cy & 7];
    for (let cx = 0; cx * c < w; cx++) {
      const x0 = cx * c;
      const sx = Math.min(w - 1, x0 + (c >> 1));
      let v = lum[sy * w + sx] / 255;
      if (flip) v = 1 - v;
      if (!(v > (row[cx & 7] + 0.5) / 64)) continue;
      const a = peak * fade;
      for (let y = y0; y < Math.min(h, y0 + c); y++) {
        for (let x = x0; x < Math.min(w, x0 + c); x++) {
          const o = (y * w + x) * 4;
          outImg.data[o] = r;
          outImg.data[o + 1] = g;
          outImg.data[o + 2] = b;
          outImg.data[o + 3] = a;
        }
      }
    }
  }
  ctx.putImageData(outImg, 0, 0);
  return out!.toDataURL("image/png");
}

function renderAscii(img: HTMLImageElement, req: Request): string | null {
  const w = Math.round(req.w * req.dpr);
  const h = Math.round(req.h * req.dpr);
  const cw = Math.round(6 * req.dpr);
  const ch = Math.round(10 * req.dpr);
  const cols = Math.floor(w / cw);
  const rows = Math.floor(h / ch);
  const l = luminance(img, cols, rows);
  if (!l) return null;
  const { lum } = l;
  // The luminance canvas was cols x rows; redraw the output at full size.
  out!.width = w;
  out!.height = h;
  const ctx = out!.getContext("2d");
  if (!ctx) return null;
  ctx.clearRect(0, 0, w, h);
  ctx.font = `${Math.round(9 * req.dpr)}px ui-monospace, Menlo, monospace`;
  ctx.textBaseline = "top";
  const [r, g, b] = parseHex(req.color);
  const flip = req.scheme === "light";
  const peak = GLYPH_ALPHA[req.scheme];
  for (let y = 0; y < rows; y++) {
    const fade = TOP_FADE + (1 - TOP_FADE) * (y / Math.max(1, rows - 1));
    ctx.fillStyle = `rgba(${r},${g},${b},${(peak * fade).toFixed(3)})`;
    let line = "";
    for (let x = 0; x < cols; x++) {
      let v = lum[y * cols + x] / 255;
      if (flip) v = 1 - v;
      const i = Math.min(ASCII_RAMP.length - 1, Math.floor(v * ASCII_RAMP.length));
      line += ASCII_RAMP[i];
    }
    // Monospace: one fillText per row keeps the glyph grid exact.
    for (let x = 0; x < cols; x++) {
      if (line[x] !== " ") ctx.fillText(line[x], x * cw, y * ch);
    }
  }
  return out!.toDataURL("image/png");
}

/** The photo styles wait on a fetch, so they leave the idle slice and land
 *  on their own; a failed fetch draws the mist instead so no card stays
 *  bare for want of a network. */
async function renderPhotoStyle(req: Request) {
  let url: string | null = null;
  try {
    const img = await photoFor(
      req.id,
      Math.round(req.w * req.dpr),
      Math.round(req.h * req.dpr),
    );
    if (cache.has(req.key)) return;
    url = img
      ? req.style === "dither"
        ? renderDither(img, req)
        : renderAscii(img, req)
      : null;
    if (!url) url = render(req);
  } catch {
    // Fall through to the plain card.
  }
  if (url) store(req.key, url);
  waiting.get(req.key)?.forEach((l) => l());
}

// ---- queue ----------------------------------------------------------------

function store(key: string, url: string) {
  cache.set(key, url);
  if (cache.size > CACHE_LIMIT) {
    // Insertion order is recency order (hits re-insert); drop the oldest.
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
}

function slice() {
  scheduled = false;
  const t0 = performance.now();
  while (queue.length > 0) {
    const key = queue.shift()!;
    const req = requests.get(key);
    requests.delete(key);
    if (!req || cache.has(key)) continue;
    if (req.style !== "mist") {
      void renderPhotoStyle(req);
      continue;
    }
    let url: string | null = null;
    try {
      url = render(req);
    } catch {
      // A failed cover is a plain card, never an error the user sees.
    }
    if (url) store(key, url);
    waiting.get(key)?.forEach((l) => l());
    if (performance.now() - t0 > 8) break;
  }
  if (queue.length > 0) schedule();
}

function schedule() {
  if (scheduled) return;
  scheduled = true;
  // Idle time where the engine offers it (WKWebView may not); otherwise a
  // macrotask, which still yields to input and paint between slices.
  if ("requestIdleCallback" in window) {
    window.requestIdleCallback(slice, { timeout: 400 });
  } else {
    setTimeout(slice, 0);
  }
}

function request(req: Request, onReady: () => void): () => void {
  let set = waiting.get(req.key);
  if (!set) waiting.set(req.key, (set = new Set()));
  set.add(onReady);
  if (!cache.has(req.key) && !requests.has(req.key)) {
    requests.set(req.key, req);
    queue.push(req.key);
    schedule();
  }
  return () => {
    set.delete(onReady);
    if (set.size === 0) {
      waiting.delete(req.key);
      // Nobody is looking any more (scrolled out, navigated away): skip it.
      if (requests.delete(req.key)) {
        const i = queue.indexOf(req.key);
        if (i >= 0) queue.splice(i, 1);
      }
    }
  };
}

/** The cover for a notebook as a data URL, or null while it is still being
 *  drawn (or if WebGL is unavailable). Re-renders only when the id, color or
 *  light/dark scheme changes. */
export function useNotebookCover(
  notebookId: string,
  color: string,
  w = COVER_W,
  h = COVER_H,
  cover?: string,
): string | null {
  const scheme = useSyncExternalStore(subscribeScheme, getScheme);
  const dpr = Math.min(2, Math.max(1, Math.round(window.devicePixelRatio || 1)));
  const { style, seed: id } = coverChoice(notebookId, cover);
  const key = `${id}|${color}|${scheme}|${dpr}|${style}|${w}x${h}`;
  const subscribe = useCallback(
    (cb: () => void) => {
      const hit = cache.get(key);
      if (hit !== undefined) {
        // Recency for the LRU: a card on screen keeps its raster.
        cache.delete(key);
        cache.set(key, hit);
      }
      return request({ key, id, color, scheme, dpr, style, w, h }, cb);
    },
    [key, id, color, scheme, dpr, style, w, h],
  );
  return useSyncExternalStore(subscribe, () => cache.get(key) ?? null);
}
