import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

let granted: boolean | null = null;

/** Send a desktop notification, requesting permission once. No-op on failure.
 *  Gated by the "Show notifications" preference (Settings → General). */
export async function notify(title: string, body: string) {
  if (localStorage.getItem("showNotifications") === "false") return;
  try {
    if (granted === null) {
      granted = await isPermissionGranted();
      if (!granted) granted = (await requestPermission()) === "granted";
    }
    if (granted) sendNotification({ title, body });
  } catch {
    /* notifications unavailable */
  }
}
