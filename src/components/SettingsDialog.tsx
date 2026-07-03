import { useEffect, useState, type ReactNode } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { THEME_LIST } from "@/lib/themes";
import { Button, Input, Modal, Spinner, Textarea } from "./ui";
import { cn } from "@/lib/utils";
import type { AiConfig, ChatConfig } from "@/lib/types";
import {
  RefreshCw,
  CheckCircle2,
  XCircle,
  Check,
  Circle,
  Zap,
  Cpu,
  MessageSquare,
  Palette,
} from "lucide-react";

/** Treat `name` and `name:latest` as the same model for matching. */
const normModel = (m: string) => m.replace(/:latest$/, "");

// MLX-accelerated and lean-active MoE chat models worth benchmarking locally.
// Compare their recorded tok/s in the status box above against your current one.
const SUGGESTED_CHAT = [
  { name: "qwen3.5:35b-a3b-coding-nvfp4", note: "MLX/NVFP4 · 3B active · fastest" },
  { name: "gemma4:31b-mlx", note: "MLX · 31B · vision · 256K ctx" },
  { name: "gemma4:12b-mlx", note: "MLX · 12B · vision · lighter" },
  { name: "gpt-oss:120b", note: "120B MoE · ~5B active · strong all-rounder" },
  { name: "nemotron-3-super", note: "120B MoE · only 12B active" },
  { name: "deepseek-v4-flash", note: "284B MoE · only 13B active" },
];

// Vision models for OCR — dedicated OCR models first (best for documents),
// then general vision models.
const SUGGESTED_VISION = [
  { name: "glm-ocr", note: "dedicated OCR · tiny 0.9B · fast" },
  { name: "deepseek-ocr", note: "dedicated OCR · markdown output · 3B" },
  { name: "gemma4:12b-mlx", note: "MLX · general vision" },
  { name: "minimax-m3", note: "vision · 1M context (general)" },
];

/** Targeting Bob (default or explicit) with a key that doesn't look like one. */
function keyLooksOff(draft: { openaiBaseUrl: string; openaiApiKey: string }): boolean {
  const bobTarget = !draft.openaiBaseUrl.trim() || draft.openaiBaseUrl.includes("bob.ibm.com");
  const key = draft.openaiApiKey.trim();
  return bobTarget && key.length > 0 && !key.startsWith("bob_");
}

const TABS = [
  { id: "models", label: "Models", icon: Cpu },
  { id: "chat", label: "Chat", icon: MessageSquare },
  { id: "appearance", label: "Appearance", icon: Palette },
];

