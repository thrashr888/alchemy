import { useState, useSyncExternalStore } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import {
  clearReindexPending,
  reindexPending,
  subscribeReindexPending,
} from "@/lib/reindex";
import { Button, LiveRegion } from "./ui";
import type { ModelStatus } from "@/lib/types";
import { AlertTriangle } from "lucide-react";

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
  tone: "error" | "warning";
  title: string;
  detail: string;
  fixes: Fix[];
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

  const check = async () => {
    setChecking(true);
    try {
      await refresh();
    } finally {
      setChecking(false);
    }
  };

  const rows: Degraded[] = [];

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
            run: () => api.openInTerminal("ollama serve").catch(() => {}),
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
              : "flex items-center gap-2.5 border-b border-warning/30 bg-warning/10 px-4 py-2 text-caption"
          }
        >
          <AlertTriangle
            aria-hidden
            className={
              row.tone === "error"
                ? "h-3.5 w-3.5 shrink-0 text-destructive"
                : "h-3.5 w-3.5 shrink-0 text-warning"
            }
          />
          <span
            className="min-w-0 flex-1 truncate"
            title={`${row.title} ${row.detail}`}
          >
            <span className="font-medium text-foreground">{row.title}</span>{" "}
            <span className="text-muted-foreground">{row.detail}</span>
          </span>
          {row.fixes.map((fix) => (
            <Button
              key={fix.label}
              size="sm"
              variant={fix.primary ? "secondary" : "ghost"}
              loading={fix.label === "Check again" && checking}
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
