import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { SUPPORTED_EXTENSIONS } from "./utils";
import { applyTheme, SYSTEM_THEME, themeIsDark } from "./themes";
import { refreshEpigraph } from "./epigraph";
import { notify } from "./notify";
import { playArrival, playDone, playError } from "./sound";
import { autoUpdateEnabled, checkForUpdatesQuietly } from "./updates";
import { DEFAULT_CHAT_CONFIG, DEFAULT_READING_PREFS } from "./types";
import type { AppState, Migration, QueueItem } from "./storeTypes";
export type { ExternalAdd, Migration, QueueItem } from "./storeTypes";
import type {
  ChatConfig,
  Message,
  Note,
  ReadingPrefs,
  ReportSchedule,
} from "./types";

// Side panels stay usable at any drag position: wide enough for content,
// narrow enough to leave the chat column room at the 1040px minimum window.
const PANEL_BOUNDS = { sources: [220, 400], studio: [260, 460] } as const;

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

// Module-level guard so the report scheduler is only started once.
let schedulerStarted = false;
// Same guard for the source-resync loop.
let sourceSyncStarted = false;
// Global Tauri event listeners bind once per page — React StrictMode runs
// init() twice in dev, and a doubled menu listener spawns doubled windows.
let listenersBound = false;
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
  patch({ status: "processing" });
  try {
    await fn();
    patch({ status: "done" });
    setTimeout(() => get().clearQueueItem(item.id), 2000);
  } catch (e) {
    patch({
      status: "error",
      error: e instanceof Error ? e.message : String(e),
    });
    playError();
  }
}

/** One-shot guard for `init`. React StrictMode double-invokes the mount
 *  effect in dev, so without this the whole boot (notebook select, schedulers,
 *  global listeners, the update check) ran twice — hence two "update available"
 *  toasts. Module scope, so it survives the StrictMode remount. */
