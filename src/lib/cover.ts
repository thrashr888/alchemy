import { useCallback, useSyncExternalStore } from "react";
import { buildProgram } from "@/components/DitherBackground";

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
const PEAK_ALPHA = { dark: 0.17, light: 0.16 } as const;
/** The wash thins toward the top, where the name and source titles sit. */
const TOP_FADE = 0.35;
const CACHE_LIMIT = 160;
/** Card colors are stored hex; anything else falls back to this neutral. */
const FALLBACK_RGB: [number, number, number] = [138, 147, 166];

type Scheme = "dark" | "light";
interface Request {
  key: string;
  id: string;
  color: string;
  scheme: Scheme;
  dpr: number;
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
  const w = Math.round(COVER_W * req.dpr);
  const h = Math.round(COVER_H * req.dpr);
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
export function useNotebookCover(id: string, color: string): string | null {
  const scheme = useSyncExternalStore(subscribeScheme, getScheme);
  const dpr = Math.min(2, Math.max(1, Math.round(window.devicePixelRatio || 1)));
  const key = `${id}|${color}|${scheme}|${dpr}`;
  const subscribe = useCallback(
    (cb: () => void) => {
      const hit = cache.get(key);
      if (hit !== undefined) {
        // Recency for the LRU: a card on screen keeps its raster.
        cache.delete(key);
        cache.set(key, hit);
      }
      return request({ key, id, color, scheme, dpr }, cb);
    },
    [key, id, color, scheme, dpr],
  );
  return useSyncExternalStore(subscribe, () => cache.get(key) ?? null);
}
