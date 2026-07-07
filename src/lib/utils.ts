import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
import type { KeyboardEvent } from "react";
import type { AiConfig, ModelHealth, ReadingPrefs } from "./types";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/**
 * File extensions the ingester accepts (mirrors the dispatch in
 * src-tauri/src/ingest.rs) — the single list behind the file-pick dialog,
 * OS drag-drop filtering, and the command menu.
 */
export const SUPPORTED_EXTENSIONS = [
  "pdf", "txt", "text", "md", "markdown",
  "docx", "pptx", "xlsx", "xls", "xlsm", "ods", "csv", "tsv",
  "gdoc", "gsheet", "gslides",
  "png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff", "heic",
];

/**
 * A source's `url` holds its origin: a web URL for fetched sources, a local
 * file path for file imports, empty for pasted text. True for the web case.
 */
export function isWebUrl(s: string): boolean {
  return /^https?:\/\//.test(s);
}

/**
 * Make a clickable non-button element (card, list row) keyboard-operable:
 * focusable, announced as a button, activated with Enter or Space.
 * Spread alongside the element's onClick.
 */
/** True when a global shortcut should be ignored: typing in a field or inside an open dialog. */
export function shortcutBlocked(e: { target: EventTarget | null }): boolean {
  const t = e.target as HTMLElement | null;
  if (!t?.closest) return false;
  return !!t.closest('[role="dialog"], input, textarea, select, [contenteditable="true"]');
}

export function cardButtonProps(onActivate: () => void) {
  return {
    role: "button" as const,
    tabIndex: 0,
    onKeyDown: (e: KeyboardEvent) => {
      // Only when the card itself is focused — not a button inside it.
      if (e.target !== e.currentTarget) return;
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        onActivate();
      }
    },
  };
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

/**
 * Connection status for the active provider — so UI never reports "Ollama
 * offline" to a gateway user who doesn't run Ollama. `ok` is null while unknown.
 */
export function providerStatus(
  config: AiConfig | null,
  ollamaOk: boolean | null,
  health: ModelHealth | null,
): { label: string; ok: boolean | null } {
  if (config?.provider === "openai") {
    return { label: "Gateway", ok: health ? health.chat.working : null };
  }
  return { label: "Ollama", ok: ollamaOk };
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
