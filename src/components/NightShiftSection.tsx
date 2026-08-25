/* The Night Shift (docs/RFC-night-shift-area.md): Home's third center column.

   One screen, not three. An earlier build had Tonight / Standing orders /
   The record — but two of those restated the Staff sidebar, and a browsable
   feed of "Nightly snapshot · 101 MB" is operator telemetry nobody reads.
   What survives is the exchange: what you hand over, and what came back.

   Order is by what the user can act on, not by what the machine did:
   blocked work first (the only actionable rows), then the composer that
   makes the capability legible, then results as openable notes. Watchers
   and filings sit at the foot, compressed — ambient, not a dashboard. */
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type { Note, PlannedRun, RunReceipt, SourceEvent } from "@/lib/types";
import { Badge, Button, EmptyState } from "./ui";
import { relativeTime } from "@/lib/utils";

/** The one sentence that states the limit, repeated verbatim wherever work
 *  gets commissioned (WRITING.md: safety reassurances repeat word-for-word). */
const BOUNDARY = "Night Shift writes notes and reports. It will not act outward.";

/** Presets are what make the capability legible. A blank box does not teach
 *  anyone that their Mac can re-read forty sources or re-verify a draft while
 *  they sleep; four named jobs do. Each is a normal commission underneath. */
const PRESETS: { label: string; kind: string; prompt: string; hint: string }[] = [
  {
    label: "Deep read a notebook",
    kind: "custom",
    prompt:
      "Read every source in this notebook closely and rebuild the summary, " +
      "noting what changed since the last time.",
    hint: "Every source, properly, with a since-last-time delta.",
  },
  {
    label: "Re-verify a draft",
    kind: "custom",
    prompt:
      "Check each substantive claim in the most recent draft against the " +
      "sources, using fresh retrieval rather than its own citations.",
    hint: "Every claim re-checked against the sources.",
  },
  {
    label: "Re-summarize everything",
    kind: "custom",
    prompt:
      "Re-distill every source in this notebook so cross-notebook questions " +
      "find them accurately.",
    hint: "Slow at your desk. Done by morning.",
  },
];

