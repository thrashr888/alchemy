import { useMemo, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Button, EmptyState, Input, Textarea, Modal, Spinner } from "./ui";
import { cn, fmtDay } from "@/lib/utils";
import {
  Clock,
  Eye,
  EyeOff,
  FileText,
  Plus,
  Play,
  Trash2,
  Power,
  Pencil,
  Zap,
} from "lucide-react";
import type { Note, ReportSchedule } from "@/lib/types";
import { ARTIFACTS } from "./studioArtifacts";

const INTERVALS = [
  { label: "Hourly", secs: 3600 },
  { label: "Every 6 hours", secs: 21600 },
  { label: "Daily", secs: 86400 },
  { label: "Weekly", secs: 604800 },
];

export function intervalLabel(secs: number): string {
  return INTERVALS.find((i) => i.secs === secs)?.label ?? `${Math.round(secs / 3600)}h`;
}

export function Reports() {
  const schedules = useStore((s) => s.reportSchedules);
  const create = useStore((s) => s.createReport);
  const update = useStore((s) => s.updateReport);
  const remove = useStore((s) => s.deleteReport);
  const runNow = useStore((s) => s.runReportNow);
  const generating = useStore((s) => s.generatingKind === "report");
  const notes = useStore((s) => s.notes);
  const markNotesRead = useStore((s) => s.markNotesRead);

  // Each schedule keeps one living note (collapse_report_notes) titled after
  // itself — that note IS the latest result.
  const latestNote = (r: ReportSchedule): Note | undefined =>
    notes.find(
      (n) =>
        n.kind === "report" &&
        (n.title === r.name || n.title.startsWith(`${r.name} — `)),
    );
  const showLatest = (r: ReportSchedule) => {
    const note = latestNote(r);
    if (!note) return;
    markNotesRead([note.id]);
    void api.noteOpened(note.id).catch(() => {});
    useStore.getState().openInReader({ type: "note", id: note.id });
  };

  const templates = useStore((s) => s.templates);
  // One registry, every surface: any generator or user template schedules as
  // a report (audio stays out — a cron'd podcast isn't a report). Template
  // schedules store "template:<id>" so renames and edits track live.
  const kinds = useMemo(
    () => [
      ...ARTIFACTS.filter((a) => a.kind !== "audio_overview").map((a) => ({
        value: a.kind,
        label: a.label,
      })),
      ...templates.map((t) => ({ value: `template:${t.id}`, label: t.name })),
      { value: "custom", label: "Custom prompt" },
    ],
    [templates],
  );

  const [editing, setEditing] = useState(false);
  // Section hidden/shown — persisted so a notes-heavy workflow keeps its room.
  // Lazy initializer: the non-lazy form re-read localStorage every render.
  const [open, setOpen] = useState(
    () => localStorage.getItem("studioReportsOpen") !== "false",
  );
  const toggleOpen = () => {
    const v = !open;
    localStorage.setItem("studioReportsOpen", String(v));
    setOpen(v);
  };
  // The schedule being edited; null means the modal creates a new one.
  const [editTarget, setEditTarget] = useState<ReportSchedule | null>(null);
  const [name, setName] = useState("");
  const [kind, setKind] = useState("briefing");
  const [prompt, setPrompt] = useState("");
  const [trigger, setTrigger] = useState<"interval" | "change">("interval");
  const [intervalSecs, setIntervalSecs] = useState(86400);

  function openEditor() {
    setEditTarget(null);
    setName("");
    setKind("briefing");
    setPrompt("");
    setTrigger("interval");
    setIntervalSecs(86400);
    setEditing(true);
  }

  function openEdit(r: ReportSchedule) {
    setEditTarget(r);
    setName(r.name);
    setKind(r.kind);
    setPrompt(r.prompt);
    setTrigger(r.trigger === "change" ? "change" : "interval");
    setIntervalSecs(r.intervalSecs);
    setEditing(true);
  }

  return (
    <div className="px-4 py-3">
      <div className="flex items-center gap-2 text-micro font-medium uppercase tracking-wide text-subtle-foreground">
        <span>Reports</span>
        <button
          type="button"
          onClick={toggleOpen}
          className="ml-auto rounded p-0.5 transition-colors hover:text-foreground"
          title={open ? "Hide reports" : "Show reports"}
          aria-label={open ? "Hide reports" : "Show reports"}
          aria-expanded={open}
        >
          {open ? <Eye className="h-3.5 w-3.5" /> : <EyeOff className="h-3.5 w-3.5" />}
        </button>
        <Button
          variant="ghost"
          size="icon"
          onClick={openEditor}
          title="Schedule a report"
          aria-label="Schedule a report"
        >
          <Plus className="h-4 w-4" />
        </Button>
      </div>

      {!open ? null : schedules.length === 0 ? (
        <EmptyState
          compact
          title="No reports scheduled"
          hint="Reports refresh your URL sources on a schedule, then write a timestamped note."
        />
      ) : (
        <div className="mt-2 flex flex-col gap-1">
          {schedules.map((r) => (
            <div key={r.id} className="group flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-surface-2">
              <button
                type="button"
                onClick={() => update({ ...r, enabled: !r.enabled })}
                title={r.enabled ? "Enabled — click to pause" : "Paused — click to enable"}
                aria-label={`${r.enabled ? "Pause" : "Enable"} report "${r.name}"`}
                aria-pressed={r.enabled}
              >
                <Power className={cn("h-3.5 w-3.5", r.enabled ? "text-success" : "text-subtle-foreground")} />
              </button>
              <div className="min-w-0 flex-1">
                <div className="truncate text-body text-foreground" title={r.name}>
                  {r.name}
                </div>
                <div className="flex items-center gap-1 text-micro text-subtle-foreground">
                  {r.trigger === "change" ? (
                    <Zap className="h-2.5 w-2.5" />
                  ) : (
                    <Clock className="h-2.5 w-2.5" />
                  )}
                  {r.trigger === "change"
                    ? `On change · at most ${intervalLabel(r.intervalSecs).toLowerCase()}`
                    : intervalLabel(r.intervalSecs)}
                  {r.lastRunAt > 0 && <span>· last {fmtDay(r.lastRunAt)}</span>}
                </div>
              </div>
              <div className="hidden items-center gap-0.5 group-hover:flex group-focus-within:flex">
                <button
                  type="button"
                  className="rounded p-1 text-muted-foreground hover:text-foreground disabled:opacity-50"
                  onClick={() => showLatest(r)}
                  disabled={!latestNote(r)}
                  title="Open the latest result"
                  aria-label={`Open the latest "${r.name}" note`}
                >
                  <FileText className="h-3.5 w-3.5" />
                </button>
                <button
                  type="button"
                  className="rounded p-1 text-muted-foreground hover:text-foreground disabled:opacity-50"
                  onClick={() => runNow(r.id)}
                  disabled={generating}
                  title="Run now"
                  aria-label={`Run "${r.name}" now`}
                >
                  {generating ? <Spinner className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}
                </button>
                <button
                  type="button"
                  className="rounded p-1 text-muted-foreground hover:text-foreground"
                  onClick={() => openEdit(r)}
                  title="Edit"
                  aria-label={`Edit "${r.name}"`}
                >
                  <Pencil className="h-3.5 w-3.5" />
                </button>
                <button
                  type="button"
                  className="rounded p-1 text-muted-foreground hover:text-destructive"
                  onClick={() => remove(r.id)}
                  title="Delete"
                  aria-label={`Delete "${r.name}"`}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      <Modal
        open={editing}
        onClose={() => setEditing(false)}
        title={editTarget ? "Edit report" : "Schedule a report"}
        width="max-w-md"
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            setEditing(false);
            const p = kind === "custom" ? prompt : "";
            if (editTarget)
              void update({ ...editTarget, name, kind, prompt: p, trigger, intervalSecs });
            else void create(name, kind, p, trigger, intervalSecs);
          }}
          className="flex flex-col gap-3"
        >
          <Field label="Name" htmlFor="report-name">
            <Input id="report-name" name="report-name" autoFocus placeholder="e.g. Morning briefing" value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <Field label="Generator" htmlFor="report-generator">
            <Select id="report-generator" value={kind} onChange={setKind} options={kinds} />
          </Field>
          {kind === "custom" && (
            <Field label="Prompt" htmlFor="report-prompt">
              <Textarea id="report-prompt" name="report-prompt" rows={4} placeholder="What should this report cover?" value={prompt} onChange={(e) => setPrompt(e.target.value)} />
            </Field>
          )}
          <Field label="Runs" htmlFor="report-trigger">
            <Select
              id="report-trigger"
              value={trigger}
              onChange={(v) => setTrigger(v === "change" ? "change" : "interval")}
              options={[
                { value: "interval", label: "On a schedule" },
                { value: "change", label: "When sources change" },
              ]}
            />
          </Field>
          <Field
            label={trigger === "change" ? "At most" : "Frequency"}
            htmlFor="report-frequency"
          >
            <Select
              id="report-frequency"
              value={String(intervalSecs)}
              onChange={(v) => setIntervalSecs(Number(v))}
              options={INTERVALS.map((i) => ({ value: String(i.secs), label: i.label }))}
            />
          </Field>
          <div className="flex justify-end gap-2 pt-1">
            <Button type="button" variant="ghost" onClick={() => setEditing(false)}>
              Cancel
            </Button>
            <Button
              type="submit"
              variant="primary"
              disabled={!name.trim() || (kind === "custom" && !prompt.trim())}
            >
              {editTarget ? "Save" : "Schedule"}
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}

function Field({ label, htmlFor, children }: { label: string; htmlFor: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={htmlFor} className="text-caption font-medium text-foreground">{label}</label>
      {children}
    </div>
  );
}

function Select({
  id,
  value,
  onChange,
  options,
}: {
  id: string;
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <select
      id={id}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="w-full rounded-md border border-input bg-surface-2 py-2 pl-3 pr-9 text-body text-foreground outline-none focus:border-primary/60"
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}
