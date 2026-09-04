import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { SourcesPanel } from "./SourcesPanel";
import { ChatPanel } from "./ChatPanel";
import { CenterModeTabs, ReaderPane } from "./ReaderPane";
import { LedgerPane } from "./LedgerPane";
import { GalleryPane } from "./GalleryPane";
import { GrowPane } from "./GrowPane";
import { StudioPanel } from "./StudioPanel";
import { AddSourceModal } from "./AddSourceModal";
import { SourcesRail, StudioRail } from "./SidebarRails";
import { HealthBanner } from "./HealthBanner";
import { Button, RowMenu, useConfirm } from "./ui";
import { NavButtons } from "./NavButtons";
import { NotebookEditModal } from "./NotebookEditModal";
import { shortcutBlocked } from "@/lib/utils";
import type { Notebook } from "@/lib/types";
import {
  Archive,
  FileDown,
  FolderOpen,
  HardDrive,
  Library,
  Pencil,
  Search,
  Users,
  Settings,
  Trash2,
} from "lucide-react";
import { notebookIcon } from "@/lib/notebookIcons";
import { DevBadge } from "./DevBadge";
import { InferenceActivity } from "./InferenceActivity";
import { UpdateBadge } from "./UpdateBadge";
import { DitherBackground } from "./DitherBackground";
import { OkfChip } from "./OkfChip";

