import { useEffect, useState, type ReactNode } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import { SYSTEM_THEME, THEME_LIST, THEMES } from "@/lib/themes";
import { playDone } from "@/lib/sound";
import { checkForUpdates, type UpdateFlow } from "@/lib/updates";
import { Button, Input, Modal, Spinner, Textarea } from "./ui";
import { cn } from "@/lib/utils";
import { AlchemySymbol } from "./AlchemyHero";
import { MacConnect } from "./MacConnect";
import type {
  AiConfig,
  BuildInfo,
  ChatConfig,
  ConnectorStatus,
  McpStatus,
} from "@/lib/types";
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
  Keyboard,
  Info,
  Globe,
  SlidersHorizontal,
  UserRound,
  AudioLines,
  Trash2,
  Bot,
  Copy,
} from "lucide-react";

/** Treat `name` and `name:latest` as the same model for matching. */
const normModel = (m: string) => m.replace(/:latest$/, "");

// MLX-accelerated and lean-active MoE chat models worth benchmarking locally.
// Compare their recorded tok/s in the status box above against your current one.
const SUGGESTED_CHAT = [
  {
    name: "qwen3.5:35b-a3b-coding-nvfp4",
    note: "MLX/NVFP4 · 3B active · fastest",
  },
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

const TABS = [
  { id: "general", label: "General", icon: SlidersHorizontal },
  { id: "models", label: "Models", icon: Cpu },
  { id: "chat", label: "Chat", icon: MessageSquare },
  { id: "personalization", label: "Personalization", icon: UserRound },
  { id: "agents", label: "Agents", icon: Bot },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "shortcuts", label: "Shortcuts", icon: Keyboard },
  { id: "about", label: "About", icon: Info },
];

