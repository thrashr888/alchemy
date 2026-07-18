import { useEffect, useMemo, useRef, useState } from "react";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import type { Note, Source } from "@/lib/types";
import { AudioPlayer, DialogueScript } from "./AudioNote";
import { Flashcards } from "./Flashcards";
import { Markdown } from "./Markdown";
import { MindMap } from "./MindMap";
import { QuizView } from "./QuizView";
import { SlideDeck } from "./SlideDeck";
import { RichEditor } from "./RichEditor";
import { StreamingBody } from "./StudioNoteViewer";
import { sourceIcon } from "./SourcesPanel";
import { Button, Input, RowMenu, Spinner } from "./ui";
import { chatReadingClass, cn, isWebUrl, shortcutBlocked } from "@/lib/utils";
import {
  AppWindow,
  ArrowLeft,
  ArrowRight,
  BookOpen,
  ChevronDown,
  ChevronUp,
  Copy,
  ExternalLink,
  FileInput,
  FolderOpen,
  MessageSquare,
  MessageSquarePlus,
  Pencil,
  RefreshCw,
  Scale,
  Search,
  Sparkles,
} from "lucide-react";

/**
 * The center-column reader — documents open here, in place, instead of in
 * modals. The sources/notes rails act as the navigator: clicking a row swaps
 * the document; history is browser-grade (back/forward, ⌘[ / ⌘]); j/k steps
 * through the rail order. Every note kind renders with its native renderer,
 * and markdown-shaped sources render as markdown instead of a text dump
 * (see docs/RFC-document-surface.md).
 */

const esc = (w: string) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * Locate a chunk's text inside the full source content. Chunks are
 * space-joined word windows while content keeps its newlines, so the match is
 * whitespace-tolerant: find the first ~12 words, then the last ~12 words
 * within the expected span.
 */
export function locatePassage(
  content: string,
  snippet: string,
): [number, number] | null {
  const words = snippet.split(/\s+/).filter(Boolean);
  if (words.length === 0) return null;
  const head = new RegExp(words.slice(0, 12).map(esc).join("\\s+"));
  const hm = head.exec(content);
  if (!hm) return null;
  const start = hm.index;
  const fallbackEnd = Math.min(
    content.length,
    start + Math.round(snippet.length * 1.1),
  );
  if (words.length <= 12)
    return [start, Math.min(fallbackEnd, start + hm[0].length)];
  const window = content.slice(start, fallbackEnd + 200);
  const tail = new RegExp(words.slice(-12).map(esc).join("\\s+"));
  const tm = tail.exec(window);
  const end = tm ? start + tm.index + tm[0].length : fallbackEnd;
  return [start, end];
}

/** All case-insensitive occurrences of `query` in `content`. */
function findMatches(content: string, query: string): [number, number][] {
  if (query.trim().length < 2) return [];
  const out: [number, number][] = [];
  const hay = content.toLowerCase();
  const needle = query.toLowerCase();
  let i = hay.indexOf(needle);
  while (i !== -1 && out.length < 500) {
    out.push([i, i + needle.length]);
    i = hay.indexOf(needle, i + needle.length);
  }
  return out;
}

/** Normalize a URL or path for in-corpus matching. */
function docKey(u: string): string {
  return u.replace(/\/+$/, "");
}

/** Resolve `../`-style segments in a joined file path. */
function normalizePath(path: string): string {
  const out: string[] = [];
  for (const seg of path.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") out.pop();
    else out.push(seg);
  }
  return "/" + out.join("/");
}

/**
 * Route a link clicked inside rendered document content. In-corpus targets
 * (another source's URL or file) open in the reader — early wiki-jumping;
 * everything else goes to the browser or Finder. Returns true when handled.
 */
