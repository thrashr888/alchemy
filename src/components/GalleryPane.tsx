import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api } from "@/lib/api";
import { removeSourcesGuarded, useStore } from "@/lib/store";
import type { Source } from "@/lib/types";
import { GROUP_LABEL, GROUP_OF, type TypeGroup } from "@/lib/sourceGroups";
import {
  Button,
  CardAction,
  EmptyState,
  Input,
  RowMenu,
  useConfirm,
  useHoverCard,
  useMarquee,
} from "./ui";
import { cn, isWebUrl, relativeTime, scrollMemory, shortcutBlocked, urlHost } from "@/lib/utils";
import { FilterBar } from "./FilterBar";
import { AttachToCardModal } from "./RegistrySection";
import { Favicon, sourceHoverData } from "./SourcesPanel";
import {
  sourceMetaItems,
  sourceOriginItems,
  useSourceMetaModals,
} from "./SourceMetaModals";
import { sourceIcon } from "@/lib/sourceIcon";
import { GraphView } from "./GraphView";
import {
  ArrowLeft,
  LayoutGrid,
  MessageSquare,
  Package,
  Pencil,
  RefreshCw,
  Search,
  Trash2,
} from "lucide-react";

/* The source Gallery (docs/RFC-source-gallery.md): the notebook's sources as
 * a masonry of visual cards — a mymind/are.na-style browse surface beside
 * Chat, Reader, and Ledger. Scraped pages lead with their og:image, PDFs
 * with their first page, images with themselves, text with its opening
 * lines; folders drill into their own level. */

/** Notebooks already swept for lead images this app run — the "-" sentinel
 *  guards across runs, this guards within one. */
const sweptNotebooks = new Set<string>();

/* Scroll positions per notebook+level ride the persistent scrollMemory in
 * lib/utils — Reader round-trips AND relaunches come back to the same place. */

/** Resolved card visuals (data URIs) per source id, so reopening the
 *  gallery paints instantly instead of re-running IPC + image fetches.
 *  "" = checked, none. The backend also disk-caches og downloads. */
const thumbMemory = new Map<string, string>();

/** At most this many thumbnail IPC calls in flight. Every mounted card used
 *  to fire its own immediately — a large gallery meant hundreds of parallel
 *  requests, each potentially file I/O, a PDF render, or a download. */
const THUMB_CONCURRENCY = 4;
const thumbQueue: (() => void)[] = [];
let thumbInFlight = 0;
function pumpThumbs() {
  while (thumbInFlight < THUMB_CONCURRENCY && thumbQueue.length > 0) {
    thumbInFlight++;
    thumbQueue.shift()!();
  }
}
function enqueueThumb(job: () => Promise<void>) {
  thumbQueue.push(() => {
    void job().finally(() => {
      thumbInFlight--;
      pumpThumbs();
    });
  });
  pumpThumbs();
}

type SortMode = "recent" | "title";

/** Kinds whose card leads with opening lines of the text. URL sources join
 *  when their page yielded no lead image. */
function wantsSnippet(s: Source): boolean {
  if (["text", "markdown", "html", "code", "mac"].includes(s.sourceType))
    return true;
  return s.sourceType === "url" && (s.imageUrl === "" || s.imageUrl === "-");
}

/** The header's compact segmented switch. Two of these sit side by side —
 *  grid/graph and the sort order — and they must read as one control family,
 *  which a second hand-rolled copy would drift away from within a release.
 *
 *  Deliberately not FilterBar's FilterButton: that one is a filter-row
 *  control (py-1, text-caption) and this is the tighter header variant. Same
 *  idea, different size class; merging them would resize one surface or the
 *  other. */
function Segmented<T extends string>({
  options,
  value,
  onChange,
  hint,
  className,
}: {
  options: readonly T[];
  value: T;
  onChange: (value: T) => void;
  /** Tooltip per option — these switches are one word wide, so the tooltip
   *  is where the meaning actually lives. */
  hint: (option: T) => string;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex shrink-0 items-center gap-0.5 rounded-lg border border-border p-0.5",
        className,
      )}
    >
      {options.map((option) => (
        <button
          key={option}
          type="button"
          onClick={() => onChange(option)}
          aria-pressed={value === option}
          title={hint(option)}
          className={cn(
            "rounded-md px-2 py-0.5 text-micro font-medium capitalize transition-colors",
            value === option
              ? "bg-surface-2 text-foreground"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {option}
        </button>
      ))}
    </div>
  );
}

