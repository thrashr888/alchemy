import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import type { Note } from "@/lib/types";
import { AudioPlayer, DialogueScript } from "./AudioNote";
import { Markdown } from "./Markdown";
import { MindMap } from "./MindMap";
import { RichEditor } from "./RichEditor";
import { Button, Input, Modal, Spinner } from "./ui";
import {
  AppWindow,
  Check,
  Copy,
  FileInput,
  MessageSquare,
  Pencil,
  RefreshCw,
} from "lucide-react";

export function StreamingBody({ text }: { text: string }) {
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

export function StudioNoteViewer({
  note,
  onClose,
}: {
  note: Note | null;
  onClose: () => void;
}) {
  const updateNote = useStore((state) => state.updateNote);
  const rebuildNote = useStore((state) => state.rebuildNote);
  const convertNoteToSource = useStore((state) => state.convertNoteToSource);
  const discussNoteInChat = useStore((state) => state.discussNoteInChat);
  const generatingKind = useStore((state) => state.generatingKind);
  const artifactStreamText = useStore((state) => state.artifactStreamText);
  const live = useStore((state) =>
    note ? (state.notes.find((candidate) => candidate.id === note.id) ?? note) : null,
  );
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");

  const startEdit = () => {
    if (!live) return;
    setTitle(live.title);
    setBody(live.content);
    setEditing(true);
  };
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
            key={live.id}
            className="flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              updateNote(live.id, title, body);
              setEditing(false);
            }}
          >
            <Input
              name="note-title"
              aria-label="Note title"
              value={title}
              onChange={(event) => setTitle(event.target.value)}
            />
            <RichEditor value={body} onChange={setBody} />
            <div className="flex justify-end gap-2">
              <Button type="button" variant="ghost" onClick={() => setEditing(false)}>
                Cancel
              </Button>
              <Button type="submit" variant="primary">Save</Button>
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
                  <AudioPlayer
                    key={live.updatedAt}
                    noteId={live.id}
                    title={live.title}
                  />
                  <DialogueScript content={live.content} />
                </div>
              ) : (
                <Markdown>{live.content}</Markdown>
              )}
            </div>
            <div className="flex flex-wrap justify-end gap-2 border-t border-border pt-3">
              {live.kind !== "note" && (
                <Button
                  variant="secondary"
                  onClick={() => rebuildNote(live)}
                  disabled={rebuilding}
                  title="Regenerate with the latest sources"
                >
                  {rebuilding ? <Spinner className="h-3.5 w-3.5" /> : <RefreshCw className="h-3.5 w-3.5" />}
                  Rebuild
                </Button>
              )}
              <CopyButton text={live.content} />
              <Button
                variant="secondary"
                onClick={() => {
                  discussNoteInChat(live.id);
                  onClose();
                }}
                title="Add this note to chat"
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
                title="Turn this note into an embedded, searchable source"
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

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      useStore.getState().pushToast("error", "Clipboard access failed. Select the text and copy it manually.");
    }
  };
  return (
    <Button variant="secondary" onClick={() => void copy()}>
      {copied ? <Check className="h-3.5 w-3.5 text-success" /> : <Copy className="h-3.5 w-3.5" />}
      {copied ? "Copied" : "Copy"}
    </Button>
  );
}
