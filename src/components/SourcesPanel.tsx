import { useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Button, Input, Textarea, Modal, EmptyState, Spinner } from "./ui";
import { cn } from "@/lib/utils";
import type { Source } from "@/lib/types";
import {
  FileText,
  FileType,
  Globe,
  Hash,
  Plus,
  Trash2,
  Upload,
  Link2,
  ClipboardPaste,
  Check,
  AlertCircle,
  X,
  Pencil,
  RefreshCw,
} from "lucide-react";

// Soft per-notebook capacity used for the "how full is this notebook" gauge.
// ~1M chars ≈ ~250k tokens — generous for local RAG over many documents.
const MAX_NOTEBOOK_CHARS = 1_000_000;

function sourceIcon(t: Source["sourceType"]) {
  switch (t) {
    case "pdf":
      return <FileType className="h-3.5 w-3.5 text-[#eb5757]" />;
    case "url":
      return <Globe className="h-3.5 w-3.5 text-[#5e9bd2]" />;
    case "markdown":
      return <Hash className="h-3.5 w-3.5 text-[#9b87f5]" />;
    default:
      return <FileText className="h-3.5 w-3.5 text-muted-foreground" />;
  }
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
  const addFiles = useStore((s) => s.addSourceFiles);
  const addUrl = useStore((s) => s.addSourceUrl);
  const addText = useStore((s) => s.addSourceText);
  const editSourceText = useStore((s) => s.editSourceText);
  const refreshSource = useStore((s) => s.refreshSource);
  const deleteSource = useStore((s) => s.deleteSource);

  const [menuOpen, setMenuOpen] = useState(false);
  const [mode, setMode] = useState<AddMode>(null);
  const [url, setUrl] = useState("");
  const [pasteTitle, setPasteTitle] = useState("");
  const [pasteText, setPasteText] = useState("");
  const [editing, setEditing] = useState<{ id: string; title: string; text: string } | null>(null);

  async function startEdit(s: Source) {
    // List payloads omit content; fetch the full text to prefill the editor.
    const content = await api.getSourceContent(s.id);
    setEditing({ id: s.id, title: s.title, text: content });
  }

  const totalChars = sources.reduce((sum, s) => sum + s.charCount, 0);
  const pct = Math.min(100, (totalChars / MAX_NOTEBOOK_CHARS) * 100);

  async function pickFiles() {
    setMenuOpen(false);
    const selected = await open({
      multiple: true,
      filters: [{ name: "Documents", extensions: ["pdf", "txt", "md", "markdown", "text"] }],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    await addFiles(paths);
  }

  return (
    <div className="flex h-full w-[280px] shrink-0 flex-col border-r border-border bg-surface">
      <div className="flex items-center px-4 h-12 border-b border-border">
        <span className="text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">
          Sources
        </span>
        <span className="ml-2 text-[11px] text-subtle-foreground">{sources.length}</span>
        <div className="relative ml-auto">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => setMenuOpen((o) => !o)}
            disabled={!currentId}
            title="Add source"
          >
            <Plus className="h-4 w-4" />
          </Button>
          {menuOpen && (
            <>
              <div className="fixed inset-0 z-10" onClick={() => setMenuOpen(false)} />
              <div className="absolute right-0 top-8 z-20 w-44 overflow-hidden rounded-md border border-border-strong bg-elevated py-1 shadow-xl">
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
                  <div className="truncate text-[12.5px]" title={q.name}>
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
                  >
                    <X className="h-3 w-3" />
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
            hint="Upload PDFs, text, or markdown, add a URL, or paste text. You can also drag files onto the window."
          />
        ) : (
          <div className="flex flex-col gap-0.5">
            {sources.map((s) => (
              <div
                key={s.id}
                className="group flex items-start gap-2 rounded-md px-2 py-2 hover:bg-surface-2"
              >
                <div className="mt-0.5">{sourceIcon(s.sourceType)}</div>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[13px] text-foreground" title={s.title}>
                    {s.title}
                  </div>
                  {s.sourceType === "url" && s.url ? (
                    <div className="truncate text-[11px] text-citation" title={s.url}>
                      {hostname(s.url)}
                    </div>
                  ) : (
                    <div className="text-[11px] text-subtle-foreground">
                      {s.chunkCount} chunks · {Intl.NumberFormat().format(s.charCount)} chars
                    </div>
                  )}
                </div>
                <div className="flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100">
                  {s.sourceType === "url" ? (
                    <button
                      className="rounded p-1 text-muted-foreground hover:text-foreground"
                      onClick={() => refreshSource(s.id)}
                      title="Refresh from URL (re-fetch & re-embed)"
                    >
                      <RefreshCw className="h-3 w-3" />
                    </button>
                  ) : (
                    <button
                      className="rounded p-1 text-muted-foreground hover:text-foreground"
                      onClick={() => startEdit(s)}
                      title="Edit text (re-embed)"
                    >
                      <Pencil className="h-3 w-3" />
                    </button>
                  )}
                  <button
                    className="rounded p-1 text-muted-foreground hover:text-destructive"
                    onClick={() => deleteSource(s.id)}
                    title="Remove source"
                  >
                    <Trash2 className="h-3 w-3" />
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
      className={cn(
        "flex w-full items-center gap-2.5 px-3 py-1.5 text-left text-[13px] text-foreground/90",
        "hover:bg-surface-2 hover:text-foreground",
      )}
      onClick={onClick}
    >
      <span className="text-muted-foreground">{icon}</span>
      {label}
    </button>
  );
}
