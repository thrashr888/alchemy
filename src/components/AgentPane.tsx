import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowUp,
  Bot,
  ChevronDown,
  Copy,
  ExternalLink,
  RotateCw,
  Square,
  Wrench,
} from "lucide-react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { Button, EmptyState, Textarea } from "./ui";
import { Markdown } from "./Markdown";
import { cn } from "@/lib/utils";
import type {
  AcpAgentInfo,
  AcpPermissionEvent,
  AcpStateEvent,
  AcpUpdateEvent,
} from "@/lib/types";
import type { AcpEntry as Entry } from "@/lib/storeTypes";

/** Hosted-agent transcript (docs/RFC-acp-agents.md): the user's own coding
 *  agent, run over ACP with Alchemy's MCP server attached, rendered as turns.
 *  Separate from the RAG chat transcript — this one is the agent's stream,
 *  not our retrieval pipeline. The transcript and agent choice live in the
 *  store (per notebook), so the pane can unmount and come back without
 *  presenting an amnesiac view. */

const COMPOSER_MAX_H = 200;
const NO_ENTRIES: Entry[] = [];

export function AgentPane({
  notebookId,
  visible = true,
}: {
  notebookId: string;
  visible?: boolean;
}) {
  const [agents, setAgents] = useState<AcpAgentInfo[] | null>(null);
  const [discoveryError, setDiscoveryError] = useState<string | null>(null);
  const [state, setState] = useState<AcpStateEvent["state"] | null>(null);
  const agentId = useStore(
    (s) => s.acpPanes[notebookId]?.agentId ?? null,
  );
  const entries = useStore((s) =>
    agentId
      ? (s.acpPanes[notebookId]?.agents[agentId]?.entries ?? NO_ENTRIES)
      : NO_ENTRIES,
  );
  const draft = useStore((s) =>
    agentId ? (s.acpPanes[notebookId]?.agents[agentId]?.draft ?? "") : "",
  );
  const setAcpAgentId = useStore((s) => s.setAcpAgentId);
  const setAcpEntries = useStore((s) => s.setAcpEntries);
  const setAcpDraft = useStore((s) => s.setAcpDraft);
  const hydrateAcpPane = useStore((s) => s.hydrateAcpPane);
  const hostedAgent = useStore((s) => s.aiConfig?.hostedAgent ?? "");

  // Restore the persisted transcript + agent choice before the picker's
  // first-available default can claim the slot. Idempotent (seeds only when
  // the store has nothing), so StrictMode's double-run is harmless.
  useEffect(() => {
    hydrateAcpPane(notebookId);
  }, [notebookId, hydrateAcpPane]);
  const setAgentId = useCallback(
    (id: string | null) => setAcpAgentId(notebookId, id),
    [notebookId, setAcpAgentId],
  );
  // Every write names the agent it belongs to. Sessions can't outlive a
  // picker change (the picker locks while one runs), so "the selected agent"
  // is always the one this stream is for.
  const setEntries = useCallback(
    (update: Entry[] | ((prev: Entry[]) => Entry[])) => {
      if (!agentId) return;
      setAcpEntries(notebookId, agentId, (prev) =>
        typeof update === "function" ? update(prev) : update,
      );
    },
    [notebookId, agentId, setAcpEntries],
  );
  const setDraft = useCallback(
    (text: string) => {
      if (!agentId) return;
      setAcpDraft(notebookId, agentId, text);
    },
    [notebookId, agentId, setAcpDraft],
  );
  const [permission, setPermission] = useState<AcpPermissionEvent | null>(null);
  const [starting, setStarting] = useState(false);
  // A session can open fine and still have no notebook tools, when Alchemy's
  // MCP server isn't running. That's the whole reason to host the agent here,
  // so it gets said out loud rather than leaving the user to wonder why the
  // agent can't find their sources.
  const [noNotebookAccess, setNoNotebookAccess] = useState(false);
  // Session failures stay on screen until the user acts on them. They used to
  // be toasts, which auto-dismissed before the message — usually "sign in
  // first" — could be read, let alone acted on. `prompt` carries the message
  // that never got sent, so Retry can replay it.
  const [failure, setFailure] = useState<{
    message: string;
    prompt?: string;
  } | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<HTMLTextAreaElement>(null);

  const running = state !== null && state !== "stopped" && state !== "error";
  const busy = state === "turn";

  // Discovery failing is worth saying out loud: silently showing "no agents"
  // makes a broken probe look like an empty machine.
  useEffect(() => {
    void api
      .acpAgents()
      .then((list) => {
        setAgents(list);
        setDiscoveryError(null);
      })
      .catch((err: unknown) => {
        setAgents([]);
        setDiscoveryError(err instanceof Error ? err.message : String(err));
      });
  }, []);

  // Re-sync a remounted pane with whatever session is already running.
  useEffect(() => {
    let stale = false;
    void api
      .acpStatus(notebookId)
      .then((id) => {
        if (stale || !id) return;
        setAgentId(id);
        setState("idle");
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [notebookId]);

  // Default the picker: the agent chosen in Settings when it's installed,
  // otherwise the first one that is. A stale preference (agent uninstalled,
  // or a build that dropped it) falls through rather than pinning the picker
  // to something that can't run.
  useEffect(() => {
    if (agentId) return;
    const preferred = agents?.find((a) => a.id === hostedAgent && a.available);
    const first = preferred ?? agents?.find((a) => a.available);
    if (first) setAgentId(first.id);
  }, [agents, agentId, hostedAgent, setAgentId]);

  // Backend events are broadcast to every window; self-filter by notebook.
  useEffect(() => {
    const unlistenState = listen<AcpStateEvent>("acp://state", (e) => {
      if (e.payload.notebookId !== notebookId) return;
      setState(e.payload.state);
      if (e.payload.state === "ready") {
        const detail = e.payload.detail as { mcpAttached?: boolean } | null;
        setNoNotebookAccess(detail?.mcpAttached === false);
      }
      if (e.payload.state === "error") {
        const detail = e.payload.detail;
        setFailure({
          message:
            typeof detail === "string" ? detail : "The agent hit an error",
        });
      }
    });
    const unlistenUpdate = listen<AcpUpdateEvent>("acp://update", (e) => {
      if (e.payload.notebookId !== notebookId) return;
      setEntries((prev) => applyUpdate(prev, e.payload.update));
    });
    const unlistenPermission = listen<AcpPermissionEvent>(
      "acp://permission",
      (e) => {
        if (e.payload.notebookId !== notebookId) return;
        setPermission(e.payload);
      },
    );
    return () => {
      void unlistenState.then((fn) => fn());
      void unlistenUpdate.then((fn) => fn());
      void unlistenPermission.then((fn) => fn());
    };
  }, [notebookId]);

  // Stop the session when the pane goes away, so no agent subprocess
  // outlives the UI that was driving it.
  useEffect(() => {
    return () => {
      void api.acpStop(notebookId).catch(() => {});
    };
  }, [notebookId]);

  // A draft restored from the store (agent switch, remount, app restart)
  // never passed through onChange, so the textarea would sit at one row with
  // the rest of the question scrolled out of sight. Size it to its content
  // whenever the text arrives from outside.
  useEffect(() => {
    const el = composerRef.current;
    if (!el || !visible) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_H)}px`;
  }, [draft, visible]);

  // Also re-run on becoming visible: while the pane is hidden (Chat view in
  // front) the scroll area has no height, so any pinning done then is lost.
  useEffect(() => {
    if (!visible) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [entries, permission, visible]);

  /// Returns whether the session came up — a failed start must not fall
  /// through to a prompt, or the real error is buried under a second,
  /// misleading "no agent session for this notebook".
  const start = useCallback(async (): Promise<boolean> => {
    if (!agentId) return false;
    setStarting(true);
    try {
      await api.acpStart(notebookId, agentId);
      setEntries([]);
      setFailure(null);
      composerRef.current?.focus();
      return true;
    } catch (err) {
      setState(null);
      setFailure({
        message: err instanceof Error ? err.message : String(err),
      });
      return false;
    } finally {
      setStarting(false);
    }
  }, [agentId, notebookId]);

  const sendPrompt = useCallback(
    async (text: string) => {
      setFailure(null);
      setEntries((prev) => [...prev, { kind: "user", text }]);
      try {
        await api.acpPrompt(notebookId, text);
      } catch (err) {
        setFailure({
          message: err instanceof Error ? err.message : String(err),
          prompt: text,
        });
      }
    },
    [notebookId],
  );

  async function submit() {
    const text = draft.trim();
    if (!text || busy) return;
    if (!running && !(await start())) {
      // Keep the text in the composer: the session never opened, so the
      // message was never sent and retyping it would be busywork.
      setFailure((f) => (f ? { ...f, prompt: text } : f));
      return;
    }
    setDraft("");
    await sendPrompt(text);
  }

  /// Re-run whatever failed: start the session, then replay the prompt that
  /// never made it.
  async function retry() {
    const pending = failure?.prompt;
    setFailure(null);
    if (!(await start())) return;
    if (pending) {
      setDraft("");
      await sendPrompt(pending);
    }
  }

  async function answerPermission(optionId: string | null) {
    if (!permission) return;
    const { requestId } = permission;
    setPermission(null);
    try {
      await api.acpPermission(notebookId, requestId, optionId);
    } catch (err) {
      setFailure({
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }

  const available = (agents ?? []).filter((a) => a.available);

  return (
    <>
      <div ref={scrollRef} className="relative z-10 flex-1 overflow-y-auto">
        <div className="mx-auto flex max-w-[720px] flex-col gap-4 px-5 py-6">
          {entries.length === 0 && !running && (
            <AgentBlankSlate agents={agents} discoveryError={discoveryError} />
          )}
          {entries.map((entry, i) => (
            <EntryRow key={i} entry={entry} />
          ))}
          {permission && (
            <PermissionPrompt
              request={permission}
              onAnswer={(id) => void answerPermission(id)}
            />
          )}
          {running && noNotebookAccess && (
            <p className="text-micro text-subtle-foreground">
              Running without notebook access — Alchemy's MCP server isn't
              available, so the agent can't search this notebook.
            </p>
          )}
          {failure && (
            <FailureNotice
              message={failure.message}
              loginCommand={
                agents?.find((a) => a.id === agentId)?.loginCommand ?? null
              }
              retrying={starting}
              onRetry={() => void retry()}
              onDismiss={() => setFailure(null)}
            />
          )}
          {busy && entries.length > 0 && (
            <p className="text-micro text-subtle-foreground">Working…</p>
          )}
        </div>
      </div>

      <div className="relative z-10 border-t border-border px-5 py-3">
        <div className="mx-auto max-w-[720px]">
          <Textarea
            ref={composerRef}
            value={draft}
            onChange={(e) => {
              setDraft(e.target.value);
              const el = e.currentTarget;
              el.style.height = "auto";
              el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_H)}px`;
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void submit();
              }
            }}
            rows={1}
            placeholder={
              available.length === 0
                ? "No agent available"
                : running
                  ? "Message the agent…"
                  : "Start a session and ask the agent…"
            }
            disabled={available.length === 0}
          />
          <div className="flex items-center gap-1.5 px-1.5 pt-1">
            <AgentPicker
              agents={agents ?? []}
              value={agentId}
              running={running}
              onChange={setAgentId}
            />
            {running && (
              <button
                onClick={() => void api.acpStop(notebookId).catch(() => {})}
                className="inline-flex items-center rounded-md border border-border bg-surface-2 px-2 py-1 text-micro text-muted-foreground transition-colors hover:text-foreground"
              >
                End session
              </button>
            )}
            {starting && (
              <span className="text-micro text-subtle-foreground">
                Starting…
              </span>
            )}
            <span className="flex-1" />
            {busy ? (
              <Button
                variant="secondary"
                size="icon"
                onClick={() => void api.acpCancel(notebookId).catch(() => {})}
                title="Stop"
                aria-label="Stop the agent"
              >
                <Square className="h-3.5 w-3.5" />
              </Button>
            ) : (
              <Button
                variant="primary"
                size="icon"
                onClick={() => void submit()}
                disabled={!draft.trim() || available.length === 0}
                title="Send"
                aria-label="Send message"
              >
                <ArrowUp className="h-4 w-4" />
              </Button>
            )}
          </div>
        </div>
      </div>
    </>
  );
}

