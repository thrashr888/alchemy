import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  Archive,
  FileDown,
  FolderOpen,
  HardDrive,
  Pencil,
  Share,
  Trash2,
} from "lucide-react";
import { useStore } from "./store";
import type { DesktopApp, Notebook, OkfBinding } from "./types";
import type { RowMenuItem } from "@/components/ui";

type Confirm = (opts: {
  title: string;
  message?: string;
  confirmLabel?: string;
  danger?: boolean;
}) => Promise<boolean>;

/** The one notebook menu. The Home shelf's rows (and their right-click)
 *  and the workspace's title dropdown used to keep separate lists and
 *  drifted — the shelf had no on-disk verbs at all. Both call this now;
 *  a host supplies only what differs: how it renames, and its confirm. */
export function notebookVerbs({
  nb,
  binding,
  desktopApps,
  onRename,
  confirm,
}: {
  nb: Notebook;
  /** This notebook's on-disk binding, or null when it isn't kept. */
  binding: OkfBinding | null;
  desktopApps: DesktopApp[];
  onRename: () => void;
  confirm: Confirm;
}): RowMenuItem[] {
  const s = () => useStore.getState();
  const apps = desktopApps.filter((a) => a.installed);
  return [
    {
      // Name, icon, AND color — the edit dialog owns the notebook's look.
      label: "Rename",
      icon: <Pencil className="h-3.5 w-3.5" />,
      onClick: onRename,
    },
    {
      label: "Export Notebook…",
      icon: <FileDown className="h-3.5 w-3.5" />,
      onClick: () => void s().exportNotebookOkf(nb.id),
    },
    {
      // docs/RFC-shared-notebook.md §1. No `symbol`: this group of plain
      // verbs wears none, and DESIGN.md's menu rule is all or none.
      label: "Share with Someone…",
      icon: <Share className="h-3.5 w-3.5" />,
      onClick: () => void s().shareNotebookWithSomeone(nb.id),
    },
    // docs/RFC-desktop-apps.md: only the apps this Mac has.
    ...(apps.length > 0
      ? [
          {
            label: "Open In",
            icon: <Share className="h-3.5 w-3.5" />,
            onClick: () => {},
            items: apps.map((a) => ({
              label: `${a.label}…`,
              onClick: () => void s().handoffNotebook(nb.id, a.id),
            })),
          },
        ]
      : []),
    // Keeping a notebook on disk (RFC-okf-live §5.5). One verb while it is
    // off; the two it earns once it is on.
    ...(binding
      ? [
          {
            label: "Show Bundle in Finder",
            icon: <HardDrive className="h-3.5 w-3.5" />,
            onClick: () => void revealItemInDir(binding.path).catch(() => {}),
          },
          {
            label: "Stop Keeping on Disk",
            icon: <FolderOpen className="h-3.5 w-3.5" />,
            onClick: () => void s().unbindNotebookOkf(nb.id),
          },
        ]
      : [
          {
            label: "Keep on Disk as OKF…",
            icon: <HardDrive className="h-3.5 w-3.5" />,
            onClick: async () => {
              const picked = await open({
                directory: true,
                title: "Choose a folder for this notebook",
              });
              if (typeof picked === "string")
                void s().bindNotebookOkf(picked, nb.id);
            },
          },
        ]),
    // HIG: the destructive pair sits last, behind its own divider.
    { label: "", separator: true, onClick: () => {} },
    {
      // The store leaves the notebook when its open one is archived.
      label: "Archive",
      symbol: "archivebox",
      icon: <Archive className="h-3.5 w-3.5" />,
      onClick: () => void s().archiveNotebooks([nb.id]),
    },
    {
      label: "Delete…",
      symbol: "trash",
      icon: <Trash2 className="h-3.5 w-3.5" />,
      danger: true,
      onClick: async () => {
        if (
          await confirm({
            title: `Delete "${nb.title}"?`,
            message:
              "This permanently deletes the notebook and all of its sources.",
            confirmLabel: "Delete",
            danger: true,
          })
        )
          void s().deleteNotebook(nb.id);
      },
    },
  ];
}
