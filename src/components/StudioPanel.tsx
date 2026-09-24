import { useState, useEffect, useMemo, useRef, type ReactNode } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { dragFormat, noteDragProps } from "@/lib/dragOut";
import {
  Badge,
  Button,
  CardAction,
  EmptyState,
  Input,
  LoadingState,
  Modal,
  ResizeHandle,
  RowMenu,
  SearchField,
  Segmented,
  SortMenu,
  Spinner,
  type RowMenuItem,
  useHoverCard,
  useMarquee,
} from "./ui";
import { Reports } from "./Reports";
import { exportNote, exportTargets } from "@/lib/noteExport";
import { DeletionProposalMark } from "./OkfBadges";
import { LazyRichEditor } from "./LazyRichEditor";
import { StreamingBody } from "./StudioNoteViewer";
import {
  cn,
  noteUnread,
  relativeTime,
  shortcutBlocked,
} from "@/lib/utils";
import type { NoteSummary as Note } from "@/lib/types";
import {
  FAMILY_ACCENT,
  KIND_LABEL,
  kindAccent,
  kindIcon,
  studioArtifacts,
  type Artifact,
} from "./studioArtifacts";
import {
  FileDown,
  FileText,
  Plus,
  Trash2,
  StickyNote,
  Square,
  PanelRightClose,
  Copy,
  ShieldCheck,
  FolderOpen,
  ChevronDown,
  ChevronUp, RotateCw } from "lucide-react";

/** How the notes list is ordered. Recency is the default because the list is
 *  mostly a record of what was just made; the other two are for finding a
 *  note you already know about. */
type NoteSort = "recent" | "title" | "type";
const NOTE_SORTS: { value: NoteSort; label: string }[] = [
  { value: "recent", label: "Recent" },
  { value: "title", label: "Title" },
  { value: "type", label: "Type" },
];

function readNoteSort(): NoteSort {
  try {
    const v = localStorage.getItem("studioNoteSort");
    if (NOTE_SORTS.some((s) => s.value === v)) return v as NoteSort;
  } catch {
    // Quota or private-mode noise; recency is the default anyway.
  }
  return "recent";
}

/** Ties break on the title, so equal dates or one type's whole run still
 *  read down alphabetically instead of reshuffling on each render. */
function sortNotes(notes: Note[], sort: NoteSort): Note[] {
  const byTitle = (a: Note, b: Note) => a.title.localeCompare(b.title);
  return [...notes].sort((a, b) => {
    if (sort === "title") return byTitle(a, b);
    if (sort === "type") {
      const label = (n: Note) => KIND_LABEL[n.kind] ?? n.kind;
      return label(a).localeCompare(label(b)) || byTitle(a, b);
    }
    return b.updatedAt - a.updatedAt || byTitle(a, b);
  });
}

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

/** Wiki pages are numerous by design — one per registry entity plus the
 *  "Notebook index" that links them — and would drown the handful of
 *  notes a person actually wrote, so the whole wiki lives behind this
 *  fold, index first. */
const WIKI_INDEX_TITLE = "Notebook index";
const isEntityPage = (n: Note) => n.title.startsWith("Entity: ");
const isWikiPage = (n: Note) =>
  isEntityPage(n) || n.title === WIKI_INDEX_TITLE;

