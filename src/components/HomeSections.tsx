import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type {
  NightShiftStatus,
  Note,
  ReportSchedule,
  SourceEvent,
} from "@/lib/types";
import { AudioPlayer } from "./AudioNote";
import { Markdown } from "./Markdown";
import { Badge, Button, EmptyState, Spinner } from "./ui";
import { cn, relativeTime } from "@/lib/utils";
import { intervalLabel } from "./Reports";
import { BookOpen, Clock, Moon, Pause, Play, Power, Zap } from "lucide-react";

/* The Home dashboard's Brief and Staff sections (RFC-v12-steward, UI §2).
 * Notebooks stays the default section and is untouched; these two render in
 * its place when the title-bar switch selects them. Registry joins when its
 * pillar exists — no placeholder tab. */

/** The arrival point: current brief(s) with the audio edition, full-width. */
export function HomeBrief({
  briefs,
  schedules,
  onRan,
}: {
  /** Report-kind notes from the Briefs notebook, newest first. */
  briefs: Note[];
  schedules: ReportSchedule[];
  /** Called after Run now completes so the parent refreshes activity. */
  onRan: () => void;
}) {
  const markNotesRead = useStore((s) => s.markNotesRead);
  const pushToast = useStore((s) => s.pushToast);
  const [running, setRunning] = useState(false);
  const briefSchedule = schedules.find((s) => s.kind === "brief");

  // Reading the section is reading the brief.
  useEffect(() => {
    if (briefs.length > 0) markNotesRead(briefs.map((b) => b.id));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [briefs.map((b) => b.id).join(",")]);

  async function runNow() {
    if (!briefSchedule) return;
    setRunning(true);
    try {
      await api.runReport(briefSchedule.id);
      onRan();
    } catch (e) {
      pushToast("error", e instanceof Error ? e.message : String(e));
    } finally {
      setRunning(false);
    }
  }

  return (
    <div className="mx-auto w-full max-w-3xl px-8 py-8">
      <div className="flex items-center gap-3">
        <h1 className="text-title font-semibold tracking-tight">Brief</h1>
        {briefSchedule && (
          <Button
            variant="secondary"
            size="sm"
            className="ml-auto"
            loading={running}
            onClick={() => void runNow()}
          >
            <Play className="h-3.5 w-3.5" />
            Run now
          </Button>
        )}
      </div>
      {briefs.length === 0 ? (
        <div className="mt-10">
          <EmptyState
            title="No brief yet"
            hint={
              briefSchedule
                ? "The Morning Brief runs on its schedule — or run it now to see today's rundown."
                : "Schedule a brief (kind “brief”) and one document each morning covers every notebook."
            }
          />
        </div>
      ) : (
        briefs.map((brief) => (
          <article key={brief.id} className="mt-6">
            <div className="flex items-center gap-2 text-micro text-subtle-foreground">
              <span className="font-medium text-foreground">{brief.title}</span>
              <span>·</span>
              <span>{relativeTime(brief.updatedAt)}</span>
            </div>
            <div className="prose-compact mt-3 flex flex-col gap-3">
              <AudioPlayer noteId={brief.id} title={brief.title} />
              <Markdown>{brief.content}</Markdown>
            </div>
          </article>
        ))
      )}
    </div>
  );
}

/** The Night Shift's own ledger: what ran, what's scheduled, what the
 *  watchers saw — human-readable output, not a process monitor. */
export function HomeStaff({
  schedules,
  reports,
  notebookTitle,
  notebookColor,
  onOpenNote,
  onRan,
}: {
  schedules: ReportSchedule[];
  reports: Note[];
  notebookTitle: Map<string, string>;
  notebookColor: Map<string, string>;
  onOpenNote: (note: Note) => void;
  onRan: () => void;
}) {
  const pushToast = useStore((s) => s.pushToast);
  const [status, setStatus] = useState<NightShiftStatus | null>(null);
  const [events, setEvents] = useState<SourceEvent[] | null>(null);
  const [runningId, setRunningId] = useState<string | null>(null);

  useEffect(() => {
    void api.nightShiftStatus().then(setStatus).catch(() => {});
    void api
      .listSourceEvents(24)
      .then(setEvents)
      .catch(() => setEvents([]));
  }, []);

  async function togglePause() {
    try {
      const paused = await api.toggleNightShiftPause();
      setStatus((s) => (s ? { ...s, paused } : s));
    } catch (e) {
      pushToast("error", e instanceof Error ? e.message : String(e));
    }
  }

  async function runNow(schedule: ReportSchedule) {
    setRunningId(schedule.id);
    try {
      await api.runReport(schedule.id);
      onRan();
    } catch (e) {
      pushToast("error", e instanceof Error ? e.message : String(e));
    } finally {
      setRunningId(null);
    }
  }

  const statusLabel = !status
    ? "…"
    : !status.backgroundEnabled
      ? "Night Shift is off"
      : status.paused
        ? "Paused until morning"
        : "Night Shift on";
  const dot = !status
    ? "bg-muted-foreground/40"
    : !status.backgroundEnabled
      ? "bg-muted-foreground/40"
      : status.paused
        ? "bg-warning"
        : "bg-success";

  return (
    <div className="mx-auto w-full max-w-3xl px-8 py-8">
      <div className="flex items-center gap-2.5">
        <h1 className="text-title font-semibold tracking-tight">Staff</h1>
        <span className={cn("ml-2 h-2 w-2 rounded-full", dot)} aria-hidden />
        <span className="text-caption text-muted-foreground">{statusLabel}</span>
        <span className="ml-auto">
          {status &&
            (status.backgroundEnabled ? (
              <Button variant="secondary" size="sm" onClick={() => void togglePause()}>
                {status.paused ? (
                  <Play className="h-3.5 w-3.5" />
                ) : (
                  <Pause className="h-3.5 w-3.5" />
                )}
                {status.paused ? "Resume" : "Pause until morning"}
              </Button>
            ) : (
              <Button
                variant="secondary"
                size="sm"
                onClick={() => useStore.getState().openSettings("general")}
              >
                Turn on in Settings
              </Button>
            ))}
        </span>
      </div>

      <StaffGroup title="Last runs">
        {reports.length === 0 ? (
          <StaffQuiet>No scheduled work has run yet.</StaffQuiet>
        ) : (
          reports.slice(0, 6).map((n) => (
            <button
              key={n.id}
              type="button"
              onClick={() => onOpenNote(n)}
              className="flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-surface-2"
            >
              <span
                className="h-2 w-2 shrink-0 rounded-full"
                style={{ backgroundColor: notebookColor.get(n.notebookId) }}
                aria-hidden
              />
              <span className="truncate text-body text-foreground">{n.title}</span>
              <Badge className="shrink-0 gap-1">
                <BookOpen className="h-2.5 w-2.5" />
                <span className="max-w-[140px] truncate">
                  {notebookTitle.get(n.notebookId) ?? "Unknown"}
                </span>
              </Badge>
              <span className="ml-auto shrink-0 text-micro text-subtle-foreground">
                {relativeTime(n.updatedAt)}
              </span>
            </button>
          ))
        )}
      </StaffGroup>

      <StaffGroup title="Scheduled">
        {schedules.length === 0 ? (
          <StaffQuiet>Nothing scheduled.</StaffQuiet>
        ) : (
          [...schedules]
            .sort((a, b) => Number(b.enabled) - Number(a.enabled) || b.lastRunAt - a.lastRunAt)
            .map((r) => (
              <div
                key={r.id}
                className="group flex items-center gap-2.5 rounded-md px-2 py-1.5 transition-colors hover:bg-surface-2"
              >
                <Power
                  className={cn(
                    "h-3.5 w-3.5 shrink-0",
                    r.enabled ? "text-success" : "text-subtle-foreground",
                  )}
                />
                <span className="truncate text-body text-foreground">{r.name}</span>
                <Badge className="shrink-0 gap-1">
                  <BookOpen className="h-2.5 w-2.5" />
                  <span className="max-w-[140px] truncate">
                    {notebookTitle.get(r.notebookId) ?? "Unknown"}
                  </span>
                </Badge>
                <span className="ml-auto flex shrink-0 items-center gap-1 text-micro text-subtle-foreground">
                  {r.trigger === "change" ? (
                    <Zap className="h-2.5 w-2.5" />
                  ) : (
                    <Clock className="h-2.5 w-2.5" />
                  )}
                  {r.trigger === "change"
                    ? `on change · at most ${intervalLabel(r.intervalSecs).toLowerCase()}`
                    : intervalLabel(r.intervalSecs)}
                  {r.lastRunAt > 0 && ` · last ${relativeTime(r.lastRunAt)}`}
                </span>
                <button
                  type="button"
                  className="hidden rounded p-1 text-muted-foreground hover:text-foreground group-hover:block"
                  onClick={() => void runNow(r)}
                  disabled={runningId !== null}
                  title="Run now"
                  aria-label={`Run "${r.name}" now`}
                >
                  {runningId === r.id ? (
                    <Spinner className="h-3.5 w-3.5" />
                  ) : (
                    <Play className="h-3.5 w-3.5" />
                  )}
                </button>
              </div>
            ))
        )}
      </StaffGroup>

      <StaffGroup title="Watchers · last 24 hours">
        {events === null ? (
          <StaffQuiet>Loading…</StaffQuiet>
        ) : events.length === 0 ? (
          <StaffQuiet>
            No source changes observed. Watched folders, pages, and Mac items
            report here when they move.
          </StaffQuiet>
        ) : (
          events.slice(0, 12).map((event) => (
            <div key={event.id} className="rounded-md px-2 py-1.5">
              <div className="flex items-center gap-2 text-body">
                <Moon className="h-3 w-3 shrink-0 text-subtle-foreground" />
                <span className="truncate text-foreground">
                  {event.sourceTitle}
                </span>
                <span className="truncate text-micro text-subtle-foreground">
                  {event.detail}
                </span>
                <span className="ml-auto shrink-0 text-micro text-subtle-foreground">
                  {relativeTime(event.at)}
                </span>
              </div>
              {event.diff && (
                <details className="mt-1 pl-5">
                  <summary className="cursor-pointer text-micro text-subtle-foreground hover:text-foreground">
                    diff
                  </summary>
                  <pre className="mt-1 overflow-x-auto rounded bg-surface-2 p-2 text-micro leading-relaxed text-muted-foreground">
                    {event.diff}
                  </pre>
                </details>
              )}
            </div>
          ))
        )}
      </StaffGroup>
    </div>
  );
}

function StaffGroup({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mt-8">
      <div className="mb-2 text-micro font-medium uppercase tracking-wide text-subtle-foreground">
        {title}
      </div>
      <div className="flex flex-col gap-0.5">{children}</div>
    </section>
  );
}

function StaffQuiet({ children }: { children: React.ReactNode }) {
  return (
    <div className="px-2 py-3 text-caption text-subtle-foreground">{children}</div>
  );
}
