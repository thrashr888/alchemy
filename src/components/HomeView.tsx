import { useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useStore } from "@/lib/store";
import { usePickList } from "@/lib/pick";
import { homeDraftKey } from "@/lib/homeChatRun";
import { HOME_CARDS, registerHomeCards, toggleHomeCard } from "@/lib/homeCards";
import { DevBadge } from "./DevBadge";
import { InferenceActivity } from "./InferenceActivity";
import { UpdateBadge } from "./UpdateBadge";
import { HealthBanner } from "./HealthBanner";
import { NavButtons } from "./NavButtons";
import {
  Badge,
  Button,
  CardAction,
  EmptyState,
  Input,
  Modal,
  RowMenu,
  type RowMenuItem,
  useMarquee,
  useConfirm,
  SearchField,
  Segmented,
} from "./ui";
import { AlchemyHero, AlchemySymbol } from "./AlchemyHero";
import { notebookVerbs } from "@/lib/notebookMenu";
import { currentEpigraph } from "@/lib/epigraph";
import { THEMES, resolveThemeId } from "@/lib/themes";
import { DitherBackground } from "./DitherBackground";
import { useHomeActivity } from "./useHomeActivity";
import { AwayDigest, ReportsFeed, lastNightLine } from "./HomeReportsFeed";
import {
  cn,
  noteUnread,
  relativeTime,
  shortcutBlocked,
  strayTypingKey,
} from "@/lib/utils";
import { sourceGlyph } from "@/lib/sourceIcon";
import type {
  Note,
  Notebook,
  NotebookPreview,
  SourceEvent,
} from "@/lib/types";
import type { HomeSection } from "@/lib/storeTypes";
import {
  Archive,
  ArchiveRestore,
  ChevronDown,
  MessagesSquare,
  Moon,
  PanelLeft,
  Plus,
  Settings,
  Sparkles,
  Trash2,
  FileText,
  Newspaper,
  Package,
  FolderInput,
  Library,
  Share2,
  Square,
  StickyNote,
} from "lucide-react";
import { BriefSidebar, StaffSidebar, useNightShiftTone } from "./HomeSections";
import {
  HomeChatControls,
  HomeChatThread,
  HomeThreadsSidebar,
  useHomeChat,
} from "./HomeChat";
import { NOTEBOOK_PALETTE, notebookIcon } from "@/lib/notebookIcons";
import { NotebookEditModal, NotebookLookFields } from "./NotebookEditModal";
import { RegistrySection } from "./RegistrySection";
import { TimelineSection } from "./TimelineSection";
import {
  HOME_VIEWS,
  HomeTable,
  matchesHomeQuery,
  setHomeView,
  useFindFocus,
  useTableSort,
} from "./HomeViewControls";
import type { SortDir, TableColumn, TableSort } from "./HomeViewControls";

/** The shared caps label (RFC-mac-chrome "Shared": 11px, 600, uppercase,
 *  tracking .04em, subtle) — the sidebar's block titles and the shelf's
 *  recency headings are the same label. */
const CAPS =
  "text-micro font-semibold uppercase tracking-[0.04em] text-subtle-foreground";

/** The shared list row: 28px, padding 0 8px, radius 6, gap 8, 13px, selected
 *  washed in `--selection`. Settings' sidebar already reads this way. */
const ROW =
  "flex h-7 w-full items-center gap-2 rounded-md px-2 text-left text-body transition-colors";

/** The spec's pop-up button, as the Library wears it: 26px, surface-2 with an
 *  inset hairline, the current choice as its label. */
const POPUP =
  "flex h-[26px] items-center gap-1.5 rounded-lg bg-surface-2 pl-2.5 pr-1.5 text-caption text-foreground shadow-[inset_0_0_0_0.5px_var(--border)] transition-colors hover:bg-elevated";

/** The shelf's columns, and which way each one first reads. */
const NOTEBOOK_COLUMNS = [
  { key: "title", label: "Title", sort: "asc" },
  { key: "sources", label: "Sources", className: "text-right", sort: "desc" },
  { key: "notes", label: "Notes", className: "text-right", sort: "desc" },
  { key: "reports", label: "Reports", className: "text-right", sort: "desc" },
  { key: "updated", label: "Updated", sort: "desc" },
  { key: "menu", label: "", className: "w-8" },
] as const satisfies TableColumn[];

const NOTEBOOK_SORT_KEYS = NOTEBOOK_COLUMNS.filter((c) => "sort" in c).map(
  (c) => c.key,
);

/** The sort pop-up's orders, named the way the pop-up's label reads them.
 *  Same state the table's column headers write, so switching shapes keeps
 *  the order you chose (`homeTableSort`). */
const NOTEBOOK_SORTS: { key: string; dir: SortDir; label: string }[] = [
  { key: "updated", dir: "desc", label: "Recently updated" },
  { key: "title", dir: "asc", label: "Name" },
  { key: "sources", dir: "desc", label: "Sources" },
  { key: "notes", dir: "desc", label: "Notes" },
  { key: "reports", dir: "desc", label: "Reports" },
];

/** Order the shelf's rows. Every column breaks its ties on the title, so a
 *  column of equal counts still reads down alphabetically instead of
 *  reshuffling on each render. */
function sortNotebooks(rows: Notebook[], sort: TableSort): Notebook[] {
  const dir = sort.dir === "asc" ? 1 : -1;
  const byTitle = (a: Notebook, b: Notebook) => a.title.localeCompare(b.title);
  return [...rows].sort((a, b) => {
    switch (sort.key) {
      case "title":
        return dir * byTitle(a, b);
      case "sources":
        return dir * (a.sourceCount - b.sourceCount) || byTitle(a, b);
      case "notes":
        return dir * (a.noteCount - b.noteCount) || byTitle(a, b);
      case "reports":
        return dir * (a.reportCount - b.reportCount) || byTitle(a, b);
      default:
        return dir * (a.updatedAt - b.updatedAt) || byTitle(a, b);
    }
  });
}

const DAY = 24 * 60 * 60 * 1000;

/** Today, Last 7 days, Earlier — the shelf's three shelves, by when the
 *  notebook was last written. Empty groups are absent rather than titled,
 *  and the rows keep the order they arrive in, so the sort pop-up still
 *  decides what reads first inside each one. */
function recencyGroups(
  rows: Notebook[],
  now = Date.now(),
): { label: string; rows: Notebook[] }[] {
  const midnight = new Date(now);
  midnight.setHours(0, 0, 0, 0);
  const today = midnight.getTime();
  const week = today - 6 * DAY;
  const groups = [
    { label: "Today", rows: [] as Notebook[] },
    { label: "Last 7 days", rows: [] as Notebook[] },
    { label: "Earlier", rows: [] as Notebook[] },
  ];
  for (const nb of rows) {
    if (nb.updatedAt >= today) groups[0].rows.push(nb);
    else if (nb.updatedAt >= week) groups[1].rows.push(nb);
    else groups[2].rows.push(nb);
  }
  return groups.filter((g) => g.rows.length > 0);
}

/** Three or four bars, at widths this notebook always draws — the card's
 *  skeleton while `notebook_previews` is in flight. Widths derived from the
 *  id mean the same notebook shows the same placeholder on every render, in
 *  every window, instead of shimmering as React re-runs. Once the contents
 *  land they replace these entirely: a shape hashed from an id was steady,
 *  but it was a shape about nothing. */
function thumbLines(id: string): number[] {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  const widths = [92, 78, 64, 50, 86, 70];
  const count = 3 + (h % 2);
  return Array.from(
    { length: count },
    (_, i) => widths[(h >>> (i * 3)) % widths.length],
  );
}

