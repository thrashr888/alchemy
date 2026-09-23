import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { AlchemySymbol } from "./AlchemyHero";
import { DitherBackground } from "./DitherBackground";
import { THEMES, resolveThemeId } from "@/lib/themes";
import { currentEpigraph } from "@/lib/epigraph";
import { MacConnect } from "./MacConnect";
import { Button, Input, Select } from "./ui";
import { cn } from "@/lib/utils";
import type { DesktopApp, ModelStatus, ProviderEntry } from "@/lib/types";
import { Check, Copy, CheckCircle2, XCircle, Circle, RefreshCw } from "lucide-react";

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

/** One door on the first-run screen: a way Alchemy can answer. */
function Door({
  label,
  note,
  pressed,
  onPick,
}: {
  label: string;
  note: string;
  pressed: boolean;
  onPick: () => void;
}) {
  return (
    <button
      type="button"
      aria-pressed={pressed}
      onClick={onPick}
      className={cn(
        "flex flex-col items-start gap-0.5 rounded-lg border px-3 py-2 text-left transition-colors",
        pressed
          ? "border-primary/60 bg-primary/10 text-foreground"
          : "border-border bg-surface text-muted-foreground hover:text-foreground",
      )}
    >
      <span className="text-body font-medium">{label}</span>
      <span className="text-micro text-subtle-foreground">{note}</span>
    </button>
  );
}