export function GalleryPane() {
  const currentId = useStore((s) => s.currentId);
  const sources = useStore((s) => s.sources);
  const { confirm, dialog: confirmDialog } = useConfirm();
  // Shared tag/note editors (SourceMetaModals) for the card menu entries.
  const meta = useSourceMetaModals();
  const [attaching, setAttaching] = useState<Source | null>(null);
  /** Find-in-gallery (Cmd/Ctrl+F). A grid's analogue of find-in-source is
   *  filtering, not stepping through ranges — same bar, same Escape/Done,
   *  but the result is which cards remain. */
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const findRef = useRef<HTMLInputElement>(null);
  const {
    show: showCard,
    hide: hideCard,
    card: hoverCard,
  } = useHoverCard("right");
  const [sweeping, setSweeping] = useState(false);
  const [folderId, setFolderId] = useState<string | null>(null);
  const [filter, setFilter] = useState<TypeGroup>("all");
  /** Cards or link graph. Persisted like the sort order: whichever way you
   *  browse is the way you browse, and being dropped back into the grid
   *  every time you open the gallery is the annoying kind of opinionated. */
  const [shape, setShapeState] = useState<"grid" | "graph">(
    () => (localStorage.getItem("galleryShape") as "grid" | "graph") || "grid",
  );
  const setShape = (next: "grid" | "graph") => {
    setShapeState(next);
    localStorage.setItem("galleryShape", next);
  };
  /** Tag chip filter (RFC-source-tags): null = all. Chips show only tags
   *  present at this level, so the row disappears in untagged notebooks. */
  const [tagFilter, setTagFilter] = useState<string | null>(null);
  const [sort, setSort] = useState<SortMode>(
    () => (localStorage.getItem("gallerySort") as SortMode) || "recent",
  );
  const [snippets, setSnippets] = useState<Record<string, string>>({});

  // Backfill lead images for pre-gallery URL sources, once per notebook per
  // run — fetch-and-stamp only, no re-embedding (RFC §backfill).
  useEffect(() => {
    if (!currentId || sweptNotebooks.has(currentId)) return;
    const missing = sources.some(
      (s) => s.sourceType === "url" && s.imageUrl === "" && isWebUrl(s.url),
    );
    if (!missing) return;
    sweptNotebooks.add(currentId);
    setSweeping(true);
    void api
      .backfillSourceImages(currentId)
      .then(async (found) => {
        if (found > 0 && useStore.getState().currentId === currentId) {
          useStore.setState({ sources: await api.listSources(currentId) });
        }
      })
      .catch(() => undefined)
      .finally(() => setSweeping(false));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentId]);

  // Cmd/Ctrl+F opens the find bar and focuses it — the same affordance the
  // reader has. The gallery and the reader never mount together (the center
  // column is a ternary), so neither listener can shadow the other.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Not while a modal owns the keyboard or the user is typing in a
      // field — find used to open behind dialogs and steal focus mid-word.
      if (shortcutBlocked(e)) return;
      if ((e.metaKey || e.ctrlKey) && e.key === "f") {
        e.preventDefault();
        setFindOpen(true);
        requestAnimationFrame(() => {
          findRef.current?.focus();
          findRef.current?.select();
        });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Edit > Find (menu.rs): the menu item has no accelerator, so it bumps
  // findBump; the mounted find surface (here, the gallery) answers.
  const findBump = useStore((s) => s.findBump);
  useEffect(() => {
    if (findBump === 0) return;
    setFindOpen(true);
    requestAnimationFrame(() => {
      findRef.current?.focus();
      findRef.current?.select();
    });
  }, [findBump]);

  // Closing clears the query, so the grid is never left silently filtered.
  useEffect(() => {
    if (!findOpen) setFindQuery("");
  }, [findOpen]);

  // Notebook switch resets the drill-in; a deleted folder falls back to root.
  useEffect(() => {
    setFolderId(null);
    setFilter("all");
    setTagFilter(null);
    setFindOpen(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentId]);
  const folder = folderId
    ? (sources.find((s) => s.id === folderId) ?? null)
    : null;
  useEffect(() => {
    if (folderId && !folder) setFolderId(null);
  }, [folderId, folder]);

  // The gallery shows one layer at a time: top-level sources at root,
  // a folder's children inside it.
  const level = sources.filter((s) =>
    folderId ? s.parentId === folderId : !s.parentId,
  );
  const present = new Set(level.map((s) => GROUP_OF[s.sourceType]));
  const groups: TypeGroup[] = [
    "all",
    ...(
      ["urls", "docs", "images", "text", "code", "mac", "folders"] as const
    ).filter((g) => present.has(g)),
  ];
  const effectiveFilter = groups.includes(filter) ? filter : "all";
  // Every tag present at this level for the chip row — biggest first
  // (most-used tags are the likeliest filters), alphabetical on ties.
  const tagCounts = new Map<string, number>();
  for (const s of level)
    for (const t of s.tags ? s.tags.split(" ") : [])
      tagCounts.set(t, (tagCounts.get(t) ?? 0) + 1);
  const levelTags = [...tagCounts.keys()].sort(
    (a, b) => tagCounts.get(b)! - tagCounts.get(a)! || a.localeCompare(b),
  );
  const effectiveTag =
    tagFilter && levelTags.includes(tagFilter) ? tagFilter : null;
  // Find matches every field the card can actually show — title, opening
  // lines, tags, author, url, host — so searching for what you remember
  // seeing works whichever part you remember. Snippets only exist for the
  // kinds that render them, which is why a PDF matches on title alone.
  const needle = findQuery.trim().toLowerCase();
  const matchesFind = (s: Source) =>
    !needle ||
    [
      s.title,
      snippets[s.id] ?? "",
      s.tags,
      s.author,
      s.url,
      urlHost(s.url) ?? "",
    ].some((f) => f?.toLowerCase().includes(needle));
  const cards = level
    .filter(
      (s) =>
        effectiveFilter === "all" || GROUP_OF[s.sourceType] === effectiveFilter,
    )
    .filter(
      (s) =>
        !effectiveTag ||
        (s.tags ? s.tags.split(" ") : []).includes(effectiveTag),
    )
    .filter(matchesFind)
    .sort((a, b) =>
      sort === "title"
        ? a.title.localeCompare(b.title, undefined, { sensitivity: "base" })
        : b.createdAt - a.createdAt,
    );

  // Opening-lines snippets, one batched IPC per level.
  const snippetIds = level
    .filter(wantsSnippet)
    .map((s) => s.id)
    .join(",");
  useEffect(() => {
    if (!snippetIds) {
      setSnippets({});
      return;
    }
    let stale = false;
    void api
      .sourceSnippets(snippetIds.split(","))
      .then((map) => {
        if (!stale) setSnippets(map);
      })
      .catch(() => undefined);
    return () => {
      stale = true;
    };
  }, [snippetIds]);

  // Masonry as JS-bucketed flex columns, not CSS multicol — WKWebView
  // reliably hit-tests but does NOT reliably paint later multicol columns
  // (cards were clickable yet invisible).
  //
  // The scroller mounts LATE when the gallery is the landing view (sources
  // arrive async, the empty state renders first) — so width measurement
  // hangs off a callback ref, not a mount-once effect, or the pane would
  // stay one-column forever.
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const [scrollerEl, setScrollerEl] = useState<HTMLDivElement | null>(null);
  const attachScroller = (el: HTMLDivElement | null) => {
    scrollerRef.current = el;
    setScrollerEl((cur) => (cur === el ? cur : el));
  };
  const [width, setWidth] = useState(0);
  useEffect(() => {
    if (!scrollerEl) return;
    const measure = () => setWidth(scrollerEl.getBoundingClientRect().width);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(scrollerEl);
    return () => ro.disconnect();
  }, [scrollerEl]);
  // Multi-select, same model as the sources sidebar: the shared "sources"
  // pick, so a selection made here is the same selection there. Cmd-click
  // toggles, shift-click ranges, and a rubber-band drag sweeps cards.
  const picked = useStore((st) => st.picked);
  const pickedIds = useMemo(
    () => new Set(picked?.kind === "sources" ? picked.ids : []),
    [picked],
  );
  const pickToggle = useStore((st) => st.pickToggle);
  const pickRange = useStore((st) => st.pickRange);
  const pickSet = useStore((st) => st.pickSet);
  const clearPicked = useStore((st) => st.clearPicked);
  const refreshSourcesBatch = useStore((st) => st.refreshSourcesBatch);
  const marqueeBase = useRef<string[]>([]);
  const { onPointerDown: marqueeDown, marquee, justEnded } = useMarquee({
    containerRef: scrollerRef,
    onStart: (additive) => {
      const pk = useStore.getState().picked;
      marqueeBase.current = additive && pk?.kind === "sources" ? pk.ids : [];
    },
    onSelect: (ids) =>
      pickSet(
        "sources",
        [...new Set([...marqueeBase.current, ...ids])],
        false,
      ),
    onClearBackground: clearPicked,
  });

  const colCount = Math.min(4, Math.max(1, Math.floor((width + 12) / 232)));
  // Shortest-column-first, not round-robin: cards range from a two-line title
  // to a 256px image plus a four-line snippet, so dealing them out in order
  // leaves one column running hundreds of pixels past the others. Packing by
  // estimated height keeps the bottom edge close to level. Sort order still
  // reads left-to-right, top-to-bottom — ties go to the leftmost column, so
  // equal-height cards deal out exactly as they did before.
  // One children index serves counts AND the per-card children lists — the
  // grid used to re-filter the whole sources array per folder card, per
  // render (and hover state renders this component).
  const childrenIndex = useMemo(() => {
    const m = new Map<string, Source[]>();
    for (const s of sources) {
      if (!s.parentId) continue;
      const list = m.get(s.parentId);
      if (list) list.push(s);
      else m.set(s.parentId, [s]);
    }
    return m;
  }, [sources]);
  const childCount = (s: Source) => (childrenIndex.get(s.id) ?? []).length;
  const columns: Source[][] = Array.from({ length: colCount }, () => []);
  const colWidth = (width - 48 - 12 * (colCount - 1)) / colCount;
  const heights = new Array<number>(colCount).fill(0);
  cards.forEach((s) => {
    let shortest = 0;
    for (let i = 1; i < colCount; i++) {
      if (heights[i] < heights[shortest] - 0.5) shortest = i;
    }
    columns[shortest].push(s);
    heights[shortest] +=
      estimateCardHeight(s, snippets[s.id], colWidth, childCount(s)) + 12;
  });

  // Sticky scroll per notebook+level: restore once the scroller exists,
  // save as it moves.
  const scrollKey = `${currentId}:${folderId ?? "root"}`;
  useLayoutEffect(() => {
    if (scrollerEl) scrollerEl.scrollTop = scrollMemory.get(scrollKey) ?? 0;
  }, [scrollerEl, scrollKey]);

  const setSortMode = (mode: SortMode) => {
    setSort(mode);
    localStorage.setItem("gallerySort", mode);
  };

  // The same actions the Sources panel rows carry, on the card's ⋯ menu —
  // and on plain right-click (RowMenu opens from the nearest .group).
  /** Batch variants of the card verbs when this card sits in a
   *  multi-selection — counts in the labels, same as the sidebar. */
  const batchMenuItems = (ids: string[]): ReturnType<typeof cardMenuItems> => {
    const count = ids.length;
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
        label: `Tag ${count} sources…`,
        icon: <Pencil className="h-3.5 w-3.5" />,
        onClick: () =>
          meta.setTagEdit({ ids, title: `${count} sources`, value: "" }),
      },
      {
        label: `Remove ${count} sources…`,
        icon: <Trash2 className="h-3.5 w-3.5" />,
        danger: true,
        onClick: () => void removeSourcesGuarded(ids, confirm),
      },
    ];
  };

  const cardMenuItems = (s: Source) => {
    const st = useStore.getState();
    const editable =
      !["url", "mac", "folder", "git", "notion", "obsidian"].includes(
        s.sourceType,
      ) && s.status !== "placeholder";
    return [
      // Chat scoped to this one source (a folder scopes to its files);
      // placeholders have no chunks yet, so there's nothing to ask.
      ...(s.status === "ready"
        ? [
            {
              label: "Ask about this source",
              icon: <MessageSquare className="h-3.5 w-3.5" />,
              onClick: () => st.askAboutSource(s.id),
            },
          ]
        : []),
      ...(editable
        ? [
            {
              label: "Edit text",
              icon: <Pencil className="h-3.5 w-3.5" />,
              onClick: () => {
                useStore.setState({ readerEditIntent: s.id });
                st.openSourceViewer(s.id, s.title);
              },
            },
          ]
        : []),
      ...(s.url
        ? [
            {
              label: s.sourceType === "mac" ? "Sync now" : "Refresh",
              icon: <RefreshCw className="h-3.5 w-3.5" />,
              onClick: () => void st.refreshSource(s.id),
            },
          ]
        : []),
      // Shared with the sources panel and reader — one menu per object.
      ...sourceOriginItems(s),
      ...sourceMetaItems(s, meta.setTagEdit, meta.setNoteEdit),
      {
        label: "File under a card…",
        icon: <Package className="h-3.5 w-3.5" />,
        onClick: () => setAttaching(s),
      },
      {
        label: "Remove…",
        icon: <Trash2 className="h-3.5 w-3.5" />,
        danger: true,
        onClick: () => void removeSourcesGuarded([s.id], confirm),
      },
    ];
  };

  return (
    <div className="flex h-full flex-1 flex-col min-w-0">
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-4">
        {folder ? (
          <>
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setFolderId(null)}
              title="Back to all sources"
              aria-label="Back to all sources"
            >
              <ArrowLeft className="h-3.5 w-3.5" />
            </Button>
            {sourceIcon(folder.sourceType, folder.url)}
            <span
              className="truncate text-body font-medium text-foreground"
              title={folder.title}
            >
              {folder.title}
            </span>
          </>
        ) : (
          <>
            <LayoutGrid className="h-4 w-4 text-muted-foreground" />
            <span className="text-body font-medium text-foreground">
              Gallery
            </span>
          </>
        )}
        <span className="text-caption text-subtle-foreground">
          {cards.length} {cards.length === 1 ? "source" : "sources"}
        </span>
        {sweeping && (
          <span className="text-caption text-subtle-foreground">
            Fetching page images…
          </span>
        )}
        {/* Grid vs graph: two ways to browse the same notebook, so the
            switch lives here rather than as a fourth top-level pane. */}
        <Segmented
          className="ml-auto"
          options={["grid", "graph"] as const}
          value={shape}
          onChange={setShape}
          hint={(mode) =>
            mode === "grid"
              ? "Cards"
              : "How these sources and notes link to each other"
          }
        />
        {/* Sort orders cards. A force layout places by connectedness, so
            there is no order for this to change — in graph mode the control
            was simply inert. */}
        {shape === "grid" && (
          <Segmented
            options={["recent", "title"] as const}
            value={sort}
            onChange={setSortMode}
            hint={(mode) => (mode === "recent" ? "Newest first" : "A to Z")}
          />
        )}
      </div>
      {findOpen && (
        <div className="flex shrink-0 items-center justify-end gap-1.5 border-b border-border px-4 py-1.5">
          <div className="relative">
            <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtle-foreground" />
            <Input
              ref={findRef}
              value={findQuery}
              onChange={(e) => setFindQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.stopPropagation();
                  setFindOpen(false);
                }
              }}
              placeholder="Find in gallery…"
              className="h-7 w-56 pl-7 text-caption"
            />
          </div>
          <span className="min-w-8 text-right text-micro tabular-nums text-subtle-foreground">
            {findQuery.trim()
              ? `${cards.length} ${cards.length === 1 ? "card" : "cards"}`
              : ""}
          </span>
          <Button variant="ghost" size="sm" onClick={() => setFindOpen(false)}>
            Done
          </Button>
        </div>
      )}
      {/* One filter bar: type groups, then tag chips — independent axes
          (a source matches both filters). Shared with the Registry's grid
          since RFC-registry §3; see FilterBar.tsx.

          Grid only: the graph filters the whole notebook rather than a
          level of the gallery, and by node kind as well as source type, so
          it renders its own bar with its own options. */}
      {shape === "grid" && (
        <FilterBar
          groups={groups.map((g) => ({
            value: g,
            label: g === "all" ? "All" : GROUP_LABEL[g],
          }))}
          group={effectiveFilter}
          onGroup={(v) => setFilter(v as TypeGroup)}
          chips={levelTags}
          chip={effectiveTag}
          onChip={setTagFilter}
        />
      )}
      {shape === "graph" ? (
        // Notebook-wide, and unaffected by the folder drill-in: a graph of
        // one folder's level hides exactly the links that make it worth
        // looking at. Type filtering lives inside the graph's own bar.
        <GraphView />
      ) : cards.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<LayoutGrid className="h-5 w-5" />}
            title={
              needle
                ? `No match for “${findQuery.trim()}”`
                : effectiveTag !== null
                  ? "Nothing with this tag here"
                  : effectiveFilter === "all"
                    ? "Nothing to explore yet"
                    : "Nothing of this type here"
            }
            hint={
              needle
                ? "Find looks at titles, opening lines, tags, author, and site."
                : effectiveFilter === "all" && effectiveTag === null
                  ? "Add sources and they'll appear here as cards."
                  : undefined
            }
          />
        </div>
      ) : (
        <div
          ref={attachScroller}
          onPointerDown={marqueeDown}
          onScroll={(e) =>
            scrollMemory.set(scrollKey, e.currentTarget.scrollTop)
          }
          className="min-h-0 flex-1 overflow-y-auto px-6 py-4"
        >
          {/* Skip the one-column flash while the first measure lands. */}
          {width > 0 && (
            <div className="flex items-start gap-3">
              {columns.map((col, i) => (
                <div key={i} className="flex min-w-0 flex-1 flex-col gap-3">
                  {col.map((s) => (
                    <div
                      key={s.id}
                      data-pick-id={s.id}
                      className={cn(
                        "rounded-lg",
                        pickedIds.has(s.id) &&
                          "ring-1 ring-primary ring-offset-1 ring-offset-background",
                      )}
                      onClickCapture={(e) => {
                        // A rubber-band drag ends in a click — that click
                        // is the drag's tail, not an open.
                        if (justEnded()) {
                          e.preventDefault();
                          e.stopPropagation();
                          return;
                        }
                        if (e.metaKey || e.ctrlKey) {
                          e.preventDefault();
                          e.stopPropagation();
                          pickToggle("sources", s.id);
                        } else if (e.shiftKey) {
                          e.preventDefault();
                          e.stopPropagation();
                          pickRange(
                            "sources",
                            cards.map((c) => c.id),
                            s.id,
                          );
                        }
                      }}
                    >
                      {GROUP_OF[s.sourceType] === "folders" ? (
                        <FolderCard
                          source={s}
                          childrenSources={childrenIndex.get(s.id) ?? []}
                          onOpen={() => setFolderId(s.id)}
                          menuItems={
                            pickedIds.has(s.id) && pickedIds.size > 1
                              ? batchMenuItems([...pickedIds])
                              : cardMenuItems(s)
                          }
                          onHover={(e) => showCard(e, sourceHoverData(s))}
                          onLeave={hideCard}
                        />
                      ) : (
                        <GalleryCard
                          source={s}
                          snippet={snippets[s.id]}
                          menuItems={
                            pickedIds.has(s.id) && pickedIds.size > 1
                              ? batchMenuItems([...pickedIds])
                              : cardMenuItems(s)
                          }
                          onHover={(e) => showCard(e, sourceHoverData(s))}
                          onLeave={hideCard}
                        />
                      )}
                    </div>
                  ))}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
      <AttachToCardModal
        sourceId={attaching?.id ?? null}
        sourceTitle={attaching?.title ?? ""}
        onClose={() => setAttaching(null)}
      />
      {marquee}
      {confirmDialog}
      {meta.modals}
      {hoverCard}
    </div>
  );
}

/** Roughly how tall a card will render, in px, so the masonry can pack
 *  columns by height instead of dealing round-robin. Deliberately an
 *  estimate: the real heights only exist after paint, and measuring them
 *  would mean a re-layout pass that fights React on every snippet arrival.
 *  It only has to RANK columns, so being a line off costs nothing visible.
 *  Mirrors the markup in `GalleryCard` / `FolderCard` — keep them in step. */
function estimateCardHeight(
  s: Source,
  snippet: string | undefined,
  colWidth: number,
  children: number,
): number {
  const inner = Math.max(80, colWidth - 24); // p-3 either side
  // ~6.6px per char at text-body, ~6px at text-caption.
  const lines = (text: string, clamp: number, charW: number) =>
    Math.min(clamp, Math.max(1, Math.ceil((text.length * charW) / inner)));
  const PADDING = 24; // p-3 top + bottom
  const CAPTION = 24; // icon + provenance row, incl. its mt-1.5

  if (GROUP_OF[s.sourceType] === "folders") {
    const peekRows = Math.ceil(Math.min(children, 4) / 2); // grid-cols-2
    return PADDING + 20 + (peekRows ? 8 + peekRows * 26 : 0) + 24;
  }

  const leadImage =
    s.sourceType === "url" && s.imageUrl !== "" && s.imageUrl !== "-";
  const hasVisual =
    s.sourceType === "pdf" || s.sourceType === "image" || leadImage;
  // PDFs crop to 4:3; everything else keeps its ratio capped at max-h-64.
  // Unmeasured images assume a middling landscape shape.
  const visual = hasVisual
    ? s.sourceType === "pdf"
      ? (colWidth * 3) / 4
      : Math.min(256, colWidth * 0.62)
    : 0;
  const title = lines(s.title, hasVisual ? 2 : 3, 6.6) * 19;
  // The snippet only renders when there is no visual to carry the card.
  const body = !hasVisual && snippet ? 6 + lines(snippet, 4, 6) * 18 : 0;
  const error = s.status === "error" ? 22 : 0;
  return visual + PADDING + title + body + CAPTION + error;
}

/** Shared card chrome: the hover-revealed ⋯ menu (also the right-click
 *  target — RowMenu listens on the nearest .group). */
function CardMenu({
  label,
  items,
  onOpen,
}: {
  label: string;
  items: React.ComponentProps<typeof RowMenu>["items"];
  onOpen?: () => void;
}) {
  return (
    <div className="absolute right-1.5 top-1.5 z-20 rounded-md bg-surface/80 opacity-0 backdrop-blur-sm transition group-hover:opacity-100 group-focus-within:opacity-100">
      <RowMenu label={label} items={items} onOpen={onOpen} />
    </div>
  );
}

/** A folder-like parent: a peek at its contents, click to drill in. */
function FolderCard({
  source: s,
  childrenSources,
  onOpen,
  menuItems,
  onHover,
  onLeave,
}: {
  source: Source;
  childrenSources: Source[];
  onOpen: () => void;
  menuItems: React.ComponentProps<typeof RowMenu>["items"];
  onHover: (e: React.MouseEvent<HTMLElement>) => void;
  onLeave: () => void;
}) {
  const peek = childrenSources.slice(0, 4);
  return (
    <div
      onMouseEnter={onHover}
      onMouseLeave={onLeave}
      className="group relative overflow-hidden rounded-lg border border-border bg-surface transition-colors hover:border-border-strong hover:bg-surface-2"
    >
      <CardAction
        label={`Browse ${s.title}`}
        onClick={onOpen}
        className="z-10 cursor-pointer"
      />
      <CardMenu
        label={`Options for ${s.title}`}
        items={menuItems}
        onOpen={onLeave}
      />
      <div className="p-3">
        <div className="flex items-center gap-1.5">
          {sourceIcon(s.sourceType, s.url)}
          <span className="truncate text-body font-medium text-foreground">
            {s.title}
          </span>
        </div>
        {peek.length > 0 && (
          <div className="mt-2 grid grid-cols-2 gap-1">
            {peek.map((c) => (
              <div
                key={c.id}
                className="flex min-w-0 items-center gap-1 rounded border border-border bg-background/40 px-1.5 py-1"
              >
                <span className="shrink-0">
                  {sourceIcon(c.sourceType, c.url)}
                </span>
                <span className="truncate text-micro text-muted-foreground">
                  {c.title}
                </span>
              </div>
            ))}
          </div>
        )}
        <div className="mt-1.5 text-caption text-subtle-foreground">
          {childrenSources.length}{" "}
          {childrenSources.length === 1 ? "item" : "items"} ·{" "}
          {relativeTime(s.createdAt)}
        </div>
      </div>
    </div>
  );
}

function GalleryCard({
  source: s,
  snippet,
  menuItems,
  onHover,
  onLeave,
}: {
  source: Source;
  snippet?: string;
  menuItems: React.ComponentProps<typeof RowMenu>["items"];
  onHover: (e: React.MouseEvent<HTMLElement>) => void;
  onLeave: () => void;
}) {
  const openSourceViewer = useStore((st) => st.openSourceViewer);
  const web = isWebUrl(s.url);
  const leadImage =
    s.sourceType === "url" && s.imageUrl !== "" && s.imageUrl !== "-"
      ? s.imageUrl
      : null;
  // PDFs, images, and og-imaged pages resolve their visual through the
  // thumbnail command (disk-cached in Rust) with a session memory in front;
  // null = pending, "" = checked-none.
  const wantsThumb =
    s.sourceType === "pdf" || s.sourceType === "image" || leadImage !== null;
  const [thumb, setThumb] = useState<string | null>(
    () => thumbMemory.get(s.id) ?? null,
  );
  const [imgFailed, setImgFailed] = useState(false);
  // Fetch only once the card is near the viewport, and only a few at a time
  // (enqueueThumb) — scrolling a big gallery streams thumbnails in instead
  // of stampeding the backend on mount.
  const rootRef = useRef<HTMLDivElement>(null);
  const [nearViewport, setNearViewport] = useState(false);
  useEffect(() => {
    if (!wantsThumb || thumbMemory.has(s.id) || nearViewport) return;
    const el = rootRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setNearViewport(true);
          obs.disconnect();
        }
      },
      { rootMargin: "300px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [s.id, wantsThumb, nearViewport]);
  useEffect(() => {
    if (!wantsThumb || !nearViewport || thumbMemory.has(s.id)) return;
    let stale = false;
    enqueueThumb(() =>
      api
        .sourceThumbnail(s.id)
        .then((uri) => {
          thumbMemory.set(s.id, uri);
          if (!stale) setThumb(uri);
        })
        .catch(() => {
          if (!stale) setThumb("");
        }),
    );
    return () => {
      stale = true;
    };
  }, [s.id, wantsThumb, nearViewport]);

  // While the cache fills (or when the download failed), fall back to the
  // remote og URL so first paint isn't gated on the round-trip.
  const visual = !imgFailed ? thumb || leadImage || null : null;
  const host = web ? urlHost(s.url) : null;
  // The icon already says what it is — the caption carries provenance
  // (domain or author) and freshness, never a redundant type label.
  const caption = [host ?? s.author, relativeTime(s.createdAt)]
    .filter(Boolean)
    .join(" · ");

  return (
    <div
      ref={rootRef}
      onMouseEnter={onHover}
      onMouseLeave={onLeave}
      className={cn(
        "group relative overflow-hidden rounded-lg border border-border bg-surface transition-colors",
        "hover:border-border-strong hover:bg-surface-2",
      )}
    >
      <CardAction
        label={`Open ${s.title}`}
        onClick={() => openSourceViewer(s.id, s.title)}
        className="z-10 cursor-pointer"
      />
      <CardMenu
        label={`Options for ${s.title}`}
        items={menuItems}
        onOpen={onLeave}
      />
      {visual && (
        <img
          src={visual}
          alt=""
          loading="lazy"
          onError={() => setImgFailed(true)}
          className={cn(
            "w-full border-b border-border object-cover",
            // First PDF pages are portrait documents — crop to a calmer
            // ratio; photos and og:images keep their own shape (capped).
            s.sourceType === "pdf" ? "aspect-[4/3] object-top" : "max-h-64",
          )}
        />
      )}
      <div className="p-3">
        <div
          className={cn(
            "text-body font-medium text-foreground",
            visual ? "line-clamp-2" : "line-clamp-3",
          )}
        >
          {s.title}
        </div>
        {!visual && snippet && (
          <div className="mt-1.5 line-clamp-4 whitespace-pre-line text-caption leading-relaxed text-muted-foreground">
            {snippet}
          </div>
        )}
        <div className="mt-1.5 flex items-center gap-1.5 text-caption text-subtle-foreground">
          {web ? <Favicon url={s.url} /> : sourceIcon(s.sourceType, s.url)}
          <span className="truncate">{caption}</span>
        </div>
        {s.status === "error" && (
          <div className="mt-1 text-caption text-destructive">
            Import failed
          </div>
        )}
      </div>
    </div>
  );
}
