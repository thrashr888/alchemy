import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { layout } from "@/lib/forceLayout";
import type { NotebookGraph } from "@/lib/types";
import { EmptyState } from "./ui";
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
export function GraphView() {
  const currentId = useStore((s) => s.currentId);
  const openSourceViewer = useStore((s) => s.openSourceViewer);
  const [graph, setGraph] = useState<NotebookGraph | null>(null);
  const [hovered, setHovered] = useState<string | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const boxRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!currentId) return;
    let stale = false;
    setGraph(null);
    void api
      .notebookGraph(currentId)
      .then((g) => !stale && setGraph(g))
      .catch(() => !stale && setGraph({ nodes: [], edges: [] }));
    return () => {
      stale = true;
    };
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
  }, [graph]);

  // The simulation is synchronous and deterministic, so it only needs to run
  // when the graph or the box actually changes.
  const positions = useMemo(() => {
    if (!graph || !size.width || !size.height) return [];
    return layout(graph.nodes, graph.edges, size.width, size.height);
  }, [graph, size.width, size.height]);

  const nodeById = useMemo(
    () => new Map(positions.map((p) => [p.id, p])),
    [positions],
  );
  const meta = useMemo(
    () => new Map((graph?.nodes ?? []).map((n) => [n.id, n])),
    [graph],
  );

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

  return (
    <div ref={boxRef} className="relative min-h-0 flex-1 overflow-hidden">
      {graph && graph.nodes.length === 0 && (
        <EmptyState
          icon={<Share2 className="h-5 w-5" />}
          title="Nothing to graph yet"
          hint="Links between sources and notes show up here — a URL, a filename, or a [[wikilink]]."
        />
      )}
      {positions.length > 0 && (
        <svg
          width={size.width}
          height={size.height}
          className="text-muted-foreground"
          role="img"
          aria-label="Notebook link graph"
        >
          {graph?.edges.map((e, i) => {
            const a = nodeById.get(e.from);
            const b = nodeById.get(e.to);
            if (!a || !b) return null;
            const lit = !neighbors || (neighbors.has(e.from) && neighbors.has(e.to));
            return (
              <line
                key={i}
                x1={a.x}
                y1={a.y}
                x2={b.x}
                y2={b.y}
                stroke="currentColor"
                strokeWidth={1}
                opacity={lit ? 0.45 : 0.08}
              />
            );
          })}
          {positions.map((p) => {
            const node = meta.get(p.id);
            if (!node) return null;
            // Hubs read larger, but sub-linearly — one note linked forty
            // times shouldn't dwarf the rest of the notebook.
            const r = 5 + Math.min(9, Math.sqrt(p.degree) * 2.6);
            const lit = !neighbors || neighbors.has(p.id);
            const isNote = node.kind === "note";
            return (
              <g
                key={p.id}
                transform={`translate(${p.x} ${p.y})`}
                opacity={lit ? 1 : 0.22}
                className="cursor-pointer"
                onMouseEnter={() => setHovered(p.id)}
                onMouseLeave={() => setHovered(null)}
                onClick={() => {
                  if (!isNote) openSourceViewer(p.id, node.title);
                }}
              >
                <title>{node.title}</title>
                {/* Notes are hollow, sources solid — the same distinction the
                    sidebar makes, carried by shape rather than a legend. */}
                <circle
                  r={r}
                  className={isNote ? "fill-background" : "fill-primary"}
                  stroke="currentColor"
                  strokeWidth={1.5}
                />
                <text
                  y={r + 12}
                  textAnchor="middle"
                  className="pointer-events-none fill-current text-micro"
                  opacity={hovered === p.id ? 1 : 0.75}
                >
                  {node.title.length > 26
                    ? `${node.title.slice(0, 25)}…`
                    : node.title}
                </text>
              </g>
            );
          })}
        </svg>
      )}
    </div>
  );
}
