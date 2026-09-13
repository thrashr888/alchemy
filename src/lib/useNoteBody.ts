import { useEffect, useState } from "react";
import { api } from "./api";
import type { Note, NoteSummary } from "./types";

/** Only the open reader owns a full body. There is no corpus-wide body cache.
 * Ignore late responses on navigation; keep the current editor mounted while
 * refreshing the same note, so unsaved edits retain their local ownership. */
export function useNoteBody(summary: NoteSummary | null | undefined) {
  const [loaded, setLoaded] = useState<Note | null>(null);
  const [failure, setFailure] = useState<{ id: string; message: string } | null>(null);
  useEffect(() => {
    let cancelled = false;
    setFailure(null);
    if (!summary) { setLoaded(null); return; }
    void api.readNote(summary.id).then(
      (note) => { if (!cancelled) setLoaded(note); },
      (error: unknown) => {
        if (!cancelled) setFailure({ id: summary.id, message: String(error) });
      },
    );
    return () => { cancelled = true; };
  }, [summary]);
  const note = summary && loaded?.id === summary.id ? loaded : null;
  const error = failure?.id === summary?.id ? failure?.message : undefined;
  return { note, error, loading: !!summary && !note && !error };
}