export function NightShiftSection({
  onOpenNote,
}: {
  /** Opening a note means switching to its notebook, which only Home knows
   *  how to do — same handler the reports feed uses. */
  onOpenNote: (note: Note) => void;
}) {
  const [plan, setPlan] = useState<PlannedRun[]>([]);
  const [receipts, setReceipts] = useState<RunReceipt[]>([]);
  const [results, setResults] = useState<Note[]>([]);
  const [events, setEvents] = useState<SourceEvent[]>([]);

  const refresh = useCallback(async () => {
    const [p, r, notes, ev] = await Promise.all([
      api.tonightPlan().catch(() => [] as PlannedRun[]),
      api.listReceipts(24 * 7, 200).catch(() => [] as RunReceipt[]),
      api.listRecentReports(12).catch(() => [] as Note[]),
      api.listSourceEvents(24).catch(() => [] as SourceEvent[]),
    ]);
    setPlan(p);
    setReceipts(r);
    setResults(notes);
    setEvents(ev);
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Anything the user can act on: work that will not run, and runs that
  // failed. Everything else on this screen is information.
  const blocked = plan.filter((p) => p.state === "blocked");
  const failed = receipts.filter((r) => r.status === "failed");
  const needsYou = blocked.length + failed.length;

  return (
    <div className="relative min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto flex w-full max-w-[960px] flex-col gap-7 px-6 pb-12">
        {needsYou > 0 && (
          <NeedsYou blocked={blocked} failed={failed} onChanged={refresh} />
        )}
        <Tonight plan={plan} onChanged={refresh} />
        <CameBack results={results} receipts={receipts} onOpenNote={onOpenNote} />
        <Watching events={events} />
      </div>
    </div>
  );
}

/** Blocked and failed work, each with the reason and the fix. This is the
 *  part of the screen that justifies a visit — it is invisible everywhere
 *  else in the app. */
function NeedsYou({
  blocked,
  failed,
  onChanged,
}: {
  blocked: PlannedRun[];
  failed: RunReceipt[];
  onChanged: () => void;
}) {
  const pushToast = useStore((s) => s.pushToast);
  return (
    <Group title="Needs you" count={blocked.length + failed.length}>
      {blocked.map((p) => (
        <div
          key={p.schedule.id}
          className="flex items-baseline justify-between gap-4 border-b border-border py-2.5 last:border-b-0"
        >
          <div className="min-w-0">
            <div className="flex items-baseline gap-2">
              <span className="text-body text-foreground">{p.schedule.name}</span>
              <Badge>Not running</Badge>
            </div>
            <p className="mt-0.5 text-micro leading-relaxed text-subtle-foreground">
              {p.reason}
            </p>
          </div>
        </div>
      ))}
      {failed.map((r) => (
        <div
          key={r.id}
          className="flex items-baseline justify-between gap-4 border-b border-border py-2.5 last:border-b-0"
        >
          <div className="min-w-0">
            <div className="flex items-baseline gap-2">
              <span className="text-body text-foreground">{r.name}</span>
              <Badge className="text-destructive">Failed</Badge>
            </div>
            <p className="mt-0.5 text-micro leading-relaxed text-subtle-foreground">
              {r.error || "The run did not finish."} It will try again on its
              next turn.
            </p>
          </div>
          {r.scheduleId && (
            <Button
              variant="secondary"
              size="sm"
              onClick={async () => {
                try {
                  await api.runReport(r.scheduleId);
                  pushToast("success", `Running ${r.name} now.`);
                  onChanged();
                } catch (err) {
                  pushToast("error", String(err));
                }
              }}
            >
              Run now
            </Button>
          )}
        </div>
      ))}
    </Group>
  );
}

/** What you hand over: the presets and the box, then what is already queued. */
function Tonight({
  plan,
  onChanged,
}: {
  plan: PlannedRun[];
  onChanged: () => void;
}) {
  const notebooks = useStore((s) => s.notebooks);
  const pushToast = useStore((s) => s.pushToast);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);

  // Commissions run in a notebook; the most recently touched one is the
  // right default, and it is named on the button so the choice is visible.
  const target = notebooks[0];

  const queued = plan.filter(
    (p) => p.schedule.trigger === "once" && p.schedule.lastRunAt === 0,
  );
  const tonight = plan.filter(
    (p) => p.schedule.trigger !== "once" && (p.state === "due" || p.state === "waiting"),
  );

  async function commission(name: string, kind: string, prompt: string) {
    if (!target) {
      pushToast("error", "Create a notebook first.");
      return;
    }
    setBusy(true);
    try {
      await api.commissionRun(target.id, name, kind, prompt, "tonight");
      setText("");
      pushToast("success", "Commissioned. It starts at 2:00 AM.");
      onChanged();
    } catch (err) {
      pushToast("error", `Couldn't queue that: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Group
      title="Tonight"
      aside={target ? `in ${target.title}` : undefined}
    >
      <p className="pb-3 pt-1 text-micro leading-relaxed text-subtle-foreground">
        Hand over work that is too slow to wait on. It runs while your Mac is
        awake, and the result waits for you as a note. {BOUNDARY}
      </p>

      <div className="flex flex-wrap gap-2 pb-3">
        {PRESETS.map((p) => (
          <button
            key={p.label}
            type="button"
            disabled={busy || !target}
            title={p.hint}
            onClick={() => void commission(p.label, p.kind, p.prompt)}
            className="rounded-md border border-border px-2.5 py-1 text-micro text-subtle-foreground transition-colors hover:border-input hover:text-foreground disabled:opacity-50"
          >
            {p.label}
          </button>
        ))}
      </div>

      <div className="flex items-center gap-2 pb-4">
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && text.trim()) {
              e.preventDefault();
              void commission(text.trim().slice(0, 60), "custom", text.trim());
            }
          }}
          placeholder="Or describe the job…"
          className="h-9 flex-1 rounded-lg border border-input bg-surface px-3 text-body text-foreground placeholder:text-subtle-foreground focus:outline-none focus:ring-1 focus:ring-ring"
        />
        <Button
          variant="primary"
          size="sm"
          loading={busy}
          disabled={!text.trim() || !target}
          onClick={() => void commission(text.trim().slice(0, 60), "custom", text.trim())}
        >
          Commission
        </Button>
      </div>

      {queued.map((p) => (
        <Row
          key={p.schedule.id}
          title={p.schedule.name}
          chip="Commissioned"
          sub={p.schedule.prompt}
          right={
            p.schedule.notBefore > 0
              ? new Date(p.schedule.notBefore).toLocaleTimeString(undefined, {
                  hour: "numeric",
                  minute: "2-digit",
                })
              : "next pass"
          }
        />
      ))}
      {tonight.map((p) => (
        <Row
          key={p.schedule.id}
          title={p.schedule.name}
          sub={`${p.notebookTitle || "Across notebooks"} · ${cadence(p)}`}
          right={p.state === "due" ? "runs shortly" : whenNext(p.dueAt)}
        />
      ))}
      {queued.length === 0 && tonight.length === 0 && (
        <Quiet>Nothing scheduled for tonight.</Quiet>
      )}
    </Group>
  );
}

/** What came back: the artifacts, openable. Not receipts — the note is the
 *  payoff, and "your deep read is ready" is the sentence worth reading. */
function CameBack({
  results,
  receipts,
  onOpenNote,
}: {
  results: Note[];
  receipts: RunReceipt[];
  onOpenNote: (note: Note) => void;
}) {
  // Cost is stated only when something was actually metered — "$0.00" on an
  // all-local night is a measurement of nothing.
  const micros = receipts.reduce((sum, r) => sum + r.costMicros, 0);
  const spend = micros > 0 ? `$${(micros / 1_000_000).toFixed(2)} this week` : undefined;

  if (results.length === 0) {
    return (
      <Group title="Came back">
        <div className="py-4">
          <EmptyState
            title="Nothing yet"
            hint="Work that finishes overnight appears here as a note you can open."
            compact
          />
        </div>
      </Group>
    );
  }

  return (
    <Group title="Came back" aside={spend}>
      {results.map((n) => (
        <button
          key={n.id}
          type="button"
          onClick={() => onOpenNote(n)}
          className="flex w-full items-baseline justify-between gap-3 border-b border-border py-2.5 text-left last:border-b-0 hover:bg-surface-2/40"
        >
          <span className="truncate text-body text-foreground">{n.title}</span>
          <span className="shrink-0 text-micro text-subtle-foreground">
            {relativeTime(n.updatedAt)}
          </span>
        </button>
      ))}
    </Group>
  );
}

/** Watchers, compressed to one ambient line per source. The Staff sidebar
 *  listed these individually; at the foot of a page they are context, not
 *  content. */
function Watching({ events }: { events: SourceEvent[] }) {
  const changed = useMemo(() => {
    const seen = new Map<string, SourceEvent>();
    for (const e of events) if (!seen.has(e.sourceId)) seen.set(e.sourceId, e);
    return [...seen.values()].slice(0, 6);
  }, [events]);

  if (changed.length === 0) return null;
  return (
    <Group title="Changed in the last day" count={changed.length}>
      {changed.map((e) => (
        <Row key={e.id} title={e.sourceTitle} sub={e.detail} right={relativeTime(e.at)} />
      ))}
    </Group>
  );
}

function cadence(p: PlannedRun): string {
  if (p.schedule.trigger === "change") return "when a source changes";
  const hours = p.schedule.intervalSecs / 3600;
  if (hours >= 168) return "weekly";
  if (hours >= 24) return "daily";
  if (hours >= 1) return `every ${Math.round(hours)}h`;
  return `every ${Math.round(p.schedule.intervalSecs / 60)}m`;
}

function whenNext(dueAt: number): string {
  if (!dueAt) return "";
  const hours = Math.round((dueAt - Date.now()) / 3600000);
  if (hours <= 0) return "runs shortly";
  if (hours < 24) return `in ${hours}h`;
  return `in ${Math.round(hours / 24)}d`;
}

function Group({
  title,
  count,
  aside,
  children,
}: {
  title: string;
  count?: number;
  aside?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col">
      <div className="flex items-baseline justify-between gap-3 border-b border-border pb-1.5">
        <h2 className="text-micro uppercase tracking-wide text-subtle-foreground">
          {title}
          {count !== undefined && count > 0 && ` · ${count}`}
        </h2>
        {aside && <span className="text-micro text-subtle-foreground">{aside}</span>}
      </div>
      <div className="flex flex-col">{children}</div>
    </section>
  );
}

function Row({
  title,
  sub,
  chip,
  right,
}: {
  title: string;
  sub?: string;
  chip?: string;
  right?: string;
}) {
  return (
    <div className="flex items-baseline justify-between gap-3 border-b border-border py-2.5 last:border-b-0">
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="flex items-baseline gap-2">
          <span className="text-body text-foreground">{title}</span>
          {chip && <Badge>{chip}</Badge>}
        </span>
        {sub && <span className="truncate text-micro text-subtle-foreground">{sub}</span>}
      </div>
      {right && <span className="shrink-0 text-micro text-subtle-foreground">{right}</span>}
    </div>
  );
}

function Quiet({ children }: { children: React.ReactNode }) {
  return <p className="py-2 text-micro text-subtle-foreground">{children}</p>;
}
