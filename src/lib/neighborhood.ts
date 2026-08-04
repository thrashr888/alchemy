/**
 * The subgraph within N hops of one document.
 *
 * A few hundred documents in one force layout is soup no matter how good the
 * labels, the zoom, or the renderer are — the picture is dense because the
 * data is dense, and no amount of camera work fixes that. The way out is to
 * stop drawing the whole thing: pick a document and show its neighbourhood,
 * which is the question anyone actually has in front of a link graph ("what
 * is this connected to?") rather than the one the full graph answers ("what
 * does everything look like at once?").
 *
 * Edges are followed in both directions. A citation is a relationship
 * regardless of who wrote it down, and a one-way walk would hide every
 * document that references the one you are standing on — usually the more
 * interesting half.
 */

export interface Hop {
  from: string;
  to: string;
}

/** Ids within `hops` steps of `origin`, including the origin itself.
 *  `hops` of 0 is just the origin; negative or non-finite is treated as 0. */
export function neighborhood(
  origin: string,
  edges: Hop[],
  hops: number,
): Set<string> {
  const seen = new Set<string>([origin]);
  const depth = Number.isFinite(hops) ? Math.max(0, Math.floor(hops)) : 0;
  if (depth === 0) return seen;

  // Adjacency once, rather than rescanning every edge per level: a wide
  // neighbourhood on a big notebook otherwise walks the edge list once per
  // hop per frontier node.
  const adjacent = new Map<string, string[]>();
  const link = (a: string, b: string) => {
    const list = adjacent.get(a);
    if (list) list.push(b);
    else adjacent.set(a, [b]);
  };
  for (const e of edges) {
    link(e.from, e.to);
    link(e.to, e.from);
  }

  let frontier = [origin];
  for (let i = 0; i < depth && frontier.length; i++) {
    const next: string[] = [];
    for (const id of frontier) {
      for (const other of adjacent.get(id) ?? []) {
        if (seen.has(other)) continue;
        seen.add(other);
        next.push(other);
      }
    }
    frontier = next;
  }
  return seen;
}
