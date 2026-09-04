import type { GrowthProposal, HygieneIssue, Source } from "./types";

// Growth-surface client state (RFC-living-notebook Pillar 2): dismissals
// and the per-notebook web-search opt-in, both localStorage — per-machine
// conveniences, not notebook data.

/** Dismissed proposals, per notebook, with a 30-day decay so a
 *  once-rejected item can earn a second look if it keeps accumulating
 *  evidence. */
export function loadGrowthDismissed(
  notebookId: string | null,
): Record<string, number> {
  if (!notebookId) return {};
  try {
    const raw = JSON.parse(
      localStorage.getItem(`growthDismissed:${notebookId}`) ?? "{}",
    ) as Record<string, number>;
    const cutoff = Date.now() - 30 * 86_400_000;
    return Object.fromEntries(
      Object.entries(raw).filter(([, ts]) => ts > cutoff),
    );
  } catch {
    return {};
  }
}

export function saveGrowthDismissed(
  notebookId: string | null,
  dismissed: Record<string, number>,
) {
  if (!notebookId) return;
  try {
    localStorage.setItem(
      `growthDismissed:${notebookId}`,
      JSON.stringify(dismissed),
    );
  } catch {
    /* storage full or unavailable — the dismissal just won't stick */
  }
}

// "Keep" decisions from the hygiene review (RFC-source-hygiene), keyed
// `${sourceId}:${bucket}` per notebook. Local suppression on purpose:
// unreachable keeps reset real backend state, but a kept duplicate or
// missing file is a viewing preference — the signal itself stays true and
// agents still see it in the MCP report.
export function loadHygieneKept(
  notebookId: string | null,
): Record<string, boolean> {
  if (!notebookId) return {};
  try {
    const raw = localStorage.getItem(`hygieneKept:${notebookId}`);
    return raw ? JSON.parse(raw) : {};
  } catch {
    return {};
  }
}

export function saveHygieneKept(
  notebookId: string | null,
  kept: Record<string, boolean>,
) {
  if (!notebookId) return;
  try {
    localStorage.setItem(`hygieneKept:${notebookId}`, JSON.stringify(kept));
  } catch {
    /* best-effort */
  }
}

export const HYGIENE_LABEL: Record<string, string> = {
  unreachable: "unreachable",
  "missing-file": "missing",
  duplicate: "duplicate",
  husk: "failed import",
  "empty-note": "empty note",
  stale: "stale",
};

/** The open-web tier's opt-in lives backend-side now (the sweep acts on
 *  it) — this migrates the old localStorage flag once, then forgets it. */
export function takeLegacyWebFlag(notebookId: string): boolean {
  try {
    const key = `growthWebSearch:${notebookId}`;
    const on = localStorage.getItem(key) === "on";
    localStorage.removeItem(key);
    return on;
  } catch {
    return false;
  }
}

/** Both the review badge and the pane use the same proposal eligibility. */
export function visibleGrowthProposals(
  proposals: readonly GrowthProposal[],
  existingUrls: ReadonlySet<string>,
  dismissed: Record<string, number>,
): GrowthProposal[] {
  return proposals.filter((p) => !dismissed[p.url] && !existingUrls.has(p.url));
}

/** One review row per object; stale signals belong to the background sweep. */
export function growthAttention(
  hygiene: readonly HygieneIssue[],
  kept: Record<string, boolean>,
): HygieneIssue[] {
  const seen = new Map<string, HygieneIssue>();
  for (const issue of hygiene) {
    if (issue.bucket === "stale" || kept[`${issue.sourceId}:${issue.bucket}`])
      continue;
    const key = `${issue.kind}:${issue.sourceId}`;
    if (!seen.has(key)) seen.set(key, issue);
  }
  return [...seen.values()];
}

/** Invalidate mined proposals when content changes, including equal-size replacements. */
export function growthSourceRevision(
  sources: readonly Pick<Source, "id" | "status" | "url" | "fetchedAt">[],
): string {
  return JSON.stringify(sources.map((s) => [s.id, s.status, s.url, s.fetchedAt]));
}
