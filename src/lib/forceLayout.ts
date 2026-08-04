/**
 * A small force-directed layout for the notebook graph
 * (docs/RFC-document-surface.md phase 5).
 *
 * Hand-rolled rather than pulling in d3-force: the whole simulation is three
 * forces and a cooling schedule, it runs to completion in one synchronous
 * pass, and a notebook graph is hundreds of nodes, not millions. Pure
 * functions over plain objects, so it can be tested and previewed without a
 * browser.
 *
 * Deterministic on purpose — no Math.random. Seeded placement means the same
 * notebook lays out the same way every time you open it, which matters more
 * for a thing you navigate by memory than a prettier random spread would.
 */

export interface LayoutNode {
  id: string;
  x: number;
  y: number;
  /** Edge count, used to weight a node's mass and radius. */
  degree: number;
}

export interface LayoutEdge {
  from: string;
  to: string;
}

/** Ticks to run for a small graph. Enough to settle completely. */
const TICKS = 320;
/** Fewest ticks we will ever run — below this the layout is visibly unsettled,
 *  and a rough graph still beats none. */
const MIN_TICKS = 70;
/** Roughly how many node-pair force calculations to spend on one layout.
 *  The simulation is O(n^2) per tick and runs synchronously, so without a
 *  budget a large notebook would freeze the pane for seconds on open. At
 *  this figure a 400-node graph lands around 150ms. Measured, not guessed:
 *  320 ticks x 400 nodes was 350ms, and that is already a visible hitch. */
const WORK_BUDGET = 320 * 200 * 200;
/** How hard unconnected nodes push apart. */
const REPULSION = 5200;
/** Spring constant along an edge. */
const ATTRACTION = 0.0016;
/** Resting distance for a linked pair. */
const IDEAL_EDGE = 90;
/** Pull toward the middle, so disconnected islands don't drift off-canvas. */
const CENTERING = 0.012;
/** Velocity retained per tick. */
const DAMPING = 0.82;
/** Never let one tick move a node further than this — a pair that starts
 *  nearly coincident would otherwise fling itself off the canvas. */
const MAX_STEP = 24;

/** A simulation you drive yourself, a slice at a time.
 *
 *  Running all the ticks in one call blocks the main thread for as long as
 *  it takes, which on a few hundred nodes is long enough that no progress
 *  can be drawn and the window looks hung. Stepping lets the caller spend a
 *  few milliseconds per frame, paint a real fraction, and stay responsive.
 */
export interface LayoutRun {
  /** Advance up to `ticks`. Returns true once the simulation is finished. */
  step: (ticks: number) => boolean;
  /** 0..1 — genuine, not an animation. */
  progress: () => number;
  /** Final positions, fitted to the box. Call after step returns true. */
  result: () => LayoutNode[];
}

/**
 * Lay out a graph inside a `width` x `height` box, all at once.
 * Convenience wrapper over `createLayout` for callers that can block.
 */
export function layout(
  nodes: { id: string; degree: number }[],
  edges: LayoutEdge[],
  width: number,
  height: number,
): LayoutNode[] {
  const run = createLayout(nodes, edges, width, height);
  while (!run.step(Number.MAX_SAFE_INTEGER));
  return run.result();
}

