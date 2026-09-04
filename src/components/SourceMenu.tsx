import { useState } from "react";
import { api } from "@/lib/api";
import { removeSourcesGuarded, useStore } from "@/lib/store";
import type { Source } from "@/lib/types";
import { isWebUrl } from "@/lib/utils";
import { FOLDER_TYPES } from "@/lib/sourceFacets";
import {
  Button,
  Input,
  Modal,
  Textarea,
  useConfirm,
  type RowMenuItem,
} from "./ui";
import {
  SourceMetaModals,
  sourceMetaItems,
  sourceOriginItems,
  type NoteEditState,
  type TagEditState,
} from "./SourceMetaModals";
import { SourceImagePicker } from "./SourceImagePicker";
import { FollowFeedModal } from "./FollowFeedModal";
import { AttachToCardModal } from "./RegistrySection";
import {
  Image as ImageIcon,
  MessageSquare,
  Package,
  Pencil,
  Plus,
  RefreshCw,
  Rss,
  Trash2,
} from "lucide-react";

/* One source, ONE menu (DESIGN.md "objects are direct"). Every surface that
 * shows a source — the sidebar rows, gallery cards, the reader's overflow,
 * graph nodes, chat citations — asks `useSourceActions` for the same list in
 * the same order, and mounts the same modals. A verb added here appears
 * everywhere at once; a surface that must drop one says which (`omit`),
 * rather than rebuilding the list with the verb missing. */

// One definition, in the pure module the Sources panel's facets share
// (src/lib/sourceFacets.ts); re-exported here because this is where every
// surface already reaches for it.
export { FOLDER_TYPES };

/** A verb the host renders elsewhere (the reader's inline toolbar) and so
 *  leaves out of the menu. */
export type SourceMenuVerb = "ask" | "refresh" | "origin" | "remove";

export interface SourceMenuOpts {
  omit?: SourceMenuVerb[];
}

interface Host {
  setTagEdit: (s: NonNullable<TagEditState>) => void;
  setNoteEdit: (s: NonNullable<NoteEditState>) => void;
  editText: (s: Source) => void;
  editMacNote: (s: Source) => void;
  /** "Follow updates…" — offer the feeds this page can be followed by. */
  followUpdates: (s: Source) => void;
  addReminder: (s: Source) => void;
  chooseImage: (s: Source) => void;
  attach: (s: Source) => void;
  remove: (s: Source) => void;
}

