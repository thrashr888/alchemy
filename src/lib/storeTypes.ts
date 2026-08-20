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
  /** Re-runs the failed import in place (set when status is "error"). */
  retry?: () => void;
}

/** One document open (or remembered) in the center-column reader.
 *  "template" opens the custom-generator editor for that template id. */
export interface ReaderDoc {
  type: "source" | "note" | "template";
  id: string;
  /** Passage to scroll to and highlight (citation jumps). */
  highlight?: string;
}

/** One app-level history entry — a place the user was: dashboard or a
 *  notebook, plus the center-column mode (and the reader's doc, sans
 *  highlight — a citation jump is an event, not a place). */
export interface NavEntry {
  nb: string | null;
  mode: "chat" | "reader" | "ledger" | "gallery";
  doc?: { type: ReaderDoc["type"]; id: string };
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
  messagesHasMore: boolean;
  messagesLoadingOlder: boolean;
  notes: Note[];
  reportSchedules: ReportSchedule[];
  templates: Template[];
  /** Re-list custom templates (after an in-app save/delete). */
  refreshTemplates: () => Promise<void>;
  aiConfig: AiConfig | null;
  ollamaOk: boolean | null;
  modelHealth: ModelHealth | null;
  modelStats: ModelStat[];
  theme: string;
  reading: ReadingPrefs;

  sending: boolean;
  streamingText: string;
  steps: string[];
  /** The live "still waiting" line, if the backend is counting down toward a
   *  timeout. Replaced on every tick and cleared the moment anything else
   *  happens, so it never joins the trail. */
  waiting: string;
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
  /** Version string the quiet startup check found, null until then —
   *  General/About read it to show the update without re-checking. */
  updateAvailable: string | null;
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
  /** Center-column Ledger mode; reader wins below it, chat is the default. */
  ledgerOpen: boolean;
  /** Center-column source Gallery mode; wins above Ledger. */
  galleryOpen: boolean;
  /** Source id the Reader should open straight into edit mode (gallery's
   *  "Edit text" action); the Reader consumes and clears it. */
  readerEditIntent: string | null;
  /** Bumped when an agent writes the ledger (mcp://changed scope "ledger"). */
  ledgerBump: number;
  /** Bumped when the registry changes (agents, or the arrival sweep filing
   *  a document). Corpus-scoped, so it fires with no notebook open. */
  registryBump: number;
  /** Home's center column: the notebook grid, or the Registry's cast. */
  homeSection: "notebooks" | "registry";
  /** How that column lays out. Cards are recognisable, rows are scannable
   *  and sortable — which one is "easier to find things in" depends on the
   *  collection, so it's a per-user choice, remembered. */
  homeView: "grid" | "table";
  /** Inline title filter over whichever section is showing. Not persisted:
   *  a filter you forgot you set is a collection that looks half-empty. */
  homeQuery: string;
  /** Card the Registry section has open; null shows the grid. */
  openCardId: string | null;
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
  /** Browser-style app-level history (Cmd+←/→, View ▸ Back/Forward).
   *  Spans home ↔ notebooks and center-column modes; the reader keeps its
   *  own doc-level history on top of this. */
  nav: { stack: NavEntry[]; index: number };
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
  navBack: () => void;
  navForward: () => void;
  /** Resolves to the new notebook's id. */
  createNotebook: (title: string) => Promise<string>;
  renameNotebook: (id: string, title: string) => Promise<void>;
  setNotebookColor: (id: string, color: string) => Promise<void>;
  setNotebookIcon: (id: string, icon: string) => Promise<void>;
  deleteNotebook: (id: string) => Promise<void>;
  setNotebookStatus: (id: string, status: "" | "archived") => Promise<void>;
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
  importOkfOpen: boolean;
  pendingImportPath: string | null;
  importOkf: (path: string, notebookId?: string | null) => Promise<void>;
  createReport: (
    name: string,
    kind: string,
    prompt: string,
    trigger: string,
    intervalSecs: number,
  ) => Promise<void>;
  updateReport: (report: ReportSchedule) => Promise<void>;
  deleteReport: (id: string) => Promise<void>;
  runReportNow: (id: string) => Promise<void>;

  pickAndAddFiles: () => Promise<void>;
  /** Open the folder picker (optionally rooted at a cloud sync folder) and add
   *  the chosen subfolder as a source. */
  pickAndAddFolder: (defaultPath?: string) => Promise<void>;
  addSourceFiles: (paths: string[]) => Promise<void>;
  addSourceUrl: (url: string, include?: string) => Promise<void>;
  addSourceText: (title: string, text: string) => Promise<void>;
  addSourceMac: (provider: string, collection: string, label: string) => Promise<void>;
  editSourceText: (sourceId: string, title: string, text: string) => Promise<void>;
  /** Set a source's tags (backend normalizes) and refresh the list. */
  setSourceTags: (sourceId: string, tags: string) => Promise<void>;
  /** Set/clear the user annotation on a source and refresh the list. */
  setSourceNote: (sourceId: string, note: string) => Promise<void>;
  refreshSource: (sourceId: string) => Promise<void>;
  handleIntegrationUrl: (raw: string) => Promise<void>;
  pendingExternalAdd: ExternalAdd | null;
  confirmExternalAdd: (notebookId: string, payload?: ExternalAdd) => Promise<void>;
  updateMacNote: (sourceId: string, body: string) => Promise<void>;
  addMacReminder: (sourceId: string, title: string, notes?: string) => Promise<void>;
  deleteSource: (id: string) => Promise<void>;
  toggleSourceSelected: (id: string) => void;
  setAllSourcesSelected: (selected: boolean) => void;
  /** "Ask about this source": scope the chat to one source (a folder scopes
   *  to its files), land in the composer ready to type. */
  askAboutSource: (id: string) => void;

  /** overrideSourceIds: per-message retrieval narrowing from @ mentions —
   *  raw owner ids (source ids and "note:<id>"), replacing the checkbox
   *  selection for this send only. */
  sendMessage: (
    content: string,
    overrideSourceIds?: string[],
    /** One-shot provider override (RFC-self-resolve phase 4): rerun this
     *  question on the named provider without touching the config. */
    providerOverride?: string,
  ) => Promise<void>;
  loadOlderMessages: () => Promise<void>;
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
  appendStep: (label: string, transient: boolean) => void;
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
  pushToast: (kind: ToastKind, message: string, onClick?: () => void) => void;
  dismissToast: (id: string) => void;
  markNotesRead: (ids: string[]) => void;
}
