import { useEffect, useRef, useState, type ReactNode } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Button, Input, Textarea, Modal, EmptyState, ResizeHandle, Spinner, useConfirm } from "./ui";
import { cn, cardButtonProps, isWebUrl } from "@/lib/utils";
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
  Link2,
  ClipboardPaste,
  Check,
  AlertCircle,
  X,
  Pencil,
  RefreshCw,
  Image as ImageIcon,
} from "lucide-react";

// Soft per-notebook capacity used for the "how full is this notebook" gauge.
// ~1M chars ≈ ~250k tokens — generous for local RAG over many documents.
const MAX_NOTEBOOK_CHARS = 1_000_000;

export function sourceIcon(t: Source["sourceType"]) {
  switch (t) {
    case "pdf":
      return <FileType className="h-3.5 w-3.5 text-[#eb5757]" />;
    case "url":
      return <Globe className="h-3.5 w-3.5 text-[#5e9bd2]" />;
    case "markdown":
      return <Hash className="h-3.5 w-3.5 text-[#9b87f5]" />;
    case "image":
      return <ImageIcon className="h-3.5 w-3.5 text-[#4cb782]" />;
    default:
      return <FileText className="h-3.5 w-3.5 text-muted-foreground" />;
  }
}

/** Source-domain favicon with a Globe fallback (kept local — no third party). */
function Favicon({ url }: { url: string }) {
  const [failed, setFailed] = useState(false);
  let origin = "";
  try {
    origin = new URL(url).origin;
  } catch {
    /* malformed */
  }
  if (failed || !origin) return <Globe className="h-3.5 w-3.5 text-[#5e9bd2]" />;
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

type AddMode = null | "url" | "text";

export function SourcesPanel() {
  const sources = useStore((s) => s.sources);
  const currentId = useStore((s) => s.currentId);
  const queue = useStore((s) => s.ingestQueue);
  const clearQueueItem = useStore((s) => s.clearQueueItem);
  const pickAndAddFiles = useStore((s) => s.pickAndAddFiles);
  const addUrl = useStore((s) => s.addSourceUrl);
  const addText = useStore((s) => s.addSourceText);
  const editSourceText = useStore((s) => s.editSourceText);
  const refreshSource = useStore((s) => s.refreshSource);
  const deleteSource = useStore((s) => s.deleteSource);
  const draggingFiles = useStore((s) => s.draggingFiles);
  const toggleSources = useStore((s) => s.toggleSources);
  const openSourceViewer = useStore((s) => s.openSourceViewer);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTriggerRef = useRef<HTMLButtonElement>(null);
  const [mode, setMode] = useState<AddMode>(null);

  // Menu keyboard behavior: focus first item on open, arrows cycle, Escape closes.
  useEffect(() => {
    if (menuOpen) menuRef.current?.querySelector<HTMLElement>("button")?.focus();
  }, [menuOpen]);

  function onMenuKey(e: React.KeyboardEvent) {
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? []);
    const idx = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") {
      e.stopPropagation();
      setMenuOpen(false);
      menuTriggerRef.current?.focus();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      items[(idx + 1) % items.length]?.focus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      items[(idx - 1 + items.length) % items.length]?.focus();
    }
  }
  const [url, setUrl] = useState("");
  const [pasteTitle, setPasteTitle] = useState("");
  const [pasteText, setPasteText] = useState("");
  const [editing, setEditing] = useState<{ id: string; title: string; text: string } | null>(null);

  // "Add source from URL" command from the Cmd+K menu. A store flag rather
  // than an event: the panel may be mid-mount when the command runs.
  const pendingAddUrl = useStore((s) => s.pendingAddUrl);
  useEffect(() => {
    if (pendingAddUrl) {
      useStore.setState({ pendingAddUrl: false });
      setUrl("");
      setMode("url");
    }
  }, [pendingAddUrl]);

  async function startEdit(s: Source) {
    // List payloads omit content; fetch the full text to prefill the editor.
    const content = await api.getSourceContent(s.id);
    setEditing({ id: s.id, title: s.title, text: content });
  }

  const totalChars = sources.reduce((sum, s) => sum + s.charCount, 0);
  const pct = Math.min(100, (totalChars / MAX_NOTEBOOK_CHARS) * 100);

  async function pickFiles() {
    setMenuOpen(false);
    await pickAndAddFiles();
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
          <span className="text-[13px] font-semibold text-foreground">Drop to add sources</span>
          <span className="text-[11px] text-muted-foreground">PDF · Office · images · text</span>
        </div>
      )}
      <div className="flex items-center px-4 h-12 border-b border-border">
        <span className="text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">
          Sources
        </span>
        <span className="ml-2 text-[11px] text-subtle-foreground">{sources.length}</span>
        <div className="ml-auto flex items-center gap-0.5">
        <div className="relative">
          <Button
            ref={menuTriggerRef}
            variant="ghost"
            size="icon"
            onClick={() => setMenuOpen((o) => !o)}
            disabled={!currentId}
            title="Add source"
            aria-label="Add source"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
          >
            <Plus className="h-4 w-4" />
          </Button>
          {menuOpen && (
            <>
              <div className="fixed inset-0 z-10" onClick={() => setMenuOpen(false)} />
              <div
                ref={menuRef}
                role="menu"
                aria-label="Add source"
                onKeyDown={onMenuKey}
                className="absolute right-0 top-8 z-20 w-44 overflow-hidden rounded-md bg-elevated py-1 shadow-[0_0_0_0.5px_var(--border-strong),0_8px_24px_-6px_rgba(0,0,0,0.4)]"
              >
                <MenuItem icon={<Upload className="h-3.5 w-3.5" />} label="Upload files" onClick={pickFiles} />
                <MenuItem
                  icon={<Link2 className="h-3.5 w-3.5" />}
                  label="From URL"
                  onClick={() => {
                    setMenuOpen(false);
                    setUrl("");
                    setMode("url");
                  }}
                />
                <MenuItem
                  icon={<ClipboardPaste className="h-3.5 w-3.5" />}
                  label="Paste text"
                  onClick={() => {
                    setMenuOpen(false);
                    setPasteTitle("");
                    setPasteText("");
                    setMode("text");
                  }}
                />
              </div>
            </>
          )}
        </div>
        <Button variant="ghost" size="icon" onClick={toggleSources} title="Collapse sources">
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
            <div
              className={cn(
                "h-full rounded-full transition-all",
                pct > 90 ? "bg-destructive" : "bg-primary",
              )}
              style={{ width: `${Math.max(2, pct)}%` }}
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
                      q.status === "error" ? "text-destructive" : "text-subtle-foreground",
                    )}
                  >
                    {q.status === "processing"
                      ? "Embedding…"
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
            hint="Upload PDFs, Office files, CSVs, images, or markdown; add a URL (Google Docs, Sheets & Slides work); or paste text. You can also drag files onto the window."
          />
        ) : (
          <div className="flex flex-col gap-0.5">
            {sources.map((s) => (
              <div
                key={s.id}
                onClick={() => {
                  if (s.status !== "error") openSourceViewer(s.id, s.title);
                }}
                {...(s.status !== "error"
                  ? cardButtonProps(() => openSourceViewer(s.id, s.title))
                  : {})}
                title={s.status !== "error" ? "Read source" : undefined}
                className={cn(
                  "group flex items-start gap-2 rounded-md px-2 py-2 hover:bg-surface-2",
                  s.status === "error" ? "bg-destructive/5" : "cursor-pointer",
                )}
              >
                <div className="mt-0.5">
                  {s.status === "error" ? (
                    <AlertCircle className="h-3.5 w-3.5 text-destructive" />
                  ) : s.sourceType === "url" && s.url ? (
                    <Favicon url={s.url} />
                  ) : (
                    sourceIcon(s.sourceType)
                  )}
                </div>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[13px] text-foreground" title={s.title}>
                    {s.title}
                  </div>
                  {s.status === "error" ? (
                    <div className="text-[11px] leading-snug text-destructive" title={s.error}>
                      {s.error || "Import failed"}
                    </div>
                  ) : s.sourceType === "url" && s.url ? (
                    <div className="truncate text-[11px] text-citation" title={s.url}>
                      {hostname(s.url)}
                    </div>
                  ) : (
                    <div className="text-[11px] text-subtle-foreground">
                      {s.chunkCount} chunks · {Intl.NumberFormat().format(s.charCount)} chars
                    </div>
                  )}
                </div>
                <div className="flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
                  {/* url holds the origin: a web URL or, for file imports,
                      the on-disk path — either way it can be refreshed. */}
                  {s.url && (
                    <button
                      className="rounded p-1 text-muted-foreground hover:text-foreground"
                      onClick={(e) => {
                        e.stopPropagation();
                        refreshSource(s.id);
                      }}
                      title={
                        isWebUrl(s.url)
                          ? "Refresh from URL (re-fetch & re-embed)"
                          : "Refresh from file (re-read & re-embed)"
                      }
                      aria-label={`Refresh "${s.title}" from ${isWebUrl(s.url) ? "URL" : "file"}`}
                    >
                      <RefreshCw className="h-3.5 w-3.5" />
                    </button>
                  )}
                  {s.sourceType !== "url" && (
                    <button
                      className="rounded p-1 text-muted-foreground hover:text-foreground"
                      onClick={(e) => {
                        e.stopPropagation();
                        void startEdit(s);
                      }}
                      title="Edit text (re-embed)"
                      aria-label={`Edit "${s.title}"`}
                    >
                      <Pencil className="h-3.5 w-3.5" />
                    </button>
                  )}
                  <button
                    className="rounded p-1 text-muted-foreground hover:text-destructive"
                    onClick={async (e) => {
                      e.stopPropagation();
                      if (
                        await confirm({
                          title: `Remove "${s.title}"?`,
                          message: "This deletes the source and its embedded chunks from the notebook.",
                          confirmLabel: "Remove",
                          danger: true,
                        })
                      )
                        deleteSource(s.id);
                    }}
                    title="Remove source"
                    aria-label={`Remove "${s.title}"`}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      <Modal open={mode === "url"} onClose={() => setMode(null)} title="Add source from URL">
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            setMode(null);
            await addUrl(url);
          }}
          className="flex flex-col gap-3"
        >
          <Input
            autoFocus
            placeholder="https://example.com/article"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
          <p className="text-[11px] leading-relaxed text-subtle-foreground">
            Google Docs, Sheets, and Slides links work too — share them as
            “Anyone with the link” first.
          </p>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setMode(null)}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={!url.trim()}>
              Fetch & add
            </Button>
          </div>
        </form>
      </Modal>

      <Modal
        open={!!editing}
        onClose={() => setEditing(null)}
        title="Edit source"
        width="max-w-lg"
      >
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            if (!editing) return;
            const { id, title, text } = editing;
            setEditing(null);
            await editSourceText(id, title, text);
          }}
          className="flex flex-col gap-3"
        >
          <Input
            autoFocus
            placeholder="Title"
            value={editing?.title ?? ""}
            onChange={(e) => setEditing((s) => (s ? { ...s, title: e.target.value } : s))}
          />
          <Textarea
            rows={12}
            placeholder="Source text…"
            value={editing?.text ?? ""}
            onChange={(e) => setEditing((s) => (s ? { ...s, text: e.target.value } : s))}
          />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setEditing(null)}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={!editing?.text.trim()}>
              Save
            </Button>
          </div>
        </form>
      </Modal>

      <Modal open={mode === "text"} onClose={() => setMode(null)} title="Paste text" width="max-w-lg">
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            setMode(null);
            await addText(pasteTitle, pasteText);
          }}
          className="flex flex-col gap-3"
        >
          <Input
            autoFocus
            placeholder="Title (optional)"
            value={pasteTitle}
            onChange={(e) => setPasteTitle(e.target.value)}
          />
          <Textarea
            rows={10}
            placeholder="Paste or type your text here…"
            value={pasteText}
            onChange={(e) => setPasteText(e.target.value)}
          />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setMode(null)}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={!pasteText.trim()}>
              Add source
            </Button>
          </div>
        </form>
      </Modal>

      {confirmDialog}
    </div>
  );
}

function MenuItem({
  icon,
  label,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      role="menuitem"
      className={cn(
        "flex w-full items-center gap-2.5 px-3 py-1.5 text-left text-[13px] text-foreground/90",
        "hover:bg-surface-2 hover:text-foreground focus-visible:bg-surface-2",
      )}
      onClick={onClick}
    >
      <span className="text-muted-foreground">{icon}</span>
      {label}
    </button>
  );
}
