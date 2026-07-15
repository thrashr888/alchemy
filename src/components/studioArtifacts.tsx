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
  Quote,
  Sparkles,
  Table,
  TriangleAlert,
  Waypoints,
} from "lucide-react";

export type Artifact = { kind: NoteKind; label: string; icon: ReactNode };

/** Shown only once the voice model is downloaded and verified. */
export const AUDIO_OVERVIEW: Artifact = {
  kind: "audio_overview",
  label: "Audio Overview",
  icon: <AudioLines className="h-3.5 w-3.5" />,
};

const SUMMARIES: Artifact[] = [
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
];

const LEARNING: Artifact[] = [
  { kind: "flashcards", label: "Flashcards", icon: <Layers className="h-3.5 w-3.5" /> },
  { kind: "quiz", label: "Quiz", icon: <ListChecks className="h-3.5 w-3.5" /> },
  { kind: "mind_map", label: "Mind map", icon: <Waypoints className="h-3.5 w-3.5" /> },
];

const DOCUMENTS: Artifact[] = [
  { kind: "prd", label: "PRD", icon: <ClipboardList className="h-3.5 w-3.5" /> },
  { kind: "prfaq", label: "PR/FAQ", icon: <Megaphone className="h-3.5 w-3.5" /> },
  { kind: "rfc", label: "RFC", icon: <FileCode2 className="h-3.5 w-3.5" /> },
  { kind: "skill", label: "Skill", icon: <Sparkles className="h-3.5 w-3.5" /> },
];

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

export const KIND_LABEL: Record<NoteKind, string> = {
  note: "Note",
  audio_overview: "Audio Overview",
  summary: "Summary",
  faq: "FAQ",
  study_guide: "Study guide",
  briefing: "Briefing",
  timeline: "Timeline",
  insights: "Insights",
  flashcards: "Flashcards",
  quiz: "Quiz",
  mind_map: "Mind map",
  data_table: "Data table",
  problems: "Problems",
  evidence: "Evidence",
  prd: "PRD",
  prfaq: "PR/FAQ",
  rfc: "RFC",
  skill: "Skill",
  report: "Report",
  template: "Template",
};
