import { useCallback, useEffect, useMemo, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import {
  HYGIENE_LABEL,
  loadHygieneKept,
  saveHygieneKept,
  takeLegacyWebFlag,
} from "@/lib/growth";
import type { GrowthProposal, TagMergeProposal } from "@/lib/types";
import type { GrowSections } from "@/lib/storeTypes";
import { Button, EmptyState, Spinner, Switch, useConfirm } from "./ui";
import { Favicon } from "./SourcesPanel";
import {
  AlertCircle,
  Archive,
  BookOpen,
  Tags,
  FileText,
  Globe,
  RefreshCw,
  Rss,
  Search,
  Sprout,
  StickyNote,
} from "lucide-react";

/** Center-pane growth review (RFC-living-notebook Pillar 2): what the
 *  notebook is hungry for, and everything the free tiers found — files on
 *  this Mac, links its own sources keep citing — plus the opt-in open-web
 *  tier through Firecrawl's keyless search. Every Add is an explicit act;
 *  nothing fetches on its own. */
export function GrowPane() {
  const currentId = useStore((s) => s.currentId);
  const sources = useStore((s) => s.sources);
  const addSourceUrl = useStore((s) => s.addSourceUrl);
  const addSourceFiles = useStore((s) => s.addSourceFiles);
  const hygiene = useStore((s) => s.hygiene);
  const refreshHygiene = useStore((s) => s.refreshHygiene);
  const hygieneKeep = useStore((s) => s.hygieneKeep);
  const refreshSource = useStore((s) => s.refreshSource);
  const deleteSourcesBatch = useStore((s) => s.deleteSourcesBatch);
  const deleteNotesBatch = useStore((s) => s.deleteNotesBatch);
  const pushToast = useStore((s) => s.pushToast);
  const selectedSourceIds = useStore((s) => s.selectedSourceIds);
  const toggleSourceSelected = useStore((s) => s.toggleSourceSelected);
  const notes = useStore((s) => s.notes);
  const { confirm, dialog: confirmDialog } = useConfirm();
  const [retrying, setRetrying] = useState<string | null>(null);
  const [retryingAll, setRetryingAll] = useState(false);
  const [checking, setChecking] = useState(false);
  // Proactive relocation for missing files: Spotlight candidates by exact
  // name, looked up per flagged source. null = still looking.
  const [foundPaths, setFoundPaths] = useState<Record<string, string[]>>({});
  const relocate = async (sourceId: string, path: string) => {
    try {
      await api.relocateSource(sourceId, path);
      useStore.getState().pushToast("success", "Source moved — re-reading it");
      await refreshSource(sourceId);
      await refreshHygiene();
    } catch {
      /* surfaced by the api layer's toast path */
    }
  };
  const locate = async (sourceId: string) => {
    const picked = await openFileDialog({ multiple: false });
    if (typeof picked === "string") void relocate(sourceId, picked);
  };

  // Needs-attention flags (RFC-source-hygiene) — merged into Grow: tending
  // what's broken is the other half of growing (and where Pillar 3's
  // curation passes will land). Sources and notes both land here; a note
  // has no origin, so it gets Keep and Remove and nothing else. Keeps live
  // in localStorage; refreshing hygiene afterwards re-runs this against the
  // fresh keeps.
  const attention = useMemo(() => {
    const kept = loadHygieneKept(currentId);
    const seen = new Map<string, (typeof hygiene)[number]>();
    for (const h of hygiene) {
      if (h.bucket === "stale") continue;
      if (kept[`${h.sourceId}:${h.bucket}`]) continue;
      const key = `${h.kind}:${h.sourceId}`;
      if (!seen.has(key)) seen.set(key, h);
    }
    return [...seen.values()];
  }, [hygiene, currentId]);
  // "Keep" on an unreachable source resets real backend state (the strike
  // count); every other flag is suppressed locally. Returns whether it
  // touched the backend, so the bulk path can wait for those.
  const keepIssue = (h: { sourceId: string; bucket: string }) => {
    if (h.bucket === "unreachable") return hygieneKeep(h.sourceId);
    const kept = loadHygieneKept(currentId);
    kept[`${h.sourceId}:${h.bucket}`] = true;
    saveHygieneKept(currentId, kept);
    return Promise.resolve();
  };
  const keepOne = (h: { sourceId: string; bucket: string }) =>
    void keepIssue(h).then(() => refreshHygiene());
  // Look up move candidates for the missing-file flags on view.
  useEffect(() => {
    const missing = attention.filter((h) => h.bucket === "missing-file");
    for (const h of missing.slice(0, 5)) {
      if (foundPaths[h.sourceId] !== undefined) continue;
      api
        .findMovedFile(h.sourceId)
        .then((paths) =>
          setFoundPaths((m) => ({ ...m, [h.sourceId]: paths })),
        )
        .catch(() =>
          setFoundPaths((m) => ({ ...m, [h.sourceId]: [] })),
        );
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [attention]);

  const retryIssue = async (h: { sourceId: string }) => {
    setRetrying(h.sourceId);
    try {
      await refreshSource(h.sourceId);
    } finally {
      setRetrying(null);
    }
    await refreshHygiene();
  };

  // ---- Whole-section verdicts -------------------------------------------
  // A list of broken sources is usually one decision, not ten, and ten
  // decisions used to mean ten toasts. Each verb below acts on everything
  // visible and says so once.
  const retryable = attention.filter(
    (h) => h.kind !== "note" && h.bucket !== "duplicate",
  );
  const keepAllAttention = async () => {
    const n = attention.length;
    await Promise.all(attention.map(keepIssue));
    await refreshHygiene();
    pushToast("success", n === 1 ? "Kept 1 flagged item" : `Kept ${n} flagged items`);
  };
  const retryAllAttention = async () => {
    setRetryingAll(true);
    let ok = 0;
    try {
      // Sequential: a re-fetch is a network round trip plus a re-embed, and
      // firing the whole list at once is how a review of twenty sources
      // becomes a stalled app.
      for (const h of retryable) {
        try {
          await api.refreshSourceUrl(h.sourceId);
          ok += 1;
        } catch {
          /* counted below; the flag stays and says why */
        }
      }
    } finally {
      setRetryingAll(false);
    }
    if (currentId) {
      const fresh = await api.listSources(currentId);
      if (useStore.getState().currentId === currentId)
        useStore.setState({ sources: fresh });
    }
    await refreshHygiene();
    const failed = retryable.length - ok;
    pushToast(
      failed > 0 ? "error" : "success",
      failed > 0
        ? `Re-fetched ${ok} of ${retryable.length}; ${failed} still failing`
        : `Re-fetched ${ok === 1 ? "1 source" : `${ok} sources`}`,
    );
  };
  const removeAllAttention = async () => {
    const sourceIds = attention
      .filter((h) => h.kind !== "note")
      .map((h) => h.sourceId);
    const noteIds = attention
      .filter((h) => h.kind === "note")
      .map((h) => h.sourceId);
    const ok = await confirm({
      title: `Remove ${attention.length} flagged items?`,
      message: "Each source is deleted with its chunks.",
      items: attention.map((h) => h.title || "Untitled"),
      confirmLabel: "Remove all",
      danger: true,
    });
    if (!ok) return;
    // Both batches carry their own undo toast, so a mixed list says it
    // twice — still one toast per batch, never one per item.
    if (sourceIds.length > 0) await deleteSourcesBatch(sourceIds);
    if (noteIds.length > 0) await deleteNotesBatch(noteIds);
    await refreshHygiene();
  };
  // The duplicate bucket is the one verdict that needs no reading: every row
  // in it has an older twin the notebook is keeping, named in `keeperId`. So
  // it gets its own verb, separate from "Remove all" — clearing a double
  // import shouldn't mean auditing the broken sources beside it.
  const duplicates = attention.filter((h) => h.bucket === "duplicate");
  const removeDuplicates = async () => {
    const sourceIds = duplicates
      .filter((h) => h.kind !== "note")
      .map((h) => h.sourceId);
    const noteIds = duplicates
      .filter((h) => h.kind === "note")
      .map((h) => h.sourceId);
    const ok = await confirm({
      title:
        duplicates.length === 1
          ? "Remove 1 duplicate?"
          : `Remove ${duplicates.length} duplicates?`,
      message: "The oldest copy of each is kept.",
      items: duplicates.map((h) => h.title || "Untitled"),
      confirmLabel: "Remove duplicates",
      danger: true,
    });
    if (!ok) return;
    if (sourceIds.length > 0) await deleteSourcesBatch(sourceIds);
    if (noteIds.length > 0) await deleteNotesBatch(noteIds);
    await refreshHygiene();
  };
  const removeIssue = (h: { kind: string; sourceId: string }) =>
    void (h.kind === "note"
      ? deleteNotesBatch([h.sourceId])
      : deleteSourcesBatch([h.sourceId]));
  // The check is cheap (a metadata scan plus fs stats), so the review can
  // simply re-run it. Agents reach the same check through the
  // `source_hygiene` MCP tool.
  const recheck = async () => {
    setChecking(true);
    try {
      await refreshHygiene();
    } finally {
      setChecking(false);
    }
  };

  // Every section loads on its own clock (alchemy-release-hxl). The pane
  // paints its frame and headers at once; each section arrives when its own
  // call returns, so the slow tiers — Spotlight's mdfind subprocesses, the
  // duplicate scan that reads content, the web search — hold nothing else
  // back. Cached results per notebook show instantly on return and refresh
  // in place underneath.
  const cached = useStore((s) =>
    currentId ? s.growSections[currentId] : undefined,
  );
  const cacheSection = useStore((s) => s.cacheGrowSection);
  const queries = cached?.queries ?? [];
  const feedTier = cached?.feeds;
  const linkTier = cached?.links;
  const localTier = cached?.local;
  const retire = cached?.tidy;
  const merges = cached?.organize ?? [];
  // Per-section failures, in the section that failed — one tier going wrong
  // is not the pane going wrong.
  const [failed, setFailed] = useState<Record<string, boolean>>({});
  const [indexBusy, setIndexBusy] = useState(false);
  // Dismissals live in the store (loaded per notebook there) so the
  // sidebar's "Grow this notebook" door empties as this pane is cleared.
  const dismissed = useStore((s) => s.growthDismissed);
  const dismiss = useStore((s) => s.dismissGrowth);
  // The open-web tier: per-notebook opt-in; results and meter arrive
  // together. null = not run this visit.
  const [webOn, setWebOn] = useState(false);
  const [webBusy, setWebBusy] = useState(false);
  const [web, setWeb] = useState<{
    proposals: GrowthProposal[];
    credits: number;
    capped: boolean;
    refreshDays: number;
  } | null>(null);

  /** Run one section's call and publish it to the store cache. Sections are
   *  independent: a rejection marks that section and no other. */
  const loadSection = useCallback(
    <K extends keyof GrowSections>(
      notebookId: string,
      key: K,
      fetcher: () => Promise<NonNullable<GrowSections[K]>>,
    ) => {
      setFailed((f) => (f[key] ? { ...f, [key]: false } : f));
      return fetcher()
        .then((rows) => cacheSection(notebookId, key, rows))
        .catch(() => {
          // Leave the last known rows in place — a failed refresh should
          // not empty a section that was showing something a moment ago.
          setFailed((f) => ({ ...f, [key]: true }));
        });
    },
    [cacheSection],
  );

  const loadAllSections = useCallback(
    (notebookId: string) => {
      void loadSection(notebookId, "queries", () =>
        api.growthQueries(notebookId),
      );
      void loadSection(notebookId, "feeds", () => api.growthFeeds(notebookId));
      void loadSection(notebookId, "links", () => api.growthLinks(notebookId));
      void loadSection(notebookId, "local", () => api.growthLocal(notebookId));
      void loadSection(notebookId, "tidy", () => api.growthRetire(notebookId));
      void loadSection(notebookId, "organize", () =>
        api.growthTagMerges(notebookId),
      );
    },
    [loadSection],
  );

  useEffect(() => {
    setWeb(null);
    setWebOn(false);
    setFailed({});
    if (!currentId) return;
    // Backend-owned opt-in (the sweep acts on it); the old localStorage
    // flag migrates over the first time this notebook's pane opens.
    if (takeLegacyWebFlag(currentId))
      void api.setGrowthWebEnabled(currentId, true);
    api
      .growthWebEnabled(currentId)
      .then((on) => setWebOn(on))
      .catch(() => undefined);
    void refreshHygiene();
    loadAllSections(currentId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentId, loadAllSections]);

  const runWebSearch = (notebookId: string) => {
    setWebBusy(true);
    api
      .growthWebSearch(notebookId)
      .then((r) =>
        setWeb({
          proposals: r.proposals,
          credits: r.creditsThisMonth,
          capped: r.capped,
          refreshDays: r.refreshEveryDays,
        }),
      )
      .catch(() =>
        setWeb({ proposals: [], credits: 0, capped: false, refreshDays: 1 }),
      )
      .finally(() => setWebBusy(false));
  };
  // Already-enabled notebooks refresh their web results on open (the
  // sweep keeps the day cache warm, so this is usually instant).
  useEffect(() => {
    if (currentId && webOn && queries.length > 0) runWebSearch(currentId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentId, webOn, queries.length]);

  const existingUrls = useMemo(
    () => new Set(sources.map((s) => s.url).filter(Boolean)),
    [sources],
  );
  const visible = (list: GrowthProposal[]) =>
    list.filter((p) => !dismissed[p.url] && !existingUrls.has(p.url));
  const add = (p: GrowthProposal) => {
    dismiss(p.url);
    if (p.kind === "local") void addSourceFiles([p.url]);
    else void addSourceUrl(p.url);
  };

  const locals = visible(localTier ?? []);
  const isSourceSelected = (id: string) =>
    !selectedSourceIds || selectedSourceIds[id] !== false;
  const retireVisible = (retire ?? []).filter(
    (r) => !dismissed[`retire:${r.sourceId}`],
  );
  const dismissRetire = (id: string) => dismiss(`retire:${id}`);
  const muteAllRetire = () => {
    for (const r of retireVisible) {
      if (isSourceSelected(r.sourceId)) toggleSourceSelected(r.sourceId);
      dismissRetire(r.sourceId);
    }
  };
  const keepAllRetire = () => {
    for (const r of retireVisible) dismissRetire(r.sourceId);
  };
  const removeAllRetire = async () => {
    const ids = retireVisible.map((r) => r.sourceId);
    const ok = await confirm({
      title: `Remove ${ids.length} sources?`,
      message: "Each one is deleted with its chunks.",
      items: retireVisible.map((r) => r.title),
      confirmLabel: "Remove all",
      danger: true,
    });
    if (ok) void deleteSourcesBatch(ids);
  };
  const mergesVisible = merges.filter(
    (m) => !dismissed[`merge:${m.from}>${m.to}`],
  );
  const applyMerge = (m: TagMergeProposal) => {
    if (!currentId) return;
    dismiss(`merge:${m.from}>${m.to}`);
    void api
      .applyTagMerge(currentId, m.from, m.to)
      .then((count) => {
        useStore
          .getState()
          .pushToast("success", `Merged #${m.from} into #${m.to} on ${count} sources`);
        cacheSection(
          currentId,
          "organize",
          merges.filter((r) => r.from !== m.from),
        );
      })
      .catch(() => undefined);
  };
  const indexNote = notes.find((nt) => nt.title === "Notebook index");
  const makeIndex = () => {
    if (!currentId) return;
    const refreshing = !!indexNote;
    setIndexBusy(true);
    api
      .generateWikiIndex(currentId)
      .then((note) => {
        useStore.setState((st) => ({
          notes: [note, ...st.notes.filter((x) => x.id !== note.id)],
        }));
        const st = useStore.getState();
        // Say what happened — a refresh rewrites in place, which is
        // otherwise invisible — and don't stack a duplicate reader entry
        // when the index is already the open document.
        st.pushToast(
          "success",
          refreshing ? "Notebook index refreshed" : "Notebook index created",
        );
        const viewing =
          st.reader.open && st.reader.history[st.reader.index]?.id === note.id;
        if (!viewing) st.openInReader({ type: "note", id: note.id });
      })
      .catch(() => undefined)
      .finally(() => setIndexBusy(false));
  };
  const mined = visible(linkTier ?? []);
  // Feeds the notebook's own pages advertised (docs/RFC-events.md §2):
  // remembered at import, followed only from here. A page that advertises a
  // feed is often also a link its siblings cite; the feed is the better
  // offer, so it wins the URL — the backend subtracts it from the link tier
  // (growth_links_impl), which keeps the two sections disjoint here.
  const feeds = visible(feedTier ?? []);
  const found = visible(web?.proposals ?? []);
  // Spotlight searches for the standing queries, so it stays pending until
  // both land — but a notebook with no questions has nothing to search for,
  // and announcing a search that will return empty is a flicker, not news.
  const localPending =
    !failed.local &&
    localTier === undefined &&
    (cached?.queries === undefined || queries.length > 0);
  // The one-line "nothing here" summary replaces all three free tiers, so it
  // may only speak once all three have actually answered.
  const freeTiersSettled =
    localTier !== undefined &&
    linkTier !== undefined &&
    feedTier !== undefined &&
    cached?.queries !== undefined;
  const freeTiersFailed = !!(failed.local || failed.links || failed.feeds);
  const freeTiersEmpty =
    locals.length === 0 && mined.length === 0 && feeds.length === 0;
  // Whether the Needs attention header earns a second line at all: whole-list
  // verdicts only make sense over more than one flag, and Retry all / Remove
  // duplicates only when there is something of that kind to act on.
  const bulkVerbs = attention.length > 1;

  const row = (p: GrowthProposal) => (
    <div
      key={p.url}
      className="flex items-center gap-2 rounded-md border border-border px-3 py-2"
    >
      {p.kind === "local" ? (
        <FileText className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      ) : (
        <Favicon url={p.url} />
      )}
      <div className="min-w-0 flex-1">
        <div className="truncate text-body text-foreground" title={p.url}>
          {p.anchor || p.url.replace(/^https?:\/\//, "")}
        </div>
        <div className="truncate text-caption text-muted-foreground">
          {p.kind === "local" ? (
            <>{p.url}</>
          ) : (
            <>
              {p.url.replace(/^https?:\/\//, "").split("/")[0]}
              {p.mentions > 0 && (
                <>
                  {" "}
                  · seen {p.mentions}×
                  {p.sourceCount > 1 ? ` in ${p.sourceCount} sources` : ""}
                </>
              )}
            </>
          )}
        </div>
        {p.matchedQuery && (
          <div className="truncate text-caption text-subtle-foreground">
            asked: “{p.matchedQuery}”
          </div>
        )}
      </div>
      <Button
        variant="ghost"
        onClick={() => add(p)}
        title={
          p.kind === "local"
            ? "Add this file as a source"
            : p.kind === "feed"
              ? "Follow this feed — new entries arrive as sources"
              : "Fetch this page and add it as a source"
        }
      >
        {p.kind === "feed" ? "Follow" : "Add"}
      </Button>
      <Button
        variant="ghost"
        onClick={() => dismiss(p.url)}
        title="Hide for 30 days"
      >
        Dismiss
      </Button>
    </div>
  );

  // Section headers pin to the top of the pane while their own section
  // scrolls: on a long Grow page the verbs you're about to use ("Add all",
  // "Remove all") should never scroll away from the rows they act on.
  // Hairline below, pane background under it, so content passes cleanly.
  const headerBase =
    "sticky top-0 z-10 -mx-5 border-b border-border bg-background/95 px-5 py-2 backdrop-blur";
  const headerRow = `${headerBase} flex items-center gap-2`;

  /** One quiet line while a section is still working. No spinner: six
   *  sections load at once, and six spinners is a light show. */
  const pendingLine = (label: string) => (
    <div className="px-1 py-2 text-caption text-muted-foreground">{label}</div>
  );
  /** What a section says when its own call failed. Plain, and only about
   *  itself — the rest of the pane is fine. */
  const errorLine = (label: string) => (
    <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
      {label}
    </div>
  );

  const section = (
    icon: React.ReactNode,
    title: string,
    hint: string,
    items: GrowthProposal[],
    state: { pending?: string; error?: string } = {},
  ) => (
    <div className="flex flex-col gap-2">
      <div className={headerRow}>
        {icon}
        <span className="text-caption font-semibold text-foreground">
          {title}
        </span>
        <span className="text-caption text-subtle-foreground">{hint}</span>
        {/* Whole-section verdicts, same shape as the retirement pass: a
            long list of citations is usually all-or-nothing. */}
        {!state.pending && items.length > 1 && (
          <div className="ml-auto flex shrink-0 items-center gap-1">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                // Files go as one drop; pages queue one at a time — the
                // ingest queue keys items by millisecond, and a tight loop
                // of adds would share one.
                const files = items
                  .filter((p) => p.kind === "local")
                  .map((p) => p.url);
                const pages = items.filter((p) => p.kind !== "local");
                for (const p of items) dismiss(p.url);
                if (files.length > 0) void addSourceFiles(files);
                void (async () => {
                  for (const p of pages) await addSourceUrl(p.url);
                })();
              }}
              title={`Add all ${items.length} to this notebook`}
            >
              Add all
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                for (const p of items) dismiss(p.url);
              }}
              title="Hide every proposal below for 30 days"
            >
              Dismiss all
            </Button>
          </div>
        )}
      </div>
      {state.error ? (
        errorLine(state.error)
      ) : state.pending ? (
        pendingLine(state.pending)
      ) : items.length === 0 ? (
        errorLine("Nothing right now.")
      ) : (
        items.map(row)
      )}
    </div>
  );

  return (
    <div className="relative flex h-full flex-1 flex-col min-w-0">
      <div className="flex h-12 items-center gap-2 border-b border-border px-5">
        <Sprout className="h-4 w-4 text-muted-foreground" />
        <span className="text-caption font-semibold uppercase tracking-wide text-muted-foreground">
          Grow
        </span>
      </div>
      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto flex max-w-[720px] flex-col gap-6 px-5 py-6">
          {!currentId ? (
            <EmptyState title="Open a notebook to grow it" />
          ) : (
            <>
              <p className="text-pretty text-body leading-relaxed text-muted-foreground">
                Ways this notebook could grow — from your own machine, from
                what your sources cite, and (if you turn it on) from the web.
                Nothing is fetched unless you add it.
              </p>
              {queries.length > 0 && (
                <div className="flex flex-col gap-1.5">
                  <span className="text-micro font-semibold uppercase tracking-wide text-subtle-foreground">
                    This notebook is hungry for
                  </span>
                  <div className="flex flex-wrap gap-1.5">
                    {queries.map((q) => (
                      <span
                        key={q}
                        className="max-w-full truncate rounded-full border border-border px-2.5 py-1 text-caption text-muted-foreground"
                        title="A recent question this notebook answered thinly"
                      >
                        {q}
                      </span>
                    ))}
                  </div>
                </div>
              )}
              {/* Sections keep their provenance (a local file, your own
                  sources' citations, and a web search are different levels
                  of trust — Spotlight groups by kind for the same reason),
                  but empty groups don't earn boxes: both free tiers quiet
                  down to one line when neither found anything. */}
              {freeTiersSettled && !freeTiersFailed && freeTiersEmpty ? (
                <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
                  Nothing new on this Mac or in your sources right now.
                </div>
              ) : (
                <>
                  {(locals.length > 0 || localPending || failed.local) &&
                    section(
                      <FileText className="h-3.5 w-3.5 text-muted-foreground" />,
                      "On this Mac",
                      "Spotlight matches for the questions above",
                      locals,
                      {
                        pending: localPending
                          ? "Searching this Mac…"
                          : undefined,
                        error: failed.local
                          ? "Couldn’t search this Mac just now."
                          : undefined,
                      },
                    )}
                  {(mined.length > 0 || linkTier === undefined || failed.links) &&
                    section(
                      <Globe className="h-3.5 w-3.5 text-muted-foreground" />,
                      "From your sources",
                      "pages your sources keep citing",
                      mined,
                      {
                        pending:
                          linkTier === undefined && !failed.links
                            ? "Reading what your sources cite…"
                            : undefined,
                        error: failed.links
                          ? "Couldn’t read your sources just now."
                          : undefined,
                      },
                    )}
                  {(feeds.length > 0 || failed.feeds) &&
                    section(
                      <Rss className="h-3.5 w-3.5 text-muted-foreground" />,
                      "Feeds",
                      "your pages advertise these — follow one to keep up",
                      feeds,
                      {
                        error: failed.feeds
                          ? "Couldn’t list the feeds your pages advertise."
                          : undefined,
                      },
                    )}
                </>
              )}
              {/* The open-web tier: the consent line. Enabling it sends the
                  standing queries to Firecrawl's keyless search — search
                  metadata only; pages are fetched when you add them. */}
              <div className="flex flex-col gap-2">
                <div className={headerRow}>
                  <Search className="h-3.5 w-3.5 text-muted-foreground" />
                  <span className="text-caption font-semibold text-foreground">
                    From the web
                  </span>
                  {web && (
                    <span className="ml-auto text-caption text-subtle-foreground">
                      {web.credits} of 1,000 free credits used this month
                      {" · refreshes "}
                      {web.refreshDays >= 2
                        ? `every ${web.refreshDays} days`
                        : "daily"}
                    </span>
                  )}
                  <Switch
                    className={web ? "" : "ml-auto"}
                    checked={webOn}
                    onChange={(on) => {
                      if (!currentId) return;
                      void api.setGrowthWebEnabled(currentId, on);
                      setWebOn(on);
                      if (on) runWebSearch(currentId);
                      else setWeb(null);
                    }}
                  />
                </div>
                {queries.length === 0 ? (
                  <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
                    Ask this notebook something it can’t answer yet — thin
                    answers become the questions the web search runs.
                  </div>
                ) : !webOn ? (
                  <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption leading-relaxed text-subtle-foreground">
                    Off for this notebook. Turning it on sends the questions
                    above (nothing else) to Firecrawl’s free search tier;
                    results are proposals, and pages are fetched only when
                    you add them.
                  </div>
                ) : webBusy ? (
                  <div className="flex items-center gap-2 px-1 py-2 text-caption text-muted-foreground">
                    <Spinner className="h-3.5 w-3.5" /> Searching the web…
                  </div>
                ) : web?.capped ? (
                  <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
                    This month’s free search budget is spent — the meter
                    resets next month.
                  </div>
                ) : found.length === 0 ? (
                  <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
                    Nothing new found for these questions.
                  </div>
                ) : (
                  found.map(row)
                )}
              </div>
              {/* Needs attention (RFC-source-hygiene), merged in from the
                  Sources panel: tending what's broken is the other half of
                  growing. Nothing is removed unless you say so. */}
              <div className="flex flex-col gap-2">
                <div className={`${headerBase} flex flex-col gap-1`}>
                  <div className="flex items-center gap-2">
                    <AlertCircle className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                    <span className="shrink-0 text-caption font-semibold text-foreground">
                      Needs attention
                    </span>
                    <span className="truncate text-caption text-subtle-foreground">
                      broken sources, duplicates, empty notes. Keep dismisses
                      the flag.
                    </span>
                    {/* The recheck button sits where every other section
                        header keeps its control: first line, hard right. */}
                    <Button
                      variant="ghost"
                      size="icon"
                      className="ml-auto shrink-0"
                      loading={checking}
                      onClick={() => void recheck()}
                      aria-label="Check for problems now"
                      title="Check for problems now"
                    >
                      {!checking && (
                        <RefreshCw aria-hidden className="h-3.5 w-3.5" />
                      )}
                    </Button>
                  </div>
                  {/* The bulk verbs never fit beside the title without
                      crushing the summary, so they get their own line — and
                      only when at least one of them would render. A clean
                      notebook keeps a one-line header. */}
                  {bulkVerbs && (
                    <div className="flex items-center justify-end gap-1">
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => void keepAllAttention()}
                        title="Dismiss every flag below and keep everything"
                      >
                        Keep all
                      </Button>
                      {retryable.length > 0 && (
                        <Button
                          variant="ghost"
                          size="sm"
                          loading={retryingAll}
                          onClick={() => void retryAllAttention()}
                          title={`Fetch ${retryable.length} sources again now`}
                        >
                          Retry all
                        </Button>
                      )}
                      {duplicates.length > 0 && (
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => void removeDuplicates()}
                          title={`Remove ${duplicates.length} extra copies and keep the oldest of each`}
                        >
                          Remove duplicates
                        </Button>
                      )}
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-destructive hover:bg-destructive/10"
                        onClick={() => void removeAllAttention()}
                        title="Remove everything below"
                      >
                        Remove all
                      </Button>
                    </div>
                  )}
                </div>
                {attention.length === 0 ? (
                  <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
                    All clean.
                  </div>
                ) : (
                  attention.map((h) => (
                    <div
                      key={`${h.kind}:${h.sourceId}:${h.bucket}`}
                      className="flex items-center gap-2 rounded-md border border-border px-3 py-2"
                    >
                      {h.kind === "note" && (
                        <StickyNote
                          aria-hidden
                          className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
                        />
                      )}
                      <div className="min-w-0 flex-1">
                        <div
                          className="truncate text-body text-foreground"
                          title={h.title}
                        >
                          {h.title || "Untitled"}
                        </div>
                        <div
                          className="truncate text-caption text-muted-foreground"
                          title={h.detail}
                        >
                          {HYGIENE_LABEL[h.bucket] ?? h.bucket} · {h.detail}
                        </div>
                      </div>
                      {h.bucket === "missing-file" &&
                        (foundPaths[h.sourceId]?.length ?? 0) > 0 && (
                          <Button
                            variant="secondary"
                            size="sm"
                            onClick={() =>
                              void relocate(
                                h.sourceId,
                                foundPaths[h.sourceId][0],
                              )
                            }
                            title={`Found at ${foundPaths[h.sourceId][0]} — point the source there and re-read it`}
                          >
                            Found it — move
                          </Button>
                        )}
                      {h.bucket === "missing-file" && (
                        <Button
                          variant="ghost"
                          onClick={() => void locate(h.sourceId)}
                          title="Pick the file's new location yourself"
                        >
                          Locate…
                        </Button>
                      )}
                      {h.kind !== "note" && h.bucket !== "duplicate" && (
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
                        onClick={() => keepOne(h)}
                        title={
                          h.kind === "note"
                            ? "Dismiss this flag and keep the note"
                            : "Dismiss this flag and keep the source"
                        }
                      >
                        Keep
                      </Button>
                      <Button
                        variant="ghost"
                        className="text-destructive hover:bg-destructive/10"
                        onClick={() => removeIssue(h)}
                        title={
                          h.kind === "note"
                            ? "Delete the note"
                            : "Remove the source and its chunks"
                        }
                      >
                        Remove
                      </Button>
                    </div>
                  ))
                )}
              </div>
              {/* The retirement pass (Pillar 3): the corpus stays sharp by
                  proposing, never acting — Mute drops a source from chat
                  scope (reversible in the Sources panel), Remove deletes. */}
              <div className="flex flex-col gap-2">
                <div className={headerRow}>
                  <Archive className="h-3.5 w-3.5 text-muted-foreground" />
                  <span className="text-caption font-semibold text-foreground">
                    Tidy
                  </span>
                  <span className="text-caption text-subtle-foreground">
                    old sources no retrieval has ever cited
                  </span>
                  {retireVisible.length > 1 && (
                    <div className="ml-auto flex gap-1">
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={muteAllRetire}
                        title="Drop every proposal below from chat & generation"
                      >
                        Mute all
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={keepAllRetire}
                        title="Keep everything — hide these proposals for 30 days"
                      >
                        Keep all
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-destructive hover:bg-destructive/10"
                        onClick={() => void removeAllRetire()}
                        title="Remove every source below"
                      >
                        Remove all
                      </Button>
                    </div>
                  )}
                </div>
                {failed.tidy ? (
                  errorLine("Couldn’t check the shelves just now.")
                ) : retire === undefined ? (
                  pendingLine("Checking the shelves…")
                ) : retireVisible.length === 0 ? (
                  <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
                    Nothing gathering dust.
                  </div>
                ) : (
                  retireVisible.map((r) => (
                    <div
                      key={r.sourceId}
                      className="flex items-center gap-2 rounded-md border border-border px-3 py-2"
                    >
                      <div className="min-w-0 flex-1">
                        <div
                          className="truncate text-body text-foreground"
                          title={r.title}
                        >
                          {r.title || "Untitled"}
                        </div>
                        <div className="truncate text-caption text-muted-foreground">
                          {r.ageDays} days old · never cited ·{" "}
                          {r.charCount.toLocaleString()} chars
                          {!isSourceSelected(r.sourceId) && " · muted"}
                        </div>
                      </div>
                      {isSourceSelected(r.sourceId) && (
                        <Button
                          variant="ghost"
                          onClick={() => {
                            toggleSourceSelected(r.sourceId);
                            dismissRetire(r.sourceId);
                          }}
                          title="Drop from chat & generation — reversible in the Sources panel"
                        >
                          Mute
                        </Button>
                      )}
                      <Button
                        variant="ghost"
                        onClick={() => dismissRetire(r.sourceId)}
                        title="Keep it — hide this proposal for 30 days"
                      >
                        Keep
                      </Button>
                      <Button
                        variant="ghost"
                        className="text-destructive hover:bg-destructive/10"
                        onClick={() => void deleteSourcesBatch([r.sourceId])}
                        title="Remove the source and its chunks"
                      >
                        Remove
                      </Button>
                    </div>
                  ))
                )}
              </div>
              {/* Tag merges (phase 5): near-duplicate tags fold together —
                  plural into singular, separator variants into the common
                  spelling. Proposal only; Merge rewrites every carrier. */}
              {mergesVisible.length > 0 && (
                <div className="flex flex-col gap-2">
                  <div className={headerRow}>
                    <Tags className="h-3.5 w-3.5 text-muted-foreground" />
                    <span className="text-caption font-semibold text-foreground">
                      Organize
                    </span>
                    <span className="text-caption text-subtle-foreground">
                      tags that look like the same word
                    </span>
                    <Button
                      variant="secondary"
                      size="sm"
                      className="ml-auto"
                      onClick={() => {
                        for (const m of mergesVisible) applyMerge(m);
                      }}
                      title="Apply every merge below"
                    >
                      Merge all
                    </Button>
                  </div>
                  {mergesVisible.map((m) => (
                    <div
                      key={`${m.from}>${m.to}`}
                      className="flex items-center gap-2 rounded-md border border-border px-3 py-2"
                    >
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-body text-foreground">
                          #{m.from} → #{m.to}
                        </div>
                        <div className="truncate text-caption text-muted-foreground">
                          {m.fromCount} + {m.toCount} sources
                        </div>
                      </div>
                      <Button
                        variant="ghost"
                        onClick={() => applyMerge(m)}
                        title={`Rewrite #${m.from} to #${m.to} everywhere`}
                      >
                        Merge
                      </Button>
                      <Button
                        variant="ghost"
                        onClick={() => dismiss(`merge:${m.from}>${m.to}`)}
                        title="They're different — hide for 30 days"
                      >
                        Keep both
                      </Button>
                    </div>
                  ))}
                </div>
              )}
              {/* The wiki index (Pillar 3's north star, deterministic v1):
                  one generated note mapping the notebook — tags, title links,
                  dust called out. An ordinary note, so it round-trips
                  through OKF and agents can edit it. */}
              <div className="flex flex-col gap-2">
                <div className={headerRow}>
                  <BookOpen className="h-3.5 w-3.5 text-muted-foreground" />
                  <span className="text-caption font-semibold text-foreground">
                    Wiki index
                  </span>
                  <span className="text-caption text-subtle-foreground">
                    a living map of this notebook, as a note
                  </span>
                  <Button
                    variant="secondary"
                    size="sm"
                    className="ml-auto"
                    loading={indexBusy}
                    onClick={makeIndex}
                  >
                    {indexNote ? "Refresh index" : "Create index note"}
                  </Button>
                </div>
                {indexNote && (
                  <>
                    <button
                      type="button"
                      onClick={() =>
                        useStore
                          .getState()
                          .openInReader({ type: "note", id: indexNote.id })
                      }
                      className="rounded-md border border-border px-3 py-2 text-left text-caption text-muted-foreground hover:bg-surface-2"
                    >
                      Open “Notebook index” — updated{" "}
                      {new Date(indexNote.updatedAt).toLocaleDateString()}
                    </button>
                    <span className="text-caption text-subtle-foreground">
                      Refreshes on its own with background work —{" "}
                      <button
                        type="button"
                        className="text-citation hover:underline"
                        onClick={() =>
                          useStore.getState().openSettings("background")
                        }
                      >
                        Nightly settings
                      </button>
                    </span>
                  </>
                )}
              </div>
            </>
          )}
        </div>
      </div>
      {confirmDialog}
    </div>
  );
}
