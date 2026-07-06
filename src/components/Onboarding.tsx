import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { AlchemySymbol } from "./AlchemyHero";
import { Button, Input } from "./ui";
import { cn } from "@/lib/utils";
import type { ModelStatus } from "@/lib/types";
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
      className="inline-flex items-center gap-1.5 rounded-md border border-border bg-surface-2 px-2 py-1 font-mono text-[11.5px] text-foreground/85 transition-colors hover:border-border-strong"
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

function StatusIcon({ ok, optional }: { ok: boolean; optional?: boolean }) {
  if (ok) return <CheckCircle2 className="h-4 w-4 shrink-0 text-success" />;
  if (optional) return <Circle className="h-4 w-4 shrink-0 text-subtle-foreground" />;
  return <XCircle className="h-4 w-4 shrink-0 text-destructive" />;
}

function Step({
  ok,
  optional,
  title,
  detail,
  children,
}: {
  ok: boolean;
  optional?: boolean;
  title: string;
  detail?: string;
  children?: React.ReactNode;
}) {
  return (
    <div
      className={cn(
        "flex flex-col gap-2 rounded-lg border px-4 py-3",
        ok ? "border-border bg-surface/50" : "border-border-strong bg-surface",
      )}
    >
      <div className="flex items-center gap-2.5">
        <StatusIcon ok={ok} optional={optional} />
        <span className="text-[13px] font-medium text-foreground">{title}</span>
        {optional && (
          <span className="rounded border border-border px-1 py-px text-[10px] uppercase tracking-wide text-subtle-foreground">
            Optional
          </span>
        )}
      </div>
      {!ok && detail && <p className="pl-6.5 text-[12px] text-muted-foreground">{detail}</p>}
      {!ok && children && <div className="flex flex-wrap items-center gap-1.5 pl-6.5">{children}</div>}
    </div>
  );
}

