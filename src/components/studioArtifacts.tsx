import type { ReactNode } from "react";
import type { NoteKind } from "@/lib/types";
import {
  AudioLines,
  BarChart3,
  Boxes,
  ClipboardList,
  Clock,
  Database,
  FileCode2,
  FileText,
  Footprints,
  GraduationCap,
  HelpCircle,
  Layers,
  Lightbulb,
  ListChecks,
  Megaphone,
  Network,
  Newspaper,
  Presentation,
  Quote,
  Route,
  Sparkles,
  StickyNote,
  Table,
  TriangleAlert,
  Users,
  Waypoints,
  Workflow,
} from "lucide-react";

export type ArtifactFamily = "generate" | "learning" | "documents";

/** Quiet color as wayfinding: each family owns one accent hue (tokens in
 *  index.css). The icon carries it — on generator tiles and on note cards —
 *  so a surface is identifiable at a glance without a filled chip or border
 *  accent. Kinds outside a family (plain notes, reports, templates) stay
 *  neutral. */
export const FAMILY_ACCENT: Record<ArtifactFamily, string> = {
  generate: "text-artifact-generate",
  learning: "text-artifact-learning",
  documents: "text-artifact-documents",
};

export type Artifact = {
  kind: NoteKind;
  label: string;
  icon: ReactNode;
  family: ArtifactFamily;
};

function inFamily(
  family: ArtifactFamily,
  artifacts: Omit<Artifact, "family">[],
): Artifact[] {
  return artifacts.map((artifact) => ({ ...artifact, family }));
}

/** Shown only once the voice model is downloaded and verified. */
export const AUDIO_OVERVIEW: Artifact = {
  kind: "audio_overview",
  label: "Audio Overview",
  icon: <AudioLines className="h-3.5 w-3.5" />,
  family: "generate",
};

/** Layout groups for Studio and the command menu: what a person is trying
 *  to do, in the order people actually reach for them (the note counts in
 *  docs/RFC-ablation.md), most-used first inside each group. Families stay
 *  the color; groups are the shelf. */
export type ArtifactGroup = "understand" | "learn" | "visualize" | "write";

export const GROUP_LABEL: Record<ArtifactGroup, string> = {
  understand: "Understand",
  learn: "Learn",
  visualize: "Visualize",
  write: "Write",
};

const UNDERSTAND = inFamily("generate", [
  { kind: "summary", label: "Summary", icon: <FileText className="h-3.5 w-3.5" /> },
  { kind: "briefing", label: "Briefing", icon: <Newspaper className="h-3.5 w-3.5" /> },
  { kind: "faq", label: "FAQ", icon: <HelpCircle className="h-3.5 w-3.5" /> },
  { kind: "timeline", label: "Timeline", icon: <Clock className="h-3.5 w-3.5" /> },
  { kind: "data_table", label: "Data table", icon: <Table className="h-3.5 w-3.5" /> },
  { kind: "insights", label: "Insights", icon: <Lightbulb className="h-3.5 w-3.5" /> },
  { kind: "round_table", label: "Round table", icon: <Users className="h-3.5 w-3.5" /> },
  { kind: "problems", label: "Problems", icon: <TriangleAlert className="h-3.5 w-3.5" /> },
  { kind: "evidence", label: "Evidence log", icon: <Quote className="h-3.5 w-3.5" /> },
]);

const LEARN = inFamily("learning", [
  {
    kind: "study_guide",
    label: "Study guide",
    icon: <GraduationCap className="h-3.5 w-3.5" />,
  },
  { kind: "quiz", label: "Quiz", icon: <ListChecks className="h-3.5 w-3.5" /> },
  { kind: "flashcards", label: "Flashcards", icon: <Layers className="h-3.5 w-3.5" /> },
]);

// Diagrams and decks share the learning accent: they are the same kind of
// artifact, a picture of the material, whichever shelf they came from.
const VISUALIZE = inFamily("learning", [
  { kind: "process", label: "Process map", icon: <Route className="h-3.5 w-3.5" /> },
  {
    kind: "relationship",
    label: "Relationship map",
    icon: <Network className="h-3.5 w-3.5" />,
  },
  { kind: "mind_map", label: "Mind map", icon: <Waypoints className="h-3.5 w-3.5" /> },
  {
    kind: "slide_deck",
    label: "Slide deck",
    icon: <Presentation className="h-3.5 w-3.5" />,
  },
  {
    kind: "infographic",
    label: "Infographic",
    icon: <BarChart3 className="h-3.5 w-3.5" />,
  },
  { kind: "journey", label: "Journey map", icon: <Footprints className="h-3.5 w-3.5" /> },
  {
    kind: "architecture",
    label: "Architecture diagram",
    icon: <Boxes className="h-3.5 w-3.5" />,
  },
  { kind: "data_model", label: "Data model", icon: <Database className="h-3.5 w-3.5" /> },
  { kind: "uml", label: "UML diagram", icon: <Workflow className="h-3.5 w-3.5" /> },
]);

