import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import type { Note } from "@/lib/types";
import { cn, noteUnread, relativeTime } from "@/lib/utils";
import { Button } from "./ui";
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
    <p className="mt-0.5 text-[12px] text-subtle-foreground">
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
}: {
  reports: Note[];
  notebookTitle: Map<string, string>;
  notebookColor: Map<string, string>;
  fallbackColor: string;
  onOpen: (note: Note) => void;
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

  return (
    <>
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-6">
        <span className="text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">
          Latest reports
        </span>
        {unreadCount > 0 && (
          <>
            <span className="rounded-full bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium tabular-nums text-citation">
              {unreadCount} unread
            </span>
            <button
              type="button"
              onClick={() => markRead(reports.filter(isUnread).map((note) => note.id))}
              className="ml-auto text-[11px] text-muted-foreground transition-colors hover:text-foreground"
            >
              Mark all read
            </button>
          </>
        )}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {unread.length === 0 && (
          <div className="px-6 py-6 text-center text-[12px] text-subtle-foreground">
            You’re all caught up.
          </div>
        )}
        {[...unread, ...visibleRead].map((note) => (
          <ReportCard
            key={note.id}
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
      <div className="flex items-center gap-1.5 text-[11px] text-subtle-foreground">
        <span
          className="inline-flex h-2 w-2 shrink-0 rounded-full"
          style={{ backgroundColor: color }}
          aria-hidden="true"
        />
        <span className="truncate">{notebook}</span>
        <span>·</span>
        <span className="shrink-0">{relativeTime(note.updatedAt)}</span>
        {unread && (
          <span className="ml-auto shrink-0 rounded-full bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium text-citation">
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
        <h3 className="text-[15px] font-semibold text-foreground hover:underline">
          {note.title}
        </h3>
      </button>
      <div className="mt-2 text-[13px] leading-relaxed">
        <Markdown>{note.content}</Markdown>
      </div>
      <div ref={endRef} aria-hidden="true" />
    </article>
  );
}
