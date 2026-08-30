import { useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useStore } from "@/lib/store";
import { usePickList } from "@/lib/pick";
import { homeDraftKey } from "@/lib/homeChatRun";
import { HOME_CARDS, registerHomeCards, toggleHomeCard } from "@/lib/homeCards";
import { DevBadge } from "./DevBadge";
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
  ResizeHandle,
  RowMenu,
  type RowMenuItem,
  useMarquee,
  useConfirm,
} from "./ui";
import { AlchemyHero } from "./AlchemyHero";
import { currentEpigraph } from "@/lib/epigraph";
import { DitherBackground } from "./DitherBackground";
import { useHomeActivity } from "./useHomeActivity";
import { AwayDigest, ReportsFeed } from "./HomeReportsFeed";
import {
  cn,
  noteUnread,
  relativeTime,
  shortcutBlocked,
} from "@/lib/utils";
import type { Note, Notebook, SourceEvent } from "@/lib/types";
import {
  Archive,
  ArchiveRestore,
  FileDown,
  ChevronRight,
  MessagesSquare,
  PanelRightClose,
  Plus,
  Search,
  Settings,
  Trash2,
  Pencil,
  FileText,
  Newspaper,
  Package,
  FolderInput,
  Library,
  Square,
} from "lucide-react";
import { BriefSidebar, SidebarRail, StaffSidebar } from "./HomeSections";
import {
  HomeChatControls,
  HomeChatThread,
  HomeThreadsSidebar,
  useHomeChat,
} from "./HomeChat";
import { NOTEBOOK_ICONS, notebookIcon } from "@/lib/notebookIcons";
import { RegistrySection } from "./RegistrySection";
import {
  HomeTable,
  HomeViewControls,
  matchesHomeQuery,
} from "./HomeViewControls";

// Keep this list in sync with Rust in `src-tauri/src/db.rs` (`NOTEBOOK_PALETTE`)
// and the `set_notebook_color` validator in `src-tauri/src/commands.rs`.
const NOTEBOOK_PALETTE = [
  "#eb5757",
  "#e8a33d",
  "#4cb782",
  "#5e9bd2",
  "#9b87f5",
  "#e274b6",
  "#4fc1c9",
  "#98a562",
];

const clampSplit = (pct: number) => Math.min(75, Math.max(15, pct));

/** The horizontal handle between two stacked side-cards. Both rails stack the
 *  same way — Chats over Staff on the left, Brief over Latest reports on the
 *  right — so they drag the same way too. */
function StackSplit({
  colRef,
  pct,
  onChange,
  defaultPct,
  label,
}: {
  /** The column the two cards share; the drag is a fraction of its height. */
  colRef: React.RefObject<HTMLDivElement | null>;
  pct: number;
  onChange: (pct: number) => void;
  defaultPct: number;
  label: string;
}) {
  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const col = colRef.current;
    if (!col) return;
    const rect = col.getBoundingClientRect();
    const move = (ev: PointerEvent) =>
      onChange(clampSplit(((ev.clientY - rect.top) / rect.height) * 100));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.style.cursor = "";
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    document.body.style.cursor = "row-resize";
  };
  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label={label}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onDoubleClick={() => onChange(defaultPct)}
      onKeyDown={(e) => {
        // Arrow keys nudge the split — the same keyboard affordance
        // ResizeHandle gives the vertical edges.
        const delta = e.key === "ArrowDown" ? 2 : e.key === "ArrowUp" ? -2 : 0;
        if (!delta) return;
        e.preventDefault();
        onChange(clampSplit(pct + delta));
      }}
      className="group/resize relative h-2 shrink-0 cursor-row-resize rounded transition-colors hover:bg-ring/30 active:bg-ring/40 focus-visible:bg-ring/30"
    >
      <span
        aria-hidden
        className="absolute top-1/2 left-1/2 flex -translate-x-1/2 -translate-y-1/2 gap-0.5 opacity-40 transition-opacity group-hover/resize:opacity-100 group-focus-visible/resize:opacity-100"
      >
        <span className="h-0.5 w-0.5 rounded-full bg-muted-foreground" />
        <span className="h-0.5 w-0.5 rounded-full bg-muted-foreground" />
        <span className="h-0.5 w-0.5 rounded-full bg-muted-foreground" />
      </span>
    </div>
  );
}

/** The scannable form of the notebook shelf. Same rows the grid shows, read
 *  down columns instead of across cards. */
function NotebookTable({
  notebooks,
  onNew,
  unreadByNb,
  rowMenu,
  pickedIds,
  onRowClick,
  onRowOpen,
  colorPop,
}: {
  notebooks: Notebook[];
  onNew: () => void;
  unreadByNb: Map<string, number>;
  /** Per-row menu, so the table has the same verbs (and the same
   *  right-click) as the cards — it had neither. */
  rowMenu: (nb: Notebook) => React.ReactNode;
  pickedIds: Set<string>;
  onRowClick: (e: React.MouseEvent, nb: Notebook) => void;
  /** Keyboard path: Tab reaches each row, Enter opens it (bypassing the
   *  pointer-only selection logic in onRowClick). */
  onRowOpen: (nb: Notebook) => void;
  /** The color palette pop-over ("Change color…" in the row menu) — rendered
   *  by the host so the table shares the grid's dismissal wiring. */
  colorPop: (nb: Notebook) => React.ReactNode;
}) {
  return (
    <>
      <HomeTable
        columns={[
          { key: "title", label: "Title" },
          { key: "sources", label: "Sources", className: "text-right" },
          { key: "notes", label: "Notes", className: "text-right" },
          { key: "reports", label: "Reports", className: "text-right" },
          { key: "updated", label: "Updated" },
          { key: "menu", label: "", className: "w-8" },
        ]}
      >
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
              {colorPop(nb)}
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
            <td className="w-8 px-1 py-2 text-right" onClick={(e) => e.stopPropagation()}>
              {rowMenu(nb)}
            </td>
          </tr>
        ))}
      </HomeTable>
      <Button variant="secondary" className="mt-3" onClick={onNew}>
        <Plus className="h-4 w-4" />
        New notebook
      </Button>
    </>
  );
}

