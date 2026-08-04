// Mirrors the serde models in src-tauri/src/models.rs (camelCase).

export type ToastKind = "success" | "error" | "info";
export interface Toast {
  id: string;
  kind: ToastKind;
  message: string;
}

export interface Notebook {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  color: string;
  /** "" (active) | "archived" — archived notebooks leave the main grid. */
  status: "" | "archived";
  sourceCount: number;
  /** Deliberate notes, excluding reports. */
  noteCount: number;
  /** Report-kind notes (scheduled runs, briefs). */
  reportCount: number;
}

/** Where an unfiled source should go — the auto-notebooking answer. */
export interface NotebookSuggestion {
  /** Empty when proposing a new notebook; `title` is then the proposal. */
  notebookId: string;
  title: string;
  isNew: boolean;
}

export interface Source {
  id: string;
  notebookId: string;
  title: string;
  sourceType:
    | "pdf"
    | "text"
    | "markdown"
    | "html"
    | "url"
    | "image"
    | "folder"
    | "mac"
    | "code"
    | "git"
    | "notion"
    | "obsidian";
  url: string;
  content: string;
  /** "placeholder" = cloud-sync file not downloaded yet; listed, not embedded. */
  status: "ready" | "error" | "placeholder";
  error: string;
  charCount: number;
  chunkCount: number;
  createdAt: number;
  /** Id of the folder source this file belongs to; empty for top-level. */
  parentId: string;
  /** File mtime (unix millis) recorded at ingest for folder children. */
  mtime: number;
  /** Embedded document authorship (PDF /Author, Office dc:creator, EXIF
   *  Artist) captured at ingest; empty when the format carries none. */
  author: string;
  /** Gallery lead image (og:image) for url sources.
   *  "" = unknown, "-" = checked and the page has none. */
  imageUrl: string;
  /** User tags: space-separated normalized tokens (lowercase, no "#").
   *  Fold into routing and the chat manifest (RFC-source-tags). */
  tags: string;
  /** The user's one annotation on this source ("why I saved this");
   *  indexed for retrieval as their own judgment. */
  note: string;
}

/** One pickable Mac-provider item (a calendar range, reminders list, note…). */
export interface MacCollection {
  id: string;
  label: string;
  detail: string;
}

/** A detected cloud-storage sync root the "Add folder" picker can open into. */
export interface CloudFolder {
  /** Stable machine key: google_drive | onedrive | box | dropbox | icloud. */
  provider: string;
  /** Display name, e.g. "Google Drive". */
  label: string;
  /** Absolute path to the sync root on disk. */
  path: string;
}

/** One Spotlight hit from the Add Source → "Search your Mac" step (mirrors
 *  `FileHit` in src-tauri/src/filesearch.rs). Only `ingestible` rows (and
 *  every folder) can be added. */
export interface MacFileHit {
  name: string;
  path: string;
  /** Lowercased extension without the dot ("" for folders / extensionless). */
  ext: string;
  /** Human kind for the row chip: "Folder", "PDF", "Code", … */
  kind: string;
  isDir: boolean;
  size: number;
  /** mtime in unix millis (0 when unknown). */
  mtime: number;
  ingestible: boolean;
}

/** Tally of what a folder rescan changed. */
export interface FolderScan {
  added: number;
  updated: number;
  removed: number;
  failed: number;
}

export interface Citation {
  chunkId: string;
  /** Empty when the passage came from a note (see noteId). */
  sourceId: string;
  /** Title of the source — or of the note for note passages. */
  sourceTitle: string;
  /** On-disk path of the source's original file; empty for web/mac sources,
   *  note passages, and citations persisted before this field existed. */
  sourcePath?: string;
  /** Non-empty when the passage came from a note: the note's id. */
  noteId: string;
  /**
   * True for source-gist rows (distilled overview evidence, RFC-infinite-
   * context §1). Optional: citations persisted before gists existed lack it.
   */
  gist?: boolean;
  /**
   * True for the user's own per-source annotation (RFC-source-tags);
   * sourceId names the annotated source. Optional: older citations lack it.
   */
  snote?: boolean;
  ordinal: number;
  snippet: string;
  distance: number;
}

export interface Message {
  id: string;
  notebookId: string;
  role: "user" | "assistant";
  content: string;
  citations: Citation[];
  /** "chat" for real answers, "tool" for tool confirmations. */
  kind: "chat" | "tool" | "error";
  /** Provider attribution caption ("Claude Code · $0.04"); empty for user
   *  turns and pre-existing rows. */
  model: string;
  createdAt: number;
}

export type NoteKind =
  | "note"
  | "summary"
  | "faq"
  | "study_guide"
  | "briefing"
  | "timeline"
  | "insights"
  | "flashcards"
  | "quiz"
  | "audio_overview"
  | "mind_map"
  | "slide_deck"
  | "infographic"
  | "data_table"
  | "round_table"
  | "problems"
  | "evidence"
  | "prd"
  | "prfaq"
  | "rfc"
  | "skill"
  | "report"
  | "template";

