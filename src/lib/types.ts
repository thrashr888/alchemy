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
  sourceCount: number;
}

export interface Source {
  id: string;
  notebookId: string;
  title: string;
  sourceType: "pdf" | "text" | "markdown" | "url" | "image";
  url: string;
  content: string;
  status: "ready" | "error";
  error: string;
  charCount: number;
  chunkCount: number;
  createdAt: number;
}

export interface Citation {
  chunkId: string;
  sourceId: string;
  sourceTitle: string;
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
  | "problems"
  | "prd"
  | "prfaq"
  | "rfc"
  | "skill"
  | "report";

export interface Note {
  id: string;
  notebookId: string;
  title: string;
  content: string;
  kind: NoteKind;
  prompt: string;
  createdAt: number;
  updatedAt: number;
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
