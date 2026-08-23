import * as React from "react";
import { AlertTriangle, Check, Copy, FolderOpen, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui";
import {
  describeThrown,
  getFatal,
  onFatal,
  report,
  restart,
  revealLog,
  type FatalState,
} from "@/lib/diagnostics";

/**
 * The recovery surface (docs/RFC-diagnostics.md). Two things can leave
 * Alchemy unusable: a React tree that throws on render, and a backend panic.
 * Both land here, and both get the same answer — say what happened, and give
 * the user a way out that isn't force-quitting from the Dock.
 *
 * The screen deliberately does not use the app's Modal: a modal renders
 * inside the tree that just failed.
 */

function Recovery({
  fatal,
  onRetry,
}: {
  fatal: FatalState;
  onRetry?: () => void;
}) {
  const [copied, setCopied] = React.useState(false);

  const details = [
    `Alchemy — ${fatal.origin === "rust" ? "backend" : "interface"} error`,
    `kind: ${fatal.kind}`,
    `message: ${fatal.message}`,
    fatal.detail ? `\n${fatal.detail}` : "",
  ].join("\n");

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(details);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch (err) {
      report("warn", "copy-diagnostics", describeThrown(err).message);
    }
  };

  return (
    <div
      role="alert"
      className="fixed inset-0 z-[1000] flex items-center justify-center bg-surface p-8"
    >
      <div className="w-full max-w-[440px] rounded-lg border border-border bg-surface-2 p-6">
        <div className="flex items-center gap-2 text-destructive">
          <AlertTriangle className="h-4 w-4" />
          <span className="text-body font-medium">Alchemy hit a problem</span>
        </div>

        <p className="mt-3 text-body text-muted-foreground">
          {fatal.origin === "rust"
            ? "Something failed in the background and Alchemy can’t continue safely. Restarting picks up where you left off — your notebooks are on disk, not in memory."
            : "This window stopped rendering. Your notebooks are safe on disk; reloading rebuilds the interface from them."}
        </p>

        <div className="mt-4 rounded-md border border-border bg-surface p-3">
          <div className="text-caption text-subtle-foreground">
            {fatal.kind}
          </div>
          <div className="mt-1 font-mono text-caption break-words text-foreground">
            {fatal.message || "No message was reported."}
          </div>
        </div>

        <div className="mt-5 flex flex-wrap items-center gap-2">
          <Button variant="primary" onClick={() => void restart()}>
            Restart Alchemy
          </Button>
          {onRetry && (
            <Button variant="secondary" onClick={onRetry}>
              <RotateCcw className="h-3.5 w-3.5" />
              Try again
            </Button>
          )}
          <Button variant="ghost" onClick={() => void copy()}>
            {copied ? (
              <Check className="h-3.5 w-3.5" />
            ) : (
              <Copy className="h-3.5 w-3.5" />
            )}
            {copied ? "Copied" : "Copy details"}
          </Button>
          <Button variant="ghost" onClick={() => void revealLog()}>
            <FolderOpen className="h-3.5 w-3.5" />
            Show log
          </Button>
        </div>

        <p className="mt-4 text-caption text-subtle-foreground">
          The full error, with a backtrace, is in the log — send it along if
          you report this.
        </p>
      </div>
    </div>
  );
}

/**
 * Watches for backend fatals raised through `diagnostics.raiseFatal`. Mounted
 * beside the app rather than around it, so a backend panic doesn't unmount
 * the interface — if the user dismisses nothing and restarts, that's fine,
 * but a window that still paints is easier to reason about than a blank one.
 */
export function FatalOverlay() {
  const [fatal, setFatal] = React.useState<FatalState | null>(() => getFatal());
  React.useEffect(() => onFatal(setFatal), []);
  if (!fatal) return null;
  return <Recovery fatal={fatal} />;
}

interface BoundaryProps {
  children: React.ReactNode;
}

interface BoundaryState {
  error: Error | null;
  /** Bumped on retry so the subtree remounts instead of reusing dead state. */
  attempt: number;
}

/**
 * Catches render-time throws anywhere below it. Without this, a single bad
 * component unmounts the whole React tree and the user is left with a white
 * window and no way to tell us why.
 */
export class ErrorBoundary extends React.Component<
  BoundaryProps,
  BoundaryState
> {
  state: BoundaryState = { error: null, attempt: 0 };

  static getDerivedStateFromError(error: Error): Partial<BoundaryState> {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    // Logged, not raised through `raiseFatal`: this boundary renders its own
    // recovery screen, and routing it through the global fatal state as well
    // would stack a second copy on top of it.
    report(
      "fatal",
      "render",
      error.message || error.name,
      // The component stack says which component threw; the JS stack says
      // where in it. Both matter, and neither is derivable from the other.
      [error.stack, info.componentStack].filter(Boolean).join("\n\n"),
    );
  }

  render() {
    if (this.state.error) {
      return (
        <Recovery
          fatal={{
            origin: "js",
            kind: "render",
            message: this.state.error.message || this.state.error.name,
            detail: this.state.error.stack,
          }}
          onRetry={() =>
            this.setState((s) => ({ error: null, attempt: s.attempt + 1 }))
          }
        />
      );
    }
    return (
      <React.Fragment key={this.state.attempt}>
        {this.props.children}
      </React.Fragment>
    );
  }
}
