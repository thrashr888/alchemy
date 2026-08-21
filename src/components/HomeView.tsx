import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import { DevBadge } from "./DevBadge";
import {
  Badge,
  Button,
  CardAction,
  EmptyState,
  Input,
  Modal,
  ResizeHandle,
  RowMenu,
  useConfirm,
} from "./ui";
import { AlchemyHero } from "./AlchemyHero";
import { currentEpigraph } from "@/lib/epigraph";
import { DitherBackground } from "./DitherBackground";
import { useHomeActivity } from "./useHomeActivity";
import { AwayDigest, ReportsFeed } from "./HomeReportsFeed";
import {
  cn,
  noteUnread,
  relativeTime,
  shortcutBlocked,
} from "@/lib/utils";
import type { Note, Notebook, SourceEvent } from "@/lib/types";
import {
  Archive,
  ArchiveRestore,
  BookOpen,
  ChevronRight,
  PanelRight,
  Plus,
  Search,
  Settings,
  Trash2,
  Pencil,
  FileText,
  Newspaper,
  Package,
  Sparkles,
  FolderInput,
} from "lucide-react";
import { BriefSidebar, SidebarRail, StaffSidebar } from "./HomeSections";
import { NOTEBOOK_ICONS, notebookIcon } from "@/lib/notebookIcons";
import { RegistrySection } from "./RegistrySection";
import {
  HomeTable,
  HomeViewControls,
  matchesHomeQuery,
} from "./HomeViewControls";

// Keep this list in sync with Rust in `src-tauri/src/db.rs` (`NOTEBOOK_PALETTE`)
// and the `set_notebook_color` validator in `src-tauri/src/commands.rs`.
const NOTEBOOK_PALETTE = [
  "#eb5757",
  "#e8a33d",
  "#4cb782",
  "#5e9bd2",
  "#9b87f5",
  "#e274b6",
  "#4fc1c9",
  "#98a562",
];

/** The scannable form of the notebook shelf. Same rows the grid shows, read
 *  down columns instead of across cards. */
function NotebookTable({
  notebooks,
  onOpen,
  onNew,
  unreadByNb,
}: {
  notebooks: Notebook[];
  onOpen: (id: string) => void;
  onNew: () => void;
  unreadByNb: Map<string, number>;
}) {
  return (
    <>
      <HomeTable
        columns={[
          { key: "title", label: "Title" },
          { key: "sources", label: "Sources", className: "text-right" },
          { key: "notes", label: "Notes", className: "text-right" },
          { key: "reports", label: "Reports", className: "text-right" },
          { key: "updated", label: "Updated" },
        ]}
      >
        {notebooks.map((nb) => (
          <tr
            key={nb.id}
            onClick={() => onOpen(nb.id)}
            className="cursor-pointer border-b border-border transition-colors last:border-b-0 hover:bg-surface-2"
          >
            <td className="px-3 py-2">
              <span className="flex items-center gap-2">
                {(() => {
                  const Icon = notebookIcon(nb.icon);
                  return (
                    <Icon
                      className="h-3.5 w-3.5 shrink-0"
                      style={{ color: nb.color || NOTEBOOK_PALETTE[0] }}
                      aria-hidden
                    />
                  );
                })()}
                <span className="truncate font-medium">{nb.title}</span>
                {(unreadByNb.get(nb.id) ?? 0) > 0 && (
                  <span
                    className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                    title={`${unreadByNb.get(nb.id)} unread`}
                  />
                )}
              </span>
            </td>
            <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
              {nb.sourceCount}
            </td>
            {/* Zero reads as nothing: a column of 0s is noise, and the eye
                should land on the notebooks that actually have material. */}
            <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
              {nb.noteCount || ""}
            </td>
            <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
              {nb.reportCount || ""}
            </td>
            <td className="px-3 py-2 text-caption text-muted-foreground">
              {relativeTime(nb.updatedAt)}
            </td>
          </tr>
        ))}
      </HomeTable>
      <Button variant="secondary" className="mt-3" onClick={onNew}>
        <Plus className="h-4 w-4" />
        New notebook
      </Button>
    </>
  );
}

/** Home's center switch, the exact sibling of the notebook's
 *  Chat|Reader|Gallery|Ledger tabs (CenterModeTabs, ReaderPane.tsx): one
 *  control, in the title bar, choosing what the center column shows about a
 *  constant subject. There the subject is one notebook; here it's the whole
 *  corpus — its notebooks, or the cast of things they're about. Same kind of
 *  switch, so it lives in the same place and wears the same chrome. */
