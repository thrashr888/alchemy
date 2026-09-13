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
   batch is a circle sized by log(count), and batches that would overlap
   merge into one ringed marker; zoomed in past a few weeks a batch's
   documents unfold beside it as ticks when they have room. Clicking a batch
   opens what it holds in a panel, with the axis left where it was. */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type {
  CorpusTimeline,
  NoteKind,
  Source,
  TimelineBatch,
  TimelineItem,
} from "@/lib/types";
import { GROUP_LABEL, groupOfNode, type TypeGroup } from "@/lib/sourceGroups";
import { NOTEBOOK_PALETTE } from "@/lib/notebookIcons";
import { sourceIcon } from "@/lib/sourceIcon";
import { kindIcon } from "./studioArtifacts";
import { effectiveValue, FilterBar, rankByCount } from "./FilterBar";
import { Button, EmptyState, LoadingState, useHoverCard } from "./ui";
import { cn } from "@/lib/utils";
import {
  ArrowUpRight,
  ChartNoAxesGantt,
  Crosshair,
  Minus,
  Plus,
  X,
  ZoomIn,
} from "lucide-react";

const DAY = 86_400_000;
const HOUR = 3_600_000;
/** The lane label column, HTML rather than SVG so titles truncate. */
const LABEL_W = 220;
const ROW_H = 34;
const AXIS_H = 28;
const PAD_R = 24;
const PANEL_W = 320;
/** Fit is 1; the ceiling (set per corpus) is an hour per screen. */
const MIN_ZOOM = 1;
const ZOOM_SPEED = 0.0025;
/** Past this many pixels per day a batch's documents unfold beside it. */
const DAYS_MODE_PX = 48;
/** A batch's documents draw as ticks only when each has this much room. */
const TICK_SPACING = 4;
/** …and get a title beside the tick when nothing else sits within this. */
const TITLE_SPACING = 120;
/** Width of a tick's hit target; the tick itself is three pixels. */
const TICK_HIT = 14;
const STALE_MS = 30 * DAY;
/** Folder containers carry no chunks, so "uncited" would flag every one;
 *  the facet is about content sources (same rule as the Sources panel). */
const FOLDER_TYPES = ["folder", "git", "notion", "obsidian", "okf", "feed"];
const NAV_HINT =
  "Drag to pan · pinch or ⌘-scroll to zoom · click a batch to see what came in · ← → + − 0 T on the keyboard";

