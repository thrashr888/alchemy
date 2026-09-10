import { describe, expect, it } from "vitest";
import {
  ROW_ASPECT,
  ROW_MAX,
  estimateSize,
  placeNodes,
  rowsFor,
  type Box,
  type LayoutNode,
} from "./diagramLayout";

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

describe("placeNodes for the diagram kinds", () => {
  const lane = (id: string, containerId?: string): LayoutNode => ({
    ...group(id, containerId),
    flow: "right",
    band: true,
    lane: true,
  });

  it("keeps one global step order across lanes nested in different pools", () => {
    // Two pools, two lanes each; the process snakes through all four.
    const nodes = [
      { ...group("p1"), band: true },
      lane("a", "p1"),
      lane("b", "p1"),
      { ...group("p2"), band: true },
      lane("c", "p2"),
      lane("d", "p2"),
      leaf("s1", "a"),
      leaf("s2", "c"),
      leaf("s3", "b"),
      leaf("s4", "d"),
      leaf("s5", "a"),
    ];
    const edges = [
      { from: "s1", to: "s2" },
      { from: "s2", to: "s3" },
      { from: "s3", to: "s4" },
      { from: "s4", to: "s5" },
    ];
    const placed = placeNodes(nodes, edges, { direction: "down" });
    const xs = ["s1", "s2", "s3", "s4", "s5"].map((id) => (placed.boxes.get(id) as Box).x);
    // Strictly increasing columns, each step one column past the last —
    // even across the pool boundary — and lanes are all the same width.
    for (let i = 1; i < xs.length; i += 1) expect(xs[i]).toBeGreaterThan(xs[i - 1]);
    const widths = ["a", "b", "c", "d"].map((id) => (placed.boxes.get(id) as Box).width);
    expect(new Set(widths).size).toBe(1);
    // Members stay inside their lane, lanes inside their pool.
    for (const [step, holder] of [
      ["s1", "a"],
      ["s2", "c"],
      ["s3", "b"],
      ["s4", "d"],
      ["a", "p1"],
      ["d", "p2"],
    ])
      expect(inside(placed.boxes.get(step) as Box, placed.boxes.get(holder) as Box)).toBe(true);
    // A band title sits on the left: the first step starts past the inset.
    const a = placed.boxes.get("a") as Box;
    expect((placed.boxes.get("s1") as Box).x - a.x).toBeGreaterThanOrEqual(40 + 28);
  });

  it("stacks a sequence container's members in input order, edges or not", () => {
    const stage: LayoutNode = { ...group("stage"), flow: "down", sequence: true };
    const nodes = [stage, leaf("touch", "stage"), leaf("pain", "stage"), leaf("need", "stage")];
    const placed = placeNodes(nodes, [], { direction: "right" });
    const [t, p, n] = ["touch", "pain", "need"].map((id) => placed.boxes.get(id) as Box);
    expect(t.x).toBe(p.x);
    expect(p.x).toBe(n.x);
    expect(t.y).toBeLessThan(p.y);
    expect(p.y).toBeLessThan(n.y);
    // Two stages side by side as columns, the journey running left to right.
    const two = [
      stage,
      { ...group("next"), flow: "down" as const, sequence: true },
      leaf("t1", "stage"),
      leaf("t2", "next"),
    ];
    const cols = placeNodes(two, [{ from: "t1", to: "t2" }], { direction: "right" });
    expect((cols.boxes.get("next") as Box).x).toBeGreaterThan((cols.boxes.get("stage") as Box).x);
    expect((cols.boxes.get("next") as Box).y).toBe((cols.boxes.get("stage") as Box).y);
    // Columns of unequal height center by default and share a top edge
    // when asked to.
    const uneven = [...two, leaf("t3", "next")];
    const centered = placeNodes(uneven, [{ from: "t1", to: "t2" }], { direction: "right" });
    const flush = placeNodes(uneven, [{ from: "t1", to: "t2" }], {
      direction: "right",
      align: "start",
    });
    expect((centered.boxes.get("stage") as Box).y).toBeGreaterThan(
      (centered.boxes.get("next") as Box).y,
    );
    expect((flush.boxes.get("stage") as Box).y).toBe((flush.boxes.get("next") as Box).y);
  });

  it("ranks a group's members left to right when the scene runs right", () => {
    // A relationship map (`align: "start"`, as eraserDiagram.ts passes for
    // the kind): eras as sibling groups, each era's chain of people reading
    // left to right on one row inside it, the unconnected stacked in the
    // first column — never a tall single column of everything.
    const nodes = [
      group("hellenistic"),
      leaf("maria", "hellenistic"),
      leaf("zosimos", "hellenistic"),
      leaf("hermes", "hellenistic"),
      group("islamic"),
      leaf("jabir", "islamic"),
      leaf("razi", "islamic"),
    ];
    const edges = [
      { from: "maria", to: "zosimos", labeled: true },
      { from: "zosimos", to: "jabir", labeled: true },
      { from: "jabir", to: "razi", labeled: true },
    ];
    const placed = placeNodes(nodes, edges, { direction: "right", align: "start" });
    const box = (id: string) => placed.boxes.get(id) as Box;
    // Inside an era, the chain runs left to right on one row.
    expect(box("maria").y).toBe(box("zosimos").y);
    expect(box("maria").x + box("maria").width).toBeLessThanOrEqual(box("zosimos").x);
    expect(box("jabir").y).toBe(box("razi").y);
    expect(box("jabir").x).toBeLessThan(box("razi").x);
    // A member with no edges shares the first column and stacks below.
    expect(box("hermes").x).toBe(box("maria").x);
    expect(box("hermes").y).not.toBe(box("maria").y);
    // The eras themselves sit side by side, the later one to the right.
    const [h, i] = [box("hellenistic"), box("islamic")];
    expect(h.x + h.width).toBeLessThanOrEqual(i.x);
    expect(overlaps(h, i)).toBe(false);
    expect(placed.width).toBeGreaterThan(placed.height);
  });

  it("chains a level's islands along the flow when asked, wrapping a long chain", () => {
    // Three eras the sources never connect to each other, plus one loose
    // person: stacked into one column by default, laid left to right for
    // a relationship map — and, since the chain runs 1088 wide by 146
    // tall, wrapped into two rows rather than left as a strip.
    const nodes = [
      group("egypt"),
      leaf("zosimos", "egypt"),
      leaf("tablet"),
      group("islam"),
      leaf("jabir", "islam"),
      leaf("razi", "islam"),
      group("europe"),
      leaf("bacon", "europe"),
      leaf("jung"),
    ];
    const edges = [
      { from: "zosimos", to: "tablet" },
      { from: "jabir", to: "razi" },
    ];
    const stacked = placeNodes(nodes, edges, { direction: "right", align: "start" });
    const chained = placeNodes(nodes, edges, {
      direction: "right",
      align: "start",
      islands: "along",
    });
    const at = (p: typeof stacked, id: string) => p.boxes.get(id) as Box;
    // By default every island starts at rank 0: one tall column.
    expect(at(stacked, "egypt").x).toBe(at(stacked, "islam").x);
    expect(at(stacked, "islam").x).toBe(at(stacked, "europe").x);
    expect(stacked.height).toBeGreaterThan(stacked.width);
    // Chained: egypt, its tablet, then islam across the first row, each
    // to the right of the last; europe and jung on a second row below,
    // starting back at the left. Document order is row order.
    const first = ["egypt", "tablet", "islam"].map((id) => at(chained, id));
    const second = ["europe", "jung"].map((id) => at(chained, id));
    for (const row of [first, second]) {
      for (let i = 1; i < row.length; i += 1) {
        expect(row[i - 1].x + row[i - 1].width).toBeLessThanOrEqual(row[i].x);
        expect(row[i].y).toBe(row[0].y);
      }
    }
    expect(second[0].x).toBe(first[0].x);
    const bottomOfFirst = Math.max(...first.map((b) => b.y + b.height));
    expect(second[0].y).toBeGreaterThanOrEqual(bottomOfFirst);
    // Inside an era nothing changes: jabir still precedes razi, and the
    // zosimos → tablet edge still reads left to right.
    expect(at(chained, "jabir").x).toBeLessThan(at(chained, "razi").x);
    expect(at(chained, "zosimos").x).toBeLessThan(at(chained, "tablet").x);
    // Two rows of a 7:1 strip make a sheet near 2:1, not a column.
    expect(chained.width / chained.height).toBeGreaterThan(1.5);
    expect(chained.width / chained.height).toBeLessThan(ROW_ASPECT);
    // Nothing overlaps unless one holds the other.
    for (const p of chained.boxes.values())
      for (const q of chained.boxes.values())
        if (p !== q && !inside(p, q) && !inside(q, p)) expect(overlaps(p, q)).toBe(false);
  });

  it("keeps a short chain of islands on one row", () => {
    // Two eras side by side are 2:1 already; wrapping would stack them.
    const nodes = [group("egypt"), leaf("zosimos", "egypt"), group("islam"), leaf("jabir", "islam")];
    const placed = placeNodes(nodes, [], { direction: "right", align: "start", islands: "along" });
    const [egypt, islam] = ["egypt", "islam"].map((id) => placed.boxes.get(id) as Box);
    expect(egypt.y).toBe(islam.y);
    expect(egypt.x + egypt.width).toBeLessThanOrEqual(islam.x);
  });

  it("never wraps to one island per row", () => {
    // Six wide eras: the literal "row no wider than 2.5 × the tallest"
    // rule would put one per row and rebuild the column. The sheet rule
    // keeps several to a row.
    const wide = (id: string): LayoutNode => ({ ...leaf(id), width: 600, height: 300 });
    const nodes = ["a", "b", "c", "d", "e", "f"].map(wide);
    const placed = placeNodes(nodes, [], { direction: "right", align: "start", islands: "along" });
    const ys = new Set([...placed.boxes.values()].map((b) => b.y));
    expect(ys.size).toBe(2);
    expect(placed.width).toBeLessThanOrEqual(ROW_MAX + 48);
    expect(placed.width).toBeGreaterThan(placed.height);
  });

  it("widens a rank gap that a labeled connection crosses", () => {
    const nodes = () => [leaf("a"), leaf("b"), leaf("c")];
    const chain = (labeled: boolean) => [
      { from: "a", to: "b", labeled },
      { from: "b", to: "c" },
    ];
    const plain = placeNodes(nodes(), chain(false), { direction: "down", gap: 40, labelRoom: 30 });
    const roomy = placeNodes(nodes(), chain(true), { direction: "down", gap: 40, labelRoom: 30 });
    const gapBetween = (p: typeof plain, from: string, to: string) =>
      (p.boxes.get(to) as Box).y - ((p.boxes.get(from) as Box).y + (p.boxes.get(from) as Box).height);
    expect(gapBetween(plain, "a", "b")).toBe(40);
    expect(gapBetween(roomy, "a", "b")).toBe(70);
    // The unlabeled boundary keeps the plain gap.
    expect(gapBetween(roomy, "b", "c")).toBe(40);
    expect(roomy.height).toBe(plain.height + 30);
    // A labeled edge into a box inside a group needs the group's padding
    // and title band too, or the label lands on the title.
    const into = placeNodes(
      [leaf("src"), group("g"), leaf("in", "g")],
      [{ from: "src", to: "in", labeled: true }],
      { direction: "down", gap: 40, labelRoom: 30, padding: 20, titleInset: 30 },
    );
    expect(gapBetween(into, "src", "g")).toBe(40 + 30 + 20 + 30);
    // Leaving a group costs its padding; a band title on the left is not
    // in a downward path's way.
    const across = placeNodes(
      [group("a"), leaf("x", "a"), { ...group("b"), band: true }, leaf("y", "b")],
      [{ from: "x", to: "y", labeled: true }],
      { direction: "down", gap: 40, labelRoom: 30, padding: 20, titleInset: 30 },
    );
    expect(gapBetween(across, "a", "b")).toBe(40 + 30 + 20 + 20);
    // Shared lane columns get the same room.
    const lanes = [lane("l1"), lane("l2"), leaf("s1", "l1"), leaf("s2", "l2"), leaf("s3", "l1")];
    const laneEdges = [
      { from: "s1", to: "s2", labeled: true },
      { from: "s2", to: "s3" },
    ];
    const cols = placeNodes(lanes, laneEdges, { direction: "down", gap: 40, labelRoom: 30 });
    const s = (id: string) => cols.boxes.get(id) as Box;
    expect(s("s2").x - (s("s1").x + s("s1").width)).toBe(70);
    expect(s("s3").x - (s("s2").x + s("s2").width)).toBe(40);
  });
});

