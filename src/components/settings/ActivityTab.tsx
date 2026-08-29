import { useEffect, useMemo, useState, type ReactNode } from "react";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Spinner } from "../ui";
import { TileShader } from "./TileShader";
import type { ActivityStats, ModelStat } from "@/lib/types";
import {
  CalendarDays,
  Clock,
  Flame,
  Library,
  MessageSquare,
  Zap,
  Moon,
  Search,
  StickyNote,
  Sun,
  Sunrise,
  Sunset,
  Receipt,
  Trophy,
  type LucideIcon,
} from "lucide-react";

/** Settings → Activity (docs/RFC-activity-view.md): usage stats aggregated
 *  read-time in Rust from timestamps the app has always recorded. */

const RANGES = [
  { id: "all", label: "All" },
  { id: "30d", label: "30d", days: 30 },
  { id: "7d", label: "7d", days: 7 },
] as const;
type RangeId = (typeof RANGES)[number]["id"];

/** A quarter of history — enough texture without overflowing the pane. */
const WEEKS = 13;
const MS_DAY = 86_400_000;

const compact = new Intl.NumberFormat("en", {
  notation: "compact",
  maximumFractionDigits: 1,
});

function hourLabel(h: number): string {
  if (h < 0) return "—";
  if (h === 0) return "12 AM";
  if (h === 12) return "12 PM";
  return h < 12 ? `${h} AM` : `${h - 12} PM`;
}

/** Local "YYYY-MM-DD" — same bucketing the Rust side uses. */
function dayKey(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}

function wordsLine(words: number): string | null {
  if (words < 1000) return null;
  return `Your sources hold ~${compact.format(words)} words.`;
}

/** Time-of-day identity for the peak-hour tile — the icon IS the value,
 *  so the shape changes and the color stays quiet. */
function peakHourFace(h: number): LucideIcon {
  if (h < 0) return Clock;
  if (h < 5 || h >= 21) return Moon;
  if (h < 11) return Sunrise;
  if (h < 17) return Sun;
  return Sunset;
}

/** Micro-dollars as money. Sub-cent spend rounds to "$0.00" rather than
 *  being dressed up in extra decimals: the point of the tile is the order of
 *  magnitude, and a fake precision on a cost is the thing to avoid. */
function money(micros: number): string {
  return `$${(Math.max(0, micros) / 1_000_000).toFixed(2)}`;
}

function Tile({
  label,
  value,
  icon: Icon,
  tone,
  wash,
  shader,
}: {
  label: string;
  value: string;
  icon?: LucideIcon;
  /** Icon color class — reserved for live state (the burning streak);
   *  everything else stays neutral. */
  tone?: string;
  /** Faint background wash for a tile whose state is "on" (live streak). */
  wash?: string;
  /** A TileShader washing the tile interior — only for live state. */
  shader?: ReactNode;
}) {
  return (
    <div
      className={cn(
        "relative overflow-hidden rounded-md border border-border px-3 py-2",
        wash,
      )}
    >
      {shader}
      {/* Label lines and the icon box share leading-4 (16px), so the icon
          centers exactly on the TOP title line even when a label wraps. */}
      <div className="relative flex items-start justify-between gap-2 text-caption text-muted-foreground">
        <span className="min-w-0 leading-4">{label}</span>
        {Icon && (
          <span className="flex h-4 shrink-0 items-center">
            <Icon
              className={cn("h-3 w-3", tone ?? "text-subtle-foreground/70")}
            />
          </span>
        )}
      </div>
      <div className="relative mt-0.5 text-[1.0625rem] font-semibold tabular-nums">
        {value}
      </div>
    </div>
  );
}

