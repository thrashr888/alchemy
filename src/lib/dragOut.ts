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

/** The format to drag a note out as: its kind-true one, so a poster leaves
 *  as a PNG and a deck as a deck. Falls back to the Word document.
 *
 *  Posters and mind maps render through the print pipeline, which opens a
 *  real window and takes a moment — WKWebView prints never-composited
 *  content as blank pages, so it cannot be hidden. That cost lands on the
 *  first drag of a given revision only; the backend caches the rendered file
 *  per note edit, so afterwards the drag is immediate.
 */
export function dragFormat(formats: string[]): string {
  return formats[0] ?? "docx";
}

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
