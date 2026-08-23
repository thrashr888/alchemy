import { useState } from "react";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { isWebUrl } from "@/lib/utils";
import type { Source } from "@/lib/types";
import { Button, Input, Modal, Textarea, type RowMenuItem } from "./ui";
import {
  ExternalLink,
  FolderOpen,
  Link2,
  StickyNote,
  Tag,
} from "lucide-react";

/* One source, one menu (DESIGN.md "objects are direct"): the shared pieces
 * of the source menu so the panel, the gallery, and the reader stop drifting
 * three separate copies. Surfaces compose these with their own verbs. */

/** Open original / Show in Finder / Copy URL — the origin trio, identical
 *  wherever a source row can be right-clicked. */
export function sourceOriginItems(s: Source): RowMenuItem[] {
  if (!s.url || s.sourceType === "mac") return [];
  const web = isWebUrl(s.url);
  return [
    ...(web
      ? [
          {
            label: "Open original",
            icon: <ExternalLink className="h-3.5 w-3.5" />,
            onClick: () => void openUrl(s.url),
          },
        ]
      : [
          {
            label: "Show in Finder",
            icon: <FolderOpen className="h-3.5 w-3.5" />,
            onClick: () => void revealItemInDir(s.url),
          },
        ]),
    {
      label: web ? "Copy URL" : "Copy file path",
      icon: <Link2 className="h-3.5 w-3.5" />,
      onClick: () => {
        void navigator.clipboard
          .writeText(s.url)
          .then(() => useStore.getState().pushToast("success", "Copied"));
      },
    },
  ];
}

export type TagEditState = { ids: string[]; title: string; value: string } | null;
export type NoteEditState = { id: string; title: string; value: string } | null;

/** Tags / note menu entries, aimed at the host's SourceMetaModals state. */
export function sourceMetaItems(
  s: Source,
  openTagEdit: (state: NonNullable<TagEditState>) => void,
  openNoteEdit: (state: NonNullable<NoteEditState>) => void,
): RowMenuItem[] {
  return [
    {
      label: s.tags ? "Edit tags…" : "Add tags…",
      icon: <Tag className="h-3.5 w-3.5" />,
      onClick: () => openTagEdit({ ids: [s.id], title: s.title, value: s.tags }),
    },
    {
      label: s.note ? "Edit note…" : "Add note…",
      icon: <StickyNote className="h-3.5 w-3.5" />,
      onClick: () =>
        openNoteEdit({ id: s.id, title: s.title, value: s.note }),
    },
  ];
}

/** Bundled state for hosts that only need the default wiring. */
export function useSourceMetaModals() {
  const [tagEdit, setTagEdit] = useState<TagEditState>(null);
  const [noteEdit, setNoteEdit] = useState<NoteEditState>(null);
  return {
    tagEdit,
    setTagEdit,
    noteEdit,
    setNoteEdit,
    modals: (
      <SourceMetaModals
        tagEdit={tagEdit}
        setTagEdit={setTagEdit}
        noteEdit={noteEdit}
        setNoteEdit={setNoteEdit}
      />
    ),
  };
}

/** The tag and note editors themselves — the single copy every surface
 *  mounts (they were pasted per-panel before). */
export function SourceMetaModals({
  tagEdit,
  setTagEdit,
  noteEdit,
  setNoteEdit,
}: {
  tagEdit: TagEditState;
  setTagEdit: (s: TagEditState) => void;
  noteEdit: NoteEditState;
  setNoteEdit: (s: NoteEditState) => void;
}) {
  const setSourceTags = useStore((s) => s.setSourceTags);
  const setSourcesTagsBatch = useStore((s) => s.setSourcesTagsBatch);
  const setSourceNote = useStore((s) => s.setSourceNote);
  return (
    <>
      <Modal
        open={!!tagEdit}
        onClose={() => setTagEdit(null)}
        title={`Tags for "${tagEdit?.title ?? ""}"`}
        width="max-w-md"
      >
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            if (!tagEdit) return;
            const { ids, value } = tagEdit;
            setTagEdit(null);
            if (ids.length === 1) await setSourceTags(ids[0], value);
            else await setSourcesTagsBatch(ids, value);
          }}
          className="flex flex-col gap-3"
        >
          <Input
            autoFocus
            name="source-tags"
            aria-label="Source tags"
            placeholder="research rust retrieval"
            value={tagEdit?.value ?? ""}
            onChange={(e) =>
              setTagEdit(tagEdit ? { ...tagEdit, value: e.target.value } : tagEdit)
            }
          />
          <p className="text-micro leading-relaxed text-subtle-foreground">
            Space-separated; "#" and case don't matter. Tags show up in
            chat's source list and help match questions to notebooks.
          </p>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setTagEdit(null)}>
              Cancel
            </Button>
            <Button type="submit" variant="primary">
              Save
            </Button>
          </div>
        </form>
      </Modal>

      <Modal
        open={!!noteEdit}
        onClose={() => setNoteEdit(null)}
        title={`Note on "${noteEdit?.title ?? ""}"`}
        width="max-w-md"
      >
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            if (!noteEdit) return;
            const { id, value } = noteEdit;
            setNoteEdit(null);
            await setSourceNote(id, value);
          }}
          className="flex flex-col gap-3"
        >
          <Textarea
            autoFocus
            rows={5}
            name="source-note"
            aria-label="Source note"
            placeholder="Why did you save this? Chat can recall it."
            value={noteEdit?.value ?? ""}
            onChange={(e) =>
              setNoteEdit(
                noteEdit ? { ...noteEdit, value: e.target.value } : noteEdit,
              )
            }
          />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setNoteEdit(null)}>
              Cancel
            </Button>
            <Button type="submit" variant="primary">
              Save
            </Button>
          </div>
        </form>
      </Modal>
    </>
  );
}
