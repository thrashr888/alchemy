/* Home's collection controls: an inline title filter and a grid/table
   toggle, shared by the Notebooks and Registry sections.

   Cards are recognisable — you find the thing by its picture and its shape.
   Rows are scannable — you find it by reading down a column. Neither wins in
   general, so both exist and the choice is remembered. The filter is title-
   only and deliberately not the ask box: this narrows what's on screen, it
   doesn't search inside anything. ⌘K is still the way to search content. */
import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import { cn, shortcutBlocked } from "@/lib/utils";
import {
  ChevronDown,
  ChevronUp,
  LayoutGrid,
  List,
  Search,
  X,
} from "lucide-react";

/** Case-insensitive substring over whatever the row shows as its name. */
export function matchesHomeQuery(query: string, ...fields: string[]): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return fields.some((f) => f?.toLowerCase().includes(q));
}

export function HomeViewControls({
  placeholder,
  trailing,
  sort,
}: {
  placeholder: string;
  /** Optional section-specific control rendered after the view toggle —
   *  the Registry's "Suggest" lives here. Keep it one small button. */
  trailing?: React.ReactNode;
  /** Optional sort order for the collection, rendered as a quiet select
   *  beside the view toggle. The caller persists the choice (the homeView
   *  localStorage idiom). */
  sort?: {
    value: string;
    options: { value: string; label: string }[];
    onChange: (value: string) => void;
  };
}) {
  const view = useStore((s) => s.homeView);
  const query = useStore((s) => s.homeQuery);
  const inputRef = useRef<HTMLInputElement>(null);

  // ⌘F narrows the collection here, matching the gallery and the reader —
  // the same key means "find within what I'm looking at" everywhere.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Not while a modal owns the keyboard or the user is typing in a
      // field — find used to open behind dialogs and steal focus mid-word.
      if (shortcutBlocked(e)) return;
      if ((e.metaKey || e.ctrlKey) && e.key === "f") {
        e.preventDefault();
        inputRef.current?.focus();
        inputRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Edit > Find (menu.rs): accelerator-less menu item, routed via findBump.
  const findBump = useStore((s) => s.findBump);
  useEffect(() => {
    if (findBump === 0) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [findBump]);

  const setView = (v: "grid" | "table") => {
    localStorage.setItem("homeView", v);
    useStore.setState({ homeView: v });
  };

  return (
    <div className="mb-3 flex items-center gap-2">
      <div className="relative min-w-0 flex-1">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtle-foreground" />
        <input
          ref={inputRef}
          value={query}
          onChange={(e) => useStore.setState({ homeQuery: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              useStore.setState({ homeQuery: "" });
              inputRef.current?.blur();
            }
          }}
          placeholder={placeholder}
          className="h-8 w-full rounded-md border border-input bg-surface-2 pl-8 pr-8 text-caption text-foreground outline-none placeholder:text-subtle-foreground focus:border-ring/70 focus:ring-1 focus:ring-ring/40"
        />
        {query && (
          <button
            onClick={() => useStore.setState({ homeQuery: "" })}
            className="absolute right-2 top-1/2 -translate-y-1/2 rounded p-0.5 text-muted-foreground transition hover:text-foreground"
            aria-label="Clear the filter"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        )}
      </div>
      {sort && (
        <select
          value={sort.value}
          onChange={(e) => sort.onChange(e.target.value)}
          title="Sort order"
          aria-label="Sort order"
          className="h-8 shrink-0 rounded-md border border-border bg-transparent px-2 text-caption text-muted-foreground outline-none transition-colors hover:text-foreground focus:border-ring/70"
        >
          {sort.options.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      )}
      <div className="flex shrink-0 items-center gap-0.5 rounded-lg border border-border p-0.5">
        {(
          [
            ["grid", "Grid", LayoutGrid],
            ["table", "Table", List],
          ] as const
        ).map(([id, label, Icon]) => (
          <button
            key={id}
            type="button"
            onClick={() => setView(id)}
            aria-pressed={view === id}
            title={`${label} view`}
            aria-label={`${label} view`}
            className={cn(
              "rounded-md p-1.5 transition-colors",
              view === id
                ? "bg-surface-2 text-foreground"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            <Icon className="h-3.5 w-3.5" />
          </button>
        ))}
      </div>
      {trailing}
    </div>
  );
}

/** Which way a column reads. Text starts ascending, counts and dates start
    descending: the first click should show the answer you came for. */
export type SortDir = "asc" | "desc";
export type TableSort = { key: string; dir: SortDir };
export type TableColumn = {
  key: string;
  label: string;
  className?: string;
  /** The direction this column starts in. Omit to leave it unsortable. */
  sort?: SortDir;
};

/** Sort state for one table, remembered across launches (DESIGN.md §9,
    state survives). Clicking the active column flips it; clicking another
    starts that column at its natural direction. */
export function useTableSort(
  storageKey: string,
  fallback: TableSort,
  keys: readonly string[],
) {
  const [sort, setSort] = useState<TableSort>(() => {
    try {
      const [key, dir] = (localStorage.getItem(storageKey) ?? "").split(":");
      // A key whose column no longer exists would sort by nothing and mark
      // no header, so it falls back rather than persisting a ghost.
      if (keys.includes(key) && (dir === "asc" || dir === "desc")) {
        return { key, dir };
      }
    } catch {
      // Quota or private-mode noise; the default order still works.
    }
    return fallback;
  });
  const toggle = (key: string, natural: SortDir) => {
    const next: TableSort =
      sort.key === key
        ? { key, dir: sort.dir === "asc" ? "desc" : "asc" }
        : { key, dir: natural };
    setSort(next);
    try {
      localStorage.setItem(storageKey, `${next.key}:${next.dir}`);
    } catch {
      // Same: the order holds for this session either way.
    }
  };
  return { sort, toggle };
}

/** The shared table shell: a hairline header row and hoverable body rows,
    matching the design system's no-tonal-fill rule. A column that names a
    natural direction becomes a real sort button, with the arrow drawn on
    the active one and a faint one on hover elsewhere. */
export function HomeTable({
  columns,
  sort,
  children,
}: {
  columns: TableColumn[];
  /** Current order plus the click handler, from `useTableSort`. Omit it and
   *  the headers stay plain labels. */
  sort?: TableSort & { onSort: (key: string, natural: SortDir) => void };
  children: React.ReactNode;
}) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-body">
        <thead>
          <tr className="border-b border-border text-left">
            {columns.map((c) => {
              const natural = sort && c.sort;
              const active = natural !== undefined && sort!.key === c.key;
              const Arrow =
                active && sort!.dir === "asc" ? ChevronUp : ChevronDown;
              return (
                <th
                  key={c.key}
                  scope="col"
                  aria-sort={
                    !natural
                      ? undefined
                      : !active
                        ? "none"
                        : sort!.dir === "asc"
                          ? "ascending"
                          : "descending"
                  }
                  className={cn(
                    "px-3 py-2 text-caption font-medium text-subtle-foreground",
                    c.className,
                  )}
                >
                  {natural ? (
                    <button
                      type="button"
                      onClick={() => sort!.onSort(c.key, natural)}
                      title={`Sort by ${c.label.toLowerCase()}`}
                      className={cn(
                        "group/sort -mx-1 inline-flex items-center gap-1 rounded px-1 py-0.5 transition-colors hover:text-foreground",
                        active && "text-foreground",
                      )}
                    >
                      {c.label}
                      <Arrow
                        aria-hidden
                        className={cn(
                          "h-3 w-3 shrink-0 transition-opacity",
                          // Focus shows the hint too: an affordance that
                          // only answers the mouse is invisible by keyboard.
                          active
                            ? "opacity-100"
                            : "opacity-0 group-hover/sort:opacity-40 group-focus-visible/sort:opacity-40",
                        )}
                      />
                    </button>
                  ) : (
                    c.label
                  )}
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>{children}</tbody>
      </table>
    </div>
  );
}
