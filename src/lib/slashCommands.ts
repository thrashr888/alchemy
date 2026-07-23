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
    description: "Toggle deep research (agentic retrieval)",
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
