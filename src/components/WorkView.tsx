import { useCallback, useEffect, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type {
  Note,
  ReportSchedule,
  SourceEvent,
  RunReceipt,
} from "@/lib/types";
import { Button, Spinner } from "./ui";
import { relativeTime } from "@/lib/utils";
import { describe } from "@/lib/errors";
import { intervalLabel } from "./Reports";
/** An overview of existing schedules, receipts and source events. Outputs
 * remain normal notebook notes; this view owns no new content store. */
export function WorkView({
  schedules,
  reports,
  onRefresh,
}: {
  schedules: ReportSchedule[];
  reports: Note[];
  onRefresh: () => void;
}) {
  const [receipts, setReceipts] = useState<RunReceipt[]>([]);
  const [events, setEvents] = useState<SourceEvent[]>([]);
  const [paused, setPaused] = useState(false);
  const [enabled, setEnabled] = useState(true);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [active, setActive] = useState<string[]>([]);
  const [running, setRunning] = useState<string | null>(null);
  const notebooks = useStore((s) => s.notebooks);
  const pushToast = useStore((s) => s.pushToast);
  const load = useCallback(async () => {
    try {
      const [runs, changes, status] = await Promise.all([
        api.workReceipts(),
        api.listSourceEvents(168),
        api.nightShiftStatus(),
      ]);
      setReceipts(runs);
      setEvents(changes);
      setPaused(status.paused);
      setEnabled(status.backgroundEnabled);
      setActive(status.activeScheduleIds ?? []);
      setError("");
    } catch (e) {
      setError(describe(e));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void load();
    const timer = setInterval(() => void load(), 15000);
    return () => clearInterval(timer);
  }, [load]);
  const openNote = (id: string) =>
    void useStore
      .getState()
      .handleIntegrationUrl(`alchemy://note/${encodeURIComponent(id)}`);
  async function run(id: string) {
    setRunning(id);
    try {
      await api.runReport(id);
      onRefresh();
      await load();
    } catch (e) {
      pushToast("error", describe(e));
    } finally {
      await load();
      setRunning(null);
    }
  }
  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-7 pb-8">
      <div className="flex items-center gap-3 border-b border-border py-4 text-caption text-muted-foreground">
        <span>
          {!enabled
            ? "Background work is off"
            : paused
              ? "Background work is paused"
              : "Background work is on"}
        </span>
        <Button
          variant="secondary"
          size="sm"
          disabled={!enabled}
          onClick={async () => {
            try {
              setPaused(await api.toggleNightShiftPause());
            } catch (e) {
              pushToast("error", describe(e));
            }
          }}
        >
          {paused ? "Resume" : "Pause until morning"}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            onRefresh();
            void load();
          }}
        >
          Refresh
        </Button>
      </div>
      {loading && (
        <div className="flex gap-2 py-4" role="status">
          <Spinner /> Loading work…
        </div>
      )}
      {error && (
        <p role="alert" className="py-4 text-caption text-destructive">
          Couldn’t load work: {error}
        </p>
      )}
      <h2 className="pt-5 pb-2 text-caption font-medium">Scheduled</h2>
      {schedules.length === 0 && (
        <p className="py-2 text-caption text-muted-foreground">
          No jobs scheduled. Schedule a report from a notebook.
        </p>
      )}
      {schedules.map((s) => (
        <div
          key={s.id}
          className="flex items-center gap-3 border-b border-border py-3"
        >
          <div className="min-w-0 flex-1">
            <div className="truncate text-body">{s.name}</div>
            <div className="text-caption text-muted-foreground">
              {notebooks.find((n) => n.id === s.notebookId)?.title ??
                "All notebooks"}{" "}
              ·{" "}
              {s.enabled
                ? s.trigger === "once"
                  ? "Once"
                  : s.trigger === "change"
                    ? "When sources change"
                    : intervalLabel(s.intervalSecs)
                : "Disabled"}
            </div>
          </div>
          {running === s.id || active.includes(s.id) ? (
            <>
              <Spinner />
              <Button
                variant="ghost"
                size="sm"
                onClick={async () => {
                  try {
                    await api.cancelWork(s.id);
                    await load();
                  } catch (e) {
                    pushToast("error", describe(e));
                  }
                }}
              >
                Stop
              </Button>
            </>
          ) : (
            <Button
              variant="secondary"
              size="sm"
              disabled={running !== null}
              onClick={() => void run(s.id)}
            >
              Run now
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            onClick={async () => {
              try {
                await api.updateReportSchedule(
                  s.id,
                  s.name,
                  s.kind,
                  s.prompt,
                  s.trigger,
                  s.intervalSecs,
                  !s.enabled,
                  s.watchSources,
                  s.watchKinds,
                );
                onRefresh();
              } catch (e) {
                pushToast("error", describe(e));
              }
            }}
          >
            {s.enabled ? "Disable" : "Enable"}
          </Button>
        </div>
      ))}
      <h2 className="pt-5 pb-2 text-caption font-medium">Recent runs</h2>
      {!loading && !error && receipts.length === 0 && (
        <p className="py-2 text-caption text-muted-foreground">
          No runs recorded in the past week.
        </p>
      )}
      {receipts.map((r) => (
        <div key={r.id} className="border-b border-border py-3">
          <div className="flex items-center gap-3">
            <span className="min-w-0 flex-1 truncate text-body">{r.name}</span>
            <span className="text-caption text-muted-foreground">
              {r.error === "Report stopped" ? "Stopped" : r.status === "failed" ? "Failed" : "Completed"} ·{" "}
              {relativeTime(r.endedAt)}
            </span>
            {r.noteId && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => openNote(r.noteId)}
              >
                Open result
              </Button>
            )}
          </div>
          <p
            className={
              r.error
                ? "text-caption text-destructive"
                : "text-caption text-muted-foreground"
            }
          >
            {r.error ? describe(r.error) : r.detail}
            {r.model ? ` · ${r.provider} / ${r.model}` : ""}
          </p>
        </div>
      ))}
      <h2 className="pt-5 pb-2 text-caption font-medium">Saved results</h2>
      {reports.slice(0, 30).map((n) => (
        <button
          type="button"
          key={n.id}
          onClick={() => openNote(n.id)}
          className="flex w-full items-center gap-3 border-b border-border py-3 text-left hover:text-primary focus-visible:outline focus-visible:outline-ring"
        >
          <span className="min-w-0 flex-1 truncate text-body">{n.title}</span>
          <span className="text-caption text-muted-foreground">
            {relativeTime(n.updatedAt)}
          </span>
        </button>
      ))}
      <h2 className="pt-5 pb-2 text-caption font-medium">
        Source changes · past week
      </h2>
      {events.slice(0, 100).map((e) => (
        <button
          type="button"
          key={e.id}
          onClick={async () => {
            const store = useStore.getState();
            if (e.kind === "growth") {
              await store.selectNotebook(e.notebookId);
              useStore.setState({ growOpen: true, galleryOpen: false });
            } else {
              await store.handleIntegrationUrl(
                `alchemy://${e.kind === "wiki" ? "note" : "source"}/${encodeURIComponent(e.sourceId)}`,
              );
            }
          }}
          className="flex w-full items-center gap-3 border-b border-border py-3 text-left hover:text-primary focus-visible:outline focus-visible:outline-ring"
        >
          <span className="min-w-0 flex-1 truncate text-body">
            {e.sourceTitle}
          </span>
          <span className="text-caption text-muted-foreground">
            {e.kind} · {relativeTime(e.at)}
          </span>
        </button>
      ))}
    </div>
  );
}