type Group = Exclude<TypeGroup, "all">;
type Fresh = "stale" | "uncited";

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
function itemIcon(item: TimelineItem) {
  return item.kind === "note"
    ? kindIcon(item.sourceType as NoteKind)
    : sourceIcon(item.sourceType as Source["sourceType"]);
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

/** Batches in one lane that would overlap at the current zoom, drawn as
 *  one marker. `batches.length > 1` is the "several imports" ring. */
interface Cluster {
  batches: TimelineBatch[];
  start: number;
  end: number;
  x0: number;
  x1: number;
  sources: number;
  notes: number;
}

export function TimelineSection() {
  const notebooks = useStore((s) => s.notebooks);
  const [data, setData] = useState<CorpusTimeline | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [group, setGroup] = useState<TypeGroup>("all");
  const [tag, setTag] = useState<string | null>(null);
  const [fresh, setFresh] = useState<Fresh | null>(null);
  // The uncited facet's data loads on first use, as in the Sources panel.
  const [citedIds, setCitedIds] = useState<Set<string> | null>(null);
  const [view, setView] = useState(viewMemory ?? { k: 1, x: 0 });
  const viewRef = useRef(view);
  viewRef.current = view;
  const commitView = useCallback((next: { k: number; x: number }) => {
    viewMemory = next;
    setView(next);
  }, []);
  const scrollRef = useRef<HTMLDivElement>(null);
  // Measured on the wrapper: the scroll pane's own width is what the panel
  // takes away, so measuring it would pin the first number it saw.
  const wrapRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const panning = useRef<{
    x: number;
    y: number;
    vx: number;
    st: number;
    moved: boolean;
  } | null>(null);
  // pointerup lands before click, so the click handlers read this instead
  // of `panning` to tell a drag's release from a tap.
  const dragged = useRef(false);
  const [selected, setSelected] = useState<TimelineBatch | null>(null);
  const [hoverLane, setHoverLane] = useState<string | null>(null);
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
    if (fresh !== "uncited" || citedIds) return;
    api
      .citedSourceIds()
      .then((ids) => setCitedIds(new Set(ids)))
      .catch(() => setCitedIds(new Set()));
  }, [fresh, citedIds]);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, [data]);

  // Filters: type group (the graph's), tag, and the Sources panel's stale /
  // uncited facets. Batches are re-counted from their surviving items so
  // the markers stay honest.
  const groupCounts = useMemo(() => {
    const m = new Map<string, number>();
    for (const b of data?.batches ?? [])
      for (const it of b.items) {
        const g = groupOfNode(it.kind, it.sourceType);
        m.set(g, (m.get(g) ?? 0) + 1);
      }
    return m;
  }, [data]);
  const tagChips = useMemo(() => {
    const m = new Map<string, number>();
    for (const b of data?.batches ?? [])
      for (const it of b.items)
        for (const t of it.tags.split(" ").filter(Boolean))
          m.set(t, (m.get(t) ?? 0) + 1);
    // The corpus carries thousands of distinct tags; the chips offer the
    // ones that would actually narrow it.
    return rankByCount(m).slice(0, 20);
  }, [data]);
  const groupValue = effectiveValue(
    group,
    ["all", ...groupCounts.keys()],
    "all",
  );
  const tagValue = effectiveValue(tag, tagChips, null);
  const now = Date.now();

  const batches = useMemo(() => {
    const keep = (it: TimelineItem) => {
      if (
        groupValue !== "all" &&
        groupOfNode(it.kind, it.sourceType) !== groupValue
      )
        return false;
      if (tagValue && !it.tags.split(" ").includes(tagValue)) return false;
      if (fresh === "stale" && now - (it.fetchedAt || it.createdAt) <= STALE_MS)
        return false;
      if (fresh === "uncited") {
        if (it.kind !== "source" || FOLDER_TYPES.includes(it.sourceType))
          return false;
        if (!citedIds || citedIds.has(it.id)) return false;
      }
      return true;
    };
    const active = groupValue !== "all" || tagValue || fresh;
    const out: TimelineBatch[] = [];
    for (const b of data?.batches ?? []) {
      if (!active) {
        out.push(b);
        continue;
      }
      const items = b.items.filter(keep);
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
    // `now` only matters at day resolution; re-reading it per render is fine.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data, groupValue, tagValue, fresh, citedIds]);

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
  const paneW = width - (selected ? PANEL_W : 0);
  const innerW = Math.max(1, paneW - LABEL_W - PAD_R);
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
  const visT1 = tOf(paneW - PAD_R);
  const ticks = data ? axisTicks(visT0, visT1, pxPerMs) : [];
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
    panning.current = {
      x: e.clientX,
      y: e.clientY,
      vx: viewRef.current.x,
      st: e.currentTarget.scrollTop,
      moved: false,
    };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const p = panning.current;
    if (!p) return;
    const dx = e.clientX - p.x;
    const dy = e.clientY - p.y;
    if (Math.abs(dx) > 2 || Math.abs(dy) > 2) p.moved = true;
    // Time pans through the view; lanes pan through the scroll position.
    e.currentTarget.scrollTop = p.st - dy;
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
  const panBy = (px: number) =>
    commitView({ ...viewRef.current, x: viewRef.current.x + px });
  /** Bring the present to the right edge at the current zoom. */
  const today = () =>
    commitView({
      ...viewRef.current,
      x: innerW - 40 - (now - t0) * basePx * viewRef.current.k,
    });
  /** Fill the pane with a span, centred on it. */
  const zoomToSpan = (start: number, end: number, minSpan: number) => {
    const span = Math.max(end - start, minSpan);
    const k = Math.max(
      MIN_ZOOM,
      Math.min(maxZoom, (innerW * 0.8) / (span * basePx)),
    );
    const mid = (start + end) / 2;
    commitView({ k, x: innerW / 2 - (mid - t0) * basePx * k });
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if ((e.target as Element).closest("input,textarea,button")) return;
    const step = e.shiftKey ? innerW : innerW / 4;
    switch (e.key) {
      case "ArrowLeft":
        panBy(step);
        break;
      case "ArrowRight":
        panBy(-step);
        break;
      case "+":
      case "=":
        zoomBy(1.5);
        break;
      case "-":
      case "_":
        zoomBy(1 / 1.5);
        break;
      case "0":
        fit();
        break;
      case "t":
      case "T":
        today();
        break;
      case "Escape":
        setSelected(null);
        break;
      default:
        return;
    }
    e.preventDefault();
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

  const clusterCard = (e: React.MouseEvent<Element>, c: Cluster) => {
    const one = c.batches.length === 1 ? c.batches[0] : null;
    const samples = one
      ? one.samples
      : c.batches.flatMap((b) => b.samples).slice(0, 8);
    const total = c.batches.reduce((n, b) => n + b.items.length, 0);
    showCard(e, {
      title: c.batches[0].notebookTitle,
      time: fmtSpan(c.start, c.end),
      meta: [
        {
          label: one ? "Added" : `${plural(c.batches.length, "import")}`,
          value: counts(c),
        },
        ...samples.map((t) => ({ label: t })),
        ...(total > samples.length
          ? [{ label: `… and ${total - samples.length} more` }]
          : []),
      ],
    });
  };
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

  // Per lane: batches that would overlap at this zoom merge into one
  // marker (a ring says "several imports"); the rest stand alone. Then the
  // marks — clusters plus unfolded documents — sorted so a document's title
  // shows only when nothing else sits within TITLE_SPACING to its right.
  const laneClusters = new Map<string, Cluster[]>();
  for (const b of batches) {
    const count = b.sources + b.notes;
    const h = Math.min(22, 8 + Math.log2(Math.max(1, count)) * 2.6);
    const x0 = xOf(b.start);
    const x1 = Math.max(xOf(b.end), x0 + h);
    const list = laneClusters.get(b.notebookId) ?? [];
    const last = list[list.length - 1];
    if (last && x0 <= last.x1 + 2) {
      last.batches.push(b);
      last.end = Math.max(last.end, b.end);
      last.x1 = Math.max(last.x1, x1);
      last.sources += b.sources;
      last.notes += b.notes;
    } else {
      list.push({
        batches: [b],
        start: b.start,
        end: b.end,
        x0,
        x1,
        sources: b.sources,
        notes: b.notes,
      });
    }
    laneClusters.set(b.notebookId, list);
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-x-4 gap-y-2 px-6">
        <div className="flex min-w-0 flex-wrap items-center gap-x-1 gap-y-1">
          <FilterBar
            bare
            groups={groups}
            group={groupValue}
            onGroup={(v) => setGroup(v as TypeGroup)}
            chips={tagChips}
            chip={tagValue}
            onChip={setTag}
          />
          {(
            [
              ["stale", "Stale", "Untouched for over 30 days"],
              ["uncited", "Uncited", "Never came back as a citation"],
            ] as const
          ).map(([id, label, title]) => (
            <button
              key={id}
              type="button"
              onClick={() => setFresh(fresh === id ? null : id)}
              title={title}
              aria-pressed={fresh === id}
              className={cn(
                "rounded-full border px-2 py-0.5 text-micro transition-colors",
                fresh === id
                  ? "border-primary/50 bg-primary/15 text-citation"
                  : "border-border text-muted-foreground hover:bg-surface-2",
              )}
            >
              {label}
            </button>
          ))}
        </div>
        <Legend />
      </div>

      <div ref={wrapRef} className="relative mt-3 min-h-0 flex-1">
        <div
          ref={scrollRef}
          tabIndex={0}
          onKeyDown={onKeyDown}
          className={cn(
            "absolute inset-y-0 left-0 select-none overflow-y-auto overflow-x-hidden outline-none",
            "focus-visible:ring-1 focus-visible:ring-ring/40",
            panning.current ? "cursor-grabbing" : "cursor-grab",
          )}
          style={{ right: selected ? PANEL_W : 0 }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endPan}
          onPointerCancel={endPan}
          onMouseLeave={() => {
            hideCard();
            setHoverLane(null);
          }}
        >
          {paneW > 0 && (
            <>
              {/* The axis stays put while the lanes scroll under it. */}
              <svg
                width={paneW}
                height={AXIS_H}
                className="sticky top-0 z-10 block bg-background"
                aria-hidden
              >
                <line
                  x1={LABEL_W}
                  x2={paneW}
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
                  width={paneW}
                  height={lanesH}
                  className="absolute inset-0 block"
                  role="img"
                  aria-label="Timeline of sources and notes by notebook"
                >
                  {/* A hovered lane lights up end to end, so a distant
                      marker still reads as its notebook's. */}
                  {hoverLane !== null && laneIndex.has(hoverLane) && (
                    <rect
                      x={0}
                      y={laneIndex.get(hoverLane)! * ROW_H}
                      width={paneW}
                      height={ROW_H}
                      fill="var(--surface-2)"
                      fillOpacity={0.6}
                    />
                  )}
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
                    <g key={l.id}>
                      <line
                        x1={0}
                        x2={paneW}
                        y1={(i + 1) * ROW_H - 0.5}
                        y2={(i + 1) * ROW_H - 0.5}
                        stroke="var(--border)"
                        strokeOpacity={0.6}
                      />
                      <rect
                        x={LABEL_W}
                        y={i * ROW_H}
                        width={Math.max(0, paneW - LABEL_W)}
                        height={ROW_H}
                        fill="transparent"
                        onMouseEnter={() => setHoverLane(l.id)}
                      />
                    </g>
                  ))}
                  {/* Markers, clipped to the axis so a pan never paints
                      over the label column. */}
                  <clipPath id="timeline-lanes">
                    <rect
                      x={LABEL_W}
                      y={0}
                      width={innerW + PAD_R}
                      height={lanesH}
                    />
                  </clipPath>
                  <g clipPath="url(#timeline-lanes)">
                    {lanes.map((l, lane) => {
                      const clusters = laneClusters.get(l.id) ?? [];
                      const cy = lane * ROW_H + ROW_H / 2;
                      // Every mark's x in this lane, for title room.
                      const marks: number[] = [];
                      const unfolded = new Set<Cluster>();
                      for (const c of clusters) {
                        const items = c.batches.flatMap((b) => b.items);
                        const spacing =
                          (c.x1 - c.x0) / Math.max(1, items.length);
                        if (
                          daysMode &&
                          c.batches.length === 1 &&
                          spacing >= TICK_SPACING
                        ) {
                          unfolded.add(c);
                          for (const it of items) marks.push(xOf(it.createdAt));
                        } else {
                          marks.push(c.x0, c.x1 + 24);
                        }
                      }
                      marks.sort((a, b) => a - b);
                      const titledSlots = new Set<number>();
                      const roomAfter = (x: number) => {
                        // First mark strictly to the right of x.
                        let lo = 0;
                        let hi = marks.length;
                        while (lo < hi) {
                          const mid = (lo + hi) >> 1;
                          if (marks[mid] <= x + 0.5) lo = mid + 1;
                          else hi = mid;
                        }
                        return lo < marks.length
                          ? marks[lo] - x
                          : Number.POSITIVE_INFINITY;
                      };
                      return clusters.map((c) => {
                        if (c.x1 < LABEL_W || c.x0 > paneW) return null;
                        const count = c.sources + c.notes;
                        const h = Math.min(
                          22,
                          8 + Math.log2(Math.max(1, count)) * 2.6,
                        );
                        const outlined = c.sources === 0;
                        const several = c.batches.length > 1;
                        const unfold = unfolded.has(c);
                        const one = c.batches[0];
                        const isSelected =
                          !!selected &&
                          c.batches.some(
                            (b) =>
                              b.notebookId === selected.notebookId &&
                              b.start === selected.start,
                          );
                        return (
                          <g
                            key={`${l.id}:${c.start}`}
                            data-node
                            onMouseEnter={(e) => {
                              setHoverLane(l.id);
                              clusterCard(e, c);
                            }}
                            onMouseLeave={hideCard}
                          >
                            {several && (
                              <rect
                                x={c.x0 - 3}
                                y={cy - h / 2 - 3}
                                width={c.x1 - c.x0 + 6}
                                height={h + 6}
                                rx={(h + 6) / 2}
                                fill="none"
                                stroke={l.color}
                                strokeOpacity={0.6}
                                strokeWidth={1}
                              />
                            )}
                            <rect
                              x={c.x0}
                              y={cy - h / 2}
                              width={c.x1 - c.x0}
                              height={h}
                              rx={h / 2}
                              fill={outlined ? "var(--background)" : l.color}
                              fillOpacity={outlined ? 1 : unfold ? 0.25 : 0.85}
                              stroke={
                                isSelected
                                  ? "var(--foreground)"
                                  : outlined
                                    ? l.color
                                    : "none"
                              }
                              strokeWidth={
                                isSelected ? 1.5 : outlined ? 1.5 : 0
                              }
                              className="cursor-pointer"
                              onClick={() => {
                                if (dragged.current) return;
                                // Several imports fused by the zoom: zoom
                                // until they part. One import: show it.
                                if (several) zoomToSpan(c.start, c.end, HOUR);
                                else setSelected(isSelected ? null : one);
                              }}
                            >
                              <title>
                                {`${l.title} · ${counts(c)}${several ? ` · ${plural(c.batches.length, "import")}` : ""}`}
                              </title>
                            </rect>
                            {!unfold && daysMode && (
                              <text
                                x={c.x1 + (several ? 9 : 6)}
                                y={cy + 3.5}
                                className="pointer-events-none text-micro tabular-nums fill-muted-foreground"
                              >
                                {count}
                              </text>
                            )}
                            {unfold &&
                              one.items.map((it) => {
                                const x = xOf(it.createdAt);
                                // Documents saved within a few pixels of
                                // each other share one title slot.
                                const slot = Math.round(x / 4);
                                const titled =
                                  !titledSlots.has(slot) &&
                                  roomAfter(x) >= TITLE_SPACING;
                                if (titled) titledSlots.add(slot);
                                return (
                                  <g
                                    key={it.id}
                                    data-node
                                    className="cursor-pointer"
                                    onMouseEnter={(e) => {
                                      e.stopPropagation();
                                      itemCard(e, one, it);
                                    }}
                                    onClick={() => {
                                      if (!dragged.current) openItem(one, it);
                                    }}
                                  >
                                    {/* The hit target: the tick plus its
                                        title, not a three-pixel line. */}
                                    <rect
                                      x={x - TICK_HIT / 2}
                                      y={cy - ROW_H / 2 + 2}
                                      width={
                                        titled
                                          ? TICK_HIT / 2 + 6 + 140
                                          : TICK_HIT
                                      }
                                      height={ROW_H - 4}
                                      fill="transparent"
                                    />
                                    <rect
                                      x={x - 1.5}
                                      y={cy - ROW_H / 2 + 8}
                                      width={3}
                                      height={ROW_H - 16}
                                      rx={1.5}
                                      fill={
                                        it.kind === "note"
                                          ? "var(--background)"
                                          : l.color
                                      }
                                      stroke={l.color}
                                      strokeWidth={it.kind === "note" ? 1.2 : 0}
                                      opacity={it.origin === "auto" ? 0.55 : 1}
                                    />
                                    {titled && (
                                      <text
                                        x={x + 6}
                                        y={cy + 3.5}
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
                      });
                    })}
                  </g>
                </svg>
                {/* Lane labels over the SVG, in HTML so titles truncate. */}
                {lanes.map((l, i) => (
                  <div
                    key={l.id}
                    className={cn(
                      "absolute left-0 flex items-center gap-2 pr-3",
                      hoverLane === l.id ? "bg-surface-2/60" : "bg-background",
                    )}
                    style={{ top: i * ROW_H, height: ROW_H, width: LABEL_W }}
                    onMouseEnter={() => setHoverLane(l.id)}
                  >
                    <span
                      aria-hidden
                      className="h-2 w-2 shrink-0 rounded-full"
                      style={{ backgroundColor: l.color }}
                    />
                    <button
                      type="button"
                      data-node
                      onClick={() => {
                        if (!dragged.current)
                          void useStore.getState().selectNotebook(l.id);
                      }}
                      title={`Open ${l.title}`}
                      className="min-w-0 truncate text-left text-caption text-foreground/85 transition-colors hover:text-foreground hover:underline underline-offset-2"
                    >
                      {l.title}
                    </button>
                  </div>
                ))}
              </div>
            </>
          )}
        </div>

        {selected && (
          <BatchPanel
            batch={selected}
            onClose={() => setSelected(null)}
            onZoom={() => zoomToSpan(selected.start, selected.end, DAY)}
            onOpen={(it) => openItem(selected, it)}
            onNotebook={() =>
              void useStore.getState().selectNotebook(selected.notebookId)
            }
          />
        )}

        <div
          className="absolute bottom-3 z-10 flex items-center gap-0.5 rounded-lg border border-border bg-surface-2/90 p-0.5 backdrop-blur"
          style={{ right: (selected ? PANEL_W : 0) + 12 }}
        >
          <ZoomButton
            label="Zoom out (−)"
            onClick={() => zoomBy(1 / 1.5)}
            icon={<Minus className="h-3.5 w-3.5" />}
          />
          <span
            className="px-2 py-1 text-micro tabular-nums text-muted-foreground"
            title="What the pane shows right now"
          >
            {fmtDay(visT0)} – {fmtDayYear(visT1)} ·{" "}
            {fmtSpanLength(visT1 - visT0)}
          </span>
          <ZoomButton
            label="Zoom in (+)"
            onClick={() => zoomBy(1.5)}
            icon={<Plus className="h-3.5 w-3.5" />}
          />
          <span aria-hidden className="mx-0.5 h-3.5 w-px bg-border-strong" />
          <button
            type="button"
            data-node
            onClick={today}
            title="Scroll to the present (T)"
            className="rounded-md px-2 py-1 text-micro font-medium text-muted-foreground transition-colors hover:bg-surface hover:text-foreground"
          >
            Today
          </button>
          <button
            type="button"
            data-node
            onClick={fit}
            title="Fit the whole corpus (0)"
            className="flex items-center gap-1 rounded-md px-2 py-1 text-micro font-medium text-muted-foreground transition-colors hover:bg-surface hover:text-foreground"
          >
            <Crosshair className="h-3.5 w-3.5" />
            Fit
          </button>
        </div>
        {hoverCard}
      </div>
      <div className="shrink-0 px-6 pb-2 pt-1 text-caption text-subtle-foreground">
        {NAV_HINT}
      </div>
    </div>
  );
}

/** What the markers mean, in one line: the question every first look asks. */
function Legend() {
  const dot = (fill: string, stroke: string, ring?: boolean) => (
    <svg width={ring ? 18 : 12} height={ring ? 18 : 12} aria-hidden>
      {ring && (
        <circle
          cx={9}
          cy={9}
          r={8}
          fill="none"
          stroke="currentColor"
          strokeOpacity={0.6}
        />
      )}
      <circle
        cx={ring ? 9 : 6}
        cy={ring ? 9 : 6}
        r={5}
        fill={fill}
        stroke={stroke}
        strokeWidth={1.5}
      />
    </svg>
  );
  return (
    <div className="flex shrink-0 items-center gap-3 text-micro text-muted-foreground">
      <span className="flex items-center gap-1.5">
        {dot("currentColor", "none")} sources
      </span>
      <span className="flex items-center gap-1.5">
        {dot("var(--background)", "currentColor")} notes only
      </span>
      <span className="flex items-center gap-1.5">
        {dot("currentColor", "none", true)} several imports
      </span>
      <span>size = how many</span>
    </div>
  );
}

/** The batch a click opened: its documents, each a click from the reader,
 *  with the axis left exactly where it was. */
function BatchPanel({
  batch,
  onClose,
  onZoom,
  onOpen,
  onNotebook,
}: {
  batch: TimelineBatch;
  onClose: () => void;
  onZoom: () => void;
  onOpen: (item: TimelineItem) => void;
  onNotebook: () => void;
}) {
  return (
    <div
      className="absolute inset-y-0 right-0 z-20 flex flex-col border-l border-border bg-background"
      style={{ width: PANEL_W }}
      role="region"
      aria-label={`Batch in ${batch.notebookTitle}`}
    >
      <div className="flex items-start justify-between gap-2 border-b border-border px-4 py-3">
        <div className="min-w-0">
          <button
            type="button"
            onClick={onNotebook}
            className="flex min-w-0 items-center gap-2 text-body font-medium text-foreground hover:underline underline-offset-2"
            title={`Open ${batch.notebookTitle}`}
          >
            <span
              aria-hidden
              className="h-2 w-2 shrink-0 rounded-full"
              style={{
                backgroundColor: batch.notebookColor || NOTEBOOK_PALETTE[0],
              }}
            />
            <span className="truncate">{batch.notebookTitle}</span>
            <ArrowUpRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          </button>
          <div className="mt-0.5 text-caption text-muted-foreground">
            {fmtSpan(batch.start, batch.end)}
          </div>
          <div className="text-caption text-muted-foreground">
            {counts(batch)}
          </div>
        </div>
        <Button
          variant="ghost"
          size="icon"
          onClick={onClose}
          aria-label="Close"
        >
          <X className="h-4 w-4" />
        </Button>
      </div>
      <div className="flex gap-2 border-b border-border px-4 py-2">
        <Button
          variant="secondary"
          onClick={onZoom}
          title="Fill the pane with this batch's day"
        >
          <ZoomIn className="h-3.5 w-3.5" />
          Zoom to its day
        </Button>
      </div>
      <ul className="min-h-0 flex-1 overflow-y-auto py-1">
        {batch.items.map((it) => (
          <li key={it.id}>
            <button
              type="button"
              onClick={() => onOpen(it)}
              title={it.title}
              className="flex w-full items-center gap-2 px-4 py-1.5 text-left transition-colors hover:bg-surface-2"
            >
              <span className="flex h-4 w-4 shrink-0 items-center justify-center text-muted-foreground [&>svg]:h-3.5 [&>svg]:w-3.5">
                {itemIcon(it)}
              </span>
              <span
                className={cn(
                  "min-w-0 flex-1 truncate text-caption text-foreground",
                  it.origin === "auto" && "text-muted-foreground",
                )}
              >
                {it.title || "Untitled"}
              </span>
              <span className="shrink-0 text-micro tabular-nums text-subtle-foreground">
                {fmtTime(it.createdAt)}
              </span>
            </button>
          </li>
        ))}
      </ul>
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
