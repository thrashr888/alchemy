import { useEffect, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { playDone } from "@/lib/sound";
import { checkForUpdates, type UpdateFlow } from "@/lib/updates";
import { Button, Modal, Spinner } from "./ui";
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
import { ModelsTab } from "./settings/ModelsTab";
import type {
  AiConfig,
  ConnectorStatus,
  McpStatus,
} from "@/lib/types";
import {
  CheckCircle2,
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
  const [saving, setSaving] = useState(false);
  const [confirmReembed, setConfirmReembed] = useState(false);

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
    try {
      const list = await api.listModels();
      setModels([...list].sort((a, b) => a.localeCompare(b)));
    } catch {
      setModels([]);
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
      tall
      bodyScroll={false}
      hideHeader
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
      {/* One scroll region: the content column. The wrapper is a flex child
          of the modal body (itself flex when bodyScroll=false), so heights
          resolve through real flex constraints — max-h-full percentages here
          silently failed in WKWebView, which is why long tabs didn't scroll.
          "Settings" is the nav's section header (no title bar / hr above). */}
      <div className="flex min-h-0 flex-1 gap-5">
        <nav className="flex w-36 shrink-0 flex-col gap-0.5">
          <h2 className="px-2.5 pb-2 pt-0.5 text-[13px] font-semibold text-foreground">
            Settings
          </h2>
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
            <ModelsTab
              draft={draft}
              setDraft={setDraft}
              commit={(c) => {
                setDraft(c);
                void save(c);
              }}
              models={models}
            />
          )}
          {tab === "models" && <PodcastVoicesSection />}
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