/** The canonical source menu. Pure: the host supplies the modal openers. */
export function sourceMenuItems(
  s: Source,
  host: Host,
  opts: SourceMenuOpts = {},
): RowMenuItem[] {
  const omit = new Set(opts.omit ?? []);
  const st = useStore.getState();
  const isFolder = FOLDER_TYPES.includes(s.sourceType);
  const isMacNote = s.url.startsWith("cider://notes/note/");
  const isMacReminders = s.url.startsWith("cider://reminders/list/");
  // A folder scopes chat to its files; placeholders have no chunks yet.
  const askable =
    s.status === "ready" &&
    (!isFolder ||
      st.sources.some((k) => k.parentId === s.id && k.status === "ready"));
  const editable =
    s.sourceType !== "url" &&
    s.sourceType !== "mac" &&
    !isFolder &&
    s.status !== "placeholder";
  const refreshLabel =
    s.sourceType === "feed"
      ? "Check feed now"
      : isFolder
    ? "Rescan folder now"
    : s.sourceType === "mac"
      ? "Sync now"
      : s.status === "placeholder"
        ? "Download & embed"
        : isWebUrl(s.url)
          ? "Refresh from URL"
          : "Refresh from file";
  return [
    ...(askable && !omit.has("ask")
      ? [
          {
            label: "Ask about this source",
            icon: <MessageSquare className="h-3.5 w-3.5" />,
            onClick: () => st.askAboutSource(s.id),
          },
        ]
      : []),
    // url holds the origin — a web URL, an on-disk path, a folder, a Mac
    // app — and any of them can be refreshed.
    ...(s.url && !omit.has("refresh")
      ? [
          {
            label: refreshLabel,
            icon: <RefreshCw className="h-3.5 w-3.5" />,
            onClick: () => void st.refreshSource(s.id),
          },
        ]
      : []),
    // A web page may advertise a feed, or sit on a host whose shape implies
    // one (docs/RFC-events.md §2). Following it is a separate, explicit act.
    ...(s.sourceType === "url" && !s.parentId && isWebUrl(s.url)
      ? [
          {
            label: "Follow updates…",
            icon: <Rss className="h-3.5 w-3.5" />,
            onClick: () => host.followUpdates(s),
          },
        ]
      : []),
    // Mac sources are mirrors — editing our copy would be overwritten, so
    // writes go to the app itself and sync back.
    ...(isMacNote
      ? [
          {
            label: "Edit note",
            icon: <Pencil className="h-3.5 w-3.5" />,
            onClick: () => host.editMacNote(s),
          },
        ]
      : []),
    ...(isMacReminders
      ? [
          {
            label: "Add reminder…",
            icon: <Plus className="h-3.5 w-3.5" />,
            onClick: () => host.addReminder(s),
          },
        ]
      : []),
    ...(editable
      ? [
          {
            label: "Edit text",
            icon: <Pencil className="h-3.5 w-3.5" />,
            onClick: () => host.editText(s),
          },
        ]
      : []),
    // Hand-pick the gallery card's image: url sources are the type whose
    // card leads with one.
    ...(s.sourceType === "url"
      ? [
          {
            label: "Choose card image…",
            icon: <ImageIcon className="h-3.5 w-3.5" />,
            onClick: () => host.chooseImage(s),
          },
        ]
      : []),
    ...(omit.has("origin") ? [] : sourceOriginItems(s)),
    ...sourceMetaItems(s, host.setTagEdit, host.setNoteEdit),
    {
      label: "File under a card…",
      icon: <Package className="h-3.5 w-3.5" />,
      onClick: () => host.attach(s),
    },
    ...(omit.has("remove")
      ? []
      : [
          {
            label: "Remove…",
            icon: <Trash2 className="h-3.5 w-3.5" />,
            danger: true,
            onClick: () => host.remove(s),
          },
        ]),
  ];
}

/** The source menu plus every modal it opens, for one host component.
 *  Mount `modals` once; call `items(s)` per row/card. */
