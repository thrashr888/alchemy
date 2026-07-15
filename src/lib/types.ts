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
  sourceCount: number;
}

export interface Source {
  id: string;
  notebookId: string;
  title: string;
  sourceType:
    "pdf" | "text" | "markdown" | "html" | "url" | "image" | "folder" | "mac";
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
}

/** One pickable Mac-provider item (a calendar range, reminders list, note…). */
export interface MacCollection {
  id: string;
  label: string;
  detail: string;
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
  /** Non-empty when the passage came from a note: the note's id. */
  noteId: string;
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
  kind: "chat" | "tool";
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
  | "data_table"
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
}

/** One global-search result (command menu). */
export interface SearchHit {
  kind: "source" | "note" | "content";
  notebookId: string;
  /** Source id for source/content hits; note id for note hits. */
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
  intervalSecs: number;
  enabled: boolean;
  lastRunAt: number;
  createdAt: number;
}

export interface AiConfig {
  /** Chat backend: "ollama" | "openai" (any OpenAI-compatible gateway). */
  provider: string;
  /** Embedding backend: "ollama" | "builtin". */
  embedder: string;
  baseUrl: string;
  chatModel: string;
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
  /** Menu bar extra (tray icon); Settings → General toggles it live. */
  trayEnabled: boolean;
  /** Weekly LLM consolidation of auto-created evidence notes (note curator
   *  phase 5). On by default — idle-gated, capped, fully recoverable; the
   *  toggle is for cost control. */
  curatorConsolidate: boolean;
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
  style: "default" | "learning" | "custom";
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
}

export const DEFAULT_READING_PREFS: ReadingPrefs = {
  font: "sans",
  fontSize: "medium",
  textAlign: "natural",
};
