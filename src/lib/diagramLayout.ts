/**
 * Placement for architecture diagrams (docs/RFC-diagrams.md).
 *
 * eraser-diagrams routes lines and paints components but treats entity
 * positions as input — nothing in it places a node. The generator writes
 * topology only, so something has to turn "api is in the backend group and
 * talks to the database" into coordinates. This does, and nothing else: a
 * layered layout that runs the same way every time, with no DOM (it is unit
 * tested in node) and no model call.
 *
 * Each container is laid out on its own, recursively, then sized around its
 * members; siblings at one level are ranked by the connections between
 * them (longest path through the lifted edges, cycles broken in DFS order)
 * and stacked as bands along the main axis. Swimlanes are the exception
 * that proves the rule: a Lane runs its steps across the scene's flow, and
 * every lane in the scene — siblings, or lanes nested in different pools —
 * shares one set of columns, so a step's column is its place in the whole
 * process, not just in its lane. A rank that a labeled connection leaves
 * gets extra room, since eraser places the label mid-line and a tight gap
 * puts it on the next box or a group's title. The renderer measures what
 * it drew and the caller runs placement again with real sizes, so the
 * sizes given here are floors and estimates, never the truth.
 */

export interface LayoutNode {
  id: string;
  containerId?: string | null;
  /** Holds other nodes (a Group, Lane, or Pool). */
  container: boolean;
  /** Containers only: the axis their members flow along, when it differs
   *  from the scene's. A swimlane stacks with its siblings but runs its
   *  steps the other way. */
  flow?: "down" | "right";
  /** Containers only: the title is a vertical band on the left edge (a
   *  Lane or Pool) rather than a chip along the top (a Group). */
  band?: boolean;
  /** Containers only: a swimlane whose steps share columns with every
   *  other lane in the scene, at any depth — global step order. */
  lane?: boolean;
  /** Containers only: members stack along the flow in input order, edges
   *  or no edges — a journey stage listing its touchpoint, then its pain
   *  points. A level holding lanes behaves this way on its own. */
  sequence?: boolean;
  /** Estimated or measured size. A container's is its minimum. */
  width: number;
  height: number;
}

export interface LayoutEdge {
  from: string;
  to: string;
  /** Carries a label eraser will place mid-line. */
  labeled?: boolean;
}

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface LayoutOptions {
  /** `down`: ranks are rows; `right`: ranks are columns. */
  direction: "down" | "right";
  /** Space between siblings and between ranks. */
  gap?: number;
  /** Inset from a container's edge to its members. */
  padding?: number;
  /** Extra inset for a container's title: below a Group's chip, right of a
   *  Lane's band. */
  titleInset?: number;
  /** Margin around the whole scene. */
  margin?: number;
  /** Extra room after a rank that a labeled connection leaves. */
  labelRoom?: number;
  /** How a rank's members line up across the main axis: centered on the
   *  widest rank (the default), or flush with its start — columns of a
   *  journey map share a top edge. */
  align?: "center" | "start";
  /** Where a level's disconnected islands go: side by side across the
   *  main axis, all starting at rank 0 (the default — unrelated tiers of
   *  an architecture sit beside each other), or chained along it, each
   *  island starting at the rank after the last one ends — a relationship
   *  map's eras read left to right instead of stacking into a column. */
  islands?: "across" | "along";
}

export interface Placement {
  boxes: Map<string, Box>;
  width: number;
  height: number;
}

interface Level {
  /** Child id → box relative to the level's content origin. */
  rel: Map<string, Box>;
  width: number;
  height: number;
}

/** Columns sibling swimlanes agree on: each member's rank and where that
 *  rank starts along the lanes' flow. */
interface SharedRanks {
  ranks: Map<string, number>;
  offsets: number[];
  length: number;
}

type Direction = "down" | "right";

/** Walk containment upward until it reaches `top` (or the root). */
function ancestorAt(
  id: string,
  top: string | null,
  parentOf: Map<string, string | null>,
): string | undefined {
  let current: string | undefined = id;
  for (let hops = 0; current !== undefined && hops <= parentOf.size; hops += 1) {
    const parent = parentOf.get(current);
    if (parent === undefined) return undefined; // unknown id
    if (parent === top) return current;
    current = parent ?? undefined;
  }
  return undefined;
}

