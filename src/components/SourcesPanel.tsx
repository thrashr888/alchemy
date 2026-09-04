import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { removeSourcesGuarded, useStore } from "@/lib/store";
import { api } from "@/lib/api";
import {
  Badge,
  Button,
  EmptyState,
  LoadingState,
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
  folderCloudProvider,
  isWebUrl,
  relativeTime,
  shortcutBlocked,
  visibleTitle,
} from "@/lib/utils";
import { sourceIcon } from "@/lib/sourceIcon";
import { sourceSubtree } from "@/lib/sourceRows";
import {
  FOLDER_TYPES,
  KIND_LABEL,
  kindCounts as countKinds,
  liveFacet,
  missingSourceIds,
  sourceKind,
  tagCounts as countTags,
  type SourceKind,
} from "@/lib/sourceFacets";
import { CloudMark } from "./CloudMarks";
import { HYGIENE_LABEL, loadHygieneKept } from "@/lib/growth";
import { useSourceActions } from "./SourceMenu";
import { ArrivalsStrip, useArrivals } from "./ArrivalsStrip";
import { OkfBadges } from "./OkfBadges";
import type { GrowthProposal, Source } from "@/lib/types";
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
  RefreshCw,
  Cloud,
  Search,
  Sprout,
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

/** Condition facets. Freshness is a fixed set — unlike kinds and tags those
 *  chips are always there to switch off. "missing" is the exception: it only
 *  offers itself when something is actually missing, so it clears itself the
 *  way a kind chip does. */
type FreshFacet = "week" | "month" | "stale" | "uncited" | "missing";

/** One row of the panel: a source (nested under its folder by `depth`) or a
 *  domain rollup grouping loose web sources from one busy host. */
type SourceRow =
  | { kind: "source"; s: Source; depth: number }
  | { kind: "group"; host: string; kids: Source[] };

/** Indent stops, in rem so nesting tracks the OS text size like everything
 *  else. Past six the panel is narrower than the indent; deeper rows share
 *  the last stop rather than walking off the edge. */
const INDENT_REM = 1.25;
const MAX_INDENT = 6;

