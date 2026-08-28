import type {
  AiConfig,
  ChatConfig,
  HygieneIssue,
  KokoroStatus,
  Message,
  MetaCitation,
  MetaThread,
  MetaTurn,
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
import type { HistoryEntry } from "./history";

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
  /** Home's section, when this entry IS Home (`nb: null`). Home has tabs the
   *  way a notebook has center modes, and a tab is a place: back should
   *  return you to the Registry, or to the conversation, you were reading. */
  section?: HomeSection;
  /** The Home conversation that was open, for `section: "chat"`. */
  thread?: string | null;
}

/** Home's center column: the notebook grid, the Registry's cast, or the
 *  corpus-wide conversation. */
export type HomeSection = "notebooks" | "registry" | "chat";

/** Home's conversation, as the store holds it: which thread is open and the
 *  turns already settled into it. Both come from the backend — the thread
 *  outlives the window now (docs/RFC-meta-chat.md). */
export interface HomeChatState {
  /** null until the first question mints one. */
  threadId: string | null;
  turns: MetaTurn[];
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

/** Finder-style UI selection (RFC-multi-select) — which rows a batch verb
 *  acts on. Deliberately separate from `selectedSourceIds` (chat scope):
 *  one is "in my retrieval context", the other is "about to be operated
 *  on". One selection at a time, app-wide, like Finder windows. */
export interface Picked {
  kind: "sources" | "notes" | "notebooks" | "cards" | "attachments";
  ids: string[];
  /** Shift-click range endpoint: the row a plain/cmd click last landed on. */
  anchor: string | null;
}

/** One row of a hosted-agent transcript (docs/RFC-acp-agents.md). */
export type AcpEntry =
  | { kind: "user"; text: string }
  | { kind: "agent"; text: string }
  | { kind: "thought"; text: string }
  | { kind: "tool"; id: string; title: string; status: string };

/** One agent's side of a notebook: what it has said, and what the user was
 *  partway through asking it. Kept apart per agent because two agents in one
 *  notebook are two conversations — switching the picker should not show
 *  Codex the transcript of a Claude Code session, or hand it a half-typed
 *  question meant for someone else. */
export interface AcpAgentPane {
  entries: AcpEntry[];
  /** Composer text that was never sent. Restored verbatim on return. */
  draft: string;
  /** The agent's own id for this conversation, kept so the next session can
   *  resume it instead of starting cold. Null until one has been opened, and
   *  meaningless to anyone but that agent. */
  sessionId: string | null;
}

/** Hosted-agent state that must outlive the pane: which agent is selected,
 *  and every agent's transcript and draft, kept per notebook so toggling
 *  Chat ↔ Agent (or remounting the panel, or relaunching the app) restores
 *  the view instead of resetting to the first agent with an empty one. */
export interface AcpPaneState {
  agentId: string | null;
  /** Keyed by agent id. Absent until that agent has something to remember. */
  agents: Record<string, AcpAgentPane>;
}

export interface AppState {
  notebooks: Notebook[];
  currentId: string | null;
  sources: Source[];
  selectedSourceIds: Record<string, boolean> | null;
  picked: Picked | null;
  /** Latest hygiene classification for the current notebook
   *  (RFC-source-hygiene): drives row badges and the review modal. */
  hygiene: HygieneIssue[];
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
  /** Notebook the in-flight send belongs to. Streams keep running when the
   *  user navigates away — this is what stops their tokens painting into
   *  whichever notebook is open instead. */
  sendingFor: string | null;
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
  /** Notebook the in-flight generation belongs to (see sendingFor). */
  generatingFor: string | null;
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
  /** The open notebook's collections (sources, messages, notes, schedules)
   *  are still in flight. Distinguishes "nothing here" from "not here yet". */
  notebookLoading: boolean;
  /** The notebook list failed to load. Distinguishes "no notebooks" from
   *  "we could not find out", which look identical on the shelf. */
  notebooksFailed: boolean;
  /** Session undo history (RFC-professional-grade Pillar 5). Newest last;
   *  a push clears `redoStack`, the standard rule. */
  undoStack: HistoryEntry[];
  redoStack: HistoryEntry[];
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
  /** Home's center column: the notebook grid, the Registry's cast, or the
   *  Chat tab's conversation. */
  homeSection: HomeSection;
  /** The Home conversation currently open (docs/RFC-meta-chat.md). Persisted
   *  per thread in the `meta_turns` table, so it survives a tab switch, a
   *  window close, and a relaunch. */
  homeChat: HomeChatState;
  /** Every Home conversation, most recently used first — the Chat tab's
   *  thread list. Refreshed when a turn settles or a thread is deleted. */
  homeThreads: MetaThread[];
  /** Home chat's own style and length. Per SURFACE, not per thread: asking
   *  across everything is a different job from asking inside one notebook,
   *  and it shouldn't inherit — or overwrite — whatever a notebook is set to. */
  homeChatConfig: ChatConfig;
  /** How that column lays out. Cards are recognisable, rows are scannable
   *  and sortable — which one is "easier to find things in" depends on the
   *  collection, so it's a per-user choice, remembered. */
  homeView: "grid" | "table";
  /** Inline title filter over whichever section is showing. Not persisted:
   *  a filter you forgot you set is a collection that looks half-empty. */
  homeQuery: string;
  /** Card the Registry section has open; null shows the grid. */
  openCardId: string | null;
  /** The Registry's New-card modal, openable from the Home hero too. */
  registryCreating: boolean;
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
  /** Re-read the Chat tab's thread list. */
  refreshHomeThreads: () => Promise<void>;
  /** Open a Home conversation and show it: its turns are loaded from the
   *  backend. `null` starts a fresh one — no row exists until it's asked. */
  openHomeThread: (threadId: string | null) => Promise<void>;
  /** Persist a settled turn into the open thread (minting the thread id on
   *  the first one) and keep it on screen. */
  appendHomeTurn: (
    role: "user" | "assistant",
    content: string,
    citations: MetaCitation[],
    kind: MetaTurn["kind"],
  ) => Promise<void>;
  deleteHomeThread: (threadId: string) => Promise<void>;
  /** Set Home chat's style/length; persisted for the surface. */
  setHomeChatConfig: (config: ChatConfig) => void;
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

