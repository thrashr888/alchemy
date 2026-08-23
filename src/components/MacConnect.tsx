import { useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Button, Spinner } from "./ui";
import {
  Calendar,
  ListChecks,
  NotebookText,
  ShieldAlert,
  TrendingUp,
} from "lucide-react";

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
 * later just works.
 */
export function MacConnect() {
  const macAvailable = useStore((s) => s.macAvailable);
  const pushToast = useStore((s) => s.pushToast);
  const [busy, setBusy] = useState<string | null>(null);
  // A connect failure that Full Disk Access would fix — rendered inline with
  // a button straight to the right Settings pane, not just a toast.
  const [fdaError, setFdaError] = useState<string | null>(null);

  // cider is linked into the app since v0.40 — the integration always exists,
  // so the only remaining gate is the initial null while the probe resolves.
  if (!macAvailable) return null;

  return (
    <div className="flex flex-col gap-2">
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
                setFdaError(null);
                pushToast("success", `${label} connected`);
              } catch (e) {
                const msg = e instanceof Error ? e.message : String(e);
                if (msg.includes("Full Disk Access")) setFdaError(msg);
                else pushToast("error", msg);
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
      {fdaError && <FdaHint message={fdaError} />}
    </div>
  );
}

/** Inline Full-Disk-Access fix-it: the instruction plus a button that opens
 *  System Settings directly on the right pane. */
export function FdaHint({ message }: { message: string }) {
  return (
    <div className="flex flex-col gap-2 rounded-md border border-border bg-surface-2/40 px-3 py-2.5">
      <div className="flex items-start gap-2 text-caption leading-relaxed text-foreground/90">
        <ShieldAlert className="mt-0.5 h-3.5 w-3.5 shrink-0 text-warning" />
        <span>{message}</span>
      </div>
      <div>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => void api.openPrivacySettings()}
        >
          Open Privacy Settings
        </Button>
      </div>
    </div>
  );
}
