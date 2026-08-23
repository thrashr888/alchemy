import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import {
  Badge,
  Button,
  Input,
  Textarea,
  Modal,
  EmptyState,
  ResizeHandle,
  RowMenu,
  type RowMenuItem,
  Spinner,
  CardAction,
  useConfirm,
  useHoverCard,
  useMarquee,
} from "./ui";
import {
  cn,
  compactNumber,
  folderProvider,
  isWebUrl,
  relativeTime,
  shortcutBlocked,
  visibleTitle,
} from "@/lib/utils";
import { sourceIcon } from "@/lib/sourceIcon";
import { AttachToCardModal } from "./RegistrySection";
import type { Source } from "@/lib/types";
import {
  ChevronRight,
  FileText,
  Globe,
  LayoutGrid,
  Plus,
  PanelLeftClose,
  Trash2,
  Upload,
  Check,
  AlertCircle,
  X,
  Pencil,
  RefreshCw,
  Cloud,
  MessageSquare,
  Package,
  StickyNote,
  Tag,
} from "lucide-react";

// Reference scale for the "how big is this notebook" gauge. Not a capacity —
// retrieval has no cliff (RFC-infinite-context: adaptive k, gists, the scale
// fence holds recall flat as the corpus grows) — 10M chars is the design
// target the eval fence covers, so the bar reads as "where you are in the
// verified operating range", going red only near its edge.
const SCALE_TARGET_CHARS = 10_000_000;

// Folder tree open/closed state persists across restarts, keyed by folder
// source id (only ids the user has explicitly toggled are stored; unseen
// folders keep the collapsed-when-many default).
const FOLDERS_COLLAPSED_KEY = "foldersCollapsed";

function loadFoldersCollapsed(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(FOLDERS_COLLAPSED_KEY);
    return raw ? JSON.parse(raw) : {};
  } catch {
    return {};
  }
}

function saveFoldersCollapsed(state: Record<string, boolean>) {
  try {
    localStorage.setItem(FOLDERS_COLLAPSED_KEY, JSON.stringify(state));
  } catch {
    /* storage full or unavailable — collapse state is best-effort */
  }
}

// "Keep" decisions from the hygiene review (RFC-source-hygiene), keyed
// `${sourceId}:${bucket}` per notebook. Local suppression on purpose:
// unreachable keeps reset real backend state, but a kept duplicate or
// missing file is a viewing preference — the signal itself stays true and
// agents still see it in the MCP report.
function loadHygieneKept(notebookId: string | null): Record<string, boolean> {
  if (!notebookId) return {};
  try {
    const raw = localStorage.getItem(`hygieneKept:${notebookId}`);
    return raw ? JSON.parse(raw) : {};
  } catch {
    return {};
  }
}

function saveHygieneKept(notebookId: string | null, kept: Record<string, boolean>) {
  if (!notebookId) return;
  try {
    localStorage.setItem(`hygieneKept:${notebookId}`, JSON.stringify(kept));
  } catch {
    /* best-effort */
  }
}

const HYGIENE_LABEL: Record<string, string> = {
  unreachable: "unreachable",
  "missing-file": "missing",
  duplicate: "duplicate",
  husk: "failed import",
  stale: "stale",
};

/** Source-domain favicon with a Globe fallback (kept local — no third party). */
/** The hover card: type, size, freshness, status — rows and gallery cards
 *  stay quiet, the beat-delayed card carries the metadata. */
export function sourceHoverData(s: Source) {
  const meta: { label: string; value?: string }[] = [
    {
      label: s.parentId ? `${s.sourceType} · folder item` : s.sourceType,
    },
    {
      label: "Size",
      value: `${compactNumber(s.charCount)} chars · ${s.chunkCount} chunks`,
    },
    { label: "Added", value: relativeTime(s.createdAt) },
  ];
  // File mtimes are real clocks; mac/git stamps are content hashes — only
  // show a time that is one.
  if (s.mtime > 946_684_800_000 && s.mtime < Date.now() + 86_400_000) {
    meta.push({ label: "File updated", value: relativeTime(s.mtime) });
  }
  if (s.status === "error") meta.push({ label: s.error || "Import failed" });
  if (s.status === "processing")
    meta.push({ label: "Indexing — chat and search pick it up shortly" });
  if (s.status === "placeholder")
    meta.push({ label: "Cloud placeholder — not downloaded yet" });
  if (s.author) meta.push({ label: "Author", value: s.author });
  if (s.tags)
    meta.push({
      label: "Tags",
      value: s.tags
        .split(" ")
        .map((t) => `#${t}`)
        .join(" "),
    });
  if (s.url) meta.push({ label: s.url });
  return { title: s.title, meta };
}

