import { useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { AlchemySymbol } from "./AlchemyHero";
import { THEMES, resolveThemeId } from "@/lib/themes";
import { MacConnect } from "./MacConnect";
import { Badge, Button, Chip, Input, Segmented, Select } from "./ui";
import { cn } from "@/lib/utils";
import type {
  ConnectorStatus,
  DesktopApp,
  ModelStatus,
  ProviderEntry,
} from "@/lib/types";
import {
  Check,
  Copy,
  CheckCircle2,
  XCircle,
  Circle,
  Cpu,
  MonitorSmartphone,
  Plug,
  RefreshCw,
  Server,
  Terminal,
} from "lucide-react";

/** One copyable shell command. */
function CommandChip({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(command);
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        } catch {
          /* clipboard unavailable */
        }
      }}
      title="Copy to clipboard"
      className="inline-flex items-center gap-1.5 rounded-md border border-border bg-surface-2 px-2 py-1 font-mono text-[0.71875rem] text-foreground/85 transition-colors hover:border-border-strong"
    >
      {command}
      {copied ? (
        <Check className="h-3 w-3 shrink-0 text-success" />
      ) : (
        <Copy className="h-3 w-3 shrink-0 text-subtle-foreground" />
      )}
    </button>
  );
}

/** One setup step's state. "todo" is a step nobody has tried yet — a
 *  hollow circle, like the optional ones — and "failed" is a check that
 *  actually ran and came back wrong. A first-run screen that opened with a
 *  column of red crosses read as a list of requirements, when it is a list
 *  of doors and you only need one. */
type StepState = "ok" | "todo" | "failed";

/** Tick, hollow circle, or cross. The word for it rides beside the icon
 *  (see `Step`), so the icon itself is decoration. */
function StatusIcon({ state }: { state: StepState }) {
  if (state === "ok")
    return <CheckCircle2 aria-hidden className="h-4 w-4 shrink-0 text-success" />;
  if (state === "todo")
    return <Circle aria-hidden className="h-4 w-4 shrink-0 text-subtle-foreground" />;
  return <XCircle aria-hidden className="h-4 w-4 shrink-0 text-destructive" />;
}

function statusWord(state: StepState, optional?: boolean): string {
  if (state === "ok") return "Ready";
  if (state === "failed") return "Not working";
  return optional ? "Not set up" : "Not set up yet";
}

function Step({
  ok,
  failed = false,
  optional,
  title,
  detail,
  children,
}: {
  ok: boolean;
  /** A check ran and came back wrong. Without it, a not-ok step is "todo". */
  failed?: boolean;
  optional?: boolean;
  title: string;
  detail?: string;
  children?: React.ReactNode;
}) {
  const state: StepState = ok ? "ok" : failed ? "failed" : "todo";
  return (
    <div
      className={cn(
        "flex flex-col gap-2 rounded-lg border px-4 py-3",
        ok ? "border-border bg-surface/50" : "border-border-strong bg-surface",
      )}
    >
      <div className="flex items-center gap-2.5">
        <StatusIcon state={state} />
        <span className="text-body font-medium text-foreground">{title}</span>
        <span className="sr-only">{statusWord(state, optional)}</span>
        {optional && (
          <span className="rounded border border-border px-1 py-px text-badge uppercase tracking-wide text-subtle-foreground">
            Optional
          </span>
        )}
      </div>
      {!ok && detail && <p className="pl-6.5 text-caption text-muted-foreground">{detail}</p>}
      {!ok && children && <div className="flex flex-wrap items-center gap-1.5 pl-6.5">{children}</div>}
    </div>
  );
}

/** The subscription CLIs the first-run doors offer, when installed. Ids are
 *  the backend's `AgentKind::id`; the same entries Settings → Models adds. */
const AGENT_DOORS: Record<string, { label: string; note: string; vendor: string }> = {
  "claude-code": { label: "Claude Code", note: "Your Claude subscription", vendor: "Claude" },
  codex: { label: "Codex", note: "Your ChatGPT subscription", vendor: "ChatGPT" },
};

/** The inset group every step's list lives in: one rounded-10 container on
 *  `surface-2`, hairline border, hairline between rows. The rows inside are
 *  radios (a choice) or plain rows with a trailing action (a check). */
function RowGroup({
  children,
  role,
  label,
}: {
  children: React.ReactNode;
  role?: "radiogroup";
  label?: string;
}) {
  return (
    <div
      role={role}
      aria-label={role ? label : undefined}
      className="divide-y divide-border overflow-hidden rounded-[10px] border border-border bg-surface-2"
    >
      {children}
    </div>
  );
}

/** A green status pill, used only when something has actually been found or
 *  is actually running. Everything else stays a plain gray badge. */
