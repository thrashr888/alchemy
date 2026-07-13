import { useStore } from "@/lib/store";
import { sourceIcon } from "./SourcesPanel";
import { cn } from "@/lib/utils";
import {
  PanelLeft,
  PanelRight,
  Plus,
  Wand2,
  StickyNote,
  AlertCircle,
} from "lucide-react";

/**
 * Thin icon rail shown when the Sources panel is collapsed — mirrors
 * NotebookLM: each source's type icon stacked vertically; click anything to
 * reopen the panel. The + opens the add-source modal (a global surface, so
 * the panel can stay collapsed).
 */
export function SourcesRail() {
  const sources = useStore((s) => s.sources);
  const toggleSources = useStore((s) => s.toggleSources);
  const currentId = useStore((s) => s.currentId);
  const openAddSource = useStore((s) => s.openAddSource);

  return (
    <div className="flex w-12 shrink-0 flex-col items-center border-r border-border bg-surface py-2">
      <button
        onClick={toggleSources}
        title="Show sources"
        className="rounded-md p-1.5 text-subtle-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
      >
        <PanelLeft className="h-4 w-4" />
      </button>
      <div className="my-1.5 h-px w-6 bg-border" />
      <button
        onClick={() => openAddSource()}
        disabled={!currentId}
        title="Add source"
        aria-label="Add source"
        className="rounded-md p-1.5 text-subtle-foreground transition-colors hover:bg-surface-2 hover:text-foreground disabled:opacity-40"
      >
        <Plus className="h-4 w-4" />
      </button>
      <div className="flex min-h-0 flex-1 flex-col items-center gap-0.5 overflow-y-auto">
        {sources.map((s) => (
          <button
            key={s.id}
            onClick={toggleSources}
            title={s.title}
            className="relative rounded-md p-1.5 transition-colors hover:bg-surface-2"
          >
            {sourceIcon(s.sourceType, s.url)}
            {s.status === "error" && (
              <AlertCircle className="absolute -right-0 -top-0 h-2.5 w-2.5 text-destructive" />
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
    <div className="flex w-12 shrink-0 flex-col items-center border-l border-border bg-surface py-2">
      <button
        onClick={toggleStudio}
        title="Show studio"
        className="rounded-md p-1.5 text-subtle-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
      >
        <PanelRight className="h-4 w-4" />
      </button>
      <div className="my-1.5 h-px w-6 bg-border" />
      <button
        onClick={toggleStudio}
        title="Generate documents"
        className="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
      >
        <Wand2 className="h-4 w-4" />
      </button>
      <button
        onClick={toggleStudio}
        title={`Notes${notes.length ? ` (${notes.length})` : ""}`}
        className={cn(
          "relative rounded-md p-1.5 transition-colors hover:bg-surface-2",
          notes.length
            ? "text-muted-foreground hover:text-foreground"
            : "text-subtle-foreground",
        )}
      >
        <StickyNote className="h-4 w-4" />
        {notes.length > 0 && (
          <span className="absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-primary/20 px-0.5 text-[10px] font-medium text-citation">
            {notes.length}
          </span>
        )}
      </button>
      <button
        onClick={toggleStudio}
        title="Add note"
        className="mt-auto rounded-md p-1.5 text-subtle-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
      >
        <Plus className="h-4 w-4" />
      </button>
    </div>
  );
}
