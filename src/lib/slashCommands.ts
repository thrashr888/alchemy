// Slash commands for the chat composer. A single registry drives three
// surfaces: the composer picker (typing "/" in an empty composer), the
// command parser (running a typed "/foo bar"), and the Settings → Shortcuts
// enumeration. Execution lives in ChatPanel (it needs the store, the confirm
// dialog, and the message list); this module is pure metadata + matching so
// it can be imported anywhere without pulling in React or the store.

import { ARTIFACTS, AUDIO_OVERVIEW, type ArtifactFamily } from "@/components/studioArtifacts";

/** Picker groups, in display order. Generators keep their Studio family; the
 *  seven verbs live under "Actions". */
export type SlashFamily = "Generate" | "Learning" | "Documents" | "Actions";

export const SLASH_FAMILIES: SlashFamily[] = [
  "Generate",
  "Learning",
  "Documents",
  "Actions",
];

export interface SlashCommandMeta {
  /** Canonical single token, e.g. "study_guide", "add". */
  name: string;
  family: SlashFamily;
  /** One-line description (Settings + picker). */
  description: string;
  /** Placeholder after the name in the picker, e.g. "<url>", "[on|off]". */
  argHint?: string;
  /** True = pressing Enter/Tab with no argument completes the name instead of
   *  running (the command is meaningless without an argument). */
  argRequired?: boolean;
  /** Extra tokens the parser and filter also accept. */
  aliases?: string[];
}

const FAMILY_OF: Record<ArtifactFamily, SlashFamily> = {
  generate: "Generate",
  learning: "Learning",
  documents: "Documents",
};

// Every built-in generator becomes a command whose name is the kind id.
// AUDIO_OVERVIEW leads the Generate group to mirror the Studio ordering.
const GENERATORS: SlashCommandMeta[] = [AUDIO_OVERVIEW, ...ARTIFACTS].map((a) => ({
  name: a.kind,
  family: FAMILY_OF[a.family],
  description: `Generate ${a.label.toLowerCase()} from your sources`,
  argHint: "[instructions]",
}));

/** How people name a generator in a sentence, beyond its Studio label —
 *  "make me a deck", "write a podcast about". Longest phrase wins, so
 *  "study guide" beats "guide" never (there is no bare "guide"). */
const SPOKEN_NAMES: Record<string, string[]> = {
  slide_deck: ["slides", "deck", "presentation", "slideshow"],
  audio_overview: ["podcast", "audio summary", "audio"],
  faq: ["faqs", "frequently asked questions"],
  study_guide: ["studyguide"],
  timeline: ["chronology"],
  mind_map: ["mindmap"],
};

/** A leading verb that means "produce one of the generators' documents". */
const INTENT_VERB =
  /^(?:please\s+)?(?:can you\s+|could you\s+)?(?:generate|create|make|write|build|produce|draft|give me|prepare)\s+(?:me\s+)?(?:(?:a new|another|some|an|the|a)\s+)?/i;

/** Words a person puts between the generator and what it's about; dropped
 *  from the instructions since the generator already knows its own name. */
const INTENT_JOIN = /^(?:based on|about|from|of|on|using|for|covering|over|with|:|—|-)\s*/i;

const ACTIONS: SlashCommandMeta[] = [
  {
    name: "add",
    family: "Actions",
    description: "Add a source from a web URL",
    argHint: "<url>",
    argRequired: true,
  },
  {
    name: "model",
    family: "Actions",
    description: "Switch which model answers this notebook",
    argHint: "<name>",
    argRequired: true,
  },
  {
    name: "research",
    family: "Actions",
    description: "Toggle deep research: several searches before answering",
    argHint: "[on|off]",
    aliases: ["deep-research", "agent"],
  },
  {
    name: "grep",
    family: "Actions",
    description: "Exact-match search across repo & folder sources",
    argHint: "<pattern>",
    argRequired: true,
  },
  {
    name: "note",
    family: "Actions",
    description: "Save a note, or the last answer as a note",
    argHint: "[text]",
  },
  {
    name: "template",
    family: "Actions",
    description: "Create a reusable custom generator (opens the editor)",
    argHint: "[what it should produce]",
  },
  {
    name: "report",
    family: "Actions",
    description: "Run a scheduled report, or open the report panel",
  },
  {
    name: "clear",
    family: "Actions",
    description: "Clear this conversation",
    aliases: ["reset"],
  },
];