/** First-run / broken-setup guide: Ollama + required models, with live rechecks. */
export function Onboarding({ onOpenSettings }: { onOpenSettings: () => void }) {
  const health = useStore((s) => s.modelHealth);
  const theme = useStore((s) => s.theme);
  const aiConfig = useStore((s) => s.aiConfig);
  const save = useStore((s) => s.saveAiConfig);
  const dismiss = useStore((s) => s.dismissOnboarding);
  const refresh = useStore((s) => s.refreshModelHealth);
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

  const provider = aiConfig?.provider ?? "ollama";
  // Which setup path the tiles show. Apple Intelligence and the subscription
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

  // Live-poll while visible so finishing a step ticks it off automatically.
  useEffect(() => {
    const t = setInterval(() => void refresh(), 4000);
    return () => clearInterval(t);
  }, [refresh]);

  if (!health) return null;
  const chat: ModelStatus = health.chat;
  const embed: ModelStatus = health.embed;
  const vision: ModelStatus = health.vision;

  return (
    <div
      // Covers the whole app until setup is done, so it has to say so —
      // otherwise a screen reader walks straight into the inert UI behind it.
      role="dialog"
      aria-modal="true"
      aria-labelledby="onboarding-title"
      className="fixed inset-0 z-40 bg-background"
    >
      {/* Two panes: the brand on the left — the hero's dithered mist, the
          transmutation sigil large enough to be the thing you look at while
          a check runs, the mark and the wordmark — and the setup on the
          right in its own scroller, so a short window scrolls the steps
          from the top and never clips the heading. Narrow windows stack
          them, the brand shrinking to a band. */}
      <div className="grid h-full grid-rows-[auto_minmax(0,1fr)] md:grid-cols-[minmax(300px,2fr)_minmax(0,3fr)] md:grid-rows-1">
        <aside className="relative isolate flex min-h-[132px] items-center justify-center overflow-hidden border-b border-border md:min-h-0 md:border-b-0 md:border-r">
          <div className="absolute inset-0">
            <DitherBackground themeKey={theme} />
          </div>
          <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_center,transparent_55%,var(--background)_130%)]" />
          <div className="relative z-10 flex flex-row items-center gap-5 px-8 py-5 text-center md:flex-col md:gap-0 md:py-10">
            <AlchemySymbol
              className="h-16 w-16 shrink-0 text-citation/70 md:h-52 md:w-52 lg:h-64 lg:w-64"
              preferred={THEMES[resolveThemeId(theme)]?.sigil}
            />
            <div className="flex flex-col items-start md:mt-9 md:items-center">
              <div className="flex items-center gap-2.5">
                <AlchemySymbol className="hidden h-5 w-5 text-citation md:block" strokeWidth={1.6} />
                <span className="font-serif text-[1.375rem] font-medium uppercase tracking-[0.22em] text-foreground/90 md:text-3xl">
                  Alchemy
                </span>
              </div>
              <p className="mt-1.5 max-w-xs text-body leading-relaxed text-muted-foreground md:mt-3 md:text-center">
                Research notebooks that stay on your Mac.
              </p>
              <p className="mt-2 hidden max-w-xs font-serif text-body italic leading-relaxed text-subtle-foreground md:block">
                “{currentEpigraph(theme)}”
              </p>
            </div>
          </div>
        </aside>

        <section className="overflow-y-auto">
          <div className="flex min-h-full items-center">
      <div className="mx-auto flex w-full max-w-[560px] flex-col gap-5 px-6 py-8 md:px-10 md:py-12">
        <div className="flex flex-col gap-2">
          <h1
            id="onboarding-title"
            className="font-serif text-[1.625rem] font-medium tracking-[0.14em] text-foreground"
          >
            Set up Alchemy
          </h1>
          <p className="max-w-md text-body leading-relaxed text-muted-foreground">
            {mode === "openai" ? (
              <>
                Connect an OpenAI-compatible gateway. Your sources are indexed
                locally; only your chat prompts are sent to the gateway.
              </>
            ) : mode === "fm" ? (
              <>
                Answers come from Apple Intelligence, on this Mac. Nothing to
                install; nothing leaves your computer.
              </>
            ) : mode === "agent" ? (
              <>
                Answers come from {AGENT_DOORS[agentMode!]?.label}, already
                signed in on this Mac. Your sources are indexed locally; only
                your questions go to {AGENT_DOORS[agentMode!]?.vendor}.
              </>
            ) : mode === "elsewhere" ? (
              <>
                Answers happen in {elsewhere!.label}. Alchemy indexes your
                sources on this Mac and hands a notebook over whenever you
                ask — nothing to install, nothing to sign in to.
              </>
            ) : (
              <>
                Alchemy runs entirely on your machine. It needs{" "}
                <button
                  className="text-citation hover:underline"
                  onClick={() => void openUrl("https://ollama.com")}
                >
                  Ollama
                </button>{" "}
                and two local models. Nothing leaves your computer.
              </>
            )}
          </p>
        </div>

        {/* The three broadest doors. */}
        <div className="grid grid-cols-3 gap-1.5">
          {(
            [
              { id: "fm", label: "Apple Intelligence", note: "On-device · zero setup" },
              { id: "ollama", label: "Ollama", note: "Local models · private" },
              { id: "openai", label: "OpenAI-compatible", note: "Your API key · 30+ services" },
            ] as const
          ).map((pv) => (
            <Door
              key={pv.id}
              label={pv.label}
              note={pv.note}
              pressed={mode === pv.id}
              onPick={() => void setMode(pv.id)}
            />
          ))}
        </div>
        {/* The subscription CLIs this Mac already has, on their own row: the
            probe lands a second or two after the overlay, and a fourth tile
            arriving inside the grid above reflowed the three. The full
            roster still lives in Settings → Models. */}
        {agentDoors.length > 0 || deskDoors.length > 0 ? (
          <div className="-mt-2 flex flex-col gap-1.5">
            <span className="text-caption text-subtle-foreground">
              Already on this Mac:
            </span>
            <div className="grid grid-cols-3 gap-1.5">
              {agentDoors.map((id) => (
                <Door
                  key={id}
                  label={AGENT_DOORS[id].label}
                  note={AGENT_DOORS[id].note}
                  pressed={agentMode === id}
                  onPick={() => void useAgent(id)}
                />
              ))}
              {/* A desktop app has no local API, so this door is a different
                  promise: answers happen there, Alchemy keeps the notebook. */}
              {deskDoors.map((a) => (
                <Door
                  key={`desk-${a.id}`}
                  label={a.id === "claude" ? "Claude Desktop" : a.label}
                  note="Answers there · notebooks here"
                  pressed={mode === "elsewhere" && elsewhere?.id === a.id}
                  onPick={() => void answerElsewhere(a.id)}
                />
              ))}
            </div>
          </div>
        ) : (
          <button
            className="-mt-3 text-center text-caption text-subtle-foreground hover:text-muted-foreground"
            onClick={onOpenSettings}
          >
            Already pay for Claude or ChatGPT? Connect a subscription in
            Settings → Models…
          </button>
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
                  {(gwModels.includes(gwModel) || !gwModel ? gwModels : [gwModel, ...gwModels]).map(
                    (m) => (
                      <option key={m} value={m}>
                        {m}
                      </option>
                    ),
                  )}
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

        <div className="flex flex-col gap-2">
          {/* Only when something actually needs Ollama — chat in Ollama mode,
              or the Ollama embedder. A goal-form title while unchecked: a red
              ✗ beside "Ollama is running" read as the app asserting a lie. */}
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
              <button className="text-caption text-citation hover:underline" onClick={onOpenSettings}>
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
            title={aiConfig?.embedder === "builtin" ? "Built-in search model" : "Search model"}
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

          <Step
            ok={macConnected.length >= 4}
            optional
            title={
              macConnected.length >= 4
                ? "Mac apps connected"
                : macConnected.length > 0
                  ? `${macConnected.length} of 4 Mac apps connected`
                  : "Connect Mac apps"
            }
            detail="Add Calendar, Reminders, and Apple Notes as auto-syncing sources. Connecting triggers the macOS permission prompts once, up front."
          >
            <MacConnect onStatus={setMacConnected} />
          </Step>
        </div>

        <div className="flex items-center justify-between">
          <span className="text-[0.71875rem] text-subtle-foreground">
            Rechecks automatically every few seconds.
          </span>
          <div className="flex items-center gap-2">
            {/* Ready is a door, not a dead end: the overlay leaves on its
                own once health agrees, but a person who just watched the
                last step go green wants the button that says so. In dev,
                `#onboarding` forced the stage; the button clears it. */}
            {(chat.working || mode === "elsewhere") && embed.working ? (
              <Button
                variant="primary"
                size="sm"
                onClick={() => {
                  if (window.location.hash === "#onboarding")
                    window.history.replaceState(null, "", window.location.pathname);
                  dismiss();
                }}
              >
                Open Alchemy
              </Button>
            ) : (
              <Button variant="ghost" size="sm" onClick={dismiss}>
                Continue anyway
              </Button>
            )}
            <Button
              variant="secondary"
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
        </div>
      </div>
          </div>
        </section>
      </div>
    </div>
  );
}
