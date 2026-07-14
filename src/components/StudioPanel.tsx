import { useState, useEffect, useRef, type ReactNode } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import {
  Button,
  Input,
  Textarea,
  Modal,
  EmptyState,
  Badge,
  ResizeHandle,
  RowMenu,
  Spinner,
  useConfirm,
} from "./ui";
import { Markdown } from "./Markdown";
import { MindMap } from "./MindMap";
import { AudioPlayer, DialogueScript } from "./AudioNote";
import { Reports } from "./Reports";
import { RichEditor } from "./RichEditor";
import {
  cardButtonProps,
  cn,
  noteUnread,
  relativeTime,
  shortcutBlocked,
} from "@/lib/utils";
import type { Note, NoteKind } from "@/lib/types";
import {
  Eye,
  EyeOff,
  FileText,
  HelpCircle,
  GraduationCap,
  Newspaper,
  Clock,
  Plus,
  Trash2,
  Pencil,
  StickyNote,
  Wand2,
  Square,
  PanelRightClose,
  Copy,
  Check,
  ClipboardList,
  Megaphone,
  FileCode2,
  FolderOpen,
  Sparkles,
  RefreshCw,
  FileInput,
  TriangleAlert,
  MessageSquare,
  Lightbulb,
  Quote,
  Table,
  Layers,
  ListChecks,
  Waypoints,
  AppWindow,
  AudioLines,
  ChevronDown,
  ChevronUp,
} from "lucide-react";

type Artifact = { kind: NoteKind; label: string; icon: ReactNode };

/** Tile tint per generator family — the color IS the section label
 *  (NotebookLM-style), replacing the old uppercase headers. */
type Tint = { tile: string; icon: string };
const TINT_GENERATE: Tint = {
  tile: "border-[#5e9bd2]/20 bg-[#5e9bd2]/10 hover:border-[#5e9bd2]/40 hover:bg-[#5e9bd2]/20",
  icon: "text-[#5e9bd2]",
};
const TINT_LEARNING: Tint = {
  tile: "border-[#9b87f5]/20 bg-[#9b87f5]/10 hover:border-[#9b87f5]/40 hover:bg-[#9b87f5]/20",
  icon: "text-[#9b87f5]",
};
const TINT_DOCUMENTS: Tint = {
  tile: "border-[#4cb782]/20 bg-[#4cb782]/10 hover:border-[#4cb782]/40 hover:bg-[#4cb782]/20",
  icon: "text-[#4cb782]",
};
const TINT_TEMPLATES: Tint = {
  tile: "border-[#e8a33d]/20 bg-[#e8a33d]/10 hover:border-[#e8a33d]/40 hover:bg-[#e8a33d]/20",
  icon: "text-[#e8a33d]",
};

/** Shown only once the voice model is downloaded & verified (Settings → Models). */
export const AUDIO_OVERVIEW: Artifact = {
  kind: "audio_overview",
  label: "Audio Overview",
  icon: <AudioLines className="h-3.5 w-3.5" />,
};

const SUMMARIES: Artifact[] = [
  {
    kind: "summary",
    label: "Summary",
    icon: <FileText className="h-3.5 w-3.5" />,
  },
  { kind: "faq", label: "FAQ", icon: <HelpCircle className="h-3.5 w-3.5" /> },
  {
    kind: "study_guide",
    label: "Study guide",
    icon: <GraduationCap className="h-3.5 w-3.5" />,
  },
  {
    kind: "briefing",
    label: "Briefing",
    icon: <Newspaper className="h-3.5 w-3.5" />,
  },
  {
    kind: "timeline",
    label: "Timeline",
    icon: <Clock className="h-3.5 w-3.5" />,
  },
  {
    kind: "insights",
    label: "Insights",
    icon: <Lightbulb className="h-3.5 w-3.5" />,
  },
  {
    kind: "data_table",
    label: "Data table",
    icon: <Table className="h-3.5 w-3.5" />,
  },
  {
    kind: "problems",
    label: "Problems",
    icon: <TriangleAlert className="h-3.5 w-3.5" />,
  },
  {
    kind: "evidence",
    label: "Evidence Log",
    icon: <Quote className="h-3.5 w-3.5" />,
  },
];

const LEARNING: Artifact[] = [
  {
    kind: "flashcards",
    label: "Flashcards",
    icon: <Layers className="h-3.5 w-3.5" />,
  },
  { kind: "quiz", label: "Quiz", icon: <ListChecks className="h-3.5 w-3.5" /> },
  {
    kind: "mind_map",
    label: "Mind map",
    icon: <Waypoints className="h-3.5 w-3.5" />,
  },
];

