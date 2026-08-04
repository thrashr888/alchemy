/**
 * Which graph labels can be drawn without landing on top of each other.
 *
 * A dense notebook puts more titles on screen than there is room for, and
 * overlapping text is worse than absent text — it makes both labels
 * unreadable instead of one. So labels are placed greedily, most-connected
 * first: a hub keeps its name, and a leaf that would collide with one goes
 * quiet until you zoom in or hover it.
 *
 * Pure and layout-space, so it runs once per layout rather than per frame.
 */

export interface LabelBox {
  id: string;
  /** Label centre, in layout coordinates. */
  x: number;
  y: number;
  text: string;
  /** Higher wins a collision. Node degree, in practice. */
  weight: number;
}

/** Rough width of the label font (text-micro, ~10px) per character. Measuring
 *  properly would mean a canvas per layout for a decision that only needs to
 *  be about right — a slightly generous estimate errs toward hiding a label,
 *  which is the safe direction. */
const CHAR_WIDTH = 5.4;
const LINE_HEIGHT = 11;
/** Breathing room so two labels never quite touch. */
const PADDING = 3;

/** Ids whose labels are safe to draw. */
export function placeLabels(labels: LabelBox[]): Set<string> {
  // Most connected first, ties broken by id so the result is stable across
  // renders — a label that flickers on every repaint is worse than one that
  // is consistently hidden.
  const ordered = [...labels].sort(
    (a, b) => b.weight - a.weight || a.id.localeCompare(b.id),
  );

  const placed: { x1: number; y1: number; x2: number; y2: number }[] = [];
  const visible = new Set<string>();

  for (const label of ordered) {
    if (!label.text) continue;
    const halfWidth = (label.text.length * CHAR_WIDTH) / 2 + PADDING;
    const box = {
      x1: label.x - halfWidth,
      y1: label.y - LINE_HEIGHT / 2 - PADDING,
      x2: label.x + halfWidth,
      y2: label.y + LINE_HEIGHT / 2 + PADDING,
    };
    const collides = placed.some(
      (p) => box.x1 < p.x2 && box.x2 > p.x1 && box.y1 < p.y2 && box.y2 > p.y1,
    );
    if (collides) continue;
    placed.push(box);
    visible.add(label.id);
  }
  return visible;
}