export function Favicon({ url }: { url: string }) {
  const [failed, setFailed] = useState(false);
  let origin = "";
  try {
    origin = new URL(url).origin;
  } catch {
    /* malformed */
  }
  if (failed || !origin)
    return <Globe className="h-3.5 w-3.5 text-muted-foreground" />;
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
  const importingFolders = useStore((s) => s.importingFolders);
  const clearQueueItem = useStore((s) => s.clearQueueItem);
  const openAddSource = useStore((s) => s.openAddSource);
  const folderScan = useStore((s) => s.folderScan);
  const editSourceText = useStore((s) => s.editSourceText);
  const updateMacNote = useStore((s) => s.updateMacNote);
  const setSourceTags = useStore((s) => s.setSourceTags);
  const setSourceNote = useStore((s) => s.setSourceNote);
  const addMacReminder = useStore((s) => s.addMacReminder);
  const refreshSource = useStore((s) => s.refreshSource);
  const deleteSource = useStore((s) => s.deleteSource);
  const draggingFiles = useStore((s) => s.draggingFiles);
  const toggleSources = useStore((s) => s.toggleSources);
  const openSourceViewer = useStore((s) => s.openSourceViewer);
  const selectedSourceIds = useStore((s) => s.selectedSourceIds);
  const toggleSourceSelected = useStore((s) => s.toggleSourceSelected);
  const setAllSourcesSelected = useStore((s) => s.setAllSourcesSelected);
  const askAboutSource = useStore((s) => s.askAboutSource);
  const picked = useStore((s) => s.picked);
  const pickOne = useStore((s) => s.pickOne);
  const pickToggle = useStore((s) => s.pickToggle);
  const pickRange = useStore((s) => s.pickRange);
  const pickSet = useStore((s) => s.pickSet);
  const clearPicked = useStore((s) => s.clearPicked);
  const refreshSourcesBatch = useStore((s) => s.refreshSourcesBatch);
  const deleteSourcesBatch = useStore((s) => s.deleteSourcesBatch);
  const hygiene = useStore((s) => s.hygiene);
  const refreshHygiene = useStore((s) => s.refreshHygiene);
  const hygieneKeep = useStore((s) => s.hygieneKeep);
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
  // Inline metadata editors (RFC-source-tags): tags as one input line,
  // the annotation as a small textarea — same modal idiom as Edit source.
  // `ids` carries one id for the row menu, several for the multi-select
  // batch verb (RFC-multi-select).
  const [tagEdit, setTagEdit] = useState<{
    ids: string[];
    title: string;
    value: string;
  } | null>(null);
  const [noteEdit, setNoteEdit] = useState<{
    id: string;
    title: string;
    value: string;
  } | null>(null);
  /** Source being filed under a registry card (RFC-registry §2). */
  const [attaching, setAttaching] = useState<{
    id: string;
    title: string;
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

  const { show: showCard, hide: hideCard, card: hoverCard } = useHoverCard("right");
  const sourceCard = sourceHoverData;

  const totalChars = sources.reduce((sum, s) => sum + s.charCount, 0);
  const pct = Math.min(100, (totalChars / SCALE_TARGET_CHARS) * 100);

  // Folder children render indented under their folder; everything else is a
  // flat top-level row. Parents with many children start collapsed — a repo
  // shouldn't wall the panel — and the chevron remembers the user's choice
  // across restarts (persisted to localStorage, keyed by folder source id,
  // mirroring the other UI-state keys in store.ts).
  const [collapsedParents, setCollapsedParents] =
    useState<Record<string, boolean>>(loadFoldersCollapsed);
  const isCollapsed = (id: string, kidCount: number) =>
    collapsedParents[id] ?? kidCount > 8;
  const toggleCollapsed = (id: string, kidCount: number) =>
    setCollapsedParents((m) => {
      const cur = m[id] ?? kidCount > 8;
      const next = { ...m, [id]: !cur };
      saveFoldersCollapsed(next);
      return next;
    });
  // Children indexed once per source list — the tree build and every row's
  // count/size lookups used to re-filter the whole array per parent, which
  // is quadratic on big folder notebooks.
  const childrenOf = useMemo(() => {
    const m = new Map<string, Source[]>();
    for (const s of sources) {
      if (!s.parentId) continue;
      const list = m.get(s.parentId);
      if (list) list.push(s);
      else m.set(s.parentId, [s]);
    }
    return m;
  }, [sources]);
  const rows: { s: Source; indent: boolean }[] = [];
  for (const s of sources) {
    if (s.parentId) continue;
    rows.push({ s, indent: false });
    if (["folder", "git", "notion", "obsidian"].includes(s.sourceType)) {
      const kids = childrenOf.get(s.id) ?? [];
      if (!isCollapsed(s.id, kids.length)) {
        for (const c of kids) {
          rows.push({ s: c, indent: true });
        }
      }
    }
  }
  const childCount = (folderId: string) =>
    (childrenOf.get(folderId) ?? []).length;
  // A folder/repo parent carries no chars of its own (char_count 0 in the DB);
  // its children are the real carriers, so its "contribution" is their sum.
  const folderChars = (folderId: string) =>
    (childrenOf.get(folderId) ?? []).reduce((sum, x) => sum + x.charCount, 0);

  // Selection: null means everything is on; the map holds only deselected ids.
  const isSelected = (id: string) =>
    !selectedSourceIds || selectedSourceIds[id] !== false;
  // Folder container rows have no chunks — only content sources count.
  const contentSources = sources.filter(
    (s) => s.sourceType !== "folder" && s.sourceType !== "obsidian",
  );
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

  // ---- Finder-style selection (RFC-multi-select) ------------------------
  const pickedIds = useMemo(
    () => new Set(picked?.kind === "sources" ? picked.ids : []),
    [picked],
  );
  const rowIds = rows.map((r) => r.s.id);
  const rowIdsRef = useRef(rowIds);
  rowIdsRef.current = rowIds;
  const setSourcesTagsBatch = useStore((s) => s.setSourcesTagsBatch);

  const listRef = useRef<HTMLDivElement>(null);
  // An additive drag unions against the selection as it stood when the drag
  // began — unioning against the live selection would ratchet (rows swept
  // over once could never leave).
  const marqueeBase = useRef<string[]>([]);
  const { onPointerDown: marqueeDown, marquee, justEnded } = useMarquee({
    containerRef: listRef,
    onStart: (additive) => {
      const p = useStore.getState().picked;
      marqueeBase.current = additive && p?.kind === "sources" ? p.ids : [];
    },
    onSelect: (ids) =>
      pickSet(
        "sources",
        [...new Set([...marqueeBase.current, ...ids])],
        false,
      ),
    onClearBackground: clearPicked,
  });

  /** The right-click menu for a row inside a multi-selection: batch
   *  variants of the single-row verbs, with counts in the labels. */
  function batchMenuItems(ids: string[]): RowMenuItem[] {
    const n = ids.length;
    const refreshable = ids.filter(
      (id) => !!sources.find((x) => x.id === id)?.url,
    );
    return [
      ...(refreshable.length
        ? [
            {
              label: `Refresh ${refreshable.length} sources`,
              icon: <RefreshCw className="h-3.5 w-3.5" />,
              onClick: () => void refreshSourcesBatch(refreshable),
            },
          ]
        : []),
      {
        label: `Tag ${n} sources…`,
        icon: <Tag className="h-3.5 w-3.5" />,
        onClick: () =>
          setTagEdit({ ids, title: `${n} sources`, value: "" }),
      },
      {
        label: `Remove ${n} sources…`,
        icon: <Trash2 className="h-3.5 w-3.5" />,
        danger: true,
        onClick: () => void confirmRemoveBatch(ids),
      },
    ];
  }

  async function confirmRemoveBatch(ids: string[]) {
    if (
      await confirm({
        title: `Remove ${ids.length} sources?`,
        message:
          "This deletes the selected sources and their embedded chunks from the notebook (folders take their files along). Nothing on disk is touched.",
        confirmLabel: "Remove",
        danger: true,
      })
    )
      void deleteSourcesBatch(ids);
  }
  const confirmRemoveBatchRef = useRef(confirmRemoveBatch);
  confirmRemoveBatchRef.current = confirmRemoveBatch;

  // ⌘A selects every visible row, Escape clears, Delete removes the
  // selection (after the app confirm). Guarded by shortcutBlocked; ⌘A also
  // steps aside while the reader is open (select-all there means text) or
  // while a notes selection is active (Studio owns it then).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (shortcutBlocked(e)) return;
      // An open row menu owns the keyboard while it's up — Escape closes it
      // without also dropping the selection. Its own handler can't stop us:
      // the menu renders in a body portal, so the native event never passes
      // through React's root container where stopPropagation would land.
      // Hence the capture phase below: by the bubble phase React has already
      // flushed the close synchronously and the menu is gone from the DOM.
      if (document.querySelector('[role="menu"]')) return;
      const st = useStore.getState();
      const p = st.picked;
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a") {
        if (p?.kind === "notes" || st.reader.open || !st.currentId) return;
        e.preventDefault();
        st.pickAll("sources", rowIdsRef.current);
      } else if (e.key === "Escape") {
        if (p) st.clearPicked();
      } else if (
        (e.key === "Backspace" || e.key === "Delete") &&
        p?.kind === "sources" &&
        p.ids.length > 0
      ) {
        e.preventDefault();
        void confirmRemoveBatchRef.current(p.ids);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  // ---- Source hygiene (RFC-source-hygiene) ------------------------------
  // Re-classify shortly after the list settles; the check is a cheap
  // metadata read.
  useEffect(() => {
    if (!currentId) return;
    const t = setTimeout(() => void refreshHygiene(), 800);
    return () => clearTimeout(t);
  }, [currentId, sources, refreshHygiene]);

  const [reviewOpen, setReviewOpen] = useState(false);
  /** Source id currently being re-fetched from the review modal. */
  const [retrying, setRetrying] = useState<string | null>(null);
  const [keptVersion, setKeptVersion] = useState(0);
  const issueBySource = useMemo(() => {
    const kept = loadHygieneKept(currentId);
    const m = new Map<string, (typeof hygiene)[number]>();
    for (const h of hygiene) {
      if (h.bucket === "stale") continue; // the sweep's job, not the user's
      if (kept[`${h.sourceId}:${h.bucket}`]) continue;
      if (!m.has(h.sourceId)) m.set(h.sourceId, h);
    }
    return m;
    // keptVersion invalidates after a "Keep" writes localStorage.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hygiene, currentId, keptVersion]);
  const proposals = [...issueBySource.values()];

  /** Fetch a flagged source again, right now. This is the user-initiated
   *  path on purpose — someone is watching, so it keeps the hard-fail
   *  semantics the background sweep deliberately avoids, and a success
   *  clears the strike count (reingest stamps it), dropping the flag.
   *  Duplicates get no Retry: re-fetching says nothing about them. */
  async function retryIssue(h: { sourceId: string; bucket: string }) {
    setRetrying(h.sourceId);
    try {
      await refreshSource(h.sourceId);
    } finally {
      setRetrying(null);
    }
    await refreshHygiene();
  }

  function keepIssue(h: { sourceId: string; bucket: string }) {
    if (h.bucket === "unreachable") {
      // Real backend state: clear the strike count, restart the cadence.
      void hygieneKeep(h.sourceId);
      return;
    }
    const kept = loadHygieneKept(currentId);
    kept[`${h.sourceId}:${h.bucket}`] = true;
    saveHygieneKept(currentId, kept);
    setKeptVersion((v) => v + 1);
  }

  return (
    <div
      style={{ width }}
      className="side-card relative mx-2 mb-2 mt-1 flex shrink-0 flex-col"
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
          <span className="text-body font-semibold text-foreground">
            Drop to add sources
          </span>
          <span className="text-micro text-muted-foreground">
            PDF · Office · images · text
          </span>
        </div>
      )}
      <div className="flex items-center px-4 h-12 border-b border-border">
        <span className="text-caption font-semibold uppercase tracking-wide text-muted-foreground">
          Sources
        </span>
        <span className="ml-2 text-micro text-subtle-foreground">
          {sources.length}
        </span>
        <div className="ml-auto flex items-center gap-0.5">
          <Button
            variant="ghost"
            size="icon"
            onClick={() =>
              useStore.setState((st) => ({
                galleryOpen: !st.galleryOpen,
                ledgerOpen: false,
              }))
            }
            disabled={!currentId}
            title="Browse source gallery"
            aria-label="Browse source gallery"
          >
            <LayoutGrid className="h-4 w-4" />
          </Button>
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
          <div className="mb-1.5 flex items-center justify-between text-micro">
            <span className="text-muted-foreground">
              {Intl.NumberFormat().format(totalChars)} chars
            </span>
            <span className="text-subtle-foreground">
              {pct < 1 ? "<1" : Math.round(pct)}% of 10M
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

      <div
        ref={listRef}
        onPointerDown={marqueeDown}
        // select-none: these rows are chrome, not prose. Without it a
        // rubber-band drag paints a native text highlight across every
        // title it crosses (the "Copy text" menu items are how text
        // leaves this app, not selection).
        className="flex-1 select-none overflow-y-auto p-2"
      >
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
                  <div className="truncate text-caption" title={q.name}>
                    {q.name}
                  </div>
                  <div
                    className={cn(
                      "text-micro",
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
                  <>
                    {q.retry && (
                      <button
                        className="rounded p-0.5 text-muted-foreground hover:text-foreground"
                        onClick={q.retry}
                        title="Retry"
                        aria-label={`Retry failed import "${q.name}"`}
                      >
                        <RefreshCw className="h-3.5 w-3.5" />
                      </button>
                    )}
                    <button
                      className="rounded p-0.5 text-muted-foreground hover:text-foreground"
                      onClick={() => clearQueueItem(q.id)}
                      title="Dismiss"
                      aria-label={`Dismiss failed import "${q.name}"`}
                    >
                      <X className="h-3.5 w-3.5" />
                    </button>
                  </>
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
            hint="Drop files or folders here, add a URL, or paste text."
          >
            <p className="max-w-[260px] text-micro text-subtle-foreground">
              PDF, Word, PowerPoint, Excel, images, EPUB, markdown, and more.
              Folders stay in sync, including cloud drives.
            </p>
          </EmptyState>
        ) : (
          <>
            {/* Master selection row: which sources feed chat & Studio. Always
                labeled — a bare checkbox over empty space read as a blank,
                menu-less source row in every notebook. */}
            <div className="mb-0.5 flex items-center gap-2 px-2 py-1.5">
              <span className="text-micro font-medium uppercase tracking-wide text-subtle-foreground">
                {allSelected
                  ? "All selected"
                  : `${selectedCount} of ${contentSources.length} selected`}
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
            {/* Hygiene proposals (RFC-source-hygiene): flagged, never
                auto-removed — the review modal decides. */}
            {proposals.length > 0 && (
              <button
                type="button"
                onClick={() => setReviewOpen(true)}
                className="mb-1 flex w-full items-center gap-2 rounded-md border border-border bg-surface-2/60 px-2 py-1.5 text-left hover:bg-surface-2"
              >
                <AlertCircle className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                <span className="truncate text-caption text-foreground">
                  {proposals.length === 1
                    ? "1 source needs attention"
                    : `${proposals.length} sources need attention`}
                </span>
                <span className="ml-auto shrink-0 text-micro text-subtle-foreground">
                  Review
                </span>
              </button>
            )}
            <div className="flex flex-col gap-0.5">
              {rows.map(({ s, indent }) => {
                const isFolder = [
                  "folder",
                  "git",
                  "notion",
                  "obsidian",
                ].includes(s.sourceType);
                const isMacNote = s.url.startsWith("cider://notes/note/");
                const isMacReminders = s.url.startsWith(
                  "cider://reminders/list/",
                );
                // A folder inserted optimistically while its children embed:
                // shown right away with a loading affordance, not yet openable.
                const importing = isFolder && importingFolders.includes(s.id);
                // Errored WEB sources still open in the reader: extraction
                // failed, but the Live view can show the actual page. Folder
                // and git parents open as the repo reader. A "processing"
                // source reads fine — its text is stored; only retrieval is
                // still catching up.
                const readable =
                  !importing &&
                  (s.status === "ready" ||
                    s.status === "processing" ||
                    (s.status === "error" && isWebUrl(s.url)));
                const kids = isFolder ? (childrenOf.get(s.id) ?? []) : [];
                const kidsOn = kids.filter((k) => isSelected(k.id)).length;
                const isPicked = pickedIds.has(s.id);
                return (
                  <div
                    key={s.id}
                    data-pick-id={s.id}
                    // Row content is pointer-events-none (clicks go to the
                    // CardAction), so the row carries the hover detail the
                    // truncated children can no longer show — as the floating
                    // info card rather than a native tooltip.
                    onMouseEnter={(e) => showCard(e, sourceCard(s))}
                    onMouseLeave={hideCard}
                    className={cn(
                      // content of the rows after it (they'd paint over the
                      // dropdown otherwise — later DOM order wins at equal z).
                      "group relative flex items-start gap-2 rounded-md px-2 py-2 hover:bg-surface-2 [content-visibility:auto] [contain-intrinsic-size:auto_44px]",
                      s.status === "error" && "bg-destructive/5",
                      // Selection is a quiet tinted wash (DESIGN §2 — color
                      // only when it means something; never a left border).
                      isPicked && "bg-primary/10 hover:bg-primary/15",
                      readable && "cursor-pointer",
                      indent && "ml-5",
                    )}
                  >
                    {!importing && (
                      <CardAction
                        label={
                          readable
                            ? `Read source ${s.title}`
                            : `Select source ${s.title}`
                        }
                        onClick={(e) => {
                          // A rubber-band drag that started on this row ends
                          // in a click — that click is the drag's tail.
                          if (justEnded()) return;
                          if (e.metaKey || e.ctrlKey) {
                            pickToggle("sources", s.id);
                            return;
                          }
                          if (e.shiftKey) {
                            pickRange("sources", rowIds, s.id);
                            return;
                          }
                          // Plain click: collapse the selection to this row
                          // (the shift anchor) and open as before.
                          pickOne("sources", s.id);
                          if (readable) openSourceViewer(s.id, s.title);
                        }}
                      />
                    )}
                    {isFolder && kids.length > 0 ? (
                      // Notion/Arc pattern: the type icon at rest, a rotating
                      // disclosure caret replacing it on hover or keyboard
                      // focus. The button toggles collapse; the rest of the
                      // row opens the repo reader. State stays legible at
                      // rest — expanded parents show indented children,
                      // collapsed ones their count badge.
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          toggleCollapsed(s.id, kids.length);
                        }}
                        aria-expanded={!isCollapsed(s.id, kids.length)}
                        aria-label={
                          isCollapsed(s.id, kids.length)
                            ? `Show ${kids.length} files in ${s.title}`
                            : `Hide files in ${s.title}`
                        }
                        className="pointer-events-auto relative z-20 mt-0.5 shrink-0 cursor-pointer"
                      >
                        <span className="group-hover:hidden group-focus-within:hidden">
                          {sourceIcon(s.sourceType, s.url)}
                        </span>
                        <ChevronRight
                          className={cn(
                            "hidden h-3.5 w-3.5 text-muted-foreground transition-transform duration-150 group-hover:block group-focus-within:block",
                            !isCollapsed(s.id, kids.length) && "rotate-90",
                          )}
                        />
                      </button>
                    ) : (
                      <div className="pointer-events-none relative z-10 mt-0.5">
                        {importing || s.status === "processing" ? (
                          <Spinner className="h-3.5 w-3.5 text-muted-foreground" />
                        ) : s.status === "error" ? (
                          <AlertCircle className="h-3.5 w-3.5 text-destructive" />
                        ) : s.status === "placeholder" ? (
                          <Cloud className="h-3.5 w-3.5 text-subtle-foreground" />
                        ) : s.sourceType === "url" && s.url ? (
                          <Favicon url={s.url} />
                        ) : (
                          sourceIcon(s.sourceType, s.url)
                        )}
                      </div>
                    )}
                    <div className="pointer-events-none relative z-10 min-w-0 flex-1">
                      {/* The ⋯ menu lives in the title row: hovering shortens the
                      title but never reflows the metadata line below. */}
                      <div className="flex items-center gap-1">
                        <span
                          className={cn(
                            "min-w-0 flex-1 truncate text-body",
                            s.status === "placeholder"
                              ? "text-muted-foreground"
                              : "text-foreground",
                          )}
                          title={visibleTitle(s.title) || s.url || "Untitled"}
                        >
                          {/* A source can arrive with a blank or zero-width
                              title (a page with no real <title>); the row must
                              never render as a bare checkbox. */}
                          {visibleTitle(s.title) ||
                            (s.url && hostname(s.url)) ||
                            "Untitled"}
                        </span>
                        {issueBySource.has(s.id) && (
                          <Badge
                            className="shrink-0"
                            title={issueBySource.get(s.id)?.detail}
                          >
                            {HYGIENE_LABEL[issueBySource.get(s.id)!.bucket] ??
                              issueBySource.get(s.id)!.bucket}
                          </Badge>
                        )}
                        {!importing && (
                          <RowMenu
                            className="pointer-events-auto z-20"
                            onOpen={hideCard}
                            label={`Options for "${s.title}"`}
                            contextItems={() => {
                              // Right-click inside a multi-selection shows
                              // the batch verbs; outside it collapses the
                              // selection to this row (Finder behavior) and
                              // opens the normal menu.
                              if (pickedIds.has(s.id) && pickedIds.size > 1)
                                return batchMenuItems([...pickedIds]);
                              pickOne("sources", s.id);
                              return null;
                            }}
                            items={[
                              // Chat scoped to this one source (a folder
                              // scopes to its files); placeholders have no
                              // chunks yet, so there's nothing to ask.
                              ...(s.status === "ready" &&
                              (!isFolder ||
                                kids.some((k) => k.status === "ready"))
                                ? [
                                    {
                                      label: "Ask about this source",
                                      icon: (
                                        <MessageSquare className="h-3.5 w-3.5" />
                                      ),
                                      onClick: () => askAboutSource(s.id),
                                    },
                                  ]
                                : []),
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
                                      icon: (
                                        <RefreshCw className="h-3.5 w-3.5" />
                                      ),
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
                                label: s.tags ? "Edit tags…" : "Add tags…",
                                icon: <Tag className="h-3.5 w-3.5" />,
                                onClick: () =>
                                  setTagEdit({
                                    ids: [s.id],
                                    title: s.title,
                                    value: s.tags,
                                  }),
                              },
                              {
                                label: s.note ? "Edit note…" : "Add note…",
                                icon: <StickyNote className="h-3.5 w-3.5" />,
                                onClick: () =>
                                  setNoteEdit({
                                    id: s.id,
                                    title: s.title,
                                    value: s.note,
                                  }),
                              },
                              {
                                label: "File under a card…",
                                icon: <Package className="h-3.5 w-3.5" />,
                                onClick: () =>
                                  setAttaching({ id: s.id, title: s.title }),
                              },
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
                        )}
                      </div>
                      {importing ? (
                        <div className="truncate text-micro text-subtle-foreground">
                          {folderScan
                            ? `Embedding ${Math.min(
                                folderScan.done + 1,
                                folderScan.total,
                              )}/${folderScan.total}…`
                            : "Adding folder…"}
                        </div>
                      ) : s.status === "error" ? (
                        <div
                          // break-anywhere: raw URLs in errors have no
                          // spaces and would otherwise force the panel wide.
                          className="line-clamp-3 text-micro leading-snug text-destructive [overflow-wrap:anywhere]"
                          title={s.error}
                        >
                          {s.error || "Import failed"}
                        </div>
                      ) : s.status === "placeholder" ? (
                        <div
                          className="text-micro text-subtle-foreground"
                          title={s.url}
                        >
                          Online-only — not downloaded
                        </div>
                      ) : isFolder ? (
                        // The folder's contribution to the notebook. Its
                        // auto-refresh behavior moves to the tooltip — a folder
                        // staying in sync isn't something the reader must watch.
                        // A cloud-provider chip (derived from the path) shows
                        // where a synced folder lives.
                        <div
                          className="flex items-center gap-1.5 text-micro text-subtle-foreground"
                          title={`${s.url}\nStays in sync — auto-refreshes`}
                        >
                          {folderProvider(s.url) && (
                            <span className="shrink-0 rounded bg-surface-2 px-1.5 py-px text-caption text-muted-foreground">
                              {folderProvider(s.url)}
                            </span>
                          )}
                          <span className="truncate">
                            {childCount(s.id)} files ·{" "}
                            {compactNumber(folderChars(s.id))} chars
                          </span>
                        </div>
                      ) : s.sourceType === "url" && s.url ? (
                        <div
                          className="truncate text-micro text-citation"
                          title={s.url}
                        >
                          {hostname(s.url)}
                        </div>
                      ) : null}
                    </div>
                    {/* Selection stays at the far right (NotebookLM-style), always
                    visible. */}
                    <div className="relative z-20 mt-0.5">
                      {importing ? null : isFolder ? (
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
            <p className="text-micro leading-relaxed text-subtle-foreground">
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

      <AttachToCardModal
        sourceId={attaching?.id ?? null}
        sourceTitle={attaching?.title ?? ""}
        onClose={() => setAttaching(null)}
      />

      <Modal
        open={!!tagEdit}
        onClose={() => setTagEdit(null)}
        title={`Tags for "${tagEdit?.title ?? ""}"`}
        width="max-w-md"
      >
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            if (!tagEdit) return;
            const { ids, value } = tagEdit;
            setTagEdit(null);
            if (ids.length === 1) await setSourceTags(ids[0], value);
            else await setSourcesTagsBatch(ids, value);
          }}
          className="flex flex-col gap-3"
        >
          <Input
            autoFocus
            name="source-tags"
            aria-label="Source tags"
            placeholder="research rust retrieval"
            value={tagEdit?.value ?? ""}
            onChange={(e) =>
              setTagEdit((s) => (s ? { ...s, value: e.target.value } : s))
            }
          />
          <p className="text-micro leading-relaxed text-subtle-foreground">
            Space-separated; "#" and case don't matter. Tags show up in
            chat's source list and help match questions to notebooks.
          </p>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setTagEdit(null)}>
              Cancel
            </Button>
            <Button type="submit" variant="primary">
              Save
            </Button>
          </div>
        </form>
      </Modal>

      <Modal
        open={!!noteEdit}
        onClose={() => setNoteEdit(null)}
        title={`Note on "${noteEdit?.title ?? ""}"`}
        width="max-w-md"
      >
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            if (!noteEdit) return;
            const { id, value } = noteEdit;
            setNoteEdit(null);
            await setSourceNote(id, value);
          }}
          className="flex flex-col gap-3"
        >
          <Textarea
            autoFocus
            rows={5}
            name="source-note"
            aria-label="Source note"
            placeholder="Why did you save this? Chat can recall it."
            value={noteEdit?.value ?? ""}
            onChange={(e) =>
              setNoteEdit((s) => (s ? { ...s, value: e.target.value } : s))
            }
          />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setNoteEdit(null)}>
              Cancel
            </Button>
            <Button type="submit" variant="primary">
              Save
            </Button>
          </div>
        </form>
      </Modal>

      {/* Hygiene review (RFC-source-hygiene): every removal is a human
          decision — per-item Keep / Remove, nothing automatic. */}
      <Modal
        open={reviewOpen}
        onClose={() => setReviewOpen(false)}
        title="Needs attention"
        width="max-w-md"
      >
        <div className="flex flex-col gap-2">
          <p className="text-micro leading-relaxed text-subtle-foreground">
            These sources look broken or outdated. Nothing is removed unless
            you say so — Keep dismisses the flag.
          </p>
          {proposals.length === 0 ? (
            <EmptyState title="All clean" />
          ) : (
            proposals.map((h) => (
              <div
                key={`${h.sourceId}:${h.bucket}`}
                className="flex items-center gap-2 rounded-md border border-border px-2.5 py-2"
              >
                <div className="min-w-0 flex-1">
                  <div
                    className="truncate text-body text-foreground"
                    title={h.title}
                  >
                    {visibleTitle(h.title) || "Untitled"}
                  </div>
                  <div
                    className="truncate text-micro text-muted-foreground"
                    title={h.detail}
                  >
                    {HYGIENE_LABEL[h.bucket] ?? h.bucket} · {h.detail}
                  </div>
                </div>
                {h.bucket !== "duplicate" && (
                  <Button
                    variant="ghost"
                    disabled={retrying === h.sourceId}
                    onClick={() => void retryIssue(h)}
                    title="Fetch it again now"
                  >
                    {retrying === h.sourceId ? "Retrying…" : "Retry"}
                  </Button>
                )}
                <Button
                  variant="ghost"
                  onClick={() => keepIssue(h)}
                  title="Dismiss this flag and keep the source"
                >
                  Keep
                </Button>
                <Button
                  variant="ghost"
                  className="text-destructive hover:bg-destructive/10"
                  onClick={() => void deleteSourcesBatch([h.sourceId])}
                  title="Remove the source and its chunks"
                >
                  Remove
                </Button>
              </div>
            ))
          )}
        </div>
      </Modal>

      {marquee}
      {confirmDialog}
      {hoverCard}
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
