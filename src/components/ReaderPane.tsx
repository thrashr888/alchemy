import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import type { Citation, Note, Source } from "@/lib/types";
import { AmbientRail, activeParagraph } from "./AmbientRail";
import { AudioPlayer, DialogueScript } from "./AudioNote";
import { Flashcards } from "./Flashcards";
import { Markdown } from "./Markdown";
import { MindMap } from "./MindMap";
import { QuizView } from "./QuizView";
import { SlideDeck } from "./SlideDeck";
import { RichEditor } from "./RichEditor";
import { StreamingBody } from "./StudioNoteViewer";
import { Favicon, sourceIcon } from "./SourcesPanel";
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
  Link2,
  ListTree,
  Search,
  SlidersHorizontal,
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

/** Markdown snippet → plain text, approximating what the rendered DOM shows
 *  (links keep their text, syntax markers drop). Good enough for matching. */
function mdToPlain(md: string): string {
  return md
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^#{1,6}\s+/gm, "")
    .replace(/^\s*>\s?/gm, "")
    .replace(/^\s*[-*+]\s+/gm, "")
    .replace(/^\s*\d+\.\s+/gm, "")
    .replace(/\|/g, " ")
    .replace(/^[-\s:|]+$/gm, "")
    .replace(/[*_`~]/g, "");
}

/**
 * Locate `needle` (a citation snippet, possibly markdown) inside the rendered
 * DOM of `container`, whitespace- and syntax-tolerant: both sides are
 * squashed to lowercase non-whitespace characters, and match offsets map back
 * to exact text-node positions. Falls back to the snippet's head when chunk
 * boundaries clip the tail. Returns a Range, or null when the text can't be
 * found (caller falls back to the plain-text view).
 */
function findTextRange(container: HTMLElement, needle: string): Range | null {
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  let hay = "";
  const map: { node: Text; offset: number }[] = [];
  while (walker.nextNode()) {
    const textNode = walker.currentNode as Text;
    const data = textNode.data;
    for (let i = 0; i < data.length; i++) {
      if (!/\s/.test(data[i])) {
        hay += data[i].toLowerCase();
        map.push({ node: textNode, offset: i });
      }
    }
  }
  let target = mdToPlain(needle).toLowerCase().replace(/\s+/g, "");
  if (target.length < 12) return null;
  let at = hay.indexOf(target);
  if (at === -1 && target.length > 80) {
    target = target.slice(0, 80);
    at = hay.indexOf(target);
  }
  if (at === -1) return null;
  const start = map[at];
  const end = map[at + target.length - 1];
  const range = document.createRange();
  range.setStart(start.node, start.offset);
  range.setEnd(end.node, end.offset + 1);
  return range;
}

/** All occurrences of `query` in the rendered DOM (squashed matching, like
 *  findTextRange), capped — powers find-in-source on the rendered view. */
function findAllRanges(container: HTMLElement, query: string): Range[] {
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  let hay = "";
  const map: { node: Text; offset: number }[] = [];
  while (walker.nextNode()) {
    const textNode = walker.currentNode as Text;
    const data = textNode.data;
    for (let i = 0; i < data.length; i++) {
      if (!/\s/.test(data[i])) {
        hay += data[i].toLowerCase();
        map.push({ node: textNode, offset: i });
      }
    }
  }
  const target = query.toLowerCase().replace(/\s+/g, "");
  if (target.length < 2) return [];
  const out: Range[] = [];
  let at = hay.indexOf(target);
  while (at !== -1 && out.length < 300) {
    const start = map[at];
    const end = map[at + target.length - 1];
    const range = document.createRange();
    range.setStart(start.node, start.offset);
    range.setEnd(end.node, end.offset + 1);
    out.push(range);
    at = hay.indexOf(target, at + target.length);
  }
  return out;
}

/** Register find highlights (all matches + the active one). */
function applyFindHighlights(ranges: Range[], active: number): boolean {
  const registry = (
    CSS as unknown as { highlights?: Map<string, unknown> }
  ).highlights;
  const HighlightCtor = (
    window as unknown as { Highlight?: new (...r: Range[]) => unknown }
  ).Highlight;
  if (!registry || !HighlightCtor) return false;
  if (ranges.length === 0) {
    registry.delete("find");
    registry.delete("find-active");
    return true;
  }
  registry.set("find", new HighlightCtor(...ranges));
  const current = ranges[Math.min(active, ranges.length - 1)];
  registry.set("find-active", new HighlightCtor(current));
  return true;
}

/** Per-document scroll positions, remembered for the session. */
const scrollMemory = new Map<string, number>();

/** Restore (once content is ready) and record a container's scroll position.
 *  `restore` false records without jumping (e.g. a citation anchor wins). */
function useScrollMemory(
  ref: React.RefObject<HTMLElement | null>,
  key: string,
  ready: boolean,
  restore: boolean,
) {
  useEffect(() => {
    const el = ref.current;
    if (!el || !ready) return;
    if (restore) {
      const saved = scrollMemory.get(key);
      if (saved) el.scrollTop = saved;
    }
    const onScroll = () => scrollMemory.set(key, el.scrollTop);
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, ready]);
}

/** CSS Custom Highlight for citation anchors (no DOM mutation). Returns
 *  false when unsupported so callers can fall back to the plain view. */
function applyCitationHighlight(range: Range | null): boolean {
  const registry = (
    CSS as unknown as { highlights?: Map<string, unknown> }
  ).highlights;
  if (!registry) return false;
  if (range) {
    const HighlightCtor = (
      window as unknown as { Highlight?: new (r: Range) => unknown }
    ).Highlight;
    if (!HighlightCtor) return false;
    registry.set("citation", new HighlightCtor(range));
  } else {
    registry.delete("citation");
  }
  return true;
}

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
  const [liveMode, setLiveMode] = useState(false);
  const [imageMode, setImageMode] = useState(true);
  useEffect(() => {
    setFindOpen(false);
    setEditing(false);
    setImageMode(true);
    // A web source whose extraction failed has no cached article — open
    // straight in the Live view instead of a dead "no text" pane.
    const doc = useStore.getState().reader.history[useStore.getState().reader.index];
    const src =
      doc?.type === "source"
        ? useStore.getState().sources.find((x) => x.id === doc.id)
        : null;
    setLiveMode(!!src && src.status === "error" && isWebUrl(src.url));
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
  const copyLinkAction = note
    ? {
        label: "Copy link",
        icon: <Link2 className="h-3.5 w-3.5" />,
        onClick: () => {
          void navigator.clipboard
            .writeText(`alchemy://note/${note.id}`)
            .then(() => useStore.getState().pushToast("success", "Link copied"));
        },
      }
    : source?.url
      ? {
          label: isWebUrl(source.url) ? "Copy URL" : "Copy file path",
          icon: <Link2 className="h-3.5 w-3.5" />,
          onClick: () => {
            void navigator.clipboard
              .writeText(source.url)
              .then(() =>
                useStore.getState().pushToast("success", "Copied"),
              );
          },
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
    ...(copyLinkAction ? [copyLinkAction] : []),
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
          {source &&
            (isWebUrl(source.url) ? (
              <Favicon url={source.url} />
            ) : (
              sourceIcon(source.sourceType, source.url)
            ))}
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
          {source && source.sourceType === "image" && source.url && (
            <div className="mr-1 flex shrink-0 items-center gap-0.5 rounded-lg border border-border p-0.5">
              {(["image", "text"] as const).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  onClick={() => setImageMode(mode === "image")}
                  aria-pressed={imageMode === (mode === "image")}
                  title={
                    mode === "image"
                      ? "The original image"
                      : "The OCR transcription (searchable)"
                  }
                  className={cn(
                    "rounded-md px-2 py-0.5 text-[11px] font-medium capitalize transition-colors",
                    imageMode === (mode === "image")
                      ? "bg-surface-2 text-foreground"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  {mode}
                </button>
              ))}
            </div>
          )}
          {source && isWebUrl(source.url) && (
            <div className="mr-1 flex shrink-0 items-center gap-0.5 rounded-lg border border-border p-0.5">
              {(["cached", "live"] as const).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  onClick={() => setLiveMode(mode === "live")}
                  aria-pressed={liveMode === (mode === "live")}
                  title={
                    mode === "live"
                      ? "The actual page, embedded in the reader"
                      : "The extracted article (fast, offline, searchable)"
                  }
                  className={cn(
                    "rounded-md px-2 py-0.5 text-[11px] font-medium capitalize transition-colors",
                    liveMode === (mode === "live")
                      ? "bg-surface-2 text-foreground"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  {mode}
                </button>
              ))}
            </div>
          )}
          {source && !liveMode && (
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
          {note &&
            !editing &&
            [
              "slide_deck",
              "mind_map",
              "quiz",
              "flashcards",
              "audio_overview",
              "report",
            ].includes(note.kind) && (
              <Button
                variant="ghost"
                size="icon"
                onClick={() => setEditing(true)}
                title="Edit the raw markdown"
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
          <Button
            variant="ghost"
            size="icon"
            onClick={() => useStore.getState().openSettings("appearance")}
            title="Reader settings (contents, citation highlights, type)"
            aria-label="Reader settings"
          >
            <SlidersHorizontal className="h-3.5 w-3.5" />
          </Button>
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
          live={liveMode}
          imageView={imageMode}
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

/** Headings extracted from markdown (fence-aware) for the TOC. */
function parseHeadings(content: string): { level: number; text: string }[] {
  const out: { level: number; text: string }[] = [];
  let inFence = false;
  for (const line of content.split("\n")) {
    if (/^```/.test(line.trim())) inFence = !inFence;
    if (inFence) continue;
    const m = /^(#{1,3})\s+(.*)$/.exec(line);
    if (m) out.push({ level: m[1].length, text: mdToPlain(m[2]).trim() });
  }
  return out;
}

/** The TOC list itself: scroll-synced, click-to-jump. Placement (rail or
 *  popover) belongs to DocRails. */
function TocList({
  headings,
  scrollerRef,
}: {
  headings: { level: number; text: string }[];
  scrollerRef: React.RefObject<HTMLElement | null>;
}) {
  const [active, setActive] = useState(0);
  // Scroll-sync: the active entry is the last heading above the viewport top.
  useEffect(() => {
    const el = scrollerRef.current;
    if (!el || headings.length < 3) return;
    const sync = () => {
      const els = el.querySelectorAll("h1, h2, h3");
      const top = el.getBoundingClientRect().top;
      let current = 0;
      els.forEach((h, i) => {
        if (h.getBoundingClientRect().top <= top + 90) current = i;
      });
      setActive(current);
    };
    sync();
    el.addEventListener("scroll", sync, { passive: true });
    return () => el.removeEventListener("scroll", sync);
  }, [headings.length, scrollerRef]);

  return (
    <nav aria-label="Table of contents" className="flex min-h-0 flex-col">
      <div className="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-subtle-foreground">
        Contents
      </div>
      <div className="flex flex-col overflow-y-auto">
        {headings.map((h, i) => (
          <button
            key={`${i}-${h.text}`}
            type="button"
            onClick={() => {
              const el = scrollerRef.current;
              if (!el) return;
              const target = [...el.querySelectorAll("h1, h2, h3")].find(
                (node) => (node.textContent ?? "").trim() === h.text,
              );
              target?.scrollIntoView({ block: "start", behavior: "smooth" });
            }}
            className={cn(
              "truncate rounded px-1.5 py-0.5 text-left text-[11px] leading-relaxed transition-colors",
              h.level === 2 && "pl-4",
              h.level === 3 && "pl-6",
              i === active
                ? "text-foreground"
                : "text-subtle-foreground hover:text-muted-foreground",
            )}
            title={h.text}
          >
            {h.text}
          </button>
        ))}
      </div>
    </nav>
  );
}

/**
 * The reader's side rails: table of contents (left) and related passages
 * (right), both hugging the centered text column — never pinned to the
 * window edge, never overlapping the text. Two translucent corner buttons
 * are the persistent controls: with room, clicking toggles the rail's
 * preference (persisted); without room, clicking opens the same content as
 * a transient popover under the button.
 */
function DocRails({
  content,
  scrollerRef,
  relatedText,
  excludeNoteId,
  excludeSourceId,
  width,
  onInsert,
}: {
  content: string;
  scrollerRef: React.RefObject<HTMLElement | null>;
  relatedText: string;
  excludeNoteId?: string;
  excludeSourceId?: string;
  width: number;
  onInsert?: (c: Citation) => void;
}) {
  const showToc = useStore((s) => s.reading.showToc);
  const showRelated = useStore((s) => s.reading.showRelated);
  const setReading = useStore((s) => s.setReading);
  const headings = useMemo(() => parseHeadings(content), [content]);
  const hasToc = headings.length >= 3;
  // Column is 760px centered; rails need their width + a 20px gap beside it.
  const tocFits = width >= 760 + 2 * (176 + 20) + 24;
  const relatedFits = width >= 760 + 2 * (224 + 20) + 24;
  const [tocPop, setTocPop] = useState(false);
  const [relPop, setRelPop] = useState(false);

  const button = (
    side: "left" | "right",
    icon: React.ReactNode,
    label: string,
    railVisible: boolean,
    enabled: boolean,
    fits: boolean,
    togglePref: () => void,
    popOpen: boolean,
    setPop: (open: boolean) => void,
  ) => (
    <button
      type="button"
      onClick={() => {
        if (fits) {
          togglePref();
          setPop(false);
        } else if (!enabled) {
          togglePref();
          setPop(true);
        } else {
          setPop(!popOpen);
        }
      }}
      title={
        fits
          ? `${railVisible ? "Hide" : "Show"} ${label}`
          : `${popOpen ? "Hide" : "Show"} ${label}`
      }
      aria-label={label}
      aria-pressed={railVisible || popOpen}
      className={cn(
        "absolute top-3 z-20 rounded-md border p-1.5 backdrop-blur transition-colors",
        side === "left" ? "left-3" : "right-3",
        railVisible || popOpen
          ? "border-border-strong bg-elevated/80 text-foreground"
          : "border-border/50 bg-elevated/50 text-subtle-foreground hover:text-muted-foreground",
      )}
    >
      {icon}
    </button>
  );

  return (
    <>
      {hasToc &&
        button(
          "left",
          <ListTree className="h-3.5 w-3.5" />,
          "table of contents",
          showToc && tocFits,
          showToc,
          tocFits,
          () => setReading({ showToc: !showToc }),
          tocPop,
          setTocPop,
        )}
      {button(
        "right",
        <Sparkles className="h-3.5 w-3.5" />,
        "related passages",
        showRelated && relatedFits,
        showRelated,
        relatedFits,
        () => setReading({ showRelated: !showRelated }),
        relPop,
        setRelPop,
      )}
      {hasToc && showToc && tocFits && (
        <div
          className="absolute bottom-10 top-14 z-10 flex w-44 flex-col"
          style={{ right: "calc(50% + 380px + 20px)" }}
        >
          <TocList headings={headings} scrollerRef={scrollerRef} />
        </div>
      )}
      {hasToc && tocPop && !(showToc && tocFits) && (
        <div className="menu-glass absolute left-3 top-12 z-20 flex max-h-[70%] w-56 flex-col overflow-y-auto rounded-lg border border-border/60 p-2.5 shadow-lg">
          <TocList headings={headings} scrollerRef={scrollerRef} />
        </div>
      )}
      {showRelated && relatedFits && (
        <div
          className="absolute bottom-10 top-14 z-10 flex w-56 flex-col overflow-y-auto"
          style={{ left: "calc(50% + 380px + 20px)" }}
        >
          <AmbientRail
            text={relatedText}
            excludeNoteId={excludeNoteId}
            excludeSourceId={excludeSourceId}
            onInsert={onInsert}
          />
        </div>
      )}
      {relPop && !(showRelated && relatedFits) && (
        <div className="menu-glass absolute right-3 top-12 z-20 flex max-h-[70%] w-64 flex-col overflow-y-auto rounded-lg border border-border/60 p-2.5 shadow-lg">
          <AmbientRail
            emptyState
            text={relatedText}
            excludeNoteId={excludeNoteId}
            excludeSourceId={excludeSourceId}
            onInsert={onInsert}
          />
        </div>
      )}
    </>
  );
}

/** The original image behind an image source. WKWebView won't decode
 *  asset:// URLs in <img> elements (fetch of the same URL works fine), so
 *  the bytes come in via fetch and render from a blob URL. */
function ImageView({ url, title }: { url: string; title: string }) {
  const [blobUrl, setBlobUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let revoked: string | null = null;
    let stale = false;
    setBlobUrl(null);
    setFailed(false);
    fetch(convertFileSrc(url))
      .then((r) => (r.ok ? r.blob() : Promise.reject(new Error(`${r.status}`))))
      .then((blob) => {
        if (stale) return;
        revoked = URL.createObjectURL(blob);
        setBlobUrl(revoked);
      })
      .catch(() => {
        if (!stale) setFailed(true);
      });
    return () => {
      stale = true;
      if (revoked) URL.revokeObjectURL(revoked);
    };
  }, [url]);
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center overflow-hidden p-6">
      {failed ? (
        <span className="text-[13px] text-muted-foreground">
          The original file could not be read — it may have moved.
        </span>
      ) : blobUrl ? (
        <img
          src={blobUrl}
          alt={title}
          className="max-h-full max-w-full rounded-md border border-border object-contain shadow-sm"
        />
      ) : (
        <Spinner className="h-4 w-4" />
      )}
    </div>
  );
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
  live,
  imageView = false,
}: {
  source: Source;
  highlight?: string;
  findOpen: boolean;
  onFindOpen: () => void;
  onFindClose: () => void;
  refreshTick: number;
  live: boolean;
  imageView?: boolean;
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
  // Rendered-DOM citation anchoring: when the passage can't be located in
  // the rendered view (or CSS highlights are unsupported), fall back to the
  // exact plain-text segment view.
  const [anchorFailed, setAnchorFailed] = useState(false);
  // Reading-mode ambient rail: the visible section drives related passages.
  const [sectionText, setSectionText] = useState("");
  const [preview, setPreview] = useState<{
    source: Source;
    top: number;
    left: number;
  } | null>(null);
  const previewTimer = useRef<number | null>(null);
  const markRef = useRef<HTMLElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const paneWidth = useElementWidth(bodyRef);

  // Live web view: a native child webview positioned over the placeholder
  // below (see live_view_* commands). Bounds track the placeholder; in-app
  // overlays (palette, modals) hide it so they are never painted over.
  const liveRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!live) return;
    const el = liveRef.current;
    if (!el) return;
    const rect = () => {
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, w: r.width, h: r.height };
    };
    void api.liveViewOpen(source.url, rect());
    const update = () => void api.liveViewBounds(rect());
    const ro = new ResizeObserver(update);
    ro.observe(el);
    window.addEventListener("resize", update);
    const overlayCheck = () =>
      void api.liveViewVisible(!document.querySelector('[role="dialog"]'));
    const mo = new MutationObserver(overlayCheck);
    mo.observe(document.body, { childList: true, subtree: true });
    return () => {
      ro.disconnect();
      mo.disconnect();
      window.removeEventListener("resize", update);
      void api.liveViewClose();
    };
  }, [live, source.url]);

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
  // Faithful rendering: markdown-shaped sources render as markdown. A find
  // query still uses the plain-text segment view (exact ranges); a citation
  // highlight anchors into the RENDERED view via CSS Custom Highlights,
  // dropping to the plain view only when the passage can't be located there.
  const markdownShaped =
    source.sourceType === "markdown" ||
    ((source.sourceType === "text" || source.sourceType === "url") &&
      !!content &&
      looksLikeMarkdown(content));
  const richMode = markdownShaped && !(highlight && anchorFailed);

  // Find-in-source on the RENDERED view: all matches get ::highlight(find),
  // the active one ::highlight(find-active) and a scroll-to. The plain
  // segment view keeps its own <mark> path for non-markdown sources.
  const [domMatchCount, setDomMatchCount] = useState(0);
  const domRanges = useRef<Range[]>([]);
  useEffect(() => {
    if (!richMode || content === null) return;
    const timer = window.setTimeout(() => {
      if (!bodyRef.current) return;
      const ranges = query.trim()
        ? findAllRanges(bodyRef.current, query.trim())
        : [];
      domRanges.current = ranges;
      setDomMatchCount(ranges.length);
      applyFindHighlights(ranges, active);
    }, 120);
    return () => {
      window.clearTimeout(timer);
      applyFindHighlights([], 0);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [richMode, content, query]);
  // Stepping through matches: retarget the active highlight and scroll.
  useEffect(() => {
    if (!richMode || domRanges.current.length === 0) return;
    const ranges = domRanges.current;
    const idx = ((active % ranges.length) + ranges.length) % ranges.length;
    applyFindHighlights(ranges, idx);
    const rect = ranges[idx].getBoundingClientRect();
    const body = bodyRef.current?.getBoundingClientRect();
    if (body && bodyRef.current) {
      bodyRef.current.scrollTop += rect.top - body.top - bodyRef.current.clientHeight / 3;
    }
  }, [active, richMode]);

  const matchTotal = richMode ? domMatchCount : matches.length;
  const activeIdx = query.trim()
    ? Math.min(active, Math.max(0, matchTotal - 1))
    : 0;

  useEffect(() => {
    markRef.current?.scrollIntoView({ block: "center" });
  }, [content, activeIdx, query, passage]);

  // Citation anchor in the RENDERED view: locate the passage among the text
  // nodes, highlight it (CSS Custom Highlight — no DOM mutation), and scroll
  // it to a third from the top. Runs after paint so the markdown DOM exists.
  useEffect(() => {
    if (!richMode || !highlight || content === null) return;
    let cancelled = false;
    // setTimeout, NOT requestAnimationFrame: rAF never fires while the
    // window is occluded (macOS pauses it), which would silently skip the
    // anchor. The markdown DOM is already committed when effects run.
    const timer = window.setTimeout(() => {
      if (cancelled || !bodyRef.current) return;
      const range = findTextRange(bodyRef.current, highlight);
      if (!range || !applyCitationHighlight(range)) {
        setAnchorFailed(true);
        return;
      }
      const rect = range.getBoundingClientRect();
      const body = bodyRef.current.getBoundingClientRect();
      bodyRef.current.scrollTop +=
        rect.top - body.top - bodyRef.current.clientHeight / 3;
    });
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      applyCitationHighlight(null);
    };
  }, [richMode, highlight, content]);
  useEffect(() => {
    setAnchorFailed(false);
  }, [highlight, source.id]);


  // Reading position survives doc-switching (session-scoped); a citation
  // anchor wins over the remembered position.
  useScrollMemory(bodyRef, `source:${source.id}`, content !== null, !highlight);

  // Track the section in view (throttled) for the reading-mode rail.
  useEffect(() => {
    if (!richMode || content === null) return;
    const el = bodyRef.current;
    if (!el) return;
    let timer: number | null = null;
    const compute = () => {
      timer = null;
      const blocks = el.querySelectorAll("p, li, h1, h2, h3, blockquote");
      const top = el.getBoundingClientRect().top;
      let text = "";
      for (const b of blocks) {
        const r = b.getBoundingClientRect();
        if (r.bottom < top + 40) continue;
        text += " " + (b.textContent ?? "");
        if (text.length > 500) break;
      }
      setSectionText(text.trim().slice(0, 600));
    };
    const onScroll = () => {
      if (timer === null) timer = window.setTimeout(compute, 350);
    };
    compute();
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      if (timer !== null) window.clearTimeout(timer);
      el.removeEventListener("scroll", onScroll);
    };
  }, [richMode, content]);

  const step = (dir: 1 | -1) => {
    if (matchTotal === 0) return;
    setActive((a) => (a + dir + matchTotal) % matchTotal);
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

  if (source.sourceType === "image" && source.url && imageView) {
    return <ImageView url={source.url} title={source.title} />;
  }

  if (live) {
    return (
      <div className="min-h-0 flex-1 p-3">
        <div
          ref={liveRef}
          className="flex h-full w-full items-center justify-center rounded-md border border-border bg-surface-2/40 text-[12px] text-muted-foreground"
        >
          Loading live page…
        </div>
      </div>
    );
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

      <div className="relative min-h-0 flex-1">
        {richMode && content !== null && (
          <DocRails
            content={content}
            scrollerRef={bodyRef}
            relatedText={sectionText}
            excludeSourceId={source.id}
            width={paneWidth}
          />
        )}
      <div
        ref={bodyRef}
        className="relative h-full overflow-y-auto"
        onClickCapture={docLinkClickHandler(source.url || undefined)}
        onMouseOver={onBodyMouseOver}
        onScroll={() => setPreview(null)}
      >
        {preview && (
          <button
            type="button"
            className="menu-glass absolute z-10 flex w-64 flex-col gap-1 rounded-md border border-border-strong p-2.5 text-left shadow-lg"
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
            className="menu-glass absolute z-10 flex items-center gap-0.5 rounded-md border border-border-strong p-0.5 shadow-lg"
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
            <div className="flex flex-col gap-1.5 text-[13px] text-muted-foreground">
              <span>No text stored for this source.</span>
              {source.status === "error" && source.error && (
                <span className="text-[12px] text-destructive/80">
                  Import failed: {source.error}
                </span>
              )}
              {isWebUrl(source.url) && (
                <span className="text-[12px]">
                  The Live view (toolbar) shows the actual page.
                </span>
              )}
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

/** Notes in the reader: every kind uses its native renderer. Prose kinds
 *  are edit-in-place — the reading surface IS the editor (bare TipTap over
 *  the pane, reading-width column), autosaving on idle with the ambient
 *  rail floating alongside. No Save/Cancel. Artifact kinds (deck, quiz,
 *  flashcards, mind map, audio) keep native renderers plus the raw-markdown
 *  form behind the toolbar's Edit pencil. */
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

  // Entering artifact raw-edit snapshots the note; cancel discards.
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
  const artifact =
    note.kind === "slide_deck" ||
    note.kind === "mind_map" ||
    note.kind === "quiz" ||
    note.kind === "flashcards" ||
    note.kind === "audio_overview";
  // Generated reports are records of a moment — read-only in the reader,
  // with deliberate editing behind the toolbar pencil like artifacts.
  const readOnly = note.kind === "report";

  // Prose notes: the seamless always-editable surface (streaming rebuilds
  // still show the raw text flowing in).
  if (!artifact && !readOnly && !(rebuilding && artifactStreamText)) {
    return <InlineNote key={note.id} note={note} />;
  }

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
        <div className="min-h-0 min-w-0 flex-1">
          <RichEditor fill value={body} onChange={setBody} />
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

/** The seamless prose-note surface: bare editor, inline title, idle
 *  autosave, floating ambient rail. The document is the whole pane. */
function InlineNote({ note }: { note: Note }) {
  const reading = useStore((s) => s.reading);
  const rootRef = useRef<HTMLDivElement>(null);
  const width = useElementWidth(rootRef);
  const insertRef = useRef<((title: string, href: string) => void) | null>(null);
  const sources = useStore((s) => s.sources);
  const [title, setTitle] = useState(note.title);
  const [status, setStatus] = useState<"idle" | "dirty" | "saved">("idle");
  const prevBody = useRef(note.content);
  const [activePara, setActivePara] = useState("");
  const [counts, setCounts] = useState(note.content);
  // Latest values for the debounced save + unmount flush. `saved` is the
  // last-persisted snapshot: nothing writes unless content really moved.
  const pending = useRef({ title: note.title, body: note.content, dirty: false });
  const saved = useRef({ title: note.title, body: note.content });
  const mountedAt = useRef(Date.now());
  const touched = useRef(false);
  const timer = useRef<number | null>(null);

  const flush = () => {
    if (!pending.current.dirty) return;
    pending.current.dirty = false;
    if (
      pending.current.body === saved.current.body &&
      pending.current.title === saved.current.title
    ) {
      return;
    }
    saved.current = { title: pending.current.title, body: pending.current.body };
    void useStore
      .getState()
      .updateNote(note.id, pending.current.title, pending.current.body);
    setStatus("saved");
  };
  const flushRef = useRef(flush);
  flushRef.current = flush;

  const queueSave = () => {
    pending.current.dirty = true;
    setStatus("dirty");
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => flushRef.current(), 1200);
  };

  // Doc switch or leaving the reader saves whatever is pending.
  useEffect(() => {
    return () => {
      if (timer.current) window.clearTimeout(timer.current);
      flushRef.current();
    };
  }, []);

  // The editor's scroller is TipTap's own element; find it once mounted so
  // scroll memory and the TOC can drive it.
  const scrollerRef = useRef<HTMLElement | null>(null);
  const [scrollerReady, setScrollerReady] = useState(false);
  useEffect(() => {
    const find = () => {
      const el = rootRef.current?.querySelector<HTMLElement>(".ProseMirror");
      if (el) {
        scrollerRef.current = el;
        setScrollerReady(true);
        return true;
      }
      return false;
    };
    if (find()) return;
    const poll = window.setInterval(() => {
      if (find()) window.clearInterval(poll);
    }, 120);
    return () => window.clearInterval(poll);
  }, []);
  useScrollMemory(scrollerRef, `note:${note.id}`, scrollerReady, true);

  return (
    <div ref={rootRef} className="relative flex min-h-0 flex-1 flex-col">
      <div className="mx-auto w-full max-w-[760px] shrink-0 px-8 pt-6">
        <input
          value={title}
          aria-label="Note title"
          placeholder="Untitled"
          onChange={(e) => {
            setTitle(e.target.value);
            pending.current.title = e.target.value;
            queueSave();
          }}
          className="w-full bg-transparent text-[22px] font-semibold leading-snug text-foreground outline-none placeholder:text-subtle-foreground"
        />
      </div>
      <div
        className={cn("min-h-0 flex-1", chatReadingClass(reading))}
        // Plain click follows a link (in-corpus links jump in the reader);
        // ⌘/⌥-click places the cursor inside the link text for editing.
        onClickCapture={(e) => {
          if (e.metaKey || e.altKey || e.ctrlKey) return;
          const a = (e.target as HTMLElement).closest?.("a");
          if (!a) return;
          e.preventDefault();
          e.stopPropagation();
          routeDocLink(a.getAttribute("href") ?? "", undefined);
        }}
      >
        <RichEditor
          bare
          insertRef={insertRef}
          value={note.content}
          onChange={(next) => {
            // TipTap emits one markdown-normalization transaction right
            // after mount (its serialization differs slightly from the
            // stored text). That is not an edit: adopt it as the baseline
            // so merely opening a note never saves or bumps it.
            if (!touched.current && Date.now() - mountedAt.current < 400) {
              saved.current = { ...saved.current, body: next };
              prevBody.current = next;
              pending.current.body = next;
              return;
            }
            touched.current = true;
            setActivePara(activeParagraph(prevBody.current, next));
            prevBody.current = next;
            pending.current.body = next;
            setCounts(next);
            queueSave();
          }}
        />
      </div>
      <DocRails
        content={counts}
        scrollerRef={scrollerRef}
        relatedText={activePara}
        excludeNoteId={note.id}
        width={width}
        onInsert={(c) => {
          // Reference by the source's own URL/path so the editor's link
          // routing (wiki-jump) resolves it; notes use their deep link.
          const src = c.sourceId
            ? sources.find((x) => x.id === c.sourceId)
            : null;
          const href = src?.url || `alchemy://note/${c.noteId}`;
          insertRef.current?.(c.sourceTitle, href);
        }}
      />
      <div className="flex shrink-0 items-center gap-2 border-t border-border px-5 py-1.5 text-[11px] tabular-nums text-subtle-foreground">
        <span className="min-w-0 truncate whitespace-nowrap">{countsLine(counts)}</span>
        <span className="ml-auto shrink-0">
          {status === "dirty" ? "Editing…" : status === "saved" ? "Saved" : ""}
        </span>
      </div>
    </div>
  );
}