function WikiNotes({
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
        className="flex w-full items-center gap-1.5 rounded px-1 py-1 text-micro font-medium text-subtle-foreground hover:text-foreground transition-colors"
        onClick={() => setOpen((o) => !o)}
      >
        {open ? (
          <ChevronUp className="h-3 w-3" />
        ) : (
          <ChevronDown className="h-3 w-3" />
        )}
        Wiki pages ({notes.length})
      </button>
      {open && (
        <div className="mt-1 flex flex-col gap-1.5">
          {[...notes.filter((n) => !isEntityPage(n)), ...notes.filter(isEntityPage)].map((n) => (
            <button
              type="button"
              key={n.id}
              onClick={() => onOpen(n)}
              className="group w-full rounded-md border border-border bg-surface-2/40 px-3 py-2 text-left transition-colors hover:border-border-strong"
            >
              <span
                className="block truncate text-caption font-medium text-foreground"
                title={n.title.replace(/^Entity: /, "")}
              >
                {n.title.replace(/^Entity: /, "")}
              </span>
              <span className="text-micro text-subtle-foreground">
                {relativeTime(n.updatedAt)}
                {isEntityPage(n)
                  ? " · linked from the notebook index"
                  : " · the wiki's front door"}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

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
        className="flex w-full items-center gap-1.5 rounded px-1 py-1 text-micro font-medium text-subtle-foreground hover:text-foreground transition-colors"
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
              <span
                className="block truncate text-caption font-medium text-foreground"
                title={n.title}
              >
                {n.title}
              </span>
              <span className="text-micro text-subtle-foreground">
                {relativeTime(n.updatedAt)} · editing keeps it
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
  const notebookLoading = useStore((s) => s.notebookLoading);
  // A generation keeps running when the user navigates elsewhere; its live
  // preview belongs to ONE notebook, so other notebooks paint no stream.
  const generatingKind = useStore((s) => s.generatingKind);
  const genProgress = useStore((s) => s.genProgress);
  const genStatus = useStore((s) => s.genStatus);
  // Deletions the other person made in a shared folder, unanswered here
  // (docs/RFC-shared-notebook.md §3).
  const proposals = useStore((s) => s.deletionProposals);
  const generatingHere = useStore(
    (s) => s.generatingFor === null || s.generatingFor === s.currentId,
  );
  const artifactStreamText = useStore((s) =>
    s.generatingFor === s.currentId ? s.artifactStreamText : "",
  );
  const audioProgress = useStore((s) => s.audioProgress);
  const kokoroReady = useStore((s) => !!s.kokoroStatus?.verified);
  const generate = useStore((s) => s.generateArtifact);
  const templates = useStore((s) => s.templates);
  const generateFromTemplate = useStore((s) => s.generateFromTemplate);
  const cancelGeneration = useStore((s) => s.cancelGeneration);
  // A failed generation keeps its row (the prompt is the record of what was
  // asked); Retry deletes that attempt and asks again with the same prompt.
  // Shared by the row's inline verb and its menu, so the two never drift.
  const retryNote = (n: Note) => {
    void (async () => {
      const full = await api.readNote(n.id);
      await deleteNote(n.id);
      await useStore.getState().generateArtifact(n.kind, full.prompt);
    })().catch((error) => useStore.getState().pushToast("error", String(error)));
  };
  const toggleStudio = useStore((s) => s.toggleStudio);
  const createNote = useStore((s) => s.createNote);
  const deleteNote = useStore((s) => s.deleteNote);
  const justCreatedNoteId = useStore((s) => s.justCreatedNoteId);
  const noteReads = useStore((s) => s.noteReads);
  const noteReadsBaseline = useStore((s) => s.noteReadsBaseline);
  const markNotesRead = useStore((s) => s.markNotesRead);
  const readerOpen = useStore((s) => s.reader.open);
  // The note the reader is showing: bolder title, quiet wash (see the
  // Sources panel for the same treatment).
  const openNoteId = useStore((s) => {
    const doc = s.reader.open ? s.reader.history[s.reader.index] : undefined;
    return doc?.type === "note" ? doc.id : null;
  });
  const picked = useStore((s) => s.picked);
  const pickOne = useStore((s) => s.pickOne);
  const pickToggle = useStore((s) => s.pickToggle);
  const pickRange = useStore((s) => s.pickRange);
  const pickSet = useStore((s) => s.pickSet);
  const clearPicked = useStore((s) => s.clearPicked);
  const deleteNotesBatch = useStore((s) => s.deleteNotesBatch);

  // ---- Finder-style selection over the notes list (RFC-multi-select) ----
  const pickedNoteIds = useMemo(
    () => new Set(picked?.kind === "notes" ? picked.ids : []),
    [picked],
  );
  // One ordered list feeds both the rows and the selection, so a shift-click
  // range runs down the notes as they are drawn.
  const [noteSort, setNoteSort] = useState<NoteSort>(readNoteSort);
  const changeNoteSort = (v: NoteSort) => {
    setNoteSort(v);
    try {
      localStorage.setItem("studioNoteSort", v);
    } catch {
      // The order holds for this session either way.
    }
  };
  // Notebooks with dozens of notes need a way in: a title filter (the list
  // rows are summaries without content),
  // the same affordance the sources panel has. Session-local on purpose.
  const [noteQuery, setNoteQuery] = useState("");
  const shownNotes = useMemo(
    () => notes.filter((n) => n.status !== "archived" && !isWikiPage(n)),
    [notes],
  );
  // Which generators are already cooking. The queue's pending note carries
  // the kind it was asked for, so the notes themselves say what is busy —
  // `generatingKind` only ever covered the rebuild and report paths, which
  // is why a queued generation lit nothing and the tile invited a second
  // press. Queued counts as busy: the work is accepted, it just hasn't
  // reached the engine yet. Typed as strings rather than NoteKind because a
  // template's note carries the literal "template:<id>" the backend
  // resolves at run time, which the union does not spell.
  const busyKinds = useMemo(
    () =>
      new Set<string>(
        notes.filter((n) => n.status === "generating").map((n) => n.kind),
      ),
    [notes],
  );
  const listedNotes = useMemo(() => {
    const q = noteQuery.trim().toLowerCase();
    const matched = q
      ? shownNotes.filter((n) => n.title.toLowerCase().includes(q))
      : shownNotes;
    return sortNotes(matched, noteSort);
  }, [shownNotes, noteQuery, noteSort]);
  const visibleNoteIds = listedNotes.map((n) => n.id);
  const visibleNoteIdsRef = useRef(visibleNoteIds);
  visibleNoteIdsRef.current = visibleNoteIds;

  const notesListRef = useRef<HTMLDivElement>(null);
  // Additive drags union against the pre-drag selection (see SourcesPanel).
  const marqueeBase = useRef<string[]>([]);
  const { onPointerDown: marqueeDown, marquee, justEnded } = useMarquee({
    containerRef: notesListRef,
    onStart: (additive) => {
      const p = useStore.getState().picked;
      marqueeBase.current = additive && p?.kind === "notes" ? p.ids : [];
    },
    onSelect: (ids) =>
      pickSet("notes", [...new Set([...marqueeBase.current, ...ids])], false),
    onClearBackground: clearPicked,
  });

  function noteBatchItems(ids: string[]): RowMenuItem[] {
    const n = ids.length;
    return [
      {
        label: `Copy ${n} Notes`,
        icon: <Copy className="h-3.5 w-3.5" />,
        onClick: () => {
          void copyNotes(ids);

        },
      },
      { label: "", separator: true, onClick: () => {} },
      {
        label: `Delete ${n} Notes…`,
        symbol: "trash",
        icon: <Trash2 className="h-3.5 w-3.5" />,
        danger: true,
        onClick: () => void confirmDeleteNotes(ids),
      },
    ];
  }

  async function confirmDeleteNotes(ids: string[]) {
    // Undo beats confirm: the delete toast restores notes with their kind
    // intact, so there is nothing left to warn about.
    await deleteNotesBatch(ids);
  }
  const confirmDeleteNotesRef = useRef(confirmDeleteNotes);
  confirmDeleteNotesRef.current = confirmDeleteNotes;

  // ⌘A / Delete apply to notes only while a notes selection is active —
  // the sources panel owns the default (see SourcesPanel's handler).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (shortcutBlocked(e)) return;
      // An open row menu owns the keyboard (see SourcesPanel for why this
      // rides the capture phase and its stopPropagation can't reach us).
      if (document.querySelector('[role="menu"]')) return;
      const p = useStore.getState().picked;
      if (p?.kind !== "notes") return;
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a") {
        e.preventDefault();
        useStore.getState().pickAll("notes", visibleNoteIdsRef.current);
      } else if (
        (e.key === "Backspace" || e.key === "Delete") &&
        p.ids.length > 0
      ) {
        e.preventDefault();
        void confirmDeleteNotesRef.current(p.ids);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  // Opening a note is what marks it read — the activity dot means "not
  // opened yet", so it clears here and nowhere else. Notes read in the
  // center-column reader (docs/RFC-document-surface.md).
  const openNoteCard = (n: Note) => {
    markNotesRead([n.id]);
    void api.noteOpened(n.id).catch(() => {});
    useStore.getState().openInReader({ type: "note", id: n.id });
  };
  const { show: showCard, hide: hideCard, card: hoverCard } = useHoverCard("left");
  const noteCard = (n: Note) => ({
    title: n.title,
    time: relativeTime(n.updatedAt),
    meta: [
      { label: KIND_LABEL[n.kind] ?? n.kind },
      ...(n.origin === "auto"
        ? [{ label: n.status === "stale" ? "Auto note · stale" : "Auto note" }]
        : []),
      { label: "Created", value: relativeTime(n.createdAt) },
    ],
  });
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
  const { primary: primaryArtifacts, groups: artifactGroups } =
    studioArtifacts(kokoroReady);
  // The inspector's three faces. Generate is the first thing a fresh
  // notebook needs; a notebook with notes opens on them. The choice sticks.
  const [studioTab, setStudioTab] = useState<StudioTab>(() => {
    try {
      const saved = localStorage.getItem("studioTab");
      if (saved === "generate" || saved === "notes" || saved === "reports") return saved;
    } catch {
      // no storage: fall through to the default
    }
    return "notes";
  });
  const changeTab = (tab: StudioTab) => {
    setStudioTab(tab);
    try {
      localStorage.setItem("studioTab", tab);
    } catch {
      // per-viewer convenience only
    }
  };
  // How many notes of each kind this notebook already has — the count a
  // generator row shows, the way Mail shows a mailbox's count.
  const kindCounts = useMemo(() => {
    const counts: Partial<Record<string, number>> = {};
    for (const n of notes) {
      if (n.status === "archived") continue;
      counts[n.kind] = (counts[n.kind] ?? 0) + 1;
    }
    return counts;
  }, [notes]);
  const reportCount = useStore((s) => s.reportSchedules.length);

  const hasSources = sources.length > 0;
  const width = useStore((s) => s.studioWidth);
  const setPanelWidth = useStore((s) => s.setPanelWidth);

  return (
    <div
      style={{ width }}
      className="side-pane relative flex shrink-0 flex-col border-l border-border"
    >
      <ResizeHandle
        edge="left"
        width={width}
        defaultWidth={320}
        onResize={(w) => setPanelWidth("studio", w)}
        label="Resize studio panel"
      />
      <div className="flex items-center px-4 h-12 border-b border-border">
        <span className="text-caption font-semibold uppercase tracking-wide text-muted-foreground">
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
      {/* An inspector, not a stack: Generate, Notes and Reports are three
          faces of one pane (macOS inspectors do this with a segmented
          control at the top), so a tall generator list never pins the
          notes below the fold and the notes never bury the generators. */}
      <div className="px-3 pt-2 pb-1">
        <Segmented
          label="Studio"
          value={studioTab}
          onChange={changeTab}
          className="w-full [&>button]:flex-1 [&>button]:justify-center"
          options={[
            { value: "generate", label: "Generate", hint: "Generators and templates" },
            {
              value: "notes",
              label: shownNotes.length > 0 ? `Notes · ${shownNotes.length}` : "Notes",
              hint: "This notebook's notes",
            },
            {
              value: "reports",
              label: reportCount > 0 ? `Reports · ${reportCount}` : "Reports",
              hint: "Scheduled reports",
            },
          ]}
        />
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
        {studioTab === "generate" && (
        <div className="p-3">
          <div className="flex items-center gap-2 text-micro font-medium uppercase tracking-wide text-subtle-foreground">
            <span>Generate</span>
            {generatingKind && !generatingHere && (
              <span className="text-badge normal-case text-subtle-foreground">
                generating in another notebook…
              </span>
            )}
            {generatingKind && generatingHere && (
              <button
                onClick={() => cancelGeneration("artifact")}
                className="flex items-center gap-1 rounded px-1.5 py-0.5 text-destructive hover:bg-destructive/10"
                title="Stop generating"
              >
                <Square className="h-3 w-3" />
                Stop
              </button>
            )}
            {audioProgress && generatingHere && (
              <span className="text-badge normal-case tabular-nums text-subtle-foreground">
                voicing {audioProgress.done}/{audioProgress.total}
              </span>
            )}
            <button
              onClick={() => {
                // Create the file first, then edit it in the reader — the
                // editor always points at a template that exists on disk.
                void (async () => {
                  try {
                    const t = await api.saveTemplate(
                      null,
                      "New template",
                      "",
                      "Describe what this generator should produce from the notebook's sources.",
                    );
                    await useStore.getState().refreshTemplates();
                    useStore.getState().openInReader({ type: "template", id: t.id });
                  } catch (e) {
                    useStore
                      .getState()
                      .pushToast("error", e instanceof Error ? e.message : String(e));
                  }
                })();
              }}
              className="ml-auto rounded p-0.5 transition-colors hover:text-foreground"
              title="New template — a reusable custom generator"
              aria-label="New template"
            >
              <Plus className="h-3.5 w-3.5" />
            </button>
            <button
              onClick={() => void api.openTemplatesFolder()}
              className="rounded p-0.5 transition-colors hover:text-foreground"
              title="Open the templates folder — each .md file is a generator"
              aria-label="Open templates folder"
            >
              <FolderOpen className="h-3.5 w-3.5" />
            </button>
          </div>
          <>
              {/* Grouped lists, one per shelf (DESIGN.md §4): the row is
                  the generator, the count on the right is how many of that
                  kind this notebook already holds. Every generator is on
                  the page; nothing hides behind More. */}
              <div className="mt-2 flex flex-col gap-3">
                <GenGroup label="Start here">
                  {primaryArtifacts.map((a) => (
                    <GenRow
                      key={a.kind}
                      icon={a.icon}
                      label={a.label}
                      family={a.family}
                      count={kindCounts[a.kind]}
                      busy={busyKinds.has(a.kind)}
                      disabled={!hasSources}
                      onClick={() => generate(a.kind, instructions)}
                    />
                  ))}
                </GenGroup>
                {artifactGroups.map((group) => {
                  const shelfTemplates = group.id === "write" ? templates : [];
                  if (group.artifacts.length === 0 && shelfTemplates.length === 0) return null;
                  return (
                    <GenGroup key={group.id} label={group.label}>
                      {group.artifacts.map((a) => (
                        <GenRow
                          key={a.kind}
                          icon={a.icon}
                          label={a.label}
                          family={a.family}
                          count={kindCounts[a.kind]}
                          busy={busyKinds.has(a.kind)}
                          disabled={!hasSources}
                          onClick={() => generate(a.kind, instructions)}
                        />
                      ))}
                      {shelfTemplates.map((t) => (
                        <GenRow
                          key={t.id}
                          icon={<FileText className="h-3.5 w-3.5" />}
                          label={t.name}
                          family="template"
                          title={`${t.description || t.name} — right-click to edit`}
                          busy={busyKinds.has(`template:${t.id}`)}
                          disabled={!hasSources}
                          onClick={() => generateFromTemplate(t)}
                          onContextMenu={(e) => {
                            e.preventDefault();
                            useStore.getState().openInReader({ type: "template", id: t.id });
                          }}
                        />
                      ))}
                    </GenGroup>
                  );
                })}
              </div>

              <Input
                name="generation-instructions"
                aria-label="Instructions for the next generation"
                value={instructions}
                onChange={(e) => setInstructions(e.target.value)}
                disabled={!hasSources}
                placeholder="Instructions for the next generation…"
                className="mt-3 h-7 bg-transparent text-caption"
              />
              {!hasSources && (
                <p className="mt-2 text-micro text-subtle-foreground">
                  Add sources to generate documents.
                </p>
              )}
            </>
        </div>
        )}

        {studioTab === "reports" && currentId && <Reports />}

        {/* Header and list share one marquee container: the sources panel
            lets a drag start on its "All selected" strip, and starting a
            notes selection was harder for want of the same run-up. */}
        {studioTab === "notes" && (
        <div ref={notesListRef} onPointerDown={marqueeDown} className="select-none">
        <div className="flex items-center justify-between px-4 pt-3 pb-1">
          <span className="text-micro font-medium uppercase tracking-wide text-subtle-foreground">
            Notes
            {shownNotes.length > 0 && (
              <span className="ml-1.5 normal-case tracking-normal text-subtle-foreground/80">
                {noteQuery.trim()
                  ? `${listedNotes.length} of ${shownNotes.length}`
                  : shownNotes.length}
              </span>
            )}
          </span>
          <div className="flex items-center gap-0.5">
            {/* A menu rather than a select: the panel is 320px at rest, and
                the three orders are a one-of choice, which is what a radio
                menu item says out loud. */}
            <SortMenu
              label="Sort notes"
              value={noteSort}
              options={NOTE_SORTS}
              onChange={changeNoteSort}
            />
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
        </div>

        {(shownNotes.length > 8 || noteQuery) && (
          <div className="px-4 pb-2">
            <SearchField
              value={noteQuery}
              onValueChange={setNoteQuery}
              onKeyDown={(e) => {
                if (e.key === "Escape" && noteQuery) {
                  e.preventDefault();
                  setNoteQuery("");
                }
              }}
              placeholder="Filter notes…"
              aria-label="Filter notes"
              autoCapitalize="none"
            />
          </div>
        )}
        <div className="px-2 pb-2">
          {notebookLoading && notes.length === 0 ? (
            <LoadingState label="Loading notes…" compact />
          ) : notes.length === 0 ? (
            <EmptyState
              icon={<StickyNote className="h-6 w-6" />}
              title="No notes yet"
              hint="Generate a document above or write your own note."
            />
          ) : listedNotes.length === 0 && noteQuery.trim() ? (
            <div className="px-3 py-2 text-caption text-subtle-foreground">
              No notes match “{noteQuery.trim()}”.
            </div>
          ) : (
            <div className="flex flex-col gap-1.5">
              {listedNotes.map((n) => (
                <div
                  key={n.id}
                  data-pick-id={n.id}
                  onMouseEnter={(e) => showCard(e, noteCard(n))}
                  onMouseLeave={hideCard}
                  // Drag the row into Finder/Mail as a real file, in the
                  // note's kind-true format (RFC-professional-grade Pillar
                  // 6). The gesture only commits past a 3px threshold, so
                  // clicking to open the note is unaffected.
                  {...noteDragProps(
                    n.id,
                    dragFormat(exportTargets(n).map((t) => t.format)),
                    (message) =>
                      useStore.getState().pushToast("error", message),
                  )}
                  className={cn(
                    // has-: an open row menu must outrank the z-10 content of
                    // the rows after it (they'd paint over the dropdown
                    // otherwise — later DOM order wins at equal z).
                    // Flat rows, not bordered cards — the title and chips
                    // carry the card; a hover wash marks the target.
                    "group relative cursor-pointer rounded-md px-3 py-2 transition-colors hover:bg-surface-2",
                    n.status === "stale" && "opacity-60",
                    pickedNoteIds.has(n.id) &&
                      "bg-primary/10 hover:bg-primary/15",
                    openNoteId === n.id &&
                      !pickedNoteIds.has(n.id) &&
                      "bg-surface-2",
                  )}
                  aria-current={openNoteId === n.id ? "true" : undefined}
                >
                  <CardAction
                    label={`Open note ${n.title}`}
                    onClick={(e) => {
                      if (justEnded()) return;
                      if (n.status === "generating") return;
                      if (e.metaKey || e.ctrlKey) {
                        pickToggle("notes", n.id);
                        return;
                      }
                      if (e.shiftKey) {
                        pickRange("notes", visibleNoteIds, n.id);
                        return;
                      }
                      pickOne("notes", n.id);
                      openNoteCard(n);
                    }}
                  />
                  <div className="pointer-events-none relative z-10 flex items-center gap-2">
                    <span
                      className={cn(
                        "pointer-events-auto shrink-0 [&_svg]:h-4 [&_svg]:w-4",
                        kindAccent(n.kind),
                      )}
                      title={KIND_LABEL[n.kind]}
                    >
                      {n.status === "generating" ? (
                        <Spinner className="h-4 w-4" />
                      ) : (
                        kindIcon(n.kind)
                      )}
                    </span>
                    <span
                      className={cn(
                        "min-w-0 flex-1 truncate text-body font-medium text-foreground",
                        openNoteId === n.id && "font-semibold",
                      )}
                    >
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
                      onOpen={hideCard}
                      label={`Options for "${n.title}"`}
                      contextItems={() => {
                        if (
                          pickedNoteIds.has(n.id) &&
                          pickedNoteIds.size > 1
                        )
                          return noteBatchItems([...pickedNoteIds]);
                        pickOne("notes", n.id);
                        return null;
                      }}
                      items={
                        n.status === "generating"
                          ? [
                              {
                                label: "Stop Generating",
                                icon: <Square className="h-3.5 w-3.5" />,
                                danger: true,
                                onClick: () =>
                                  void api.cancelGenerationJob(n.id),
                              },
                            ]
                          : [
                        // A failed attempt: the fix is the first verb, not
                        // buried under Copy Text (Reminders, 2026-09-23).
                        ...(n.status === "error"
                          ? [
                              {
                                label: "Retry",
                                icon: <RotateCw className="h-3.5 w-3.5" />,
                                onClick: () => retryNote(n),
                              },
                            ]
                          : []),
                        { label: "", separator: true, onClick: () => {} },
                        {
                          label: "Copy Text",
                          icon: <Copy className="h-3.5 w-3.5" />,
                          onClick: () => {
                            void copyNotes([n.id]);
                          },
                        },
                        ...exportTargets(n).map((t) => ({
                          label: t.label,
                          icon: <FileDown className="h-3.5 w-3.5" />,
                          onClick: () => void exportNote(n, t),
                        })),
                        {
                          label: "Second Look",
                          icon: <ShieldCheck className="h-3.5 w-3.5" />,
                          onClick: () => {
                            void api.runSecondLook(n.id).then(
                              () =>
                                useStore
                                  .getState()
                                  .pushToast(
                                    "success",
                                    "Second Look running — the verdict note will appear here",
                                  ),
                              (e: unknown) =>
                                useStore
                                  .getState()
                                  .pushToast(
                                    "error",
                                    e instanceof Error ? e.message : String(e),
                                  ),
                            );
                          },
                        },
                        { label: "", separator: true, onClick: () => {} },
                        {
                          label: "Delete",
                          symbol: "trash",
                          icon: <Trash2 className="h-3.5 w-3.5" />,
                          danger: true,
                          onClick: () => void deleteNote(n.id),
                        },
                      ]
                      }
                    />
                  </div>
                  <div className="pointer-events-none relative z-10 mt-1 flex items-center gap-1.5 pl-[22px]">
                    {/* The kind icon on the title row replaces the old text
                        chip. Badges re-enable hit-testing so their explanatory
                        tooltips still show inside the pointer-events-none row. */}
                    {n.origin === "auto" && (
                      <span
                        className="pointer-events-auto"
                        title="Chat saved this on its own. Edit it to make it yours."
                      >
                        <Badge>auto</Badge>
                      </span>
                    )}
                    {n.status === "stale" && (
                      <span
                        className="pointer-events-auto"
                        title="Unused for a while. Alchemy archives old notes; opening or editing keeps this one."
                      >
                        <Badge>stale</Badge>
                      </span>
                    )}
                    {/* The other person deleted this note in the shared folder and
                        the question is still open (docs/RFC-shared-notebook.md §3). */}
                    {proposals[n.id] ? (
                      <DeletionProposalMark id={n.id} />
                    ) : n.status === "generating" ? (
                      <span className="pointer-events-auto flex items-center gap-1.5 text-micro text-subtle-foreground">
                        {genStatus[n.id]?.status === "waiting"
                          ? genStatus[n.id]?.detail || "Waiting for the model engine…"
                          : n.kind === "audio_overview" && audioProgress
                            ? `Voicing ${audioProgress.done}/${audioProgress.total}`
                            : genProgress[n.id]
                              ? `Generating — ${(genProgress[n.id] / 1000).toFixed(1)}k chars`
                              : genStatus[n.id]?.status === "running"
                                ? genStatus[n.id]?.detail || "Generating"
                                : "Queued"}
                        <button
                          type="button"
                          onClick={() => void api.cancelGenerationJob(n.id)}
                          className="flex items-center gap-0.5 rounded px-1 py-0.5 text-destructive hover:bg-destructive/10"
                          title="Stop this generation"
                        >
                          <Square className="h-2.5 w-2.5" />
                          Stop
                        </button>
                      </span>
                    ) : n.status === "error" ? (
                      <span className="pointer-events-auto flex items-center gap-1.5 text-micro">
                        <Badge>failed</Badge>
                        <button
                          type="button"
                          onClick={() => retryNote(n)}
                          className="rounded px-1 py-0.5 text-subtle-foreground hover:text-foreground"
                          title="Delete this attempt and generate again"
                        >
                          Retry
                        </button>
                      </span>
                    ) : (
                      <span className="text-micro text-subtle-foreground">
                        {relativeTime(n.updatedAt)}
                      </span>
                    )}
                  </div>
                </div>
              ))}
            </div>
          )}
          <WikiNotes
            notes={notes.filter(
              (n) => n.status !== "archived" && isWikiPage(n),
            )}
            onOpen={openNoteCard}
          />
          <ArchivedNotes
            notes={notes.filter((n) => n.status === "archived")}
            onOpen={openNoteCard}
          />
        </div>
        </div>
        )}
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
            <span className="flex items-center gap-2 text-caption tabular-nums text-muted-foreground">
              <Spinner className="h-3.5 w-3.5" />
              {audioProgress
                ? `Voicing line ${audioProgress.done} of ${audioProgress.total}`
                : "Streaming. Closing this keeps generating."}
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
          <LazyRichEditor value={draftBody} onChange={setDraftBody} />
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

      {marquee}
      {hoverCard}
    </div>
  );
}

/** One generator tile in the flowing Studio grid. */
type StudioTab = "generate" | "notes" | "reports";

/** One shelf of generators: a small caps label over a hairline group. */
function GenGroup({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section>
      <div className="mb-1 px-1 text-micro font-medium uppercase tracking-wide text-subtle-foreground">
        {label}
      </div>
      <div className="overflow-hidden rounded-lg border border-border bg-surface/60 [&>*+*]:border-t [&>*+*]:border-border">
        {children}
      </div>
    </section>
  );
}

/** A generator as a list row: the family accent on its icon (wayfinding,
 *  DESIGN.md §2), its name, and how many of that kind the notebook already
 *  holds. Pressing it generates; while it runs the icon spins and a second
 *  press is refused, because the pending note below is where the run
 *  reports. */
function GenRow({
  icon,
  label,
  title,
  family,
  count,
  disabled,
  busy,
  onClick,
  onContextMenu,
}: {
  icon: ReactNode;
  label: string;
  title?: string;
  family: Artifact["family"] | "template";
  count?: number;
  disabled: boolean;
  busy?: boolean;
  onClick: () => void;
  /** Template rows: right-click opens the editor. */
  onContextMenu?: (e: React.MouseEvent) => void;
}) {
  const tint = family === "template" ? TINT_TEMPLATES : TINT_BY_FAMILY[family];
  return (
    <button
      type="button"
      disabled={disabled || busy}
      aria-busy={busy || undefined}
      onClick={onClick}
      onContextMenu={onContextMenu}
      aria-label={[label, busy && "generating"].filter(Boolean).join(", ")}
      title={[label, busy ? "Generating…" : title].filter(Boolean).join(" — ")}
      className="flex h-9 w-full items-center gap-2.5 px-3 text-left text-caption text-foreground/90 transition-colors hover:bg-surface-2 disabled:pointer-events-none disabled:opacity-40"
    >
      <span className={cn("shrink-0", tint.icon)}>
        {busy ? <Spinner className="h-3.5 w-3.5" /> : icon}
      </span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {count ? (
        <span className="text-micro tabular-nums text-subtle-foreground">{count}</span>
      ) : null}
    </button>
  );
}

async function copyNotes(ids: string[]) {
  try {
    const pieces: string[] = [];
    for (const id of ids) {
      const note = await api.readNote(id);
      pieces.push(ids.length === 1 ? note.content : `# ${note.title}\n\n${note.content}`);
    }
    await navigator.clipboard.writeText(pieces.join("\n\n---\n\n"));
    useStore.getState().pushToast("success", ids.length === 1 ? "Note copied" : `${ids.length} notes copied`);
  } catch (error) {
    useStore.getState().pushToast("error", String(error));
  }
}