export function SettingsDialog({
  open,
  onClose,
  initialTab = "general",
}: {
  open: boolean;
  onClose: () => void;
  initialTab?: string;
}) {
  const aiConfig = useStore((s) => s.aiConfig);
  const save = useStore((s) => s.saveAiConfig);
  const reembedAll = useStore((s) => s.reembedAll);
  const refreshModelHealth = useStore((s) => s.refreshModelHealth);
  const totalSources = useStore((s) =>
    s.notebooks.reduce((sum, n) => sum + n.sourceCount, 0),
  );

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
      if (aiConfig.provider === "openai" && aiConfig.openaiApiKey) {
        void loadGatewayModels(aiConfig.openaiBaseUrl, aiConfig.openaiApiKey);
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, aiConfig]);

  async function refreshModels() {
    setLoadingModels(true);
    try {
      const list = await api.listModels();
      setModels([...list].sort((a, b) => a.localeCompare(b)));
      setConnOk(true);
    } catch {
      setModels([]);
      setConnOk(false);
    } finally {
      setLoadingModels(false);
    }
  }

  async function loadGatewayModels(baseUrl?: string, apiKey?: string) {
    const url = baseUrl ?? draft?.openaiBaseUrl ?? "";
    const key = apiKey ?? draft?.openaiApiKey ?? "";
    setLoadingGateway(true);
    setGatewayError(null);
    try {
      const gateway = await api.listGatewayModels(url, key);
      setGatewayModels([...gateway].sort((a, b) => a.localeCompare(b)));
    } catch (e) {
      setGatewayModels([]);
      setGatewayError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingGateway(false);
    }
  }

  const embedChanged =
    !!draft &&
    (draft.embedder !== (aiConfig?.embedder ?? "ollama") ||
      (draft.embedder === "ollama" &&
        normModel(draft.embedModel) !== normModel(aiConfig?.embedModel ?? "")));

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
        const models = await api.listGatewayModels(
          draft.openaiBaseUrl,
          draft.openaiApiKey,
        );
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
    if (draft && aiConfig)
      setDraft({ ...draft, embedModel: aiConfig.embedModel });
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
      {/* Fixed height: the nav stays put and only the tab content scrolls
          (also keeps the modal from resizing as tabs change). */}
      <div className="flex h-[60vh] gap-5">
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

        <div className="flex min-w-0 flex-1 flex-col gap-4 overflow-y-auto pr-1">
          {tab === "general" && <GeneralTab />}
          {tab === "models" && (
            <>
              <StatusBox
                connOk={connOk}
                provider={draft.provider}
                modelCount={models.length}
                chatModel={
                  draft.provider === "openai"
                    ? draft.openaiChatModel
                    : draft.chatModel
                }
                loading={loadingModels}
                onRefresh={() => {
                  void refreshModels();
                  void refreshModelHealth();
                }}
              />

              <Field
                label="AI provider"
                hint="Where chat and document generation run."
              >
                <div className="grid grid-cols-2 gap-1.5">
                  {[
                    { id: "ollama", label: "Ollama", note: "Local & private" },
                    {
                      id: "openai",
                      label: "OpenAI-compatible",
                      note: "Cloud or enterprise gateway",
                    },
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
                      <span className="text-[12.5px] font-medium">
                        {pv.label}
                      </span>
                      <span className="text-[11px] text-subtle-foreground">
                        {pv.note}
                      </span>
                    </button>
                  ))}
                </div>
              </Field>

              {draft.provider === "openai" && (
                <>
                  <Field
                    label="Gateway URL"
                    hint="Empty = inferred from your key for OpenAI, Anthropic, OpenRouter, and Groq. Any OpenAI-compatible base URL works (usually ends in /v1)."
                  >
                    <Input
                      value={draft.openaiBaseUrl}
                      onChange={(e) =>
                        setDraft({ ...draft, openaiBaseUrl: e.target.value })
                      }
                      placeholder="https://api.example.com/v1"
                    />
                  </Field>
                  <Field
                    label="API key"
                    hint="Stored locally in your config file; sent only to the gateway."
                  >
                    <Input
                      type="password"
                      value={draft.openaiApiKey}
                      onChange={(e) =>
                        setDraft({ ...draft, openaiApiKey: e.target.value })
                      }
                      placeholder="sk-… or your gateway's key format"
                    />
                  </Field>
                  <Field
                    label="Gateway chat model"
                    hint={
                      gatewayError
                        ? `Couldn't list models: ${gatewayError}`
                        : "The model id billed to your account."
                    }
                  >
                    <div className="flex gap-1.5">
                      {gatewayModels.length > 0 ? (
                        <Select
                          value={draft.openaiChatModel}
                          onChange={(v) =>
                            setDraft({ ...draft, openaiChatModel: v })
                          }
                          options={gatewayModels}
                        />
                      ) : (
                        <Input
                          value={draft.openaiChatModel}
                          onChange={(e) =>
                            setDraft({
                              ...draft,
                              openaiChatModel: e.target.value,
                            })
                          }
                          placeholder="model id"
                        />
                      )}
                      <Button
                        variant="secondary"
                        onClick={() => void loadGatewayModels()}
                        loading={loadingGateway}
                        title="Refresh the gateway's model list"
                      >
                        {gatewayModels.length > 0 ? "Refresh" : "Load"}
                      </Button>
                    </div>
                  </Field>
                </>
              )}

              {draft.provider !== "openai" && (
                <Field label="Ollama URL">
                  <Input
                    value={draft.baseUrl}
                    onChange={(e) =>
                      setDraft({ ...draft, baseUrl: e.target.value })
                    }
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
                label="Embeddings"
                hint={
                  embedChanged && totalSources > 0
                    ? `Saving will re-embed all ${totalSources} source${totalSources === 1 ? "" : "s"}.`
                    : "How sources are indexed for retrieval. Built-in needs no Ollama and runs instantly on CPU."
                }
              >
                <div className="grid grid-cols-2 gap-1.5">
                  {[
                    {
                      id: "builtin",
                      label: "Built-in",
                      note: "potion-base-8M · no Ollama · instant",
                    },
                    {
                      id: "ollama",
                      label: "Ollama model",
                      note: "e.g. nomic-embed-text",
                    },
                  ].map((ev) => (
                    <button
                      key={ev.id}
                      onClick={() => setDraft({ ...draft, embedder: ev.id })}
                      className={cn(
                        "flex flex-col items-start gap-0.5 rounded-md border px-3 py-2 text-left transition-colors",
                        draft.embedder === ev.id
                          ? "border-primary/60 bg-primary/10 text-foreground"
                          : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
                      )}
                    >
                      <span className="text-[12.5px] font-medium">
                        {ev.label}
                      </span>
                      <span className="text-[11px] text-subtle-foreground">
                        {ev.note}
                      </span>
                    </button>
                  ))}
                </div>
              </Field>

              {draft.embedder === "ollama" && (
                <Field
                  label="Embedding model"
                  hint="Used to index sources for retrieval. nomic-embed-text is recommended."
                >
                  <ModelPicker
                    value={draft.embedModel}
                    models={models}
                    onChange={(v) => setDraft({ ...draft, embedModel: v })}
                  />
                </Field>
              )}

              {draft.provider === "openai" ? (
                <Field
                  label="Vision model"
                  hint="Pick a vision-capable model (e.g. gpt-4o or a Claude model) to enable OCR for images & scanned PDFs."
                >
                  {gatewayModels.length > 0 ? (
                    <Select
                      value={draft.openaiVisionModel ?? ""}
                      onChange={(v) =>
                        setDraft({ ...draft, openaiVisionModel: v })
                      }
                      options={gatewayModels}
                      emptyLabel="OCR disabled — choose a vision-capable model to enable"
                    />
                  ) : (
                    <Input
                      value={draft.openaiVisionModel ?? ""}
                      onChange={(e) =>
                        setDraft({
                          ...draft,
                          openaiVisionModel: e.target.value,
                        })
                      }
                      placeholder="a vision-capable model id (empty = OCR disabled)"
                    />
                  )}
                </Field>
              ) : (
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
              )}

              <PodcastVoicesSection />
            </>
          )}

          {tab === "chat" && <ChatTab />}

          {tab === "personalization" && <PersonalizationTab />}

          {tab === "agents" && <AgentsTab />}

          {tab === "appearance" && <AppearanceTab />}

          {tab === "shortcuts" && <ShortcutsTab />}

          {tab === "about" && <AboutTab />}
        </div>
      </div>

      <Modal
        open={confirmReembed}
        onClose={cancelSwitch}
        title="Switch embedding model?"
      >
        <div className="flex flex-col gap-4">
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            Different embedders produce incompatible vectors, so switching to{" "}
            <span className="font-medium text-foreground">
              {draft.embedder === "builtin"
                ? "the built-in embedder"
                : draft.embedModel}
            </span>{" "}
            requires re-embedding all{" "}
            <span className="font-medium text-foreground">{totalSources}</span>{" "}
            source
            {totalSources === 1 ? "" : "s"}. This runs locally and may take a
            moment.
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
  {
    id: "custom",
    label: "Custom",
    hint: "Give your own goal, style, or role.",
  },
] as const;

