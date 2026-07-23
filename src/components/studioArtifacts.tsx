import type { ReactNode } from "react";
import type { NoteKind } from "@/lib/types";
import {
  AudioLines,
  ClipboardList,
  Clock,
  FileCode2,
  FileText,
  GraduationCap,
  HelpCircle,
  Layers,
  Lightbulb,
  ListChecks,
  Megaphone,
  Newspaper,
  Presentation,
  Quote,
  Sparkles,
  StickyNote,
  Table,
  TriangleAlert,
  Waypoints,
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

const SUMMARIES = inFamily("generate", [
  { kind: "summary", label: "Summary", icon: <FileText className="h-3.5 w-3.5" /> },
  { kind: "faq", label: "FAQ", icon: <HelpCircle className="h-3.5 w-3.5" /> },
  {
    kind: "study_guide",
    label: "Study guide",
    icon: <GraduationCap className="h-3.5 w-3.5" />,
  },
  { kind: "briefing", label: "Briefing", icon: <Newspaper className="h-3.5 w-3.5" /> },
  { kind: "timeline", label: "Timeline", icon: <Clock className="h-3.5 w-3.5" /> },
  { kind: "insights", label: "Insights", icon: <Lightbulb className="h-3.5 w-3.5" /> },
  { kind: "data_table", label: "Data table", icon: <Table className="h-3.5 w-3.5" /> },
  { kind: "problems", label: "Problems", icon: <TriangleAlert className="h-3.5 w-3.5" /> },
  { kind: "evidence", label: "Evidence Log", icon: <Quote className="h-3.5 w-3.5" /> },
]);

const LEARNING = inFamily("learning", [
  { kind: "flashcards", label: "Flashcards", icon: <Layers className="h-3.5 w-3.5" /> },
  { kind: "quiz", label: "Quiz", icon: <ListChecks className="h-3.5 w-3.5" /> },
  { kind: "mind_map", label: "Mind map", icon: <Waypoints className="h-3.5 w-3.5" /> },
  {
    kind: "slide_deck",
    label: "Slide deck",
    icon: <Presentation className="h-3.5 w-3.5" />,
  },
]);

const DOCUMENTS = inFamily("documents", [
  { kind: "prd", label: "PRD", icon: <ClipboardList className="h-3.5 w-3.5" /> },
  { kind: "prfaq", label: "PR/FAQ", icon: <Megaphone className="h-3.5 w-3.5" /> },
  { kind: "rfc", label: "RFC", icon: <FileCode2 className="h-3.5 w-3.5" /> },
  { kind: "skill", label: "Skill", icon: <Sparkles className="h-3.5 w-3.5" /> },
]);

/** Every built-in generator, for surfaces beyond Studio such as the command menu. */
export const ARTIFACTS: Artifact[] = [...SUMMARIES, ...LEARNING, ...DOCUMENTS];

const PRIMARY_KINDS: NoteKind[] = ["summary", "study_guide", "briefing", "faq"];

export function studioArtifacts(kokoroReady: boolean): {
  primary: Artifact[];
  secondary: Artifact[];
} {
  const available = kokoroReady ? [AUDIO_OVERVIEW, ...ARTIFACTS] : ARTIFACTS;
  const primaryKinds = kokoroReady
    ? (["audio_overview", "summary", "study_guide", "briefing"] as NoteKind[])
    : PRIMARY_KINDS;
  const primary = primaryKinds
    .map((kind) => available.find((artifact) => artifact.kind === kind))
    .filter((artifact): artifact is Artifact => !!artifact);
  const primarySet = new Set(primaryKinds);
  return {
    primary,
    secondary: available.filter((artifact) => !primarySet.has(artifact.kind)),
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
