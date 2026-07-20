import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { playDone } from "@/lib/sound";
import { checkForUpdates, type UpdateFlow } from "@/lib/updates";
import { Button, Input, Modal, Spinner } from "./ui";
import { cn } from "@/lib/utils";
import { MacConnect } from "./MacConnect";
import {
  AboutTab,
  AppearanceTab,
  ChatTab,
  Field,
  PersonalizationTab,
  ShortcutsTab,
} from "./settings/SettingsTabs";
import type {
  AiConfig,
  ConnectorStatus,
  McpStatus,
} from "@/lib/types";
import {
  RefreshCw,
  CheckCircle2,
  XCircle,
  Circle,
  Zap,
  Cpu,
  MessageSquare,
  Palette,
  Keyboard,
  Info,
  SlidersHorizontal,
  UserRound,
  FolderGit2,
  Wand2,
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

/** Known OpenAI-compatible gateways: pick one, the URL fills; the select
 *  derives its value back from the URL so it round-trips. Custom stays for
 *  anything else. Entries with YOUR-… placeholders need the account/resource
 *  segment edited (the select then correctly shows Custom). Notable
 *  absences: Bob (blocks third-party clients by vendor policy) and GitHub
 *  Copilot proper (no plain API key — its subscription rides the copilot
 *  CLI, i.e. the agent-provider family; GitHub Models below is the
 *  PAT-keyed cousin). */
const GATEWAY_PRESETS = [
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

const TABS = [
  { id: "general", label: "General", icon: SlidersHorizontal },
  { id: "sources", label: "Sources", icon: FolderGit2 },
  { id: "studio", label: "Studio", icon: Wand2 },
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
  const [agentClis, setAgentClis] = useState<
    { id: string; installed: boolean; detail: string }[]
  >([]);
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
      void api.agentCliStatus().then(setAgentClis).catch(() => {});
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
      const sorted = [...gateway].sort((a, b) => a.localeCompare(b));
      setGatewayModels(sorted);
      // A model the new gateway doesn't know is a stale leftover from the
      // previous one — clear it so Save auto-picks or the user chooses.
      setDraft((d) =>
        d && d.openaiChatModel && sorted.length > 0 && !sorted.includes(d.openaiChatModel)
          ? { ...d, openaiChatModel: "" }
          : d,
      );
    } catch (e) {
      setGatewayModels([]);
      setGatewayError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingGateway(false);
    }
  }

  // Switching gateways should refresh the model list by itself — debounce
  // URL/key edits so half-typed values don't spam the endpoint.
  const gatewayDebounce = useRef<number | null>(null);
  function queueGatewayRefresh(url: string, key: string) {
    if (gatewayDebounce.current) window.clearTimeout(gatewayDebounce.current);
    if (!url.trim() && !key.trim()) return;
    gatewayDebounce.current = window.setTimeout(() => {
      void loadGatewayModels(url, key);
    }, 700);
  }

  // Which configured provider's connection fields are loaded into the flat
  // draft fields below (the editing bridge: entry → flat on select, flat →
  // entry on Save via syncEntriesIntoDraft).
  const [editingId, setEditingId] = useState<string | null>(null);
  const editingKind =
    draft?.providers?.find((p) => p.id === editingId)?.kind ?? null;
  const chatKind =
    draft?.providers?.find((p) => p.id === draft.chatProvider)?.kind ??
    (draft?.provider === "openai" ? "gateway" : "ollama");

  function selectForEdit(id: string) {
    if (!draft) return;
    const entry = draft.providers.find((p) => p.id === id);
    if (!entry) return;
    setEditingId(id);
    if (entry.kind === "gateway") {
      setDraft({
        ...draft,
        openaiBaseUrl: entry.baseUrl,
        openaiApiKey: entry.apiKey,
        openaiChatModel: entry.chatModel,
      });
      if (entry.apiKey) void loadGatewayModels(entry.baseUrl, entry.apiKey);
    } else if (entry.kind === "ollama") {
      setDraft({
        ...draft,
        baseUrl: entry.baseUrl || draft.baseUrl,
        chatModel: entry.chatModel || draft.chatModel,
      });
    }
  }

  /** Flat fields → the entry being edited; called before save. */
  function syncEntriesIntoDraft(d: AiConfig): AiConfig {
    if (!editingId) return d;
    return {
      ...d,
      providers: d.providers.map((p) => {
        if (p.id !== editingId) return p;
        if (p.kind === "gateway") {
          const preset = GATEWAY_PRESETS.find(
            (g) => g.url === d.openaiBaseUrl.trim(),
          );
          return {
            ...p,
            label: preset?.name ?? p.label,
            baseUrl: d.openaiBaseUrl,
            apiKey: d.openaiApiKey,
            chatModel: d.openaiChatModel,
          };
        }
        if (p.kind === "ollama")
          return { ...p, baseUrl: d.baseUrl, chatModel: d.chatModel };
        return p;
      }),
    };
  }

  function addProvider(kind: string, label: string) {
    if (!draft) return;
    // Agent CLIs are singletons; gateways/ollama can repeat (work + personal).
    const singleton = kind !== "gateway" && kind !== "ollama";
    if (singleton && draft.providers.some((p) => p.kind === kind)) return;
    const id = singleton ? kind : `p${Date.now().toString(36)}`;
    const entry = {
      id,
      kind,
      label,
      baseUrl: kind === "ollama" ? draft.baseUrl : "",
      apiKey: "",
      chatModel: "",
    };
    setDraft({ ...draft, providers: [...draft.providers, entry] });
    if (!singleton) setEditingId(id);
  }

  function removeProvider(id: string) {
    if (!draft) return;
    const providers = draft.providers.filter((p) => p.id !== id);
    const fallback = providers[0]?.id ?? "";
    setDraft({
      ...draft,
      providers,
      chatProvider: draft.chatProvider === id ? fallback : draft.chatProvider,
      studioProvider:
        draft.studioProvider === id ? fallback : draft.studioProvider,
    });
    if (editingId === id) setEditingId(null);
  }

  function providerDetail(p: { id: string; kind: string; chatModel: string }) {
    if (p.kind === "gateway")
      return p.chatModel || "no model picked";
    if (p.kind === "ollama") return p.chatModel || draft?.chatModel || "";
    const cli = agentClis.find((c) => c.id === p.kind);
    return cli ? (cli.installed ? cli.detail : "Not installed") : "…";
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
    let toSave = syncEntriesIntoDraft(draft);
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
              type="button"
              key={t.id}
              onClick={() => setTab(t.id)}
              aria-current={tab === t.id ? "page" : undefined}
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
          {tab === "sources" && <SourcesTab />}
          {tab === "studio" && <StudioTab />}
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
                label="Providers"
                hint="Add the engines you use; click a row to edit its connection. Agent CLIs need no setup — their login is the credential."
              >
                <div className="flex flex-col gap-1">
                  {draft.providers.map((p) => (
                    <div
                      key={p.id}
                      className={cn(
                        "group flex items-center gap-2 rounded-md border px-2.5 py-1.5",
                        editingId === p.id
                          ? "border-primary/60 bg-primary/10"
                          : "border-border bg-surface-2",
                      )}
                    >
                      <button
                        type="button"
                        onClick={() => selectForEdit(p.id)}
                        className="flex min-w-0 flex-1 flex-col items-start text-left"
                      >
                        <span className="text-[12.5px] font-medium text-foreground">
                          {p.label}
                        </span>
                        <span className="max-w-full truncate text-[11px] text-subtle-foreground">
                          {providerDetail(p)}
                        </span>
                      </button>
                      {draft.chatProvider === p.id && (
                        <span className="shrink-0 rounded-full bg-primary/15 px-2 py-px text-[11px] text-citation">
                          Chat
                        </span>
                      )}
                      {draft.studioProvider === p.id && (
                        <span className="shrink-0 rounded-full border border-border px-2 py-px text-[11px] text-muted-foreground">
                          Studio
                        </span>
                      )}
                      <button
                        type="button"
                        aria-label={`Remove ${p.label}`}
                        onClick={() => removeProvider(p.id)}
                        className="rounded p-1 text-subtle-foreground opacity-0 hover:bg-surface-2 hover:text-destructive group-focus-within:opacity-100 group-hover:opacity-100"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  ))}
                  <div className="flex flex-wrap gap-1 pt-0.5">
                    {[
                      { kind: "ollama", label: "Ollama" },
                      { kind: "gateway", label: "Gateway" },
                      { kind: "claude-code", label: "Claude Code" },
                      { kind: "codex", label: "Codex" },
                      { kind: "gemini-cli", label: "Gemini CLI" },
                      { kind: "cursor-cli", label: "Cursor CLI" },
                      { kind: "bob-shell", label: "Bob Shell" },
                    ]
                      .filter(
                        (k) =>
                          k.kind === "gateway" ||
                          k.kind === "ollama" ||
                          !draft.providers.some((p) => p.kind === k.kind),
                      )
                      .map((k) => (
                        <button
                          type="button"
                          key={k.kind}
                          onClick={() => addProvider(k.kind, k.label)}
                          className="rounded-full border border-border px-2.5 py-0.5 text-[11.5px] text-muted-foreground hover:bg-surface-2 hover:text-foreground"
                        >
                          + {k.label}
                        </button>
                      ))}
                  </div>
                </div>
              </Field>
              <Field
                label="Main model"
                hint="Answers notebook chat. Auxiliary tasks follow it unless overridden."
              >
                <div className="flex items-center gap-1.5">
                  <select
                    aria-label="Main provider"
                    value={draft.chatProvider}
                    onChange={(e) =>
                      setDraft({ ...draft, chatProvider: e.target.value })
                    }
                    className="h-8 flex-1 rounded-md border border-input bg-surface-2 px-2 text-[13px] text-foreground focus:outline-none"
                  >
                    {draft.providers.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.label}
                        {p.chatModel ? ` · ${p.chatModel}` : ""}
                      </option>
                    ))}
                  </select>
                  <Button
                    variant="secondary"
                    onClick={() => selectForEdit(draft.chatProvider)}
                    title="Edit this provider's connection and model"
                  >
                    Edit
                  </Button>
                </div>
              </Field>
              <Field
                label="Auxiliary models"
                hint="Helper tasks run on the main model by default; override any of them."
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
                            setDraft({
                              ...draft,
                              studioProvider: e.target.value,
                            })
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
                          onClick={() =>
                            setDraft({ ...draft, studioProvider: "" })
                          }
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
                  <div className="flex items-center gap-2">
                    <span className="w-24 shrink-0 text-[12.5px] text-foreground">
                      Embeddings
                    </span>
                    <span className="flex-1 text-[12px] text-subtle-foreground">
                      configured below · never follows chat (index-coupled)
                    </span>
                  </div>
                </div>
              </Field>

              {editingKind === "gateway" && (
                <>
                  <Field
                    label="Gateway"
                    hint="Pick a known provider to fill the URL, or Custom for any OpenAI-compatible endpoint."
                  >
                    <select
                      aria-label="Gateway preset"
                      value={
                        GATEWAY_PRESETS.find((p) => p.url === draft.openaiBaseUrl.trim())
                          ?.url ?? "custom"
                      }
                      onChange={(e) => {
                        if (e.target.value !== "custom") {
                          setDraft({ ...draft, openaiBaseUrl: e.target.value });
                          void loadGatewayModels(
                            e.target.value,
                            draft.openaiApiKey,
                          );
                        }
                      }}
                      className="h-8 w-full rounded-md border border-input bg-surface-2 px-2 text-[13px] text-foreground focus:outline-none"
                    >
                      {GATEWAY_PRESETS.map((p) => (
                        <option key={p.url} value={p.url}>
                          {p.name}
                        </option>
                      ))}
                      <option value="custom">Custom…</option>
                    </select>
                  </Field>
                  <Field
                    label="Gateway URL"
                    hint="Empty = inferred from your key for OpenAI, Anthropic, OpenRouter, and Groq. Any OpenAI-compatible base URL works (usually ends in /v1)."
                  >
                    <Input
                      name="gateway-url"
                      aria-label="Gateway URL"
                      value={draft.openaiBaseUrl}
                      onChange={(e) => {
                        setDraft({ ...draft, openaiBaseUrl: e.target.value });
                        queueGatewayRefresh(e.target.value, draft.openaiApiKey);
                      }}
                      placeholder="https://api.example.com/v1"
                    />
                  </Field>
                  <Field
                    label="API key"
                    hint="Stored locally in your config file; sent only to the gateway."
                  >
                    <Input
                      type="password"
                      name="gateway-api-key"
                      aria-label="API key"
                      value={draft.openaiApiKey}
                      onChange={(e) => {
                        setDraft({ ...draft, openaiApiKey: e.target.value });
                        queueGatewayRefresh(draft.openaiBaseUrl, e.target.value);
                      }}
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
                          ariaLabel="Gateway chat model"
                          value={draft.openaiChatModel}
                          onChange={(v) =>
                            setDraft({ ...draft, openaiChatModel: v })
                          }
                          options={gatewayModels}
                        />
                      ) : (
                        <Input
                          name="gateway-chat-model"
                          aria-label="Gateway chat model"
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

              {editingKind === "ollama" && (
                <Field label="Ollama URL">
                  <Input
                    name="ollama-url"
                    aria-label="Ollama URL"
                    value={draft.baseUrl}
                    onChange={(e) =>
                      setDraft({ ...draft, baseUrl: e.target.value })
                    }
                    placeholder="http://localhost:11434"
                  />
                </Field>
              )}

              {editingKind === "ollama" && (
                <Field
                  label="Chat model"
                  hint="Used to answer questions and generate documents. Models tagged nvfp4/mlx run on Ollama's MLX engine (Apple-Silicon accelerated)."
                >
                  <ModelPicker
                    label="Chat model"
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
                      type="button"
                      key={ev.id}
                      onClick={() => setDraft({ ...draft, embedder: ev.id })}
                      aria-pressed={draft.embedder === ev.id}
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
                    label="Embedding model"
                    value={draft.embedModel}
                    models={models}
                    onChange={(v) => setDraft({ ...draft, embedModel: v })}
                  />
                </Field>
              )}

              {chatKind === "gateway" ? (
                <Field
                  label="Vision model"
                  hint="Pick a vision-capable model (e.g. gpt-4o or a Claude model) to enable OCR for images & scanned PDFs."
                >
                  {gatewayModels.length > 0 ? (
                    <Select
                      ariaLabel="Vision model"
                      value={draft.openaiVisionModel ?? ""}
                      onChange={(v) =>
                        setDraft({ ...draft, openaiVisionModel: v })
                      }
                      options={gatewayModels}
                      emptyLabel="OCR disabled — choose a vision-capable model to enable"
                    />
                  ) : (
                    <Input
                      name="vision-model"
                      aria-label="Vision model"
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
                    label="Vision model"
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
          aria-label="Recheck model connection"
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
    </div>
  );
}

/** Everything about getting content in: Mac apps, git repositories. */
function SourcesTab() {
  return (
    <div className="flex flex-col gap-5">
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

      <GitSyncSelect />
    </div>
  );
}

/** Everything about generation: templates, the note curator. */
function StudioTab() {
  const pushToast = useStore((s) => s.pushToast);
  return (
    <div className="flex flex-col gap-5">
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
          <Button
            variant="ghost"
            size="sm"
            className="ml-1.5"
            onClick={() => void api.openTemplatesFolder()}
          >
            Show in Finder
          </Button>
        </div>
      </div>
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

/** Auto-sync cadence for remote git sources (RFC-git-sources §8). Git
 *  sources themselves are always on — this only paces the network probes.
 *  Manual Refresh always syncs regardless. */
function GitSyncSelect() {
  const aiConfig = useStore((s) => s.aiConfig);
  const saveAiConfig = useStore((s) => s.saveAiConfig);
  if (!aiConfig) return null;
  return (
    <div className="flex flex-col gap-1">
      <label className="flex items-center justify-between gap-3">
        <span className="text-[13px] text-foreground">
          Auto-sync git repositories
        </span>
        <select
          value={String(aiConfig.gitSyncMinutes)}
          onChange={(e) =>
            void saveAiConfig({
              ...aiConfig,
              gitSyncMinutes: Number(e.target.value),
            })
          }
          className="h-8 rounded-md border border-input bg-surface-2 px-2 text-[13px] text-foreground focus:outline-none"
        >
          <option value="15">Every 15 minutes</option>
          <option value="60">Hourly</option>
          <option value="360">Every 6 hours</option>
          <option value="1440">Daily</option>
          <option value="0">Off</option>
        </select>
      </label>
      <span className="text-[11px] leading-relaxed text-subtle-foreground">
        Remote repos re-fetch when their branch moves, using your own git
        credentials — Alchemy never stores tokens. Manual Refresh always
        syncs, even when this is off.
      </span>
    </div>
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
                type="button"
                title={`Copy manual setup\n${c.snippet}`}
                onClick={() => copySnippet(c)}
                aria-label={`Copy manual setup for ${c.name}`}
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

function Select({
  ariaLabel,
  value,
  onChange,
  options,
  emptyLabel,
}: {
  ariaLabel: string;
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
      aria-label={ariaLabel}
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

function ModelPicker({
  label,
  value,
  models,
  onChange,
  suggestions = [],
}: {
  label: string;
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
      <Input aria-label={label} value={value} onChange={(e) => onChange(e.target.value)} />
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