/** The Agent view before its first turn. The state worth designing for is
 *  the empty machine: an agent picker with nothing in it explains nothing, so
 *  name the three agents that work here and hand over the command that
 *  installs each. Install hints come from the backend so they stay next to
 *  the binary names discovery actually probes for. */
function AgentBlankSlate({
  agents,
  discoveryError,
}: {
  agents: AcpAgentInfo[] | null;
  discoveryError: string | null;
}) {
  const pushToast = useStore((s) => s.pushToast);
  const icon = <Bot className="h-6 w-6" />;

  if (discoveryError) {
    return (
      <EmptyState
        icon={icon}
        title="Couldn't check for agents"
        hint={discoveryError}
      />
    );
  }
  if (agents === null) {
    return <EmptyState icon={icon} title="Looking for agents…" />;
  }
  if (agents.some((a) => a.available)) {
    return (
      <EmptyState
        icon={icon}
        title="Run your own coding agent"
        hint="It can search this notebook's sources while it works."
      />
    );
  }
  return (
    <EmptyState
      icon={icon}
      title="No coding agents installed"
      hint="Install one and it appears here."
    >
      <div className="mt-2 flex w-full max-w-sm flex-col divide-y divide-border rounded-md border border-border text-left">
        {agents.map((a) => (
          <div key={a.id} className="flex items-center gap-2 px-2.5 py-2">
            <span className="shrink-0 text-caption text-foreground">
              {a.label}
            </span>
            <code className="ml-auto truncate font-mono text-micro text-subtle-foreground">
              {a.installHint}
            </code>
            <button
              type="button"
              onClick={() => {
                void navigator.clipboard.writeText(a.installHint);
                pushToast("success", `Install command for ${a.label} copied`);
              }}
              title={`Copy: ${a.installHint}`}
              aria-label={`Copy the install command for ${a.label}`}
              className="shrink-0 rounded p-1 text-subtle-foreground transition-colors hover:text-foreground"
            >
              <Copy className="h-3.5 w-3.5" />
            </button>
          </div>
        ))}
      </div>
    </EmptyState>
  );
}

