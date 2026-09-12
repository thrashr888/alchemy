/* The Timeline (docs/RFC-timeline.md): Home's fourth center column — every
   source and note in the corpus along one axis by the moment it was added,
   grouped into the batches it actually arrived in, one lane per notebook.

   Additions only: `createdAt` is written once, at import, and is what a
   person means by "when I brought this in". Updates and source events churn
   and stay off the axis. The backend does the batching (timeline.rs), so an
   agent asking "what came in last week" sees the same batches this draws.

   SVG rather than canvas, like the graph: a few hundred batches is nothing
   for the DOM, and it gets hit-testing, focus, and the theme's colors free.
   Two zoom levels, chosen by how much room a day has: at month scale a
   batch is a pill sized by log(count); zoomed in past a few weeks the pill
   stays and its documents unfold beside it as ticks when they have room. */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type { CorpusTimeline, TimelineBatch, TimelineItem } from "@/lib/types";
import {
  GROUP_COLOR,
  GROUP_LABEL,
  groupOfNode,
  type TypeGroup,
} from "@/lib/sourceGroups";
import { NOTEBOOK_PALETTE } from "@/lib/notebookIcons";
import { effectiveValue, FilterBar, rankByCount } from "./FilterBar";
import { EmptyState, LoadingState, useHoverCard } from "./ui";
import { cn } from "@/lib/utils";
import { ChartNoAxesGantt, Crosshair, Minus, Plus } from "lucide-react";

const DAY = 86_400_000;
const HOUR = 3_600_000;
/** The lane label column, HTML rather than SVG so titles truncate. */
const LABEL_W = 200;
const ROW_H = 34;
const AXIS_H = 28;
const PAD_R = 24;
/** Fit is 1; the ceiling (set per corpus) is an hour per screen. */
const MIN_ZOOM = 1;
const ZOOM_SPEED = 0.0025;
/** Past this many pixels per day a batch's documents unfold beside it. */
const DAYS_MODE_PX = 48;
/** A batch's documents draw as ticks only when each has this much room. */
const TICK_SPACING = 4;
/** …and get a title beside the tick when they have this much. */
const TITLE_SPACING = 110;
const NAV_HINT =
  "Drag to pan · pinch or ⌘-scroll to zoom · click a batch to open its day";

type Group = Exclude<TypeGroup, "all">;

/** Pan and zoom survive leaving the tab within an app run, like the graph's
 *  viewMemory: stepping out to a document and back should return you to
 *  where you were looking. Fit is always one click away. */
let viewMemory: { k: number; x: number } | null = null;

function fmtDay(t: number) {
  return new Date(t).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}
function fmtDayYear(t: number) {
  return new Date(t).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}
function fmtTime(t: number) {
  return new Date(t).toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
}
function fmtSpan(start: number, end: number) {
  const sameDay =
    new Date(start).toDateString() === new Date(end).toDateString();
  if (sameDay) {
    return start === end
      ? `${fmtDayYear(start)} · ${fmtTime(start)}`
      : `${fmtDayYear(start)} · ${fmtTime(start)}–${fmtTime(end)}`;
  }
  return `${fmtDayYear(start)} – ${fmtDayYear(end)}`;
}
/** "3 days", "6 hours", "2 months" — the visible span, for the zoom control. */
function fmtSpanLength(ms: number) {
  if (ms < DAY * 2) return plural(Math.max(1, Math.round(ms / HOUR)), "hour");
  if (ms < DAY * 60) return plural(Math.round(ms / DAY), "day");
  return plural(Math.round(ms / (DAY * 30.4)), "month");
}
function plural(n: number, one: string) {
  return `${n} ${one}${n === 1 ? "" : "s"}`;
}
function counts(b: { sources: number; notes: number }) {
  return [
    b.sources > 0 && plural(b.sources, "source"),
    b.notes > 0 && plural(b.notes, "note"),
  ]
    .filter(Boolean)
    .join(" · ");
}
function typeLabel(item: TimelineItem) {
  if (item.kind === "note") {
    return item.sourceType === "note"
      ? "Note"
      : `${item.sourceType.replace(/_/g, " ")} note`;
  }
  return GROUP_LABEL[groupOfNode(item.kind, item.sourceType) as Group];
}

/** Axis ticks for the visible range: the coarsest step that still leaves
 *  ~70px between labels, aligned to real calendar boundaries. */
