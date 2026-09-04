/* Flattening a folder into panel rows (alchemy-release-dbk).

   The Sources panel used to emit a folder's children one level deep, while
   the renderer drew a working disclosure chevron on any row that had
   children. On a folder nested inside a folder — which an OKF bundle
   restores whenever it was exported that way — clicking that chevron
   rotated the caret over a subtree that was never a candidate for a row.
   The collapse that did nothing.

   Kept here, apart from the component, because it is the one piece of the
   panel that is pure: sources in, rows out. */

/** The shape the walk needs: an id, and children found by parent id. */
export interface RowNode {
  id: string;
}

export interface SubtreeRow<T> {
  s: T;
  /** 0 for a top-level source, +1 per folder above it. */
  depth: number;
}

export interface SubtreeOpts<T> {
  /** Is this container closed? Asked only when it has children. */
  collapsed(s: T, kidCount: number): boolean;
  /** When a filter is on: does this source match it? A subtree with no
   *  match anywhere in it is dropped, and a filter expands every folder it
   *  reaches into — a match hidden under a closed folder reads as no match
   *  at all. Undefined means no filter. */
  matches?(s: T): boolean;
}

/** A source and everything under it, flattened depth-first. */
export function sourceSubtree<T extends RowNode>(
  root: T,
  childrenOf: Map<string, T[]>,
  opts: SubtreeOpts<T>,
): SubtreeRow<T>[] {
  const out: SubtreeRow<T>[] = [];
  const seen = new Set<string>();
  const walk = (s: T, depth: number): boolean => {
    // A restored parent chain that points back at itself would otherwise
    // recurse until the stack gives out.
    if (seen.has(s.id)) return false;
    seen.add(s.id);
    const kids = childrenOf.get(s.id) ?? [];
    const at = out.length;
    out.push({ s, depth });
    const expanded = !!opts.matches || !opts.collapsed(s, kids.length);
    let kept = 0;
    if (expanded) for (const c of kids) if (walk(c, depth + 1)) kept++;
    if (opts.matches && kept === 0 && !opts.matches(s)) {
      out.length = at;
      return false;
    }
    return true;
  };
  walk(root, 0);
  return out;
}