describe("estimateSize for the BPMN and table tags", () => {
  it("gives events and gateways a disc plus caption, tables a row per field", () => {
    const bare = estimateSize({ tag: "Event" });
    expect(bare).toEqual({ width: 56, height: 56 });
    const captioned = estimateSize({ tag: "Gateway", texts: [{ text: "Known issue?" }] });
    expect(captioned.height).toBe(80);
    expect(captioned.width).toBeGreaterThan(56);
    const table = estimateSize({
      tag: "DatabaseTable",
      label: "orders",
      fields: [{ name: "id", type: "uuid", meta: "PK" }, { name: "customer_id", type: "uuid" }],
    });
    expect(table.height).toBe(40 + 24 * 2);
    expect(table.width).toBeGreaterThanOrEqual(160);
    const activity = estimateSize({ tag: "Activity", icon: "search", texts: [{ text: "Triage" }] });
    expect(activity.width).toBeGreaterThanOrEqual(120);
    expect(activity.height).toBe(56);
  });
});

describe("rowsFor", () => {
  it("leaves a strip that fits the sheet aspect on one row", () => {
    expect(rowsFor(1000, 400)).toBe(1); // 2.5:1 exactly
    expect(rowsFor(600, 400)).toBe(1);
    expect(rowsFor(0, 400)).toBe(1);
  });

  it("wraps a long strip into the rows that bring the sheet near the aspect", () => {
    // The in-app relationship map: 3871 × 423 → two rows of ~1900.
    expect(rowsFor(3871, 423)).toBe(2);
    // A 10,000-wide strip of 300-tall islands: sqrt(2.5 × 300 × 10000) ≈ 2739
    // exceeds ROW_MAX, so the cap decides.
    expect(rowsFor(10_000, 300)).toBe(Math.round(10_000 / ROW_MAX));
    // Tall islands cap at ROW_MAX too.
    expect(rowsFor(4800, 2000)).toBe(2);
  });

  it("never makes a row narrower than the longest island", () => {
    // The harness sample: three connected eras (one 2300-wide island)
    // and a note. Wrapping cannot split the island, so the note stays
    // beside it instead of alone on a second row.
    expect(rowsFor(2645, 308, 2300)).toBe(1);
    expect(rowsFor(3871, 423, 700)).toBe(2);
  });
});
