import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { SUPPORTED_EXTENSIONS } from "./utils";
import { applyTheme, SYSTEM_THEME } from "./themes";
import { notify } from "./notify";
import { playDone } from "./sound";
import { autoUpdateEnabled, checkForUpdatesQuietly } from "./updates";
import { DEFAULT_CHAT_CONFIG, DEFAULT_READING_PREFS } from "./types";
import type {
  AiConfig,
  ChatConfig,
  Message,
  ModelHealth,
  ModelStat,
  Note,
  NoteKind,
  Notebook,
  ReadingPrefs,
  ReportSchedule,
  Source,
  Toast,
  ToastKind,
} from "./types";

export interface QueueItem {
  id: string;
  name: string;
  status: "pending" | "processing" | "done" | "error";
  error?: string;
}

export interface Migration {
  done: number;
  total: number;
  title: string;
}

interface AppState {
  notebooks: Notebook[];
  currentId: string | null;
  sources: Source[];
  messages: Message[];
  notes: Note[];
  reportSchedules: ReportSchedule[];
  aiConfig: AiConfig | null;
  ollamaOk: boolean | null;
  modelHealth: ModelHealth | null;
  modelStats: ModelStat[];
  theme: string;
  reading: ReadingPrefs;

  sending: boolean;
  streamingText: string;
  steps: string[];
  agentMode: boolean;
  chatConfig: ChatConfig;
  followups: string[];
  summary: string;
  summaryLoading: boolean;
  generatingKind: NoteKind | null;
  ingestQueue: QueueItem[];
  migration: Migration | null;
  draggingFiles: boolean;
  sourcesOpen: boolean;
  studioOpen: boolean;
  /** Draggable side-panel widths (px), persisted. */
  sourcesWidth: number;
  studioWidth: number;
  onboardingDismissed: boolean;
  settingsOpen: boolean;
  settingsTab: string;
  /** Cmd+K command menu. */
  paletteOpen: boolean;
  /** Command-menu ask for the URL form — a flag (not an event) because the
   *  Sources panel may still be mounting when the command runs. */
  pendingAddUrl: boolean;
  /** Menu asked for an update check — Settings' General tab runs it on mount. */
  pendingUpdateCheck: boolean;
  embedderDownload: { label: string; done: number; total: number } | null;
  error: string | null;
  /** Text of a chat send that failed, handed back to the composer so it isn't lost. */
  failedInput: string | null;
  /** Text another surface (e.g. the source reader) wants in the composer. */
  pendingInput: string | null;
  /** Ephemeral toasts (success/info auto-dismiss; errors linger a bit longer). */
  toasts: Toast[];
  /** Id of a just-generated note, so the Studio panel can auto-open it. */
  justCreatedNoteId: string | null;
  /** Cmd+N pressed while Studio was collapsed — open the composer on mount. */
  pendingNewNote: boolean;
  /** Streaming buffer for the in-flight Studio generation (artifact://token). */
  artifactStreamText: string;
  /** Audio Overview synthesis progress (audio://progress), null when idle. */
  audioProgress: { done: number; total: number } | null;
  /** Source open in the reader, optionally scrolled to a cited passage. */
  viewingSource: { sourceId: string; title: string; highlight?: string } | null;

  init: () => Promise<void>;
  /** Register page-lifetime Tauri event listeners (called once from init). */
  bindGlobalListeners: () => void;
  refreshNotebooks: () => Promise<void>;
  selectNotebook: (id: string) => Promise<void>;
  closeNotebook: () => void;
  createNotebook: (title: string) => Promise<void>;
  renameNotebook: (id: string, title: string) => Promise<void>;
  deleteNotebook: (id: string) => Promise<void>;
  setTheme: (theme: string) => void;
  setReading: (patch: Partial<ReadingPrefs>) => void;
  clearQueueItem: (id: string) => void;
  setDraggingFiles: (v: boolean) => void;
  toggleSources: () => void;
  toggleStudio: () => void;
  setPanelWidth: (panel: "sources" | "studio", width: number) => void;
  dismissOnboarding: () => void;
  openSettings: (tab?: string) => void;
  closeSettings: () => void;
  setPaletteOpen: (open: boolean) => void;
  /** Open/close the command menu, refusing to stack over an open dialog. */
  togglePalette: () => void;
  /** Pick a directory and export the current notebook as an OKF bundle. */
  exportNotebookOkf: () => Promise<void>;
  createReport: (name: string, kind: string, prompt: string, intervalSecs: number) => Promise<void>;
  updateReport: (r: ReportSchedule) => Promise<void>;
  deleteReport: (id: string) => Promise<void>;
  runReportNow: (id: string) => Promise<void>;
  startReportScheduler: () => void;

