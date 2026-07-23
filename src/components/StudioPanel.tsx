import { useState, useEffect, type ReactNode } from "react";
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
  CardAction,
  useConfirm,
} from "./ui";
import { Reports } from "./Reports";
import { RichEditor } from "./RichEditor";
import { StreamingBody } from "./StudioNoteViewer";
import {
  cn,
  noteUnread,
  relativeTime,
  shortcutBlocked,
} from "@/lib/utils";
import type { Note } from "@/lib/types";
import {
  FAMILY_ACCENT,
  KIND_LABEL,
  kindAccent,
  kindIcon,
  studioArtifacts,
  type Artifact,
} from "./studioArtifacts";
import {
  FileText,
  Plus,
  Trash2,
  StickyNote,
  Wand2,
  Square,
  PanelRightClose,
  Copy,
  FolderOpen,
  ChevronDown,
  ChevronUp,
} from "lucide-react";

/** Generator families carry a quiet color identity: the icon takes the family
 *  accent (tokens in index.css) and the tile gets a faint matching wash that
 *  warms on hover. Restraint over noise — a whisper of hue for wayfinding, no
 *  filled chips, and never a colored border accent. The neutral border keeps
 *  the resting grid calm; color reads mostly from the icon. */
type Tint = { tile: string; icon: string };
const TINT_BY_FAMILY: Record<Artifact["family"], Tint> = {
  generate: {
    tile: "border-border bg-artifact-generate/5 hover:border-artifact-generate/25 hover:bg-artifact-generate/10",
    icon: FAMILY_ACCENT.generate,
  },
  learning: {
    tile: "border-border bg-artifact-learning/5 hover:border-artifact-learning/25 hover:bg-artifact-learning/10",
    icon: FAMILY_ACCENT.learning,
  },
  documents: {
    tile: "border-border bg-artifact-documents/5 hover:border-artifact-documents/25 hover:bg-artifact-documents/10",
    icon: FAMILY_ACCENT.documents,
  },
};
const TINT_TEMPLATES: Tint = {
  tile: "border-border bg-artifact-template/5 hover:border-artifact-template/25 hover:bg-artifact-template/10",
  icon: "text-artifact-template",
};
const TINT_DISCLOSURE: Tint = {
  tile: "border-border bg-surface-2 hover:border-border-strong hover:bg-elevated",
  icon: "text-muted-foreground",
};

/** Notes the curator archived: out of retrieval, collapsed but never gone.
 *  Opening one still works; editing it revives it (see RFC-note-curator). */