/** The same simulation, driven a slice at a time. */
export function createLayout(
  nodes: { id: string; degree: number }[],
  edges: LayoutEdge[],
  width: number,
  height: number,
): LayoutRun {
  if (nodes.length === 0) {
    return { step: () => true, progress: () => 1, result: () => [] };
  }
  const cx = width / 2;
  const cy = height / 2;

  // Seeded ring placement: deterministic, and starting on a circle rather
  // than a point means the repulsion force has a direction to work with.
  const radius = Math.min(width, height) * 0.36;
  const placed: LayoutNode[] = nodes.map((n, i) => {
    const angle = (i / nodes.length) * Math.PI * 2;
    return {
      id: n.id,
      degree: n.degree,
      x: cx + Math.cos(angle) * radius,
      y: cy + Math.sin(angle) * radius,
    };
  });
  if (nodes.length === 1) {
    return { step: () => true, progress: () => 1, result: () => placed };
  }

  const index = new Map(placed.map((n, i) => [n.id, i]));
  const vx = new Float64Array(placed.length);
  const vy = new Float64Array(placed.length);
  // Well-connected nodes should move less, so hubs settle in the middle and
  // leaves swing around them.
  const mass = placed.map((n) => 1 + n.degree * 0.5);

  const ticks = Math.max(
    MIN_TICKS,
    Math.min(TICKS, Math.round(WORK_BUDGET / (placed.length * placed.length))),
  );
  let tick = 0;

  const runTicks = (budget: number) => {
    const end = Math.min(ticks, tick + budget);
    for (; tick < end; tick++) {
      // Cooling: large rearrangements early, fine settling late.
      const cool = 1 - tick / ticks;

      for (let i = 0; i < placed.length; i++) {
        let fx = 0;
        let fy = 0;

        // Repulsion, every pair.
        for (let j = 0; j < placed.length; j++) {
          if (i === j) continue;
          let dx = placed[i].x - placed[j].x;
          let dy = placed[i].y - placed[j].y;
          let d2 = dx * dx + dy * dy;
          if (d2 < 0.01) {
            // Coincident: nudge apart along a stable, index-derived direction
            // rather than at random, to keep the layout reproducible.
            dx = (i % 2 === 0 ? 1 : -1) * 0.1;
            dy = (j % 2 === 0 ? 1 : -1) * 0.1;
            d2 = dx * dx + dy * dy;
          }
          const force = REPULSION / d2;
          const d = Math.sqrt(d2);
          fx += (dx / d) * force;
          fy += (dy / d) * force;
        }

        // Centering.
        fx += (cx - placed[i].x) * CENTERING;
        fy += (cy - placed[i].y) * CENTERING;

        vx[i] = (vx[i] + fx / mass[i]) * DAMPING;
        vy[i] = (vy[i] + fy / mass[i]) * DAMPING;
      }

      // Attraction along edges, applied to both ends.
      for (const e of edges) {
        const a = index.get(e.from);
        const b = index.get(e.to);
        if (a === undefined || b === undefined || a === b) continue;
        const dx = placed[b].x - placed[a].x;
        const dy = placed[b].y - placed[a].y;
        const d = Math.sqrt(dx * dx + dy * dy) || 1;
        const force = (d - IDEAL_EDGE) * ATTRACTION * d;
        const ux = (dx / d) * force;
        const uy = (dy / d) * force;
        vx[a] += ux / mass[a];
        vy[a] += uy / mass[a];
        vx[b] -= ux / mass[b];
        vy[b] -= uy / mass[b];
      }

      for (let i = 0; i < placed.length; i++) {
        const step = Math.hypot(vx[i], vy[i]) * cool;
        const scale = step > MAX_STEP ? MAX_STEP / step : 1;
        placed[i].x += vx[i] * cool * scale;
        placed[i].y += vy[i] * cool * scale;
      }
    }
    return tick >= ticks;
  };

  return {
    step: runTicks,
    progress: () => tick / ticks,
    result: () => fit(placed, width, height),
  };
}

/** Scale and translate the settled layout to fill the box with a margin. */
function fit(nodes: LayoutNode[], width: number, height: number): LayoutNode[] {
  const margin = 48;
  const xs = nodes.map((n) => n.x);
  const ys = nodes.map((n) => n.y);
  const minX = Math.min(...xs);
  const maxX = Math.max(...xs);
  const minY = Math.min(...ys);
  const maxY = Math.max(...ys);
  const spanX = maxX - minX || 1;
  const spanY = maxY - minY || 1;
  // One scale for both axes — independent scaling would shear the layout and
  // make edge lengths lie about relatedness.
  const scale = Math.min(
    (width - margin * 2) / spanX,
    (height - margin * 2) / spanY,
    // Never blow a tiny graph up to fill the pane; two linked notes should
    // sit near each other, not at opposite corners.
    1.6,
  );
  const offsetX = (width - spanX * scale) / 2;
  const offsetY = (height - spanY * scale) / 2;
  return nodes.map((n) => ({
    ...n,
    x: (n.x - minX) * scale + offsetX,
    y: (n.y - minY) * scale + offsetY,
  }));
}