/** Fold one session/update into the transcript. Message and thought chunks
 *  stream token by token, so they append into the trailing entry of their
 *  kind rather than starting a new one; tool calls update in place by id. */
function applyUpdate(prev: Entry[], update: AcpUpdateEvent["update"]): Entry[] {
  const kind = update.sessionUpdate;
  const text = update.content?.text ?? "";
  if (kind === "agent_message_chunk" || kind === "agent_thought_chunk") {
    const want = kind === "agent_message_chunk" ? "agent" : "thought";
    const last = prev[prev.length - 1];
    if (last && last.kind === want) {
      const merged = { ...last, text: last.text + text } as Entry;
      return [...prev.slice(0, -1), merged];
    }
    return [...prev, { kind: want, text } as Entry];
  }
  if (kind === "tool_call") {
    const id = String(update.toolCallId ?? prev.length);
    return [
      ...prev,
      {
        kind: "tool",
        id,
        title: update.title ?? "tool",
        status: update.status ?? "pending",
      },
    ];
  }
  if (kind === "tool_call_update") {
    const id = update.toolCallId ? String(update.toolCallId) : null;
    // Status-only updates omit the id; they refer to the newest tool call.
    const idx = id
      ? prev.findIndex((e) => e.kind === "tool" && e.id === id)
      : findLastToolIndex(prev);
    if (idx < 0) return prev;
    const target = prev[idx];
    if (target.kind !== "tool") return prev;
    const next = [...prev];
    next[idx] = {
      ...target,
      status: update.status ?? target.status,
      title: update.title ?? target.title,
    };
    return next;
  }
  return prev;
}

