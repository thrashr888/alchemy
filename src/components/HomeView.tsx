import { useEffect, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Button, Input, Modal, Badge, useConfirm } from "./ui";
import { AlchemyHero } from "./AlchemyHero";
import { intervalLabel } from "./Reports";
import { cn, relativeTime, providerStatus, cardButtonProps, shortcutBlocked } from "@/lib/utils";
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
  CheckCircle2,
  Circle,
  FileText,
} from "lucide-react";

export function HomeView({ onOpenSettings }: { onOpenSettings: () => void }) {
  const notebooks = useStore((s) => s.notebooks);
  const ollamaOk = useStore((s) => s.ollamaOk);
  const aiConfig = useStore((s) => s.aiConfig);
  const modelHealth = useStore((s) => s.modelHealth);
  const provider = providerStatus(aiConfig, ollamaOk, modelHealth);
  const open = useStore((s) => s.selectNotebook);
  const create = useStore((s) => s.createNotebook);
  const rename = useStore((s) => s.renameNotebook);
  const remove = useStore((s) => s.deleteNotebook);
  const theme = useStore((s) => s.theme);

  const [creating, setCreating] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [renaming, setRenaming] = useState<{ id: string; title: string } | null>(null);
  const { confirm, dialog: confirmDialog } = useConfirm();

  // All scheduled reports, recent documents, and corpus totals across
  // notebooks — the app's ongoing activity, refreshed with the notebook list.
  const [allReports, setAllReports] = useState<ReportSchedule[]>([]);
  const [recentNotes, setRecentNotes] = useState<Note[]>([]);
  const [stats, setStats] = useState<CorpusStats | null>(null);
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
      .corpusStats()
      .then(setStats)
      .catch(() => setStats(null));
  }, [notebooks]);
  const notebookTitle = new Map(notebooks.map((n) => [n.id, n.title]));

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
        <span className="text-[15px] font-semibold tracking-tight">Alchemy</span>
        <div className="ml-auto flex items-center gap-3">
          <div className="flex items-center gap-1.5">
            {provider.ok === null ? (
              <Circle className="h-2.5 w-2.5 text-subtle-foreground" />
            ) : provider.ok ? (
              <CheckCircle2 className="h-3.5 w-3.5 text-success" />
            ) : (
              <Circle className="h-2.5 w-2.5 fill-destructive text-destructive" />
            )}
            <span className="text-[11px] text-muted-foreground">
              {provider.ok === null
                ? "Checking…"
                : provider.ok
                  ? `${provider.label} connected`
                  : `${provider.label} offline`}
            </span>
          </div>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => useStore.getState().setPaletteOpen(true)}
            title="Search & commands (⌘K)"
            aria-label="Open the command menu"
          >
            <Search className="h-4 w-4" />
          </Button>
          <Button variant="ghost" size="icon" onClick={onOpenSettings} title="Settings">
            <Settings className="h-4 w-4" />
          </Button>
        </div>
      </header>

      {notebooks.length === 0 ? (
        <div className="flex-1">
          <AlchemyHero
            title="Alchemy"
            subtitle="Local-first research notebooks — chat with your own sources, grounded in citations, running entirely on your machine."
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
        <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-[960px] px-6 py-10">
          <div className="mb-6 flex items-end justify-between">
            <div>
              <h1 className="text-[22px] font-semibold tracking-tight">Your notebooks</h1>
              <p className="mt-1 text-[13px] text-muted-foreground">
                {stats
                  ? `${notebooks.length} ${notebooks.length === 1 ? "notebook" : "notebooks"} · ${stats.sources} ${stats.sources === 1 ? "source" : "sources"} · ${Intl.NumberFormat().format(stats.chars)} chars indexed`
                  : "Most recently used first."}
              </p>
            </div>
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
                <div className="mb-auto flex h-8 w-8 items-center justify-center rounded-lg bg-primary/12 text-primary">
                  <BookOpen className="h-4 w-4" />
                </div>
                <div className="mt-3 truncate text-[14px] font-medium" title={nb.title}>
                  {nb.title}
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
                          message: "This permanently deletes the notebook and all of its sources.",
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
              </div>
            ))}
          </div>

          {/* The last few generated/edited documents across all notebooks. */}
          {recentNotes.length > 0 && (
            <div className="mt-10">
              <div className="mb-2 text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
                Recent documents
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
                    <span className="truncate text-[13px] text-foreground">{n.title}</span>
                    <Badge className="shrink-0 gap-1">
                      <BookOpen className="h-2.5 w-2.5" />
                      <span className="max-w-[160px] truncate">
                        {notebookTitle.get(n.notebookId) ?? "Unknown notebook"}
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
                    a.enabled !== b.enabled ? (a.enabled ? -1 : 1) : b.lastRunAt - a.lastRunAt,
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
                          r.enabled ? "text-success" : "text-subtle-foreground",
                        )}
                      />
                      <span className="truncate text-[13px] text-foreground">{r.name}</span>
                      <Badge className="shrink-0 gap-1">
                        <BookOpen className="h-2.5 w-2.5" />
                        <span className="max-w-[160px] truncate">
                          {notebookTitle.get(r.notebookId) ?? "Unknown notebook"}
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
      )}

      <Modal open={creating} onClose={() => setCreating(false)} title="New notebook">
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
            <Button type="button" variant="ghost" onClick={() => setCreating(false)}>
              Cancel
            </Button>
            <Button type="submit" variant="primary">
              Create & open
            </Button>
          </div>
        </form>
      </Modal>

      {confirmDialog}

      <Modal open={!!renaming} onClose={() => setRenaming(null)} title="Rename notebook">
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
            onChange={(e) => setRenaming((r) => (r ? { ...r, title: e.target.value } : r))}
          />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setRenaming(null)}>
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