export interface Note {
  id: string;
  notebookId: string;
  title: string;
  content: string;
  kind: NoteKind;
  prompt: string;
  /** "" for deliberate notes, "auto" for chat-created evidence records.
   *  Editing an auto note flips it to "" (user-owned). */
  origin: string;
  /** Curator state for auto notes: "" | "stale" (dimmed) | "archived"
   *  (out of retrieval, collapsed). Use or an edit revives. */
  status: string;
  createdAt: number;
  updatedAt: number;
}

/** Which build a window belongs to (Settings → About) — dev and the
 *  installed app share a data dir and look identical. */
export interface BuildInfo {
  version: string;
  commit: string;
  /** "dev" (tauri dev) | "release" (installed app). */
  profile: string;
}

/** A custom Studio generator: one ~/Documents/Alchemy/templates/*.md file. */
export interface Template {
  /** Filename stem, e.g. "swot-analysis". */
  id: string;
  name: string;
  description: string;
  /** Generation instruction (file body), run over the notebook's sources. */
  prompt: string;
}

/** Aggregate corpus totals for the home page. */
export interface CorpusStats {
  sources: number;
  chars: number;
  notes: number;
  ledger: number;
}

/** One exact-match window from the `/grep` chat command (Rust grep_sources). */
export interface GrepHit {
  sourceId: string;
  sourceTitle: string;
  /** Absolute file path of the repo-/folder-backed child source. */
  path: string;
  /** 1-based line where the window begins. */
  line: number;
  /** The matching line window, real lines joined with whitespace intact. */
  window: string;
}

/** One global-search result (command menu). */
export interface SearchHit {
  kind: "source" | "note" | "content" | "card" | "ledger";
  notebookId: string;
  /** Source id for source/content hits, note id for notes, card id for
   *  registry cards (which carry no notebookId — they're corpus-scoped),
   *  entry id for ledger rows. */
  id: string;
  title: string;
  snippet: string;
}

/** Podcast voice model (Kokoro) readiness. */
export interface KokoroStatus {
  downloaded: boolean;
  /** A test synthesis succeeded — the Audio Overview generator may show. */
  verified: boolean;
}

export interface ModelStatus {
  name: string;
  installed: boolean;
  working: boolean;
  detail: string;
}

export interface ModelHealth {
  reachable: boolean;
  chat: ModelStatus;
  embed: ModelStatus;
  vision: ModelStatus;
}

export interface ModelStat {
  name: string;
  lastTokensPerSec: number;
  avgTokensPerSec: number;
  samples: number;
}

export interface ReportSchedule {
  id: string;
  notebookId: string;
  name: string;
  kind: string;
  prompt: string;
  /** "interval" (clock-fired) or "change" (a standing question — runs when
   *  sources in the notebook change, intervalSecs as the throttle floor). */
  trigger: "interval" | "change";
  intervalSecs: number;
  enabled: boolean;
  lastRunAt: number;
  createdAt: number;
}

/** One anchor pinning a ledger entry to verbatim source text. */
export interface LedgerAnchor {
  sourceId: string;
  quote: string;
}

/** One typed ledger row (the Steward's memory): kind-specific lifecycles —
 *  assertion asserted→corroborated|contradicted|stale, fact current→
 *  superseded, decision decided→superseded, question open→answered, log. */
export interface LedgerEntry {
  id: string;
  notebookId: string;
  kind: "assertion" | "fact" | "decision" | "question" | "log";
  text: string;
  why: string;
  status: string;
  /** "" for user/agent rows, "auto" for chat-minted rows. */
  origin: string;
  anchors: LedgerAnchor[];
  createdAt: number;
  updatedAt: number;
}

/** The Registry's kinds (RFC-registry). */
export type CardKind =
  | "asset"
  | "person"
  | "policy"
  | "provider"
  | "project"
  | "dependency";

/** One key fact on a card — the reader's doc-properties grid shape. */
export interface CardFact {
  label: string;
  value: string;
}

/** One document filed under a card. `matched` is the receipt and is never
 *  empty: the identifier string that matched, "name", or "manual". */
export interface CardAttachment {
  sourceId: string;
  notebookId: string;
  status: "confirmed" | "proposed" | "rejected";
  matched: string;
  at: number;
}

/** One registry card (RFC-registry): a confirmed cast member. Corpus-scoped
 *  — deliberately has no notebookId; its home is derived from where its
 *  documents live. Cards have no lifecycle; their attachments do. */
export interface RegistryCard {
  id: string;
  kind: CardKind;
  name: string;
  /** "" = yours (made or confirmed), "auto" = suggested and awaiting your
   *  verdict, "dismissed" = turned down and remembered so the suggester
   *  never proposes it again. */
  origin: "" | "auto" | "dismissed";
  /** Space-separated normalized tokens — the only thing that ever attaches
   *  a document without asking. */
  identifiers: string;
  note: string;
  facts: CardFact[];
  attachments: CardAttachment[];
  createdAt: number;
  updatedAt: number;
}

/** One observed source change (watchers, RFC-night-shift): written by the
 *  refresh paths, read by the Brief, the Staff section, and agents. */