  /** Omit the id to export the currently open notebook (palette/menu). */
  exportNotebookOkf: (notebookId?: string) => Promise<void>;
  /** Bumped by Edit > Find; whichever find-capable surface is mounted
   *  (reader, gallery, home) opens its find bar. */
  findBump: number;
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

  // Finder-style selection (RFC-multi-select). Range order comes from the
  // caller's visible row order — the store never re-derives list layout.
  /** Replace the selection with one row (plain click / right-click outside). */
  pickOne: (kind: Picked["kind"], id: string) => void;
  /** Toggle one row in/out (cmd-click). */
  pickToggle: (kind: Picked["kind"], id: string) => void;
  /** Select anchor→id within the given visible order (shift-click). */
  pickRange: (kind: Picked["kind"], orderedIds: string[], id: string) => void;
  /** Marquee result: replace, or union with the existing selection. */
  pickSet: (kind: Picked["kind"], ids: string[], additive: boolean) => void;
  /** Select-all within one list (⌘A). */
  pickAll: (kind: Picked["kind"], ids: string[]) => void;
  clearPicked: () => void;

  // Batch verbs (RFC-multi-select): one IPC call, one re-list, one toast.
  refreshSourcesBatch: (sourceIds: string[]) => Promise<void>;
  deleteSourcesBatch: (sourceIds: string[]) => Promise<void>;
  setSourcesTagsBatch: (sourceIds: string[], tags: string) => Promise<void>;
  deleteNotesBatch: (noteIds: string[]) => Promise<void>;

  /** Re-run the hygiene classification for the current notebook. */
  refreshHygiene: () => Promise<void>;
  /** "Keep" an unreachable source: clear strikes, restart the cadence. */
  hygieneKeep: (sourceId: string) => Promise<void>;
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

  /** Hosted-agent pane state per notebook; see AcpPaneState. Persisted to
   *  localStorage so an app restart reopens on the last session's view. */
  acpPanes: Record<string, AcpPaneState>;
  /** Seed a notebook's pane from localStorage if the store has none yet. */
  hydrateAcpPane: (notebookId: string) => void;
  setAcpAgentId: (notebookId: string, agentId: string | null) => void;
  setAcpEntries: (
    notebookId: string,
    agentId: string,
    update: (prev: AcpEntry[]) => AcpEntry[],
  ) => void;
  setAcpDraft: (notebookId: string, agentId: string, draft: string) => void;
  setAcpSessionId: (
    notebookId: string,
    agentId: string,
    sessionId: string | null,
  ) => void;
  /** Wipe one agent's transcript and draft; the others keep theirs. */
  clearAcpPane: (notebookId: string, agentId: string) => void;

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
  /** Resolves true when the rebuild finished whole; failure is reported
   *  through `error` rather than thrown, so this is how a caller knows. */
  reembedAll: () => Promise<boolean>;
  refreshKokoroStatus: () => Promise<void>;
  setupKokoro: () => Promise<void>;
  removeKokoro: () => Promise<void>;
  setError: (error: string | null) => void;
  pushToast: (kind: ToastKind, message: string, onClick?: () => void) => void;
  dismissToast: (id: string) => void;
  /** Record a reversible mutation silently — for changes that need no toast
   *  of their own (a rename, a tag edit) but should still answer Cmd-Z. */
  pushHistory: (
    label: string,
    undo: () => Promise<void>,
    redo: () => Promise<void>,
  ) => HistoryEntry;
  /** Record a reversible mutation and show its undo toast in one call —
   *  both routes drive the same entry, so undoing twice is impossible. */
  undoableToast: (
    message: string,
    label: string,
    undo: () => Promise<void>,
    redo: () => Promise<void>,
  ) => void;
  /** ⌘Z / ⇧⌘Z, routed from the Edit menu (see menu.rs). */
  undoLast: () => Promise<void>;
  redoLast: () => Promise<void>;
  markNotesRead: (ids: string[]) => void;
}
