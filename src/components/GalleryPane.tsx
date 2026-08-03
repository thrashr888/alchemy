import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type { Source } from "@/lib/types";
import {
  Button,
  CardAction,
  EmptyState,
  RowMenu,
  useConfirm,
  useHoverCard,
} from "./ui";
import { cn, isWebUrl, relativeTime, urlHost } from "@/lib/utils";
import { Favicon, sourceHoverData } from "./SourcesPanel";
import { sourceIcon } from "@/lib/sourceIcon";
import {
  ArrowLeft,
  ExternalLink,
  FolderOpen,
  LayoutGrid,
  Link2,
  Pencil,
  RefreshCw,
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

/** Scroll positions per notebook+level, so Reader round-trips and folder
 *  drill-ins come back to the same place. App-run lifetime is the point. */
const scrollMemory = new Map<string, number>();

/** Resolved card visuals (data URIs) per source id, so reopening the
 *  gallery paints instantly instead of re-running IPC + image fetches.
 *  "" = checked, none. The backend also disk-caches og downloads. */
const thumbMemory = new Map<string, string>();

type SortMode = "recent" | "title";
type TypeGroup =
  | "all"
  | "pages"
  | "docs"
  | "images"
  | "text"
  | "code"
  | "mac"
  | "folders";

const GROUP_OF: Record<Source["sourceType"], Exclude<TypeGroup, "all">> = {
  url: "pages",
  html: "text",
  pdf: "docs",
  image: "images",
  text: "text",
  markdown: "text",
  code: "code",
  mac: "mac",
  folder: "folders",
  git: "folders",
  notion: "folders",
  obsidian: "folders",
};

const GROUP_LABEL: Record<Exclude<TypeGroup, "all">, string> = {
  pages: "Pages",
  docs: "PDFs",
  images: "Images",
  text: "Text",
  code: "Code",
  mac: "Mac",
  folders: "Folders",
};

/** Kinds whose card leads with opening lines of the text. URL sources join
 *  when their page yielded no lead image. */
function wantsSnippet(s: Source): boolean {
  if (["text", "markdown", "html", "code", "mac"].includes(s.sourceType))
    return true;
  return s.sourceType === "url" && (s.imageUrl === "" || s.imageUrl === "-");
}

export function GalleryPane() {
  const currentId = useStore((s) => s.currentId);
  const sources = useStore((s) => s.sources);
  const { confirm, dialog: confirmDialog } = useConfirm();
  const { show: showCard, hide: hideCard, card: hoverCard } = useHoverCard("right");
  const [sweeping, setSweeping] = useState(false);
  const [folderId, setFolderId] = useState<string | null>(null);
  const [filter, setFilter] = useState<TypeGroup>("all");
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

  // Notebook switch resets the drill-in; a deleted folder falls back to root.
  useEffect(() => {
    setFolderId(null);
    setFilter("all");
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
      ["pages", "docs", "images", "text", "code", "mac", "folders"] as const
    ).filter((g) => present.has(g)),
  ];
  const effectiveFilter = groups.includes(filter) ? filter : "all";
  const cards = level
    .filter(
      (s) => effectiveFilter === "all" || GROUP_OF[s.sourceType] === effectiveFilter,
    )
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
  // (cards were clickable yet invisible). Round-robin keeps sort order
  // roughly row-wise.
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
  const colCount = Math.min(4, Math.max(1, Math.floor((width + 12) / 232)));
  const columns: Source[][] = Array.from({ length: colCount }, () => []);
  cards.forEach((s, i) => columns[i % colCount].push(s));

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
  const cardMenuItems = (s: Source) => {
    const st = useStore.getState();
    const web = isWebUrl(s.url);
    const editable =
      !["url", "mac", "folder", "git", "notion", "obsidian"].includes(
        s.sourceType,
      ) && s.status !== "placeholder";
    return [
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
      ...(s.url && web
        ? [
            {
              label: "Open original",
              icon: <ExternalLink className="h-3.5 w-3.5" />,
              onClick: () => void openUrl(s.url),
            },
          ]
        : []),
      ...(s.url && !web && s.sourceType !== "mac"
        ? [
            {
              label: "Show in Finder",
              icon: <FolderOpen className="h-3.5 w-3.5" />,
              onClick: () => void revealItemInDir(s.url),
            },
          ]
        : []),
      ...(s.url && s.sourceType !== "mac"
        ? [
            {
              label: web ? "Copy URL" : "Copy file path",
              icon: <Link2 className="h-3.5 w-3.5" />,
              onClick: () => {
                void navigator.clipboard
                  .writeText(s.url)
                  .then(() =>
                    useStore.getState().pushToast("success", "Copied"),
                  );
              },
            },
          ]
        : []),
      {
        label: "Remove…",
        icon: <Trash2 className="h-3.5 w-3.5" />,
        danger: true,
        onClick: async () => {
          if (
            await confirm({
              title: `Remove "${s.title}"?`,
              message:
                GROUP_OF[s.sourceType] === "folders"
                  ? "This removes the folder and everything under it from the notebook."
                  : "This removes the source and its indexed content from the notebook.",
              confirmLabel: "Remove",
              danger: true,
            })
          )
            void useStore.getState().deleteSource(s.id);
        },
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
        <div className="ml-auto flex shrink-0 items-center gap-0.5 rounded-lg border border-border p-0.5">
          {(["recent", "title"] as const).map((mode) => (
            <button
              key={mode}
              type="button"
              onClick={() => setSortMode(mode)}
              aria-pressed={sort === mode}
              title={mode === "recent" ? "Newest first" : "A to Z"}
              className={cn(
                "rounded-md px-2 py-0.5 text-micro font-medium capitalize transition-colors",
                sort === mode
                  ? "bg-surface-2 text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {mode}
            </button>
          ))}
        </div>
      </div>
      {groups.length > 2 && (
        <div className="flex shrink-0 items-center gap-1 border-b border-border px-4 py-1.5">
          {groups.map((g) => (
            <button
              key={g}
              type="button"
              onClick={() => setFilter(g)}
              aria-pressed={effectiveFilter === g}
              className={cn(
                "rounded-md px-2 py-1 text-caption transition-colors",
                effectiveFilter === g
                  ? "bg-surface-2 font-medium text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {g === "all" ? "All" : GROUP_LABEL[g]}
            </button>
          ))}
        </div>
      )}
      {cards.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<LayoutGrid className="h-5 w-5" />}
            title={
              effectiveFilter === "all"
                ? "Nothing to explore yet"
                : "Nothing of this type here"
            }
            hint={
              effectiveFilter === "all"
                ? "Add sources and they'll appear here as cards."
                : undefined
            }
          />
        </div>
      ) : (
        <div
          ref={attachScroller}
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
                {col.map((s) =>
                  GROUP_OF[s.sourceType] === "folders" ? (
                    <FolderCard
                      key={s.id}
                      source={s}
                      childrenSources={sources.filter(
                        (c) => c.parentId === s.id,
                      )}
                      onOpen={() => setFolderId(s.id)}
                      menuItems={cardMenuItems(s)}
                      onHover={(e) => showCard(e, sourceHoverData(s))}
                      onLeave={hideCard}
                    />
                  ) : (
                    <GalleryCard
                      key={s.id}
                      source={s}
                      snippet={snippets[s.id]}
                      menuItems={cardMenuItems(s)}
                      onHover={(e) => showCard(e, sourceHoverData(s))}
                      onLeave={hideCard}
                    />
                  ),
                )}
              </div>
            ))}
          </div>
          )}
        </div>
      )}
      {confirmDialog}
      {hoverCard}
    </div>
  );
}

