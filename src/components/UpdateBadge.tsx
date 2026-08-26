import { ArrowUpCircle } from "lucide-react";
import { useStore } from "@/lib/store";

/** Title-bar update notice, beside the DEV chip: the quiet startup check
 *  (`checkForUpdatesQuietly`) sets `updateAvailable`, and this is the notice
 *  that stays put after the launch toast has gone. Clicking it opens
 *  Settings → General with the check already queued, so the Install button
 *  is right there. Renders nothing when the app is current. */
export function UpdateBadge() {
  const version = useStore((s) => s.updateAvailable);
  if (!version) return null;
  return (
    <button
      type="button"
      onClick={() => {
        useStore.setState({ pendingUpdateCheck: true });
        useStore.getState().openSettings("general");
      }}
      title={`Alchemy ${version} is available — click to review and install`}
      aria-label={`Update to Alchemy ${version}`}
      className="mr-1 flex select-none items-center gap-1 rounded-full border border-primary/40 bg-primary/10 px-2 py-0.5 text-badge font-semibold tracking-wide text-citation transition-colors hover:bg-primary/20"
    >
      <ArrowUpCircle aria-hidden className="h-3 w-3" />
      {version}
    </button>
  );
}