const CHAT_LENGTHS = [
  { id: "default", label: "Default" },
  { id: "longer", label: "Longer" },
  { id: "shorter", label: "Shorter" },
] as const;

const CHAT_FONTS = [
  { id: "sans", label: "Sans", className: "font-sans" },
  { id: "serif", label: "Serif", className: "font-serif" },
  { id: "mono", label: "Mono", className: "font-mono" },
  { id: "system", label: "System", className: "chat-system" },
] as const;

const CHAT_SIZES = [
  { id: "small", label: "Small" },
  { id: "medium", label: "Medium" },
  { id: "large", label: "Large" },
] as const;

const CHAT_ALIGNS = [
  { id: "natural", label: "Natural" },
  { id: "justified", label: "Justified" },
] as const;

/** Chat style & length for the current notebook; applies as you click. */
function ChatTab() {
  const chatConfig = useStore((s) => s.chatConfig);
  const setChatConfig = useStore((s) => s.setChatConfig);
  const currentId = useStore((s) => s.currentId);
  const notebook = useStore((s) =>
    s.notebooks.find((n) => n.id === s.currentId),
  );

  const apply = (patch: Partial<ChatConfig>) =>
    setChatConfig({ ...chatConfig, ...patch });
  const styleHint = CHAT_STYLES.find((s) => s.id === chatConfig.style)?.hint;

  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-muted-foreground">
        {currentId ? (
          <>
            Tune how the assistant responds in{" "}
            <span className="font-medium text-foreground">
              {notebook?.title ?? "this notebook"}
            </span>
            . Changes apply immediately.
          </>
        ) : (
          "Open a notebook to tune its chat — each notebook keeps its own style."
        )}
      </p>

      <Field label="Conversational goal, style, or role">
        <div className="flex flex-wrap gap-1.5">
          {CHAT_STYLES.map((s) => (
            <Pill
              key={s.id}
              active={chatConfig.style === s.id}
              onClick={() => apply({ style: s.id })}
            >
              {s.label}
            </Pill>
          ))}
        </div>
        {styleHint && (
          <span className="text-[11px] text-subtle-foreground">
            {styleHint}
          </span>
        )}
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
            <Pill
              key={l.id}
              active={chatConfig.length === l.id}
              onClick={() => apply({ length: l.id })}
            >
              {l.label}
            </Pill>
          ))}
        </div>
      </Field>
    </div>
  );
}

