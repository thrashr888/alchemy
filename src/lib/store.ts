import { create } from "zustand";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  getCurrentWebviewWindow,
  WebviewWindow,
} from "@tauri-apps/api/webviewWindow";
import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { isWebUrl, SUPPORTED_EXTENSIONS, visibleTitle } from "./utils";
import { applyTheme, SYSTEM_THEME, themeIsDark } from "./themes";
import { refreshEpigraph } from "./epigraph";
import { describe } from "./errors";
import { notify } from "./notify";
import { dropEntry, makeEntry, pushEntry } from "./history";
import { historyOf, mergeLoadedTurns } from "./homeChatRun";
import { claimTextUndo } from "./textUndo";
import { playArrival, playDone, playError } from "./sound";
import { autoUpdateEnabled, checkForUpdatesQuietly } from "./updates";
import { DEFAULT_CHAT_CONFIG, DEFAULT_READING_PREFS } from "./types";
import type {
  AcpAgentPane,
  AcpEntry,
  AcpPaneState,
  AppState,
  HomeSection,
  Migration,
  NavEntry,
  QueueItem,
  ReaderDoc,
} from "./storeTypes";
export type { ExternalAdd, Migration, QueueItem } from "./storeTypes";
import type {
  ChatConfig,
  Message,
  MetaTurn,
  Note,
  ReadingPrefs,
  Source,
} from "./types";

/** Home conversations are identified client-side: the id has to exist before
 *  the first question is asked, because an in-flight answer is keyed to it
 *  (see `openHomeThread`). Nothing is written until a turn settles, so an
 *  id nobody asks into simply never becomes a thread. */
function newThreadId(): string {
  return crypto.randomUUID();
}

/** Can an undo toast bring this source back by re-importing its origin?
 *  Connector types (git/notion/obsidian) carry setup state a re-add can't
 *  reproduce — their delete keeps the confirm dialog instead. */
export function sourceRestorable(s: Source): boolean {
  return !["git", "notion", "obsidian"].includes(s.sourceType);
}

/** The undo half of the source-remove toast: re-import from the origin the
 *  dialog copy always promised was untouched. Pasted text has no origin, so
 *  its content rides in as a pre-delete snapshot. Fresh id, fresh embed;
 *  tags and the user's note are re-applied after. */
async function restoreSource(
  nb: string,
  s: Source,
  text: string | undefined,
): Promise<Source | null> {
  let restored: Source;
  if (s.sourceType === "mac") {
    const m = /^cider:\/\/([^/]+)\/[^/]+\/(.+)$/.exec(s.url);
    if (!m) return null;
    restored = await api.addSourceMac(nb, m[1], m[2], s.title);
  } else if (s.sourceType === "folder") {
    restored = await api.addSourceFolder(nb, s.url);
  } else if (s.sourceType === "url" || isWebUrl(s.url)) {
    restored = await api.addSourceUrl(nb, s.url);
  } else if (s.url) {
    restored = await api.addSourceFile(nb, s.url);
  } else {
    restored = await api.addSourceText(nb, s.title, text ?? "");
  }
  if (s.tags) await api.setSourceTags(restored.id, s.tags);
  if (s.note) await api.setSourceNote(restored.id, s.note);
  return restored;
}

/** The one guarded source-remove path (DESIGN.md §9: undo beats confirm).
 *  Restorable sources delete immediately — the toast carries the undo.
 *  Connector sources, which an undo can't rebuild, still ask first; this is
 *  the single copy of that dialog for every call site. */
export async function removeSourcesGuarded(
  ids: string[],
  confirmFn: (opts: {
    title: string;
    message?: string;
    items?: string[];
    confirmLabel?: string;
    danger?: boolean;
  }) => Promise<boolean>,
): Promise<void> {
  const sources = useStore.getState().sources.filter((s) => ids.includes(s.id));
  const blocked = sources.filter((s) => !sourceRestorable(s));
  if (blocked.length > 0) {
    const ok = await confirmFn({
      title:
        ids.length === 1
          ? `Remove “${sources[0]?.title ?? "source"}”?`
          : `Remove ${ids.length} sources?`,
      message:
        "Connected sources can't be brought back by undo — restoring means reconnecting. Nothing on disk is touched.",
      items: blocked.map((s) => s.title),
      confirmLabel: "Remove",
      danger: true,
    });
    if (!ok) return;
  }
  await useStore.getState().deleteSourcesBatch(ids);
}

// Side panels stay usable at any drag position: wide enough for content,
// narrow enough to leave the chat column room at the 1040px minimum window.
const PANEL_BOUNDS = { sources: [220, 400], studio: [260, 460] } as const;
const CHAT_PAGE_SIZE = 80;

function clampPanel(panel: "sources" | "studio", width: number): number {
  const [min, max] = PANEL_BOUNDS[panel];
  return Math.round(Math.min(max, Math.max(min, width)));
}