function ArchivedNotes({
  notes,
  onOpen,
}: {
  notes: Note[];
  onOpen: (n: Note) => void;
}) {
  const [open, setOpen] = useState(false);
  if (notes.length === 0) return null;
  return (
    <div className="mt-2 border-t border-border pt-2">
      <button
        className="flex w-full items-center gap-1.5 rounded px-1 py-1 text-[11px] font-medium text-subtle-foreground hover:text-foreground transition-colors"
        onClick={() => setOpen((o) => !o)}
      >
        {open ? (
          <ChevronUp className="h-3 w-3" />
        ) : (
          <ChevronDown className="h-3 w-3" />
        )}
        Archived ({notes.length})
      </button>
      {open && (
        <div className="mt-1 flex flex-col gap-1.5">
          {notes.map((n) => (
            <button
              type="button"
              key={n.id}
              onClick={() => onOpen(n)}
              className="group w-full rounded-md border border-border bg-surface-2/40 px-3 py-2 text-left opacity-50 transition-opacity hover:opacity-80"
            >
              <span className="block truncate text-[12px] font-medium text-foreground">
                {n.title}
              </span>
              <span className="text-[11px] text-subtle-foreground">
                {relativeTime(n.updatedAt)} — editing revives it
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

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
  const readerOpen = useStore((s) => s.reader.open);
  const { confirm, dialog: confirmDialog } = useConfirm();

  // Opening a note is what marks it read — the activity dot means "not
  // opened yet", so it clears here and nowhere else. Notes read in the
  // center-column reader (docs/RFC-document-surface.md).
  const openNoteCard = (n: Note) => {
    markNotesRead([n.id]);
    void api.noteOpened(n.id).catch(() => {});
    useStore.getState().openInReader({ type: "note", id: n.id });
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
  // Keep the common generators visible; the long tail and custom templates
  // share one progressive-disclosure control.
  const [moreOpen, setMoreOpen] = useState(false);
  const { primary: primaryArtifacts, secondary: secondaryArtifacts } =
    studioArtifacts(kokoroReady);
  const moreCount = secondaryArtifacts.length + templates.length;

  const hasSources = sources.length > 0;
  const width = useStore((s) => s.studioWidth);
  const setPanelWidth = useStore((s) => s.setPanelWidth);

  return (
    <div
      style={{ width }}
      className="side-card relative mx-2 mb-2 mt-1 flex shrink-0 flex-col"
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
          aria-label="Collapse studio"
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
          </div>
          <>
              {/* Keep the frequent actions immediately available; everything
                  else, including custom templates, lives behind More. */}
              <div className="mt-2 grid grid-cols-2 gap-1.5">
                {primaryArtifacts.map((a) => (
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
                      category={a.family}
                      tint={TINT_BY_FAMILY[a.family]}
                      disabled={!hasSources || !!generatingKind}
                      onClick={() => generate(a.kind, instructions)}
                    />
                  ))}
                {moreOpen && secondaryArtifacts.map((a) => (
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
                    category={a.family}
                    tint={TINT_BY_FAMILY[a.family]}
                    disabled={!hasSources || !!generatingKind}
                    onClick={() => generate(a.kind, instructions)}
                  />
                ))}
                {moreOpen && templates.map((t) => (
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
                    category="template"
                    tint={TINT_TEMPLATES}
                    disabled={!hasSources || !!generatingKind}
                    onClick={() => generateFromTemplate(t)}
                  />
                ))}
                {moreCount > 0 && (
                  <GenTile
                    icon={
                      moreOpen ? (
                        <ChevronUp className="h-3.5 w-3.5" />
                      ) : (
                        <ChevronDown className="h-3.5 w-3.5" />
                      )
                    }
                    label={moreOpen ? "Less" : `More (${moreCount})`}
                    title={
                      moreOpen
                        ? "Show only common generators"
                        : `Show ${moreCount} more generators and templates`
                    }
                    tint={TINT_DISCLOSURE}
                    disabled={false}
                    onClick={() => setMoreOpen((open) => !open)}
                  />
                )}
              </div>

              {showInstructions ? (
                <Textarea
                  rows={2}
                  autoFocus
                  name="generation-instructions"
                  aria-label="Generation instructions"
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
            aria-label="New note"
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
              {notes.filter((n) => n.status !== "archived").map((n) => (
                <div
                  key={n.id}
                  className={cn(
                    // has-: an open row menu must outrank the z-10 content of
                    // the rows after it (they'd paint over the dropdown
                    // otherwise — later DOM order wins at equal z).
                    // Flat rows, not bordered cards — the title and chips
                    // carry the card; a hover wash marks the target.
                    "group relative cursor-pointer rounded-md px-3 py-2 transition-colors hover:bg-surface-2",
                    n.status === "stale" && "opacity-60",
                  )}
                >
                  <CardAction
                    label={`Open note ${n.title}`}
                    onClick={() => openNoteCard(n)}
                  />
                  <div className="pointer-events-none relative z-10 flex items-center gap-2">
                    <span
                      className={cn(
                        "pointer-events-auto shrink-0 [&_svg]:h-4 [&_svg]:w-4",
                        kindAccent(n.kind),
                      )}
                      title={KIND_LABEL[n.kind]}
                    >
                      {kindIcon(n.kind)}
                    </span>
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
                      className="pointer-events-auto z-20"
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
                  <div className="pointer-events-none relative z-10 mt-1 flex items-center gap-1.5 pl-[22px]">
                    {/* The kind icon on the title row replaces the old text
                        chip. Badges re-enable hit-testing so their explanatory
                        tooltips still show inside the pointer-events-none row. */}
                    {n.origin === "auto" && (
                      <span
                        className="pointer-events-auto"
                        title="Chat saved this on its own — editing it makes it yours"
                      >
                        <Badge>auto</Badge>
                      </span>
                    )}
                    {n.status === "stale" && (
                      <span
                        className="pointer-events-auto"
                        title="Unused for a while — the curator will archive it eventually; opening or editing revives it"
                      >
                        <Badge>stale</Badge>
                      </span>
                    )}
                    <span className="text-[11px] text-subtle-foreground">
                      {relativeTime(n.updatedAt)}
                    </span>
                  </div>
                </div>
              ))}
            </div>
          )}
          <ArchivedNotes
            notes={notes.filter((n) => n.status === "archived")}
            onOpen={openNoteCard}
          />
        </div>
      </div>


      {/* Live preview of the in-flight generation (rebuilds stream inside the
          note viewer instead, so only show this when no note is open). */}
      <Modal
        open={
          !!generatingKind && !readerOpen && !!artifactStreamText && !previewHidden
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
            name="note-title"
            aria-label="Note title"
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
  category,
  tint,
  disabled,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  title?: string;
  category?: Artifact["family"] | "template";
  tint: Tint;
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <button
      disabled={disabled}
      onClick={onClick}
      title={title}
      aria-label={category ? `${label} — ${category}` : label}
      className={cn(
        "flex items-center gap-2 rounded-md border px-2.5 py-2 text-[12px] text-foreground/90 transition-colors disabled:pointer-events-none disabled:opacity-40",
        tint.tile,
      )}
    >
      <span className={tint.icon}>{icon}</span>
      <span className="truncate">{label}</span>
    </button>
  );
}
