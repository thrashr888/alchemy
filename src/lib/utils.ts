import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
import type { AiConfig, ReadingPrefs } from "./types";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/**
 * File extensions the ingester accepts (mirrors the dispatch in
 * src-tauri/src/ingest.rs) — the single list behind the file-pick dialog,
 * OS drag-drop filtering, and the command menu.
 */
export const SUPPORTED_EXTENSIONS = [
  "pdf", "txt", "text", "md", "markdown", "html", "htm", "xhtml",
  "docx", "pptx", "epub", "xlsx", "xls", "xlsm", "ods", "csv", "tsv",
  "gdoc", "gsheet", "gslides",
  "png", "jpg", "jpeg", "jpe", "webp", "gif", "bmp", "tif", "tiff",
  "heic", "heif", "avif", "ico", "jp2",
];

/**
 * A source's `url` holds its origin: a web URL for fetched sources, a local
 * file path for file imports, empty for pasted text. True for the web case.
 */
export function isWebUrl(s: string): boolean {
  return /^https?:\/\//.test(s);
}

/**
 * Has this note (or report) changed since the user last opened it? Notes from
 * before read tracking existed fall under the baseline and count as read.
 */
export function noteUnread(
  n: { id: string; updatedAt: number },
  reads: Record<string, number>,
  baseline: number,
): boolean {
  return n.updatedAt > (reads[n.id] ?? baseline);
}

/** True when a global shortcut should be ignored: typing in a field or inside an open dialog. */
export function shortcutBlocked(e: { target: EventTarget | null }): boolean {
  const t = e.target as HTMLElement | null;
  if (!t?.closest) return false;
  return !!t.closest('[role="dialog"], input, textarea, select, [contenteditable="true"]');
}

/** Reading-preference classes for the chat message container (see index.css). */
export function chatReadingClass(cfg: ReadingPrefs): string {
  const font =
    cfg.font === "serif"
      ? "chat-serif"
      : cfg.font === "mono"
        ? "chat-mono"
        : cfg.font === "system"
          ? "chat-system"
          : "";
  const align = cfg.textAlign === "justified" ? "chat-justify" : "";
  return cn(font, `chat-size-${cfg.fontSize}`, align);
}

/** Human label for the active chat provider. */
export function providerLabel(config: AiConfig | null): string {
  return config?.provider === "openai" ? "Gateway" : "Ollama";
}

export function relativeTime(ms: number): string {
  const diff = Date.now() - ms;
  const s = Math.floor(diff / 1000);
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d}d ago`;
  return new Date(ms).toLocaleDateString();
}