/** Load a notebook's persisted source selection (null = all selected). */
function loadSourceSel(notebookId: string): Record<string, boolean> | null {
  try {
    const raw = localStorage.getItem(`sourceSel:${notebookId}`);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

/** Home chat's persisted style and length. Same shape and same key grammar as
 *  a notebook's `chatConfig:<id>`, under the surface's own name — Home is a
 *  place you ask from, not a notebook, so it keeps its own answer voice. */
const HOME_CHAT_CONFIG_KEY = "homeChatConfig";

/** Unsent Home composer text, one map under one key rather than a key per
 *  thread — conversations are cheap to make and the sprawl would outlive
 *  them. Pruned when a thread is deleted. */
const HOME_DRAFTS_KEY = "homeChatDrafts";

function loadHomeDrafts(): Record<string, string> {
  try {
    const raw = localStorage.getItem(HOME_DRAFTS_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    if (!parsed || typeof parsed !== "object") return {};
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(parsed as Record<string, unknown>))
      if (typeof v === "string" && v) out[k] = v;
    return out;
  } catch {
    return {};
  }
}

function loadHomeChatConfig(): ChatConfig {
  try {
    const raw = localStorage.getItem(HOME_CHAT_CONFIG_KEY);
    return raw
      ? { ...DEFAULT_CHAT_CONFIG, ...JSON.parse(raw) }
      : DEFAULT_CHAT_CONFIG;
  } catch {
    return DEFAULT_CHAT_CONFIG;
  }
}

/** Persist a notebook's source selection; null (all selected) clears the key. */
function saveSourceSel(
  notebookId: string | null,
  sel: Record<string, boolean> | null,
) {
  if (!notebookId) return;
  if (sel === null) localStorage.removeItem(`sourceSel:${notebookId}`);
  else localStorage.setItem(`sourceSel:${notebookId}`, JSON.stringify(sel));
}

/** Load a notebook's persisted hosted-agent pane (transcript + agent choice),
 *  so an app restart reopens on the last session's view. */
function loadAcpPane(notebookId: string): AcpPaneState | null {
  try {
    const raw = localStorage.getItem(`acpPane:${notebookId}`);
    if (!raw) return null;
    const stored = JSON.parse(raw) as Partial<AcpPaneState> & {
      /** Pre-per-agent shape: one transcript for the whole notebook. */
      entries?: AcpEntry[];
    };
    const agentId = stored.agentId ?? null;
    // Migrate the flat shape by filing its transcript under the agent that
    // was selected when it was written — that is whose conversation it was.
    if (!stored.agents && Array.isArray(stored.entries)) {
      return {
        agentId,
        agents: agentId
          ? {
              [agentId]: {
                entries: stored.entries,
                draft: "",
                sessionId: null,
              },
            }
          : {},
      };
    }
    return stored.agents ? { agentId, agents: stored.agents } : null;
  } catch {
    return null;
  }
}

/** One agent's slice, or an empty one — the shape every writer starts from. */
function acpAgentPane(
  panes: Record<string, AcpPaneState>,
  notebookId: string,
  agentId: string,
): AcpAgentPane {
  return (
    panes[notebookId]?.agents[agentId] ?? { entries: [], draft: "", sessionId: null }
  );
}

/** Entries stream token by token, so writes coalesce behind a short timer
 *  rather than hitting localStorage on every chunk. */
const acpSaveTimers = new Map<string, number>();
function saveAcpPaneSoon(notebookId: string, pane: AcpPaneState) {
  clearTimeout(acpSaveTimers.get(notebookId));
  acpSaveTimers.set(
    notebookId,
    window.setTimeout(() => {
      acpSaveTimers.delete(notebookId);
      try {
        localStorage.setItem(`acpPane:${notebookId}`, JSON.stringify(pane));
      } catch {
        // Quota or private-mode noise; the transcript is a convenience.
      }
    }, 300),
  );
}

/** Glass chrome: native vibrancy under the window + the html.glass CSS
 *  switch that lifts panel backgrounds so the blur shows through. The
 *  style attribute picks the opacity level (macOS Clear/Tinted). */
function applyGlass(
  enabled: boolean,
  dark: boolean,
  style: "tinted" | "clear",
  pinned: boolean,
) {
  const root = document.documentElement;
  root.classList.toggle("glass", enabled);
  if (enabled) root.dataset.glass = style;
  else delete root.dataset.glass;
  // pinned=false for the System theme: appearance pinning is app-global
  // on macOS and would stop prefers-color-scheme from following the OS.
  void api.setWindowGlass(enabled, dark, pinned).catch(() => {
    root.classList.remove("glass");
    delete root.dataset.glass;
  });
}

function loadReadingPrefs(): ReadingPrefs {
  try {
    const raw = localStorage.getItem("readingPrefs");
    return raw
      ? { ...DEFAULT_READING_PREFS, ...JSON.parse(raw) }
      : DEFAULT_READING_PREFS;
  } catch {
    return DEFAULT_READING_PREFS;
  }
}

/** Note read-state, merging the earlier reports-only key on first load. */
function loadNoteReads(): Record<string, number> {
  try {
    return {
      ...JSON.parse(localStorage.getItem("reportReads") ?? "{}"),
      ...JSON.parse(localStorage.getItem("noteReads") ?? "{}"),
    };
  } catch {
    return {};
  }
}

/** The read horizon is stamped once, on the first launch with read tracking. */
function loadNoteReadsBaseline(): number {
  const v = Number(localStorage.getItem("noteReadsBaseline") ?? 0);
  if (v > 0) return v;
  const now = Date.now();
  localStorage.setItem("noteReadsBaseline", String(now));
  return now;
}

// Global Tauri event listeners bind once per page — React StrictMode runs
// init() twice in dev, and a doubled menu listener spawns doubled windows.
let listenersBound = false;
// appendToken's per-frame buffer — plumbing, not state (see appendToken).
let tokenBuffer = "";
let tokenFlushHandle = 0;
// The artifact stream's twin of the chat token buffer.
let artifactBuffer = "";
let artifactFlushHandle = 0;
// The Home (corpus) stream's twin of the same, and the promise of the run
// currently holding the meta channel. The backend answers one corpus question
// per window — `meta:<window>` is a single cancellation scope and meta://token
// a single event channel — so a new question waits for the old one to hand the
// channel back rather than interleaving with it.
let metaBuffer = "";
let metaFlushHandle = 0;
let metaRun: Promise<void> | null = null;
let metaSeq = 0;
/** Conversations deleted out from under a run in flight. Its turns are
 *  dropped rather than written — persisting them would resurrect the thread
 *  the user just deleted. */
const abandonedThreads = new Set<string>();
// folder://progress arrives once per ingested file; coalesce to one store
// write per frame so a 5,000-file import doesn't mean 5,000 full re-renders
// of the sources panel.
let folderScanPending: { done: number; total: number; title: string } | null =
  null;
let folderScanFlushHandle = 0;
// mcp://changed arrives once per agent tool call; one trailing notebooks
// refresh (a full list + native menu rebuild) covers a burst of them.
let notebooksRefreshTimer: ReturnType<typeof setTimeout> | null = null;
// True while navBack/navForward replays a history entry, so the location
// subscriber doesn't record the replay as a fresh navigation.
let navApplying = false;
// Monotonic toast ids (avoids Date.now collisions on rapid toasts).
let toastSeq = 0;

type Getter = () => AppState;
type Setter = (partial: Partial<AppState>) => void;

/** Drive one queue item through processing → done/error and auto-clear successes. */
async function runQueued(
  get: Getter,
  set: Setter,
  item: QueueItem,
  fn: () => Promise<unknown>,
) {
  const patch = (p: Partial<QueueItem>) =>
    set({
      ingestQueue: get().ingestQueue.map((q) =>
        q.id === item.id ? { ...q, ...p } : q,
      ),
    });
  patch({ status: "processing", error: undefined, retry: undefined });
  try {
    await fn();
    patch({ status: "done" });
    setTimeout(() => get().clearQueueItem(item.id), 2000);
  } catch (e) {
    patch({
      status: "error",
      error: e instanceof Error ? e.message : String(e),
      // A failed import keeps its work attached — Retry re-runs it in place.
      retry: () => void runQueued(get, set, item, fn),
    });
    playError();
  }
}

/** One-shot guard for `init`. React StrictMode double-invokes the mount
 *  effect in dev, so without this the whole boot (notebook select, schedulers,
 *  global listeners, the update check) ran twice. Module scope, so it
 *  survives the StrictMode remount. */
let initStarted = false;

/** Resolves once init's first `listNotebooks` has landed. OS entry points
 *  (deep links from the browser extension, Services, the tray) can fire
 *  before that — the backend buffers them and replays on listener bind, which
 *  happens a few lines ahead of the load — so anything that reasons about
 *  which notebooks EXIST has to wait here first. */
let markNotebooksLoaded: () => void = () => {};
const notebooksLoaded = new Promise<void>((resolve) => {
  markNotebooksLoaded = resolve;
  // If boot fails before the load lands, the capture should still get its
  // (worse) answer rather than hanging on a promise that never settles.
  setTimeout(resolve, 10_000);
});

export const useStore = create<AppState>((set, get) => {
  /** Run an async action, surfacing any failure as the global error instead of
   *  swallowing it (unhandled rejection = the UI silently does nothing). */
  const guard = async (fn: () => Promise<void>) => {
    try {
      await fn();
    } catch (e) {
      set({ error: e instanceof Error ? e.message : String(e) });
    }
  };

  /** Source ids to send over IPC: null when everything is selected (the
   *  backend searches all), otherwise the ready non-folder ids still selected
   *  (an empty array retrieves nothing — the user deselected everything). */
  const selectedIdsForIpc = (): string[] | null => {
    const sel = get().selectedSourceIds;
    if (sel === null) return null;
    return get()
      .sources.filter(
        (s) =>
          s.status === "ready" &&
          s.sourceType !== "folder" &&
          sel[s.id] !== false,
      )
      .map((s) => s.id);
  };

  return {
    notebooks: [],
    currentId: null,
    sources: [],
    selectedSourceIds: null,
    picked: null,
    hygiene: [],
    messages: [],
    messagesHasMore: false,
    messagesLoadingOlder: false,
    notes: [],
    reportSchedules: [],
    templates: [],
    aiConfig: null,
    ollamaOk: null,
    modelHealth: null,
    modelStats: [],
    // Fresh installs follow the OS appearance; an explicit pick sticks.
    theme: localStorage.getItem("theme") ?? SYSTEM_THEME,
    reading: loadReadingPrefs(),

    sending: false,
    sendingFor: null,
    streamingText: "",
    steps: [],
    waiting: "",
    agentMode: localStorage.getItem("agentMode") === "true",
    chatConfig: DEFAULT_CHAT_CONFIG,
    followups: [],
    summary: "",
    summaryLoading: false,
    generatingKind: null,
    generatingFor: null,
    generatingTemplateId: null,
    ingestQueue: [],
    migration: null,
    draggingFiles: false,
    sourcesOpen: localStorage.getItem("sourcesOpen") !== "false",
    studioOpen: localStorage.getItem("studioOpen") !== "false",
    sourcesWidth: clampPanel(
      "sources",
      Number(localStorage.getItem("sourcesWidth")) || 280,
    ),
    studioWidth: clampPanel(
      "studio",
      Number(localStorage.getItem("studioWidth")) || 320,
    ),
    onboardingDismissed: localStorage.getItem("onboardingDismissed") === "true",
    settingsOpen: false,
    settingsTab: "general",
    paletteOpen: false,
    addSourceOpen: false,
    addSourceStep: null,
    macAvailable: null,
    pendingAddUrl: false,
    pendingAddText: false,
    pendingExternalAdd: null,
    pendingUpdateCheck: false,
    updateAvailable: null,
    embedderDownload: null,
    failedInput: null,
    pendingInput: null,
    pendingAsk: null,
    findBump: 0,
    importOkfOpen: false,
    pendingImportPath: null,
    error: null,
    toasts: [],
    notebookLoading: false,
    notebooksFailed: false,
    undoStack: [],
    redoStack: [],
    justCreatedNoteId: null,
    // Center-column Ledger mode (Chat | Reader | Ledger) + a bump counter
    // the pane watches so agent writes appear live (mcp://changed).
    ledgerOpen: false,
    galleryOpen: false,
    growOpen: false,
    readerEditIntent: null,
    ledgerBump: 0,
    registryBump: 0,
    homeSection: "notebooks",
    homeChat: { threadId: null, turns: [] },
    homeRun: null,
    homeDrafts: loadHomeDrafts(),
    homeThreads: [],
    homeChatConfig: loadHomeChatConfig(),
    homeView:
      (localStorage.getItem("homeView") as "grid" | "table") || "grid",
    homeQuery: "",
    openCardId: null,
    registryCreating: false,
    pendingNewNote: false,
    artifactStreamText: "",
    audioProgress: null,
    kokoroStatus: null,
    kokoroBusy: false,
    reader: { open: false, history: [], index: -1 },
    // Home is always the floor of the app-level history; restores and
    // navigations stack on top of it via the location subscriber below.
    nav: {
      stack: [{ nb: null, mode: "chat", section: "notebooks" }],
      index: 0,
    },
    folderScan: null,
    importingFolders: [],
    noteReads: loadNoteReads(),
    noteReadsBaseline: loadNoteReadsBaseline(),

    init: async () => {
      // Runs once per launch even though StrictMode fires the effect twice.
      if (initStarted) return;
      initStarted = true;
      // Deferred: inserting NSGlassEffectView while WKWebView is still
      // doing its first paint can blank the webview for the whole session
      // (setTimeout, not rAF — rAF stalls in occluded windows).
      if (get().reading.glass)
        window.setTimeout(() => {
          // Re-check at fire time: the user may have toggled glass off
          // during the deferral window.
          const { reading, theme } = get();
          if (reading.glass)
            applyGlass(
              true,
              themeIsDark(theme),
              reading.glassStyle,
              theme !== "system",
            );
        }, 600);
      // System theme + glass: the OS appearance flip re-tints the material
      // (the window itself is unpinned and follows the OS).
      window
        .matchMedia("(prefers-color-scheme: dark)")
        .addEventListener("change", () => {
          const { reading, theme } = get();
          if (reading.glass && theme === "system")
            applyGlass(true, themeIsDark(theme), reading.glassStyle, false);
        });
      applyTheme(get().theme);
      // Daily epigraph: regenerate in the background if stale; shows next open.
      void refreshEpigraph(get().theme);
      // Every page load (incl. dev reloads) resets the macOS stoplights to
      // their default position — put them back first thing.
      void api.fixTrafficLights();
      if (!listenersBound) {
        listenersBound = true;
        get().bindGlobalListeners();
      }
      // Settled, not all-or-nothing. These used to share one Promise.all
      // rejection: a single slow read — and the notebook list is a cold scan
      // of three tables, which on a large library can approach the IPC
      // timeout — took the whole boot down with it, leaving the shelf
      // indistinguishable from a brand-new install. Forever, since init does
      // not run twice.
      const settled = <T,>(p: Promise<T>) =>
        p.then(
          (value) => ({ ok: true as const, value }),
          (err: unknown) => ({ ok: false as const, err }),
        );
      const [notebooksR, aiConfigR, ollamaOk, templates] = await Promise.all([
        settled(api.listNotebooks()),
        settled(api.getAiConfig()),
        api.checkOllama().catch(() => false),
        // Templates are global (a user folder), not per-notebook. A read failure
        // just hides the section — never blocks boot.
        api.listTemplates().catch(() => []),
      ]);
      const aiConfig = aiConfigR.ok ? aiConfigR.value : get().aiConfig;
      const notebooks = notebooksR.ok ? notebooksR.value : [];
      set({
        notebooks,
        notebooksFailed: !notebooksR.ok,
        aiConfig,
        ollamaOk,
        templates,
      });
      if (!notebooksR.ok) {
        set({ error: describe(notebooksR.err) });
      } else if (!aiConfigR.ok) {
        set({ error: describe(aiConfigR.err) });
      }
      // Releases any OS entry point that arrived before the corpus was known.
      markNotebooksLoaded();
      // showNotifications lives in config now (the Night Shift's resident
      // scheduler reads it backend-side, as does notify()'s send_notification
      // path). Honor a pre-migration localStorage opt-out once, then mirror
      // config down so the legacy key can't re-trigger this migration.
      // Skipped entirely when the config read failed: mirroring a config we
      // never loaded would write defaults over the user's real settings.
      if (aiConfig) {
        if (
          localStorage.getItem("showNotifications") === "false" &&
          aiConfig.showNotifications
        ) {
          void api.setAiConfig({ ...aiConfig, showNotifications: false });
        } else {
          localStorage.setItem(
            "showNotifications",
            String(aiConfig.showNotifications),
          );
        }
        // Quiet-while-focused mirrors config → localStorage for the sound
        // module's synchronous check (desktop notifications read config
        // directly, backend-side).
        localStorage.setItem(
          "quietWhenFocused",
          String(aiConfig.quietWhenFocused),
        );
      }
      void get().refreshModelHealth();
      void get().refreshModelStats();
      void get().refreshKokoroStatus();
      void get().refreshHomeThreads();
      // One-shot probe: are the Mac providers (cider) installed and reachable?
      void api
        .macAvailable()
        .catch(() => false)
        .then((macAvailable) => set({ macAvailable }));
      // Secondary windows boot into the notebook the opener asked for (or a
      // fresh home screen); the main window reopens the last-used notebook.
      const boot = window.__ALCHEMY_NOTEBOOK__;
      if (boot && notebooks.some((n) => n.id === boot)) {
        await get().selectNotebook(boot);
      } else if (!window.__ALCHEMY_FRESH__ && !boot) {
        // Restore the precise last view: the dashboard stays the dashboard
        // (an explicit lastView with nb: null beats lastNotebookId), and a
        // notebook reopens in its center mode — chat, reader, or ledger.
        let view: {
          nb: string | null;
          mode: "chat" | "reader" | "ledger" | "gallery";
          doc?: ReaderDoc;
          readerHistory?: ReaderDoc[];
          readerIndex?: number;
          section?: HomeSection;
          card?: string | null;
          chatThread?: string | null;
        } | null = null;
        try {
          view = JSON.parse(localStorage.getItem("lastView") ?? "null");
        } catch {
          /* ignore */
        }
        // Home's section survives the reload whether or not a notebook does
        // — coming back to the dashboard should come back to the Registry if
        // that's where you were, with the same card open.
        if (view?.section)
          set({ homeSection: view.section, openCardId: view.card ?? null });
        // The conversation survives the relaunch with the section: coming
        // back to the Chat tab and finding it blank is the bug Paul hit.
        if (view?.section === "chat")
          await get().openHomeThread(view.chatThread ?? null);
        const last =
          view === null ? localStorage.getItem("lastNotebookId") : view.nb;
        if (last && notebooks.some((n) => n.id === last)) {
          await get().selectNotebook(last);
          // Restore the reader's whole back/forward stack, not just the
          // open page — ⌘[ works across a relaunch.
          const hist = (view?.readerHistory ?? []).filter(
            (d) => !!d?.type && !!d?.id,
          );
          if (hist.length > 0) {
            const index = Math.min(
              Math.max(view?.readerIndex ?? hist.length - 1, 0),
              hist.length - 1,
            );
            set({ reader: { open: view?.mode === "reader", history: hist, index } });
          }
          if (view?.mode === "ledger") set({ ledgerOpen: true });
          else if (view?.mode === "gallery") set({ galleryOpen: true });
          else if (view?.mode === "reader" && hist.length === 0 && view.doc)
            get().openInReader(view.doc);
        }
      }
      void api.rebuildAppMenu();
      // Quiet update check, once per launch, main window only.
      if (getCurrentWebview().label === "main" && autoUpdateEnabled()) {
        setTimeout(() => {
          // The title-bar UpdateBadge is the notice — it stays put, and
          // clicking it lands on Settings → General with the check already
          // run. No toast: a transient nag on top of a persistent badge.
          void checkForUpdatesQuietly((v) => set({ updateAvailable: v }));
        }, 4000);
      }
    },

    bindGlobalListeners: () => {
      // Chat streaming tokens + agent progress steps. Store-level, not in
      // ChatPanel: the stream keeps running when the user navigates to Home
      // or another notebook (where ChatPanel is unmounted), and returning
      // mid-stream must show the whole accumulated text, not a gap. Events
      // broadcast to every window — only the one with a send in flight
      // accumulates them.
      void listen<{ content: string }>("chat://token", (e) => {
        if (get().sending) get().appendToken(e.payload.content);
      });
      void listen<{ label: string; transient: boolean }>("chat://step", (e) => {
        if (get().sending) get().appendStep(e.payload.label, e.payload.transient);
      });
      // Home's corpus answer streams the same way, and for the same reason
      // lives out here rather than in the view: the run belongs to the
      // conversation it was asked in, so leaving that conversation — for
      // another thread, or for a notebook behind a citation — must not tear
      // its listeners down. A queued run hasn't been sent yet; anything
      // arriving belongs to the run it is waiting on.
      void listen<{ content: string }>("meta://token", (e) => {
        const run = get().homeRun;
        if (run && !run.queued) get().appendHomeToken(e.payload.content);
      });
      void listen<{ label: string; transient: boolean }>("meta://step", (e) => {
        const run = get().homeRun;
        if (run && !run.queued)
          get().appendHomeStep(e.payload.label, e.payload.transient);
      });
      // Verify-and-repair swaps a revised answer under the same message id
      // (backend spawn_answer_verify) — apply only when the message is in
      // this window's transcript.
      void listen<{ id: string; content: string }>("chat://revised", (e) => {
        const { messages } = get();
        if (!messages.some((m) => m.id === e.payload.id)) return;
        set({
          messages: messages.map((m) =>
            m.id === e.payload.id ? { ...m, content: e.payload.content } : m,
          ),
        });
      });
      // Built-in embedder first-use download progress (one-time ~30 MB).
      void listen<{ label: string; done: number; total: number }>(
        "embedder://progress",
        (e) => {
          const p = e.payload;
          const finished =
            p.total > 0 && p.done >= p.total && p.label === "model.safetensors";
          set({ embedderDownload: finished ? null : p });
          if (finished) setTimeout(() => set({ embedderDownload: null }), 1500);
        },
      );
      // Studio generations stream their tokens; buffer them for the live
      // preview, committing once per frame (the chat token path's shape) —
      // per-token commits re-parsed the accumulated markdown every token.
      void listen<{ content: string }>("artifact://token", (e) => {
        if (!get().generatingKind) return;
        artifactBuffer += e.payload.content;
        if (artifactFlushHandle !== 0) return;
        artifactFlushHandle = requestAnimationFrame(() => {
          artifactFlushHandle = 0;
          const chunk = artifactBuffer;
          artifactBuffer = "";
          if (!get().generatingKind || !chunk) return;
          set({ artifactStreamText: get().artifactStreamText + chunk });
        });
      });
      // Audio Overview synthesis reports per-line progress after the script.
      void listen<{ done: number; total: number }>("audio://progress", (e) => {
        if (get().generatingKind) set({ audioProgress: e.payload });
      });
      // Folder scans report per-file ingest progress; the Sources panel shows it
      // on the active queue item. The final tick (done === total) clears it.
      void listen<{ done: number; total: number; title: string }>(
        "folder://progress",
        (e) => {
          const p = e.payload;
          if (p.done >= p.total) {
            folderScanPending = null;
            set({ folderScan: null });
            return;
          }
          folderScanPending = p;
          if (folderScanFlushHandle !== 0) return;
          folderScanFlushHandle = requestAnimationFrame(() => {
            folderScanFlushHandle = 0;
            if (folderScanPending) set({ folderScan: folderScanPending });
          });
        },
      );
      // A background folder rescan changed a notebook's sources — reload the
      // list if this window is showing it, and say what changed.
      void listen<{
        notebookId: string;
        added: number;
        updated: number;
        removed: number;
        failed: number;
      }>("sources://changed", (e) => {
        const p = e.payload;
        if (get().currentId !== p.notebookId) return;
        void api.listSources(p.notebookId).then((sources) => set({ sources }));
        void get().refreshHygiene();
        const parts = [
          p.added && `${p.added} added`,
          p.updated && `${p.updated} updated`,
          p.removed && `${p.removed} removed`,
          p.failed && `${p.failed} failed`,
        ].filter(Boolean);
        if (parts.length)
          get().pushToast("info", `Folder sync: ${parts.join(", ")}`);
        playArrival();
      });
      // An agent changed something through the MCP server — refresh whatever
      // this window is looking at so the change appears live.
      void listen<{ scope: string; notebookId: string | null }>(
        "mcp://changed",
        (e) => {
          const { scope, notebookId } = e.payload;
          // A settings change (chat/MCP settings tool, error-row fix button)
          // is a config move, not an arrival — refresh quietly, no chime.
          if (scope === "settings") {
            void api.getAiConfig().then((aiConfig) => set({ aiConfig }));
            return;
          }
          // The small model finished naming a Home conversation — the Chats
          // list re-derives itself off the turns. Background bookkeeping, not
          // an arrival: nothing chimes and no notebook is re-read.
          if (scope === "homechat") {
            void get().refreshHomeThreads();
            return;
          }
          playArrival();
          // Debounced: an agent looping tool calls fires one of these per
          // call, and each refresh is a notebooks read plus a native menu
          // rebuild. One trailing refresh covers the burst.
          if (notebooksRefreshTimer === null) {
            notebooksRefreshTimer = setTimeout(() => {
              notebooksRefreshTimer = null;
              void get().refreshNotebooks();
            }, 250);
          }
          // Templates are app-global — refresh before the notebook gate.
          if (scope === "templates") void get().refreshTemplates();
          // So is the Registry (it has no notebook), and its surface is Home
          // — where currentId is null, so it must bump before that gate too.
          if (scope === "registry")
            set((state) => ({ registryBump: state.registryBump + 1 }));
          const current = get().currentId;
          if (!current || (notebookId && notebookId !== current)) return;
          if (scope === "sources")
            void api.listSources(current).then((sources) => set({ sources }));
          if (scope === "notes")
            void api.listNotes(current).then((notes) => set({ notes }));
          if (scope === "ledger")
            set((state) => ({ ledgerBump: state.ledgerBump + 1 }));
          if (scope === "reports")
            void api
              .listReportSchedules(current)
              .then((reportSchedules) => set({ reportSchedules }));
        },
      );
      // The settings tool's per-surface style verb (RFC-conversational-setup
      // §2): the validated change arrives as an event because ChatConfig is
      // frontend state, and every window has to agree. An empty notebookId
      // means Home's own config — asking across everything is a different
      // job from asking inside one notebook, and neither resets the other.
      void listen<{
        notebookId: string;
        style?: string | null;
        length?: string | null;
      }>("settings://style", (e) => {
        const { notebookId, style, length } = e.payload;
        const home = !notebookId;
        const key = home ? HOME_CHAT_CONFIG_KEY : `chatConfig:${notebookId}`;
        let cur: ChatConfig = home
          ? { ...get().homeChatConfig }
          : { ...DEFAULT_CHAT_CONFIG };
        if (!home) {
          try {
            const raw = localStorage.getItem(key);
            if (raw) cur = { ...DEFAULT_CHAT_CONFIG, ...JSON.parse(raw) };
          } catch {
            // Unreadable stored config — rebuild from the defaults.
          }
        }
        // The backend validated these against the same rosters this union
        // mirrors (selfheal::resolve_style / settings_style).
        if (style != null) cur.style = style as ChatConfig["style"];
        if (length != null) cur.length = length as ChatConfig["length"];
        if (home) {
          // One writer for Home's config, so the pills and the tool can
          // never disagree about where it is kept.
          get().setHomeChatConfig(cur);
          return;
        }
        localStorage.setItem(key, JSON.stringify(cur));
        if (get().currentId === notebookId) set({ chatConfig: cur });
      });
      // The settings tool's theme verb (§3): the resolved id travels as an
      // event and applies through the same setTheme path Settings uses.
      void listen<{ theme: string }>("settings://theme", (e) => {
        get().setTheme(e.payload.theme);
      });
      // Safety net: the backend broadcasts every finished generation. If the
      // invoke path lost the result (e.g. a long synthesis outlived a timeout),
      // this still lands the note in the list instead of losing it silently.
      void listen<Note>("generate://done", (e) => {
        const note = e.payload;
        if (get().currentId !== note.notebookId) return;
        set({ notes: [note, ...get().notes.filter((n) => n.id !== note.id)] });
      });
      // First Audio Overview downloads the Kokoro voice model (~93 MB); reuse
      // the embedder's download overlay with its own title. "done" clears it.
      void listen<{ label: string; done: number; total: number }>(
        "tts://download",
        (e) => {
          const p = e.payload;
          if (p.label === "done") {
            set({ embedderDownload: null });
            return;
          }
          set({
            embedderDownload: {
              ...p,
              title: "Downloading the Audio Overview voices",
            },
          });
        },
      );
      // App-menu actions broadcast to every window with the intended target's
      // label in the payload — each window acts only on events addressed to it.
      // (JS "Any" listeners receive every event regardless of emit target, so
      // this self-filter is what actually prevents N windows from all reacting.)
      const label = getCurrentWebview().label;
      // A note pop-out (or print-export shell) renders only the note — no
      // palette, settings, importer, or add-source modal is mounted, so a
      // menu action addressed here would flip state nothing displays. Hand
      // the action to the main window and bring it forward instead. (The
      // re-emitted payload targets "main"; every window's listener sees it,
      // but the self-filter below keeps everyone else out.)
      const isReaderShell = !!window.__ALCHEMY_NOTE__;
      const forwardMenuToMain = async (event: string, id: string) => {
        await emit(event, { target: "main", id });
        const main = await WebviewWindow.getByLabel("main");
        await main?.show();
        await main?.setFocus();
      };
      void listen<{ target: string; id: string }>("menu://action", (e) => {
        if (e.payload.target !== label) return;
        if (isReaderShell) {
          void forwardMenuToMain("menu://action", e.payload.id);
          return;
        }
        const s = get();
        if (e.payload.id === "menu-settings") s.openSettings();
        else if (e.payload.id === "menu-about") s.openSettings("about");
        else if (e.payload.id === "menu-search") s.togglePalette();
        else if (e.payload.id === "menu-check-updates") {
          set({ pendingUpdateCheck: true });
          s.openSettings("general");
        } else if (e.payload.id === "menu-new-window") void api.newWindow();
        else if (e.payload.id === "menu-add-url") {
          if (get().currentId) s.openAddSource("url");
          else s.pushToast("info", "Open a notebook first, then add sources");
        }
        else if (e.payload.id === "menu-export-okf") void s.exportNotebookOkf();
        else if (e.payload.id === "menu-import-okf")
          set({ importOkfOpen: true });
        else if (e.payload.id === "menu-back") s.navBack();
        else if (e.payload.id === "menu-forward") s.navForward();
        // Undo is app-routed because the menu accelerator eats the keystroke
        // before any keydown handler runs. A focused editor or input gets
        // first claim; only when nothing is being typed into does Cmd-Z mean
        // "reverse my last mutation".
        else if (e.payload.id === "menu-undo") {
          if (!claimTextUndo(false)) void s.undoLast();
        } else if (e.payload.id === "menu-redo") {
          if (!claimTextUndo(true)) void s.redoLast();
        } else if (e.payload.id === "menu-new-notebook") {
          // Same auto-title the palette's New Notebook uses.
          const taken = new Set(get().notebooks.map((n) => n.title));
          let title = "Untitled notebook";
          for (let i = 2; taken.has(title); i++) title = `Untitled notebook ${i}`;
          void s.createNotebook(title);
        } else if (e.payload.id === "menu-new-note") {
          if (!get().currentId) {
            s.pushToast("info", "Open a notebook first, then add a note");
            return;
          }
          // Open the panel; StudioPanel opens the composer when it mounts.
          set({ pendingNewNote: true });
          if (!get().studioOpen) s.toggleStudio();
        } else if (e.payload.id === "menu-add-files") {
          if (get().currentId) s.openAddSource();
          else s.pushToast("info", "Open a notebook first, then add sources");
        } else if (e.payload.id === "menu-find") {
          set({ findBump: get().findBump + 1 });
        } else if (e.payload.id === "menu-toggle-sources") {
          toggleNotebookPanel("sources");
        } else if (e.payload.id === "menu-toggle-studio") {
          toggleNotebookPanel("studio");
        } else if (e.payload.id === "menu-toggle-gallery") {
          if (get().currentId) toggleNotebookPanel("gallery");
          else s.pushToast("info", "Open a notebook to browse its gallery");
        } else if (e.payload.id === "menu-toggle-ledger") {
          if (get().currentId) toggleNotebookPanel("ledger");
          else s.pushToast("info", "Open a notebook to read its ledger");
        } else if (e.payload.id === "menu-toggle-glass") {
          s.setReading({ glass: !get().reading.glass });
        } else if (e.payload.id.startsWith("theme:")) {
          s.setTheme(e.payload.id.slice("theme:".length));
        } else if (e.payload.id.startsWith("generate:")) {
          if (get().currentId)
            void s.generateArtifact(
              e.payload.id.slice("generate:".length) as Note["kind"],
            );
          else s.pushToast("info", "Open a notebook first, then generate");
        } else if (e.payload.id === "menu-export-note") {
          const r = get().reader;
          const doc = r.open ? r.history[r.index] : undefined;
          const note =
            doc?.type === "note"
              ? get().notes.find((n) => n.id === doc.id)
              : undefined;
          if (!note) {
            s.pushToast("info", "Open a note in the reader to export it");
            return;
          }
          void (async () => {
            const { exportNote, exportTargets } = await import("./noteExport");
            await exportNote(note, exportTargets(note)[0]);
          })();
        } else if (e.payload.id === "menu-archive-notebook") {
          const id = get().currentId;
          if (!id) {
            s.pushToast("info", "Open a notebook to archive it");
            return;
          }
          void s.setNotebookStatus(id, "archived").then(() => s.closeNotebook());
        } else if (e.payload.id === "menu-delete-notebook") {
          const id = get().currentId;
          const nb = get().notebooks.find((n) => n.id === id);
          if (!id || !nb) {
            s.pushToast("info", "Open a notebook to delete it");
            return;
          }
          // Native confirm: this is the one delete no toast can undo, and
          // the menu has no in-app dialog host.
          void ask(
            `This permanently deletes "${nb.title}" and all of its sources.`,
            { title: `Delete "${nb.title}"?`, kind: "warning" },
          ).then((ok) => {
            if (ok) void s.deleteNotebook(id);
          });
        }
      });
      void listen<{ target: string; id: string }>(
        "menu://open-notebook",
        (e) => {
          if (e.payload.target !== label) return;
          if (isReaderShell) {
            // Opening a notebook under a pop-out note would swap the note's
            // notebook out from beneath it — route to the main window.
            void forwardMenuToMain("menu://open-notebook", e.payload.id);
            return;
          }
          void get().selectNotebook(e.payload.id);
        },
      );

      // OS entry points (deep links, tray, Services, Spotlight) all arrive
      // as alchemy:// URLs on the main window; the backend buffers anything
      // that fires before this listener is up.
      if (label === "main") {
        void listen<string>("integrations://url", (e) => {
          void get().handleIntegrationUrl(e.payload);
        });
        void listen("integrations://ask", () => {
          get().setPaletteOpen(true);
        });
        void listen<string>("integrations://add-step", (e) => {
          const step = e.payload === "text" ? "text" : "url";
          const s = get();
          if (s.currentId) {
            s.openAddSource(step);
            return;
          }
          // Capture from the menu bar shouldn't dead-end on the home
          // screen — hop into the most recent notebook and open there.
          // Skip archived and system (Briefs) — a capture should land in a
          // notebook the user actually works in.
          const recent = s.notebooks.find((n) => !n.status);
          if (!recent) {
            s.pushToast("error", "Create a notebook first, then add sources");
            return;
          }
          void s.selectNotebook(recent.id).then(() => {
            get().pushToast("info", `Adding to “${recent.title}”`);
            get().openAddSource(step);
          });
        });
        void listen<string>("integrations://toast", (e) => {
          get().pushToast("info", e.payload);
        });
        void api.integrationsReady().then((pending) => {
          for (const url of pending) void get().handleIntegrationUrl(url);
        });
      }
    },

    confirmExternalAdd: async (notebookId, payload) => {
      const add = payload ?? get().pendingExternalAdd;
      set({ pendingExternalAdd: null });
      if (!add) return;
      try {
        if (get().currentId !== notebookId)
          await get().selectNotebook(notebookId);
        if (add.files.length) await get().addSourceFiles(add.files);
        else if (add.url) await get().addSourceUrl(add.url);
        else if (add.text) await get().addSourceText(add.title ?? "", add.text);
      } catch (e) {
        get().pushToast("error", e instanceof Error ? e.message : String(e));
      }
    },

    handleIntegrationUrl: async (raw) => {
      let u: URL;
      try {
        u = new URL(raw);
      } catch {
        return;
      }
      if (u.protocol !== "alchemy:") return;
      const kind = u.hostname || u.pathname.replace(/^\/+/, "").split("/")[0];
      const tail = decodeURIComponent(u.pathname.replace(/^\/+/, ""));
      try {
        if (kind === "notebook" && tail) {
          await get().selectNotebook(tail);
        } else if (kind === "note" && tail) {
          const nb = await api.locateNote(tail);
          if (!nb) {
            get().pushToast("error", "That note no longer exists");
            return;
          }
          await get().selectNotebook(nb);
          // The just-created hook opens the note card (and marks it read).
          set({ studioOpen: true, justCreatedNoteId: tail });
        } else if (kind === "ask") {
          // The tray item and ⌥Space reach the palette over the
          // `integrations://ask` event, which no URL could trigger — so a
          // Shortcut had no way in (docs/shortcuts.md). Same destination,
          // now addressable; `q` pre-fills the question.
          // Seed the question in the SAME set as the open: the palette reads
          // pendingAsk in its open effect, so setting it afterwards would
          // arrive too late and silently drop the question (HomeView's ask
          // box sets both together for exactly this reason).
          const q = u.searchParams.get("q");
          if (q) set({ pendingAsk: q, paletteOpen: true });
          else get().setPaletteOpen(true);
        } else if (kind === "add") {
          const p = u.searchParams;
          const payload = {
            files: p.getAll("file"),
            url: p.get("url"),
            text: p.get("text"),
            title: p.get("title"),
          };
          if (!payload.files.length && !payload.url && !payload.text) return;
          // Cold start: the browser extension (or any deep link) launches the
          // app, and the backend replays the buffered URL as soon as the
          // listeners bind — which is BEFORE init's first `listNotebooks`
          // resolves. Without this wait an extension capture on a not-running
          // app reported "Create a notebook first" to someone with dozens.
          await notebooksLoaded;
          if (get().notebooks.length === 0) {
            get().pushToast(
              "error",
              "Create a notebook first, then add sources",
            );
            return;
          }
          const nb = p.get("notebook");
          if (nb) {
            // The caller named a notebook — no need to ask.
            await get().confirmExternalAdd(nb, payload);
          } else {
            // External adds can't know which notebook the user meant (there
            // may be several windows) — ask, defaulting to the most recent.
            set({ pendingExternalAdd: payload });
          }
        }
      } catch (e) {
        get().pushToast("error", e instanceof Error ? e.message : String(e));
      }
    },

    refreshModelHealth: async () => {
      try {
        set({ modelHealth: await api.checkModels() });
      } catch {
        set({ modelHealth: null });
      }
    },

    refreshModelStats: async () => {
      try {
        set({ modelStats: await api.getModelStats() });
      } catch {
        /* keep prior stats */
      }
    },

    refreshNotebooks: async () => {
      set({ notebooks: await api.listNotebooks(), notebooksFailed: false });
      void api.rebuildAppMenu();
    },

    selectNotebook: async (id) => {
      localStorage.setItem("lastNotebookId", id);
      let chatConfig: ChatConfig = DEFAULT_CHAT_CONFIG;
      try {
        const raw = localStorage.getItem(`chatConfig:${id}`);
        if (raw) chatConfig = { ...DEFAULT_CHAT_CONFIG, ...JSON.parse(raw) };
      } catch {
        /* ignore */
      }
      set({
        currentId: id,
        sources: [],
        selectedSourceIds: loadSourceSel(id),
        picked: null,
        hygiene: [],
        messages: [],
        messagesHasMore: false,
        messagesLoadingOlder: false,
        notes: [],
        reportSchedules: [],
        followups: [],
        chatConfig,
        summary: localStorage.getItem(`summary:${id}`) ?? "",
        reader: { open: false, history: [], index: -1 },
        // Every collection above was just emptied. Until the fetch lands,
        // an empty Sources list means "still loading", not "no sources" —
        // one flag, because they all arrive in the same Promise.all.
        notebookLoading: true,
      });
      const nb = get().notebooks.find((n) => n.id === id);
      if (nb) void getCurrentWebviewWindow().setTitle(`${nb.title} — Alchemy`);
      try {
        const [sources, messagePage, notes, reportSchedules] = await Promise.all(
          [
            api.listSources(id),
            api.listMessagesPage(id, undefined, CHAT_PAGE_SIZE),
            api.listNotes(id),
            api.listReportSchedules(id),
          ],
        );
        // Guarded on both paths: a slow load for a notebook the user already
        // navigated away from must not clear the newer one's flag.
        if (get().currentId === id)
          set({
            sources,
            messages: messagePage.messages,
            messagesHasMore: messagePage.hasMore,
            notes,
            reportSchedules,
            notebookLoading: false,
          });
      } catch (e) {
        if (get().currentId === id)
          set({
            notebookLoading: false,
            error: e instanceof Error ? e.message : String(e),
          });
        return;
      }
      // Catch up THIS notebook's folder and file sources right away rather
      // than waiting for the next minute tick — scoped, because the corpus-
      // wide sweep this used to fire competed with the notebook's own loads
      // and duplicated the scheduler tick already due within the minute.
      // Changes come back via sources://changed.
      void api.resyncSources(id).catch(() => {});
    },

    closeNotebook: () => {
      void getCurrentWebviewWindow().setTitle("Alchemy");
      set({
        currentId: null,
        sources: [],
        selectedSourceIds: null,
        messages: [],
        messagesHasMore: false,
        messagesLoadingOlder: false,
        notes: [],
        reportSchedules: [],
        ingestQueue: [],
        steps: [],
        waiting: "",
        reader: { open: false, history: [], index: -1 },
      });
    },

    navBack: () => void applyNav(-1),
    navForward: () => void applyNav(1),

    // ---- Home chat threads (docs/RFC-meta-chat.md) ----------------------
    // The conversation used to die with the view. It persists per thread now,
    // so the Chat tab can be left and come back to — and so back/forward can
    // land on a conversation the way it lands on a notebook.

    refreshHomeThreads: async () => {
      try {
        const homeThreads = await api.listMetaThreads();
        set({ homeThreads });
        // Drop drafts for conversations that no longer exist. A New-chat id
        // is minted before anything is asked, so an abandoned one would
        // otherwise leave its half-typed question in storage for good; the
        // thread being looked at is spared, since it may be exactly that.
        const live = new Set(homeThreads.map((t) => `t:${t.id}`));
        const open = get().homeChat.threadId;
        if (open) live.add(`t:${open}`);
        live.add("shelf");
        const drafts = get().homeDrafts;
        const kept = Object.fromEntries(
          Object.entries(drafts).filter(([k]) => live.has(k)),
        );
        if (Object.keys(kept).length !== Object.keys(drafts).length) {
          localStorage.setItem(HOME_DRAFTS_KEY, JSON.stringify(kept));
          set({ homeDrafts: kept });
        }
      } catch {
        /* the list is a way back in, not the conversation itself */
      }
    },

    newHomeThread: () => {
      // A conversation gets its id before anything is asked into it, so a run
      // is keyed to a thread that can't change under it. Nothing is written
      // until a turn settles, so an id nobody asks into never becomes a row.
      const threadId = newThreadId();
      set({ homeChat: { threadId, turns: [] } });
      return threadId;
    },

    openHomeThread: async (threadId) => {
      // Switching conversations never touches a run: the answer is being
      // written into ITS thread, and walking next door to check something is
      // not a reason to throw it away. It keeps streaming into `homeRun`,
      // settles into its own thread, and coming back shows it mid-flight.
      //
      // A fresh conversation gets its id up front, before anything is asked,
      // so the run is keyed to a conversation that can't change under it.
      if (threadId === null) {
        get().newHomeThread();
        set({ homeSection: "chat" });
        return;
      }
      if (get().homeChat.threadId !== threadId)
        set({ homeChat: { threadId, turns: [] } });
      set({ homeSection: "chat" });
      try {
        const turns = await api.listMetaTurns(threadId);
        // Switched away while it loaded — those turns belong to a thread
        // nobody is looking at any more.
        if (get().homeChat.threadId !== threadId) return;
        // An answer that settled while the list was in flight is already on
        // screen and newer than what the backend handed back; keep anything
        // the fetch doesn't know about rather than blinking it away.
        set({
          homeChat: {
            threadId,
            turns: mergeLoadedTurns(turns, get().homeChat.turns),
          },
        });
      } catch (e) {
        set({ error: describe(e) });
      }
    },

    appendHomeTurn: async (role, content, citations, kind, intoThread) => {
      const threadId = intoThread ?? get().homeChat.threadId ?? newThreadId();
      // The conversation was deleted while this was being written.
      if (abandonedThreads.has(threadId)) return;
      // A turn lands in the conversation it was asked in. Whether it also
      // lands ON SCREEN depends on which conversation is open — a run that
      // settles while you're reading another thread writes into its own.
      const showing = () => get().homeChat.threadId === threadId;
      // Optimistic: the turn is on screen before the write lands, and stays
      // there if the write fails — a lost row is worse than a lost answer,
      // but showing neither is worst.
      const pending: MetaTurn = {
        id: `pending-${newThreadId()}`,
        threadId,
        role,
        content,
        citations,
        kind,
        createdAt: Date.now(),
      };
      if (showing())
        set((s) => ({
          homeChat: { threadId, turns: [...s.homeChat.turns, pending] },
        }));
      else if (get().homeChat.threadId === null)
        // Nothing open at all (a question asked before the Chat tab was ever
        // visited): the thread being written into becomes the open one.
        set({ homeChat: { threadId, turns: [pending] } });
      try {
        const saved = await api.addMetaTurn(
          threadId,
          role,
          content,
          citations,
          kind,
        );
        set((s) =>
          s.homeChat.threadId === threadId
            ? {
                homeChat: {
                  threadId,
                  turns: s.homeChat.turns.map((t) =>
                    t.id === pending.id ? saved : t,
                  ),
                },
              }
            : {},
        );
        // The thread list's timestamp and turn count move with the write,
        // whichever conversation is being looked at.
        void get().refreshHomeThreads();
      } catch (e) {
        get().pushToast(
          "error",
          `Couldn't save this turn: ${describe(e)}`,
        );
      }
    },

    askHome: async (question) => {
      const q = question.trim();
      if (!q) return;
      // The thread id exists before the question does (openHomeThread mints
      // it), so the run is keyed to a conversation that can't change under it.
      const threadId = get().homeChat.threadId ?? newThreadId();
      const prior = historyOf(get().homeChat.turns);
      const previous = metaRun;
      // `metaSeq` is what makes a superseded run stay superseded: it decides
      // which run owns `homeRun`, so an outgoing run's settling never wipes
      // the state of the one that displaced it.
      const seq = ++metaSeq;
      // One corpus answer at a time per window. A second question doesn't
      // silently drop the first: it winds it down exactly as Stop does —
      // cancel, keep the partial, file it under its own thread — and only
      // then takes the channel. Until then this run is `queued`, which is
      // also what keeps the outgoing run's tokens out of this one's buffer.
      set({
        homeRun: {
          threadId,
          question: q,
          streaming: "",
          steps: [],
          waiting: previous ? "Finishing the previous answer…" : "",
          stopped: false,
          queued: !!previous,
        },
      });
      const clear = () => {
        if (metaSeq !== seq) return;
        metaRun = null;
        metaBuffer = "";
        set({ homeRun: null });
      };
      const run = (async () => {
        if (previous) {
          void api.cancelGeneration("meta");
          await previous.catch(() => {});
          // Displaced in turn while waiting for the channel — the newer
          // question owns `homeRun` now, so leave it alone.
          if (metaSeq !== seq) return;
          // Stop, pressed before this run ever started, means the question
          // was withdrawn: nothing was asked and nothing is recorded.
          if (get().homeRun?.stopped) return clear();
          set((s) =>
            s.homeRun
              ? { homeRun: { ...s.homeRun, queued: false, waiting: "" } }
              : {},
          );
        }
        await get().appendHomeTurn("user", q, [], "chat", threadId);
        try {
          const res = await api
            // No depth argument: the backend picks depth per model class
            // (deep rerank on gateways where the extra call is cheap,
            // single-pass local). The config is Home's own — Style, Length.
            .askEverything(q, prior, undefined, get().homeChatConfig, threadId);
          // A command ("add this url", "open the Japan notebook") was carried
          // out instead of answered. It lands as one quiet tool row, never
          // as a stopped partial — there was no stream to cut short.
          if (res.kind === "tool") {
            await get().settleHomeTool(threadId, res);
            return;
          }
          // Superseded runs settle as stopped: a question asked over the top
          // of this one cancelled it, and the partial is what it got to.
          const stopped =
            metaSeq !== seq || (get().homeRun?.stopped ?? false);
          // A stop before the first token leaves nothing to show — the user
          // already knows they cancelled.
          if (stopped && !res.answer.trim()) return;
          await get().appendHomeTurn(
            "assistant",
            res.answer,
            res.citations,
            stopped ? "stopped" : "chat",
            threadId,
          );
        } catch (e) {
          await get().appendHomeTurn(
            "assistant",
            describe(e),
            [],
            "error",
            threadId,
          );
        } finally {
          clear();
        }
      })();
      metaRun = run;
      await run;
    },

    settleHomeTool: async (threadId, answer) => {
      // Deleting this conversation is the one reply that must not be written
      // into it: the row would resurrect the thread the user just dropped.
      // The backend already removed the turns, so all that's left is what a
      // sidebar delete does — abandon the run, open a fresh conversation,
      // drop the draft — and say so where it can still be read.
      if (answer.effect?.kind === "deleteChat") {
        abandonedThreads.add(threadId);
        if (get().homeChat.threadId === threadId)
          set({ homeChat: { threadId: newThreadId(), turns: [] } });
        get().setHomeDraft(`t:${threadId}`, "");
        void get().refreshHomeThreads();
        get().pushToast("success", answer.answer);
        return;
      }
      await get().appendHomeTurn(
        "assistant",
        answer.answer,
        [],
        "tool",
        threadId,
      );
      // The backend can't drive this window; it can only say where to go.
      // One nav entry, exactly as clicking the notebook would make.
      if (answer.effect?.kind === "openNotebook" && answer.effect.notebookId) {
        const id = answer.effect.notebookId;
        await navAtomic(() => get().selectNotebook(id));
      }
    },

    stopHome: () => {
      const run = get().homeRun;
      if (!run) return;
      // Keep what arrived: the backend resolves a cancelled run with the
      // partial answer and its citations. A queued run has nothing in flight
      // to cancel — the flag withdraws it before it starts.
      set({ homeRun: { ...run, stopped: true } });
      if (!run.queued) void api.cancelGeneration("meta");
    },

    appendHomeToken: (t) => {
      // Same per-frame commit as the notebook stream: tokens arrive faster
      // than frames, and committing each one re-parses the whole answer.
      metaBuffer += t;
      if (metaFlushHandle !== 0) return;
      metaFlushHandle = requestAnimationFrame(() => {
        metaFlushHandle = 0;
        const chunk = metaBuffer;
        metaBuffer = "";
        const run = get().homeRun;
        if (!run || run.queued || !chunk) return;
        set({
          homeRun: { ...run, streaming: run.streaming + chunk, waiting: "" },
        });
      });
    },

    appendHomeStep: (label, transient) => {
      const run = get().homeRun;
      if (!run) return;
      // A transient line is a live status, not a trail entry: it replaces the
      // previous one and never accumulates.
      set({
        homeRun: transient
          ? { ...run, waiting: label }
          : { ...run, steps: [...run.steps, label], waiting: "" },
      });
    },

    setHomeDraft: (key, text) => {
      const drafts = { ...get().homeDrafts };
      if ((drafts[key] ?? "") === text) return;
      if (text) drafts[key] = text;
      else delete drafts[key];
      localStorage.setItem(HOME_DRAFTS_KEY, JSON.stringify(drafts));
      set({ homeDrafts: drafts });
    },

    deleteHomeThread: async (threadId) => {
      try {
        await api.deleteMetaThread(threadId);
      } catch (e) {
        set({ error: describe(e) });
        return;
      }
      // Deleting a conversation that is still being answered stops it and
      // throws the answer away — persisting the partial would resurrect the
      // thread the user just deleted.
      if (get().homeRun?.threadId === threadId) {
        abandonedThreads.add(threadId);
        set({ homeRun: null });
        void api.cancelGeneration("meta");
      }
      // Deleting the conversation you're reading leaves a fresh one open,
      // not an empty screen with no way forward.
      if (get().homeChat.threadId === threadId)
        set({ homeChat: { threadId: newThreadId(), turns: [] } });
      // Its unsent draft goes with it.
      get().setHomeDraft(`t:${threadId}`, "");
      await get().refreshHomeThreads();
    },

    setHomeChatConfig: (config) => {
      localStorage.setItem(HOME_CHAT_CONFIG_KEY, JSON.stringify(config));
      set({ homeChatConfig: config });
    },

    setTheme: (theme) => {
      localStorage.setItem("theme", theme);
      applyTheme(theme);
      set({ theme });
      // The native glass tint tracks the palette's lightness.
      if (get().reading.glass)
        applyGlass(
          true,
          themeIsDark(theme),
          get().reading.glassStyle,
          theme !== "system",
        );
    },

    setReading: (patch) => {
      const reading = { ...get().reading, ...patch };
      localStorage.setItem("readingPrefs", JSON.stringify(reading));
      set({ reading });
      if ("glass" in patch || "glassStyle" in patch)
        applyGlass(
          reading.glass,
          themeIsDark(get().theme),
          reading.glassStyle,
          get().theme !== "system",
        );
    },

    clearQueueItem: (id) =>
      set({ ingestQueue: get().ingestQueue.filter((q) => q.id !== id) }),

    setDraggingFiles: (v) => set({ draggingFiles: v }),

    dismissOnboarding: () => {
      localStorage.setItem("onboardingDismissed", "true");
      set({ onboardingDismissed: true });
    },

    openSettings: (tab = "general") =>
      set({ settingsOpen: true, settingsTab: tab }),
    closeSettings: () => set({ settingsOpen: false }),
    setPaletteOpen: (open) => set({ paletteOpen: open }),
    togglePalette: () => {
      const { paletteOpen, settingsOpen } = get();
      if (paletteOpen) {
        set({ paletteOpen: false });
        return;
      }
      // Explicit intent wins: an open dialog is dismissed (same as pressing
      // Escape first), never silently swallowed.
      if (settingsOpen) get().closeSettings();
      if (document.querySelector('[aria-modal="true"]')) {
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
      }
      set({ paletteOpen: true });
    },

    openAddSource: (step) =>
      set({ addSourceOpen: true, addSourceStep: step ?? null }),
    closeAddSource: () => set({ addSourceOpen: false }),

    toggleSources: () => {
      const v = !get().sourcesOpen;
      localStorage.setItem("sourcesOpen", String(v));
      set({ sourcesOpen: v });
    },
    toggleStudio: () => {
      const v = !get().studioOpen;
      localStorage.setItem("studioOpen", String(v));
      set({ studioOpen: v });
    },
    setPanelWidth: (panel, width) => {
      const w = clampPanel(panel, width);
      localStorage.setItem(
        panel === "sources" ? "sourcesWidth" : "studioWidth",
        String(w),
      );
      set(panel === "sources" ? { sourcesWidth: w } : { studioWidth: w });
    },

    createNotebook: async (title) => {
      const nb = await api.createNotebook(title);
      set({ notebooks: [nb, ...get().notebooks] });
      void api.rebuildAppMenu();
      await get().selectNotebook(nb.id);
      // Returned so callers that create-then-act (the external add picker's
      // "new notebook" suggestion) don't have to re-find it by title.
      return nb.id;
    },

    renameNotebook: (id, title) =>
      guard(async () => {
        const before = get().notebooks.find((n) => n.id === id)?.title;
        await api.renameNotebook(id, title);
        await get().refreshNotebooks();
        // Silent history: a rename needs no toast, but it is the mutation
        // people most often want back (RFC-professional-grade Pillar 5).
        if (before === undefined || before === title) return;
        const rename = async (to: string) => {
          await api.renameNotebook(id, to);
          await get().refreshNotebooks();
        };
        get().pushHistory(
          "Rename Notebook",
          () => rename(before),
          () => rename(title),
        );
      }),

    setNotebookColor: (id, color) =>
      guard(async () => {
        const prev = get().notebooks;
        set({
          notebooks: prev.map((n) => (n.id === id ? { ...n, color } : n)),
        });
        try {
          await api.setNotebookColor(id, color);
        } catch (e) {
          set({ notebooks: prev });
          await get().refreshNotebooks();
          throw e;
        }
      }),

    setNotebookIcon: (id, icon) =>
      guard(async () => {
        const prev = get().notebooks;
        set({
          notebooks: prev.map((n) => (n.id === id ? { ...n, icon } : n)),
        });
        try {
          await api.setNotebookIcon(id, icon);
        } catch (e) {
          set({ notebooks: prev });
          await get().refreshNotebooks();
          throw e;
        }
      }),

    deleteNotebook: (id) =>
      guard(async () => {
        await api.deleteNotebook(id);
        const remaining = get().notebooks.filter((n) => n.id !== id);
        set({ notebooks: remaining });
        if (get().currentId === id) {
          if (remaining.length > 0) await get().selectNotebook(remaining[0].id);
          else
            set({
              currentId: null,
              sources: [],
              messages: [],
              messagesHasMore: false,
              notes: [],
            });
        }
      }),

    setNotebookStatus: (id, status) =>
      guard(async () => {
        const prev = get().notebooks;
        set({
          notebooks: prev.map((n) => (n.id === id ? { ...n, status } : n)),
        });
        try {
          await api.setNotebookStatus(id, status);
        } catch (e) {
          set({ notebooks: prev });
          await get().refreshNotebooks();
          throw e;
        }
        // Leave an archived notebook if it was open.
        if (status === "archived" && get().currentId === id) {
          const active = get().notebooks.filter(
            (n) => !n.status && n.id !== id,
          );
          if (active.length > 0) await get().selectNotebook(active[0].id);
          else
            set({
              currentId: null,
              sources: [],
              messages: [],
              messagesHasMore: false,
              notes: [],
            });
        }
      }),

    pickAndAddFiles: async () => {
      const picked = await open({
        multiple: true,
        filters: [{ name: "Documents", extensions: SUPPORTED_EXTENSIONS }],
      });
      if (!picked) return;
      await get().addSourceFiles(Array.isArray(picked) ? picked : [picked]);
    },

    pickAndAddFolder: async (defaultPath?: string) => {
      const id = get().currentId;
      if (!id) return;
      // defaultPath seeds the native picker inside a detected cloud sync root
      // so the user drills down to a subfolder — never the whole drive.
      const picked = await open({ directory: true, defaultPath });
      if (!picked || Array.isArray(picked)) return;
      const name = picked.split("/").pop() || picked;
      const item: QueueItem = {
        id: `${Date.now()}`,
        name,
        status: "pending",
      };
      // Optimistic folder row: the backend embeds every child before it
      // returns, so without this the folder wouldn't appear in the list until
      // the whole import finished. Insert a placeholder now (marked importing
      // so the panel shows a loading affordance) and let the real listSources
      // reconcile it away when addSourceFolder resolves.
      const tempId = `pending-folder-${Date.now()}`;
      const optimistic: Source = {
        id: tempId,
        notebookId: id,
        title: name,
        imageUrl: "",
        sourceType: "folder",
        author: "",
        url: picked,
        content: "",
        status: "ready",
        error: "",
        charCount: 0,
        chunkCount: 0,
        createdAt: Date.now(),
        parentId: "",
        mtime: 0,
        tags: "",
        note: "",
        fetchedAt: Date.now(),
        fetchFailures: 0,
      };
      set({
        ingestQueue: [...get().ingestQueue, item],
        sources: [...get().sources, optimistic],
        importingFolders: [...get().importingFolders, tempId],
        error: null,
      });
      await runQueued(get, set, item, () => api.addSourceFolder(id, picked));
      set({
        folderScan: null,
        importingFolders: get().importingFolders.filter((f) => f !== tempId),
      });
      if (get().currentId === id) {
        // Success replaces the whole list (dropping the temp row); on failure
        // listSources isn't reached, so drop the temp row explicitly.
        set({ sources: await api.listSources(id) });
      } else {
        set({ sources: get().sources.filter((s) => s.id !== tempId) });
      }
    },

    addSourceFiles: async (paths) => {
      const id = get().currentId;
      if (!id || paths.length === 0) return;

      // Enqueue everything, then process serially so embedding stays sequential.
      const items: QueueItem[] = paths.map((p) => ({
        id: `${Date.now()}-${Math.random().toString(36).slice(2)}`,
        name: p.split("/").pop() || p,
        status: "pending",
      }));
      set({ ingestQueue: [...get().ingestQueue, ...items], error: null });

      for (let i = 0; i < paths.length; i++) {
        await runQueued(get, set, items[i], () =>
          api.addSourceFile(id, paths[i]),
        );
        if (get().currentId === id) set({ sources: await api.listSources(id) });
      }
    },

    addSourceUrl: async (url, include?: string) => {
      const id = get().currentId;
      if (!id) return;
      const item: QueueItem = {
        id: `${Date.now()}`,
        name: url,
        status: "pending",
      };
      set({ ingestQueue: [...get().ingestQueue, item], error: null });
      await runQueued(get, set, item, () => api.addSourceUrl(id, url, include));
      if (get().currentId === id) set({ sources: await api.listSources(id) });
    },

    addSourceText: async (title, text) => {
      const id = get().currentId;
      if (!id) return;
      const item: QueueItem = {
        id: `${Date.now()}`,
        name: title.trim() || "Pasted text",
        status: "pending",
      };
      set({ ingestQueue: [...get().ingestQueue, item], error: null });
      await runQueued(get, set, item, () => api.addSourceText(id, title, text));
      if (get().currentId === id) set({ sources: await api.listSources(id) });
    },

    addSourceMac: async (provider, collection, label) => {
      const id = get().currentId;
      if (!id) return;
      const item: QueueItem = {
        id: `${Date.now()}`,
        name: label,
        status: "pending",
      };
      set({ ingestQueue: [...get().ingestQueue, item], error: null });
      await runQueued(get, set, item, () =>
        api.addSourceMac(id, provider, collection, label),
      );
      if (get().currentId === id) set({ sources: await api.listSources(id) });
    },

    editSourceText: async (sourceId, title, text) => {
      const id = get().currentId;
      if (!id) return;
      const item: QueueItem = {
        id: `${Date.now()}`,
        name: title.trim() || "Source",
        status: "pending",
      };
      set({ ingestQueue: [...get().ingestQueue, item], error: null });
      await runQueued(get, set, item, () =>
        api.updateSourceText(sourceId, title, text),
      );
      if (get().currentId === id) set({ sources: await api.listSources(id) });
    },

    // Tag/note edits are quick metadata writes: update in place from the
    // returned row (no ingest-queue theater), fall back to a full re-list
    // only via guard's error surface.
    setSourceTags: (sourceId, tags) =>
      guard(async () => {
        const updated = await api.setSourceTags(sourceId, tags);
        set({
          sources: get().sources.map((s) =>
            s.id === sourceId ? { ...s, tags: updated.tags } : s,
          ),
        });
      }),

    setSourceNote: (sourceId, note) =>
      guard(async () => {
        const updated = await api.setSourceNote(sourceId, note);
        set({
          sources: get().sources.map((s) =>
            s.id === sourceId ? { ...s, note: updated.note } : s,
          ),
        });
      }),

    refreshSource: async (sourceId) => {
      const id = get().currentId;
      if (!id) return;
      const src = get().sources.find((s) => s.id === sourceId);
      const item: QueueItem = {
        id: `${Date.now()}`,
        name: src?.title ?? "Source",
        status: "pending",
      };
      set({ ingestQueue: [...get().ingestQueue, item], error: null });
      await runQueued(get, set, item, () => api.refreshSourceUrl(sourceId));
      if (get().currentId === id) set({ sources: await api.listSources(id) });
    },

    // Write-back to the Mac item behind a source (Apple Notes / Reminders),
    // then the resynced copy replaces ours — the queue shows the re-embed.
    updateMacNote: async (sourceId, body) => {
      const id = get().currentId;
      if (!id) return;
      const src = get().sources.find((s) => s.id === sourceId);
      const item: QueueItem = {
        id: `${Date.now()}`,
        name: src?.title ?? "Note",
        status: "pending",
      };
      set({ ingestQueue: [...get().ingestQueue, item], error: null });
      await runQueued(get, set, item, async () => {
        await api.updateMacNote(sourceId, body);
        get().pushToast("success", "Saved to Apple Notes");
      });
      if (get().currentId === id) set({ sources: await api.listSources(id) });
    },

    addMacReminder: async (sourceId, title, notes) => {
      const id = get().currentId;
      if (!id) return;
      const src = get().sources.find((s) => s.id === sourceId);
      const item: QueueItem = {
        id: `${Date.now()}`,
        name: src?.title ?? "Reminders",
        status: "pending",
      };
      set({ ingestQueue: [...get().ingestQueue, item], error: null });
      await runQueued(get, set, item, async () => {
        await api.addMacReminder(sourceId, title, notes);
        get().pushToast("success", `Added to ${src?.title ?? "Reminders"}`);
      });
      if (get().currentId === id) set({ sources: await api.listSources(id) });
    },

    deleteSource: (id) => get().deleteSourcesBatch([id]),

    // ---- Finder-style selection (RFC-multi-select) ----------------------

    pickOne: (kind, id) => set({ picked: { kind, ids: [id], anchor: id } }),

    pickToggle: (kind, id) => {
      const p = get().picked;
      const ids =
        p?.kind === kind
          ? p.ids.includes(id)
            ? p.ids.filter((x) => x !== id)
            : [...p.ids, id]
          : [id];
      set({ picked: ids.length ? { kind, ids, anchor: id } : null });
    },

    pickRange: (kind, orderedIds, id) => {
      const p = get().picked;
      const anchor = p?.kind === kind ? p.anchor : null;
      const a = anchor ? orderedIds.indexOf(anchor) : -1;
      const b = orderedIds.indexOf(id);
      if (a === -1 || b === -1) {
        set({ picked: { kind, ids: [id], anchor: id } });
        return;
      }
      const ids = orderedIds.slice(Math.min(a, b), Math.max(a, b) + 1);
      // The anchor survives the range — the next shift-click re-ranges from
      // the same fixed point, like Finder.
      set({ picked: { kind, ids, anchor } });
    },

    pickSet: (kind, ids, additive) => {
      const p = get().picked;
      const merged =
        additive && p?.kind === kind
          ? [...new Set([...p.ids, ...ids])]
          : [...ids];
      set({
        picked: merged.length
          ? { kind, ids: merged, anchor: p?.kind === kind ? p.anchor : null }
          : null,
      });
    },

    pickAll: (kind, ids) =>
      set({
        picked: ids.length ? { kind, ids: [...ids], anchor: ids[0] } : null,
      }),

    clearPicked: () => {
      if (get().picked) set({ picked: null });
    },

    // ---- Batch verbs (RFC-multi-select) ---------------------------------

    refreshSourcesBatch: async (sourceIds) => {
      const id = get().currentId;
      if (!id || sourceIds.length === 0) return;
      // Fire-and-return: the backend refreshes sequentially off the IPC
      // thread and emits one sources://changed with the tally at the end —
      // the standing listener re-lists and toasts.
      await api.refreshSources(id, sourceIds);
      set({ picked: null });
      get().pushToast(
        "info",
        sourceIds.length === 1
          ? "Refreshing 1 source…"
          : `Refreshing ${sourceIds.length} sources…`,
      );
    },

    deleteSourcesBatch: (sourceIds) =>
      guard(async () => {
        if (sourceIds.length === 0) return;
        const nb = get().currentId ?? "";
        // Snapshot before the rows go: what to restore, and the content of
        // pasted-text sources (their only copy). Children of a folder being
        // deleted are skipped — restoring the folder re-scans them.
        const doomed = get().sources.filter(
          (s) => sourceIds.includes(s.id) && !sourceIds.includes(s.parentId),
        );
        const texts = new Map<string, string>();
        for (const s of doomed) {
          if (!s.url && sourceRestorable(s))
            texts.set(s.id, await api.getSourceContent(s.id));
        }
        await api.deleteSources(nb, sourceIds);
        if (nb) set({ sources: await api.listSources(nb) });
        set({ picked: null });
        void get().refreshHygiene();
        const restorable = doomed.filter(sourceRestorable);
        const label =
          sourceIds.length === 1
            ? `Removed “${doomed[0]?.title ?? "source"}”`
            : `Removed ${sourceIds.length} sources`;
        if (restorable.length === 0 || !nb) {
          get().pushToast("success", label);
          return;
        }
        // Undo re-imports rather than resurrects, so the restored sources
        // carry fresh ids — redo has to delete those, not the dead ones.
        let restoredIds: string[] = [];
        get().undoableToast(
          label,
          sourceIds.length === 1
            ? "Remove Source"
            : `Remove ${sourceIds.length} Sources`,
          async () => {
            restoredIds = [];
            for (const s of restorable) {
              const back = await restoreSource(nb, s, texts.get(s.id));
              if (back) restoredIds.push(back.id);
            }
            if (get().currentId === nb)
              set({ sources: await api.listSources(nb) });
            void get().refreshHygiene();
          },
          async () => {
            if (restoredIds.length > 0) await api.deleteSources(nb, restoredIds);
            if (get().currentId === nb)
              set({ sources: await api.listSources(nb) });
            void get().refreshHygiene();
          },
        );
      }),

    setSourcesTagsBatch: (sourceIds, tags) =>
      guard(async () => {
        if (sourceIds.length === 0) return;
        // Each source keeps its own prior tag string — one shared "before"
        // would flatten distinct sets into whichever was read last.
        const before = new Map(
          get()
            .sources.filter((s) => sourceIds.includes(s.id))
            .map((s) => [s.id, s.tags] as const),
        );
        await api.setSourcesTags(sourceIds, tags);
        const nb = get().currentId;
        if (nb) set({ sources: await api.listSources(nb) });
        get().pushToast(
          "success",
          sourceIds.length === 1
            ? "Tags saved"
            : `Tagged ${sourceIds.length} sources`,
        );
        const relist = async () => {
          const open = get().currentId;
          if (open) set({ sources: await api.listSources(open) });
        };
        get().pushHistory(
          "Edit Tags",
          async () => {
            for (const [id, prior] of before)
              await api.setSourceTags(id, prior);
            await relist();
          },
          async () => {
            await api.setSourcesTags(sourceIds, tags);
            await relist();
          },
        );
      }),

    deleteNotesBatch: (noteIds) =>
      guard(async () => {
        if (noteIds.length === 0) return;
        // Snapshot for the undo toast: restore_note re-inserts with kind and
        // prompt intact, so studio artifacts keep their viewer.
        const doomed = get().notes.filter((n) => noteIds.includes(n.id));
        await api.deleteNotes(noteIds);
        set({
          notes: get().notes.filter((n) => !noteIds.includes(n.id)),
          picked: null,
        });
        const label =
          noteIds.length === 1
            ? `Deleted “${visibleTitle(doomed[0]?.title ?? "note")}”`
            : `Deleted ${noteIds.length} notes`;
        get().undoableToast(
          label,
          noteIds.length === 1 ? "Delete Note" : `Delete ${noteIds.length} Notes`,
          async () => {
            // restore_note re-inserts under the original id, so redo can
            // reuse the very same list.
            for (const n of doomed) await api.restoreNote(n);
            const nb = get().currentId;
            if (nb) set({ notes: await api.listNotes(nb) });
          },
          async () => {
            await api.deleteNotes(noteIds);
            set({ notes: get().notes.filter((n) => !noteIds.includes(n.id)) });
          },
        );
      }),

    // ---- Source hygiene (RFC-source-hygiene) ----------------------------

    refreshHygiene: async () => {
      const id = get().currentId;
      if (!id) return;
      try {
        const hygiene = await api.sourceHygiene(id);
        if (get().currentId === id) set({ hygiene });
      } catch {
        // Classification is best-effort chrome — never surface a failure.
      }
    },

    hygieneKeep: async (sourceId) => {
      try {
        await api.hygieneKeep(sourceId);
      } catch {
        /* strike reset is best-effort */
      }
      set({
        hygiene: get().hygiene.filter((h) => h.sourceId !== sourceId),
      });
    },

    toggleSourceSelected: (id) => {
      const next = { ...(get().selectedSourceIds ?? {}) };
      if (next[id] === false) delete next[id];
      else next[id] = false;
      // An empty map means nothing is deselected — collapse back to null so
      // future sources stay auto-included.
      const sel = Object.keys(next).length === 0 ? null : next;
      saveSourceSel(get().currentId, sel);
      set({ selectedSourceIds: sel });
    },

    askAboutSource: (sourceId) => {
      const { sources } = get();
      const target = sources.find((s) => s.id === sourceId);
      if (!target) return;
      // Folder-like parents (folders, repos, vaults) carry no chunks
      // themselves — asking about one means asking about its files.
      const keep = new Set([sourceId]);
      if (["folder", "git", "notion", "obsidian"].includes(target.sourceType))
        for (const s of sources) if (s.parentId === sourceId) keep.add(s.id);
      const sel: Record<string, boolean> = {};
      for (const s of sources)
        if (s.sourceType !== "folder" && !keep.has(s.id)) sel[s.id] = false;
      // Nothing deselected (single-source notebook) collapses to null so
      // future sources stay auto-included.
      const next = Object.keys(sel).length === 0 ? null : sel;
      saveSourceSel(get().currentId, next);
      // Empty pendingInput carries no text — it just tells the chat composer
      // to focus once it's in front (the reader occupies its column now).
      set({
        selectedSourceIds: next,
        pendingInput: "",
        galleryOpen: false,
        ledgerOpen: false,
        growOpen: false,
      });
      if (get().reader.open) get().closeReader();
      get().pushToast(
        "success",
        `Chat focused on “${visibleTitle(target.title) || "this source"}”`,
      );
    },

    setAllSourcesSelected: (selected) => {
      let sel: Record<string, boolean> | null = null;
      if (!selected) {
        sel = {};
        // Folder container rows carry no chunks; only content sources matter.
        for (const s of get().sources)
          if (s.sourceType !== "folder") sel[s.id] = false;
      }
      saveSourceSel(get().currentId, sel);
      set({ selectedSourceIds: sel });
    },

    sendMessage: async (content, overrideSourceIds, providerOverride) => {
      const id = get().currentId;
      if (!id || get().sending) return;
      const optimistic: Message = {
        id: `tmp-${Date.now()}`,
        notebookId: id,
        role: "user",
        content,
        citations: [],
        kind: "chat",
        model: "",
        createdAt: Date.now(),
      };
      set({
        messages: [...get().messages, optimistic],
        sending: true,
        sendingFor: id,
        streamingText: "",
        steps: [],
        waiting: "",
        followups: [],
        error: null,
        failedInput: null,
      });
      try {
        const cfg = get().chatConfig;
        // @ mentions replace (not merge with) the checkbox selection: the
        // user named exactly what this question is about.
        const sourceIds = overrideSourceIds ?? selectedIdsForIpc();
        // A provider-override rerun (the error row's "Answer with X") takes
        // the direct chat path even in deep-research mode: the point is one
        // grounded answer from the named local engine, now.
        if (get().agentMode && !providerOverride) {
          await api.sendMessageAgentic(id, content, cfg, sourceIds);
        } else {
          await api.sendMessage(id, content, cfg, sourceIds, providerOverride);
        }
        // Reload in parallel; chat tools can touch sources, notes, report
        // schedules, and templates, so refresh them all with the transcript.
        const [messagePage, sources, notes, reportSchedules, templates] = await Promise.all([
          api.listMessagesPage(id, undefined, CHAT_PAGE_SIZE),
          api.listSources(id),
          api.listNotes(id),
          api.listReportSchedules(id),
          api.listTemplates(),
        ]);
        // The user may have switched notebooks while a slow tool ran — never
        // write another notebook's data over the current one.
        if (get().currentId === id) {
          // Keep any older pages the user deliberately loaded. The latest
          // page replaces the optimistic row and contributes the new turn;
          // merging by id avoids collapsing a long transcript after send.
          const existing = get().messages.filter((m) => !m.id.startsWith("tmp-"));
          const byId = new Map(existing.map((message) => [message.id, message]));
          for (const message of messagePage.messages) byId.set(message.id, message);
          const merged = [...byId.values()].sort(
            (a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id),
          );
          set({
            messages: merged,
            messagesHasMore:
              existing.length > CHAT_PAGE_SIZE
                ? get().messagesHasMore
                : messagePage.hasMore,
            sources,
            notes,
            reportSchedules,
            templates,
            streamingText: "",
          });
          playDone();
          void get().loadFollowups();
        } else {
          // Finished while the user was elsewhere — the answer is persisted;
          // the toast is the way back (selectNotebook re-lists on arrival).
          const title = get().notebooks.find((n) => n.id === id)?.title ?? "notebook";
          playDone();
          get().pushToast(
            "success",
            `Answer ready in “${title}” — click to open`,
            () => void get().selectNotebook(id),
          );
        }
        await get().refreshNotebooks();
      } catch (e) {
        if (get().currentId === id) {
          // Drop the optimistic user turn and hand the text back to the composer
          // so a failed send never silently eats what the user typed.
          set({
            messages: get().messages.filter((m) => m.id !== optimistic.id),
            error: e instanceof Error ? e.message : String(e),
            failedInput: content,
          });
        }
      } finally {
        // sending/steps are global in-flight flags — always clear them, even if
        // the user switched notebooks while the request ran.
        set({
          sending: false,
          sendingFor: null,
          streamingText: "",
          steps: [],
          waiting: "",
        });
        void get().refreshModelStats();
      }
    },

    loadOlderMessages: async () => {
      const { currentId, messages, messagesHasMore, messagesLoadingOlder } = get();
      if (
        !currentId ||
        !messagesHasMore ||
        messagesLoadingOlder ||
        messages.length === 0
      )
        return;
      const before = {
        createdAt: messages[0].createdAt,
        id: messages[0].id,
      };
      set({ messagesLoadingOlder: true });
      try {
        const page = await api.listMessagesPage(
          currentId,
          before,
          CHAT_PAGE_SIZE,
        );
        if (get().currentId !== currentId) return;
        const current = get().messages;
        const known = new Set(current.map((m) => m.id));
        const older = page.messages.filter((m) => !known.has(m.id));
        set({
          messages: [...older, ...current],
          messagesHasMore: page.hasMore,
        });
      } catch (e) {
        get().pushToast("error", e instanceof Error ? e.message : String(e));
      } finally {
        if (get().currentId === currentId) set({ messagesLoadingOlder: false });
      }
    },

    cancelGeneration: (scope) => {
      void api.cancelGeneration(scope);
    },

    // Every "view this source" path in the app funnels through here, so the
    // center-column reader picks them all up (citations, rail, palette).
    openSourceViewer: (sourceId, _title, highlight) =>
      get().openInReader({ type: "source", id: sourceId, highlight }),
    closeSourceViewer: () => get().closeReader(),

    refreshTemplates: async () => {
      set({ templates: await api.listTemplates() });
    },

    openInReader: (doc) => {
      const { history, index } = get().reader;
      const current = history[index];
      // Re-opening the current doc just updates the highlight in place —
      // clicking three citations into one source is one history entry.
      // Opening a document is an explicit trip to the Reader, so it always
      // leaves Ledger mode — the ledger otherwise wins the center column
      // and the reader opens invisibly underneath it.
      if (current && current.type === doc.type && current.id === doc.id) {
        const next = [...history];
        next[index] = doc;
        set({
          ledgerOpen: false,
          galleryOpen: false,
          growOpen: false,
          reader: { open: true, history: next, index },
        });
        return;
      }
      const next = [...history.slice(0, index + 1), doc];
      set({
        ledgerOpen: false,
        galleryOpen: false,
        growOpen: false,
        reader: { open: true, history: next, index: next.length - 1 },
      });
    },

    closeReader: () =>
      set((state) => ({ reader: { ...state.reader, open: false } })),

    readerNavigate: (delta) => {
      const { history, index } = get().reader;
      const next = index + delta;
      if (next < 0 || next >= history.length) return;
      set({ reader: { open: true, history, index: next } });
    },

    readerStep: (dir) => {
      const { reader, sources, notes } = get();
      const current = reader.history[reader.index];
      if (!current) return;
      // Rail order: sources (excluding folder placeholders) then notes.
      const docs: { type: "source" | "note"; id: string }[] = [
        ...sources
          .filter((s) => s.status !== "placeholder")
          .map((s) => ({ type: "source" as const, id: s.id })),
        ...notes.map((n) => ({ type: "note" as const, id: n.id })),
      ];
      const at = docs.findIndex(
        (d) => d.type === current.type && d.id === current.id,
      );
      const target = docs[at + dir];
      if (at === -1 || !target) return;
      get().openInReader(target);
    },

    appendToken: (t) => {
      // Tokens arrive far faster than frames, and committing each one
      // re-rendered the transcript and re-parsed the streamed markdown per
      // token. Buffer and commit once per animation frame; the tail left in
      // the buffer when a stream ends is dropped on purpose — the finished
      // message arrives whole from the backend.
      if (get().waiting) set({ waiting: "" });
      tokenBuffer += t;
      if (tokenFlushHandle !== 0) return;
      tokenFlushHandle = requestAnimationFrame(() => {
        tokenFlushHandle = 0;
        const chunk = tokenBuffer;
        tokenBuffer = "";
        if (!get().sending || !chunk) return;
        set({ streamingText: get().streamingText + chunk });
      });
    },

    appendStep: (label, transient) =>
      // A transient line is a live status, not a trail entry: it replaces the
      // previous one and never accumulates.
      set(
        transient
          ? { waiting: label }
          : { steps: [...get().steps, label], waiting: "" },
      ),

    toggleAgentMode: () => {
      const next = !get().agentMode;
      localStorage.setItem("agentMode", String(next));
      set({ agentMode: next });
    },

    setChatConfig: (config) => {
      const id = get().currentId;
      if (id) localStorage.setItem(`chatConfig:${id}`, JSON.stringify(config));
      set({ chatConfig: config });
    },

    loadFollowups: async () => {
      const id = get().currentId;
      if (!id) return;
      try {
        const followups = await api.suggestFollowups(id);
        if (get().currentId === id) set({ followups });
      } catch {
        /* best-effort */
      }
    },

    refreshSummary: async () => {
      const id = get().currentId;
      if (!id) return;
      set({ summaryLoading: true });
      try {
        const summary = await api.generateNotebookSummary(id);
        localStorage.setItem(`summary:${id}`, summary);
        if (get().currentId === id) set({ summary });
      } catch (e) {
        set({ error: e instanceof Error ? e.message : String(e) });
      } finally {
        set({ summaryLoading: false });
      }
    },

    clearChat: async () => {
      const id = get().currentId;
      if (!id) return;
      await api.clearChat(id);
      set({ messages: [], messagesHasMore: false });
    },

    acpPanes: {},
    hydrateAcpPane: (notebookId) =>
      set((s) => {
        if (s.acpPanes[notebookId]) return {};
        const stored = loadAcpPane(notebookId);
        if (!stored) return {};
        return { acpPanes: { ...s.acpPanes, [notebookId]: stored } };
      }),
    setAcpAgentId: (notebookId, agentId) =>
      set((s) => {
        const pane: AcpPaneState = {
          agents: s.acpPanes[notebookId]?.agents ?? {},
          agentId,
        };
        saveAcpPaneSoon(notebookId, pane);
        return { acpPanes: { ...s.acpPanes, [notebookId]: pane } };
      }),
    setAcpEntries: (notebookId, agentId, update) =>
      set((s) => {
        const mine = acpAgentPane(s.acpPanes, notebookId, agentId);
        const pane: AcpPaneState = {
          agentId: s.acpPanes[notebookId]?.agentId ?? agentId,
          agents: {
            ...s.acpPanes[notebookId]?.agents,
            [agentId]: { ...mine, entries: update(mine.entries) },
          },
        };
        saveAcpPaneSoon(notebookId, pane);
        return { acpPanes: { ...s.acpPanes, [notebookId]: pane } };
      }),
    setAcpDraft: (notebookId, agentId, draft) =>
      set((s) => {
        const mine = acpAgentPane(s.acpPanes, notebookId, agentId);
        if (mine.draft === draft) return {};
        const pane: AcpPaneState = {
          agentId: s.acpPanes[notebookId]?.agentId ?? agentId,
          agents: {
            ...s.acpPanes[notebookId]?.agents,
            [agentId]: { ...mine, draft },
          },
        };
        saveAcpPaneSoon(notebookId, pane);
        return { acpPanes: { ...s.acpPanes, [notebookId]: pane } };
      }),
    setAcpSessionId: (notebookId, agentId, sessionId) =>
      set((s) => {
        const mine = acpAgentPane(s.acpPanes, notebookId, agentId);
        if (mine.sessionId === sessionId) return {};
        const pane: AcpPaneState = {
          agentId: s.acpPanes[notebookId]?.agentId ?? agentId,
          agents: {
            ...s.acpPanes[notebookId]?.agents,
            [agentId]: { ...mine, sessionId },
          },
        };
        saveAcpPaneSoon(notebookId, pane);
        return { acpPanes: { ...s.acpPanes, [notebookId]: pane } };
      }),
    clearAcpPane: (notebookId, agentId) =>
      set((s) => {
        const mine = s.acpPanes[notebookId]?.agents[agentId];
        if (!mine || (mine.entries.length === 0 && !mine.draft)) return {};
        const pane: AcpPaneState = {
          agentId: s.acpPanes[notebookId]?.agentId ?? agentId,
          agents: {
            ...s.acpPanes[notebookId]?.agents,
            [agentId]: { entries: [], draft: "", sessionId: null },
          },
        };
        saveAcpPaneSoon(notebookId, pane);
        return { acpPanes: { ...s.acpPanes, [notebookId]: pane } };
      }),

    generateArtifact: async (kind, prompt) => {
      const id = get().currentId;
      if (!id || get().generatingKind) return;
      set({
        generatingKind: kind,
        generatingFor: id,
        artifactStreamText: "",
        error: null,
      });
      try {
        const note = await api.generateArtifact(
          id,
          kind,
          prompt,
          selectedIdsForIpc(),
        );
        // Auto-open the new note so the outcome is visible where the user acted,
        // not just appended to the Notes list below the fold.
        // Filter by id before prepending: the note:// event listener may have
        // upserted this note already, and an unfiltered prepend rendered the
        // same note twice (deleting "one" then removed both cards — they were
        // one row shown twice).
        // The user may have navigated away while it generated — never write
        // another notebook's note into the open one. The note is persisted;
        // the toast is the way back.
        if (get().currentId === id) {
          set({
            notes: [note, ...get().notes.filter((n) => n.id !== note.id)],
            justCreatedNoteId: note.id,
          });
          get().pushToast("success", `${note.title} ready`);
        } else {
          get().pushToast(
            "success",
            `${note.title} ready — click to open`,
            () =>
              void get()
                .selectNotebook(id)
                .then(() => set({ justCreatedNoteId: note.id })),
          );
        }
        void get().refreshModelStats();
        playDone();
        void notify("Document ready", `“${note.title}” finished generating.`);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        // A user-initiated Stop isn't an error — surface it quietly.
        if (msg.includes("Generation stopped"))
          get().pushToast("info", "Generation stopped");
        else set({ error: msg });
      } finally {
        set({
          generatingKind: null,
          generatingFor: null,
          artifactStreamText: "",
          audioProgress: null,
        });
      }
    },

    generateFromTemplate: async (t) => {
      const id = get().currentId;
      if (!id || get().generatingKind) return;
      set({
        generatingKind: "template",
        generatingFor: id,
        generatingTemplateId: t.id,
        artifactStreamText: "",
        error: null,
      });
      try {
        const note = await api.generateArtifact(id, "template", t.prompt);
        // The backend titles unknown kinds "Report" — rename to the template's name.
        await api.updateNote(note.id, t.name, note.content);
        const titled = { ...note, title: t.name };
        if (get().currentId === id) {
          set({
            notes: [titled, ...get().notes.filter((n) => n.id !== note.id)],
            justCreatedNoteId: note.id,
          });
          get().pushToast("success", `${t.name} ready`);
        } else {
          get().pushToast(
            "success",
            `${t.name} ready — click to open`,
            () =>
              void get()
                .selectNotebook(id)
                .then(() => set({ justCreatedNoteId: note.id })),
          );
        }
        void get().refreshModelStats();
        playDone();
        void notify("Document ready", `“${t.name}” finished generating.`);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        if (msg.includes("Generation stopped"))
          get().pushToast("info", "Generation stopped");
        else set({ error: msg });
      } finally {
        set({
          generatingKind: null,
          generatingFor: null,
          generatingTemplateId: null,
          artifactStreamText: "",
          audioProgress: null,
        });
      }
    },

    rebuildNote: async (note) => {
      const id = get().currentId;
      if (!id || get().generatingKind) return;
      set({ generatingKind: note.kind, artifactStreamText: "", error: null });
      try {
        const updated = await api.rebuildNote(
          note.id,
          id,
          note.kind,
          note.prompt,
        );
        // Template rebuilds keep their template name (the backend re-titles
        // unknown kinds "Report").
        if (note.kind === "template" && updated.title !== note.title) {
          await api.updateNote(updated.id, note.title, updated.content);
          updated.title = note.title;
        }
        set({
          notes: get().notes.map((n) => (n.id === updated.id ? updated : n)),
        });
        playDone();
        void notify("Rebuilt", `“${updated.title}” was rebuilt.`);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        if (msg.includes("Generation stopped"))
          get().pushToast("info", "Rebuild stopped");
        else set({ error: msg });
      } finally {
        set({
          generatingKind: null,
          artifactStreamText: "",
          audioProgress: null,
        });
      }
    },

    createNote: (title, content) =>
      guard(async () => {
        const id = get().currentId;
        if (!id) return;
        const note = await api.createNote(id, title, content);
        set({ notes: [note, ...get().notes.filter((n) => n.id !== note.id)] });
      }),

    updateNote: (noteId, title, content) =>
      guard(async () => {
        const id = get().currentId;
        if (!id) return;
        await api.updateNote(noteId, title, content);
        set({ notes: await api.listNotes(id) });
      }),

    deleteNote: (noteId) => get().deleteNotesBatch([noteId]),

    discussNoteInChat: (noteId) =>
      guard(async () => {
        const msg = await api.addNoteToChat(noteId);
        set({ messages: [...get().messages, msg] });
      }),

    convertNoteToSource: async (noteId) => {
      const id = get().currentId;
      if (!id) return;
      try {
        await api.convertNoteToSource(noteId);
        set({
          notes: get().notes.filter((n) => n.id !== noteId),
          sources: await api.listSources(id),
        });
        await get().refreshNotebooks();
        get().pushToast("success", "Note added as a source");
      } catch (e) {
        set({ error: e instanceof Error ? e.message : String(e) });
      }
    },

    saveAiConfig: async (config) => {
      await api.setAiConfig(config);
      const ollamaOk = await api.checkOllama().catch(() => false);
      set({ aiConfig: config, ollamaOk });
      void get().refreshModelHealth();
    },

    reembedAll: async () => {
      set({
        migration: { done: 0, total: 0, title: "Starting…" },
        error: null,
      });
      const unlisten = await listen<Migration>("migrate://progress", (e) => {
        set({ migration: e.payload });
      });
      // Reported, not thrown — the overlay is the feedback — but the caller
      // still needs to know whether the index came out whole, and reading
      // `error` after the await would also catch an unrelated failure that
      // landed meanwhile.
      let ok = true;
      try {
        await api.reembedAll();
      } catch (e) {
        ok = false;
        set({ error: e instanceof Error ? e.message : String(e) });
      } finally {
        unlisten();
        set({ migration: null });
        const id = get().currentId;
        if (id) set({ sources: await api.listSources(id) });
      }
      return ok;
    },

    // The one export verb: a .okf.zip is the notebook's portable form —
    // share it, back it up, or unzip it into an OKF folder for OK tooling.
    exportNotebookOkf: async (notebookId) => {
      const id = notebookId ?? get().currentId;
      if (!id) {
        get().pushToast("info", "Open a notebook to export it");
        return;
      }
      const nb = get().notebooks.find((n) => n.id === id);
      const slug = (nb?.title ?? "notebook")
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "");
      const dest = await save({
        title: "Export notebook as…",
        defaultPath: `${slug || "notebook"}.okf.zip`,
        filters: [{ name: "OKF bundle", extensions: ["zip"] }],
      });
      if (!dest) return;
      try {
        const path = await api.exportNotebookOkfZip(id, dest);
        get().pushToast("success", `Saved ${path.split("/").pop() ?? "the bundle"}`);
      } catch (e) {
        get().pushToast("error", e instanceof Error ? e.message : String(e));
      }
    },

    importOkf: async (path, notebookId) => {
      const item: QueueItem = {
        id: `${Date.now()}`,
        name: "Importing notebook…",
        status: "pending",
      };
      set({ ingestQueue: [...get().ingestQueue, item], error: null });
      let imported: { id: string; title: string } | null = null;
      await runQueued(get, set, item, async () => {
        const nb = await api.importNotebookOkf(path, notebookId);
        imported = nb;
        get().pushToast("success", `Imported into “${nb.title}”`);
      });
      await get().refreshNotebooks();
      const nb = imported as { id: string; title: string } | null;
      if (nb) await get().selectNotebook(nb.id);
    },

    createReport: (name, kind, prompt, trigger, intervalSecs) =>
      guard(async () => {
        const id = get().currentId;
        if (!id) return;
        await api.createReportSchedule(id, name, kind, prompt, trigger, intervalSecs);
        set({ reportSchedules: await api.listReportSchedules(id) });
        get().pushToast("success", `Scheduled “${name}”`);
      }),

    updateReport: (r) =>
      guard(async () => {
        await api.updateReportSchedule(
          r.id,
          r.name,
          r.kind,
          r.prompt,
          r.trigger,
          r.intervalSecs,
          r.enabled,
        );
        const id = get().currentId;
        if (id) set({ reportSchedules: await api.listReportSchedules(id) });
      }),

    deleteReport: (rid) =>
      guard(async () => {
        // A schedule is user-authored config the backend hard-deletes, so the
        // toast carries the undo: recreate from this snapshot on click.
        const gone = get().reportSchedules.find((r) => r.id === rid);
        await api.deleteReportSchedule(rid);
        set({
          reportSchedules: get().reportSchedules.filter((r) => r.id !== rid),
        });
        if (!gone) return;
        // As with sources, the restored schedule is a new row with a new
        // id; redo deletes that one.
        let restoredId: string | null = null;
        get().undoableToast(
          `Deleted “${gone.name}”`,
          "Delete Schedule",
          async () => {
            const restored = await api.createReportSchedule(
              gone.notebookId,
              gone.name,
              gone.kind,
              gone.prompt,
              gone.trigger,
              gone.intervalSecs,
            );
            restoredId = restored.id;
            if (!gone.enabled)
              await api.updateReportSchedule(
                restored.id,
                gone.name,
                gone.kind,
                gone.prompt,
                gone.trigger,
                gone.intervalSecs,
                false,
              );
            const id = get().currentId;
            if (id) set({ reportSchedules: await api.listReportSchedules(id) });
          },
          async () => {
            if (!restoredId) return;
            await api.deleteReportSchedule(restoredId);
            set({
              reportSchedules: get().reportSchedules.filter(
                (r) => r.id !== restoredId,
              ),
            });
          },
        );
      }),

    runReportNow: async (rid) => {
      const schedule = get().reportSchedules.find((r) => r.id === rid);
      set({ generatingKind: "report" });
      try {
        await api.runReport(rid);
        playDone();
        void notify(
          "Report ready",
          schedule ? `“${schedule.name}” was generated.` : "Report generated.",
        );
        const id = get().currentId;
        if (id) {
          set({
            notes: await api.listNotes(id),
            reportSchedules: await api.listReportSchedules(id),
          });
        }
      } catch (e) {
        set({ error: e instanceof Error ? e.message : String(e) });
      } finally {
        set({ generatingKind: null });
      }
    },

    refreshKokoroStatus: async () => {
      try {
        set({ kokoroStatus: await api.kokoroStatus() });
      } catch {
        /* leave previous status */
      }
    },

    setupKokoro: async () => {
      if (get().kokoroBusy) return;
      set({ kokoroBusy: true });
      try {
        const status = await api.setupKokoro();
        set({ kokoroStatus: status });
        get().pushToast("success", "Audio Overview voices ready");
        playDone();
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        if (msg.includes("Generation stopped"))
          get().pushToast("info", "Download cancelled");
        else set({ error: msg });
        void get().refreshKokoroStatus();
      } finally {
        set({ kokoroBusy: false });
      }
    },

    removeKokoro: () =>
      guard(async () => {
        set({ kokoroStatus: await api.removeKokoro() });
        get().pushToast("success", "Audio Overview voices removed");
      }),

    setError: (e) => set({ error: e }),

    pushToast: (kind, message, onClick) => {
      const id = `toast-${++toastSeq}`;
      set({ toasts: [...get().toasts, { id, kind, message, onClick }] });
      // Clickable toasts linger — the user needs time to notice and act.
      const ttl = kind === "error" ? 7000 : onClick ? 8000 : 3500;
      setTimeout(() => get().dismissToast(id), ttl);
    },

    dismissToast: (id) =>
      set({ toasts: get().toasts.filter((t) => t.id !== id) }),

    // ---- Undo history (RFC-professional-grade Pillar 5) -----------------

    pushHistory: (label, undo, redo) => {
      const entry = makeEntry(label, undo, redo);
      // A fresh mutation invalidates any redo branch — the standard rule.
      set({ undoStack: pushEntry(get().undoStack, entry), redoStack: [] });
      return entry;
    },

    undoableToast: (message, label, undo, redo) => {
      const entry = get().pushHistory(label, undo, redo);
      // One entry, two routes. Clicking the toast drops it from the stack,
      // so a later Cmd-Z can never undo the same mutation a second time.
      get().pushToast("success", `${message} — click to undo`, () => {
        set({ undoStack: dropEntry(get().undoStack, entry.id) });
        void guard(entry.undo);
      });
    },

    undoLast: async () => {
      const stack = get().undoStack;
      const top = stack[stack.length - 1];
      if (!top) return;
      // Pop before running so a repeated Cmd-Z can't fire the same undo
      // twice; a failed undo puts the entry back rather than losing the
      // only way home.
      set({ undoStack: stack.slice(0, -1) });
      try {
        await top.undo();
        set({ redoStack: pushEntry(get().redoStack, top) });
        get().pushToast("info", `Undid ${top.label.toLowerCase()}`);
      } catch (e) {
        set({
          undoStack: pushEntry(get().undoStack, top),
          error: e instanceof Error ? e.message : String(e),
        });
      }
    },

    redoLast: async () => {
      const stack = get().redoStack;
      const top = stack[stack.length - 1];
      if (!top) return;
      set({ redoStack: stack.slice(0, -1) });
      try {
        await top.redo();
        set({ undoStack: pushEntry(get().undoStack, top) });
        get().pushToast("info", `Redid ${top.label.toLowerCase()}`);
      } catch (e) {
        set({
          redoStack: pushEntry(get().redoStack, top),
          error: e instanceof Error ? e.message : String(e),
        });
      }
    },

    markNotesRead: (ids) => {
      if (ids.length === 0) return;
      const noteReads = { ...get().noteReads };
      const now = Date.now();
      for (const id of ids) noteReads[id] = now;
      localStorage.setItem("noteReads", JSON.stringify(noteReads));
      set({ noteReads });
    },
  };
});

/** Replay a history entry: move the pointer, then drive the store to that
 *  place. Entries for since-deleted notebooks are pruned and skipped. */
async function applyNav(delta: 1 | -1): Promise<void> {
  // Note-reader windows render one fixed note — nothing to navigate.
  if (window.__ALCHEMY_NOTE__) return;
  const s = useStore.getState();
  const { stack, index } = s.nav;
  const at = index + delta;
  const target = stack[at];
  if (!target) return;
  if (target.nb && !s.notebooks.some((n) => n.id === target.nb)) {
    const pruned = stack.filter((_, i) => i !== at);
    useStore.setState({
      nav: { stack: pruned, index: delta === -1 ? index - 1 : index },
    });
    return applyNav(delta);
  }
  navApplying = true;
  try {
    useStore.setState({ nav: { stack, index: at } });
    if (target.nb !== s.currentId) {
      if (target.nb) await s.selectNotebook(target.nb);
      else s.closeNotebook();
    }
    const st = useStore.getState();
    if (target.mode === "reader" && target.doc) {
      st.openInReader(target.doc);
    } else {
      useStore.setState({
        galleryOpen: target.mode === "gallery",
        ledgerOpen: target.mode === "ledger",
        growOpen: false,
      });
      if (st.reader.open) st.closeReader();
    }
    // Home's tabs are places too — a notebook has center modes, Home has
    // sections, and back should return you to the one you were reading.
    // A chat entry names its conversation, so back lands in that thread.
    if (target.nb === null) {
      const section = target.section ?? "notebooks";
      if (section === "chat")
        await useStore.getState().openHomeThread(target.thread ?? null);
      else useStore.setState({ homeSection: section });
    }
  } finally {
    navApplying = false;
  }
}

/** A notebook's four sidebars, in rail order — which is the View menu's order
 *  and ⌘1–4's order (menu.rs keeps all three the same). */
export const NOTEBOOK_PANELS = [
  "sources",
  "studio",
  "gallery",
  "ledger",
] as const;

export type NotebookPanel = (typeof NOTEBOOK_PANELS)[number];

/** Show or hide one of them. The single owner of what each toggle means, for
 *  the View menu, ⌘1–4 (App.tsx), and anything else that grows one. Gallery
 *  and Ledger share the center, so opening either closes the other.
 *  A no-op with no notebook open — callers that want to say so do it first. */
export function toggleNotebookPanel(panel: NotebookPanel): void {
  const s = useStore.getState();
  if (!s.currentId) return;
  if (panel === "sources") s.toggleSources();
  else if (panel === "studio") s.toggleStudio();
  else if (panel === "gallery")
    useStore.setState({
      galleryOpen: !s.galleryOpen,
      ledgerOpen: false,
      growOpen: false,
    });
  else
    useStore.setState({
      ledgerOpen: !s.ledgerOpen,
      galleryOpen: false,
      growOpen: false,
    });
}

/** Depth of an in-progress compound navigation (see `navAtomic`). */
let navAtomicDepth = 0;

/** Run a navigation that takes several store writes and record it as ONE
 *  history entry.
 *
 *  A citation jump out of Home's chat selects the notebook and then opens the
 *  reader — two writes, and so two entries, the middle one being that
 *  notebook's chat view, which nobody asked for and nobody was ever looking
 *  at. Back landed there instead of on the conversation. Suppress recording
 *  for the duration and record the destination once. */
export async function navAtomic(go: () => Promise<void> | void): Promise<void> {
  navAtomicDepth++;
  try {
    await go();
  } finally {
    navAtomicDepth--;
  }
  if (navAtomicDepth === 0) recordNav(useStore.getState());
}

// Record every location change into the app-level history. All windows:
// each window owns its own store instance and hence its own back stack.
useStore.subscribe((s, prev) => {
  if (
    s.currentId === prev.currentId &&
    s.ledgerOpen === prev.ledgerOpen &&
    s.galleryOpen === prev.galleryOpen &&
    s.reader === prev.reader &&
    s.homeSection === prev.homeSection &&
    s.homeChat.threadId === prev.homeChat.threadId
  )
    return;
  recordNav(s);
});

function recordNav(s: ReturnType<typeof useStore.getState>) {
  if (navApplying || navAtomicDepth > 0) return;
  const mode = s.galleryOpen
    ? ("gallery" as const)
    : s.ledgerOpen
      ? ("ledger" as const)
      : s.reader.open
        ? ("reader" as const)
        : ("chat" as const);
  const rdoc = s.reader.open ? s.reader.history[s.reader.index] : undefined;
  // Highlight is a one-time citation jump, not a place — drop it.
  const doc = rdoc && { type: rdoc.type, id: rdoc.id };
  // Home's section (and, in the Chat tab, which conversation) is only part
  // of the location while Home IS the location — switching tabs behind an
  // open notebook must not stack entries for a screen nobody is looking at.
  const section = s.currentId ? undefined : s.homeSection;
  const thread = section === "chat" ? (s.homeChat.threadId ?? null) : undefined;
  const { stack, index } = s.nav;
  const cur = stack[index];
  if (
    cur &&
    cur.nb === s.currentId &&
    cur.mode === mode &&
    cur.doc?.type === doc?.type &&
    cur.doc?.id === doc?.id &&
    cur.section === section &&
    cur.thread === thread
  )
    return;
  // A fresh navigation discards forward entries, browser-style.
  const next: NavEntry[] = [
    ...stack.slice(0, index + 1),
    { nb: s.currentId, mode, doc, section, thread },
  ];
  if (next.length > 100) next.splice(0, next.length - 100);
  useStore.setState({ nav: { stack: next, index: next.length - 1 } });
}

// Every failure cues once, wherever it surfaces — the global error banner or
// an error toast. playError throttles, so an error that sets both cues once.
useStore.subscribe((s, prev) => {
  if (s.error && s.error !== prev.error) playError();
  const latest = s.toasts[s.toasts.length - 1];
  if (s.toasts.length > prev.toasts.length && latest?.kind === "error")
    playError();
});

// Remember the precise open view — dashboard vs notebook, and the center
// mode (chat / reader / ledger) with the reader's current doc — so a reload
// or relaunch lands back exactly where the user was. Main window only:
// secondary windows share localStorage and would clobber the main spot.
// Settings is deliberately not a view; dialogs don't survive reloads.
if (getCurrentWebview().label === "main") {
  useStore.subscribe((s, prev) => {
    if (
      s.currentId === prev.currentId &&
      s.ledgerOpen === prev.ledgerOpen &&
      s.galleryOpen === prev.galleryOpen &&
      s.reader === prev.reader &&
      s.homeSection === prev.homeSection &&
      s.homeChat.threadId === prev.homeChat.threadId &&
      s.openCardId === prev.openCardId
    )
      return;
    const doc = s.reader.open ? s.reader.history[s.reader.index] : undefined;
    localStorage.setItem(
      "lastView",
      JSON.stringify({
        nb: s.currentId,
        mode: s.galleryOpen
          ? "gallery"
          : s.ledgerOpen
            ? "ledger"
            : s.reader.open
              ? "reader"
              : "chat",
        // Highlight is a one-time citation jump, not a place — drop it.
        doc: doc && { type: doc.type, id: doc.id },
        // The whole back/forward stack (doc refs only), so ⌘[ still works
        // after a relaunch — not just the page you were on.
        readerHistory: s.reader.history.map((d) => ({
          type: d.type,
          id: d.id,
        })),
        readerIndex: s.reader.index,
        // Home is a place too: the Registry and the card you had open — or
        // the conversation you were in — are as much "where you were" as a
        // notebook's center mode.
        section: s.homeSection,
        card: s.openCardId,
        chatThread: s.homeChat.threadId,
      }),
    );
  });
}

// The store rides on `window` in every build — debugging in dev, and a
// window into live UI state for users' AI agents in prod (the debug
// bridge's invoke path bypasses the frontend, so this is the only one).
(window as unknown as Record<string, unknown>).__store = useStore;
