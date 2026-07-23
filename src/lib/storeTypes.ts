import type {
  AiConfig,
  ChatConfig,
  KokoroStatus,
  Message,
  ModelHealth,
  ModelStat,
  Note,
  NoteKind,
  Notebook,
  ReadingPrefs,
  ReportSchedule,
  Source,
  Template,
  Toast,
  ToastKind,
} from "./types";

export interface QueueItem {
  id: string;
  name: string;
  status: "pending" | "processing" | "done" | "error";
  error?: string;
}

/** One document open (or remembered) in the center-column reader. */
export interface ReaderDoc {
  type: "source" | "note";
  id: string;
  /** Passage to scroll to and highlight (citation jumps). */
  highlight?: string;
}

export interface ExternalAdd {
  files: string[];
  url: string | null;
  text: string | null;
  title: string | null;
}

export interface Migration {
  done: number;
  total: number;
  title: string;
}

export interface AppState {
  notebooks: Notebook[];
  currentId: string | null;
  sources: Source[];
  selectedSourceIds: Record<string, boolean> | null;
  messages: Message[];
  notes: Note[];
  reportSchedules: ReportSchedule[];
  templates: Template[];
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
  generatingTemplateId: string | null;
  ingestQueue: QueueItem[];
  migration: Migration | null;
  draggingFiles: boolean;
  sourcesOpen: boolean;
  studioOpen: boolean;
  sourcesWidth: number;
  studioWidth: number;
  onboardingDismissed: boolean;
  settingsOpen: boolean;
  settingsTab: string;
  paletteOpen: boolean;
  addSourceOpen: boolean;
  addSourceStep: "url" | "text" | null;
  macAvailable: boolean | null;
  pendingAddUrl: boolean;
  pendingAddText: boolean;
  pendingUpdateCheck: boolean;
  embedderDownload: {
    label: string;
    done: number;
    total: number;
    title?: string;
  } | null;
  error: string | null;
  failedInput: string | null;
  pendingInput: string | null;
  pendingAsk: string | null;
  toasts: Toast[];
  justCreatedNoteId: string | null;
  pendingNewNote: boolean;
  artifactStreamText: string;
  audioProgress: { done: number; total: number } | null;
  kokoroStatus: KokoroStatus | null;
  kokoroBusy: boolean;
  /** Center-column reader: current doc + browser-style history. `open`
   *  flips the center column between Chat and Reader; history survives a
   *  return to chat so the Reader tab can restore where you were. */
  reader: {
    open: boolean;
    history: ReaderDoc[];
    index: number;
  };
  folderScan: { done: number; total: number; title: string } | null;
  /** Temp ids of folders inserted optimistically while their children embed —
   *  the Sources panel shows these rows with a loading affordance until
   *  `addSourceFolder` resolves and the real list replaces them. */
  importingFolders: string[];
  noteReads: Record<string, number>;
  noteReadsBaseline: number;

  init: () => Promise<void>;
  bindGlobalListeners: () => void;
  refreshNotebooks: () => Promise<void>;
  selectNotebook: (id: string) => Promise<void>;
  closeNotebook: () => void;
  createNotebook: (title: string) => Promise<void>;
  renameNotebook: (id: string, title: string) => Promise<void>;
  setNotebookColor: (id: string, color: string) => Promise<void>;
  deleteNotebook: (id: string) => Promise<void>;
  setTheme: (theme: string) => void;
  setReading: (patch: Partial<ReadingPrefs>) => void;
  clearQueueItem: (id: string) => void;
  setDraggingFiles: (value: boolean) => void;
  toggleSources: () => void;
  toggleStudio: () => void;
  setPanelWidth: (panel: "sources" | "studio", width: number) => void;
  dismissOnboarding: () => void;
  openSettings: (tab?: string) => void;
  closeSettings: () => void;
  setPaletteOpen: (open: boolean) => void;
  togglePalette: () => void;
  openAddSource: (step?: "url" | "text") => void;
  closeAddSource: () => void;

  exportNotebookOkf: () => Promise<void>;
  shareNotebookOkf: () => Promise<void>;
  importOkfOpen: boolean;
  pendingImportPath: string | null;
  importOkf: (path: string, notebookId?: string | null) => Promise<void>;
  createReport: (
    name: string,
    kind: string,
    prompt: string,
    intervalSecs: number,
  ) => Promise<void>;
  updateReport: (report: ReportSchedule) => Promise<void>;
  deleteReport: (id: string) => Promise<void>;
  runReportNow: (id: string) => Promise<void>;
  startReportScheduler: () => void;

  pickAndAddFiles: () => Promise<void>;
  pickAndAddFolder: () => Promise<void>;
  addSourceFiles: (paths: string[]) => Promise<void>;
  startSourceSync: () => void;
  addSourceUrl: (url: string, include?: string) => Promise<void>;
  addSourceText: (title: string, text: string) => Promise<void>;
  addSourceMac: (provider: string, collection: string, label: string) => Promise<void>;
  editSourceText: (sourceId: string, title: string, text: string) => Promise<void>;
  refreshSource: (sourceId: string) => Promise<void>;
  handleIntegrationUrl: (raw: string) => Promise<void>;
  pendingExternalAdd: ExternalAdd | null;
  confirmExternalAdd: (notebookId: string, payload?: ExternalAdd) => Promise<void>;
  updateMacNote: (sourceId: string, body: string) => Promise<void>;
  addMacReminder: (sourceId: string, title: string, notes?: string) => Promise<void>;
  deleteSource: (id: string) => Promise<void>;
  toggleSourceSelected: (id: string) => void;
  setAllSourcesSelected: (selected: boolean) => void;

  sendMessage: (content: string) => Promise<void>;
  cancelGeneration: (scope?: "chat" | "artifact") => void;
  openSourceViewer: (sourceId: string, title: string, highlight?: string) => void;
  closeSourceViewer: () => void;
  /** Open a document in the center-column reader (pushes history). */
  openInReader: (doc: ReaderDoc) => void;
  /** Leave the reader (back to chat); history survives for the Reader tab. */
  closeReader: () => void;
  /** Browser-style back/forward through reader history. */
  readerNavigate: (delta: 1 | -1) => void;
  /** Step to the previous/next document in rail order (sources then notes). */
  readerStep: (dir: 1 | -1) => void;
  appendToken: (token: string) => void;
  appendStep: (label: string) => void;
  toggleAgentMode: () => void;
  setChatConfig: (config: ChatConfig) => void;
  loadFollowups: () => Promise<void>;
  refreshSummary: () => Promise<void>;
  clearChat: () => Promise<void>;

  generateArtifact: (kind: NoteKind, prompt?: string) => Promise<void>;
  generateFromTemplate: (template: Template) => Promise<void>;
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
  refreshKokoroStatus: () => Promise<void>;
  setupKokoro: () => Promise<void>;
  removeKokoro: () => Promise<void>;
  setError: (error: string | null) => void;
  pushToast: (kind: ToastKind, message: string) => void;
  dismissToast: (id: string) => void;
  markNotesRead: (ids: string[]) => void;
}
