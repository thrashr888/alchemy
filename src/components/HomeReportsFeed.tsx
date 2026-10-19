import type { Note, SourceEvent } from "@/lib/types";
import { tallyEvents, unseenEvents } from "@/lib/arrivals";

/** One quiet line describing activity since the previous home visit. */
export function AwayDigest({
  prevVisit,
  notebooks,
  reports,
  events = [],
}: {
  prevVisit: number;
  notebooks: { updatedAt: number }[];
  reports: Note[];
  /** Source-change events across notebooks; the arrivals since the last
   *  visit join the line (RFC-events §6). */
  events?: SourceEvent[];
}) {
  if (!prevVisit) return null;
  const newReports = reports.filter((report) => report.updatedAt > prevVisit).length;
  const updatedNotebooks = notebooks.filter(
    (notebook) => notebook.updatedAt > prevVisit,
  ).length;
  const parts = [
    newReports > 0 && `${newReports} new ${newReports === 1 ? "report" : "reports"}`,
    updatedNotebooks > 0 &&
      `${updatedNotebooks} ${updatedNotebooks === 1 ? "notebook" : "notebooks"} updated`,
    ...tallyEvents(unseenEvents(events, prevVisit)),
  ].filter(Boolean);
  if (parts.length === 0) return null;
  return (
    <p className="mt-0.5 text-caption text-subtle-foreground">
      Since you were away: {parts.join(" · ")}
    </p>
  );
}
