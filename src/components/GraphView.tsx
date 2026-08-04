import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { createLayout, layout } from "@/lib/forceLayout";
import { placeLabels } from "@/lib/graphLabels";
import { neighborhood } from "@/lib/neighborhood";
import {
  GROUP_COLOR,
  GROUP_LABEL,
  groupOfNode,
  type TypeGroup,
} from "@/lib/sourceGroups";
import { FilterBar, rankByCount } from "./FilterBar";
import type { NotebookGraph } from "@/lib/types";
import { EmptyState, useHoverCard } from "./ui";
import { sourceHoverData } from "./SourcesPanel";
import { cn, relativeTime } from "@/lib/utils";
import { Crosshair, Minus, Plus, Share2, X } from "lucide-react";

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

/** Set when a document is opened BY CLICKING IT IN THE GRAPH, remembering
 *  the graph state at that moment.
 *
 *  Arriving with a document open normally means "show me its neighbourhood",
 *  which is the local-graph behaviour worth having. But if you got to that
 *  document by clicking it here, you were already looking at the graph, and
 *  changing what you come back to is just losing your place. So a return
 *  trip through the graph restores exactly the view you left. */
let openedFromGraph: {
  notebook: string;
  id: string;
  focus: string | null;
  hops: number;
} | null = null;

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
  /** Whatever the reader last had open. Arriving at the graph from a
   *  document should land on that document's neighbourhood, the way a local
   *  graph does — the full picture is rarely the question you arrived with. */
  const readerDoc = useStore((s) => s.reader.history[s.reader.index]);
  const { show: showCard, hide: hideCard, card: hoverCard } = useHoverCard("right");
  const [graph, setGraph] = useState<NotebookGraph | null>(null);
  const [hovered, setHovered] = useState<string | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [view, setView] = useState({ x: 0, y: 0, k: 1 });
  const [progress, setProgress] = useState(0);
  /** Focused document, or null for the whole notebook. Clicking a node
   *  focuses it — exploring structure is what the graph is for; the chip
   *  that appears carries the button that opens the document itself. */
  const [focus, setFocus] = useState<string | null>(null);
  const [hops, setHops] = useState(2);
  /** Type filter, the graph's own — the grid filters a level of the gallery,
   *  the graph filters the whole notebook, so they do not share state. */
  const [group, setGroup] = useState<TypeGroup>("all");
  /** Consumed once per arrival. The pane unmounts whenever the centre column
   *  shows something else, so "mounted" IS "arrived" — and opening a
   *  document from the graph and coming back re-focuses on that document,
   *  which is the loop worth having. Cleared after firing so it never
   *  overrides a focus the reader has since cleared by hand. */
  const pendingArrival = useRef(true);
  const viewRef = useRef(view);
  viewRef.current = view;
  const commitViewRef = useRef<(v: { x: number; y: number; k: number }) => void>(
    () => {},
  );

  /** Set the view AND remember it. Every pan and zoom goes through here so
   *  there is no path that moves the camera without recording where. */
  const commitView = (next: { x: number; y: number; k: number }) => {
    setView(next);
    viewMemory.set(viewKey, next);
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

  useEffect(() => {
    if (!pendingArrival.current || !graph) return;
    pendingArrival.current = false;
    const returning =
      openedFromGraph &&
      openedFromGraph.notebook === currentId &&
      openedFromGraph.id === readerDoc?.id;
    if (returning) {
      // Came back from a document opened here — restore, do not re-decide.
      setFocus(openedFromGraph!.focus);
      setHops(openedFromGraph!.hops);
    } else if (readerDoc && graph.nodes.some((n) => n.id === readerDoc.id)) {
      setFocus(readerDoc.id);
    }
    openedFromGraph = null;
  }, [graph, readerDoc, currentId]);

  // Come back to where this view was last left, not to the origin. Keyed
  // by focus too: the whole-notebook camera and a neighbourhood's camera are
  // different places, and inheriting one for the other is disorienting.
  const viewKey = `${currentId}:${group}:${focus ?? ""}:${hops}`;
  useEffect(() => {
    setView(viewMemory.get(viewKey) ?? { x: 0, y: 0, k: 1 });
  }, [viewKey]);

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
  /** The graph actually drawn: everything, or one document's neighbourhood.
   *  Narrowed before layout on purpose — filtering afterwards would leave
   *  the survivors sitting in the positions the hairball gave them. */
  const shown = useMemo(() => {
    if (!graph) return null;
    let nodes = graph.nodes;
    let edges = graph.edges;
    if (group !== "all") {
      const keep = new Set(
        nodes
          .filter((n) => groupOfNode(n.kind, n.sourceType) === group)
          .map((n) => n.id),
      );
      nodes = nodes.filter((n) => keep.has(n.id));
      edges = edges.filter((e) => keep.has(e.from) && keep.has(e.to));
    }
    if (focus && nodes.some((n) => n.id === focus)) {
      const keep = neighborhood(focus, edges, hops);
      nodes = nodes.filter((n) => keep.has(n.id));
      edges = edges.filter((e) => keep.has(e.from) && keep.has(e.to));
    }
    return { nodes, edges };
  }, [graph, focus, hops, group]);

  /** Groups actually present, biggest first — an option that would filter to
   *  nothing never renders, same rule the gallery's bar follows. */
  const groups = useMemo(() => {
    const counts = new Map<string, number>();
    for (const n of graph?.nodes ?? []) {
      const g = groupOfNode(n.kind, n.sourceType);
      counts.set(g, (counts.get(g) ?? 0) + 1);
    }
    return [
      { value: "all", label: "All" },
      ...rankByCount(counts).map((g) => ({
        value: g,
        label: GROUP_LABEL[g as Exclude<TypeGroup, "all">] ?? g,
      })),
    ];
  }, [graph]);

  const [positions, setPositions] = useState<ReturnType<typeof layout>>([]);

  /** Shown for every layout pass, not just a slow one — watching the graph
   *  settle is honest feedback about what the pane is doing, and the bar is
   *  a real fraction rather than a spinner pretending. */
  const loading = !shown || (shown.nodes.length > 0 && positions.length === 0);
  useEffect(() => {
    if (!shown || !size.width || !size.height) {
      setPositions([]);
      return;
    }
    const key = `${cacheKey}:${group}:${focus ?? ""}:${hops}:${size.width}x${size.height}`;
    const hit = layoutCache.get(key);
    if (hit) {
      setPositions(hit);
      return;
    }
    // Driven a slice per frame rather than in one call: the fraction below
    // is real, and the window keeps answering the pointer while it settles.
    let stale = false;
    let frame = 0;
    const run = createLayout(shown.nodes, shown.edges, size.width, size.height);
    setProgress(0);
    const pump = () => {
      if (stale) return;
      // A tick budget rather than a time budget — the cost per tick is
      // stable for a given graph, and a wall-clock budget would make the
      // layout itself depend on how busy the machine happened to be, which
      // costs determinism for nothing.
      const ticksPerFrame = Math.max(1, Math.round(4000 / shown.nodes.length));
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
  }, [shown, size.width, size.height, cacheKey, focus, hops, group]);

  const nodeById = useMemo(
    () => new Map(positions.map((p) => [p.id, p])),
    [positions],
  );
  const meta = useMemo(
    () => new Map((shown?.nodes ?? []).map((n) => [n.id, n])),
    [shown],
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
    if (!hovered || !shown) return null;
    const set = new Set<string>([hovered]);
    for (const e of shown.edges) {
      if (e.from === hovered) set.add(e.to);
      if (e.to === hovered) set.add(e.from);
    }
    return set;
  }, [hovered, shown]);

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
    <div className="flex min-h-0 flex-1 flex-col">
      <FilterBar
        groups={groups}
        group={group}
        onGroup={(v) => setGroup(v as TypeGroup)}
        groupDot={(v) =>
          v === "all"
            ? undefined
            : GROUP_COLOR[v as Exclude<TypeGroup, "all">]
        }
      />
      <div ref={boxRef} className="relative min-h-0 flex-1 overflow-hidden">
      {loading && (
        <div className="flex h-full flex-col items-center justify-center gap-2">
          <span className="text-caption text-muted-foreground">
            {!graph
              ? "Reading the notebook…"
              : `Laying out ${shown?.nodes.length ?? 0} documents…`}
          </span>
          <div className="h-1 w-48 overflow-hidden rounded-full bg-surface-2">
            <div
              className="h-full rounded-full bg-primary transition-[width] duration-150"
              // Reading the notebook has no measurable fraction, so it shows
              // a fixed sliver rather than a fake crawl; the layout half is
              // the real number.
              style={{ width: `${shown ? Math.round(progress * 100) : 8}%` }}
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
      {focus && meta.get(focus) && (
        <div className="absolute left-3 top-3 flex items-center gap-1 rounded-lg border border-border bg-surface-2/90 p-1 pl-2.5 backdrop-blur">
          <span className="max-w-56 truncate text-caption text-foreground">
            {meta.get(focus)!.title}
          </span>
          <span className="text-caption text-subtle-foreground">
            {shown ? `${shown.nodes.length - 1} linked` : ""}
          </span>
          {/* How far out to walk. One hop is what cites this; two is the
              conversation around it; past three you are back in the soup. */}
          <div className="ml-1 flex items-center gap-0.5 rounded-md border border-border p-0.5">
            {[1, 2, 3].map((n) => (
              <button
                key={n}
                type="button"
                onClick={() => setHops(n)}
                aria-pressed={hops === n}
                title={`${n} hop${n > 1 ? "s" : ""} out`}
                className={cn(
                  "rounded px-1.5 text-micro font-medium tabular-nums transition-colors",
                  hops === n
                    ? "bg-surface text-foreground"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {n}
              </button>
            ))}
          </div>
          <button
            type="button"
            onClick={() => {
              openedFromGraph = {
                notebook: currentId ?? "",
                id: focus,
                focus,
                hops,
              };
              openInReader({
                type: meta.get(focus)!.kind === "note" ? "note" : "source",
                id: focus,
              });
            }}
            className="rounded-md px-2 py-1 text-micro font-medium text-muted-foreground transition-colors hover:bg-surface hover:text-foreground"
          >
            Open
          </button>
          <button
            type="button"
            onClick={() => setFocus(null)}
            title="Show the whole notebook"
            aria-label="Clear focus"
            className="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-surface hover:text-foreground"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
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
            {shown?.edges.map((e, i) => {
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
              // Clearance must counter-scale with everything else. A flat
              // graph-unit gap looked right at 100% and opened into a chasm
              // between arrowhead and node by 700%. The marker is sized in
              // stroke widths, so this tracks it.
              const gap = radiusOf(b.degree) + 1.5 * glyph;
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
                  onClick={(e) => {
                    // Plain click opens the document, the same as clicking it
                    // anywhere else in the app; the graph does not get to
                    // redefine what a click means. Cmd/Ctrl-click is the
                    // graph-specific gesture.
                    if (e.metaKey || e.ctrlKey) {
                      setFocus(p.id === focus ? null : p.id);
                      return;
                    }
                    openedFromGraph = {
                      notebook: currentId ?? "",
                      id: p.id,
                      focus,
                      hops,
                    };
                    openInReader({
                      type: isNote ? "note" : "source",
                      id: p.id,
                    });
                  }}
                >
                  {/* Notes are hollow, sources solid — the same distinction
                      the sidebar makes, carried by shape not a legend. */}
                  {/* Colour is the type; hollow-vs-filled is still note
                      -vs-source, so the two readings do not compete. */}
                  <circle
                    r={r}
                    fill={
                      isNote
                        ? "var(--background)"
                        : GROUP_COLOR[
                            groupOfNode(node.kind, node.sourceType) as Exclude<
                              TypeGroup,
                              "all"
                            >
                          ]
                    }
                    stroke={
                      isNote
                        ? GROUP_COLOR.notes
                        : "color-mix(in srgb, var(--foreground) 45%, transparent)"
                    }
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
      {!focus && positions.length > 0 && (
        <div className="pointer-events-none absolute left-3 top-3 text-caption text-subtle-foreground">
          {NAV_HINT}
        </div>
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

/** Cmd on a Mac, Ctrl elsewhere — the app is macOS-first but the webview
 *  runs both, and a hint naming the wrong key is worse than none. */
const NAV_HINT =
  typeof navigator !== "undefined" && /Mac/i.test(navigator.platform)
    ? "Click to open · ⌘-click to see just its neighbourhood"
    : "Click to open · Ctrl-click to see just its neighbourhood";

/** Titles are long and the graph is not the place to read one in full. */
function labelOf(title: string) {
  return title.length > 26 ? `${title.slice(0, 25)}…` : title;
}
