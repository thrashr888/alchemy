import { useEffect, useRef, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type { Source } from "@/lib/types";
import { CardAction, EmptyState } from "./ui";
import { cn, isWebUrl, relativeTime, urlHost } from "@/lib/utils";
import { Favicon } from "./SourcesPanel";
import { SOURCE_TYPE_LABEL, useElementWidth } from "./ReaderPane";
import { sourceIcon } from "@/lib/sourceIcon";
import { LayoutGrid } from "lucide-react";

/* The source Gallery (docs/RFC-source-gallery.md): the notebook's sources as
 * a masonry of visual cards — a mymind/are.na-style browse surface beside
 * Chat, Reader, and Ledger. Scraped pages lead with their og:image, PDFs
 * with their first page, images with themselves; the rest are typographic. */

/** Notebooks already swept for lead images this app run — the "-" sentinel
 *  guards across runs, this guards within one. */
const sweptNotebooks = new Set<string>();

export function GalleryPane() {
  const currentId = useStore((s) => s.currentId);
  const sources = useStore((s) => s.sources);
  const [sweeping, setSweeping] = useState(false);

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

  // Newest first — a browse surface leads with what just arrived.
  const cards = [...sources].sort((a, b) => b.createdAt - a.createdAt);

  // Masonry as JS-bucketed flex columns, not CSS multicol — WKWebView
  // reliably hit-tests but does NOT reliably paint later multicol columns
  // (cards were clickable yet invisible). Round-robin keeps newest-first
  // order roughly row-wise.
  const scrollerRef = useRef<HTMLDivElement>(null);
  const width = useElementWidth(scrollerRef);
  const colCount = Math.min(4, Math.max(1, Math.floor((width + 12) / 232)));
  const columns: Source[][] = Array.from({ length: colCount }, () => []);
  cards.forEach((s, i) => columns[i % colCount].push(s));

  return (
    <div className="flex h-full flex-1 flex-col min-w-0">
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-4">
        <LayoutGrid className="h-4 w-4 text-muted-foreground" />
        <span className="text-body font-medium text-foreground">Gallery</span>
        <span className="text-caption text-subtle-foreground">
          {cards.length} {cards.length === 1 ? "source" : "sources"}
        </span>
        {sweeping && (
          <span className="ml-auto text-caption text-subtle-foreground">
            Fetching page images…
          </span>
        )}
      </div>
      {cards.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<LayoutGrid className="h-5 w-5" />}
            title="Nothing to explore yet"
            hint="Add sources and they'll appear here as cards."
          />
        </div>
      ) : (
        <div
          ref={scrollerRef}
          className="min-h-0 flex-1 overflow-y-auto px-6 py-4"
        >
          <div className="flex items-start gap-3">
            {columns.map((col, i) => (
              <div key={i} className="flex min-w-0 flex-1 flex-col gap-3">
                {col.map((s) => (
                  <GalleryCard key={s.id} source={s} />
                ))}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function GalleryCard({ source: s }: { source: Source }) {
  const openSourceViewer = useStore((st) => st.openSourceViewer);
  const web = isWebUrl(s.url);
  const leadImage =
    s.sourceType === "url" && s.imageUrl !== "" && s.imageUrl !== "-"
      ? s.imageUrl
      : null;
  // PDFs and images fetch their thumbnail lazily; null = pending, "" = none.
  const wantsThumb = s.sourceType === "pdf" || s.sourceType === "image";
  const [thumb, setThumb] = useState<string | null>(null);
  const [imgFailed, setImgFailed] = useState(false);
  useEffect(() => {
    if (!wantsThumb) return;
    let stale = false;
    void api
      .sourceThumbnail(s.id)
      .then((uri) => {
        if (!stale) setThumb(uri);
      })
      .catch(() => {
        if (!stale) setThumb("");
      });
    return () => {
      stale = true;
    };
  }, [s.id, wantsThumb]);

  const visual = !imgFailed ? (leadImage ?? (thumb || null)) : null;
  const host = web ? urlHost(s.url) : null;
  const caption =
    host ??
    (s.author ||
      `${SOURCE_TYPE_LABEL[s.sourceType] ?? s.sourceType} · ${relativeTime(s.createdAt)}`);

  return (
    <div
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
            // Text-only cards give the title more room to breathe.
            visual ? "line-clamp-2" : "line-clamp-4",
          )}
        >
          {s.title}
        </div>
        <div className="mt-1.5 flex items-center gap-1.5 text-caption text-subtle-foreground">
          {web ? (
            <Favicon url={s.url} />
          ) : (
            sourceIcon(s.sourceType, s.url)
          )}
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
