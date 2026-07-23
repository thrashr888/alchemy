import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/**
 * Adherence to the macOS Accessibility text size. WKWebView ignores it, so the
 * native layer (src-tauri/src/textsize.rs) reads the effective Dynamic Type
 * size and hands us a scale factor. We fold it into `--system-text-scale` on
 * <html>; index.css multiplies it into the root font-size so all rem type
 * scales. Default (unset) is 1 — index.css already falls back to 1, so an app
 * at the default size renders pixel-identical.
 *
 * main.tsx is the shared entry for the main window and every `new_window`
 * pop-out, so calling init here covers them all with no per-window wiring.
 */

const VAR = "--system-text-scale";

function apply(scale: number): void {
  if (!Number.isFinite(scale) || scale <= 0) return;
  document.documentElement.style.setProperty(VAR, String(scale));
}

let started = false;

/** Query the native scale once at boot, then track changes broadcast on focus. */
export function initTextScale(): void {
  if (started) return;
  started = true;
  void invoke<number>("get_system_text_scale").then(apply).catch(() => {});
  void listen<number>("ui://text-scale", (e) => apply(e.payload));
}
