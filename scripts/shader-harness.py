#!/usr/bin/env python3
"""Render Alchemy's WebGL shaders outside the app so they can be looked at.

The two shader components — src/components/DitherBackground.tsx (the
notebook backdrop, 17 theme-driven modes) and
src/components/settings/TileShader.tsx (the Activity stat-tile washes) —
are GLSL ES 1.0 inside template literals. Editing them blind produces
confident misses; the only reliable check is pixels next to a reference.

This script pulls the live VERT/FRAG sources and the theme palette out of
the TypeScript, and writes one self-contained HTML page that compiles both
programs with a real WebGL1 context. Because the whole FRAG is compiled, the
page is also the compile gate: a GLSL error in any mode kills the backdrop
for every theme, and here it shows up as a red banner instead.

    python3 scripts/shader-harness.py            # write the page, print its path
    python3 scripts/shader-harness.py --serve    # ...and serve it on :8791
    python3 scripts/shader-harness.py --open     # ...and open it in the browser

Page URLs (all params optional):

    /                                    contact sheet — every dither mode with
                                         the theme that uses it, plus the tiles
    /?theme=dracula                      contact sheet in one theme's palette
    /?mode=network&theme=metropolis      one dither mode, full viewport
    /?mode=snow&t=4.5                    frozen at t=4.5s (deterministic shots)
    /?mode=contrib&density=0.9&gain=1.4  the component's density/intensity props
    /?mode=mist&tint=ff8800&bg=101010    override the palette
    /?shader=tile&mode=sky&hour=20       a tile field; ember | sky | spark
    /?shader=tile&mode=spark&series=.1,.2,.35,.5,.9

Machine-readable status: the page sets <html data-status="ok|fail"> and
document.title to "shaders ok" / "shaders FAIL", and console.error()s every
compile log, so an agent can read_page / read_console_messages instead of
squinting at a screenshot.
"""

from __future__ import annotations

import argparse
import http.server
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DITHER = ROOT / "src/components/DitherBackground.tsx"
TILE = ROOT / "src/components/settings/TileShader.tsx"
THEMES = ROOT / "src/lib/themes.ts"


def template(src: str, name: str, path: Path) -> str:
    m = re.search(r"const %s = `(.*?)`;" % name, src, re.S)
    if not m:
        sys.exit(f"{path}: no `const {name} = \\`...\\`;` template found")
    body = m.group(1)
    if "${" in body:
        sys.exit(f"{path}: {name} uses ${{}} interpolation; extend this script")
    return body


def const_map(src: str, name: str, path: Path) -> dict[str, int]:
    """`const NAME: Record<...> = { key: 0, ... }` → {key: 0}."""
    m = re.search(r"const %s\b[^=]*=\s*\{(.*?)\};" % name, src, re.S)
    if not m:
        sys.exit(f"{path}: no `const {name} = {{...}}` found")
    return {k: int(v) for k, v in re.findall(r"(\w+):\s*(\d+)", m.group(1))}


def themes() -> list[dict]:
    src = THEMES.read_text()
    out = []
    for m in re.finditer(r"\n  \"?([\w-]+)\"?: \{\n(.*?)\n  \},", src, re.S):
        body = m.group(2)

        def field(key: str) -> str | None:
            f = re.search(r'(?<![\w-])%s: "([^"]+)"' % re.escape(key), body)
            return f.group(1) if f else None

        theme = {
            "id": field("id") or m.group(1),
            "dark": "dark: true" in body,
            "shader": field("shader") or "mist",
            "bg": field("background"),
            "primary": field("primary"),
        }
        if theme["bg"] and theme["primary"]:
            out.append(theme)
    if not out:
        sys.exit(f"{THEMES}: no themes parsed; the file's shape changed")
    default = re.search(r'DEFAULT_THEME = "(\w+)"', src)
    return out, (default.group(1) if default else out[0]["id"])


