import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import type { Citation } from "@/lib/types";
import { sourceIcon } from "./SourcesPanel";
import { cn } from "@/lib/utils";
import { Sparkles, StickyNote } from "lucide-react";

function SourceChip({ c }: { c: Citation }) {
  const source = useStore((s) => s.sources.find((x) => x.id === c.sourceId));
  return (
    <span className="flex min-w-0 items-center gap-1.5 text-[11px] font-medium text-foreground">
      {c.noteId ? (
        <StickyNote className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      ) : (
        source && sourceIcon(source.sourceType, source.url)
      )}
      <span className="truncate">{c.sourceTitle}</span>
    </span>
  );
}

/**
 * Ambient connections (docs/RFC-document-surface.md, phase 3): while the
 * user writes, the paragraph they're working on is embedded on a debounce
 * and the top related source passages appear in this quiet rail — no
 * clicking, no asking; the notebook holds up relevant evidence while you
 * think. Clicking a passage opens the source in the reader at that spot.
 */

/** The paragraph the user is working on: the first one that changed, or the
 *  last non-empty one when nothing differs (e.g. on entry). */
export function activeParagraph(prev: string, next: string): string {
  const a = prev.split(/\n{2,}/);
  const b = next.split(/\n{2,}/);
  for (let i = 0; i < b.length; i++) {
    if (a[i] !== b[i]) return (b[i] ?? "").trim().slice(0, 600);
  }
  for (let i = b.length - 1; i >= 0; i--) {
    const p = (b[i] ?? "").trim();
    if (p) return p.slice(0, 600);
  }
  return "";
}

export function AmbientRail({
  text,
  excludeNoteId,
  floating = false,
}: {
  text: string;
  /** The note being edited — its own passages are not "connections". */
  excludeNoteId?: string;
  /** Float over the surface's right edge, materializing only with hits —
   *  the seamless editor has no fixed rail column. */
  floating?: boolean;
}) {
  const notebookId = useStore((s) => s.currentId);
  const [hits, setHits] = useState<Citation[]>([]);
  const lastQuery = useRef("");

  useEffect(() => {
    if (!notebookId) return;
    const query = text.trim();
    if (query.length < 24 || query === lastQuery.current) return;
    const timer = window.setTimeout(() => {
      lastQuery.current = query;
      void api
        .relatedPassages(notebookId, query, 6)
        .then((found) => {
          const usable = found.filter(
            (c) => !excludeNoteId || c.noteId !== excludeNoteId,
          );
          // Sources carry the primary evidence — notes only fill leftover
          // slots (they're often derived from the same sources anyway).
          const sources = usable.filter((c) => !c.noteId);
          const notes = usable.filter((c) => c.noteId);
          setHits([...sources, ...notes].slice(0, 3));
        })
        .catch(() => undefined);
    }, 800);
    return () => window.clearTimeout(timer);
  }, [text, notebookId, excludeNoteId]);

  if (floating && hits.length === 0) return null;
  return (
    <div
      className={cn(
        floating
          ? "absolute bottom-10 right-3 top-24 z-10 flex w-56 flex-col gap-2 overflow-y-auto"
          : "flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto",
      )}
    >
      <div className="flex items-center gap-1.5 text-[10px] font-medium uppercase tracking-wider text-subtle-foreground">
        <Sparkles className="h-3 w-3" />
        Related
      </div>
      {hits.length === 0 ? (
        <div className="text-[11px] leading-relaxed text-subtle-foreground/70">
          Passages from your sources appear here as you write.
        </div>
      ) : (
        hits.map((c) => (
          <button
            key={c.chunkId}
            type="button"
            onClick={() =>
              useStore.getState().openInReader({
                type: c.noteId ? "note" : "source",
                id: c.noteId || c.sourceId,
                highlight: c.snippet,
              })
            }
            className={cn(
              "flex flex-col gap-1 rounded-md border p-2 text-left transition-colors",
              floating
                ? "border-border/60 bg-elevated/90 shadow-sm backdrop-blur hover:border-border-strong"
                : "border-border bg-surface-2/40 hover:border-border-strong hover:bg-surface-2",
            )}
          >
            <SourceChip c={c} />
            <span className="line-clamp-4 text-[11px] leading-relaxed text-muted-foreground">
              {c.snippet}
            </span>
          </button>
        ))
      )}
    </div>
  );
}