let initStarted = false;

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
    embedderDownload: null,
    failedInput: null,
    pendingInput: null,
    pendingAsk: null,
    importOkfOpen: false,
    pendingImportPath: null,
    error: null,
    toasts: [],
    justCreatedNoteId: null,
    pendingNewNote: false,
    artifactStreamText: "",
    audioProgress: null,
    kokoroStatus: null,
    kokoroBusy: false,
    reader: { open: false, history: [], index: -1 },
    folderScan: null,
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
        const last = localStorage.getItem("lastNotebookId");
        if (last && notebooks.some((n) => n.id === last)) {
          await get().selectNotebook(last);
        }
      }
      get().startReportScheduler();
      get().startSourceSync();
      void api.rebuildAppMenu();
      // Quiet update check, once per launch, main window only.
      if (getCurrentWebview().label === "main" && autoUpdateEnabled()) {
        setTimeout(() => {
          void checkForUpdatesQuietly((m) => get().pushToast("info", m));
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
      // Studio generations stream their tokens; buffer them for the live preview.
      void listen<{ content: string }>("artifact://token", (e) => {
        if (get().generatingKind)
          set({
            artifactStreamText: get().artifactStreamText + e.payload.content,
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
          set({ folderScan: p.done >= p.total ? null : p });
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
          playArrival();
          void get().refreshNotebooks();
          const current = get().currentId;
          if (!current || (notebookId && notebookId !== current)) return;
          if (scope === "sources")
            void api.listSources(current).then((sources) => set({ sources }));
          if (scope === "notes")
            void api.listNotes(current).then((notes) => set({ notes }));
        },
      );
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
        else if (e.payload.id === "menu-share-okf") void s.shareNotebookOkf();
        else if (e.payload.id === "menu-import-okf")
          set({ importOkfOpen: true });
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
          const recent = s.notebooks[0];
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
        notes: [],
        reportSchedules: [],
        streamingText: "",
        steps: [],
        followups: [],
        chatConfig,
        summary: localStorage.getItem(`summary:${id}`) ?? "",
        reader: { open: false, history: [], index: -1 },
      });
      const nb = get().notebooks.find((n) => n.id === id);
      if (nb) void getCurrentWebviewWindow().setTitle(`${nb.title} — Alchemy`);
      const [sources, messages, notes, reportSchedules] = await Promise.all([
        api.listSources(id),
        api.listMessages(id),
        api.listNotes(id),
        api.listReportSchedules(id),
      ]);
      if (get().currentId === id)
        set({ sources, messages, notes, reportSchedules });
      // Catch up folder and file sources right away rather than waiting for the
      // next minute tick. Changes come back via sources://changed.
      void api.resyncSources().catch(() => {});
    },

    closeNotebook: () => {
      void getCurrentWebviewWindow().setTitle("Alchemy");
      set({
        currentId: null,
        sources: [],
        selectedSourceIds: null,
        messages: [],
        notes: [],
        reportSchedules: [],
        ingestQueue: [],
        steps: [],
        reader: { open: false, history: [], index: -1 },
      });
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

    deleteNotebook: (id) =>
      guard(async () => {
        await api.deleteNotebook(id);
        const remaining = get().notebooks.filter((n) => n.id !== id);
        set({ notebooks: remaining });
        if (get().currentId === id) {
          if (remaining.length > 0) await get().selectNotebook(remaining[0].id);
          else set({ currentId: null, sources: [], messages: [], notes: [] });
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

    pickAndAddFolder: async () => {
      const id = get().currentId;
      if (!id) return;
      const picked = await open({ directory: true });
      if (!picked || Array.isArray(picked)) return;
      const item: QueueItem = {
        id: `${Date.now()}`,
        name: picked.split("/").pop() || picked,
        status: "pending",
      };
      set({ ingestQueue: [...get().ingestQueue, item], error: null });
      await runQueued(get, set, item, () => api.addSourceFolder(id, picked));
      set({ folderScan: null });
      if (get().currentId === id) set({ sources: await api.listSources(id) });
    },

    startSourceSync: () => {
      if (sourceSyncStarted) return;
      // Main window only — the backend serializes scans, but one tick loop per
      // app is still one too few reasons to run N of them.
      if (getCurrentWebview().label !== "main") return;
      sourceSyncStarted = true;
      const tick = async () => {
        try {
          await api.resyncSources();
          // Changed notebooks are announced via sources://changed; every window
          // (including this one) refreshes from its own listener.
        } catch {
          /* disk or embedder hiccup — next tick retries */
        }
      };
      void tick();
      setInterval(() => void tick(), 60_000);
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

    sendMessage: async (content) => {
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
        followups: [],
        error: null,
        failedInput: null,
      });
      try {
        const cfg = get().chatConfig;
        const sourceIds = selectedIdsForIpc();
        if (get().agentMode) {
          await api.sendMessageAgentic(id, content, cfg, sourceIds);
        } else {
          await api.sendMessage(id, content, cfg, sourceIds);
        }
        // Reload in parallel; chat tools can touch sources, notes, and report
        // schedules, so refresh them all alongside the transcript.
        const [messages, sources, notes, reportSchedules] = await Promise.all([
          api.listMessages(id),
          api.listSources(id),
          api.listNotes(id),
          api.listReportSchedules(id),
        ]);
        // The user may have switched notebooks while a slow tool ran — never
        // write another notebook's data over the current one.
        if (get().currentId === id) {
          set({ messages, sources, notes, reportSchedules, streamingText: "" });
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
        set({ sending: false, streamingText: "", steps: [] });
        void get().refreshModelStats();
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

    openInReader: (doc) => {
      const { history, index } = get().reader;
      const current = history[index];
      // Re-opening the current doc just updates the highlight in place —
      // clicking three citations into one source is one history entry.
      if (current && current.type === doc.type && current.id === doc.id) {
        const next = [...history];
        next[index] = doc;
        set({ reader: { open: true, history: next, index } });
        return;
      }
      const next = [...history.slice(0, index + 1), doc];
      set({ reader: { open: true, history: next, index: next.length - 1 } });
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

    appendToken: (t) => set({ streamingText: get().streamingText + t }),

    appendStep: (label) => set({ steps: [...get().steps, label] }),

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
      set({ messages: [] });
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
        set({ notes: [note, ...get().notes], justCreatedNoteId: note.id });
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
        set({ notes: [note, ...get().notes] });
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

    exportNotebookOkf: async () => {
      const id = get().currentId;
      if (!id) {
        get().pushToast("info", "Open a notebook to export it");
        return;
      }
      const dest = await open({
        directory: true,
        title: "Export OKF bundle into…",
      });
      if (!dest) return;
      try {
        const path = await api.exportNotebookOkf(id, dest as string);
        get().pushToast("success", `Exported to ${path}`);
      } catch (e) {
        get().pushToast("error", e instanceof Error ? e.message : String(e));
      }
    },

    shareNotebookOkf: async () => {
      const id = get().currentId;
      if (!id) {
        get().pushToast("info", "Open a notebook to share it");
        return;
      }
      const nb = get().notebooks.find((n) => n.id === id);
      const slug = (nb?.title ?? "notebook")
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "");
      const dest = await save({
        title: "Share notebook as…",
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

    createReport: (name, kind, prompt, intervalSecs) =>
      guard(async () => {
        const id = get().currentId;
        if (!id) return;
        await api.createReportSchedule(id, name, kind, prompt, intervalSecs);
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

    startReportScheduler: () => {
      if (schedulerStarted) return;
      // Only the main window runs the scheduler — one tick loop per app, not
      // one per window, or reports would generate once per open window.
      if (getCurrentWebview().label !== "main") return;
      schedulerStarted = true;
      const tick = async () => {
        let due: ReportSchedule[];
        try {
          const all = await api.listAllReportSchedules();
          const now = Date.now();
          due = all.filter(
            (s) => s.enabled && now - s.lastRunAt >= s.intervalSecs * 1000,
          );
        } catch {
          return;
        }
        for (const s of due) {
          try {
            await api.runReport(s.id);
            void notify("Report ready", `“${s.name}” was generated.`);
            playArrival();
            const cur = get().currentId;
            if (cur === s.notebookId) {
              set({
                notes: await api.listNotes(cur),
                reportSchedules: await api.listReportSchedules(cur),
              });
            }
          } catch {
            /* try again next tick */
          }
        }
      };
      void tick();
      setInterval(() => void tick(), 60_000);
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

    pushToast: (kind, message) => {
      const id = `toast-${++toastSeq}`;
      set({ toasts: [...get().toasts, { id, kind, message }] });
      const ttl = kind === "error" ? 7000 : 3500;
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

// Every failure cues once, wherever it surfaces — the global error banner or
// an error toast. playError throttles, so an error that sets both cues once.
useStore.subscribe((s, prev) => {
  if (s.error && s.error !== prev.error) playError();
  const latest = s.toasts[s.toasts.length - 1];
  if (s.toasts.length > prev.toasts.length && latest?.kind === "error")
    playError();
});

// The store rides on `window` in every build — debugging in dev, and a
// window into live UI state for users' AI agents in prod (the debug
// bridge's invoke path bypasses the frontend, so this is the only one).
(window as unknown as Record<string, unknown>).__store = useStore;
