import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type { LedgerEntry } from "@/lib/types";
import { Button, EmptyState, Input, RowMenu, useConfirm } from "./ui";
import { cn, relativeTime } from "@/lib/utils";
import {
  HelpCircle,
  Info,
  Milestone,
  PenLine,
  Quote,
  Logs,
  ShieldCheck,
  Trash2,
} from "lucide-react";

/* The Ledger (RFC-v12-steward pillar 2): the notebook's typed memory as the
 * third center mode beside Chat and Reader. Dense rows, lifecycle chips,
 * anchors that open the source at the quoted passage. */

const KINDS = [
  { id: "assertion", label: "Assertion", icon: Quote },
  { id: "fact", label: "Fact", icon: ShieldCheck },
  { id: "decision", label: "Decision", icon: Milestone },
  { id: "question", label: "Question", icon: HelpCircle },
  { id: "log", label: "Log", icon: PenLine },
] as const;

type Kind = (typeof KINDS)[number]["id"];

/** Mirror of the Rust lifecycle vocabulary (commands/ledger.rs). */
const STATUSES: Record<Kind, string[]> = {
  assertion: ["asserted", "corroborated", "contradicted", "stale"],
  fact: ["current", "superseded"],
  decision: ["decided", "superseded"],
  question: ["open", "answered"],
  log: ["logged"],
};

/** Chips carry the state color (icon + text, never color alone). */
function statusChip(status: string): string {
  switch (status) {
    case "corroborated":
    case "answered":
      return "border-success/40 text-success";
    case "contradicted":
      return "border-destructive/40 text-destructive";
    case "stale":
    case "superseded":
      return "border-border-strong text-subtle-foreground";
    case "logged":
      return "border-border-strong text-muted-foreground";
    default: // asserted / current / decided / open
      return "border-citation/40 text-citation";
  }
}

