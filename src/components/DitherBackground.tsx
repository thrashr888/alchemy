import { useEffect, useRef, useState } from "react";
import { THEMES, resolveThemeId, type ShaderVariant } from "@/lib/themes";

/** Live prefers-reduced-motion. The CSS guard can't reach a WebGL loop, so
 *  the shader components subscribe to the media query themselves — with a
 *  change listener (like themes.ts's OS-appearance one), not a mount-time
 *  snapshot, so flipping the OS setting takes effect without a reload. */
export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(
    () => window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  );
  useEffect(() => {
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => setReduced(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  return reduced;
}

/**
 * Animated WebGL1 background: a theme-tinted luminance field quantized with
 * 4x4 Bayer ordered dithering. The field varies per theme (Theme.shader) —
 * aetheric mist by default, code rain, retro horizon, paper grain, an
 * instrument dial, a slipstream, a trellis lattice, rebus bars, bokeh city
 * lights, snowfall, a moonlit veil, a glitching raster, orbital paths, a
 * contribution wall, a solar corona, café steam, or CRT phosphor — but every
 * keeps the dither, the central glow, and the transmutation ring so it always
 * reads as the same design element.
 * WebGL1 (with an array-free Bayer) so it runs everywhere, incl. WKWebView.
 */
const SHADER_MODE: Record<ShaderVariant, number> = {
  mist: 0,
  rain: 1,
  horizon: 2,
  grain: 3,
  dial: 4,
  slipstream: 5,
  trellis: 6,
  bars: 7,
  network: 8,
  snow: 9,
  moon: 10,
  glitch: 11,
  orbit: 12,
  contrib: 13,
  corona: 14,
  steam: 15,
  phosphor: 16,
};
export function DitherBackground({
  themeKey,
  className,
  intensity = 1,
  density = 0.5,
}: {
  themeKey?: string;
  className?: string;
  /** Tint strength multiplier — small surfaces (banners) need more than a
   *  full-bleed hero to read as intentional. 1 = the hero's subtlety. */
  intensity?: number;
  /** 0–1 "how much is going on in this notebook" — variants that draw
   *  countable things (nodes, flakes, bars, specks) scale their population
   *  with it. Callers map source counts in; 0.5 is a neutral field. */
  density?: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const reducedMotion = useReducedMotion();

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const gl = (canvas.getContext("webgl", { antialias: false, alpha: false }) ||
      canvas.getContext("experimental-webgl", { antialias: false })) as WebGLRenderingContext | null;
    if (!gl) {
      canvas.style.display = "none";
      return;
    }

    const program = buildProgram(gl);
    if (!program) {
      canvas.style.display = "none";
      return;
    }
    gl.useProgram(program);

    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(program, "a_pos");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

    const uRes = gl.getUniformLocation(program, "u_res");
    const uTime = gl.getUniformLocation(program, "u_time");
    const uTint = gl.getUniformLocation(program, "u_tint");
    const uBg = gl.getUniformLocation(program, "u_bg");
    const uGain = gl.getUniformLocation(program, "u_gain");
    const uMode = gl.getUniformLocation(program, "u_mode");
    const uDensity = gl.getUniformLocation(program, "u_density");
    gl.uniform1f(uGain, intensity);
    gl.uniform1f(uDensity, Math.min(1, Math.max(0, density)));

    const variant: ShaderVariant = THEMES[resolveThemeId(themeKey)]?.shader ?? "mist";
    gl.uniform1f(uMode, SHADER_MODE[variant]);

    const readVar = (name: string, fallback: [number, number, number]) =>
      hexToRgb(getComputedStyle(document.documentElement).getPropertyValue(name).trim()) ?? fallback;
    gl.uniform3fv(uTint, readVar("--primary", [0.37, 0.42, 0.82]));
    gl.uniform3fv(uBg, readVar("--background", [0.03, 0.035, 0.04]));

    const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
    const resize = () => {
      const w = Math.max(1, Math.floor(canvas.clientWidth * dpr));
      const h = Math.max(1, Math.floor(canvas.clientHeight * dpr));
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
      }
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.uniform2f(uRes, canvas.width, canvas.height);
    };
    resize();

    let raf = 0;
    let last = 0;
    const startT = performance.now();
    // Grain is a texture, not weather — it draws once, like reduced motion.
    const isStatic = reducedMotion || variant === "grain";
    const render = (now: number) => {
      if (!isStatic) raf = requestAnimationFrame(render);
      if (now - last < 33) return;
      last = now;
      resize();
      gl.uniform1f(uTime, isStatic ? 0 : (now - startT) / 1000);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    raf = requestAnimationFrame(render);

    const ro = new ResizeObserver(() => {
      resize();
      // Static variants get no animation frames, so redraw on resize here.
      if (isStatic) {
        gl.uniform1f(uTime, 0);
        gl.drawArrays(gl.TRIANGLES, 0, 3);
      }
    });
    ro.observe(canvas);

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      gl.deleteBuffer(buf);
      gl.deleteProgram(program);
    };
  }, [themeKey, intensity, density, reducedMotion]);

  return (
    <canvas
      ref={canvasRef}
      className={className}
      style={{ width: "100%", height: "100%", display: "block" }}
      aria-hidden
    />
  );
}