/**
 * Longest-path ranks over a DAG made by dropping back edges found in a DFS
 * from each node in input order. Every node gets a rank; a cycle just
 * loses the edge that closes it.
 */
/** A lifted edge: endpoints at one level, and the room its label needs
 *  between ranks (0 when unlabeled). */
type Edge = [from: string, to: string, room: number];

function rank(ids: string[], edges: Edge[]): Map<string, number> {
  const out = new Map<string, string[]>(ids.map((id) => [id, []]));
  for (const [from, to] of edges) {
    if (from !== to && out.has(from) && out.has(to)) out.get(from)?.push(to);
  }
  const state = new Map<string, 0 | 1 | 2>();
  const forward = new Map<string, string[]>(ids.map((id) => [id, []]));
  const visit = (id: string) => {
    state.set(id, 1);
    for (const next of out.get(id) ?? []) {
      const s = state.get(next) ?? 0;
      if (s === 1) continue; // back edge: closes a cycle
      forward.get(id)?.push(next);
      if (s === 0) visit(next);
    }
    state.set(id, 2);
  };
  for (const id of ids) if (!state.get(id)) visit(id);

  const indegree = new Map<string, number>(ids.map((id) => [id, 0]));
  for (const targets of forward.values())
    for (const t of targets) indegree.set(t, (indegree.get(t) ?? 0) + 1);
  const ranks = new Map<string, number>(ids.map((id) => [id, 0]));
  const queue = ids.filter((id) => indegree.get(id) === 0);
  while (queue.length) {
    const id = queue.shift() as string;
    for (const next of forward.get(id) ?? []) {
      ranks.set(next, Math.max(ranks.get(next) ?? 0, (ranks.get(id) ?? 0) + 1));
      indegree.set(next, (indegree.get(next) ?? 1) - 1);
      if (indegree.get(next) === 0) queue.push(next);
    }
  }
  return ranks;
}

/**
 * Shift each connected island's ranks so the islands follow one another
 * along the main axis, in the order their first member appears.
 */
function chainIslands(ids: string[], ranks: Map<string, number>, edges: Edge[]): void {
  const parent = new Map<string, string>(ids.map((id) => [id, id]));
  const find = (id: string): string => {
    let root = id;
    while (parent.get(root) !== root) root = parent.get(root) as string;
    return root;
  };
  for (const [from, to] of edges) {
    if (!parent.has(from) || !parent.has(to)) continue;
    const a = find(from);
    const b = find(to);
    if (a !== b) parent.set(b, a);
  }
  const offset = new Map<string, number>();
  let next = 0;
  for (const id of ids) {
    const root = find(id);
    if (!offset.has(root)) {
      offset.set(root, next);
      const depth = Math.max(0, ...ids.filter((m) => find(m) === root).map((m) => ranks.get(m) ?? 0));
      next += depth + 1;
    }
  }
  for (const id of ids) ranks.set(id, (ranks.get(id) ?? 0) + (offset.get(find(id)) ?? 0));
}

/** Ranks → ordered layers, each sorted by the mean position of its predecessors. */
function layers(ids: string[], ranks: Map<string, number>, edges: Edge[]): string[][] {
  const depth = Math.max(0, ...ids.map((id) => ranks.get(id) ?? 0));
  const result: string[][] = Array.from({ length: depth + 1 }, () => []);
  for (const id of ids) result[ranks.get(id) ?? 0].push(id);
  const position = new Map<string, number>();
  result[0].forEach((id, i) => position.set(id, i));
  for (let r = 1; r <= depth; r += 1) {
    const layer = result[r];
    const key = new Map<string, number>();
    layer.forEach((id, i) => {
      const preds = edges
        .filter(([, to]) => to === id)
        .map(([from]) => position.get(from))
        .filter((p): p is number => p !== undefined);
      key.set(id, preds.length ? preds.reduce((a, b) => a + b, 0) / preds.length : i);
    });
    layer.sort((a, b) => (key.get(a) ?? 0) - (key.get(b) ?? 0));
    layer.forEach((id, i) => position.set(id, i));
  }
  return result;
}

