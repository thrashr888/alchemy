import { useState } from "react";
import { useStore } from "@/lib/store";
import { Button, Input, Textarea, Modal, Spinner } from "./ui";
import { cn } from "@/lib/utils";
import { Clock, Plus, Play, Trash2, Power } from "lucide-react";

const INTERVALS = [
  { label: "Hourly", secs: 3600 },
  { label: "Every 6 hours", secs: 21600 },
  { label: "Daily", secs: 86400 },
  { label: "Weekly", secs: 604800 },
];

// Report generators (subset of note kinds that read well as recurring reports).
const KINDS = [
  { value: "briefing", label: "Briefing" },
  { value: "summary", label: "Summary" },
  { value: "timeline", label: "Timeline" },
  { value: "faq", label: "FAQ" },
  { value: "custom", label: "Custom prompt" },
];

function intervalLabel(secs: number): string {
  return INTERVALS.find((i) => i.secs === secs)?.label ?? `${Math.round(secs / 3600)}h`;
}

export function Reports() {
  const schedules = useStore((s) => s.reportSchedules);
  const create = useStore((s) => s.createReport);
  const update = useStore((s) => s.updateReport);
  const remove = useStore((s) => s.deleteReport);
  const runNow = useStore((s) => s.runReportNow);
  const generating = useStore((s) => s.generatingKind === "report");

  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [kind, setKind] = useState("briefing");
  const [prompt, setPrompt] = useState("");
  const [intervalSecs, setIntervalSecs] = useState(86400);

  function openEditor() {
    setName("");
    setKind("briefing");
    setPrompt("");
    setIntervalSecs(86400);
    setEditing(true);
  }

  return (
    <div className="border-t border-border px-4 py-3">
      <div className="mb-2 flex items-center">
        <span className="text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">
          Reports
        </span>
        <Button variant="ghost" size="icon" className="ml-auto" onClick={openEditor} title="Schedule a report">
          <Plus className="h-4 w-4" />
        </Button>
      </div>

      {schedules.length === 0 ? (
        <p className="text-[11.5px] text-subtle-foreground">
          Schedule recurring reports that refresh your URL sources, then generate a timestamped note.
        </p>
      ) : (
        <div className="flex flex-col gap-1">
          {schedules.map((r) => (
            <div key={r.id} className="group flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-surface-2">
              <button
                onClick={() => update({ ...r, enabled: !r.enabled })}
                title={r.enabled ? "Enabled — click to pause" : "Paused — click to enable"}
              >
                <Power className={cn("h-3.5 w-3.5", r.enabled ? "text-success" : "text-subtle-foreground")} />
              </button>
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13px] text-foreground" title={r.name}>
                  {r.name}
                </div>
                <div className="flex items-center gap-1 text-[11px] text-subtle-foreground">
                  <Clock className="h-2.5 w-2.5" />
                  {intervalLabel(r.intervalSecs)}
                  {r.lastRunAt > 0 && <span>· last {new Date(r.lastRunAt).toLocaleDateString()}</span>}
                </div>
              </div>
              <div className="flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100">
                <button
                  className="rounded p-1 text-muted-foreground hover:text-foreground disabled:opacity-50"
                  onClick={() => runNow(r.id)}
                  disabled={generating}
                  title="Run now"
                >
                  {generating ? <Spinner className="h-3 w-3" /> : <Play className="h-3 w-3" />}
                </button>
                <button
                  className="rounded p-1 text-muted-foreground hover:text-destructive"
                  onClick={() => remove(r.id)}
                  title="Delete"
                >
                  <Trash2 className="h-3 w-3" />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      <Modal open={editing} onClose={() => setEditing(false)} title="Schedule a report" width="max-w-md">
        <form
          onSubmit={(e) => {
            e.preventDefault();
            setEditing(false);
            void create(name, kind, kind === "custom" ? prompt : "", intervalSecs);
          }}
          className="flex flex-col gap-3"
        >
          <Field label="Name">
            <Input autoFocus placeholder="e.g. Morning briefing" value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <Field label="Generator">
            <Select value={kind} onChange={setKind} options={KINDS} />
          </Field>
          {kind === "custom" && (
            <Field label="Prompt">
              <Textarea rows={4} placeholder="What should this report cover?" value={prompt} onChange={(e) => setPrompt(e.target.value)} />
            </Field>
          )}
          <Field label="Frequency">
            <Select
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
              Schedule
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-[12px] font-medium text-foreground">{label}</label>
      {children}
    </div>
  );
}

function Select({
  value,
  onChange,
  options,
}: {
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="w-full rounded-md border border-input bg-surface-2 px-2.5 py-1.5 text-[13px] text-foreground outline-none focus:border-primary/60"
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}