function hexToRgb(hex: string): [number, number, number] | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex);
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

const VERT = `
attribute vec2 a_pos;
void main(){ gl_Position = vec4(a_pos, 0.0, 1.0); }`;

const FRAG = `
precision highp float;
uniform vec2 u_res;
uniform float u_time;
uniform vec3 u_tint;
uniform vec3 u_bg;
uniform float u_gain;
uniform float u_mode;
uniform float u_density;

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123); }
float vnoise(vec2 p){
  vec2 i = floor(p); vec2 f = fract(p);
  float a = hash(i), b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.)), d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.0 - 2.0*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float fbm(vec2 p){
  float v = 0.0, a = 0.5;
  for(int i = 0; i < 5; i++){ v += a * vnoise(p); p *= 2.02; a *= 0.5; }
  return v;
}
// 4x4 Bayer via the recursive 2x2 pattern (no array indexing -> WebGL1 safe).
float bayer4(vec2 p){
  vec2 a = mod(p, 2.0);
  vec2 b = floor(0.5 * mod(p, 4.0));
  float lo = 4.0 * mix(mix(0.0, 2.0, a.x), mix(3.0, 1.0, a.x), a.y);
  float hi = mix(mix(0.0, 2.0, b.x), mix(3.0, 1.0, b.x), b.y);
  return (lo + hi) / 16.0;
}

// mode 0 — aetheric mist (the default Alchemy field).
float mistField(vec2 uv, float glow){
  float t = u_time * 0.03;
  float m = fbm(uv*2.4 + vec2(t, -t*0.6)) + 0.35*fbm(uv*5.0 - vec2(t*0.5, t));
  return clamp(glow*0.6 + m*0.55 - 0.16, 0.0, 1.0);
}
// mode 1 — code rain: quantized columns of falling trails with bright heads.
float rainField(vec2 uv, float glow){
  // Extra slow on purpose: the backdrop should read as weather, not motion.
  float col = floor(uv.x * 44.0);
  float speed = 0.02 + 0.04 * hash(vec2(col, 7.0));
  float y = uv.y * 2.2 + mod(u_time, 2048.0) * speed;
  // Value noise clusters near 0.5, so keep thresholds tight around it.
  float n = vnoise(vec2(col * 0.61 + 13.7, y));
  float trail = smoothstep(0.34, 0.72, n);
  float head = smoothstep(0.62, 0.80, n);
  return clamp((trail*0.55 + head*0.75) * (0.45 + glow*0.55), 0.0, 1.0);
}
// mode 2 — retro horizon: striped sun over a perspective grid rolling toward
// the viewer, with a whisper of mist so it still reads as Alchemy.
float horizonField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  vec2 sp = uv - vec2(0.0, 0.12);
  float sun = smoothstep(0.33, 0.32, length(sp));
  float stripes = step(0.42, fract(sp.y * 16.0));
  sun *= mix(1.0, stripes, smoothstep(0.02, -0.12, sp.y));
  float horizon = -0.16;
  float below = step(uv.y, horizon);
  float py = horizon - uv.y + 0.001;
  // Verticals converge on the horizon at constant screen width...
  float gx = uv.x / (py + 0.05);
  float lv = smoothstep(0.10, 0.0, abs(fract(gx*0.7 + 0.5) - 0.5) * (py + 0.05) * 30.0);
  // ...while horizontals thin with distance (perspective-correct rows).
  float rz = 0.35 / (py + 0.05) - t * 0.12;
  float lh = smoothstep(0.12, 0.0, abs(fract(rz) - 0.5));
  float grid = max(lv, lh) * below * smoothstep(0.0, 0.12, py);
  float m = 0.18 * fbm(uv*3.0 + vec2(t*0.03, 0.0));
  return clamp(max(sun * (0.5 + 0.5*glow), grid*0.55) + m - 0.05, 0.0, 1.0);
}
// mode 3 — paper grain: static anisotropic fibers + fine speckle.
float grainField(vec2 uv, float glow){
  float m = fbm(vec2(uv.x*12.0, uv.y*5.0)) + 0.3*vnoise(uv*40.0);
  return clamp(glow*0.5 + m*0.42 - 0.20, 0.0, 1.0);
}
// mode 4 — instrument dial: a tick scale ringing the bezel with a needle
// sweeping it, the centre gauge of a five-dial cluster. The transmutation
// ring in main() lands inside the scale and reads as the dial face.
float dialField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  float r = length(uv);
  float ang = atan(uv.y, uv.x);
  // The scale sits just inside the ring, which reads as the bezel. Anything
  // further out clips at the top and bottom of a wide hero (uv.y is +-0.5).
  float band = smoothstep(0.295, 0.315, r) * smoothstep(0.350, 0.332, r);
  // A tachometer, not a clock: the scale breaks at the bottom (db is the
  // angular distance from straight down) and runs ~270 degrees over the top.
  float db = abs(fract((ang + 1.5708) * 0.15915 + 0.5) - 0.5) * 6.2832;
  float arc = band * smoothstep(0.50, 0.80, db);
  // 60 minor ticks (9.5493 = 60/2PI), a major every fifth (1.9099 = 12/2PI).
  float minor = smoothstep(0.46, 0.50, abs(fract(ang * 9.5493) - 0.5)) * 0.34;
  float major = smoothstep(0.44, 0.50, abs(fract(ang * 1.9099) - 0.5)) * 0.62;
  // Redline: the last stretch of scale before the needle runs out of dial.
  float redline = smoothstep(-1.30, -1.20, ang) * smoothstep(-0.70, -0.82, ang);
  float ticks = (minor + major) * arc + redline * band * 0.45;
  // Needle: pivots at the hub, sweeping clockwise over the top from the rest
  // stop (4.0 rad, lower left) to the redline (-1.0 rad, lower right).
  float sweep = 4.0 - 5.0 * (0.5 + 0.5 * sin(t * 0.30));
  float da = abs(fract((ang - sweep) * 0.15915 + 0.5) - 0.5) * 6.2832;
  float needle = smoothstep(0.09, 0.0, da) * smoothstep(0.34, 0.04, r);
  return clamp((ticks + needle*0.85) * (0.5 + glow*0.5) + glow*0.12, 0.0, 1.0);
}
// mode 5 — slipstream: horizontal speed streaks tearing past, fastest across
// the centre line, over a heat shimmer. Velocity rather than weather.
float slipstreamField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  float lane = floor(uv.y * 90.0);
  float speed = 0.20 + 0.45 * hash(vec2(lane, 3.0));
  // Noise squashed along x so each lane smears into a motion-blurred streak.
  float n = vnoise(vec2(uv.x*2.2 - t*speed, lane*0.37));
  float streak = smoothstep(0.55, 0.92, n);
  float centre = smoothstep(0.55, 0.0, abs(uv.y));
  float shimmer = 0.14 * fbm(vec2(uv.x*3.0, uv.y*8.0 + t*0.16));
  return clamp(streak*centre * (0.45 + glow*0.55) + shimmer + glow*0.10, 0.0, 1.0);
}

// mode 7 — rebus bars: Paul Rand's eight-bar stripes. Horizontal scanlines
// gated into rectangular striped patches — eight rows to a patch, edges set
// by a noise contour drifting glacially — so the field reads as the
// letterform stripes of the mark dissolving across a punched-card grid.
float barsField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  float ny = uv.y * 40.0;
  float f = fract(ny);
  // The stripe: bar and gap near even, softened just enough to survive
  // the dither without aliasing into moire.
  float stripe = smoothstep(0.05, 0.13, f) * smoothstep(0.58, 0.50, f);
  // Glyph blocks: every 8 stripes share a row of crisp rectangles — the
  // rebus's own proportions — running as a slow ticker, alternate rows
  // sliding opposite ways, each block re-rolled as it wraps. More
  // sources, more of the field carries stripes.
  float grp = floor(floor(ny) / 8.0);
  float dir = mix(-1.0, 1.0, mod(grp, 2.0));
  float qx = uv.x * 5.0 + dir * t * 0.05;
  float bx = floor(qx);
  float th = mix(0.62, 0.42, u_density);
  float gate = step(th, hash(vec2(bx, grp * 7.0 + 13.0)));
  float bf = fract(qx);
  float edge = smoothstep(0.0, 0.05, bf) * smoothstep(1.0, 0.95, bf);
  return clamp(stripe * gate * edge * (0.35 + glow*0.65) + glow*0.08, 0.0, 1.0);
}

// mode 8 — network lights: a strip of defocused neon bars — city lights
// shot through a rain-wet window. One gently tilted band; vertical capsule
// lamps at regular pitch, blazing to a continuous core mid-strip and
// falling away to sparse dim bars at the ends, each lamp breathing on its
// own clock. u_density is how much of the city is lit.
float networkField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  float y = uv.y + uv.x * 0.07;
  float x = uv.x + t * 0.004;
  float pitch = 18.0;
  float px = x * pitch;
  float col = floor(px);
  float h1 = hash(vec2(col, 3.0));
  float h2 = hash(vec2(col, 9.0));
  // Envelope: a hot compact core, sparse dim outriders, then darkness.
  float g1 = (x + 0.04) * 3.0;
  float e = exp(-g1 * g1);
  float outer = smoothstep(0.85, 0.35, abs(x + 0.04));
  // Lamps thin out only past the strip's shoulders, never mid-core.
  float on = step(mix(0.75, 0.30, u_density) * smoothstep(0.35, 0.65, abs(x + 0.04)), h1);
  // Rounded capsule per lamp (SDF); core lamps run fat and crisp, the
  // outriders slimmer and defocused.
  vec2 p = vec2((fract(px) - 0.5) / pitch, y - 0.030 * (h2 - 0.5));
  float hl = 0.055 + 0.030 * h2;
  p.y -= clamp(p.y, -hl, hl);
  float d = length(p);
  float w = mix(0.008, 0.016, e);
  float blur = mix(0.004, 0.014, 1.0 - e);
  float bar = smoothstep(w + blur, w - blur * 0.5, d);
  float lamp = 0.80 + 0.20 * sin(t * (0.20 + 0.30 * h1) + h1 * 6.2832);
  // Halation: the core's lamps bloom into one continuous blaze.
  float halo = e * exp(-y * y * 60.0) * 0.45;
  float L = bar * on * outer * mix(0.25, 1.0, e) * lamp + halo;
  return clamp(L + glow * 0.05, 0.0, 1.0);
}

// mode 9 — snowfall: three flake layers at different sizes and speeds for
// parallax depth, a soft drift bank along the bottom, and a whisper of air
// glow. u_density is how hard it's coming down.
float snowLayer(vec2 uv, float t, float scale, float speed, float sway){
  vec2 g = vec2(uv.x * scale + sin(t * 0.10 + uv.y * 2.0) * sway, uv.y * scale + t * speed);
  vec2 cell = floor(g);
  vec2 f = fract(g) - 0.5;
  float h = hash(cell);
  vec2 off = vec2(h - 0.5, hash(cell + 3.3) - 0.5) * 0.6;
  float flake = smoothstep(0.16, 0.02, length(f - off));
  return flake * step(mix(0.72, 0.35, u_density), h);
}
float snowField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  float s = 0.70 * snowLayer(uv, t, 9.0, 0.35, 0.15)
          + 0.45 * snowLayer(uv, t * 0.8, 16.0, 0.25, 0.10)
          + 0.30 * snowLayer(uv, t * 0.6, 26.0, 0.18, 0.06);
  float bank = smoothstep(-0.28, -0.55, uv.y) * (0.25 + 0.15 * fbm(vec2(uv.x * 3.0, 0.5)));
  float air = 0.08 * fbm(uv * 2.0 + vec2(t * 0.01, 0.0));
  return clamp(s * (0.50 + glow * 0.50) + bank + air + glow * 0.10, 0.0, 1.0);
}

// mode 10 — moonlit: a gibbous moon high off-centre behind slow banks of
// fog drifting on two speeds; the veil thins and thickens across the disc.
// Gothic weather, nothing dripping.
float moonField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  vec2 mp = uv - vec2(0.30, 0.22);
  float disc = smoothstep(0.130, 0.122, length(mp));
  float bite = smoothstep(0.150, 0.142, length(mp - vec2(0.052, 0.024)));
  float moon = clamp(disc - bite * 0.55, 0.0, 1.0);
  float halo = 0.45 * exp(-dot(mp, mp) * 16.0);
  float veil = fbm(uv * 3.0 + vec2(t * 0.017, 0.0));
  moon *= 0.55 + 0.45 * smoothstep(0.65, 0.35, veil);
  float fog = 0.30 * fbm(uv * 2.2 + vec2(t * 0.014, 0.0))
            + 0.20 * fbm(uv * 4.0 - vec2(t * 0.020, t * 0.004));
  fog *= mix(0.7, 1.15, u_density);
  return clamp(moon * 0.9 + halo + fog * (0.5 + glow * 0.5) - 0.06 + glow * 0.08, 0.0, 1.0);
}

// mode 11 — glitch: a calm raster hum with one interference band rolling
// up slowly — until, every few seconds, the feed tears for a fraction of a
// second: many rows shear at once, re-rolling several times before the
// signal re-locks. u_density is how corrupted the feed runs.
float glitchField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  // The idle signal, slowed to a drowse.
  float raster = 0.10 * step(0.5, fract(uv.y * 60.0));
  float band = 0.14 * smoothstep(0.45, 0.0, abs(fract(uv.y * 1.2 - t * 0.012) - 0.5));
  // Each ~4s epoch hides one brief burst window at a hashed offset; inside
  // it the tears re-roll many times a second, then vanish.
  float epoch = floor(t * 0.25);
  float ph = fract(t * 0.25);
  float w = 0.10 + 0.80 * hash(vec2(epoch, 3.0));
  float active = smoothstep(0.045, 0.015, abs(ph - w));
  float tick = floor(t * 12.0);
  float row = floor(uv.y * 42.0);
  float burst = step(mix(0.85, 0.55, u_density), hash(vec2(row, tick))) * active;
  float shear = (hash(vec2(row, tick + 41.0)) - 0.5) * 0.6 * burst;
  float blocks = step(0.55, vnoise(vec2((uv.x + shear) * 5.0, row * 0.83)));
  float tear = burst * blocks;
  return clamp(tear * (0.60 + glow * 0.40) + raster + band + glow * 0.12, 0.0, 1.0);
}

// mode 12 — orbit: concentric orbital paths around the centre, a satellite
// riding each with a comet trail hugging its ring, alternating directions.
// Sparse pulsing instrument specks fill the space between; u_density is how
// much telemetry is up.
float orbitField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  float r = length(uv);
  float a = atan(uv.y, uv.x);
  float L = 0.0;
  for (int i = 0; i < 4; i++){
    float fi = float(i);
    float rad = 0.14 + 0.13 * fi;
    L += 0.10 * smoothstep(0.0035, 0.0, abs(r - rad));
    float dir = mix(1.0, -1.0, mod(fi, 2.0));
    float ang = dir * t * (0.16 - 0.03 * fi) + fi * 2.4;
    float lag = fract(dir * (ang - a) * 0.15915);
    L += smoothstep(0.006, 0.0, abs(r - rad)) * smoothstep(0.18, 0.0, lag) * 0.5;
    vec2 sat = rad * vec2(cos(ang), sin(ang));
    L += 0.7 * smoothstep(0.030, 0.004, length(uv - sat));
  }
  vec2 gc = floor(uv * 14.0);
  float sp = step(mix(0.97, 0.88, u_density), hash(gc))
           * smoothstep(0.35, 0.05, length(fract(uv * 14.0) - 0.5));
  float pulse = 0.5 + 0.5 * sin(t * 0.8 + hash(gc) * 6.2832);
  return clamp(L * (0.5 + glow * 0.5) + sp * pulse * 0.3 + glow * 0.10, 0.0, 1.0);
}

// mode 13 — contribution graph: a wall of small rounded cells at five
// quantized levels, each easing up or down a level on its own slow clock.
// u_density is how active the year was.
float contribField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  vec2 g = uv * 28.0;
  vec2 cell = floor(g);
  vec2 f = fract(g) - 0.5;
  vec2 q = abs(f) - 0.32;
  float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - 0.06;
  float sq = smoothstep(0.02, -0.03, d);
  float h = hash(cell);
  float wander = vnoise(vec2(h * 91.0, t * 0.04));
  // Activity arrives in patches — a low-frequency cluster field, so lit
  // cells group into productive stretches with quiet weeks between.
  float cluster = vnoise(cell * 0.16 + vec2(t * 0.01, 0.0));
  float lvl = floor(clamp(h * 0.45 + wander * 0.40 + cluster * 1.1 + u_density * 0.45 - 1.05, 0.0, 0.999) * 5.0) / 4.0;
  return clamp(sq * lvl * (0.50 + glow * 0.50) + glow * 0.06, 0.0, 1.0);
}

// mode 14 — corona: the sun as a disc of quiet fire — granulation inside,
// prominences breathing at the rim, streamers reaching out and fading.
// Angular noise rides (cos, sin) so there is no seam.
float coronaField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  float r = length(uv);
  vec2 dir = uv / max(r, 0.001);
  float disc = smoothstep(0.185, 0.180, r);
  float gran = fbm(uv * 11.0 + vec2(t * 0.010, 0.0));
  float limb = smoothstep(0.045, 0.0, abs(r - 0.183)) * 0.55;
  // Prominences: high-contrast tongues licking off the rim.
  float tongue = fbm(dir * 2.2 + vec2(t * 0.040, 0.0));
  tongue = tongue * tongue;
  float prom = smoothstep(0.20 + 0.16 * tongue, 0.185, r) * (1.0 - disc);
  // Streamers: long, structured rays — sharpened noise, quick radial decay.
  float ray = fbm(dir * 3.2 + vec2(0.0, t * 0.015));
  ray = ray * ray * ray;
  float streamer = exp(-(r - 0.185) * 3.5) * ray * step(0.185, r);
  return clamp(disc * (0.45 + 0.5 * gran) + limb + prom * 0.8 + streamer * 1.2 + glow * 0.04, 0.0, 1.0);
}

// mode 15 — steam: curls rising off the cup, shearing sideways as they
// climb and thinning into nothing well before the top of the frame.
float steamField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  vec2 p = vec2(uv.x * 4.2, uv.y * 2.4 - t * 0.05);
  // Two warps deep for the curling: the second reads the first.
  float w1 = fbm(p + vec2(0.0, t * 0.02));
  float w2 = fbm(p * 1.7 + vec2(w1 * 2.4, t * 0.01));
  float s = fbm(p * 2.6 + vec2(w2 * 2.4, 0.0));
  s = s * s; s = s * s * 3.2;
  // Two narrow wisp columns off cup positions — the columns themselves
  // sway a little, like heat finding its way up.
  float c1 = uv.x + 0.26 + 0.04 * sin(uv.y * 5.0 + t * 0.10);
  float c2 = uv.x - 0.32 + 0.04 * sin(uv.y * 6.0 - t * 0.08);
  float plume = exp(-c1 * c1 * 55.0) + 0.75 * exp(-c2 * c2 * 70.0);
  float fade = smoothstep(0.35, -0.42, uv.y);
  return clamp(s * plume * fade * 1.4 + glow * 0.06, 0.0, 1.0);
}

// mode 16 — phosphor: an amber CRT at rest — scanlines under a slow
// refresh bar rolling down, the faint burn-in of characters long since
// scrolled away, screen-curvature vignette, a breath of flicker.
float phosphorField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  float scan = 0.5 + 0.5 * sin(uv.y * 320.0);
  float pos = 0.55 - fract(t * 0.05) * 1.3;
  float bar = 0.16 * exp(-(uv.y - pos) * (uv.y - pos) * 60.0);
  vec2 ch = vec2(uv.x * 42.0, uv.y * 19.0);
  float glyph = step(0.74, vnoise(floor(ch) * 0.73)) * step(0.30, fract(ch.x * 0.5));
  float vig = smoothstep(1.05, 0.45, length(uv * vec2(0.9, 1.3)));
  float flicker = 0.97 + 0.03 * sin(t * 7.0);
  return clamp((0.10 + 0.10 * scan + glyph * 0.22 + bar) * vig * flicker + glow * 0.10, 0.0, 1.0);
}

// mode 6 — trellis: a triangulated tube lattice, the frame under the tank.
// Three line families 60 degrees apart tile the plane with triangles; a slow
// wave travels the frame so it breathes without reading as motion.
float trellisField(vec2 uv, float glow){
  float t = mod(u_time, 2048.0);
  float s = 6.0;
  float a = abs(fract((uv.x) * s) - 0.5);
  float b = abs(fract((uv.x*0.5 + uv.y*0.866) * s) - 0.5);
  float c = abs(fract((-uv.x*0.5 + uv.y*0.866) * s) - 0.5);
  float d = min(min(a, b), c);
  float tube = smoothstep(0.055, 0.0, d);
  // Nodes: where two families meet, the joint reads a touch brighter.
  float second = min(max(min(a,b), min(max(a,b), c)), 0.5);
  float node = smoothstep(0.05, 0.0, second) * 0.5;
  float wave = 0.62 + 0.38 * sin(t*0.22 - (uv.x + uv.y) * 2.2);
  return clamp((tube + node) * (0.30 + glow*0.70) * wave + glow*0.10, 0.0, 1.0);
}

void main(){
  vec2 uv = (gl_FragCoord.xy - 0.5 * u_res) / u_res.y;
  float r = length(uv);
  // Floored so the field reaches the edges instead of dying at the corners
  // of a wide surface — the centre stays brighter, but never alone.
  float glow = mix(0.30, 1.0, smoothstep(1.15, 0.05, r));
  float L;
  if (u_mode < 0.5)      L = mistField(uv, glow);
  else if (u_mode < 1.5) L = rainField(uv, glow);
  else if (u_mode < 2.5) L = horizonField(uv, glow);
  else if (u_mode < 3.5) L = grainField(uv, glow);
  else if (u_mode < 4.5) L = dialField(uv, glow);
  else if (u_mode < 5.5) L = slipstreamField(uv, glow);
  else if (u_mode < 6.5) L = trellisField(uv, glow);
  else if (u_mode < 7.5) L = barsField(uv, glow);
  else if (u_mode < 8.5) L = networkField(uv, glow);
  else if (u_mode < 9.5) L = snowField(uv, glow);
  else if (u_mode < 10.5) L = moonField(uv, glow);
  else if (u_mode < 11.5) L = glitchField(uv, glow);
  else if (u_mode < 12.5) L = orbitField(uv, glow);
  else if (u_mode < 13.5) L = contribField(uv, glow);
  else if (u_mode < 14.5) L = coronaField(uv, glow);
  else if (u_mode < 15.5) L = steamField(uv, glow);
  else                    L = phosphorField(uv, glow);
  float ring = smoothstep(0.006, 0.0, abs(r - 0.36)) * glow * 0.4;
  L = max(L, ring);
  float d = bayer4(gl_FragCoord.xy) - 0.5;
  float q = floor(L * 5.0 + d + 0.5) / 5.0;
  vec3 col = mix(u_bg, u_tint, clamp(q * 0.22 * u_gain, 0.0, 1.0));
  gl_FragColor = vec4(col, 1.0);
}`;

function buildProgram(gl: WebGLRenderingContext): WebGLProgram | null {
  const compile = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
      console.warn("dither shader:", gl.getShaderInfoLog(s));
      return null;
    }
    return s;
  };
  const vs = compile(gl.VERTEX_SHADER, VERT);
  const fs = compile(gl.FRAGMENT_SHADER, FRAG);
  if (!vs || !fs) return null;
  const p = gl.createProgram()!;
  gl.attachShader(p, vs);
  gl.attachShader(p, fs);
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
    console.warn("dither link:", gl.getProgramInfoLog(p));
    return null;
  }
  return p;
}
