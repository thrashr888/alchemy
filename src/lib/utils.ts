import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
import type { AiConfig, ModelHealth, ReadingPrefs } from "./types";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
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
  return config?.provider === "openai" ? "IBM Bob" : "Ollama";
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
    return { label: "IBM Bob", ok: health ? health.chat.working : null };
  }
  return { label: "Ollama", ok: ollamaOk };
}

/** True when targeting Bob's gateway with a key that doesn't look like a Bob key. */
export function bobKeyLooksOff(baseUrl: string, apiKey: string): boolean {
  const key = apiKey.trim();
  if (!key) return false;
  const url = baseUrl.trim();
  const targetsBob = url === "" || url.includes("bob.ibm.com");
  return targetsBob && !key.startsWith("bob_");
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
