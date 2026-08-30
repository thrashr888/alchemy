import { useEffect, useMemo, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import {
  HYGIENE_LABEL,
  loadGrowthDismissed,
  loadHygieneKept,
  saveGrowthDismissed,
  saveHygieneKept,
  setWebSearchEnabled,
  webSearchEnabled,
} from "@/lib/growth";
import type { GrowthProposal } from "@/lib/types";
import { Button, EmptyState, LoadingState, Spinner } from "./ui";
import { Favicon } from "./SourcesPanel";
import { AlertCircle, FileText, Globe, Search, Sprout } from "lucide-react";

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
  const [retrying, setRetrying] = useState<string | null>(null);

  // Needs-attention flags (RFC-source-hygiene) — merged into Grow: tending
  // what's broken is the other half of growing (and where Pillar 3's
  // curation passes will land). Keeps live in localStorage; refreshing
  // hygiene afterwards re-runs this against the fresh keeps.
  const attention = useMemo(() => {
    const kept = loadHygieneKept(currentId);
    const seen = new Map<string, (typeof hygiene)[number]>();
    for (const h of hygiene) {
      if (h.bucket === "stale") continue;
      if (kept[`${h.sourceId}:${h.bucket}`]) continue;
      if (!seen.has(h.sourceId)) seen.set(h.sourceId, h);
    }
    return [...seen.values()];
  }, [hygiene, currentId]);
  const keepIssue = (h: { sourceId: string; bucket: string }) => {
    if (h.bucket === "unreachable") {
      void hygieneKeep(h.sourceId).then(() => refreshHygiene());
      return;
    }
    const kept = loadHygieneKept(currentId);
    kept[`${h.sourceId}:${h.bucket}`] = true;
    saveHygieneKept(currentId, kept);
    void refreshHygiene();
  };
  const retryIssue = async (h: { sourceId: string }) => {
    setRetrying(h.sourceId);
    try {
      await refreshSource(h.sourceId);
    } finally {
      setRetrying(null);
    }
    await refreshHygiene();
  };

  const [queries, setQueries] = useState<string[]>([]);
  const [proposals, setProposals] = useState<GrowthProposal[] | null>(null);
  // The Spotlight tier arrives on its own clock — mdfind subprocesses are
  // the slow part, so the section fills in async instead of gating the pane.
  const [localTier, setLocalTier] = useState<GrowthProposal[] | null>(null);
  const [dismissed, setDismissed] = useState<Record<string, number>>({});
  // The open-web tier: per-notebook opt-in; results and meter arrive
  // together. null = not run this visit.
  const [webOn, setWebOn] = useState(false);
  const [webBusy, setWebBusy] = useState(false);
  const [web, setWeb] = useState<{
    proposals: GrowthProposal[];
    credits: number;
    capped: boolean;
  } | null>(null);

  useEffect(() => {
    setQueries([]);
    setProposals(null);
    setWeb(null);
    setDismissed(loadGrowthDismissed(currentId));
    setWebOn(webSearchEnabled(currentId));
    if (!currentId) return;
    void refreshHygiene();
    let stale = false;
    api
      .growthProposals(currentId)
      .then((overview) => {
        if (stale) return;
        setQueries(overview.queries);
        setProposals(overview.proposals);
      })
      .catch(() => setProposals([]));
    setLocalTier(null);
    api
      .growthLocal(currentId)
      .then((hits) => {
        if (!stale) setLocalTier(hits);
      })
      .catch(() => setLocalTier([]));
    return () => {
      stale = true;
    };
  }, [currentId]);

  const runWebSearch = (notebookId: string) => {
    setWebBusy(true);
    api
      .growthWebSearch(notebookId)
      .then((r) =>
        setWeb({
          proposals: r.proposals,
          credits: r.creditsThisMonth,
          capped: r.capped,
        }),
      )
      .catch(() => setWeb({ proposals: [], credits: 0, capped: false }))
      .finally(() => setWebBusy(false));
  };
  // Already-enabled notebooks refresh their web results on open.
  useEffect(() => {
    if (currentId && webSearchEnabled(currentId) && queries.length > 0)
      runWebSearch(currentId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentId, queries.length]);

  const existingUrls = useMemo(
    () => new Set(sources.map((s) => s.url).filter(Boolean)),
    [sources],
  );
  const visible = (list: GrowthProposal[]) =>
    list.filter((p) => !dismissed[p.url] && !existingUrls.has(p.url));
  const dismiss = (url: string) =>
    setDismissed((m) => {
      const next = { ...m, [url]: Date.now() };
      saveGrowthDismissed(currentId, next);
      return next;
    });
  const add = (p: GrowthProposal) => {
    dismiss(p.url);
    if (p.kind === "local") void addSourceFiles([p.url]);
    else void addSourceUrl(p.url);
  };

  const locals = visible(localTier ?? []);
  const mined = visible(proposals ?? []).filter((p) => p.kind === "web");
  const found = visible(web?.proposals ?? []);

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
            : "Fetch this page and add it as a source"
        }
      >
        Add
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

  const section = (
    icon: React.ReactNode,
    title: string,
    hint: string,
    items: GrowthProposal[],
    busy = false,
  ) => (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        {icon}
        <span className="text-caption font-semibold text-foreground">
          {title}
        </span>
        <span className="text-caption text-subtle-foreground">{hint}</span>
      </div>
      {busy ? (
        <div className="flex items-center gap-2 px-1 py-2 text-caption text-muted-foreground">
          <Spinner className="h-3.5 w-3.5" /> Searching this Mac…
        </div>
      ) : items.length === 0 ? (
        <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
          Nothing right now.
        </div>
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
          ) : proposals === null ? (
            <LoadingState label="Reading the frontier…" />
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
              {section(
                <FileText className="h-3.5 w-3.5 text-muted-foreground" />,
                "On this Mac",
                "Spotlight matches for the questions above",
                locals,
                localTier === null && queries.length > 0,
              )}
              {section(
                <Globe className="h-3.5 w-3.5 text-muted-foreground" />,
                "From your sources",
                "pages your sources keep citing",
                mined,
              )}
              {/* The open-web tier: the consent line. Enabling it sends the
                  standing queries to Firecrawl's keyless search — search
                  metadata only; pages are fetched when you add them. */}
              <div className="flex flex-col gap-2">
                <div className="flex items-center gap-2">
                  <Search className="h-3.5 w-3.5 text-muted-foreground" />
                  <span className="text-caption font-semibold text-foreground">
                    From the web
                  </span>
                  {web && (
                    <span className="ml-auto text-caption text-subtle-foreground">
                      {web.credits} of 1,000 free credits used this month
                    </span>
                  )}
                  {webOn && (
                    <button
                      type="button"
                      className={
                        (web ? "" : "ml-auto ") +
                        "text-caption text-subtle-foreground hover:text-foreground hover:underline"
                      }
                      onClick={() => {
                        setWebSearchEnabled(currentId, false);
                        setWebOn(false);
                        setWeb(null);
                      }}
                      title="Stop sending this notebook's questions to Firecrawl"
                    >
                      Turn off
                    </button>
                  )}
                </div>
                {queries.length === 0 ? (
                  <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
                    Ask this notebook something it can’t answer yet — thin
                    answers become the questions the web search runs.
                  </div>
                ) : !webOn ? (
                  <div className="flex flex-col gap-2 rounded-md border border-border px-3 py-2.5">
                    <span className="text-caption leading-relaxed text-muted-foreground">
                      Search the web for the questions above via Firecrawl’s
                      free tier. Only the questions are sent; results are
                      proposals, and pages are fetched only when you add
                      them.
                    </span>
                    <Button
                      variant="secondary"
                      size="sm"
                      className="self-start"
                      onClick={() => {
                        setWebSearchEnabled(currentId, true);
                        setWebOn(true);
                        runWebSearch(currentId);
                      }}
                    >
                      Enable web search for this notebook
                    </Button>
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
                <div className="flex items-center gap-2">
                  <AlertCircle className="h-3.5 w-3.5 text-muted-foreground" />
                  <span className="text-caption font-semibold text-foreground">
                    Needs attention
                  </span>
                  <span className="text-caption text-subtle-foreground">
                    broken or outdated sources — Keep dismisses the flag
                  </span>
                </div>
                {attention.length === 0 ? (
                  <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
                    All clean.
                  </div>
                ) : (
                  attention.map((h) => (
                    <div
                      key={`${h.sourceId}:${h.bucket}`}
                      className="flex items-center gap-2 rounded-md border border-border px-3 py-2"
                    >
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
            </>
          )}
        </div>
      </div>
    </div>
  );
}
