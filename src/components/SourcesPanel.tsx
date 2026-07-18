import { useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import {
  Button,
  Input,
  Textarea,
  Modal,
  EmptyState,
  ResizeHandle,
  RowMenu,
  Spinner,
  CardAction,
  useConfirm,
} from "./ui";
import { cn, isWebUrl } from "@/lib/utils";
import type { Source } from "@/lib/types";
import {
  FileText,
  FileType,
  Globe,
  Hash,
  Plus,
  PanelLeftClose,
  Trash2,
  Upload,
  Check,
  AlertCircle,
  X,
  Pencil,
  RefreshCw,
  Image as ImageIcon,
  Folder,
  Cloud,
  CodeXml,
  Command,
  Calendar,
  ListChecks,
  NotebookText,
  TrendingUp,
} from "lucide-react";

// Soft per-notebook capacity used for the "how full is this notebook" gauge.
// ~1M chars ≈ ~250k tokens — generous for local RAG over many documents.
const MAX_NOTEBOOK_CHARS = 1_000_000;

export function sourceIcon(t: Source["sourceType"], url?: string) {
  // Mac sources show the app they mirror (same icons as the add-source
  // modal's provider tiles), in that app's signature color.
  if (t === "mac" && url) {
    if (url.startsWith("cider://calendar/"))
      return <Calendar className="h-3.5 w-3.5 text-[#eb5757]" />;
    if (url.startsWith("cider://reminders/"))
      return <ListChecks className="h-3.5 w-3.5 text-[#e8a33d]" />;
    if (url.startsWith("cider://notes/"))
      return <NotebookText className="h-3.5 w-3.5 text-[#e5c454]" />;
    if (url.startsWith("cider://stocks/"))
      return <TrendingUp className="h-3.5 w-3.5 text-[#4cb782]" />;
  }
  switch (t) {
    case "pdf":
      return <FileType className="h-3.5 w-3.5 text-[#eb5757]" />;
    case "url":
      return <Globe className="h-3.5 w-3.5 text-[#5e9bd2]" />;
    case "markdown":
      return <Hash className="h-3.5 w-3.5 text-[#9b87f5]" />;
    case "image":
      return <ImageIcon className="h-3.5 w-3.5 text-[#4cb782]" />;
    case "folder":
      return <Folder className="h-3.5 w-3.5 text-[#e8a33d]" />;
    case "mac":
      return <Command className="h-3.5 w-3.5 text-[#5ec2c2]" />;
    case "html":
      return <CodeXml className="h-3.5 w-3.5 text-[#5e9bd2]" />;
    default:
      return <FileText className="h-3.5 w-3.5 text-muted-foreground" />;
  }
}

/** Source-domain favicon with a Globe fallback (kept local — no third party). */
export function Favicon({ url }: { url: string }) {
  const [failed, setFailed] = useState(false);
  let origin = "";
  try {
    origin = new URL(url).origin;
  } catch {
    /* malformed */
  }
  if (failed || !origin)
    return <Globe className="h-3.5 w-3.5 text-[#5e9bd2]" />;
  return (
    <img
      src={`${origin}/favicon.ico`}
      alt=""
      className="h-3.5 w-3.5 rounded-sm object-contain"
      onError={() => setFailed(true)}
    />
  );
}

function hostname(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/** Compact selection checkbox; supports the folder/master indeterminate state.
 *  Clicks stop propagating so the row's open-reader handler never fires. */
function SelectBox({
  checked,
  indeterminate = false,
  onToggle,
  label,
}: {
  checked: boolean;
  indeterminate?: boolean;
  onToggle: () => void;
  label: string;
}) {
  return (
    <input
      type="checkbox"
      ref={(el) => {
        if (el) el.indeterminate = indeterminate && !checked;
      }}
      checked={checked}
      onChange={onToggle}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => e.stopPropagation()}
      title={label}
      aria-label={label}
      className="select-quiet"
    />
  );
}

export function SourcesPanel() {
  const notebookColor = useStore(
    (s) => s.notebooks.find((n) => n.id === s.currentId)?.color,
  );
  const sources = useStore((s) => s.sources);
  const currentId = useStore((s) => s.currentId);
  const queue = useStore((s) => s.ingestQueue);
  const clearQueueItem = useStore((s) => s.clearQueueItem);
  const openAddSource = useStore((s) => s.openAddSource);
  const folderScan = useStore((s) => s.folderScan);
  const editSourceText = useStore((s) => s.editSourceText);
  const updateMacNote = useStore((s) => s.updateMacNote);
  const addMacReminder = useStore((s) => s.addMacReminder);
  const refreshSource = useStore((s) => s.refreshSource);
  const deleteSource = useStore((s) => s.deleteSource);
  const draggingFiles = useStore((s) => s.draggingFiles);
  const toggleSources = useStore((s) => s.toggleSources);
  const openSourceViewer = useStore((s) => s.openSourceViewer);
  const selectedSourceIds = useStore((s) => s.selectedSourceIds);
  const toggleSourceSelected = useStore((s) => s.toggleSourceSelected);
  const setAllSourcesSelected = useStore((s) => s.setAllSourcesSelected);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const [editing, setEditing] = useState<{
    id: string;
    title: string;
    text: string;
    /** Editing the Apple Note itself — save writes back through cider. */
    macNote?: boolean;
  } | null>(null);
  const [addingReminder, setAddingReminder] = useState<{
    sourceId: string;
    list: string;
  } | null>(null);

  async function startEdit(s: Source) {
    // List payloads omit content; fetch the full text to prefill the editor.
    const content = await api.getSourceContent(s.id);
    setEditing({ id: s.id, title: s.title, text: content });
  }

  async function startEditMacNote(s: Source) {
    // The real note body (first line is the title — Notes derives the visible
    // title from it), not our rendered markdown copy.
    const body = await api.macNoteBody(s.id);
    setEditing({ id: s.id, title: s.title, text: body, macNote: true });
  }

  const totalChars = sources.reduce((sum, s) => sum + s.charCount, 0);
  const pct = Math.min(100, (totalChars / MAX_NOTEBOOK_CHARS) * 100);

  // Folder children render indented under their folder; everything else is a
  // flat top-level row.
  const rows: { s: Source; indent: boolean }[] = [];
  for (const s of sources) {
    if (s.parentId) continue;
    rows.push({ s, indent: false });
    if (s.sourceType === "folder") {
      for (const c of sources.filter((x) => x.parentId === s.id)) {
        rows.push({ s: c, indent: true });
      }
    }
  }
  const childCount = (folderId: string) =>
    sources.filter((x) => x.parentId === folderId).length;

  // Selection: null means everything is on; the map holds only deselected ids.
  const isSelected = (id: string) =>
    !selectedSourceIds || selectedSourceIds[id] !== false;
  // Folder container rows have no chunks — only content sources count.
  const contentSources = sources.filter((s) => s.sourceType !== "folder");
  const selectedCount = contentSources.filter((s) => isSelected(s.id)).length;
  const allSelected = selectedCount === contentSources.length;

  /** Tri-state folder toggle: partial/none → select all children; all → none. */
  function toggleFolderSelected(folderId: string) {
    const kids = sources.filter((x) => x.parentId === folderId);
    const target = !kids.every((k) => isSelected(k.id));
    for (const k of kids) {
      if (isSelected(k.id) !== target) toggleSourceSelected(k.id);
    }
  }

  const width = useStore((s) => s.sourcesWidth);
  const setPanelWidth = useStore((s) => s.setPanelWidth);

  return (
    <div
      style={{ width }}
      className="relative flex h-full shrink-0 flex-col border-r border-border bg-surface"
    >
      <ResizeHandle
        edge="right"
        width={width}
        defaultWidth={280}
        onResize={(w) => setPanelWidth("sources", w)}
        label="Resize sources panel"
      />
      {draggingFiles && currentId && (
        <div className="pointer-events-none absolute inset-1.5 z-30 flex flex-col items-center justify-center gap-2 rounded-lg border-2 border-dashed border-primary/60 bg-primary/10">
          <Upload className="h-6 w-6 text-primary" />
          <span className="text-[13px] font-semibold text-foreground">
            Drop to add sources
          </span>
          <span className="text-[11px] text-muted-foreground">
            PDF · Office · images · text
          </span>
        </div>
      )}
      <div className="flex items-center px-4 h-12 border-b border-border">
        <span className="text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">
          Sources
        </span>
        <span className="ml-2 text-[11px] text-subtle-foreground">
          {sources.length}
        </span>
        <div className="ml-auto flex items-center gap-0.5">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => openAddSource()}
            disabled={!currentId}
            title="Add source"
            aria-label="Add source"
          >
            <Plus className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={toggleSources}
            title="Collapse sources"
            aria-label="Collapse sources"
          >
            <PanelLeftClose className="h-4 w-4" />
          </Button>
        </div>
      </div>

      {/* Notebook capacity gauge */}
      {sources.length > 0 && (
        <div className="border-b border-border px-4 py-2.5">
          <div className="mb-1.5 flex items-center justify-between text-[11px]">
            <span className="text-muted-foreground">
              {Intl.NumberFormat().format(totalChars)} chars
            </span>
            <span className="text-subtle-foreground">
              {pct < 1 ? "<1" : Math.round(pct)}% of capacity
            </span>
          </div>
          <div className="h-1.5 overflow-hidden rounded-full bg-surface-2">
            {/* The notebook's color carries into its gauge — the one place the
                color lives inside the workspace besides the title dot. */}
            <div
              className={cn(
                "h-full rounded-full transition-all",
                pct > 90 && "bg-destructive",
              )}
              style={{
                width: `${Math.max(2, pct)}%`,
                ...(pct <= 90 && notebookColor
                  ? { backgroundColor: notebookColor }
                  : {}),
              }}
            />
          </div>
        </div>
      )}

      <div className="flex-1 overflow-y-auto p-2">
        {/* Active upload queue */}
        {queue.length > 0 && (
          <div className="mb-2 flex flex-col gap-1">
            {queue.map((q) => (
              <div
                key={q.id}
                className="flex items-start gap-2 rounded-md border border-border bg-surface-2/60 px-2 py-2"
              >
                <div className="mt-0.5">
                  {q.status === "done" ? (
                    <Check className="h-3.5 w-3.5 text-success" />
                  ) : q.status === "error" ? (
                    <AlertCircle className="h-3.5 w-3.5 text-destructive" />
                  ) : (
                    <Spinner className="h-3.5 w-3.5 text-muted-foreground" />
                  )}
                </div>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[12px]" title={q.name}>
                    {q.name}
                  </div>
                  <div
                    className={cn(
                      "text-[11px]",
                      q.status === "error"
                        ? "text-destructive"
                        : "text-subtle-foreground",
                    )}
                  >
                    {q.status === "processing"
                      ? folderScan
                        ? `Embedding ${Math.min(folderScan.done + 1, folderScan.total)}/${folderScan.total}: ${folderScan.title}`
                        : "Embedding…"
                      : q.status === "pending"
                        ? "Queued"
                        : q.status === "done"
                          ? "Added"
                          : q.error}
                  </div>
                </div>
                {q.status === "error" && (
                  <button
                    className="rounded p-0.5 text-muted-foreground hover:text-foreground"
                    onClick={() => clearQueueItem(q.id)}
                    title="Dismiss"
                    aria-label={`Dismiss failed import "${q.name}"`}
                  >
                    <X className="h-3.5 w-3.5" />
                  </button>
                )}
              </div>
            ))}
          </div>
        )}

        {!currentId ? (
          <EmptyState title="No notebook selected" />
        ) : sources.length === 0 && queue.length === 0 ? (
          <EmptyState
            icon={<FileText className="h-7 w-7" />}
            title="No sources yet"
            hint="Upload PDFs, Office files, CSVs, images, or markdown; add a folder (it stays in sync — great for OneDrive/Dropbox); add a URL (Google Docs, Sheets & Slides work); or paste text. You can also drag files or folders onto the window."
          />
        ) : (
          <>
            {/* Master selection row: which sources feed chat & Studio. */}
            <div className="mb-0.5 flex items-center gap-2 px-2 py-1.5">
              <span className="text-[11px] text-muted-foreground">
                {selectedCount} of {contentSources.length} selected
              </span>
              <div className="ml-auto">
                <SelectBox
                  checked={allSelected}
                  indeterminate={selectedCount > 0 && !allSelected}
                  onToggle={() => setAllSourcesSelected(!allSelected)}
                  label={
                    allSelected ? "Deselect all sources" : "Select all sources"
                  }
                />
              </div>
            </div>
            <div className="flex flex-col gap-0.5">
              {rows.map(({ s, indent }) => {
                const isFolder = s.sourceType === "folder";
                const isMacNote = s.url.startsWith("cider://notes/note/");
                const isMacReminders = s.url.startsWith(
                  "cider://reminders/list/",
                );
                const readable = s.status === "ready" && !isFolder;
                const kids = isFolder
                  ? sources.filter((x) => x.parentId === s.id)
                  : [];
                const kidsOn = kids.filter((k) => isSelected(k.id)).length;
                return (
                  <div
                    key={s.id}
                    // Row content is pointer-events-none (clicks go to the
                    // CardAction), so the row carries the hover detail the
                    // truncated children can no longer show.
                    title={[
                      s.title,
                      s.status === "error"
                        ? s.error || "Import failed"
                        : s.url || undefined,
                      readable ? "Read source" : undefined,
                    ]
                      .filter(Boolean)
                      .join("\n")}
                    className={cn(
                      // has-: an open row menu must outrank the z-10/z-20
                      // content of the rows after it (they'd paint over the
                      // dropdown otherwise — later DOM order wins at equal z).
                      "group relative flex items-start gap-2 rounded-md px-2 py-2 hover:bg-surface-2 has-[[aria-expanded=true]]:z-30",
                      s.status === "error" && "bg-destructive/5",
                      readable && "cursor-pointer",
                      indent && "ml-5",
                    )}
                  >
                    {readable && (
                      <CardAction
                        label={`Read source ${s.title}`}
                        onClick={() => openSourceViewer(s.id, s.title)}
                      />
                    )}
                    <div className="pointer-events-none relative z-10 mt-0.5">
                      {s.status === "error" ? (
                        <AlertCircle className="h-3.5 w-3.5 text-destructive" />
                      ) : s.status === "placeholder" ? (
                        <Cloud className="h-3.5 w-3.5 text-subtle-foreground" />
                      ) : s.sourceType === "url" && s.url ? (
                        <Favicon url={s.url} />
                      ) : (
                        sourceIcon(s.sourceType, s.url)
                      )}
                    </div>
                    <div className="pointer-events-none relative z-10 min-w-0 flex-1">
                      {/* The ⋯ menu lives in the title row: hovering shortens the
                      title but never reflows the metadata line below. */}
                      <div className="flex items-center gap-1">
                        <span
                          className={cn(
                            "min-w-0 flex-1 truncate text-[13px]",
                            s.status === "placeholder"
                              ? "text-muted-foreground"
                              : "text-foreground",
                          )}
                          title={s.title}
                        >
                          {s.title}
                        </span>
                        <RowMenu
                          className="pointer-events-auto z-20"
                          label={`Options for "${s.title}"`}
                          items={[
                            // url holds the origin: a web URL, an on-disk path, or
                            // a folder — any of them can be refreshed.
                            ...(s.url
                              ? [
                                  {
                                    label: isFolder
                                      ? "Rescan folder now"
                                      : s.sourceType === "mac"
                                        ? "Sync now"
                                        : s.status === "placeholder"
                                          ? "Download & embed"
                                          : isWebUrl(s.url)
                                            ? "Refresh from URL"
                                            : "Refresh from file",
                                    icon: <RefreshCw className="h-3.5 w-3.5" />,
                                    onClick: () => void refreshSource(s.id),
                                  },
                                ]
                              : []),
                            // Mac sources are mirrors — editing our copy would
                            // just be overwritten, so writes go to the app
                            // itself and sync back.
                            ...(isMacNote
                              ? [
                                  {
                                    label: "Edit note",
                                    icon: <Pencil className="h-3.5 w-3.5" />,
                                    onClick: () => void startEditMacNote(s),
                                  },
                                ]
                              : []),
                            ...(isMacReminders
                              ? [
                                  {
                                    label: "Add reminder…",
                                    icon: <Plus className="h-3.5 w-3.5" />,
                                    onClick: () =>
                                      setAddingReminder({
                                        sourceId: s.id,
                                        list: s.title,
                                      }),
                                  },
                                ]
                              : []),
                            ...(s.sourceType !== "url" &&
                            s.sourceType !== "mac" &&
                            !isFolder &&
                            s.status !== "placeholder"
                              ? [
                                  {
                                    label: "Edit text",
                                    icon: <Pencil className="h-3.5 w-3.5" />,
                                    onClick: () => void startEdit(s),
                                  },
                                ]
                              : []),
                            {
                              label: "Remove",
                              icon: <Trash2 className="h-3.5 w-3.5" />,
                              danger: true,
                              onClick: async () => {
                                if (
                                  await confirm({
                                    title: `Remove "${s.title}"?`,
                                    message: isFolder
                                      ? `This removes the folder and its ${childCount(s.id)} file sources (with their embedded chunks) from the notebook. Nothing on disk is touched.`
                                      : "This deletes the source and its embedded chunks from the notebook.",
                                    confirmLabel: "Remove",
                                    danger: true,
                                  })
                                )
                                  deleteSource(s.id);
                              },
                            },
                          ]}
                        />
                      </div>
                      {s.status === "error" ? (
                        <div
                          className="text-[11px] leading-snug text-destructive"
                          title={s.error}
                        >
                          {s.error || "Import failed"}
                        </div>
                      ) : s.status === "placeholder" ? (
                        <div
                          className="text-[11px] text-subtle-foreground"
                          title={s.url}
                        >
                          Online-only — not downloaded
                        </div>
                      ) : isFolder ? (
                        <div
                          className="truncate text-[11px] text-subtle-foreground"
                          title={s.url}
                        >
                          {childCount(s.id)} files · auto-refreshes
                        </div>
                      ) : s.sourceType === "url" && s.url ? (
                        <div
                          className="truncate text-[11px] text-citation"
                          title={s.url}
                        >
                          {hostname(s.url)}
                        </div>
                      ) : (
                        <div className="text-[11px] text-subtle-foreground">
                          {s.chunkCount} chunks ·{" "}
                          {Intl.NumberFormat().format(s.charCount)} chars
                        </div>
                      )}
                    </div>
                    {/* Selection stays at the far right (NotebookLM-style), always
                    visible. */}
                    <div className="relative z-20 mt-0.5">
                      {isFolder ? (
                        <SelectBox
                          checked={kids.length > 0 && kidsOn === kids.length}
                          indeterminate={kidsOn > 0 && kidsOn < kids.length}
                          onToggle={() => toggleFolderSelected(s.id)}
                          label={`Include "${s.title}" files in chat & generation`}
                        />
                      ) : (
                        <SelectBox
                          checked={isSelected(s.id)}
                          onToggle={() => toggleSourceSelected(s.id)}
                          label={`Include "${s.title}" in chat & generation`}
                        />
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          </>
        )}
      </div>

      <Modal
        open={!!editing}
        onClose={() => setEditing(null)}
        title={editing?.macNote ? "Edit Apple Note" : "Edit source"}
        width="max-w-lg"
      >
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            if (!editing) return;
            const { id, title, text, macNote } = editing;
            setEditing(null);
            if (macNote) await updateMacNote(id, text);
            else await editSourceText(id, title, text);
          }}
          className="flex flex-col gap-3"
        >
          {/* The note's title IS its first line — no separate title field. */}
          {!editing?.macNote && (
            <Input
              autoFocus
              name="source-title"
              aria-label="Source title"
              placeholder="Title"
              value={editing?.title ?? ""}
              onChange={(e) =>
                setEditing((s) => (s ? { ...s, title: e.target.value } : s))
              }
            />
          )}
          <Textarea
            autoFocus={editing?.macNote}
            rows={12}
            name="source-text"
            aria-label={editing?.macNote ? "Apple Note text" : "Source text"}
            placeholder="Source text…"
            value={editing?.text ?? ""}
            onChange={(e) =>
              setEditing((s) => (s ? { ...s, text: e.target.value } : s))
            }
          />
          {editing?.macNote && (
            <p className="text-[11px] leading-relaxed text-subtle-foreground">
              Saves straight into Apple Notes — the first line is the note's
              title.
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setEditing(null)}
            >
              Cancel
            </Button>
            <Button
              type="submit"
              variant="primary"
              disabled={!editing?.text.trim()}
            >
              {editing?.macNote ? "Save to Apple Notes" : "Save"}
            </Button>
          </div>
        </form>
      </Modal>

      <Modal
        open={!!addingReminder}
        onClose={() => setAddingReminder(null)}
        title={`Add reminder to "${addingReminder?.list ?? ""}"`}
        width="max-w-md"
      >
        <AddReminderForm
          key={addingReminder?.sourceId ?? "none"}
          onSubmit={async (title, notes) => {
            if (!addingReminder) return;
            const { sourceId } = addingReminder;
            setAddingReminder(null);
            await addMacReminder(sourceId, title, notes);
          }}
          onCancel={() => setAddingReminder(null)}
        />
      </Modal>

      {confirmDialog}
    </div>
  );
}

/** Title + optional notes for a new reminder in a connected list. */
function AddReminderForm({
  onSubmit,
  onCancel,
}: {
  onSubmit: (title: string, notes?: string) => void;
  onCancel: () => void;
}) {
  const [title, setTitle] = useState("");
  const [notes, setNotes] = useState("");
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (title.trim()) onSubmit(title.trim(), notes.trim() || undefined);
      }}
      className="flex flex-col gap-3"
    >
      <Input
        autoFocus
        name="reminder-title"
        aria-label="Reminder title"
        placeholder="Remind me to…"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
      />
      <Textarea
        rows={3}
        name="reminder-notes"
        aria-label="Reminder notes"
        placeholder="Notes (optional)"
        value={notes}
        onChange={(e) => setNotes(e.target.value)}
      />
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={!title.trim()}>
          Add reminder
        </Button>
      </div>
    </form>
  );
}