/** The notebook source a link resolves to, if any (wiki-jump targets). */
function resolveInCorpus(
  rawHref: string,
  origin: string | undefined,
): Source | null {
  if (!rawHref || rawHref.startsWith("#")) return null;
  const sources = useStore.getState().sources;
  const byKey = (key: string) =>
    sources.find((src) => docKey(src.url) === docKey(key)) ?? null;
  if (/^https?:\/\//.test(rawHref)) return byKey(rawHref);
  if (!origin) return null;
  if (/^https?:\/\//.test(origin)) {
    try {
      return byKey(new URL(rawHref, origin).toString());
    } catch {
      return null;
    }
  }
  if (origin.startsWith("/")) {
    const dir = origin.slice(0, origin.lastIndexOf("/"));
    return byKey(normalizePath(`${dir}/${rawHref}`));
  }
  return null;
}

function routeDocLink(rawHref: string, origin: string | undefined): boolean {
  if (!rawHref || rawHref.startsWith("#")) return true; // anchors: no-op for now
  const state = useStore.getState();
  const hit = resolveInCorpus(rawHref, origin);
  if (hit) {
    state.openInReader({ type: "source", id: hit.id });
    return true;
  }
  if (/^https?:\/\//.test(rawHref)) {
    void openUrl(rawHref);
    return true;
  }
  if (!origin) return true;
  if (/^https?:\/\//.test(origin)) {
    try {
      void openUrl(new URL(rawHref, origin).toString());
    } catch {
      // Unresolvable href — swallow rather than navigating the webview.
    }
    return true;
  }
  if (origin.startsWith("/")) {
    const dir = origin.slice(0, origin.lastIndexOf("/"));
    void revealItemInDir(normalizePath(`${dir}/${rawHref}`));
    return true;
  }
  return true;
}

/** Click-capture handler for document bodies: takes over every <a>. */
function docLinkClickHandler(origin: string | undefined) {
  return (e: React.MouseEvent) => {
    const a = (e.target as HTMLElement).closest?.("a");
    if (!a) return;
    e.preventDefault();
    e.stopPropagation();
    routeDocLink(a.getAttribute("href") ?? "", origin);
  };
}

/** Does this text read as markdown? (Agent-pasted text sources usually do.) */
function looksLikeMarkdown(text: string): boolean {
  return /^#{1,6}\s|^\s*[-*]\s+\S|^\s*\d+\.\s+\S|\*\*[^*\n]+\*\*|^\s*>\s+\S|\|.+\|/m.test(
    text,
  );
}

/** How much selected text travels into a chat question before truncation. */
const MAX_PASSAGE_CHARS = 1200;

/** Observed width of an element (for the toolbar's responsive tiers). */
function useElementWidth(ref: React.RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setWidth(el.getBoundingClientRect().width);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return width;
}

/** Chat ⇄ Reader segmented control for the WINDOW toolbar (Apple puts view
 *  switching in the titlebar — Notes, Safari). Renders nothing until a
 *  document has been opened, so fresh notebooks keep the plain toolbar. */
export function CenterModeTabs() {
  const hasDocs = useStore((s) => s.reader.history.length > 0);
  const active = useStore((s) => (s.reader.open ? "reader" : "chat"));
  const tab = (
    id: "chat" | "reader",
    icon: React.ReactNode,
    label: string,
    onClick: () => void,
    disabled = false,
  ) => (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active === id}
      disabled={disabled}
      title={disabled ? "Open a source or note to read it here" : label}
      className={cn(
        "flex items-center gap-1.5 rounded-md px-2 py-1 text-[12px] font-medium transition-colors",
        active === id
          ? "bg-surface-2 text-foreground"
          : "text-muted-foreground hover:text-foreground",
        disabled && "cursor-default opacity-40 hover:text-muted-foreground",
      )}
    >
      {icon}
      {label}
    </button>
  );
  const s = useStore.getState();
  return (
    <div className="flex items-center gap-0.5 rounded-lg border border-border p-0.5">
      {tab("chat", <MessageSquare className="h-3.5 w-3.5" />, "Chat", () =>
        s.closeReader(),
      )}
      {tab(
        "reader",
        <BookOpen className="h-3.5 w-3.5" />,
        "Reader",
        () => useStore.setState((st) => ({ reader: { ...st.reader, open: true } })),
        !hasDocs,
      )}
    </div>
  );
}