function findLastToolIndex(entries: Entry[]): number {
  for (let i = entries.length - 1; i >= 0; i--) {
    if (entries[i].kind === "tool") return i;
  }
  return -1;
}

/** A session failure that stays put. Most of these are "the agent isn't
 *  signed in", which is fixable in a terminal — so the notice carries the
 *  sign-in command and a retry, rather than making the user find both. */
function FailureNotice({
  message,
  loginCommand,
  retrying,
  onRetry,
  onDismiss,
}: {
  message: string;
  loginCommand: string | null;
  retrying: boolean;
  onRetry: () => void;
  onDismiss: () => void;
}) {
  return (
    <div className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2.5">
      <p className="text-caption text-destructive [overflow-wrap:anywhere]">
        {message}
      </p>
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        {loginCommand && (
          <Button
            size="sm"
            variant="secondary"
            onClick={() => void api.openInTerminal(loginCommand)}
          >
            <ExternalLink className="h-3.5 w-3.5" />
            Open Terminal: {loginCommand}
          </Button>
        )}
        <Button
          size="sm"
          variant="secondary"
          disabled={retrying}
          onClick={onRetry}
        >
          <RotateCw className={cn("h-3.5 w-3.5", retrying && "animate-spin")} />
          {retrying ? "Retrying…" : "Retry"}
        </Button>
        <Button size="sm" variant="ghost" onClick={onDismiss}>
          Dismiss
        </Button>
      </div>
    </div>
  );
}

