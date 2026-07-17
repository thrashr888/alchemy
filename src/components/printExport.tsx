import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";

/** One print operation per window, ever — they serialize on the main thread. */
let printInFlight = false;

/**
 * One-click PDF export, fully local: pick a destination in the native save
 * dialog, then the backend runs a silent NSPrintSaveJob over the print-only
 * DOM (same pagination as real printing — see print_webview). While
 * `printing` is true the caller renders a <PrintPortal> with the print
 * layout; the export starts once it's in the DOM, and the saved file is
 * revealed in Finder.
 */
export function usePrintExport(options?: {
  /** Landscape pages (slide decks); flashcard sheets stay portrait. */
  landscape?: boolean;
  /** Suggested filename, without extension. */
  suggestedName?: string;
}): { printing: boolean; exportPdf: () => void } {
  const landscape = options?.landscape ?? false;
  const suggestedName = options?.suggestedName ?? "Export";
  const [savePath, setSavePath] = useState<string | null>(null);

  useEffect(() => {
    if (!savePath) return;
    let cancelled = false;
    // Two frames: one for the portal to mount, one for layout/autofit.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        // Checked HERE, not just in the cleanup: StrictMode runs the effect
        // twice in dev, and an invoke fired from a cancelled run would print
        // a second time — after the portal unmounts, that second operation
        // grinds through paginating the ENTIRE app DOM on the main thread.
        if (cancelled || printInFlight) return;
        printInFlight = true;
        void invoke("print_webview", { landscape, savePath })
          .then(() => {
            useStore.getState().pushToast("success", "PDF saved");
            void revealItemInDir(savePath);
          })
          .catch((err) => {
            useStore.getState().pushToast("error", `PDF export failed: ${err}`);
          })
          .finally(() => {
            printInFlight = false;
            if (!cancelled) setSavePath(null);
          });
      });
    });
    return () => {
      cancelled = true;
    };
  }, [savePath, landscape]);

  const exportPdf = () => {
    void save({
      defaultPath: `${suggestedName}.pdf`,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    }).then((path) => {
      if (path) setSavePath(path);
    });
  };

  return { printing: savePath !== null, exportPdf };
}

/**
 * Print-only layout rendered straight under <body>. It stays genuinely
 * visible as an opaque overlay while the export runs — WKWebView's print
 * operation paints hidden or off-screen content as blank pages — and print
 * media swaps it in for the app entirely (see "PDF export" in index.css).
 * `pageCss` sets the @page box for this export.
 */
export function PrintPortal({
  pageCss,
  children,
}: {
  pageCss: string;
  children: React.ReactNode;
}) {
  return createPortal(
    <div className="print-surface">
      <style>{pageCss}</style>
      {children}
    </div>,
    document.body,
  );
}