  pickAndAddFiles: () => Promise<void>;
  addSourceFiles: (paths: string[]) => Promise<void>;
  addSourceUrl: (url: string) => Promise<void>;
  addSourceText: (title: string, text: string) => Promise<void>;
  editSourceText: (sourceId: string, title: string, text: string) => Promise<void>;
  refreshSource: (sourceId: string) => Promise<void>;
  deleteSource: (id: string) => Promise<void>;

  sendMessage: (content: string) => Promise<void>;
  cancelGeneration: (scope?: "chat" | "artifact") => void;
  openSourceViewer: (sourceId: string, title: string, highlight?: string) => void;
  closeSourceViewer: () => void;
  appendToken: (t: string) => void;
  appendStep: (label: string) => void;
  toggleAgentMode: () => void;
  setChatConfig: (config: ChatConfig) => void;
  loadFollowups: () => Promise<void>;
  refreshSummary: () => Promise<void>;
  clearChat: () => Promise<void>;

  generateArtifact: (kind: NoteKind, prompt?: string) => Promise<void>;
  rebuildNote: (note: Note) => Promise<void>;
  createNote: (title: string, content: string) => Promise<void>;
  updateNote: (id: string, title: string, content: string) => Promise<void>;
  deleteNote: (id: string) => Promise<void>;
  discussNoteInChat: (id: string) => Promise<void>;
  convertNoteToSource: (id: string) => Promise<void>;

  saveAiConfig: (config: AiConfig) => Promise<void>;
  refreshModelHealth: () => Promise<void>;
  refreshModelStats: () => Promise<void>;
  reembedAll: () => Promise<void>;
  setError: (e: string | null) => void;
  pushToast: (kind: ToastKind, message: string) => void;
  dismissToast: (id: string) => void;
}

// Side panels stay usable at any drag position: wide enough for content,
// narrow enough to leave the chat column room at the 1040px minimum window.
const PANEL_BOUNDS = { sources: [220, 400], studio: [260, 460] } as const;

function clampPanel(panel: "sources" | "studio", width: number): number {
  const [min, max] = PANEL_BOUNDS[panel];
  return Math.round(Math.min(max, Math.max(min, width)));
}

function loadReadingPrefs(): ReadingPrefs {
  try {
    const raw = localStorage.getItem("readingPrefs");
    return raw ? { ...DEFAULT_READING_PREFS, ...JSON.parse(raw) } : DEFAULT_READING_PREFS;
  } catch {
    return DEFAULT_READING_PREFS;
  }
}

