import { useEffect, useMemo, useRef, useState } from "react";
import { Minus, Plus, RotateCcw } from "lucide-react";
import { Markdown } from "./Markdown";

/**
 * Native mind-map renderer. The generator emits a plain indented outline
 * (which even small local models produce reliably); we do the visual work
 * here instead of asking the model for Mermaid or SVG, so a mind map never
 * arrives broken. Falls back to Markdown when the content doesn't parse.
 */

interface MNode {
  label: string;
  children: MNode[];
}

/** Parse an indented `- ` outline (first line = root) into a tree. */
export function parseOutline(md: string): MNode | null {
  const lines = md
    .split("\n")
    .map((l) => l.replace(/\s+$/, ""))
    .filter((l) => l.trim() && !/^```/.test(l.trim()));

  let root: MNode | null = null;
  // Stack of (node, indent) — the parent of a bullet is the nearest
  // shallower entry. Indent -1 marks the root so top-level bullets nest.
  const stack: { node: MNode; indent: number }[] = [];

  for (const raw of lines) {
    const m = /^(\s*)[-*•]\s+(.*)$/.exec(raw);
    if (!m) {
      // First non-bullet line becomes the central topic; later prose is noise.
      if (!root) {
        const label = raw.replace(/^#+\s*/, "").replace(/[*_`]/g, "").trim();
        if (label) {
          root = { label, children: [] };
          stack.push({ node: root, indent: -1 });
        }
      }
      continue;
    }
    const indent = m[1].replace(/\t/g, "  ").length;
    const label = m[2].replace(/[*_`]/g, "").trim();
    if (!label) continue;
    if (!root) {
      root = { label: "Mind map", children: [] };
      stack.push({ node: root, indent: -1 });
    }
    while (stack.length > 1 && indent <= stack[stack.length - 1].indent) stack.pop();
    const node: MNode = { label, children: [] };
    stack[stack.length - 1].node.children.push(node);
    stack.push({ node, indent });
  }
  return root && root.children.length > 0 ? root : null;
}

// Layout constants tuned for the 11px label font below.
const CHAR_W = 6.6;
const LINE_H = 15;
const PAD_X = 10;
const PAD_Y = 7;
const COL_GAP = 40;
const ROW_GAP = 10;
const WRAP_AT = 22;

function wrapLabel(label: string): string[] {
  const words = label.split(/\s+/);
  const lines: string[] = [];
  let cur = "";
  for (const w of words) {
    if (cur && (cur + " " + w).length > WRAP_AT) {
      lines.push(cur);
      cur = w;
    } else {
      cur = cur ? `${cur} ${w}` : w;
    }
  }
  if (cur) lines.push(cur);
  if (lines.length > 2) {
    lines.length = 2;
    lines[1] = `${lines[1].slice(0, WRAP_AT - 1)}…`;
  }
  return lines;
}

interface Laid {
  lines: string[];
  depth: number;
  x: number;
  y: number; // center
  w: number;
  h: number;
  children: Laid[];
}

function layout(root: MNode): { root: Laid; width: number; height: number } {
  const colWidth: number[] = [];

  const measure = (n: MNode, depth: number): Laid => {
    const lines = wrapLabel(n.label);
    const w = Math.max(...lines.map((l) => l.length)) * CHAR_W + PAD_X * 2;
    const h = lines.length * LINE_H + PAD_Y * 2;
    colWidth[depth] = Math.max(colWidth[depth] ?? 0, w);
    return { lines, depth, x: 0, y: 0, w, h, children: n.children.map((c) => measure(c, depth + 1)) };
  };
  const laid = measure(root, 0);

  // Column x positions: each depth starts after the widest node before it.
  const colX: number[] = [0];
  for (let d = 0; d < colWidth.length; d++) colX[d + 1] = colX[d] + colWidth[d] + COL_GAP;

  // Leaves stack top-to-bottom; every parent centers on its children.
  let cursor = 0;
  const place = (n: Laid) => {
    n.x = colX[n.depth];
    if (n.children.length === 0) {
      n.y = cursor + n.h / 2;
      cursor += n.h + ROW_GAP;
      return;
    }
    n.children.forEach(place);
    n.y = (n.children[0].y + n.children[n.children.length - 1].y) / 2;
  };
  place(laid);

  const maxDepth = colWidth.length - 1;
  return {
    root: laid,
    width: colX[maxDepth] + colWidth[maxDepth],
    height: Math.max(cursor - ROW_GAP, laid.h),
  };
}

function flatten(n: Laid, out: Laid[] = []): Laid[] {
  out.push(n);
  n.children.forEach((c) => flatten(c, out));
  return out;
}

const MARGIN = 8;

export function MindMap({ content }: { content: string }) {
  const laid = useMemo(() => {
    const tree = parseOutline(content);
    return tree ? layout(tree) : null;
  }, [content]);

  // Model produced something that isn't an outline — show it as markdown
  // rather than nothing.
  if (!laid) return <Markdown>{content}</Markdown>;

  const nodes = flatten(laid.root);
  return (
    <PanCanvas>
      <svg
        width={laid.width + MARGIN * 2}
        height={laid.height + MARGIN * 2}
        viewBox={`${-MARGIN} ${-MARGIN} ${laid.width + MARGIN * 2} ${laid.height + MARGIN * 2}`}
        role="img"
        aria-label="Mind map"
        className="max-w-none font-sans"
      >
        {nodes.map((n, i) => (
          <g key={i}>
            {n.children.map((c, j) => (
              <path
                key={j}
                d={`M ${n.x + n.w} ${n.y} C ${n.x + n.w + COL_GAP / 2} ${n.y}, ${
                  c.x - COL_GAP / 2
                } ${c.y}, ${c.x} ${c.y}`}
                fill="none"
                stroke="var(--primary)"
                strokeOpacity={0.45}
                strokeWidth={1.2}
              />
            ))}
            <rect
              x={n.x}
              y={n.y - n.h / 2}
              width={n.w}
              height={n.h}
              rx={7}
              fill={n.depth === 0 ? "var(--primary)" : "var(--surface-2)"}
              fillOpacity={n.depth === 0 ? 0.18 : 1}
              stroke={n.depth <= 1 ? "var(--primary)" : "var(--border-strong)"}
              strokeOpacity={n.depth === 0 ? 0.9 : n.depth === 1 ? 0.5 : 1}
            />
            <text
              x={n.x + n.w / 2}
              y={n.y - ((n.lines.length - 1) * LINE_H) / 2}
              textAnchor="middle"
              dominantBaseline="central"
              fontSize={11}
              fontWeight={n.depth <= 1 ? 600 : 400}
              fill="var(--foreground)"
            >
              {n.lines.map((l, k) => (
                <tspan key={k} x={n.x + n.w / 2} dy={k === 0 ? 0 : LINE_H}>
                  {l}
                </tspan>
              ))}
            </text>
          </g>
        ))}
      </svg>
    </PanCanvas>
  );
}

/** Infinite-canvas panning and zooming (Photoshop-style): drag with a grab
 *  cursor or two-finger scroll to move; pinch (ctrl+wheel on macOS) or the
 *  corner buttons to zoom around the cursor. */
function PanCanvas({ children }: { children: React.ReactNode }) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ x: 0, y: 0, scale: 1 });
  const drag = useRef<{ x: number; y: number; ox: number; oy: number } | null>(null);

  /** Zoom by a factor keeping the given viewport point fixed. */
  const zoomAt = (factor: number, cx: number, cy: number) => {
    setView((v) => {
      const scale = Math.min(3, Math.max(0.25, v.scale * factor));
      const k = scale / v.scale;
      return { scale, x: cx - (cx - v.x) * k, y: cy - (cy - v.y) * k };
    });
  };

  // Native wheel listener: React's synthetic wheel can't preventDefault
  // (passive), and the page behind must not scroll while panning. Trackpad
  // pinch arrives as wheel events with ctrlKey set.
  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (e.ctrlKey || e.metaKey) {
        const rect = el.getBoundingClientRect();
        setView((v) => {
          const factor = Math.exp(-e.deltaY * 0.01);
          const scale = Math.min(3, Math.max(0.25, v.scale * factor));
          const k = scale / v.scale;
          const cx = e.clientX - rect.left;
          const cy = e.clientY - rect.top;
          return { scale, x: cx - (cx - v.x) * k, y: cy - (cy - v.y) * k };
        });
      } else {
        setView((v) => ({ ...v, x: v.x - e.deltaX, y: v.y - e.deltaY }));
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const center = (): [number, number] => {
    const r = viewportRef.current?.getBoundingClientRect();
    return r ? [r.width / 2, r.height / 2] : [0, 0];
  };

  return (
    <div
      ref={viewportRef}
      className="relative h-full min-h-[320px] w-full cursor-grab touch-none select-none overflow-hidden active:cursor-grabbing"
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        drag.current = { x: e.clientX, y: e.clientY, ox: view.x, oy: view.y };
        (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        setView((v) => ({
          ...v,
          x: d.ox + (e.clientX - d.x),
          y: d.oy + (e.clientY - d.y),
        }));
      }}
      onPointerUp={() => {
        drag.current = null;
      }}
      onPointerCancel={() => {
        drag.current = null;
      }}
    >
      <div
        style={{
          transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
          transformOrigin: "0 0",
        }}
        className="w-max"
      >
        {children}
      </div>
      <div
        className="absolute bottom-3 right-3 z-10 flex items-center gap-0.5 rounded-md border border-border/60 bg-elevated/80 p-0.5 backdrop-blur"
        onPointerDown={(e) => e.stopPropagation()}
      >
        <button
          type="button"
          onClick={() => zoomAt(1 / 1.25, ...center())}
          title="Zoom out"
          aria-label="Zoom out"
          className="rounded p-1 text-muted-foreground transition-colors hover:text-foreground"
        >
          <Minus className="h-3.5 w-3.5" />
        </button>
        <span className="min-w-10 px-1 py-0.5 text-center text-[11px] tabular-nums text-subtle-foreground">
          {Math.round(view.scale * 100)}%
        </span>
        <button
          type="button"
          onClick={() => zoomAt(1.25, ...center())}
          title="Zoom in"
          aria-label="Zoom in"
          className="rounded p-1 text-muted-foreground transition-colors hover:text-foreground"
        >
          <Plus className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          onClick={() => setView({ x: 0, y: 0, scale: 1 })}
          title="Reset to 100%"
          aria-label="Reset zoom to 100%"
          className="rounded p-1 text-muted-foreground transition-colors hover:text-foreground"
        >
          <RotateCcw className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}
