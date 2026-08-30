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
  stale: "stale",
};

/** The open-web tier is opt-in per notebook (the RFC's consent line):
 *  enabling it means this notebook's standing queries go to Firecrawl. */
export function webSearchEnabled(notebookId: string | null): boolean {
  return (
    !!notebookId &&
    localStorage.getItem(`growthWebSearch:${notebookId}`) === "on"
  );
}

export function setWebSearchEnabled(notebookId: string | null, on: boolean) {
  if (!notebookId) return;
  try {
    if (on) localStorage.setItem(`growthWebSearch:${notebookId}`, "on");
    else localStorage.removeItem(`growthWebSearch:${notebookId}`);
  } catch {
    /* fine — the user just gets asked again next time */
  }
}