def build() -> str:
    dither_src = DITHER.read_text()
    tile_src = TILE.read_text()
    theme_list, default_theme = themes()
    data = {
        "dither": {
            "vert": template(dither_src, "VERT", DITHER),
            "frag": template(dither_src, "FRAG", DITHER),
            "modes": const_map(dither_src, "SHADER_MODE", DITHER),
        },
        "tile": {
            "vert": template(tile_src, "VERT", TILE),
            "frag": template(tile_src, "FRAG", TILE),
            "modes": {"ember": 0, "sky": 1, "spark": 2},
            "maxSeries": int(re.search(r"MAX_SERIES = (\d+)", tile_src).group(1)),
        },
        "themes": theme_list,
        "defaultTheme": default_theme,
    }
    # </script> inside the JSON would end the data block early.
    payload = json.dumps(data).replace("</", "<\\/")
    return PAGE.replace("__DATA__", payload)


PAGE = r"""<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>shaders</title>
<style>
  html, body { margin: 0; background: #0b0b0d; color: #cfd2d8; font: 12px/1.4 system-ui, sans-serif; }
  #err { display: none; background: #5a1111; color: #ffd4d4; padding: 10px 14px; white-space: pre-wrap; font-family: ui-monospace, monospace; }
  #err.show { display: block; }
  .bar { display: flex; gap: 14px; align-items: baseline; padding: 6px 10px; color: #8a8f99; }
  .bar b { color: #cfd2d8; }
  .bar code { color: #b8bcc6; }
  .sheet { display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: 8px; padding: 0 8px 8px; }
  .cell { position: relative; display: block; text-decoration: none; }
  .cell canvas { display: block; width: 100%; height: 210px; }
  .cell.tile canvas { background: #1a1b21; }
  .cell span { position: absolute; left: 6px; top: 5px; padding: 1px 6px; border-radius: 3px; background: rgba(0,0,0,.55); color: #e6e8ee; }
  .cell small { position: absolute; right: 6px; top: 5px; padding: 1px 6px; border-radius: 3px; background: rgba(0,0,0,.55); color: #a7abb5; }
  .single canvas { display: block; width: 100vw; height: 100vh; }
  .single.tile canvas { background: #1a1b21; }
</style>
<pre id="err"></pre>
<div id="root"></div>
<script id="data" type="application/json">__DATA__</script>
<script>
const DATA = JSON.parse(document.getElementById("data").textContent);
const q = new URLSearchParams(location.search);
const num = (k, d) => (q.has(k) && q.get(k) !== "" ? parseFloat(q.get(k)) : d);
const hex = (s, d) => {
  const m = /^#?([0-9a-f]{6})$/i.exec((s || "").trim());
  if (!m) return d;
  const n = parseInt(m[1], 16);
  return [(n >> 16) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
};
const errors = [];
function fail(where, log) {
  errors.push(where + ":\n" + log);
  console.error("shader harness —", where, "\n" + log);
  const el = document.getElementById("err");
  el.textContent = errors.join("\n\n");
  el.classList.add("show");
  setStatus(); // don't wait for a frame — rAF is paused in hidden tabs
}
function setStatus() {
  const ok = errors.length === 0;
  document.documentElement.dataset.status = ok ? "ok" : "fail";
  document.title = ok ? "shaders ok" : "shaders FAIL";
}

function program(gl, vert, frag, where) {
  const compile = (type, src, label) => {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
      fail(where + " " + label, gl.getShaderInfoLog(s) || "(no log)");
      return null;
    }
    return s;
  };
  const vs = compile(gl.VERTEX_SHADER, vert, "vertex");
  const fs = compile(gl.FRAGMENT_SHADER, frag, "fragment");
  if (!vs || !fs) return null;
  const p = gl.createProgram();
  gl.attachShader(p, vs);
  gl.attachShader(p, fs);
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
    fail(where + " link", gl.getProgramInfoLog(p) || "(no log)");
    return null;
  }
  return p;
}

// Browsers cap live WebGL contexts (~16 in Chromium) and evict the oldest,
// so the sheet can't give each cell its own. Two shared offscreen contexts
// — opaque for the backdrop, alpha for the tiles — render every cell and
// blit into per-cell 2D canvases. One compile per program, as in the app.
const renderers = {};
function renderer(tile) {
  const key = tile ? "tile" : "dither";
  if (key in renderers) return renderers[key];
  const canvas = document.createElement("canvas");
  const gl = canvas.getContext("webgl", tile
    ? { antialias: false, alpha: true, premultipliedAlpha: true }
    : { antialias: false, alpha: false });
  if (!gl) { fail(key, "no WebGL1 context"); return (renderers[key] = null); }
  const src = DATA[key];
  const p = program(gl, src.vert, src.frag, key);
  if (!p) return (renderers[key] = null);
  gl.useProgram(p);
  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(p, "a_pos");
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  const u = (n) => gl.getUniformLocation(p, n);
  const U = {
    res: u("u_res"), time: u("u_time"), tint: u("u_tint"), gain: u("u_gain"), mode: u("u_mode"),
    bg: u("u_bg"), density: u("u_density"), hour: u("u_hour"), n: u("u_n"),
    // "u_series[0]" is the array's canonical name — some GL stacks return
    // null for the bare name, and uniform1fv on null is a silent no-op.
    series: u("u_series[0]") ?? u("u_series"),
  };
  const draw = (spec, w, h, t) => {
    if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
    gl.viewport(0, 0, w, h);
    gl.uniform2f(U.res, w, h);
    gl.uniform1f(U.time, t);
    gl.uniform1f(U.gain, spec.gain);
    gl.uniform1f(U.mode, spec.mode);
    gl.uniform3fv(U.tint, spec.tint);
    if (tile) {
      gl.uniform1f(U.hour, spec.hour);
      gl.uniform1fv(U.series, spec.packed);
      gl.uniform1f(U.n, spec.n);
    } else {
      gl.uniform3fv(U.bg, spec.bg);
      gl.uniform1f(U.density, spec.density);
    }
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  };
  return (renderers[key] = { canvas, draw });
}

// Mirrors the components: dpr capped at 1.5, a 33ms frame throttle, and
// `t` freezing u_time for deterministic shots.
function mount(target, spec) {
  const r = renderer(spec.tile);
  if (!r) return;
  const ctx = target.getContext("2d");
  const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
  const frozen = spec.t !== null;
  const start = performance.now();
  let last = 0;
  const paint = (t) => {
    const w = Math.max(1, Math.floor(target.clientWidth * dpr));
    const h = Math.max(1, Math.floor(target.clientHeight * dpr));
    if (target.width !== w || target.height !== h) { target.width = w; target.height = h; }
    r.draw(spec, w, h, t);
    ctx.clearRect(0, 0, w, h);
    ctx.drawImage(r.canvas, 0, 0);
  };
  const frame = (now) => {
    if (!frozen) requestAnimationFrame(frame);
    if (now - last < 33) return;
    last = now;
    paint(frozen ? spec.t : (now - start) / 1000);
  };
  requestAnimationFrame(frame);
  new ResizeObserver(() => { if (frozen) paint(spec.t); }).observe(target);
}

const themeById = Object.fromEntries(DATA.themes.map((t) => [t.id, t]));
const pickedTheme = q.get("theme") ? themeById[q.get("theme")] : null;
if (q.get("theme") && !pickedTheme) fail("theme", "unknown theme id " + q.get("theme"));
const themeFor = (mode) =>
  pickedTheme
  || (mode === "mist" ? themeById[DATA.defaultTheme] : DATA.themes.find((t) => t.shader === mode))
  || themeById[DATA.defaultTheme];

const shared = {
  gain: num("gain", 1),
  density: Math.min(1, Math.max(0, num("density", 0.5))),
  hour: num("hour", 14),
  t: q.has("t") ? num("t", 0) : null,
};
const defaultSeries = [0.02, 0.05, 0.07, 0.12, 0.14, 0.2, 0.22, 0.31, 0.36, 0.4, 0.48, 0.55, 0.58, 0.66, 0.71, 0.8, 0.86, 0.9, 1];
const series = q.get("series") ? q.get("series").split(",").map(parseFloat).filter((v) => !isNaN(v)) : defaultSeries;
const TILE_TINT = [0.91, 0.64, 0.24];

function ditherSpec(modeName, theme) {
  return {
    ...shared, tile: false,
    label: "dither/" + modeName,
    mode: DATA.dither.modes[modeName],
    tint: hex(q.get("tint"), hex(theme.primary)),
    bg: hex(q.get("bg"), hex(theme.bg)),
  };
}
function tileSpec(modeName, extra = {}) {
  const pts = series.slice(-DATA.tile.maxSeries).map((v) => Math.max(0, Math.min(1, v)));
  const packed = new Float32Array(DATA.tile.maxSeries);
  packed.set(pts);
  return {
    ...shared, tile: true, packed, n: Math.max(2, pts.length),
    label: "tile/" + modeName,
    mode: DATA.tile.modes[modeName],
    tint: hex(q.get("tint"), TILE_TINT),
    ...extra,
  };
}

const root = document.getElementById("root");
const shader = q.get("shader") === "tile" ? "tile" : "dither";
const modeParam = q.get("mode");
const modes = DATA[shader].modes;
const modeName = modeParam && !isNaN(parseFloat(modeParam))
  ? Object.keys(modes).find((k) => modes[k] === parseInt(modeParam, 10))
  : modeParam;

if (modeParam && modeParam !== "all") {
  if (!(modeName in modes)) {
    fail("mode", "unknown " + shader + " mode " + modeParam + " — one of " + Object.keys(modes).join(", "));
  } else {
    root.className = "single " + shader;
    const c = document.createElement("canvas");
    root.appendChild(c);
    mount(c, shader === "tile" ? tileSpec(modeName) : ditherSpec(modeName, themeFor(modeName)));
  }
} else {
  const bar = document.createElement("div");
  bar.className = "bar";
  bar.innerHTML = "<b>Alchemy shaders</b> <span>" + Object.keys(DATA.dither.modes).length
    + " dither modes · 3 tile fields · " + DATA.themes.length + " themes</span>"
    + "<code>?mode=&lt;name&gt; ?theme=&lt;id&gt; ?t=&lt;s&gt; ?shader=tile</code>";
  root.appendChild(bar);
  const sheet = document.createElement("div");
  sheet.className = "sheet";
  root.appendChild(sheet);
  const cell = (href, title, note, spec, cls) => {
    const a = document.createElement("a");
    a.className = "cell " + cls;
    a.href = href;
    const c = document.createElement("canvas");
    a.appendChild(c);
    a.insertAdjacentHTML("beforeend", "<span>" + title + "</span><small>" + note + "</small>");
    sheet.appendChild(a);
    mount(c, spec);
  };
  const keep = new URLSearchParams(q);
  keep.delete("mode"); keep.delete("shader");
  const tail = keep.toString() ? "&" + keep.toString() : "";
  for (const name of Object.keys(DATA.dither.modes)) {
    const theme = themeFor(name);
    cell("?mode=" + name + tail, name, theme.id, ditherSpec(name, theme), "dither");
  }
  cell("?shader=tile&mode=ember" + tail, "ember", "tile", tileSpec("ember"), "tile");
  for (const h of [7, 13, 20]) {
    cell("?shader=tile&mode=sky&hour=" + h + tail, "sky", "hour " + h, tileSpec("sky", { hour: h }), "tile");
  }
  cell("?shader=tile&mode=spark" + tail, "spark", series.length + " pts", tileSpec("spark"), "tile");
}
// Compile results are in by the time the first frame is requested.
requestAnimationFrame(() => requestAnimationFrame(setStatus));
</script>
"""


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", type=Path, default=Path(tempfile.gettempdir()) / "alchemy-shader-harness",
                    help="directory to write index.html into (default: $TMPDIR/alchemy-shader-harness)")
    ap.add_argument("--serve", nargs="?", const=8791, type=int, metavar="PORT",
                    help="serve the page on 127.0.0.1:PORT (default 8791) until Ctrl-C")
    ap.add_argument("--open", action="store_true", help="open the page in the default browser")
    args = ap.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    page = args.out / "index.html"
    page.write_text(build())
    print(f"wrote {page}")

    url = f"http://127.0.0.1:{args.serve}/" if args.serve else page.as_uri()
    if args.open:
        subprocess.run(["open", url], check=False)
    if not args.serve:
        print(url)
        return

    os.chdir(args.out)
    handler = http.server.SimpleHTTPRequestHandler
    handler.log_message = lambda *a, **k: None  # type: ignore[method-assign]
    with http.server.ThreadingHTTPServer(("127.0.0.1", args.serve), handler) as srv:
        print(f"serving {url}   (Ctrl-C to stop)")
        print(f"  {url}?mode=network&theme=metropolis   one mode, full window")
        print(f"  {url}?shader=tile&mode=sky&hour=20    a tile field")
        try:
            srv.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
