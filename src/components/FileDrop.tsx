import { useEffect } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isTauri } from "@tauri-apps/api/core";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
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
    if (!isTauri()) return;
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
      })
      .catch((error) => {
        if (!active) return;
        console.error("file-drop listener failed", error);
        useStore
          .getState()
          .pushToast("error", "File drop is unavailable. Use Add Source instead.");
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

  // OKF bundles (a shared .okf.zip or an exported folder) route to import,
  // not source ingestion — and they work from the homepage too. Only zips
  // and extensionless paths (candidate folders) are worth probing.
  const rest: string[] = [];
  for (const p of paths) {
    const e = ext(p);
    const probeWorthy = e === "zip" || e === "";
    if (probeWorthy && (await api.probeOkf(p).catch(() => false))) {
      useStore.setState({ pendingImportPath: p, importOkfOpen: true });
    } else {
      rest.push(p);
    }
  }
  if (rest.length === 0) return;

  if (!currentId) {
    setError("Select or create a notebook before adding sources.");
    return;
  }
  // Extensionless paths pass through: they're folders (which become synced
  // folder sources) or plain-text files — the backend handles both.
  const supported = rest.filter(
    (p) => SUPPORTED_EXTENSIONS.includes(ext(p)) || ext(p) === "",
  );
  if (supported.length === 0) {
    setError("Unsupported file type. Drop PDF, Office, image, or text files.");
    return;
  }
  await addSourceFiles(supported);
}