const DOCUMENTS: Artifact[] = [
  {
    kind: "prd",
    label: "PRD",
    icon: <ClipboardList className="h-3.5 w-3.5" />,
  },
  {
    kind: "prfaq",
    label: "PR/FAQ",
    icon: <Megaphone className="h-3.5 w-3.5" />,
  },
  { kind: "rfc", label: "RFC", icon: <FileCode2 className="h-3.5 w-3.5" /> },
  { kind: "skill", label: "Skill", icon: <Sparkles className="h-3.5 w-3.5" /> },
];

/** Every generator, for surfaces beyond the Studio panel (command menu). */
export const ARTIFACTS: Artifact[] = [...SUMMARIES, ...LEARNING, ...DOCUMENTS];

/**
 * Card preview text: skip a leading markdown heading (or a first line equal to
 * the title) so the card doesn't repeat its own title, then flatten markdown.
 */
function notePreview(n: Note): string {
  const lines = n.content.split("\n");
  let i = 0;
  while (i < lines.length && !lines[i].trim()) i++;
  const first = (lines[i] ?? "").trim();
  const firstText = first
    .replace(/^#+\s*/, "")
    .replace(/[*_`]/g, "")
    .trim();
  if (
    first.startsWith("#") ||
    firstText.toLowerCase() === n.title.trim().toLowerCase()
  )
    i++;
  return lines
    .slice(i)
    .join(" ")
    .replace(/[#*`>_]/g, "")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 160);
}