function axisTicks(
  t0: number,
  t1: number,
  pxPerMs: number,
): { at: number; label: string; major: boolean }[] {
  const minGap = 70 / pxPerMs;
  const out: { at: number; label: string; major: boolean }[] = [];
  const startD = new Date(t0);
  const midnight = new Date(
    startD.getFullYear(),
    startD.getMonth(),
    startD.getDate(),
  );
  if (minGap <= HOUR * 6) {
    const step = minGap <= HOUR ? 1 : minGap <= HOUR * 3 ? 3 : 6;
    for (let t = midnight.getTime(); t <= t1; t += step * HOUR) {
      const h = new Date(t).getHours();
      out.push({
        at: t,
        label: h === 0 ? fmtDay(t) : fmtTime(t),
        major: h === 0,
      });
    }
  } else if (minGap <= DAY) {
    for (let t = midnight.getTime(); t <= t1; t += DAY) {
      const day = new Date(t);
      out.push({ at: t, label: fmtDay(t), major: day.getDate() === 1 });
    }
  } else if (minGap <= DAY * 7) {
    // Weeks, on Mondays.
    midnight.setDate(midnight.getDate() - ((midnight.getDay() + 6) % 7));
    for (let t = midnight.getTime(); t <= t1; t += DAY * 7) {
      out.push({ at: t, label: fmtDay(t), major: false });
    }
  } else {
    // Months, or every third when even those crowd.
    const step = minGap <= DAY * 31 ? 1 : 3;
    const d = new Date(startD.getFullYear(), startD.getMonth(), 1);
    while (d.getTime() <= t1) {
      const first = out.length === 0;
      out.push({
        at: d.getTime(),
        label:
          d.getMonth() === 0 || first
            ? d.toLocaleDateString(undefined, {
                month: "short",
                year: "numeric",
              })
            : d.toLocaleDateString(undefined, { month: "short" }),
        major: d.getMonth() === 0,
      });
      d.setMonth(d.getMonth() + step);
    }
  }
  return out.filter((t) => t.at >= t0 - minGap && t.at <= t1);
}

