import { useEffect } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useStore } from "@/lib/store";
import { SUPPORTED_EXTENSIONS } from "@/lib/utils";

function ext(path: string): string {
  const i = path.lastIndexOf(".");
  return i === -1 ? "" : path.slice(i + 1).toLowerCase();
}

/**
 * Listens for native OS file drops (Tauri suppresses HTML5 dnd when its own
 * drag-drop is enabled) and routes dropped paths into the active notebook.
 * The drop affordance itself is rendered on the Sources panel via the
 * `draggingFiles` store flag this sets.
 */
export function FileDrop() {
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let active = true;

    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        const { setDraggingFiles } = useStore.getState();
        if (p.type === "enter" || p.type === "over") {
          setDraggingFiles(true);
        } else if (p.type === "leave") {
          setDraggingFiles(false);
        } else if (p.type === "drop") {
          setDraggingFiles(false);
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

  return null;
}

async function handleDrop(paths: string[]) {
  const { currentId, addSourceFiles, setError } = useStore.getState();
  if (!currentId) {
    setError("Select or create a notebook before adding sources.");
    return;
  }
  const supported = paths.filter((p) => SUPPORTED_EXTENSIONS.includes(ext(p)));
  if (supported.length === 0) {
    setError("Unsupported file type. Drop PDF, Office, image, or text files.");
    return;
  }
  await addSourceFiles(supported);
}