/** The scannable form of the notebook shelf. Same rows the grid shows, read
 *  down columns instead of across cards. */
function NotebookTable({
  notebooks,
  unreadByNb,
  rowMenu,
  pickedIds,
  onRowClick,
  onRowOpen,
  sort,
  onSort,
}: {
  notebooks: Notebook[];
  unreadByNb: Map<string, number>;
  /** Per-row menu, so the table has the same verbs (and the same
   *  right-click) as the cards — it had neither. */
  rowMenu: (nb: Notebook) => React.ReactNode;
  pickedIds: Set<string>;
  onRowClick: (e: React.MouseEvent, nb: Notebook) => void;
  /** Keyboard path: Tab reaches each row, Enter opens it (bypassing the
   *  pointer-only selection logic in onRowClick). */
  onRowOpen: (nb: Notebook) => void;
  /** The rows arrive already ordered — the shelf sorts them upstream so the
   *  selection's range order matches what's on screen. */
  sort: TableSort;
  onSort: (key: string, natural: SortDir) => void;
}) {
  return (
    <HomeTable columns={[...NOTEBOOK_COLUMNS]} sort={{ ...sort, onSort }}>
      {notebooks.map((nb) => (
        <tr
          key={nb.id}
          data-pick-id={nb.id}
          tabIndex={0}
          onClick={(e) => onRowClick(e, nb)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && e.target === e.currentTarget) {
              e.preventDefault();
              onRowOpen(nb);
            }
          }}
          className={cn(
            "group cursor-pointer border-b border-border transition-colors last:border-b-0 hover:bg-surface-2",
            pickedIds.has(nb.id) && "bg-primary/10 hover:bg-primary/15",
          )}
        >
          <td className="relative px-3 py-2">
            <span className="flex items-center gap-2">
              {(() => {
                const Icon = notebookIcon(nb.icon);
                return (
                  <Icon
                    className="h-3.5 w-3.5 shrink-0"
                    style={{ color: nb.color || NOTEBOOK_PALETTE[0] }}
                    aria-hidden
                  />
                );
              })()}
              <span className="truncate font-medium">{nb.title}</span>
              {(unreadByNb.get(nb.id) ?? 0) > 0 && (
                <span
                  className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                  title={`${unreadByNb.get(nb.id)} unread`}
                />
              )}
            </span>
          </td>
          <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
            {nb.sourceCount}
          </td>
          {/* Zero reads as nothing: a column of 0s is noise, and the eye
              should land on the notebooks that actually have material. */}
          <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
            {nb.noteCount || ""}
          </td>
          <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
            {nb.reportCount || ""}
          </td>
          <td className="px-3 py-2 text-caption text-muted-foreground">
            {relativeTime(nb.updatedAt)}
          </td>
          {/* The menu column: right-clicking the row opens the same menu
              (RowMenu binds to the nearest .group), which the table had no
              way to offer before. */}
          <td
            className="w-8 px-1 py-2 text-right"
            onClick={(e) => e.stopPropagation()}
          >
            {rowMenu(nb)}
          </td>
        </tr>
      ))}
    </HomeTable>
  );
}

/** One row in the Library's sidebar. Everything in that sidebar is this
 *  shape — a glyph, a name, and either a count, a dot, or a badge — so the
 *  three blocks read as one list of places rather than three widgets. */
function LibraryRow({
  icon,
  label,
  count,
  dot,
  dotClass,
  badge,
  selected,
  onClick,
  title,
}: {
  icon: React.ReactNode;
  label: string;
  /** Trailing count. Omitted (not zeroed) when there is nothing to count. */
  count?: number;
  /** A 6px dot instead of a count: something is waiting, or a state. */
  dot?: boolean;
  dotClass?: string;
  /** A filled count badge — louder than a dot, for a number you must act on. */
  badge?: number;
  selected: boolean;
  onClick: () => void;
  title?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title ?? label}
      aria-current={selected ? "page" : undefined}
      className={cn(
        ROW,
        selected
          ? "bg-[var(--selection)] font-medium text-foreground"
          : "text-muted-foreground hover:bg-surface-2 hover:text-foreground",
      )}
    >
      <span className="flex h-3.5 w-3.5 shrink-0 items-center justify-center">
        {icon}
      </span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {dot && (
        <span
          aria-hidden
          className={cn(
            "h-1.5 w-1.5 shrink-0 rounded-full",
            dotClass ?? "bg-primary",
          )}
        />
      )}
      {badge !== undefined && badge > 0 && (
        <span className="shrink-0 rounded-[9px] bg-primary px-1.5 py-px text-micro font-semibold tabular-nums text-primary-foreground">
          {badge}
        </span>
      )}
      {count !== undefined && !dot && badge === undefined && (
        <span className="shrink-0 text-micro tabular-nums text-subtle-foreground">
          {count}
        </span>
      )}
    </button>
  );
}

function SidebarBlock({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col">
      <div className={cn(CAPS, "px-2 pb-1")}>{title}</div>
      <div className="flex flex-col gap-px">{children}</div>
    </section>
  );
}

/** A note's glyph at card scale. `kindIcon` (studioArtifacts) is the full
 *  vocabulary at 14px; a 212px card only needs the distinction anybody makes
 *  from across the room — a report, or a note. */
function noteGlyph(kind: string) {
  return kind === "report" ? Newspaper : StickyNote;
}

/** One image tile in a thumb's strip. A lead image is a remote og: URL, so
 *  it can 404, expire, or be behind a login — a tile that can't load removes
 *  itself rather than drawing the broken-image glyph, and the strip closes up
 *  around it. */
function ThumbTile({ url }: { url: string }) {
  const [failed, setFailed] = useState(false);
  if (failed) return null;
  return (
    <img
      src={url}
      alt=""
      loading="lazy"
      onError={() => setFailed(true)}
      className="h-9 w-14 shrink-0 rounded-md border border-border object-cover"
    />
  );
}

/** One notebook as the Library draws it: a thumb standing in for what is
 *  inside — the titles it holds, the pictures it holds, the last thing that
 *  was asked of it — then its name and one line of the same. The counts moved
 *  to the tooltip: "29 notes" says how much, and a card has room to say what.
 *
 *  `preview` is undefined until the shelf's one backend call lands, and the
 *  ruled lines are the skeleton for exactly that gap. */
