import { useEffect, useMemo, useRef, useState } from "react";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Modal, Input, Spinner } from "./ui";
import { sourceIcon } from "./SourcesPanel";
import { cn, isWebUrl } from "@/lib/utils";
import {
  ChevronDown,
  ChevronUp,
  ExternalLink,
  FolderOpen,
  MessageSquarePlus,
  Scale,
  Search,
  Sparkles,
  X,
} from "lucide-react";

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
  // Look for the tail only within the window the chunk could occupy.
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

/** How much selected text travels into a chat question before truncation. */
const MAX_PASSAGE_CHARS = 1200;

/** Full-text reader for a source, scrolled to a cited passage or search hit. */
export function SourceViewer() {
  const viewing = useStore((s) => s.viewingSource);
  const close = useStore((s) => s.closeSourceViewer);
  const sources = useStore((s) => s.sources);
  const sendMessage = useStore((s) => s.sendMessage);
  const sending = useStore((s) => s.sending);
  const [content, setContent] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  /** Live text selection inside the reader body, with popover coordinates. */
  const [sel, setSel] = useState<{
    text: string;
    top: number;
    left: number;
  } | null>(null);
  const markRef = useRef<HTMLElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  // Cmd/Ctrl+F focuses the find field while the reader is open.
  useEffect(() => {
    if (!viewing) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "f") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [viewing]);

  const source = sources.find((s) => s.id === viewing?.sourceId);

  useEffect(() => {
    setContent(null);
    setQuery("");
    setActive(0);
    setSel(null);
    if (!viewing) return;
    let stale = false;
    api
      .getSourceContent(viewing.sourceId)
      .then((text) => {
        if (!stale) setContent(text);
      })
      .catch(() => {
        if (!stale) setContent("");
      });
    return () => {
      stale = true;
    };
  }, [viewing]);

  const matches = useMemo(
    () => (content ? findMatches(content, query) : []),
    [content, query],
  );
  const passage = useMemo(
    () =>
      content && viewing?.highlight && !query.trim()
        ? locatePassage(content, viewing.highlight)
        : null,
    [content, viewing, query],
  );

  // Highlight search matches when searching, else the cited passage.
  const ranges: [number, number][] = query.trim()
    ? matches
    : passage
      ? [passage]
      : [];
  const activeIdx = query.trim()
    ? Math.min(active, Math.max(0, matches.length - 1))
    : 0;

  // Bring the active highlight into view once content and ranges settle.
  useEffect(() => {
    markRef.current?.scrollIntoView({ block: "center" });
  }, [content, activeIdx, query, passage]);

  const step = (dir: 1 | -1) => {
    if (matches.length === 0) return;
    setActive((a) => (a + dir + matches.length) % matches.length);
  };

  const title = source?.title ?? viewing?.title ?? "this source";

  // Listen at the window so releasing the mouse OUTSIDE the text container
  // (common when dragging a selection) still raises the toolbar. The handler
  // itself validates that the selection lives inside the reader body.
  const updateSelectionRef = useRef<() => void>(() => {});
  useEffect(() => {
    if (!viewing) return;
    const onUp = () => updateSelectionRef.current();
    window.addEventListener("mouseup", onUp);
    return () => window.removeEventListener("mouseup", onUp);
  }, [viewing]);

  /** Track the selection inside the reader body to place the ask toolbar. */
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
      // Coordinates are relative to the scroll container so the toolbar
      // travels with the text; clamped so it never clips out of view.
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

  /** Fire a grounded chat question about the selected passage and close. */
  function askAbout(question: string) {
    const p = selectedPassage();
    if (!p) return;
    setSel(null);
    close();
    void sendMessage(`${question}\n\n"${p}"`);
  }

  /** Stage the passage in the composer so the user can write the question. */
  function askCustom() {
    const p = selectedPassage();
    if (!p) return;
    setSel(null);
    close();
    useStore.setState({
      pendingInput: `About this passage from "${title}":\n"${p}"\n\n`,
    });
  }

  const segments: { text: string; hit: boolean; current: boolean }[] = [];
  if (content) {
    let pos = 0;
    ranges.forEach(([s, e], i) => {
      if (s > pos)
        segments.push({
          text: content.slice(pos, s),
          hit: false,
          current: false,
        });
      segments.push({
        text: content.slice(s, e),
        hit: true,
        current: i === activeIdx,
      });
      pos = e;
    });
    if (pos < content.length)
      segments.push({ text: content.slice(pos), hit: false, current: false });
  }

  return (
    <Modal
      open={!!viewing}
      onClose={close}
      title={source?.title ?? viewing?.title ?? ""}
      width="max-w-3xl"
    >
      {viewing && (
        <div className="flex h-[70vh] flex-col gap-3">
          <div className="flex shrink-0 items-center gap-2">
            {source && (
              <span className="flex items-center gap-1.5 text-[11px] text-subtle-foreground">
                {sourceIcon(source.sourceType, source.url)}
                {source.chunkCount} chunks ·{" "}
                {Intl.NumberFormat().format(source.charCount)} chars
                <span className="hidden sm:inline">
                  {" "}
                  · select text to ask about it
                </span>
              </span>
            )}
            {source?.url &&
              (isWebUrl(source.url) ? (
                <button
                  className="inline-flex items-center gap-1 text-[11px] text-citation hover:underline"
                  onClick={() => void openUrl(source.url)}
                  title={source.url}
                >
                  Open original
                  <ExternalLink className="h-3 w-3" />
                </button>
              ) : (
                <button
                  className="inline-flex items-center gap-1 text-[11px] text-citation hover:underline"
                  onClick={() => void revealItemInDir(source.url)}
                  title={source.url}
                >
                  Show in Finder
                  <FolderOpen className="h-3 w-3" />
                </button>
              ))}
            <div className="ml-auto flex items-center gap-1.5">
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
                  }}
                  placeholder="Find in source…"
                  className="h-7 w-52 pl-7 pr-7 text-[12px]"
                />
                {query && (
                  <button
                    className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-muted-foreground hover:text-foreground"
                    onClick={() => setQuery("")}
                    aria-label="Clear search"
                  >
                    <X className="h-3 w-3" />
                  </button>
                )}
              </div>
              {query.trim() && (
                <>
                  <span className="text-[11px] tabular-nums text-subtle-foreground">
                    {matches.length === 0
                      ? "0/0"
                      : `${activeIdx + 1}/${matches.length}`}
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
                </>
              )}
            </div>
          </div>

          <div
            ref={bodyRef}
            className="relative min-h-0 flex-1 overflow-y-auto rounded-md border border-border bg-surface-2/40 p-4"
          >
            {sel && content && (
              <div
                className="absolute z-10 flex items-center gap-0.5 rounded-md border border-border-strong bg-elevated p-0.5 shadow-lg"
                style={{
                  top: sel.top,
                  left: sel.left,
                  transform: "translate(-50%, calc(-100% - 6px))",
                }}
                // preventDefault keeps the browser from collapsing the
                // selection on mousedown; stopPropagation keeps the container's
                // mouseup handler from dismissing the toolbar before click.
                onMouseDown={(e) => e.preventDefault()}
                onMouseUp={(e) => e.stopPropagation()}
                role="toolbar"
                aria-label="Ask about selection"
              >
                <SelAction
                  icon={<Sparkles className="h-3.5 w-3.5" />}
                  label="Explain"
                  disabled={sending}
                  onClick={() =>
                    askAbout(`Explain this passage from "${title}":`)
                  }
                />
                <SelAction
                  icon={<Scale className="h-3.5 w-3.5" />}
                  label="Compare sources"
                  disabled={sending}
                  onClick={() =>
                    askAbout(
                      `What do the other sources say about this passage from "${title}"? ` +
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
            {content === null ? (
              <div className="flex items-center gap-2 text-[13px] text-muted-foreground">
                <Spinner className="h-3.5 w-3.5" /> Loading source…
              </div>
            ) : content === "" ? (
              <div className="text-[13px] text-muted-foreground">
                No text stored for this source.
              </div>
            ) : (
              <p className="whitespace-pre-wrap text-[13px] leading-relaxed text-foreground/90 selectable">
                {segments.map((seg, i) =>
                  seg.hit ? (
                    <mark
                      key={i}
                      ref={seg.current ? markRef : undefined}
                      className={cn(
                        "rounded-sm px-0.5 text-foreground",
                        // Strong highlight only for the active search match; a
                        // cited passage can span paragraphs, so keep it soft.
                        seg.current && query.trim()
                          ? "bg-primary/40"
                          : "bg-primary/15",
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
      )}
    </Modal>
  );
}

/** One action in the floating selection toolbar. */
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