/** Who the user is — woven into system prompts across chat and generations. */
function PersonalizationTab() {
  const aiConfig = useStore((s) => s.aiConfig);
  const save = useStore((s) => s.saveAiConfig);
  const [draft, setDraft] = useState({
    name: "",
    profession: "",
    instructions: "",
  });

  useEffect(() => {
    if (aiConfig?.profile) setDraft(aiConfig.profile);
    // Load once per dialog open — refreshing on every aiConfig change would
    // clobber in-progress typing when a blur-save round-trips.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Fields save when you leave them (blur) — no Save button to remember.
  const saveOnBlur = () => {
    if (!aiConfig) return;
    const p = aiConfig.profile ?? {
      name: "",
      profession: "",
      instructions: "",
    };
    const changed =
      draft.name !== p.name ||
      draft.profession !== p.profession ||
      draft.instructions !== p.instructions;
    if (changed) void save({ ...aiConfig, profile: { ...draft } });
  };

  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-muted-foreground">
        Tell the assistant who you are. This is added to every chat and
        generated document's system prompt so answers fit you — it is never sent
        anywhere except your configured model. Changes save automatically.
      </p>

      <Field label="What should the assistant call you?">
        <Input
          placeholder="e.g. Paul"
          value={draft.name}
          onChange={(e) => setDraft({ ...draft, name: e.target.value })}
          onBlur={saveOnBlur}
        />
      </Field>

      <Field label="What best describes your work?">
        <Input
          placeholder="e.g. Product management"
          value={draft.profession}
          onChange={(e) => setDraft({ ...draft, profession: e.target.value })}
          onBlur={saveOnBlur}
        />
      </Field>

      <Field label="Instructions for the assistant">
        <Textarea
          rows={8}
          placeholder={
            "Preferences it should keep in mind across all notebooks, e.g.\n" +
            "- I prefer concise answers with tables for comparisons\n" +
            "- Prices in USD; I'm in the San Francisco Bay Area"
          }
          value={draft.instructions}
          onChange={(e) => setDraft({ ...draft, instructions: e.target.value })}
          onBlur={saveOnBlur}
        />
      </Field>
    </div>
  );
}

/** Appearance: theme + chat reading preferences (font, size, alignment). */
function AppearanceTab() {
  const reading = useStore((s) => s.reading);
  const setReading = useStore((s) => s.setReading);
  return (
    <div className="flex flex-col gap-4">
      <Field label="Theme" hint="Applies immediately.">
        <ThemePicker />
      </Field>

      <div className="h-px bg-border" />

      <Field
        label="Chat font"
        hint="How chat responses are displayed. Doesn't change the model."
      >
        <div className="flex flex-wrap gap-1.5">
          {CHAT_FONTS.map((f) => (
            <Pill
              key={f.id}
              active={reading.font === f.id}
              onClick={() => setReading({ font: f.id })}
            >
              <span className={f.className}>{f.label}</span>
            </Pill>
          ))}
        </div>
      </Field>

      <Field label="Text size">
        <div className="flex flex-wrap gap-1.5">
          {CHAT_SIZES.map((s) => (
            <Pill
              key={s.id}
              active={reading.fontSize === s.id}
              onClick={() => setReading({ fontSize: s.id })}
            >
              {s.label}
            </Pill>
          ))}
        </div>
      </Field>

      <Field label="Alignment">
        <div className="flex flex-wrap gap-1.5">
          {CHAT_ALIGNS.map((a) => (
            <Pill
              key={a.id}
              active={reading.textAlign === a.id}
              onClick={() => setReading({ textAlign: a.id })}
            >
              {a.label}
            </Pill>
          ))}
        </div>
      </Field>
    </div>
  );
}

const SHORTCUTS: { keys: string[]; label: string; context?: string }[] = [
  { keys: ["⌘", "N"], label: "New notebook", context: "Home" },
  { keys: ["⌘", "N"], label: "New note", context: "Notebook" },
  { keys: ["⌘", "K"], label: "Open the command menu" },
  { keys: ["⌘", "F"], label: "Find in source", context: "Reader" },
  { keys: ["⌘", "1"], label: "Show or hide Sources", context: "Notebook" },
  { keys: ["⌘", "2"], label: "Show or hide Studio", context: "Notebook" },
  { keys: ["⌘", ","], label: "Open Settings" },
  { keys: ["↩"], label: "Send message · next find match" },
  { keys: ["⇧", "↩"], label: "New line in the composer" },
  { keys: ["esc"], label: "Close dialog or menu" },
];

function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="inline-flex h-[22px] min-w-[22px] items-center justify-center rounded-md border border-border-strong bg-surface-2 px-1.5 font-sans text-[12px] text-foreground/85 shadow-[0_1px_0_var(--border)]">
      {children}
    </kbd>
  );
}

/** Read-only reference of the app's keyboard commands. */
function ShortcutsTab() {
  return (
    <div className="flex flex-col gap-1">
      {SHORTCUTS.map((s, i) => (
        <div key={i} className="flex items-center gap-3 rounded-md px-1 py-1.5">
          <div className="flex w-20 shrink-0 items-center gap-1">
            {s.keys.map((k) => (
              <Kbd key={k}>{k}</Kbd>
            ))}
          </div>
          <span className="text-[13px] text-foreground/90">{s.label}</span>
          {s.context && (
            <span className="ml-auto text-[11px] text-subtle-foreground">
              {s.context}
            </span>
          )}
        </div>
      ))}
      <p className="mt-2 text-[11px] leading-relaxed text-subtle-foreground">
        On Windows and Linux, use Ctrl in place of ⌘.
      </p>
    </div>
  );
}

