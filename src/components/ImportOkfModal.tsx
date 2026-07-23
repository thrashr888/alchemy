import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "@/lib/store";
import { Button, Modal } from "./ui";
import { FileArchive, FolderOpen } from "lucide-react";

/**
 * Import an OKF bundle — the receiving end of "Share notebook as .okf.zip"
 * (a coworker's bundle, or your own from another machine). Into a new
 * notebook by default, or merged into an existing one (duplicate sources
 * skip quietly, so re-importing is harmless).
 */
export function ImportOkfModal() {
  const isOpen = useStore((s) => s.importOkfOpen);
  const notebooks = useStore((s) => s.notebooks);
  const importOkf = useStore((s) => s.importOkf);
  // Set when a bundle was dropped on the window — the file is already
  // chosen, so the dialog only asks where it should land.
  const dropped = useStore((s) => s.pendingImportPath);
  const [dest, setDest] = useState<string>("");

  const close = () =>
    useStore.setState({ importOkfOpen: false, pendingImportPath: null });

  async function pick(directory: boolean) {
    const path = await open(
      directory
        ? { directory: true, title: "Choose an OKF bundle folder" }
        : {
            title: "Choose an .okf.zip",
            filters: [{ name: "OKF bundle", extensions: ["zip"] }],
          },
    );
    if (!path) return;
    close();
    await importOkf(path as string, dest || null);
  }

  return (
    <Modal
      open={isOpen}
      onClose={close}
      title="Import a notebook"
      width="max-w-md"
    >
      <div className="flex flex-col gap-4">
        <p className="text-caption leading-relaxed text-muted-foreground">
          Bring in an{" "}
          <code className="rounded bg-surface-2 px-1 py-0.5">.okf.zip</code>{" "}
          someone shared (or an exported bundle folder). Sources are re-embedded
          locally; nothing leaves this Mac.
        </p>
        <label className="flex flex-col gap-1.5">
          <span className="text-caption text-muted-foreground">Import into</span>
          <select
            value={dest}
            onChange={(e) => setDest(e.target.value)}
            className="h-8 w-full rounded-md border border-input bg-surface-2 px-2 text-body text-foreground outline-none focus:border-ring/70 focus:ring-1 focus:ring-ring/40"
          >
            <option value="">New notebook (named from the bundle)</option>
            {notebooks.map((nb) => (
              <option key={nb.id} value={nb.id}>
                Add to: {nb.title}
              </option>
            ))}
          </select>
        </label>
        {dropped && (
          <div className="flex items-center gap-2 rounded-md border border-border bg-surface-2/40 px-3 py-2 text-caption text-foreground/90">
            <FileArchive className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <span className="min-w-0 truncate">
              {dropped.split("/").pop() ?? dropped}
            </span>
          </div>
        )}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={close}>
            Cancel
          </Button>
          {dropped ? (
            <Button
              type="button"
              variant="primary"
              onClick={() => {
                const path = dropped;
                close();
                void importOkf(path, dest || null);
              }}
            >
              <FileArchive className="h-3.5 w-3.5" />
              Import
            </Button>
          ) : (
            <>
              <Button
                type="button"
                variant="secondary"
                onClick={() => void pick(true)}
              >
                <FolderOpen className="h-3.5 w-3.5" />
                Bundle folder…
              </Button>
              <Button
                type="button"
                variant="primary"
                onClick={() => void pick(false)}
              >
                <FileArchive className="h-3.5 w-3.5" />
                Choose .okf.zip…
              </Button>
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}
