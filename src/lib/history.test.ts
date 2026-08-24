import { describe, expect, it } from "vitest";
import {
  dropEntry,
  HISTORY_LIMIT,
  makeEntry,
  pushEntry,
  redoLabel,
  undoLabel,
  type HistoryEntry,
} from "./history";

const noop = async () => {};
const entry = (label: string): HistoryEntry => makeEntry(label, noop, noop);

describe("history stack", () => {
  it("gives every entry a distinct id", () => {
    expect(entry("Delete Note").id).not.toBe(entry("Delete Note").id);
  });

  it("pushes newest last", () => {
    const stack = pushEntry(pushEntry([], entry("First")), entry("Second"));
    expect(stack.map((e) => e.label)).toEqual(["First", "Second"]);
  });

  it("drops the oldest entry past the limit", () => {
    let stack: HistoryEntry[] = [];
    for (let i = 0; i < HISTORY_LIMIT + 10; i++) {
      stack = pushEntry(stack, entry(`Mutation ${i}`));
    }
    expect(stack).toHaveLength(HISTORY_LIMIT);
    // The oldest ten are gone; the newest is still on top.
    expect(stack[0].label).toBe("Mutation 10");
    expect(stack[stack.length - 1].label).toBe(`Mutation ${HISTORY_LIMIT + 9}`);
  });

  it("removes an entry from the middle without disturbing the rest", () => {
    const [a, b, c] = [entry("A"), entry("B"), entry("C")];
    expect(dropEntry([a, b, c], b.id).map((e) => e.label)).toEqual(["A", "C"]);
  });

  it("ignores a drop for an entry already gone", () => {
    expect(dropEntry([entry("A")], "history-does-not-exist")).toHaveLength(1);
  });

  it("labels the next undo from the top of the stack", () => {
    const stack = pushEntry(
      pushEntry([], entry("Delete Note")),
      entry("Remove Source"),
    );
    expect(undoLabel(stack)).toBe("Undo Remove Source");
    expect(redoLabel(stack)).toBe("Redo Remove Source");
  });

  it("has no label when there is nothing to undo", () => {
    expect(undoLabel([])).toBeNull();
    expect(redoLabel([])).toBeNull();
  });
});