/** Home's center switch, the exact sibling of the notebook's
 *  Chat|Reader|Gallery|Ledger tabs (CenterModeTabs, ReaderPane.tsx): one
 *  control, in the title bar, choosing what the center column shows about a
 *  constant subject. There the subject is one notebook; here it's the whole
 *  corpus — its notebooks, the cast of things they're about, or the
 *  conversation you're having with all of them at once. Same kind of switch,
 *  so it lives in the same place and wears the same chrome. */
function HomeSectionTabs() {
  const section = useStore((s) => s.homeSection);

  const tabs = [
    { id: "notebooks", label: "Notebooks", icon: Library },
    { id: "chat", label: "Chat", icon: MessagesSquare },
    { id: "registry", label: "Registry", icon: Package },
  ] as const;
  return (
    <div className="flex items-center gap-0.5 rounded-lg border border-border p-0.5">
      {tabs.map(({ id, label, icon: Icon }) => (
        <button
          key={id}
          type="button"
          onClick={() => {
            if (id === "chat") {
              // Reopens whatever conversation was last on screen, minting a
              // fresh one only when there has never been one.
              void useStore
                .getState()
                .openHomeThread(useStore.getState().homeChat.threadId);
              return;
            }
            useStore.setState({ homeSection: id, openCardId: null });
          }}
          aria-pressed={section === id}
          title={
            id === "registry"
              ? "The things your documents are about"
              : id === "chat"
                ? "Ask across every notebook"
                : "Your notebooks"
          }
          className={cn(
            "flex items-center gap-1.5 rounded-md px-2 py-1 text-caption transition-colors",
            section === id
              ? "bg-surface-2 font-medium text-foreground"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          <Icon className="h-3.5 w-3.5" />
          {label}
        </button>
      ))}
    </div>
  );
}

export function HomeView({ onOpenSettings }: { onOpenSettings: () => void }) {
  const notebooks = useStore((s) => s.notebooks);
  const notebooksFailed = useStore((s) => s.notebooksFailed);
  const open = useStore((s) => s.selectNotebook);
  const create = useStore((s) => s.createNotebook);
  const rename = useStore((s) => s.renameNotebook);
  const setColor = useStore((s) => s.setNotebookColor);
  const remove = useStore((s) => s.deleteNotebook);
  const setStatus = useStore((s) => s.setNotebookStatus);
  const theme = useStore((s) => s.theme);
  const homeSection = useStore((s) => s.homeSection);
  const homeView = useStore((s) => s.homeView);
  const homeQuery = useStore((s) => s.homeQuery);
  // Shader must not mount under glass (rAF keeps running when display:none).
  const glassOn = useStore((s) => s.reading.glass);

  // Finder-style selection over the shelf (docs/RFC-multi-select.md), the
  // same grammar the sources and notes lists use.
  const shownIdsRef = useRef<string[]>([]);
  const [creating, setCreating] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [renaming, setRenaming] = useState<{
    id: string;
    title: string;
    icon: string;
  } | null>(null);
  const [colorPickerFor, setColorPickerFor] = useState<string | null>(null);
  const [archivedOpen, setArchivedOpen] = useState(false);
  // "system" notebooks (Briefs) are working infrastructure, not shelf items.
  const activeNotebooks = notebooks.filter((n) => !n.status);
  const archivedNotebooks = notebooks.filter((n) => n.status === "archived");
  // The inline filter narrows both views; the archived shelf is untouched
  // (it's already a deliberate drill-in).
  const shownNotebooks = activeNotebooks.filter((n) =>
    matchesHomeQuery(homeQuery, n.title),
  );
  // The Steward's sidebars (RFC-v12-steward UI §2, as sidebars): Staff on
  // the left, Brief above Latest Reports on the right. Each collapses on
  // its own, persisted. Registry joins when its pillar exists.
  const [reportsOpen, setReportsOpen] = useState(
    () => localStorage.getItem("homeReportsOpen") !== "0",
  );
  const toggleReports = () => {
    setReportsOpen((open) => {
      localStorage.setItem("homeReportsOpen", open ? "0" : "1");
      return !open;
    });
  };
  const [staffOpen, setStaffOpen] = useState(
    () => localStorage.getItem("homeStaffOpen") !== "0",
  );
  const toggleStaff = () => {
    setStaffOpen((open) => {
      localStorage.setItem("homeStaffOpen", open ? "0" : "1");
      return !open;
    });
  };
  // The Chats card collapses on its own, the way Staff below it and Brief
  // opposite do — same key grammar. Its default is the one that differs:
  // before there is a first conversation the card has nothing to list, so a
  // rail nobody has touched starts folded and opens itself the moment the
  // first chat exists (the effect below). An ABSENT `homeChatsOpen` is what
  // "never chose" means — every deliberate toggle writes it.
  const [chatsOpen, setChatsOpen] = useState(
    () => (localStorage.getItem("homeChatsOpen") ?? "0") !== "0",
  );
  const toggleChats = () => {
    setChatsOpen((open) => {
      localStorage.setItem("homeChatsOpen", open ? "0" : "1");
      return !open;
    });
  };
  // The one auto-open, on the 0 → 1 crossing of the thread list: the first
  // conversation you ever have is what makes the card worth its width, so it
  // shows up on its own the moment it has something to list. Writing the key
  // as it fires is what keeps this to once, ever — a later collapse, deleting
  // every thread and starting over, and a second window all read the same
  // written key and leave the card where it was put.
  //
  // The ref starts false rather than at the current count on purpose: a first
  // chat asked from ⌘K inside a notebook lands while this view is unmounted,
  // and a crossing measured from the count at mount would never fire for it.
  // "Never chose, and there is now a conversation" is the real condition; the
  // written key, not the count, is what makes it once. (Empty threads minted
  // by New chat or the palette aren't in `homeThreads` until a turn settles,
  // so this means a real answer rather than an open box.)
  const homeThreads = useStore((s) => s.homeThreads);
  const sawThreads = useRef(false);
  useEffect(() => {
    if (!homeThreads.length || sawThreads.current) return;
    sawThreads.current = true;
    if (localStorage.getItem("homeChatsOpen") !== null) return;
    localStorage.setItem("homeChatsOpen", "1");
    setChatsOpen(true);
  }, [homeThreads]);
  const [briefOpen, setBriefOpen] = useState(
    () => localStorage.getItem("homeBriefOpen") !== "0",
  );
  const clampStaffW = (w: number) => Math.min(440, Math.max(240, w));
  const [staffWidth, setStaffWidth] = useState(() =>
    clampStaffW(Number(localStorage.getItem("homeStaffWidth") ?? 300)),
  );
  const clampRightW = (w: number) => Math.min(820, Math.max(360, w));
  const [rightWidth, setRightWidth] = useState(() =>
    clampRightW(Number(localStorage.getItem("homeRightWidth") ?? 520)),
  );
  const [briefSplit, setBriefSplit] = useState(() =>
    clampSplit(Number(localStorage.getItem("homeBriefSplit") ?? 40)),
  );
  // The left rail splits the same way when it stacks: Chats over Staff. The
  // percentage is the TOP card's height, so it names Chats — its own key,
  // since `homeStaffSplit` measured the other card.
  const [chatsSplit, setChatsSplit] = useState(() =>
    clampSplit(Number(localStorage.getItem("homeChatsSplit") ?? 45)),
  );
  const leftColRef = useRef<HTMLDivElement>(null);
  const rightColRef = useRef<HTMLDivElement>(null);
  const staffResizeHandle = (
    <ResizeHandle
      edge="right"
      width={staffWidth}
      defaultWidth={300}
      label="Resize the Staff sidebar"
      onResize={(w) => {
        const width = clampStaffW(w);
        setStaffWidth(width);
        localStorage.setItem("homeStaffWidth", String(Math.round(width)));
      }}
    />
  );
  // The reading column's resize handle, rendered once per stacked card —
  // each card's left edge is the column's, so either drags the whole column.
  const rightResizeHandle = (
    <ResizeHandle
      edge="left"
      width={rightWidth}
      defaultWidth={520}
      label="Resize the reading column"
      onResize={(w) => {
        const width = clampRightW(w);
        setRightWidth(width);
        localStorage.setItem("homeRightWidth", String(Math.round(width)));
      }}
    />
  );
  const toggleBrief = () => {
    setBriefOpen((open) => {
      localStorage.setItem("homeBriefOpen", open ? "0" : "1");
      return !open;
    });
  };

  // View > Chats/Staff/Brief/Latest Reports (menu.rs), and ⌘1–4 with them.
  // The four cards' open state is this component's — and this component only
  // mounts on Home — so the toggles live here rather than in the store's menu
  // router, and they flip exactly the state each card's own collapse button
  // writes. Held in a ref so a subscription outlives a render.
  const homeToggles = useRef({
    toggleChats,
    toggleStaff,
    toggleBrief,
    toggleReports,
  });
  homeToggles.current = { toggleChats, toggleStaff, toggleBrief, toggleReports };
  // ⌘1–4 is caught above both views (App.tsx), since the same keys mean a
  // notebook's panels when one is open — so publish the toggles for it.
  useEffect(() => {
    registerHomeCards((card) => {
      const t = homeToggles.current;
      if (card === "chats") t.toggleChats();
      else if (card === "staff") t.toggleStaff();
      else if (card === "brief") t.toggleBrief();
      else t.toggleReports();
    });
    return () => registerHomeCards(null);
  }, []);
  useEffect(() => {
    if (!isTauri()) return;
    const label = getCurrentWebview().label;
    // The menu items are disabled off Home, so an action can't arrive with no
    // card to toggle — it goes through the same registration ⌘1–4 uses.
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
  shownIdsRef.current = shownNotebooks.map((n) => n.id);
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

  /** The single-notebook verbs, shared by the cards and the table so both
   *  surfaces offer the same menu (and the same right-click). */
  const notebookRowItems = (nb: Notebook): RowMenuItem[] => [

    {
      label: "Rename",
      icon: <Pencil className="h-3.5 w-3.5" />,
      onClick: () =>
        setRenaming({
          id: nb.id,
          title: nb.title,
          icon: nb.icon,
        }),
    },
    // Right-click parity: color was hover-swatch-only (cards) and export was
    // palette-only — both belong on the object's one menu.
    {
      label: "Change color…",
      icon: (
        <span
          className="block h-3.5 w-3.5 rounded-full border border-border"
          style={{ backgroundColor: nb.color || NOTEBOOK_PALETTE[0] }}
        />
      ),
      onClick: () => setColorPickerFor(nb.id),
    },
    {
      label: "Export notebook…",
      icon: <FileDown className="h-3.5 w-3.5" />,
      onClick: () => void useStore.getState().exportNotebookOkf(nb.id),
    },
    {
      label: "Archive",
      icon: <Archive className="h-3.5 w-3.5" />,
      onClick: () => void setStatus(nb.id, "archived"),
    },
    {
      label: "Delete…",
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
  ];

  /** Batch verbs for a right-click inside a multi-selection. Archiving is
   *  reversible and needs no confirm; deleting names every notebook it will
   *  take, because the count alone can't be checked against. */
  const notebookBatchItems = (ids: string[]): RowMenuItem[] => [
    {
      label: `Archive ${ids.length} notebooks`,
      icon: <Archive className="h-3.5 w-3.5" />,
      onClick: () =>
        void (async () => {
          for (const id of ids) await setStatus(id, "archived");
          useStore.getState().clearPicked();
          useStore
            .getState()
            .pushToast("success", `Archived ${ids.length} notebooks`);
        })(),
    },
    {
      label: `Delete ${ids.length} notebooks…`,
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

  // The unified ask box: one input over the WHOLE corpus. Enter lands you in
  // the Chat tab (meta-chat, docs/RFC-meta-chat.md) — no notebook choice
  // needed; citations name where the answers live. ⌘K's ask mode is the same
  // pipeline in glance form; this one keeps the thread, and keeps it for good.
  const askRef = useRef<HTMLInputElement>(null);
  const chat = useHomeChat();
  const chatOpen = homeSection === "chat";
  // Half-typed text belongs to the conversation it was typed in, not to the
  // box: switching threads to check something and coming back finds it still
  // there. The shelf keeps its own slot — a question typed over the notebook
  // grid isn't a follow-up to anything.
  const homeThreadId = useStore((s) => s.homeChat.threadId);
  const draftKey = homeDraftKey(chatOpen, homeThreadId);
  const ask = useStore((s) => s.homeDrafts[draftKey] ?? "");
  const setHomeDraft = useStore((s) => s.setHomeDraft);
  const setAsk = (text: string) => setHomeDraft(draftKey, text);
  async function submitAsk(e: React.FormEvent) {
    e.preventDefault();
    const q = ask.trim();
    // A question asked over the top of a running one supersedes it (askHome
    // winds the old one down and keeps its partial), so only a run in THIS
    // conversation blocks the composer — that one has a Stop button instead.
    if (!q || chat.loading) return;
    setAsk("");
    // Asking from the shelf is asking to be in the conversation — a new one:
    // a question typed over the notebook grid is a fresh subject, and
    // grafting it onto whatever was last discussed would send that thread's
    // history to the model as context for it. Follow-ups are asked from
    // inside the Chat tab, where this same box is the follow-up composer.
    // The thread must be open (and its id minted) before the run starts.
    if (!chatOpen) await useStore.getState().openHomeThread(null);
    chat.ask(q);
  }
  // A settled answer hands the caret back: the follow-up is the next move,
  // and the composer sits in the same place it was typed in. Arriving in a
  // conversation is the same move — New chat, or a row in the Chats card,
  // mints or opens a thread id, and what you do next is type into it, so the
  // caret is already there rather than parked on the button you pressed.
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

  // Palette popup stays local to one card and closes on outside interaction or Escape.
  useEffect(() => {
    if (!colorPickerFor) return;
    const onPointerDown = (e: PointerEvent) => {
      const t = e.target as HTMLElement | null;
      if (
        t &&
        (t.closest("[data-notebook-color-trigger]") ||
          t.closest("[data-notebook-color-palette]"))
      ) {
        return;
      }
      setColorPickerFor(null);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setColorPickerFor(null);
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [colorPickerFor]);

  const onPickColor = (notebookId: string, color: string) => {
    setColorPickerFor(null);
    setColor(notebookId, color);
  };

  /** The color palette pop-over, positioned by the host (card corner or
   *  table row). One markup: the outside-click/Escape dismissal keys off
   *  its data attribute, so it works wherever it renders. */
  const colorPalettePop = (nb: Notebook, className: string) =>
    colorPickerFor === nb.id ? (
      <div
        onClick={(e) => e.stopPropagation()}
        onPointerDown={(e) => e.stopPropagation()}
        data-notebook-color-palette
        className={cn(
          "menu-glass absolute z-30 flex rounded-md border border-border px-2 py-1.5 shadow-sm",
          className,
        )}
      >
        {NOTEBOOK_PALETTE.map((c) => (
          <button
            key={c}
            type="button"
            onClick={() => onPickColor(nb.id, c)}
            onPointerDown={(e) => e.stopPropagation()}
            aria-label={`Set ${nb.title} color to ${c}`}
            className={cn(
              "m-0.5 h-5 w-5 rounded-full border border-border",
              c === (nb.color || NOTEBOOK_PALETTE[0])
                ? "ring-2 ring-foreground ring-offset-1 ring-offset-surface"
                : "",
            )}
            style={{ backgroundColor: c }}
          />
        ))}
      </div>
    ) : null;

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
        useStore.setState({
          growOpen: true,
          galleryOpen: false,
          ledgerOpen: false,
        });
      else
        useStore
          .getState()
          .openInReader({ type: "source", id: event.sourceId });
    });
  }

  // Cmd/Ctrl+N: new notebook.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "n" && !shortcutBlocked(e)) {
        e.preventDefault();
        setNewTitle("");
        setCreating(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Briefs live in their own sidebar card, not the reports feed — the feed
  // would double-show them one card below.
  const briefNotes = reports.filter(
    (r) => notebookTitle.get(r.notebookId) === "Briefs",
  );
  const feedReports = reports.filter(
    (r) => notebookTitle.get(r.notebookId) !== "Briefs",
  );
  const briefUnread = briefNotes.some((r) =>
    noteUnread(r, noteReads, noteReadsBaseline),
  );

  /** The unified ask box: one input, the whole corpus. On the shelf it sits
   *  under the heading and starts a thread; inside the Chat tab it is the
   *  follow-up composer, docked at the bottom under the conversation the way
   *  a notebook's composer sits under its transcript. One markup either way —
   *  only its place, its placeholder, and its controls row differ. */
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
            placeholder={
              chatOpen
                ? "Ask a follow-up…"
                : "Ask or search across all your notebooks…"
            }
            aria-label={
              chatOpen
                ? "Ask a follow-up across all notebooks"
                : "Ask a question across all notebooks"
            }
            className="h-8 min-w-0 flex-1 bg-transparent pl-2.5 pr-1.5 text-body text-foreground outline-none placeholder:text-subtle-foreground"
          />
          {!chatOpen && (
            <button
              type="button"
              onClick={() => useStore.getState().setPaletteOpen(true)}
              title="Search notebooks, sources & notes (⌘K)"
              aria-label="Open search"
              className="flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-caption text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
            >
              <Search className="h-3.5 w-3.5" />
              <kbd className="rounded border border-border bg-surface-2 px-1 py-0.5 text-badge text-subtle-foreground">
                ⌘K
              </kbd>
            </button>
          )}
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
        {/* Style, length, and model — only where the conversation is, since
            they describe the answer being written rather than the shelf. */}
        {chatOpen && (
          <div className="flex items-center gap-1.5 px-1 pt-1.5">
            <HomeChatControls />
          </div>
        )}
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

  // Backend already returns notebooks sorted by most-recently-updated.
  return (
    <div className="app-root flex h-dvh w-screen flex-col overflow-hidden text-foreground">
      <header
        data-tauri-drag-region
        className="flex h-12 items-center gap-2.5 pl-[84px] pr-5"
      >
        <NavButtons />
        <div className="h-4 w-px bg-border" />
        {/* Just the wordmark: you are already at your notebooks, so a second
            books icon here only competed with the Notebooks tab beside it
            (and with the go-home button in the notebook header, which wears
            the same Library glyph). One icon, one meaning. */}
        <span className="text-section font-semibold tracking-tight">
          Alchemy
        </span>
        <div className="mx-2">
          <HomeSectionTabs />
        </div>
        <div className="ml-auto flex items-center gap-3">
          <DevBadge />
          <UpdateBadge />
          <Button
            variant="ghost"
            size="icon"
            onClick={() => useStore.getState().setPaletteOpen(true)}
            title="Search & commands (⌘K)"
            aria-label="Open the command menu"
          >
            <Search className="h-4 w-4" />
          </Button>
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
            <Button
              variant="primary"
              onClick={() => {
                setNewTitle("");
                setCreating(true);
              }}
            >
              <Plus className="h-4 w-4" />
              New notebook
            </Button>
          </AlchemyHero>
        </div>
      ) : (
        <div className="relative flex min-h-0 flex-1">
          {/* The dither shader from the hero, as a banner behind the heading —
          full window width, running behind the sidebar cards, fading into
          the background before the notebook grid starts. */}
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
          {/* Three regions, same side-card idiom as the notebook view:
            Chats + Staff rail left, the section's own center, Brief +
            reports column right. Each sidebar collapses on its own, and
            neither rail depends on which section is on screen. */}
          {chatsOpen || staffOpen ? (
            <div
              ref={leftColRef}
              className="relative mx-2 mb-2 mt-1 hidden shrink-0 flex-col lg:flex"
              style={{ width: staffWidth }}
            >
              {/* Past conversations lead this rail, the way the Brief leads
                  the one opposite: the thread you were in is the way back
                  into the work, and it is reachable from every section — a
                  conversation is not a property of the shelf you happen to
                  be looking at. One handle per stacked card, as on the
                  right: each card's right edge is the column's, so either
                  drags its width. */}
              {chatsOpen ? (
                <HomeThreadsSidebar
                  className={staffOpen ? "shrink-0" : "flex-1"}
                  style={staffOpen ? { height: `${chatsSplit}%` } : undefined}
                  resizeHandle={staffResizeHandle}
                  onCollapse={toggleChats}
                />
              ) : (
                <div className="side-card relative flex w-12 shrink-0 flex-col items-center self-start py-2">
                  <SidebarRail
                    icon="chats"
                    title="Show Chats"
                    onClick={toggleChats}
                  />
                </div>
              )}
              {chatsOpen && staffOpen ? (
                <StackSplit
                  colRef={leftColRef}
                  pct={chatsSplit}
                  defaultPct={45}
                  label="Resize Chats"
                  onChange={(pct) => {
                    setChatsSplit(pct);
                    localStorage.setItem(
                      "homeChatsSplit",
                      String(Math.round(pct)),
                    );
                  }}
                />
              ) : (
                <div className="h-2 shrink-0" />
              )}
              {staffOpen ? (
                <aside className="side-card relative flex min-h-0 flex-1 flex-col">
                  {staffResizeHandle}
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
                    onCollapse={toggleStaff}
                  />
                </aside>
              ) : (
                <div className="side-card relative flex w-12 shrink-0 flex-col items-center self-start py-2">
                  <SidebarRail
                    icon="staff"
                    title="Show Staff"
                    onClick={toggleStaff}
                  />
                </div>
              )}
            </div>
          ) : (
            <div className="side-card relative mx-2 mt-1 hidden w-12 shrink-0 flex-col items-center gap-1 self-start py-2 lg:flex">
              <SidebarRail
                icon="chats"
                title="Show Chats"
                onClick={toggleChats}
              />
              <SidebarRail icon="staff" title="Show Staff" onClick={toggleStaff} />
            </div>
          )}
          <div className="relative flex min-w-0 flex-1 flex-col overflow-hidden">
            {/* Heading + ask box stay put; only the shelves (or the
            conversation) below scroll. */}
            <div
              className={cn(
                "relative z-10 mx-auto w-full shrink-0 px-6",
                // The composer lines up with the conversation it feeds, so
                // the column narrows to the reading measure while one is open.
                chatOpen ? "max-w-[760px] pt-6" : "max-w-[960px] pt-10",
              )}
            >
              {/* The conversation takes the center column: the shelf's
              heading and verbs would only compete with it, and the ask box
              below becomes the follow-up composer. The chat gets no heading
              of its own — the tab already names the place, and the citations
              say where answers come from. */}
              {/* Wraps rather than squeezes: with both sidebars open this
                  column is far narrower than its 960px cap, so the action
                  cluster drops to its own line instead of crushing the
                  heading. */}
              {!chatOpen && (
              <div className="mb-5 flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
                <div className="min-w-[260px] flex-1">
                  <h1 className="text-page font-semibold tracking-tight">
                    {homeSection === "registry"
                      ? "Your registry"
                      : "Your notebooks"}
                  </h1>
                  {homeSection === "registry" ? (
                    <p className="mt-1 text-body text-muted-foreground">
                      The things your documents are about: assets, people,
                      projects.
                    </p>
                  ) : (
                  <p className="mt-1 text-body text-muted-foreground">
                    {stats
                      ? [
                          `${activeNotebooks.length} ${activeNotebooks.length === 1 ? "notebook" : "notebooks"}`,
                          `${stats.sources} ${stats.sources === 1 ? "source" : "sources"}`,
                          stats.notes > 0 &&
                            `${stats.notes} ${stats.notes === 1 ? "note" : "notes"}`,
                          stats.ledger > 0 &&
                            `${stats.ledger} ledger ${stats.ledger === 1 ? "entry" : "entries"}`,
                          `${Intl.NumberFormat().format(stats.chars)} characters indexed`,
                        ]
                          .filter(Boolean)
                          .join(" · ")
                      : "Most recently used first."}
                  </p>
                  )}
                  {homeSection === "notebooks" && (
                    <AwayDigest
                      prevVisit={prevVisit}
                      notebooks={notebooks}
                      reports={reports}
                    />
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  {homeSection === "registry" ? (
                    // The registry's verbs are its own: source/import belong
                    // to notebooks, and the primary action here mints a card.
                    <Button
                      variant="primary"
                      onClick={() => useStore.setState({ registryCreating: true })}
                    >
                      <Plus className="h-4 w-4" />
                      New card
                    </Button>
                  ) : (
                    <>
                      <Button
                        variant="secondary"
                        onClick={() =>
                          useStore.setState({
                            // Empty payload = capture first, then file. Home has
                            // no current notebook, so this is the one add path
                            // that has to pick one — and it suggests which.
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
                        <Plus className="h-4 w-4" />
                        Add source…
                      </Button>
                      <Button
                        variant="secondary"
                        onClick={() => useStore.setState({ importOkfOpen: true })}
                        title="Import a shared .okf.zip or bundle folder"
                      >
                        <FolderInput className="h-4 w-4" />
                        Import…
                      </Button>
                      <Button
                        variant="primary"
                        onClick={() => {
                          setNewTitle("");
                          setCreating(true);
                        }}
                      >
                        <Plus className="h-4 w-4" />
                        New notebook
                      </Button>
                    </>
                  )}
                </div>
              </div>
              )}

              {/* On the shelf the ask box lives here, under the heading:
              Enter asks across every notebook and opens the answer as a
              conversation. In the Chat tab it moves to the bottom of the
              pane, below the conversation it feeds. */}
              {!chatOpen && <div className="mb-8">{askComposer}</div>}
            </div>

            {chatOpen ? (
              // The conversation borrows the shelf's scroll region rather
              // than floating over it: leaving the tab puts the notebooks
              // back exactly where they were.
              <HomeChatThread chat={chat} />
            ) : homeSection === "registry" ? (
              <RegistrySection />
            ) : (
            <div
              ref={shelfRef}
              onPointerDown={marqueeDown}
              className="relative min-h-0 flex-1 select-none overflow-y-auto"
            >
              <div className="mx-auto w-full max-w-[960px] px-6 pb-10">
              <HomeViewControls placeholder="Filter notebooks by title…" />
              {homeView === "table" ? (
                <NotebookTable
                  notebooks={shownNotebooks}
                  onNew={() => {
                    setNewTitle("");
                    setCreating(true);
                  }}
                  unreadByNb={unreadByNb}
                  pickedIds={pick.pickedIds}
                  onRowClick={(e, nb) => {
                    if (justEnded()) return;
                    if (!pick.handleClick(e, nb.id)) open(nb.id);
                  }}
                  onRowOpen={(nb) => open(nb.id)}
                  colorPop={(nb) => colorPalettePop(nb, "left-8 top-8")}
                  rowMenu={(nb) => (
                    <RowMenu
                      label={`Options for ${nb.title}`}
                      contextItems={() =>
                        pick.contextItems(nb.id, notebookBatchItems)
                      }
                      items={notebookRowItems(nb)}
                    />
                  )}
                />
              ) : (
              <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
                {shownNotebooks.map((nb) => (
                  <div
                    key={nb.id}
                    data-pick-id={nb.id}
                    // Card content is pointer-events-none, so the hover
                    // tooltip for the truncated title lives on the card.
                    title={nb.title}
                    className={cn(
                      "group relative flex min-h-[132px] cursor-pointer flex-col rounded-lg border border-border bg-surface p-4 transition-colors hover:border-border-strong hover:bg-surface-2",
                      "has-[[aria-expanded=true]]:z-30",
                      pick.pickedIds.has(nb.id) &&
                        "bg-primary/10 hover:bg-primary/15",
                    )}
                  >
                    <CardAction
                      label={`Open notebook ${nb.title}`}
                      onClick={(e) => {
                        if (justEnded()) return;
                        if (!pick.handleClick(e, nb.id)) open(nb.id);
                      }}
                    />
                    <div
                      className="pointer-events-none relative z-10 mb-auto flex h-8 w-8 items-center justify-center rounded-lg"
                      style={{
                        backgroundColor: `color-mix(in srgb, ${nb.color || NOTEBOOK_PALETTE[0]} 16%, transparent)`,
                        color: nb.color || NOTEBOOK_PALETTE[0],
                      }}
                    >
                      {(() => {
                        const Icon = notebookIcon(nb.icon);
                        return <Icon className="h-4 w-4" />;
                      })()}
                    </div>
                    <div className="pointer-events-none relative z-10 mt-3 flex items-center gap-1.5">
                      <span
                        className="truncate text-card font-medium"
                        title={nb.title}
                      >
                        {nb.title}
                      </span>
                      {(unreadByNb.get(nb.id) ?? 0) > 0 && (
                        <span
                          className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                          title={`${unreadByNb.get(nb.id)} unread ${unreadByNb.get(nb.id) === 1 ? "report" : "reports"}`}
                          aria-label={`${unreadByNb.get(nb.id)} unread reports`}
                        />
                      )}
                    </div>
                    <div className="pointer-events-none relative z-10 mt-1 flex items-center gap-1.5 text-micro text-subtle-foreground">
                      <Badge className="gap-1">
                        <FileText className="h-2.5 w-2.5" />
                        {nb.sourceCount}
                      </Badge>
                      <span>·</span>
                      <span>{relativeTime(nb.updatedAt)}</span>
                    </div>

                    <div className="absolute right-2 top-2 z-20 flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
                      <button
                        type="button"
                        className="rounded p-1 text-muted-foreground transition hover:bg-elevated"
                        style={{
                          backgroundColor: nb.color || NOTEBOOK_PALETTE[0],
                        }}
                        onClick={(e) => {
                          e.stopPropagation();
                          setColorPickerFor((cur) =>
                            cur === nb.id ? null : nb.id,
                          );
                        }}
                        onPointerDown={(e) => e.stopPropagation()}
                        data-notebook-color-trigger
                        aria-expanded={colorPickerFor === nb.id}
                        aria-label={`Change color for ${nb.title}`}
                        title="Change notebook color"
                      >
                        <span className="relative block h-3 w-3 rounded-full border border-background" />
                      </button>
                      <RowMenu
                        label={`Options for ${nb.title}`}
                        contextItems={() =>
                          pick.contextItems(nb.id, notebookBatchItems)
                        }
                        items={notebookRowItems(nb)}
                      />
                    </div>
                    {colorPalettePop(nb, "right-2 top-10")}
                  </div>
                ))}
              </div>
              )}
              {shownNotebooks.length === 0 && activeNotebooks.length > 0 && (
                <p className="py-8 text-center text-body text-muted-foreground">
                  No notebook matches &ldquo;{homeQuery.trim()}&rdquo;.
                </p>
              )}

              {/* Archived notebooks: collapsed row list, data intact. */}
              {archivedNotebooks.length > 0 && (
                <div className="mt-8">
                  <button
                    type="button"
                    onClick={() => setArchivedOpen((v) => !v)}
                    aria-expanded={archivedOpen}
                    className="flex cursor-pointer items-center gap-1 text-micro font-medium uppercase tracking-wide text-subtle-foreground transition-colors hover:text-muted-foreground"
                  >
                    <ChevronRight
                      className={cn(
                        "h-3 w-3 transition-transform",
                        archivedOpen && "rotate-90",
                      )}
                    />
                    Archived · {archivedNotebooks.length}
                  </button>
                  {archivedOpen && (
                    <div className="mt-2 flex flex-col gap-1">
                      {archivedNotebooks.map((nb) => (
                        <div
                          key={nb.id}
                          className="group flex items-center gap-2.5 rounded-md border border-border bg-surface px-3 py-2 transition-colors hover:border-border-strong hover:bg-surface-2"
                        >
                          <Archive className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          <span className="truncate text-body text-foreground">
                            {nb.title}
                          </span>
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
                  )}
                </div>
              )}

              {/* Recent notes live in the Staff sidebar now — the center
                  column is just the notebook shelves. */}
              </div>
            </div>
            )}

            {/* The follow-up composer, docked under the conversation the way
            a notebook's is: the thread scrolls, this stays. */}
            {chatOpen && (
              <div className="relative z-10 w-full shrink-0 px-6 pb-5 pt-2">
                <div className="mx-auto w-full max-w-[760px]">
                  {askComposer}
                </div>
              </div>
            )}
          </div>

          {/* Right column: the Brief card above the reports feed — the
            morning-read surface, arrival point first. */}
          {!briefOpen && !reportsOpen ? (
            <div className="side-card relative mx-2 mt-1 hidden w-12 shrink-0 flex-col items-center gap-1 self-start py-2 lg:flex">
              <SidebarRail
                icon="brief"
                title="Show the brief"
                dot={briefUnread}
                onClick={toggleBrief}
              />
              <SidebarRail
                icon="reports"
                title="Show latest reports"
                dot={totalUnread > 0}
                onClick={toggleReports}
              />
            </div>
          ) : (
            <div
              ref={rightColRef}
              className="relative mx-2 mb-2 mt-1 hidden shrink-0 flex-col lg:flex"
              style={{ width: rightWidth }}
            >
              {/* One handle per stacked card (a column-spanning handle floats
                  over the gap between the cards' rounded corners); both drag
                  the whole column's width. The card's left edge IS the
                  column's, so the parent-rect math is unchanged. */}
              {briefOpen ? (
                <BriefSidebar
                  onCollapse={toggleBrief}
                  briefs={briefNotes}
                  schedules={allReports}
                  unread={briefUnread}
                  onRan={refreshActivity}
                  resizeHandle={rightResizeHandle}
                  className={reportsOpen ? "shrink-0" : "min-h-0 flex-1"}
                  style={
                    reportsOpen ? { height: `${briefSplit}%` } : undefined
                  }
                />
              ) : (
                // Collapsed to the single-icon rail, hugging the column's
                // outer edge — the mirror of Staff and Chats on the left.
                <div className="side-card relative flex w-12 shrink-0 flex-col items-center self-end py-2">
                  <SidebarRail
                    icon="brief"
                    title="Show the brief"
                    dot={briefUnread}
                    onClick={toggleBrief}
                  />
                </div>
              )}
              {briefOpen && reportsOpen ? (
                <StackSplit
                  colRef={rightColRef}
                  pct={briefSplit}
                  defaultPct={40}
                  label="Resize the brief"
                  onChange={(pct) => {
                    setBriefSplit(pct);
                    localStorage.setItem(
                      "homeBriefSplit",
                      String(Math.round(pct)),
                    );
                  }}
                />
              ) : (
                <div className="h-2 shrink-0" />
              )}
              {reportsOpen ? (
                <aside className="side-card relative flex min-h-0 flex-1 flex-col">
                  {rightResizeHandle}
                  {feedReports.length > 0 ? (
                    <ReportsFeed
                      onCollapse={toggleReports}
                      reports={feedReports}
                      notebookTitle={notebookTitle}
                      notebookColor={notebookColor}
                      fallbackColor={NOTEBOOK_PALETTE[0]}
                      onOpen={openNote}
                    />
                  ) : (
                    // Empty and loading states carry their own header:
                    // ReportsFeed owns the collapse control, so without one
                    // here an empty feed can never be closed again.
                    <>
                      <div className="flex min-h-12 shrink-0 items-center gap-2 border-b border-border px-6 py-2">
                        <span className="whitespace-nowrap text-caption font-semibold uppercase tracking-wide text-muted-foreground">
                          Latest reports
                        </span>
                        <button
                          type="button"
                          onClick={toggleReports}
                          title="Collapse reports"
                          aria-label="Collapse the reports feed"
                          aria-expanded
                          className="ml-auto rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
                        >
                          <PanelRightClose className="h-4 w-4" />
                        </button>
                      </div>
                      {activityLoading ? (
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
                              activityError
                                ? "Reports unavailable"
                                : "Reports appear here"
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
                    </>
                  )}
                </aside>
              ) : (
                <div className="side-card relative flex w-12 shrink-0 flex-col items-center self-end py-2">
                  <SidebarRail
                    icon="reports"
                    title="Show latest reports"
                    dot={totalUnread > 0}
                    onClick={toggleReports}
                  />
                </div>
              )}
            </div>
          )}
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
            create(newTitle);
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

      <Modal
        open={!!renaming}
        onClose={() => setRenaming(null)}
        title="Edit notebook"
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (renaming) {
              const r = renaming;
              const before = notebooks.find((n) => n.id === r.id);
              // Icon first, sequenced: rename() ends with a full refresh,
              // and firing both unordered let that refresh read the DB
              // before the icon write landed — reverting the optimistic
              // icon until some later refresh ("shows up two edits later").
              void (async () => {
                if (before && before.icon !== r.icon)
                  await useStore.getState().setNotebookIcon(r.id, r.icon);
                if (before && before.title !== r.title)
                  await rename(r.id, r.title);
              })();
            }
            setRenaming(null);
          }}
          className={cn("flex flex-col gap-3")}
        >
          <Input
            autoFocus
            name="notebook-title"
            aria-label="Notebook title"
            value={renaming?.title ?? ""}
            onChange={(e) =>
              setRenaming((r) => (r ? { ...r, title: e.target.value } : r))
            }
          />
          {/* Icon picker: the auto-picked icon can always be overridden
              here; the plain book is a first-class choice, not an absence. */}
          <div className="grid grid-cols-8 gap-1">
            {["", ...Object.keys(NOTEBOOK_ICONS).filter((k) => k !== "book-open")].map(
              (name) => {
                const Icon = notebookIcon(name);
                const active = (renaming?.icon ?? "") === name;
                return (
                  <button
                    key={name || "default"}
                    type="button"
                    aria-pressed={active}
                    aria-label={name ? `Icon: ${name.replace(/-/g, " ")}` : "Default icon"}
                    title={name ? name.replace(/-/g, " ") : "Default"}
                    onClick={() =>
                      setRenaming((r) => (r ? { ...r, icon: name } : r))
                    }
                    className={cn(
                      "flex h-8 items-center justify-center rounded-md border transition-colors",
                      active
                        ? "border-primary/60 bg-primary/10 text-foreground"
                        : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
                    )}
                  >
                    <Icon className="h-4 w-4" />
                  </button>
                );
              },
            )}
          </div>
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setRenaming(null)}
            >
              Cancel
            </Button>
            <Button type="submit" variant="primary">
              Save
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}
