import { ChevronLeft, ChevronRight } from "lucide-react";
import { useStore } from "@/lib/store";
import { Button } from "./ui";

/** App-level browser history. Reader-local history remains in ReaderPane. */
export function AppNavigationControls() {
  const canGoBack = useStore((s) => s.nav.index > 0);
  const canGoForward = useStore((s) => s.nav.index < s.nav.stack.length - 1);
  const goBack = useStore((s) => s.navBack);
  const goForward = useStore((s) => s.navForward);

  return (
    <nav
      aria-label="History navigation"
      className="flex shrink-0 items-center gap-0.5"
    >
      {/* The wrapper keeps the native tooltip available while the button is
          disabled (Button deliberately turns pointer events off then). */}
      <span className="inline-flex" title="Back (⌘←)">
        <Button
          variant="ghost"
          size="icon"
          onClick={goBack}
          disabled={!canGoBack}
          aria-label="Back"
          aria-keyshortcuts="Meta+ArrowLeft"
        >
          <ChevronLeft aria-hidden className="h-4 w-4" />
        </Button>
      </span>
      <span className="inline-flex" title="Forward (⌘→)">
        <Button
          variant="ghost"
          size="icon"
          onClick={goForward}
          disabled={!canGoForward}
          aria-label="Forward"
          aria-keyshortcuts="Meta+ArrowRight"
        >
          <ChevronRight aria-hidden className="h-4 w-4" />
        </Button>
      </span>
    </nav>
  );
}
