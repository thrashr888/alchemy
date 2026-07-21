import { useEffect, useRef, useState } from "react";
import { api } from "@/lib/api";
import { Button, Input, Modal } from "../ui";
import { Field } from "./SettingsTabs";
import { cn } from "@/lib/utils";
import type { AiConfig, ProviderEntry } from "@/lib/types";
import {
  ChevronRight,
  CheckCircle2,
  KeyRound,
  Laptop,
  Pencil,
  Server,
  Sparkles,
  Trash2,
} from "lucide-react";

/** Known OpenAI-compatible gateways: pick one, the URL fills. Custom stays
 *  for anything else. YOUR-… placeholders need their account segment edited.
 *  Notable absences: Bob (vendor policy — but Bob Shell is a subscription
 *  provider) and GitHub Copilot proper (its subscription rides the copilot
 *  CLI; GitHub Models is the PAT-keyed cousin). */
export const GATEWAY_PRESETS = [
  { name: "Anthropic", url: "https://api.anthropic.com/v1" },
  {
    name: "AWS Bedrock (us-east-1)",
    url: "https://bedrock-runtime.us-east-1.amazonaws.com/openai/v1",
  },
  {
    name: "Azure OpenAI (edit resource)",
    url: "https://YOUR-RESOURCE.openai.azure.com/openai/v1",
  },
  { name: "Cerebras", url: "https://api.cerebras.ai/v1" },
  {
    name: "Cloudflare Workers AI (edit account)",
    url: "https://api.cloudflare.com/client/v4/accounts/YOUR-ACCOUNT-ID/ai/v1",
  },
  { name: "Cohere", url: "https://api.cohere.ai/compatibility/v1" },
  { name: "DeepInfra", url: "https://api.deepinfra.com/v1/openai" },
  { name: "DeepSeek", url: "https://api.deepseek.com/v1" },
  { name: "DigitalOcean Gradient", url: "https://inference.do-ai.run/v1" },
  { name: "Fireworks", url: "https://api.fireworks.ai/inference/v1" },
  { name: "GitHub Models", url: "https://models.github.ai/inference" },
  {
    name: "Google Gemini (AI Studio)",
    url: "https://generativelanguage.googleapis.com/v1beta/openai",
  },
  { name: "Groq", url: "https://api.groq.com/openai/v1" },
  { name: "Hugging Face", url: "https://router.huggingface.co/v1" },
  { name: "LM Studio (local)", url: "http://localhost:1234/v1" },
  { name: "Meta Llama API", url: "https://api.llama.com/compat/v1" },
  { name: "MiniMax", url: "https://api.minimax.io/v1" },
  { name: "Mistral", url: "https://api.mistral.ai/v1" },
  { name: "Moonshot (Kimi)", url: "https://api.moonshot.ai/v1" },
  { name: "Nous (Hermes)", url: "https://inference-api.nousresearch.com/v1" },
  { name: "Novita", url: "https://api.novita.ai/v3/openai" },
  { name: "NVIDIA NIM", url: "https://integrate.api.nvidia.com/v1" },
  { name: "OpenAI", url: "https://api.openai.com/v1" },
  { name: "OpenCode Zen", url: "https://opencode.ai/zen/v1" },
  { name: "OpenRouter", url: "https://openrouter.ai/api/v1" },
  { name: "Perplexity", url: "https://api.perplexity.ai" },
  {
    name: "Qwen (DashScope intl)",
    url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
  },
  { name: "SambaNova", url: "https://api.sambanova.ai/v1" },
  { name: "Together", url: "https://api.together.xyz/v1" },
  { name: "Vercel AI Gateway", url: "https://ai-gateway.vercel.sh/v1" },
  { name: "xAI (Grok)", url: "https://api.x.ai/v1" },
  { name: "Z.ai (GLM)", url: "https://api.z.ai/api/paas/v4" },
];

/** Preferred default model per service so the wizard's model field arrives
 *  pre-picked instead of alphabetical luck. */