function hostname(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/** Compact selection checkbox; supports the folder/master indeterminate state.
 *  Clicks stop propagating so the row's open-reader handler never fires. */
/** One compact facet toggle under the filter box. */
function FacetChip({
  active,
  onClick,
  title,
  children,
}: {
  active: boolean;
  onClick: () => void;
  title?: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      aria-pressed={active}
      className={cn(
        "rounded-full border px-2 py-0.5 text-micro transition-colors",
        active
          ? "border-primary/50 bg-primary/15 text-citation"
          : "border-border text-muted-foreground hover:bg-surface-2",
      )}
    >
      {children}
    </button>
  );
}

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
  const notebookLoading = useStore((s) => s.notebookLoading);
  const currentId = useStore((s) => s.currentId);
  const queue = useStore((s) => s.ingestQueue);
  const importingFolders = useStore((s) => s.importingFolders);
  const clearQueueItem = useStore((s) => s.clearQueueItem);
  const openAddSource = useStore((s) => s.openAddSource);
  const folderScan = useStore((s) => s.folderScan);
  const draggingFiles = useStore((s) => s.draggingFiles);
  const toggleSources = useStore((s) => s.toggleSources);
  const openSourceViewer = useStore((s) => s.openSourceViewer);
  const selectedSourceIds = useStore((s) => s.selectedSourceIds);
  const toggleSourceSelected = useStore((s) => s.toggleSourceSelected);
  const setAllSourcesSelected = useStore((s) => s.setAllSourcesSelected);
  const picked = useStore((s) => s.picked);
  const pickOne = useStore((s) => s.pickOne);
  const pickToggle = useStore((s) => s.pickToggle);
  const pickRange = useStore((s) => s.pickRange);
  const pickSet = useStore((s) => s.pickSet);
  const clearPicked = useStore((s) => s.clearPicked);
  const refreshSourcesBatch = useStore((s) => s.refreshSourcesBatch);
  const hygiene = useStore((s) => s.hygiene);
  const refreshHygiene = useStore((s) => s.refreshHygiene);
  // Arrivals (RFC-events §6): what the watchers saw since the last dismiss.
  const arrivals = useArrivals(currentId);
  const { confirm, dialog: confirmDialog } = useConfirm();

  // One source, one menu: the row verbs and every modal they open come
  // from the shared source actions (SourceMenu.tsx).
  const actions = useSourceActions();
  const { setTagEdit } = actions;

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

  // Growth tray (RFC-living-notebook Pillar 2): frontier links mined from
  // the notebook's own sources, loaded once per notebook. Nothing fetches
  // until the user accepts a proposal.
  const [growth, setGrowth] = useState<GrowthProposal[]>([]);
  // Dismissals come from the store (per-notebook, persisted there) so the
  // Grow pane clearing its last proposal also drops this door live.
  const growthDismissed = useStore((s) => s.growthDismissed);
  // The frontier is mined from source text, so it moves whenever a source
  // finishes importing — re-read on the ready count, not just the notebook,
  // or the door's number drifts from what the Grow pane shows after a batch
  // of adds.
  const readyCount = sources.filter((s) => s.status === "ready").length;
  useEffect(() => {
    if (!currentId) {
      setGrowth([]);
      return;
    }
    let stale = false;
    api
      .growthProposals(currentId)
      .then((overview) => {
        if (!stale) setGrowth(overview.proposals);
      })
      .catch(() => undefined);
    return () => {
      stale = true;
    };
  }, [currentId, readyCount]);
  const existingUrls = useMemo(
    () => new Set(sources.map((s) => s.url).filter(Boolean)),
    [sources],
  );
  const growthVisible = growth.filter(
    (p) => !growthDismissed[p.url] && !existingUrls.has(p.url),
  );

  // Search-first navigation (RFC-living-notebook Pillar 1): past a handful
  // of sources the filter box is the way in. Facets narrow by kind, tag,
  // and freshness; everything applies before rows are built.
  const [query, setQuery] = useState("");
  const [kindFacet, setKindFacet] = useState<SourceKind | null>(null);
  const [tagFacet, setTagFacet] = useState<string | null>(null);
  const [freshFacet, setFreshFacet] = useState<FreshFacet | null>(null);
  // The uncited facet's data loads on first use: source ids that have ever
  // come back as citations (retrieval traces, months of history).
  const [citedIds, setCitedIds] = useState<Set<string> | null>(null);
  useEffect(() => {
    if (freshFacet !== "uncited" || citedIds) return;
    api
      .citedSourceIds()
      .then((ids) => setCitedIds(new Set(ids)))
      .catch(() => setCitedIds(new Set()));
  }, [freshFacet, citedIds]);

  // Facet chips only offer what the notebook holds, with counts — both read
  // off the live source list, so a removal moves them in the same render.
  const kindCounts = useMemo(() => countKinds(sources), [sources]);
  const tagTotals = useMemo(
    () => new Set(sources.flatMap((s) => s.tags.split(" ").filter(Boolean))),
    [sources],
  );
  // Removing the last source under a facet used to leave the panel filtered
  // by a chip that no longer rendered — a filter with no way out. A facet
  // holds only while the list still offers it (alchemy-release-zhk).
  const liveKind = liveFacet(kindFacet, kindCounts);
  const liveTag = liveFacet(tagFacet, tagTotals);
  // Sources whose bytes aren't here — deleted out from under the notebook,
  // or a cloud stub still in the cloud (RFC-okf-live §5.7).
  const missingIds = useMemo(
    () => missingSourceIds(sources, hygiene),
    [sources, hygiene],
  );
  const liveFresh =
    freshFacet === "missing" && missingIds.size === 0 ? null : freshFacet;
  useEffect(() => {
    if (kindFacet !== null && liveKind === null) setKindFacet(null);
    if (tagFacet !== null && liveTag === null) setTagFacet(null);
    if (freshFacet !== null && liveFresh === null) setFreshFacet(null);
  }, [kindFacet, tagFacet, freshFacet, liveKind, liveTag, liveFresh]);
  const tagChips = useMemo(
    () => countTags(sources, liveTag),
    [sources, liveTag],
  );

  const q = query.trim().toLowerCase();
  const filterActive = !!q || !!liveKind || !!liveTag || !!liveFresh;
  const clearFilters = () => {
    setQuery("");
    setKindFacet(null);
    setTagFacet(null);
    setFreshFacet(null);
  };
  const matchesFilters = (s: Source): boolean => {
    if (liveKind && sourceKind(s) !== liveKind) return false;
    if (liveTag && !s.tags.split(" ").includes(liveTag)) return false;
    if (liveFresh) {
      if (liveFresh === "missing") {
        if (!missingIds.has(s.id)) return false;
      } else if (liveFresh === "uncited") {
        // Folder containers carry no chunks, so "uncited" would flag every
        // one of them; the facet is about content sources.
        if (FOLDER_TYPES.includes(s.sourceType)) return false;
        if (!citedIds || citedIds.has(s.id)) return false;
      } else {
        const age = Date.now() - (s.fetchedAt || s.createdAt);
        if (liveFresh === "week" && age > 7 * 86_400_000) return false;
        if (liveFresh === "month" && age > 30 * 86_400_000) return false;
        if (liveFresh === "stale" && age <= 30 * 86_400_000) return false;
      }
    }
    if (q) {
      const hay =
        `${s.title} ${s.url} ${s.tags} ${s.author}`.toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  };

  // Rows: folder children indent under their parent, however deep the tree
  // goes; loose web sources from a busy domain fold into a group row
  // (Pillar 1 rollups) at rest — an active filter answers its own question,
  // so it shows flat matches.
  const rows: SourceRow[] = [];
  const looseByHost = new Map<string, Source[]>();
  if (!filterActive) {
    for (const s of sources) {
      if (s.parentId || s.sourceType !== "url" || !s.url) continue;
      const host = hostname(s.url);
      if (!host) continue;
      const list = looseByHost.get(host);
      if (list) list.push(s);
      else looseByHost.set(host, [s]);
    }
  }
  const hostEmitted = new Set<string>();
  // A folder and everything under it, however deep — an OKF bundle restores
  // whatever parent chain it was exported with, so a folder can sit inside a
  // folder, and the chevron on those inner rows used to rotate over a
  // subtree that was never a candidate for a row (alchemy-release-dbk).
  const subtree = (s: Source) =>
    sourceSubtree(s, childrenOf, {
      collapsed: (x, kidCount) => isCollapsed(x.id, kidCount),
      matches: filterActive ? matchesFilters : undefined,
    });
  for (const s of sources) {
    if (s.parentId) continue;
    if (FOLDER_TYPES.includes(s.sourceType)) {
      for (const r of subtree(s))
        rows.push({ kind: "source", s: r.s, depth: r.depth });
      continue;
    }
    const host =
      !filterActive && s.sourceType === "url" && s.url ? hostname(s.url) : "";
    const hostKids = host ? (looseByHost.get(host) ?? []) : [];
    if (hostKids.length >= 5) {
      if (hostEmitted.has(host)) continue;
      hostEmitted.add(host);
      rows.push({ kind: "group", host, kids: hostKids });
      if (!isCollapsed(`domain:${host}`, hostKids.length))
        for (const c of hostKids) rows.push({ kind: "source", s: c, depth: 1 });
      continue;
    }
    if (filterActive && !matchesFilters(s)) continue;
    rows.push({ kind: "source", s, depth: 0 });
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
    (s) => !FOLDER_TYPES.includes(s.sourceType),
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
  const rowIds = rows.flatMap((r) => (r.kind === "source" ? [r.s.id] : []));

  const rowIdsRef = useRef(rowIds);
  rowIdsRef.current = rowIds;

  const listRef = useRef<HTMLDivElement>(null);
  // Windowed rendering (Pillar 1): only the visible slice of rows mounts —
  // a 10k-source notebook renders dozens of row components, not thousands.
  // Heights are measured (error rows and folder meta run taller than 44px).
  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => 44,
    overscan: 12,
    getItemKey: (i) => {
      const r = rows[i];
      return r.kind === "group" ? `domain:${r.host}` : r.s.id;
    },
  });
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
    // Undo beats confirm: restorable sources delete straight away with a
    // click-to-undo toast; only connector sources still ask.
    await removeSourcesGuarded(ids, confirm);
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

  const issueBySource = useMemo(() => {
    const kept = loadHygieneKept(currentId);
    const m = new Map<string, (typeof hygiene)[number]>();
    for (const h of hygiene) {
      if (h.kind === "note") continue; // no row here to badge
      if (h.bucket === "stale") continue; // the sweep's job, not the user's
      if (kept[`${h.sourceId}:${h.bucket}`]) continue;
      if (!m.has(h.sourceId)) m.set(h.sourceId, h);
    }
    return m;
    // Keeps write localStorage; refreshHygiene() afterwards replaces the
    // store array, which re-runs this memo against the fresh keeps.
  }, [hygiene, currentId]);
  const proposals = [...issueBySource.values()];
  // Flagged notes have no row in this panel, but they are part of what the
  // Grow door offers to review, so they count toward its badge.
  const flaggedNotes = useMemo(() => {
    const kept = loadHygieneKept(currentId);
    return hygiene.filter(
      (h) => h.kind === "note" && !kept[`${h.sourceId}:${h.bucket}`],
    ).length;
  }, [hygiene, currentId]);

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
                // Fallback fill: a notebook with no color (imports, fixtures)
                // must still draw a bar — primary, not transparent.
                pct > 90 ? "bg-destructive" : !notebookColor && "bg-primary",
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

      {/* Search-first navigation: past a handful of sources the filter box
          is the primary way in; chips narrow by kind, tag, condition, and
          citation history. Hidden in small notebooks — eight rows filter
          themselves — but never while a filter is on: removing sources down
          past that threshold used to take the only way to switch it off
          along with it. */}
      {currentId && (sources.length > 8 || filterActive) && (
        <div className="flex flex-col gap-1.5 border-b border-border px-3 py-2">
          <div className="relative">
            <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtle-foreground" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Filter sources…"
              aria-label="Filter sources"
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="none"
              spellCheck={false}
              {...({ writingsuggestions: "false" } as Record<string, string>)}
              className="w-full rounded-md border border-input bg-transparent py-1 pl-7 pr-6 text-caption text-foreground outline-none placeholder:text-subtle-foreground focus:border-ring/60"
            />
            {query && (
              <button
                type="button"
                onClick={() => setQuery("")}
                aria-label="Clear filter"
                className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-subtle-foreground hover:text-foreground"
              >
                <X className="h-3 w-3" />
              </button>
            )}
          </div>
          <div className="flex flex-wrap gap-1">
            {[...kindCounts.entries()]
              .filter(([, n]) => n > 0)
              .map(([kind, n]) => (
                <FacetChip
                  key={kind}
                  active={liveKind === kind}
                  onClick={() => setKindFacet(liveKind === kind ? null : kind)}
                >
                  {KIND_LABEL[kind]} {n}
                </FacetChip>
              ))}
            {tagChips.map(([tag, n]) => (
              <FacetChip
                key={`#${tag}`}
                active={liveTag === tag}
                onClick={() => setTagFacet(liveTag === tag ? null : tag)}
              >
                #{tag} {n}
              </FacetChip>
            ))}
            {/* Only offered when something is actually missing — the chip
                carries its count, the way the kind chips do. */}
            {missingIds.size > 0 && (
              <FacetChip
                active={liveFresh === "missing"}
                title="File moved or deleted, or still in the cloud"
                onClick={() =>
                  setFreshFacet(liveFresh === "missing" ? null : "missing")
                }
              >
                Missing {missingIds.size}
              </FacetChip>
            )}
            {(
              [
                ["week", "7d", "Fetched or added this week"],
                ["month", "30d", "Fetched or added this month"],
                ["stale", "Stale", "Untouched for over 30 days"],
                ["uncited", "Uncited", "Never came back as a citation"],
              ] as const
            ).map(([id, label, title]) => (
              <FacetChip
                key={id}
                active={liveFresh === id}
                title={title}
                onClick={() => setFreshFacet(liveFresh === id ? null : id)}
              >
                {label}
              </FacetChip>
            ))}
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
        ) : notebookLoading && sources.length === 0 && queue.length === 0 ? (
          <LoadingState label="Loading sources…" />
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
            {/* What changed since the reader last looked — one line of
                tallies, the events on click, gone on "Mark seen". */}
            <ArrivalsStrip unseen={arrivals.unseen} onDismiss={arrivals.dismiss} />
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
            {/* One door to the Grow surface (RFC-living-notebook):
                growth proposals and needs-attention flags together, with
                an activity dot when anything is waiting. */}
            {growthVisible.length + proposals.length + flaggedNotes > 0 && (
              <button
                type="button"
                onClick={() =>
                  useStore.setState({
                    growOpen: true,
                    galleryOpen: false,
                    ledgerOpen: false,
                  })
                }
                className="mb-1 flex w-full items-center gap-2 rounded-md border border-border bg-surface-2/60 px-2 py-1.5 text-left hover:bg-surface-2"
              >
                <Sprout className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                <span className="truncate text-caption text-foreground">
                  Grow this notebook
                </span>
                <span
                  aria-hidden
                  className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                />
                <span className="ml-auto shrink-0 text-caption text-subtle-foreground">
                  {growthVisible.length + proposals.length + flaggedNotes} · Review
                </span>
              </button>
            )}
            {filterActive && rows.length === 0 && (
              <EmptyState title="No sources match">
                <Button variant="ghost" size="sm" onClick={clearFilters}>
                  Clear filters
                </Button>
              </EmptyState>
            )}
            <div
              className="relative"
              style={{ height: rowVirtualizer.getTotalSize() }}
            >
              {rowVirtualizer.getVirtualItems().map((vi) => {
                const row = rows[vi.index];
                const itemStyle = {
                  position: "absolute" as const,
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${vi.start}px)`,
                };
                if (row.kind === "group") {
                  const { host, kids: groupKids } = row;
                  const gid = `domain:${host}`;
                  const onCount = groupKids.filter((k) =>
                    isSelected(k.id),
                  ).length;
                  const collapsed = isCollapsed(gid, groupKids.length);
                  return (
                    <div
                      key={vi.key}
                      data-index={vi.index}
                      ref={rowVirtualizer.measureElement}
                      style={itemStyle}
                      className="pb-0.5"
                    >
                      <div className="group relative flex items-center gap-2 rounded-md px-2 py-2 hover:bg-surface-2">
                        <button
                          type="button"
                          onClick={() =>
                            toggleCollapsed(gid, groupKids.length)
                          }
                          aria-expanded={!collapsed}
                          aria-label={
                            collapsed
                              ? `Show ${groupKids.length} sources from ${host}`
                              : `Hide sources from ${host}`
                          }
                          className="relative z-20 shrink-0 cursor-pointer"
                        >
                          <span className="group-hover:hidden group-focus-within:hidden">
                            <Favicon url={`https://${host}`} />
                          </span>
                          <ChevronRight
                            className={cn(
                              "hidden h-3.5 w-3.5 text-muted-foreground transition-transform duration-150 group-hover:block group-focus-within:block",
                              !collapsed && "rotate-90",
                            )}
                          />
                        </button>
                        <span
                          className="min-w-0 flex-1 truncate text-body text-foreground"
                          title={`${groupKids.length} sources from ${host}`}
                        >
                          {host}
                        </span>
                        <span className="shrink-0 text-micro text-subtle-foreground">
                          {groupKids.length}
                        </span>
                        <SelectBox
                          checked={
                            groupKids.length > 0 &&
                            onCount === groupKids.length
                          }
                          indeterminate={
                            onCount > 0 && onCount < groupKids.length
                          }
                          onToggle={() => {
                            const want = onCount !== groupKids.length;
                            for (const k of groupKids)
                              if (isSelected(k.id) !== want)
                                toggleSourceSelected(k.id);
                          }}
                          label={`Include ${host} sources in chat & generation`}
                        />
                      </div>
                    </div>
                  );
                }
                const { s, depth } = row;
                const isFolder = FOLDER_TYPES.includes(s.sourceType);
                // Which sync root a folder came out of, if any — same
                // provider keys the Add sources dialog draws marks from.
                const cloud = isFolder ? folderCloudProvider(s.url) : null;
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
                    key={vi.key}
                    data-index={vi.index}
                    ref={rowVirtualizer.measureElement}
                    style={itemStyle}
                    className="pb-0.5"
                  >
                  <div
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
                    )}
                    style={
                      depth > 0
                        ? {
                            marginLeft: `${Math.min(depth, MAX_INDENT) * INDENT_REM}rem`,
                          }
                        : undefined
                    }
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
                        {/* New-dot: an unseen arrival landed here (folder
                            children roll up to their parent). Cleared by
                            the strip's Mark seen. */}
                        {arrivals.sourceIds.has(s.id) && (
                          <span
                            aria-label="Changed since you last looked"
                            title="Changed since you last looked"
                            className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                          />
                        )}
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
                            items={actions.items(s)}
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
                          {cloud && (
                            <span className="flex shrink-0 items-center gap-1 rounded bg-surface-2 px-1.5 py-px text-caption text-muted-foreground">
                              <CloudMark
                                provider={cloud.key}
                                className="h-3 w-3"
                              />
                              {cloud.label}
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
                      {/* What a bundle says about this concept's standing
                          (RFC-okf-live §4). Renders nothing for every source
                          that isn't part of one. */}
                      <div className="flex flex-wrap items-center gap-1 empty:hidden">
                        <OkfBadges sourceId={s.id} />
                      </div>
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
                  </div>
                );
              })}
            </div>
          </>
        )}
      </div>

      {actions.modals}

      {marquee}
      {confirmDialog}
      {hoverCard}
    </div>
  );
}