function StatusBadge({
  ok,
  children,
  title,
}: {
  ok?: boolean;
  children: React.ReactNode;
  title?: string;
}) {
  return (
    <Badge
      title={title}
      className={cn(
        "shrink-0",
        ok && "border-success/30 bg-success/10 text-success",
      )}
    >
      {children}
    </Badge>
  );
}

type RadioOption<T extends string> = {
  value: T;
  icon: React.ReactNode;
  title: string;
  hint: string;
  /** Right-hand status pill; `ok` gives it the success tone. */
  status?: { label: string; ok?: boolean };
  /** A door this Mac does not have. Shown dimmed and unpickable. */
  disabled?: boolean;
};

/**
 * One grouped inset radio list — the Setup Assistant's single question.
 * Roving tabindex, arrows move and choose, as a radiogroup should.
 */
function RadioList<T extends string>({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: T | null;
  onChange: (value: T) => void;
  options: readonly RadioOption<T>[];
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const firstEnabled = options.findIndex((o) => !o.disabled);

  function move(from: number, dir: 1 | -1) {
    const n = options.length;
    for (let hop = 1; hop <= n; hop += 1) {
      const i = (from + dir * hop + n * n) % n;
      if (options[i].disabled) continue;
      onChange(options[i].value);
      refs.current[i]?.focus();
      return;
    }
  }

  return (
    <RowGroup role="radiogroup" label={label}>
      {options.map((option, i) => {
        const checked = value === option.value;
        return (
          <button
            key={option.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            disabled={option.disabled}
            // A radiogroup is one tab stop: the chosen row, or the first
            // pickable one while nothing is chosen.
            tabIndex={checked || (value === null && i === firstEnabled) ? 0 : -1}
            onClick={() => onChange(option.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown" || e.key === "ArrowRight") {
                e.preventDefault();
                move(i, 1);
              } else if (e.key === "ArrowUp" || e.key === "ArrowLeft") {
                e.preventDefault();
                move(i, -1);
              }
            }}
            className={cn(
              "flex w-full items-center gap-3 px-3.5 py-3 text-left transition-colors outline-none",
              "focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60",
              option.disabled
                ? "cursor-default opacity-50"
                : "hover:bg-surface",
            )}
          >
            <span
              aria-hidden
              className={cn(
                "flex h-4 w-4 shrink-0 items-center justify-center rounded-full border transition-colors",
                checked ? "border-primary bg-primary" : "border-border-strong",
              )}
            >
              {checked && (
                <span className="h-1.5 w-1.5 rounded-full bg-primary-foreground" />
              )}
            </span>
            <span className="shrink-0 text-muted-foreground">{option.icon}</span>
            <span className="flex min-w-0 flex-1 flex-col">
              <span className="text-body font-medium text-foreground">
                {option.title}
              </span>
              <span className="truncate text-caption text-muted-foreground">
                {option.hint}
              </span>
            </span>
            {option.status && (
              <StatusBadge ok={option.status.ok}>
                {option.status.label}
              </StatusBadge>
            )}
          </button>
        );
      })}
    </RowGroup>
  );
}

/**
 * Step 3's list: the agent clients on this Mac, and whether each one can
 * reach Alchemy's MCP server. Same readings and same Connect as
 * Settings → Agents; a client that installs the connection itself (Claude
 * Desktop's extension sheet) is watched until its config appears.
 */
function AgentRows() {
  const pushToast = useStore((s) => s.pushToast);
  const [rows, setRows] = useState<ConnectorStatus[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const watching = useRef<number | null>(null);

  function read() {
    api
      .listAgentConnectors()
      .then(setRows)
      .catch(() => setRows([]));
  }
  // The rows read files other apps write — Cursor's config after its install
  // sheet, Claude Desktop's registry — so one read at mount goes stale the
  // moment the user leaves to say yes somewhere else.
  useEffect(() => {
    read();
    window.addEventListener("focus", read);
    return () => {
      window.removeEventListener("focus", read);
      if (watching.current !== null) window.clearInterval(watching.current);
    };
  }, []);

  function watchUntilConfigured(id: string, name: string) {
    if (watching.current !== null) window.clearInterval(watching.current);
    let polls = 0;
    watching.current = window.setInterval(() => {
      polls += 1;
      api
        .listAgentConnectors()
        .then((list) => {
          setRows(list);
          const row = list.find((x) => x.id === id);
          if (row?.configured || polls >= 40) {
            if (watching.current !== null) window.clearInterval(watching.current);
            watching.current = null;
            if (row?.configured) pushToast("success", `${name} connected.`);
          }
        })
        .catch(() => {});
    }, 3000);
  }

  function connect(c: ConnectorStatus) {
    setBusy(c.id);
    api
      .connectAgent(c.id)
      .then((updated) => {
        setRows((list) =>
          (list ?? []).map((x) => (x.id === updated.id ? updated : x)),
        );
        pushToast(
          "success",
          // A client that installs the connection itself isn't connected
          // yet — its own sheet is still waiting on the user.
          updated.connectNote ??
            (updated.configured
              ? `${updated.name} connected. Restart it to pick up the change.`
              : `Skill installed for ${updated.name}`),
        );
        if (updated.connectNote && !updated.configured)
          watchUntilConfigured(updated.id, updated.name);
      })
      .catch((e) =>
        pushToast("error", e instanceof Error ? e.message : String(e)),
      )
      .finally(() => setBusy(null));
  }

  function copySnippet(c: ConnectorStatus) {
    void navigator.clipboard.writeText(c.snippet);
    pushToast("success", `Setup for ${c.name} copied`);
  }

  if (rows === null)
    return (
      <RowGroup>
        <div className="px-3.5 py-3 text-caption text-subtle-foreground">
          Looking for agent clients…
        </div>
      </RowGroup>
    );

  const sorted = [...rows].sort((a, b) => a.name.localeCompare(b.name));
  return (
    <RowGroup>
      {sorted.map((c) => (
        <div
          key={c.id}
          className={cn(
            "flex items-center gap-3 px-3.5 py-3",
            !c.installed && "opacity-50",
          )}
        >
          <Plug
            aria-hidden
            className="h-[18px] w-[18px] shrink-0 text-muted-foreground"
          />
          <div className="flex min-w-0 flex-1 flex-col">
            <span className="text-body font-medium text-foreground">{c.name}</span>
            <span className="truncate text-caption text-muted-foreground">
              {c.configPath}
            </span>
          </div>
          {c.configured ? (
            <StatusBadge ok title={c.configPath}>
              Connected{c.supportsSkill && c.skillInstalled ? " + skill" : ""}
            </StatusBadge>
          ) : c.installed ? (
            <Button
              variant="secondary"
              size="sm"
              loading={busy === c.id}
              onClick={() => (c.canAuto ? connect(c) : copySnippet(c))}
            >
              {c.canAuto ? "Connect" : "Copy command"}
            </Button>
          ) : (
            <StatusBadge>Not installed</StatusBadge>
          )}
          {/* Skill catch-up for manual or partial rows. */}
          {c.installed && c.configured && c.supportsSkill && !c.skillInstalled && (
            <Button
              variant="ghost"
              size="sm"
              loading={busy === c.id}
              onClick={() => connect(c)}
            >
              Add skill
            </Button>
          )}
          {/* Escape hatch: the manual setup, always copyable. */}
          <button
            type="button"
            title={`Copy manual setup\n${c.snippet}`}
            onClick={() => copySnippet(c)}
            aria-label={`Copy manual setup for ${c.name}`}
            className="shrink-0 rounded p-1 text-subtle-foreground transition-colors hover:text-foreground"
          >
            <Copy className="h-3.5 w-3.5" />
          </button>
        </div>
      ))}
      {sorted.length === 0 && (
        <div className="px-3.5 py-3 text-caption text-subtle-foreground">
          No agent clients found on this Mac.
        </div>
      )}
    </RowGroup>
  );
}

/** Which door answers questions. The four rows of step one. */
type AnswerPath = "fm" | "elsewhere" | "agent" | "byo";

const STEPS = ["Answers", "Mac apps", "Agents"] as const;

/**
 * First run, as a Setup Assistant: the brand on the left, one question at a
 * time on the right. Three steps — where answers come from, which Mac apps
 * to read, which agents may reach Alchemy — each a grouped inset list whose
 * status pills keep updating while the screen is open. Nothing here is a
 * requirement; the screen is a set of doors and you only need one.
 */
export function Onboarding({ onOpenSettings }: { onOpenSettings: () => void }) {
  const health = useStore((s) => s.modelHealth);
  const theme = useStore((s) => s.theme);
  const aiConfig = useStore((s) => s.aiConfig);
  const macAvailable = useStore((s) => s.macAvailable);
  const save = useStore((s) => s.saveAiConfig);
  const dismiss = useStore((s) => s.dismissOnboarding);
  const refresh = useStore((s) => s.refreshModelHealth);
  const [stage, setStage] = useState(0);
  const [checking, setChecking] = useState(false);
  const [gwUrl, setGwUrl] = useState("");
  const [gwKey, setGwKey] = useState("");
  const [gwModel, setGwModel] = useState("");
  const [gwVision, setGwVision] = useState("");
  const [gwSaving, setGwSaving] = useState(false);
  const [gwModels, setGwModels] = useState<string[]>([]);
  const [gwStatus, setGwStatus] = useState<string | null>(null);
  // Installed subscription CLIs, probed once: a Mac with Claude Code and no
  // Ollama, no Apple Intelligence, and no API key still has a chat model.
  // Hiding that door behind "Settings → Models…" blocked exactly that Mac.
  const [agentDoors, setAgentDoors] = useState<string[]>([]);
  // Desktop AI apps on this Mac (docs/RFC-desktop-apps.md phase 3): a door
  // that says answers happen THERE, and Alchemy indexes here.
  const [deskDoors, setDeskDoors] = useState<DesktopApp[]>([]);
  // Apple Intelligence: the row's pill should say Ready before it is chosen,
  // which needs the same per-provider probe Settings → Models reads.
  const [fmReady, setFmReady] = useState<boolean | null>(null);
  useEffect(() => {
    let cancelled = false;
    void api
      .desktopApps()
      .then((apps) => {
        if (!cancelled) setDeskDoors(apps.filter((a) => a.installed));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);
  // Which Mac apps count as connected — MacConnect reads it, prompt-free.
  const [macConnected, setMacConnected] = useState<string[]>([]);
  useEffect(() => {
    let cancelled = false;
    void api
      .agentCliStatus()
      .then((clis) => {
        if (cancelled) return;
        setAgentDoors(clis.filter((c) => c.installed && c.id in AGENT_DOORS).map((c) => c.id));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  // Seed gateway drafts from config once it loads.
  useEffect(() => {
    if (aiConfig) {
      setGwUrl((v) => v || aiConfig.openaiBaseUrl);
      setGwKey((v) => v || aiConfig.openaiApiKey);
      setGwModel((v) => v || aiConfig.openaiChatModel);
      setGwVision((v) => v || aiConfig.openaiVisionModel);
    }
  }, [aiConfig]);

  // The on-device entry is always listed (`AiConfig::normalize`), so its
  // readiness is one probe away. It is slow enough to keep off the 4s tick:
  // read at mount, and again whenever the window comes back.
  const fmId = aiConfig?.providers.find((p) => p.kind === "fm")?.id ?? null;
  useEffect(() => {
    if (!fmId) return;
    let cancelled = false;
    const probe = () =>
      void api
        .providerReadinessOne(fmId)
        .then((r) => {
          if (!cancelled) setFmReady(r.ready);
        })
        .catch(() => {
          if (!cancelled) setFmReady(false);
        });
    probe();
    window.addEventListener("focus", probe);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", probe);
    };
  }, [fmId]);

  const provider = aiConfig?.provider ?? "ollama";
  // Which setup path the rows show. Apple Intelligence and the subscription
  // CLIs ride the modern chatProvider field; the flat provider string still
  // drives the two original paths (normalize mirrors them either way).
  const chosenKind = aiConfig?.providers.find(
    (p) => p.id === aiConfig.chatProvider,
  )?.kind;
  const agentMode = chosenKind && chosenKind in AGENT_DOORS ? chosenKind : null;
  const elsewhere = deskDoors.find((a) => a.id === aiConfig?.answersIn) ?? null;
  const mode: "fm" | "ollama" | "openai" | "agent" | "elsewhere" = elsewhere
    ? "elsewhere"
    : agentMode
      ? "agent"
      : aiConfig?.chatProvider === "on-device"
      ? "fm"
      : provider === "openai"
        ? "openai"
        : "ollama";
  // The radio selection IS the applied configuration: picking a row applies
  // it at once (as the old doors did), so the checks below always describe
  // the path the row claims.
  const path: AnswerPath =
    mode === "fm"
      ? "fm"
      : mode === "elsewhere"
        ? "elsewhere"
        : mode === "agent"
          ? "agent"
          : "byo";

  async function setMode(m: "fm" | "ollama" | "openai") {
    if (!aiConfig) return;
    // Any door but Ollama's indexes with the built-in embedder unless Ollama
    // is already running: a Mac without Ollama should never be told to
    // start it for a path that doesn't need it. Choosing a model here also
    // means answers happen in Alchemy again.
    const embedder = health?.reachable ? aiConfig.embedder : "builtin";
    const base = { ...aiConfig, answersIn: "" };
    if (m === "fm") await save({ ...base, chatProvider: "on-device", embedder });
    else if (m === "ollama")
      await save({ ...base, provider: "ollama", chatProvider: "ollama" });
    else await save({ ...base, provider: "openai", chatProvider: "", embedder });
    await refresh();
  }

  /** Answers happen in a desktop app; Alchemy indexes here and hands a
   *  notebook over on request. No model to install, nothing to sign in to. */
  async function answerElsewhere(id: string) {
    if (!aiConfig) return;
    await save({
      ...aiConfig,
      answersIn: id,
      setupSeen: true,
      embedder: health?.reachable ? aiConfig.embedder : "builtin",
    });
    await refresh();
  }

  // The gateway door is "open" once a key or URL has been saved; until then
  // the chat probe is answering for whatever provider normalize fell back
  // to, and its detail would describe the wrong door.
  const gatewayConfigured = !!(
    aiConfig?.openaiApiKey.trim() || aiConfig?.openaiBaseUrl.trim()
  );

  /** Answer with an installed subscription CLI — the same entry Settings →
   *  Models adds — and index with the built-in embedder, since this is the
   *  path for a Mac with no Ollama. */
  async function useAgent(id: string) {
    if (!aiConfig) return;
    const entry: ProviderEntry = {
      id,
      kind: id,
      label: AGENT_DOORS[id]?.label ?? id,
      baseUrl: "",
      apiKey: "",
      chatModel: "",
      effort: "",
    };
    // Match on KIND: an entry Settings made earlier may carry another id
    // ("claude" for Claude Code) and a chosen model — answer with that one
    // rather than minting a twin beside it.
    const existing = aiConfig.providers.find((p) => p.kind === id);
    const providers = existing
      ? aiConfig.providers
      : [...aiConfig.providers, entry];
    await save({
      ...aiConfig,
      providers,
      chatProvider: existing?.id ?? id,
      answersIn: "",
      embedder: health?.reachable ? aiConfig.embedder : "builtin",
    });
    await refresh();
  }

  async function saveGateway() {
    if (!aiConfig) return;
    setGwSaving(true);
    setGwStatus(null);
    let model = gwModel.trim();
    // No model chosen? Ask the gateway and auto-pick the first one.
    try {
      const models = await api.listGatewayModels(gwUrl.trim(), gwKey.trim());
      setGwModels(models.slice(0, 8));
      if (!model && models.length > 0) {
        model = models[0];
        setGwModel(model);
      }
    } catch (e) {
      setGwModels([]);
      setGwStatus(e instanceof Error ? e.message : String(e));
    }
    await save({
      ...aiConfig,
      provider: "openai",
      // Gateway-only mode: without Ollama, index sources with the built-in embedder.
      embedder: health?.reachable ? aiConfig.embedder : "builtin",
      openaiBaseUrl: gwUrl.trim(),
      openaiApiKey: gwKey.trim(),
      openaiChatModel: model,
      openaiVisionModel: gwVision.trim(),
    });
    if (model) {
      // Let the success state land before health flips the overlay away.
      setGwStatus(`Connected. Using ${model}.`);
      setGwSaving(false);
      await new Promise((r) => setTimeout(r, 1400));
    } else {
      setGwSaving(false);
    }
    await refresh();
  }

  /** Picking a row in step one applies that door straight away — the same
   *  call the old tiles made. A door this Mac doesn't have is unpickable,
   *  so there is no empty branch to fall through. */
  function pick(next: AnswerPath) {
    if (next === "fm") void setMode("fm");
    else if (next === "elsewhere") void answerElsewhere(deskDoors[0].id);
    else if (next === "agent") void useAgent(agentDoors[0]);
    else void setMode(gatewayConfigured ? "openai" : "ollama");
  }

  /** Leave now and set the rest up later — the old "Continue anyway". */
  function later() {
    dismiss();
  }

  /** Done: clear the dev-only `#onboarding` hash that forced this stage,
   *  then leave. Without the clear, a reload lands back here. */
  function finish() {
    if (window.location.hash === "#onboarding")
      window.history.replaceState(null, "", window.location.pathname);
    dismiss();
  }

  // Live-poll while visible so finishing a step ticks it off automatically.
  useEffect(() => {
    const t = setInterval(() => void refresh(), 4000);
    return () => clearInterval(t);
  }, [refresh]);

  // Continue changes the question, and a heading that swaps silently leaves a
  // screen reader on the old one. Move focus to it; the dialog is labelled by
  // it, so the new question is read out.
  const heading = useRef<HTMLHeadingElement | null>(null);
  useEffect(() => {
    if (stage > 0) heading.current?.focus();
  }, [stage]);

  if (!health) return null;
  const chat: ModelStatus = health.chat;
  const embed: ModelStatus = health.embed;
  const vision: ModelStatus = health.vision;

  const deskLabel = (a: DesktopApp) =>
    a.id === "claude" ? "Claude Desktop" : a.label;

  const answerOptions: readonly RadioOption<AnswerPath>[] = [
    {
      value: "fm",
      icon: <Cpu className="h-[18px] w-[18px]" />,
      title: "Apple Intelligence on this Mac",
      hint: "Private, nothing to install.",
      status: fmReady ? { label: "Ready", ok: true } : undefined,
    },
    {
      value: "elsewhere",
      icon: <MonitorSmartphone className="h-[18px] w-[18px]" />,
      title: "Claude Desktop, ChatGPT or Copilot",
      hint: "Answers there, notebooks here.",
      status:
        deskDoors.length > 0
          ? { label: `${deskLabel(deskDoors[0])} found`, ok: true }
          : { label: "None found" },
      disabled: deskDoors.length === 0,
    },
    {
      value: "agent",
      icon: <Terminal className="h-[18px] w-[18px]" />,
      title: "A coding agent you already pay for",
      hint: "Claude Code, Codex, Copilot CLI.",
      status:
        agentDoors.length > 0
          ? {
              label: `${AGENT_DOORS[agentDoors[0]].label} found`,
              ok: true,
            }
          : { label: "None found" },
      disabled: agentDoors.length === 0,
    },
    {
      value: "byo",
      icon: <Server className="h-[18px] w-[18px]" />,
      title: "Ollama or an API key",
      hint: "A local server, or a gateway with your key.",
      status: health.reachable ? { label: "Ollama running", ok: true } : undefined,
    },
  ];

  const titles = [
    "Where should answers come from?",
    "Which Mac apps should Alchemy read?",
    "Which agents can reach your notebooks?",
  ];
  const captions = [
    "Pick one. Every path indexes your sources on this Mac; you can change this later in Settings.",
    "Each one becomes a source that keeps itself current. Connecting asks macOS for permission once, up front.",
    "Agents can create notebooks, add sources, search, and write notes. The server listens on 127.0.0.1 only.",
  ];
  const previews = [
    "Mac apps and sharing come next.",
    "Agent access comes last.",
    "Everything here is also in Settings.",
  ];

  return (
    <div
      // Covers the whole app until setup is done, so it has to say so —
      // otherwise a screen reader walks straight into the inert UI behind it.
      role="dialog"
      aria-modal="true"
      aria-labelledby="onboarding-title"
      className="fixed inset-0 z-40 bg-background"
    >
      {/* Two panes, Setup Assistant style: the brand holds still on the left
          while the right pane asks one question at a time. Narrow windows
          stack them, the brand shrinking to a band. */}
      <div className="grid h-full grid-rows-[auto_minmax(0,1fr)] md:grid-cols-[300px_minmax(0,1fr)] md:grid-rows-1">
        <aside className="relative isolate flex items-center justify-center overflow-hidden border-b border-border bg-surface px-8 py-6 md:border-b-0 md:border-r md:py-10">
          {/* A faint wash of the accent, not a hero gradient. */}
          <div
            aria-hidden
            className="absolute inset-0 bg-[radial-gradient(ellipse_at_50%_40%,color-mix(in_srgb,var(--primary)_10%,transparent),transparent_62%)]"
          />
          <div className="relative z-10 flex flex-row items-center gap-5 md:flex-col md:gap-0">
            <AlchemySymbol
              className="h-14 w-14 shrink-0 text-citation/70 md:h-40 md:w-40"
              preferred={THEMES[resolveThemeId(theme)]?.sigil}
            />
            <div className="flex flex-col items-start md:mt-8 md:items-center">
              <span className="text-[1.25rem] font-semibold tracking-tight text-foreground">
                Alchemy
              </span>
              <p className="mt-1 max-w-[14rem] text-caption leading-relaxed text-muted-foreground md:text-center">
                Your sources, your notebook, your Mac.
              </p>
            </div>
          </div>
        </aside>

        <section className="flex min-h-0 flex-col">
          <div className="min-h-0 flex-1 overflow-y-auto">
            <div className="mx-auto flex w-full max-w-[640px] flex-col gap-5 px-6 py-8 md:px-10">
              <div className="flex flex-col gap-2">
                <Badge className="self-start">
                  Step {stage + 1} of {STEPS.length} · {STEPS[stage]}
                </Badge>
                <h1
                  id="onboarding-title"
                  ref={heading}
                  tabIndex={-1}
                  className="text-[1.25rem] font-semibold tracking-tight text-foreground outline-none"
                >
                  {titles[stage]}
                </h1>
                <p className="text-caption leading-relaxed text-muted-foreground">
                  {captions[stage]}
                </p>
              </div>

              {stage === 0 && (
                <>
                  <RadioList
                    label="Where answers come from"
                    value={path}
                    onChange={pick}
                    options={answerOptions}
                  />

                  {/* More than one of a kind on this Mac: name them, and let
                      the choice be made here rather than in Settings. */}
                  {path === "elsewhere" && deskDoors.length > 1 && (
                    <div className="flex flex-wrap items-center gap-1.5">
                      {deskDoors.map((a) => (
                        <Chip
                          key={a.id}
                          active={elsewhere?.id === a.id}
                          onClick={() => void answerElsewhere(a.id)}
                        >
                          {deskLabel(a)}
                        </Chip>
                      ))}
                    </div>
                  )}
                  {path === "agent" && agentDoors.length > 1 && (
                    <div className="flex flex-wrap items-center gap-1.5">
                      {agentDoors.map((id) => (
                        <Chip
                          key={id}
                          active={agentMode === id}
                          title={AGENT_DOORS[id].note}
                          onClick={() => void useAgent(id)}
                        >
                          {AGENT_DOORS[id].label}
                        </Chip>
                      ))}
                    </div>
                  )}
                  {path === "byo" && (
                    <Segmented
                      label="Local server or gateway"
                      value={mode === "openai" ? "openai" : "ollama"}
                      onChange={(v) => void setMode(v)}
                      options={[
                        { value: "ollama", label: "Ollama" },
                        { value: "openai", label: "API key" },
                      ]}
                      className="self-start"
                    />
                  )}

                  {mode === "openai" && (
                    <div className="flex flex-col gap-1.5 rounded-lg border border-border-strong bg-surface px-4 py-3">
                      <span className="text-body font-medium text-foreground">Gateway</span>
                      <Input
                        value={gwUrl}
                        onChange={(e) => setGwUrl(e.target.value)}
                        placeholder="Gateway URL (optional for OpenAI, Anthropic, OpenRouter, or Groq keys)"
                      />
                      <Input
                        type="password"
                        value={gwKey}
                        onChange={(e) => setGwKey(e.target.value)}
                        onFocus={(e) => e.currentTarget.select()}
                        placeholder="API key"
                      />
                      <Input
                        value={gwVision}
                        onChange={(e) => setGwVision(e.target.value)}
                        placeholder="Vision model for OCR (optional, e.g. gpt-4o)"
                      />
                      <div className="flex gap-1.5">
                        {gwModels.length > 0 ? (
                          <Select
                            value={gwModel}
                            onChange={setGwModel}
                            aria-label="Gateway model"
                            className="w-full"
                          >
                            {!gwModel && <option value="">Choose a model…</option>}
                            {(gwModels.includes(gwModel) || !gwModel
                              ? gwModels
                              : [gwModel, ...gwModels]
                            ).map((m) => (
                              <option key={m} value={m}>
                                {m}
                              </option>
                            ))}
                          </Select>
                        ) : (
                          <Input
                            value={gwModel}
                            onChange={(e) => setGwModel(e.target.value)}
                            placeholder="Model id"
                          />
                        )}
                        <Button
                          variant="primary"
                          size="sm"
                          className="shrink-0"
                          onClick={() => void saveGateway()}
                          loading={gwSaving}
                          disabled={!gwKey.trim() && !gwUrl.trim()}
                        >
                          Save & check
                        </Button>
                      </div>
                      <span
                        className={cn(
                          "text-micro",
                          gwStatus && !gwStatus.startsWith("Connected")
                            ? "text-destructive"
                            : gwStatus
                              ? "text-success"
                              : "text-subtle-foreground",
                        )}
                      >
                        {gwStatus
                          ? gwStatus
                          : "Stored locally; sent only to the gateway you configure."}
                      </span>
                    </div>
                  )}

                  {/* What the chosen door still needs. Every check the old
                      screen ran, in the same order, under the row that
                      asked for it. */}
                  <div className="flex flex-col gap-2">
                    {/* Only when something actually needs Ollama — chat in Ollama
                        mode, or the Ollama embedder. A goal-form title while
                        unchecked: a red ✗ beside "Ollama is running" read as the
                        app asserting a lie. */}
                    {(mode === "ollama" || aiConfig?.embedder === "ollama") && (
                      <Step
                        ok={health.reachable}
                        failed={mode === "ollama" && !health.reachable}
                        title={
                          health.reachable
                            ? "Ollama is running"
                            : mode === "ollama"
                              ? "Start Ollama"
                              : "Start Ollama (for source indexing)"
                        }
                        detail="Install Ollama, then start it. Alchemy connects to it locally."
                      >
                        <CommandChip command="brew install ollama" />
                        <CommandChip command="ollama serve" />
                        <button
                          className="text-caption text-citation hover:underline"
                          onClick={() => void openUrl("https://ollama.com/download")}
                        >
                          or download the app
                        </button>
                      </Step>
                    )}

                    <Step
                      ok={
                        mode === "elsewhere"
                          ? true
                          : mode === "ollama"
                            ? health.reachable && chat.working
                            : chat.working
                      }
                      failed={
                        mode === "elsewhere"
                          ? false
                          : mode === "openai"
                            ? gatewayConfigured && !chat.working
                            : mode === "ollama"
                              ? health.reachable && !chat.working
                              : !chat.working
                      }
                      title={
                        mode === "elsewhere"
                          ? `Answers in ${elsewhere!.label}`
                          : mode === "openai"
                            ? chat.working
                              ? "Gateway connected"
                              : "Connect a gateway"
                            : mode === "fm"
                              ? "Apple Intelligence"
                              : mode === "agent"
                                ? chat.working
                                  ? `${AGENT_DOORS[agentMode!]?.label} is signed in`
                                  : `Sign in to ${AGENT_DOORS[agentMode!]?.label}`
                                : chat.working
                                  ? "Chat model ready"
                                  : "Get a chat model"
                      }
                      detail={
                        mode === "openai"
                          ? gatewayConfigured
                            ? chat.detail
                            : "Paste an API key above and press Save & check."
                          : mode === "fm" || mode === "agent"
                            ? chat.detail
                            : health.reachable
                              ? `Answers questions and generates documents. ${chat.detail}`
                              : "Waiting for Ollama."
                      }
                    >
                      {mode === "ollama" && health.reachable && (
                        <CommandChip command={`ollama pull ${chat.name}`} />
                      )}
                      {mode === "ollama" && health.reachable && (
                        <button
                          className="text-caption text-citation hover:underline"
                          onClick={onOpenSettings}
                        >
                          or pick a smaller model
                        </button>
                      )}
                    </Step>

                    <Step
                      ok={embed.working}
                      failed={
                        aiConfig?.embedder === "builtin"
                          ? !embed.working && /fail|error|missing/i.test(embed.detail)
                          : health.reachable && !embed.working
                      }
                      title={
                        aiConfig?.embedder === "builtin"
                          ? "Built-in search model"
                          : "Search model"
                      }
                      detail={
                        aiConfig?.embedder === "builtin"
                          ? embed.detail
                          : health.reachable
                            ? `Indexes your sources for search (274 MB). ${embed.detail}`
                            : "Waiting for Ollama."
                      }
                    >
                      {aiConfig?.embedder !== "builtin" && health.reachable && (
                        <CommandChip command={`ollama pull ${embed.name}`} />
                      )}
                    </Step>

                    <Step
                      ok={mode === "openai" ? vision.working : health.reachable && vision.working}
                      optional
                      title="Vision model"
                      detail={
                        mode === "openai"
                          ? "Enables OCR for images and scanned PDFs. Set a vision-capable model (e.g. gpt-4o) in the Gateway box above."
                          : "Enables OCR for images and scanned PDFs. Skip it if you don't need that."
                      }
                    >
                      {mode !== "openai" && health.reachable && (
                        <CommandChip command={`ollama pull ${vision.name || "glm-ocr"}`} />
                      )}
                    </Step>
                  </div>

                  <button
                    className="text-left text-caption text-subtle-foreground hover:text-muted-foreground"
                    onClick={onOpenSettings}
                  >
                    {agentDoors.length > 0 || deskDoors.length > 0
                      ? "More providers in Settings → Models…"
                      : "Already pay for Claude or ChatGPT? Connect a subscription in Settings → Models…"}
                  </button>

                  <div className="flex items-center justify-between gap-3">
                    <span className="text-caption text-subtle-foreground">
                      Rechecks automatically every few seconds.
                    </span>
                    <Button
                      variant="ghost"
                      size="sm"
                      loading={checking}
                      onClick={async () => {
                        setChecking(true);
                        await refresh();
                        setChecking(false);
                      }}
                    >
                      {!checking && <RefreshCw className="h-3.5 w-3.5" />}
                      Recheck
                    </Button>
                  </div>
                </>
              )}

              {stage === 1 && (
                <RowGroup>
                  {macAvailable ? (
                    <MacConnect layout="rows" onStatus={setMacConnected} />
                  ) : (
                    <div className="px-3.5 py-3 text-caption text-subtle-foreground">
                      Mac apps are unavailable on this machine.
                    </div>
                  )}
                </RowGroup>
              )}

              {stage === 2 && <AgentRows />}
            </div>
          </div>

          <footer className="flex shrink-0 items-center justify-between gap-3 border-t border-border px-6 py-3 md:px-10">
            <div className="flex min-w-0 items-center gap-2">
              {stage > 0 && (
                <Button variant="ghost" size="sm" onClick={() => setStage(stage - 1)}>
                  Back
                </Button>
              )}
              <span className="truncate text-caption text-subtle-foreground">
                {stage === 1 && macConnected.length > 0
                  ? `${macConnected.length} of 4 connected. ${previews[stage]}`
                  : previews[stage]}
              </span>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <Button variant="secondary" size="sm" onClick={later}>
                Set Up Later
              </Button>
              {stage < STEPS.length - 1 ? (
                <Button variant="primary" size="sm" onClick={() => setStage(stage + 1)}>
                  Continue
                </Button>
              ) : (
                <Button variant="primary" size="sm" onClick={finish}>
                  Open Alchemy
                </Button>
              )}
            </div>
          </footer>
        </section>
      </div>
    </div>
  );
}
