import { useEffect } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useStore } from "@/lib/store";
import { Flashcards } from "./Flashcards";
import { Markdown } from "./Markdown";
import { MindMap } from "./MindMap";
import { QuizView } from "./QuizView";
import { SlideDeck } from "./SlideDeck";
import { AudioPlayer, DialogueScript } from "./AudioNote";
import { Spinner } from "./ui";
import { cn } from "@/lib/utils";
import { StickyNote } from "lucide-react";

/**
 * A whole window devoted to one note — opened from the note modal's
 * "Open in window". The boot script sets both the notebook and note ids;
 * store.init loads the notebook, and this view renders the note full-size
 * (mind maps especially outgrow the modal).
 */
export function NoteWindow({ noteId }: { noteId: string }) {
  const currentId = useStore((s) => s.currentId);
  const notes = useStore((s) => s.notes);
  const note = notes.find((n) => n.id === noteId);
  // The store clears notes on selectNotebook, so "loaded but missing" is only
  // trustworthy once the notebook is current and the notes list settled.
  const loading = !note && (!currentId || notes.length === 0);

  useEffect(() => {
    if (note) void getCurrentWebviewWindow().setTitle(`${note.title} — Alchemy`);
  }, [note]);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
      <header
        data-tauri-drag-region
        className="flex h-12 shrink-0 items-center gap-2 border-b border-border pl-[84px] pr-4"
      >
        <StickyNote className="h-3.5 w-3.5 shrink-0 text-primary" />
        <span className="truncate text-[13px] font-semibold" title={note?.title}>
          {note?.title ?? "Note"}
        </span>
      </header>
      <div
        className={cn(
          "flex-1",
          // Slides size themselves to the window; everything else scrolls.
          note?.kind === "slide_deck" ? "min-h-0 overflow-hidden" : "overflow-y-auto",
        )}
      >
        <div
          className={cn(
            "mx-auto px-8 py-8",
            // Mind maps and slide decks want the full window; prose reads
            // best at column width.
            note?.kind === "mind_map" && "max-w-none",
            note?.kind === "slide_deck" && "h-full max-w-none py-6",
            note?.kind !== "mind_map" && note?.kind !== "slide_deck" && "max-w-[760px]",
          )}
        >
          {loading ? (
            <div className="flex items-center gap-2 text-[13px] text-muted-foreground">
              <Spinner className="h-3.5 w-3.5" /> Loading note…
            </div>
          ) : !note ? (
            <div className="text-[13px] text-muted-foreground">
              This note no longer exists — it may have been deleted.
            </div>
          ) : note.kind === "mind_map" ? (
            <MindMap content={note.content} />
          ) : note.kind === "flashcards" ? (
            <Flashcards content={note.content} noteId={note.id} />
          ) : note.kind === "quiz" ? (
            <QuizView content={note.content} />
          ) : note.kind === "slide_deck" ? (
            <SlideDeck content={note.content} note={note} />
          ) : note.kind === "audio_overview" ? (
            <div className="flex flex-col gap-4">
              <AudioPlayer noteId={note.id} title={note.title} key={note.updatedAt} />
              <DialogueScript content={note.content} />
            </div>
          ) : (
            <Markdown>{note.content}</Markdown>
          )}
        </div>
      </div>
    </div>
  );
}