/** App identity, version, and links. */
function AboutTab() {
  const [version, setVersion] = useState("");
  const [build, setBuild] = useState<BuildInfo | null>(null);
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(""));
    api
      .buildInfo()
      .then(setBuild)
      .catch(() => setBuild(null));
  }, []);
  return (
    <div className="flex flex-col items-center gap-1 py-6 text-center">
      <AlchemySymbol className="h-16 w-16 text-citation/70" />
      <div className="mt-3 text-[17px] font-semibold tracking-tight">
        Alchemy
      </div>
      <div className="text-[13px] text-muted-foreground">
        Local-first research notebooks
      </div>
      {version && (
        <div className="mt-2 text-[12px] text-subtle-foreground">
          Version {version}
          {build && (
            <>
              {" · "}
              <span className="font-mono">{build.commit}</span>
              {build.profile === "dev" && (
                <span className="ml-1.5 rounded bg-primary/15 px-1.5 py-0.5 font-medium text-citation">
                  dev
                </span>
              )}
            </>
          )}
        </div>
      )}
      <button
        className="mt-4 inline-flex items-center gap-1.5 text-[12px] text-citation hover:underline"
        onClick={() => void openUrl("https://github.com/thrashr888/alchemy")}
      >
        <Globe className="h-3.5 w-3.5" />
        github.com/thrashr888/alchemy
      </button>
      <div className="mt-4 text-[12px] text-subtle-foreground">
        © {new Date().getFullYear()} Paul Thrasher
      </div>
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
  provider,
  modelCount,
  chatModel,
  loading,
  onRefresh,
}: {
  connOk: boolean | null;
  provider: string;
  modelCount: number;
  chatModel: string;
  loading: boolean;
  onRefresh: () => void;
}) {
  const health = useStore((s) => s.modelHealth);
  const stats = useStore((s) => s.modelStats);
  const kokoro = useStore((s) => s.kokoroStatus);
  const chatStat = stats.find(
    (s) => normModel(s.name) === normModel(chatModel),
  );

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

  // For the gateway provider, connection state comes from the gateway's chat
  // health, not the Ollama probe — a gateway user may not run Ollama at all.
  const isGateway = provider === "openai";
  const ok: boolean | null = isGateway
    ? health
      ? health.chat.working
      : null
    : connOk;
  const okText = isGateway
    ? `Connected to gateway · ${modelCount} models available`
    : `Connected · ${modelCount} models available`;
  const failText = isGateway
    ? "Cannot reach the gateway — check the URL and API key below."
    : "Cannot reach Ollama. Is `ollama serve` running?";
  const checkingText = isGateway ? "Checking gateway…" : "Checking Ollama…";

  return (
    <div className="flex flex-col gap-2 rounded-md border border-border bg-surface-2 px-3 py-2.5">
      {/* Overall connection */}
      <div className="flex items-center gap-2 text-[12px]">
        {ok === null ? (
          <Spinner className="h-3.5 w-3.5 text-muted-foreground" />
        ) : ok ? (
          <CheckCircle2 className="h-4 w-4 text-success" />
        ) : (
          <XCircle className="h-4 w-4 text-destructive" />
        )}
        <span
          className={cn(
            ok === false ? "text-destructive" : "text-muted-foreground",
          )}
        >
          {ok === null ? checkingText : ok ? okText : failText}
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

      {ok && (
        <div className="flex flex-col gap-1.5 border-t border-border pt-2">
          <Row label="Chat" status={health?.chat} />
          <Row label="Embed" status={health?.embed} />
          <Row label="Vision" status={health?.vision} optional />
          <Row
            label="Voices"
            optional
            status={{
              working: !!kokoro?.verified,
              detail: !kokoro
                ? "Checking…"
                : kokoro.verified
                  ? "Kokoro-82M ready — Audio Overview enabled"
                  : kokoro.downloaded
                    ? "Downloaded — verify below to enable Audio Overview"
                    : "Not set up — download below to enable Audio Overview",
            }}
          />
          {chatStat && chatStat.samples > 0 && (
            <div className="flex items-center gap-2 text-[12px]">
              <Zap className="h-3.5 w-3.5 shrink-0 text-citation" />
              <span className="w-12 shrink-0 text-muted-foreground">Speed</span>
              <span className="text-foreground/80">
                ~{chatStat.avgTokensPerSec.toFixed(1)} tok/s avg
                <span className="text-subtle-foreground">
                  {" "}
                  · last {chatStat.lastTokensPerSec.toFixed(1)} ·{" "}
                  {chatStat.samples} run
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

/** Toggle row: label + native checkbox, persisted to localStorage. */
function PrefToggle({
  storageKey,
  label,
  hint,
  onEnable,
}: {
  storageKey: string;
  label: string;
  hint: string;
  onEnable?: () => void;
}) {
  const [on, setOn] = useState(localStorage.getItem(storageKey) !== "false");
  return (
    <label className="flex cursor-pointer items-start gap-2.5">
      <input
        type="checkbox"
        checked={on}
        onChange={(e) => {
          const v = e.target.checked;
          localStorage.setItem(storageKey, String(v));
          setOn(v);
          if (v) onEnable?.();
        }}
        className="mt-0.5 h-4 w-4 accent-[var(--primary)]"
      />
      <span className="flex flex-col gap-0.5">
        <span className="text-[13px] text-foreground">{label}</span>
        <span className="text-[11px] text-subtle-foreground">{hint}</span>
      </span>
    </label>
  );
}

/** App-level preferences: updates, notifications, sounds. */
function GeneralTab() {
  const pushToast = useStore((s) => s.pushToast);
  const [checking, setChecking] = useState(false);
  const [update, setUpdate] = useState<UpdateFlow | null>(null);
  const [installing, setInstalling] = useState(false);

  // "Check for Updates…" from the app menu lands here with the flag set.
  const pendingUpdateCheck = useStore((s) => s.pendingUpdateCheck);
  useEffect(() => {
    // Read the live value: StrictMode replays mount effects with the same
    // captured snapshot, so checking the prop would double-run the check.
    if (useStore.getState().pendingUpdateCheck) {
      useStore.setState({ pendingUpdateCheck: false });
      void onCheck();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingUpdateCheck]);

  async function onCheck() {
    setChecking(true);
    const flow = await checkForUpdates();
    setUpdate(flow);
    setChecking(false);
    if (flow.status === "none")
      pushToast("success", "You're on the latest version.");
    if (flow.status === "error")
      pushToast("error", `Update check failed: ${flow.message}`);
  }

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col gap-3">
        <PrefToggle
          storageKey="autoUpdateCheck"
          label="Automatically check for updates"
          hint="Checks GitHub once per launch; installing is always your call."
        />
        <div className="flex items-center gap-2 pl-6.5">
          <Button
            variant="secondary"
            size="sm"
            onClick={() => void onCheck()}
            loading={checking}
          >
            Check for updates…
          </Button>
          {update?.status === "available" && (
            <Button
              variant="primary"
              size="sm"
              loading={installing}
              onClick={() => {
                setInstalling(true);
                void update.install().catch((e) => {
                  setInstalling(false);
                  pushToast(
                    "error",
                    e instanceof Error ? e.message : String(e),
                  );
                });
              }}
            >
              Install {update.version} & relaunch
            </Button>
          )}
        </div>
      </div>

      <div className="h-px bg-border" />

      <div className="flex flex-col gap-1.5">
        <div className="text-[13px]">Studio templates</div>
        <p className="text-[11px] leading-relaxed text-subtle-foreground">
          Custom generators live in ~/Documents/Alchemy/templates — one .md file
          per generator. Deleting a file removes its tile for good; this puts
          the default pack back (without touching files you've edited).
        </p>
        <div>
          <Button
            variant="secondary"
            size="sm"
            onClick={async () => {
              try {
                const n = await api.installDefaultTemplates();
                useStore.setState({ templates: await api.listTemplates() });
                pushToast(
                  "success",
                  n > 0
                    ? `Installed ${n} template file${n === 1 ? "" : "s"}`
                    : "All default templates are already installed",
                );
              } catch (e) {
                pushToast("error", e instanceof Error ? e.message : String(e));
              }
            }}
          >
            Install template files
          </Button>
        </div>
      </div>

      <div className="h-px bg-border" />

      <div className="flex flex-col gap-1.5">
        <div className="text-[13px]">Mac apps</div>
        <p className="text-[11px] leading-relaxed text-subtle-foreground">
          Calendar, Reminders, and Apple Notes can be added as auto-syncing
          sources. Connecting here triggers the macOS permission prompts once,
          so adding them to a notebook later just works.
        </p>
        <MacConnect showInstallHint />
      </div>

      <div className="h-px bg-border" />

      <PrefToggle
        storageKey="showNotifications"
        label="Show notifications"
        hint="Desktop notification when a document, rebuild, or report finishes."
      />
      <PrefToggle
        storageKey="playSounds"
        label="Play sounds"
        hint="Soft event cues: work finishing, new arrivals while the app is in the background, and errors. Never clicks or hovers."
        onEnable={playDone}
      />
      <TrayToggle />
      <CuratorToggle />
    </div>
  );
}

/** Weekly LLM consolidation of auto-created evidence notes — on by default
 *  (idle-gated, capped, recoverable); the toggle is cost control
 *  (RFC-note-curator §4). */
function CuratorToggle() {
  const aiConfig = useStore((s) => s.aiConfig);
  const saveAiConfig = useStore((s) => s.saveAiConfig);
  if (!aiConfig) return null;
  return (
    <label className="flex cursor-pointer items-start gap-2.5">
      <input
        type="checkbox"
        checked={aiConfig.curatorConsolidate}
        onChange={(e) =>
          void saveAiConfig({ ...aiConfig, curatorConsolidate: e.target.checked })
        }
        className="mt-0.5 h-4 w-4 accent-[var(--primary)]"
      />
      <span className="flex flex-col gap-0.5">
        <span className="text-[13px] text-foreground">
          Consolidate auto notes weekly
        </span>
        <span className="text-[11px] leading-relaxed text-subtle-foreground">
          Once a week, while you're away, the model merges chat-created
          evidence notes that state the same claim. The merged-away note is
          archived, never deleted, and each notebook's Curator report lists
          what happened. Uses your chat model.
        </span>
      </span>
    </label>
  );
}

/** Menu bar extra on/off — lives in AiConfig so the backend applies it live. */
function TrayToggle() {
  const aiConfig = useStore((s) => s.aiConfig);
  const saveAiConfig = useStore((s) => s.saveAiConfig);
  if (!aiConfig) return null;
  return (
    <label className="flex cursor-pointer items-start gap-2.5">
      <input
        type="checkbox"
        checked={aiConfig.trayEnabled}
        onChange={(e) =>
          void saveAiConfig({ ...aiConfig, trayEnabled: e.target.checked })
        }
        className="mt-0.5 h-4 w-4 accent-[var(--primary)]"
      />
      <span className="flex flex-col gap-0.5">
        <span className="text-[13px] text-foreground">Show menu bar icon</span>
        <span className="text-[11px] leading-relaxed text-subtle-foreground">
          Ask Alchemy, add the clipboard as a source, and jump to recent
          notebooks from the menu bar. ⌥Space summons Alchemy either way.
        </span>
      </span>
    </label>
  );
}

/** Agent access: the embedded MCP server + one connect row per agent client. */
function AgentsTab() {
  const aiConfig = useStore((s) => s.aiConfig);
  const saveAiConfig = useStore((s) => s.saveAiConfig);
  const pushToast = useStore((s) => s.pushToast);
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [connectors, setConnectors] = useState<ConnectorStatus[]>([]);
  const [busy, setBusy] = useState<string | null>(null);

  function refresh() {
    api
      .mcpStatus()
      .then(setStatus)
      .catch(() => setStatus(null));
    api
      .listAgentConnectors()
      .then(setConnectors)
      .catch(() => setConnectors([]));
  }
  useEffect(refresh, []);

  if (!aiConfig) return null;
  const running = status?.running ?? false;

  function connect(c: ConnectorStatus) {
    setBusy(c.id);
    api
      .connectAgent(c.id)
      .then((updated) => {
        setConnectors((list) =>
          list.map((x) => (x.id === updated.id ? updated : x)),
        );
        pushToast(
          "success",
          updated.configured
            ? `${updated.name} connected — restart it to pick up the change`
            : `Skill installed for ${updated.name}`,
        );
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

  const sorted = [...connectors].sort((a, b) => a.name.localeCompare(b.name));

  return (
    <div className="flex flex-col gap-5">
      <label className="flex cursor-pointer items-start gap-2.5">
        <input
          type="checkbox"
          checked={aiConfig.mcpEnabled}
          onChange={(e) => {
            void saveAiConfig({
              ...aiConfig,
              mcpEnabled: e.target.checked,
            }).then(() =>
              // The server starts/stops on save; give it a beat before polling.
              setTimeout(refresh, 400),
            );
          }}
          className="mt-0.5 h-4 w-4 accent-[var(--primary)]"
        />
        <span className="flex flex-col gap-0.5">
          <span className="text-[13px] text-foreground">
            Let AI agents use Alchemy (MCP)
          </span>
          <span className="text-[11px] text-subtle-foreground">
            Agents can create notebooks, add sources, search, and write notes —
            changes appear live in the app. Local-only: the server listens on
            127.0.0.1 and nothing leaves this Mac.
          </span>
        </span>
      </label>

      <div className="flex items-center gap-2 text-[12px]">
        <span
          className={cn(
            "h-2 w-2 rounded-full",
            running ? "bg-success" : "bg-muted-foreground/40",
          )}
        />
        <span className="text-muted-foreground">
          {running ? (
            <>
              Running at <span className="text-foreground">{status?.url}</span>
            </>
          ) : (
            "Not running"
          )}
        </span>
        {running && status?.url && (
          <button
            onClick={() => {
              void navigator.clipboard.writeText(status.url);
              pushToast("success", "Server URL copied");
            }}
            title="Copy server URL"
            aria-label="Copy the MCP server URL"
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
          >
            <Copy className="h-3 w-3" />
          </button>
        )}
      </div>

      <Field
        label="Clients"
        hint="Connect writes the client's own MCP config and installs the Alchemy skill where supported. The copy button gives the same setup as a command or snippet."
      >
        <div className="flex flex-col divide-y divide-border rounded-md border border-border">
          {sorted.map((c) => (
            <div
              key={c.id}
              className={cn(
                "flex items-center gap-2 px-2.5 py-2",
                !c.installed && "opacity-50",
              )}
            >
              <div className="flex min-w-0 flex-1 flex-col">
                <span className="text-[12.5px] text-foreground">{c.name}</span>
                <span className="truncate text-[10.5px] text-subtle-foreground">
                  {c.configPath}
                </span>
              </div>

              {c.configured ? (
                <span className="flex items-center gap-1 text-[11px] text-success">
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  Connected
                  {c.supportsSkill && c.skillInstalled ? " + skill" : ""}
                </span>
              ) : c.installed ? (
                c.canAuto ? (
                  <Button
                    variant="secondary"
                    size="sm"
                    loading={busy === c.id}
                    onClick={() => connect(c)}
                  >
                    Connect
                  </Button>
                ) : (
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => copySnippet(c)}
                  >
                    Copy command
                  </Button>
                )
              ) : (
                <span className="text-[11px] text-subtle-foreground">
                  Not installed
                </span>
              )}

              {/* Skill catch-up for manual/partial rows. */}
              {c.installed &&
                c.configured &&
                c.supportsSkill &&
                !c.skillInstalled && (
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
                title={`Copy manual setup\n${c.snippet}`}
                onClick={() => copySnippet(c)}
                className="rounded p-1 text-subtle-foreground transition-colors hover:text-foreground"
              >
                <Copy className="h-3.5 w-3.5" />
              </button>
            </div>
          ))}
          {connectors.length === 0 && (
            <div className="px-2.5 py-3 text-[11.5px] text-subtle-foreground">
              Loading clients…
            </div>
          )}
        </div>
      </Field>
    </div>
  );
}

function ThemePicker() {
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const darkBg = THEMES.midnight.vars.background;
  const lightBg = THEMES.light.vars.background;
  return (
    <div className="grid grid-cols-2 gap-1.5">
      {/* Follow the OS appearance: Midnight when dark, Light when light. */}
      <button
        onClick={() => setTheme(SYSTEM_THEME)}
        className={cn(
          "flex items-center gap-2 rounded-md border px-2 py-1.5 text-left text-[12px] transition-colors",
          theme === SYSTEM_THEME
            ? "border-primary/60 bg-primary/10 text-foreground"
            : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
        )}
      >
        <span
          className="flex h-5 w-5 shrink-0 items-center justify-center rounded border"
          style={{
            background: `linear-gradient(135deg, ${darkBg} 50%, ${lightBg} 50%)`,
            borderColor: THEMES.midnight.vars["border-strong"],
          }}
        >
          <span
            className="h-2.5 w-2.5 rounded-full"
            style={{ background: THEMES.midnight.vars.primary }}
          />
        </span>
        <span className="flex-1 truncate">System</span>
        {theme === SYSTEM_THEME && (
          <Check className="h-3.5 w-3.5 text-primary" />
        )}
      </button>
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
              style={{
                background: t.vars.background,
                borderColor: t.vars["border-strong"],
              }}
            >
              <span
                className="h-2.5 w-2.5 rounded-full"
                style={{ background: t.vars.primary }}
              />
            </span>
            <span className="flex-1 truncate">{t.label}</span>
            {active && <Check className="h-3.5 w-3.5 text-primary" />}
          </button>
        );
      })}
    </div>
  );
}

function Select({
  value,
  onChange,
  options,
  emptyLabel,
}: {
  value: string;
  onChange: (v: string) => void;
  options: string[];
  /** When set, "" is always offered with this label (a real, selectable state). */
  emptyLabel?: string;
}) {
  const list =
    options.includes(value) || !value ? options : [value, ...options];
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="h-8 w-full appearance-none rounded-md border border-input bg-surface-2 px-2.5 text-[13px] text-foreground outline-none transition-colors focus:border-ring/60"
    >
      {(emptyLabel || !value) && (
        <option value="">{emptyLabel ?? "Choose a model…"}</option>
      )}
      {list.map((m) => (
        <option key={m} value={m}>
          {m}
        </option>
      ))}
    </select>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-[12px] font-medium text-foreground">{label}</label>
      {children}
      {hint && (
        <span className="text-[11px] text-subtle-foreground">{hint}</span>
      )}
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

/**
 * Settings → Models: manage the on-device podcast voice model (Kokoro-82M).
 * The Audio Overview generator stays hidden until a download AND a test
 * synthesis have succeeded, so users never hit a broken or robotic episode.
 */
function PodcastVoicesSection() {
  const status = useStore((s) => s.kokoroStatus);
  const busy = useStore((s) => s.kokoroBusy);
  const setup = useStore((s) => s.setupKokoro);
  const remove = useStore((s) => s.removeKokoro);
  const download = useStore((s) => s.embedderDownload);
  const downloading = busy && !!download?.title?.includes("podcast");

  const state = !status
    ? { label: "Checking…", cls: "text-subtle-foreground" }
    : status.verified
      ? { label: "Ready — voices verified", cls: "text-success" }
      : status.downloaded
        ? {
            label: "Downloaded, not yet verified",
            cls: "text-muted-foreground",
          }
        : { label: "Not downloaded", cls: "text-muted-foreground" };

  return (
    <Field
      label="Podcast voices"
      hint="Audio Overview speaks with Kokoro-82M, a neural TTS that runs entirely on-device (one-time ~93 MB download). The generator appears in the Studio once the voices are downloaded and verified with a test synthesis."
    >
      <div className="flex items-center gap-3 rounded-md border border-border bg-surface-2/60 px-3 py-2.5">
        <AudioLines className="h-4 w-4 shrink-0 text-muted-foreground" />
        <div className="flex min-w-0 flex-col">
          <span className={cn("text-[12px] font-medium", state.cls)}>
            {state.label}
          </span>
          {downloading && download && (
            <span className="text-[11px] tabular-nums text-subtle-foreground">
              {download.total > 0
                ? `${download.label} — ${Math.round((download.done / download.total) * 100)}% of ${(download.total / 1e6).toFixed(0)} MB`
                : `${(download.done / 1e6).toFixed(1)} MB…`}
            </span>
          )}
          {busy && !downloading && (
            <span className="text-[11px] text-subtle-foreground">
              Verifying with a test synthesis…
            </span>
          )}
        </div>
        <div className="ml-auto flex items-center gap-1.5">
          {busy ? (
            <Button
              variant="secondary"
              onClick={() => void api.cancelGeneration("tts")}
            >
              Cancel
            </Button>
          ) : (
            <Button
              variant={status?.verified ? "secondary" : "primary"}
              onClick={() => void setup()}
            >
              {status?.verified
                ? "Test again"
                : status?.downloaded
                  ? "Verify voices"
                  : "Download & verify"}
            </Button>
          )}
          {status?.downloaded && !busy && (
            <Button
              variant="ghost"
              size="icon"
              onClick={() => void remove()}
              title="Remove the downloaded voice model (~93 MB)"
              aria-label="Remove the podcast voice model"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          )}
        </div>
      </div>
    </Field>
  );
}
