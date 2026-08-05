import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "@/lib/api";
import type { CorpusStats, Note, Notebook, ReportSchedule } from "@/lib/types";

export interface HomeActivityData {
  schedules: ReportSchedule[];
  recentNotes: Note[];
  reports: Note[];
  stats: CorpusStats | null;
}

const EMPTY_ACTIVITY: HomeActivityData = {
  schedules: [],
  recentNotes: [],
  reports: [],
  stats: null,
};

/** Load independent home activity in parallel while preserving successful data. */
export function useHomeActivity(notebooks: Notebook[]) {
  const [data, setData] = useState<HomeActivityData>(EMPTY_ACTIVITY);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);

  const refresh = useCallback(async () => {
    const id = ++requestId.current;
    setLoading(true);

    // Commit each slice as it settles so fast slices render without waiting
    // for the slowest one; failures keep the previous data.
    const load = <K extends keyof HomeActivityData>(
      key: K,
      promise: Promise<HomeActivityData[K]>,
    ) =>
      promise.then(
        (value) => {
          if (id === requestId.current) {
            setData((current) => ({ ...current, [key]: value }));
          }
          return true;
        },
        () => false,
      );

    const settled = await Promise.all([
      load("schedules", api.listAllReportSchedules()),
      load("recentNotes", api.listRecentNotes(5)),
      load("reports", api.listRecentReports(50)),
      load("stats", api.corpusStats()),
    ]);
    if (id !== requestId.current) return;

    const failed = settled.filter((ok) => !ok).length;
    setError(
      failed > 0
        ? `Couldn’t refresh ${failed === 1 ? "part" : `${failed} parts`} of home activity.`
        : null,
    );
    setLoading(false);
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
