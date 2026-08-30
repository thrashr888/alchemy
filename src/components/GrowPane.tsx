import { useEffect, useMemo, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import {
  loadGrowthDismissed,
  saveGrowthDismissed,
  setWebSearchEnabled,
  webSearchEnabled,
} from "@/lib/growth";
import type { GrowthProposal } from "@/lib/types";
import { Button, EmptyState, LoadingState, Spinner } from "./ui";
import { Favicon } from "./SourcesPanel";
import { FileText, Globe, Search, Sprout } from "lucide-react";

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

  const [queries, setQueries] = useState<string[]>([]);
  const [proposals, setProposals] = useState<GrowthProposal[] | null>(null);
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
    let stale = false;
    api
      .growthProposals(currentId)
      .then((overview) => {
        if (stale) return;
        setQueries(overview.queries);
        setProposals(overview.proposals);
      })
      .catch(() => setProposals([]));
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

  const locals = visible(proposals ?? []).filter((p) => p.kind === "local");
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
        <div className="truncate text-micro text-muted-foreground">
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
          <div className="truncate text-micro text-subtle-foreground">
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
  ) => (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        {icon}
        <span className="text-caption font-semibold text-foreground">
          {title}
        </span>
        <span className="text-micro text-subtle-foreground">{hint}</span>
      </div>
      {items.length === 0 ? (
        <div className="rounded-md border border-dashed border-border px-3 py-2 text-micro text-subtle-foreground">
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
                        className="max-w-full truncate rounded-full border border-border px-2.5 py-0.5 text-micro text-muted-foreground"
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
                    <span className="ml-auto text-micro text-subtle-foreground">
                      {web.credits} of 1,000 free credits used this month
                    </span>
                  )}
                </div>
                {queries.length === 0 ? (
                  <div className="rounded-md border border-dashed border-border px-3 py-2 text-micro text-subtle-foreground">
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
                  <div className="rounded-md border border-dashed border-border px-3 py-2 text-micro text-subtle-foreground">
                    This month’s free search budget is spent — the meter
                    resets next month.
                  </div>
                ) : found.length === 0 ? (
                  <div className="rounded-md border border-dashed border-border px-3 py-2 text-micro text-subtle-foreground">
                    Nothing new found for these questions.
                  </div>
                ) : (
                  found.map(row)
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
