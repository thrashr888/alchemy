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
import { Spinner } from "./ui";
import { cn, relativeTime } from "@/lib/utils";
import { intervalLabel } from "./Reports";
import {
  Clock,
  Moon,
  Newspaper,
  PanelLeft,
  PanelRight,
  Pause,
  Play,
  Power,
  Zap,
} from "lucide-react";

/* Home's Steward sidebars (RFC-v12-steward UI §2, as sidebars rather than
 * pages): Staff on the left mirroring the reports feed's side-card idiom,
 * the Brief as the top-right card above Latest Reports. Registry joins when
 * its pillar exists. */

/** The Brief card: the arrival point, collapsible to its header row. */
export function BriefSidebar({
  open,
  onToggle,
  briefs,
  schedules,
  unread,
  onRan,
}: {
  open: boolean;
  onToggle: () => void;
  /** Report-kind notes from the Briefs notebook, newest first. */
  briefs: Note[];
  schedules: ReportSchedule[];
  unread: boolean;
  onRan: () => void;
}) {
  const markNotesRead = useStore((s) => s.markNotesRead);
  const pushToast = useStore((s) => s.pushToast);
  const [running, setRunning] = useState(false);
  const briefSchedule = schedules.find((s) => s.kind === "brief");
  const brief = briefs[0];

  // Reading the open card is reading the brief.
  useEffect(() => {
    if (open && brief) markNotesRead([brief.id]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, brief?.id, brief?.updatedAt]);

  async function runNow() {
    if (!briefSchedule || running) return;
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
    <section
      className={cn(
        "side-card flex min-h-0 flex-col",
        open && "max-h-[55%] shrink-0",
      )}
    >
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-6">
        <span className="text-caption font-semibold uppercase tracking-wide text-muted-foreground">
          Brief
        </span>
        {unread && (
          <span className="h-1.5 w-1.5 rounded-full bg-primary" aria-label="New brief" />
        )}
        <div className="ml-auto flex items-center gap-1">
          {briefSchedule && (
            <button
              type="button"
              onClick={() => void runNow()}
              title="Run the brief now"
              aria-label="Run the brief now"
              className="rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
            >
              {running ? (
                <Spinner className="h-3.5 w-3.5" />
              ) : (
                <Play className="h-3.5 w-3.5" />
              )}
            </button>
          )}
          <button
            type="button"
            onClick={onToggle}
            title={open ? "Collapse the brief" : "Show the brief"}
            aria-expanded={open}
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
          >
            <PanelRight className="h-4 w-4" />
          </button>
        </div>
      </div>
      {open && (
        <div className="min-h-0 flex-1 overflow-y-auto px-6 py-4">
          {!brief ? (
            <p className="text-caption text-subtle-foreground">
              No brief yet — it runs each morning, or press play above to run
              it now.
            </p>
          ) : (
            <>
              <div className="text-micro text-subtle-foreground">
                {relativeTime(brief.updatedAt)}
              </div>
              <div className="prose-compact mt-2 flex flex-col gap-3">
                <AudioPlayer noteId={brief.id} title={brief.title} />
                <Markdown>{brief.content}</Markdown>
              </div>
            </>
          )}
        </div>
      )}
    </section>
  );
}

/** Staff as a left sidebar: the Night Shift's ledger, side-card idiom. */
export function StaffSidebar({
  schedules,
  reports,
  notebookTitle,
  notebookColor,
  onOpenNote,
  onRan,
  onCollapse,
}: {
  schedules: ReportSchedule[];
  reports: Note[];
  notebookTitle: Map<string, string>;
  notebookColor: Map<string, string>;
  onOpenNote: (note: Note) => void;
  onRan: () => void;
  onCollapse: () => void;
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

  const dot = !status
    ? "bg-muted-foreground/40"
    : !status.backgroundEnabled
      ? "bg-muted-foreground/40"
      : status.paused
        ? "bg-warning"
        : "bg-success";
  const statusLabel = !status
    ? ""
    : !status.backgroundEnabled
      ? "Off"
      : status.paused
        ? "Paused until morning"
        : "On";

  return (
    <>
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-4">
        <Moon className="h-3.5 w-3.5 text-muted-foreground" />
        <span className="text-caption font-semibold uppercase tracking-wide text-muted-foreground">
          Staff
        </span>
        <span className={cn("h-1.5 w-1.5 rounded-full", dot)} title={statusLabel} />
        <div className="ml-auto flex items-center gap-1">
          {status?.backgroundEnabled && (
            <button
              type="button"
              onClick={() => void togglePause()}
              title={status.paused ? "Resume scheduled runs" : "Pause until morning"}
              aria-label={status.paused ? "Resume scheduled runs" : "Pause until morning"}
              className="rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
            >
              {status.paused ? (
                <Play className="h-3.5 w-3.5" />
              ) : (
                <Pause className="h-3.5 w-3.5" />
              )}
            </button>
          )}
          <button
            type="button"
            onClick={onCollapse}
            title="Collapse Staff"
            aria-label="Collapse the Staff sidebar"
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
          >
            <PanelLeft className="h-4 w-4" />
          </button>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {status && !status.backgroundEnabled && (
          <button
            type="button"
            onClick={() => useStore.getState().openSettings("general")}
            className="mb-3 w-full rounded-md border border-border bg-surface px-3 py-2 text-left text-caption text-muted-foreground transition-colors hover:bg-surface-2"
          >
            Night Shift is off — turn it on in Settings to run reports and
            syncs in the background.
          </button>
        )}

        <StaffGroup title="Last runs">
          {reports.length === 0 ? (
            <StaffQuiet>No scheduled work has run yet.</StaffQuiet>
          ) : (
            reports.slice(0, 5).map((n) => (
              <button
                key={n.id}
                type="button"
                onClick={() => onOpenNote(n)}
                title={`Open in "${notebookTitle.get(n.notebookId) ?? "notebook"}"`}
                className="flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left transition-colors hover:bg-surface-2"
              >
                <span
                  className="h-2 w-2 shrink-0 rounded-full"
                  style={{ backgroundColor: notebookColor.get(n.notebookId) }}
                  aria-hidden
                />
                <span className="truncate text-caption text-foreground">
                  {n.title}
                </span>
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
              .sort(
                (a, b) =>
                  Number(b.enabled) - Number(a.enabled) ||
                  b.lastRunAt - a.lastRunAt,
              )
              .map((r) => (
                <div
                  key={r.id}
                  className="group flex items-center gap-2 rounded-md px-1.5 py-1 transition-colors hover:bg-surface-2"
                >
                  <Power
                    className={cn(
                      "h-3 w-3 shrink-0",
                      r.enabled ? "text-success" : "text-subtle-foreground",
                    )}
                  />
                  <span
                    className="min-w-0 truncate text-caption text-foreground"
                    title={`${r.name} — ${notebookTitle.get(r.notebookId) ?? "notebook"}`}
                  >
                    {r.name}
                  </span>
                  <span className="ml-auto flex shrink-0 items-center gap-1 text-micro text-subtle-foreground group-hover:hidden">
                    {r.trigger === "change" ? (
                      <Zap className="h-2.5 w-2.5" />
                    ) : (
                      <Clock className="h-2.5 w-2.5" />
                    )}
                    {intervalLabel(r.intervalSecs)}
                  </span>
                  <button
                    type="button"
                    className="ml-auto hidden shrink-0 rounded p-0.5 text-muted-foreground hover:text-foreground group-hover:block"
                    onClick={() => void runNow(r)}
                    disabled={runningId !== null}
                    title="Run now"
                    aria-label={`Run "${r.name}" now`}
                  >
                    {runningId === r.id ? (
                      <Spinner className="h-3 w-3" />
                    ) : (
                      <Play className="h-3 w-3" />
                    )}
                  </button>
                </div>
              ))
          )}
        </StaffGroup>

        <StaffGroup title="Watchers · 24h">
          {events === null ? (
            <StaffQuiet>Loading…</StaffQuiet>
          ) : events.length === 0 ? (
            <StaffQuiet>No source changes observed.</StaffQuiet>
          ) : (
            events.slice(0, 10).map((event) => (
              <div key={event.id} className="rounded-md px-1.5 py-1">
                <div className="flex items-center gap-1.5">
                  <span className="min-w-0 truncate text-caption text-foreground">
                    {event.sourceTitle}
                  </span>
                  <span className="ml-auto shrink-0 text-micro text-subtle-foreground">
                    {relativeTime(event.at)}
                  </span>
                </div>
                <div className="truncate text-micro text-subtle-foreground">
                  {event.detail}
                </div>
                {event.diff && (
                  <details className="mt-0.5">
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
    </>
  );
}

/** The collapsed rails: one icon that reopens its sidebar. */
export function SidebarRail({
  icon,
  title,
  dot,
  onClick,
}: {
  icon: "staff" | "brief" | "reports";
  title: string;
  dot?: boolean;
  onClick: () => void;
}) {
  const Icon = icon === "staff" ? Moon : Newspaper;
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      aria-label={title}
      className="relative rounded-md p-2 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
    >
      <Icon className="h-4 w-4" />
      {dot && (
        <span className="absolute right-1 top-1 h-1.5 w-1.5 rounded-full bg-primary" />
      )}
    </button>
  );
}

function StaffGroup({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mb-5">
      <div className="mb-1.5 text-micro font-medium uppercase tracking-wide text-subtle-foreground">
        {title}
      </div>
      <div className="flex flex-col gap-0.5">{children}</div>
    </section>
  );
}

function StaffQuiet({ children }: { children: React.ReactNode }) {
  return (
    <div className="px-1.5 py-2 text-micro text-subtle-foreground">{children}</div>
  );
}
