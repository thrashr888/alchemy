import { useEffect, useRef, useState } from "react";
import { navAtomic, useStore } from "@/lib/store";
import { usePickList } from "@/lib/pick";
import { homeDraftKey } from "@/lib/homeChatRun";
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
import { tagHue } from "@/lib/sourceGroups";
import type {
  Note,
  Notebook,
  NotebookPreview,
  SourceEvent,
} from "@/lib/types";
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
  StickyNote,
  Sun,
  Users,
} from "lucide-react";
import { BriefSidebar, StaffSidebar, useNightShiftTone } from "./HomeSections";
import {
  HomeChatMenu,
  HomeChatSidebarThreads,
  HomeChatThread,
  useHomeChat,
} from "./HomeChat";
import { Composer } from "./Composer";
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

/** What a shared notebook says about who it is shared with.
 *
 *  The binding records devices, not people: nothing on disk names an owner,
 *  because macOS marks a shared item through Foundation resource keys that
 *  neither `mdls` nor an xattr exposes (docs/RFC-shared-notebook.md §1). So
 *  one peer is named and more are counted, and a folder nobody else has
 *  written to yet says only that it is shared.
 *
 *  The serial in a device name ("Anne's MacBook (C02ABC)") is there to tell
 *  two Macs called "MacBook Pro" apart in a record; it is not what anybody
 *  calls the machine, so it comes off before the name reaches a card. */
function peerName(device: string): string {
  return device.replace(/\s*\([^()]*\)\s*$/, "").trim() || device;
}

function sharedLabel(peers: string[] | undefined): string {
  const named = (peers ?? []).map(peerName).filter(Boolean);
  if (named.length === 1) return `Shared with ${named[0]}`;
  if (named.length > 1) return `Shared with ${named.length} devices`;
  return "Shared";
}

/** The two-person mark on a shared notebook, wherever the shelf names one.
 *
 *  Monochrome and 12px on purpose: "shared" is a fact about where the
 *  notebook lives, not a state asking to be acted on, and DESIGN.md spends
 *  color only where it means something. `shrink-0` so a long title truncates
 *  before the mark does — the name can survive being cut, the mark cannot. */
