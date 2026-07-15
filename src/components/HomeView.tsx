import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import { DevBadge } from "./DevBadge";
import { api } from "@/lib/api";
import { Button, Input, Modal, Badge, EmptyState, useConfirm } from "./ui";
import { AlchemyHero } from "./AlchemyHero";
import { currentEpigraph } from "@/lib/epigraph";
import { DitherBackground } from "./DitherBackground";
import { Markdown } from "./Markdown";
import { intervalLabel } from "./Reports";
import {
  cn,
  noteUnread,
  relativeTime,
  cardButtonProps,
  shortcutBlocked,
} from "@/lib/utils";
import type { CorpusStats, Note, ReportSchedule } from "@/lib/types";
import {
  BookOpen,
  Clock,
  Plus,
  Power,
  Search,
  Settings,
  Trash2,
  Pencil,
  FileText,
  Newspaper,
  Sparkles,
  FolderInput,
} from "lucide-react";

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

export function HomeView({ onOpenSettings }: { onOpenSettings: () => void }) {
  const notebooks = useStore((s) => s.notebooks);
  const open = useStore((s) => s.selectNotebook);
  const create = useStore((s) => s.createNotebook);
  const rename = useStore((s) => s.renameNotebook);
  const setColor = useStore((s) => s.setNotebookColor);
  const remove = useStore((s) => s.deleteNotebook);
  const theme = useStore((s) => s.theme);

  const [creating, setCreating] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [renaming, setRenaming] = useState<{
    id: string;
    title: string;
  } | null>(null);
  const [colorPickerFor, setColorPickerFor] = useState<string | null>(null);
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

  // All scheduled reports, recent documents, and corpus totals across
  // notebooks — the app's ongoing activity, refreshed with the notebook list.
  const [allReports, setAllReports] = useState<ReportSchedule[]>([]);
  const [recentNotes, setRecentNotes] = useState<Note[]>([]);
  const [stats, setStats] = useState<CorpusStats | null>(null);
  // Latest generated reports, read in place in the right-hand feed. Fifty
  // covers the pane's purpose — the notebook holds the full archive.
  const [reports, setReports] = useState<Note[]>([]);
  useEffect(() => {
    api
      .listAllReportSchedules()
      .then(setAllReports)
      .catch(() => setAllReports([]));
    api
      .listRecentNotes(5)
      .then(setRecentNotes)
      .catch(() => setRecentNotes([]));
    api
      .listRecentReports(50)
      .then(setReports)
      .catch(() => setReports([]));
    api
      .corpusStats()
      .then(setStats)
      .catch(() => setStats(null));
  }, [notebooks]);
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

  // Backend already returns notebooks sorted by most-recently-updated.
  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
      <header
        data-tauri-drag-region
        className="flex items-center gap-2.5 h-12 border-b border-border pl-[84px] pr-5"
      >
        <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-primary/15 text-primary">
          <BookOpen className="h-4 w-4" />
        </div>
        <span className="text-[15px] font-semibold tracking-tight">
          Alchemy
        </span>
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
        <div className="flex min-h-0 flex-1">
          {/* Left pane: notebooks & activity. Right pane: the reports feed.
            Two independent scroll regions, same idiom as the notebook view. */}
          <div className="relative min-w-0 flex-1 overflow-y-auto">
            {/* The dither shader from the hero, as a banner behind the heading —
            it fades into the background before the notebook grid starts. */}
            <div
              className="pointer-events-none absolute inset-x-0 top-0 h-64 overflow-hidden"
              aria-hidden="true"
            >
              <DitherBackground themeKey={theme} intensity={2} />
              <div className="absolute inset-0 bg-[linear-gradient(to_bottom,transparent_55%,var(--background)_100%)]" />
            </div>
            <div className="relative mx-auto max-w-[960px] px-6 py-10">
              <div className="mb-5 flex items-end justify-between">
                <div>
                  <h1 className="text-[22px] font-semibold tracking-tight">
                    Your notebooks
                  </h1>
                  <p className="mt-1 text-[13px] text-muted-foreground">
                    {stats
                      ? `${notebooks.length} ${notebooks.length === 1 ? "notebook" : "notebooks"} · ${stats.sources} ${stats.sources === 1 ? "source" : "sources"} · ${Intl.NumberFormat().format(stats.chars)} chars indexed`
                      : "Most recently used first."}
                  </p>
                  <AwayDigest
                    prevVisit={prevVisit}
                    notebooks={notebooks}
                    reports={reports}
                  />
                </div>
                <div className="flex items-center gap-2">
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
                    className="h-8 min-w-0 flex-1 bg-transparent px-1.5 text-[13px] text-foreground outline-none placeholder:text-subtle-foreground"
                  />
                  <button
                    type="button"
                    onClick={() => useStore.getState().setPaletteOpen(true)}
                    title="Search notebooks, sources & notes (⌘K)"
                    aria-label="Open search"
                    className="flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-[12px] text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
                  >
                    <Search className="h-3.5 w-3.5" />
                    <kbd className="rounded border border-border bg-surface-2 px-1 py-0.5 text-[10px] text-subtle-foreground">
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
              </div>

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
                  <span className="text-[13px] font-medium">New notebook</span>
                </button>

                {notebooks.map((nb) => (
                  <div
                    key={nb.id}
                    onClick={() => open(nb.id)}
                    {...cardButtonProps(() => open(nb.id))}
                    className="group relative flex min-h-[132px] cursor-pointer flex-col rounded-lg border border-border bg-surface p-4 transition-colors hover:border-border-strong hover:bg-surface-2"
                  >
                    <span
                      className="pointer-events-none absolute left-0 top-0 h-full w-[3px] rounded-l-lg"
                      style={{
                        backgroundColor: nb.color || NOTEBOOK_PALETTE[0],
                      }}
                    />
                    <div className="mb-auto flex h-8 w-8 items-center justify-center rounded-lg bg-primary/12 text-primary">
                      <BookOpen className="h-4 w-4" />
                    </div>
                    <div className="mt-3 flex items-center gap-1.5">
                      <span
                        className="truncate text-[14px] font-medium"
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
                    <div className="mt-1 flex items-center gap-1.5 text-[11px] text-subtle-foreground">
                      <Badge className="gap-1">
                        <FileText className="h-2.5 w-2.5" />
                        {nb.sourceCount}
                      </Badge>
                      <span>·</span>
                      <span>{relativeTime(nb.updatedAt)}</span>
                    </div>

                    <div className="absolute right-2 top-2 flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
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
                        aria-label={`Change color for ${nb.title}`}
                        title="Change notebook color"
                      >
                        <span className="relative block h-3 w-3 rounded-full border border-background" />
                      </button>
                      <button
                        className="rounded p-1 text-muted-foreground hover:bg-elevated hover:text-foreground"
                        onClick={(e) => {
                          e.stopPropagation();
                          setRenaming({ id: nb.id, title: nb.title });
                        }}
                        title="Rename"
                        aria-label={`Rename "${nb.title}"`}
                      >
                        <Pencil className="h-3.5 w-3.5" />
                      </button>
                      <button
                        className="rounded p-1 text-muted-foreground hover:bg-elevated hover:text-destructive"
                        onClick={async (e) => {
                          e.stopPropagation();
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
                        }}
                        title="Delete"
                        aria-label={`Delete "${nb.title}"`}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                    {colorPickerFor === nb.id && (
                      <div
                        onClick={(e) => e.stopPropagation()}
                        onPointerDown={(e) => e.stopPropagation()}
                        data-notebook-color-palette
                        className="absolute right-2 top-10 z-10 flex rounded-md border border-border bg-surface px-2 py-1.5 shadow-sm"
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

              {/* The last few generated/edited documents across all notebooks. */}
              {recentNotes.length > 0 && (
                <div className="mt-10">
                  <div className="mb-2 text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
                    Recent notes
                  </div>
                  <div className="flex flex-col gap-1">
                    {recentNotes.map((n) => (
                      <div
                        key={n.id}
                        onClick={() => openNote(n)}
                        {...cardButtonProps(() => openNote(n))}
                        title={`Open in "${notebookTitle.get(n.notebookId) ?? "notebook"}"`}
                        className="flex cursor-pointer items-center gap-2.5 rounded-md border border-border bg-surface px-3 py-2 transition-colors hover:border-border-strong hover:bg-surface-2"
                      >
                        <FileText className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                        <span className="truncate text-[13px] text-foreground">
                          {n.title}
                        </span>
                        <Badge className="shrink-0 gap-1">
                          <BookOpen className="h-2.5 w-2.5" />
                          <span className="max-w-[160px] truncate">
                            {notebookTitle.get(n.notebookId) ??
                              "Unknown notebook"}
                          </span>
                        </Badge>
                        <span className="ml-auto shrink-0 text-[11px] text-subtle-foreground">
                          {relativeTime(n.updatedAt)}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {/* Scheduled reports across all notebooks — the app's ongoing activity. */}
              {allReports.length > 0 && (
                <div className="mt-10">
                  <div className="mb-2 text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
                    Scheduled reports
                  </div>
                  <div className="flex flex-col gap-1">
                    {[...allReports]
                      .sort((a, b) =>
                        a.enabled !== b.enabled
                          ? a.enabled
                            ? -1
                            : 1
                          : b.lastRunAt - a.lastRunAt,
                      )
                      .map((r) => (
                        <div
                          key={r.id}
                          onClick={() => open(r.notebookId)}
                          {...cardButtonProps(() => open(r.notebookId))}
                          title={`Open "${notebookTitle.get(r.notebookId) ?? "notebook"}"`}
                          className="flex cursor-pointer items-center gap-2.5 rounded-md border border-border bg-surface px-3 py-2 transition-colors hover:border-border-strong hover:bg-surface-2"
                        >
                          <Power
                            className={cn(
                              "h-3.5 w-3.5 shrink-0",
                              r.enabled
                                ? "text-success"
                                : "text-subtle-foreground",
                            )}
                          />
                          <span className="truncate text-[13px] text-foreground">
                            {r.name}
                          </span>
                          <Badge className="shrink-0 gap-1">
                            <BookOpen className="h-2.5 w-2.5" />
                            <span className="max-w-[160px] truncate">
                              {notebookTitle.get(r.notebookId) ??
                                "Unknown notebook"}
                            </span>
                          </Badge>
                          <span className="ml-auto flex shrink-0 items-center gap-1 text-[11px] text-subtle-foreground">
                            <Clock className="h-2.5 w-2.5" />
                            {intervalLabel(r.intervalSecs)}
                            {r.lastRunAt > 0 ? (
                              <span>· last {relativeTime(r.lastRunAt)}</span>
                            ) : (
                              <span>· never run</span>
                            )}
                            {!r.enabled && <span>· paused</span>}
                          </span>
                        </div>
                      ))}
                  </div>
                </div>
              )}
            </div>
          </div>

          {/* Reports feed: unread first as a continuously scrolling read —
            the homepage doubles as the morning-read surface. */}
          <aside className="hidden min-w-0 flex-1 flex-col border-l border-border lg:flex">
            {reports.length > 0 ? (
              <ReportsFeed
                reports={reports}
                notebookTitle={notebookTitle}
                notebookColor={notebookColor}
                onOpen={openNote}
              />
            ) : (
              <div className="flex flex-1 items-center justify-center p-8">
                <EmptyState
                  icon={<Newspaper className="h-7 w-7" />}
                  title="Reports land here"
                  hint="Schedule a recurring report from any notebook's Studio panel — it refreshes the notebook's URL sources, writes a timestamped note, and shows up on this page to read with your coffee."
                />
              </div>
            )}
          </aside>
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
        title="Rename notebook"
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (renaming) rename(renaming.id, renaming.title);
            setRenaming(null);
          }}
          className={cn("flex flex-col gap-3")}
        >
          <Input
            autoFocus
            value={renaming?.title ?? ""}
            onChange={(e) =>
              setRenaming((r) => (r ? { ...r, title: e.target.value } : r))
            }
          />
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

/** One quiet line: what landed since home was last open. Renders nothing on a
 *  first visit or when nothing happened — silence beats a zero. */
function AwayDigest({
  prevVisit,
  notebooks,
  reports,
}: {
  prevVisit: number;
  notebooks: { updatedAt: number }[];
  reports: Note[];
}) {
  if (!prevVisit) return null;
  const newReports = reports.filter((r) => r.updatedAt > prevVisit).length;
  const updatedNbs = notebooks.filter((n) => n.updatedAt > prevVisit).length;
  const parts = [
    newReports > 0 &&
      `${newReports} new ${newReports === 1 ? "report" : "reports"}`,
    updatedNbs > 0 &&
      `${updatedNbs} ${updatedNbs === 1 ? "notebook" : "notebooks"} updated`,
  ].filter(Boolean);
  if (parts.length === 0) return null;
  return (
    <p className="mt-0.5 text-[12px] text-subtle-foreground">
      Since you were away: {parts.join(" · ")}
    </p>
  );
}

/**
 * The reports feed: unread reports (newest first), then a Show-more button
 * exposing already-read ones five at a time. Read state lives in localStorage
 * keyed by note id and compared to updatedAt, so a rebuilt report goes unread
 * again; a card becomes read once the user scrolls past its end. Grouping is
 * snapshotted per visit — cards don't jump between groups as you read.
 */
function ReportsFeed({
  reports,
  notebookTitle,
  notebookColor,
  onOpen,
}: {
  reports: Note[];
  notebookTitle: Map<string, string>;
  notebookColor: Map<string, string>;
  onOpen: (n: Note) => void;
}) {
  const reads = useStore((s) => s.noteReads);
  const baseline = useStore((s) => s.noteReadsBaseline);
  const markRead = useStore((s) => s.markNotesRead);
  const isUnread = (n: Note) => noteUnread(n, reads, baseline);
  const unreadCount = reports.filter(isUnread).length;

  // Group membership freezes on the first render for this set of reports:
  // marking-as-read updates the badges live, but a card scrolled past must
  // not vanish into the read fold mid-scroll.
  const initialReads = useRef<Record<string, number> | null>(null);
  if (initialReads.current === null) {
    initialReads.current = { ...reads };
  }
  const wasUnread = (n: Note) =>
    noteUnread(n, initialReads.current ?? {}, baseline);
  const unread = reports.filter(wasUnread);
  const read = reports.filter((n) => !wasUnread(n));

  // Read reports stay behind a Show-more fold, five at a time.
  const [readShown, setReadShown] = useState(0);
  const visibleRead = read.slice(0, readShown);
  const remaining = read.length - visibleRead.length;

  return (
    <>
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-6">
        <span className="text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">
          Latest reports
        </span>
        {unreadCount > 0 && (
          <>
            <span className="rounded-full bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium tabular-nums text-citation">
              {unreadCount} unread
            </span>
            <button
              onClick={() =>
                markRead(reports.filter(isUnread).map((n) => n.id))
              }
              className="ml-auto text-[11px] text-muted-foreground transition-colors hover:text-foreground"
            >
              Mark all read
            </button>
          </>
        )}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {unread.length === 0 && (
          <div className="px-6 py-6 text-center text-[12px] text-subtle-foreground">
            You're all caught up.
          </div>
        )}
        {[...unread, ...visibleRead].map((n) => (
          <ReportCard
            key={n.id}
            note={n}
            unread={isUnread(n)}
            onSeen={() => markRead([n.id])}
            notebook={notebookTitle.get(n.notebookId) ?? "Unknown notebook"}
            color={notebookColor.get(n.notebookId) || NOTEBOOK_PALETTE[0]}
            onOpen={() => {
              markRead([n.id]);
              onOpen(n);
            }}
          />
        ))}
        {remaining > 0 && (
          <div className="flex justify-center px-6 py-5">
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setReadShown((s) => s + 5)}
            >
              Load older reports
            </Button>
          </div>
        )}
      </div>
    </>
  );
}

/** One report in the feed; read once the user has scrolled past its end. */
function ReportCard({
  note,
  unread,
  onSeen,
  notebook,
  color,
  onOpen,
}: {
  note: Note;
  unread: boolean;
  onSeen: () => void;
  notebook: string;
  color: string;
  onOpen: () => void;
}) {
  // A marker at the card's end: once it enters the pane, the user has reached
  // (or scrolled past) the end of this report. Short reports fully on screen
  // are read on sight; the last report is read when the feed bottoms out.
  const endRef = useRef<HTMLDivElement>(null);
  const seenRef = useRef(onSeen);
  seenRef.current = onSeen;

  useEffect(() => {
    const el = endRef.current;
    if (!el || !unread) return;
    const io = new IntersectionObserver(([e]) => {
      if (e.isIntersecting) seenRef.current();
    });
    io.observe(el);
    return () => io.disconnect();
  }, [unread]);

  return (
    <article
      className={cn(
        "border-b border-border px-6 py-5",
        unread && "border-l-2 border-l-primary bg-primary/[0.04]",
      )}
    >
      <div className="flex items-center gap-1.5 text-[11px] text-subtle-foreground">
        <span
          className="inline-flex h-2 w-2 shrink-0 rounded-full"
          style={{ backgroundColor: color }}
          aria-hidden="true"
        />
        <span className="truncate">{notebook}</span>
        <span>·</span>
        <span className="shrink-0">{relativeTime(note.updatedAt)}</span>
        {unread && (
          <span className="ml-auto shrink-0 rounded-full bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium text-citation">
            new
          </span>
        )}
      </div>
      <button
        onClick={onOpen}
        className="mt-1 block w-full text-left"
        title={`Open in "${notebook}"`}
      >
        <h3 className="text-[15px] font-semibold text-foreground hover:underline">
          {note.title}
        </h3>
      </button>
      <div className="mt-2 text-[13px] leading-relaxed">
        <Markdown>{note.content}</Markdown>
      </div>
      <div ref={endRef} aria-hidden="true" />
    </article>
  );
}
