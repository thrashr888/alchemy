/* Home's collection controls: an inline title filter and a grid/table
   toggle, shared by the Notebooks and Registry sections.

   Cards are recognisable — you find the thing by its picture and its shape.
   Rows are scannable — you find it by reading down a column. Neither wins in
   general, so both exist and the choice is remembered. The filter is title-
   only and deliberately not the ask box: this narrows what's on screen, it
   doesn't search inside anything. ⌘K is still the way to search content. */
import { useEffect, useRef } from "react";
import { useStore } from "@/lib/store";
import { cn, shortcutBlocked } from "@/lib/utils";
import { LayoutGrid, List, Search, X } from "lucide-react";

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

/** The shared table shell: a hairline header row and hoverable body rows,
    matching the design system's no-tonal-fill rule. */
export function HomeTable({
  columns,
  children,
}: {
  columns: { key: string; label: string; className?: string }[];
  children: React.ReactNode;
}) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-body">
        <thead>
          <tr className="border-b border-border text-left">
            {columns.map((c) => (
              <th
                key={c.key}
                className={cn(
                  "px-3 py-2 text-caption font-medium text-subtle-foreground",
                  c.className,
                )}
              >
                {c.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>{children}</tbody>
      </table>
    </div>
  );
}
