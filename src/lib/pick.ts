import { useMemo } from "react";
import { useStore } from "./store";
import type { Picked } from "./storeTypes";

/**
 * Finder-style row selection for one list (docs/RFC-multi-select.md).
 *
 * Every list that grew selection wanted the same twelve lines — the modifier
 * branch, the picked-id set, the "is this row inside the selection" question
 * a context menu asks — so they live here once. `orderedIds` is the list's
 * own visible order; shift-ranges are resolved against it, and the store
 * never re-derives layout.
 */
export function usePickList(kind: Picked["kind"], orderedIds: string[]) {
  const picked = useStore((s) => s.picked);
  const pickOne = useStore((s) => s.pickOne);
  const pickToggle = useStore((s) => s.pickToggle);
  const pickRange = useStore((s) => s.pickRange);
  const pickSet = useStore((s) => s.pickSet);
  const clearPicked = useStore((s) => s.clearPicked);

  const pickedIds = useMemo(
    () => new Set(picked?.kind === kind ? picked.ids : []),
    [picked, kind],
  );

  /**
   * Handle a click on a row. Returns true when the click was a *selection*
   * gesture and the caller should skip its normal activation — a plain click
   * still opens the thing, so selection never steals the primary click.
   */
  const handleClick = (
    e: { metaKey: boolean; ctrlKey: boolean; shiftKey: boolean },
    id: string,
  ) => {
    if (e.metaKey || e.ctrlKey) {
      pickToggle(kind, id);
      return true;
    }
    if (e.shiftKey) {
      pickRange(kind, orderedIds, id);
      return true;
    }
    // Plain click collapses to this row and sets the shift anchor, then lets
    // the caller open it.
    pickOne(kind, id);
    return false;
  };

  /**
   * What a right-click should show. Inside a multi-selection it's the batch
   * menu; outside, the selection collapses to that row (Finder's rule) and
   * the caller's own single-row menu opens.
   */
  const contextItems = <T,>(id: string, batch: (ids: string[]) => T) => {
    if (pickedIds.has(id) && pickedIds.size > 1) return batch([...pickedIds]);
    pickOne(kind, id);
    return null;
  };

  return {
    pickedIds,
    handleClick,
    contextItems,
    pickSet,
    clearPicked,
    selectedIds: [...pickedIds],
  };
}
