// Drag a note out of Alchemy as a real file (RFC-professional-grade Pillar 6).
//
// Deliberately NOT the HTML5 drag API. A webview drag can only offer what
// the pasteboard already holds, and Finder wants a file — so the drag is
// opened natively (dragout.rs) and the browser's own drag must stay out of
// the way (`draggable={false}` on the handle).
//
// Two phases, because AppKit attaches the session to the live mouse event:
// staging renders the export (a PDF or a deck is not instant) on mouse-down,
// and the native drag opens once the pointer has actually moved. Rendering
// inside the gesture would leave AppKit holding a stale event and no session.

import { invoke, isTauri } from "@tauri-apps/api/core";

/** Pointer travel before a press becomes a drag — the AppKit default, and
 *  far enough that a click on a note tile is never mistaken for one. */
const THRESHOLD_PX = 3;

/** Formats whose export is pure background work (export.rs funnels each
 *  through spawn_blocking).
 *
 *  `pdf` and `png` are deliberately absent: both render through
 *  `print_note_pdf`, which opens a real on-screen window — WKWebView prints
 *  never-composited content as blank pages, so the export window cannot be
 *  hidden — and waits up to 60s for the file to settle. Perfectly fine
 *  behind a Save dialog the user is already waiting on; catastrophic inside
 *  a drag, where it would throw a window up mid-gesture and stall the drag
 *  behind a print job. */
const DRAG_SAFE = new Set(["docx", "xlsx", "pptx", "m4a"]);

/** The format to drag a note out as, given its kind-true export targets.
 *  Falls back to the Word document, which every note can produce without
 *  touching the print pipeline. */
export function dragFormat(formats: string[]): string {
  return formats.find((f) => DRAG_SAFE.has(f)) ?? "docx";
}

/**
 * Wire a note tile for drag-out. Returns props to spread onto the element;
 * outside Tauri (the browser dev build) it returns nothing and the element
 * behaves normally.
 */
export function noteDragProps(
  noteId: string,
  format: string,
  onError?: (message: string) => void,
): {
  draggable?: boolean;
  "data-drag-out"?: boolean;
  onMouseDown?: (e: React.MouseEvent) => void;
} {
  if (!isTauri()) return {};
  return {
    // The native session owns this gesture; an HTML5 drag racing it would
    // hand the receiver a text/uri-list nobody asked for.
    draggable: false,
    // Tells the marquee to keep its hands off this row (ui.tsx). Dragging
    // an item drags the item; bands start on background. That is the Finder
    // rule, and a row can only mean one of the two.
    "data-drag-out": true,
    onMouseDown: (e: React.MouseEvent) => {
      if (e.button !== 0) return;
      const startX = e.clientX;
      const startY = e.clientY;
      let started = false;

      const onMove = (m: MouseEvent) => {
        if (
          started ||
          (Math.abs(m.clientX - startX) < THRESHOLD_PX &&
            Math.abs(m.clientY - startY) < THRESHOLD_PX)
        )
          return;
        started = true;
        cleanup();
        // Export only once the press is unambiguously a drag. Staging on
        // mouse-down instead would re-render the note on every click that
        // merely opens it.
        void invoke<string>("stage_note_for_drag", { noteId, format })
          .then((path) => invoke("start_file_drag", { path }))
          .catch((err) => onError?.(String(err)));
      };
      const cleanup = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", cleanup);
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", cleanup);
    },
  };
}
