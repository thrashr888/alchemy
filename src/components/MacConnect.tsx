import { useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Button, Spinner } from "./ui";
import { Calendar, ListChecks, NotebookText, TrendingUp } from "lucide-react";

const PROVIDERS = [
  { id: "calendar", label: "Calendar", icon: Calendar },
  { id: "reminders", label: "Reminders", icon: ListChecks },
  { id: "notes", label: "Apple Notes", icon: NotebookText },
  { id: "stocks", label: "Stocks", icon: TrendingUp },
] as const;

/**
 * "Connect" buttons for the Mac providers (Settings → General, onboarding).
 * Each runs one benign read through cider so the macOS consent prompt fires
 * at a predictable moment — clicking Allow here means adding a Mac source
 * later just works. Hidden entirely when cider isn't installed unless
 * `showInstallHint`.
 */
export function MacConnect({
  showInstallHint = false,
}: {
  showInstallHint?: boolean;
}) {
  const macAvailable = useStore((s) => s.macAvailable);
  const pushToast = useStore((s) => s.pushToast);
  const [busy, setBusy] = useState<string | null>(null);

  if (macAvailable === false) {
    return showInstallHint ? (
      <p className="text-[11px] leading-relaxed text-subtle-foreground">
        Install <code className="rounded bg-surface-2 px-1">cider</code> to use
        Calendar, Reminders &amp; Apple Notes as sources:{" "}
        <code className="rounded bg-surface-2 px-1">brew install cider</code>
      </p>
    ) : null;
  }
  if (!macAvailable) return null;

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {PROVIDERS.map(({ id, label, icon: Icon }) => (
        <Button
          key={id}
          variant="secondary"
          size="sm"
          disabled={busy !== null}
          onClick={async () => {
            setBusy(id);
            try {
              await api.macConnect(id);
              pushToast("success", `${label} connected`);
            } catch (e) {
              pushToast("error", e instanceof Error ? e.message : String(e));
            } finally {
              setBusy(null);
            }
          }}
        >
          {busy === id ? (
            <Spinner className="h-3.5 w-3.5" />
          ) : (
            <Icon className="h-3.5 w-3.5" />
          )}
          Connect {label}
        </Button>
      ))}
    </div>
  );
}
