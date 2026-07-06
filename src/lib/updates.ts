/** Auto-update checks via the Tauri updater plugin, fed by the signed
 *  `latest.json` each GitHub release publishes. */

import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export function autoUpdateEnabled(): boolean {
  return localStorage.getItem("autoUpdateCheck") !== "false";
}

/** Quiet startup check: surface availability as a toast, never interrupt. */
export async function checkForUpdatesQuietly(onFound: (message: string) => void) {
  try {
    const update = await check();
    if (update) {
      onFound(`Alchemy ${update.version} is available — install from Settings → General.`);
    }
  } catch {
    /* offline or endpoint unavailable — try again next launch */
  }
}

export type UpdateFlow =
  | { status: "none" }
  | { status: "error"; message: string }
  | { status: "available"; version: string; install: () => Promise<void> };

/** Interactive check for the Settings button: report every outcome. */
export async function checkForUpdates(): Promise<UpdateFlow> {
  try {
    const update = await check();
    if (!update) return { status: "none" };
    return {
      status: "available",
      version: update.version,
      install: async () => {
        await update.downloadAndInstall();
        await relaunch();
      },
    };
  } catch (e) {
    return { status: "error", message: e instanceof Error ? e.message : String(e) };
  }
}