export function Workspace({ onOpenSettings }: { onOpenSettings: () => void }) {
  const currentId = useStore((s) => s.currentId);
  const readerOpen = useStore((s) => s.reader.open);
  const ledgerOpen = useStore((s) => s.ledgerOpen);
  const galleryOpen = useStore((s) => s.galleryOpen);
  const growOpen = useStore((s) => s.growOpen);
  const notebooks = useStore((s) => s.notebooks);
  const close = useStore((s) => s.closeNotebook);
  const binding = useStore((s) => s.okfBinding);
  const sourcesOpen = useStore((s) => s.sourcesOpen);
  const studioOpen = useStore((s) => s.studioOpen);
  const theme = useStore((s) => s.theme);
  const glassOn = useStore((s) => s.reading.glass);
  // Blank chat = no messages and nothing streaming (ChatPanel's own test).
  const chatBlank = useStore((s) => s.messages.length === 0 && !s.sending);
  // The backdrop's population tracks the notebook: ~40 sources reads full.
  const sourceCount = useStore((s) => s.sources.length);

  const notebook = notebooks.find((n) => n.id === currentId);
  const [editing, setEditing] = useState<Notebook | null>(null);
  const { confirm, dialog: confirmDialog } = useConfirm();

  // Dev-only automation hook: lets tauri-browser (and console debugging)
  // drive the reader through the store, which invoke-level access can't.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    (window as unknown as { __reader?: unknown }).__reader = (doc: {
      type: "source" | "note";
      id: string;
      highlight?: string;
    }) => useStore.getState().openInReader(doc);
  }, []);

  // Panel + note shortcuts: Cmd+1 sources, Cmd+2 studio, Cmd+N new note
  // (opening the studio panel first when it's collapsed).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || shortcutBlocked(e)) return;
      const { studioOpen, toggleSources, toggleStudio } = useStore.getState();
      if (e.key === "1") {
        e.preventDefault();
        toggleSources();
      } else if (e.key === "2") {
        e.preventDefault();
        toggleStudio();
      } else if (e.key === "n" && !studioOpen) {
        e.preventDefault();
        // Open the panel; StudioPanel opens the composer when it mounts.
        useStore.setState({ pendingNewNote: true });
        toggleStudio();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="app-root flex h-dvh w-screen flex-col overflow-hidden bg-background text-foreground">
      <header
        data-tauri-drag-region
        className="flex h-12 items-center gap-2 pl-[84px] pr-3"
      >
        <NavButtons />
        <div className="mx-1 h-4 w-px bg-border" />
        {/* A destination, not a direction: the chevron this button used to
            carry read as a second Back arrow next to the real one. The
            Library glyph is the app's one icon for "your notebooks" — the
            Notebooks tab on Home wears it too. */}
        {/* No `title`: the button already says "Notebooks", so the tooltip
            only restated it — and a native tooltip raised from inside a
            `data-tauri-drag-region` header outlives the drag. Move the
            window while it is up and macOS leaves it painted at its old
            screen point, which is how "Your notebooks" ended up floating
            over the Studio list. */}
        <Button variant="ghost" size="sm" onClick={close}>
          <Library className="h-4 w-4" />
          Notebooks
        </Button>
        <div className="mx-1 h-4 w-px bg-border" />
        {/* `group`: the name cluster is a right-clickable object — the ⋯
            RowMenu inside binds contextmenu to this div, carrying the same
            verbs as a notebook row on Home (color lives in Rename's dialog).
            The ⋯ stays visible (hover-reveal reflowed the tabs beside it)
            and the name is chrome, not copy — no text selection. */}
        <div className="group relative flex select-none items-center gap-1.5 min-w-0">
          {(() => {
            const Icon = notebookIcon(notebook?.icon);
            return <Icon className="h-3.5 w-3.5 shrink-0 text-primary" />;
          })()}
          <span
            className="inline-flex h-2.5 w-2.5 shrink-0 rounded-full border border-background"
            style={{ backgroundColor: notebook?.color }}
            aria-hidden="true"
          />
          <span
            className="truncate text-body font-semibold"
            title={notebook?.title}
          >
            {notebook?.title ?? "Notebook"}
          </span>
          <OkfChip />
          {notebook && (
            <RowMenu
              alwaysVisible
              label={`Options for ${notebook.title}`}
              items={[
                {
                  label: "Rename",
                  icon: <Pencil className="h-3.5 w-3.5" />,
                  onClick: () => setEditing(notebook),
                },
                {
                  label: "Export notebook…",
                  icon: <FileDown className="h-3.5 w-3.5" />,
                  onClick: () =>
                    void useStore.getState().exportNotebookOkf(notebook.id),
                },
                // Keeping a notebook on disk (RFC-okf-live §5.5). One verb
                // while it is off; the two it earns once it is on.
                ...(binding
                  ? [
                      {
                        label: "Show bundle in Finder",
                        icon: <HardDrive className="h-3.5 w-3.5" />,
                        onClick: () =>
                          void revealItemInDir(binding.path).catch(() => {}),
                      },
                      {
                        // Sharing is Finder's (RFC-okf-live §5.7): iCloud and
                        // Dropbox already share any folder, so the useful
                        // thing to do is put the user in front of the right
                        // one and say what to do there.
                        label: "Share folder…",
                        icon: <Users className="h-3.5 w-3.5" />,
                        onClick: () => {
                          void revealItemInDir(binding.path).catch(() => {});
                          useStore
                            .getState()
                            .pushToast(
                              "info",
                              "Right-click the folder in Finder and choose Share to invite someone.",
                            );
                        },
                      },
                      {
                        label: "Stop keeping on disk",
                        icon: <FolderOpen className="h-3.5 w-3.5" />,
                        onClick: () =>
                          void useStore.getState().unbindNotebookOkf(),
                      },
                    ]
                  : [
                      {
                        label: "Keep on disk as OKF…",
                        icon: <HardDrive className="h-3.5 w-3.5" />,
                        onClick: async () => {
                          const picked = await open({
                            directory: true,
                            title: "Choose a folder for this notebook",
                          });
                          if (typeof picked === "string")
                            void useStore.getState().bindNotebookOkf(picked);
                        },
                      },
                    ]),
                {
                  // The store leaves the notebook when its current one is
                  // archived or deleted — no extra navigation here.
                  label: "Archive",
                  icon: <Archive className="h-3.5 w-3.5" />,
                  onClick: () =>
                    void useStore
                      .getState()
                      .setNotebookStatus(notebook.id, "archived"),
                },
                {
                  label: "Delete…",
                  icon: <Trash2 className="h-3.5 w-3.5" />,
                  danger: true,
                  onClick: async () => {
                    if (
                      await confirm({
                        title: `Delete "${notebook.title}"?`,
                        message:
                          "This permanently deletes the notebook and all of its sources.",
                        confirmLabel: "Delete",
                        danger: true,
                      })
                    )
                      void useStore.getState().deleteNotebook(notebook.id);
                  },
                },
              ]}
            />
          )}
        </div>
        <div className="mx-2">
          <CenterModeTabs />
        </div>
        <div className="ml-auto flex items-center gap-1">
          {/* Left of the DEV pill in dev builds, and the same slot in
              release builds: one place, in every window, that says a model
              is working. */}
          <InferenceActivity />
          <DevBadge />
          <UpdateBadge />
          <Button
            variant="ghost"
            size="icon"
            onClick={() => useStore.getState().setPaletteOpen(true)}
            title="Search & commands (⌘K)"
            aria-label="Open the command menu"
          >
            <Search className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={onOpenSettings}
            title="Settings"
            aria-label="Open settings"
          >
            <Settings className="h-4 w-4" />
          </Button>
        </div>
      </header>

      {/* The banner flags model problems — its click-to-fix goes to Models. */}
      <HealthBanner
        onOpenSettings={() => useStore.getState().openSettings("models")}
      />

      <div className="relative flex flex-1 overflow-hidden">
        {/* Blank-chat shader as the window's backdrop: full width, behind
            the side panels — their cards sit on top, the gutters reveal it.
            The panels' roots are positioned, so they paint above this. */}
        {chatBlank &&
          !readerOpen &&
          !ledgerOpen &&
          !galleryOpen &&
          !growOpen &&
          !glassOn && (
          <>
            <div className="glass-mist pointer-events-none absolute inset-0">
              <DitherBackground
                themeKey={theme}
                density={Math.min(1, sourceCount / 40)}
              />
            </div>
            <div className="chat-mist-fade glass-mist pointer-events-none absolute inset-0" />
          </>
        )}
        {sourcesOpen ? <SourcesPanel /> : <SourcesRail />}
        <div className="flex min-w-0 flex-1 overflow-hidden pt-1">
          {growOpen ? (
            <GrowPane />
          ) : galleryOpen ? (
            <GalleryPane />
          ) : ledgerOpen ? (
            <LedgerPane />
          ) : readerOpen ? (
            <ReaderPane />
          ) : (
            <ChatPanel />
          )}
        </div>
        {studioOpen ? <StudioPanel /> : <StudioRail />}
      </div>

      {/* Global: adding sources works even while the panel is collapsed. */}
      <AddSourceModal />
      {confirmDialog}
      <NotebookEditModal notebook={editing} onClose={() => setEditing(null)} />
    </div>
  );
}
