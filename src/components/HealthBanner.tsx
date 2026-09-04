import { useEffect, useState, useSyncExternalStore } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import {
  clearReindexPending,
  reindexPending,
  subscribeReindexPending,
} from "@/lib/reindex";
import { Button, LiveRegion } from "./ui";
import type { IcloudMoveOffer, ModelStatus } from "@/lib/types";
import { AlertTriangle, HardDrive, LifeBuoy } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { crashNotice, revealLog, type CrashNotice } from "@/lib/diagnostics";

/**
 * The degraded states, designed.
 *
 * Three things can leave the app running but unable to do its job: the local
 * engine is not up, a role has no usable model, or a search-index rebuild
 * never finished. Each gets a sentence saying what stopped working and a
 * button that fixes it right here — a Terminal launch for the two commands
 * the backend allowlists (`ollama serve`, `ollama pull <model>`), a jump to
 * the Models pane when the fix is a choice, a rebuild when the index is the
 * problem. Error prose with no way out is the thing this replaces.
 *
 * Tone follows the stakes: destructive when nothing can be answered, warning
 * when the app works but is quietly returning less than it should.
 */

/** One action button in a banner row. */
interface Fix {
  label: string;
  run: () => void | Promise<void>;
  primary?: boolean;
}

interface Degraded {
  key: string;
  /** "offer" is an invitation, not a problem: hairline, no tint, no alarm. */
  tone: "error" | "warning" | "offer";
  title: string;
  detail: string;
  fixes: Fix[];
  /** Overrides the tone's default glyph where the row isn't about storage. */
  icon?: typeof HardDrive;
}