function NotebookCard({
  nb,
  preview,
  unread,
  shared,
  picked,
  onOpen,
  menu,
}: {
  nb: Notebook;
  preview: NotebookPreview | undefined;
  unread: number;
  shared: boolean;
  picked: boolean;
  onOpen: (e: React.MouseEvent) => void;
  menu: React.ReactNode;
}) {
  const color = nb.color || NOTEBOOK_PALETTE[0];
  const images = preview?.images ?? [];
  // An image strip and three lines don't both fit in 140px; the pictures win,
  // because they say more per pixel than a third title does.
  const lineBudget = images.length > 0 ? 2 : 3;
  const note = preview?.notes[0];
  // Sources, then a note: the mix is the point — a card that shows only
  // titles reads as a folder, and one that shows only its note reads as a
  // document. Reserve the last line for the note when there is one.
  const titleRows = (preview?.sources ?? [])
    .slice(0, note ? lineBudget - 1 : lineBudget)
    .map((s) => ({
      key: s.id,
      // The preview carries no `url`, so this is the type's glyph and not the
      // file family's: a .docx reads as text here where a Sources row reads
      // as Word. A card gets the columns a card needs, not a row's.
      Glyph: sourceGlyph(s.sourceType),
      text: s.title,
    }));
  const contentRows = note
    ? [
        ...titleRows,
        { key: note.id, Glyph: noteGlyph(note.kind), text: note.title },
      ]
    : titleRows;

  // The meta line leads with contents too, in the order they're worth
  // knowing: what was written, else what was asked, else what arrived.
  const lead = note
    ? { Glyph: noteGlyph(note.kind), text: note.title }
    : preview?.lastQuestion
      ? { Glyph: MessagesSquare, text: preview.lastQuestion }
      : preview?.sources[0]
        ? {
            Glyph: sourceGlyph(preview.sources[0].sourceType),
            text: preview.sources[0].title,
          }
        : null;
  const metaTail = [
    // Whose share it is would read better than "Shared", but the binding
    // only records THAT a folder is shared, never with whom — no peer name
    // reaches the front end yet (docs/RFC-shared-notebook.md).
    shared && "Shared",
    relativeTime(nb.updatedAt),
  ].filter(Boolean) as string[];
  const counts = [
    `${nb.sourceCount} ${nb.sourceCount === 1 ? "source" : "sources"}`,
    nb.noteCount > 0 &&
      `${nb.noteCount} ${nb.noteCount === 1 ? "note" : "notes"}`,
  ].filter(Boolean) as string[];
  return (
    <div
      data-pick-id={nb.id}
      title={`${nb.title} — ${counts.join(" · ")}`}
      className={cn(
        "group relative flex w-[212px] cursor-pointer flex-col gap-2.5",
        "has-[[aria-expanded=true]]:z-30",
      )}
    >
      <CardAction label={`Open notebook ${nb.title}`} onClick={onOpen} />
      {/* The thumb: the notebook as an object. A dot in its color, its name
          set small, and then what is actually inside — a few of its titles,
          and its pictures along the bottom. */}
      <div
        className={cn(
          "pointer-events-none relative z-10 flex h-[140px] flex-col overflow-hidden rounded-[10px] bg-surface p-3.5",
          "shadow-[inset_0_0_0_0.5px_var(--border-strong)] transition-colors",
          "group-hover:bg-surface-2",
          // A picked card trades its hairline for a ring in the selection's
          // own color: the hairline is the resting edge, the ring is an
          // answer to "which ones did I choose".
          picked &&
            "bg-primary/10 shadow-[inset_0_0_0_1.5px_var(--primary)] group-hover:bg-primary/15",
        )}
      >
        <div className="flex items-center gap-1.5">
          <span
            aria-hidden
            className="h-2.5 w-2.5 shrink-0 rounded-full"
            style={{ backgroundColor: color }}
          />
          <span className="truncate text-badge font-semibold uppercase tracking-[0.04em] text-muted-foreground">
            {nb.title}
          </span>
        </div>
        {!preview ? (
          // Still loading. Widths come from the notebook id, so the skeleton
          // is steady instead of shimmering as React re-runs.
          <div className="mt-3 flex flex-col gap-1.5">
            {thumbLines(nb.id).map((w, i) => (
              <span
                key={i}
                aria-hidden
                className="h-1.5 rounded-[3px] bg-border"
                style={{ width: `${w}%` }}
              />
            ))}
          </div>
        ) : contentRows.length === 0 ? (
          <div className="mt-2.5 truncate text-micro text-subtle-foreground">
            Add a source…
          </div>
        ) : (
          <div className="mt-2.5 flex flex-col gap-1">
            {contentRows.map(({ key, Glyph, text }) => (
              <div
                key={key}
                className="flex min-w-0 items-center gap-1.5 text-micro text-muted-foreground"
              >
                <Glyph className="h-3 w-3 shrink-0" aria-hidden />
                <span className="truncate">{text}</span>
              </div>
            ))}
          </div>
        )}
        {images.length > 0 && (
          <div className="mt-auto flex items-end gap-1.5 pt-2">
            {images.map((url) => (
              <ThumbTile key={url} url={url} />
            ))}
          </div>
        )}
      </div>
      <div className="pointer-events-none relative z-10 flex flex-col gap-0.5">
        <div className="flex items-center gap-1.5">
          <span className="truncate text-body font-semibold text-foreground">
            {nb.title}
          </span>
          {unread > 0 && (
            <span
              className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
              title={`${unread} unread ${unread === 1 ? "report" : "reports"}`}
              aria-label={`${unread} unread reports`}
            />
          )}
        </div>
        <div className="flex min-w-0 items-center gap-1 text-caption text-muted-foreground">
          {lead && (
            <>
              <lead.Glyph className="h-3 w-3 shrink-0" aria-hidden />
              <span className="truncate">{lead.text}</span>
              <span aria-hidden>·</span>
            </>
          )}
          <span className="shrink-0 whitespace-nowrap">
            {metaTail.join(" · ")}
          </span>
        </div>
      </div>
      <div className="absolute right-1.5 top-1.5 z-20 flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
        {menu}
      </div>
    </div>
  );
}

