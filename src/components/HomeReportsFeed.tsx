import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import type { Note, SourceEvent } from "@/lib/types";
import { cn, noteUnread, relativeTime } from "@/lib/utils";
import { eventCount, tallyEvents, unseenEvents } from "@/lib/arrivals";
import { Button } from "./ui";
import {
  ChevronDown,
  ChevronUp,
  Newspaper,
  PanelRightClose,
} from "lucide-react";
import { Markdown } from "./Markdown";

/** One quiet line describing activity since the previous home visit. */
export function AwayDigest({
  prevVisit,
  notebooks,
  reports,
  events = [],
}: {
  prevVisit: number;
  notebooks: { updatedAt: number }[];
  reports: Note[];
  /** Source-change events across notebooks; the arrivals since the last
   *  visit join the line (RFC-events §6). */
  events?: SourceEvent[];
}) {
  if (!prevVisit) return null;
  const newReports = reports.filter((report) => report.updatedAt > prevVisit).length;
  const updatedNotebooks = notebooks.filter(
    (notebook) => notebook.updatedAt > prevVisit,
  ).length;
  const parts = [
    newReports > 0 && `${newReports} new ${newReports === 1 ? "report" : "reports"}`,
    updatedNotebooks > 0 &&
      `${updatedNotebooks} ${updatedNotebooks === 1 ? "notebook" : "notebooks"} updated`,
    ...tallyEvents(unseenEvents(events, prevVisit)),
  ].filter(Boolean);
  if (parts.length === 0) return null;
  return (
    <p className="mt-0.5 text-caption text-subtle-foreground">
      Since you were away: {parts.join(" · ")}
    </p>
  );
}

/** Last night, as a window: the twelve hours ending at 9am today, or ending
 *  now when it is still before nine. Everything the Night Shift is meant to
 *  do happens inside it, so one range answers "what got done while I was
 *  asleep" without asking the backend for a second opinion. */
export function lastNightWindow(now = Date.now()): { start: number; end: number } {
  const nine = new Date(now);
  nine.setHours(9, 0, 0, 0);
  const end = Math.min(nine.getTime(), now);
  return { start: end - 12 * 60 * 60 * 1000, end };
}

/** The Library footer's one line: what the night shift did, counted from the
 *  same rows the Brief and the reports feed read. Clauses whose count is zero
 *  are left out rather than written as "0"; when every count is zero there
 *  was no run to describe.
 *
 *  Duplicates set aside are deliberately absent: hygiene findings live per
 *  notebook in Grow, and reading them here would mean a corpus scan on every
 *  Home render (the scan-storm lesson). When a corpus-wide count exists it
 *  joins this list. */
export function lastNightLine({
  reports,
  events,
  now = Date.now(),
}: {
  /** Report-kind notes across every notebook, Briefs included. */
  reports: Note[];
  events: SourceEvent[];
  now?: number;
}): string {
  const { start, end } = lastNightWindow(now);
  const inWindow = (at: number) => at >= start && at <= end;
  const written = reports.filter((r) => inWindow(r.updatedAt)).length;
  const tally = (kind: string) =>
    events
      .filter((e) => e.kind === kind && inWindow(e.at))
      .reduce((n, e) => n + eventCount(e), 0);
  const refreshed = tally("updated");
  const added = tally("added");
  const parts = [
    written > 0 && `${written} ${written === 1 ? "report" : "reports"} written`,
    refreshed > 0 && `${refreshed} ${refreshed === 1 ? "source" : "sources"} refreshed`,
    added > 0 && `${added} ${added === 1 ? "source" : "sources"} added`,
  ].filter(Boolean) as string[];
  if (parts.length === 0) return "No nightly run yet.";
  return `Last night: ${parts.join(", ")}.`;
}

/** Read reports revealed per press of "Load older reports", and the number
 *  a caught-up feed opens with. */
const PAGE = 5;

