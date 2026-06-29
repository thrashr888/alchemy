import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import { applyTheme, DEFAULT_THEME } from "./themes";
import type { AiConfig, Message, ModelHealth, Note, NoteKind, Notebook, Source } from "./types";

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
  aiConfig: AiConfig | null;
  ollamaOk: boolean | null;
  modelHealth: ModelHealth | null;
  theme: string;

  sending: boolean;
  streamingText: string;
  steps: string[];
  agentMode: boolean;
  generatingKind: NoteKind | null;
  ingestQueue: QueueItem[];
  migration: Migration | null;
  error: string | null;

  init: () => Promise<void>;
  refreshNotebooks: () => Promise<void>;
  selectNotebook: (id: string) => Promise<void>;
  closeNotebook: () => void;
  createNotebook: (title: string) => Promise<void>;
  renameNotebook: (id: string, title: string) => Promise<void>;
  deleteNotebook: (id: string) => Promise<void>;
  setTheme: (theme: string) => void;
  clearQueueItem: (id: string) => void;

  addSourceFiles: (paths: string[]) => Promise<void>;
  addSourceUrl: (url: string) => Promise<void>;
  addSourceText: (title: string, text: string) => Promise<void>;
  editSourceText: (sourceId: string, title: string, text: string) => Promise<void>;
  refreshSource: (sourceId: string) => Promise<void>;
  deleteSource: (id: string) => Promise<void>;

  sendMessage: (content: string) => Promise<void>;
  appendToken: (t: string) => void;
  appendStep: (label: string) => void;
  toggleAgentMode: () => void;
  clearChat: () => Promise<void>;

  generateArtifact: (kind: NoteKind, prompt?: string) => Promise<void>;
  rebuildNote: (note: Note) => Promise<void>;
  createNote: (title: string, content: string) => Promise<void>;
  updateNote: (id: string, title: string, content: string) => Promise<void>;
  deleteNote: (id: string) => Promise<void>;

  saveAiConfig: (config: AiConfig) => Promise<void>;
  refreshModelHealth: () => Promise<void>;
  reembedAll: () => Promise<void>;
  setError: (e: string | null) => void;
}

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

export const useStore = create<AppState>((set, get) => ({
  notebooks: [],
  currentId: null,
  sources: [],
  messages: [],
  notes: [],
  aiConfig: null,
  ollamaOk: null,
  modelHealth: null,
  theme: localStorage.getItem("theme") ?? DEFAULT_THEME,

  sending: false,
  streamingText: "",
  steps: [],
  agentMode: localStorage.getItem("agentMode") === "true",
  generatingKind: null,
  ingestQueue: [],
  migration: null,
  error: null,

  init: async () => {
    applyTheme(get().theme);
    const [notebooks, aiConfig, ollamaOk] = await Promise.all([
      api.listNotebooks(),
      api.getAiConfig(),
      api.checkOllama().catch(() => false),
    ]);
    set({ notebooks, aiConfig, ollamaOk });
    void get().refreshModelHealth();
    // Reopen the last-used notebook if it still exists; otherwise show the picker.
    const last = localStorage.getItem("lastNotebookId");
    if (last && notebooks.some((n) => n.id === last)) {
      await get().selectNotebook(last);
    }
  },

  refreshModelHealth: async () => {
    try {
      set({ modelHealth: await api.checkModels() });
    } catch {
      set({ modelHealth: null });
    }
  },

  refreshNotebooks: async () => set({ notebooks: await api.listNotebooks() }),

  selectNotebook: async (id) => {
    localStorage.setItem("lastNotebookId", id);
    set({ currentId: id, sources: [], messages: [], notes: [], streamingText: "", steps: [] });
    const [sources, messages, notes] = await Promise.all([
      api.listSources(id),
      api.listMessages(id),
      api.listNotes(id),
    ]);
    if (get().currentId === id) set({ sources, messages, notes });
  },

  closeNotebook: () =>
    set({ currentId: null, sources: [], messages: [], notes: [], ingestQueue: [], steps: [] }),

  setTheme: (theme) => {
    localStorage.setItem("theme", theme);
    applyTheme(theme);
    set({ theme });
  },

  clearQueueItem: (id) => set({ ingestQueue: get().ingestQueue.filter((q) => q.id !== id) }),

  createNotebook: async (title) => {
    const nb = await api.createNotebook(title);
    set({ notebooks: [nb, ...get().notebooks] });
    await get().selectNotebook(nb.id);
  },

  renameNotebook: async (id, title) => {
    await api.renameNotebook(id, title);
    await get().refreshNotebooks();
  },

  deleteNotebook: async (id) => {
    await api.deleteNotebook(id);
    const remaining = get().notebooks.filter((n) => n.id !== id);
    set({ notebooks: remaining });
    if (get().currentId === id) {
      if (remaining.length > 0) await get().selectNotebook(remaining[0].id);
      else set({ currentId: null, sources: [], messages: [], notes: [] });
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

  deleteSource: async (id) => {
    await api.deleteSource(id);
    const nb = get().currentId;
    if (nb) set({ sources: await api.listSources(nb) });
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
      createdAt: Date.now(),
    };
    set({
      messages: [...get().messages, optimistic],
      sending: true,
      streamingText: "",
      steps: [],
      error: null,
    });
    try {
      if (get().agentMode) {
        await api.sendMessageAgentic(id, content);
      } else {
        await api.sendMessage(id, content);
      }
      // Reload to get canonical user + assistant rows with citations.
      set({ messages: await api.listMessages(id), streamingText: "" });
      await get().refreshNotebooks();
    } catch (e) {
      set({ error: e instanceof Error ? e.message : String(e) });
    } finally {
      set({ sending: false, streamingText: "", steps: [] });
    }
  },

  appendToken: (t) => set({ streamingText: get().streamingText + t }),

  appendStep: (label) => set({ steps: [...get().steps, label] }),

  toggleAgentMode: () => {
    const next = !get().agentMode;
    localStorage.setItem("agentMode", String(next));
    set({ agentMode: next });
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
    set({ generatingKind: kind, error: null });
    try {
      const note = await api.generateArtifact(id, kind, prompt);
      set({ notes: [note, ...get().notes] });
    } catch (e) {
      set({ error: e instanceof Error ? e.message : String(e) });
    } finally {
      set({ generatingKind: null });
    }
  },

  rebuildNote: async (note) => {
    const id = get().currentId;
    if (!id || get().generatingKind) return;
    set({ generatingKind: note.kind, error: null });
    try {
      const updated = await api.rebuildNote(note.id, id, note.kind, note.prompt);
      set({ notes: get().notes.map((n) => (n.id === updated.id ? updated : n)) });
    } catch (e) {
      set({ error: e instanceof Error ? e.message : String(e) });
    } finally {
      set({ generatingKind: null });
    }
  },

  createNote: async (title, content) => {
    const id = get().currentId;
    if (!id) return;
    const note = await api.createNote(id, title, content);
    set({ notes: [note, ...get().notes] });
  },

  updateNote: async (noteId, title, content) => {
    await api.updateNote(noteId, title, content);
    const id = get().currentId;
    if (id) set({ notes: await api.listNotes(id) });
  },

  deleteNote: async (noteId) => {
    await api.deleteNote(noteId);
    set({ notes: get().notes.filter((n) => n.id !== noteId) });
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

  setError: (e) => set({ error: e }),
}));