// The working documents product and engineering people ship — kept as
// tiles for them even where one person's store shows no runs.
const WRITE = inFamily("documents", [
  { kind: "prd", label: "PRD", icon: <ClipboardList className="h-3.5 w-3.5" /> },
  { kind: "prfaq", label: "PR/FAQ", icon: <Megaphone className="h-3.5 w-3.5" /> },
  { kind: "rfc", label: "RFC", icon: <FileCode2 className="h-3.5 w-3.5" /> },
  { kind: "skill", label: "Skill", icon: <Sparkles className="h-3.5 w-3.5" /> },
]);

const GROUPS: [ArtifactGroup, Artifact[]][] = [
  ["understand", UNDERSTAND],
  ["learn", LEARN],
  ["visualize", VISUALIZE],
  ["write", WRITE],
];

/** Every built-in generator, in shelf order, for surfaces beyond Studio such
 *  as the command menu. */
export const ARTIFACTS: Artifact[] = GROUPS.flatMap(([, artifacts]) => artifacts);

/** The four a notebook reaches for first: the most-run generator overall,
 *  the briefing, and the two visual kinds people actually make (process
 *  maps and decks). Audio takes a slot once its voice model is present. */
const PRIMARY_KINDS: NoteKind[] = ["summary", "briefing", "process", "slide_deck"];

export type ArtifactShelf = {
  id: ArtifactGroup;
  label: string;
  artifacts: Artifact[];
};

export function studioArtifacts(kokoroReady: boolean): {
  primary: Artifact[];
  groups: ArtifactShelf[];
} {
  const available = kokoroReady ? [AUDIO_OVERVIEW, ...ARTIFACTS] : ARTIFACTS;
  const primaryKinds = kokoroReady
    ? (["audio_overview", "summary", "briefing", "process"] as NoteKind[])
    : PRIMARY_KINDS;
  const primary = primaryKinds
    .map((kind) => available.find((artifact) => artifact.kind === kind))
    .filter((artifact): artifact is Artifact => !!artifact);
  const primarySet = new Set(primaryKinds);
  return {
    primary,
    groups: GROUPS.map(([id, artifacts]) => ({
      id,
      label: GROUP_LABEL[id],
      artifacts: artifacts.filter((artifact) => !primarySet.has(artifact.kind)),
    })),
  };
}

/**
 * Generator kinds take their label from the Artifact records above, so the
 * badge label always matches the generator button and the default note title.
 */
const GENERATOR_LABELS = Object.fromEntries(
  [AUDIO_OVERVIEW, ...ARTIFACTS].map((artifact) => [artifact.kind, artifact.label]),
) as Record<Exclude<NoteKind, "note" | "report" | "template">, string>;

export const KIND_LABEL: Record<NoteKind, string> = {
  note: "Note",
  report: "Report",
  template: "Template",
  ...GENERATOR_LABELS,
};

/** Row icon for a note kind (NotebookLM-style: the icon says what a note
 *  is, so list rows need no text chip). Artifact tiles' icons where a
 *  generator exists; explicit icons for the kinds that aren't generators. */
export function kindIcon(kind: NoteKind): ReactNode {
  if (kind === AUDIO_OVERVIEW.kind) return AUDIO_OVERVIEW.icon;
  const artifact = ARTIFACTS.find((a) => a.kind === kind);
  if (artifact) return artifact.icon;
  switch (kind) {
    case "report":
      return <Newspaper className="h-3.5 w-3.5" />;
    case "template":
      return <ClipboardList className="h-3.5 w-3.5" />;
    default:
      return <StickyNote className="h-3.5 w-3.5" />;
  }
}

/** The family accent color class for a note kind's icon, or neutral for the
 *  kinds that belong to no family. `kindIcon` only sees a kind, so the
 *  kind->family lookup lives here alongside it. */
export function kindAccent(kind: NoteKind): string {
  if (kind === AUDIO_OVERVIEW.kind) return FAMILY_ACCENT[AUDIO_OVERVIEW.family];
  const artifact = ARTIFACTS.find((a) => a.kind === kind);
  if (artifact) return FAMILY_ACCENT[artifact.family];
  // Template-generated notes (a custom .md generator, e.g. a user story) share
  // the amber the template tiles use — they're a category, not a family.
  if (kind === "template") return "text-artifact-template";
  return "text-muted-foreground";
}
