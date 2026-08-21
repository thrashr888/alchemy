import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { SUPPORTED_EXTENSIONS, visibleTitle } from "./utils";
import { applyTheme, SYSTEM_THEME, themeIsDark } from "./themes";
import { refreshEpigraph } from "./epigraph";
import { notify } from "./notify";
import { playArrival, playDone, playError } from "./sound";
import { autoUpdateEnabled, checkForUpdatesQuietly } from "./updates";
import { DEFAULT_CHAT_CONFIG, DEFAULT_READING_PREFS } from "./types";
import type {
  AppState,
  Migration,
  NavEntry,
  QueueItem,
  ReaderDoc,
} from "./storeTypes";
export type { ExternalAdd, Migration, QueueItem } from "./storeTypes";
import type {
  ChatConfig,
  Message,
  Note,
  ReadingPrefs,
  Source,
} from "./types";

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

/** Persist a notebook's source selection; null (all selected) clears the key. */
function saveSourceSel(
  notebookId: string | null,
  sel: Record<string, boolean> | null,
) {
  if (!notebookId) return;
  if (sel === null) localStorage.removeItem(`sourceSel:${notebookId}`);
  else localStorage.setItem(`sourceSel:${notebookId}`, JSON.stringify(sel));
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
 *  global listeners, the update check) ran twice — hence two "update available"
 *  toasts. Module scope, so it survives the StrictMode remount. */
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
    streamingText: "",
    steps: [],
    waiting: "",
    agentMode: localStorage.getItem("agentMode") === "true",
    chatConfig: DEFAULT_CHAT_CONFIG,
    followups: [],
    summary: "",
    summaryLoading: false,
    generatingKind: null,
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
    importOkfOpen: false,
    pendingImportPath: null,
    error: null,
    toasts: [],
    justCreatedNoteId: null,
    // Center-column Ledger mode (Chat | Reader | Ledger) + a bump counter
    // the pane watches so agent writes appear live (mcp://changed).
    ledgerOpen: false,
    galleryOpen: false,
    readerEditIntent: null,
    ledgerBump: 0,
    registryBump: 0,
    homeSection: "notebooks",
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
    nav: { stack: [{ nb: null, mode: "chat" }], index: 0 },
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
      const [notebooks, aiConfig, ollamaOk, templates] = await Promise.all([
        api.listNotebooks(),
        api.getAiConfig(),
        api.checkOllama().catch(() => false),
        // Templates are global (a user folder), not per-notebook. A read failure
        // just hides the section — never blocks boot.
        api.listTemplates().catch(() => []),
      ]);
      set({ notebooks, aiConfig, ollamaOk, templates });
      // Releases any OS entry point that arrived before the corpus was known.
      markNotebooksLoaded();
      // showNotifications lives in config now (the Night Shift's resident
      // scheduler reads it backend-side, as does notify()'s send_notification
      // path). Honor a pre-migration localStorage opt-out once, then mirror
      // config down so the legacy key can't re-trigger this migration.
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
      void get().refreshModelHealth();
      void get().refreshModelStats();
      void get().refreshKokoroStatus();
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
          section?: "notebooks" | "registry";
          card?: string | null;
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
        const last =
          view === null ? localStorage.getItem("lastNotebookId") : view.nb;
        if (last && notebooks.some((n) => n.id === last)) {
          await get().selectNotebook(last);
          if (view?.mode === "ledger") set({ ledgerOpen: true });
          else if (view?.mode === "gallery") set({ galleryOpen: true });
          else if (view?.mode === "reader" && view.doc)
            get().openInReader(view.doc);
        }
      }
      void api.rebuildAppMenu();
      // Quiet update check, once per launch, main window only.
      if (getCurrentWebview().label === "main" && autoUpdateEnabled()) {
        setTimeout(() => {
          // Clicking the notice lands on Settings → General with the check
          // already run, so the Install button is right there. The version
          // is also remembered so General/About show it on their own.
          void checkForUpdatesQuietly((v) => {
            set({ updateAvailable: v });
            get().pushToast(
              "info",
              `Alchemy ${v} is available — click to review and install.`,
              () => {
                set({ pendingUpdateCheck: true });
                get().openSettings("general");
              },
            );
          });
        }, 4000);
      }
    },

    bindGlobalListeners: () => {
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
      // The settings tool's per-notebook style verb (RFC-conversational-setup
      // §2): the validated change arrives as an event because ChatConfig is
      // frontend state. Merge into the notebook's stored config; update the
      // live store only when that notebook is in front here.
      void listen<{
        notebookId: string;
        style?: string | null;
        length?: string | null;
      }>("settings://style", (e) => {
        const { notebookId, style, length } = e.payload;
        const key = `chatConfig:${notebookId}`;
        let cur: ChatConfig = { ...DEFAULT_CHAT_CONFIG };
        try {
          const raw = localStorage.getItem(key);
          if (raw) cur = { ...DEFAULT_CHAT_CONFIG, ...JSON.parse(raw) };
        } catch {
          // Unreadable stored config — rebuild from the defaults.
        }
        // The backend validated these against the same rosters this union
        // mirrors (selfheal::resolve_style / settings_style).
        if (style != null) cur.style = style as ChatConfig["style"];
        if (length != null) cur.length = length as ChatConfig["length"];
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
              title: "Downloading the podcast voice model",
            },
          });
        },
      );
      // App-menu actions broadcast to every window with the intended target's
      // label in the payload — each window acts only on events addressed to it.
      // (JS "Any" listeners receive every event regardless of emit target, so
      // this self-filter is what actually prevents N windows from all reacting.)
      const label = getCurrentWebview().label;
      void listen<{ target: string; id: string }>("menu://action", (e) => {
        if (e.payload.target !== label) return;
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
      });
      void listen<{ target: string; id: string }>(
        "menu://open-notebook",
        (e) => {
          if (e.payload.target !== label) return;
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
      set({ notebooks: await api.listNotebooks() });
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
        messages: [],
        messagesHasMore: false,
        messagesLoadingOlder: false,
        notes: [],
        reportSchedules: [],
        streamingText: "",
        steps: [],
        waiting: "",
        followups: [],
        chatConfig,
        summary: localStorage.getItem(`summary:${id}`) ?? "",
        reader: { open: false, history: [], index: -1 },
      });
      const nb = get().notebooks.find((n) => n.id === id);
      if (nb) void getCurrentWebviewWindow().setTitle(`${nb.title} — Alchemy`);
      const [sources, messagePage, notes, reportSchedules] = await Promise.all([
        api.listSources(id),
        api.listMessagesPage(id, undefined, CHAT_PAGE_SIZE),
        api.listNotes(id),
        api.listReportSchedules(id),
      ]);
      if (get().currentId === id)
        set({
          sources,
          messages: messagePage.messages,
          messagesHasMore: messagePage.hasMore,
          notes,
          reportSchedules,
        });
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
        await api.renameNotebook(id, title);
        await get().refreshNotebooks();
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

    deleteSource: (id) =>
      guard(async () => {
        await api.deleteSource(id);
        const nb = get().currentId;
        if (nb) set({ sources: await api.listSources(nb) });
        get().pushToast("success", "Source removed");
      }),

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
      });
      if (get().reader.open) get().closeReader();
      get().pushToast(
        "success",
        `Chat scoped to "${visibleTitle(target.title) || "this source"}"`,
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
        set({ sending: false, streamingText: "", steps: [], waiting: "" });
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
          reader: { open: true, history: next, index },
        });
        return;
      }
      const next = [...history.slice(0, index + 1), doc];
      set({
        ledgerOpen: false,
        galleryOpen: false,
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

    generateArtifact: async (kind, prompt) => {
      const id = get().currentId;
      if (!id || get().generatingKind) return;
      set({ generatingKind: kind, artifactStreamText: "", error: null });
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
        set({
          notes: [note, ...get().notes.filter((n) => n.id !== note.id)],
          justCreatedNoteId: note.id,
        });
        void get().refreshModelStats();
        get().pushToast("success", `${note.title} ready`);
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
        generatingTemplateId: t.id,
        artifactStreamText: "",
        error: null,
      });
      try {
        const note = await api.generateArtifact(id, "template", t.prompt);
        // The backend titles unknown kinds "Report" — rename to the template's name.
        await api.updateNote(note.id, t.name, note.content);
        const titled = { ...note, title: t.name };
        set({
          notes: [titled, ...get().notes.filter((n) => n.id !== note.id)],
          justCreatedNoteId: note.id,
        });
        void get().refreshModelStats();
        get().pushToast("success", `${t.name} ready`);
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
        void notify("Rebuilt", `“${updated.title}” was regenerated.`);
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

    deleteNote: (noteId) =>
      guard(async () => {
        await api.deleteNote(noteId);
        set({ notes: get().notes.filter((n) => n.id !== noteId) });
        get().pushToast("success", "Note deleted");
      }),

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
      try {
        await api.reembedAll();
      } catch (e) {
        set({ error: e instanceof Error ? e.message : String(e) });
      } finally {
        unlisten();
        set({ migration: null });
        const id = get().currentId;
        if (id) set({ sources: await api.listSources(id) });
      }
    },

    // The one export verb: a .okf.zip is the notebook's portable form —
    // share it, back it up, or unzip it into an OKF folder for OK tooling.
    exportNotebookOkf: async () => {
      const id = get().currentId;
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
        get().pushToast("success", `Saved ${path}`);
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
        await api.deleteReportSchedule(rid);
        set({
          reportSchedules: get().reportSchedules.filter((r) => r.id !== rid),
        });
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
        get().pushToast("success", "Podcast voices ready");
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
        get().pushToast("success", "Podcast voices removed");
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
      });
      if (st.reader.open) st.closeReader();
    }
  } finally {
    navApplying = false;
  }
}

