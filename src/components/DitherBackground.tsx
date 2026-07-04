import { useEffect, useRef } from "react";

/**
 * Animated WebGL1 background: drifting "aetheric" mist + a central glow,
 * quantized with 4x4 Bayer ordered dithering and tinted to the current theme.
 * WebGL1 (with an array-free Bayer) so it runs everywhere, incl. WKWebView.
 */
export function DitherBackground({ themeKey, className }: { themeKey?: string; className?: string }) {
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
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);
    resize();

    let raf = 0;
    let last = 0;
    const startT = performance.now();
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const render = (now: number) => {
      // Reduced motion: draw a single static frame instead of animating.
      if (!reducedMotion) raf = requestAnimationFrame(render);
      if (now - last < 33) return;
      last = now;
      resize();
      gl.uniform1f(uTime, reducedMotion ? 0 : (now - startT) / 1000);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    raf = requestAnimationFrame(render);

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      gl.deleteBuffer(buf);
      gl.deleteProgram(program);
    };
  }, [themeKey]);

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
void main(){
  vec2 uv = (gl_FragCoord.xy - 0.5 * u_res) / u_res.y;
  float t = u_time * 0.03;
  float m = fbm(uv*2.4 + vec2(t, -t*0.6)) + 0.35*fbm(uv*5.0 - vec2(t*0.5, t));
  float r = length(uv);
  float glow = smoothstep(1.15, 0.05, r);
  float L = clamp(glow*0.6 + m*0.55 - 0.16, 0.0, 1.0);
  float ring = smoothstep(0.006, 0.0, abs(r - 0.36)) * glow * 0.4;
  L = max(L, ring);
  float d = bayer4(gl_FragCoord.xy) - 0.5;
  float q = floor(L * 5.0 + d + 0.5) / 5.0;
  vec3 col = mix(u_bg, u_tint, q * 0.22);
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
