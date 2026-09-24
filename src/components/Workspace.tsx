import { useEffect, useState } from "react";
import { useStore } from "@/lib/store";
import { notebookVerbs } from "@/lib/notebookMenu";
import { SourcesPanel } from "./SourcesPanel";
import { ChatPanel } from "./ChatPanel";
import { CenterModeTabs, ReaderPane } from "./ReaderPane";
import { GalleryPane } from "./GalleryPane";
import { GrowPane } from "./GrowPane";
import { StudioPanel } from "./StudioPanel";
import { AddSourceModal } from "./AddSourceModal";
import { SourcesRail, StudioRail } from "./SidebarRails";
import { HealthBanner } from "./HealthBanner";
import { Button, RowMenu, useConfirm, type RowMenuItem } from "./ui";
import { NavButtons } from "./NavButtons";
import { NotebookEditModal } from "./NotebookEditModal";
import { shortcutBlocked } from "@/lib/utils";
import type { Notebook } from "@/lib/types";
import {
  ChevronDown,
  Library,
  Search,
  Settings,
} from "lucide-react";
import { notebookIcon } from "@/lib/notebookIcons";
import { DevBadge } from "./DevBadge";
import { InferenceActivity } from "./InferenceActivity";
import { UpdateBadge } from "./UpdateBadge";
import { DitherBackground } from "./DitherBackground";

export function Workspace({ onOpenSettings }: { onOpenSettings: () => void }) {
  const currentId = useStore((s) => s.currentId);
  const readerOpen = useStore((s) => s.reader.open);
  const galleryOpen = useStore((s) => s.galleryOpen);
  const growOpen = useStore((s) => s.growOpen);
  const notebooks = useStore((s) => s.notebooks);
  const close = useStore((s) => s.closeNotebook);
  const binding = useStore((s) => s.okfBinding);
  const desktopApps = useStore((s) => s.desktopApps);
  useEffect(() => {
    void useStore.getState().refreshDesktopApps();
  }, []);
  const sourcesOpen = useStore((s) => s.sourcesOpen);
  const studioOpen = useStore((s) => s.studioOpen);
  const theme = useStore((s) => s.theme);
  const glassOn = useStore((s) => s.reading.glass);
  // Blank chat = no messages and nothing streaming (ChatPanel's own test).
  const chatBlank = useStore((s) => s.messages.length === 0 && !s.sending);
  // The backdrop's population tracks the notebook: ~40 sources reads full.
  const sourceCount = useStore((s) => s.sources.length);

  const notebook = notebooks.find((n) => n.id === currentId);
  const subtitle = [
    `${sourceCount} ${sourceCount === 1 ? "source" : "sources"}`,
    binding ? (binding.shared ? "Shared" : "On disk") : null,
    binding?.lastWriteAt ? `written ${wroteAgo(binding.lastWriteAt)}` : null,
  ]
    .filter(Boolean)
    .join(" · ");
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

  // The notebook's own verbs — the top half of the title menu; the switcher
  // is the bottom half. One dropdown, not a ⌄ beside a ⋯.
  // The same menu the Home shelf offers (src/lib/notebookMenu.tsx).
  const verbs: RowMenuItem[] = notebook
    ? notebookVerbs({
        nb: notebook,
        binding,
        desktopApps,
        onRename: () => setEditing(notebook),
        confirm,
      })
    : [];

  return (
    <div className="app-root flex h-dvh w-screen flex-col overflow-hidden bg-background text-foreground">
      <header
        data-tauri-drag-region
        className="toolbar flex h-[52px] shrink-0 items-center gap-2 pl-[84px] pr-3"
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
        <Button
          variant="ghost"
          size="icon"
          onClick={close}
          aria-label="All notebooks"
        >
          <Library className="h-4 w-4" />
        </Button>
        <div className="mx-1 h-4 w-px bg-border" />
        {/* `group`: the name cluster is a right-clickable object — the title
            RowMenu binds contextmenu to this div, carrying the same verbs as
            a notebook row on Home (color lives in Rename's dialog) plus the
            switcher. The name is chrome, not copy — no text selection. */}
        <div className="group relative flex select-none items-center gap-1.5 min-w-0">
          {/* One menu off the name, shaped the way the HIG shapes a pop-up:
              the choices are the body — the other notebooks, most recently
              touched first, the current one ticked — with the notebook's own
              verbs folded into a "Notebook" submenu (the menu bar has the
              same menu) and the Library behind a divider at the end.
              Archived and system notebooks stay out of the list; they are
              not places someone jumps to mid-thought. */}
          <RowMenu
            alwaysVisible
            label={notebook ? `Options for ${notebook.title}` : "Switch notebook"}
            tooltip={false}
            trigger={
              <span className="flex min-w-0 items-center gap-2">
                {/* The icon wears the notebook's color and sits inside the
                    pill, so the hover covers the whole title cluster. */}
                {(() => {
                  const Icon = notebookIcon(notebook?.icon);
                  return (
                    <Icon
                      className="h-4 w-4 shrink-0 text-primary"
                      style={notebook?.color ? { color: notebook.color } : undefined}
                    />
                  );
                })()}
                <span className="flex min-w-0 flex-col items-start leading-tight">
                  <span
                    className="truncate text-body font-semibold"
                    title={notebook?.title}
                  >
                    {notebook?.title ?? "Notebook"}
                  </span>
                  {/* The window's subtitle, the way Notes and Mail put the
                      count under the title: what is here, where it is kept,
                      when it was last written. The old "On disk" chip is
                      this line now; Show Bundle in Finder is in the menu. */}
                  <span className="truncate text-micro text-muted-foreground">
                    {subtitle}
                  </span>
                </span>
                <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
              </span>
            }
            triggerClassName="flex min-w-0 items-center rounded-lg px-2 py-1 transition-colors hover:bg-surface-2"
            menuClassName="w-64"
            align="left"
            items={[
              ...(notebook
                ? [
                    { label: "Notebook", items: verbs, onClick: () => {} },
                    { label: "", separator: true, onClick: () => {} },
                  ]
                : []),
              ...[...notebooks]
                .filter((n) => n.status === "")
                .sort((a, b) => b.updatedAt - a.updatedAt)
                .slice(0, 12)
                .map((n) => {
                  const Icon = notebookIcon(n.icon);
                  return {
                    label: n.title,
                    icon: <Icon className="h-3.5 w-3.5" />,
                    iconColor: n.color || undefined,
                    checked: n.id === currentId,
                    onClick: () => {
                      if (n.id !== currentId)
                        void useStore.getState().selectNotebook(n.id);
                    },
                  };
                }),
              { label: "", separator: true, onClick: () => {} },
              {
                label: "All Notebooks…",
                symbol: "books.vertical",
                icon: <Library className="h-3.5 w-3.5" />,
                onClick: close,
              },
            ]}
          />
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
        <div className="sheet flex min-w-0 flex-1 overflow-hidden">
          {growOpen ? (
            <GrowPane />
          ) : galleryOpen ? (
            <GalleryPane />
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

/** "written 2 min ago" for the title's subtitle — the same clock the On
 *  disk chip kept, in fewer words. */
function wroteAgo(ms: number): string {
  const secs = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (secs < 60) return "just now";
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins} min ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours} h ago`;
  return new Date(ms).toLocaleDateString();
}