function SharedMark({ label }: { label: string }) {
  // The span carries the tooltip and the name: a lucide icon takes neither
  // a `title` child nor a `title` prop, so hanging them on the glyph itself
  // gives a mark nothing can read and nothing can hover.
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      className="flex shrink-0 items-center text-muted-foreground"
    >
      <Users className="h-3 w-3" aria-hidden />
    </span>
  );
}

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
  builtIns,
  unreadByNb,
  sharedLabelOf,
  rowMenu,
  pickedIds,
  onRowClick,
  onRowOpen,
  sort,
  onSort,
}: {
  notebooks: Notebook[];
  /** The notebooks Alchemy ships, gathered under their own caps row at the
   *  foot of the same table. A second `<table>` would be a second set of
   *  column widths beside the first; a group row keeps one grid. */
  builtIns: Notebook[];
  unreadByNb: Map<string, number>;
  /** "Shared with Anne's MacBook", or null when the notebook is not shared
   *  — the same string the card's meta line and its mark carry. */
  sharedLabelOf: (nb: Notebook) => string | null;
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
  const row = (nb: Notebook) => {
    const shared = sharedLabelOf(nb);
    return (
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
            {shared && <SharedMark label={shared} />}
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
    );
  };

  return (
    <HomeTable columns={[...NOTEBOOK_COLUMNS]} sort={{ ...sort, onSort }}>
      {notebooks.map(row)}
      {builtIns.length > 0 && (
        <>
          {/* The same caps label the grid draws over its Built in shelf,
              carried across the table's full width so the group reads as a
              section rather than as more rows. */}
          <tr>
            <td
              colSpan={NOTEBOOK_COLUMNS.length}
              className={cn(CAPS, "px-3 pb-1.5 pt-5")}
            >
              Built in
            </td>
          </tr>
          {builtIns.map(row)}
        </>
      )}
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

/** The Library's Chats row, drawn as a disclosure rather than a plain
 *  `LibraryRow` — Finder's sidebar folders and Mail's mailbox tree, not a
 *  single link (DESIGN.md §9, "Chats is a `NavigationSplitView`"). The
 *  chevron opens and closes the sessions nested beneath it independently of
 *  selection; the row itself still opens Chats, and the trailing `+` starts
 *  a new conversation without leaving the sidebar to find one. */
function ChatsRow({
  open,
  onToggle,
  selected,
  sectionActive,
  dot,
  onSelect,
  onNewChat,
  title,
}: {
  open: boolean;
  onToggle: () => void;
  /** Washes the row: true only when Chats is open AND the open conversation
   *  is the blank one, so a chosen session's own row can wash instead. */
  selected: boolean;
  /** True whenever Chats is the section on screen, blank or not — widens
   *  the `+` button's visibility past hover/focus. */
  sectionActive: boolean;
  dot: boolean;
  onSelect: () => void;
  onNewChat: () => void;
  title?: string;
}) {
  return (
    <div
      className={cn(
        "group/chats relative flex h-7 w-full items-center gap-0.5 rounded-md pr-1 transition-colors",
        selected ? "bg-[var(--selection)]" : "hover:bg-surface-2",
      )}
    >
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        aria-label={open ? "Collapse Chats" : "Expand Chats"}
        title={open ? "Collapse Chats" : "Expand Chats"}
        className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:text-foreground"
      >
        <ChevronDown
          className={cn("h-3 w-3 transition-transform", !open && "-rotate-90")}
        />
      </button>
      <button
        type="button"
        onClick={onSelect}
        title={title ?? "Ask across every notebook"}
        aria-current={selected ? "page" : undefined}
        className={cn(
          "flex h-full min-w-0 flex-1 items-center gap-2 text-left text-body",
          selected ? "font-medium text-foreground" : "text-muted-foreground",
        )}
      >
        <MessagesSquare className="h-3.5 w-3.5 shrink-0" />
        <span className="min-w-0 flex-1 truncate">Chats</span>
        {dot && (
          <span
            aria-hidden
            className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
          />
        )}
      </button>
      <button
        type="button"
        onClick={onNewChat}
        aria-label="New chat"
        title="New chat"
        className={cn(
          "flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60",
          sectionActive
            ? "opacity-100"
            : "opacity-0 group-hover/chats:opacity-100 group-focus-within/chats:opacity-100",
        )}
      >
        <Plus className="h-3 w-3" />
      </button>
    </div>
  );
}

/** One tag in the Library's sidebar. The same 28px row as everywhere else in
 *  that sidebar, with the dot in the tag's place: tags carry no color in the
 *  model, so it is a stable hash over the app's one categorical palette
 *  (`tagHue`, src/lib/sourceGroups.ts) — the same dot the Sources pane
 *  draws for the same tag, in every theme. */
function TagRow({
  tag,
  count,
  selected,
  onClick,
}: {
  tag: string;
  count: number;
  selected: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={
        selected
          ? `Showing notebooks tagged #${tag} — click to clear`
          : `Show notebooks tagged #${tag}`
      }
      aria-pressed={selected}
      className={cn(
        ROW,
        selected
          ? "bg-[var(--selection)] font-medium text-foreground"
          : "text-muted-foreground hover:bg-surface-2 hover:text-foreground",
      )}
    >
      <span
        aria-hidden
        className="h-2 w-2 shrink-0 rounded-full"
        style={{ background: tagHue(tag) }}
      />
      <span className="min-w-0 flex-1 truncate">{tag}</span>
      <span className="shrink-0 text-micro tabular-nums text-subtle-foreground">
        {count}
      </span>
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
  /** "Shared with Anne's MacBook", "Shared with 2 devices", "Shared" — or
   *  null when this notebook is not shared with anyone. */
  shared: string | null;
  picked: boolean;
  onOpen: (e: React.MouseEvent) => void;
  menu: React.ReactNode;
}) {
  const color = nb.color || NOTEBOOK_PALETTE[0];
  const images = preview?.images ?? [];
  // An image strip and three lines don't both fit in 140px; the pictures win,
  // because they say more per pixel than a third title does.
  const lineBudget = images.length > 0 ? 2 : 3;
  // Wiki pages ("Entity: MSFT", the index) are the notebook's bookkeeping;
  // the backend already ranks them last, so notes[0] is a wiki page only
  // when the notebook has nothing authored or generated. Then the card
  // shows sources rather than the ledger, and the meta line falls through
  // to the question or the newest source. The wiki still counts as a note
  // in the tooltip.
  const note = preview?.notes.find((n) => n.kind !== "wiki");
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
    // The mark beside the name already says "shared", so the meta line only
    // spends a word on it when it has something the mark cannot carry —
    // who. "Shared" alone would be the icon said twice.
    shared && shared !== "Shared" && shared,
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
        "group relative flex w-[212px] cursor-pointer flex-col gap-1.5",
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
      <div className="pointer-events-none relative z-10 flex flex-col gap-0.5 px-1">
        <div className="flex items-center gap-1.5">
          <span className="truncate text-body font-semibold text-foreground">
            {nb.title}
          </span>
          {shared && <SharedMark label={shared} />}
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
  const corpusTags = useStore((s) => s.corpusTags);
  const homeTagFilter = useStore((s) => s.homeTagFilter);
  const setHomeTagFilter = useStore((s) => s.setHomeTagFilter);
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
   *  scope on the notebooks section rather than sections. The store holds
   *  it, not this component: the View menu, ⌘1/⌘3/⌘5 and back/forward all
   *  set it, and none of them can reach a useState that only exists while
   *  Home is mounted. */
  const scope = useStore((s) => s.homeScope);
  const goShelf = useStore((s) => s.goHomeShelf);
  const goSection = useStore((s) => s.goHomeSection);

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
  /** What this notebook says about who it is shared with, or null. */
  const sharedOf = (nb: Notebook) =>
    isShared(nb.id) ? sharedLabel(okfBindings[nb.id]?.peers) : null;
  // Shared is a scope on the shelf, not a shelf of its own: a collaborative
  // notebook is still one of your notebooks, so it stays in the main list
  // and this narrows to it rather than moving it out.
  const sharedNotebooks = activeNotebooks.filter((n) => isShared(n.id));

  // A tag row narrows the shelf to the notebooks that tag's sources sit in.
  // The ids travel with the tag (`corpus_tags`), so this costs no call — and
  // a tag that has gone (its last source retagged) narrows to nothing rather
  // than silently showing everything, which would read as a broken filter.
  const activeTag = homeTagFilter
    ? (corpusTags.find((t) => t.tag === homeTagFilter) ?? {
        tag: homeTagFilter,
        count: 0,
        notebookIds: [] as string[],
      })
    : null;
  /** The rows the Tags block draws: the corpus's busiest, plus the one that
   *  is on if the top eight no longer hold it. DESIGN.md's rule for the
   *  Sources pane, and for the same reason — a filter that is running must
   *  stay visible so it can always be switched off. The selection survives a
   *  relaunch, so without this a tag whose last source was retagged would
   *  leave the shelf empty with nothing on screen to clear. */
  const tagRows =
    activeTag && !corpusTags.some((t) => t.tag === activeTag.tag)
      ? [...corpusTags, activeTag]
      : corpusTags;

  const scoped =
    scope === "archived"
      ? archivedNotebooks
      : scope === "shared"
        ? sharedNotebooks
        : activeNotebooks;
  // The toolbar's filter narrows whichever scope is on screen; the tag row
  // narrows it further, because both are the same question asked two ways.
  const filteredNotebooks = scoped.filter(
    (n) =>
      matchesHomeQuery(homeQuery, n.title) &&
      (!activeTag || activeTag.notebookIds.includes(n.id)),
  );
  const { sort: nbSort, toggle: toggleNbSort } = useTableSort(
    "homeTableSort",
    { key: "updated", dir: "desc" },
    NOTEBOOK_SORT_KEYS,
  );
  // One order for both shapes: the grid's recency groups decide which shelf
  // a notebook sits on, the sort decides the order within it.
  const shownNotebooks = sortNotebooks(filteredNotebooks, nbSort);
  // The notebooks Alchemy ships leave the recency groups and gather at the
  // foot of the shelf. They are real notebooks — same card, same verbs — but
  // they arrived with the app rather than from your work, and mixing them
  // into "Today" makes the shelf answer the wrong question. Archived keeps
  // its own rows, so the split is for the shelf proper.
  const isBuiltIn = (nb: Notebook) => !!nb.builtIn && scope !== "archived";
  const ownNotebooks = shownNotebooks.filter((nb) => !isBuiltIn(nb));
  const builtInNotebooks = shownNotebooks.filter(isBuiltIn);
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
    // The badge counts proposals waiting on an answer, so only the queue
    // clears it: looking at who is already cast is not a ruling on who was
    // proposed, and visiting Cards used to silence the badge anyway.
    if (homeSection === "suggested") useStore.getState().markRegistrySeen();
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

  // The View menu's Home group and ⌘1–⌘9 select these same rows, from the
  // store rather than from here: every place is a section (and, for the
  // shelf, a scope) the store holds, so nothing has to be registered while
  // Home happens to be mounted. See `goHomePlace` in src/lib/store.ts.

  const { confirm, dialog: confirmDialog } = useConfirm();

  // ---- Shelf selection (docs/RFC-multi-select.md) ----------------------
  // In render order, not sort order: shift-click selects the range you see,
  // and the built-ins sit at the foot of the shelf whatever the sort says.
  const pick = usePickList(
    "notebooks",
    [...ownNotebooks, ...builtInNotebooks].map((n) => n.id),
  );
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
  // list's rows, ⌘K's ask mode, or by simply typing (below).
  const askRef = useRef<HTMLTextAreaElement>(null);
  const chat = useHomeChat();
  const chatOpen = homeSection === "chat";
  // Half-typed text belongs to the conversation it was typed in, not to the
  // box: switching threads to check something and coming back finds it still
  // there.
  const homeThreadId = useStore((s) => s.homeChat.threadId);
  const openHomeThread = useStore((s) => s.openHomeThread);
  const draftKey = homeDraftKey(chatOpen, homeThreadId);
  const ask = useStore((s) => s.homeDrafts[draftKey] ?? "");
  const setHomeDraft = useStore((s) => s.setHomeDraft);
  const setAsk = (text: string) => setHomeDraft(draftKey, text);
  function submitAsk() {
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

  // The Chats row's disclosure — its sessions nest under it in the sidebar
  // now, Mail-mailbox style, and whether that sub-list is open persists like
  // the Sources pane's own Tags fold does. Open by default, so the sessions
  // are one glance away the first time a reader meets the sidebar.
  const [chatsSidebarOpen, setChatsSidebarOpen] = useState(
    () => localStorage.getItem("homeChatsOpen") !== "false",
  );
  const toggleChatsSidebar = () => {
    const v = !chatsSidebarOpen;
    localStorage.setItem("homeChatsOpen", String(v));
    setChatsSidebarOpen(v);
  };
  // "Blank" means the open conversation hasn't earned a row yet (RFC-mac-chrome
  // "Home": nothing is written until a turn settles) — the parent row keeps
  // the wash only then, the way Mail's Inbox row stays selected until a
  // message underneath it is, and a chosen session washes instead once one
  // exists.
  const chatBlank = !homeThreads.some((t) => t.id === homeThreadId);

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
  const feedUnreadCount = feedReports.filter((r) =>
    noteUnread(r, noteReads, noteReadsBaseline),
  ).length;
  // What the night shift did, in one line — the sidebar's Brief row wears it
  // as a tooltip, and the Brief section itself repeats it as a quiet second
  // line (DESIGN.md §9 "Home is a library"). It used to be the footer's only
  // job; the footer is now every section's status bar, so this line moved to
  // the one place it is actually about.
  const lastNight = lastNightLine({ reports, events: sourceEvents });

  const searchRef = useRef<HTMLInputElement>(null);
  useFindFocus(searchRef);

  /** The follow-up composer, docked under the conversation the way the
   *  notebook's Chat page docks its own (`Composer`, shared with
   *  `ChatPanel`) — 680 wide, radius 22, one pop-up for how the answer gets
   *  made. No slash commands or @-mentions here (nothing to attach to, and
   *  a corpus question doesn't name one source), so Home wires only Enter
   *  to send. */
  const askComposer = (
    <>
      <Composer
        value={ask}
        onChange={setAsk}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            submitAsk();
          }
        }}
        textareaRef={askRef}
        placeholder="Ask across everything…"
        sending={chat.loading}
        onSubmit={submitAsk}
        onStop={chat.stop}
        menu={<HomeChatMenu />}
      />
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

  /** The sheet's heading: the section's name as the page title, nothing
   *  else — what it holds moved to the status-bar footer below (DESIGN.md
   *  §9 "Home is a library"). Notebooks keeps one more line, the "Since you
   *  were away" digest, because that is about the visit rather than a count. */
  const shelfTitle =
    scope === "shared" ? "Shared" : scope === "archived" ? "Archived" : "Notebooks";

  /** The status bar's one line: what the section on screen holds, the way
   *  Finder counts a window's contents. Registry Cards and Suggested count
   *  themselves; the shelf counts notebooks and, unfiltered, the whole
   *  corpus; the Brief carries nothing here because its line lives in the
   *  section itself (the tooltip and the quiet line below, both fed by
   *  `lastNight`). Chats has no footer at all (see `footer`) — its count
   *  lives on the sidebar's Chats row instead. */
  const footerLine = (): string => {
    if (homeSection === "staff") {
      const n = allReports.length;
      return n > 0
        ? `${n} ${n === 1 ? "schedule" : "schedules"}`
        : "Nothing scheduled";
    }
    if (homeSection === "brief") return "";
    if (homeSection === "reports") {
      const n = feedReports.length;
      if (n === 0) return "No reports yet";
      return feedUnreadCount > 0
        ? `${n} ${n === 1 ? "report" : "reports"} · ${feedUnreadCount} unread`
        : `${n} ${n === 1 ? "report" : "reports"}`;
    }
    if (homeSection === "registry") {
      const n = registryCounts?.total ?? 0;
      return `${n} ${n === 1 ? "entry" : "entries"}`;
    }
    if (homeSection === "suggested") {
      const n = registrySignal?.shown ?? 0;
      return n > 0 ? `${n} waiting` : "Nothing waiting";
    }
    if (homeSection === "timeline")
      return "Every source and note, by the day it arrived.";
    // Notebooks — Shared and Archived are the same shelf, narrowed.
    if (scope === "archived")
      return `${archivedNotebooks.length} archived · data intact`;
    // While a tag is on, the line counts what the tag narrowed to and names
    // it — the status bar is where a filter says it is running, so the
    // shelf never looks mysteriously short. The corpus totals are about the
    // whole corpus, so they stand down rather than describe a subset.
    if (activeTag) {
      const n = filteredNotebooks.length;
      return `${n} ${n === 1 ? "notebook" : "notebooks"} · #${activeTag.tag}`;
    }
    // Built-ins are not counted as yours — they are the shelf's own section
    // with its own count, and "22 notebooks" meaning "16 of them mine" was
    // the number quietly disagreeing with the shelf.
    const own = activeNotebooks.filter((n) => !n.builtIn).length;
    const n = scope === "shared" ? sharedNotebooks.length : own;
    const head =
      scope === "shared" ? `${n} shared` : `${n} ${n === 1 ? "notebook" : "notebooks"}`;
    if (!stats) return head;
    return [
      head,
      `${Intl.NumberFormat().format(stats.sources)} ${stats.sources === 1 ? "source" : "sources"}`,
      stats.notes > 0 &&
        `${Intl.NumberFormat().format(stats.notes)} ${stats.notes === 1 ? "note" : "notes"}`,
    ]
      .filter(Boolean)
      .join(" · ");
  };

  // The status bar's counts for these three sections come from
  // `useHomeActivity` (`stats`/`reports`), so a failed read makes them say
  // so instead of showing a wrong number. Registry and Suggested read their
  // own store slices and are unaffected by that failure.
  const footerActivityDependent =
    homeSection === "notebooks" ||
    homeSection === "reports" ||
    homeSection === "staff";

  // Where the Registry's own sort/suggest/orphan-cleanup controls land: the
  // heading row's trailing slot, beside "New card" — the same slot
  // Notebooks' Add source/Import occupy — rather than a second toolbar row
  // inside the Registry's own content column. `RegistrySection` portals its
  // controls into this node instead of drawing them itself.
  const [registryActionsEl, setRegistryActionsEl] =
    useState<HTMLDivElement | null>(null);

  const heading = (() => {
    if (homeSection === "registry")
      return {
        title: "Entries",
        actions: (
          <>
            <div ref={setRegistryActionsEl} className="flex items-center gap-2" />
            <Button
              variant="primary"
              size="sm"
              className="h-[26px] rounded-lg"
              onClick={() => useStore.setState({ registryCreating: true })}
            >
              <Plus className="h-3.5 w-3.5" />
              New entry
            </Button>
          </>
        ),
      };
    if (homeSection === "suggested")
      return { title: "Suggested", actions: null };
    if (homeSection === "timeline")
      return { title: "Timeline", actions: null };
    if (homeSection === "staff") return { title: "Staff", actions: null };
    if (homeSection === "brief") return { title: "Brief", actions: null };
    if (homeSection === "reports")
      return {
        title: "Nightly Reports",
        actions:
          feedUnreadCount > 0 ? (
            <Button
              variant="ghost"
              size="sm"
              onClick={() =>
                useStore
                  .getState()
                  .markNotesRead(
                    feedReports
                      .filter((r) => noteUnread(r, noteReads, noteReadsBaseline))
                      .map((r) => r.id),
                  )
              }
            >
              Mark all read
            </Button>
          ) : null,
      };
    // Chats draws no heading row at all: the sheet is transcript + composer
    // only. New chat moved to the Library sidebar's Chats row (a small `+`
    // beside it), and the count that used to sit in this row's trailing
    // slot already lives on that sidebar row.
    if (homeSection === "chat") return null;
    if (homeSection !== "notebooks") return null;
    return {
      title: shelfTitle,
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
          notebooks={ownNotebooks}
          builtIns={builtInNotebooks}
          sort={nbSort}
          onSort={toggleNbSort}
          unreadByNb={unreadByNb}
          sharedLabelOf={sharedOf}
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
          {[
            ...recencyGroups(ownNotebooks),
            // After Earlier, and last whatever the sort says: the shipped
            // notebooks are a shelf of their own, with their own count.
            ...(builtInNotebooks.length > 0
              ? [{ label: "Built in", rows: builtInNotebooks }]
              : []),
          ].map((group) => (
            <section key={group.label}>
              <div className={cn(CAPS, "pb-2.5")}>{group.label}</div>
              <div className="flex flex-wrap gap-5">
                {group.rows.map((nb) => (
                  <NotebookCard
                    key={nb.id}
                    nb={nb}
                    preview={notebookPreviews[nb.id]}
                    unread={unreadByNb.get(nb.id) ?? 0}
                    shared={sharedOf(nb)}
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
            : activeTag
              ? `Nothing is tagged #${activeTag.tag} any more.`
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
        // The sessions list moved into the Library sidebar's Chats
        // disclosure (a sub-list, Mail-mailbox style), so the sheet is just
        // transcript + composer now — full width, no second column, no
        // narrow-width collapse to carry. The composer sits 18px above the
        // sheet's bottom edge; there is no footer on this section to dock
        // above instead (see `footerLine`).
        <div className="relative z-10 flex min-h-0 flex-1 flex-col">
          <HomeChatThread chat={chat} />
          <div className="relative z-10 w-full shrink-0 px-6 pb-[18px] pt-2">
            <div className="mx-auto w-full max-w-[680px]">{askComposer}</div>
          </div>
        </div>
      );
    if (homeSection === "registry")
      return (
        <RegistrySection view="cards" actionsPortal={registryActionsEl} />
      );
    if (homeSection === "suggested")
      return <RegistrySection view="suggested" />;
    if (homeSection === "timeline") return <TimelineSection />;
    if (homeSection === "staff")
      return (
        <div className="relative z-10 flex min-h-0 flex-1 flex-col">
          <StaffSidebar
            bare
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
          lastNight={lastNight}
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

  /** The heading row: the section's name as the page title, pinned above
   *  whatever scrolls, plus its trailing controls. What it holds is the
   *  footer's job now (below); the one exception is Notebooks' "Since you
   *  were away" line, which is about the visit rather than a count. */
  const headingBlock = heading && (
    <div className="relative z-10 flex shrink-0 flex-col gap-1 px-7 pb-[18px] pt-[22px]">
      <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2">
        <h1 className="min-w-0 flex-1 text-[26px] font-bold tracking-[-.01em] text-foreground">
          {heading.title}
        </h1>
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

  /** The Library's footer: a Finder-style status bar, one centered line of
   *  what the section on screen holds. Every section gets one except Chats:
   *  the sessions list that used to need "4 conversations" said here now
   *  lives as the sidebar's own count, and a status bar under the composer
   *  with nothing left to say was a hairline that added nothing — the
   *  composer sits 18px above the sheet's bottom edge instead, like the
   *  notebook's own chat. */
  const footer = homeSection === "chat" ? null : (
    <div className="relative z-10 flex shrink-0 items-center justify-center border-t border-border px-7 pb-3.5 pt-3 text-center text-caption text-muted-foreground">
      {/* A failed activity read used to be reported as the whole footer. The
          counts on sections fed by that same read say so instead of showing
          a wrong number; Registry and Suggested read their own store slices
          and are never affected by it. */}
      {activityError && footerActivityDependent ? (
        <span
          role="alert"
          className="flex min-w-0 items-center gap-2 truncate text-destructive"
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
        footerLine() && <span className="min-w-0 truncate">{footerLine()}</span>
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
                  homeSection === "registry" ? "Filter entries…" : "Filter notebooks…"
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

          {/* The Library's sidebar: the Brief on its own above three blocks
              of places, the material itself rather than a card on it
              (RFC-mac-chrome §2). */}
          {sidebarOpen && (
            <nav
              aria-label="Library"
              className="side-pane relative z-10 hidden w-[220px] shrink-0 flex-col gap-3.5 overflow-y-auto border-r border-border p-2.5 lg:flex"
            >
              {/* No caps label of its own — the arrival point sits above the
                  Library rather than inside it, the way Mail's Inbox sits
                  above its own sidebar's account blocks. */}
              <LibraryRow
                icon={<Sun className="h-3.5 w-3.5" />}
                label="Brief"
                dot={briefUnread}
                selected={homeSection === "brief"}
                title={lastNight}
                onClick={() => goSection("brief")}
              />

              <SidebarBlock title="Library">
                <LibraryRow
                  icon={<Library className="h-3.5 w-3.5" />}
                  label="Notebooks"
                  count={activeNotebooks.length}
                  selected={homeSection === "notebooks" && scope === "all"}
                  onClick={() => goShelf("all")}
                />
                <div className="flex flex-col">
                  <ChatsRow
                    open={chatsSidebarOpen}
                    onToggle={toggleChatsSidebar}
                    // The parent washes when it is the visible selection:
                    // a blank new conversation, or an open thread whose
                    // own row is hidden behind a folded sub-list.
                    selected={chatOpen && (chatBlank || !chatsSidebarOpen)}
                    sectionActive={chatOpen}
                    dot={chatUnread}
                    onSelect={() =>
                      void useStore
                        .getState()
                        .openHomeThread(useStore.getState().homeChat.threadId)
                    }
                    onNewChat={() => void navAtomic(() => openHomeThread(null))}
                  />
                  {chatsSidebarOpen && <HomeChatSidebarThreads />}
                </div>
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
                  label="Entries"
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
                  selected={homeSection === "suggested"}
                  title="Entries waiting for a yes or no"
                  onClick={() => goSection("suggested")}
                />
              </SidebarBlock>

              {/* The third block (RFC-mac-chrome "Home"). Tags are a
                  per-source field, so a corpus-wide list is a rollup — one
                  projected scan in Rust (`corpus_tags`) on the same leash as
                  the cards' contents, never a read of every notebook's
                  sources on every render (the scan-storm lesson). The block
                  is absent until something is tagged, rather than standing
                  there empty. */}
              {tagRows.length > 0 && (
                <SidebarBlock title="Tags">
                  {tagRows.map((t) => (
                    <TagRow
                      key={t.tag}
                      tag={t.tag}
                      count={t.count}
                      selected={homeTagFilter === t.tag}
                      onClick={() => {
                        // A tag narrows the shelf, so choosing one goes to
                        // the shelf — from the Registry or the Brief it
                        // would otherwise filter a page you cannot see.
                        setHomeTagFilter(t.tag);
                        if (homeSection !== "notebooks") goShelf("all");
                      }}
                    />
                  ))}
                </SidebarBlock>
              )}
            </nav>
          )}

          <div className="sheet relative flex min-w-0 flex-1 flex-col overflow-hidden">
            {headingBlock}
            {body}
            {footer}
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