export function useSourceActions() {
  const [tagEdit, setTagEdit] = useState<TagEditState>(null);
  const [noteEdit, setNoteEdit] = useState<NoteEditState>(null);
  const [attaching, setAttaching] = useState<Source | null>(null);
  const [imageFor, setImageFor] = useState<Source | null>(null);
  const [addingReminder, setAddingReminder] = useState<{
    sourceId: string;
    list: string;
  } | null>(null);
  const [editing, setEditing] = useState<{
    id: string;
    title: string;
    text: string;
    /** Editing the Apple Note itself — save writes back through cider. */
    macNote?: boolean;
  } | null>(null);
  const { confirm, dialog: confirmDialog } = useConfirm();
  const [following, setFollowing] = useState<Source | null>(null);
  const editSourceText = useStore((s) => s.editSourceText);
  const updateMacNote = useStore((s) => s.updateMacNote);
  const addMacReminder = useStore((s) => s.addMacReminder);

  const host: Host = {
    setTagEdit,
    setNoteEdit,
    editText: (s) => {
      // List payloads omit content; fetch the full text to prefill.
      void api
        .getSourceContent(s.id)
        .then((content) => setEditing({ id: s.id, title: s.title, text: content }));
    },
    editMacNote: (s) => {
      // The real note body (first line is the title — Notes derives the
      // visible title from it), not our rendered markdown copy.
      void api
        .macNoteBody(s.id)
        .then((body) =>
          setEditing({ id: s.id, title: s.title, text: body, macNote: true }),
        );
    },
    addReminder: (s) => setAddingReminder({ sourceId: s.id, list: s.title }),
    followUpdates: setFollowing,
    chooseImage: setImageFor,
    attach: setAttaching,
    remove: (s) => void removeSourcesGuarded([s.id], confirm),
  };

  const modals = (
    <>
      <SourceMetaModals
        tagEdit={tagEdit}
        setTagEdit={setTagEdit}
        noteEdit={noteEdit}
        setNoteEdit={setNoteEdit}
      />
      <AttachToCardModal
        sourceId={attaching?.id ?? null}
        sourceTitle={attaching?.title ?? ""}
        onClose={() => setAttaching(null)}
      />
      {imageFor && (
        <SourceImagePicker
          source={imageFor}
          open
          onClose={() => setImageFor(null)}
        />
      )}
      <FollowFeedModal source={following} onClose={() => setFollowing(null)} />
      <Modal
        open={!!editing}
        onClose={() => setEditing(null)}
        title={editing?.macNote ? "Edit Apple Note" : "Edit source"}
        width="max-w-lg"
      >
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            if (!editing) return;
            const { id, title, text, macNote } = editing;
            setEditing(null);
            if (macNote) await updateMacNote(id, text);
            else await editSourceText(id, title, text);
          }}
          className="flex flex-col gap-3"
        >
          {/* The note's title IS its first line — no separate title field. */}
          {!editing?.macNote && (
            <Input
              autoFocus
              name="source-title"
              aria-label="Source title"
              placeholder="Title"
              value={editing?.title ?? ""}
              onChange={(e) =>
                setEditing((s) => (s ? { ...s, title: e.target.value } : s))
              }
            />
          )}
          <Textarea
            autoFocus={editing?.macNote}
            rows={12}
            name="source-text"
            aria-label={editing?.macNote ? "Apple Note text" : "Source text"}
            placeholder="Source text…"
            value={editing?.text ?? ""}
            onChange={(e) =>
              setEditing((s) => (s ? { ...s, text: e.target.value } : s))
            }
          />
          {editing?.macNote && (
            <p className="text-micro leading-relaxed text-subtle-foreground">
              Saves straight into Apple Notes — the first line is the note's
              title.
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setEditing(null)}
            >
              Cancel
            </Button>
            <Button
              type="submit"
              variant="primary"
              disabled={!editing?.text.trim()}
            >
              {editing?.macNote ? "Save to Apple Notes" : "Save"}
            </Button>
          </div>
        </form>
      </Modal>
      <Modal
        open={!!addingReminder}
        onClose={() => setAddingReminder(null)}
        title={`Add reminder to "${addingReminder?.list ?? ""}"`}
        width="max-w-md"
      >
        <AddReminderForm
          key={addingReminder?.sourceId ?? "none"}
          onSubmit={async (title, notes) => {
            if (!addingReminder) return;
            const { sourceId } = addingReminder;
            setAddingReminder(null);
            await addMacReminder(sourceId, title, notes);
          }}
          onCancel={() => setAddingReminder(null)}
        />
      </Modal>
      {confirmDialog}
    </>
  );

  return {
    /** The menu for one source. */
    items: (s: Source, opts?: SourceMenuOpts) => sourceMenuItems(s, host, opts),
    /** Batch verbs share the tag editor. */
    setTagEdit,
    /** The remove flow (undo toast, or a confirm for connector sources). */
    confirm,
    modals,
  };
}

/** Title + optional notes for a new reminder in a connected list. */
function AddReminderForm({
  onSubmit,
  onCancel,
}: {
  onSubmit: (title: string, notes?: string) => void;
  onCancel: () => void;
}) {
  const [title, setTitle] = useState("");
  const [notes, setNotes] = useState("");
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (title.trim()) onSubmit(title.trim(), notes.trim() || undefined);
      }}
      className="flex flex-col gap-3"
    >
      <Input
        autoFocus
        name="reminder-title"
        aria-label="Reminder title"
        placeholder="Remind me to…"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
      />
      <Textarea
        rows={3}
        name="reminder-notes"
        aria-label="Reminder notes"
        placeholder="Notes (optional)"
        value={notes}
        onChange={(e) => setNotes(e.target.value)}
      />
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={!title.trim()}>
          Add reminder
        </Button>
      </div>
    </form>
  );
}