export function placeNodes(
  nodes: LayoutNode[],
  edges: LayoutEdge[],
  options: LayoutOptions,
): Placement {
  const gap = options.gap ?? 64;
  const padding = options.padding ?? 28;
  const titleInset = options.titleInset ?? 40;
  const margin = options.margin ?? 24;
  const labelRoom = options.labelRoom ?? 28;
  const align = options.align ?? "center";

  const byId = new Map(nodes.map((n) => [n.id, n]));
  // Parent of each node: the named container when it exists and is one,
  // else the root. A containment cycle is cut at the node that closes it.
  const parentOf = new Map<string, string | null>();
  for (const node of nodes) {
    const parent = node.containerId ?? null;
    const target = parent === null ? undefined : byId.get(parent);
    parentOf.set(node.id, target?.container && parent !== node.id ? parent : null);
  }
  for (const node of nodes) {
    const seen = new Set<string>([node.id]);
    let current = parentOf.get(node.id) ?? null;
    while (current !== null) {
      if (seen.has(current)) {
        parentOf.set(node.id, null);
        break;
      }
      seen.add(current);
      current = parentOf.get(current) ?? null;
    }
  }

  const children = new Map<string | null, LayoutNode[]>();
  for (const node of nodes) {
    const parent = parentOf.get(node.id) ?? null;
    if (!children.has(parent)) children.set(parent, []);
    children.get(parent)?.push(node);
  }

  const size = new Map<string, { width: number; height: number }>();
  const levels = new Map<string | null, Level>();
  /** A container's title takes room on one edge: below a Group's chip, or
   *  right of a Lane's vertical band. */
  const inset = (node: LayoutNode) =>
    node.band ? { left: titleInset, top: 0 } : { left: 0, top: titleInset };

  /** Where a rank's band starts along the main axis, given each band's
   *  extent and the label room each boundary needs. */
  const offsetsOf = (bands: number[], roomAfter: number[]) => {
    const offsets: number[] = [];
    let cursor = 0;
    bands.forEach((b, r) => {
      offsets[r] = cursor;
      cursor += (b ?? 0) + gap + (roomAfter[r] ?? 0);
    });
    const last = bands.length - 1;
    const length = last < 0 ? 0 : offsets[last] + (bands[last] ?? 0);
    return { offsets, length };
  };

  /** The label room each rank boundary needs: after rank r, the most any
   *  labeled edge running from rank ≤ r to rank > r (or back) asks for. */
  const labeledBoundaries = (ranks: Map<string, number>, ranked: Edge[]) => {
    const after: number[] = [];
    for (const [from, to, room] of ranked) {
      if (!room) continue;
      const a = ranks.get(from);
      const b = ranks.get(to);
      if (a === undefined || b === undefined || a === b) continue;
      for (let r = Math.min(a, b); r < Math.max(a, b); r += 1)
        after[r] = Math.max(after[r] ?? 0, room);
    }
    return after;
  };

  /** Room for an edge's label between ranks. eraser sets the label at the
   *  path's midpoint; when an end sits inside a container at this level,
   *  the path runs on through that container's padding — and, entering,
   *  its title when the title is on that edge — before it reaches the box,
   *  plus whatever sideways jog the router adds, and the midpoint drifts
   *  out of the gap onto the title unless the gap grows by as much. The
   *  whole run is counted, not the difference: a label clear of a title is
   *  worth a taller diagram. */
  const labelRoomFor = (edge: LayoutEdge, from: string, to: string, down: boolean) => {
    if (!edge.labeled) return 0;
    // A path leaves a container by its far edge, which holds no title;
    // it enters the next by the near edge, which may.
    const outward = edge.from === from ? 0 : padding;
    const inward = () => {
      if (edge.to === to) return 0;
      const holder = byId.get(to);
      const band = holder ? inset(holder) : { left: 0, top: 0 };
      return padding + (down ? band.top : band.left);
    };
    return labelRoom + outward + inward();
  };

  // Every lane in the scene — siblings or lanes in different pools — ranks
  // its steps together and gives each rank one column, so a step's column
  // is its place in the whole process. Computed once, on first use.
  let laneColumns: SharedRanks | undefined;
  const sharedLanes = (): SharedRanks => {
    if (laneColumns) return laneColumns;
    const lanes = nodes.filter((n) => n.container && n.lane);
    const laneIds = new Set(lanes.map((l) => l.id));
    const steps = nodes.filter((n) => laneIds.has(parentOf.get(n.id) ?? ""));
    const laneFlow = lanes[0]?.flow ?? options.direction;
    for (const step of steps) ensureSize(step, laneFlow);
    const stepIds = new Set(steps.map((s) => s.id));
    const stepEdges = edges
      .filter((e) => stepIds.has(e.from) && stepIds.has(e.to))
      .map((e): Edge => [e.from, e.to, e.labeled ? labelRoom : 0]);
    const ranks = rank([...stepIds], stepEdges);
    const laneDown = laneFlow === "down";
    const band: number[] = [];
    for (const step of steps) {
      const r = ranks.get(step.id) ?? 0;
      const d = size.get(step.id) ?? { width: 0, height: 0 };
      band[r] = Math.max(band[r] ?? 0, laneDown ? d.height : d.width);
    }
    for (let r = 0; r < band.length; r += 1) band[r] ??= 0;
    const { offsets, length } = offsetsOf(band, labeledBoundaries(ranks, stepEdges));
    laneColumns = { ranks, offsets, length };
    return laneColumns;
  };

  const ensureSize = (member: LayoutNode, direction: Direction) => {
    if (size.has(member.id)) return;
    if (member.container) {
      const inner = layoutLevel(
        member.id,
        member.flow ?? direction,
        member.lane ? sharedLanes() : undefined,
      );
      const band = inset(member);
      size.set(member.id, {
        width: Math.max(member.width, inner.width + padding * 2 + band.left),
        height: Math.max(member.height, inner.height + padding * 2 + band.top),
      });
    } else {
      size.set(member.id, { width: member.width, height: member.height });
    }
  };

  const layoutLevel = (top: string | null, direction: Direction, shared?: SharedRanks): Level => {
    const down = direction === "down";
    const members = children.get(top) ?? [];
    const along = (id: string) => {
      const d = size.get(id) ?? { width: 0, height: 0 };
      return down ? d.height : d.width;
    };
    const across = (id: string) => {
      const d = size.get(id) ?? { width: 0, height: 0 };
      return down ? d.width : d.height;
    };

    for (const member of members) ensureSize(member, direction);

    const ids = members.map((m) => m.id);
    const lifted: Edge[] = [];
    for (const edge of edges) {
      const from = ancestorAt(edge.from, top, parentOf);
      const to = ancestorAt(edge.to, top, parentOf);
      if (from && to && from !== to) lifted.push([from, to, labelRoomFor(edge, from, to, down)]);
    }
    // Swimlanes stack, one band each, in the order the document lists
    // them — never side by side because no edge happened to rank them.
    const sequence = byId.get(top ?? "")?.sequence || members.some((m) => m.lane);
    const ranks = shared
      ? new Map(ids.map((id) => [id, shared.ranks.get(id) ?? 0]))
      : sequence
        ? new Map(ids.map((id, i) => [id, i]))
        : rank(ids, lifted);
    if (!shared && !sequence && options.islands === "along") chainIslands(ids, ranks, lifted);
    const ordered = layers(ids, ranks, lifted);
    const rel = new Map<string, Box>();
    // Along the main axis each rank is one band; across it, the rank's
    // members sit side by side, centered on the widest rank.
    const bandAcross = ordered.map(
      (layer) => layer.reduce((sum, id) => sum + across(id), 0) + gap * Math.max(0, layer.length - 1),
    );
    const maxAcross = Math.max(0, ...bandAcross);
    const bandAlong = ordered.map((layer) => Math.max(0, ...layer.map(along)));
    const { offsets, length } = shared
      ? shared
      : offsetsOf(bandAlong, labeledBoundaries(ranks, lifted));
    ordered.forEach((layer, i) => {
      if (!layer.length) return;
      const mainCursor = offsets[i] ?? 0;
      let crossCursor = align === "start" ? 0 : (maxAcross - bandAcross[i]) / 2;
      for (const id of layer) {
        const { width, height } = size.get(id) ?? { width: 0, height: 0 };
        rel.set(id, {
          x: Math.round(down ? crossCursor : mainCursor),
          y: Math.round(down ? mainCursor : crossCursor),
          width,
          height,
        });
        crossCursor += across(id) + gap;
      }
    });
    const level: Level = {
      rel,
      width: down ? maxAcross : length,
      height: down ? length : maxAcross,
    };
    levels.set(top, level);
    return level;
  };

  const root = layoutLevel(null, options.direction);
  const boxes = new Map<string, Box>();
  const place = (top: string | null, originX: number, originY: number) => {
    const level = levels.get(top);
    if (!level) return;
    for (const [id, box] of level.rel) {
      const abs = { ...box, x: box.x + originX, y: box.y + originY };
      boxes.set(id, abs);
      const node = byId.get(id);
      if (node?.container) {
        const band = inset(node);
        place(id, abs.x + padding + band.left, abs.y + padding + band.top);
      }
    }
  };
  place(null, margin, margin);

  return {
    boxes,
    width: Math.round(root.width + margin * 2),
    height: Math.round(root.height + margin * 2),
  };
}