export function SettingsDialog({
  open,
  onClose,
  initialTab = "models",
}: {
  open: boolean;
  onClose: () => void;
  initialTab?: string;
}) {
  const aiConfig = useStore((s) => s.aiConfig);
  const save = useStore((s) => s.saveAiConfig);
  const reembedAll = useStore((s) => s.reembedAll);
  const refreshModelHealth = useStore((s) => s.refreshModelHealth);
  const totalSources = useStore((s) => s.notebooks.reduce((sum, n) => sum + n.sourceCount, 0));

  const [tab, setTab] = useState(initialTab);
  const [draft, setDraft] = useState<AiConfig | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [connOk, setConnOk] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmReembed, setConfirmReembed] = useState(false);
  const [gatewayModels, setGatewayModels] = useState<string[]>([]);
  const [gatewayError, setGatewayError] = useState<string | null>(null);
  const [loadingGateway, setLoadingGateway] = useState(false);

  useEffect(() => {
    if (open) setTab(initialTab);
  }, [open, initialTab]);

  useEffect(() => {
    if (open && aiConfig) {
      setDraft({ ...aiConfig });
      void refreshModels();
      void refreshModelHealth();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, aiConfig]);

  async function refreshModels() {
    setLoadingModels(true);
    try {
      const list = await api.listModels();
      setModels(list);
      setConnOk(true);
    } catch {
      setModels([]);
      setConnOk(false);
    } finally {
      setLoadingModels(false);
    }
  }

  async function loadGatewayModels() {
    if (!draft?.openaiBaseUrl) return;
    setLoadingGateway(true);
    setGatewayError(null);
    try {
      setGatewayModels(await api.listGatewayModels(draft.openaiBaseUrl, draft.openaiApiKey));
    } catch (e) {
      setGatewayModels([]);
      setGatewayError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingGateway(false);
    }
  }

  const embedChanged =
    !!draft && normModel(draft.embedModel) !== normModel(aiConfig?.embedModel ?? "");

  async function onSave() {
    if (!draft) return;
    // Switching the embedding model invalidates existing vectors — re-embed.
    if (embedChanged && totalSources > 0) {
      setConfirmReembed(true);
      return;
    }
    setSaving(true);
    let toSave = draft;
    // Gateway provider with no model picked: ask the gateway and take the first.
    if (draft.provider === "openai" && !draft.openaiChatModel.trim()) {
      try {
        const models = await api.listGatewayModels(draft.openaiBaseUrl, draft.openaiApiKey);
        if (models.length > 0) {
          toSave = { ...draft, openaiChatModel: models[0] };
          setDraft(toSave);
        }
      } catch {
        /* health will surface the gateway error */
      }
    }
    await save(toSave);
    setSaving(false);
    onClose();
  }

  async function confirmSwitch() {
    if (!draft) return;
    setConfirmReembed(false);
    setSaving(true);
    await save(draft);
    setSaving(false);
    onClose();
    await reembedAll();
  }

  function cancelSwitch() {
    // Keep the previous embedding model so we never leave a broken index.
    setConfirmReembed(false);
    if (draft && aiConfig) setDraft({ ...draft, embedModel: aiConfig.embedModel });
  }

  if (!draft) {
    return (
      <Modal open={open} onClose={onClose} title="Settings">
        <div className="flex items-center justify-center py-8">
          <Spinner className="h-5 w-5 text-muted-foreground" />
        </div>
      </Modal>
    );
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Settings"
      width="max-w-2xl"
      footer={
        tab === "models" ? (
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" onClick={onSave} loading={saving}>
              Save
            </Button>
          </div>
        ) : undefined
      }
    >
      <div className="flex gap-5">
        <nav className="flex w-36 shrink-0 flex-col gap-0.5">
          {TABS.map((t) => (
            <button
              key={t.id}
              onClick={() => setTab(t.id)}
              className={cn(
                "flex items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12.5px] transition-colors",
                tab === t.id
                  ? "bg-surface-2 font-medium text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              <t.icon className="h-3.5 w-3.5" />
              {t.label}
            </button>
          ))}
        </nav>

        <div className="flex min-w-0 flex-1 flex-col gap-4">
          {tab === "models" && (
            <>
              <StatusBox
                connOk={connOk}
                modelCount={models.length}
                chatModel={draft.provider === "openai" ? draft.openaiChatModel : draft.chatModel}
                loading={loadingModels}
                onRefresh={() => {
                  void refreshModels();
                  void refreshModelHealth();
                }}
              />

              <Field label="AI provider" hint="Where chat and document generation run.">
                <div className="grid grid-cols-2 gap-1.5">
                  {[
                    { id: "ollama", label: "Ollama", note: "Local & private" },
                    { id: "openai", label: "IBM Bob / OpenAI-compatible", note: "Enterprise gateway" },
                  ].map((pv) => (
                    <button
                      key={pv.id}
                      onClick={() => setDraft({ ...draft, provider: pv.id })}
                      className={cn(
                        "flex flex-col items-start gap-0.5 rounded-md border px-3 py-2 text-left transition-colors",
                        draft.provider === pv.id
                          ? "border-primary/60 bg-primary/10 text-foreground"
                          : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
                      )}
                    >
                      <span className="text-[12.5px] font-medium">{pv.label}</span>
                      <span className="text-[11px] text-subtle-foreground">{pv.note}</span>
                    </button>
                  ))}
                </div>
              </Field>

              {draft.provider === "openai" && (
                <>
                  <Field
                    label="Gateway URL"
                    hint="Leave empty to use IBM Bob's default gateway. Any OpenAI-compatible base URL works (usually ends in /v1)."
                  >
                    <Input
                      value={draft.openaiBaseUrl}
                      onChange={(e) => setDraft({ ...draft, openaiBaseUrl: e.target.value })}
                      placeholder="https://api.us-east.bob.ibm.com/inference/v1 (default)"
                    />
                  </Field>
                  <Field
                    label="API key"
                    hint={
                      keyLooksOff(draft)
                        ? "Heads up: Bob keys start with bob_ — double-check you pasted the whole key."
                        : "Stored locally in your config file; sent only to the gateway."
                    }
                  >
                    <Input
                      type="password"
                      value={draft.openaiApiKey}
                      onChange={(e) => setDraft({ ...draft, openaiApiKey: e.target.value })}
                      placeholder="bob_prod_…"
                    />
                  </Field>
                  <Field
                    label="Gateway chat model"
                    hint={
                      gatewayError
                        ? `Couldn't list models: ${gatewayError}`
                        : "The model id billed to your account (bobcoins on Bob)."
                    }
                  >
                    <div className="flex flex-col gap-1.5">
                      <div className="flex gap-1.5">
                        <Input
                          value={draft.openaiChatModel}
                          onChange={(e) => setDraft({ ...draft, openaiChatModel: e.target.value })}
                          placeholder="model id"
                        />
                        <Button
                          variant="secondary"
                          onClick={() => void loadGatewayModels()}
                          loading={loadingGateway}
                          title="List the gateway's models"
                        >
                          Load
                        </Button>
                      </div>
                      {gatewayModels.length > 0 && (
                        <div className="flex flex-wrap gap-1">
                          {gatewayModels.slice(0, 20).map((m) => (
                            <button
                              key={m}
                              onClick={() => setDraft({ ...draft, openaiChatModel: m })}
                              className={cn(
                                "rounded border px-1.5 py-0.5 text-[11px] transition-colors",
                                m === draft.openaiChatModel
                                  ? "border-primary/50 bg-primary/15 text-citation"
                                  : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
                              )}
                            >
                              {m}
                            </button>
                          ))}
                        </div>
                      )}
                    </div>
                  </Field>
                </>
              )}

              {draft.provider !== "openai" && (
                <Field label="Ollama URL">
                  <Input
                    value={draft.baseUrl}
                    onChange={(e) => setDraft({ ...draft, baseUrl: e.target.value })}
                    placeholder="http://localhost:11434"
                  />
                </Field>
              )}

              {draft.provider !== "openai" && (
                <Field
                  label="Chat model"
                  hint="Used to answer questions and generate documents. Models tagged nvfp4/mlx run on Ollama's MLX engine (Apple-Silicon accelerated)."
                >
                  <ModelPicker
                    value={draft.chatModel}
                    models={models}
                    onChange={(v) => setDraft({ ...draft, chatModel: v })}
                    suggestions={SUGGESTED_CHAT}
                  />
                </Field>
              )}

              <Field
                label="Embedding model"
                hint={
                  embedChanged && totalSources > 0
                    ? `Saving will re-embed all ${totalSources} source${totalSources === 1 ? "" : "s"} with this model.`
                    : draft.provider === "openai"
                      ? "Runs on local Ollama (localhost:11434) until the built-in embedder ships."
                      : "Used to index sources for retrieval. nomic-embed-text is recommended."
                }
              >
                <ModelPicker
                  value={draft.embedModel}
                  models={models}
                  onChange={(v) => setDraft({ ...draft, embedModel: v })}
                />
              </Field>

              <Field
                label="Vision model"
                hint="OCR for image & scanned-PDF sources. Dedicated OCR models (glm-ocr, deepseek-ocr) work best; leave blank to disable."
              >
                <ModelPicker
                  value={draft.visionModel ?? ""}
                  models={models}
                  onChange={(v) => setDraft({ ...draft, visionModel: v })}
                  suggestions={SUGGESTED_VISION}
                />
              </Field>
            </>
          )}

          {tab === "chat" && <ChatTab />}

          {tab === "appearance" && (
            <Field label="Theme" hint="Applies immediately.">
              <ThemePicker />
            </Field>
          )}
        </div>
      </div>

      <Modal open={confirmReembed} onClose={cancelSwitch} title="Switch embedding model?">
        <div className="flex flex-col gap-4">
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            Different embedding models produce incompatible vectors, so switching to{" "}
            <span className="font-medium text-foreground">{draft.embedModel}</span> requires
            re-embedding all{" "}
            <span className="font-medium text-foreground">{totalSources}</span> source
            {totalSources === 1 ? "" : "s"}. This runs locally and may take a moment.
          </p>
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={cancelSwitch}>
              Cancel
            </Button>
            <Button variant="primary" onClick={confirmSwitch}>
              Switch & re-embed
            </Button>
          </div>
        </div>
      </Modal>
    </Modal>
  );
}

const CHAT_STYLES = [
  {
    id: "default",
    label: "Default",
    hint: "Balanced, grounded answers for research and brainstorming.",
  },
  {
    id: "learning",
    label: "Learning Guide",
    hint: "Explains step by step, defines terms, builds intuition.",
  },
  { id: "custom", label: "Custom", hint: "Give your own goal, style, or role." },
] as const;

const CHAT_LENGTHS = [
  { id: "default", label: "Default" },
  { id: "longer", label: "Longer" },
  { id: "shorter", label: "Shorter" },
] as const;

/** Chat style & length for the current notebook; applies as you click. */
function ChatTab() {
  const chatConfig = useStore((s) => s.chatConfig);
  const setChatConfig = useStore((s) => s.setChatConfig);
  const currentId = useStore((s) => s.currentId);
  const notebook = useStore((s) => s.notebooks.find((n) => n.id === s.currentId));

  const apply = (patch: Partial<ChatConfig>) => setChatConfig({ ...chatConfig, ...patch });
  const styleHint = CHAT_STYLES.find((s) => s.id === chatConfig.style)?.hint;

  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-muted-foreground">
        {currentId ? (
          <>
            Tune how the assistant responds in{" "}
            <span className="font-medium text-foreground">{notebook?.title ?? "this notebook"}</span>
            . Changes apply immediately.
          </>
        ) : (
          "Open a notebook to tune its chat — each notebook keeps its own style."
        )}
      </p>

      <Field label="Conversational goal, style, or role">
        <div className="flex flex-wrap gap-1.5">
          {CHAT_STYLES.map((s) => (
            <Pill key={s.id} active={chatConfig.style === s.id} onClick={() => apply({ style: s.id })}>
              {s.label}
            </Pill>
          ))}
        </div>
        {styleHint && <span className="text-[11px] text-subtle-foreground">{styleHint}</span>}
        {chatConfig.style === "custom" && (
          <Textarea
            rows={4}
            className="mt-1"
            placeholder="e.g. Act as a skeptical peer reviewer; challenge claims and ask for evidence."
            value={chatConfig.customPrompt}
            onChange={(e) => apply({ customPrompt: e.target.value })}
          />
        )}
      </Field>

      <Field label="Response length">
        <div className="flex flex-wrap gap-1.5">
          {CHAT_LENGTHS.map((l) => (
            <Pill key={l.id} active={chatConfig.length === l.id} onClick={() => apply({ length: l.id })}>
              {l.label}
            </Pill>
          ))}
        </div>
      </Field>
    </div>
  );
}

function Pill({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "rounded-md border px-3 py-1.5 text-[12px] transition-colors",
        active
          ? "border-primary/60 bg-primary/15 text-citation"
          : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

function StatusBox({
  connOk,
  modelCount,
  chatModel,
  loading,
  onRefresh,
}: {
  connOk: boolean | null;
  modelCount: number;
  chatModel: string;
  loading: boolean;
  onRefresh: () => void;
}) {
  const health = useStore((s) => s.modelHealth);
  const stats = useStore((s) => s.modelStats);
  const chatStat = stats.find((s) => normModel(s.name) === normModel(chatModel));

  const Row = ({
    label,
    status,
    optional,
  }: {
    label: string;
    status?: { working: boolean; detail: string };
    optional?: boolean;
  }) => (
    <div className="flex items-center gap-2 text-[12px]">
      {status?.working ? (
        <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-success" />
      ) : optional ? (
        <Circle className="h-3.5 w-3.5 shrink-0 text-subtle-foreground" />
      ) : (
        <XCircle className="h-3.5 w-3.5 shrink-0 text-destructive" />
      )}
      <span className="w-12 shrink-0 text-muted-foreground">{label}</span>
      <span
        className={cn(
          "truncate",
          status?.working
            ? "text-foreground/80"
            : optional
              ? "text-muted-foreground"
              : "text-destructive",
        )}
      >
        {status?.detail ?? "Unknown"}
      </span>
    </div>
  );

  return (
    <div className="flex flex-col gap-2 rounded-md border border-border bg-surface-2 px-3 py-2.5">
      {/* Overall connection */}
      <div className="flex items-center gap-2 text-[12px]">
        {connOk === null ? (
          <Spinner className="h-3.5 w-3.5 text-muted-foreground" />
        ) : connOk ? (
          <CheckCircle2 className="h-4 w-4 text-success" />
        ) : (
          <XCircle className="h-4 w-4 text-destructive" />
        )}
        <span className={cn(connOk === false ? "text-destructive" : "text-muted-foreground")}>
          {connOk === null
            ? "Checking Ollama…"
            : connOk
              ? `Connected · ${modelCount} models available`
              : "Cannot reach Ollama. Is `ollama serve` running?"}
        </span>
        <Button
          variant="ghost"
          size="icon"
          className="ml-auto"
          onClick={onRefresh}
          loading={loading}
          title="Recheck"
        >
          {!loading && <RefreshCw className="h-3.5 w-3.5" />}
        </Button>
      </div>

      {connOk && (
        <div className="flex flex-col gap-1.5 border-t border-border pt-2">
          <Row label="Chat" status={health?.chat} />
          <Row label="Embed" status={health?.embed} />
          <Row label="Vision" status={health?.vision} optional />
          {chatStat && chatStat.samples > 0 && (
            <div className="flex items-center gap-2 text-[12px]">
              <Zap className="h-3.5 w-3.5 shrink-0 text-citation" />
              <span className="w-12 shrink-0 text-muted-foreground">Speed</span>
              <span className="text-foreground/80">
                ~{chatStat.avgTokensPerSec.toFixed(1)} tok/s avg
                <span className="text-subtle-foreground">
                  {" "}
                  · last {chatStat.lastTokensPerSec.toFixed(1)} · {chatStat.samples} run
                  {chatStat.samples === 1 ? "" : "s"}
                </span>
              </span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function ThemePicker() {
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  return (
    <div className="grid grid-cols-2 gap-1.5">
      {THEME_LIST.map((t) => {
        const active = t.id === theme;
        return (
          <button
            key={t.id}
            onClick={() => setTheme(t.id)}
            className={cn(
              "flex items-center gap-2 rounded-md border px-2 py-1.5 text-left text-[12px] transition-colors",
              active
                ? "border-primary/60 bg-primary/10 text-foreground"
                : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
            )}
          >
            {/* Live swatch built from the theme's own tokens. */}
            <span
              className="flex h-5 w-5 shrink-0 items-center justify-center rounded border"
              style={{ background: t.vars.background, borderColor: t.vars["border-strong"] }}
            >
              <span className="h-2.5 w-2.5 rounded-full" style={{ background: t.vars.primary }} />
            </span>
            <span className="flex-1 truncate">{t.label}</span>
            {active && <Check className="h-3.5 w-3.5 text-primary" />}
          </button>
        );
      })}
    </div>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-[12px] font-medium text-foreground">{label}</label>
      {children}
      {hint && <span className="text-[11px] text-subtle-foreground">{hint}</span>}
    </div>
  );
}

function ModelPicker({
  value,
  models,
  onChange,
  suggestions = [],
}: {
  value: string;
  models: string[];
  onChange: (v: string) => void;
  suggestions?: { name: string; note: string }[];
}) {
  // Suggestions the user hasn't pulled yet (installed ones already show below).
  const notInstalled = suggestions.filter(
    (s) => !models.some((m) => normModel(m) === normModel(s.name)),
  );
  return (
    <div className="flex flex-col gap-1.5">
      <Input value={value} onChange={(e) => onChange(e.target.value)} />
      {models.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {models.map((m) => (
            <button
              key={m}
              onClick={() => onChange(m)}
              className={cn(
                "rounded px-1.5 py-0.5 text-[11px] border transition-colors",
                normModel(m) === normModel(value)
                  ? "border-primary/50 bg-primary/15 text-citation"
                  : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
              )}
            >
              {m}
            </button>
          ))}
        </div>
      )}
      {notInstalled.length > 0 && (
        <div className="flex flex-col gap-1">
          <span className="text-[11px] uppercase tracking-wide text-subtle-foreground">
            Suggested · pull to use
          </span>
          <div className="flex flex-wrap gap-1">
            {notInstalled.map((s) => (
              <button
                key={s.name}
                onClick={() => onChange(s.name)}
                title={`${s.note} — run: ollama pull ${s.name}`}
                className="rounded border border-dashed border-border-strong bg-surface-2 px-1.5 py-0.5 text-[11px] text-muted-foreground transition-colors hover:text-foreground"
              >
                {s.name}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