const RECOMMENDED_MODELS: Record<string, string> = {
  OpenAI: "gpt-5.6-terra",
  Anthropic: "claude-sonnet-5",
  Groq: "llama-4.1-70b",
  OpenRouter: "openrouter/auto",
  "NVIDIA NIM": "meta/llama-3.3-70b-instruct",
};

/** Fallback pick when no recommendation matches: catalogs like NVIDIA's list
 *  hundreds of models where the alphabetical first is a retired function that
 *  404s — prefer a current instruct model from a major namespace instead. */
function fallbackModel(sorted: string[]): string {
  const major = /^(meta|nvidia|mistralai|qwen|openai|google)\//;
  return (
    sorted.find((m) => major.test(m) && /instruct/i.test(m)) ??
    sorted.find((m) => /instruct|chat/i.test(m)) ??
    sorted[0] ??
    ""
  );
}

const AGENT_LABELS: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
  "gemini-cli": "Gemini CLI",
  "cursor-cli": "Cursor CLI",
  opencode: "OpenCode",
  copilot: "GitHub Copilot",
  hermes: "Hermes",
  "bob-shell": "Bob Shell",
};

type Readiness = { id: string; ready: boolean; detail: string };
type CliStatus = { id: string; installed: boolean; detail: string };

/** Settings → Model: first-run doors, the ready-list, the add wizard, and
 *  Advanced (studio/titles routing, embeddings, vision). Plumbing lives in
 *  the inference router; this pane is presentation over draft config. */
