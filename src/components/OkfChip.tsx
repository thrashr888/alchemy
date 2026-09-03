// The "On disk" pill beside a bound notebook's name (docs/RFC-okf-live.md
// §5.5). Quiet by design: it says the notebook has a second home, where it
// is, and when Alchemy last wrote there. Clicking opens the folder.
import { HardDrive } from "lucide-react";
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
  return (
    <button
      type="button"
      onClick={() => void revealItemInDir(binding.path).catch(() => {})}
      title={`${binding.path}\nLast written ${ago(binding.lastWriteAt)}`}
      aria-label={`Show the bundle folder for this notebook in Finder. ${binding.path}`}
      className="flex shrink-0 items-center gap-1 rounded border border-border px-1.5 py-px text-micro text-muted-foreground transition-colors hover:border-border-strong hover:text-foreground"
    >
      <HardDrive className="h-3 w-3" aria-hidden="true" />
      On disk
    </button>
  );
}
