// Mirrors the serde models in src-tauri/src/models.rs (camelCase).

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
  sourceType: "pdf" | "text" | "markdown" | "url";
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
  createdAt: number;
}

export type NoteKind =
  | "note"
  | "summary"
  | "faq"
  | "study_guide"
  | "briefing"
  | "timeline"
  | "prd"
  | "prfaq"
  | "rfc"
  | "skill";

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
}

export interface ModelStat {
  name: string;
  lastTokensPerSec: number;
  avgTokensPerSec: number;
  samples: number;
}

export interface AiConfig {
  baseUrl: string;
  chatModel: string;
  embedModel: string;
}
