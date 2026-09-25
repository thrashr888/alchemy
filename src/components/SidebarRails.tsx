import { useStore } from "@/lib/store";
import { sourceIcon } from "@/lib/sourceIcon";
import { cn } from "@/lib/utils";
import {
  Plus,
  Wand2,
  StickyNote,
  AlertCircle,
} from "lucide-react";

/**
 * The chrome's icon button (docs/RFC-mac-chrome.md, "Shared": 24px high,
 * min-width 28, padding 0 7px, radius 6, 12px, muted; hover surface-2 +
 * foreground). Stated as a class rather than taken from `Button`, whose
 * `size="icon"` is a 28px square: the chrome's buttons are shorter than they
 * are wide, so a row of them reads as one strip rather than a row of keys.
 * Lives here because it is the one module the toolbar, the Sources pane and
 * the rails all sit downstream of; it belongs in `ui.tsx` as `tb` once that
 * file's own pass lands.
 */
export const CHROME_BUTTON =
  "inline-flex h-6 min-w-7 shrink-0 items-center justify-center rounded-md px-[7px] text-caption font-medium text-muted-foreground transition-colors outline-none hover:bg-surface-2 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 disabled:pointer-events-none disabled:opacity-40";

/** The same button squeezed into a 48px rail: 24px square, no min-width. */
const RAIL_BUTTON =
  "inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors outline-none hover:bg-surface-2 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 disabled:pointer-events-none disabled:opacity-40";

/**
 * Thin icon rail shown when the Sources panel is collapsed — mirrors
 * NotebookLM: each source's type icon stacked vertically; click anything to
 * reopen the panel. The + opens the add-source modal (a global surface, so
 * the panel can stay collapsed). Padding matches the pane it stands in for,
 * so nothing shifts vertically when the pane opens.
 */
export function SourcesRail() {
  const sources = useStore((s) => s.sources);
  const toggleSources = useStore((s) => s.toggleSources);
  const currentId = useStore((s) => s.currentId);
  const openAddSource = useStore((s) => s.openAddSource);

  return (
    <div className="side-pane relative flex w-12 shrink-0 flex-col items-center gap-0.5 border-r border-border p-2.5">
      {/* No expand button up here any more: the toolbar's sidebar toggle is
          visible in this state too, so the rail was showing the same command
          twice, 24px apart. What is left is status — the notebook's sources
          as a column of type glyphs — and every one of them still opens the
          pane, by click or by keyboard, as does ⌘1 from anywhere. */}
      <button
        type="button"
        onClick={() => openAddSource()}
        disabled={!currentId}
        title="Add source"
        aria-label="Add source"
        className={RAIL_BUTTON}
      >
        <Plus className="h-4 w-4" />
      </button>
      <div className="flex min-h-0 flex-1 flex-col items-center gap-0.5 overflow-y-auto pt-0.5">
        {sources.map((s) => (
          <button
            key={s.id}
            type="button"
            onClick={toggleSources}
            title={s.title}
            // Every icon here opens the panel, so the source it stands for is
            // what distinguishes it. The red dot means a failed import; say so,
            // since a dot is nothing to a screen reader.
            aria-label={
              s.status === "error"
                ? `Show sources. ${s.title} failed to import.`
                : `Show sources. ${s.title}`
            }
            className={cn(RAIL_BUTTON, "relative")}
          >
            {sourceIcon(s.sourceType, s.url)}
            {s.status === "error" && (
              <AlertCircle
                aria-hidden
                className="absolute -right-0 -top-0 h-2.5 w-2.5 text-destructive"
              />
            )}
          </button>
        ))}
      </div>
    </div>
  );
}

/** Thin icon rail shown when the Studio panel is collapsed. */
export function StudioRail() {
  const notes = useStore((s) => s.notes);
  const toggleStudio = useStore((s) => s.toggleStudio);
  return (
    <div className="side-pane relative flex w-12 shrink-0 flex-col items-center gap-0.5 border-l border-border p-2.5">
      {/* Same as the Sources rail: the toolbar's inspector toggle is up
          there in this state, so the rail drops its copy of it. The three
          buttons that remain all open the panel. */}
      <button
        type="button"
        onClick={toggleStudio}
        title="Generate documents"
        aria-label="Show studio. Generate documents"
        className={RAIL_BUTTON}
      >
        <Wand2 className="h-4 w-4" />
      </button>
      <button
        type="button"
        onClick={toggleStudio}
        title={`Notes${notes.length ? ` (${notes.length})` : ""}`}
        aria-label={`Show studio. Notes${notes.length ? ` (${notes.length})` : ""}`}
        className={cn(RAIL_BUTTON, "relative")}
      >
        <StickyNote className="h-4 w-4" />
        {notes.length > 0 && (
          <span className="absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-primary/20 px-0.5 text-badge font-medium text-citation">
            {notes.length}
          </span>
        )}
      </button>
      <button
        type="button"
        onClick={toggleStudio}
        title="Add note"
        aria-label="Add note"
        className={cn(RAIL_BUTTON, "mt-auto")}
      >
        <Plus className="h-4 w-4" />
      </button>
    </div>
  );
}