function HomeSectionTabs() {
  const section = useStore((s) => s.homeSection);
  const tabs = [
    { id: "notebooks", label: "Notebooks", icon: BookOpen },
    { id: "registry", label: "Registry", icon: Package },
  ] as const;
  return (
    <div className="flex items-center gap-0.5 rounded-lg border border-border p-0.5">
      {tabs.map(({ id, label, icon: Icon }) => (
        <button
          key={id}
          type="button"
          onClick={() =>
            useStore.setState({ homeSection: id, openCardId: null })
          }
          aria-pressed={section === id}
          title={
            id === "registry"
              ? "The things your documents are about"
              : "Your notebooks"
          }
          className={cn(
            "flex items-center gap-1.5 rounded-md px-2 py-1 text-caption transition-colors",
            section === id
              ? "bg-surface-2 font-medium text-foreground"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          <Icon className="h-3.5 w-3.5" />
          {label}
        </button>
      ))}
    </div>
  );
}

export function HomeView({ onOpenSettings }: { onOpenSettings: () => void }) {
  const notebooks = useStore((s) => s.notebooks);
  const open = useStore((s) => s.selectNotebook);
  const create = useStore((s) => s.createNotebook);
  const rename = useStore((s) => s.renameNotebook);
  const setColor = useStore((s) => s.setNotebookColor);
  const remove = useStore((s) => s.deleteNotebook);
  const setStatus = useStore((s) => s.setNotebookStatus);
  const theme = useStore((s) => s.theme);
  const homeSection = useStore((s) => s.homeSection);
  const homeView = useStore((s) => s.homeView);
  const homeQuery = useStore((s) => s.homeQuery);
  // Shader must not mount under glass (rAF keeps running when display:none).
  const glassOn = useStore((s) => s.reading.glass);

  const [creating, setCreating] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [renaming, setRenaming] = useState<{
    id: string;
    title: string;
    icon: string;
  } | null>(null);
  const [colorPickerFor, setColorPickerFor] = useState<string | null>(null);
  const [archivedOpen, setArchivedOpen] = useState(false);
  // "system" notebooks (Briefs) are working infrastructure, not shelf items.
  const activeNotebooks = notebooks.filter((n) => !n.status);
  const archivedNotebooks = notebooks.filter((n) => n.status === "archived");
  // The inline filter narrows both views; the archived shelf is untouched
  // (it's already a deliberate drill-in).
  const shownNotebooks = activeNotebooks.filter((n) =>
    matchesHomeQuery(homeQuery, n.title),
  );
  // The Steward's sidebars (RFC-v12-steward UI §2, as sidebars): Staff on
  // the left, Brief above Latest Reports on the right. Each collapses on
  // its own, persisted. Registry joins when its pillar exists.
  const [reportsOpen, setReportsOpen] = useState(
    () => localStorage.getItem("homeReportsOpen") !== "0",
  );
  const toggleReports = () => {
    setReportsOpen((open) => {
      localStorage.setItem("homeReportsOpen", open ? "0" : "1");
      return !open;
    });
  };
  const [staffOpen, setStaffOpen] = useState(
    () => localStorage.getItem("homeStaffOpen") !== "0",
  );
  const toggleStaff = () => {
    setStaffOpen((open) => {
      localStorage.setItem("homeStaffOpen", open ? "0" : "1");
      return !open;
    });
  };
  const [briefOpen, setBriefOpen] = useState(
    () => localStorage.getItem("homeBriefOpen") !== "0",
  );
  const clampSplit = (pct: number) => Math.min(75, Math.max(15, pct));
  const clampStaffW = (w: number) => Math.min(440, Math.max(240, w));
  const [staffWidth, setStaffWidth] = useState(() =>
    clampStaffW(Number(localStorage.getItem("homeStaffWidth") ?? 300)),
  );
  const clampRightW = (w: number) => Math.min(820, Math.max(360, w));
  const [rightWidth, setRightWidth] = useState(() =>
    clampRightW(Number(localStorage.getItem("homeRightWidth") ?? 520)),
  );
  const [briefSplit, setBriefSplit] = useState(() =>
    clampSplit(Number(localStorage.getItem("homeBriefSplit") ?? 40)),
  );
  const rightColRef = useRef<HTMLDivElement>(null);
  // The reading column's resize handle, rendered once per stacked card —
  // each card's left edge is the column's, so either drags the whole column.
  const rightResizeHandle = (
    <ResizeHandle
      edge="left"
      width={rightWidth}
      defaultWidth={520}
      label="Resize the reading column"
      onResize={(w) => {
        const width = clampRightW(w);
        setRightWidth(width);
        localStorage.setItem("homeRightWidth", String(Math.round(width)));
      }}
    />
  );
  const onSplitDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const col = rightColRef.current;
    if (!col) return;
    const rect = col.getBoundingClientRect();
    const move = (ev: PointerEvent) => {
      const pct = clampSplit(((ev.clientY - rect.top) / rect.height) * 100);
      setBriefSplit(pct);
      localStorage.setItem("homeBriefSplit", String(Math.round(pct)));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.style.cursor = "";
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    document.body.style.cursor = "row-resize";
  };
  const toggleBrief = () => {
    setBriefOpen((open) => {
      localStorage.setItem("homeBriefOpen", open ? "0" : "1");
      return !open;
    });
  };
  const { confirm, dialog: confirmDialog } = useConfirm();

  // The unified ask box: one input over the WHOLE corpus. Enter hands the
  // question to the palette's ask mode (meta-chat, docs/RFC-meta-chat.md) —
  // no notebook choice needed; citations name where answers live.
  const [ask, setAsk] = useState("");
  function submitAsk(e: React.FormEvent) {
    e.preventDefault();
    const q = ask.trim();
    if (!q) return;
    setAsk("");
    useStore.setState({ pendingAsk: q, paletteOpen: true });
  }

  // "Since you were away": what landed since the last time home was open.
  const [prevVisit] = useState<number>(() =>
    Number(localStorage.getItem("lastHomeVisit") ?? 0),
  );
  useEffect(() => {
    localStorage.setItem("lastHomeVisit", String(Date.now()));
  }, []);

  const {
    schedules: allSchedules,
    recentNotes,
    stats,
    reports,
    loading: activityLoading,
    error: activityError,
    refresh: refreshActivity,
  } = useHomeActivity(notebooks);
  // Archived notebooks' schedules are paused by the backend — showing them
  // as "scheduled" in Staff would be a lie.
  const archivedIds = new Set(archivedNotebooks.map((n) => n.id));
  const allReports = allSchedules.filter((s) => !archivedIds.has(s.notebookId));
  const notebookTitle = new Map(notebooks.map((n) => [n.id, n.title]));
  const notebookColor = new Map(notebooks.map((n) => [n.id, n.color]));

  // Unread-report counts per notebook, for the activity dot on each card.
  const noteReads = useStore((s) => s.noteReads);
  const noteReadsBaseline = useStore((s) => s.noteReadsBaseline);
  const unreadByNb = new Map<string, number>();
  for (const r of reports) {
    if (noteUnread(r, noteReads, noteReadsBaseline)) {
      unreadByNb.set(r.notebookId, (unreadByNb.get(r.notebookId) ?? 0) + 1);
    }
  }
  const totalUnread = [...unreadByNb.values()].reduce((a, b) => a + b, 0);

  // Palette popup stays local to one card and closes on outside interaction or Escape.
  useEffect(() => {
    if (!colorPickerFor) return;
    const onPointerDown = (e: PointerEvent) => {
      const t = e.target as HTMLElement | null;
      if (
        t &&
        (t.closest("[data-notebook-color-trigger]") ||
          t.closest("[data-notebook-color-palette]"))
      ) {
        return;
      }
      setColorPickerFor(null);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setColorPickerFor(null);
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [colorPickerFor]);

  const onPickColor = (notebookId: string, color: string) => {
    setColorPickerFor(null);
    setColor(notebookId, color);
  };

  function openNote(note: Note) {
    // StudioPanel auto-opens this id once the notebook's notes load.
    useStore.setState({ justCreatedNoteId: note.id });
    void open(note.notebookId);
  }

  // A watcher event reads in its source's own notebook: switch, then open
  // the reader on the source (same shape as the alchemy:// deep links).
  function openEventSource(event: SourceEvent) {
    void open(event.notebookId).then(() => {
      useStore.getState().openInReader({ type: "source", id: event.sourceId });
    });
  }

  // Cmd/Ctrl+N: new notebook.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "n" && !shortcutBlocked(e)) {
        e.preventDefault();
        setNewTitle("");
        setCreating(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Briefs live in their own sidebar card, not the reports feed — the feed
  // would double-show them one card below.
  const briefNotes = reports.filter(
    (r) => notebookTitle.get(r.notebookId) === "Briefs",
  );
  const feedReports = reports.filter(
    (r) => notebookTitle.get(r.notebookId) !== "Briefs",
  );
  const briefUnread = briefNotes.some((r) =>
    noteUnread(r, noteReads, noteReadsBaseline),
  );

  // Backend already returns notebooks sorted by most-recently-updated.
  return (
    <div className="app-root flex h-dvh w-screen flex-col overflow-hidden text-foreground">
      <header
        data-tauri-drag-region
        className="flex h-12 items-center gap-2.5 pl-[84px] pr-5"
      >
        <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-primary/15 text-primary">
          <BookOpen className="h-4 w-4" />
        </div>
        <span className="text-section font-semibold tracking-tight">
          Alchemy
        </span>
        <div className="mx-2">
          <HomeSectionTabs />
        </div>
        <div className="ml-auto flex items-center gap-3">
          <DevBadge />
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

      {notebooks.length === 0 ? (
        <div className="flex-1">
          <AlchemyHero
            title="Alchemy"
            subtitle="Local-first research notebooks — chat with your own sources, grounded in citations, running entirely on your machine."
            epigraph={currentEpigraph(theme)}
            themeKey={theme}
          >
            <Button
              variant="primary"
              onClick={() => {
                setNewTitle("");
                setCreating(true);
              }}
            >
              <Plus className="h-4 w-4" />
              New notebook
            </Button>
          </AlchemyHero>
        </div>
      ) : (
        <div className="relative flex min-h-0 flex-1">
          {/* Three regions, same side-card idiom as the notebook view:
            Staff rail left, notebooks center, Brief + reports column right.
            Each sidebar collapses on its own. */}
          {staffOpen ? (
            <aside
              className="side-card relative mx-2 mb-2 mt-1 hidden shrink-0 flex-col lg:flex"
              style={{ width: staffWidth }}
            >
              <ResizeHandle
                edge="right"
                width={staffWidth}
                defaultWidth={300}
                label="Resize the Staff sidebar"
                onResize={(w) => {
                  const width = clampStaffW(w);
                  setStaffWidth(width);
                  localStorage.setItem("homeStaffWidth", String(Math.round(width)));
                }}
              />
              <StaffSidebar
                schedules={allReports}
                reports={reports}
                recentNotes={recentNotes}
                notebookTitle={notebookTitle}
                notebookColor={notebookColor}
                onOpenNote={openNote}
                onOpenNotebook={(id) => void open(id)}
                onOpenEvent={openEventSource}
                onRan={refreshActivity}
                onCollapse={toggleStaff}
              />
            </aside>
          ) : (
            <div className="side-card mx-2 mt-1 hidden w-12 shrink-0 flex-col items-center self-start py-2 lg:flex">
              <SidebarRail icon="staff" title="Show Staff" onClick={toggleStaff} />
            </div>
          )}
          <div className="relative flex min-w-0 flex-1 flex-col overflow-hidden">
            {/* The dither shader from the hero, as a banner behind the heading —
            it fades into the background before the notebook grid starts. */}
            {!glassOn && (
            <div
              className="glass-mist pointer-events-none absolute inset-x-0 top-0 h-64 overflow-hidden"
              aria-hidden="true"
            >
              <DitherBackground themeKey={theme} intensity={2} />
              <div className="absolute inset-0 bg-[linear-gradient(to_bottom,transparent_55%,var(--background)_100%)]" />
            </div>
            )}
            {/* Heading + ask box stay put; only the shelves below scroll. */}
            <div className="relative z-10 mx-auto w-full max-w-[960px] shrink-0 px-6 pt-10">
              {/* Wraps rather than squeezes: with both sidebars open this
                  column is far narrower than its 960px cap, so the action
                  cluster drops to its own line instead of crushing the
                  heading. */}
              <div className="mb-5 flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
                <div className="min-w-[260px] flex-1">
                  <h1 className="text-page font-semibold tracking-tight">
                    {homeSection === "registry"
                      ? "Your registry"
                      : "Your notebooks"}
                  </h1>
                  {homeSection === "registry" ? (
                    <p className="mt-1 text-body text-muted-foreground">
                      The things your documents are about — assets, people,
                      policies, providers, projects, dependencies.
                    </p>
                  ) : (
                  <p className="mt-1 text-body text-muted-foreground">
                    {stats
                      ? [
                          `${activeNotebooks.length} ${activeNotebooks.length === 1 ? "notebook" : "notebooks"}`,
                          `${stats.sources} ${stats.sources === 1 ? "source" : "sources"}`,
                          stats.notes > 0 &&
                            `${stats.notes} ${stats.notes === 1 ? "note" : "notes"}`,
                          stats.ledger > 0 &&
                            `${stats.ledger} ledger ${stats.ledger === 1 ? "entry" : "entries"}`,
                          `${Intl.NumberFormat().format(stats.chars)} chars indexed`,
                        ]
                          .filter(Boolean)
                          .join(" · ")
                      : "Most recently used first."}
                  </p>
                  )}
                  {homeSection === "notebooks" && (
                    <AwayDigest
                      prevVisit={prevVisit}
                      notebooks={notebooks}
                      reports={reports}
                    />
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Button
                    variant="secondary"
                    onClick={() =>
                      useStore.setState({
                        // Empty payload = capture first, then file. Home has
                        // no current notebook, so this is the one add path
                        // that has to pick one — and it suggests which.
                        pendingExternalAdd: {
                          files: [],
                          url: null,
                          text: null,
                          title: null,
                        },
                      })
                    }
                    title="Save a link or note — Alchemy suggests the notebook"
                  >
                    <Plus className="h-4 w-4" />
                    Add source…
                  </Button>
                  <Button
                    variant="secondary"
                    onClick={() => useStore.setState({ importOkfOpen: true })}
                    title="Import a shared .okf.zip or bundle folder"
                  >
                    <FolderInput className="h-4 w-4" />
                    Import…
                  </Button>
                  <Button
                    variant="primary"
                    onClick={() => {
                      setNewTitle("");
                      setCreating(true);
                    }}
                  >
                    <Plus className="h-4 w-4" />
                    New notebook
                  </Button>
                </div>
              </div>

              {/* The unified ask box: one input, the whole corpus. Enter asks
              across every notebook (palette ask mode); the ⌘K chip is the
              same surface in search mode. */}
              <div className="mb-8">
                <form
                  onSubmit={submitAsk}
                  className="flex min-w-0 items-center gap-1.5 rounded-xl border border-border bg-surface/80 p-1.5 shadow-sm backdrop-blur transition-colors focus-within:border-primary/50"
                >
                  <Sparkles className="ml-2 h-4 w-4 shrink-0 text-citation" />
                  <input
                    value={ask}
                    onChange={(e) => setAsk(e.target.value)}
                    placeholder="Ask or search across all your notebooks…"
                    aria-label="Ask a question across all notebooks"
                    className="h-8 min-w-0 flex-1 bg-transparent px-1.5 text-body text-foreground outline-none placeholder:text-subtle-foreground"
                  />
                  <button
                    type="button"
                    onClick={() => useStore.getState().setPaletteOpen(true)}
                    title="Search notebooks, sources & notes (⌘K)"
                    aria-label="Open search"
                    className="flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-caption text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
                  >
                    <Search className="h-3.5 w-3.5" />
                    <kbd className="rounded border border-border bg-surface-2 px-1 py-0.5 text-badge text-subtle-foreground">
                      ⌘K
                    </kbd>
                  </button>
                  <Button
                    type="submit"
                    variant="primary"
                    size="sm"
                    disabled={!ask.trim()}
                  >
                    Ask
                  </Button>
                </form>
                {activityError && (
                  <div
                    role="alert"
                    className="mt-2 flex items-center gap-3 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-caption text-destructive"
                  >
                    <span className="min-w-0 flex-1">{activityError}</span>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => void refreshActivity()}
                      loading={activityLoading}
                    >
                      Retry
                    </Button>
                  </div>
                )}
              </div>
            </div>

            {homeSection === "registry" ? (
              <RegistrySection />
            ) : (
            <div className="relative min-h-0 flex-1 overflow-y-auto">
              <div className="mx-auto w-full max-w-[960px] px-6 pb-10">
              <HomeViewControls placeholder="Filter notebooks by title…" />
              {homeView === "table" ? (
                <NotebookTable
                  notebooks={shownNotebooks}
                  onOpen={open}
                  onNew={() => {
                    setNewTitle("");
                    setCreating(true);
                  }}
                  unreadByNb={unreadByNb}
                />
              ) : (
              <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
                {/* New-notebook tile */}
                <button
                  onClick={() => {
                    setNewTitle("");
                    setCreating(true);
                  }}
                  className="flex min-h-[132px] flex-col items-center justify-center gap-2 rounded-lg border border-dashed border-border-strong bg-surface/40 text-muted-foreground transition-colors hover:border-primary/50 hover:text-foreground"
                >
                  <Plus className="h-6 w-6" />
                  <span className="text-body font-medium">New notebook</span>
                </button>

                {shownNotebooks.map((nb) => (
                  <div
                    key={nb.id}
                    // Card content is pointer-events-none, so the hover
                    // tooltip for the truncated title lives on the card.
                    title={nb.title}
                    className={cn(
                      "group relative flex min-h-[132px] cursor-pointer flex-col rounded-lg border border-border bg-surface p-4 transition-colors hover:border-border-strong hover:bg-surface-2",
                      "has-[[aria-expanded=true]]:z-30",
                    )}
                  >
                    <CardAction
                      label={`Open notebook ${nb.title}`}
                      onClick={() => open(nb.id)}
                    />
                    <div
                      className="pointer-events-none relative z-10 mb-auto flex h-8 w-8 items-center justify-center rounded-lg"
                      style={{
                        backgroundColor: `color-mix(in srgb, ${nb.color || NOTEBOOK_PALETTE[0]} 16%, transparent)`,
                        color: nb.color || NOTEBOOK_PALETTE[0],
                      }}
                    >
                      {(() => {
                        const Icon = notebookIcon(nb.icon);
                        return <Icon className="h-4 w-4" />;
                      })()}
                    </div>
                    <div className="pointer-events-none relative z-10 mt-3 flex items-center gap-1.5">
                      <span
                        className="truncate text-card font-medium"
                        title={nb.title}
                      >
                        {nb.title}
                      </span>
                      {(unreadByNb.get(nb.id) ?? 0) > 0 && (
                        <span
                          className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                          title={`${unreadByNb.get(nb.id)} unread ${unreadByNb.get(nb.id) === 1 ? "report" : "reports"}`}
                          aria-label={`${unreadByNb.get(nb.id)} unread reports`}
                        />
                      )}
                    </div>
                    <div className="pointer-events-none relative z-10 mt-1 flex items-center gap-1.5 text-micro text-subtle-foreground">
                      <Badge className="gap-1">
                        <FileText className="h-2.5 w-2.5" />
                        {nb.sourceCount}
                      </Badge>
                      <span>·</span>
                      <span>{relativeTime(nb.updatedAt)}</span>
                    </div>

                    <div className="absolute right-2 top-2 z-20 flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
                      <button
                        type="button"
                        className="rounded p-1 text-muted-foreground transition hover:bg-elevated"
                        style={{
                          backgroundColor: nb.color || NOTEBOOK_PALETTE[0],
                        }}
                        onClick={(e) => {
                          e.stopPropagation();
                          setColorPickerFor((cur) =>
                            cur === nb.id ? null : nb.id,
                          );
                        }}
                        onPointerDown={(e) => e.stopPropagation()}
                        data-notebook-color-trigger
                        aria-expanded={colorPickerFor === nb.id}
                        aria-label={`Change color for ${nb.title}`}
                        title="Change notebook color"
                      >
                        <span className="relative block h-3 w-3 rounded-full border border-background" />
                      </button>
                      <RowMenu
                        label={`Options for ${nb.title}`}
                        items={[
                          {
                            label: "Rename",
                            icon: <Pencil className="h-3.5 w-3.5" />,
                            onClick: () =>
                              setRenaming({
                                id: nb.id,
                                title: nb.title,
                                icon: nb.icon,
                              }),
                          },
                          {
                            label: "Archive",
                            icon: <Archive className="h-3.5 w-3.5" />,
                            onClick: () => void setStatus(nb.id, "archived"),
                          },
                          {
                            label: "Delete…",
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
                                remove(nb.id);
                            },
                          },
                        ]}
                      />
                    </div>
                    {colorPickerFor === nb.id && (
                      <div
                        onClick={(e) => e.stopPropagation()}
                        onPointerDown={(e) => e.stopPropagation()}
                        data-notebook-color-palette
                        className="menu-glass absolute right-2 top-10 z-30 flex rounded-md border border-border px-2 py-1.5 shadow-sm"
                      >
                        {NOTEBOOK_PALETTE.map((c) => (
                          <button
                            key={c}
                            type="button"
                            onClick={() => onPickColor(nb.id, c)}
                            onPointerDown={(e) => e.stopPropagation()}
                            aria-label={`Set ${nb.title} color to ${c}`}
                            className={cn(
                              "m-0.5 h-5 w-5 rounded-full border border-border",
                              c === (nb.color || NOTEBOOK_PALETTE[0])
                                ? "ring-2 ring-foreground ring-offset-1 ring-offset-surface"
                                : "",
                            )}
                            style={{ backgroundColor: c }}
                          />
                        ))}
                      </div>
                    )}
                  </div>
                ))}
              </div>
              )}
              {shownNotebooks.length === 0 && activeNotebooks.length > 0 && (
                <p className="py-8 text-center text-body text-muted-foreground">
                  No notebook matches &ldquo;{homeQuery.trim()}&rdquo;.
                </p>
              )}

              {/* Archived notebooks: collapsed row list, data intact. */}
              {archivedNotebooks.length > 0 && (
                <div className="mt-8">
                  <button
                    type="button"
                    onClick={() => setArchivedOpen((v) => !v)}
                    aria-expanded={archivedOpen}
                    className="flex cursor-pointer items-center gap-1 text-micro font-medium uppercase tracking-wide text-subtle-foreground transition-colors hover:text-muted-foreground"
                  >
                    <ChevronRight
                      className={cn(
                        "h-3 w-3 transition-transform",
                        archivedOpen && "rotate-90",
                      )}
                    />
                    Archived · {archivedNotebooks.length}
                  </button>
                  {archivedOpen && (
                    <div className="mt-2 flex flex-col gap-1">
                      {archivedNotebooks.map((nb) => (
                        <div
                          key={nb.id}
                          className="group flex items-center gap-2.5 rounded-md border border-border bg-surface px-3 py-2 transition-colors hover:border-border-strong hover:bg-surface-2"
                        >
                          <Archive className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          <span className="truncate text-body text-foreground">
                            {nb.title}
                          </span>
                          <Badge className="shrink-0 gap-1">
                            <FileText className="h-2.5 w-2.5" />
                            {nb.sourceCount}
                          </Badge>
                          <div className="ml-auto flex shrink-0 items-center gap-1 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() => void setStatus(nb.id, "")}
                            >
                              <ArchiveRestore className="mr-1 h-3.5 w-3.5" />
                              Unarchive
                            </Button>
                            <RowMenu
                              label={`Options for ${nb.title}`}
                              items={[
                                {
                                  label: "Delete…",
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
                                      remove(nb.id);
                                  },
                                },
                              ]}
                            />
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}

              {/* Recent notes live in the Staff sidebar now — the center
                  column is just the notebook shelves. */}
              </div>
            </div>
            )}
          </div>

          {/* Right column: the Brief card above the reports feed — the
            morning-read surface, arrival point first. */}
          {!briefOpen && !reportsOpen ? (
            <div className="side-card mx-2 mt-1 hidden w-12 shrink-0 flex-col items-center gap-1 self-start py-2 lg:flex">
              <SidebarRail
                icon="brief"
                title="Show the brief"
                dot={briefUnread}
                onClick={toggleBrief}
              />
              <SidebarRail
                icon="reports"
                title="Show latest reports"
                dot={totalUnread > 0}
                onClick={toggleReports}
              />
            </div>
          ) : (
            <div
              ref={rightColRef}
              className="relative mx-2 mb-2 mt-1 hidden shrink-0 flex-col lg:flex"
              style={{ width: rightWidth }}
            >
              {/* One handle per stacked card (a column-spanning handle floats
                  over the gap between the cards' rounded corners); both drag
                  the whole column's width. The card's left edge IS the
                  column's, so the parent-rect math is unchanged. */}
              <>
                <BriefSidebar
                  open={briefOpen}
                  onToggle={toggleBrief}
                  briefs={briefNotes}
                  schedules={allReports}
                  unread={briefUnread}
                  onRan={refreshActivity}
                  resizeHandle={rightResizeHandle}
                  className={cn(
                    briefOpen && !reportsOpen && "min-h-0 flex-1",
                    briefOpen && reportsOpen && "shrink-0",
                  )}
                  style={
                    briefOpen && reportsOpen
                      ? { height: `${briefSplit}%` }
                      : undefined
                  }
                />
                {briefOpen && reportsOpen ? (
                  <div
                    role="separator"
                    aria-orientation="horizontal"
                    aria-label="Resize the brief"
                    onPointerDown={onSplitDrag}
                    onDoubleClick={() => {
                      setBriefSplit(40);
                      localStorage.setItem("homeBriefSplit", "40");
                    }}
                    className="group/resize relative h-2 shrink-0 cursor-row-resize rounded transition-colors hover:bg-ring/30 active:bg-ring/40"
                  >
                    <span
                      aria-hidden
                      className="absolute top-1/2 left-1/2 flex -translate-x-1/2 -translate-y-1/2 gap-0.5 opacity-40 transition-opacity group-hover/resize:opacity-100"
                    >
                      <span className="h-0.5 w-0.5 rounded-full bg-muted-foreground" />
                      <span className="h-0.5 w-0.5 rounded-full bg-muted-foreground" />
                      <span className="h-0.5 w-0.5 rounded-full bg-muted-foreground" />
                    </span>
                  </div>
                ) : (
                  <div className="h-2 shrink-0" />
                )}
                <aside
                  className={cn(
                    "side-card relative flex min-h-0 flex-col",
                    reportsOpen && "flex-1",
                  )}
                >
                  {rightResizeHandle}
                  {reportsOpen ? (
                    feedReports.length > 0 ? (
                      <ReportsFeed
                        onCollapse={toggleReports}
                        reports={feedReports}
                        notebookTitle={notebookTitle}
                        notebookColor={notebookColor}
                        fallbackColor={NOTEBOOK_PALETTE[0]}
                        onOpen={openNote}
                      />
                    ) : activityLoading ? (
              <div
                role="status"
                className="flex flex-1 items-center justify-center p-8 text-caption text-muted-foreground"
              >
                Loading reports…
              </div>
            ) : (
              <div className="flex flex-1 items-center justify-center p-8">
                <EmptyState
                  icon={<Newspaper className="h-7 w-7" />}
                  title={activityError ? "Reports unavailable" : "Reports land here"}
                  hint={
                    activityError
                      ? "Alchemy couldn’t load recent reports."
                      : "Schedule a recurring report from a notebook’s Studio panel."
                  }
                >
                  {activityError && (
                    <Button
                      variant="secondary"
                      size="sm"
                      onClick={() => void refreshActivity()}
                    >
                      Retry
                    </Button>
                  )}
                </EmptyState>
              </div>
                    )
                  ) : (
                    <div className="flex h-12 shrink-0 items-center gap-2 px-6">
                      <span className="whitespace-nowrap text-caption font-semibold uppercase tracking-wide text-muted-foreground">
                        Latest reports
                      </span>
                      {totalUnread > 0 && (
                        <span
                          title={`${totalUnread} unread`}
                          className="rounded-full bg-primary/15 px-1.5 py-0.5 text-badge font-medium tabular-nums text-citation"
                        >
                          {totalUnread}
                        </span>
                      )}
                      <button
                        type="button"
                        onClick={toggleReports}
                        title="Show latest reports"
                        aria-expanded={false}
                        className="ml-auto rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
                      >
                        <PanelRight className="h-4 w-4" />
                      </button>
                    </div>
                  )}
                </aside>
              </>
            </div>
          )}
        </div>
      )}

      <Modal
        open={creating}
        onClose={() => setCreating(false)}
        title="New notebook"
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            create(newTitle);
            setCreating(false);
          }}
          className="flex flex-col gap-3"
        >
          <Input
            autoFocus
            name="notebook-title"
            aria-label="Notebook title"
            placeholder="Notebook title"
            value={newTitle}
            onChange={(e) => setNewTitle(e.target.value)}
          />
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setCreating(false)}
            >
              Cancel
            </Button>
            <Button type="submit" variant="primary">
              Create & open
            </Button>
          </div>
        </form>
      </Modal>

      {confirmDialog}

      <Modal
        open={!!renaming}
        onClose={() => setRenaming(null)}
        title="Edit notebook"
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (renaming) {
              const r = renaming;
              const before = notebooks.find((n) => n.id === r.id);
              // Icon first, sequenced: rename() ends with a full refresh,
              // and firing both unordered let that refresh read the DB
              // before the icon write landed — reverting the optimistic
              // icon until some later refresh ("shows up two edits later").
              void (async () => {
                if (before && before.icon !== r.icon)
                  await useStore.getState().setNotebookIcon(r.id, r.icon);
                if (before && before.title !== r.title)
                  await rename(r.id, r.title);
              })();
            }
            setRenaming(null);
          }}
          className={cn("flex flex-col gap-3")}
        >
          <Input
            autoFocus
            name="notebook-title"
            aria-label="Notebook title"
            value={renaming?.title ?? ""}
            onChange={(e) =>
              setRenaming((r) => (r ? { ...r, title: e.target.value } : r))
            }
          />
          {/* Icon picker: the auto-picked icon can always be overridden
              here; the plain book is a first-class choice, not an absence. */}
          <div className="grid grid-cols-8 gap-1">
            {["", ...Object.keys(NOTEBOOK_ICONS).filter((k) => k !== "book-open")].map(
              (name) => {
                const Icon = notebookIcon(name);
                const active = (renaming?.icon ?? "") === name;
                return (
                  <button
                    key={name || "default"}
                    type="button"
                    aria-pressed={active}
                    aria-label={name ? `Icon: ${name.replace(/-/g, " ")}` : "Default icon"}
                    title={name ? name.replace(/-/g, " ") : "Default"}
                    onClick={() =>
                      setRenaming((r) => (r ? { ...r, icon: name } : r))
                    }
                    className={cn(
                      "flex h-8 items-center justify-center rounded-md border transition-colors",
                      active
                        ? "border-primary/60 bg-primary/10 text-foreground"
                        : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
                    )}
                  >
                    <Icon className="h-4 w-4" />
                  </button>
                );
              },
            )}
          </div>
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setRenaming(null)}
            >
              Cancel
            </Button>
            <Button type="submit" variant="primary">
              Save
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}