export function ReaderPane() {
  const reader = useStore((s) => s.reader);
  const sources = useStore((s) => s.sources);
  const notes = useStore((s) => s.notes);
  const refreshSource = useStore((s) => s.refreshSource);
  const current = reader.history[reader.index] ?? null;
  // Find-bar visibility and refresh live up here so the single toolbar can
  // host their buttons (HIG: one toolbar; the find bar appears on demand).
  const rootRef = useRef<HTMLDivElement>(null);
  const paneWidth = useElementWidth(rootRef);
  // Below this, secondary actions fold into the overflow menu (HIG-style
  // toolbar collapse); above it, everything is one click.
  const compact = paneWidth > 0 && paneWidth < 560;
  const [findOpen, setFindOpen] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [refreshTick, setRefreshTick] = useState(0);
  const [editing, setEditing] = useState(false);
  useEffect(() => {
    setFindOpen(false);
    setEditing(false);
  }, [current?.id]);

  const source =
    current?.type === "source"
      ? (sources.find((s) => s.id === current.id) ?? null)
      : null;
  const note =
    current?.type === "note"
      ? (notes.find((n) => n.id === current.id) ?? null)
      : null;

  // Keyboard: Esc back to chat, ⌘[ / ⌘] history, j/k rail order.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useStore.getState();
      if (e.key === "Escape" && !shortcutBlocked(e)) {
        e.preventDefault();
        s.closeReader();
        return;
      }
      if ((e.metaKey || e.ctrlKey) && (e.key === "[" || e.key === "]")) {
        e.preventDefault();
        s.readerNavigate(e.key === "]" ? 1 : -1);
        return;
      }
      if (!shortcutBlocked(e) && !e.metaKey && !e.ctrlKey && !e.altKey) {
        if (e.key === "j") s.readerStep(1);
        else if (e.key === "k") s.readerStep(-1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const s = useStore.getState();
  const originAction = source?.url
    ? isWebUrl(source.url)
      ? {
          label: "Open original",
          icon: <ExternalLink className="h-3.5 w-3.5" />,
          onClick: () => void openUrl(source.url),
        }
      : source.sourceType !== "mac"
        ? {
            label: "Show in Finder",
            icon: <FolderOpen className="h-3.5 w-3.5" />,
            onClick: () => void revealItemInDir(source.url),
          }
        : null
    : null;
  const refreshAction = source?.url
    ? {
        label: source.sourceType === "mac" ? "Sync now" : "Refresh",
        icon: <RefreshCw className="h-3.5 w-3.5" />,
        onClick: () => {
          if (syncing || !source) return;
          setSyncing(true);
          void refreshSource(source.id)
            .catch(() => undefined)
            .finally(() => {
              setSyncing(false);
              setRefreshTick((t) => t + 1);
            });
        },
      }
    : null;
  const popOutAction = note
    ? {
        label: "Open in its own window",
        icon: <AppWindow className="h-3.5 w-3.5" />,
        onClick: () => void api.newWindow(note.notebookId, note.id),
      }
    : null;
  // Roomy: source actions all inline (no menu at all); notes keep only the
  // rare actions behind the menu. Compact: secondaries fold into the menu.
  const inlineActions = compact
    ? []
    : [originAction, refreshAction, popOutAction].filter(
        (a): a is NonNullable<typeof a> => a !== null,
      );
  const overflowItems = [
    ...(compact
      ? [originAction, refreshAction].filter(
          (a): a is NonNullable<typeof a> => a !== null,
        )
      : []),
    ...(note
      ? [
          ...(note.kind !== "note"
            ? [
                {
                  label: "Rebuild",
                  icon: <RefreshCw className="h-3.5 w-3.5" />,
                  onClick: () => void useStore.getState().rebuildNote(note),
                },
              ]
            : []),
          {
            label: "Copy text",
            icon: <Copy className="h-3.5 w-3.5" />,
            onClick: () => {
              void navigator.clipboard.writeText(note.content).then(
                () => useStore.getState().pushToast("success", "Note copied"),
                () =>
                  useStore
                    .getState()
                    .pushToast("error", "Clipboard access failed."),
              );
            },
          },
          {
            label: "Discuss in chat",
            icon: <MessageSquare className="h-3.5 w-3.5" />,
            onClick: () => {
              void useStore.getState().discussNoteInChat(note.id);
              useStore.getState().closeReader();
            },
          },
          {
            label: "Convert to source",
            icon: <FileInput className="h-3.5 w-3.5" />,
            onClick: () => void useStore.getState().convertNoteToSource(note.id),
          },
          ...(compact && popOutAction ? [popOutAction] : []),
        ]
      : []),
  ];
  return (
    <div ref={rootRef} className="relative flex h-full flex-1 flex-col bg-background min-w-0">
      <div className="relative z-10 flex h-12 shrink-0 items-center gap-0.5 border-b border-border px-3">
        <Button
          variant="ghost"
          size="icon"
          onClick={() => s.readerNavigate(-1)}
          disabled={reader.index <= 0}
          title="Back (⌘[)"
          aria-label="Back"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          onClick={() => s.readerNavigate(1)}
          disabled={reader.index >= reader.history.length - 1}
          title="Forward (⌘])"
          aria-label="Forward"
        >
          <ArrowRight className="h-3.5 w-3.5" />
        </Button>
        <div className="mx-1.5 flex min-w-0 flex-1 items-center gap-1.5">
          {source && sourceIcon(source.sourceType, source.url)}
          <span
            className="truncate text-[13px] font-medium text-foreground"
            title={source?.title ?? note?.title}
          >
            {source?.title ?? note?.title ?? "Document"}
          </span>
        </div>
        <div className="flex shrink-0 items-center gap-0.5">
          {syncing && (
            <span className="px-1.5 text-muted-foreground" title="Refreshing…">
              <RefreshCw className="h-3.5 w-3.5 animate-spin" />
            </span>
          )}
          {source && (
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setFindOpen((open) => !open)}
              title="Find in source (⌘F)"
              aria-label="Find in source"
            >
              <Search className="h-4 w-4" />
            </Button>
          )}
          {note && !editing && (
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setEditing(true)}
              title="Edit note"
              aria-label="Edit note"
            >
              <Pencil className="h-4 w-4" />
            </Button>
          )}
          {inlineActions.map((action) => (
            <Button
              key={action.label}
              variant="ghost"
              size="icon"
              onClick={action.onClick}
              title={action.label}
              aria-label={action.label}
            >
              {action.icon}
            </Button>
          ))}
          {overflowItems.length > 0 && (
            <RowMenu
              className="!flex"
              label="Document actions"
              items={overflowItems}
            />
          )}
        </div>
      </div>
      {current === null ? (
        <div className="flex flex-1 items-center justify-center text-[13px] text-muted-foreground">
          Open a source or note to read it here.
        </div>
      ) : source ? (
        <SourceReader
          key={source.id}
          source={source}
          highlight={current.highlight}
          findOpen={findOpen}
          onFindOpen={() => setFindOpen(true)}
          onFindClose={() => setFindOpen(false)}
          refreshTick={refreshTick}
        />
      ) : note ? (
        <NoteReader
          key={note.id}
          note={note}
          editing={editing}
          onEditingChange={setEditing}
        />
      ) : (
        <div className="flex flex-1 items-center justify-center text-[13px] text-muted-foreground">
          This document no longer exists — it may have been deleted.
        </div>
      )}
    </div>
  );
}