/** Sub-second TTFTs read in ms, the rest in seconds. */
function fmtMs(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)}s`;
}

/** The fastest-models ranking: which model answers soonest, ordered.
 *
 *  Time to first token is the metric because it is the wait the user
 *  actually feels — throughput rides along as a secondary number, since a
 *  model can be quick to start and slow to finish. Bars are wait time
 *  (shorter = faster), so the ranking reads as a descending staircase, and
 *  each row shows how many runs it averages: a model measured once has no
 *  business silently outranking one measured fifty times. */
function SpeedRanking({ rows }: { rows: ModelStat[] }) {
  const slowest = Math.max(...rows.map((r) => r.avgTtftMs), 0);
  return (
    <div className="min-w-0">
      <div className="flex items-baseline justify-between gap-3 pb-1.5">
        <span className="text-caption font-medium text-muted-foreground">
          Fastest models
        </span>
        <span className="text-micro text-subtle-foreground">
          average time to first token
        </span>
      </div>
      {/* Says why it is empty rather than vanishing: a section that hides
          itself is indistinguishable from one that failed to load. */}
      {rows.length === 0 && (
        <p className="text-caption leading-relaxed text-subtle-foreground">
          No timings yet. Each chat records how long its model took to start
          answering; models you have already used appear here after their
          next reply.
        </p>
      )}
      <div className="flex flex-col gap-1">
        {rows.map((r, i) => {
          const lead = i === 0;
          return (
            <div
              key={r.name}
              className="grid grid-cols-[1rem_minmax(0,1fr)_5rem_auto] items-center gap-2.5 text-body"
            >
              <span className="text-caption tabular-nums text-subtle-foreground">
                {i + 1}
              </span>
              <span className="flex min-w-0 items-center gap-1.5">
                <span className="truncate">{r.name}</span>
                {lead && (
                  <Zap
                    className="h-3 w-3 shrink-0 text-success"
                    aria-label="Fastest"
                  />
                )}
              </span>
              {/* Wait-time bar, scaled to the slowest model in the list. */}
              <span
                className="h-1 overflow-hidden rounded-full bg-muted-foreground/15"
                aria-hidden
              >
                <span
                  className={cn(
                    "block h-full rounded-full",
                    lead ? "bg-success" : "bg-muted-foreground/50",
                  )}
                  style={{
                    width: `${slowest > 0 ? Math.max((r.avgTtftMs / slowest) * 100, 4) : 100}%`,
                  }}
                />
              </span>
              <span className="shrink-0 tabular-nums text-caption text-muted-foreground">
                {fmtMs(r.avgTtftMs)}
                <span className="text-subtle-foreground">
                  {r.avgTokensPerSec > 0 &&
                    ` \u00b7 ${Math.round(r.avgTokensPerSec)} tok/s`}
                  {` \u00b7 ${r.ttftSamples} run${r.ttftSamples === 1 ? "" : "s"}`}
                </span>
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function CountList({
  title,
  rows,
}: {
  title: string;
  rows: { label: string; count: number }[];
}) {
  if (rows.length === 0) return null;
  return (
    <div className="min-w-0">
      <div className="pb-1.5 text-caption font-medium text-muted-foreground">
        {title}
      </div>
      <div className="flex flex-col gap-1">
        {rows.map((r) => (
          <div
            key={r.label}
            className="flex items-baseline justify-between gap-3 text-body"
          >
            <span className="truncate">{r.label}</span>
            <span className="tabular-nums text-muted-foreground">
              {compact.format(r.count)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

/** GitHub-grammar heatmap: 52 week columns × 7 day rows, intensity scaled
 *  to the user's own p90 so light users still see texture. Color carries
 *  meaning (quantity) — DESIGN.md compliant. */
function Heatmap({ stats }: { stats: ActivityStats }) {
  const { columns, monthLabels, summary } = useMemo(() => {
    const totals = new Map<string, number>();
    for (const d of stats.days) {
      totals.set(d.date, d.messages + d.sources + d.notes + d.retrievals);
    }
    const nonzero = [...totals.values()].filter((v) => v > 0).sort((a, b) => a - b);
    const p90 = nonzero.length
      ? nonzero[Math.floor(0.9 * (nonzero.length - 1))]
      : 1;

    const today = new Date();
    today.setHours(12, 0, 0, 0); // noon dodges DST edges in day arithmetic
    const weekStart = new Date(today.getTime() - today.getDay() * MS_DAY);
    const start = new Date(weekStart.getTime() - (WEEKS - 1) * 7 * MS_DAY);

    const columns: { level: number; title: string; future: boolean }[][] = [];
    const monthLabels: string[] = [];
    let prevMonth = -1;
    for (let w = 0; w < WEEKS; w++) {
      const col = [];
      const first = new Date(start.getTime() + w * 7 * MS_DAY);
      // Label the column where a month starts. Column 0 usually begins
      // mid-month — labeling it crams against the real first label, so it
      // only earns one when it genuinely opens its month.
      if (first.getMonth() !== prevMonth) {
        prevMonth = first.getMonth();
        monthLabels.push(
          w > 0 || first.getDate() <= 7
            ? first.toLocaleString("en", { month: "short" })
            : "",
        );
      } else {
        monthLabels.push("");
      }
      for (let dow = 0; dow < 7; dow++) {
        const date = new Date(start.getTime() + (w * 7 + dow) * MS_DAY);
        const key = dayKey(date);
        const total = totals.get(key) ?? 0;
        const level =
          total === 0
            ? 0
            : Math.min(4, Math.max(1, Math.ceil((total / p90) * 4)));
        col.push({
          level,
          title: total === 0 ? key : `${key} — ${total} events`,
          future: date.getTime() > today.getTime(),
        });
      }
      columns.push(col);
    }
    // 91 undifferentiated squares are noise read one at a time, so the grid
    // is one labelled image: the shape of the last quarter in a sentence.
    let events = 0;
    let activeDays = 0;
    for (const total of totals.values()) {
      events += total;
      if (total > 0) activeDays += 1;
    }
    const summary = `Activity over the last ${WEEKS} weeks: ${events} events across ${activeDays} ${activeDays === 1 ? "day" : "days"}.`;
    return { columns, monthLabels, summary };
  }, [stats.days]);

  const LEVEL = [
    "bg-border/40",
    "bg-primary opacity-30",
    "bg-primary opacity-55",
    "bg-primary opacity-80",
    "bg-primary",
  ];
  return (
    <div className="flex flex-col gap-1" role="img" aria-label={summary}>
      <div className="flex gap-[3px]">
        {monthLabels.map((m, i) => (
          <div
            key={`m${String(i)}`}
            className="min-w-0 flex-1 overflow-visible whitespace-nowrap text-center text-[0.625rem] leading-4 text-subtle-foreground"
          >
            {m}
          </div>
        ))}
      </div>
      {/* Full-width: columns flex to fill the pane, so cells stretch
          horizontally while keeping a fixed height. */}
      <div className="flex gap-[3px]">
        {columns.map((col, w) => (
          <div
            key={`w${String(w)}`}
            className="flex min-w-0 flex-1 flex-col gap-[3px]"
          >
            {col.map((cell, d) => (
              <div
                key={`d${String(d)}`}
                title={cell.future ? undefined : cell.title}
                className={cn(
                  "h-3.5 w-full rounded-[3px]",
                  cell.future ? "opacity-0" : LEVEL[cell.level],
                )}
              />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

export function ActivityTab() {
  const [stats, setStats] = useState<ActivityStats | null>(null);
  const [modelStats, setModelStats] = useState<ModelStat[]>([]);
  const [error, setError] = useState(false);
  const [range, setRange] = useState<RangeId>("all");

  useEffect(() => {
    api
      .activityStats()
      .then(setStats)
      .catch(() => setError(true));
    api
      .getModelStats()
      .then(setModelStats)
      .catch(() => {});
  }, []);

  // Chat models with a measured time-to-first-token, quickest first.
  const speed = useMemo(
    () =>
      modelStats
        .filter((m) => m.avgTtftMs > 0)
        .sort((a, b) => a.avgTtftMs - b.avgTtftMs),
    [modelStats],
  );

  const ranged = useMemo(() => {
    if (!stats) return null;
    const r = RANGES.find((x) => x.id === range);
    if (!r || !("days" in r)) {
      return {
        messages: stats.totalMessages,
        sources: stats.totalSources,
        notes: stats.totalNotes,
        retrievals: stats.totalRetrievals,
      };
    }
    const cutoff = dayKey(new Date(Date.now() - (r.days - 1) * MS_DAY));
    const out = { messages: 0, sources: 0, notes: 0, retrievals: 0 };
    for (const d of stats.days) {
      if (d.date < cutoff) continue;
      out.messages += d.messages;
      out.sources += d.sources;
      out.notes += d.notes;
      out.retrievals += d.retrievals;
    }
    return out;
  }, [stats, range]);

  if (error) {
    return (
      <div className="py-8 text-center text-body text-muted-foreground">
        Couldn&rsquo;t load activity.
      </div>
    );
  }
  if (!stats || !ranged) {
    return (
      <div className="flex items-center justify-center py-8">
        <Spinner className="h-5 w-5 text-muted-foreground" />
      </div>
    );
  }
  if (stats.days.length === 0) {
    return (
      <div className="py-8 text-center text-body text-muted-foreground">
        Your activity will appear here as you use Alchemy.
      </div>
    );
  }

  const since = stats.firstActivityAt
    ? new Date(stats.firstActivityAt).toLocaleString("en", {
        month: "short",
        year: "numeric",
      })
    : null;
  const book = wordsLine(Math.round(stats.corpusChars / 6));

  return (
    <div className="flex flex-col gap-4 pb-2">
      <div className="flex items-center justify-between">
        <div className="text-caption text-subtle-foreground">
          {since ? `Since ${since}` : ""}
        </div>
        <div className="flex gap-0.5 rounded-md border border-border p-0.5">
          {RANGES.map((r) => (
            <button
              key={r.id}
              type="button"
              onClick={() => setRange(r.id)}
              className={cn(
                "rounded px-2 py-0.5 text-caption transition-colors",
                range === r.id
                  ? "bg-surface-2 font-medium text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {r.label}
            </button>
          ))}
        </div>
      </div>

      <div className="grid grid-cols-4 gap-2">
        <Tile
          label="Messages"
          value={compact.format(ranged.messages)}
          icon={MessageSquare}
        />
        <Tile
          label="Sources"
          value={compact.format(ranged.sources)}
          icon={Library}
        />
        <Tile
          label="Notes"
          value={compact.format(ranged.notes)}
          icon={StickyNote}
        />
        <Tile
          label="Retrievals"
          value={compact.format(ranged.retrievals)}
          icon={Search}
        />
        <Tile
          label="Active days"
          value={String(stats.activeDays)}
          icon={CalendarDays}
        />
        {/* The one colored tile: the flame only burns while a streak is
            alive — live state, not decor. Embers rise harder the longer
            the streak. */}
        <Tile
          label="Streak"
          value={`${String(stats.currentStreak)}d`}
          icon={Flame}
          tone={
            stats.currentStreak > 0 ? "text-artifact-template" : undefined
          }
          shader={
            stats.currentStreak > 0 ? (
              <TileShader
                mode="ember"
                tintVar="--artifact-template"
                intensity={0.6 + 0.4 * Math.min(1, stats.currentStreak / 7)}
              />
            ) : undefined
          }
        />
        <Tile
          label="Longest streak"
          value={`${String(stats.longestStreak)}d`}
          icon={Trophy}
        />
        <Tile
          label="Background spend"
          value={money(stats.backgroundCostMicros)}
          icon={Receipt}
        />
        {/* The sky IS the value: the sun sits where the hour puts it, or
            stars come out for a night-owl peak. */}
        <Tile
          label="Peak hour"
          value={hourLabel(stats.peakHour)}
          icon={peakHourFace(stats.peakHour)}
          shader={
            stats.peakHour >= 0 ? (
              <TileShader
                mode="sky"
                hour={stats.peakHour}
                tintVar={
                  stats.peakHour < 5 || stats.peakHour >= 21
                    ? "--citation"
                    : "--artifact-template"
                }
              />
            ) : undefined
          }
        />
      </div>

      <div className="text-caption text-subtle-foreground">
        Background spend covers scheduled work over the last 30 days. Local
        models are free, so it stays at $0.00 unless you point a role at a paid
        provider.
      </div>

      <div className="flex flex-col gap-1.5">
        <Heatmap stats={stats} />
        {book && (
          <div className="text-caption text-subtle-foreground">{book}</div>
        )}
      </div>

      <div className="grid grid-cols-2 gap-5">
        <CountList title="Most used models" rows={stats.models} />
        <CountList title="Most active notebooks" rows={stats.notebooks} />
      </div>

      <SpeedRanking rows={speed} />

      {stats.sourceTypes.length > 0 && (
        <div>
          <div className="pb-1.5 text-caption font-medium text-muted-foreground">
            Sources by type
          </div>
          <div className="flex flex-wrap gap-1.5">
            {stats.sourceTypes.map((t) => (
              <span
                key={t.label}
                className="rounded-full border border-border px-2 py-0.5 text-caption text-muted-foreground"
              >
                {t.label}{" "}
                <span className="tabular-nums text-subtle-foreground">
                  {compact.format(t.count)}
                </span>
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
