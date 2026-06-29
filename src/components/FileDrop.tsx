import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useStore } from "@/lib/store";
import { FileDown } from "lucide-react";

const SUPPORTED = [
  "pdf", "txt", "text", "md", "markdown",
  "docx", "pptx", "xlsx", "xls", "xlsm", "ods",
  "png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff", "heic",
];

function ext(path: string): string {
  const i = path.lastIndexOf(".");
  return i === -1 ? "" : path.slice(i + 1).toLowerCase();
}

/**
 * Full-window drop target. Tauri delivers native OS file drops via the webview
 * drag-drop event (HTML5 dnd is suppressed when that's enabled), so we listen
 * there and route dropped paths straight into the active notebook's ingest.
 */
export function FileDrop() {
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let active = true;

    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === "enter" || p.type === "over") {
          setDragging(true);
        } else if (p.type === "leave") {
          setDragging(false);
        } else if (p.type === "drop") {
          setDragging(false);
          void handleDrop(p.paths);
        }
      })
      .then((fn) => {
        if (active) unlisten = fn;
        else fn();
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  async function handleDrop(paths: string[]) {
    const { currentId, addSourceFiles, setError } = useStore.getState();
    if (!currentId) {
      setError("Select or create a notebook before adding sources.");
      return;
    }
    const supported = paths.filter((p) => SUPPORTED.includes(ext(p)));
    if (supported.length === 0) {
      setError("Unsupported file type. Drop PDF, text, or Markdown files.");
      return;
    }
    await addSourceFiles(supported);
  }

  if (!dragging) return null;

  return (
    <div className="pointer-events-none fixed inset-0 z-[60] flex items-center justify-center bg-background/85">
      <div className="flex flex-col items-center gap-3 rounded-lg border-2 border-dashed border-primary/60 bg-elevated px-12 py-10 shadow-xl">
        <div className="flex h-14 w-14 items-center justify-center rounded-md bg-primary/15 text-primary">
          <FileDown className="h-7 w-7" />
        </div>
        <div className="text-[15px] font-semibold text-foreground">Drop to add sources</div>
        <div className="text-[12.5px] text-muted-foreground">
          PDF · Office · images (OCR) · text
        </div>
      </div>
    </div>
  );
}
