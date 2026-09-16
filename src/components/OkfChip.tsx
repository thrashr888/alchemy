// The "On disk" pill beside a bound notebook's name (docs/RFC-okf-live.md
// §5.5). Quiet by design: it says the notebook has a second home, where it
// is, and when Alchemy last wrote there. Clicking opens the folder.
import { HardDrive, Share } from "lucide-react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";

function ago(ms: number): string {
  if (!ms) return "not written yet";
  const secs = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (secs < 60) return "moments ago";
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins} min ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours} h ago`;
  return new Date(ms).toLocaleDateString();
}

export function OkfChip() {
  const binding = useStore((s) => s.okfBinding);
  if (!binding) return null;
  // A shared notebook says so here rather than earning a second pill: this
  // chip is the one place that says where the notebook is kept, and "shared"
  // is a fact about that place (docs/RFC-shared-notebook.md §1). Still a
  // hairline chip, still no color — being shared is not a warning.
  const shared = !!binding.shared;
  const Icon = shared ? Share : HardDrive;
  return (
    <button
      type="button"
      onClick={() => void revealItemInDir(binding.path).catch(() => {})}
      title={
        shared
          ? `${binding.path}\nShared: whoever you shared this folder with sees these files, and you see theirs.\nLast written ${ago(binding.lastWriteAt)}`
          : `${binding.path}\nLast written ${ago(binding.lastWriteAt)}`
      }
      aria-label={`Show the ${shared ? "shared " : ""}bundle folder for this notebook in Finder. ${binding.path}`}
      className="flex shrink-0 items-center gap-1 rounded border border-border px-1.5 py-px text-micro text-muted-foreground transition-colors hover:border-border-strong hover:text-foreground"
    >
      <Icon className="h-3 w-3" aria-hidden="true" />
      {shared ? "Shared" : "On disk"}
    </button>
  );
}
