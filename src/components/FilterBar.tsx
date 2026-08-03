/* One filter bar, two independent axes — extracted from GalleryPane when the
   Registry became its second consumer (docs/RFC-registry.md §3).

   The shape both surfaces share: a row of primary group buttons, a hairline
   separator, then a row of secondary chips. An item matches both filters
   independently. Groups and chips are always computed from what is actually
   present, so an empty option never renders, and a selection that no longer
   exists falls back rather than showing an empty grid — see useFilterAxis. */
import { cn } from "../lib/utils";

/** One selectable option: the stored value plus how it reads. */
export interface FilterOption {
  value: string;
  label: string;
}

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
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "rounded-md px-2 py-1 text-caption transition-colors",
        active
          ? "bg-surface-2 font-medium text-foreground"
          : "text-muted-foreground hover:text-foreground",
      )}
    >
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
  chipAllLabel = "All tags",
  chipPrefix = "#",
}: {
  /** Primary axis. Renders only when there's a real choice (>2 options,
   *  i.e. "All" plus at least two kinds). */
  groups: FilterOption[];
  group: string;
  onGroup: (value: string) => void;
  /** Secondary axis. Null means "all". */
  chips?: string[];
  chip?: string | null;
  onChip?: (value: string | null) => void;
  chipAllLabel?: string;
  chipPrefix?: string;
}) {
  const showGroups = groups.length > 2;
  const showChips = (chips?.length ?? 0) > 0 && !!onChip;
  if (!showGroups && !showChips) return null;
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-1 border-b border-border px-4 py-1.5">
      {showGroups &&
        groups.map((g) => (
          <FilterButton
            key={g.value}
            active={group === g.value}
            onClick={() => onGroup(g.value)}
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
          {chips!.map((t) => (
            <FilterButton
              key={t}
              active={chip === t}
              onClick={() => onChip!(chip === t ? null : t)}
            >
              {chipPrefix}
              {t}
            </FilterButton>
          ))}
        </>
      )}
    </div>
  );
}
