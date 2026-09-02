import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "@/lib/api";
import type { HomeActivity, Notebook, SourceEvent } from "@/lib/types";

type HomeActivityData = Omit<HomeActivity, "stats"> & {
  stats: HomeActivity["stats"] | null;
  /** Source-change events across every notebook, newest first — the
   *  Arrivals tallies in the away digest (RFC-events §6). */
  events: SourceEvent[];
};

const EMPTY_ACTIVITY: HomeActivityData = {
  schedules: [],
  recentNotes: [],
  reports: [],
  stats: null,
  events: [],
};

/** As far back as "since you were away" reaches; the table keeps 30 days. */
const EVENTS_WINDOW_HOURS = 24 * 7;

/** Load one backend snapshot so notes feed recent activity, reports, and stats
 * without three overlapping corpus scans. */
export function useHomeActivity(notebooks: Notebook[]) {
  const [data, setData] = useState<HomeActivityData>(EMPTY_ACTIVITY);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);

  const refresh = useCallback(async () => {
    const id = ++requestId.current;
    setLoading(true);

    try {
      // Events are a separate small read; a miss there must not blank the
      // whole home, so it degrades to "no arrivals".
      const [snapshot, events] = await Promise.all([
        api.homeActivity(),
        api.listSourceEvents(EVENTS_WINDOW_HOURS).catch((): SourceEvent[] => []),
      ]);
      if (id !== requestId.current) return;
      setData({ ...snapshot, events });
      setError(null);
    } catch {
      if (id !== requestId.current) return;
      // Keep the previous successful snapshot visible.
      setError("Couldn’t refresh home activity.");
    } finally {
      if (id === requestId.current) setLoading(false);
    }
  }, []);

  // Keyed on a content fingerprint, not array identity: refreshNotebooks
  // rebuilds the array on every mcp://changed, and each rebuild used to
  // refire all four corpus queries even when nothing had actually moved.
  const fingerprint = notebooks.map((n) => `${n.id}:${n.updatedAt}`).join("|");
  useEffect(() => {
    void refresh();
    return () => {
      requestId.current += 1;
    };
  }, [fingerprint, refresh]);

  return { ...data, loading, error, refresh };
}
