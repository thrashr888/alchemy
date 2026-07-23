import { useStore } from "@/lib/store";
import { Spinner } from "./ui";
import { Layers } from "lucide-react";

/** Blocking overlay shown while all source embeddings are rebuilt. */
export function MigrationOverlay() {
  const migration = useStore((s) => s.migration);
  if (!migration) return null;

  const { done, total, title } = migration;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;

  return (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-background/85">
      <div className="w-[420px] rounded-lg border border-border-strong bg-elevated p-6 shadow-xl">
        <div className="mb-4 flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-md bg-primary/15 text-primary">
            <Layers className="h-4.5 w-4.5" />
          </div>
          <div>
            <div className="text-card font-semibold text-foreground">Re-embedding sources</div>
            <div className="text-caption text-muted-foreground">
              Rebuilding the vector index with your new model.
            </div>
          </div>
        </div>

        <div className="mb-2 h-2 overflow-hidden rounded-full bg-surface-2">
          <div
            className="h-full rounded-full bg-primary transition-all duration-300"
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