export function LedgerPane() {
  const currentId = useStore((s) => s.currentId);
  const sources = useStore((s) => s.sources);
  const ledgerBump = useStore((s) => s.ledgerBump);
  const openSourceViewer = useStore((s) => s.openSourceViewer);
  const pushToast = useStore((s) => s.pushToast);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const [entries, setEntries] = useState<LedgerEntry[] | null>(null);
  const [filter, setFilter] = useState<"all" | Kind>("all");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [helpHover, setHelpHover] = useState(false);

  // Composer: pick a kind, write the line. Why appears for decisions (their
  // because is the point) but every kind accepts one.
  const [draftKind, setDraftKind] = useState<Kind>("assertion");
  const [draftText, setDraftText] = useState("");
  const [draftWhy, setDraftWhy] = useState("");

  const load = async () => {
    if (!currentId) return;
    try {
      setEntries(await api.listLedger(currentId));
    } catch {
      setEntries([]);
    }
  };
  useEffect(() => {
    setEntries(null);
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentId, ledgerBump]);

  async function add() {
    if (!currentId || !draftText.trim()) return;
    try {
      await api.addLedgerEntry(currentId, draftKind, draftText, draftWhy);
      setDraftText("");
      setDraftWhy("");
      void load();
    } catch (e) {
      pushToast("error", e instanceof Error ? e.message : String(e));
    }
  }

  async function setStatus(entry: LedgerEntry, status: string) {
    try {
      await api.updateLedgerEntry(entry.id, { status });
      void load();
    } catch (e) {
      pushToast("error", e instanceof Error ? e.message : String(e));
    }
  }

  async function remove(entry: LedgerEntry) {
    if (
      !(await confirm({
        title: "Delete this entry?",
        message: `“${entry.text.slice(0, 120)}” will be removed from the ledger permanently.`,
        confirmLabel: "Delete",
        danger: true,
      }))
    )
      return;
    try {
      await api.deleteLedgerEntry(entry.id);
      void load();
    } catch (e) {
      pushToast("error", e instanceof Error ? e.message : String(e));
    }
  }

  const sourceTitle = (id: string) =>
    sources.find((s) => s.id === id)?.title ?? "Unknown source";

  const shown =
    entries?.filter((e) => filter === "all" || e.kind === filter) ?? [];
  const kindMeta = (kind: string) => KINDS.find((k) => k.id === kind);

  return (
    <div className="flex min-w-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-6">
        <span className="text-caption font-semibold uppercase tracking-wide text-muted-foreground">
          Ledger
        </span>
        <span className="relative flex">
          <button
            type="button"
            onMouseEnter={() => setHelpHover(true)}
            onMouseLeave={() => setHelpHover(false)}
            onFocus={() => setHelpHover(true)}
            onBlur={() => setHelpHover(false)}
            aria-label="What is the ledger?"
            className="rounded p-0.5 text-subtle-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
          >
            <Info className="h-3.5 w-3.5" />
          </button>
          {helpHover && (
            <div className="pointer-events-none absolute left-0 top-full z-50 mt-2 w-96 rounded-lg border border-border-strong bg-surface-2 p-3 text-caption font-normal normal-case leading-relaxed tracking-normal text-muted-foreground shadow-2xl">
              <p>
                The ledger is this notebook&rsquo;s memory: assertions, facts,
                decisions (with their why), open questions, and log lines —
                each with a lifecycle, anchored to sources by exact quotes.
              </p>
              <p className="mt-1.5">
                It mostly fills itself: chat answers that establish something
                new appear here as anchored assertions (marked auto), and
                agents write entries too. Record your own with the composer.
                Click an anchor to open the source at the quoted passage; use
                a row&rsquo;s menu to move it through its lifecycle as the
                record corroborates, contradicts, or supersedes it.
              </p>
            </div>
          )}
        </span>
        {entries && entries.length > 0 && (
          <span className="text-micro tabular-nums text-subtle-foreground">
            {shown.length} of {entries.length}
          </span>
        )}
        <div className="ml-auto flex items-center gap-1">
          {(["all", ...KINDS.map((k) => k.id)] as const).map((id) => (
            <button
              key={id}
              type="button"
              onClick={() => setFilter(id as "all" | Kind)}
              aria-pressed={filter === id}
              className={cn(
                "rounded-md px-2 py-1 text-caption transition-colors",
                filter === id
                  ? "bg-surface-2 font-medium text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {id === "all" ? "All" : `${kindMeta(id)?.label}s`}
            </button>
          ))}
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-3xl px-6 py-4">
          {/* Composer: capture is one line, one motion. */}
          <div className="rounded-lg border border-border bg-surface p-3">
            <div className="flex items-center gap-1">
              {KINDS.map((k) => (
                <button
                  key={k.id}
                  type="button"
                  onClick={() => setDraftKind(k.id)}
                  aria-pressed={draftKind === k.id}
                  className={cn(
                    "flex items-center gap-1.5 rounded-md px-2 py-1 text-caption transition-colors",
                    draftKind === k.id
                      ? "bg-surface-2 font-medium text-foreground"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  <k.icon className="h-3.5 w-3.5" />
                  {k.label}
                </button>
              ))}
            </div>
            <form
              className="mt-2 flex flex-col gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                void add();
              }}
            >
              <Input
                name="ledger-text"
                aria-label="New ledger entry"
                placeholder={
                  draftKind === "decision"
                    ? "We decided…"
                    : draftKind === "question"
                      ? "Open question…"
                      : draftKind === "log"
                        ? "What happened, in a line…"
                        : "What the sources establish…"
                }
                value={draftText}
                onChange={(e) => setDraftText(e.target.value)}
              />
              {(draftKind === "decision" || draftWhy) && (
                <Input
                  name="ledger-why"
                  aria-label="Why"
                  placeholder="Because… (alternatives rejected, context)"
                  value={draftWhy}
                  onChange={(e) => setDraftWhy(e.target.value)}
                />
              )}
              <div className="flex justify-end">
                <Button
                  type="submit"
                  variant="primary"
                  size="sm"
                  disabled={!draftText.trim()}
                >
                  Record
                </Button>
              </div>
            </form>
          </div>

          {/* The record, newest first. */}
          {entries === null ? (
            <div className="py-10 text-center text-caption text-muted-foreground">
              Loading…
            </div>
          ) : shown.length === 0 ? (
            <div className="py-10">
              <EmptyState
                icon={<Logs className="h-7 w-7" />}
                title={
                  filter === "all" ? "Nothing on the record yet" : "None yet"
                }
                hint="Facts, decisions, and open questions appear here, each anchored to a source. Agents can write entries too."
              />
            </div>
          ) : (
            <div className="mt-4 flex flex-col">
              {shown.map((entry) => {
                const meta = kindMeta(entry.kind);
                const Icon = meta?.icon ?? Logs;
                return (
                  <article
                    key={entry.id}
                    className="group border-b border-border py-3"
                  >
                    <div className="flex items-start gap-2.5">
                      <Icon
                        className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground"
                        aria-label={meta?.label}
                      />
                      <div className="min-w-0 flex-1">
                        <p className="text-body leading-relaxed text-foreground">
                          {entry.text}
                        </p>
                        {entry.why && (
                          <p className="mt-0.5 text-caption leading-relaxed text-subtle-foreground">
                            {entry.why}
                          </p>
                        )}
                        <div className="mt-1.5 flex items-center gap-2">
                          <span
                            className={cn(
                              "rounded-full border px-1.5 py-px text-badge font-medium uppercase tracking-wide",
                              statusChip(entry.status),
                            )}
                          >
                            {entry.status}
                          </span>
                          {entry.origin === "auto" && (
                            <span
                              className="rounded-full border border-border-strong px-1.5 py-px text-badge font-medium uppercase tracking-wide text-subtle-foreground"
                              title="Recorded automatically from chat"
                            >
                              auto
                            </span>
                          )}
                          {entry.anchors.length > 0 && (
                            <button
                              type="button"
                              onClick={() =>
                                setExpanded(
                                  expanded === entry.id ? null : entry.id,
                                )
                              }
                              className="text-micro text-citation transition-colors hover:underline"
                            >
                              {entry.anchors.length}{" "}
                              {entry.anchors.length === 1
                                ? "anchor"
                                : "anchors"}
                            </button>
                          )}
                          <span className="ml-auto text-micro text-subtle-foreground">
                            {relativeTime(entry.createdAt)}
                          </span>
                          <RowMenu
                            className="opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100"
                            label={`Options for this ${entry.kind}`}
                            items={[
                              ...STATUSES[entry.kind as Kind]
                                .filter((s) => s !== entry.status)
                                .map((s) => ({
                                  label: `Mark ${s}`,
                                  onClick: () => void setStatus(entry, s),
                                })),
                              {
                                label: "Delete",
                                icon: <Trash2 className="h-3.5 w-3.5" />,
                                danger: true,
                                onClick: () => void remove(entry),
                              },
                            ]}
                          />
                        </div>
                        {expanded === entry.id && entry.anchors.length > 0 && (
                          <div className="mt-2 flex flex-col gap-1.5">
                            {entry.anchors.map((a, i) => (
                              <button
                                key={i}
                                type="button"
                                onClick={() =>
                                  openSourceViewer(
                                    a.sourceId,
                                    sourceTitle(a.sourceId),
                                    a.quote || undefined,
                                  )
                                }
                                className="rounded-md border border-border bg-surface px-2.5 py-1.5 text-left transition-colors hover:bg-surface-2"
                                title="Open in the reader at this passage"
                              >
                                <span className="text-micro font-medium text-citation">
                                  {sourceTitle(a.sourceId)}
                                </span>
                                {a.quote && (
                                  <span className="mt-0.5 block truncate text-micro text-muted-foreground">
                                    “{a.quote}”
                                  </span>
                                )}
                              </button>
                            ))}
                          </div>
                        )}
                      </div>
                    </div>
                  </article>
                );
              })}
            </div>
          )}
        </div>
      </div>
      {confirmDialog}
    </div>
  );
}
