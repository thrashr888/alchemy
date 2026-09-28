import { ChevronLeft, ChevronRight } from "lucide-react";
import { useStore } from "@/lib/store";
import { CHROME_BUTTON } from "./SidebarRails";

/** Browser-style back/forward for the app-level location history
 *  (`nav` in the store; see `applyNav` and the location subscriber there).
 *
 *  Sits at the far left of every window header, immediately right of the
 *  traffic lights — the Finder/Safari position, which is the only place a
 *  Mac user looks for these. Both ends of the stack disable rather than
 *  disappear, so the pair never reflows the rest of the header. 2px apart
 *  and on the chrome's icon-button spec (docs/RFC-mac-chrome.md, "Toolbar"),
 *  so the two read as one segmented pair rather than two loose keys. */
export function NavButtons() {
  const nav = useStore((s) => s.nav);
  const navBack = useStore((s) => s.navBack);
  const navForward = useStore((s) => s.navForward);

  const canBack = nav.index > 0;
  const canForward = nav.index < nav.stack.length - 1;

  return (
    <div className="flex shrink-0 items-center gap-0.5">
      <button
        type="button"
        disabled={!canBack}
        onClick={navBack}
        title="Back (⌘[)"
        aria-label="Back"
        className={CHROME_BUTTON}
      >
        <ChevronLeft className="h-4 w-4" />
      </button>
      <button
        type="button"
        disabled={!canForward}
        onClick={navForward}
        title="Forward (⌘])"
        aria-label="Forward"
        className={CHROME_BUTTON}
      >
        <ChevronRight className="h-4 w-4" />
      </button>
    </div>
  );
}
