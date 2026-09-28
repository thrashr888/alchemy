import { useCallback, useEffect, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import type { MacProviderStatus } from "@/lib/types";
import { Badge, Button, Spinner } from "./ui";
import {
  Calendar,
  Check,
  ListChecks,
  NotebookText,
  ShieldAlert,
  TrendingUp,
} from "lucide-react";

const PROVIDERS = [
  {
    id: "calendar",
    label: "Calendar",
    icon: Calendar,
    hint: "Your events, as a syncing source.",
  },
  {
    id: "reminders",
    label: "Reminders",
    icon: ListChecks,
    hint: "Every list, kept in step.",
  },
  {
    id: "notes",
    label: "Apple Notes",
    icon: NotebookText,
    hint: "Notes come in and stay current.",
  },
  {
    id: "stocks",
    label: "Stocks",
    icon: TrendingUp,
    hint: "Your watchlist and its prices.",
  },
] as const;

/**
 * "Connect" buttons for the Mac providers (Settings → General, onboarding).
 * Each runs one benign read through cider so the macOS consent prompt fires
 * at a predictable moment — clicking Allow here means adding a Mac source
 * later just works.
 *
 * Two layouts over one set of readings: `chips` is the dense strip Settings
 * uses, `rows` is the Setup Assistant's grouped list — one row per app with
 * its status on the right. The rows variant renders bare rows and no
 * container, so the caller's inset group owns the border and the hairlines.
 */
export function MacConnect({
  onStatus,
  layout = "chips",
}: {
  /** Told which providers count as connected, each time that is read. */
  onStatus?: (connected: string[]) => void;
  layout?: "chips" | "rows";
} = {}) {
  const macAvailable = useStore((s) => s.macAvailable);
  const pushToast = useStore((s) => s.pushToast);
  const [busy, setBusy] = useState<string | null>(null);
  // Connected state is read, not remembered in the button: the store reads
  // without a prompt, so a Mac that already has Reminders notebooks shows
  // Reminders as connected instead of asking again.
  const [status, setStatus] = useState<Record<string, MacProviderStatus>>({});
  const readStatus = useCallback(async () => {
    try {
      const rows = await api.macStatus();
      setStatus(Object.fromEntries(rows.map((r) => [r.id, r])));
      onStatus?.(rows.filter((r) => r.connected).map((r) => r.id));
    } catch {
      // Without a reading the buttons simply all say Connect, as before.
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    void readStatus();
    // Once more a few seconds in: right after launch the first read can
    // land before cider's prompt-free probe has answered, and a screen
    // that says "Connect" for apps that are connected is worse than a
    // late tick. Also re-read when the window comes back — a permission
    // granted in System Settings shows up without a restart.
    const again = window.setTimeout(() => void readStatus(), 5000);
    const onFocus = () => void readStatus();
    window.addEventListener("focus", onFocus);
    return () => {
      window.clearTimeout(again);
      window.removeEventListener("focus", onFocus);
    };
  }, [readStatus]);
  // A connect failure that Full Disk Access would fix — rendered inline with
  // a button straight to the right Settings pane, not just a toast.
  const [fdaError, setFdaError] = useState<string | null>(null);

  /** One provider's consent run, shared by both layouts. */
  async function connect(id: string, label: string) {
    setBusy(id);
    try {
      await api.macConnect(id);
      setFdaError(null);
      pushToast("success", `${label} connected`);
      void readStatus();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg.includes("Full Disk Access")) setFdaError(msg);
      else pushToast("error", msg);
    } finally {
      setBusy(null);
    }
  }

  // cider is linked into the app since v0.40 — the integration always exists,
  // so the only remaining gate is the initial null while the probe resolves.
  if (!macAvailable) return null;

  if (layout === "rows")
    return (
      <>
        {PROVIDERS.map(({ id, label, icon: Icon, hint }) => {
          const row = status[id];
          return (
            <div key={id} className="flex items-center gap-3 px-3.5 py-3">
              <Icon
                aria-hidden
                className="h-[18px] w-[18px] shrink-0 text-muted-foreground"
              />
              <div className="flex min-w-0 flex-1 flex-col">
                <span className="text-body font-medium text-foreground">
                  {label}
                </span>
                <span className="truncate text-caption text-muted-foreground">
                  {row?.connected ? row.detail || hint : hint}
                </span>
              </div>
              {row?.connected ? (
                <Badge
                  title={row.detail}
                  className="border-success/30 bg-success/10 text-success"
                >
                  Connected
                </Badge>
              ) : (
                <Button
                  variant="secondary"
                  size="sm"
                  loading={busy === id}
                  disabled={busy !== null}
                  onClick={() => void connect(id, label)}
                >
                  Connect
                </Button>
              )}
            </div>
          );
        })}
        {fdaError && (
          <div className="px-3.5 py-3">
            <FdaHint message={fdaError} />
          </div>
        )}
      </>
    );

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-1.5">
        {PROVIDERS.map(({ id, label, icon: Icon }) =>
          status[id]?.connected ? (
            <span
              key={id}
              className="inline-flex h-7 items-center gap-1.5 rounded-md border border-border px-2 text-caption text-muted-foreground"
              title={status[id].detail}
            >
              <Check className="h-3.5 w-3.5 text-success" aria-hidden />
              {label} connected
            </span>
          ) : (
            <Button
              key={id}
              variant="secondary"
              size="sm"
              disabled={busy !== null}
              onClick={() => void connect(id, label)}
            >
              {busy === id ? (
                <Spinner className="h-3.5 w-3.5" />
              ) : (
                <Icon className="h-3.5 w-3.5" />
              )}
              Connect {label}
            </Button>
          ),
        )}
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