const ICON_PX: Record<string, number> = { sm: 32, md: 50, lg: 72, xl: 100 };

/**
 * A first-pass size for an eraser entity, before the renderer has measured
 * anything: close enough that the second pass moves things a little, not a
 * lot. Containers report their minimum; their members decide the rest. For
 * a Shape this doubles as the authored minimum — eraser wraps a Shape's
 * text at 100px unless it is given a width, and its stock examples always
 * are.
 */
export function estimateSize(entity: {
  tag: string;
  [prop: string]: unknown;
}): { width: number; height: number } {
  const texts = Array.isArray(entity.texts)
    ? entity.texts
        .map((t) => (t && typeof t === "object" && "text" in t ? String(t.text) : ""))
        .filter(Boolean)
    : [];
  const longest = Math.max(0, ...texts.flatMap((t) => t.split("\n")).map((l) => l.length));
  const lines = texts.reduce((n, t) => n + t.split("\n").length, 0);
  switch (entity.tag) {
    case "Icon": {
      const px = ICON_PX[String(entity.size ?? "md")] ?? 50;
      return { width: Math.max(px, longest * 7 + 8), height: px + (texts.length ? 24 : 0) };
    }
    case "Textbox": {
      const text = String(entity.text ?? "");
      return { width: Math.min(260, text.length * 7 + 16), height: 22 * (1 + Math.floor(text.length / 36)) };
    }
    case "Group":
    case "Lane":
    case "Pool":
      return { width: 160, height: 80 };
    case "Activity":
      return {
        width: Math.min(260, Math.max(120, longest * 7.5 + 48 + (entity.icon ? 30 : 0))),
        height: 56 + 18 * Math.max(0, lines - 1),
      };
    case "Event":
    case "Gateway":
      // A 56px disc or diamond with its caption hanging below.
      return { width: Math.max(56, longest * 7 + 8), height: 56 + (texts.length ? 24 : 0) };
    case "DatabaseTable": {
      const fields = Array.isArray(entity.fields) ? entity.fields : [];
      const rows = fields.map((f) =>
        f && typeof f === "object"
          ? ["name", "type", "meta"]
              .map((k) => (k in f ? String((f as Record<string, unknown>)[k] ?? "") : ""))
              .join("  ")
          : "",
      );
      const widest = Math.max(String(entity.label ?? "").length + 4, ...rows.map((r) => r.length));
      return {
        width: Math.min(360, Math.max(160, widest * 7.2 + 32)),
        height: 40 + 24 * fields.length,
      };
    }
    case "Shape": {
      const tall = entity.shape === "cylinder" || entity.shape === "diamond";
      const iconRoom = entity.icon ? 36 : 0;
      return {
        width: Math.min(280, Math.max(140, longest * 7.5 + 48 + iconRoom)),
        height: (tall ? 88 : 64) + 18 * Math.max(0, lines - 1),
      };
    }
    default:
      return { width: 140, height: 60 };
  }
}
