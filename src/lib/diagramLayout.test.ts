import { describe, expect, it } from "vitest";
import { estimateSize, placeNodes, type Box, type LayoutNode } from "./diagramLayout";

const leaf = (id: string, containerId?: string): LayoutNode => ({
  id,
  containerId,
  container: false,
  width: 100,
  height: 50,
});
const group = (id: string, containerId?: string): LayoutNode => ({
  id,
  containerId,
  container: true,
  width: 0,
  height: 0,
});

const overlaps = (a: Box, b: Box) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

const inside = (inner: Box, outer: Box) =>
  inner.x >= outer.x &&
  inner.y >= outer.y &&
  inner.x + inner.width <= outer.x + outer.width &&
  inner.y + inner.height <= outer.y + outer.height;

describe("placeNodes", () => {
  it("stacks a chain along the main axis without overlaps", () => {
    const nodes = [leaf("a"), leaf("b"), leaf("c")];
    const edges = [
      { from: "a", to: "b" },
      { from: "b", to: "c" },
    ];
    const down = placeNodes(nodes, edges, { direction: "down" });
    const [a, b, c] = ["a", "b", "c"].map((id) => down.boxes.get(id) as Box);
    expect(a.y).toBeLessThan(b.y);
    expect(b.y).toBeLessThan(c.y);
    expect(a.x).toBe(b.x);
    expect(overlaps(a, b)).toBe(false);
    expect(overlaps(b, c)).toBe(false);

    const right = placeNodes(nodes, edges, { direction: "right" });
    const [ra, rb, rc] = ["a", "b", "c"].map((id) => right.boxes.get(id) as Box);
    expect(ra.x).toBeLessThan(rb.x);
    expect(rb.x).toBeLessThan(rc.x);
    expect(ra.y).toBe(rb.y);
    expect(right.width).toBeGreaterThan(right.height);
  });

  it("sizes a container around its members, below the title band", () => {
    const nodes = [group("g"), leaf("a", "g"), leaf("b", "g"), leaf("out")];
    const placed = placeNodes(nodes, [{ from: "a", to: "b" }, { from: "out", to: "a" }], {
      direction: "down",
      padding: 20,
      titleInset: 30,
    });
    const g = placed.boxes.get("g") as Box;
    const a = placed.boxes.get("a") as Box;
    const b = placed.boxes.get("b") as Box;
    const out = placed.boxes.get("out") as Box;
    expect(inside(a, g)).toBe(true);
    expect(inside(b, g)).toBe(true);
    expect(a.y - g.y).toBeGreaterThanOrEqual(50); // padding + title band
    expect(a.x - g.x).toBe(20);
    expect(overlaps(out, g)).toBe(false);
    // `out` feeds the group, so it ranks above it.
    expect(out.y).toBeLessThan(g.y);
    expect(placed.width).toBe(Math.max(g.x + g.width, out.x + out.width) + 24);
  });

  it("places siblings in one rank side by side, centered on the widest rank", () => {
    const nodes = [leaf("src"), leaf("x"), leaf("y"), leaf("z")];
    const edges = [
      { from: "src", to: "x" },
      { from: "src", to: "y" },
      { from: "src", to: "z" },
    ];
    const placed = placeNodes(nodes, edges, { direction: "down", gap: 10 });
    const src = placed.boxes.get("src") as Box;
    const row = ["x", "y", "z"].map((id) => placed.boxes.get(id) as Box);
    expect(new Set(row.map((b) => b.y)).size).toBe(1);
    expect(row[0].x + row[0].width + 10).toBe(row[1].x);
    // The lone source centers over the row of three.
    const rowMid = (row[0].x + row[2].x + row[2].width) / 2;
    expect(Math.abs(src.x + src.width / 2 - rowMid)).toBeLessThanOrEqual(1);
  });

  it("survives cycles and unknown containers", () => {
    const nodes = [leaf("a"), leaf("b", "nowhere"), leaf("c", "a")];
    const edges = [
      { from: "a", to: "b" },
      { from: "b", to: "c" },
      { from: "c", to: "a" },
    ];
    const placed = placeNodes(nodes, edges, { direction: "down" });
    expect(placed.boxes.size).toBe(3);
    const boxes = [...placed.boxes.values()];
    for (const p of boxes) for (const q of boxes) if (p !== q) expect(overlaps(p, q)).toBe(false);
  });

  it("cuts a containment cycle instead of looping", () => {
    const nodes = [group("g1", "g2"), group("g2", "g1"), leaf("a", "g1")];
    const placed = placeNodes(nodes, [], { direction: "down" });
    expect(placed.boxes.size).toBe(3);
    expect(inside(placed.boxes.get("a") as Box, placed.boxes.get("g1") as Box)).toBe(true);
  });

  it("lets a swimlane run its steps across the scene's flow", () => {
    const lane: LayoutNode = { ...group("lane"), flow: "right" };
    const nodes = [lane, leaf("s1", "lane"), leaf("s2", "lane"), leaf("s3", "lane")];
    const edges = [
      { from: "s1", to: "s2" },
      { from: "s2", to: "s3" },
    ];
    const placed = placeNodes(nodes, edges, { direction: "down" });
    const [s1, s2, s3] = ["s1", "s2", "s3"].map((id) => placed.boxes.get(id) as Box);
    expect(s1.y).toBe(s2.y);
    expect(s2.y).toBe(s3.y);
    expect(s1.x).toBeLessThan(s2.x);
    expect(s2.x).toBeLessThan(s3.x);
    const box = placed.boxes.get("lane") as Box;
    expect(box.width).toBeGreaterThan(box.height);
  });

  it("is deterministic", () => {
    const nodes = [group("g"), leaf("a", "g"), leaf("b", "g"), leaf("c"), leaf("d")];
    const edges = [
      { from: "c", to: "a" },
      { from: "a", to: "b" },
      { from: "b", to: "d" },
    ];
    const one = placeNodes(nodes, edges, { direction: "right" });
    const two = placeNodes(nodes, edges, { direction: "right" });
    expect([...one.boxes]).toEqual([...two.boxes]);
  });
});

describe("estimateSize", () => {
  it("grows with the text and the icon preset", () => {
    const small = estimateSize({ tag: "Icon", size: "sm", texts: [{ text: "DB" }] });
    const large = estimateSize({ tag: "Icon", size: "xl", texts: [{ text: "Primary database" }] });
    expect(large.width).toBeGreaterThan(small.width);
    expect(large.height).toBeGreaterThan(small.height);
    const box = estimateSize({ tag: "Shape", texts: [{ text: "A much longer component name" }] });
    expect(box.width).toBeGreaterThan(estimateSize({ tag: "Shape", texts: [{ text: "API" }] }).width);
    expect(box.width).toBeLessThanOrEqual(260);
  });
});