// Record every location change into the app-level history. All windows:
// each window owns its own store instance and hence its own back stack.
useStore.subscribe((s, prev) => {
  if (
    s.currentId === prev.currentId &&
    s.ledgerOpen === prev.ledgerOpen &&
    s.galleryOpen === prev.galleryOpen &&
    s.reader === prev.reader
  )
    return;
  if (navApplying) return;
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
  const { stack, index } = s.nav;
  const cur = stack[index];
  if (
    cur &&
    cur.nb === s.currentId &&
    cur.mode === mode &&
    cur.doc?.type === doc?.type &&
    cur.doc?.id === doc?.id
  )
    return;
  // A fresh navigation discards forward entries, browser-style.
  const next: NavEntry[] = [
    ...stack.slice(0, index + 1),
    { nb: s.currentId, mode, doc },
  ];
  if (next.length > 100) next.splice(0, next.length - 100);
  useStore.setState({ nav: { stack: next, index: next.length - 1 } });
});

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
        // Home is a place too: the Registry and the card you had open are
        // as much "where you were" as a notebook's center mode.
        section: s.homeSection,
        card: s.openCardId,
      }),
    );
  });
}

// The store rides on `window` in every build — debugging in dev, and a
// window into live UI state for users' AI agents in prod (the debug
// bridge's invoke path bypasses the frontend, so this is the only one).
(window as unknown as Record<string, unknown>).__store = useStore;