export function HomeView({ onOpenSettings }: { onOpenSettings: () => void }) {
  const notebooks = useStore((s) => s.notebooks);
  const notebookPreviews = useStore((s) => s.notebookPreviews);
  const notebooksFailed = useStore((s) => s.notebooksFailed);
  const open = useStore((s) => s.selectNotebook);
  const create = useStore((s) => s.createNotebook);
  const remove = useStore((s) => s.deleteNotebook);
  const setStatus = useStore((s) => s.setNotebookStatus);
  const registryCounts = useStore((s) => s.registryCounts);
  const theme = useStore((s) => s.theme);
  const homeSection = useStore((s) => s.homeSection);
  const homeView = useStore((s) => s.homeView);
  const homeQuery = useStore((s) => s.homeQuery);
  // Shader must not mount under glass (rAF keeps running when display:none).
  const glassOn = useStore((s) => s.reading.glass);

  const [creating, setCreating] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  // The look is offered up front, like Edit: the color defaults to the
  // palette slot the backend would rotate to, the icon to "from the title".
  const [newIcon, setNewIcon] = useState("");
  const [newColor, setNewColor] = useState("");
  const [editing, setEditing] = useState<Notebook | null>(null);

  /** The Library's one sidebar. Toggled from the toolbar's leading glyph,
   *  like every macOS sidebar; remembered, like every panel here. */
  const [sidebarOpen, setSidebarOpen] = useState(
    () => localStorage.getItem("homeSidebarOpen") !== "0",
  );
  const toggleSidebar = () =>
    setSidebarOpen((on) => {
      localStorage.setItem("homeSidebarOpen", on ? "0" : "1");
      return !on;
    });

  /** Which notebooks the shelf is showing. Shared and Archived are not
   *  places of their own — they are the same shelf, narrowed — so they are a
   *  scope on the notebooks section rather than sections. */
  const [scope, setScope] = useState<"all" | "shared" | "archived">(
    () =>
      (localStorage.getItem("homeScope") as "all" | "shared" | "archived") ||
      "all",
  );
  const goShelf = (next: "all" | "shared" | "archived") => {
    localStorage.setItem("homeScope", next);
    setScope(next);
    useStore.setState({ homeSection: "notebooks", openCardId: null });
  };
  const goSection = (section: HomeSection) =>
    useStore.setState({ homeSection: section, openCardId: null });

  // "system" notebooks (Briefs) are working infrastructure, not shelf items.
  const activeNotebooks = notebooks.filter((n) => !n.status);
  const archivedNotebooks = notebooks.filter((n) => n.status === "archived");

  // One menu for the rows and their right-click, shared with the workspace
  // (src/lib/notebookMenu.tsx). The shelf needs every notebook's binding
  // for the on-disk verbs; the map refreshes with the open notebook's.
  const desktopApps = useStore((s) => s.desktopApps);
  const okfBindings = useStore((s) => s.okfBindings);
  const okfBinding = useStore((s) => s.okfBinding);
  const isShared = (id: string) => !!okfBindings[id]?.shared;
  const sharedNotebooks = activeNotebooks.filter((n) => isShared(n.id));

  const scoped =
    scope === "archived"
      ? archivedNotebooks
      : scope === "shared"
        ? sharedNotebooks
        : activeNotebooks;
  // The toolbar's filter narrows whichever scope is on screen.
  const filteredNotebooks = scoped.filter((n) =>
    matchesHomeQuery(homeQuery, n.title),
  );
  const { sort: nbSort, toggle: toggleNbSort } = useTableSort(
    "homeTableSort",
    { key: "updated", dir: "desc" },
    NOTEBOOK_SORT_KEYS,
  );
  // One order for both shapes: the grid's recency groups decide which shelf
  // a notebook sits on, the sort decides the order within it.
  const shownNotebooks = sortNotebooks(filteredNotebooks, nbSort);
  const sortLabel =
    NOTEBOOK_SORTS.find((s) => s.key === nbSort.key)?.label ??
    "Recently updated";

  const staffTone = useNightShiftTone();
  const homeThreads = useStore((s) => s.homeThreads);
  const chatUnread = useStore((s) => s.homeChatUnread);
  const registryBump = useStore((s) => s.registryBump);
  const registrySignal = useStore((s) => s.registrySignal);
  const registrySeenAt = useStore((s) => s.registrySeenAt);
  // The Registry's badge works from any section, so the signal is read here
  // — at mount and on every registry bump — not by the section that shows it.
  useEffect(() => {
    void useStore.getState().refreshRegistrySignal();
  }, [registryBump]);
  // On screen means seen. Chat's flag drops the moment its section shows; the
  // Registry's baseline moves to now while it shows, so what arrives while
  // you're reading it never badges after you leave.
  useEffect(() => {
    if (homeSection === "chat" && chatUnread)
      useStore.setState({ homeChatUnread: false });
    if (homeSection === "registry") useStore.getState().markRegistrySeen();
  }, [homeSection, chatUnread, registrySignal]);
  // What the cards draw, asked for when the shelf comes on screen. Four
  // corpus scans is not something to run from a notebook or from the Brief,
  // so it is the section that asks; after that `refreshNotebooks` keeps it
  // current on a 5s leash (src/lib/store.ts, `queueNotebookPreviews`).
  useEffect(() => {
    if (homeSection !== "notebooks") return;
    void useStore.getState().refreshNotebookPreviews();
  }, [homeSection]);
  const suggestedCount =
    registrySignal && registrySignal.newest > registrySeenAt
      ? registrySignal.shown
      : 0;

  // View > Chats/Staff/Brief/Nightly Reports (menu.rs), and ⌘1–4 with them.
  // The four used to fold four cards; with the cards gone they choose the
  // section the sidebar row chooses, so the menu still names every surface.
  // Held in a ref so the subscription outlives a render.
  const homeJump = useRef(goSection);
  homeJump.current = goSection;
  useEffect(() => {
    registerHomeCards((card) => {
      if (card === "chats") {
        // Reopens whatever conversation was last on screen, minting a fresh
        // one only when there has never been one.
        void useStore
          .getState()
          .openHomeThread(useStore.getState().homeChat.threadId);
        return;
      }
      homeJump.current(card === "staff" ? "staff" : card === "brief" ? "brief" : "reports");
    });
    return () => registerHomeCards(null);
  }, []);
  useEffect(() => {
    if (!isTauri()) return;
    const label = getCurrentWebview().label;
    // The menu items are disabled off Home, so an action can't arrive with no
    // section to select — it goes through the same registration ⌘1–4 uses.
    const un = listen<{ target: string; id: string }>("menu://action", (e) => {
      if (e.payload.target !== label) return;
      const card = HOME_CARDS.find(
        (c) => e.payload.id === `menu-toggle-home-${c}`,
      );
      if (card) toggleHomeCard(card);
    });
    return () => {
      void un.then((off) => off());
    };
  }, []);

  const { confirm, dialog: confirmDialog } = useConfirm();

  // ---- Shelf selection (docs/RFC-multi-select.md) ----------------------
  const pick = usePickList("notebooks", shownNotebooks.map((n) => n.id));
  const titleOf = (id: string) =>
    notebooks.find((n) => n.id === id)?.title ?? "Untitled";

  const shelfRef = useRef<HTMLDivElement>(null);
  const marqueeBase = useRef<string[]>([]);
  const {
    onPointerDown: marqueeDown,
    marquee,
    justEnded,
  } = useMarquee({
    containerRef: shelfRef,
    onStart: (additive) => {
      const p = useStore.getState().picked;
      marqueeBase.current = additive && p?.kind === "notebooks" ? p.ids : [];
    },
    onSelect: (ids) =>
      pick.pickSet(
        "notebooks",
        [...new Set([...marqueeBase.current, ...ids])],
        false,
      ),
    onClearBackground: pick.clearPicked,
  });

  useEffect(() => {
    void useStore.getState().refreshDesktopApps();
  }, []);
  useEffect(() => {
    void useStore.getState().refreshOkfBindings();
  }, [okfBinding, notebooks.length]);
  const notebookRowItems = (nb: Notebook): RowMenuItem[] =>
    notebookVerbs({
      nb,
      binding: okfBindings[nb.id] ?? null,
      desktopApps,
      onRename: () => setEditing(nb),
      confirm,
    });

  /** Batch verbs for a right-click inside a multi-selection. Archiving is
   *  reversible and needs no confirm; deleting names every notebook it will
   *  take, because the count alone can't be checked against. */
  const notebookBatchItems = (ids: string[]): RowMenuItem[] => [
    {
      label: `Archive ${ids.length} Notebooks`,
      symbol: "archivebox",
      icon: <Archive className="h-3.5 w-3.5" />,
      onClick: () =>
        void (async () => {
          await useStore.getState().archiveNotebooks(ids);
          useStore.getState().clearPicked();
        })(),
    },
    {
      label: `Delete ${ids.length} Notebooks…`,
      symbol: "trash",
      icon: <Trash2 className="h-3.5 w-3.5" />,
      danger: true,
      onClick: () =>
        void (async () => {
          const ok = await confirm({
            title: `Delete ${ids.length} notebooks?`,
            message:
              "This permanently deletes each notebook and all of its sources.",
            items: ids.map(titleOf),
            confirmLabel: "Delete",
            danger: true,
          });
          if (!ok) return;
          for (const id of ids) await remove(id);
          useStore.getState().clearPicked();
          useStore
            .getState()
            .pushToast("success", `Deleted ${ids.length} notebooks`);
        })(),
    },
  ];

  const rowMenuFor = (nb: Notebook) => (
    <RowMenu
      label={`Options for ${nb.title}`}
      contextItems={() => pick.contextItems(nb.id, notebookBatchItems)}
      items={notebookRowItems(nb)}
    />
  );

  // The conversation: one thread over the WHOLE corpus (meta-chat,
  // docs/RFC-meta-chat.md). Its composer lives in the Chat section, where the
  // answers are; the Library reaches it through the Chats row, the Chats
  // sidebar's thread list, ⌘K's ask mode, or by simply typing (below).
  const askRef = useRef<HTMLInputElement>(null);
  const chat = useHomeChat();
  const chatOpen = homeSection === "chat";
  // Half-typed text belongs to the conversation it was typed in, not to the
  // box: switching threads to check something and coming back finds it still
  // there.
  const homeThreadId = useStore((s) => s.homeChat.threadId);
  const draftKey = homeDraftKey(chatOpen, homeThreadId);
  const ask = useStore((s) => s.homeDrafts[draftKey] ?? "");
  const setHomeDraft = useStore((s) => s.setHomeDraft);
  const setAsk = (text: string) => setHomeDraft(draftKey, text);
  function submitAsk(e: React.FormEvent) {
    e.preventDefault();
    const q = ask.trim();
    // A question asked over the top of a running one supersedes it (askHome
    // winds the old one down and keeps its partial), so only a run in THIS
    // conversation blocks the composer — that one has a Stop button instead.
    if (!q || chat.loading) return;
    setAsk("");
    chat.ask(q);
  }
  // A settled answer hands the caret back: the follow-up is the next move,
  // and the composer sits in the same place it was typed in. Arriving in a
  // conversation is the same move — New chat, or a row in the Chats sidebar,
  // mints or opens a thread id, and what you do next is type into it.
  useEffect(() => {
    if (chatOpen && !chat.loading) askRef.current?.focus();
  }, [chatOpen, chat.loading, homeThreadId]);

  // "Since you were away": what landed since the last time home was open.
  const [prevVisit] = useState<number>(() =>
    Number(localStorage.getItem("lastHomeVisit") ?? 0),
  );
  useEffect(() => {
    localStorage.setItem("lastHomeVisit", String(Date.now()));
  }, []);

  const {
    schedules: allSchedules,
    recentNotes,
    stats,
    reports,
    events: sourceEvents,
    loading: activityLoading,
    error: activityError,
    refresh: refreshActivity,
  } = useHomeActivity(notebooks);
  // Archived notebooks' schedules are paused by the backend — showing them
  // as "scheduled" in Staff would be a lie.
  const archivedIds = new Set(archivedNotebooks.map((n) => n.id));
  const allReports = allSchedules.filter((s) => !archivedIds.has(s.notebookId));
  const notebookTitle = new Map(notebooks.map((n) => [n.id, n.title]));
  const notebookColor = new Map(notebooks.map((n) => [n.id, n.color]));

  // Unread-report counts per notebook, for the activity dot on each card.
  const noteReads = useStore((s) => s.noteReads);
  const noteReadsBaseline = useStore((s) => s.noteReadsBaseline);
  const unreadByNb = new Map<string, number>();
  for (const r of reports) {
    if (noteUnread(r, noteReads, noteReadsBaseline)) {
      unreadByNb.set(r.notebookId, (unreadByNb.get(r.notebookId) ?? 0) + 1);
    }
  }
  const totalUnread = [...unreadByNb.values()].reduce((a, b) => a + b, 0);

  function openNote(note: Note) {
    // StudioPanel auto-opens this id once the notebook's notes load.
    useStore.setState({ justCreatedNoteId: note.id });
    void open(note.notebookId);
  }

  // A watcher event reads in its source's own notebook: switch, then open
  // the reader on the source (same shape as the alchemy:// deep links).
  function openEventSource(event: SourceEvent) {
    void open(event.notebookId).then(() => {
      // Growth events are places, not documents: the wiki event opens its
      // index note, a growth event opens the Grow pane itself.
      if (event.kind === "wiki")
        useStore.getState().openInReader({ type: "note", id: event.sourceId });
      else if (event.kind === "growth")
        useStore.setState({ growOpen: true, galleryOpen: false });
      else
        useStore
          .getState()
          .openInReader({ type: "source", id: event.sourceId });
    });
  }

  const startCreate = () => {
    setNewTitle("");
    setNewIcon("");
    setNewColor("");
    setCreating(true);
  };

  // Cmd/Ctrl+N: new notebook.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "n" && !shortcutBlocked(e)) {
        e.preventDefault();
        startCreate();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Type to ask: a bare keystroke with nothing editable focused goes to the
  // conversation. With a composer on screen, focusing on keydown lets the
  // browser deliver the character there itself. On the Library there is no
  // composer to focus — the sheet is a shelf of notebooks — so the keystroke
  // opens a fresh conversation and is planted as its first character, which
  // is what asking from the shelf always did.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!strayTypingKey(e)) return;
      const el = askRef.current;
      if (el) {
        el.focus();
        el.selectionStart = el.selectionEnd = el.value.length;
        return;
      }
      e.preventDefault();
      const char = e.key;
      void useStore
        .getState()
        .openHomeThread(null)
        .then(() => {
          const s = useStore.getState();
          const key = homeDraftKey(true, s.homeChat.threadId);
          s.setHomeDraft(key, (s.homeDrafts[key] ?? "") + char);
        });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Briefs live in their own section, not the reports feed — the feed would
  // double-show them.
  const briefNotes = reports.filter(
    (r) => notebookTitle.get(r.notebookId) === "Briefs",
  );
  const feedReports = reports.filter(
    (r) => notebookTitle.get(r.notebookId) !== "Briefs",
  );
  const briefUnread = briefNotes.some((r) =>
    noteUnread(r, noteReads, noteReadsBaseline),
  );

  const searchRef = useRef<HTMLInputElement>(null);
  useFindFocus(searchRef);

  /** The follow-up composer, docked under the conversation the way a
   *  notebook's is: the thread scrolls, this stays. */
  const askComposer = (
    <>
      <form
        onSubmit={submitAsk}
        className="min-w-0 rounded-xl border border-border bg-surface/80 p-1.5 shadow-sm backdrop-blur transition-colors focus-within:border-primary/50"
      >
        <div className="flex min-w-0 items-center gap-1.5">
          <input
            ref={askRef}
            value={ask}
            onChange={(e) => setAsk(e.target.value)}
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            {...({ writingsuggestions: "false" } as Record<string, string>)}
            placeholder="Ask a follow-up…"
            aria-label="Ask a follow-up across all notebooks"
            className="h-8 min-w-0 flex-1 bg-transparent pl-2.5 pr-1.5 text-body text-foreground outline-none placeholder:text-subtle-foreground"
          />
          {chat.loading ? (
            // Stop keeps whatever streamed — the backend resolves a
            // cancelled run with the partial answer and its citations.
            <Button
              type="button"
              variant="secondary"
              size="sm"
              onClick={chat.stop}
              title="Stop answering (Esc)"
            >
              <Square className="h-3 w-3 fill-current" />
              Stop
            </Button>
          ) : (
            <Button
              type="submit"
              variant="primary"
              size="sm"
              disabled={!ask.trim()}
            >
              Ask
            </Button>
          )}
        </div>
        {/* Style, length, and model — they describe the answer being
            written, so they live with the composer. */}
        <div className="flex items-center gap-1.5 px-1 pt-1.5">
          <HomeChatControls />
        </div>
      </form>
      {activityError && (
        <div
          role="alert"
          className="mt-2 flex items-center gap-3 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-caption text-destructive"
        >
          <span className="min-w-0 flex-1">{activityError}</span>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void refreshActivity()}
            loading={activityLoading}
          >
            Retry
          </Button>
        </div>
      )}
    </>
  );

  /** The sheet's heading: the section's name as the page title, with what it
   *  holds counted on the same baseline. The sections that carry their own
   *  caps header (the conversation, Staff, the Brief, the reports feed) get
   *  none — one name per surface. */
  const shelfTitle =
    scope === "shared" ? "Shared" : scope === "archived" ? "Archived" : "Notebooks";
  const shelfSummary = () => {
    if (scope === "archived")
      return `${archivedNotebooks.length} archived · data intact`;
    const n = scope === "shared" ? sharedNotebooks.length : activeNotebooks.length;
    const head = scope === "shared" ? `${n} shared` : `${n} active`;
    if (!stats) return `${head} · most recently used first`;
    return [
      head,
      `${Intl.NumberFormat().format(stats.sources)} ${stats.sources === 1 ? "source" : "sources"}`,
      stats.notes > 0 &&
        `${Intl.NumberFormat().format(stats.notes)} ${stats.notes === 1 ? "note" : "notes"}`,
    ]
      .filter(Boolean)
      .join(" · ");
  };

  const heading = (() => {
    if (homeSection === "registry")
      return {
        title: "Registry",
        summary:
          registryCounts && registryCounts.total > 0
            ? [
                `${registryCounts.total} ${registryCounts.total === 1 ? "card" : "cards"}`,
                ...registryCounts.kinds.map(
                  (k) => `${k.count} ${k.label.toLowerCase()}`,
                ),
              ].join(" · ")
            : "The things your documents are about: assets, people, projects.",
        actions: (
          <Button
            variant="primary"
            size="sm"
            className="h-[26px] rounded-lg"
            onClick={() => useStore.setState({ registryCreating: true })}
          >
            <Plus className="h-3.5 w-3.5" />
            New card
          </Button>
        ),
      };
    if (homeSection === "timeline")
      return {
        title: "Timeline",
        summary:
          "Every source and note, by the day it arrived — grouped into the batches they came in.",
        actions: null,
      };
    if (homeSection !== "notebooks") return null;
    return {
      title: shelfTitle,
      summary: shelfSummary(),
      // The two ways material arrives that aren't "a new notebook". They sit
      // with the shelf they fill rather than in the toolbar, which the spec
      // gives to New Notebook alone.
      actions: (
        <>
          <Button
            variant="secondary"
            size="sm"
            className="h-[26px] rounded-lg"
            onClick={() =>
              useStore.setState({
                // Empty payload = capture first, then file. Home has no
                // current notebook, so this is the one add path that has to
                // pick one — and it suggests which.
                pendingExternalAdd: {
                  files: [],
                  url: null,
                  text: null,
                  title: null,
                },
              })
            }
            title="Save a link or note; Alchemy suggests the notebook"
          >
            <Plus className="h-3.5 w-3.5" />
            Add source…
          </Button>
          <Button
            variant="secondary"
            size="sm"
            className="h-[26px] rounded-lg"
            onClick={() => useStore.setState({ importOkfOpen: true })}
            title="Import a shared .okf.zip or bundle folder"
          >
            <FolderInput className="h-3.5 w-3.5" />
            Import…
          </Button>
        </>
      ),
    };
  })();

  /** The archived shelf: rows rather than cards, each with the way back. */
  const archivedRows = (
    <div className="flex flex-col gap-1">
      {shownNotebooks.map((nb) => (
        <div
          key={nb.id}
          className="group flex items-center gap-2.5 rounded-md border border-border bg-surface px-3 py-2 transition-colors hover:border-border-strong hover:bg-surface-2"
        >
          <Archive className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <span className="truncate text-body text-foreground">{nb.title}</span>
          <Badge className="shrink-0 gap-1">
            <FileText className="h-2.5 w-2.5" />
            {nb.sourceCount}
          </Badge>
          <div className="ml-auto flex shrink-0 items-center gap-1 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void setStatus(nb.id, "")}
            >
              <ArchiveRestore className="mr-1 h-3.5 w-3.5" />
              Unarchive
            </Button>
            <RowMenu
              label={`Options for ${nb.title}`}
              items={[
                {
                  label: "Delete…",
                  symbol: "trash",
                  icon: <Trash2 className="h-3.5 w-3.5" />,
                  danger: true,
                  onClick: async () => {
                    if (
                      await confirm({
                        title: `Delete "${nb.title}"?`,
                        message:
                          "This permanently deletes the notebook and all of its sources.",
                        confirmLabel: "Delete",
                        danger: true,
                      })
                    )
                      remove(nb.id);
                  },
                },
              ]}
            />
          </div>
        </div>
      ))}
    </div>
  );

  /** The shelf itself, in whichever shape is chosen. */
  const shelf = (
    <>
      {scope === "archived" ? (
        archivedRows
      ) : homeView === "table" ? (
        <NotebookTable
          notebooks={shownNotebooks}
          sort={nbSort}
          onSort={toggleNbSort}
          unreadByNb={unreadByNb}
          pickedIds={pick.pickedIds}
          onRowClick={(e, nb) => {
            if (justEnded()) return;
            if (!pick.handleClick(e, nb.id)) open(nb.id);
          }}
          onRowOpen={(nb) => open(nb.id)}
          rowMenu={rowMenuFor}
        />
      ) : (
        <div className="flex flex-col gap-[18px]">
          {recencyGroups(shownNotebooks).map((group) => (
            <section key={group.label}>
              <div className={cn(CAPS, "pb-2.5")}>{group.label}</div>
              <div className="flex flex-wrap gap-5">
                {group.rows.map((nb) => (
                  <NotebookCard
                    key={nb.id}
                    nb={nb}
                    preview={notebookPreviews[nb.id]}
                    unread={unreadByNb.get(nb.id) ?? 0}
                    shared={isShared(nb.id)}
                    picked={pick.pickedIds.has(nb.id)}
                    onOpen={(e) => {
                      if (justEnded()) return;
                      if (!pick.handleClick(e, nb.id)) open(nb.id);
                    }}
                    menu={rowMenuFor(nb)}
                  />
                ))}
              </div>
            </section>
          ))}
        </div>
      )}
      {shownNotebooks.length === 0 && (
        <p className="py-8 text-center text-body text-muted-foreground">
          {homeQuery.trim()
            ? `No notebook matches “${homeQuery.trim()}”.`
            : scope === "shared"
              ? "No notebook is shared yet. Share one from its own menu."
              : scope === "archived"
                ? "Nothing archived."
                : "No notebooks."}
        </p>
      )}
    </>
  );

  /** What the sheet shows for the selected section. Each of the four moved
   *  surfaces keeps its own component and its own scroll region. */
  const body = (() => {
    if (chatOpen)
      return (
        // The conversation surface: the list of conversations beside the one
        // you are in. It was the left rail's top card; a thread list is part
        // of the chat, not of the shelf, so it travels with it.
        <div className="relative z-10 flex min-h-0 flex-1">
          <HomeThreadsSidebar
            bare
            className="hidden w-[220px] shrink-0 border-r border-border xl:flex"
          />
          <div className="flex min-w-0 flex-1 flex-col">
            <HomeChatThread chat={chat} />
          </div>
        </div>
      );
    if (homeSection === "registry") return <RegistrySection />;
    if (homeSection === "timeline") return <TimelineSection />;
    if (homeSection === "staff")
      return (
        <div className="relative z-10 flex min-h-0 flex-1 flex-col">
          <StaffSidebar
            schedules={allReports}
            reports={reports}
            recentNotes={recentNotes}
            notebookTitle={notebookTitle}
            notebookColor={notebookColor}
            onOpenNote={openNote}
            onOpenNotebook={(id) => void open(id)}
            onOpenEvent={openEventSource}
            onRan={refreshActivity}
          />
        </div>
      );
    if (homeSection === "brief")
      return (
        <BriefSidebar
          bare
          className="relative z-10 min-h-0 flex-1"
          briefs={briefNotes}
          schedules={allReports}
          unread={briefUnread}
          onRan={refreshActivity}
        />
      );
    if (homeSection === "reports")
      return (
        <div className="relative z-10 flex min-h-0 flex-1 flex-col">
          {feedReports.length > 0 ? (
            <ReportsFeed
              reports={feedReports}
              notebookTitle={notebookTitle}
              notebookColor={notebookColor}
              fallbackColor={NOTEBOOK_PALETTE[0]}
              onOpen={openNote}
            />
          ) : activityLoading ? (
            <div
              role="status"
              className="flex flex-1 items-center justify-center p-8 text-caption text-muted-foreground"
            >
              Loading reports…
            </div>
          ) : (
            <div className="flex flex-1 items-center justify-center p-8">
              <EmptyState
                icon={<Newspaper className="h-7 w-7" />}
                title={
                  activityError ? "Reports unavailable" : "Reports appear here"
                }
                hint={
                  activityError
                    ? "Alchemy couldn’t load recent reports."
                    : "Schedule a recurring report from a notebook’s Studio panel."
                }
              >
                {activityError && (
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => void refreshActivity()}
                  >
                    Retry
                  </Button>
                )}
              </EmptyState>
            </div>
          )}
        </div>
      );
    // The Library. The heading is pinned above (as it is for the Registry
    // and the Timeline); only the shelves scroll.
    return (
      <div
        ref={shelfRef}
        onPointerDown={marqueeDown}
        className="relative z-10 flex min-h-0 flex-1 select-none flex-col gap-[18px] overflow-y-auto px-7 pb-[22px]"
      >
        {shelf}
      </div>
    );
  })();

  /** The heading row: the section's name as the page title with what it holds
   *  counted on the same baseline, pinned above whatever scrolls. Sections
   *  that carry their own caps header get none. */
  const headingBlock = heading && (
    <div className="relative z-10 flex shrink-0 flex-col gap-1 px-7 pb-[18px] pt-[22px]">
      <div className="flex flex-wrap items-end justify-between gap-x-6 gap-y-2">
        <div className="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-3 gap-y-1">
          <h1 className="text-[26px] font-bold tracking-[-.01em] text-foreground">
            {heading.title}
          </h1>
          <p className="text-body text-muted-foreground">{heading.summary}</p>
        </div>
        {heading.actions && (
          <div className="flex shrink-0 items-center gap-2">
            {heading.actions}
          </div>
        )}
      </div>
      {homeSection === "notebooks" && (
        <AwayDigest
          prevVisit={prevVisit}
          notebooks={notebooks}
          reports={reports}
          events={sourceEvents}
        />
      )}
    </div>
  );

  /** The Library's footer: what the night shift did, and the way into the
   *  Brief that explains it. Everywhere but the conversation, which docks its
   *  composer in the same place. */
  const footer = chatOpen ? null : (
    <div className="relative z-10 flex shrink-0 items-center gap-4 border-t border-border px-7 pb-3.5 pt-3 text-caption text-muted-foreground">
      {/* A failed activity read used to be reported beside the shelf's ask
          box. The box is gone, and the counts on this line come from the very
          read that failed — so the line says so, rather than reporting the
          zeroes as a quiet night. */}
      {activityError ? (
        <span
          role="alert"
          className="flex min-w-0 flex-1 items-center gap-2 truncate text-destructive"
        >
          {activityError}
          <Button
            variant="ghost"
            size="sm"
            className="h-5"
            onClick={() => void refreshActivity()}
            loading={activityLoading}
          >
            Retry
          </Button>
        </span>
      ) : (
        <span className="min-w-0 flex-1 truncate">
          {lastNightLine({ reports, events: sourceEvents })}
        </span>
      )}
      {/* The one door to the Brief, which is what explains the line on the
          left. Absent when the Brief is already what you are reading. */}
      {homeSection !== "brief" && (
        <button
          type="button"
          onClick={() => goSection("brief")}
          className="shrink-0 rounded text-caption text-citation transition-colors hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
        >
          Read the Brief
        </button>
      )}
    </div>
  );

  const showCollectionControls =
    homeSection === "notebooks" || homeSection === "registry";

  return (
    <div className="app-root flex h-dvh w-screen flex-col overflow-hidden text-foreground">
      {/* One 52px strip, the same class the workspace wears. The left pad is
          the spec's 14px plus the 60px traffic-light gutter. */}
      <header
        data-tauri-drag-region
        className="toolbar flex h-[52px] shrink-0 items-center gap-3 pl-[88px] pr-3.5"
      >
        <Button
          variant="ghost"
          size="icon"
          className="h-6 w-7"
          onClick={toggleSidebar}
          aria-pressed={sidebarOpen}
          title={sidebarOpen ? "Hide the sidebar" : "Show the sidebar"}
          aria-label={sidebarOpen ? "Hide the sidebar" : "Show the sidebar"}
        >
          <PanelLeft className="h-4 w-4" />
        </Button>
        <NavButtons />
        {/* The sigil and the wordmark are also the way back to the shelf from
            any other section, the way a site's logo is its home link. */}
        <button
          type="button"
          onClick={() => goShelf("all")}
          title="Your notebooks"
          className="flex shrink-0 items-center gap-2 rounded-md text-body font-semibold text-foreground transition-colors hover:text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
        >
          <AlchemySymbol
            className="h-4 w-4 shrink-0 text-citation/80"
            strokeWidth={4}
            preferred={THEMES[resolveThemeId(theme)]?.sigil}
          />
          Alchemy
        </button>
        <div className="ml-auto flex items-center gap-3">
          {showCollectionControls && (
            <>
              {/* Archived notebooks have one shape — rows with the way back
                  — so the switch would name a choice that isn't there. */}
              {scope !== "archived" && (
                <Segmented
                  label="View"
                  size="sm"
                  options={HOME_VIEWS}
                  value={homeView}
                  onChange={setHomeView}
                />
              )}
              {homeSection === "notebooks" && (
                <RowMenu
                  label="Sort order"
                  alwaysVisible
                  rowContext={false}
                  tooltip={false}
                  trigger={
                    <>
                      {sortLabel}
                      <ChevronDown className="h-2.5 w-2.5 shrink-0 text-muted-foreground" />
                    </>
                  }
                  triggerClassName={POPUP}
                  items={NOTEBOOK_SORTS.map((o) => ({
                    label: o.label,
                    checked: nbSort.key === o.key,
                    onClick: () => {
                      if (nbSort.key !== o.key) toggleNbSort(o.key, o.dir);
                    },
                  }))}
                />
              )}
              <SearchField
                ref={searchRef}
                variant="field"
                value={homeQuery}
                onValueChange={(v) => useStore.setState({ homeQuery: v })}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    e.stopPropagation();
                    useStore.setState({ homeQuery: "" });
                    searchRef.current?.blur();
                  }
                }}
                placeholder={
                  homeSection === "registry" ? "Filter cards…" : "Filter notebooks…"
                }
                className="w-[200px] shrink-0"
                inputClassName="h-[26px] rounded-lg"
              />
            </>
          )}
          <Button
            variant="primary"
            size="sm"
            className="h-[26px] rounded-lg"
            onClick={startCreate}
            title="New notebook (⌘N)"
          >
            <Plus className="h-3.5 w-3.5" />
            New Notebook
          </Button>
          {/* Left of the DEV pill in dev builds, and the same slot in
              release builds: one place, in every window, that says a model
              is working. */}
          <InferenceActivity />
          <DevBadge />
          <UpdateBadge />
          <Button
            variant="ghost"
            size="icon"
            onClick={onOpenSettings}
            title="Settings"
            aria-label="Open settings"
          >
            <Settings className="h-4 w-4" />
          </Button>
        </div>
      </header>

      {/* Same degraded-state bar the notebook view carries: a model problem
          or a half-finished index is just as true on the shelf, and once
          onboarding has been dismissed this is the only place that says so. */}
      <HealthBanner
        onOpenSettings={() => useStore.getState().openSettings("models")}
      />

      {notebooksFailed ? (
        // Not the same as an empty shelf: the library is probably fine and
        // the read timed out. Offering the new-install hero here invites
        // someone to start over on top of work that is still there.
        <div className="flex-1">
          <EmptyState
            icon={<Library className="h-5 w-5" />}
            title="Couldn't load your notebooks"
            hint="The library didn't answer in time. Nothing has been lost — it may just be busy."
          >
            <Button
              variant="primary"
              className="mt-3"
              onClick={() => void useStore.getState().refreshNotebooks()}
            >
              Try again
            </Button>
          </EmptyState>
        </div>
      ) : notebooks.length === 0 ? (
        <div className="flex-1">
          <AlchemyHero
            title="Alchemy"
            subtitle="Research notebooks that stay on your Mac."
            epigraph={currentEpigraph(theme)}
            themeKey={theme}
          >
            <Button variant="primary" onClick={startCreate}>
              <Plus className="h-4 w-4" />
              New notebook
            </Button>
          </AlchemyHero>
        </div>
      ) : (
        <div className="relative flex min-h-0 flex-1">
          {/* The dither shader from the hero, as a banner behind the heading —
              full window width, running behind the sidebar, fading into the
              background before the shelves start. */}
          {!glassOn && (
            <div
              className="glass-mist pointer-events-none absolute inset-x-0 top-0 h-64 overflow-hidden"
              aria-hidden="true"
            >
              <DitherBackground
                themeKey={theme}
                intensity={2}
                // Home reads the whole corpus: ~200 sources is a full field.
                density={Math.min(
                  1,
                  notebooks.reduce((n, nb) => n + nb.sourceCount, 0) / 200,
                )}
              />
              <div className="absolute inset-0 bg-[linear-gradient(to_bottom,transparent_55%,var(--background)_100%)]" />
            </div>
          )}

          {/* The Library's sidebar: three blocks of places, the material
              itself rather than a card on it (RFC-mac-chrome §2). */}
          {sidebarOpen && (
            <nav
              aria-label="Library"
              className="side-pane relative z-10 hidden w-[220px] shrink-0 flex-col gap-3.5 overflow-y-auto border-r border-border p-2.5 lg:flex"
            >
              <SidebarBlock title="Library">
                <LibraryRow
                  icon={<Library className="h-3.5 w-3.5" />}
                  label="Notebooks"
                  count={activeNotebooks.length}
                  selected={homeSection === "notebooks" && scope === "all"}
                  onClick={() => goShelf("all")}
                />
                <LibraryRow
                  icon={<MessagesSquare className="h-3.5 w-3.5" />}
                  label="Chats"
                  count={homeThreads.length}
                  dot={chatUnread}
                  selected={chatOpen}
                  title="Ask across every notebook"
                  onClick={() =>
                    void useStore
                      .getState()
                      .openHomeThread(useStore.getState().homeChat.threadId)
                  }
                />
                <LibraryRow
                  icon={<Share2 className="h-3.5 w-3.5" />}
                  label="Shared"
                  count={sharedNotebooks.length}
                  selected={homeSection === "notebooks" && scope === "shared"}
                  title="Notebooks kept in a folder you share"
                  onClick={() => goShelf("shared")}
                />
                <LibraryRow
                  icon={<Newspaper className="h-3.5 w-3.5" />}
                  label="Nightly Reports"
                  count={feedReports.length}
                  dot={totalUnread > 0}
                  selected={homeSection === "reports"}
                  title="What the scheduled runs wrote"
                  onClick={() => goSection("reports")}
                />
                <LibraryRow
                  icon={<Archive className="h-3.5 w-3.5" />}
                  label="Archived"
                  count={archivedNotebooks.length}
                  selected={homeSection === "notebooks" && scope === "archived"}
                  onClick={() => goShelf("archived")}
                />
                <LibraryRow
                  icon={<Moon className="h-3.5 w-3.5" />}
                  label="Staff"
                  dot={!!staffTone.label}
                  dotClass={staffTone.dot}
                  selected={homeSection === "staff"}
                  title={
                    staffTone.label
                      ? `The night shift · ${staffTone.label}`
                      : "The night shift"
                  }
                  onClick={() => goSection("staff")}
                />
              </SidebarBlock>

              <SidebarBlock title="Registry">
                <LibraryRow
                  icon={<Package className="h-3.5 w-3.5" />}
                  label="Cards"
                  count={registryCounts?.total ?? 0}
                  selected={homeSection === "registry"}
                  title="The things your documents are about"
                  onClick={() => goSection("registry")}
                />
                <LibraryRow
                  icon={<Sparkles className="h-3.5 w-3.5" />}
                  label="Suggested"
                  // A badge while they are unanswered, a plain count once
                  // they have been looked at: the number is the same, only
                  // the volume changes.
                  count={registrySignal?.shown ?? 0}
                  badge={suggestedCount || undefined}
                  selected={false}
                  title="Cards waiting for a yes or no"
                  onClick={() => goSection("registry")}
                />
              </SidebarBlock>

              {/* Tags belong here (RFC-mac-chrome "Home") and are not built:
                  tags are a per-source field, so a corpus-wide tag list means
                  either a backend rollup or reading every notebook's sources
                  on every render — the scan-storm lesson. The block appears
                  when a corpus tag count exists. */}
            </nav>
          )}

          <div className="sheet relative flex min-w-0 flex-1 flex-col overflow-hidden">
            {headingBlock}
            {body}
            {footer}
            {chatOpen && (
              <div className="relative z-10 w-full shrink-0 px-6 pb-5 pt-2">
                <div className="mx-auto w-full max-w-[760px]">{askComposer}</div>
              </div>
            )}
          </div>
        </div>
      )}

      <Modal
        open={creating}
        onClose={() => setCreating(false)}
        title="New notebook"
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            create(newTitle, {
              icon: newIcon || undefined,
              color: newColor || undefined,
            });
            setCreating(false);
          }}
          className="flex flex-col gap-3"
        >
          <Input
            autoFocus
            name="notebook-title"
            aria-label="Notebook title"
            placeholder="Notebook title"
            value={newTitle}
            onChange={(e) => setNewTitle(e.target.value)}
          />
          <NotebookLookFields
            autoIcon
            icon={newIcon}
            color={
              newColor ||
              NOTEBOOK_PALETTE[notebooks.length % NOTEBOOK_PALETTE.length]
            }
            onIcon={setNewIcon}
            onColor={setNewColor}
          />
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setCreating(false)}
            >
              Cancel
            </Button>
            <Button type="submit" variant="primary">
              Create & open
            </Button>
          </div>
        </form>
      </Modal>

      {marquee}
      {confirmDialog}

      <NotebookEditModal notebook={editing} onClose={() => setEditing(null)} />
    </div>
  );
}
