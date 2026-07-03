import { useStore } from "@/lib/store";
import { AlertTriangle } from "lucide-react";

/** Slim warning bar shown when Ollama or a configured model isn't usable. */
export function HealthBanner({ onOpenSettings }: { onOpenSettings: () => void }) {
  const health = useStore((s) => s.modelHealth);
  if (!health) return null;

  const issues: string[] = [];
  if (!health.embed.working) issues.push(`Embeddings: ${health.embed.detail}`);
  if (!health.chat.working) issues.push(`Chat model: ${health.chat.detail}`);
  if (issues.length === 0) return null;

  return (
    <div className="flex items-center gap-2 border-b border-destructive/30 bg-destructive/10 px-4 py-1.5 text-[12px] text-destructive">
      <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
      <span className="min-w-0 flex-1 truncate" title={issues.join(" · ")}>
        {issues.join(" · ")}
      </span>
      <button
        onClick={onOpenSettings}
        className="shrink-0 rounded px-1.5 py-0.5 font-medium underline-offset-2 hover:underline"
      >
        Settings
      </button>
    </div>
  );
}
