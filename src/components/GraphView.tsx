import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { createLayout, layout } from "@/lib/forceLayout";
import { placeLabels } from "@/lib/graphLabels";
import type { NotebookGraph } from "@/lib/types";
import { EmptyState, useHoverCard } from "./ui";
import { sourceHoverData } from "./SourcesPanel";
import { relativeTime } from "@/lib/utils";
import { Crosshair, Minus, Plus, Share2 } from "lucide-react";

/**
 * The notebook as a link graph (docs/RFC-document-surface.md phase 5):
 * sources and notes as nodes, references between them as edges. The backend
 * finds the edges the three ways documents actually refer to each other —
 * absolute URL, bare filename, `[[wikilink]]` — so nothing here asks the user
 * to link anything a new way.
 *
 * Rendered as SVG rather than canvas: a few hundred nodes is nothing for the
 * DOM, and it gets hit-testing, focus, and the theme's own colors for free.
 */

/** Built graphs and their settled layouts, per notebook, for this app run.
 *  Re-entering the graph should be instant — the work was already done, and
 *  redoing it from scratch on every visit is what made the pane feel like it
 *  was thinking. Keyed by notebook + document set, so it survives navigation
 *  but never serves a stale picture. Same lifetime and reasoning as the
 *  gallery's thumbMemory. */
const graphCache = new Map<string, NotebookGraph>();
const layoutCache = new Map<string, ReturnType<typeof layout>>();

/** Pan and zoom per notebook, so stepping out to a document and coming back
 *  returns you to where you were looking rather than to the top of the world.
 *  Same app-run lifetime and the same reasoning as the gallery's
 *  scrollMemory. Re-center is always one click away if a remembered view
 *  turns out to be somewhere you no longer want to be. */
const viewMemory = new Map<string, { x: number; y: number; k: number }>();

/** Zoom limits. Out far enough to see a 400-node notebook whole, in far
 *  enough to read a label in the middle of a dense cluster. */
const MIN_ZOOM = 0.35;
/** 12x. A 330-document notebook is still shoulder-to-shoulder at 4x — the
 *  ceiling has to clear the densest core, not the average case. */
const MAX_ZOOM = 12;
/** Wheel sensitivity. Trackpads send small deltas per event, so this needs
 *  to be far higher than a mouse wheel would want: a normal two-finger swipe
 *  should cross most of the range, not inch across it. */
const ZOOM_SPEED = 0.01;
/** Label size in screen pixels, held roughly constant across zoom. */
const LABEL_PX = 10;

/**
 * How much to shrink glyphs as the view grows.
 *
 * Nodes and labels live inside the zoom transform, so by default they scale
 * with it — zoom into a clump and you get a bigger clump, with the labels
 * overlapping exactly as much as before. Counter-scaling by 1/k holds them
 * at a constant SCREEN size while the positions spread apart, which is what
 * makes zooming actually resolve a cluster.
 *
 * The lower clamp is 1/MAX_ZOOM rather than a round number on purpose: any
 * floor above it means counter-scaling quits partway up the range and the
 * glyphs start growing on screen again, which is the exact problem this
 * function exists to prevent. Tie it to the ceiling and the whole range is
 * honest constant-size. The upper clamp stops a zoomed-out graph inflating
 * into nothing but dots.
 */
function glyphScale(k: number) {
  return Math.min(1.3, Math.max(1 / MAX_ZOOM, 1 / k));
}