export function TimelineSection() {
  const notebooks = useStore((s) => s.notebooks);
  const [data, setData] = useState<CorpusTimeline | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [group, setGroup] = useState<TypeGroup>("all");
  const [chip, setChip] = useState<string | null>(null);
  const [view, setView] = useState(viewMemory ?? { k: 1, x: 0 });
  const viewRef = useRef(view);
  viewRef.current = view;
  const commitView = useCallback((next: { k: number; x: number }) => {
    viewMemory = next;
    setView(next);
  }, []);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const panning = useRef<{ x: number; vx: number; moved: boolean } | null>(
    null,
  );
  // pointerup lands before click, so the click handlers read this instead
  // of `panning` to tell a drag's release from a tap.
  const dragged = useRef(false);
  const {
    show: showCard,
    hide: hideCard,
    card: hoverCard,
  } = useHoverCard("right");

  // Reload when the notebook list refreshes — the store re-lists after
  // imports, deletes, and agent writes, so that's the cheap change signal.
  useEffect(() => {
    let live = true;
    api.corpusTimeline().then(
      (t) => live && setData(t),
      (e) => live && setFailed(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [notebooks]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, [data]);

  // Filters: group by type (the graph's legend), chip by notebook. Batches
  // are re-counted from their surviving items so the pills stay honest.
  const groupCounts = useMemo(() => {
    const m = new Map<string, number>();
    for (const b of data?.batches ?? [])
      for (const it of b.items) {
        const g = groupOfNode(it.kind, it.sourceType);
        m.set(g, (m.get(g) ?? 0) + 1);
      }
    return m;
  }, [data]);
  const notebookTitles = useMemo(() => {
    const m = new Map<string, number>();
    for (const b of data?.batches ?? [])
      m.set(
        b.notebookTitle,
        (m.get(b.notebookTitle) ?? 0) + b.sources + b.notes,
      );
    return rankByCount(m);
  }, [data]);
  const groupValue = effectiveValue(
    group,
    ["all", ...groupCounts.keys()],
    "all",
  );
  const chipValue = effectiveValue(chip, notebookTitles, null);

  const batches = useMemo(() => {
    const out: TimelineBatch[] = [];
    for (const b of data?.batches ?? []) {
      if (chipValue && b.notebookTitle !== chipValue) continue;
      if (groupValue === "all") {
        out.push(b);
        continue;
      }
      const items = b.items.filter(
        (it) => groupOfNode(it.kind, it.sourceType) === groupValue,
      );
      if (items.length === 0) continue;
      out.push({
        ...b,
        items,
        start: items[0].createdAt,
        end: items[items.length - 1].createdAt,
        sources: items.filter((it) => it.kind === "source").length,
        notes: items.filter((it) => it.kind === "note").length,
        samples: items.slice(0, 8).map((it) => it.title),
      });
    }
    return out;
  }, [data, groupValue, chipValue]);

  // Lanes: notebooks with a batch in view, the most recently active first,
  // so the live ones sit at the top and the dormant ones sink.
  const lanes = useMemo(() => {
    const latest = new Map<
      string,
      { title: string; color: string; end: number }
    >();
    for (const b of batches) {
      const cur = latest.get(b.notebookId);
      if (!cur || b.end > cur.end)
        latest.set(b.notebookId, {
          title: b.notebookTitle,
          color: b.notebookColor || NOTEBOOK_PALETTE[0],
          end: b.end,
        });
    }
    return [...latest.entries()]
      .sort((a, b) => b[1].end - a[1].end)
      .map(([id, v]) => ({ id, ...v }));
  }, [batches]);
  const laneIndex = useMemo(
    () => new Map(lanes.map((l, i) => [l.id, i])),
    [lanes],
  );

  // Time → x. The domain is the whole corpus padded half a day each side;
  // the view scales and shifts it. Everything below reads through `xOf`.
  const innerW = Math.max(1, width - LABEL_W - PAD_R);
  const t0 = (data?.first ?? 0) - DAY / 2;
  const t1 = (data?.last ?? 0) + DAY / 2;
  const basePx = innerW / Math.max(1, t1 - t0);
  const pxPerMs = basePx * view.k;
  const xOf = (t: number) => LABEL_W + view.x + (t - t0) * pxPerMs;
  const tOf = (x: number) => t0 + (x - LABEL_W - view.x) / pxPerMs;
  const maxZoom = Math.max(MIN_ZOOM, (t1 - t0) / HOUR);
  const pxPerDay = pxPerMs * DAY;
  const daysMode = pxPerDay >= DAYS_MODE_PX;
  const visT0 = tOf(LABEL_W);
  const visT1 = tOf(width - PAD_R);
  const ticks = data ? axisTicks(visT0, visT1, pxPerMs) : [];
  const now = Date.now();
  const nowVisible = now >= visT0 && now <= visT1;

  /** Zoom about a pane x, keeping the moment under it fixed. */
  const zoomAt = useCallback(
    (px: number, k: number) => {
      const v = viewRef.current;
      const next = Math.max(MIN_ZOOM, Math.min(maxZoom, k));
      if (next === v.k) return;
      const scale = next / v.k;
      const rel = px - LABEL_W;
      commitView({ k: next, x: rel - (rel - v.x) * scale });
    },
    [commitView, maxZoom],
  );

  // Wheel: vertical scroll stays the lane list's; sideways scroll pans time;
  // pinch (ctrlKey) and ⌘-scroll zoom about the pointer. Attached natively
  // with { passive: false } — React's root wheel listener is passive, so
  // preventDefault there is ignored (GraphView and MindMap hit this first).
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault();
        const rect = el.getBoundingClientRect();
        zoomAt(
          e.clientX - rect.left,
          viewRef.current.k * Math.exp(-e.deltaY * ZOOM_SPEED),
        );
      } else if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        e.preventDefault();
        const v = viewRef.current;
        commitView({ ...v, x: v.x - e.deltaX });
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [zoomAt, commitView, data]);

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || (e.target as Element).closest("[data-node]")) return;
    panning.current = { x: e.clientX, vx: viewRef.current.x, moved: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const p = panning.current;
    if (!p) return;
    const dx = e.clientX - p.x;
    if (Math.abs(dx) > 2) p.moved = true;
    commitView({ ...viewRef.current, x: p.vx + dx });
  };
  const endPan = (e: React.PointerEvent<HTMLDivElement>) => {
    dragged.current = !!panning.current?.moved;
    panning.current = null;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
  };

  const zoomBy = (factor: number) =>
    zoomAt(LABEL_W + innerW / 2, viewRef.current.k * factor);
  const fit = () => commitView({ k: 1, x: 0 });

  /** A batch click opens its day: the day (or days) it spans fill the
   *  pane, centred on the batch, so its documents have room to unfold. */
  const zoomToBatch = (b: TimelineBatch) => {
    const span = Math.max(b.end - b.start, DAY);
    const k = Math.max(
      MIN_ZOOM,
      Math.min(maxZoom, (innerW * 0.8) / (span * basePx)),
    );
    const mid = (b.start + b.end) / 2;
    commitView({ k, x: innerW / 2 - (mid - t0) * basePx * k });
  };

  const openItem = (b: TimelineBatch, it: TimelineItem) => {
    // Documents live in their notebook, so opening one means going there.
    void useStore
      .getState()
      .selectNotebook(b.notebookId)
      .then(() =>
        useStore.getState().openInReader({ type: it.kind, id: it.id }),
      );
  };

  const batchCard = (e: React.MouseEvent<Element>, b: TimelineBatch) =>
    showCard(e, {
      title: b.notebookTitle,
      time: fmtSpan(b.start, b.end),
      meta: [
        { label: "Added", value: counts(b) },
        ...b.samples.map((t) => ({ label: t })),
        ...(b.items.length > b.samples.length
          ? [{ label: `… and ${b.items.length - b.samples.length} more` }]
          : []),
      ],
    });
  const itemCard = (
    e: React.MouseEvent<Element>,
    b: TimelineBatch,
    it: TimelineItem,
  ) =>
    showCard(e, {
      title: it.title || "Untitled",
      time: `${fmtDayYear(it.createdAt)} · ${fmtTime(it.createdAt)}`,
      meta: [
        { label: typeLabel(it) + (it.origin === "auto" ? " · auto" : "") },
        { label: b.notebookTitle },
      ],
    });

  if (failed)
    return (
      <EmptyState
        icon={<ChartNoAxesGantt className="h-5 w-5" />}
        title="The timeline couldn't load"
        hint={failed}
      />
    );
  if (!data) return <LoadingState label="Reading the corpus…" />;
  if (data.batches.length === 0)
    return (
      <EmptyState
        icon={<ChartNoAxesGantt className="h-5 w-5" />}
        title="Nothing on the timeline yet"
        hint="Sources and notes appear here by the day they were added."
      />
    );

  const groups = [
    { value: "all", label: "All" },
    ...rankByCount(groupCounts).map((g) => ({
      value: g,
      label: GROUP_LABEL[g as Group] ?? g,
    })),
  ];
  const lanesH = Math.max(1, lanes.length) * ROW_H;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center justify-between gap-4 px-6">
        <FilterBar
          bare
          groups={groups}
          group={groupValue}
          onGroup={(v) => setGroup(v as TypeGroup)}
          groupDot={(v) => (v === "all" ? undefined : GROUP_COLOR[v as Group])}
          chips={notebookTitles}
          chip={chipValue}
          onChip={setChip}
          chipAllLabel="All notebooks"
          chipPrefix=""
        />
        <div className="shrink-0 text-caption tabular-nums text-muted-foreground">
          {counts({
            sources: batches.reduce((n, b) => n + b.sources, 0),
            notes: batches.reduce((n, b) => n + b.notes, 0),
          })}
          {" · since "}
          {fmtDayYear(data.first)}
        </div>
      </div>

      <div className="relative mt-3 min-h-0 flex-1">
        <div
          ref={scrollRef}
          className={cn(
            "relative h-full select-none overflow-y-auto overflow-x-hidden",
            panning.current ? "cursor-grabbing" : "cursor-grab",
          )}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endPan}
          onPointerCancel={endPan}
          onMouseLeave={hideCard}
        >
          {width > 0 && (
            <>
              {/* The axis stays put while the lanes scroll under it. */}
              <svg
                width={width}
                height={AXIS_H}
                className="sticky top-0 z-10 block bg-background"
                aria-hidden
              >
                <line
                  x1={LABEL_W}
                  x2={width}
                  y1={AXIS_H - 0.5}
                  y2={AXIS_H - 0.5}
                  stroke="var(--border)"
                />
                {ticks.map((t) => (
                  <g key={t.at} transform={`translate(${xOf(t.at)} 0)`}>
                    <line
                      y1={AXIS_H - 6}
                      y2={AXIS_H}
                      stroke={
                        t.major ? "var(--border-strong)" : "var(--border)"
                      }
                    />
                    <text
                      x={4}
                      y={AXIS_H - 9}
                      className={cn(
                        "text-micro",
                        t.major
                          ? "fill-foreground font-medium"
                          : "fill-subtle-foreground",
                      )}
                    >
                      {t.label}
                    </text>
                  </g>
                ))}
                {nowVisible && (
                  <text
                    x={xOf(now) - 4}
                    y={AXIS_H - 9}
                    textAnchor="end"
                    className="text-micro fill-muted-foreground"
                  >
                    today
                  </text>
                )}
              </svg>

              <div className="relative" style={{ height: lanesH }}>
                <svg
                  width={width}
                  height={lanesH}
                  className="absolute inset-0 block"
                  role="img"
                  aria-label="Timeline of sources and notes by notebook"
                >
                  {/* Calendar gridlines and hairline lane separators. */}
                  {ticks.map((t) => (
                    <line
                      key={t.at}
                      x1={xOf(t.at)}
                      x2={xOf(t.at)}
                      y1={0}
                      y2={lanesH}
                      stroke={
                        t.major ? "var(--border-strong)" : "var(--border)"
                      }
                      strokeOpacity={t.major ? 0.8 : 0.5}
                    />
                  ))}
                  {nowVisible && (
                    <line
                      x1={xOf(now)}
                      x2={xOf(now)}
                      y1={0}
                      y2={lanesH}
                      stroke="var(--muted-foreground)"
                      strokeDasharray="2 3"
                    />
                  )}
                  {lanes.map((l, i) => (
                    <line
                      key={l.id}
                      x1={0}
                      x2={width}
                      y1={(i + 1) * ROW_H - 0.5}
                      y2={(i + 1) * ROW_H - 0.5}
                      stroke="var(--border)"
                      strokeOpacity={0.6}
                    />
                  ))}
                  {/* Batches, clipped to the axis so a pan never paints over
                    the label column. */}
                  <clipPath id="timeline-lanes">
                    <rect
                      x={LABEL_W}
                      y={0}
                      width={innerW + PAD_R}
                      height={lanesH}
                    />
                  </clipPath>
                  <g clipPath="url(#timeline-lanes)">
                    {batches.map((b) => {
                      const lane = laneIndex.get(b.notebookId);
                      if (lane === undefined) return null;
                      const count = b.sources + b.notes;
                      const h = Math.min(
                        22,
                        8 + Math.log2(Math.max(1, count)) * 2.6,
                      );
                      // A batch that spans minutes is a circle at month
                      // scale (size reads as count); one that spans hours
                      // stretches into a pill as the zoom gives it room.
                      const x0 = xOf(b.start);
                      const x1 = Math.max(xOf(b.end), x0 + h);
                      if (x1 < LABEL_W || x0 > width) return null;
                      const y = lane * ROW_H + (ROW_H - h) / 2;
                      const color = b.notebookColor || NOTEBOOK_PALETTE[0];
                      const outlined = b.sources === 0;
                      // Documents unfold beside the pill when each has room.
                      const spacing = (x1 - x0) / Math.max(1, b.items.length);
                      const unfold = daysMode && spacing >= TICK_SPACING;
                      return (
                        <g
                          key={`${b.notebookId}:${b.start}`}
                          data-node
                          onMouseEnter={(e) => batchCard(e, b)}
                          onMouseLeave={hideCard}
                        >
                          <rect
                            x={x0}
                            y={y}
                            width={x1 - x0}
                            height={h}
                            rx={h / 2}
                            fill={outlined ? "transparent" : color}
                            fillOpacity={outlined ? 1 : unfold ? 0.25 : 0.85}
                            stroke={color}
                            strokeWidth={outlined ? 1.5 : 0}
                            className="cursor-pointer"
                            onClick={() => {
                              if (!dragged.current && !unfold) zoomToBatch(b);
                            }}
                          >
                            <title>{`${b.notebookTitle} · ${counts(b)}`}</title>
                          </rect>
                          {daysMode && !unfold && (
                            <text
                              x={x1 + 6}
                              y={lane * ROW_H + ROW_H / 2 + 3.5}
                              className="pointer-events-none text-micro tabular-nums fill-muted-foreground"
                            >
                              {count}
                            </text>
                          )}
                          {unfold &&
                            b.items.map((it, i) => {
                              const x = xOf(it.createdAt);
                              const tc =
                                GROUP_COLOR[
                                  groupOfNode(it.kind, it.sourceType) as Group
                                ] ?? color;
                              const next = b.items[i + 1];
                              const room = next
                                ? xOf(next.createdAt) - x
                                : Number.POSITIVE_INFINITY;
                              return (
                                <g
                                  key={it.id}
                                  data-node
                                  className="cursor-pointer"
                                  onMouseEnter={(e) => {
                                    e.stopPropagation();
                                    itemCard(e, b, it);
                                  }}
                                  onClick={() => {
                                    if (!dragged.current) openItem(b, it);
                                  }}
                                >
                                  <rect
                                    x={x - 1.5}
                                    y={lane * ROW_H + 8}
                                    width={3}
                                    height={ROW_H - 16}
                                    rx={1.5}
                                    fill={
                                      it.kind === "note"
                                        ? "var(--background)"
                                        : tc
                                    }
                                    stroke={tc}
                                    strokeWidth={it.kind === "note" ? 1.2 : 0}
                                    opacity={it.origin === "auto" ? 0.55 : 1}
                                  />
                                  {room >= TITLE_SPACING && (
                                    <text
                                      x={x + 6}
                                      y={lane * ROW_H + ROW_H / 2 + 3.5}
                                      className="pointer-events-none text-micro fill-muted-foreground"
                                    >
                                      {it.title.length > 22
                                        ? `${it.title.slice(0, 21)}…`
                                        : it.title}
                                    </text>
                                  )}
                                </g>
                              );
                            })}
                        </g>
                      );
                    })}
                  </g>
                </svg>
                {/* Lane labels over the SVG, in HTML so titles truncate. */}
                {lanes.map((l, i) => (
                  <div
                    key={l.id}
                    className="absolute left-0 flex items-center gap-2 bg-background pr-3"
                    style={{ top: i * ROW_H, height: ROW_H, width: LABEL_W }}
                  >
                    <span
                      aria-hidden
                      className="h-2 w-2 shrink-0 rounded-full"
                      style={{ backgroundColor: l.color }}
                    />
                    <button
                      type="button"
                      data-node
                      onClick={() =>
                        setChip(chipValue === l.title ? null : l.title)
                      }
                      title={
                        chipValue === l.title
                          ? "Show every notebook"
                          : `Only ${l.title}`
                      }
                      className={cn(
                        "min-w-0 truncate text-left text-caption transition-colors",
                        chipValue === l.title
                          ? "font-medium text-foreground"
                          : "text-muted-foreground hover:text-foreground",
                      )}
                    >
                      {l.title}
                    </button>
                  </div>
                ))}
              </div>
            </>
          )}

          {hoverCard}
        </div>
        <div className="absolute bottom-3 right-3 z-10 flex items-center gap-0.5 rounded-lg border border-border bg-surface-2/90 p-0.5 backdrop-blur">
          <ZoomButton
            label="Zoom out"
            onClick={() => zoomBy(1 / 1.5)}
            icon={<Minus className="h-3.5 w-3.5" />}
          />
          <button
            type="button"
            data-node
            onClick={fit}
            title="Fit the whole timeline"
            className="rounded-md px-2 py-1 text-micro font-medium tabular-nums text-muted-foreground transition-colors hover:text-foreground"
          >
            {fmtSpanLength(visT1 - visT0)}
          </button>
          <ZoomButton
            label="Zoom in"
            onClick={() => zoomBy(1.5)}
            icon={<Plus className="h-3.5 w-3.5" />}
          />
          <span aria-hidden className="mx-0.5 h-3.5 w-px bg-border-strong" />
          <ZoomButton
            label="Fit"
            onClick={fit}
            icon={<Crosshair className="h-3.5 w-3.5" />}
          />
        </div>
      </div>
      <div className="shrink-0 px-6 pb-2 pt-1 text-caption text-subtle-foreground">
        {NAV_HINT}
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
      data-node
      onClick={onClick}
      title={label}
      aria-label={label}
      className="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-surface hover:text-foreground"
    >
      {icon}
    </button>
  );
}
