import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import { notebookVerbs } from "@/lib/notebookMenu";
import { SourcesPanel } from "./SourcesPanel";
import { ChatPanel } from "./ChatPanel";
import { CenterModeTabs, ReaderPane } from "./ReaderPane";
import { GalleryPane } from "./GalleryPane";
import { GrowPane } from "./GrowPane";
import { StudioPanel } from "./StudioPanel";
import { AddSourceModal } from "./AddSourceModal";
import { CHROME_BUTTON, SourcesRail, StudioRail } from "./SidebarRails";
import { HealthBanner } from "./HealthBanner";
import { RowMenu, useConfirm, type RowMenuItem } from "./ui";
import { NavButtons } from "./NavButtons";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { NotebookEditModal } from "./NotebookEditModal";
import { cn, shortcutBlocked } from "@/lib/utils";
import type { Notebook } from "@/lib/types";
import {
  ChevronDown,
  Library,
  PanelLeft,
  PanelRight,
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
  // Press-and-move on the title pill drags the window (see the pill).
  const titleDrag = useRef<{ x: number; y: number; dragged: boolean } | null>(
    null,
  );
  const binding = useStore((s) => s.okfBinding);
  const desktopApps = useStore((s) => s.desktopApps);
  useEffect(() => {
    void useStore.getState().refreshDesktopApps();
  }, []);
  const sourcesOpen = useStore((s) => s.sourcesOpen);
  const studioOpen = useStore((s) => s.studioOpen);
  const toggleSources = useStore((s) => s.toggleSources);
  const toggleStudio = useStore((s) => s.toggleStudio);
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
    // A bound notebook is a folder something else may also be writing, so
    // the clock reads "synced": the last time this app and that folder
    // agreed. An unbound notebook has no folder and no third segment.
    binding?.lastWriteAt ? `synced ${wroteAgo(binding.lastWriteAt)}` : null,
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

  const TitleIcon = notebookIcon(notebook?.icon);
  // The notebook's color rides the title tile: a 22% wash of it behind its
  // own glyph. With no color set the tile is the primary wash and the glyph
  // the citation tone — the toolbar's one spot of color either way.
  const tint = notebook?.color;

  return (
    <div className="app-root flex h-dvh w-screen flex-col overflow-hidden bg-background text-foreground">
      {/* One 52px strip that is also the title bar (docs/RFC-mac-chrome.md,
          "Toolbar"): the traffic-light gutter, the window's own controls and
          the title pill on the leading edge, the mode tabs centered in what
          those leave, then search and the inspector toggle trailing. */}
      <header
        data-tauri-drag-region
        className="toolbar flex h-[52px] shrink-0 items-center gap-3 pl-[88px] pr-3.5"
      >
        {/* No `min-w-0` here: the cluster may shrink, but not below what its
            controls and the pill's 150px floor need — otherwise the pill
            spills out of it and under the centered switcher. The search
            field is what gives after that (it shrinks to 96px). */}
        <div className="flex flex-1 basis-0 items-center gap-3">
          {/* Show/hide the Sources pane: the Finder position, left of
              Back/Forward, and the same command as ⌘1. */}
          <button
            type="button"
            onClick={toggleSources}
            aria-pressed={sourcesOpen}
            title={sourcesOpen ? "Hide sources (⌘1)" : "Show sources (⌘1)"}
            aria-label={sourcesOpen ? "Hide sources" : "Show sources"}
            // Plain at rest either way, like Finder's sidebar toggle: the
            // pane itself is the state. The inspector toggle keeps its wash
            // (docs/RFC-mac-chrome.md, "Toolbar").
            className={CHROME_BUTTON}
          >
            <PanelLeft className="h-4 w-4" />
          </button>
          <NavButtons />
          {/* The way back to the shelf sits with the other "where am I"
              controls on the left, where the hand goes for it (Paul kept
              reaching left). Finder puts its path controls there too. */}
          <button
            type="button"
            onClick={close}
            aria-label="All notebooks"
            title="All notebooks"
            className={CHROME_BUTTON}
          >
            <Library className="h-4 w-4" />
          </button>
          {/* `group`: the name cluster is a right-clickable object — the title
              RowMenu binds contextmenu to this div, carrying the same verbs as
              a notebook row on Home (color lives in Rename's dialog) plus the
              switcher. The name is chrome, not copy — no text selection. */}
          <div
            className="group relative flex min-w-0 select-none items-center"
            // The name is also a handle: press and move, and the window moves
            // with it, as a real title does. A plain click still opens the
            // pop-up — the drag starts only past a few pixels, and the click
            // that would follow a drag is swallowed. The header's own
            // `data-tauri-drag-region` cannot reach inside a button.
            onPointerDown={(e) => {
              if (e.button !== 0) return;
              titleDrag.current = { x: e.clientX, y: e.clientY, dragged: false };
            }}
            onPointerMove={(e) => {
              const d = titleDrag.current;
              if (!d || d.dragged) return;
              if (Math.hypot(e.clientX - d.x, e.clientY - d.y) < 4) return;
              d.dragged = true;
              void getCurrentWebviewWindow().startDragging();
            }}
            onPointerUp={() => {
              // Let the click event see `dragged` before the slate is wiped.
              setTimeout(() => (titleDrag.current = null), 0);
            }}
            onClickCapture={(e) => {
              if (titleDrag.current?.dragged) {
                e.preventDefault();
                e.stopPropagation();
              }
            }}
          >
            {/* One menu off the name, shaped the way the HIG shapes a pop-up:
                the choices are the body — the other notebooks, most recently
                touched first, the current one ticked — with the notebook's own
                verbs folded into a "Notebook" submenu (the menu bar has the
                same menu) and the Library behind a divider at the end.
                Archived and system notebooks stay out of the list; they are
                not places someone jumps to mid-thought. */}
            <RowMenu
              alwaysVisible
              label={
                notebook ? `Options for ${notebook.title}` : "Switch notebook"
              }
              tooltip={false}
              trigger={
                <>
                  {/* The tile wears the notebook's color and sits inside the
                      pill, so the hover wash covers the whole cluster with
                      even padding on every side. */}
                  <span
                    aria-hidden
                    className="flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-md"
                    style={{
                      background: `color-mix(in srgb, ${tint ?? "var(--primary)"} 22%, transparent)`,
                    }}
                  >
                    <TitleIcon
                      className="h-3.5 w-3.5 text-citation"
                      style={tint ? { color: tint } : undefined}
                    />
                  </span>
                  <span className="flex min-w-0 flex-col items-start">
                    <span
                      className="max-w-full truncate text-body font-semibold leading-4"
                      title={notebook?.title}
                    >
                      {notebook?.title ?? "Notebook"}
                    </span>
                    {/* The window's subtitle, the way Notes and Mail put the
                        count under the title: what is here, where it is kept,
                        when it last agreed with that folder. The old "On disk"
                        chip is this line now; Show Bundle in Finder is in the
                        menu. */}
                    <span className="max-w-full truncate text-micro leading-[13px] text-muted-foreground">
                      {subtitle}
                    </span>
                  </span>
                  <ChevronDown className="h-2.5 w-2.5 shrink-0 text-muted-foreground" />
                </>
              }
              // 36px tall with 12px sides: at 32/8 the 22px tile all but
              // touched the pill's leading edge, so the hover wash read as a
              // box drawn around the icon rather than around the name. The
              // tile and the two text lines are unchanged.
              triggerClassName="flex h-9 min-w-0 items-center gap-2.5 rounded-[8px] px-3 transition-colors hover:bg-surface-2"
              // The menu's wrapper must be allowed to give way, or the pill
              // holds its full width and the centered switcher lands on it.
              // Floor and ceiling: never squeezed below a readable name, never
              // wide enough to push the switcher off center — a long title
              // truncates, the way a window title does.
              // Below 1200px the ceiling drops so the tabs can stay near
              // center at the 1040px minimum; a long name simply shows less.
              className="min-w-[150px] max-w-[180px] !shrink min-[1200px]:max-w-[300px]"
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
        </div>

        {/* The mode tabs are the toolbar's principal item and sit on the
            window's center line: the two clusters beside them split the
            free space equally (`flex-1 basis-0` each), so the tabs move
            off center only when one side genuinely runs out of room — and
            then it is the search field that gives (96px floor), then the
            title (150px floor), never a navigation control. */}
        <div className="flex shrink-0 justify-center">
          <CenterModeTabs />
        </div>

        {/* basis-[74px], not 0: the toolbar's left padding is 88px (the
            traffic lights) against 14px on the right, so the trailing
            cluster takes those 74px extra and the tabs land on the window's
            center line rather than the content box's. */}
        <div className="flex min-w-0 flex-1 basis-[74px] items-center justify-end gap-1.5">
        <div className="flex shrink-0 items-center gap-1.5">
          {/* Left of the DEV pill in dev builds, and the same slot in
              release builds: one place, in every window, that says a model
              is working. It holds its width whether or not anything is
              running, so nothing left of it moves when a model starts. */}
          <InferenceActivity />
          <DevBadge />
          <UpdateBadge />
        </div>

        {/* The mock's 200×26 search field, shaped as the door it actually
            is: the command menu is the app's one search surface (sources,
            notes, notebooks, commands), so this opens that instead of
            holding a second query with a second result list.

            It is a child of the toolbar rather than of the trailing cluster
            for a layout reason: the whole right side used to be one
            `shrink-0` group, so at the window's 1040 minimum the title pill
            paid for everything and gave up its name entirely. Nesting can't
            fix that — a flex item with a definite width contributes that
            width to its parent's min-content however low its `min-width`
            goes, so the group refuses to narrow at all. As a toolbar item
            its own `min-width` is what flexbox honours, and it is the one
            thing up here that narrows: 200 at rest, 120 at the floor, where
            the word and the shortcut hint still both fit. `-ml-1.5` pulls
            the toolbar's 12px gap back to the 6px this cluster uses. */}
        <button
          type="button"
          onClick={() => useStore.getState().setPaletteOpen(true)}
          title="Search & commands (⌘K)"
          aria-label="Search and commands"
          className="-ml-1.5 flex h-[26px] w-[200px] min-w-[96px] shrink items-center gap-1.5 rounded-[8px] bg-surface-2 px-2 text-left text-body text-subtle-foreground shadow-[inset_0_0_0_0.5px_var(--border)] outline-none transition-colors hover:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          <Search aria-hidden className="h-3.5 w-3.5 shrink-0" />
          <span className="truncate">Search</span>
          <span aria-hidden className="ml-auto shrink-0 text-micro">
            ⌘K
          </span>
        </button>

        <div className="-ml-1.5 flex shrink-0 items-center gap-1.5">
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
          <button
            type="button"
            onClick={onOpenSettings}
            title="Settings"
            aria-label="Open settings"
            className={CHROME_BUTTON}
          >
            <Settings className="h-4 w-4" />
          </button>
          {/* The inspector toggle sits at the trailing edge, where Finder,
              Mail and Notes keep theirs, and reads pressed while it is up. */}
          <button
            type="button"
            onClick={toggleStudio}
            aria-pressed={studioOpen}
            title={studioOpen ? "Hide studio (⌘2)" : "Show studio (⌘2)"}
            aria-label={studioOpen ? "Hide studio" : "Show studio"}
            className={cn(
              CHROME_BUTTON,
              studioOpen && "bg-surface-2 text-foreground",
            )}
          >
            <PanelRight className="h-4 w-4" />
          </button>
        </div>
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

/** "synced 2 min ago" for the title's subtitle — the same clock the On
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