function EntryRow({ entry }: { entry: Entry }) {
  if (entry.kind === "user") {
    return (
      <div className="self-end rounded-lg border border-border bg-surface-2 px-3 py-2 text-body">
        {entry.text}
      </div>
    );
  }
  if (entry.kind === "agent") {
    return (
      <div className="text-body">
        <Markdown>{entry.text}</Markdown>
      </div>
    );
  }
  if (entry.kind === "thought") return <ThoughtBlock text={entry.text} />;
  return <ToolChip title={entry.title} status={entry.status} />;
}

function ThoughtBlock({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="text-micro text-subtle-foreground">
      <button
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="inline-flex items-center gap-1 transition-colors hover:text-muted-foreground"
      >
        <ChevronDown
          className={cn("h-3 w-3 transition-transform", !open && "-rotate-90")}
        />
        Thinking
      </button>
      {open && <p className="mt-1 whitespace-pre-wrap pl-4">{text}</p>}
    </div>
  );
}

function ToolChip({ title, status }: { title: string; status: string }) {
  return (
    <div className="inline-flex items-center gap-1.5 self-start rounded-md border border-border bg-surface-2 px-2 py-1 text-micro text-muted-foreground">
      <Wrench className="h-3 w-3" />
      <span className="font-mono">{title}</span>
      <span className="text-subtle-foreground">{statusLabel(status)}</span>
    </div>
  );
}

function statusLabel(status: string): string {
  switch (status) {
    case "completed":
      return "done";
    case "in_progress":
      return "running";
    case "failed":
      return "failed";
    default:
      return status.replace(/_/g, " ");
  }
}

function PermissionPrompt({
  request,
  onAnswer,
}: {
  request: AcpPermissionEvent;
  onAnswer: (optionId: string | null) => void;
}) {
  return (
    <div className="rounded-lg border border-border bg-surface-2 px-3 py-2.5">
      <p className="text-caption text-foreground">
        The agent wants to run{" "}
        <span className="font-mono">{request.toolTitle || "a tool"}</span>.
      </p>
      <div className="mt-2 flex flex-wrap gap-1.5">
        {request.options.map((opt) => (
          <Button
            key={opt.id}
            size="sm"
            variant={opt.kind.startsWith("allow") ? "primary" : "secondary"}
            onClick={() => onAnswer(opt.id)}
          >
            {opt.name}
          </Button>
        ))}
        <Button size="sm" variant="ghost" onClick={() => onAnswer(null)}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

function AgentPicker({
  agents,
  value,
  running,
  onChange,
}: {
  agents: AcpAgentInfo[];
  value: string | null;
  running: boolean;
  onChange: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const current = agents.find((a) => a.id === value);
  return (
    <span className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-haspopup="menu"
        disabled={running}
        title={
          running
            ? "End the session to switch agents"
            : "Choose which agent to run"
        }
        className="inline-flex items-center gap-1 rounded-md border border-border bg-surface-2 px-2 py-1 text-micro text-muted-foreground transition-colors hover:text-foreground disabled:opacity-60"
      >
        <Bot className="h-3 w-3" />
        {current?.label ?? "Agent"}
        <ChevronDown className="h-3 w-3" />
      </button>
      {open && (
        <>
          <button
            type="button"
            aria-label="Close menu"
            className="fixed inset-0 z-20 cursor-default"
            onClick={() => setOpen(false)}
          />
          <div
            role="menu"
            aria-label="Choose an agent"
            className="menu-glass absolute bottom-full left-0 z-30 mb-1.5 min-w-52 rounded-md py-1"
          >
            {agents.map((agent) => (
              <button
                key={agent.id}
                role="menuitem"
                disabled={!agent.available}
                onClick={() => {
                  onChange(agent.id);
                  setOpen(false);
                }}
                className={cn(
                  "flex w-full items-center justify-between px-2.5 py-1.5 text-left text-caption transition-colors",
                  agent.available
                    ? "text-foreground hover:bg-surface-2"
                    : "cursor-not-allowed text-subtle-foreground",
                )}
              >
                {agent.label}
                {!agent.available && (
                  <span className="text-micro">not installed</span>
                )}
              </button>
            ))}
          </div>
        </>
      )}
    </span>
  );
}