/** The full registry, grouped by family in display order. */
export const SLASH_COMMANDS: SlashCommandMeta[] = [...GENERATORS, ...ACTIONS];

/** Fold spaces, hyphens, underscores, and slashes away so "study guide",
 *  "study-guide", and "studyguide" all normalize to the kind id "studyguide". */
export const slashNorm = (s: string): string =>
  s.toLowerCase().replace(/[\s_/-]+/g, "");

function isSubsequence(needle: string, haystack: string): boolean {
  let i = 0;
  for (let j = 0; j < haystack.length && i < needle.length; j++) {
    if (haystack[j] === needle[i]) i++;
  }
  return i === needle.length;
}

function commandMatches(c: SlashCommandMeta, raw: string, norm: string): boolean {
  const name = slashNorm(c.name);
  if (name.includes(norm) || isSubsequence(norm, name)) return true;
  if (c.aliases?.some((a) => slashNorm(a).includes(norm))) return true;
  // Description fallback so "/deep" surfaces /research etc.
  return c.description.toLowerCase().includes(raw.toLowerCase().trim());
}

/** Filter the registry by the name portion the user has typed (no leading
 *  slash). Preserves registry order so family grouping stays intact. */
export function slashFilter(query: string): SlashCommandMeta[] {
  const norm = slashNorm(query);
  if (!norm) return SLASH_COMMANDS;
  return SLASH_COMMANDS.filter((c) => commandMatches(c, query, norm));
}

/** Read a plain sentence as a generator request: "generate a slide deck
 *  based on TEC-4577" runs the Slide deck generator with "TEC-4577" as its
 *  instructions, exactly as "/slide_deck TEC-4577" would. Only the leading
 *  verb + a generator's name qualifies — "what would a slide deck need?"
 *  is a question and stays a chat message. Returns null when it isn't one. */
export function parseGenerateIntent(text: string): ParsedSlash | null {
  const trimmed = text.trim();
  const m = INTENT_VERB.exec(trimmed);
  if (!m) return null;
  const rest = trimmed.slice(m[0].length);
  const restNorm = rest.toLowerCase();
  let best: { cmd: SlashCommandMeta; phrase: string } | null = null;
  for (const a of [AUDIO_OVERVIEW, ...ARTIFACTS]) {
    const cmd = SLASH_COMMANDS.find((c) => c.name === a.kind);
    if (!cmd) continue;
    const phrases = [
      a.label.toLowerCase(),
      a.kind.replace(/_/g, " "),
      ...(SPOKEN_NAMES[a.kind] ?? []),
    ];
    for (const phrase of phrases) {
      if (!restNorm.startsWith(phrase)) continue;
      // Whole words only: "deck" must not match "decking".
      const after = rest.slice(phrase.length);
      if (after && /^[\p{L}\p{N}]/u.test(after)) continue;
      if (!best || phrase.length > best.phrase.length) best = { cmd, phrase };
    }
  }
  if (!best) return null;
  let arg = rest.slice(best.phrase.length).trim();
  arg = arg.replace(INTENT_JOIN, "").trim();
  return { cmd: best.cmd, arg };
}

export interface ParsedSlash {
  cmd: SlashCommandMeta;
  /** Everything after the command name — trailing instructions/arguments. */
  arg: string;
}

/** Parse a fully typed "/command args" string. Command names are at most two
 *  words once de-underscored ("study guide" → study_guide), so we try a
 *  one-token name first, then a two-token name, treating the remainder as the
 *  argument. Returns null for an unknown command (caller sends it as text). */
export function parseSlash(text: string): ParsedSlash | null {
  if (!text.startsWith("/")) return null;
  const body = text.slice(1);
  if (!body.trim()) return null;
  const tokens = body.split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return null;

  const tryTokens = (n: number): ParsedSlash | null => {
    if (tokens.length < n) return null;
    const guess = slashNorm(tokens.slice(0, n).join(""));
    const cmd = SLASH_COMMANDS.find(
      (c) =>
        slashNorm(c.name) === guess ||
        c.aliases?.some((a) => slashNorm(a) === guess),
    );
    if (!cmd) return null;
    return { cmd, arg: tokens.slice(n).join(" ").trim() };
  };

  return tryTokens(1) ?? tryTokens(2);
}
