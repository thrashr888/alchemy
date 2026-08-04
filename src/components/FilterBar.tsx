/* One filter bar, two independent axes — extracted from GalleryPane when the
   Registry became its second consumer (docs/RFC-registry.md §3).

   The shape both surfaces share: a row of primary group buttons, a hairline
   separator, then a row of secondary chips. An item matches both filters
   independently. Groups and chips are always computed from what is actually
   present, so an empty option never renders, and a selection that no longer
   exists falls back rather than showing an empty grid — see useFilterAxis. */
import { useState } from "react";
import { cn } from "../lib/utils";

/** One selectable option: the stored value plus how it reads. */
export interface FilterOption {
  value: string;
  label: string;
}

/** How many chips show before the rest fold behind "+N more". Five is what
   fits on one line beside the group buttons at a normal window width. */
const CHIP_LIMIT = 5;

/** Count-desc, alphabetical on ties — the ordering the gallery's tag chips
   established: the ones you'd reach for first, stable across renders. */
export function rankByCount(counts: Map<string, number>): string[] {
  return [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([k]) => k);
}

/** Keep a selection honest: if what's selected isn't on offer any more, read
   as the fallback instead of filtering everything away. */
export function effectiveValue<T extends string | null>(
  selected: T,
  available: readonly string[],
  fallback: T,
): T {
  if (selected === null) return selected;
  return available.includes(selected as string) ? selected : fallback;
}

function FilterButton({
  active,
  onClick,
  children,
  dot,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
  /** Category colour, shown as a leading dot. Where a surface colours things
   *  by group, this row is also the legend — a separate key would be one
   *  more thing to keep in sync and one more thing to look at. */
  dot?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-caption transition-colors",
        active
          ? "bg-surface-2 font-medium text-foreground"
          : "text-muted-foreground hover:text-foreground",
      )}
    >
      {dot && (
        <span
          aria-hidden
          className="h-2 w-2 shrink-0 rounded-full"
          style={{ backgroundColor: dot }}
        />
      )}
      {children}
    </button>
  );
}

export function FilterBar({
  groups,
  group,
  onGroup,
  chips,
  chip,
  onChip,
  groupDot,
  chipAllLabel = "All tags",
  chipPrefix = "#",
  bare = false,
}: {
  /** Primary axis. Renders only when there's a real choice (>2 options,
   *  i.e. "All" plus at least two kinds). */
  groups: FilterOption[];
  group: string;
  onGroup: (value: string) => void;
  /** Optional category colour per group value, making this row the legend
   *  for a surface that colours by group. */
  groupDot?: (value: string) => string | undefined;
  /** Secondary axis. Null means "all". */
  chips?: string[];
  chip?: string | null;
  onChip?: (value: string | null) => void;
  chipAllLabel?: string;
  chipPrefix?: string;
  /** Embedded in a content column rather than spanning the pane: drops the
   *  full-bleed hairline and the pane padding, which otherwise draw a band
   *  across the width and leave dead clickable space beside the chips. */
  bare?: boolean;
}) {
  /** Chips past the first few are folded away. A well-tagged notebook has
   *  eighty of them, and eight rows of chips pushes the actual content off
   *  screen — the filter bar stops being a bar. Chips are ranked by count,
   *  so the visible few are the ones worth reaching for. */
  const [expanded, setExpanded] = useState(false);
  const showGroups = groups.length > 2;
  const showChips = (chips?.length ?? 0) > 0 && !!onChip;
  if (!showGroups && !showChips) return null;
  const all = chips ?? [];
  // A selected chip stays visible even when it ranks below the cut, or
  // collapsing would hide the filter that is currently in force.
  const visible =
    expanded || all.length <= CHIP_LIMIT + 1
      ? all
      : [...new Set([...all.slice(0, CHIP_LIMIT), ...(chip ? [chip] : [])])];
  const hiddenCount = all.length - visible.length;
  return (
    <div
      className={cn(
        "flex shrink-0 flex-wrap items-center gap-1",
        bare ? "mb-3" : "border-b border-border px-4 py-1.5",
      )}
    >
      {showGroups &&
        groups.map((g) => (
          <FilterButton
            key={g.value}
            active={group === g.value}
            onClick={() => onGroup(g.value)}
            dot={groupDot?.(g.value)}
          >
            {g.label}
          </FilterButton>
        ))}
      {showGroups && showChips && (
        <span aria-hidden className="mx-1.5 h-3.5 w-px bg-border-strong" />
      )}
      {showChips && (
        <>
          <FilterButton active={chip == null} onClick={() => onChip!(null)}>
            {chipAllLabel}
          </FilterButton>
          {visible.map((t) => (
            <FilterButton
              key={t}
              active={chip === t}
              onClick={() => onChip!(chip === t ? null : t)}
            >
              {chipPrefix}
              {t}
            </FilterButton>
          ))}
          {(hiddenCount > 0 || expanded) && (
            <button
              type="button"
              onClick={() => setExpanded((v) => !v)}
              className="rounded-md px-2 py-1 text-caption text-muted-foreground underline-offset-2 transition-colors hover:text-foreground hover:underline"
            >
              {expanded ? "Show less" : `+${hiddenCount} more`}
            </button>
          )}
        </>
      )}
    </div>
  );
}
