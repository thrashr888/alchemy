import { describe, expect, it } from "vitest";
import { sourceSubtree } from "./sourceRows";

interface N {
  id: string;
  parentId: string;
  title: string;
}

const n = (id: string, parentId = "", title = id): N => ({
  id,
  parentId,
  title,
});

function index(nodes: N[]): Map<string, N[]> {
  const m = new Map<string, N[]>();
  for (const x of nodes) {
    if (!x.parentId) continue;
    const list = m.get(x.parentId);
    if (list) list.push(x);
    else m.set(x.parentId, [x]);
  }
  return m;
}

/** repo/ ── src/ ── a.ts
 *        │       └─ b.ts
 *        └─ README.md          */
const repo = n("repo");
const src = n("src", "repo");
const a = n("a", "src");
const b = n("b", "src");
const readme = n("readme", "repo");
const tree = index([repo, src, a, b, readme]);

const open = { collapsed: () => false };

describe("sourceSubtree", () => {
  it("walks every level, not just the first", () => {
    expect(sourceSubtree(repo, tree, open)).toEqual([
      { s: repo, depth: 0 },
      { s: src, depth: 1 },
      { s: a, depth: 2 },
      { s: b, depth: 2 },
      { s: readme, depth: 1 },
    ]);
  });

  it("closes a nested folder without closing its parent", () => {
    const rows = sourceSubtree(repo, tree, {
      collapsed: (s) => s.id === "src",
    });
    expect(rows.map((r) => r.s.id)).toEqual(["repo", "src", "readme"]);
  });

  it("closes the top folder and hides the whole subtree", () => {
    const rows = sourceSubtree(repo, tree, {
      collapsed: (s) => s.id === "repo",
    });
    expect(rows.map((r) => r.s.id)).toEqual(["repo"]);
  });

  it("asks about collapse with the child count", () => {
    const asked: [string, number][] = [];
    sourceSubtree(repo, tree, {
      collapsed: (s, kids) => {
        asked.push([s.id, kids]);
        return false;
      },
    });
    expect(asked).toEqual([
      ["repo", 2],
      ["src", 2],
      ["a", 0],
      ["b", 0],
      ["readme", 0],
    ]);
  });

  it("expands past a closed folder to reach a filter match", () => {
    const rows = sourceSubtree(repo, tree, {
      collapsed: () => true,
      matches: (s) => s.id === "b",
    });
    expect(rows.map((r) => r.s.id)).toEqual(["repo", "src", "b"]);
  });

  it("drops a subtree nothing in it matched", () => {
    const rows = sourceSubtree(repo, tree, {
      collapsed: () => false,
      matches: (s) => s.id === "nothing",
    });
    expect(rows).toEqual([]);
  });

  it("keeps a matching folder even when no child matches", () => {
    const rows = sourceSubtree(repo, tree, {
      collapsed: () => false,
      matches: (s) => s.id === "repo",
    });
    expect(rows.map((r) => r.s.id)).toEqual(["repo"]);
  });

  it("survives a parent chain that points back at itself", () => {
    const x = n("x");
    const y = n("y", "x");
    const loop = new Map<string, N[]>([
      ["x", [y]],
      ["y", [x]],
    ]);
    expect(sourceSubtree(x, loop, open).map((r) => r.s.id)).toEqual(["x", "y"]);
  });
});
