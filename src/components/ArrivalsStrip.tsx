import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ChevronRight, Inbox } from "lucide-react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import {
  EVENT_VERB,
  loadSeenAt,
  saveSeenAt,
  tallyEvents,
  unseenEvents,
} from "@/lib/arrivals";
import type { SourceEvent } from "@/lib/types";
import { cn, relativeTime } from "@/lib/utils";
import { CardAction } from "./ui";

/**
 * Arrivals (docs/RFC-events.md §6): one strip above the sources list when
 * the watchers saw something since the reader last dismissed it. Reads
 * `source_events` and nothing else; never calls a model. The watermark is
 * per notebook in the database (`app_state`), read on open.
 */

/** The events table keeps 30 days; a week is as far back as "since you
 *  looked" stays meaningful in a panel. */
const WINDOW_HOURS = 24 * 7;
/** Rows shown when the strip is open; the rest stay in the tally. */
const SHOWN = 30;
/** Diff lines per row — a glance, not a review. */
const DIFF_LINES = 6;

export function useArrivals(notebookId: string | null) {
  const [events, setEvents] = useState<SourceEvent[]>([]);
  // Read from the database on open and on notebook change; until it
  // answers, nothing counts as unseen (a flash of "25 new" would be wrong
  // more often than right).
  const [seenAt, setSeenAt] = useState(Number.MAX_SAFE_INTEGER);
  useEffect(() => {
    let live = true;
    setSeenAt(Number.MAX_SAFE_INTEGER);
    if (!notebookId) return;
    void loadSeenAt(notebookId).then((at) => {
      if (live) setSeenAt(at);
    });
    return () => {
      live = false;
    };
  }, [notebookId]);

  const requestId = useRef(0);
  const refresh = useCallback(async () => {
    const id = ++requestId.current;
    if (!notebookId) {
      setEvents([]);
      return;
    }
    try {
      const rows = await api.listSourceEvents(WINDOW_HOURS, notebookId);
      if (id === requestId.current) setEvents(rows);
    } catch {
      /* best-effort: the strip is a convenience, not a record */
    }
  }, [notebookId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Producers emit after they write, but not always after the event row
  // lands (reingest writes it inside the same pass) — a short settle keeps
  // the re-read from racing the writer. Multi-window: listeners are not
  // filtered by target, so filter by the payload's notebook.
  useEffect(() => {
    if (!notebookId) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const bump = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => void refresh(), 400);
    };
    const unA = listen<{ notebookId: string }>("sources://changed", (e) => {
      if (e.payload.notebookId === notebookId) bump();
    });
    const unB = listen<{ scope: string; notebookId: string | null }>(
      "mcp://changed",
      (e) => {
        if (!e.payload.notebookId || e.payload.notebookId === notebookId) bump();
      },
    );
    return () => {
      if (timer) clearTimeout(timer);
      void unA.then((f) => f());
      void unB.then((f) => f());
    };
  }, [notebookId, refresh]);

  const unseen = useMemo(() => unseenEvents(events, seenAt), [events, seenAt]);
  const sourceIds = useMemo(() => new Set(unseen.map((e) => e.sourceId)), [unseen]);

  const dismiss = useCallback(() => {
    if (!notebookId) return;
    // Not "now": a row that lands between the read and the click stays
    // unseen, and clock skew across a resync never hides a real arrival.
    const at = unseen.reduce((m, e) => Math.max(m, e.at), 0);
    saveSeenAt(notebookId, at);
    setSeenAt(at);
  }, [notebookId, unseen]);

  return { unseen, sourceIds, dismiss };
}

export function ArrivalsStrip({
  unseen,
  onDismiss,
}: {
  unseen: SourceEvent[];
  onDismiss: () => void;
}) {
  const [open, setOpen] = useState(false);
  const sources = useStore((s) => s.sources);
  const openInReader = useStore((s) => s.openInReader);
  const present = useMemo(() => new Set(sources.map((s) => s.id)), [sources]);
  const tallies = useMemo(() => tallyEvents(unseen), [unseen]);
  if (unseen.length === 0 || tallies.length === 0) return null;
  const shown = unseen.slice(0, SHOWN);
  const line = tallies.join(" · ");
  return (
    <div className="mb-1 rounded-md border border-border bg-surface-2/60">
      <div className="flex items-center gap-2 px-2 py-1.5">
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          aria-expanded={open}
          className="flex min-w-0 flex-1 items-center gap-2 text-left"
          title={line}
        >
          <Inbox className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <span className="min-w-0 flex-1 truncate text-caption text-foreground">
            {line}
          </span>
          <ChevronRight
            className={cn(
              "h-3 w-3 shrink-0 text-muted-foreground transition-transform duration-150",
              open && "rotate-90",
            )}
          />
        </button>
        <button
          type="button"
          onClick={onDismiss}
          className="shrink-0 whitespace-nowrap text-micro text-muted-foreground transition-colors hover:text-foreground"
          title="Clear the arrivals and the new-dots on their sources"
        >
          Mark seen
        </button>
      </div>
      {open && (
        <ul className="flex flex-col border-t border-border">
          {shown.map((e) => {
            const readable = e.kind !== "removed" && present.has(e.sourceId);
            const diff = e.diff
              ? e.diff.split("\n").slice(0, DIFF_LINES).join("\n")
              : "";
            const more = e.diff ? e.diff.split("\n").length - DIFF_LINES : 0;
            return (
              <li
                key={e.id}
                className={cn(
                  "group relative border-b border-border px-2 py-1.5 last:border-b-0",
                  readable && "cursor-pointer hover:bg-surface-2",
                )}
              >
                {readable && (
                  <CardAction
                    label={`Open ${e.sourceTitle}`}
                    onClick={() => openInReader({ type: "source", id: e.sourceId })}
                  />
                )}
                <div className="pointer-events-none relative z-10 flex items-baseline gap-1.5 text-micro">
                  <span className="shrink-0 text-muted-foreground">
                    {EVENT_VERB[e.kind] ?? e.kind}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-foreground">
                    {e.sourceTitle}
                  </span>
                  <span className="shrink-0 text-subtle-foreground">
                    {relativeTime(e.at)}
                  </span>
                </div>
                {e.detail && (
                  <div className="pointer-events-none relative z-10 truncate text-micro text-muted-foreground">
                    {e.detail}
                  </div>
                )}
                {diff && (
                  <pre className="pointer-events-none relative z-10 mt-1 overflow-hidden whitespace-pre-wrap break-words font-mono text-micro leading-snug text-subtle-foreground">
                    {diff}
                    {more > 0 && `\n… ${more} more`}
                  </pre>
                )}
              </li>
            );
          })}
          {unseen.length > SHOWN && (
            <li className="px-2 py-1.5 text-micro text-subtle-foreground">
              … and {unseen.length - SHOWN} more
            </li>
          )}
        </ul>
      )}
    </div>
  );
}
