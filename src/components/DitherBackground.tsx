import { useEffect, useRef } from "react";
import { THEMES, resolveThemeId, type ShaderVariant } from "@/lib/themes";

/**
 * Animated WebGL1 background: a theme-tinted luminance field quantized with
 * 4x4 Bayer ordered dithering. The field varies per theme (Theme.shader) —
 * aetheric mist by default, code rain, retro horizon, paper grain, an
 * instrument dial, a slipstream, or a trellis lattice — but every variant
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
};
export function DitherBackground({
  themeKey,
  className,
  intensity = 1,
}: {
  themeKey?: string;
  className?: string;
  /** Tint strength multiplier — small surfaces (banners) need more than a
   *  full-bleed hero to read as intentional. 1 = the hero's subtlety. */
  intensity?: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

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
    gl.uniform1f(uGain, intensity);

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
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
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
  }, [themeKey, intensity]);

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
  float glow = smoothstep(1.15, 0.05, r);
  float L;
  if (u_mode < 0.5)      L = mistField(uv, glow);
  else if (u_mode < 1.5) L = rainField(uv, glow);
  else if (u_mode < 2.5) L = horizonField(uv, glow);
  else if (u_mode < 3.5) L = grainField(uv, glow);
  else if (u_mode < 4.5) L = dialField(uv, glow);
  else if (u_mode < 5.5) L = slipstreamField(uv, glow);
  else                   L = trellisField(uv, glow);
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