/** Shared card chrome: the hover-revealed ⋯ menu (also the right-click
 *  target — RowMenu listens on the nearest .group). */
function CardMenu({
  label,
  items,
}: {
  label: string;
  items: React.ComponentProps<typeof RowMenu>["items"];
}) {
  return (
    <div className="absolute right-1.5 top-1.5 z-20 rounded-md bg-surface/80 opacity-0 backdrop-blur-sm transition group-hover:opacity-100 group-focus-within:opacity-100">
      <RowMenu label={label} items={items} />
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
      <CardMenu label={`Options for ${s.title}`} items={menuItems} />
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
                <span className="shrink-0">{sourceIcon(c.sourceType, c.url)}</span>
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
  useEffect(() => {
    if (!wantsThumb || thumbMemory.has(s.id)) return;
    let stale = false;
    void api
      .sourceThumbnail(s.id)
      .then((uri) => {
        thumbMemory.set(s.id, uri);
        if (!stale) setThumb(uri);
      })
      .catch(() => {
        if (!stale) setThumb("");
      });
    return () => {
      stale = true;
    };
  }, [s.id, wantsThumb]);

  // While the cache fills (or when the download failed), fall back to the
  // remote og URL so first paint isn't gated on the round-trip.
  const visual = !imgFailed ? (thumb || leadImage || null) : null;
  const host = web ? urlHost(s.url) : null;
  // The icon already says what it is — the caption carries provenance
  // (domain or author) and freshness, never a redundant type label.
  const caption = [host ?? s.author, relativeTime(s.createdAt)]
    .filter(Boolean)
    .join(" · ");

  return (
    <div
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
      <CardMenu label={`Options for ${s.title}`} items={menuItems} />
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
