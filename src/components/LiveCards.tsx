import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  CalendarDays,
  Circle,
  CircleCheck,
  ExternalLink,
  ListChecks,
  RefreshCw,
  Rss,
  TrendingUp,
} from "lucide-react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import {
  isStale,
  liveKind,
  parseLiveCard,
  type CalendarCard,
  type FeedCard,
  type LiveCard,
  type LiveKind,
  type RemindersCard,
  type StocksCard,
} from "@/lib/liveCards";
import type { Citation, Source } from "@/lib/types";
import { cn, fmtDateTime, fmtDay, relativeTime } from "@/lib/utils";

/**
 * Live answer cards (docs/RFC-events.md §7). A reply that cites a live
 * source — a Mac list or a feed — grows a native card beneath the prose,
 * derived at render time from `Message.citations` and the source's STORED
 * text. Nothing is persisted and no model runs: old transcripts light up,
 * refresh re-fetches the source and the card re-renders, the prose stands.
 * When the text does not parse there is no card at all.
 */

/** Rows per card before "+N more" — a card is a glance, the reader has the
 *  whole list. */
const SHOWN = 12;

/** Source text by id, stamped with the fetch that produced it, so ten
 *  replies citing the same watchlist read it once. */
const contentCache = new Map<string, { stamp: string; text: string }>();

export function LiveCards({ citations }: { citations: Citation[] }) {
  const sources = useStore((s) => s.sources);
  const live = useMemo(() => {
    const seen = new Set<string>();
    const out: { source: Source; kind: LiveKind }[] = [];
    for (const c of citations) {
      if (!c.sourceId || seen.has(c.sourceId)) continue;
      seen.add(c.sourceId);
      const source = sources.find((s) => s.id === c.sourceId);
      const kind = source && liveKind(source);
      if (source && kind) out.push({ source, kind });
    }
    return out;
  }, [citations, sources]);
  if (live.length === 0) return null;
  return (
    <div className="flex flex-col gap-2">
      {live.map(({ source, kind }) => (
        <LiveCardView key={source.id} source={source} kind={kind} />
      ))}
    </div>
  );
}

function LiveCardView({ source, kind }: { source: Source; kind: LiveKind }) {
  const refreshSource = useStore((s) => s.refreshSource);
  const [text, setText] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // A resync in this notebook re-reads the text even when the list row's
  // stamps have not reached this window yet (multi-window: filter by the
  // payload's notebook, never the event target).
  const [bump, setBump] = useState(0);
  useEffect(() => {
    const un = listen<{ notebookId: string }>("sources://changed", (e) => {
      if (e.payload.notebookId === source.notebookId) setBump((b) => b + 1);
    });
    return () => void un.then((f) => f());
  }, [source.notebookId]);

  const stamp = `${source.fetchedAt}:${source.mtime}`;
  useEffect(() => {
    let stale = false;
    const hit = contentCache.get(source.id);
    if (hit && hit.stamp === stamp && bump === 0) {
      setText(hit.text);
      return;
    }
    api
      .getSourceContent(source.id)
      .then((t) => {
        contentCache.set(source.id, { stamp, text: t });
        if (!stale) setText(t);
      })
      .catch(() => {
        if (!stale) setText(null);
      });
    return () => {
      stale = true;
    };
  }, [source.id, stamp, bump]);

  const card = useMemo(() => (text ? parseLiveCard(kind, text) : null), [kind, text]);
  if (!card) return null;

  const fetchedAt = source.fetchedAt || source.createdAt;
  const stale = isStale(fetchedAt);
  const Icon = ICON[kind];

  async function refresh() {
    setRefreshing(true);
    try {
      await refreshSource(source.id);
    } finally {
      setRefreshing(false);
    }
  }

  return (
    <section
      aria-label={`${LABEL[kind]} from ${source.title}`}
      className="overflow-hidden rounded-lg border border-border bg-surface"
    >
      <header className="flex items-center gap-2 border-b border-border px-3 py-1.5">
        <Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <span className="min-w-0 flex-1 truncate text-caption font-medium text-foreground">
          {source.title}
        </span>
        <span
          className="shrink-0 text-micro text-subtle-foreground"
          title={fetchedAt ? fmtDateTime(fetchedAt) : undefined}
        >
          {stale ? `as of ${fmtDateTime(fetchedAt)}` : fetchedAt ? `fetched ${relativeTime(fetchedAt)}` : ""}
        </span>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={refreshing}
          title="Fetch again"
          aria-label={`Fetch ${source.title} again`}
          className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground disabled:opacity-50"
        >
          <RefreshCw className={cn("h-3.5 w-3.5", refreshing && "animate-spin")} />
        </button>
      </header>
      <CardBody card={card} />
    </section>
  );
}

const ICON = {
  stocks: TrendingUp,
  calendar: CalendarDays,
  reminders: ListChecks,
  feed: Rss,
} as const;

const LABEL: Record<LiveKind, string> = {
  stocks: "Quotes",
  calendar: "Events",
  reminders: "Reminders",
  feed: "Entries",
};

function CardBody({ card }: { card: LiveCard }) {
  switch (card.kind) {
    case "stocks":
      return <StocksBody card={card} />;
    case "calendar":
      return <CalendarBody card={card} />;
    case "reminders":
      return <RemindersBody card={card} />;
    case "feed":
      return <FeedBody card={card} />;
  }
}

function More({ n }: { n: number }) {
  return n > 0 ? (
    <div className="px-3 py-1.5 text-micro text-subtle-foreground">+{n} more in the source</div>
  ) : null;
}

/** RFC 3339 → the app's date-time format; the raw string when it isn't one. */
function fmtStamp(s: string): string {
  const t = Date.parse(s);
  return Number.isNaN(t) ? s : fmtDateTime(t);
}