export interface SourceEvent {
  id: string;
  notebookId: string;
  sourceId: string;
  sourceTitle: string;
  kind: string;
  detail: string;
  /** Capped ±-prefixed line excerpt; empty when nothing textual changed. */
  diff: string;
  at: number;
}

export interface NightShiftStatus {
  backgroundEnabled: boolean;
  paused: boolean;
}

/** One configured inference provider (list entry in Settings → Models). */
export interface ProviderEntry {
  id: string;
  kind: string;
  label: string;
  baseUrl: string;
  apiKey: string;
  chatModel: string;
}

export interface AiConfig {
  providers: ProviderEntry[];
  chatProvider: string;
  studioProvider: string;
  /** Chat backend: "ollama" | "openai" (any OpenAI-compatible gateway). */
  provider: string;
  /** Embedding backend: "ollama" | "builtin". */
  embedder: string;
  baseUrl: string;
  chatModel: string;
  /** Ollama model for the Small role (gists, tags, Weave verdicts, registry
   *  suggestions). Empty = Apple Foundation Models when available, else the
   *  chat provider. */
  smallModel: string;
  embedModel: string;
  visionModel: string;
  openaiBaseUrl: string;
  openaiApiKey: string;
  openaiChatModel: string;
  openaiVisionModel: string;
  /** Who the user is; woven into system prompts so answers fit them. */
  profile: UserProfile;
  /** Embedded MCP server for agent access (localhost streamable HTTP). */
  mcpEnabled: boolean;
  mcpPort: number;
  /** Browser-extension clip receiver (localhost; accepts a rendered DOM from
   *  the user's logged-in tab, see docs/RFC-page-capture.md §8). */
  clipEnabled: boolean;
  clipPort: number;
  /** Menu bar extra (tray icon); Settings → General toggles it live. Also
   *  the residency switch: tray on = closing the window leaves Alchemy
   *  running in the menu bar (docs/RFC-night-shift.md). */
  trayEnabled: boolean;
  /** Night Shift master switch: scheduled reports + automatic source
   *  resyncs from the resident scheduler. Off = on-demand only. */
  backgroundEnabled: boolean;
  /** Desktop notifications; in config (not localStorage) so the resident
   *  scheduler can honor it with no window open. */
  showNotifications: boolean;
  /** Weekly LLM consolidation of auto-created evidence notes (note curator
   *  phase 5). On by default — idle-gated, capped, fully recoverable; the
   *  toggle is for cost control. */
  curatorConsolidate: boolean;
  visionProvider: string;
  setupSeen: boolean;
  gitSyncMinutes: number;
  notionToken: string;
}

/** One passage behind a meta-chat answer: what it is and where it lives. */
export interface MetaCitation {
  kind: "source" | "note";
  notebookId: string;
  notebookTitle: string;
  /** Source id for source passages; note id for notes. */
  id: string;
  title: string;
  snippet: string;
}

/** A corpus-wide answer (docs/RFC-meta-chat.md). */
export interface MetaAnswer {
  answer: string;
  citations: MetaCitation[];
}

export interface McpStatus {
  running: boolean;
  port: number;
  url: string;
}

/** One agent client (Claude Code, Codex, …) and its connection state. */
export interface ConnectorStatus {
  id: string;
  name: string;
  installed: boolean;
  configured: boolean;
  /** False = we don't write its config; user copies the snippet. */
  canAuto: boolean;
  supportsSkill: boolean;
  skillInstalled: boolean;
  /** CLI one-liner or config snippet for manual setup. */
  snippet: string;
  /** Where its config lives, e.g. "~/.codex/config.toml". */
  configPath: string;
}

export interface UserProfile {
  name: string;
  profession: string;
  /** Standing instructions, kept in mind across chats and generations. */
  instructions: string;
}

export interface ChatConfig {
  /** Writing-standard ids mirror rag::CHAT_STYLES in the backend. */
  style:
    | "default"
    | "learning"
    | "scientific"
    | "adhd"
    | "ste100"
    | "govuk"
    | "plain"
    | "custom";
  customPrompt: string;
  length: "default" | "longer" | "shorter";
}

export const DEFAULT_CHAT_CONFIG: ChatConfig = {
  style: "default",
  customPrompt: "",
  length: "default",
};

/** Global chat reading preferences (display-only; set in Appearance). */
export interface ReadingPrefs {
  font: "sans" | "serif" | "mono" | "system";
  fontSize: "small" | "medium" | "large";
  textAlign: "natural" | "justified";
  /** Reader: floating table of contents on structured documents. */
  showToc: boolean;
  /** Reader: the related-passages rail beside documents and the editor. */
  showRelated: boolean;
  /** Experimental: window vibrancy behind translucent sidebar chrome. */
  glass: boolean;
  /** Glass opacity level, mirroring macOS's Clear/Tinted styles. */
  glassStyle: "tinted" | "clear";
}

export const DEFAULT_READING_PREFS: ReadingPrefs = {
  font: "sans",
  fontSize: "medium",
  textAlign: "natural",
  showToc: true,
  showRelated: true,
  glass: false,
  glassStyle: "tinted",
};