/** Unread reports first, followed by already-read reports on demand. */
export function ReportsFeed({
  reports,
  notebookTitle,
  notebookColor,
  fallbackColor,
  onOpen,
  onCollapse,
}: {
  reports: Note[];
  notebookTitle: Map<string, string>;
  notebookColor: Map<string, string>;
  fallbackColor: string;
  onOpen: (note: Note) => void;
  /** Collapse the feed to its rail (home treats it as a sidebar). */
  onCollapse?: () => void;
}) {
  const reads = useStore((state) => state.noteReads);
  const baseline = useStore((state) => state.noteReadsBaseline);
  const markRead = useStore((state) => state.markNotesRead);
  const isUnread = (note: Note) => noteUnread(note, reads, baseline);
  const unreadCount = reports.filter(isUnread).length;

  // Freeze group membership for this visit so cards do not jump while reading.
  const initialReads = useRef<Record<string, number> | null>(null);
  if (initialReads.current === null) initialReads.current = { ...reads };
  const wasUnread = (note: Note) =>
    noteUnread(note, initialReads.current ?? {}, baseline);
  const unread = reports.filter(wasUnread);
  const read = reports.filter((note) => !wasUnread(note));

  // Caught up is not the same as empty. With nothing unread the feed used
  // to render no cards at all and put "Load older reports" under the
  // all-caught-up line — a panel of eight reports showing none of them, over
  // a button offering something older than nothing. A feed that holds
  // reports shows reports; the button then means what it says.
  const [readShown, setReadShown] = useState(() =>
    unread.length === 0 ? Math.min(read.length, PAGE) : 0,
  );
  const visibleRead = read.slice(0, readShown);
  const remaining = read.length - visibleRead.length;

  // Prev/next stepping with an "n of M" cursor — a long feed is hard to
  // place yourself in by scroll alone. The cursor follows manual scrolling
  // (topmost visible card wins) and stepping past the rendered tail loads
  // more read reports first.
  const rendered = [...unread, ...visibleRead];
  const orderedIds = [...unread, ...read].map((n) => n.id);
  const total = reports.length;
  const scrollRef = useRef<HTMLDivElement>(null);
  const cardRefs = useRef(new Map<string, HTMLDivElement>());
  const [current, setCurrent] = useState(0);

  const syncCurrent = () => {
    const el = scrollRef.current;
    if (!el) return;
    const top = el.getBoundingClientRect().top;
    let idx = 0;
    rendered.forEach((n, i) => {
      const rect = cardRefs.current.get(n.id)?.getBoundingClientRect();
      if (rect && rect.top - top <= 24) idx = i;
    });
    setCurrent(idx);
  };

  const step = (delta: number) => {
    const target = Math.max(0, Math.min(total - 1, current + delta));
    const needShown = target - unread.length + 1;
    if (needShown > readShown) setReadShown(needShown);
    // Two frames: one for the newly shown card to mount, one to scroll it.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        cardRefs.current
          .get(orderedIds[target])
          ?.scrollIntoView({ block: "start", behavior: "smooth" });
        setCurrent(target);
      }),
    );
  };

  return (
    <>
      {/* Nothing here hides until you hover it. The stepping cursor is how you
          read the feed, not a secondary verb — "3 of 8" is the only place the
          position is written down — and Mark all read is the way out of a
          backlog, which is exactly when it should be in sight. A control you
          have to hover to find is a control you don't know you have.
          The card is user-resizable, so the wrap keys off the header's OWN
          width rather than the window's: a container query. Wide, it is one
          line; narrow, the identity and the collapse toggle keep line one and
          the verbs drop to line two, which is the pair you can afford to look
          down for. The header grows a row and the scroll region below gives
          up the height (min-h-0 flex-1), so nothing overlaps. */}
      <div className="@container shrink-0 border-b border-border">
        <div className="grid min-h-12 grid-cols-[1fr_auto] items-center gap-x-2 gap-y-1 px-6 py-2 @md:grid-cols-[1fr_auto_auto]">
          {/* Same icon as the collapsed rail, and the same grammar as Staff and
              Chats across the way: icon, then the caption. */}
          <div className="col-start-1 row-start-1 flex min-w-0 items-center gap-2">
            <Newspaper className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <span className="truncate text-caption font-semibold uppercase tracking-wide text-muted-foreground">
              Latest reports
            </span>
            {unreadCount > 0 && (
              <span
                title={`${unreadCount} unread`}
                className="shrink-0 rounded-full bg-primary/15 px-1.5 py-0.5 text-badge font-medium tabular-nums text-citation"
              >
                {unreadCount}
              </span>
            )}
          </div>
          {/* The cursor and chevrons only mean something over visible cards —
              the caught-up state (nothing rendered) shows neither, and with
              nothing unread there is no second line at all. */}
          {(rendered.length > 0 || unreadCount > 0) && (
            <div className="col-span-2 col-start-1 row-start-2 flex items-center justify-end gap-2 @md:col-span-1 @md:col-start-2 @md:row-start-1">
              {rendered.length > 0 && (
                <>
                  <span className="whitespace-nowrap text-micro tabular-nums text-subtle-foreground">
                    {current + 1} of {total}
                  </span>
                  <div className="flex items-center">
                    <button
                      type="button"
                      onClick={() => step(-1)}
                      disabled={current <= 0}
                      title="Previous report"
                      aria-label="Jump to the previous report"
                      className="rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground disabled:opacity-40 disabled:hover:bg-transparent"
                    >
                      <ChevronUp className="h-4 w-4" />
                    </button>
                    <button
                      type="button"
                      onClick={() => step(1)}
                      disabled={current >= total - 1}
                      title="Next report"
                      aria-label="Jump to the next report"
                      className="rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground disabled:opacity-40 disabled:hover:bg-transparent"
                    >
                      <ChevronDown className="h-4 w-4" />
                    </button>
                  </div>
                </>
              )}
              {unreadCount > 0 && (
                <button
                  type="button"
                  onClick={() =>
                    markRead(reports.filter(isUnread).map((note) => note.id))
                  }
                  className="whitespace-nowrap text-micro text-muted-foreground transition-colors hover:text-foreground"
                >
                  Mark all read
                </button>
              )}
            </div>
          )}
          {onCollapse && (
            <button
              type="button"
              onClick={onCollapse}
              title="Collapse reports"
              aria-label="Collapse the reports feed"
              className="col-start-2 row-start-1 rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground @md:col-start-3"
            >
              <PanelRightClose className="h-4 w-4" />
            </button>
          )}
        </div>
      </div>
      <div ref={scrollRef} onScroll={syncCurrent} className="min-h-0 flex-1 overflow-y-auto">
        {/* Nothing new to read, said once at the top. It stands alone when
            the feed is empty and sits over the recent reports otherwise —
            either way it answers "is there anything for me" before the
            cards do. */}
        {unread.length === 0 && (
          <div
            className={cn(
              "px-6 text-center text-caption text-subtle-foreground",
              rendered.length > 0 ? "border-b border-border py-3" : "py-6",
            )}
          >
            You’re all caught up.
          </div>
        )}
        {rendered.map((note) => (
          <div
            key={note.id}
            ref={(el) => {
              if (el) cardRefs.current.set(note.id, el);
              else cardRefs.current.delete(note.id);
            }}
          >
            <ReportCard
              note={note}
              unread={isUnread(note)}
              onSeen={() => markRead([note.id])}
              notebook={notebookTitle.get(note.notebookId) ?? "Unknown notebook"}
              color={notebookColor.get(note.notebookId) || fallbackColor}
              onOpen={() => {
                markRead([note.id]);
                onOpen(note);
              }}
            />
          </div>
        ))}
        {rendered.length > 0 && remaining > 0 && (
          <div className="flex justify-center px-6 py-5">
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setReadShown((shown) => shown + PAGE)}
            >
              Load older reports
            </Button>
          </div>
        )}
      </div>
    </>
  );
}

