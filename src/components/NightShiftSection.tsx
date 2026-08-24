/* The Night Shift (docs/RFC-night-shift-area.md): Home's third center column.

   Notebooks organize the corpus by document, the Registry by thing, and this
   by time — the work you have decided should happen without you. Three views:
   Tonight is where jobs get commissioned, Standing orders is where recurring
   work is authored, and The record is the morning after.

   Deliberately not a dashboard: flat rows, no charts, and nothing updates
   while you watch. You look at this before bed and again over coffee. */
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type { Notebook, ReportSchedule, RunReceipt } from "@/lib/types";
import { Badge, Button, EmptyState } from "./ui";
import { cn, relativeTime } from "@/lib/utils";
import { Moon, Clock, Receipt } from "lucide-react";

type View = "tonight" | "orders" | "record";

/** The one sentence that states the limit, repeated verbatim wherever work
 *  gets commissioned (WRITING.md: safety reassurances repeat word-for-word). */
const BOUNDARY = "Night Shift writes notes and reports. It will not act outward.";

export function NightShiftSection() {
  const [view, setView] = useState<View>("tonight");
  const [schedules, setSchedules] = useState<ReportSchedule[]>([]);
  const [receipts, setReceipts] = useState<RunReceipt[]>([]);
  const notebooks = useStore((s) => s.notebooks);

  const refresh = useCallback(async () => {
    const [plan, record] = await Promise.all([
      api.tonightPlan().catch(() => [] as ReportSchedule[]),
      api.listReceipts(24 * 14, 200).catch(() => [] as RunReceipt[]),
    ]);
    setSchedules(plan);
    setReceipts(record);
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const nbTitle = useCallback(
    (id: string) => notebooks.find((n: Notebook) => n.id === id)?.title ?? "",
    [notebooks],
  );

  return (
    <div className="relative min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto w-full max-w-[960px] px-6 pb-10">
        <ViewTabs view={view} onChange={setView} />
        {view === "tonight" && (
          <TonightView
            schedules={schedules}
            nbTitle={nbTitle}
            onChanged={refresh}
          />
        )}
        {view === "orders" && (
          <OrdersView schedules={schedules} nbTitle={nbTitle} />
        )}
        {view === "record" && <RecordView receipts={receipts} />}
      </div>
    </div>
  );
}

function ViewTabs({
  view,
  onChange,
}: {
  view: View;
  onChange: (v: View) => void;
}) {
  const tabs = [
    { id: "tonight" as const, label: "Tonight", icon: Moon },
    { id: "orders" as const, label: "Standing orders", icon: Clock },
    { id: "record" as const, label: "The record", icon: Receipt },
  ];
  return (
    <div className="flex items-center gap-0.5 rounded-lg border border-border p-0.5 self-start w-fit">
      {tabs.map(({ id, label, icon: Icon }) => (
        <button
          key={id}
          type="button"
          onClick={() => onChange(id)}
          aria-pressed={view === id}
          className={cn(
            "flex items-center gap-1.5 rounded-md px-2.5 py-1 text-body transition-colors",
            view === id
              ? "bg-surface-2 text-foreground"
              : "text-subtle-foreground hover:text-foreground",
          )}
        >
          <Icon className="size-3.5" />
          {label}
        </button>
      ))}
    </div>
  );
}

/** Tonight: what is queued, and the composer that queues more. */
function TonightView({
  schedules,
  nbTitle,
  onChanged,
}: {
  schedules: ReportSchedule[];
  nbTitle: (id: string) => string;
  onChanged: () => void;
}) {
  const commissions = schedules.filter(
    (s) => s.trigger === "once" && s.lastRunAt === 0,
  );
  const recurring = schedules.filter((s) => s.trigger !== "once");

  return (
    <div className="flex flex-col gap-6 pt-5">
      <Group title="Commissioned" count={commissions.length}>
        {commissions.length === 0 ? (
          <Quiet>
            Nothing commissioned. Hand the night a job below, or ask in chat.
          </Quiet>
        ) : (
          commissions.map((s) => (
            <Row
              key={s.id}
              title={s.name}
              chip="COMMISSION"
              sub={
                s.prompt ||
                `${s.kind} · ${nbTitle(s.notebookId) || "this notebook"}`
              }
              right={
                s.notBefore > 0
                  ? new Date(s.notBefore).toLocaleTimeString(undefined, {
                      hour: "numeric",
                      minute: "2-digit",
                    })
                  : "Next pass"
              }
            />
          ))
        )}
      </Group>

      <Group title="Due on schedule" count={recurring.length}>
        {recurring.length === 0 ? (
          <Quiet>No recurring work scheduled.</Quiet>
        ) : (
          recurring.map((s) => (
            <Row
              key={s.id}
              title={s.name}
              // Work whose hour has already passed says so here rather than
              // looking simply "not yet run" — the Mac was asleep, and it
              // will run on the next pass.
              late={overdueLabel(s)}
              sub={`${nbTitle(s.notebookId) || "Cross-notebook"} · ${cadence(s)}`}
              right={
                s.lastRunAt > 0 ? `ran ${relativeTime(s.lastRunAt)}` : "not yet run"
              }
            />
          ))
        )}
      </Group>

      <Composer onQueued={onChanged} />
    </div>
  );
}

/** The commissioning box. Deliberately the same shape as the ask box: it is
 *  the chat tool router with a night-shift bias, one parser and two mouths
 *  (docs/RFC-night-shift-area.md §4). */
function Composer({ onQueued }: { onQueued: () => void }) {
  const notebooks = useStore((s) => s.notebooks);
  const pushToast = useStore((s) => s.pushToast);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const target = notebooks[0];

  async function queue() {
    const prompt = text.trim();
    if (!prompt || !target) return;
    setBusy(true);
    try {
      // The first line is the job's name; the whole text is the instruction.
      const name = prompt.split("\n")[0].slice(0, 60);
      await api.commissionRun(target.id, name, "custom", prompt, "tonight");
      setText("");
      pushToast("success", "Commissioned. It starts at 2:00 AM.");
      onQueued();
    } catch (err) {
      pushToast("error", `Couldn't queue that: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-micro text-subtle-foreground">{BOUNDARY}</span>
      <div className="flex items-center gap-2">
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void queue();
            }
          }}
          placeholder="Commission overnight work…"
          className="h-10 flex-1 rounded-lg border border-input bg-surface px-3 text-body text-foreground placeholder:text-subtle-foreground focus:outline-none focus:ring-1 focus:ring-ring"
        />
        <Button
          variant="primary"
          size="sm"
          loading={busy}
          disabled={!text.trim() || !target}
          onClick={() => void queue()}
        >
          Commission
        </Button>
      </div>
    </div>
  );
}

/** Standing orders: recurring work as objects, grouped by what they are. */
function OrdersView({
  schedules,
  nbTitle,
}: {
  schedules: ReportSchedule[];
  nbTitle: (id: string) => string;
}) {
  const groups = useMemo(() => {
    const recurring = schedules.filter((s) => s.trigger !== "once");
    return [
      {
        title: "Reports",
        items: recurring.filter((s) => s.trigger === "interval"),
      },
      {
        title: "Standing questions",
        items: recurring.filter((s) => s.trigger === "change"),
      },
    ];
  }, [schedules]);

  if (schedules.filter((s) => s.trigger !== "once").length === 0) {
    return (
      <div className="pt-10">
        <EmptyState
          title="No standing orders"
          hint="Schedule a report in a notebook's Studio, or ask in chat — they all show up here."
        />
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-6 pt-5">
      {groups.map(
        (g) =>
          g.items.length > 0 && (
            <Group key={g.title} title={g.title} count={g.items.length}>
              {g.items.map((s) => (
                <OrderRow key={s.id} order={s} nbTitle={nbTitle} />
              ))}
            </Group>
          ),
      )}
    </div>
  );
}

/** One standing order, expanding to its own run history — the rail's job in
 *  the mock, folded inline so the column stays one readable list. */
function OrderRow({
  order,
  nbTitle,
}: {
  order: ReportSchedule;
  nbTitle: (id: string) => string;
}) {
  const [runs, setRuns] = useState<RunReceipt[] | null>(null);
  const [open, setOpen] = useState(false);

  async function toggle() {
    const next = !open;
    setOpen(next);
    if (next && runs === null) {
      setRuns(await api.receiptsForSchedule(order.id, 5).catch(() => []));
    }
  }

  return (
    <div className="border-b border-border last:border-b-0">
      <button
        type="button"
        onClick={() => void toggle()}
        aria-expanded={open}
        className="flex w-full items-baseline justify-between gap-3 py-2.5 text-left"
      >
        <span className="flex items-baseline gap-2">
          <span className="text-body text-foreground">{order.name}</span>
          {!order.enabled && <Badge>Paused</Badge>}
        </span>
        <span className="text-micro text-subtle-foreground">
          {nbTitle(order.notebookId) || "Cross-notebook"} · {cadence(order)}
        </span>
      </button>
      {open && (
        <div className="pb-3 pl-3">
          {runs === null ? (
            <Quiet>Loading…</Quiet>
          ) : runs.length === 0 ? (
            <Quiet>No runs recorded yet.</Quiet>
          ) : (
            runs.map((r) => (
              <div
                key={r.id}
                className="flex items-baseline justify-between gap-3 py-1"
              >
                <span className="text-micro text-subtle-foreground">
                  {new Date(r.endedAt).toLocaleString(undefined, {
                    weekday: "short",
                    hour: "numeric",
                    minute: "2-digit",
                  })}
                </span>
                <span className="text-micro text-subtle-foreground">
                  {r.status === "ok" ? r.detail || "ok" : `failed: ${r.error}`}
                </span>
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}

/** The record: receipts grouped by the night they belong to. */
function RecordView({ receipts }: { receipts: RunReceipt[] }) {
  const nights = useMemo(() => {
    const byDay = new Map<string, RunReceipt[]>();
    for (const r of receipts) {
      const key = new Date(r.endedAt).toLocaleDateString(undefined, {
        weekday: "long",
        month: "short",
        day: "numeric",
      });
      const list = byDay.get(key);
      if (list) list.push(r);
      else byDay.set(key, [r]);
    }
    return [...byDay.entries()];
  }, [receipts]);

  if (receipts.length === 0) {
    return (
      <div className="pt-10">
        <EmptyState
          title="Nothing has run yet"
          hint="Every Night Shift run leaves a receipt here: what it read, what it wrote, and what it cost."
        />
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-6 pt-5">
      {nights.map(([day, runs]) => (
        <Group
          key={day}
          title={day}
          count={runs.length}
          aside={costLine(runs)}
        >
          {runs.map((r) => (
            <Row
              key={r.id}
              title={r.name}
              chip={r.trigger === "once" ? "COMMISSION" : undefined}
              failed={r.status === "failed"}
              late={lateLabel(r)}
              sub={r.status === "ok" ? r.detail : r.error}
              right={
                r.costMicros > 0
                  ? `$${(r.costMicros / 1_000_000).toFixed(2)}`
                  : r.provider === "local" || r.provider === "ollama"
                    ? "local"
                    : ""
              }
            />
          ))}
        </Group>
      ))}
    </div>
  );
}

/** An interval order whose next turn is already behind us. Shown on
 *  Tonight so a schedule that slept through its hour reads as "overdue,
 *  running shortly" rather than as broken. */
function overdueLabel(s: ReportSchedule): string | undefined {
  if (s.trigger !== "interval" || s.lastRunAt === 0) return undefined;
  const due = s.lastRunAt + s.intervalSecs * 1000;
  const ms = Date.now() - due;
  if (ms < 15 * 60 * 1000) return undefined;
  const hours = Math.round(ms / 3600000);
  return hours < 1 ? "overdue" : `overdue by ${hours}h`;
}

/** "3h late", or nothing when the run was on time or its due time predates
 *  the recording of one. Mirrors the Rust threshold: under a quarter hour is
 *  the pass interval doing its job, not news. */
function lateLabel(r: RunReceipt): string | undefined {
  if (!r.dueAt) return undefined;
  const ms = r.startedAt - r.dueAt;
  if (ms < 15 * 60 * 1000) return undefined;
  const minutes = Math.round(ms / 60000);
  if (minutes < 90) return `${minutes}m late`;
  const hours = Math.round(minutes / 60);
  if (hours < 36) return `${hours}h late`;
  return `${Math.round(hours / 24)}d late`;
}

/** Total for one night, stated only when something was actually metered —
 *  "$0.00" on an all-local night reads like a measurement of nothing. */
function costLine(runs: RunReceipt[]): string {
  const micros = runs.reduce((sum, r) => sum + r.costMicros, 0);
  if (micros === 0) return "all local";
  return `$${(micros / 1_000_000).toFixed(2)}`;
}

function cadence(s: ReportSchedule): string {
  if (s.trigger === "change") return "on change";
  const hours = s.intervalSecs / 3600;
  if (hours >= 168) return "weekly";
  if (hours >= 24) return "daily";
  if (hours >= 1) return `every ${Math.round(hours)}h`;
  return `every ${Math.round(s.intervalSecs / 60)}m`;
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
        {aside && (
          <span className="text-micro text-subtle-foreground">{aside}</span>
        )}
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
  failed,
  late,
}: {
  title: string;
  sub?: string;
  chip?: string;
  right?: string;
  failed?: boolean;
  /** "3h late" — shown as its own chip so the delay is legible at a glance
   *  without colour carrying the meaning. */
  late?: string;
}) {
  return (
    <div className="flex items-baseline justify-between gap-3 border-b border-border py-2.5 last:border-b-0">
      <div className="min-w-0 flex flex-col gap-0.5">
        <span className="flex items-baseline gap-2">
          <span className="text-body text-foreground">{title}</span>
          {chip && <Badge>{chip}</Badge>}
          {late && <Badge>{late}</Badge>}
          {failed && <Badge className="text-destructive">Failed</Badge>}
        </span>
        {sub && (
          <span className="truncate text-micro text-subtle-foreground">
            {sub}
          </span>
        )}
      </div>
      {right && (
        <span className="shrink-0 text-micro text-subtle-foreground">
          {right}
        </span>
      )}
    </div>
  );
}

function Quiet({ children }: { children: React.ReactNode }) {
  return (
    <p className="py-2 text-micro text-subtle-foreground">{children}</p>
  );
}
