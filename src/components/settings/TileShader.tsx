import { useEffect, useRef } from "react";
import { useReducedMotion } from "../DitherBackground";

/**
 * Tiny per-tile WebGL fields for the Activity stat tiles — same family as
 * DitherBackground (WebGL1, 4x4 Bayer dither, quantized levels) but drawn on
 * a TRANSPARENT canvas so they wash over the tile's own surface instead of
 * painting an opaque background block. Two fields, both state-driven:
 *
 *   ember — sparks drifting up from a warm bed; the streak tile while it
 *           burns. `intensity` scales with streak length.
 *   sky   — the peak-hour tile's weather: a soft sun whose position across
 *           the tile IS the hour (dawn left, noon high, dusk right), or a
 *           sparse twinkling starfield at night.
 */
export function TileShader({
  mode,
  hour = 12,
  intensity = 1,
  tintVar,
}: {
  mode: "ember" | "sky";
  /** Local hour 0–23; only the sky reads it. */
  hour?: number;
  /** 0–1 luminance multiplier. */
  intensity?: number;
  /** CSS custom property to tint with, e.g. "--artifact-template". */
  tintVar: string;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const reducedMotion = useReducedMotion();

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const gl = canvas.getContext("webgl", {
      antialias: false,
      alpha: true,
      premultipliedAlpha: true,
    }) as WebGLRenderingContext | null;
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
    gl.bufferData(
      gl.ARRAY_BUFFER,
      new Float32Array([-1, -1, 3, -1, -1, 3]),
      gl.STATIC_DRAW,
    );
    const loc = gl.getAttribLocation(program, "a_pos");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

    const uRes = gl.getUniformLocation(program, "u_res");
    const uTime = gl.getUniformLocation(program, "u_time");
    const uTint = gl.getUniformLocation(program, "u_tint");
    const uGain = gl.getUniformLocation(program, "u_gain");
    const uMode = gl.getUniformLocation(program, "u_mode");
    const uHour = gl.getUniformLocation(program, "u_hour");
    gl.uniform1f(uGain, intensity);
    gl.uniform1f(uMode, mode === "ember" ? 0 : 1);
    gl.uniform1f(uHour, hour);
    const tint = hexToRgb(
      getComputedStyle(document.documentElement)
        .getPropertyValue(tintVar)
        .trim(),
    ) ?? [0.91, 0.64, 0.24];
    gl.uniform3fv(uTint, tint);

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
    const isStatic = reducedMotion;
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
  }, [mode, hour, intensity, tintVar, reducedMotion]);

  return (
    <canvas
      ref={canvasRef}
      className="pointer-events-none absolute inset-0"
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
uniform float u_gain;
uniform float u_mode;
uniform float u_hour;

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123); }
float vnoise(vec2 p){
  vec2 i = floor(p); vec2 f = fract(p);
  float a = hash(i), b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.)), d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.0 - 2.0*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float bayer4(vec2 p){
  vec2 a = mod(p, 2.0);
  vec2 b = floor(0.5 * mod(p, 4.0));
  float lo = 4.0 * mix(mix(0.0, 2.0, a.x), mix(3.0, 1.0, a.x), a.y);
  float hi = mix(mix(0.0, 2.0, b.x), mix(3.0, 1.0, b.x), b.y);
  return (lo + hi) / 16.0;
}

// ember — sparks drifting up from a warm bed along the tile floor. Paced
// livelier than the big backdrops: a tile this small can afford motion.
float emberField(vec2 uv){
  float t = mod(u_time, 2048.0);
  float col = floor(uv.x * 30.0);
  float speed = 0.25 + 0.30 * hash(vec2(col, 5.0));
  float n = vnoise(vec2(col * 0.83 + 3.1, (uv.y - t * speed) * 7.0));
  float spark = smoothstep(0.74, 0.93, n);
  // Sparks cool as they rise; the bed smoulders unevenly.
  float fade = smoothstep(1.05, 0.0, uv.y);
  float bed = smoothstep(0.30, 0.0, uv.y)
            * (0.45 + 0.30 * vnoise(vec2(uv.x * 9.0, t * 1.1)));
  return clamp(spark * fade * 0.9 + bed, 0.0, 1.0);
}

// sky — the synthwave sunset from the horizon theme, scaled to a tile: a
// round striped sun riding the hour's arc (dawn left, noon high, dusk
// right), or twinkling stars after dark. Aspect-corrected so the disc
// stays round in a wide tile.
float skyField(vec2 uv){
  float t = mod(u_time, 2048.0);
  if (u_hour < 5.0 || u_hour >= 21.0) {
    // Sparse stars, each twinkling on its own clock.
    vec2 g = floor(uv * vec2(26.0, 9.0));
    float h = hash(g);
    float star = step(0.90, h);
    vec2 f = fract(uv * vec2(26.0, 9.0)) - 0.5;
    float dot_ = smoothstep(0.30, 0.05, length(f));
    float tw = 0.55 + 0.45 * sin(t * (2.0 + h * 4.0) + h * 41.0);
    return clamp(star * dot_ * tw, 0.0, 1.0);
  }
  float aspect = u_res.x / u_res.y;
  vec2 q = vec2(uv.x * aspect, uv.y);
  float p = (u_hour - 5.0) / 16.0;
  vec2 sun = vec2((0.12 + 0.76 * p) * aspect, 0.30 + 0.45 * sin(3.14159 * p));
  vec2 sp = q - sun;
  float disc = 1.0 - smoothstep(0.355, 0.37, length(sp));
  // The synthwave cut: chunky stripes slice the disc's lower half, drifting
  // down like the sun is forever setting. Coarse bands — at tile scale,
  // fine stripes dissolve into the dither.
  float stripes = step(0.45, fract(sp.y * 7.0 + t * 0.06));
  disc *= mix(1.0, stripes, smoothstep(0.03, -0.06, sp.y));
  float glow = 0.25 * exp(-dot(sp, sp) * 4.0);
  float haze = 0.10 * vnoise(vec2(q.x * 3.0 + t * 0.22, q.y * 3.0));
  return clamp(disc * 0.95 + glow + haze, 0.0, 1.0);
}

void main(){
  vec2 uv = gl_FragCoord.xy / u_res;
  float L = u_mode < 0.5 ? emberField(uv) : skyField(uv);
  float d = bayer4(gl_FragCoord.xy) - 0.5;
  // 6 levels (vs the backdrops' 5): tile features are small, so a step more
  // tonal resolution keeps the sun's edge from dissolving.
  float q = floor(L * 6.0 + d + 0.5) / 6.0;
  // Transparent wash: quantized alpha, premultiplied tint. Kept faint so
  // the tile stays a tile.
  float a = clamp(q * 0.30 * u_gain, 0.0, 0.5);
  gl_FragColor = vec4(u_tint * a, a);
}`;

function buildProgram(gl: WebGLRenderingContext): WebGLProgram | null {
  const compile = (type: number, src: string) => {
    const s = gl.createShader(type);
    if (!s) return null;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
      console.warn("tile shader:", gl.getShaderInfoLog(s));
      return null;
    }
    return s;
  };
  const vs = compile(gl.VERTEX_SHADER, VERT);
  const fs = compile(gl.FRAGMENT_SHADER, FRAG);
  if (!vs || !fs) return null;
  const p = gl.createProgram();
  if (!p) return null;
  gl.attachShader(p, vs);
  gl.attachShader(p, fs);
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
    console.warn("tile shader link:", gl.getProgramInfoLog(p));
    return null;
  }
  return p;
}