function ReportCard({
  note,
  unread,
  onSeen,
  notebook,
  color,
  onOpen,
}: {
  note: Note;
  unread: boolean;
  onSeen: () => void;
  notebook: string;
  color: string;
  onOpen: () => void;
}) {
  const endRef = useRef<HTMLDivElement>(null);
  const seenRef = useRef(onSeen);
  seenRef.current = onSeen;

  useEffect(() => {
    const element = endRef.current;
    if (!element || !unread) return;
    const observer = new IntersectionObserver(([entry]) => {
      if (entry.isIntersecting) seenRef.current();
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [unread]);

  return (
    <article
      className={cn(
        "border-b border-border px-6 py-5",
        unread && "bg-primary/[0.04]",
      )}
    >
      <div className="flex items-center gap-1.5 text-micro text-subtle-foreground">
        <span
          className="inline-flex h-2 w-2 shrink-0 rounded-full"
          style={{ backgroundColor: color }}
          aria-hidden="true"
        />
        <span className="truncate">{notebook}</span>
        <span>·</span>
        <span className="shrink-0">{relativeTime(note.updatedAt)}</span>
        {unread && (
          <span className="ml-auto shrink-0 rounded-full bg-primary/15 px-1.5 py-0.5 text-badge font-medium text-citation">
            new
          </span>
        )}
      </div>
      <button
        type="button"
        onClick={onOpen}
        className="mt-1 block w-full text-left"
        title={`Open in "${notebook}"`}
      >
        <h3 className="text-section font-semibold text-foreground hover:underline">
          {note.title}
        </h3>
      </button>
      <div className="mt-2 text-body leading-relaxed">
        <Markdown>{note.content}</Markdown>
      </div>
      <div ref={endRef} aria-hidden="true" />
    </article>
  );
}
