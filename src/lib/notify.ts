import { api } from "./api";

/** Send a desktop notification. Routed through the backend, which owns both
 *  gates — the "Show notifications" preference and the quiet-while-focused
 *  rule — so focus is measured across every window in one place
 *  (scheduler::notifications_wanted). No-op on failure. */
export async function notify(title: string, body: string) {
  try {
    await api.sendNotification(title, body);
  } catch {
    /* notifications unavailable */
  }
}
