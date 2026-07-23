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
  "docx", "pptx", "epub", "boxnote", "xlsx", "xls", "xlsm", "ods", "csv", "tsv",
  "gdoc", "gsheet", "gslides",
  "png", "jpg", "jpeg", "jpe", "webp", "gif", "bmp", "tif", "tiff",
  "heic", "heif", "avif", "ico", "jp2",
  // Code and config (mirrors CODE_EXTENSIONS in src-tauri/src/ingest.rs) —
  // ingested verbatim and chunked code-aware.
  "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "rb", "java",
  "kt", "kts", "swift", "c", "h", "cc", "cpp", "hpp", "hh", "m", "mm",
  "php", "sh", "bash", "zsh", "fish", "sql", "scala", "lua", "r", "ex",
  "exs", "erl", "zig", "nix", "proto", "graphql", "vue", "svelte",
  "css", "scss", "less", "toml", "yaml", "yml", "json", "jsonc", "hcl",
  "tf", "tfvars", "ini", "cfg", "conf", "env", "xml", "plist", "gradle",
  "cmake", "asm", "s", "d", "dart", "hs", "ml", "clj", "cljs", "el",
  "vim", "ps1", "bat", "cmd",
];

/**
 * A source's `url` holds its origin: a web URL for fetched sources, a local
 * file path for file imports, empty for pasted text. True for the web case.
 */
/** A source/note title with visible characters, else "". `trim()` alone is
 *  insufficient — a captured page title can be a zero-width space or BOM
 *  (U+200B, U+FEFF), which isn't whitespace, so trim keeps it and the row
 *  renders blank. Strips whitespace + zero-width/control chars. */
export function visibleTitle(title: string): string {
  // Visible = any char that isn't whitespace, control, or zero-width /
  // BOM formatting (U+200B-200D, U+2060, U+FEFF). trim() misses those.
  const zeroWidth = /[\u200b-\u200d\u2060\ufeff]/;
  for (const ch of title) {
    // eslint-disable-next-line no-control-regex
    if (!/\s/.test(ch) && !/[\u0000-\u001f]/.test(ch) && !zeroWidth.test(ch)) {
      return title.trim();
    }
  }
  return "";
}

export function isWebUrl(s: string): boolean {
  return /^https?:\/\//.test(s);
}

/**
 * Human cloud-provider label for a folder source's local path, or null when
 * it isn't under a known sync root. A pure mirror of the backend's
 * `list_cloud_folders` detection — provenance is derived from the path alone,
 * so no new Source field is needed. macOS File Provider mounts live under
 * ~/Library/CloudStorage/<Provider>-<account>; iCloud under Mobile Documents;
 * older clients keep ~/Dropbox and ~/Box at the home root.
 */
export function folderProvider(path: string): string | null {
  const cloud = path.match(/\/Library\/CloudStorage\/([^/]+)/);
  if (cloud) {
    const dir = cloud[1];
    if (dir.startsWith("GoogleDrive-")) return "Google Drive";
    if (dir.startsWith("OneDrive")) return "OneDrive";
    if (dir === "Box" || dir.startsWith("Box-")) return "Box";
    if (dir.startsWith("Dropbox")) return "Dropbox";
  }
  if (path.includes("/Library/Mobile Documents/com~apple~CloudDocs"))
    return "iCloud Drive";
  // Legacy top-level sync roots, anchored to the home dir so an unrelated
  // "Box" or "Dropbox" project folder deeper in the tree doesn't match.
  if (/^\/(?:Users|home)\/[^/]+\/Dropbox(?:\/|$)/.test(path)) return "Dropbox";
  if (/^\/(?:Users|home)\/[^/]+\/Box(?:\/|$)/.test(path)) return "Box";
  return null;
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

/** True when a global shortcut should be ignored: a dialog is open or the user is typing in a field. */
export function shortcutBlocked(e: { target: EventTarget | null }): boolean {
  if (document.querySelector('[role="dialog"]')) return true;
  const t = e.target as HTMLElement | null;
  if (!t?.closest) return false;
  return !!t.closest('input, textarea, select, [contenteditable="true"]');
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

/** Cached absolute-day formatter — Intl.DateTimeFormat construction is
 *  expensive and these render in hot paths (properties rows, report meta). */
const dayFormat = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  year: "numeric",
});
export function fmtDay(ms: number): string {
  return dayFormat.format(ms);
}

/** Hostname of a URL, or null when it doesn't parse (hand-ingested source
 *  URLs are resilient-but-messy). */
export function urlHost(url: string): string | null {
  try {
    return new URL(url).hostname;
  } catch {
    return null;
  }
}

/** Cached compact number formatter ("1.2M", "48K") — Intl construction is
 *  expensive and this renders per folder row. */
const compactFormat = new Intl.NumberFormat("en", {
  notation: "compact",
  maximumFractionDigits: 1,
});
export function compactNumber(n: number): string {
  return compactFormat.format(n);
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

/** The paragraph the user is working on: the first one that changed, or the
 *  last non-empty one when nothing differs (e.g. on entry). Lives here (not
 *  in AmbientRail) so component modules keep components-only exports and
 *  Vite Fast Refresh never has to invalidate them. */
export function activeParagraph(prev: string, next: string): string {
  const a = prev.split(/\n{2,}/);
  const b = next.split(/\n{2,}/);
  for (let i = 0; i < b.length; i++) {
    if (a[i] !== b[i]) return (b[i] ?? "").trim().slice(0, 600);
  }
  for (let i = b.length - 1; i >= 0; i--) {
    const p = (b[i] ?? "").trim();
    if (p) return p.slice(0, 600);
  }
  return "";
}