/** YYYY-MM-DD → "Sep 4, 2026" in local time (parse as a date, not UTC
 *  midnight, or the day shifts west of Greenwich). */
function fmtIsoDay(d: string): string {
  const [y, m, day] = d.split("-").map(Number);
  return y && m && day ? fmtDay(new Date(y, m - 1, day).getTime()) : d;
}

function StocksBody({ card }: { card: StocksCard }) {
  const rows = card.rows.slice(0, SHOWN);
  return (
    <>
      <div className="overflow-x-auto">
        <table className="w-full text-caption">
          <thead>
            <tr className="border-b border-border-strong text-micro font-medium text-muted-foreground">
              <th className="px-3 py-1.5 text-left font-medium">Symbol</th>
              <th className="px-3 py-1.5 text-right font-medium">Price</th>
              <th className="px-3 py-1.5 text-right font-medium">Change</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.symbol} className="border-b border-border last:border-b-0">
                <td className="px-3 py-1.5">
                  <span className="font-medium text-foreground">{r.symbol}</span>
                  {r.name && (
                    <span className="ml-2 text-micro text-muted-foreground">{r.name}</span>
                  )}
                </td>
                <td className="px-3 py-1.5 text-right tabular-nums text-foreground">
                  {r.price || "—"}
                </td>
                <td
                  className={cn(
                    "px-3 py-1.5 text-right tabular-nums",
                    r.changePct === null
                      ? "text-subtle-foreground"
                      : r.changePct > 0
                        ? "text-success"
                        : r.changePct < 0
                          ? "text-destructive"
                          : "text-muted-foreground",
                  )}
                >
                  {r.change || "—"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <More n={card.rows.length - rows.length} />
      {card.asOf && (
        <div className="border-t border-border px-3 py-1.5 text-micro text-subtle-foreground">
          Prices as of {fmtStamp(card.asOf)} · Apple Stocks
        </div>
      )}
    </>
  );
}

function CalendarBody({ card }: { card: CalendarCard }) {
  const events = card.events.slice(0, SHOWN);
  let lastDay = "";
  return (
    <>
      <ul className="flex flex-col">
        {events.map((e, i) => {
          const heading = e.day !== lastDay ? e.day : null;
          lastDay = e.day;
          return (
            <li key={i} className="border-b border-border last:border-b-0">
              {heading && (
                <div className="px-3 pt-2 text-micro font-medium text-muted-foreground">
                  {fmtIsoDay(heading)}
                </div>
              )}
              <div className="flex items-baseline gap-3 px-3 py-1.5 text-caption">
                <span className="w-14 shrink-0 tabular-nums text-muted-foreground">{e.time}</span>
                <span className="min-w-0 flex-1">
                  <span className="text-foreground">{e.title}</span>
                  <span className="ml-2 text-micro text-muted-foreground">{e.calendar}</span>
                  {e.location && (
                    <span className="block truncate text-micro text-subtle-foreground">
                      {e.location}
                    </span>
                  )}
                </span>
              </div>
            </li>
          );
        })}
      </ul>
      <More n={card.events.length - events.length} />
    </>
  );
}

function RemindersBody({ card }: { card: RemindersCard }) {
  const items = card.items.slice(0, SHOWN);
  return (
    <>
      <ul className="flex flex-col">
        {items.map((r, i) => (
          <li
            key={r.id ?? i}
            className="flex items-baseline gap-2 border-b border-border px-3 py-1.5 text-caption last:border-b-0"
          >
            {r.done ? (
              <CircleCheck className="h-3.5 w-3.5 shrink-0 translate-y-0.5 text-success" aria-label="Done" />
            ) : (
              <Circle className="h-3.5 w-3.5 shrink-0 translate-y-0.5 text-subtle-foreground" aria-label="Open" />
            )}
            <span className="min-w-0 flex-1">
              <span className={cn("text-foreground", r.done && "text-muted-foreground line-through")}>
                {r.title}
              </span>
              {r.notes && (
                <span className="block truncate text-micro text-subtle-foreground">{r.notes}</span>
              )}
            </span>
            {r.due && (
              <span className="shrink-0 text-micro tabular-nums text-muted-foreground">
                due {fmtIsoDay(r.due)}
              </span>
            )}
          </li>
        ))}
      </ul>
      <More n={card.items.length - items.length} />
    </>
  );
}

function FeedBody({ card }: { card: FeedCard }) {
  const entries = card.entries.slice(0, SHOWN);
  return (
    <>
      <ul className="flex flex-col">
        {entries.map((e, i) => (
          <li
            key={e.link ?? i}
            className="flex items-baseline gap-3 border-b border-border px-3 py-1.5 text-caption last:border-b-0"
          >
            <span className="w-20 shrink-0 tabular-nums text-micro text-muted-foreground">
              {fmtIsoDay(e.published.slice(0, 10))}
            </span>
            <span className="min-w-0 flex-1">
              {e.link ? (
                <button
                  type="button"
                  onClick={() => void openUrl(e.link!)}
                  className="inline-flex max-w-full items-center gap-1 text-left text-citation hover:underline"
                  title={e.link}
                >
                  <span className="truncate">{e.title}</span>
                  <ExternalLink className="h-3 w-3 shrink-0" />
                </button>
              ) : (
                <span className="text-foreground">{e.title}</span>
              )}
              {e.excerpt && (
                <span className="block truncate text-micro text-subtle-foreground">{e.excerpt}</span>
              )}
            </span>
          </li>
        ))}
      </ul>
      <More n={card.entries.length - entries.length} />
    </>
  );
}
