import { useState, type ReactNode } from "react";
import { useStore } from "@/lib/store";
import { Button, Input, Textarea, Modal, EmptyState, Badge, Spinner } from "./ui";
import { Markdown } from "./Markdown";
import { Reports } from "./Reports";
import { RichEditor } from "./RichEditor";
import { relativeTime } from "@/lib/utils";
import type { Note, NoteKind } from "@/lib/types";
import {
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
  Copy,
  Check,
  ClipboardList,
  Megaphone,
  FileCode2,
  Sparkles,
  RefreshCw,
  FileInput,
  TriangleAlert,
} from "lucide-react";

type Artifact = { kind: NoteKind; label: string; icon: ReactNode };

const SUMMARIES: Artifact[] = [
  { kind: "summary", label: "Summary", icon: <FileText className="h-3.5 w-3.5" /> },
  { kind: "faq", label: "FAQ", icon: <HelpCircle className="h-3.5 w-3.5" /> },
  { kind: "study_guide", label: "Study guide", icon: <GraduationCap className="h-3.5 w-3.5" /> },
  { kind: "briefing", label: "Briefing", icon: <Newspaper className="h-3.5 w-3.5" /> },
  { kind: "timeline", label: "Timeline", icon: <Clock className="h-3.5 w-3.5" /> },
  { kind: "problems", label: "Problems", icon: <TriangleAlert className="h-3.5 w-3.5" /> },
];

const DOCUMENTS: Artifact[] = [
  { kind: "prd", label: "PRD", icon: <ClipboardList className="h-3.5 w-3.5" /> },
  { kind: "prfaq", label: "PR/FAQ", icon: <Megaphone className="h-3.5 w-3.5" /> },
  { kind: "rfc", label: "RFC", icon: <FileCode2 className="h-3.5 w-3.5" /> },
  { kind: "skill", label: "Skill", icon: <Sparkles className="h-3.5 w-3.5" /> },
];

const KIND_LABEL: Record<NoteKind, string> = {
  note: "Note",
  summary: "Summary",
  faq: "FAQ",
  study_guide: "Study guide",
  briefing: "Briefing",
  timeline: "Timeline",
  problems: "Problems",
  prd: "PRD",
  prfaq: "PR/FAQ",
  rfc: "RFC",
  skill: "Skill",
  report: "Report",
};