const KIND_LABEL: Record<NoteKind, string> = {
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

export function StudioPanel() {
  const currentId = useStore((s) => s.currentId);
  const sources = useStore((s) => s.sources);
  const notes = useStore((s) => s.notes);
  const generatingKind = useStore((s) => s.generatingKind);
  const artifactStreamText = useStore((s) => s.artifactStreamText);
  const audioProgress = useStore((s) => s.audioProgress);
  const kokoroReady = useStore((s) => !!s.kokoroStatus?.verified);
  const generate = useStore((s) => s.generateArtifact);
  const templates = useStore((s) => s.templates);
  const generatingTemplateId = useStore((s) => s.generatingTemplateId);
  const generateFromTemplate = useStore((s) => s.generateFromTemplate);
  const cancelGeneration = useStore((s) => s.cancelGeneration);
  const toggleStudio = useStore((s) => s.toggleStudio);
  const createNote = useStore((s) => s.createNote);
  const deleteNote = useStore((s) => s.deleteNote);
  const justCreatedNoteId = useStore((s) => s.justCreatedNoteId);
  const noteReads = useStore((s) => s.noteReads);
  const noteReadsBaseline = useStore((s) => s.noteReadsBaseline);
  const markNotesRead = useStore((s) => s.markNotesRead);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const [viewing, setViewing] = useState<Note | null>(null);
  // Opening a note is what marks it read — the activity dot means "not
  // opened yet", so it clears here and nowhere else.
  const openNoteCard = (n: Note) => {
    markNotesRead([n.id]);
    setViewing(n);
  };
  // The user can hide the live preview without stopping the generation.
  const [previewHidden, setPreviewHidden] = useState(false);
  useEffect(() => setPreviewHidden(false), [generatingKind]);

  // A freshly generated note opens automatically so the result is visible where
  // the user clicked, not just appended to the list below.
  useEffect(() => {
    if (!justCreatedNoteId) return;
    const note = notes.find((n) => n.id === justCreatedNoteId);
    if (note) {
      openNoteCard(note);
      useStore.setState({ justCreatedNoteId: null });
    }
  }, [justCreatedNoteId, notes]);
  const [composing, setComposing] = useState(false);
  const [draftTitle, setDraftTitle] = useState("");
  const [draftBody, setDraftBody] = useState("");

  // Cmd/Ctrl+N: new note. When the panel was collapsed, Workspace opens it
  // and sets pendingNewNote so the composer opens on mount.
  const pendingNewNote = useStore((s) => s.pendingNewNote);
  useEffect(() => {
    if (!pendingNewNote) return;
    useStore.setState({ pendingNewNote: false });
    if (currentId) {
      setDraftTitle("");
      setDraftBody("");
      setComposing(true);
    }
  }, [pendingNewNote, currentId]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (
        (e.metaKey || e.ctrlKey) &&
        e.key === "n" &&
        !shortcutBlocked(e) &&
        currentId
      ) {
        e.preventDefault();
        setDraftTitle("");
        setDraftBody("");
        setComposing(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [currentId]);
  const [instructions, setInstructions] = useState("");
  const [showInstructions, setShowInstructions] = useState(false);
  // Generators hidden/shown — persisted so a notes-heavy workflow keeps its room.
  const [genOpen, setGenOpen] = useState(
    localStorage.getItem("studioGenOpen") !== "false",
  );
  const toggleGenOpen = () => {
    const v = !genOpen;
    localStorage.setItem("studioGenOpen", String(v));
    setGenOpen(v);
  };

  // Templates beyond the first few collapse behind a More tile so the
  // generator grid stays short; expansion is per-session.
  const [templatesExpanded, setTemplatesExpanded] = useState(false);
  const TEMPLATES_PREVIEW = 3;
  const collapsible = templates.length > TEMPLATES_PREVIEW + 1;
  const visibleTemplates =
    collapsible && !templatesExpanded
      ? templates.slice(0, TEMPLATES_PREVIEW)
      : templates;
  const hiddenTemplateCount = collapsible
    ? templates.length - TEMPLATES_PREVIEW
    : 0;

  const hasSources = sources.length > 0;
  const width = useStore((s) => s.studioWidth);
  const setPanelWidth = useStore((s) => s.setPanelWidth);

  return (
    <div
      style={{ width }}
      className="relative flex h-full shrink-0 flex-col border-l border-border bg-surface"
    >
      <ResizeHandle
        edge="left"
        width={width}
        defaultWidth={320}
        onResize={(w) => setPanelWidth("studio", w)}
        label="Resize studio panel"
      />
      <div className="flex items-center px-4 h-12 border-b border-border">
        <Wand2 className="h-4 w-4 text-muted-foreground" />
        <span className="ml-2 text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">
          Studio
        </span>
        <Button
          variant="ghost"
          size="icon"
          className="ml-auto"
          onClick={toggleStudio}
          title="Collapse studio"
        >
          <PanelRightClose className="h-4 w-4" />
        </Button>
      </div>

      {/* Everything below the header scrolls as one column, so a tall
          generator section never pins the notes list below the fold. */}
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
        <div className="p-3">
          <div className="flex items-center gap-2 text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
            <span>Generate</span>
            {generatingKind && (
              <button
                onClick={() => cancelGeneration("artifact")}
                className="flex items-center gap-1 rounded px-1.5 py-0.5 text-destructive hover:bg-destructive/10"
                title="Stop generating"
              >
                <Square className="h-3 w-3" />
                Stop
              </button>
            )}
            {audioProgress && (
              <span className="text-[10px] normal-case tabular-nums text-subtle-foreground">
                voicing {audioProgress.done}/{audioProgress.total}
              </span>
            )}
            <button
              onClick={() => void api.openTemplatesFolder()}
              className="ml-auto rounded p-0.5 transition-colors hover:text-foreground"
              title="Open the templates folder — each .md file is a generator"
              aria-label="Open templates folder"
            >
              <FolderOpen className="h-3.5 w-3.5" />
            </button>
            <button
              onClick={toggleGenOpen}
              className="rounded p-0.5 transition-colors hover:text-foreground"
              title={genOpen ? "Hide generators" : "Show generators"}
              aria-label={genOpen ? "Hide generators" : "Show generators"}
              aria-expanded={genOpen}
            >
              {genOpen ? (
                <Eye className="h-3.5 w-3.5" />
              ) : (
                <EyeOff className="h-3.5 w-3.5" />
              )}
            </button>
          </div>
          {genOpen && (
            <>
              {/* One continuous grid — generator families flow into each other
                and are told apart by tile tint, not headers: blue = generate,
                violet = learning, green = documents, amber = templates.
                Templates beyond the first few live behind the More tile. */}
              <div className="mt-2 grid grid-cols-2 gap-1.5">
                {(kokoroReady ? [AUDIO_OVERVIEW, ...SUMMARIES] : SUMMARIES).map(
                  (a) => (
                    <GenTile
                      key={a.kind}
                      icon={
                        generatingKind === a.kind ? (
                          <Spinner className="h-3.5 w-3.5" />
                        ) : (
                          a.icon
                        )
                      }
                      label={a.label}
                      tint={TINT_GENERATE}
                      disabled={!hasSources || !!generatingKind}
                      onClick={() => generate(a.kind, instructions)}
                    />
                  ),
                )}
                {LEARNING.map((a) => (
                  <GenTile
                    key={a.kind}
                    icon={
                      generatingKind === a.kind ? (
                        <Spinner className="h-3.5 w-3.5" />
                      ) : (
                        a.icon
                      )
                    }
                    label={a.label}
                    tint={TINT_LEARNING}
                    disabled={!hasSources || !!generatingKind}
                    onClick={() => generate(a.kind, instructions)}
                  />
                ))}
                {DOCUMENTS.map((a) => (
                  <GenTile
                    key={a.kind}
                    icon={
                      generatingKind === a.kind ? (
                        <Spinner className="h-3.5 w-3.5" />
                      ) : (
                        a.icon
                      )
                    }
                    label={a.label}
                    tint={TINT_DOCUMENTS}
                    disabled={!hasSources || !!generatingKind}
                    onClick={() => generate(a.kind, instructions)}
                  />
                ))}
                {visibleTemplates.map((t) => (
                  <GenTile
                    key={t.id}
                    icon={
                      generatingTemplateId === t.id ? (
                        <Spinner className="h-3.5 w-3.5" />
                      ) : (
                        <FileText className="h-3.5 w-3.5" />
                      )
                    }
                    label={t.name}
                    title={t.description || t.name}
                    tint={TINT_TEMPLATES}
                    disabled={!hasSources || !!generatingKind}
                    onClick={() => generateFromTemplate(t)}
                  />
                ))}
                {hiddenTemplateCount > 0 && (
                  <GenTile
                    icon={
                      templatesExpanded ? (
                        <ChevronUp className="h-3.5 w-3.5" />
                      ) : (
                        <ChevronDown className="h-3.5 w-3.5" />
                      )
                    }
                    label={
                      templatesExpanded
                        ? "Less"
                        : `More (${hiddenTemplateCount})`
                    }
                    title={
                      templatesExpanded
                        ? "Show fewer templates"
                        : `Show ${hiddenTemplateCount} more templates`
                    }
                    tint={TINT_TEMPLATES}
                    disabled={false}
                    onClick={() => setTemplatesExpanded((v) => !v)}
                  />
                )}
              </div>

              {showInstructions ? (
                <Textarea
                  rows={2}
                  autoFocus
                  value={instructions}
                  onChange={(e) => setInstructions(e.target.value)}
                  placeholder="Optional instructions applied to the next generation…"
                  className="mt-2.5 text-[12px]"
                />
              ) : (
                <button
                  onClick={() => setShowInstructions(true)}
                  disabled={!hasSources}
                  className="mt-2.5 text-[11px] text-muted-foreground transition-colors hover:text-foreground disabled:opacity-40"
                >
                  + Add instructions
                </button>
              )}
              {!hasSources && (
                <p className="mt-2 text-[11px] text-subtle-foreground">
                  Add sources to generate documents.
                </p>
              )}
            </>
          )}
        </div>

        {currentId && <Reports />}

        <div className="flex items-center justify-between px-4 pt-3 pb-1">
          <span className="text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
            Notes
          </span>
          <Button
            variant="ghost"
            size="icon"
            disabled={!currentId}
            onClick={() => {
              setDraftTitle("");
              setDraftBody("");
              setComposing(true);
            }}
            title="New note"
          >
            <Plus className="h-4 w-4" />
          </Button>
        </div>

        <div className="px-2 pb-2">
          {notes.length === 0 ? (
            <EmptyState
              icon={<StickyNote className="h-6 w-6" />}
              title="No notes yet"
              hint="Generate a document above or write your own note."
            />
          ) : (
            <div className="flex flex-col gap-1.5">
              {notes.map((n) => (
                <div
                  key={n.id}
                  onClick={() => openNoteCard(n)}
                  {...cardButtonProps(() => openNoteCard(n))}
                  className="group cursor-pointer rounded-md border border-border bg-surface-2/40 px-3 py-2.5 transition-colors hover:border-border-strong hover:bg-surface-2"
                >
                  <div className="flex items-center gap-2">
                    <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-foreground">
                      {n.title}
                    </span>
                    {noteUnread(n, noteReads, noteReadsBaseline) && (
                      <span
                        className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                        title="Not opened yet"
                        aria-label="Unread"
                      />
                    )}
                    <RowMenu
                      label={`Options for "${n.title}"`}
                      items={[
                        {
                          label: "Copy text",
                          icon: <Copy className="h-3.5 w-3.5" />,
                          onClick: () => {
                            void navigator.clipboard.writeText(n.content);
                            useStore
                              .getState()
                              .pushToast("success", "Note copied");
                          },
                        },
                        {
                          label: "Delete",
                          icon: <Trash2 className="h-3.5 w-3.5" />,
                          danger: true,
                          onClick: async () => {
                            if (
                              await confirm({
                                title: `Delete "${n.title}"?`,
                                message:
                                  "This note will be permanently removed.",
                                confirmLabel: "Delete",
                                danger: true,
                              })
                            )
                              deleteNote(n.id);
                          },
                        },
                      ]}
                    />
                  </div>
                  <div className="mt-1 flex items-center gap-1.5">
                    {n.kind !== "note" &&
                      n.title.trim().toLowerCase() !==
                        KIND_LABEL[n.kind].toLowerCase() && (
                        <Badge>{KIND_LABEL[n.kind]}</Badge>
                      )}
                    <span className="text-[11px] text-subtle-foreground">
                      {relativeTime(n.updatedAt)}
                    </span>
                  </div>
                  <p className="mt-1.5 line-clamp-2 text-[12px] leading-relaxed text-muted-foreground">
                    {notePreview(n)}
                  </p>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      <NoteViewer note={viewing} onClose={() => setViewing(null)} />

      {/* Live preview of the in-flight generation (rebuilds stream inside the
          note viewer instead, so only show this when no note is open). */}
      <Modal
        open={
          !!generatingKind && !viewing && !!artifactStreamText && !previewHidden
        }
        onClose={() => setPreviewHidden(true)}
        title={
          generatingKind ? `Generating ${KIND_LABEL[generatingKind]}…` : ""
        }
        width="max-w-2xl"
        footer={
          <div className="flex items-center justify-between">
            <span className="flex items-center gap-2 text-[12px] tabular-nums text-muted-foreground">
              <Spinner className="h-3.5 w-3.5" />
              {audioProgress
                ? `Voicing the episode — line ${audioProgress.done} of ${audioProgress.total}`
                : "Streaming — closing this keeps generating"}
            </span>
            <Button
              variant="danger"
              onClick={() => cancelGeneration("artifact")}
            >
              <Square className="h-3.5 w-3.5" />
              Stop
            </Button>
          </div>
        }
      >
        <StreamingBody text={artifactStreamText} />
      </Modal>

      <Modal
        open={composing}
        onClose={() => setComposing(false)}
        title="New note"
        width="max-w-lg"
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            createNote(draftTitle, draftBody);
            setComposing(false);
          }}
          className="flex flex-col gap-3"
        >
          <Input
            autoFocus
            placeholder="Title"
            value={draftTitle}
            onChange={(e) => setDraftTitle(e.target.value)}
          />
          <RichEditor value={draftBody} onChange={setDraftBody} />
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setComposing(false)}
            >
              Cancel
            </Button>
            <Button
              type="submit"
              variant="primary"
              disabled={!draftBody.trim()}
            >
              Save note
            </Button>
          </div>
        </form>
      </Modal>

      {confirmDialog}
    </div>
  );
}

/** One generator tile in the flowing Studio grid. */
function GenTile({
  icon,
  label,
  title,
  tint,
  disabled,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  title?: string;
  tint: Tint;
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <button
      disabled={disabled}
      onClick={onClick}
      title={title}
      className={cn(
        "flex items-center gap-2 rounded-md border px-2.5 py-2 text-[12px] text-foreground/90 transition-colors disabled:opacity-40 disabled:pointer-events-none",
        tint.tile,
      )}
    >
      <span className={tint.icon}>{icon}</span>
      <span className="truncate">{label}</span>
    </button>
  );
}

function CopyButton({
  text,
  label,
  iconOnly,
  variant = "ghost",
}: {
  text: string;
  label?: string;
  iconOnly?: boolean;
  variant?: "ghost" | "secondary";
}) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }
  if (iconOnly) {
    return (
      <button
        className="rounded p-1 text-muted-foreground hover:text-foreground"
        onClick={copy}
        title="Copy to clipboard"
      >
        {copied ? (
          <Check className="h-3 w-3 text-success" />
        ) : (
          <Copy className="h-3 w-3" />
        )}
      </button>
    );
  }
  return (
    <Button variant={variant} onClick={copy}>
      {copied ? (
        <Check className="h-3.5 w-3.5 text-success" />
      ) : (
        <Copy className="h-3.5 w-3.5" />
      )}
      {copied ? "Copied" : (label ?? "Copy")}
    </Button>
  );
}

/** Markdown that follows its own tail while tokens stream in. */
function StreamingBody({ text }: { text: string }) {
  const endRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [text]);
  return (
    <div>
      <Markdown>{text}</Markdown>
      <div ref={endRef} />
    </div>
  );
}

function NoteViewer({
  note,
  onClose,
}: {
  note: Note | null;
  onClose: () => void;
}) {
  const updateNote = useStore((s) => s.updateNote);
  const rebuildNote = useStore((s) => s.rebuildNote);
  const convertNoteToSource = useStore((s) => s.convertNoteToSource);
  const discussNoteInChat = useStore((s) => s.discussNoteInChat);
  const generatingKind = useStore((s) => s.generatingKind);
  const artifactStreamText = useStore((s) => s.artifactStreamText);
  // Track the live note so a rebuild's new content shows without reopening.
  const live = useStore((s) =>
    note ? (s.notes.find((n) => n.id === note.id) ?? note) : null,
  );
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");

  function startEdit() {
    if (!live) return;
    setTitle(live.title);
    setBody(live.content);
    setEditing(true);
  }

  const rebuilding = !!generatingKind && !!live && live.kind !== "note";

  return (
    <Modal
      open={!!note}
      onClose={() => {
        setEditing(false);
        onClose();
      }}
      title={live?.title ?? ""}
      width="max-w-2xl"
      headerActions={
        live && (
          <Button
            variant="ghost"
            size="icon"
            onClick={() => {
              void api.newWindow(live.notebookId, live.id);
              onClose();
            }}
            title="Open in its own window"
            aria-label="Open this note in its own window"
          >
            <AppWindow className="h-4 w-4" />
          </Button>
        )
      }
    >
      {live &&
        (editing ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              updateNote(live.id, title, body);
              setEditing(false);
            }}
            className="flex flex-col gap-3"
            key={live.id}
          >
            <Input value={title} onChange={(e) => setTitle(e.target.value)} />
            <RichEditor value={body} onChange={setBody} />
            <div className="flex justify-end gap-2">
              <Button
                type="button"
                variant="ghost"
                onClick={() => setEditing(false)}
              >
                Cancel
              </Button>
              <Button type="submit" variant="primary">
                Save
              </Button>
            </div>
          </form>
        ) : (
          <div className="flex flex-col gap-3">
            <div className="max-h-[60vh] overflow-y-auto pr-1">
              {rebuilding && artifactStreamText ? (
                <StreamingBody text={artifactStreamText} />
              ) : live.kind === "mind_map" ? (
                <MindMap content={live.content} />
              ) : live.kind === "audio_overview" ? (
                <div className="flex flex-col gap-4">
                  {/* Key by updatedAt so a rebuild swaps in the new episode. */}
                  <AudioPlayer
                    noteId={live.id}
                    title={live.title}
                    key={live.updatedAt}
                  />
                  <DialogueScript content={live.content} />
                </div>
              ) : (
                <Markdown>{live.content}</Markdown>
              )}
            </div>
            <div className="flex justify-end gap-2 border-t border-border pt-3">
              {live.kind !== "note" && (
                <Button
                  variant="secondary"
                  onClick={() => rebuildNote(live)}
                  disabled={rebuilding}
                  title="Regenerate with the latest sources"
                >
                  {rebuilding ? (
                    <Spinner className="h-3.5 w-3.5" />
                  ) : (
                    <RefreshCw className="h-3.5 w-3.5" />
                  )}
                  Rebuild
                </Button>
              )}
              <CopyButton
                text={live.content}
                variant="secondary"
                label="Copy"
              />
              <Button
                variant="secondary"
                onClick={() => {
                  discussNoteInChat(live.id);
                  onClose();
                }}
                title="Add this note to the chat so you can discuss it"
              >
                <MessageSquare className="h-3.5 w-3.5" />
                Discuss in chat
              </Button>
              <Button
                variant="secondary"
                onClick={() => {
                  convertNoteToSource(live.id);
                  onClose();
                }}
                title="Turn this note into a source (embedded & searchable)"
              >
                <FileInput className="h-3.5 w-3.5" />
                Convert to source
              </Button>
              <Button variant="secondary" onClick={startEdit}>
                <Pencil className="h-3.5 w-3.5" />
                Edit
              </Button>
            </div>
          </div>
        ))}
    </Modal>
  );
}
