import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { layout } from "@/lib/forceLayout";
import { placeLabels } from "@/lib/graphLabels";
import type { NotebookGraph } from "@/lib/types";
import { EmptyState, useHoverCard } from "./ui";
import { sourceHoverData } from "./SourcesPanel";
import { relativeTime } from "@/lib/utils";
import { Share2 } from "lucide-react";

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

/** Zoom limits. Out far enough to see a 400-node notebook whole, in far
 *  enough to read a label in the middle of a dense cluster. */
const MIN_ZOOM = 0.35;
const MAX_ZOOM = 4;
/** Past this zoom the nodes have separated enough that every label fits, so
 *  the collision cull stops hiding them. */
const ALL_LABELS_ZOOM = 1.6;

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

  useEffect(() => {
    if (!currentId) return;
    let stale = false;
    void api
      .notebookGraph(currentId)
      .then((g) => !stale && setGraph(g))
      .catch(() => !stale && setGraph({ nodes: [], edges: [] }));
    return () => {
      stale = true;
    };
  }, [currentId, docKey]);

  // A new notebook is a new picture — don't inherit the last one's pan.
  useEffect(() => setView({ x: 0, y: 0, k: 1 }), [currentId]);

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
  useEffect(() => {
    if (!graph || !size.width || !size.height) {
      setPositions([]);
      return;
    }
    let stale = false;
    const id = requestAnimationFrame(() => {
      const next = layout(graph.nodes, graph.edges, size.width, size.height);
      if (!stale) setPositions(next);
    });
    return () => {
      stale = true;
      cancelAnimationFrame(id);
    };
  }, [graph, size.width, size.height]);

  const nodeById = useMemo(
    () => new Map(positions.map((p) => [p.id, p])),
    [positions],
  );
  const meta = useMemo(
    () => new Map((graph?.nodes ?? []).map((n) => [n.id, n])),
    [graph],
  );

  const radiusOf = (degree: number) => 5 + Math.min(9, Math.sqrt(degree) * 2.6);

  /** Which labels can be drawn without colliding. Recomputed only when the
   *  layout changes; zoom just relaxes the rule. */
  const visibleLabels = useMemo(
    () =>
      placeLabels(
        positions.map((p) => ({
          id: p.id,
          x: p.x,
          y: p.y + radiusOf(p.degree) + 12,
          text: labelOf(meta.get(p.id)?.title ?? ""),
          weight: p.degree,
        })),
      ),
    [positions, meta],
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
      setView((v) => {
        const k = Math.max(
          MIN_ZOOM,
          Math.min(MAX_ZOOM, v.k * Math.exp(-e.deltaY * 0.0015)),
        );
        if (k === v.k) return v;
        // Keep the graph-space point under the cursor fixed across the change.
        const scale = k / v.k;
        return { k, x: px - (px - v.x) * scale, y: py - (py - v.y) * scale };
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [positions.length]);

  const onPointerDown = (e: React.PointerEvent<SVGSVGElement>) => {
    // Left button only, and never start a pan on a node — that is a click.
    if (e.button !== 0 || (e.target as Element).closest("[data-node]")) return;
    panning.current = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y };
    svgRef.current?.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const p = panning.current;
    if (!p) return;
    setView((v) => ({ ...v, x: p.vx + (e.clientX - p.x), y: p.vy + (e.clientY - p.y) }));
  };
  const endPan = (e: React.PointerEvent<SVGSVGElement>) => {
    panning.current = null;
    svgRef.current?.releasePointerCapture?.(e.pointerId);
  };

  const showAllLabels = view.k >= ALL_LABELS_ZOOM;

  return (
    <div ref={boxRef} className="relative min-h-0 flex-1 overflow-hidden">
      {(!graph || (graph.nodes.length > 0 && positions.length === 0)) && (
        <div className="flex h-full items-center justify-center">
          <span className="text-caption text-muted-foreground">
            {!graph
              ? "Reading the notebook…"
              : `Laying out ${graph.nodes.length} documents…`}
          </span>
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
          className="touch-none text-muted-foreground"
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
                  strokeWidth={1}
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
                lit && (showAllLabels || visibleLabels.has(p.id) || hovered === p.id);
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
                    strokeWidth={1.5}
                  />
                  {labelled && (
                    <text
                      y={r + 12}
                      textAnchor="middle"
                      className="pointer-events-none fill-current text-micro"
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
      {hoverCard}
    </div>
  );
}

/** Titles are long and the graph is not the place to read one in full. */
function labelOf(title: string) {
  return title.length > 26 ? `${title.slice(0, 25)}…` : title;
}