export function StudioPanel() {
  const currentId = useStore((s) => s.currentId);
  const sources = useStore((s) => s.sources);
  const notes = useStore((s) => s.notes);
  const generatingKind = useStore((s) => s.generatingKind);
  const generate = useStore((s) => s.generateArtifact);
  const createNote = useStore((s) => s.createNote);
  const deleteNote = useStore((s) => s.deleteNote);

  const [viewing, setViewing] = useState<Note | null>(null);
  const [composing, setComposing] = useState(false);
  const [draftTitle, setDraftTitle] = useState("");
  const [draftBody, setDraftBody] = useState("");
  const [instructions, setInstructions] = useState("");
  const [showInstructions, setShowInstructions] = useState(false);

  const hasSources = sources.length > 0;

  return (
    <div className="flex h-full w-[320px] shrink-0 flex-col border-l border-border bg-surface">
      <div className="flex items-center px-4 h-12 border-b border-border">
        <Wand2 className="h-4 w-4 text-muted-foreground" />
        <span className="ml-2 text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">
          Studio
        </span>
      </div>

      <div className="border-b border-border p-3">
        <div className="mb-2 text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
          Generate
        </div>
        <ArtifactGrid
          artifacts={SUMMARIES}
          disabled={!hasSources || !!generatingKind}
          generatingKind={generatingKind}
          onPick={(k) => generate(k, instructions)}
        />

        <div className="mb-2 mt-3 text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
          Documents
        </div>
        <ArtifactGrid
          artifacts={DOCUMENTS}
          disabled={!hasSources || !!generatingKind}
          generatingKind={generatingKind}
          onPick={(k) => generate(k, instructions)}
        />

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

      <div className="flex-1 overflow-y-auto px-2 pb-2">
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
                onClick={() => setViewing(n)}
                className="group cursor-pointer rounded-md border border-border bg-surface-2/40 px-3 py-2.5 transition-colors hover:border-border-strong hover:bg-surface-2"
              >
                <div className="flex items-center gap-2">
                  <span className="truncate text-[13px] font-medium text-foreground">
                    {n.title}
                  </span>
                  <div className="ml-auto flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100">
                    <span onClick={(e) => e.stopPropagation()}>
                      <CopyButton text={n.content} iconOnly />
                    </span>
                    <button
                      className="rounded p-1 text-muted-foreground hover:text-destructive"
                      onClick={(e) => {
                        e.stopPropagation();
                        deleteNote(n.id);
                      }}
                    >
                      <Trash2 className="h-3 w-3" />
                    </button>
                  </div>
                </div>
                <div className="mt-1 flex items-center gap-1.5">
                  {n.kind !== "note" && <Badge>{KIND_LABEL[n.kind]}</Badge>}
                  <span className="text-[11px] text-subtle-foreground">
                    {relativeTime(n.updatedAt)}
                  </span>
                </div>
                <p className="mt-1.5 line-clamp-2 text-[12px] leading-relaxed text-muted-foreground">
                  {n.content.replace(/[#*`>_-]/g, "").slice(0, 160)}
                </p>
              </div>
            ))}
          </div>
        )}
      </div>

      <NoteViewer note={viewing} onClose={() => setViewing(null)} />

      <Modal open={composing} onClose={() => setComposing(false)} title="New note" width="max-w-lg">
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
            <Button type="button" variant="ghost" onClick={() => setComposing(false)}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={!draftBody.trim()}>
              Save note
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}

function ArtifactGrid({
  artifacts,
  disabled,
  generatingKind,
  onPick,
}: {
  artifacts: Artifact[];
  disabled: boolean;
  generatingKind: NoteKind | null;
  onPick: (kind: NoteKind) => void;
}) {
  return (
    <div className="grid grid-cols-2 gap-1.5">
      {artifacts.map((a) => (
        <button
          key={a.kind}
          disabled={disabled}
          onClick={() => onPick(a.kind)}
          className="flex items-center gap-2 rounded-md border border-border bg-surface-2 px-2.5 py-2 text-[12.5px] text-foreground/90 transition-colors hover:border-border-strong hover:bg-elevated disabled:opacity-40 disabled:pointer-events-none"
        >
          <span className="text-muted-foreground">
            {generatingKind === a.kind ? <Spinner className="h-3.5 w-3.5" /> : a.icon}
          </span>
          {a.label}
        </button>
      ))}
    </div>
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
        {copied ? <Check className="h-3 w-3 text-success" /> : <Copy className="h-3 w-3" />}
      </button>
    );
  }
  return (
    <Button variant={variant} onClick={copy}>
      {copied ? <Check className="h-3.5 w-3.5 text-success" /> : <Copy className="h-3.5 w-3.5" />}
      {copied ? "Copied" : label ?? "Copy"}
    </Button>
  );
}

function NoteViewer({ note, onClose }: { note: Note | null; onClose: () => void }) {
  const updateNote = useStore((s) => s.updateNote);
  const rebuildNote = useStore((s) => s.rebuildNote);
  const convertNoteToSource = useStore((s) => s.convertNoteToSource);
  const generatingKind = useStore((s) => s.generatingKind);
  // Track the live note so a rebuild's new content shows without reopening.
  const live = useStore((s) => (note ? s.notes.find((n) => n.id === note.id) ?? note : null));
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
              <Button type="button" variant="ghost" onClick={() => setEditing(false)}>
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
              <Markdown>{live.content}</Markdown>
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
              <CopyButton text={live.content} variant="secondary" label="Copy" />
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
