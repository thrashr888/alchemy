import { ChevronLeft, ChevronRight } from "lucide-react";
import { useStore } from "@/lib/store";
import { Button } from "./ui";

/** Browser-style back/forward for the app-level location history
 *  (`nav` in the store; see `applyNav` and the location subscriber there).
 *
 *  Sits at the far left of every window header, immediately right of the
 *  traffic lights — the Finder/Safari position, which is the only place a
 *  Mac user looks for these. Both ends of the stack disable rather than
 *  disappear, so the pair never reflows the rest of the header. */
export function NavButtons() {
  const nav = useStore((s) => s.nav);
  const navBack = useStore((s) => s.navBack);
  const navForward = useStore((s) => s.navForward);

  const canBack = nav.index > 0;
  const canForward = nav.index < nav.stack.length - 1;

  return (
    <div className="flex shrink-0 items-center">
      <Button
        variant="ghost"
        size="icon"
        disabled={!canBack}
        onClick={navBack}
        title="Back (⌘[)"
        aria-label="Back"
      >
        <ChevronLeft className="h-4 w-4" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        disabled={!canForward}
        onClick={navForward}
        title="Forward (⌘])"
        aria-label="Forward"
      >
        <ChevronRight className="h-4 w-4" />
      </Button>
    </div>
  );
}