/** "N words · N chars · ~N tokens" (chars/4 — the number agents care about). */
function countsLine(text: string): string {
  const words = text.split(/\s+/).filter(Boolean).length;
  const chars = text.length;
  const fmt = Intl.NumberFormat();
  return `${fmt.format(words)} words · ${fmt.format(chars)} chars · ~${fmt.format(
    Math.round(chars / 4),
  )} tokens`;
}

/** Full-text source reading: faithful markdown when the content is markdown-
 *  shaped, find-in-source, citation highlight, and select-to-ask. */
function SourceReader({
  source,
  highlight,
  findOpen,
  onFindOpen,
  onFindClose,
  refreshTick,
}: {
  source: Source;
  highlight?: string;
  findOpen: boolean;
  onFindOpen: () => void;
  onFindClose: () => void;
  refreshTick: number;
}) {
  const sendMessage = useStore((s) => s.sendMessage);
  const sending = useStore((s) => s.sending);
  const reading = useStore((s) => s.reading);
  const [content, setContent] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [sel, setSel] = useState<{ text: string; top: number; left: number } | null>(
    null,
  );
  const [backlinks, setBacklinks] = useState<
    { kind: "source" | "note"; id: string; title: string }[]
  >([]);
  const [preview, setPreview] = useState<{
    source: Source;
    top: number;
    left: number;
  } | null>(null);
  const previewTimer = useRef<number | null>(null);
  const markRef = useRef<HTMLElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  // "Linked from" — who in this notebook points at the open document.
  useEffect(() => {
    let stale = false;
    setBacklinks([]);
    void api
      .sourceBacklinks(source.id)
      .then((links) => {
        if (!stale) setBacklinks(links);
      })
      .catch(() => undefined);
    return () => {
      stale = true;
    };
  }, [source.id]);

  // Wikipedia-style hover previews for links that resolve to another source.
  function onBodyMouseOver(e: React.MouseEvent) {
    const a = (e.target as HTMLElement).closest?.("a");
    if (previewTimer.current) {
      window.clearTimeout(previewTimer.current);
      previewTimer.current = null;
    }
    if (!a || !bodyRef.current) {
      setPreview(null);
      return;
    }
    const hit = resolveInCorpus(a.getAttribute("href") ?? "", source.url || undefined);
    if (!hit) {
      setPreview(null);
      return;
    }
    const rect = a.getBoundingClientRect();
    const wrap = bodyRef.current.getBoundingClientRect();
    previewTimer.current = window.setTimeout(() => {
      setPreview({
        source: hit,
        top: Math.max(rect.top - wrap.top + bodyRef.current!.scrollTop, 40),
        left: Math.min(
          Math.max(rect.left - wrap.left + rect.width / 2, 140),
          Math.max(wrap.width - 140, 140),
        ),
      });
    }, 350);
  }

  useEffect(() => {
    let stale = false;
    api
      .getSourceContent(source.id)
      .then((text) => {
        if (!stale) setContent(text);
      })
      .catch(() => {
        if (!stale) setContent("");
      });
    return () => {
      stale = true;
    };
  }, [source.id, refreshTick]);

  // Cmd/Ctrl+F opens the find bar and focuses it (Safari-style: the bar
  // exists only while finding).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "f") {
        e.preventDefault();
        onFindOpen();
        requestAnimationFrame(() => {
          searchRef.current?.focus();
          searchRef.current?.select();
        });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onFindOpen]);

  // The bar opening (via toolbar button) grabs focus; closing clears the
  // query so highlights drop back to the citation passage.
  useEffect(() => {
    if (findOpen) {
      requestAnimationFrame(() => searchRef.current?.focus());
    } else {
      setQuery("");
      setActive(0);
    }
  }, [findOpen]);

  const matches = useMemo(
    () => (content ? findMatches(content, query) : []),
    [content, query],
  );
  const passage = useMemo(
    () =>
      content && highlight && !query.trim()
        ? locatePassage(content, highlight)
        : null,
    [content, highlight, query],
  );

  const ranges: [number, number][] = query.trim()
    ? matches
    : passage
      ? [passage]
      : [];
  const activeIdx = query.trim()
    ? Math.min(active, Math.max(0, matches.length - 1))
    : 0;

  useEffect(() => {
    markRef.current?.scrollIntoView({ block: "center" });
  }, [content, activeIdx, query, passage]);

  const step = (dir: 1 | -1) => {
    if (matches.length === 0) return;
    setActive((a) => (a + dir + matches.length) % matches.length);
  };

  // Selection → ask toolbar (window-level mouseup so releasing outside the
  // container still raises it; the handler validates the selection home).
  const updateSelectionRef = useRef<() => void>(() => {});
  useEffect(() => {
    const onUp = () => updateSelectionRef.current();
    window.addEventListener("mouseup", onUp);
    return () => window.removeEventListener("mouseup", onUp);
  }, []);

  function updateSelection() {
    const container = bodyRef.current;
    const s = window.getSelection();
    if (!container || !s || s.isCollapsed || s.rangeCount === 0) {
      setSel(null);
      return;
    }
    const range = s.getRangeAt(0);
    if (!container.contains(range.commonAncestorContainer)) {
      setSel(null);
      return;
    }
    const text = s.toString().trim();
    if (text.length < 3) {
      setSel(null);
      return;
    }
    const rect = range.getBoundingClientRect();
    const wrap = container.getBoundingClientRect();
    setSel({
      text,
      top: Math.max(rect.top - wrap.top + container.scrollTop, 44),
      left: Math.min(
        Math.max(rect.left - wrap.left + rect.width / 2, 150),
        Math.max(wrap.width - 150, 150),
      ),
    });
  }
  updateSelectionRef.current = updateSelection;

  const selectedPassage = () =>
    sel && sel.text.length > MAX_PASSAGE_CHARS
      ? `${sel.text.slice(0, MAX_PASSAGE_CHARS)}…`
      : (sel?.text ?? "");

  function askAbout(question: string) {
    const p = selectedPassage();
    if (!p) return;
    setSel(null);
    useStore.getState().closeReader();
    void sendMessage(`${question}\n\n"${p}"`);
  }

  function askCustom() {
    const p = selectedPassage();
    if (!p) return;
    setSel(null);
    useStore.getState().closeReader();
    useStore.setState({
      pendingInput: `About this passage from "${source.title}":\n"${p}"\n\n`,
    });
  }

  // Faithful rendering: markdown-shaped sources render as markdown UNLESS a
  // find query or citation highlight is active — those need the plain-text
  // segment view to mark exact ranges (rendered-DOM highlighting is phase 2).
  const richMode =
    (source.sourceType === "markdown" ||
      ((source.sourceType === "text" || source.sourceType === "url") &&
        !!content &&
        looksLikeMarkdown(content))) &&
    ranges.length === 0;

  const segments: { text: string; hit: boolean; current: boolean }[] = [];
  if (content && !richMode) {
    let pos = 0;
    ranges.forEach(([s, e], i) => {
      if (s > pos)
        segments.push({ text: content.slice(pos, s), hit: false, current: false });
      segments.push({ text: content.slice(s, e), hit: true, current: i === activeIdx });
      pos = e;
    });
    if (pos < content.length)
      segments.push({ text: content.slice(pos), hit: false, current: false });
  }

  return (
    <>
      {findOpen && (
        <div className="flex shrink-0 items-center justify-end gap-1.5 border-b border-border px-5 py-1.5">
          <div className="relative">
            <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtle-foreground" />
            <Input
              ref={searchRef}
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setActive(0);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") step(e.shiftKey ? -1 : 1);
                else if (e.key === "Escape") {
                  e.stopPropagation();
                  onFindClose();
                }
              }}
              placeholder="Find in source…"
              className="h-7 w-56 pl-7 text-[12px]"
            />
          </div>
          <span className="min-w-8 text-right text-[11px] tabular-nums text-subtle-foreground">
            {query.trim()
              ? matches.length === 0
                ? "0/0"
                : `${activeIdx + 1}/${matches.length}`
              : ""}
          </span>
          <button
            className="rounded p-1 text-muted-foreground hover:text-foreground disabled:opacity-40"
            onClick={() => step(-1)}
            disabled={matches.length === 0}
            aria-label="Previous match"
          >
            <ChevronUp className="h-3.5 w-3.5" />
          </button>
          <button
            className="rounded p-1 text-muted-foreground hover:text-foreground disabled:opacity-40"
            onClick={() => step(1)}
            disabled={matches.length === 0}
            aria-label="Next match"
          >
            <ChevronDown className="h-3.5 w-3.5" />
          </button>
          <Button variant="ghost" size="sm" onClick={onFindClose}>
            Done
          </Button>
        </div>
      )}

      <div
        ref={bodyRef}
        className="relative min-h-0 flex-1 overflow-y-auto"
        onClickCapture={docLinkClickHandler(source.url || undefined)}
        onMouseOver={onBodyMouseOver}
        onScroll={() => setPreview(null)}
      >
        {preview && (
          <button
            type="button"
            className="absolute z-10 flex w-64 flex-col gap-1 rounded-md border border-border-strong bg-elevated p-2.5 text-left shadow-lg"
            style={{
              top: preview.top,
              left: preview.left,
              transform: "translate(-50%, calc(-100% - 8px))",
            }}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() =>
              useStore.getState().openInReader({ type: "source", id: preview.source.id })
            }
          >
            <span className="flex items-center gap-1.5 text-[12px] font-medium text-foreground">
              {sourceIcon(preview.source.sourceType, preview.source.url)}
              <span className="truncate">{preview.source.title}</span>
            </span>
            <span className="text-[11px] text-subtle-foreground">
              In this notebook · {preview.source.chunkCount} chunks ·{" "}
              {Intl.NumberFormat().format(preview.source.charCount)} chars
            </span>
          </button>
        )}
        {sel && content && (
          <div
            className="absolute z-10 flex items-center gap-0.5 rounded-md border border-border-strong bg-elevated p-0.5 shadow-lg"
            style={{
              top: sel.top,
              left: sel.left,
              transform: "translate(-50%, calc(-100% - 6px))",
            }}
            onMouseDown={(e) => e.preventDefault()}
            onMouseUp={(e) => e.stopPropagation()}
            role="toolbar"
            aria-label="Ask about selection"
          >
            <SelAction
              icon={<Sparkles className="h-3.5 w-3.5" />}
              label="Explain"
              disabled={sending}
              onClick={() => askAbout(`Explain this passage from "${source.title}":`)}
            />
            <SelAction
              icon={<Scale className="h-3.5 w-3.5" />}
              label="Compare sources"
              disabled={sending}
              onClick={() =>
                askAbout(
                  `What do the other sources say about this passage from "${source.title}"? ` +
                    "Note where they agree, disagree, or add context:",
                )
              }
            />
            <SelAction
              icon={<MessageSquarePlus className="h-3.5 w-3.5" />}
              label="Ask…"
              onClick={askCustom}
            />
          </div>
        )}
        <div className={cn("mx-auto max-w-[760px] px-8 py-6", chatReadingClass(reading))}>
          {content === null ? (
            <div className="flex items-center gap-2 text-[13px] text-muted-foreground">
              <Spinner className="h-3.5 w-3.5" /> Loading source…
            </div>
          ) : content === "" ? (
            <div className="text-[13px] text-muted-foreground">
              No text stored for this source.
            </div>
          ) : richMode ? (
            <div className="selectable">
              <Markdown>{content}</Markdown>
            </div>
          ) : (
            <p className="reader-plain whitespace-pre-wrap text-[13px] leading-relaxed text-foreground/90 selectable">
              {segments.map((seg, i) =>
                seg.hit ? (
                  <mark
                    key={i}
                    ref={seg.current ? markRef : undefined}
                    className={cn(
                      "rounded-sm px-0.5 text-foreground",
                      seg.current && query.trim() ? "bg-primary/40" : "bg-primary/15",
                    )}
                  >
                    {seg.text}
                  </mark>
                ) : (
                  <span key={i}>{seg.text}</span>
                ),
              )}
            </p>
          )}
        </div>
      </div>
      {content && (
        <div className="flex shrink-0 items-center gap-2 border-t border-border px-5 py-1 text-[11px] tabular-nums text-subtle-foreground">
          <span className="min-w-0 truncate whitespace-nowrap">
            {source.chunkCount} chunks · {countsLine(content)}
          </span>
          {backlinks.length > 0 && (
            <span className="group ml-auto flex shrink-0 items-center">
              <RowMenu
                // The text link is the visible affordance; the RowMenu's own
                // trigger only anchors the dropdown.
                className="!flex [&>button:first-child]:hidden"
                label={`Linked from ${backlinks.length} ${
                  backlinks.length === 1 ? "document" : "documents"
                }`}
                items={backlinks.map((b) => ({
                  label: `${b.title}${b.kind === "note" ? " (note)" : ""}`,
                  icon: <BookOpen className="h-3.5 w-3.5" />,
                  onClick: () =>
                    useStore
                      .getState()
                      .openInReader({ type: b.kind, id: b.id }),
                }))}
              />
              <button
                type="button"
                className="text-citation hover:underline"
                onClick={(e) => {
                  const menu = (e.currentTarget.previousElementSibling as HTMLElement)
                    ?.querySelector("button");
                  (menu as HTMLButtonElement | null)?.click();
                }}
              >
                ← linked from {backlinks.length}
              </button>
            </span>
          )}
        </div>
      )}
    </>
  );
}

