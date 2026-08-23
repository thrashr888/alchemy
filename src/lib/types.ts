// Mirrors the serde models in src-tauri/src/models.rs (camelCase).

export type ToastKind = "success" | "error" | "info";
export interface Toast {
  id: string;
  kind: ToastKind;
  message: string;
  /** Present → the toast body is clickable (e.g. "update available" opens
   *  Settings). Clicking dismisses the toast too. */
  onClick?: () => void;
}

export interface Notebook {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  color: string;
  /** Lucide icon slug, "" → default book. See lib/notebookIcons.ts. */
  icon: string;
  /** "" (active) | "archived" | "system". Archived notebooks leave the main
   *  grid; system notebooks (Briefs) never appear on the shelf at all but
   *  work like any other notebook when opened. */
  status: "" | "archived" | "system";
  sourceCount: number;
  /** Deliberate notes, excluding reports. */
  noteCount: number;
  /** Report-kind notes (scheduled runs, briefs). */
  reportCount: number;
}

/** One document in the notebook link graph. */
export interface GraphNode {
  id: string;
  kind: "source" | "note";
  title: string;
  sourceType: string;
  /** Inbound + outbound edges, used to size the node. */
  degree: number;
}

/** The notebook as a link graph (RFC-document-surface phase 5). */
export interface NotebookGraph {
  nodes: GraphNode[];
  edges: { from: string; to: string }[];
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
  /** "placeholder" = cloud-sync file not downloaded yet; listed, not embedded.
   *  "processing" = landed and readable; chunks/embeddings still indexing in
   *  the background (RFC-import-pipeline §2) — flips to "ready" via events. */
  status: "ready" | "error" | "placeholder" | "processing";
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

export interface MessagePage {
  messages: Message[];
  hasMore: boolean;
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

/** One local calendar day of activity (Settings → Activity; Rust activity.rs). */
export interface ActivityDay {
  /** Local date, "YYYY-MM-DD". */
  date: string;
  messages: number;
  sources: number;
  notes: number;
  retrievals: number;
}

/** A labeled count for the "most used" lists. */
export interface ActivityCount {
  label: string;
  count: number;
}

/** Everything Settings → Activity renders, aggregated read-time in Rust. */
export interface ActivityStats {
  /** Ascending by date; sparse — active days only. */
  days: ActivityDay[];
  totalMessages: number;
  totalUserMessages: number;
  totalSources: number;
  totalNotes: number;
  totalNotebooks: number;
  /** Retained trace history only (~months), not lifetime. */
  totalRetrievals: number;
  corpusChars: number;
  assistantWords: number;
  activeDays: number;
  currentStreak: number;
  longestStreak: number;
  /** Local hour 0–23; -1 with no messages. */
  peakHour: number;
  favoriteModel: string;
  models: ActivityCount[];
  notebooks: ActivityCount[];
  sourceTypes: ActivityCount[];
  firstActivityAt: number;
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

export interface HomeActivity {
  schedules: ReportSchedule[];
  recentNotes: Note[];
  reports: Note[];
  stats: CorpusStats;
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
  "asset" | "person" | "policy" | "provider" | "project" | "dependency";

/** One key fact on a card — the reader's doc-properties grid shape. */
export interface CardFact {
  label: string;
  value: string;
}

/** What an explicit "suggest cards" ask produced. `reply` carries the
 *  model's raw answer so "it suggested nothing" can be told apart from
 *  "it said something that didn't survive the grounding gate". */
export interface SuggestOutcome {
  created: string[];
  reply: string;
  /** True when another suggest pass already held the single-flight guard
   *  and this ask did nothing. */
  alreadyRunning: boolean;
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
  /** Triage verdict on a still-suggested card: "" = not yet triaged,
   *  "recommended" = the triage pass thinks this one matters, "routine" =
   *  triaged and not singled out. Cleared once the card is ruled on. */
  triage: "" | "recommended" | "routine";
  /** Space-separated normalized tokens (VIN, policy number, serial) — the
   *  strongest auto-attach signal; strong full-name matches attach too,
   *  with a "name matched" receipt. */
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

/** What a provider offers the model picker. `supportsDefault` is false for a
 *  gateway, which has no fallback model of its own — it needs a name. */
export interface ProviderModels {
  models: string[];
  supportsDefault: boolean;
  /** Reasoning-effort levels, cheapest first. Empty = this provider has no
   *  effort control, and the composer hides the pill entirely. */
  efforts: string[];
  /** What "Default" resolves to, when knowable — Ollama falls through to the
   *  app's main model. `null` for a vendor CLI, whose default is its own
   *  business and which won't tell us. */
  defaultModel: string | null;
}

/** One configured inference provider (list entry in Settings → Models). */
export interface ProviderEntry {
  id: string;
  kind: string;
  label: string;
  baseUrl: string;
  apiKey: string;
  chatModel: string;
  /** Reasoning effort; "" = the provider's own default. */
  effort: string;
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
  /** Hosted coding agent the Agent view opens with — an `acpAgents` id, or
   *  empty for "whichever is installed". */
  hostedAgent: string;
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
  /** Quiet-while-focused rule: skip notifications and sound cues while an
   *  Alchemy window is focused. On by default; off = always deliver. */
  quietWhenFocused: boolean;
  /** Weekly LLM consolidation of auto-created evidence notes (note curator
   *  phase 5). On by default — idle-gated, capped, fully recoverable; the
   *  toggle is for cost control. */
  curatorConsolidate: boolean;
  visionProvider: string;
  setupSeen: boolean;
  gitSyncMinutes: number;
  notionToken: string;
  /** Diagnose-and-suggest on unclassified provider errors (RFC-self-resolve
   *  phase 2): one small-model call per unknown failure. On by default; the
   *  toggle is cost control. */
  selfDiagnose: boolean;
}

/** One passage behind a meta-chat answer: what it is and where it lives. */
export interface MetaCitation {
  /** "card" = a registry card riding as answer context (corpus-scoped:
   *  notebookId is empty and the citation opens the card on Home). */
  kind: "source" | "note" | "card";
  notebookId: string;
  notebookTitle: string;
  /** Source id for source passages; note id for notes; card id for cards. */
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

/** An ACP-capable agent Alchemy can host (docs/RFC-acp-agents.md). */
export interface AcpAgentInfo {
  id: string;
  label: string;
  available: boolean;
  /** Terminal command that signs this agent in, offered as an auth-failure fix. */
  loginCommand: string;
}

/** Lifecycle of a hosted agent session, from the `acp://state` event. */
export interface AcpStateEvent {
  notebookId: string;
  agentId: string;
  state: "starting" | "ready" | "turn" | "idle" | "error" | "stopped";
  detail?: unknown;
}

/** One `session/update` notification, passed through as the ACP schema
 *  shape — `sessionUpdate` discriminates the variant. */
export interface AcpUpdateEvent {
  notebookId: string;
  update: {
    sessionUpdate: string;
    content?: { type: string; text?: string };
    title?: string;
    status?: string;
    toolCallId?: string;
    [key: string]: unknown;
  };
}

/** A permission request awaiting the user's answer. */
export interface AcpPermissionEvent {
  notebookId: string;
  requestId: string;
  toolTitle: string;
  options: { id: string; name: string; kind: string }[];
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
    | "friendly"
    | "professional"
    | "scientific"
    | "adhd"
    | "ste100"
    | "govuk"
    | "plain"
    | "gdev"
    | "custom";
  customPrompt: string;
  /** Persisted ids; the UI labels these Concise, Balanced, and Thorough. */
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
