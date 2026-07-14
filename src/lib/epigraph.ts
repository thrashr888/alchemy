// Daily generated epigraph for the hero / blank states. Ornament, not
// content: one short aphorism, regenerated at most daily via local inference,
// with a curated fallback list so absence of a model changes nothing. The
// cached line is only shown while its mood matches the active theme — after
// a theme switch the fallback shows until the next app open regenerates.

import { api } from "./api";
import { THEMES, resolveThemeId } from "./themes";

const DEFAULT_MOOD = "quiet alchemical mysticism";
const KEY = "epigraph";
const DAY_MS = 86_400_000;

/** Curated aphorisms, day-rotated. Same register the generator is asked for. */
const FALLBACKS = [
  "The stone is not found but refined, one reading at a time.",
  "Lead becomes gold slowly; understanding, the same.",
  "Every source is ore; the question is the furnace.",
  "What is dissolved with care returns as clarity.",
  "The work is patient: gather, distill, and ask again.",
  "No flame needed but attention; no vessel but the page.",
  "From many pages, one draught of understanding.",
  "As the corpus ripens, so the answers clarify.",
  "Read as the alchemist weighs: nothing trusted, everything tested.",
  "Knowledge transmutes only in a sealed and quiet vessel.",
];

interface Cached {
  text: string;
  mood: string;
  ts: number;
}

function themeMood(themeKey?: string): string {
  return THEMES[resolveThemeId(themeKey)]?.mood ?? DEFAULT_MOOD;
}

function readCache(): Cached | null {
  try {
    const c = JSON.parse(localStorage.getItem(KEY) ?? "null") as Cached | null;
    return c && typeof c.text === "string" && typeof c.mood === "string" ? c : null;
  } catch {
    return null;
  }
}

/** The generated line for this theme's mood, or null if none cached. */
export function generatedEpigraph(themeKey?: string): string | null {
  const c = readCache();
  return c && c.mood === themeMood(themeKey) ? c.text : null;
}

/** Epigraph to display: the mood-matched generated line, else day-seeded fallback. */
export function currentEpigraph(themeKey?: string): string {
  const gen = generatedEpigraph(themeKey);
  if (gen) return gen;
  const day = Math.floor(Date.now() / DAY_MS);
  return FALLBACKS[(day + themeMood(themeKey).length) % FALLBACKS.length];
}

/** Fire-and-forget on app open: regenerate when stale (>24h) or the theme's
 *  mood changed. Never blocks, never surfaces errors, never live-swaps —
 *  a fresh line appears on the NEXT open. */
export async function refreshEpigraph(themeKey?: string): Promise<void> {
  const mood = themeMood(themeKey);
  const c = readCache();
  if (c && c.mood === mood && Date.now() - c.ts < DAY_MS) return;
  try {
    const text = sanitize(await api.generateEpigraph(mood));
    if (text) localStorage.setItem(KEY, JSON.stringify({ text, mood, ts: Date.now() } satisfies Cached));
  } catch {
    // Inference offline or model misbehaved — the fallback list covers it.
  }
}

/** One clean line or nothing: models that ramble don't get displayed. */
function sanitize(raw: string): string | null {
  const line = raw
    .replace(/[\r\n]+/g, " ")
    .replace(/^["'“”\s]+|["'“”\s.]+$/g, "")
    .trim();
  const words = line.split(/\s+/).length;
  if (!line || line.length > 90 || words < 4 || words > 18) return null;
  return line;
}
