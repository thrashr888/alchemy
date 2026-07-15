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
    const results = await Promise.allSettled([
      api.listAllReportSchedules(),
      api.listRecentNotes(5),
      api.listRecentReports(50),
      api.corpusStats(),
    ] as const);
    if (id !== requestId.current) return;

    setData((current) => ({
      schedules: results[0].status === "fulfilled" ? results[0].value : current.schedules,
      recentNotes: results[1].status === "fulfilled" ? results[1].value : current.recentNotes,
      reports: results[2].status === "fulfilled" ? results[2].value : current.reports,
      stats: results[3].status === "fulfilled" ? results[3].value : current.stats,
    }));
    const failed = results.filter((result) => result.status === "rejected").length;
    setError(
      failed > 0
        ? `Couldn’t refresh ${failed === 1 ? "part" : `${failed} parts`} of home activity.`
        : null,
    );
    setLoading(false);
  }, []);

  useEffect(() => {
    void refresh();
    return () => {
      requestId.current += 1;
    };
  }, [notebooks, refresh]);

  return { ...data, loading, error, refresh };
}