// Module-level guard so the report scheduler is only started once.
let schedulerStarted = false;
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
    set({ ingestQueue: get().ingestQueue.map((q) => (q.id === item.id ? { ...q, ...p } : q)) });
  patch({ status: "processing" });
  try {
    await fn();
    patch({ status: "done" });
    setTimeout(() => get().clearQueueItem(item.id), 2000);
  } catch (e) {
    patch({ status: "error", error: e instanceof Error ? e.message : String(e) });
  }
}

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

  return {
  notebooks: [],
  currentId: null,
  sources: [],
  messages: [],
  notes: [],
  reportSchedules: [],
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
  ingestQueue: [],
  migration: null,
  draggingFiles: false,
  sourcesOpen: localStorage.getItem("sourcesOpen") !== "false",
  studioOpen: localStorage.getItem("studioOpen") !== "false",
  sourcesWidth: clampPanel("sources", Number(localStorage.getItem("sourcesWidth")) || 280),
  studioWidth: clampPanel("studio", Number(localStorage.getItem("studioWidth")) || 320),
  onboardingDismissed: localStorage.getItem("onboardingDismissed") === "true",
  settingsOpen: false,
  settingsTab: "general",
  paletteOpen: false,
  pendingAddUrl: false,
  pendingUpdateCheck: false,
  embedderDownload: null,
  failedInput: null,
  pendingInput: null,
  error: null,
  toasts: [],
  justCreatedNoteId: null,
  pendingNewNote: false,
  artifactStreamText: "",
  audioProgress: null,
  viewingSource: null,

  init: async () => {
    applyTheme(get().theme);
    // Every page load (incl. dev reloads) resets the macOS stoplights to
    // their default position — put them back first thing.
    void api.fixTrafficLights();
    if (!listenersBound) {
      listenersBound = true;
      get().bindGlobalListeners();
    }
    const [notebooks, aiConfig, ollamaOk] = await Promise.all([
      api.listNotebooks(),
      api.getAiConfig(),
      api.checkOllama().catch(() => false),
    ]);
    set({ notebooks, aiConfig, ollamaOk });
    void get().refreshModelHealth();
    void get().refreshModelStats();
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
        const finished = p.total > 0 && p.done >= p.total && p.label === "model.safetensors";
        set({ embedderDownload: finished ? null : p });
        if (finished) setTimeout(() => set({ embedderDownload: null }), 1500);
      },
    );
    // Studio generations stream their tokens; buffer them for the live preview.
    void listen<{ content: string }>("artifact://token", (e) => {
      if (get().generatingKind)
        set({ artifactStreamText: get().artifactStreamText + e.payload.content });
    });
    // Audio Overview synthesis reports per-line progress after the script.
    void listen<{ done: number; total: number }>("audio://progress", (e) => {
      if (get().generatingKind) set({ audioProgress: e.payload });
    });
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
      }
      else if (e.payload.id === "menu-new-window") void api.newWindow();
      else if (e.payload.id === "menu-export-okf") void s.exportNotebookOkf();
    });
    void listen<{ target: string; id: string }>("menu://open-notebook", (e) => {
      if (e.payload.target !== label) return;
      void get().selectNotebook(e.payload.id);
    });
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
      messages: [],
      notes: [],
      reportSchedules: [],
      streamingText: "",
      steps: [],
      followups: [],
      chatConfig,
      summary: localStorage.getItem(`summary:${id}`) ?? "",
      viewingSource: null,
    });
    const nb = get().notebooks.find((n) => n.id === id);
    if (nb) void getCurrentWebviewWindow().setTitle(`${nb.title} — Alchemy`);
    const [sources, messages, notes, reportSchedules] = await Promise.all([
      api.listSources(id),
      api.listMessages(id),
      api.listNotes(id),
      api.listReportSchedules(id),
    ]);
    if (get().currentId === id) set({ sources, messages, notes, reportSchedules });
  },

  closeNotebook: () => {
    void getCurrentWebviewWindow().setTitle("Alchemy");
    set({
      currentId: null,
      sources: [],
      messages: [],
      notes: [],
      reportSchedules: [],
      ingestQueue: [],
      steps: [],
      viewingSource: null,
    });
  },

  setTheme: (theme) => {
    localStorage.setItem("theme", theme);
    applyTheme(theme);
    set({ theme });
  },

  setReading: (patch) => {
    const reading = { ...get().reading, ...patch };
    localStorage.setItem("readingPrefs", JSON.stringify(reading));
    set({ reading });
  },

  clearQueueItem: (id) => set({ ingestQueue: get().ingestQueue.filter((q) => q.id !== id) }),

  setDraggingFiles: (v) => set({ draggingFiles: v }),

  dismissOnboarding: () => {
    localStorage.setItem("onboardingDismissed", "true");
    set({ onboardingDismissed: true });
  },

  openSettings: (tab = "general") => set({ settingsOpen: true, settingsTab: tab }),
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
    localStorage.setItem(panel === "sources" ? "sourcesWidth" : "studioWidth", String(w));
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
      await runQueued(get, set, items[i], () => api.addSourceFile(id, paths[i]));
      if (get().currentId === id) set({ sources: await api.listSources(id) });
    }
  },

  addSourceUrl: async (url) => {
    const id = get().currentId;
    if (!id) return;
    const item: QueueItem = { id: `${Date.now()}`, name: url, status: "pending" };
    set({ ingestQueue: [...get().ingestQueue, item], error: null });
    await runQueued(get, set, item, () => api.addSourceUrl(id, url));
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

  editSourceText: async (sourceId, title, text) => {
    const id = get().currentId;
    if (!id) return;
    const item: QueueItem = { id: `${Date.now()}`, name: title.trim() || "Source", status: "pending" };
    set({ ingestQueue: [...get().ingestQueue, item], error: null });
    await runQueued(get, set, item, () => api.updateSourceText(sourceId, title, text));
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

  deleteSource: (id) =>
    guard(async () => {
      await api.deleteSource(id);
      const nb = get().currentId;
      if (nb) set({ sources: await api.listSources(nb) });
      get().pushToast("success", "Source removed");
    }),

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
      if (get().agentMode) {
        await api.sendMessageAgentic(id, content, cfg);
      } else {
        await api.sendMessage(id, content, cfg);
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

  openSourceViewer: (sourceId, title, highlight) =>
    set({ viewingSource: { sourceId, title, highlight } }),
  closeSourceViewer: () => set({ viewingSource: null }),

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
      const note = await api.generateArtifact(id, kind, prompt);
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
      if (msg.includes("Generation stopped")) get().pushToast("info", "Generation stopped");
      else set({ error: msg });
    } finally {
      set({ generatingKind: null, artifactStreamText: "", audioProgress: null });
    }
  },

  rebuildNote: async (note) => {
    const id = get().currentId;
    if (!id || get().generatingKind) return;
    set({ generatingKind: note.kind, artifactStreamText: "", error: null });
    try {
      const updated = await api.rebuildNote(note.id, id, note.kind, note.prompt);
      set({ notes: get().notes.map((n) => (n.id === updated.id ? updated : n)) });
      playDone();
      void notify("Rebuilt", `“${updated.title}” was regenerated.`);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg.includes("Generation stopped")) get().pushToast("info", "Rebuild stopped");
      else set({ error: msg });
    } finally {
      set({ generatingKind: null, artifactStreamText: "", audioProgress: null });
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
    set({ migration: { done: 0, total: 0, title: "Starting…" }, error: null });
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
    const dest = await open({ directory: true, title: "Export OKF bundle into…" });
    if (!dest) return;
    try {
      const path = await api.exportNotebookOkf(id, dest as string);
      get().pushToast("success", `Exported to ${path}`);
    } catch (e) {
      get().pushToast("error", e instanceof Error ? e.message : String(e));
    }
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
      await api.updateReportSchedule(r.id, r.name, r.kind, r.prompt, r.intervalSecs, r.enabled);
      const id = get().currentId;
      if (id) set({ reportSchedules: await api.listReportSchedules(id) });
    }),

  deleteReport: (rid) =>
    guard(async () => {
      await api.deleteReportSchedule(rid);
      set({ reportSchedules: get().reportSchedules.filter((r) => r.id !== rid) });
    }),

  runReportNow: async (rid) => {
    const schedule = get().reportSchedules.find((r) => r.id === rid);
    set({ generatingKind: "report" });
    try {
      await api.runReport(rid);
      playDone();
      void notify("Report ready", schedule ? `“${schedule.name}” was generated.` : "Report generated.");
      const id = get().currentId;
      if (id) {
        set({ notes: await api.listNotes(id), reportSchedules: await api.listReportSchedules(id) });
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
        due = all.filter((s) => s.enabled && now - s.lastRunAt >= s.intervalSecs * 1000);
      } catch {
        return;
      }
      for (const s of due) {
        try {
          await api.runReport(s.id);
          void notify("Report ready", `“${s.name}” was generated.`);
          const cur = get().currentId;
          if (cur === s.notebookId) {
            set({ notes: await api.listNotes(cur), reportSchedules: await api.listReportSchedules(cur) });
          }
        } catch {
          /* try again next tick */
        }
      }
    };
    void tick();
    setInterval(() => void tick(), 60_000);
  },

  setError: (e) => set({ error: e }),

  pushToast: (kind, message) => {
    const id = `toast-${++toastSeq}`;
    set({ toasts: [...get().toasts, { id, kind, message }] });
    const ttl = kind === "error" ? 7000 : 3500;
    setTimeout(() => get().dismissToast(id), ttl);
  },

  dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
  };
});
