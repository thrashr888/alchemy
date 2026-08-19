import { useEffect, useState, type ReactNode } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { playDone } from "@/lib/sound";
import { checkForUpdates, type UpdateFlow } from "@/lib/updates";
import { Button, Input, Modal, Spinner, Switch } from "./ui";
import { openUrl } from "@tauri-apps/plugin-opener";
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
import type { AiConfig, ConnectorStatus, McpStatus } from "@/lib/types";
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
  const [saving, setSaving] = useState(false);
  const [confirmReembed, setConfirmReembed] = useState(false);

  useEffect(() => {
    if (open) setTab(initialTab);
  }, [open, initialTab]);

  useEffect(() => {
    if (open && aiConfig) {
      setDraft({ ...aiConfig });
      void refreshModelHealth();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, aiConfig]);

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
      {/* Content-sized up to the window: short tabs (About) sit at the nav's
          natural height, long tabs (Models, Appearance, Agents) grow to the
          cap and scroll only past it. The scroll cap MUST be a definite
          height on the scrolling column itself — a percentage (max-h-full)
          collapses here because the panel is capped by max-h, not a fixed
          height, so overflow-y-auto never gets a bound (that was the
          "long tabs don't scroll" regression). 8.5rem clears the header,
          body padding, and the models tab's Save footer. bodyScroll={false}
          keeps the modal body from scrolling too, so exactly one region
          moves. "Settings" is the nav's section header (no title bar). */}
      <div className="flex gap-5">
        <nav className="flex w-36 shrink-0 flex-col gap-0.5">
          <h2 className="px-2.5 pb-2 pt-0.5 text-body font-semibold text-foreground">
            Settings
          </h2>
          {TABS.map((t) => (
            <button
              type="button"
              key={t.id}
              onClick={() => setTab(t.id)}
              aria-current={tab === t.id ? "page" : undefined}
              className={cn(
                "flex items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[0.78125rem] transition-colors",
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

        <div className="flex min-w-0 flex-1 flex-col">
          {/* Pane header: names the active tab and pushes the content below
              the modal's floating close button (which otherwise collides
              with the first row's trailing switch). pr-10 keeps a long tab
              name from running under the X. */}
          <h2 className="pb-3 pl-1 pr-10 pt-0.5 text-body font-semibold text-foreground">
            {TABS.find((t) => t.id === tab)?.label}
          </h2>
          {/* The scroll cap MUST stay a definite height on this column (see
              the note above); 11rem additionally clears the pane header. */}
          <div className="flex max-h-[calc(92vh-11rem)] min-w-0 flex-col gap-4 overflow-y-auto px-1">
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
      </div>

      <Modal
        open={confirmReembed}
        onClose={cancelSwitch}
        title="Switch embedding model?"
      >
        <div className="flex flex-col gap-4">
          <p className="text-body leading-relaxed text-muted-foreground">
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
/** A macOS-style settings row: label and hint leading, Switch trailing. */
function SettingRow({
  label,
  hint,
  checked,
  onChange,
}: {
  label: ReactNode;
  hint: ReactNode;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="flex cursor-pointer items-start justify-between gap-4">
      <span className="flex flex-col gap-0.5">
        <span className="text-body text-foreground">{label}</span>
        <span className="text-micro leading-relaxed text-subtle-foreground">
          {hint}
        </span>
      </span>
      {/* Centers the 16px switch on the label's 20px line box. */}
      <Switch checked={checked} onChange={onChange} className="mt-0.5" />
    </label>
  );
}

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
  // Lazy initializer: the non-lazy form re-read localStorage every render.
  const [on, setOn] = useState(
    () => localStorage.getItem(storageKey) !== "false",
  );
  return (
    <SettingRow
      label={label}
      hint={hint}
      checked={on}
      onChange={(v) => {
        localStorage.setItem(storageKey, String(v));
        setOn(v);
        if (v) onEnable?.();
      }}
    />
  );
}

/** App-level preferences: updates, notifications, sounds. */
function GeneralTab() {
  const pushToast = useStore((s) => s.pushToast);
  const [checking, setChecking] = useState(false);
  const [update, setUpdate] = useState<UpdateFlow | null>(null);
  const [installing, setInstalling] = useState(false);

  // "Check for Updates…" from the app menu lands here with the flag set;
  // the quiet startup check leaves `updateAvailable` behind — either way,
  // this tab should be showing the Install button without another click,
  // including when the quiet check completes while the tab is already open
  // (hence `updateAvailable` in the deps).
  const pendingUpdateCheck = useStore((s) => s.pendingUpdateCheck);
  const updateAvailable = useStore((s) => s.updateAvailable);
  useEffect(() => {
    // Read live values: StrictMode replays mount effects with the same
    // captured snapshot, so checking the props would double-run the check.
    const s = useStore.getState();
    // An explicit menu check always re-runs; a known-available version only
    // triggers the interactive check once (`update` holds its outcome).
    if (s.pendingUpdateCheck || (s.updateAvailable && !update)) {
      useStore.setState({ pendingUpdateCheck: false });
      void onCheck();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingUpdateCheck, updateAvailable]);

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

      <NotificationsToggle />
      <PrefToggle
        storageKey="playSounds"
        label="Play sounds"
        hint="Soft cues for finished work, new arrivals, and errors."
        onEnable={playDone}
      />
      <BackgroundToggle />
      <TrayToggle />
    </div>
  );
}

/** Everything about getting content in: Mac apps, git repositories. */
function SourcesTab() {
  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col gap-1.5">
        <div className="text-body">Mac apps</div>
        <p className="text-micro leading-relaxed text-subtle-foreground">
          Connect once to grant macOS permissions; any notebook can then add
          Calendar, Reminders, and Apple Notes as auto-syncing sources.
        </p>
        <MacConnect />
      </div>

      <div className="h-px bg-border" />

      <GitSyncSelect />

      <div className="h-px bg-border" />

      <NotionTokenField />

      <div className="h-px bg-border" />

      <WebClipperToggle />
    </div>
  );
}

const CLIPPER_URL =
  "https://chromewebstore.google.com/detail/alchemy-web-clipper/bdiidbpifneigmcknjbgolbclbbgjheh";

/** Browser-extension clip receiver: a localhost endpoint that accepts the
 *  rendered DOM the clipper scrapes from the user's logged-in tab, so private
 *  and login-walled pages capture too (docs/RFC-page-capture.md §8). */
function WebClipperToggle() {
  const aiConfig = useStore((s) => s.aiConfig);
  const saveAiConfig = useStore((s) => s.saveAiConfig);
  if (!aiConfig) return null;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="text-body">Web clipper</div>
      <SettingRow
        label="Accept pages from the browser extension"
        hint={
          <>
            The{" "}
            <button
              type="button"
              onClick={() => void openUrl(CLIPPER_URL)}
              className="text-citation hover:underline"
            >
              Alchemy Web Clipper
            </button>{" "}
            sends the page you're viewing — including login-walled pages — to
            Alchemy over a local endpoint.
          </>
        }
        checked={aiConfig.clipEnabled}
        onChange={(v) => void saveAiConfig({ ...aiConfig, clipEnabled: v })}
      />
    </div>
  );
}

/** Notion internal-integration token — pasting one makes notion.so URLs
 *  import as living page trees instead of one-shot page captures. The token
 *  is validated against the API on entry so the user sees it work. */
function NotionTokenField() {
  const aiConfig = useStore((s) => s.aiConfig);
  const saveAiConfig = useStore((s) => s.saveAiConfig);
  const [draft, setDraft] = useState<string | null>(null);
  const [check, setCheck] = useState<
    | { state: "idle" | "checking" }
    | { state: "ok"; workspace: string }
    | { state: "error"; message: string }
  >({ state: "idle" });

  // Validate whatever token is currently saved when the field mounts, so a
  // returning user sees the green check without re-typing.
  useEffect(() => {
    const saved = useStore.getState().aiConfig?.notionToken ?? "";
    if (saved) void verify(saved);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function verify(token: string) {
    const t = token.trim();
    if (!t) {
      setCheck({ state: "idle" });
      return;
    }
    setCheck({ state: "checking" });
    try {
      const workspace = await api.notionCheck(t);
      setCheck({ state: "ok", workspace });
    } catch (e) {
      setCheck({
        state: "error",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }

  if (!aiConfig) return null;
  const value = draft ?? aiConfig.notionToken;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="text-body">Notion</div>
      <Input
        type="password"
        aria-label="Notion integration token"
        placeholder="ntn_… integration token"
        value={value}
        onChange={(e) => {
          setDraft(e.target.value);
          setCheck({ state: "idle" });
        }}
        onBlur={() => {
          if (draft !== null && draft.trim() !== aiConfig.notionToken) {
            void saveAiConfig({ ...aiConfig, notionToken: draft.trim() });
            void verify(draft);
          }
          setDraft(null);
        }}
      />
      {check.state === "checking" && (
        <span className="flex items-center gap-1.5 text-caption text-subtle-foreground">
          <Spinner className="h-3 w-3" /> Checking the token…
        </span>
      )}
      {check.state === "ok" && (
        <span className="flex items-center gap-1.5 text-caption text-success">
          <CheckCircle2 className="h-3.5 w-3.5" /> Connected to{" "}
          {check.workspace}
        </span>
      )}
      {check.state === "error" && (
        <span className="text-caption leading-relaxed text-destructive/90">
          {check.message}
        </span>
      )}
      <span className="text-caption leading-relaxed text-subtle-foreground">
        Create an internal integration at{" "}
        <button
          type="button"
          onClick={() => void openUrl("https://www.notion.so/my-integrations")}
          className="text-citation hover:underline"
        >
          notion.so/my-integrations
        </button>
        , share pages with it (••• → Connections), then paste a page URL into
        any notebook. The token stays on this Mac.
      </span>
    </div>
  );
}

/** Everything about generation: templates, the note curator. */
function StudioTab() {
  const pushToast = useStore((s) => s.pushToast);
  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col gap-1.5">
        <div className="text-body">Studio templates</div>
        <p className="text-micro leading-relaxed text-subtle-foreground">
          One .md file per generator in ~/Documents/Alchemy/templates. This
          restores the default pack without touching files you've edited.
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
    <SettingRow
      label="Consolidate auto notes weekly"
      hint="Weekly, while you're away, merges chat-created notes that state the same claim. Merged notes are archived, and each notebook's Curator report lists what happened."
      checked={aiConfig.curatorConsolidate}
      onChange={(v) =>
        void saveAiConfig({ ...aiConfig, curatorConsolidate: v })
      }
    />
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
        <span className="text-body text-foreground">
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
          className="h-8 rounded-md border border-input bg-surface-2 px-2 text-body text-foreground focus:outline-none"
        >
          <option value="15">Every 15 minutes</option>
          <option value="60">Hourly</option>
          <option value="360">Every 6 hours</option>
          <option value="1440">Daily</option>
          <option value="0">Off</option>
        </select>
      </label>
      <span className="text-micro leading-relaxed text-subtle-foreground">
        Re-fetches when the branch moves, with your own git credentials —
        Alchemy stores no tokens.
      </span>
    </div>
  );
}

/** Menu bar extra on/off — lives in AiConfig so the backend applies it live. */
/** Config-backed (not localStorage) so the Night Shift's resident scheduler
 *  can honor it with no window open; mirrored to localStorage for notify()'s
 *  synchronous check. */
function NotificationsToggle() {
  const aiConfig = useStore((s) => s.aiConfig);
  const saveAiConfig = useStore((s) => s.saveAiConfig);
  if (!aiConfig) return null;
  return (
    <SettingRow
      label="Show notifications"
      hint="When imports, rebuilds, and reports finish — even with no window open."
      checked={aiConfig.showNotifications}
      onChange={(v) => {
        localStorage.setItem("showNotifications", String(v));
        void saveAiConfig({ ...aiConfig, showNotifications: v });
      }}
    />
  );
}

/** The Night Shift master switch (docs/RFC-night-shift.md). */
function BackgroundToggle() {
  const aiConfig = useStore((s) => s.aiConfig);
  const saveAiConfig = useStore((s) => s.saveAiConfig);
  if (!aiConfig) return null;
  return (
    <SettingRow
      label="Night Shift"
      hint="Runs scheduled reports and source syncing with the window closed. Off, Alchemy works only when you ask."
      checked={aiConfig.backgroundEnabled}
      onChange={(v) => void saveAiConfig({ ...aiConfig, backgroundEnabled: v })}
    />
  );
}

function TrayToggle() {
  const aiConfig = useStore((s) => s.aiConfig);
  const saveAiConfig = useStore((s) => s.saveAiConfig);
  if (!aiConfig) return null;
  return (
    <SettingRow
      label="Show menu bar icon"
      hint="Closing the window keeps Alchemy running in the menu bar; off, it quits."
      checked={aiConfig.trayEnabled}
      onChange={(v) => void saveAiConfig({ ...aiConfig, trayEnabled: v })}
    />
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
      <SettingRow
        label="Let AI agents use Alchemy (MCP)"
        hint="Agents can create notebooks, add sources, search, and write notes. The server listens on 127.0.0.1 only."
        checked={aiConfig.mcpEnabled}
        onChange={(v) => {
          void saveAiConfig({ ...aiConfig, mcpEnabled: v }).then(() =>
            // The server starts/stops on save; give it a beat before polling.
            setTimeout(refresh, 400),
          );
        }}
      />

      <div className="flex items-center gap-2 text-caption">
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
        hint="Connect writes the client's MCP config and installs the Alchemy skill where supported."
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
                <span className="text-[0.78125rem] text-foreground">{c.name}</span>
                <span className="truncate text-[0.65625rem] text-subtle-foreground">
                  {c.configPath}
                </span>
              </div>

              {c.configured ? (
                <span className="flex items-center gap-1 text-micro text-success">
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
                <span className="text-micro text-subtle-foreground">
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
            <div className="px-2.5 py-3 text-[0.71875rem] text-subtle-foreground">
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
      hint="Audio Overview speaks with Kokoro-82M, on-device TTS (~93 MB download). The generator appears once a test synthesis verifies the voices."
    >
      <div className="flex items-center gap-3 rounded-md border border-border bg-surface-2/60 px-3 py-2.5">
        <AudioLines className="h-4 w-4 shrink-0 text-muted-foreground" />
        <div className="flex min-w-0 flex-col">
          <span className={cn("text-caption font-medium", state.cls)}>
            {state.label}
          </span>
          {downloading && download && (
            <span className="text-micro tabular-nums text-subtle-foreground">
              {download.total > 0
                ? `${download.label} — ${Math.round((download.done / download.total) * 100)}% of ${(download.total / 1e6).toFixed(0)} MB`
                : `${(download.done / 1e6).toFixed(1)} MB…`}
            </span>
          )}
          {busy && !downloading && (
            <span className="text-micro text-subtle-foreground">
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