export function GraphView() {
  const currentId = useStore((s) => s.currentId);
  const sources = useStore((s) => s.sources);
  const notes = useStore((s) => s.notes);
  const openInReader = useStore((s) => s.openInReader);
  const { show: showCard, hide: hideCard, card: hoverCard } = useHoverCard("right");
  const [graph, setGraph] = useState<NotebookGraph | null>(null);
  const [hovered, setHovered] = useState<string | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [view, setView] = useState({ x: 0, y: 0, k: 1 });
  const [progress, setProgress] = useState(0);
  const viewRef = useRef(view);
  viewRef.current = view;
  const commitViewRef = useRef<(v: { x: number; y: number; k: number }) => void>(
    () => {},
  );

  /** Set the view AND remember it. Every pan and zoom goes through here so
   *  there is no path that moves the camera without recording where. */
  const commitView = (next: { x: number; y: number; k: number }) => {
    setView(next);
    if (currentId) viewMemory.set(currentId, next);
  };
  commitViewRef.current = commitView;
  const boxRef = useRef<HTMLDivElement | null>(null);
  const svgRef = useRef<SVGSVGElement | null>(null);
  /** Set while dragging; null otherwise. Held in a ref so the move handler
   *  doesn't re-subscribe on every frame. */
  const panning = useRef<{ x: number; y: number; vx: number; vy: number } | null>(
    null,
  );

  // The graph is derived from the notebook's documents, so it has to be
  // refetched when they change — not just when the notebook does. Keying on
  // the id/title set (rather than the arrays) means a re-render that doesn't
  // touch documents costs nothing, but adding, deleting, or renaming one
  // rebuilds the graph.
  const docKey = useMemo(
    () =>
      [
        ...sources.map((s) => `s${s.id}:${s.title}`),
        ...notes.map((n) => `n${n.id}:${n.title}`),
      ].join("|"),
    [sources, notes],
  );

  const cacheKey = `${currentId}:${docKey}`;
  useEffect(() => {
    if (!currentId) return;
    const hit = graphCache.get(cacheKey);
    if (hit) {
      setGraph(hit);
      return;
    }
    let stale = false;
    setGraph(null);
    void api
      .notebookGraph(currentId)
      .then((g) => {
        graphCache.set(cacheKey, g);
        if (!stale) setGraph(g);
      })
      .catch(() => !stale && setGraph({ nodes: [], edges: [] }));
    return () => {
      stale = true;
    };
  }, [currentId, cacheKey]);

  // Come back to where this notebook was last left, not to the origin.
  useEffect(() => {
    if (!currentId) return;
    setView(viewMemory.get(currentId) ?? { x: 0, y: 0, k: 1 });
  }, [currentId]);

  useEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    const measure = () =>
      setSize({ width: el.clientWidth, height: el.clientHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // The simulation is synchronous and deterministic, so it only needs to run
  // when the graph or the box actually changes — never on pan or zoom.
  //
  // Deferred a frame rather than computed inline: a few hundred nodes is a
  // few hundred milliseconds of blocking force simulation, and running it in
  // the same commit means the "Laying out…" line never paints — the pane
  // just freezes, which is exactly what it looks like when nothing is
  // happening at all.
  const [positions, setPositions] = useState<ReturnType<typeof layout>>([]);

  /** Shown for every layout pass, not just a slow one — watching the graph
   *  settle is honest feedback about what the pane is doing, and the bar is
   *  a real fraction rather than a spinner pretending. */
  const loading = !graph || (graph.nodes.length > 0 && positions.length === 0);
  useEffect(() => {
    if (!graph || !size.width || !size.height) {
      setPositions([]);
      return;
    }
    const key = `${cacheKey}:${size.width}x${size.height}`;
    const hit = layoutCache.get(key);
    if (hit) {
      setPositions(hit);
      return;
    }
    // Driven a slice per frame rather than in one call: the fraction below
    // is real, and the window keeps answering the pointer while it settles.
    let stale = false;
    let frame = 0;
    const run = createLayout(graph.nodes, graph.edges, size.width, size.height);
    setProgress(0);
    const pump = () => {
      if (stale) return;
      // A tick budget rather than a time budget — the cost per tick is
      // stable for a given graph, and a wall-clock budget would make the
      // layout itself depend on how busy the machine happened to be, which
      // costs determinism for nothing.
      const ticksPerFrame = Math.max(1, Math.round(4000 / graph.nodes.length));
      const done = run.step(ticksPerFrame);
      setProgress(run.progress());
      if (done) {
        const next = run.result();
        layoutCache.set(key, next);
        setPositions(next);
        return;
      }
      frame = requestAnimationFrame(pump);
    };
    frame = requestAnimationFrame(pump);
    return () => {
      stale = true;
      cancelAnimationFrame(frame);
    };
  }, [graph, size.width, size.height, cacheKey]);

  const nodeById = useMemo(
    () => new Map(positions.map((p) => [p.id, p])),
    [positions],
  );
  const meta = useMemo(
    () => new Map((graph?.nodes ?? []).map((n) => [n.id, n])),
    [graph],
  );

  const glyph = glyphScale(view.k);
  /** Node radius in graph units — screen-constant via the counter-scale. */
  const radiusOf = (degree: number) =>
    (5 + Math.min(9, Math.sqrt(degree) * 2.6)) * glyph;

  /** Which labels can be drawn without colliding. Recomputed only when the
   *  layout changes; zoom just relaxes the rule. */
  const visibleLabels = useMemo(
    () =>
      placeLabels(
        positions.map((p) => ({
          id: p.id,
          x: p.x,
          y: p.y + radiusOf(p.degree) + 12 * glyph,
          text: labelOf(meta.get(p.id)?.title ?? ""),
          weight: p.degree,
        })),
        glyph,
      ),
    // `glyph` is a dependency on purpose: zooming in shrinks the boxes in
    // graph space, so collisions resolve and more labels earn their place —
    // no threshold needed, the geometry decides.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [positions, meta, glyph],
  );

  /** What the hover card shows for a node. Sources reuse the sidebar's own
   *  card builder, so a source reads identically wherever you meet it;
   *  notes get the equivalent built from what a note actually has. */
  const cardFor = (id: string, kind: string) => {
    if (kind === "note") {
      const note = notes.find((n) => n.id === id);
      if (!note) return null;
      const links = (graph?.edges ?? []).filter(
        (e) => e.from === id || e.to === id,
      ).length;
      return {
        title: note.title,
        time: relativeTime(note.updatedAt),
        meta: [
          { label: note.kind.replace(/_/g, " ") },
          { label: "Links", value: `${links}` },
          { label: "Size", value: `${note.content.length} chars` },
        ],
      };
    }
    const source = sources.find((x) => x.id === id);
    if (!source) return null;
    const links = (graph?.edges ?? []).filter(
      (e) => e.from === id || e.to === id,
    ).length;
    const data = sourceHoverData(source);
    // The one fact the graph adds over every other surface.
    return { ...data, meta: [{ label: "Links", value: `${links}` }, ...data.meta] };
  };

  /** Nodes one hop from the hovered one — everything else dims. */
  const neighbors = useMemo(() => {
    if (!hovered || !graph) return null;
    const set = new Set<string>([hovered]);
    for (const e of graph.edges) {
      if (e.from === hovered) set.add(e.to);
      if (e.to === hovered) set.add(e.from);
    }
    return set;
  }, [hovered, graph]);

  // Wheel zooms about the pointer, so the thing under the cursor stays under
  // the cursor — the behaviour every map has trained people to expect.
  //
  // Attached natively with { passive: false } rather than as React's onWheel:
  // React registers wheel at the root as PASSIVE, so preventDefault there is
  // ignored and the pane scrolls while you zoom. MindMap.tsx hit this first
  // and solves it the same way.
  useEffect(() => {
    const el = svgRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const px = e.clientX - rect.left;
      const py = e.clientY - rect.top;
      const v = viewRef.current;
      const k = Math.max(
        MIN_ZOOM,
        Math.min(MAX_ZOOM, v.k * Math.exp(-e.deltaY * ZOOM_SPEED)),
      );
      if (k === v.k) return;
      // Keep the graph-space point under the cursor fixed across the change.
      const scale = k / v.k;
      commitViewRef.current({
        k,
        x: px - (px - v.x) * scale,
        y: py - (py - v.y) * scale,
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [positions.length]);

  const onPointerDown = (e: React.PointerEvent<SVGSVGElement>) => {
    // Left button only, and never start a pan on a node — that is a click.
    if (e.button !== 0 || (e.target as Element).closest("[data-node]")) return;
    e.preventDefault();
    panning.current = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y };
    svgRef.current?.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const p = panning.current;
    if (!p) return;
    commitView({
      ...viewRef.current,
      x: p.vx + (e.clientX - p.x),
      y: p.vy + (e.clientY - p.y),
    });
  };
  const endPan = (e: React.PointerEvent<SVGSVGElement>) => {
    panning.current = null;
    svgRef.current?.releasePointerCapture?.(e.pointerId);
  };

  /** Buttons zoom about the middle of the pane, the way the wheel zooms
   *  about the pointer — otherwise the view lurches sideways on every tap. */
  const zoomBy = (factor: number) => {
    const v = viewRef.current;
    const k = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, v.k * factor));
    if (k === v.k) return;
    const cx = size.width / 2;
    const cy = size.height / 2;
    const scale = k / v.k;
    commitView({ k, x: cx - (cx - v.x) * scale, y: cy - (cy - v.y) * scale });
  };


  return (
    <div ref={boxRef} className="relative min-h-0 flex-1 overflow-hidden">
      {loading && (
        <div className="flex h-full flex-col items-center justify-center gap-2">
          <span className="text-caption text-muted-foreground">
            {!graph
              ? "Reading the notebook…"
              : `Laying out ${graph.nodes.length} documents…`}
          </span>
          <div className="h-1 w-48 overflow-hidden rounded-full bg-surface-2">
            <div
              className="h-full rounded-full bg-primary transition-[width] duration-150"
              // Reading the notebook has no measurable fraction, so it shows
              // a fixed sliver rather than a fake crawl; the layout half is
              // the real number.
              style={{ width: `${graph ? Math.round(progress * 100) : 8}%` }}
            />
          </div>
        </div>
      )}
      {graph && graph.nodes.length === 0 && (
        <EmptyState
          icon={<Share2 className="h-5 w-5" />}
          title="Nothing to graph yet"
          hint="Links between sources and notes show up here — a URL, a filename, or a [[wikilink]]."
        />
      )}
      {positions.length > 0 && (
        <svg
          ref={svgRef}
          width={size.width}
          height={size.height}
          // select-none: dragging to pan otherwise sweeps a text selection
          // across every SVG <text> in the graph, painting the pane in the
          // theme's selection colour with darker blocks over each label.
          className="touch-none select-none text-muted-foreground"
          style={{ cursor: panning.current ? "grabbing" : "grab" }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endPan}
          onPointerCancel={endPan}
          role="img"
          aria-label="Notebook link graph"
        >
          <defs>
            {/* Edges are directed in the data; without a head they read as
                mutual, which is the opposite of what a citation means. */}
            <marker
              id="graph-arrow"
              viewBox="0 0 8 8"
              refX="7"
              refY="4"
              markerWidth="5"
              markerHeight="5"
              orient="auto-start-reverse"
            >
              <path d="M 0 1 L 7 4 L 0 7 z" fill="currentColor" />
            </marker>
          </defs>
          <g transform={`translate(${view.x} ${view.y}) scale(${view.k})`}>
            {graph?.edges.map((e, i) => {
              const a = nodeById.get(e.from);
              const b = nodeById.get(e.to);
              if (!a || !b) return null;
              const lit =
                !neighbors || (neighbors.has(e.from) && neighbors.has(e.to));
              // Stop the line short of the target so the arrowhead sits
              // against the circle instead of buried under it.
              const dx = b.x - a.x;
              const dy = b.y - a.y;
              const d = Math.hypot(dx, dy) || 1;
              const gap = radiusOf(b.degree) + 5;
              return (
                <line
                  key={i}
                  x1={a.x}
                  y1={a.y}
                  x2={b.x - (dx / d) * gap}
                  y2={b.y - (dy / d) * gap}
                  stroke="currentColor"
                  strokeWidth={1 * glyph}
                  opacity={lit ? 0.45 : 0.08}
                  markerEnd={lit ? "url(#graph-arrow)" : undefined}
                />
              );
            })}
            {positions.map((p) => {
              const node = meta.get(p.id);
              if (!node) return null;
              // Hubs read larger, but sub-linearly — one note linked forty
              // times shouldn't dwarf the rest of the notebook.
              const r = radiusOf(p.degree);
              const lit = !neighbors || neighbors.has(p.id);
              const isNote = node.kind === "note";
              const labelled =
                lit && (visibleLabels.has(p.id) || hovered === p.id);
              return (
                <g
                  key={p.id}
                  data-node
                  transform={`translate(${p.x} ${p.y})`}
                  opacity={lit ? 1 : 0.22}
                  className="cursor-pointer"
                  onMouseEnter={(e) => {
                    setHovered(p.id);
                    const data = cardFor(p.id, node.kind);
                    if (data) showCard(e, data);
                  }}
                  onMouseLeave={() => {
                    setHovered(null);
                    hideCard();
                  }}
                  onClick={() =>
                    openInReader({ type: isNote ? "note" : "source", id: p.id })
                  }
                >
                  {/* Notes are hollow, sources solid — the same distinction
                      the sidebar makes, carried by shape not a legend. */}
                  <circle
                    r={r}
                    className={isNote ? "fill-background" : "fill-primary"}
                    stroke="currentColor"
                    strokeWidth={1.5 * glyph}
                  />
                  {labelled && (
                    <text
                      y={r + 12 * glyph}
                      textAnchor="middle"
                      fontSize={LABEL_PX * glyph}
                      className="pointer-events-none fill-current"
                      opacity={hovered === p.id ? 1 : 0.75}
                    >
                      {labelOf(node.title)}
                    </text>
                  )}
                </g>
              );
            })}
          </g>
        </svg>
      )}
      {positions.length > 0 && (
        <div className="absolute bottom-3 right-3 flex items-center gap-0.5 rounded-lg border border-border bg-surface-2/90 p-0.5 backdrop-blur">
          <ZoomButton
            label="Zoom out"
            onClick={() => zoomBy(1 / 1.3)}
            icon={<Minus className="h-3.5 w-3.5" />}
          />
          <button
            type="button"
            onClick={() => zoomBy(1 / view.k)}
            title="Zoom to 100%"
            className="rounded-md px-2 py-1 text-micro font-medium tabular-nums text-muted-foreground transition-colors hover:text-foreground"
          >
            {Math.round(view.k * 100)}%
          </button>
          <ZoomButton
            label="Zoom in"
            onClick={() => zoomBy(1.3)}
            icon={<Plus className="h-3.5 w-3.5" />}
          />
          <span aria-hidden className="mx-0.5 h-3.5 w-px bg-border-strong" />
          {/* Pan far enough and the graph is off-screen with no landmark to
              steer back by — this is the way home. */}
          <ZoomButton
            label="Re-center"
            onClick={() => commitView({ x: 0, y: 0, k: 1 })}
            icon={<Crosshair className="h-3.5 w-3.5" />}
          />
        </div>
      )}
      {hoverCard}
    </div>
  );
}

function ZoomButton({
  label,
  onClick,
  icon,
}: {
  label: string;
  onClick: () => void;
  icon: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={label}
      aria-label={label}
      className="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-surface hover:text-foreground"
    >
      {icon}
    </button>
  );
}

/** Titles are long and the graph is not the place to read one in full. */
function labelOf(title: string) {
  return title.length > 26 ? `${title.slice(0, 25)}…` : title;
}
