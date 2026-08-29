import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import type { Note } from "@/lib/types";
import { cn, noteUnread, relativeTime } from "@/lib/utils";
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
}: {
  prevVisit: number;
  notebooks: { updatedAt: number }[];
  reports: Note[];
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
  ].filter(Boolean);
  if (parts.length === 0) return null;
  return (
    <p className="mt-0.5 text-caption text-subtle-foreground">
      Since you were away: {parts.join(" · ")}
    </p>
  );
}

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

  const [readShown, setReadShown] = useState(0);
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
      {/* The stepping cursor is how you read the feed, not a secondary verb:
          "3 of 8" is the only place the position is written down, and a
          control you have to hover to find is a control you don't know you
          have. It stays. Mark all read still waits for a hover or a tab into
          the header — it is destructive-ish and rare. */}
      <div className="group flex min-h-12 shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b border-border px-6 py-2">
        {/* Same icon as the collapsed rail, and the same grammar as Staff and
            Chats across the way: icon, then the caption. */}
        <Newspaper className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <span className="whitespace-nowrap text-caption font-semibold uppercase tracking-wide text-muted-foreground">
          Latest reports
        </span>
        {unreadCount > 0 && (
          <span
            title={`${unreadCount} unread`}
            className="rounded-full bg-primary/15 px-1.5 py-0.5 text-badge font-medium tabular-nums text-citation"
          >
            {unreadCount}
          </span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-2">
          {/* The cursor and chevrons only mean something over visible cards —
              the caught-up state (nothing rendered) shows neither. */}
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
              onClick={() => markRead(reports.filter(isUnread).map((note) => note.id))}
              className="whitespace-nowrap text-micro text-muted-foreground opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100 hover:text-foreground"
            >
              Mark all read
            </button>
          )}
          {onCollapse && (
            <button
              type="button"
              onClick={onCollapse}
              title="Collapse reports"
              aria-label="Collapse the reports feed"
              className="rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
            >
              <PanelRightClose className="h-4 w-4" />
            </button>
          )}
        </div>
      </div>
      <div ref={scrollRef} onScroll={syncCurrent} className="min-h-0 flex-1 overflow-y-auto">
        {/* Once older reports are loaded, the space belongs to them. */}
        {unread.length === 0 && readShown === 0 && (
          <div className="px-6 py-6 text-center text-caption text-subtle-foreground">
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
        {remaining > 0 && (
          <div className="flex justify-center px-6 py-5">
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setReadShown((shown) => shown + 5)}
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
