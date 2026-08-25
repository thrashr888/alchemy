import { useEffect, useRef } from "react";
import { useStore } from "@/lib/store";
import { Spinner } from "./ui";
import { Layers } from "lucide-react";

/** Blocking overlay shown while all source embeddings are rebuilt. */
export function MigrationOverlay() {
  const migration = useStore((s) => s.migration);
  // Nothing inside is focusable — the point is that the app is unavailable —
  // so focus the panel itself, which is what makes VoiceOver read the dialog
  // out instead of leaving the user in the frozen UI behind it.
  const panelRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (migration) panelRef.current?.focus();
  }, [migration]);
  if (!migration) return null;

  const { done, total, title } = migration;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;

  return (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-background/85">
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="migration-title"
        aria-describedby="migration-detail"
        tabIndex={-1}
        className="w-[420px] rounded-lg border border-border-strong bg-elevated p-6 shadow-xl outline-none"
      >
        <div className="mb-4 flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-md bg-primary/15 text-primary">
            <Layers className="h-4.5 w-4.5" aria-hidden />
          </div>
          <div>
            <div
              id="migration-title"
              className="text-card font-semibold text-foreground"
            >
              Re-indexing sources
            </div>
            <div
              id="migration-detail"
              className="text-caption text-muted-foreground"
            >
              Rebuilding the search index with your new model.
            </div>
          </div>
        </div>

        <div
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={pct}
          aria-valuetext={
            total > 0 ? `${done} of ${total} sources` : "Starting"
          }
          className="mb-2 h-2 overflow-hidden rounded-full bg-surface-2"
        >
          <div
            className="h-full rounded-full bg-primary transition-all duration-200"
            style={{ width: `${Math.max(3, pct)}%` }}
          />
        </div>

        <div className="flex items-center justify-between text-caption">
          <span className="flex items-center gap-1.5 min-w-0 text-muted-foreground">
            <Spinner className="h-3 w-3 shrink-0" />
            <span className="truncate" title={title}>
              {title}
            </span>
          </span>
          <span className="shrink-0 text-subtle-foreground">
            {total > 0 ? `${done} / ${total}` : "…"}
          </span>
        </div>
      </div>
    </div>
  );
}