/** First-run / broken-setup guide: Ollama + required models, with live rechecks. */
export function Onboarding({ onOpenSettings }: { onOpenSettings: () => void }) {
  const health = useStore((s) => s.modelHealth);
  const aiConfig = useStore((s) => s.aiConfig);
  const save = useStore((s) => s.saveAiConfig);
  const dismiss = useStore((s) => s.dismissOnboarding);
  const refresh = useStore((s) => s.refreshModelHealth);
  const [checking, setChecking] = useState(false);
  const [gwUrl, setGwUrl] = useState("");
  const [gwKey, setGwKey] = useState("");
  const [gwModel, setGwModel] = useState("");
  const [gwSaving, setGwSaving] = useState(false);
  const [gwModels, setGwModels] = useState<string[]>([]);
  const [gwStatus, setGwStatus] = useState<string | null>(null);

  // Seed gateway drafts from config once it loads.
  useEffect(() => {
    if (aiConfig) {
      setGwUrl((v) => v || aiConfig.openaiBaseUrl);
      setGwKey((v) => v || aiConfig.openaiApiKey);
      setGwModel((v) => v || aiConfig.openaiChatModel);
    }
  }, [aiConfig]);

  const provider = aiConfig?.provider ?? "ollama";

  async function setProvider(p: string) {
    if (!aiConfig) return;
    await save({ ...aiConfig, provider: p });
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
    });
    if (model) {
      // Let the success state land before health flips the overlay away.
      setGwStatus(`Connected — using ${model}`);
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
    <div className="fixed inset-0 z-40 flex items-center justify-center overflow-y-auto bg-background">
      <div className="flex w-full max-w-[520px] flex-col gap-5 px-6 py-10">
        <div className="flex flex-col items-center gap-3 text-center">
          <AlchemySymbol className="h-14 w-14 text-citation" />
          <h1 className="font-serif text-[26px] font-medium tracking-[0.14em] text-foreground">
            Set up Alchemy
          </h1>
          <p className="max-w-sm text-[13px] leading-relaxed text-muted-foreground">
            {provider === "openai" ? (
              <>
                Connect an OpenAI-compatible gateway. Your sources are indexed
                locally — only your chat prompts are sent to the gateway.
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
                and two local models — nothing leaves your computer.
              </>
            )}
          </p>
        </div>

        <div className="grid grid-cols-2 gap-1.5">
          {[
            { id: "ollama", label: "Ollama", note: "Local & private" },
            { id: "openai", label: "OpenAI-compatible", note: "Cloud or enterprise gateway" },
          ].map((pv) => (
            <button
              key={pv.id}
              onClick={() => void setProvider(pv.id)}
              className={cn(
                "flex flex-col items-start gap-0.5 rounded-lg border px-3 py-2 text-left transition-colors",
                provider === pv.id
                  ? "border-primary/60 bg-primary/10 text-foreground"
                  : "border-border bg-surface text-muted-foreground hover:text-foreground",
              )}
            >
              <span className="text-[13px] font-medium">{pv.label}</span>
              <span className="text-[11px] text-subtle-foreground">{pv.note}</span>
            </button>
          ))}
        </div>

        {provider === "openai" && (
          <div className="flex flex-col gap-1.5 rounded-lg border border-border-strong bg-surface px-4 py-3">
            <span className="text-[13px] font-medium text-foreground">Gateway</span>
            <Input
              value={gwUrl}
              onChange={(e) => setGwUrl(e.target.value)}
              placeholder="Gateway URL (usually ends in /v1)"
            />
            <Input
              type="password"
              value={gwKey}
              onChange={(e) => setGwKey(e.target.value)}
              placeholder="API key"
            />
            <div className="flex gap-1.5">
              {gwModels.length > 0 ? (
                <select
                  value={gwModel}
                  onChange={(e) => setGwModel(e.target.value)}
                  className="h-8 w-full appearance-none rounded-md border border-input bg-surface-2 px-2.5 text-[13px] text-foreground outline-none transition-colors focus:border-ring/60"
                >
                  {!gwModel && <option value="">Choose a model…</option>}
                  {(gwModels.includes(gwModel) || !gwModel ? gwModels : [gwModel, ...gwModels]).map(
                    (m) => (
                      <option key={m} value={m}>
                        {m}
                      </option>
                    ),
                  )}
                </select>
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
                onClick={() => void saveGateway()}
                loading={gwSaving}
                disabled={!gwKey.trim() && !gwUrl.trim()}
              >
                Save & check
              </Button>
            </div>
            <span
              className={cn(
                "text-[11px]",
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
          {!(provider === "openai" && aiConfig?.embedder === "builtin") && (
          <Step
            ok={health.reachable}
            title={provider === "openai" ? "Ollama is running (for source indexing)" : "Ollama is running"}
            detail="Install Ollama, then start it. Alchemy connects to it locally."
          >
            <CommandChip command="brew install ollama" />
            <CommandChip command="ollama serve" />
            <button
              className="text-[12px] text-citation hover:underline"
              onClick={() => void openUrl("https://ollama.com/download")}
            >
              or download the app
            </button>
          </Step>
          )}

          <Step
            ok={provider === "openai" ? chat.working : health.reachable && chat.working}
            title={provider === "openai" ? "Gateway connected" : "Chat model"}
            detail={
              provider === "openai"
                ? chat.detail
                : health.reachable
                  ? `Answers questions and generates documents. ${chat.detail}`
                  : "Waiting for Ollama."
            }
          >
            {provider !== "openai" && health.reachable && (
              <CommandChip command={`ollama pull ${chat.name}`} />
            )}
            {provider !== "openai" && health.reachable && (
              <button className="text-[12px] text-citation hover:underline" onClick={onOpenSettings}>
                or pick a smaller model
              </button>
            )}
          </Step>

          <Step
            ok={embed.working}
            title={aiConfig?.embedder === "builtin" ? "Built-in embedder" : "Embedding model"}
            detail={
              aiConfig?.embedder === "builtin"
                ? embed.detail
                : health.reachable
                  ? `Indexes your sources for retrieval (274 MB). ${embed.detail}`
                  : "Waiting for Ollama."
            }
          >
            {aiConfig?.embedder !== "builtin" && health.reachable && (
              <CommandChip command={`ollama pull ${embed.name}`} />
            )}
          </Step>

          <Step
            ok={health.reachable && vision.working}
            optional
            title="Vision model"
            detail="Enables OCR for images and scanned PDFs. Skip it if you don't need that."
          >
            {health.reachable && (
              <CommandChip command={`ollama pull ${vision.name || "glm-ocr"}`} />
            )}
          </Step>
        </div>

        <div className="flex items-center justify-between">
          <span className="text-[11.5px] text-subtle-foreground">
            Rechecks automatically every few seconds.
          </span>
          <div className="flex items-center gap-2">
            <Button variant="ghost" size="sm" onClick={dismiss}>
              Continue anyway
            </Button>
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
  );
}