/** The model name in "Not installed — run `ollama pull qwen3`". */
function pullTarget(detail: string): string | null {
  return /run `ollama pull ([^`]+)`/.exec(detail)?.[1] ?? null;
}

export function HealthBanner({
  onOpenSettings,
}: {
  onOpenSettings: () => void;
}) {
  const health = useStore((s) => s.modelHealth);
  const refresh = useStore((s) => s.refreshModelHealth);
  const reembedAll = useStore((s) => s.reembedAll);
  const pending = useSyncExternalStore(subscribeReindexPending, reindexPending);
  const [checking, setChecking] = useState(false);
  const aiConfig = useStore((s) => s.aiConfig);
  const notebooks = useStore((s) => s.notebooks);
  const pushToast = useStore((s) => s.pushToast);
  const [binding, setBinding] = useState(false);
  // The container migration (RFC-okf-live §5.7, stage two). Asked once, and
  // only when the backend says there is something to move — the answer needs
  // an entitlement check and a look at the bindings, so it is a command, not
  // a config flag the frontend can read on its own.
  const [icloud, setIcloud] = useState<IcloudMoveOffer | null>(null);
  const [moving, setMoving] = useState(false);
  useEffect(() => {
    let alive = true;
    void api
      .icloudContainerOffer()
      .then((offer) => {
        if (alive) setIcloud(offer);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  // A crash macOS recorded for the previous run (docs/RFC-diagnostics.md,
  // "As built"). The backend finds it on a background thread after the window
  // is up, so this asks twice: once on mount, once after the scan has had
  // time to finish. Dismissal is local — the notice is already only shown on
  // the one launch that follows the crash.
  const [crash, setCrash] = useState<CrashNotice | null>(null);
  const [crashDismissed, setCrashDismissed] = useState(false);
  useEffect(() => {
    let alive = true;
    const ask = () => {
      void crashNotice().then((notice) => {
        if (alive && notice) setCrash(notice);
      });
    };
    ask();
    const later = window.setTimeout(ask, 6000);
    return () => {
      alive = false;
      window.clearTimeout(later);
    };
  }, []);

  // Progress while the whole shelf is bound, so a slow pass says what it is
  // doing rather than sitting on a spinner.
  const [bindingAt, setBindingAt] = useState("");
  useEffect(() => {
    let alive = true;
    const un = listen<{ done: number; total: number; title: string }>(
      "okf://binding",
      (e) => {
        if (!alive) return;
        const p = e.payload;
        setBindingAt(p.title ? `${p.title} (${p.done + 1}/${p.total})` : "");
      },
    );
    return () => {
      alive = false;
      void un.then((f) => f());
    };
  }, []);

  const check = async () => {
    setChecking(true);
    try {
      await refresh();
    } finally {
      setChecking(false);
    }
  };

  const rows: Degraded[] = [];

  // One line, no alarm: the crash is over, the app is running, and the only
  // useful action is getting at the report. Anything louder would be a banner
  // about something the user already lived through.
  if (crash && !crashDismissed) {
    rows.push({
      key: "crash",
      tone: "offer",
      icon: LifeBuoy,
      title: "Alchemy crashed last time.",
      detail: "The report is in the log.",
      fixes: [
        { label: "Reveal", run: () => revealLog() },
        { label: "Dismiss", run: () => setCrashDismissed(true) },
      ],
    });
  }

  // The one-time offer (docs/RFC-okf-live.md §5.7). Existing notebooks
  // predate the folder, so they get asked once — either button answers it,
  // and the ⋯ menu's verb stays the way in afterwards.
  if (aiConfig && !aiConfig.keepOnDiskAsked && notebooks.some((n) => !n.status)) {
    rows.push({
      key: "keep-on-disk",
      tone: "offer",
      title: "Keep your notebooks on disk?",
      detail: binding
        ? `Each becomes a folder of markdown in ${binding}. Put that in iCloud Drive and your Macs stay in step.`
        : "Each becomes a folder of markdown you can read, search, and sync.",
      fixes: [
        {
          label: "Keep on disk",
          primary: true,
          run: async () => {
            setBinding(true);
            try {
              const bound = await api.keepNotebooksOnDisk();
              // Starter notebooks stay off disk (RFC-okf-live.md §5.7), so a
              // fresh install binds nothing here and the sentence has to
              // still be true.
              pushToast(
                "success",
                bound === 0
                  ? "New notebooks will be kept on disk."
                  : bound === 1
                    ? "1 notebook is now kept on disk."
                    : `${bound} notebooks are now kept on disk.`,
              );
            } catch (err) {
              pushToast("error", err instanceof Error ? err.message : String(err));
            } finally {
              setBinding(false);
              void api.getAiConfig().then((c) => useStore.setState({ aiConfig: c }));
            }
          },
        },
        {
          label: "Not now",
          run: async () => {
            await api.dismissKeepOnDiskOffer().catch(() => {});
            void api.getAiConfig().then((c) => useStore.setState({ aiConfig: c }));
          },
        },
      ],
    });
  }

  // Stage two of the same section: Alchemy now has a folder of its own at the
  // iCloud Drive root, and the notebooks in the plain folder can move into
  // it. Same tone as the offer above, because it is the same kind of thing —
  // an invitation, answered once either way.
  if (icloud?.available) {
    rows.push({
      key: "icloud-container",
      tone: "offer",
      title: "Move your notebooks to the Alchemy folder?",
      detail: `Alchemy has its own folder in iCloud Drive now. Your ${icloud.count} ${
        icloud.count === 1 ? "notebook" : "notebooks"
      }${
        icloud.others > 0
          ? ` and ${icloud.others} other ${icloud.others === 1 ? "bundle" : "bundles"}`
          : ""
      } ${icloud.count === 1 && icloud.others === 0 ? "moves" : "move"} there. Nothing is deleted.`,
      fixes: [
        {
          label: "Move them",
          primary: true,
          run: async () => {
            setMoving(true);
            try {
              const moved = await api.moveNotebooksToIcloudContainer();
              pushToast(
                "success",
                `${moved} ${moved === 1 ? "notebook" : "notebooks"} moved to the Alchemy folder.`,
              );
              setIcloud(null);
            } catch (err) {
              pushToast("error", err instanceof Error ? err.message : String(err));
            } finally {
              setMoving(false);
              void api.getAiConfig().then((c) => useStore.setState({ aiConfig: c }));
            }
          },
        },
        {
          label: "Not now",
          run: async () => {
            await api.dismissIcloudContainerOffer().catch(() => {});
            setIcloud(null);
            void api.getAiConfig().then((c) => useStore.setState({ aiConfig: c }));
          },
        },
      ],
    });
  }

  // An unfinished rebuild is first: it is the one state where everything
  // looks fine and the answers are quietly thinner.
  if (pending) {
    rows.push({
      key: "reindex",
      tone: "warning",
      title: "The search index is incomplete.",
      detail:
        "A re-index didn't finish, so some sources won't appear in search or citations.",
      fixes: [
        {
          label: "Rebuild now",
          primary: true,
          run: async () => {
            await reembedAll();
            // reembedAll reports failure through store.error rather than
            // throwing, so a second failed run leaves the banner standing.
            if (!useStore.getState().error) clearReindexPending();
          },
        },
        { label: "Ignore", run: clearReindexPending },
      ],
    });
  }

  if (health) {
    // Ollama down takes the whole banner: naming each broken role separately
    // would say the same thing twice with one cause.
    const broken = !health.chat.working || !health.embed.working;
    if (broken && !health.reachable) {
      rows.push({
        key: "ollama",
        tone: "error",
        title: "Ollama isn't running.",
        detail: "Alchemy needs it to answer questions and to index sources.",
        fixes: [
          {
            label: "Start Ollama",
            primary: true,
            run: async () => {
              const store = useStore.getState();
              try {
                const via = await api.startOllama();
                store.pushToast(
                  "info",
                  via === "app"
                    ? "Starting the Ollama app…"
                    : "Started `ollama serve` in the background.",
                );
                // The server needs a moment to bind before a probe means
                // anything; "Check again" is still there if it misses.
                window.setTimeout(() => void check(), 2500);
              } catch (err) {
                store.pushToast(
                  "error",
                  err instanceof Error ? err.message : String(err),
                );
              }
            },
          },
          { label: "Check again", run: check },
        ],
      });
    } else {
      if (!health.chat.working) rows.push(roleRow("chat", health.chat));
      if (!health.embed.working) rows.push(roleRow("embed", health.embed));
    }
  }

  function roleRow(role: "chat" | "embed", status: ModelStatus): Degraded {
    const pull = pullTarget(status.detail);
    const unset = !status.name.trim();
    const title = unset
      ? role === "chat"
        ? "No chat model set."
        : "No search model set."
      : pull
        ? `${role === "chat" ? "The chat model" : "The search model"} isn't installed.`
        : role === "chat"
          ? "Chat can't answer."
          : "Sources can't be indexed.";
    const detail = unset
      ? "Pick one in Settings and this works again."
      : pull
        ? role === "chat"
          ? `Install ${pull} and chat can answer again.`
          : `Install ${pull} and new sources index again.`
        : status.detail;
    const fixes: Fix[] = [];
    if (pull) {
      fixes.push({
        label: `Install ${pull}`,
        primary: true,
        run: () => api.openInTerminal(`ollama pull ${pull}`).catch(() => {}),
      });
      fixes.push({ label: "Check again", run: check });
    } else {
      fixes.push({
        label: unset ? "Choose a model" : "Open Settings",
        primary: true,
        run: onOpenSettings,
      });
      if (!unset) fixes.push({ label: "Check again", run: check });
    }
    return { key: role, tone: "error", title, detail, fixes };
  }

  // Same discipline as the toaster: the region is mounted whether or not
  // anything is wrong, so a problem that appears mid-session is spoken. The
  // rows themselves are not the live region — their fix buttons would be read
  // out with every announcement.
  const announcer = (
    <LiveRegion
      announcements={rows.map((r) => ({
        id: r.key,
        text: `${r.title} ${r.detail}`,
      }))}
    />
  );
  if (rows.length === 0) return announcer;

  return (
    <div className="flex flex-col">
      {announcer}
      {rows.map((row) => (
        <div
          key={row.key}
          className={
            row.tone === "error"
              ? "flex items-center gap-2.5 border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-caption"
              : row.tone === "warning"
                ? "flex items-center gap-2.5 border-b border-warning/30 bg-warning/10 px-4 py-2 text-caption"
                : "flex items-center gap-2.5 border-b border-border px-4 py-2 text-caption"
          }
        >
          {row.tone === "offer" ? (
            (() => {
              const Glyph = row.icon ?? HardDrive;
              return (
                <Glyph
                  aria-hidden
                  className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
                />
              );
            })()
          ) : (
            <AlertTriangle
              aria-hidden
              className={
                row.tone === "error"
                  ? "h-3.5 w-3.5 shrink-0 text-destructive"
                  : "h-3.5 w-3.5 shrink-0 text-warning"
              }
            />
          )}
          <span
            className="min-w-0 flex-1 truncate"
            title={`${row.title} ${row.detail}`}
          >
            <span className="font-medium text-foreground">{row.title}</span>{" "}
            <span className="text-muted-foreground">
              {row.key === "keep-on-disk" && bindingAt
                ? `Keeping ${bindingAt}…`
                : row.detail}
            </span>
          </span>
          {row.fixes.map((fix) => (
            <Button
              key={fix.label}
              size="sm"
              variant={fix.primary ? "secondary" : "ghost"}
              loading={
              (fix.label === "Check again" && checking) ||
              (fix.label === "Keep on disk" && binding) ||
              (fix.label === "Move them" && moving)
            }
              onClick={() => void fix.run()}
            >
              {fix.label}
            </Button>
          ))}
        </div>
      ))}
    </div>
  );
}
