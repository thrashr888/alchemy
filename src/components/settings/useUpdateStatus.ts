import { useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { useStore } from "@/lib/store";
import { checkForUpdates, type UpdateFlow } from "@/lib/updates";

/** The version in hand, and whether a newer one exists. Shared by the
 *  Settings sidebar's identity block (which wants an answer the instant the
 *  dialog opens) and General's Version row (which drives its own explicit
 *  "Check for updates…" button) — one hook so "am I current?" is answered
 *  from the same check everywhere instead of two independent ones landing
 *  at different moments. */
export type UpdateStatus =
  | { state: "checking" }
  | { state: "current" }
  | { state: "available"; version: string }
  | { state: "error" };

// One in-flight request shared across every hook instance: the sidebar
// header and General can both mount within the same tick (opening Settings
// lands on General by default) and neither should double-hit the update
// feed for an answer the other already asked for.
let inFlight: Promise<UpdateFlow> | null = null;
function sharedCheck(): Promise<UpdateFlow> {
  if (!inFlight) inFlight = checkForUpdates().finally(() => (inFlight = null));
  return inFlight;
}

export function useUpdateStatus() {
  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion().then(setVersion).catch(() => setVersion(""));
  }, []);

  // "Check for Updates…" from the app menu lands here with the flag set;
  // the quiet startup check (store.ts) leaves `updateAvailable` behind —
  // either way this should show the result without another click.
  const pendingUpdateCheck = useStore((s) => s.pendingUpdateCheck);
  const updateAvailable = useStore((s) => s.updateAvailable);
  const [flow, setFlow] = useState<UpdateFlow | null>(null);
  const [checking, setChecking] = useState(false);
  const askedOnMount = useRef(false);

  async function recheck(): Promise<UpdateFlow> {
    setChecking(true);
    const result = await sharedCheck();
    setFlow(result);
    setChecking(false);
    if (result.status === "available")
      useStore.setState({ updateAvailable: result.version });
    if (result.status === "none") useStore.setState({ updateAvailable: null });
    return result;
  }

  useEffect(() => {
    const s = useStore.getState();
    if (s.pendingUpdateCheck || (s.updateAvailable && !flow)) {
      useStore.setState({ pendingUpdateCheck: false });
      void recheck();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingUpdateCheck, updateAvailable]);

  // The identity block wants an answer as soon as Settings opens even when
  // nothing has triggered a check yet (autoUpdateCheck off, or the 4s
  // startup delay hasn't fired) — ask once, quietly.
  useEffect(() => {
    if (!askedOnMount.current && !flow && !updateAvailable) {
      askedOnMount.current = true;
      void recheck();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const status: UpdateStatus = checking
    ? { state: "checking" }
    : flow?.status === "available"
      ? { state: "available", version: flow.version }
      : flow?.status === "error"
        ? { state: "error" }
        : updateAvailable
          ? { state: "available", version: updateAvailable }
          : flow?.status === "none"
            ? { state: "current" }
            : { state: "checking" };

  return { version, status, flow, checking, recheck };
}

/** The identity block's second line: "0.65.1 · Up to date", "0.66.0
 *  available", or "Checking…" — RFC-mac-chrome.md, Settings. */
export function updateStatusLine(version: string, status: UpdateStatus): string {
  if (status.state === "checking") return "Checking…";
  if (status.state === "available") return `${status.version} available`;
  if (status.state === "error")
    return version ? `${version} · Update check failed` : "Update check failed";
  return version ? `${version} · Up to date` : "Up to date";
}
