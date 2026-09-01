import {
  Fragment,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import type { Citation, Note, Source, Template } from "@/lib/types";
import { AmbientRail } from "./AmbientRail";
import { CardMetaRow, CardRail } from "./RegistrySection";
import { activeParagraph } from "@/lib/utils";
import { AudioPlayer, DialogueScript } from "./AudioNote";
import { Flashcards } from "./Flashcards";
import { Infographic } from "./Infographic";
import { Markdown } from "./Markdown";
import { MindMap } from "./MindMap";
import { UmlDiagram } from "./UmlDiagram";
import { QuizView } from "./QuizView";
import { SlideDeck } from "./SlideDeck";
import { RichEditor } from "./RichEditor";
import { StreamingBody } from "./StudioNoteViewer";
import { KIND_LABEL } from "./studioArtifacts";
import { Favicon } from "./SourcesPanel";
import { useSourceActions } from "./SourceMenu";
import { sourceIcon } from "@/lib/sourceIcon";
import { Button, Input, RowMenu, Spinner, Textarea, useDelayedFlag } from "./ui";
import {
  chatReadingClass,
  cn,
  fmtDay,
  folderProvider,
  isWebUrl,
  scrollMemory,
  shortcutBlocked,
} from "@/lib/utils";
import {
  AppWindow,
  ArrowLeft,
  ArrowRight,
  Sprout,
  BookOpen,
  ChevronDown,
  ChevronUp,
  Copy,
  Download,
  ExternalLink,
  FileInput,
  FolderOpen,
  LayoutGrid,
  MessageSquare,
  MessageSquarePlus,
  Pencil,
  RefreshCw,
  Logs,
  Link2,
  ListTree,
  Search,
  SlidersHorizontal,
  Sparkles,
  ChevronRight,
  ChevronsDownUp,
  ChevronsUpDown,
  ListOrdered,
} from "lucide-react";

/**
 * The center-column reader — documents open here, in place, instead of in
 * modals. The sources/notes rails act as the navigator: clicking a row swaps
 * the document; back/forward is the app-level history (NavButtons, ⌘[ / ⌘]),
 * which every opened document joins; j/k steps through the rail order. Every note kind renders with its native renderer,
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
function locatePassage(
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

/** Drop the leading "> url · ref · date" provenance line git-cloned sources
 *  prepend for LLM context. The reader already shows Origin/Ref above, so it
 *  shouldn't appear as line 1 of the code view. Display only — the stored
 *  content keeps it. */
function stripLeadingProvenance(text: string): string {
  if (!text.startsWith("> ")) return text;
  const nl = text.indexOf("\n");
  return nl === -1 ? text : text.slice(nl + 1).replace(/^\n+/, "");
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
  // Title match: notes link to sources by name (the wiki index writes
  // `[Title](<Title>)`), and a bare title is also what a hand-typed
  // wikilink means. Exact, case-insensitive.
  let decoded = rawHref;
  try {
    decoded = decodeURIComponent(rawHref);
  } catch {
    /* stray % — compare raw */
  }
  const wanted = decoded.trim().toLowerCase();
  const byTitle = sources.find(
    (src) => src.title.trim().toLowerCase() === wanted,
  );
  if (byTitle) return byTitle;
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
    const direct = byKey(normalizePath(`${dir}/${rawHref}`));
    if (direct) return direct;
    // Obsidian shortest-path rule: a bare [[Note]] target resolves anywhere
    // in the vault, not just beside the linking file — fall back to a
    // filename-stem match across local sources.
    if (!rawHref.includes("/")) {
      const stem = rawHref.replace(/\.md$/i, "").toLowerCase();
      return (
        sources.find(
          (src) =>
            src.url.startsWith("/") &&
            (src.url.split("/").pop() ?? "")
              .replace(/\.[a-z0-9]{1,5}$/i, "")
              .toLowerCase() === stem,
        ) ?? null
      );
    }
    return null;
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
  // A note can be the target too — the wiki index and entity pages link
  // each other by title, the same way they link sources.
  let decodedNote = rawHref;
  try {
    decodedNote = decodeURIComponent(rawHref);
  } catch {
    /* stray % — compare raw */
  }
  const wantedNote = decodedNote.trim().toLowerCase();
  const noteHit = state.notes.find(
    (n) => n.title.trim().toLowerCase() === wantedNote,
  );
  if (noteHit) {
    state.openInReader({ type: "note", id: noteHit.id });
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
/** TreeWalker filter: skip document-metadata blocks (properties rows) so
 *  find and citation anchoring only ever match the content itself. */
const skipDocMeta: NodeFilter = {
  acceptNode: (n) =>
    (n as Text).parentElement?.closest("[data-doc-meta]")
      ? NodeFilter.FILTER_REJECT
      : NodeFilter.FILTER_ACCEPT,
};

function findTextRange(container: HTMLElement, needle: string): Range | null {
  const walker = document.createTreeWalker(
    container,
    NodeFilter.SHOW_TEXT,
    skipDocMeta,
  );
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
  const walker = document.createTreeWalker(
    container,
    NodeFilter.SHOW_TEXT,
    skipDocMeta,
  );
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

// Per-document scroll positions ride the persistent scrollMemory in
// lib/utils, so reading position survives relaunch, not just the session.

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
export function useElementWidth(ref: React.RefObject<HTMLElement | null>): number {
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
  const active = useStore((s) =>
    s.growOpen
      ? "grow"
      : s.galleryOpen
        ? "gallery"
        : s.ledgerOpen
          ? "ledger"
          : s.reader.open
            ? "reader"
            : "chat",
  );
  const tab = (
    id: "chat" | "reader" | "gallery" | "ledger" | "grow",
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
        "flex items-center gap-1.5 rounded-md px-2 py-1 text-caption font-medium transition-colors",
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
      {tab("chat", <MessageSquare className="h-3.5 w-3.5" />, "Chat", () => {
        useStore.setState({
          ledgerOpen: false,
          galleryOpen: false,
          growOpen: false,
        });
        s.closeReader();
      })}
      {tab(
        "reader",
        <BookOpen className="h-3.5 w-3.5" />,
        "Reader",
        () =>
          useStore.setState((st) => ({
            ledgerOpen: false,
            galleryOpen: false,
            growOpen: false,
            reader: { ...st.reader, open: true },
          })),
        !hasDocs,
      )}
      {tab("gallery", <LayoutGrid className="h-3.5 w-3.5" />, "Gallery", () =>
        useStore.setState({
          galleryOpen: true,
          ledgerOpen: false,
          growOpen: false,
        }),
      )}
      {tab("grow", <Sprout className="h-3.5 w-3.5" />, "Grow", () =>
        useStore.setState({
          growOpen: true,
          galleryOpen: false,
          ledgerOpen: false,
        }),
      )}
      {tab("ledger", <Logs className="h-3.5 w-3.5" />, "Ledger", () =>
        useStore.setState({
          ledgerOpen: true,
          galleryOpen: false,
          growOpen: false,
        }),
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
  // The source verbs and their modals, shared with every other surface.
  const actions = useSourceActions();
  const [refreshTick, setRefreshTick] = useState(0);
  const [editing, setEditing] = useState(false);
  const [liveMode, setLiveMode] = useState(false);
  const [imageMode, setImageMode] = useState(true);
  // PDFs open as text, not pages: the reader is where citations land, and
  // find/highlight/select-to-ask all live on the text view. Pages are one
  // click away for anyone who wants the original layout.
  const [pageMode, setPageMode] = useState(false);
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
  const templates = useStore((s) => s.templates);
  const template =
    current?.type === "template"
      ? (templates.find((t) => t.id === current.id) ?? null)
      : null;

  // Keyboard: Esc back to chat, j/k rail order. ⌘[ / ⌘] are the app-level
  // history's (App.tsx) — the reader's documents are entries in it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useStore.getState();
      if (e.key === "Escape" && !shortcutBlocked(e)) {
        e.preventDefault();
        s.closeReader();
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

  // Mirrors the Sources panel "Edit text" gate: extracted text the user may
  // rewrite. URL/mac mirrors and folder-like parents stay read-only.
  const sourceEditable =
    !!source &&
    source.sourceType !== "url" &&
    source.sourceType !== "mac" &&
    !["folder", "git", "notion", "obsidian"].includes(source.sourceType) &&
    source.status !== "placeholder";
  // The gallery's "Edit text" action opens the doc with edit intent set —
  // consume it once the matching source is in front.
  const editIntent = useStore((st) => st.readerEditIntent);
  useEffect(() => {
    if (!editIntent || !source || source.id !== editIntent) return;
    useStore.setState({ readerEditIntent: null });
    if (sourceEditable) setEditing(true);
  }, [editIntent, source, sourceEditable]);
  // "Ask about this source": scope the chat to this one document and land in
  // the composer. Placeholders have no chunks yet — nothing to ask about.
  const askAction =
    source && source.status === "ready"
      ? {
          label: "Ask about this source",
          icon: <MessageSquare className="h-3.5 w-3.5" />,
          onClick: () => useStore.getState().askAboutSource(source.id),
        }
      : null;
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
    : [askAction, originAction, refreshAction, popOutAction].filter(
        (a): a is NonNullable<typeof a> => a !== null,
      );
  // The shared source menu, minus whatever the roomy toolbar shows inline.
  // Refresh is always the reader's own (it re-renders the document after
  // the sync), slotted right after Ask where the shared list keeps it.
  const sourceItems = source
    ? actions.items(source, {
        omit: compact ? ["refresh"] : ["ask", "origin", "refresh"],
      })
    : [];
  if (compact && refreshAction) {
    sourceItems.splice(
      sourceItems[0]?.label === "Ask about this source" ? 1 : 0,
      0,
      refreshAction,
    );
  }
  const overflowItems = [
    ...(copyLinkAction ? [copyLinkAction] : []),
    ...sourceItems,
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
    <div ref={rootRef} className="relative flex h-full flex-1 flex-col min-w-0">
      {/* `group`: the toolbar RowMenu binds right-click to its nearest .group
          ancestor — without one, right-clicking the title bar did nothing. */}
      <div className="group relative z-10 flex h-12 shrink-0 items-center gap-0.5 border-b border-border px-3">
        {/* No back/forward here: every document opened lands in the
            app-level history, so the window's NavButtons (and ⌘[ / ⌘])
            already cover it. */}
        <div className="mx-1.5 flex min-w-0 flex-1 items-center gap-1.5">
          {source &&
            (isWebUrl(source.url) ? (
              <Favicon url={source.url} />
            ) : (
              sourceIcon(source.sourceType, source.url)
            ))}
          <span
            className="truncate text-body font-medium text-foreground"
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
                    "rounded-md px-2 py-0.5 text-micro font-medium capitalize transition-colors",
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
          {source && isPdfFile(source) && (
            <div className="mr-1 flex shrink-0 items-center gap-0.5 rounded-lg border border-border p-0.5">
              {(["text", "pages"] as const).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  onClick={() => setPageMode(mode === "pages")}
                  aria-pressed={pageMode === (mode === "pages")}
                  title={
                    mode === "pages"
                      ? "The original pages, as laid out"
                      : "The extracted text (searchable, citable)"
                  }
                  className={cn(
                    "rounded-md px-2 py-0.5 text-micro font-medium capitalize transition-colors",
                    pageMode === (mode === "pages")
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
                    "rounded-md px-2 py-0.5 text-micro font-medium capitalize transition-colors",
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
              "infographic",
              "mind_map",
              "uml",
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
          {sourceEditable && !editing && !liveMode && (
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setEditing(true)}
              title="Edit the source text (re-indexes on save)"
              aria-label="Edit source text"
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
        <div className="flex flex-1 items-center justify-center text-body text-muted-foreground">
          Open a source or note to read it here.
        </div>
      ) : source ? (
        editing && sourceEditable ? (
          <SourceEditor
            key={source.id}
            source={source}
            onDone={(saved) => {
              setEditing(false);
              if (saved) setRefreshTick((t) => t + 1);
            }}
          />
        ) : (
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
            pageView={pageMode}
          />
        )
      ) : note ? (
        <NoteReader
          key={note.id}
          note={note}
          editing={editing}
          onEditingChange={setEditing}
        />
      ) : template ? (
        <TemplateEditor key={template.id} template={template} />
      ) : (
        <div className="flex flex-1 items-center justify-center text-body text-muted-foreground">
          This document no longer exists — it may have been deleted.
        </div>
      )}
      {actions.modals}
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
      <div className="mb-1.5 text-badge font-medium uppercase tracking-wider text-subtle-foreground">
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
              // shrink-0: without it the flex column compresses entries into
              // each other once the list outgrows the rail, and a long TOC
              // renders as overlapping text.
              "shrink-0 truncate rounded px-1.5 py-0.5 text-left text-micro leading-relaxed transition-colors",
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
          {excludeSourceId && <CardRail sourceId={excludeSourceId} />}
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
          {excludeSourceId && <CardRail sourceId={excludeSourceId} />}
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

/** The original image behind an image source. The backend resolves the stored
 * source id so the renderer never needs broad asset-protocol filesystem scope. */
function ImageView({ sourceId, title }: { sourceId: string; title: string }) {
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let stale = false;
    setImageUrl(null);
    setFailed(false);
    api
      .sourceImage(sourceId)
      .then((url) => !stale && setImageUrl(url))
      .catch(() => {
        if (!stale) setFailed(true);
      });
    return () => {
      stale = true;
    };
  }, [sourceId]);
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center overflow-hidden p-6">
      {failed ? (
        <span className="text-body text-muted-foreground">
          The original file could not be read — it may have moved.
        </span>
      ) : imageUrl ? (
        <img
          src={imageUrl}
          alt={title}
          className="max-h-full max-w-full rounded-md border border-border object-contain shadow-sm"
        />
      ) : (
        <Spinner className="h-4 w-4" />
      )}
    </div>
  );
}

/** Can this source be shown as rendered pages? Only a PDF still backed by a
 *  file on disk — a PDF harvested from the web keeps its http URL and has no
 *  local bytes for PDFium to rasterize. */
/** Can this source be shown as pages? Any PDF with something behind it —
 *  a path, or a URL whose bytes the backend can fetch and cache. Web PDFs
 *  used to be excluded here, which quietly withheld page view from exactly
 *  the arxiv links v0.32.0 taught Alchemy to import. */
function isPdfFile(source: Source): boolean {
  return source.sourceType === "pdf" && !!source.url;
}

/** The PDF as pages, rendered by PDFium (RFC-document-surface phase 5).
 *  Pages rasterize on demand as they scroll into view — a 300-page document
 *  costs only the pages actually looked at — and each one holds its slot with
 *  a US-Letter-ratio placeholder so the scrollbar doesn't jump while they
 *  arrive. This view is deliberately not searchable; the text toggle is where
 *  find, citations, and select-to-ask live. */
function PdfPageView({
  sourceId,
  title,
}: {
  sourceId: string;
  title: string;
}) {
  /** Resolved local path. A file source is already local; a URL source is
   *  downloaded into the cache on first open (commands::pdf_local_path). */
  const [path, setPath] = useState("");
  const [count, setCount] = useState(0);
  const [failed, setFailed] = useState(false);
  const [pages, setPages] = useState<Record<number, string>>({});
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // One width for every page, measured once per resize: pages in a PDF share
  // a page size almost always, and re-rendering each on its own measurement
  // would thrash PDFium for no visible gain.
  const [width, setWidth] = useState(0);

  useEffect(() => {
    let stale = false;
    setPath("");
    setCount(0);
    setFailed(false);
    setPages({});
    api
      .pdfLocalPath(sourceId)
      .then((p) => !stale && setPath(p))
      .catch(() => !stale && setFailed(true));
    return () => {
      stale = true;
    };
  }, [sourceId]);

  useEffect(() => {
    if (!path) return;
    let stale = false;
    api
      .pdfPageCount(path)
      .then((n) => {
        if (stale) return;
        setCount(n);
        if (n === 0) setFailed(true);
      })
      .catch(() => !stale && setFailed(true));
    return () => {
      stale = true;
    };
  }, [path]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = () =>
      setWidth(Math.round(Math.min(el.clientWidth - 48, 1100)));
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [count]);

  // Fetch a page's bitmap the first time its placeholder intersects the
  // viewport. Rendered pages are keyed by page number and never evicted —
  // scrolling back up is instant, and a PNG per page is small next to the
  // document already in memory.
  const observe = useCallback(
    (node: HTMLDivElement | null, page: number) => {
      if (!node || !width || pages[page]) return;
      const io = new IntersectionObserver(
        (entries) => {
          if (!entries.some((e) => e.isIntersecting)) return;
          io.disconnect();
          api
            .pdfPageImage(path, page, width)
            .then((url) => setPages((prev) => ({ ...prev, [page]: url })))
            .catch(() => {
              /* one unreadable page shouldn't blank the whole document */
            });
        },
        { rootMargin: "600px" },
      );
      io.observe(node);
    },
    [path, width, pages],
  );

  if (failed) {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center p-6">
        <span className="text-body text-muted-foreground">
          The pages could not be rendered — the file may have moved.
        </span>
      </div>
    );
  }

  return (
    <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto p-6">
      <div className="mx-auto flex flex-col items-center gap-6">
        {Array.from({ length: count }, (_, i) => i + 1).map((page) => (
          <div
            key={page}
            ref={(node) => observe(node, page)}
            className="w-full"
            style={{ maxWidth: width || undefined }}
          >
            {pages[page] ? (
              <img
                src={pages[page]}
                alt={`${title} — page ${page}`}
                className="w-full rounded-md border border-border shadow-sm"
              />
            ) : (
              <div
                className="flex w-full items-center justify-center rounded-md border border-border bg-surface-2/40"
                style={{ aspectRatio: "8.5 / 11" }}
              >
                <Spinner className="h-4 w-4" />
              </div>
            )}
            <div className="pt-1.5 text-center text-micro text-subtle-foreground">
              {page} / {count}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

/** Full-text source reading: faithful markdown when the content is markdown-
 *  shaped, find-in-source, citation highlight, and select-to-ask. */
export const SOURCE_TYPE_LABEL: Record<Source["sourceType"], string> = {
  pdf: "PDF",
  text: "Text",
  markdown: "Markdown",
  html: "HTML",
  url: "Web page",
  image: "Image",
  folder: "Folder",
  mac: "Mac app",
  code: "Code",
  git: "Git repository",
  notion: "Notion pages",
  obsidian: "Obsidian vault",
};

/** Git provenance parsed from the content header line the ingesters write
 *  (`> origin · branch @ sha · date`) — provenance is content, not schema,
 *  so the properties block re-reads it rather than growing columns. */
export function parseGitProvenance(
  text: string | null | undefined,
): { origin: string; ref: string; sha: string; date?: string } | null {
  if (!text) return null;
  for (const line of text.split("\n", 6)) {
    const m = line.match(
      /^> (.+?) · (.+?) @ ([\w-]+)(?: · (\d{4}-\d{2}-\d{2}))?$/,
    );
    if (m) return { origin: m[1], ref: m[2], sha: m[3], date: m[4] };
  }
  return null;
}

/** Linear-style properties block at the top of a document: quiet
 *  label/value rows that answer "what is this" before the content. */
/** Custom-generator editor: name, description, and the generation prompt of
 *  one ~/Documents/Alchemy/templates/*.md file, saved back in place. Plain
 *  controlled inputs — a template is an instruction, not a document, so it
 *  gets a form rather than the rich note editor. */
function TemplateEditor({ template }: { template: Template }) {
  const refreshTemplates = useStore((s) => s.refreshTemplates);
  const [name, setName] = useState(template.name);
  const [description, setDescription] = useState(template.description);
  const [prompt, setPrompt] = useState(template.prompt);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const dirty =
    name !== template.name ||
    description !== template.description ||
    prompt !== template.prompt;

  async function save() {
    if (saving || !name.trim() || !prompt.trim()) return;
    setSaving(true);
    try {
      await api.saveTemplate(template.id, name, description, prompt);
      await refreshTemplates();
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      useStore.getState().pushToast("error", e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    // A hand-written generator prompt is unrecoverable once its file is gone,
    // and this view closes with the delete — snapshot the saved state so the
    // toast can restore it (save_template with the same id rewrites the file).
    const gone = {
      id: template.id,
      name: template.name,
      description: template.description,
      prompt: template.prompt,
    };
    try {
      await api.deleteTemplate(gone.id);
      await refreshTemplates();
      const s = useStore.getState();
      s.closeReader();
      s.pushToast("success", `Deleted “${gone.name}” — click to undo`, () => {
        void (async () => {
          try {
            await api.saveTemplate(gone.id, gone.name, gone.description, gone.prompt);
            await useStore.getState().refreshTemplates();
          } catch (e) {
            useStore
              .getState()
              .pushToast("error", e instanceof Error ? e.message : String(e));
          }
        })();
      });
    } catch (e) {
      useStore.getState().pushToast("error", e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="mx-auto flex max-w-[720px] flex-col gap-4 px-8 py-8">
        <div className="flex flex-col gap-1">
          <span className="text-micro font-semibold uppercase tracking-wide text-subtle-foreground">
            Custom generator
          </span>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Template name"
            aria-label="Template name"
          />
        </div>
        <Input
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="One-line description (shown on the Studio tile)"
          aria-label="Template description"
        />
        <div className="flex min-h-0 flex-col gap-1">
          <span className="text-micro font-semibold uppercase tracking-wide text-subtle-foreground">
            Generation prompt
          </span>
          <Textarea
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            rows={14}
            placeholder="What should this generator produce from the notebook's sources?"
            aria-label="Generation prompt"
            className="font-mono leading-relaxed"
          />
          <span className="text-caption text-subtle-foreground">
            Runs over the notebook's sources like any generator. Saved to
            ~/Documents/Alchemy/templates/{template.id}.md.
          </span>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="primary"
            size="sm"
            onClick={() => void save()}
            disabled={saving || !dirty || !name.trim() || !prompt.trim()}
          >
            {saved ? "Saved" : saving ? "Saving…" : "Save"}
          </Button>
          <span className="flex-1" />
          <Button variant="danger" size="sm" onClick={() => void remove()}>
            Delete template
          </Button>
        </div>
      </div>
    </div>
  );
}

/** A properties row the user can edit in place: click the value, type,
 *  Enter or blur saves, Escape cancels. Lives inside DocProperties' grid. */
function MetaEditable({
  label,
  raw,
  display,
  placeholder,
  onSave,
}: {
  label: string;
  raw: string;
  display: string;
  placeholder: string;
  onSave: (value: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(raw);
  useEffect(() => setValue(raw), [raw]);
  const commit = () => {
    setEditing(false);
    if (value !== raw) onSave(value);
  };
  return (
    <>
      <span className="pt-px text-subtle-foreground">{label}</span>
      {editing ? (
        <input
          autoFocus
          aria-label={label}
          value={value}
          onChange={(event) => setValue(event.target.value)}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commit();
            } else if (event.key === "Escape") {
              setValue(raw);
              setEditing(false);
            }
          }}
          className="min-w-0 rounded-sm border border-input bg-transparent px-1 text-caption text-foreground outline-none focus:border-ring"
        />
      ) : (
        <button
          type="button"
          onClick={() => setEditing(true)}
          title={display || placeholder}
          className={cn(
            "min-w-0 truncate text-left",
            display
              ? "text-muted-foreground hover:text-foreground"
              : "italic text-subtle-foreground hover:text-muted-foreground",
          )}
        >
          {display || placeholder}
        </button>
      )}
    </>
  );
}

function DocProperties({
  source,
  note,
  git,
}: {
  source?: Source;
  note?: Note;
  git?: ReturnType<typeof parseGitProvenance>;
}) {
  const rows: { label: string; value: string; href?: string }[] = [];
  if (source) {
    // A cloud-synced folder says which service it came from — "Folder" alone
    // can't distinguish a Box mount from a plain local directory.
    const provider = isWebUrl(source.url) ? null : folderProvider(source.url);
    rows.push({
      label: "Type",
      value: provider
        ? `${SOURCE_TYPE_LABEL[source.sourceType]} · ${provider}`
        : SOURCE_TYPE_LABEL[source.sourceType],
    });
    // The actual URL, not just its host — the address is the provenance,
    // and clicking it opens the page in the browser.
    if (isWebUrl(source.url))
      rows.push({ label: "URL", value: source.url, href: source.url });
    // Embedded document authorship (PDF /Author, Office dc:creator, EXIF
    // Artist) — present only when the file actually carries it.
    if (source.author) rows.push({ label: "Author", value: source.author });
    // The on-disk path, so a human can find the original and an agent reading
    // the properties block gets the same handle Show in Finder uses.
    if (source.url && !isWebUrl(source.url) && source.sourceType !== "mac")
      rows.push({ label: "Path", value: source.url });
    if (git) {
      if (git.origin !== "local repository")
        rows.push({ label: "Origin", value: git.origin });
      rows.push({
        label: "Ref",
        value: `${git.ref} @ ${git.sha}${git.date ? ` · ${git.date}` : ""}`,
      });
    }
    rows.push({ label: "Added", value: fmtDay(source.createdAt) });
    rows.push({
      label: "Size",
      value: `${source.charCount.toLocaleString()} chars · ${source.chunkCount} chunks`,
    });
  } else if (note) {
    rows.push({ label: "Type", value: KIND_LABEL[note.kind] ?? "Note" });
    if (note.origin === "auto") rows.push({ label: "Origin", value: "From chat" });
    rows.push({ label: "Created", value: fmtDay(note.createdAt) });
    if (fmtDay(note.updatedAt) !== fmtDay(note.createdAt))
      rows.push({ label: "Updated", value: fmtDay(note.updatedAt) });
  }
  if (rows.length === 0 && !source) return null;
  return (
    // data-doc-meta: excluded from find-in-source and citation anchoring —
    // matching "example.com" against the Site row would be noise.
    <div data-doc-meta className="mb-6 border-b border-border pb-4">
      <div className="grid w-fit max-w-full grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-caption">
        {rows.map((r) => (
          <Fragment key={r.label}>
            <span className="text-subtle-foreground">{r.label}</span>
            {r.href ? (
              <button
                type="button"
                onClick={() => void openUrl(r.href!)}
                className="min-w-0 truncate text-left text-citation hover:underline"
                title={`Open ${r.value}`}
              >
                {r.value}
              </button>
            ) : (
              <span className="min-w-0 truncate text-muted-foreground" title={r.value}>
                {r.value}
              </span>
            )}
          </Fragment>
        ))}
        {/* User metadata (RFC-source-tags): always present for sources —
            click to edit in place, so the empty state teaches the feature. */}
        {source && (
          <>
            <CardMetaRow sourceId={source.id} />
            <MetaEditable
              label="Tags"
              raw={source.tags}
              display={source.tags
                .split(" ")
                .filter(Boolean)
                .map((t) => `#${t}`)
                .join(" ")}
              placeholder="Add tags…"
              onSave={(v) => void useStore.getState().setSourceTags(source.id, v)}
            />
            <MetaEditable
              label="Note"
              raw={source.note}
              display={source.note}
              placeholder="Add a note…"
              onSave={(v) => void useStore.getState().setSourceNote(source.id, v)}
            />
          </>
        )}
      </div>
    </div>
  );
}

/** One open reminder parsed out of a Reminders source body. The id rides the
 *  text as a trailing code span (mac.rs carries it for exactly this); rows
 *  without one render inert — there is no way to name them to cider. */
type ReminderRow = {
  title: string;
  id: string | null;
  due: string | null;
  notes: string | null;
};

function parseReminders(text: string): { heading: string; items: ReminderRow[] } {
  const items: ReminderRow[] = [];
  let heading = "";
  for (const line of text.split("\n")) {
    const h = /^# (.*)$/.exec(line);
    if (h) {
      heading = h[1];
      continue;
    }
    const m = /^- \[ \] (.*)$/.exec(line);
    if (m) {
      let rest = m[1];
      let due: string | null = null;
      const dm = / — due (\d{4}-\d{2}-\d{2})$/.exec(rest);
      if (dm) {
        due = dm[1];
        rest = rest.slice(0, -dm[0].length);
      }
      let id: string | null = null;
      const im = / `([^`]+)`$/.exec(rest);
      if (im) {
        id = im[1];
        rest = rest.slice(0, -im[0].length);
      }
      items.push({ title: rest, id, due, notes: null });
      continue;
    }
    const n = /^ {2}- (.*)$/.exec(line);
    if (n && items.length > 0) items[items.length - 1].notes = n[1];
  }
  return { heading, items };
}

/** A Reminders-list source rendered live: each reminder is a real checkbox
 *  wired to complete_mac_reminder — the same call agents already have — not
 *  inert markdown. Checking one completes it in Apple Reminders and resyncs
 *  the source; the row holds its checked state until the refetched body
 *  (open reminders only) drops it. */
function RemindersView({
  content,
  sourceId,
  onCompleted,
}: {
  content: string;
  sourceId: string;
  onCompleted: () => void;
}) {
  const parsed = useMemo(() => parseReminders(content), [content]);
  const [busy, setBusy] = useState<string | null>(null);
  const [done, setDone] = useState<Set<string>>(new Set());

  async function complete(row: ReminderRow) {
    if (!row.id || busy) return;
    setBusy(row.id);
    try {
      await api.completeMacReminder(sourceId, row.id);
      setDone((d) => new Set(d).add(row.id!));
      onCompleted();
    } catch (err) {
      useStore.getState().pushToast("error", String(err));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="selectable">
      {parsed.heading && (
        <h1 className="mb-4 text-title font-semibold text-foreground">
          {parsed.heading}
        </h1>
      )}
      <ul className="flex flex-col gap-1.5">
        {parsed.items.map((row, i) => {
          const checked = !!row.id && done.has(row.id);
          return (
            <li key={row.id ?? `row-${i}`} className="flex items-start gap-2.5">
              <input
                type="checkbox"
                className="select-quiet mt-1"
                checked={checked}
                disabled={!row.id || checked || busy === row.id}
                onChange={() => void complete(row)}
                aria-label={`Complete “${row.title}”`}
                title={row.id ? "Check off in Apple Reminders" : undefined}
              />
              <div className="min-w-0">
                <div
                  className={cn(
                    "text-body leading-relaxed text-foreground/90",
                    checked && "text-muted-foreground line-through",
                  )}
                >
                  {row.title}
                  {row.due && (
                    <span className="ml-2 text-caption text-subtle-foreground">
                      due {row.due}
                    </span>
                  )}
                </div>
                {row.notes && (
                  <div className="text-caption text-muted-foreground">
                    {row.notes}
                  </div>
                )}
              </div>
            </li>
          );
        })}
        {parsed.items.length === 0 && (
          <li className="text-body text-muted-foreground">
            Nothing left on this list.
          </li>
        )}
      </ul>
    </div>
  );
}

function SourceReader({
  source,
  highlight,
  findOpen,
  onFindOpen,
  onFindClose,
  refreshTick,
  live,
  imageView = false,
  pageView = false,
}: {
  source: Source;
  highlight?: string;
  findOpen: boolean;
  onFindOpen: () => void;
  onFindClose: () => void;
  refreshTick: number;
  live: boolean;
  imageView?: boolean;
  pageView?: boolean;
}) {
  const sendMessage = useStore((s) => s.sendMessage);
  const sending = useStore((s) => s.sending);
  const reading = useStore((s) => s.reading);
  const refreshSource = useStore((s) => s.refreshSource);
  // Placeholder hydration: the queued refresh downloads + embeds, then the
  // bumped tick re-runs the content loader above.
  const [hydrating, setHydrating] = useState(false);
  const [hydrateTick, setHydrateTick] = useState(0);
  const [content, setContent] = useState<string | null>(null);
  // Most sources land in well under 250ms — flashing a spinner for that
  // blink is distracting. Only slow loads (PDFs) earn the indicator.
  const showLoading = useDelayedFlag(content === null);
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
  // overlays (palette, modals, hover cards, row menus) hide it so they are
  // never painted over — a native child webview sits above every HTML
  // layer, so z-index cannot win this fight.
  const liveRef = useRef<HTMLDivElement>(null);
  // Where the live view actually is — polled, since navigation happens
  // inside the native child where no DOM event reaches us. Drives the
  // toolbar's address line and its "Add as source" offer.
  const [liveUrl, setLiveUrl] = useState<string | null>(null);
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
      void api.liveViewVisible(
        !document.querySelector('[role="dialog"],[data-overlay]'),
      );
    const mo = new MutationObserver(overlayCheck);
    mo.observe(document.body, { childList: true, subtree: true });
    const poll = window.setInterval(() => {
      api
        .liveViewUrl()
        .then((u) => setLiveUrl(u))
        .catch(() => undefined);
    }, 1000);
    return () => {
      ro.disconnect();
      mo.disconnect();
      window.removeEventListener("resize", update);
      window.clearInterval(poll);
      setLiveUrl(null);
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
        if (!stale)
          setContent(
            source.sourceType === "code" ? stripLeadingProvenance(text) : text,
          );
      })
      .catch(() => {
        if (!stale) setContent("");
      });
    return () => {
      stale = true;
    };
  }, [source.id, refreshTick, hydrateTick]);

  // Cmd/Ctrl+F opens the find bar and focuses it (Safari-style: the bar
  // exists only while finding).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Not while a modal owns the keyboard or the user is typing in a
      // field — find used to open behind dialogs and steal focus mid-word.
      if (shortcutBlocked(e)) return;
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

  // Edit > Find (menu.rs) lands here too — the menu can't carry the ⌘F
  // accelerator (it would override focused text fields), so it bumps
  // findBump and whichever find surface is mounted answers.
  const findBump = useStore((s) => s.findBump);
  useEffect(() => {
    if (findBump === 0) return;
    onFindOpen();
    requestAnimationFrame(() => {
      searchRef.current?.focus();
      searchRef.current?.select();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [findBump]);

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
  // Memoized on content: the regex sniff and the word count both walk the
  // whole document, and this component re-renders per find keystroke and
  // selection change.
  const contentLooksMarkdown = useMemo(
    () => !!content && looksLikeMarkdown(content),
    [content],
  );
  const statsLine = useMemo(
    () => (content ? countsLine(content) : ""),
    [content],
  );
  const markdownShaped =
    source.sourceType === "markdown" ||
    ((source.sourceType === "text" ||
      source.sourceType === "url" ||
      // Apple integrations (Notes, Calendar, Reminders, Stocks) come out of
      // mac.rs as Markdown — Stocks is a GFM table, and reading it as flat
      // text showed the pipes instead of a table. Reminders keeps its own
      // checkbox view, which is checked before this one.
      source.sourceType === "mac" ||
      // PDFs extract to Markdown now (pdf-inspector reconstructs headings,
      // lists and tables) — but older sources were ingested as flat text, so
      // this still asks the content rather than trusting the type.
      source.sourceType === "pdf") &&
      contentLooksMarkdown);
  const richMode = markdownShaped && !(highlight && anchorFailed);
  // Code-file sources render with the same shiki view the repo reader uses
  // (CodeView). Shiki swaps the code block's DOM in asynchronously, which
  // would detach a citation Range anchored before the swap — so when a
  // citation highlight is active we fall back to the exact plain-text view
  // (verbatim, synchronously highlighted, like before). The normal open gets
  // syntax colors; find-in-source over the colored view re-anchors on the
  // rendered DOM each keystroke.
  const codeMode = source.sourceType === "code" && !highlight;
  // A Reminders list renders as live checkboxes (complete-in-place) instead
  // of inert markdown — except during find or a citation anchor, which need
  // the text views' highlight machinery.
  const remindersMode =
    source.sourceType === "mac" &&
    source.url.startsWith("cider://reminders/list/") &&
    !query.trim() &&
    !highlight;
  // "DOM-rendered": find walks text nodes instead of painting the plain
  // <mark> segments. True for both the markdown and code (shiki) views.
  const domMode = richMode || codeMode;

  // Find-in-source on the RENDERED view: all matches get ::highlight(find),
  // the active one ::highlight(find-active) and a scroll-to. The plain
  // segment view keeps its own <mark> path for non-markdown sources.
  const [domMatchCount, setDomMatchCount] = useState(0);
  const domRanges = useRef<Range[]>([]);
  useEffect(() => {
    if (!domMode || content === null) return;
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
  }, [domMode, content, query]);
  // Stepping through matches: retarget the active highlight and scroll.
  useEffect(() => {
    if (!domMode || domRanges.current.length === 0) return;
    const ranges = domRanges.current;
    const idx = ((active % ranges.length) + ranges.length) % ranges.length;
    applyFindHighlights(ranges, idx);
    const rect = ranges[idx].getBoundingClientRect();
    const body = bodyRef.current?.getBoundingClientRect();
    if (body && bodyRef.current) {
      bodyRef.current.scrollTop += rect.top - body.top - bodyRef.current.clientHeight / 3;
    }
  }, [active, domMode]);

  const matchTotal = domMode ? domMatchCount : matches.length;
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
    if (!domMode || !highlight || content === null) return;
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
  }, [domMode, highlight, content]);
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
  // selectionchange rides along, debounced, for keyboard selection —
  // shift+arrows never fires a mouseup.
  const updateSelectionRef = useRef<() => void>(() => {});
  useEffect(() => {
    const onUp = () => updateSelectionRef.current();
    let timer: number | null = null;
    const onChange = () => {
      if (timer) window.clearTimeout(timer);
      timer = window.setTimeout(() => updateSelectionRef.current(), 200);
    };
    window.addEventListener("mouseup", onUp);
    document.addEventListener("selectionchange", onChange);
    return () => {
      if (timer) window.clearTimeout(timer);
      window.removeEventListener("mouseup", onUp);
      document.removeEventListener("selectionchange", onChange);
    };
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
  if (content && !domMode) {
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
    return <ImageView sourceId={source.id} title={source.title} />;
  }

  if (isPdfFile(source) && pageView) {
    return <PdfPageView sourceId={source.id} title={source.title} />;
  }

  if (live) {
    // "Same page" tolerates the fragment and a trailing slash; a changed
    // path or query is a different page and earns the Add offer.
    const normPage = (u: string) => u.split("#")[0].replace(/\/$/, "");
    const wandered = !!liveUrl && normPage(liveUrl) !== normPage(source.url);
    return (
      <div className="flex min-h-0 flex-1 flex-col p-3">
        {/* Nav chrome: the page's own context menu is suppressed (this is
            a convenience surface, not a browser), so back/forward live
            here — and a page the user wandered to can join the notebook. */}
        <div className="mb-2 flex min-w-0 items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => void api.liveViewBack()}
            title="Back"
            aria-label="Live view back"
          >
            <ArrowLeft className="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => void api.liveViewForward()}
            title="Forward"
            aria-label="Live view forward"
          >
            <ArrowRight className="h-3.5 w-3.5" />
          </Button>
          <span
            className="min-w-0 flex-1 truncate text-micro text-subtle-foreground"
            title={liveUrl ?? source.url}
          >
            {liveUrl ?? source.url}
          </span>
          {wandered && (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                if (liveUrl) void useStore.getState().addSourceUrl(liveUrl);
              }}
              title="Add the page you're viewing to this notebook"
            >
              Add as source
            </Button>
          )}
        </div>
        <div
          ref={liveRef}
          className="flex min-h-0 flex-1 items-center justify-center rounded-md border border-border bg-surface-2/40 text-caption text-muted-foreground"
        >
          Loading live page…
        </div>
      </div>
    );
  }

  // Folder, git, and Notion parents open as the repo reader (RFC-git-sources
  // §7): file tree + file pane instead of the flat map text. All hooks above
  // have run, so the early return is safe.
  if (["folder", "git", "notion", "obsidian"].includes(source.sourceType)) {
    return <RepoView source={source} map={content} />;
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
              className="h-7 w-56 pl-7 text-caption"
            />
          </div>
          <span className="min-w-8 text-right text-micro tabular-nums text-subtle-foreground">
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
            <span className="flex items-center gap-1.5 text-caption font-medium text-foreground">
              {sourceIcon(preview.source.sourceType, preview.source.url)}
              <span className="truncate">{preview.source.title}</span>
            </span>
            <span className="text-micro text-subtle-foreground">
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
              icon={<Logs className="h-3.5 w-3.5" />}
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
        <div className={cn("mx-auto max-w-[760px] px-14 py-6", chatReadingClass(reading))}>
          {!!content && <ParentJump source={source} />}
          {!!content && (
            <DocProperties source={source} git={parseGitProvenance(content)} />
          )}
          {content === null ? (
            showLoading && (
              <div className="flex items-center gap-2 text-body text-muted-foreground">
                <Spinner className="h-3.5 w-3.5" /> Loading source…
              </div>
            )
          ) : content === "" ? (
            <div className="flex flex-col gap-1.5 text-body text-muted-foreground">
              {source.status === "placeholder" ? (
                <>
                  <span>
                    This file is online-only in{" "}
                    {folderProvider(source.url) ?? "its cloud drive"} and
                    hasn't been downloaded to this Mac yet.
                  </span>
                  {/* Same queued refresh the Sources row uses: hydrates
                      (brctl for iCloud, the read itself for File Provider
                      mounts), extracts, and embeds. */}
                  <Button
                    variant="secondary"
                    size="sm"
                    className="mt-1.5 self-start"
                    disabled={hydrating}
                    onClick={() => {
                      if (hydrating) return;
                      setHydrating(true);
                      void refreshSource(source.id)
                        .catch(() => undefined)
                        .finally(() => {
                          setHydrating(false);
                          setHydrateTick((t) => t + 1);
                        });
                    }}
                  >
                    {hydrating ? (
                      <Spinner className="h-3.5 w-3.5" />
                    ) : (
                      <Download className="h-3.5 w-3.5" />
                    )}
                    {hydrating ? "Downloading…" : "Download & embed"}
                  </Button>
                </>
              ) : (
                <span>No text stored for this source.</span>
              )}
              {source.status === "error" && source.error && (
                <span className="text-caption text-destructive/80 [overflow-wrap:anywhere]">
                  Import failed: {source.error}
                </span>
              )}
              {isWebUrl(source.url) && (
                <span className="text-caption">
                  The Live view (toolbar) shows the actual page.
                </span>
              )}
            </div>
          ) : remindersMode ? (
            <RemindersView
              content={content}
              sourceId={source.id}
              onCompleted={() => setHydrateTick((t) => t + 1)}
            />
          ) : codeMode ? (
            <CodeView path={source.title} code={content} lineNums />
          ) : richMode ? (
            <div className="selectable">
              <Markdown wikilinks>{content}</Markdown>
            </div>
          ) : (
            <p className="reader-plain whitespace-pre-wrap text-body leading-relaxed text-foreground/90 selectable">
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
        <div className="flex shrink-0 items-center gap-2 border-t border-border px-5 py-1 text-micro tabular-nums text-subtle-foreground">
          <span className="min-w-0 truncate whitespace-nowrap">
            {source.chunkCount} chunks · {statsLine}
          </span>
          {backlinks.length > 0 && (
            <span className="group ml-auto flex shrink-0 items-center">
              <RowMenu
                // The text link IS the trigger: a hidden ⋯ button clicked
                // from outside measures 0x0, which parks the dropdown in the
                // window's top-left corner instead of beside the link.
                alwaysVisible
                className="!flex"
                triggerClassName="text-citation hover:underline"
                trigger={<>← linked from {backlinks.length}</>}
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
        "flex items-center gap-1.5 whitespace-nowrap rounded px-2 py-1 text-caption text-foreground/90",
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
/** Raw-text editor for a source's extracted text, behind the toolbar
 *  pencil. Saving re-chunks and re-embeds through the ingest queue. */
function SourceEditor({
  source,
  onDone,
}: {
  source: Source;
  onDone: (saved: boolean) => void;
}) {
  const editSourceText = useStore((s) => s.editSourceText);
  const setSourceTags = useStore((s) => s.setSourceTags);
  const setSourceNote = useStore((s) => s.setSourceNote);
  const [title, setTitle] = useState(source.title);
  const [tags, setTags] = useState(source.tags);
  const [note, setNote] = useState(source.note);
  const [text, setText] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // List payloads omit content; fetch the full text to prefill the editor.
  useEffect(() => {
    let stale = false;
    api
      .getSourceContent(source.id)
      .then((t) => {
        if (!stale) setText(t);
      })
      .catch(() => {
        if (!stale) setText("");
      });
    return () => {
      stale = true;
    };
  }, [source.id]);

  if (text === null) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Spinner />
      </div>
    );
  }
  return (
    <form
      className="flex min-h-0 flex-1 flex-col gap-3 px-6 py-4"
      onSubmit={(event) => {
        event.preventDefault();
        if (saving) return;
        setSaving(true);
        // Tags/note ride the same Save: cheap column updates first (they
        // don't re-index the body), then the text edit which re-embeds.
        const meta: Promise<unknown>[] = [];
        if (tags !== source.tags) meta.push(setSourceTags(source.id, tags));
        if (note !== source.note) meta.push(setSourceNote(source.id, note));
        void Promise.all(meta)
          .then(() => editSourceText(source.id, title, text))
          .then(() => onDone(true))
          .finally(() => setSaving(false));
      }}
    >
      <Input
        name="source-title"
        aria-label="Source title"
        value={title}
        onChange={(event) => setTitle(event.target.value)}
      />
      <div className="flex gap-3">
        <Input
          name="source-tags"
          aria-label="Source tags"
          placeholder="Tags — space-separated, e.g. energy q3"
          className="flex-1"
          value={tags}
          onChange={(event) => setTags(event.target.value)}
        />
        <Input
          name="source-note"
          aria-label="Source note"
          placeholder="Your note — why this source matters"
          className="flex-[2]"
          value={note}
          onChange={(event) => setNote(event.target.value)}
        />
      </div>
      <Textarea
        aria-label="Source text"
        className="min-h-0 flex-1 resize-none font-mono text-caption leading-relaxed"
        value={text}
        onChange={(event) => setText(event.target.value)}
      />
      <div className="flex shrink-0 items-center justify-end gap-2">
        <span className="mr-auto text-caption text-subtle-foreground">
          Saving re-indexes this source.
        </span>
        <Button type="button" variant="ghost" onClick={() => onDone(false)}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" loading={saving}>
          Save
        </Button>
      </div>
    </form>
  );
}

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
  const fillsPane =
    note.kind === "slide_deck" || note.kind === "mind_map" || note.kind === "uml";
  const artifact =
    note.kind === "slide_deck" ||
    note.kind === "infographic" ||
    note.kind === "mind_map" ||
    note.kind === "uml" ||
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
          fillsPane ? "overflow-hidden px-6 py-4" : "overflow-y-auto px-14 py-6",
        )}
      >
        <div className={cn("mx-auto h-full", fillsPane ? "max-w-none" : "max-w-[760px]")}>
          {!fillsPane && <DocProperties note={note} />}
          {rebuilding && artifactStreamText ? (
            <StreamingBody text={artifactStreamText} />
          ) : note.kind === "mind_map" ? (
            <MindMap content={note.content} />
          ) : note.kind === "uml" ? (
            <UmlDiagram content={note.content} />
          ) : note.kind === "flashcards" ? (
            <Flashcards content={note.content} noteId={note.id} />
          ) : note.kind === "quiz" ? (
            <QuizView content={note.content} />
          ) : note.kind === "slide_deck" ? (
            <SlideDeck content={note.content} note={note} />
          ) : note.kind === "infographic" ? (
            <Infographic content={note.content} title={note.title} />
          ) : note.kind === "audio_overview" ? (
            <div className="flex flex-col gap-4">
              <AudioPlayer key={note.updatedAt} noteId={note.id} title={note.title} />
              <DialogueScript content={note.content} />
            </div>
          ) : (
            <div
              className={cn(
                chatReadingClass(reading),
                note.kind === "report" && "prose-compact",
              )}
              onClickCapture={docLinkClickHandler(undefined)}
            >
              {/* Briefs carry an audio edition; the player self-hides for
                  every report note without one. */}
              {note.kind === "report" && (
                <AudioPlayer noteId={note.id} title={note.title} />
              )}
              <Markdown>{note.content}</Markdown>
            </div>
          )}
        </div>
      </div>
      {!fillsPane && (
        <div className="shrink-0 overflow-hidden truncate whitespace-nowrap border-t border-border px-5 py-1.5 text-micro tabular-nums text-subtle-foreground">
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

  // Doc switch or leaving the reader saves whatever is pending — and so
  // does quitting: pagehide is the last synchronous moment the webview
  // gives us, and a 1200ms debounce otherwise loses the final keystrokes.
  useEffect(() => {
    const onPageHide = () => flushRef.current();
    window.addEventListener("pagehide", onPageHide);
    return () => {
      window.removeEventListener("pagehide", onPageHide);
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
      <div className="mx-auto w-full max-w-[760px] shrink-0 px-14 pt-6">
        <input
          value={title}
          aria-label="Note title"
          placeholder="Untitled"
          onChange={(e) => {
            setTitle(e.target.value);
            pending.current.title = e.target.value;
            queueSave();
          }}
          className="w-full bg-transparent text-page font-semibold leading-snug text-foreground outline-none placeholder:text-subtle-foreground"
        />
        <div className="mt-4">
          <DocProperties note={note} />
        </div>
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
      <div className="flex shrink-0 items-center gap-2 border-t border-border px-5 py-1.5 text-micro tabular-nums text-subtle-foreground">
        <span className="min-w-0 truncate whitespace-nowrap">{countsLine(counts)}</span>
        <span className="ml-auto shrink-0">
          {status === "dirty" ? "Editing…" : status === "saved" ? "Saved" : ""}
        </span>
      </div>
    </div>
  );
}

// ---- Repo reader (RFC-git-sources §7) --------------------------------------

/** Relative path of a child within its parent's root. Git parents' children
 *  live in the app-data cache (`…/git/<parent-id>/…`); folder children live
 *  under the folder path itself. */
function childRel(parent: Source, child: Source): string {
  const marker = `/git/${parent.id}/`;
  const i = child.url.indexOf(marker);
  if (i !== -1) return child.url.slice(i + marker.length);
  const root = parent.url.endsWith("/") ? parent.url : parent.url + "/";
  return child.url.startsWith(root) ? child.url.slice(root.length) : child.url;
}

type RepoNode = {
  name: string;
  path: string;
  child?: Source;
  kids: RepoNode[];
};

function buildRepoTree(pairs: { rel: string; child: Source }[]): RepoNode[] {
  const root: RepoNode[] = [];
  for (const { rel, child } of [...pairs].sort((a, b) =>
    a.rel.localeCompare(b.rel),
  )) {
    const parts = rel.split("/").filter(Boolean);
    let level = root;
    let path = "";
    for (let i = 0; i < parts.length; i++) {
      path = path ? `${path}/${parts[i]}` : parts[i];
      const isFile = i === parts.length - 1;
      if (isFile) {
        level.push({ name: parts[i], path, child, kids: [] });
      } else {
        let dir = level.find((n) => !n.child && n.name === parts[i]);
        if (!dir) {
          dir = { name: parts[i], path, kids: [] };
          level.push(dir);
        }
        level = dir.kids;
      }
    }
  }
  return root;
}

function allDirPaths(nodes: RepoNode[], out: string[] = []): string[] {
  for (const n of nodes) {
    if (!n.child) {
      out.push(n.path);
      allDirPaths(n.kids, out);
    }
  }
  return out;
}

/** Entries of the directory at `path` ("" = root) in the built tree. */
function dirEntries(tree: RepoNode[], path: string): RepoNode[] {
  if (!path) return tree;
  let level = tree;
  for (const part of path.split("/")) {
    const dir = level.find((n) => !n.child && n.name === part);
    if (!dir) return [];
    level = dir.kids;
  }
  return level;
}

function RepoTreeRows({
  nodes,
  depth,
  closed,
  onToggleDir,
  selected,
  onSelect,
}: {
  nodes: RepoNode[];
  depth: number;
  closed: Set<string>;
  onToggleDir: (path: string) => void;
  selected: string | null;
  onSelect: (child: Source) => void;
}) {
  return (
    <>
      {nodes.map((n) =>
        n.child ? (
          <button
            key={n.path}
            type="button"
            onClick={() => onSelect(n.child!)}
            title={n.path}
            className={cn(
              "flex w-full min-w-0 items-center gap-1.5 rounded-md px-1.5 py-[3px] text-left text-caption",
              selected === n.child.id
                ? "bg-surface-2 text-foreground"
                : "text-muted-foreground hover:bg-surface-2",
            )}
            style={{ paddingLeft: `${6 + depth * 12}px` }}
          >
            <span
              className={cn(
                "h-1.5 w-1.5 shrink-0 rounded-full",
                n.child.chunkCount > 0
                  ? "bg-[color:var(--citation)]"
                  : "border border-subtle-foreground",
              )}
              title={n.child.chunkCount > 0 ? "Indexed" : "Text only"}
            />
            <span className="truncate">{n.name}</span>
          </button>
        ) : (
          <Fragment key={n.path}>
            <button
              type="button"
              onClick={() => onToggleDir(n.path)}
              title={n.path}
              className="flex w-full min-w-0 items-center gap-1 rounded-md px-1.5 py-[3px] text-left text-caption text-muted-foreground hover:bg-surface-2"
              style={{ paddingLeft: `${4 + depth * 12}px` }}
              aria-expanded={!closed.has(n.path)}
            >
              <ChevronRight
                className={cn(
                  "h-3 w-3 shrink-0 text-subtle-foreground transition-transform duration-150",
                  !closed.has(n.path) && "rotate-90",
                )}
              />
              <span className="truncate">{n.name}/</span>
            </button>
            {!closed.has(n.path) && (
              <RepoTreeRows
                nodes={n.kids}
                depth={depth + 1}
                closed={closed}
                onToggleDir={onToggleDir}
                selected={selected}
                onSelect={onSelect}
              />
            )}
          </Fragment>
        ),
      )}
    </>
  );
}

/** Finder-style breadcrumb: each directory segment opens a menu of that
 *  directory's entries; picking a file opens it, picking a directory expands
 *  it in the tree. */
function RepoBreadcrumb({
  repoTitle,
  rel,
  tree,
  onSelect,
  onRevealDir,
}: {
  repoTitle: string;
  rel: string;
  tree: RepoNode[];
  onSelect: (child: Source) => void;
  onRevealDir: (path: string) => void;
}) {
  const [openAt, setOpenAt] = useState<string | null>(null);
  const parts = rel.split("/").filter(Boolean);
  const segs: { label: string; dirPath: string | null }[] = [
    { label: repoTitle, dirPath: "" },
    ...parts.map((p, i) => ({
      label: p,
      dirPath: i < parts.length - 1 ? parts.slice(0, i + 1).join("/") : null,
    })),
  ];
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-0.5 text-caption">
      {segs.map((seg, i) => (
        <Fragment key={`${seg.label}-${i}`}>
          {i > 0 && (
            <ChevronRight className="h-3 w-3 shrink-0 text-subtle-foreground" />
          )}
          {seg.dirPath !== null ? (
            <span className="relative">
              <button
                type="button"
                onClick={() =>
                  setOpenAt(openAt === seg.dirPath ? null : seg.dirPath)
                }
                className="rounded px-1 py-0.5 font-mono text-muted-foreground hover:bg-surface-2 hover:text-foreground"
              >
                {seg.label}
              </button>
              {openAt === seg.dirPath && (
                <>
                  <button
                    type="button"
                    aria-label="Close menu"
                    className="fixed inset-0 z-20 cursor-default"
                    onClick={() => setOpenAt(null)}
                  />
                  <div className="menu-glass absolute left-0 top-full z-30 mt-1 max-h-72 min-w-44 overflow-y-auto rounded-md py-1">
                    {dirEntries(tree, seg.dirPath).map((n) => (
                      <button
                        key={n.path}
                        type="button"
                        onClick={() => {
                          setOpenAt(null);
                          if (n.child) onSelect(n.child);
                          else onRevealDir(n.path);
                        }}
                        className="flex w-full items-center gap-1.5 px-2.5 py-1 text-left text-caption text-foreground hover:bg-surface-2"
                      >
                        {n.child ? (
                          <span
                            className={cn(
                              "h-1.5 w-1.5 shrink-0 rounded-full",
                              n.child.chunkCount > 0
                                ? "bg-[color:var(--citation)]"
                                : "border border-subtle-foreground",
                            )}
                          />
                        ) : (
                          <ChevronRight className="h-3 w-3 shrink-0 text-subtle-foreground" />
                        )}
                        <span className="truncate">
                          {n.name}
                          {n.child ? "" : "/"}
                        </span>
                      </button>
                    ))}
                  </div>
                </>
              )}
            </span>
          ) : (
            <span className="px-1 font-mono text-foreground">{seg.label}</span>
          )}
        </Fragment>
      ))}
    </div>
  );
}

/** Shiki language ids by extension; anything else renders as plain mono. */
const SHIKI_LANGS: Record<string, string> = {
  rs: "rust", ts: "typescript", tsx: "tsx", js: "javascript", jsx: "jsx",
  mjs: "javascript", cjs: "javascript", py: "python", go: "go", rb: "ruby",
  java: "java", kt: "kotlin", swift: "swift", c: "c", h: "c", cc: "cpp",
  cpp: "cpp", hpp: "cpp", php: "php", sh: "shellscript", bash: "shellscript",
  zsh: "shellscript", sql: "sql", scala: "scala", lua: "lua", toml: "toml",
  yaml: "yaml", yml: "yaml", json: "json", jsonc: "jsonc", hcl: "hcl",
  tf: "hcl", css: "css", scss: "scss", html: "html", xml: "xml",
  proto: "proto", graphql: "graphql", vue: "vue", svelte: "svelte",
  dockerfile: "dockerfile", md: "markdown",
};

// The css-variables theme is built once; colors ride the app tokens (see
// index.css) so every scheme carries through.
let shikiThemePromise: Promise<unknown> | null = null;

function CodeView({
  path,
  code,
  lineNums,
}: {
  path: string;
  code: string;
  lineNums: boolean;
}) {
  const [html, setHtml] = useState<string | null>(null);
  useEffect(() => {
    let on = true;
    void (async () => {
      try {
        const shiki = await import("shiki");
        shikiThemePromise ??= Promise.resolve(
          shiki.createCssVariablesTheme({
            name: "alchemy",
            variablePrefix: "--shiki-",
            fontStyle: true,
          }),
        );
        const theme =
          (await shikiThemePromise) as import("shiki").ThemeRegistrationAny;
        const name = path.split("/").pop()?.toLowerCase() ?? "";
        const ext = name.includes(".") ? name.split(".").pop()! : name;
        const lang = SHIKI_LANGS[ext] ?? "txt";
        const out = await shiki.codeToHtml(code, { lang, theme });
        if (on) setHtml(out);
      } catch {
        if (on) setHtml(null);
      }
    })();
    return () => {
      on = false;
    };
  }, [path, code]);
  if (!html) {
    return (
      <pre className="reader-plain selectable overflow-x-auto whitespace-pre font-mono text-caption leading-relaxed text-foreground/90">
        {code}
      </pre>
    );
  }
  return (
    <div
      className={cn(
        "shiki-view selectable overflow-x-auto text-caption leading-relaxed",
        lineNums && "shiki-nums",
      )}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

/** The repo reader: DocProperties, an independently scrolling file tree and
 *  file pane, Finder-style breadcrumbs, and select-to-chat over code. */
function RepoView({ source, map }: { source: Source; map: string | null }) {
  const sources = useStore((s) => s.sources);
  const sendMessage = useStore((s) => s.sendMessage);
  const pairs = useMemo(
    () =>
      sources
        .filter((c) => c.parentId === source.id)
        .map((child) => ({ rel: childRel(source, child), child })),
    [sources, source],
  );
  const tree = useMemo(() => buildRepoTree(pairs), [pairs]);
  const readme = useMemo(() => {
    const candidates = pairs
      .filter(({ rel }) => /(^|\/)readme(\.[a-z]+)?$/i.test(rel))
      .sort((a, b) => a.rel.split("/").length - b.rel.split("/").length);
    return candidates[0]?.child ?? null;
  }, [pairs]);
  const [sel, setSel] = useState<Source | null>(null);
  const [selContent, setSelContent] = useState<string | null>(null);
  // Same 250ms grace as the text reader: no spinner flash on fast loads.
  const showMapLoading = useDelayedFlag(map === null);
  const showFileLoading = useDelayedFlag(selContent === null);
  const [readmeContent, setReadmeContent] = useState<string | null>(null);
  const [closed, setClosed] = useState<Set<string>>(new Set());
  const [lineNums, setLineNums] = useState(true);
  const [tierBusy, setTierBusy] = useState(false);
  const [codeSel, setCodeSel] = useState<{
    text: string;
    top: number;
    left: number;
  } | null>(null);
  const paneRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!sel) return;
    let on = true;
    setSelContent(null);
    api
      .getSourceContent(sel.id)
      .then((c) => on && setSelContent(c))
      .catch(() => on && setSelContent(""));
    return () => {
      on = false;
    };
  }, [sel]);

  useEffect(() => {
    if (!readme) return;
    let on = true;
    api
      .getSourceContent(readme.id)
      .then((c) => on && setReadmeContent(c))
      .catch(() => on && setReadmeContent(null));
    return () => {
      on = false;
    };
  }, [readme]);

  const selRel = sel ? childRel(source, sel) : null;
  const selIsCode = !!sel && sel.sourceType === "code";
  const anyClosed = closed.size > 0;

  /** Promote to embedded / demote to search-only (RFC-git-sources §4) —
   *  persists per file and re-ingests to match. */
  async function toggleTier() {
    if (!sel || tierBusy) return;
    setTierBusy(true);
    try {
      const updated = await api.setChildEmbedded(sel.id, !(sel.chunkCount > 0));
      const id = useStore.getState().currentId;
      if (id) useStore.setState({ sources: await api.listSources(id) });
      setSel(updated);
    } finally {
      setTierBusy(false);
    }
  }

  // Keyboard selection (shift+arrows) never fires the pane's onMouseUp —
  // follow selectionchange too, debounced so drags don't churn the toolbar.
  const captureSelectionRef = useRef<() => void>(() => {});
  captureSelectionRef.current = captureSelection;
  useEffect(() => {
    let timer: number | null = null;
    const onChange = () => {
      if (timer) window.clearTimeout(timer);
      timer = window.setTimeout(() => captureSelectionRef.current(), 200);
    };
    document.addEventListener("selectionchange", onChange);
    return () => {
      if (timer) window.clearTimeout(timer);
      document.removeEventListener("selectionchange", onChange);
    };
  }, []);

  function captureSelection() {
    const container = paneRef.current;
    const s = window.getSelection();
    if (!container || !s || s.isCollapsed) {
      setCodeSel(null);
      return;
    }
    const text = s.toString().trim();
    if (text.length < 4 || !container.contains(s.anchorNode)) {
      setCodeSel(null);
      return;
    }
    const rect = s.getRangeAt(0).getBoundingClientRect();
    const host = container.getBoundingClientRect();
    setCodeSel({
      text,
      top: rect.top - host.top + container.scrollTop - 34,
      left: Math.max(8, rect.left - host.left),
    });
  }

  function chatAbout(prefix: string) {
    if (!codeSel) return;
    const block = `\`\`\`\n${codeSel.text}\n\`\`\``;
    setCodeSel(null);
    useStore.getState().closeReader();
    void sendMessage(
      `${prefix} \`${selRel}\` in "${source.title}":\n\n${block}`,
    );
  }

  function askCodeCustom() {
    if (!codeSel) return;
    const block = `\`\`\`\n${codeSel.text}\n\`\`\``;
    setCodeSel(null);
    useStore.getState().closeReader();
    useStore.setState({
      pendingInput: `About this code from \`${selRel}\` in "${source.title}":\n${block}\n\n`,
    });
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col px-8 py-5">
      <DocProperties source={source} git={parseGitProvenance(map)} />
      <div className="flex min-h-0 flex-1">
        <div className="flex w-[240px] shrink-0 flex-col border-r border-border">
          <div className="flex items-center justify-between pb-1 pr-2">
            <button
              type="button"
              onClick={() => setSel(null)}
              className={cn(
                "flex items-center gap-1.5 rounded-md px-1.5 py-[3px] text-left text-caption",
                sel === null
                  ? "bg-surface-2 text-foreground"
                  : "text-muted-foreground hover:bg-surface-2",
              )}
            >
              <ListTree className="h-3 w-3 shrink-0" />
              Overview
            </button>
            <button
              type="button"
              title={anyClosed ? "Expand all" : "Collapse all"}
              aria-label={anyClosed ? "Expand all folders" : "Collapse all folders"}
              onClick={() =>
                setClosed(anyClosed ? new Set() : new Set(allDirPaths(tree)))
              }
              className="rounded p-1 text-subtle-foreground hover:bg-surface-2 hover:text-foreground"
            >
              {anyClosed ? (
                <ChevronsUpDown className="h-3.5 w-3.5" />
              ) : (
                <ChevronsDownUp className="h-3.5 w-3.5" />
              )}
            </button>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto py-1 pr-2">
            <RepoTreeRows
              nodes={tree}
              depth={0}
              closed={closed}
              onToggleDir={(p) =>
                setClosed((prev) => {
                  const next = new Set(prev);
                  if (next.has(p)) next.delete(p);
                  else next.add(p);
                  return next;
                })
              }
              selected={sel?.id ?? null}
              onSelect={setSel}
            />
          </div>
        </div>
        <div
          ref={paneRef}
          onMouseUp={captureSelection}
          className="relative min-w-0 flex-1 overflow-y-auto py-1 pl-6"
        >
          {codeSel && (
            <div
              className="menu-glass absolute z-30 flex items-center gap-0.5 rounded-md px-1 py-0.5"
              style={{ top: Math.max(0, codeSel.top), left: codeSel.left }}
            >
              <SelAction
                icon={<Sparkles className="h-3.5 w-3.5" />}
                label="Explain this"
                onClick={() => chatAbout("Explain this code from")}
              />
              <SelAction
                icon={<MessageSquarePlus className="h-3.5 w-3.5" />}
                label="Ask…"
                onClick={askCodeCustom}
              />
            </div>
          )}
          {sel === null ? (
            map === null ? (
              showMapLoading && (
                <div className="flex items-center gap-2 text-body text-muted-foreground">
                  <Spinner className="h-3.5 w-3.5" /> Loading…
                </div>
              )
            ) : (
              <div className="selectable">
                {readme && readmeContent && (
                  <>
                    <div className="mb-2 flex items-center gap-2">
                      <span className="text-micro font-medium uppercase tracking-wide text-subtle-foreground">
                        Readme
                      </span>
                      <button
                        type="button"
                        onClick={() => setSel(readme)}
                        className="text-micro text-citation hover:underline"
                      >
                        Open file →
                      </button>
                    </div>
                    <Markdown>{readmeContent}</Markdown>
                    <div className="my-5 h-px bg-border" />
                    <div className="mb-2 text-micro font-medium uppercase tracking-wide text-subtle-foreground">
                      Map
                    </div>
                  </>
                )}
                <Markdown>{map}</Markdown>
              </div>
            )
          ) : (
            <>
              <div className="sticky top-0 z-10 -ml-1 flex items-center gap-2 bg-background/85 py-1 pl-1 backdrop-blur">
                <RepoBreadcrumb
                  repoTitle={source.title}
                  rel={selRel ?? ""}
                  tree={tree}
                  onSelect={setSel}
                  onRevealDir={(p) =>
                    setClosed((prev) => {
                      const next = new Set(prev);
                      next.delete(p);
                      return next;
                    })
                  }
                />
                <button
                  type="button"
                  onClick={() => void toggleTier()}
                  disabled={tierBusy}
                  title={
                    sel.chunkCount > 0
                      ? "Keep this file findable by text match only"
                      : "Include this file in search and citations"
                  }
                  className={cn(
                    "flex shrink-0 items-center gap-1 rounded-full border border-border px-2 py-px text-micro hover:border-border-strong hover:bg-surface-2",
                    sel.chunkCount > 0 ? "text-citation" : "text-muted-foreground",
                  )}
                >
                  {tierBusy && <Spinner className="h-2.5 w-2.5" />}
                  {sel.chunkCount > 0 ? "Indexed" : "Text only"}
                </button>
                <span className="flex-1" />
                {/* Reveal the selected file itself — the reader header's Show
                    in Finder only ever reaches the folder root. */}
                {sel.url && !isWebUrl(sel.url) && (
                  <button
                    type="button"
                    title="Show this file in Finder"
                    aria-label="Show this file in Finder"
                    onClick={() => void revealItemInDir(sel.url)}
                    className="rounded p-1 text-subtle-foreground hover:bg-surface-2 hover:text-foreground"
                  >
                    <FolderOpen className="h-3.5 w-3.5" />
                  </button>
                )}
                {selIsCode && (
                  <button
                    type="button"
                    title={lineNums ? "Hide line numbers" : "Show line numbers"}
                    aria-label={
                      lineNums ? "Hide line numbers" : "Show line numbers"
                    }
                    onClick={() => setLineNums((v) => !v)}
                    className={cn(
                      "rounded p-1 hover:bg-surface-2",
                      lineNums
                        ? "text-citation"
                        : "text-subtle-foreground hover:text-foreground",
                    )}
                  >
                    <ListOrdered className="h-3.5 w-3.5" />
                  </button>
                )}
              </div>
              {selContent === null ? (
                showFileLoading && (
                  <div className="flex items-center gap-2 text-body text-muted-foreground">
                    <Spinner className="h-3.5 w-3.5" /> Loading file…
                  </div>
                )
              ) : selIsCode ? (
                <CodeView
                  path={selRel ?? ""}
                  code={selContent}
                  lineNums={lineNums}
                />
              ) : (
                // Vault/folder notes render [[wikilinks]] as hops, routed
                // through the selected file's path as the link origin.
                <div
                  className="selectable"
                  onClickCapture={docLinkClickHandler(sel?.url || undefined)}
                >
                  <Markdown wikilinks>{selContent}</Markdown>
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

/** For folder/repo children: a quiet jump back to the parent's repo reader. */
function ParentJump({ source }: { source: Source }) {
  const parent = useStore((s) =>
    source.parentId ? s.sources.find((x) => x.id === source.parentId) : undefined,
  );
  const openSourceViewer = useStore((s) => s.openSourceViewer);
  if (!parent) return null;
  return (
    <button
      type="button"
      onClick={() => openSourceViewer(parent.id, parent.title)}
      className="mb-2 flex items-center gap-1 text-caption text-muted-foreground hover:text-citation"
    >
      <ChevronRight className="h-3 w-3 rotate-180" />
      in {parent.title}
    </button>
  );
}