export function ModelsTab({
  draft,
  setDraft,
  commit,
  models,
}: {
  draft: AiConfig;
  setDraft: (c: AiConfig) => void;
  /** Persist immediately (first-run doors, wizard adds) — decisive actions
   *  shouldn't depend on finding the Save button. */
  commit: (c: AiConfig) => void;
  /** Ollama's local model list (for the local-server editor). */
  models: string[];
}) {
  const [readiness, setReadiness] = useState<Readiness[]>([]);
  const [clis, setClis] = useState<CliStatus[]>([]);
  const [wizard, setWizard] = useState<null | { editId?: string }>(null);
  const [advancedOpen, setAdvancedOpen] = useState(false);

  useEffect(() => {
    void api.providerReadiness().then(setReadiness).catch(() => {});
    void api.agentCliStatus().then(setClis).catch(() => {});
  }, [draft.providers.length]);

  const readyOf = (id: string) => readiness.find((r) => r.id === id);
  const installedClis = clis.filter((c) => c.installed);

  function choose(patch: Partial<AiConfig>) {
    commit({ ...draft, ...patch, setupSeen: true });
  }

  /** First-run: add the best detected subscription CLI and answer with it. */
  function useBestCli() {
    const best = installedClis[0];
    if (!best) return;
    const providers = draft.providers.some((p) => p.kind === best.id)
      ? draft.providers
      : [
          ...draft.providers,
          {
            id: best.id,
            kind: best.id,
            label: AGENT_LABELS[best.id] ?? best.id,
            baseUrl: "",
            apiKey: "",
            chatModel: "",
          },
        ];
    choose({ providers, chatProvider: best.id });
  }

  if (!draft.setupSeen) {
    return (
      <div className="flex flex-col gap-3">
        <div>
          <div className="text-[15px] font-semibold text-foreground">
            How should Alchemy answer?
          </div>
          <p className="mt-0.5 text-[12px] text-subtle-foreground">
            It already works — this just picks the brain. Change it anytime
            from the chat box.
          </p>
        </div>
        <FirstRunDoor
          icon={<Laptop className="h-4 w-4 text-muted-foreground" />}
          title="On this Mac"
          subtitle="Nothing to set up. Never leaves your computer."
          note={
            readyOf("on-device")?.ready
              ? "✓ ready"
              : readyOf("on-device")?.detail
          }
          noteOk={readyOf("on-device")?.ready ?? false}
          action="Use this"
          onAction={() => choose({ chatProvider: "on-device" })}
        />
        <FirstRunDoor
          icon={<Sparkles className="h-4 w-4 text-muted-foreground" />}
          title="Your subscriptions"
          subtitle="Already pay for Claude or ChatGPT? Use the account you're signed into."
          note={
            installedClis.length > 0
              ? `✓ Found on this Mac: ${installedClis
                  .map((c) => AGENT_LABELS[c.id] ?? c.id)
                  .join(" · ")}`
              : "None found on this Mac"
          }
          noteOk={installedClis.length > 0}
          recommended={installedClis.length > 0}
          action={
            installedClis.length > 0
              ? `Use ${AGENT_LABELS[installedClis[0].id] ?? installedClis[0].id}`
              : "See options…"
          }
          primary={installedClis.length > 0}
          onAction={() => {
            if (installedClis.length > 0) useBestCli();
            else {
              setDraft({ ...draft, setupSeen: true });
              setWizard({});
            }
          }}
        />
        <FirstRunDoor
          icon={<KeyRound className="h-4 w-4 text-muted-foreground" />}
          title="Your own key"
          subtitle="An API key from OpenAI, Gemini, Mistral — 30+ services."
          action="Add a key…"
          onAction={() => {
            setDraft({ ...draft, setupSeen: true });
            setWizard({});
          }}
        />
        <button
          type="button"
          onClick={() => choose({})}
          className="mt-1 text-center text-[12px] text-subtle-foreground hover:text-muted-foreground"
        >
          Skip — keep the automatic choice
        </button>
        {wizard && (
          <ProviderWizard
            draft={draft}
            commit={commit}
            clis={clis}
            models={models}
            editId={wizard.editId}
            onClose={() => setWizard(null)}
          />
        )}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-5">
      <Field
        label="Model"
        hint="Answers chat and writes studio documents. Change it anytime — here or from the chat box."
      >
        <div
          className="flex flex-col gap-1"
          role="radiogroup"
          aria-label="Model"
          onKeyDown={(e) => {
            if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
            e.preventDefault();
            const ids = draft.providers.map((p) => p.id);
            const i = ids.indexOf(draft.chatProvider);
            const next =
              ids[
                (i + (e.key === "ArrowDown" ? 1 : ids.length - 1)) % ids.length
              ];
            setDraft({ ...draft, chatProvider: next });
          }}
        >
          {draft.providers.map((p) => {
            const r = readyOf(p.id);
            const selected = draft.chatProvider === p.id;
            return (
              <div
                key={p.id}
                className={cn(
                  "group flex items-center gap-2.5 rounded-lg border px-2.5 py-2",
                  selected
                    ? "border-primary/60 bg-primary/10"
                    : "border-border bg-surface-2",
                )}
              >
                <button
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  onClick={() => setDraft({ ...draft, chatProvider: p.id })}
                  className="flex min-w-0 flex-1 items-center gap-2.5 text-left"
                >
                  <span
                    className={cn(
                      "h-3.5 w-3.5 shrink-0 rounded-full border",
                      selected
                        ? "border-[5px] border-[color:var(--primary)]"
                        : "border-subtle-foreground",
                    )}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="text-[13px] text-foreground">{p.label}</span>
                    <br />
                    <span className="block truncate text-[11px] text-subtle-foreground">
                      {r?.detail ?? "checking…"}
                    </span>
                  </span>
                </button>
                <span
                  className={cn(
                    "w-20 shrink-0 text-right text-[11px]",
                    r?.ready ? "text-success" : "text-subtle-foreground",
                  )}
                >
                  {r ? (r.ready ? "ready" : "unavailable") : ""}
                </span>
                <span className="flex w-[52px] shrink-0 items-center justify-end gap-0.5">
                {(p.kind === "gateway" || p.kind === "ollama") && (
                  <button
                    type="button"
                    aria-label={`Edit ${p.label}`}
                    onClick={() => setWizard({ editId: p.id })}
                    className="rounded p-1 text-subtle-foreground opacity-0 hover:bg-surface-2 hover:text-foreground group-focus-within:opacity-100 group-hover:opacity-100"
                  >
                    <Pencil className="h-3.5 w-3.5" />
                  </button>
                )}
                {p.kind !== "fm" && (
                  <button
                    type="button"
                    aria-label={`Remove ${p.label}`}
                    onClick={() => {
                      const providers = draft.providers.filter(
                        (x) => x.id !== p.id,
                      );
                      const fallback = providers[0]?.id ?? "";
                      setDraft({
                        ...draft,
                        providers,
                        chatProvider:
                          draft.chatProvider === p.id
                            ? fallback
                            : draft.chatProvider,
                        studioProvider:
                          draft.studioProvider === p.id
                            ? ""
                            : draft.studioProvider,
                      });
                    }}
                    className="rounded p-1 text-subtle-foreground opacity-0 hover:bg-surface-2 hover:text-destructive group-focus-within:opacity-100 group-hover:opacity-100"
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                )}
                </span>
              </div>
            );
          })}
          <button
            type="button"
            onClick={() => setWizard({})}
            className="mt-0.5 self-start text-[12px] text-citation hover:underline"
          >
            + Add a provider…
            <span className="ml-1.5 text-[11px] text-subtle-foreground no-underline">
              subscriptions, keys, local servers
            </span>
          </button>
        </div>
      </Field>

      <div>
        <button
          type="button"
          onClick={() => setAdvancedOpen((v) => !v)}
          aria-expanded={advancedOpen}
          className="flex items-center gap-1.5 text-[12px] text-muted-foreground hover:text-foreground"
        >
          <ChevronRight
            className={cn(
              "h-3.5 w-3.5 transition-transform duration-150",
              advancedOpen && "rotate-90",
            )}
          />
          Advanced
          <span className="text-[11px] text-subtle-foreground">
            studio & titles routing · embeddings · vision
          </span>
        </button>
        {advancedOpen && (
          <div className="mt-3 flex flex-col gap-5 pl-5">
            <Field
              label="Task routing"
              hint="Helper tasks run on the main model unless overridden."
            >
              <div className="flex flex-col gap-1.5">
                <div className="flex items-center gap-2">
                  <span className="w-24 shrink-0 text-[12.5px] text-foreground">
                    Studio
                  </span>
                  {draft.studioProvider === "" ? (
                    <>
                      <span className="flex-1 text-[12px] text-subtle-foreground">
                        use main model
                      </span>
                      <Button
                        variant="ghost"
                        onClick={() =>
                          setDraft({
                            ...draft,
                            studioProvider: draft.chatProvider,
                          })
                        }
                      >
                        Change
                      </Button>
                    </>
                  ) : (
                    <>
                      <select
                        aria-label="Studio provider"
                        value={draft.studioProvider}
                        onChange={(e) =>
                          setDraft({ ...draft, studioProvider: e.target.value })
                        }
                        className="h-7 flex-1 rounded-md border border-input bg-surface-2 px-2 text-[12.5px] text-foreground focus:outline-none"
                      >
                        {draft.providers.map((p) => (
                          <option key={p.id} value={p.id}>
                            {p.label}
                            {p.chatModel ? ` · ${p.chatModel}` : ""}
                          </option>
                        ))}
                      </select>
                      <Button
                        variant="ghost"
                        onClick={() => setDraft({ ...draft, studioProvider: "" })}
                      >
                        Set to main
                      </Button>
                    </>
                  )}
                </div>
                <div className="flex items-center gap-2">
                  <span className="w-24 shrink-0 text-[12.5px] text-foreground">
                    Titles
                  </span>
                  <span className="flex-1 text-[12px] text-subtle-foreground">
                    on-device Apple model · automatic, falls back to main
                  </span>
                </div>
              </div>
            </Field>
            <Field
              label="Embeddings"
              hint="Powers search and citations. Changing it re-indexes every source — deliberately separate from chat."
            >
              <div className="flex flex-col gap-1.5">
                <select
                  aria-label="Embedding engine"
                  value={draft.embedder}
                  onChange={(e) =>
                    setDraft({ ...draft, embedder: e.target.value })
                  }
                  className="h-8 rounded-md border border-input bg-surface-2 px-2 text-[13px] text-foreground focus:outline-none"
                >
                  <option value="builtin">Built-in (no setup, private)</option>
                  <option value="ollama">Ollama model</option>
                </select>
                {draft.embedder === "ollama" && (
                  <Input
                    aria-label="Embedding model"
                    value={draft.embedModel}
                    onChange={(e) =>
                      setDraft({ ...draft, embedModel: e.target.value })
                    }
                    placeholder="nomic-embed-text"
                  />
                )}
              </div>
            </Field>
            <Field
              label="Vision (OCR)"
              hint={
                draft.visionProvider === "ollama"
                  ? "Needs Ollama running with this model pulled."
                  : draft.visionProvider === "gateway"
                    ? "Uses your gateway key; pick a vision-capable model."
                    : "Reads images and scanned PDFs. Off means image sources are listed, not read."
              }
            >
              <div className="flex flex-col gap-1.5">
                <select
                  aria-label="Vision engine"
                  value={draft.visionProvider}
                  onChange={(e) =>
                    setDraft({ ...draft, visionProvider: e.target.value })
                  }
                  className="h-8 rounded-md border border-input bg-surface-2 px-2 text-[13px] text-foreground focus:outline-none"
                >
                  <option value="">Off</option>
                  <option value="ollama">Ollama model</option>
                  <option value="gateway">Gateway model</option>
                </select>
                {draft.visionProvider === "ollama" && (
                  <Input
                    aria-label="Vision model"
                    value={draft.visionModel}
                    onChange={(e) =>
                      setDraft({ ...draft, visionModel: e.target.value })
                    }
                    placeholder="glm-ocr · deepseek-ocr · gemma4:12b-mlx"
                  />
                )}
                {draft.visionProvider === "gateway" && (
                  <Input
                    aria-label="Vision model"
                    value={draft.openaiVisionModel}
                    onChange={(e) =>
                      setDraft({ ...draft, openaiVisionModel: e.target.value })
                    }
                    placeholder="a vision-capable gateway model"
                  />
                )}
              </div>
            </Field>
          </div>
        )}
      </div>

      {wizard && (
        <ProviderWizard
          draft={draft}
          commit={commit}
          clis={clis}
          models={models}
          editId={wizard.editId}
          onClose={() => setWizard(null)}
        />
      )}
    </div>
  );
}

function Select({
  ariaLabel,
  value,
  onChange,
  options,
}: {
  ariaLabel: string;
  value: string;
  onChange: (v: string) => void;
  options: string[];
}) {
  const list =
    options.includes(value) || !value ? options : [value, ...options];
  return (
    <select
      aria-label={ariaLabel}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="h-8 w-full appearance-none rounded-md border border-input bg-surface-2 px-2.5 text-[13px] text-foreground outline-none transition-colors focus:border-ring/60"
    >
      {!value && <option value="">Choose a model…</option>}
      {list.map((m) => (
        <option key={m} value={m}>
          {m}
        </option>
      ))}
    </select>
  );
}

function FirstRunDoor({
  icon,
  title,
  subtitle,
  note,
  noteOk,
  action,
  onAction,
  recommended,
  primary,
}: {
  icon: React.ReactNode;
  title: string;
  subtitle: string;
  note?: string;
  /** Colors the note as good news; false keeps it neutral (never
   *  color-only: good notes also carry a ✓ prefix). */
  noteOk?: boolean;
  action: string;
  onAction: () => void;
  recommended?: boolean;
  primary?: boolean;
}) {
  return (
    <div
      className={cn(
        "flex items-center gap-3 rounded-lg border px-3 py-2.5",
        recommended
          ? "border-primary/60 bg-primary/[0.08]"
          : "border-border bg-surface-2",
      )}
    >
      <span className="shrink-0">{icon}</span>
      <span className="min-w-0 flex-1">
        <span className="text-[13px] font-medium text-foreground">{title}</span>
        {recommended && (
          <span className="ml-1.5 rounded-full bg-primary/15 px-2 py-px text-[10.5px] text-citation">
            recommended
          </span>
        )}
        <span className="block text-[12px] text-muted-foreground">
          {subtitle}
        </span>
        {note && (
          <span
            className={cn(
              "block text-[11px]",
              noteOk ? "text-success" : "text-subtle-foreground",
            )}
          >
            {note}
          </span>
        )}
      </span>
      <Button variant={primary ? "primary" : "secondary"} onClick={onAction}>
        {action}
      </Button>
    </div>
  );
}

/** The add/edit wizard: three doors → a subscription pick (zero fields), a
 *  key path (pick service, paste key, model auto-picks), or a local server. */
function ProviderWizard({
  draft,
  commit,
  clis,
  models,
  editId,
  onClose,
}: {
  draft: AiConfig;
  commit: (c: AiConfig) => void;
  clis: CliStatus[];
  models: string[];
  editId?: string;
  onClose: () => void;
}) {
  const editing = draft.providers.find((p) => p.id === editId);
  const [step, setStep] = useState<"door" | "key" | "local" | "done">(
    editing ? (editing.kind === "ollama" ? "local" : "key") : "door",
  );
  const [service, setService] = useState(() => {
    const preset = GATEWAY_PRESETS.find((g) => g.url === editing?.baseUrl);
    return preset?.name ?? (editing ? "Custom" : "OpenAI");
  });
  const [customUrl, setCustomUrl] = useState(editing?.baseUrl ?? "");
  const [key, setKey] = useState(editing?.apiKey ?? "");
  const [model, setModel] = useState(editing?.chatModel ?? "");
  const [found, setFound] = useState<string[]>([]);
  const [checking, setChecking] = useState(false);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [localUrl, setLocalUrl] = useState(editing?.baseUrl || draft.baseUrl);
  const [localModel, setLocalModel] = useState(
    editing?.chatModel || draft.chatModel,
  );
  const [added, setAdded] = useState<ProviderEntry | null>(null);
  const debounce = useRef<number | null>(null);

  const baseUrl =
    service === "Custom"
      ? customUrl
      : (GATEWAY_PRESETS.find((g) => g.name === service)?.url ?? "");

  function probeKey(url: string, k: string) {
    if (debounce.current) window.clearTimeout(debounce.current);
    if (!url.trim() || !k.trim()) return;
    debounce.current = window.setTimeout(() => {
      setChecking(true);
      setKeyError(null);
      api
        .listGatewayModels(url, k)
        .then((list) => {
          const sorted = [...list].sort((a, b) => a.localeCompare(b));
          setFound(sorted);
          setModel(
            (m) =>
              m ||
              (RECOMMENDED_MODELS[service] &&
              sorted.includes(RECOMMENDED_MODELS[service])
                ? RECOMMENDED_MODELS[service]
                : fallbackModel(sorted)),
          );
        })
        .catch((e) => {
          setFound([]);
          setKeyError(e instanceof Error ? e.message : String(e));
        })
        .finally(() => setChecking(false));
    }, 600);
  }

  function finishAdd(entry: ProviderEntry, answerWithIt: boolean) {
    const providers = editing
      ? draft.providers.map((p) => (p.id === entry.id ? entry : p))
      : [...draft.providers, entry];
    const next = {
      ...draft,
      providers,
      setupSeen: true,
      chatProvider: answerWithIt ? entry.id : draft.chatProvider,
    };
    commit(next);
    onClose();
  }

  function addSubscription(cli: CliStatus) {
    const entry: ProviderEntry = {
      id: cli.id,
      kind: cli.id,
      label: AGENT_LABELS[cli.id] ?? cli.id,
      baseUrl: "",
      apiKey: "",
      chatModel: "",
    };
    setAdded(entry);
    setStep("done");
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={editing ? `Edit ${editing.label}` : "Add a provider"}
    >
      {step === "door" && (
        <div className="flex flex-col gap-2">
          <div className="text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
            A subscription on this Mac
          </div>
          {clis.filter((c) => c.installed).length === 0 && (
            <p className="text-[12px] text-subtle-foreground">
              None found — Claude, ChatGPT, Gemini, Cursor, and Bob appear here
              once their apps are installed and signed in.
            </p>
          )}
          {clis
            .filter((c) => c.installed)
            .map((c) => (
              <button
                type="button"
                key={c.id}
                onClick={() => addSubscription(c)}
                disabled={draft.providers.some((p) => p.kind === c.id)}
                className="flex items-center gap-2.5 rounded-lg border border-border bg-surface-2 px-3 py-2 text-left hover:border-border-strong disabled:opacity-50"
              >
                <span className="min-w-0 flex-1">
                  <span className="text-[13px] text-foreground">
                    {AGENT_LABELS[c.id] ?? c.id}
                  </span>
                  <span className="block truncate text-[11px] text-subtle-foreground">
                    {c.detail} · nothing to configure
                  </span>
                </span>
                {draft.providers.some((p) => p.kind === c.id) ? (
                  <span className="text-[11px] text-subtle-foreground">
                    added
                  </span>
                ) : (
                  <ChevronRight className="h-3.5 w-3.5 text-subtle-foreground" />
                )}
              </button>
            ))}
          <div className="mt-1 text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
            Something else
          </div>
          <button
            type="button"
            onClick={() => setStep("key")}
            className="flex items-center gap-2.5 rounded-lg border border-border bg-surface-2 px-3 py-2 text-left hover:border-border-strong"
          >
            <KeyRound className="h-4 w-4 shrink-0 text-muted-foreground" />
            <span className="flex-1 text-[13px] text-foreground">
              An API key
              <span className="block text-[11px] text-subtle-foreground">
                OpenAI, Gemini, Mistral — 30+ services
              </span>
            </span>
            <ChevronRight className="h-3.5 w-3.5 text-subtle-foreground" />
          </button>
          <button
            type="button"
            onClick={() => setStep("local")}
            className="flex items-center gap-2.5 rounded-lg border border-border bg-surface-2 px-3 py-2 text-left hover:border-border-strong"
          >
            <Server className="h-4 w-4 shrink-0 text-muted-foreground" />
            <span className="flex-1 text-[13px] text-foreground">
              A local server
              <span className="block text-[11px] text-subtle-foreground">
                Ollama, LM Studio, vLLM — anything OpenAI-compatible
              </span>
            </span>
            <ChevronRight className="h-3.5 w-3.5 text-subtle-foreground" />
          </button>
        </div>
      )}

      {step === "key" && (
        <div className="flex flex-col gap-3">
          <Field label="Service">
            <select
              aria-label="Service"
              value={service}
              onChange={(e) => {
                setService(e.target.value);
                setFound([]);
                setModel("");
                const url =
                  GATEWAY_PRESETS.find((g) => g.name === e.target.value)?.url ??
                  customUrl;
                probeKey(url, key);
              }}
              className="h-8 w-full rounded-md border border-input bg-surface-2 px-2 text-[13px] text-foreground focus:outline-none"
            >
              {GATEWAY_PRESETS.map((g) => (
                <option key={g.name} value={g.name}>
                  {g.name}
                </option>
              ))}
              <option value="Custom">Custom…</option>
            </select>
          </Field>
          {service === "Custom" && (
            <Field label="Base URL" hint="Any OpenAI-compatible endpoint, usually ending in /v1.">
              <Input
                aria-label="Base URL"
                value={customUrl}
                onChange={(e) => {
                  setCustomUrl(e.target.value);
                  probeKey(e.target.value, key);
                }}
                placeholder="https://api.example.com/v1"
              />
            </Field>
          )}
          <Field
            label="API key"
            hint={
              keyError
                ? `Couldn't reach it: ${keyError}`
                : checking
                  ? "Checking the key…"
                  : found.length > 0
                    ? `✓ key works — ${found.length} models found`
                    : "Stored locally in your config file; sent only to this service."
            }
          >
            <Input
              type="password"
              aria-label="API key"
              value={key}
              onChange={(e) => {
                setKey(e.target.value);
                probeKey(baseUrl, e.target.value);
              }}
              placeholder="paste your key"
            />
          </Field>
          <Field label="Model" hint="Picked for you — change it if you like.">
            {found.length > 0 ? (
              <Select
                ariaLabel="Model"
                value={model}
                onChange={setModel}
                options={found}
              />
            ) : (
              <Input
                aria-label="Model"
                value={model}
                onChange={(e) => setModel(e.target.value)}
                placeholder="model id"
              />
            )}
          </Field>
          <div className="flex items-center justify-between pt-1">
            <Button variant="ghost" onClick={() => (editing ? onClose() : setStep("door"))}>
              {editing ? "Cancel" : "‹ Back"}
            </Button>
            <Button
              variant="primary"
              disabled={!key.trim() || !baseUrl.trim()}
              onClick={() =>
                finishAdd(
                  {
                    id: editing?.id ?? `p${Date.now().toString(36)}`,
                    kind: "gateway",
                    label: service === "Custom" ? "Gateway" : service,
                    baseUrl,
                    apiKey: key,
                    chatModel: model,
                  },
                  !editing,
                )
              }
            >
              {editing ? "Save" : `Add ${service === "Custom" ? "gateway" : service}`}
            </Button>
          </div>
        </div>
      )}

      {step === "local" && (
        <div className="flex flex-col gap-3">
          <Field label="Server URL" hint="Ollama's default is already filled in.">
            <Input
              aria-label="Server URL"
              value={localUrl}
              onChange={(e) => setLocalUrl(e.target.value)}
              placeholder="http://localhost:11434"
            />
          </Field>
          <Field label="Model">
            {models.length > 0 ? (
              <Select
                ariaLabel="Local model"
                value={localModel}
                onChange={setLocalModel}
                options={models}
              />
            ) : (
              <Input
                aria-label="Local model"
                value={localModel}
                onChange={(e) => setLocalModel(e.target.value)}
                placeholder="gpt-oss:120b"
              />
            )}
          </Field>
          <div className="flex items-center justify-between pt-1">
            <Button variant="ghost" onClick={() => (editing ? onClose() : setStep("door"))}>
              {editing ? "Cancel" : "‹ Back"}
            </Button>
            <Button
              variant="primary"
              onClick={() =>
                finishAdd(
                  {
                    id: editing?.id ?? `p${Date.now().toString(36)}`,
                    kind: "ollama",
                    label: "Ollama",
                    baseUrl: localUrl,
                    apiKey: "",
                    chatModel: localModel,
                  },
                  !editing,
                )
              }
            >
              {editing ? "Save" : "Add local server"}
            </Button>
          </div>
        </div>
      )}

      {step === "done" && added && (
        <div className="flex flex-col gap-3">
          <div className="flex items-center gap-2.5">
            <CheckCircle2 className="h-4 w-4 shrink-0 text-success" />
            <span className="min-w-0 flex-1">
              <span className="text-[13px] font-medium text-foreground">
                {added.label} added
              </span>
              <span className="block text-[12px] text-subtle-foreground">
                signed in as you · nothing to configure
              </span>
            </span>
          </div>
          <div className="flex gap-2">
            <Button variant="primary" onClick={() => finishAdd(added, true)}>
              Answer chat with it
            </Button>
            <Button variant="secondary" onClick={() => finishAdd(added, false)}>
              Just keep it available
            </Button>
          </div>
        </div>
      )}
    </Modal>
  );
}