function SelAction({
  icon,
  label,
  onClick,
  disabled,
}: {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      className={cn(
        "flex items-center gap-1.5 whitespace-nowrap rounded px-2 py-1 text-[12px] text-foreground/90",
        "transition-colors hover:bg-surface-2 hover:text-foreground disabled:opacity-40",
      )}
      onClick={onClick}
      disabled={disabled}
    >
      <span className="text-citation">{icon}</span>
      {label}
    </button>
  );
}

/** Notes in the reader: every kind uses its native renderer. Actions live in
 *  the toolbar's overflow menu; artifact kinds that manage their own bottom
 *  controls (deck, mind map) fill the pane with no extra chrome. */
function NoteReader({
  note,
  editing,
  onEditingChange,
}: {
  note: Note;
  editing: boolean;
  onEditingChange: (editing: boolean) => void;
}) {
  const reading = useStore((s) => s.reading);
  const updateNote = useStore((s) => s.updateNote);
  const generatingKind = useStore((s) => s.generatingKind);
  const artifactStreamText = useStore((s) => s.artifactStreamText);
  const [title, setTitle] = useState(note.title);
  const [body, setBody] = useState(note.content);

  // Entering edit mode snapshots the note; leaving without saving discards.
  useEffect(() => {
    if (editing) {
      setTitle(note.title);
      setBody(note.content);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing]);

  const rebuilding = !!generatingKind && note.kind !== "note";
  // Kinds that size themselves to the pane and bring their own controls.
  const fillsPane = note.kind === "slide_deck" || note.kind === "mind_map";

  if (editing) {
    return (
      <form
        className="flex min-h-0 flex-1 flex-col gap-3 px-6 py-4"
        onSubmit={(event) => {
          event.preventDefault();
          updateNote(note.id, title, body);
          onEditingChange(false);
        }}
      >
        <Input
          name="note-title"
          aria-label="Note title"
          value={title}
          onChange={(event) => setTitle(event.target.value)}
        />
        <div className="min-h-0 flex-1 overflow-y-auto">
          <RichEditor value={body} onChange={setBody} />
        </div>
        <div className="flex shrink-0 justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onEditingChange(false)}>
            Cancel
          </Button>
          <Button type="submit" variant="primary">
            Save
          </Button>
        </div>
      </form>
    );
  }

  return (
    <>
      <div
        className={cn(
          "min-h-0 flex-1",
          fillsPane ? "overflow-hidden px-6 py-4" : "overflow-y-auto px-8 py-6",
        )}
      >
        <div className={cn("mx-auto h-full", fillsPane ? "max-w-none" : "max-w-[760px]")}>
          {rebuilding && artifactStreamText ? (
            <StreamingBody text={artifactStreamText} />
          ) : note.kind === "mind_map" ? (
            <MindMap content={note.content} />
          ) : note.kind === "flashcards" ? (
            <Flashcards content={note.content} noteId={note.id} />
          ) : note.kind === "quiz" ? (
            <QuizView content={note.content} />
          ) : note.kind === "slide_deck" ? (
            <SlideDeck content={note.content} note={note} />
          ) : note.kind === "audio_overview" ? (
            <div className="flex flex-col gap-4">
              <AudioPlayer key={note.updatedAt} noteId={note.id} title={note.title} />
              <DialogueScript content={note.content} />
            </div>
          ) : (
            <div
              className={chatReadingClass(reading)}
              onClickCapture={docLinkClickHandler(undefined)}
            >
              <Markdown>{note.content}</Markdown>
            </div>
          )}
        </div>
      </div>
      {!fillsPane && (
        <div className="shrink-0 overflow-hidden truncate whitespace-nowrap border-t border-border px-5 py-1.5 text-[11px] tabular-nums text-subtle-foreground">
          {countsLine(note.content)}
        </div>
      )}
    </>
  );
}
