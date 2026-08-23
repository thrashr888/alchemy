/* The Registry (docs/RFC-registry.md): Home's other center column — the
   confirmed cast of things, and the documents filed under them.

   Cards are corpus-scoped, so this lives on Home rather than in a notebook's
   center-mode switch. The grid is the source gallery's grid re-aimed: kind
   groups where the gallery has type groups, notebook chips where it has tag
   chips, one shared FilterBar. Every attachment shows its receipt — the
   identifier that matched, "name", or "manual" — because a machine that
   files without showing its reason is one you stop trusting on the first
   mistake. */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { usePickList } from "@/lib/pick";
import type {
  CardAttachment,
  CardFact,
  CardKind,
  RegistryCard,
  Source,
} from "@/lib/types";
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
} from "./ui";
import { FilterBar, rankByCount } from "./FilterBar";
import {
  HomeTable,
  HomeViewControls,
  matchesHomeQuery,
} from "./HomeViewControls";
import { cn, relativeTime } from "@/lib/utils";
import {
  ArrowLeft,
  Boxes,
  Building2,
  Check,
  FileText,
  FolderKanban,
  Package,
  Plus,
  ScrollText,
  Sparkles,
  Trash2,
  User,
  X,
} from "lucide-react";

/** The one card-delete path (DESIGN.md §9: undo beats confirm). Deletes
 *  immediately; the toast's undo recreates each card — identifiers, note,
 *  facts — and re-files its attachments. Ruling metadata (origin/triage)
 *  doesn't survive, which only matters for suggested cards, and those are
 *  dismissed rather than deleted. */
async function deleteCardsUndoable(cards: RegistryCard[], after: () => void) {
  if (cards.length === 0) return;
  for (const c of cards) await api.deleteRegistryCard(c.id);
  after();
  const label =
    cards.length === 1
      ? `Deleted “${cards[0].name}” — click to undo`
      : `Deleted ${cards.length} cards — click to undo`;
  useStore.getState().pushToast("success", label, () =>
    void (async () => {
      try {
        for (const c of cards) {
          const restored = await api.addRegistryCard(
            c.kind,
            c.name,
            c.identifiers,
            c.note,
            c.facts,
          );
          for (const a of c.attachments) {
            await api.attachSourceToCard(restored.id, a.sourceId, a.status);
          }
        }
      } catch (e) {
        useStore
          .getState()
          .pushToast("error", e instanceof Error ? e.message : String(e));
      }
      after();
    })(),
  );
}

const KINDS: { id: CardKind; label: string; icon: React.ReactNode }[] = [
  { id: "asset", label: "Assets", icon: <Package className="h-4 w-4" /> },
  { id: "person", label: "People", icon: <User className="h-4 w-4" /> },
  {
    id: "policy",
    label: "Policies",
    icon: <ScrollText className="h-4 w-4" />,
  },
  {
    id: "provider",
    label: "Providers",
    icon: <Building2 className="h-4 w-4" />,
  },
  {
    id: "project",
    label: "Projects",
    icon: <FolderKanban className="h-4 w-4" />,
  },
  {
    id: "dependency",
    label: "Dependencies",
    icon: <Boxes className="h-4 w-4" />,
  },
];

export function kindIcon(kind: string) {
  return KINDS.find((k) => k.id === kind)?.icon ?? <Package className="h-4 w-4" />;
}

/** Singular label for one card ("Asset"), vs the plural filter groups. */
function kindLabel(kind: string) {
  const l = KINDS.find((k) => k.id === kind)?.label ?? kind;
  return l.endsWith("ies") ? l.slice(0, -3) + "y" : l.replace(/s$/, "");
}

const confirmed = (c: RegistryCard) =>
  c.attachments.filter((a) => a.status === "confirmed");
const proposed = (c: RegistryCard) =>
  c.attachments.filter((a) => a.status === "proposed");

/** A user-owned card with no standing documents. The sweep prunes rows
 *  whose sources were deleted, so "no confirmed or proposed rows" IS the
 *  orphan test client-side. Badged, never auto-removed — a card the user
 *  kept vanishing on its own would break the shows-its-reason trust model
 *  (auto-origin orphans are the sweep's to remove, and never reach `mine`). */
const isOrphan = (c: RegistryCard) =>
  !c.origin && confirmed(c).length === 0 && proposed(c).length === 0;

const ORPHAN_HINT =
  "No documents left; its sources may have been deleted with their notebook. " +
  "Alchemy retried the match and found nothing.";

type RegistrySort = "latest" | "docs" | "title";
const SORTS: { value: RegistrySort; label: string }[] = [
  { value: "latest", label: "Latest" },
  { value: "docs", label: "Documents" },
  { value: "title", label: "A–Z" },
];

function sortCards(cards: RegistryCard[], sort: RegistrySort): RegistryCard[] {
  const out = [...cards];
  switch (sort) {
    case "docs":
      out.sort(
        (a, b) =>
          confirmed(b).length - confirmed(a).length ||
          a.name.localeCompare(b.name),
      );
      break;
    case "title":
      out.sort((a, b) => a.name.localeCompare(b.name));
      break;
    default:
      out.sort(
        (a, b) =>
          Math.max(b.updatedAt, b.createdAt) - Math.max(a.updatedAt, a.createdAt),
      );
  }
  return out;
}

/** How an attachment got here, in the user's words. The identifier case
    quotes the actual string — that's the whole point of the receipt. */
function receipt(a: CardAttachment) {
  if (a.matched === "manual") return "filed by hand";
  if (a.matched === "name") return "name matched";
  return `matched ${a.matched}`;
}

export function RegistrySection() {
  const registryBump = useStore((s) => s.registryBump);
  const openCardId = useStore((s) => s.openCardId);
  const view = useStore((s) => s.homeView);
  const query = useStore((s) => s.homeQuery);
  const [cards, setCards] = useState<RegistryCard[]>([]);
  const [kind, setKind] = useState<string>("all");
  const [notebook, setNotebook] = useState<string | null>(null);
  // Store-held so the Home hero's "New card" button opens the same modal.
  const creating = useStore((s) => s.registryCreating);
  const setCreating = (open: boolean) =>
    useStore.setState({ registryCreating: open });
  const [loaded, setLoaded] = useState(false);
  const [suggesting, setSuggesting] = useState(false);
  // Sort order, remembered like homeView.
  const [sort, setSort] = useState<RegistrySort>(
    () => (localStorage.getItem("registrySort") as RegistrySort) || "latest",
  );
  const changeSort = (v: string) => {
    localStorage.setItem("registrySort", v);
    setSort(v as RegistrySort);
  };
  const { confirm, dialog: confirmDialog } = useConfirm();

  const load = useCallback(async () => {
    try {
      setCards(await api.listRegistry());
    } finally {
      setLoaded(true);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load, registryBump]);

  const notebooks = useStore((s) => s.notebooks);
  const nbTitle = useCallback(
    (id: string) => notebooks.find((n) => n.id === id)?.title ?? "",
    [notebooks],
  );

  // Suggested cards are proposals, not cast members: they get their own
  // group above the grid, and dismissed ones never render at all (the row
  // survives only as the suggester's refusal memory).
  const suggested = useMemo(
    () => cards.filter((c) => c.origin === "auto"),
    [cards],
  );
  const mine = useMemo(() => cards.filter((c) => !c.origin), [cards]);

  // Both axes are computed from what's actually present, so an empty option
  // never renders (the gallery's rule).
  const kinds = useMemo(() => {
    const present = new Set(mine.map((c) => c.kind));
    return [
      { value: "all", label: "All" },
      ...KINDS.filter((k) => present.has(k.id)).map((k) => ({
        value: k.id as string,
        label: k.label,
      })),
    ];
  }, [mine]);

  const nbChips = useMemo(() => {
    const counts = new Map<string, number>();
    for (const c of mine)
      for (const a of c.attachments) {
        if (a.status === "rejected" || !a.notebookId) continue;
        const t = nbTitle(a.notebookId);
        if (t) counts.set(t, (counts.get(t) ?? 0) + 1);
      }
    return rankByCount(counts);
  }, [mine, nbTitle]);

  const shown = useMemo(
    () =>
      sortCards(
        mine.filter((c) => {
          if (!matchesHomeQuery(query, c.name, c.identifiers)) return false;
          if (kind !== "all" && c.kind !== kind) return false;
          if (notebook !== null) {
            const inNb = c.attachments.some(
              (a) =>
                a.status !== "rejected" && nbTitle(a.notebookId) === notebook,
            );
            if (!inNb) return false;
          }
          return true;
        }),
        sort,
      ),
    [mine, kind, notebook, nbTitle, query, sort],
  );

  // ---- Index selection (docs/RFC-multi-select.md) ----------------------
  const pick = usePickList("cards", shown.map((c) => c.id));
  const indexRef = useRef<HTMLDivElement>(null);
  const indexBase = useRef<string[]>([]);
  const {
    onPointerDown: indexMarqueeDown,
    marquee: indexMarquee,
    justEnded,
  } = useMarquee({
    containerRef: indexRef,
    onStart: (additive) => {
      const p = useStore.getState().picked;
      indexBase.current = additive && p?.kind === "cards" ? p.ids : [];
    },
    onSelect: (ids) =>
      pick.pickSet("cards", [...new Set([...indexBase.current, ...ids])], false),
    onClearBackground: pick.clearPicked,
  });

  /** Deleting cards never touches the documents filed under them, so the
   *  confirm says so — and names every card, since a count can't be
   *  checked against. */
  const cardBatchItems = (ids: string[]): RowMenuItem[] => [
    {
      label: `Delete ${ids.length} cards…`,
      icon: <Trash2 className="h-3.5 w-3.5" />,
      danger: true,
      onClick: () =>
        void (async () => {
          const cards = mine.filter((c) => ids.includes(c.id));
          await deleteCardsUndoable(cards, () => {
            useStore.getState().clearPicked();
            void load();
          });
        })(),
    },
  ];

  // User-owned orphans (see isOrphan): badged in the list, removed only by
  // this explicit bulk action — one confirm, then they go together.
  const orphans = useMemo(() => mine.filter(isOrphan), [mine]);
  const cleanUpOrphans = async () => {
    const ok = await confirm({
      title: `Remove ${orphans.length} orphaned card${orphans.length === 1 ? "" : "s"}?`,
      message:
        "These cards have no documents left — their sources were deleted and " +
        "rematching found nothing. Identifiers and facts on them go too.",
      // Named, not just counted: "4 orphaned cards" is impossible to check
      // against, and this is the one screen where the user can still say no.
      items: orphans.map((c) => `${c.name} \u00b7 ${kindLabel(c.kind)}`),
      confirmLabel: "Remove",
      danger: true,
    });
    if (!ok) return;
    for (const c of orphans) await api.deleteRegistryCard(c.id);
    void load();
  };

  // The explicit ask (RFC-registry §3): read every notebook, propose, and
  // let the background triage sort what lands. The strip refreshes itself
  // on the registry bump; the toast is the receipt for "nothing new".
  const suggestNow = async () => {
    if (suggesting) return;
    setSuggesting(true);
    try {
      const out = await api.suggestCardsNow();
      useStore
        .getState()
        .pushToast(
          out.created.length > 0 ? "success" : "info",
          out.alreadyRunning
            ? "A suggest pass is already running"
            : out.created.length > 0
              ? `Suggested ${out.created.length} card${out.created.length === 1 ? "" : "s"}`
              : "Nothing new to suggest",
        );
      void load();
    } catch (e) {
      useStore.getState().pushToast("error", String(e));
    } finally {
      setSuggesting(false);
    }
  };

  const open = cards.find((c) => c.id === openCardId) ?? null;
  if (openCardId && open) {
    return (
      <>
        <CardDetail
          card={open}
          onBack={() => useStore.setState({ openCardId: null })}
          onChanged={load}
        />
        {/* The header's "New card" button lives in HomeView and flips a
            store flag; this branch used to return before the modal that
            listens for it, so on a detail page the button did nothing. */}
        <NewCardModal
          open={creating}
          onClose={() => setCreating(false)}
          onCreated={(c) => {
            void load();
            useStore.setState({ openCardId: c.id });
          }}
        />
      </>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* relative z-10, like the notebook shelf's scroller: Home paints a
          256px-tall dither banner as an ABSOLUTE sibling behind the heading,
          and without a stacking context of its own this column composites
          under it. Only transparent controls lost that fight — the filter
          input has an opaque background, the grid/table toggle is a hairline
          border — which is why the toggle alone went invisible, and only
          when a collapsed sidebar shortened the header enough to slide the
          row up into the banner. */}
      <div
        ref={indexRef}
        onPointerDown={indexMarqueeDown}
        className="relative z-10 min-h-0 flex-1 select-none overflow-y-auto"
      >
        <div className="mx-auto w-full max-w-[960px] px-6 pb-10">
          <SuggestionStrip cards={suggested} onChanged={load} />
          {/* Unconditional, like the notebook shelf's: gating this on
              "are there confirmed cards" made the whole row — filter AND
              view toggle — vanish whenever the cast was empty or held only
              suggestions, which reads as the control disappearing. */}
          <HomeViewControls
            placeholder="Filter cards by name or identifier…"
            sort={{ value: sort, options: SORTS, onChange: changeSort }}
            trailing={
              <>
                {orphans.length > 0 && (
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => void cleanUpOrphans()}
                    title={ORPHAN_HINT}
                  >
                    Clean up orphans ({orphans.length})
                  </Button>
                )}
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => void suggestNow()}
                  loading={suggesting}
                  title="Read your notebooks and suggest cards worth tracking"
                >
                  <Sparkles className="h-3.5 w-3.5" />
                  Suggest
                </Button>
              </>
            }
          />
          {/* Kind groups and notebook chips sit inside the content column,
              under the controls — full-bleed they drew a band across the
              pane with dead clickable space beside them. */}
          <FilterBar
            bare
            groups={kinds}
            group={kinds.some((k) => k.value === kind) ? kind : "all"}
            onGroup={setKind}
            chips={nbChips}
            chip={
              notebook !== null && nbChips.includes(notebook) ? notebook : null
            }
            onChip={setNotebook}
            chipAllLabel="All notebooks"
            chipPrefix=""
          />
          {loaded && mine.length === 0 ? (
            <EmptyState
              icon={<Package className="h-5 w-5" />}
              title="No cards yet"
              hint="A card is a thing your documents are about — a vehicle, a policy, a project. Give it an identifier like a VIN or policy number and matching documents file themselves."
            >
              <Button
                variant="primary"
                className="mt-3"
                onClick={() => setCreating(true)}
              >
                <Plus className="h-4 w-4" />
                New card
              </Button>
            </EmptyState>
          ) : view === "table" ? (
            <CardTable
              cards={shown}
              nbTitle={nbTitle}
              onChanged={load}
              pickedIds={pick.pickedIds}
              onRowClick={(e, id) => {
                if (justEnded()) return;
                if (!pick.handleClick(e, id))
                  useStore.setState({ openCardId: id });
              }}
              onContextItems={(id) => () =>
                pick.contextItems(id, cardBatchItems)}
            />
          ) : (
            <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
              {shown.map((c) => (
                <CardTile
                  key={c.id}
                  card={c}
                  onOpen={() => useStore.setState({ openCardId: c.id })}
                  onChanged={load}
                  picked={pick.pickedIds}
                  onActivate={(e) => {
                    if (justEnded()) return true;
                    return pick.handleClick(e, c.id) || undefined;
                  }}
                  onContextItems={() => pick.contextItems(c.id, cardBatchItems)}
                />
              ))}
            </div>
          )}
          {shown.length === 0 && mine.length > 0 && (
            <EmptyState
              compact
              title={`No card matches \u201c${query.trim()}\u201d`}
              hint="The filter looks at names and identifiers."
            />
          )}
        </div>
      </div>
      {indexMarquee}
      <NewCardModal
        open={creating}
        onClose={() => setCreating(false)}
        onCreated={(c) => {
          void load();
          useStore.setState({ openCardId: c.id });
        }}
      />
      {confirmDialog}
    </div>
  );
}

/** The "Filed under" row in the reader's document-properties grid.
 *  The rail (CardRail) is the working surface; this is the *fact* — what
 *  this document belongs to, sitting with Type/Added/Tags where a reader
 *  looks for a document's identity. Renders nothing when the document is
 *  filed nowhere, like every other optional row in that grid. */
export function CardMetaRow({ sourceId }: { sourceId: string }) {
  const registryBump = useStore((s) => s.registryBump);
  const [cards, setCards] = useState<RegistryCard[]>([]);

  useEffect(() => {
    if (!sourceId) return;
    let alive = true;
    void api.cardsForSource(sourceId).then((c) => alive && setCards(c));
    return () => {
      alive = false;
    };
  }, [sourceId, registryBump]);

  if (cards.length === 0) return null;
  return (
    <>
      <span className="text-subtle-foreground">Filed under</span>
      <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
        {cards.map((c) => {
          const a = c.attachments.find((x) => x.sourceId === sourceId)!;
          return (
            <button
              key={c.id}
              onClick={() => {
                useStore.getState().closeNotebook();
                useStore.setState({
                  homeSection: "registry",
                  openCardId: c.id,
                });
              }}
              title={`${kindLabel(c.kind)} \u00b7 ${receipt(a)}`}
              className="inline-flex items-center gap-1 text-foreground transition-colors hover:text-primary"
            >
              <span className="text-muted-foreground">{kindIcon(c.kind)}</span>
              {c.name}
              {a.status === "proposed" && (
                <span className="rounded border border-border px-1 text-micro text-muted-foreground">
                  proposed
                </span>
              )}
            </button>
          );
        })}
      </span>
    </>
  );
}

/** The reader's right-rail Registry panel (RFC-registry §4): what this
    document is filed under, and any proposal waiting on a verdict.

    Proposals resolve here rather than only on Home because this is where
    the evidence is on screen — the document itself is the argument. Rides
    the Related rail's existing docked/popover fit logic; it is a sibling in
    that column, not a third rail with its own toggle. */
export function CardRail({ sourceId }: { sourceId: string }) {
  const registryBump = useStore((s) => s.registryBump);
  const [cards, setCards] = useState<RegistryCard[]>([]);

  const load = useCallback(() => {
    if (!sourceId) return;
    void api.cardsForSource(sourceId).then(setCards);
  }, [sourceId]);
  useEffect(load, [load, registryBump]);

  if (cards.length === 0) return null;

  const setStatus = async (cardId: string, status: string) => {
    await api.setAttachmentStatus(cardId, sourceId, status);
    load();
  };

  return (
    <div className="mb-4 shrink-0">
      <div className="mb-1.5 flex items-center gap-1.5 text-badge font-medium uppercase tracking-wider text-subtle-foreground">
        <Package className="h-3 w-3" />
        Filed under
      </div>
      <div className="flex flex-col gap-1.5">
        {cards.map((c) => {
          const a = c.attachments.find((x) => x.sourceId === sourceId)!;
          const pending = a.status === "proposed";
          return (
            <div
              key={c.id}
              className="rounded-md border border-border p-2 transition-colors hover:border-border-strong"
            >
              <button
                className="flex w-full items-center gap-1.5 text-left"
                onClick={() => {
                  // closeNotebook, not a raw currentId:null — it also clears
                  // sources/messages/reader and resets the window title.
                  useStore.getState().closeNotebook();
                  useStore.setState({
                    homeSection: "registry",
                    openCardId: c.id,
                  });
                }}
              >
                <span className="text-muted-foreground">{kindIcon(c.kind)}</span>
                <span className="min-w-0 flex-1 truncate text-caption font-medium">
                  {c.name}
                </span>
              </button>
              {c.facts.slice(0, 3).map((f, i) => (
                <div
                  key={i}
                  className="mt-1 flex gap-2 text-micro text-muted-foreground"
                >
                  <span className="text-subtle-foreground">{f.label}</span>
                  <span className="min-w-0 flex-1 truncate">{f.value}</span>
                </div>
              ))}
              <div className="mt-1.5 text-micro text-subtle-foreground">
                {receipt(a)}
              </div>
              {pending && (
                <>
                  <div className="mt-1.5 text-micro text-muted-foreground">
                    Filing changes nothing in the document.
                  </div>
                  <div className="mt-1.5 flex items-center gap-1">
                    <Button
                      size="sm"
                      onClick={() => void setStatus(c.id, "confirmed")}
                    >
                      <Check className="h-3 w-3" />
                      Confirm
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => void setStatus(c.id, "rejected")}
                      title="Won't be suggested again"
                    >
                      Not this
                    </Button>
                  </div>
                </>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** File one document under a card from wherever the document is — the
    gallery, the sources list, the reader. This is the fastest path and the
    one that seeds the cast, so it can also mint the card it files into. */
export function AttachToCardModal({
  sourceId,
  sourceTitle,
  onClose,
}: {
  sourceId: string | null;
  sourceTitle: string;
  onClose: () => void;
}) {
  const [cards, setCards] = useState<RegistryCard[]>([]);
  const [q, setQ] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!sourceId) return;
    setQ("");
    void api.listRegistry().then(setCards);
  }, [sourceId]);

  const attach = async (cardId: string) => {
    if (!sourceId || busy) return;
    setBusy(true);
    try {
      const card = await api.attachSourceToCard(cardId, sourceId);
      useStore.getState().pushToast("info", `Filed under “${card.name}”`);
      onClose();
    } finally {
      setBusy(false);
    }
  };

  const create = async () => {
    if (!sourceId || !q.trim() || busy) return;
    setBusy(true);
    try {
      const card = await api.addRegistryCard("asset", q.trim());
      await api.attachSourceToCard(card.id, sourceId);
      onClose();
    } finally {
      setBusy(false);
    }
  };

  const shown = cards.filter((c) =>
    c.name.toLowerCase().includes(q.trim().toLowerCase()),
  );

  return (
    <Modal
      open={!!sourceId}
      onClose={onClose}
      title="File under a card"
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-3">
        <p className="text-caption text-muted-foreground">
          Groups &ldquo;{sourceTitle}&rdquo; under a card. Filing changes
          nothing in the document.
        </p>
        <Input
          autoFocus
          placeholder="Find or name a card…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <div className="max-h-64 overflow-y-auto">
          {shown.map((c) => (
            <button
              key={c.id}
              onClick={() => void attach(c.id)}
              className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-surface-2"
            >
              <span className="text-muted-foreground">{kindIcon(c.kind)}</span>
              <span className="min-w-0 flex-1 truncate text-body">{c.name}</span>
              <span className="shrink-0 text-micro text-subtle-foreground">
                {kindLabel(c.kind)}
              </span>
            </button>
          ))}
          {shown.length === 0 && q.trim() && (
            <button
              onClick={() => void create()}
              className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-surface-2"
            >
              <Plus className="h-4 w-4 text-muted-foreground" />
              <span className="text-body">
                New asset card &ldquo;{q.trim()}&rdquo;
              </span>
            </button>
          )}
          {shown.length === 0 && !q.trim() && (
            <EmptyState
              compact
              title="No cards yet"
              hint="Type a name above to create one."
            />
          )}
        </div>
      </div>
    </Modal>
  );
}

/** The scannable form of the cast: one row per card, columns you can read
    down. Same data, same click target — only the shape differs. */
function CardTable({
  cards,
  nbTitle,
  onChanged,
  pickedIds,
  onRowClick,
  onContextItems,
}: {
  cards: RegistryCard[];
  nbTitle: (id: string) => string;
  onChanged: () => void;
  /** Index selection, shared with the grid so switching view keeps it. */
  pickedIds: Set<string>;
  onRowClick: (e: React.MouseEvent, id: string) => void;
  onContextItems: (id: string) => () => RowMenuItem[] | null;
}) {
  return (
    <>
      {/* No "New card" button here — the header's covers both views. */}
      <HomeTable
        columns={[
          { key: "name", label: "Name" },
          { key: "kind", label: "Kind" },
          { key: "docs", label: "Documents", className: "text-right" },
          { key: "nb", label: "Notebooks" },
          { key: "id", label: "Identifiers" },
          { key: "menu", label: "" },
        ]}
      >
        {cards.map((c) => {
          const notebooks = [
            ...new Set(
              c.attachments
                .filter((a) => a.status !== "rejected")
                .map((a) => nbTitle(a.notebookId))
                .filter(Boolean),
            ),
          ];
          const pending = proposed(c).length;
          return (
            <tr
              key={c.id}
              data-pick-id={c.id}
              onClick={(e) => onRowClick(e, c.id)}
              className={cn(
                "group cursor-pointer border-b border-border transition-colors last:border-b-0 hover:bg-surface-2",
                pickedIds.has(c.id) && "bg-primary/10 hover:bg-primary/15",
              )}
            >
              <td className="px-3 py-2">
                <span className="flex items-center gap-2">
                  <span className="text-muted-foreground">
                    {kindIcon(c.kind)}
                  </span>
                  <span className="truncate font-medium">{c.name}</span>
                  {pending > 0 && (
                    <span
                      className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                      title={`${pending} waiting`}
                    />
                  )}
                  {isOrphan(c) && (
                    <span
                      className="shrink-0 rounded border border-border px-1 text-micro text-subtle-foreground"
                      title={ORPHAN_HINT}
                    >
                      orphaned
                    </span>
                  )}
                </span>
              </td>
              <td className="px-3 py-2 text-caption text-muted-foreground">
                {kindLabel(c.kind)}
              </td>
              <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
                {confirmed(c).length}
              </td>
              <td className="px-3 py-2 text-caption text-muted-foreground">
                {notebooks.join(", ")}
              </td>
              <td className="px-3 py-2 text-caption text-subtle-foreground">
                {c.identifiers}
              </td>
              <td className="w-8 px-2 py-2" onClick={(e) => e.stopPropagation()}>
                <RowMenu
                  contextItems={onContextItems(c.id)}
                  items={[
                    {
                      label: "Open",
                      onClick: () => useStore.setState({ openCardId: c.id }),
                    },
                    {
                      label: "Delete card",
                      danger: true,
                      onClick: () => void deleteCardsUndoable([c], onChanged),
                    },
                  ]}
                />
              </td>
            </tr>
          );
        })}
      </HomeTable>
    </>
  );
}

/** What the suggester proposed, awaiting your verdict.
    Above the grid rather than in it: these are guesses, and mixing guesses
    into the cast is exactly how a registry stops being trustworthy.
    Confirming makes a card yours; dismissing is remembered, so the same
    guess never comes back. */
function SuggestionStrip({
  cards,
  onChanged,
}: {
  cards: RegistryCard[];
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  if (cards.length === 0) return null;

  // Recommended first — the triage pass exists so the ones worth keeping
  // are the first ones you read.
  const ordered = [...cards].sort(
    (a, b) =>
      Number(b.triage === "recommended") - Number(a.triage === "recommended"),
  );
  const recommended = cards.filter((c) => c.triage === "recommended").length;

  // Dismissing is sticky ("won't be suggested again"), so the toast carries
  // the undo: put the dismissed cards back in the queue as suggestions. The
  // triage highlight is queue metadata and doesn't survive the round trip.
  const undoDismiss = (dismissed: RegistryCard[]) => {
    const label =
      dismissed.length === 1
        ? `Dismissed “${dismissed[0].name}” — click to undo`
        : `Dismissed ${dismissed.length} suggestions — click to undo`;
    useStore.getState().pushToast("success", label, () =>
      void (async () => {
        for (const c of dismissed) await api.setCardOrigin(c.id, "auto");
        onChanged();
      })(),
    );
  };

  const rule = async (id: string, origin: string) => {
    setBusy(id);
    try {
      const card = cards.find((c) => c.id === id);
      await api.setCardOrigin(id, origin);
      if (origin === "dismissed" && card) undoDismiss([card]);
      onChanged();
    } finally {
      setBusy(null);
    }
  };

  const ruleAll = async (key: string, origin: string, onlyRec?: boolean) => {
    setBusy(key);
    try {
      const affected = onlyRec
        ? cards.filter((c) => c.triage === "recommended")
        : cards;
      await api.ruleAllSuggested(origin, onlyRec);
      if (origin === "dismissed" && affected.length > 0) undoDismiss(affected);
      onChanged();
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="mb-6">
      <div className="flex items-center gap-2">
        <h2 className="text-badge font-medium uppercase tracking-wider text-subtle-foreground">
          Suggested
        </h2>
        {/* A sweep verdict only earns its place once ruling one-by-one is a
            chore; a single suggestion keeps the single pair of buttons. */}
        {cards.length > 1 && (
          <span className="ml-auto flex items-center gap-1">
            {/* Only when triage split the queue — with everything (or
                nothing) recommended it would just restate Keep all. */}
            {recommended > 0 && recommended < cards.length && (
              <Button
                size="sm"
                variant="secondary"
                onClick={() => void ruleAll("rec", "", true)}
                loading={busy === "rec"}
                disabled={busy === "all"}
                title="Keep the marked suggestions; the rest stay queued"
              >
                <Sparkles className="h-3.5 w-3.5" />
                Keep recommended
              </Button>
            )}
            <Button
              size="sm"
              variant={
                recommended > 0 && recommended < cards.length
                  ? "ghost"
                  : "secondary"
              }
              onClick={() => void ruleAll("all", "")}
              loading={busy === "all"}
              disabled={busy === "rec"}
            >
              <Check className="h-3.5 w-3.5" />
              Keep all
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void ruleAll("all", "dismissed")}
              disabled={busy !== null}
              title="These won't be suggested again"
            >
              <X className="h-3.5 w-3.5" />
              Dismiss all
            </Button>
          </span>
        )}
      </div>
      <p className="mt-1 text-caption text-muted-foreground">
        Things that recur across your documents. Keeping one adds it to your
        registry and files its documents under it. Filing changes nothing in
        the documents themselves.
      </p>
      <div className="mt-2 flex flex-wrap gap-2">
        {ordered.map((c) => (
          <div
            key={c.id}
            className="flex items-center gap-2 rounded-lg border border-dashed border-border-strong bg-surface/40 px-2.5 py-1.5"
          >
            {c.triage === "recommended" && (
              <Sparkles
                className="h-3 w-3 shrink-0 text-primary"
                aria-label="Recommended"
              />
            )}
            <span className="text-muted-foreground">{kindIcon(c.kind)}</span>
            <span className="text-body" title={c.triage === "recommended" ? "Recommended — recurs across your documents" : undefined}>{c.name}</span>
            <span className="text-micro text-subtle-foreground">
              {kindLabel(c.kind)}
            </span>
            <Button
              size="sm"
              onClick={() => void rule(c.id, "")}
              loading={busy === c.id}
            >
              <Check className="h-3.5 w-3.5" />
              Keep
            </Button>
            <button
              className="rounded p-1 text-muted-foreground transition hover:text-destructive"
              title="Won't be suggested again"
              onClick={() => void rule(c.id, "dismissed")}
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        ))}
      </div>
    </section>
  );
}

function CardTile({
  card,
  onOpen,
  onChanged,
  picked,
  onActivate,
  onContextItems,
}: {
  card: RegistryCard;
  onOpen: () => void;
  onChanged: () => void;
  /** Ids currently selected on the index, for the row wash. */
  picked?: Set<string>;
  /** Click handler that may consume the click as a selection gesture;
   *  returning undefined falls through to opening the card. */
  onActivate?: (e: React.MouseEvent) => true | undefined;
  onContextItems?: () => RowMenuItem[] | null;
}) {
  const docs = confirmed(card).length;
  const pending = proposed(card).length;
  return (
    <div
      title={card.name}
      data-pick-id={card.id}
      className={cn(
        "group relative flex min-h-[132px] cursor-pointer flex-col rounded-lg border border-border bg-surface p-4 transition-colors hover:border-border-strong hover:bg-surface-2",
        "has-[[aria-expanded=true]]:z-30",
        picked?.has(card.id) && "bg-primary/10 hover:bg-primary/15",
      )}
    >
      <CardAction
        label={`Open card ${card.name}`}
        onClick={(e) => onActivate?.(e) ?? onOpen()}
      />
      <div className="pointer-events-none relative z-10 mb-auto flex h-8 w-8 items-center justify-center rounded-lg bg-surface-2 text-muted-foreground">
        {kindIcon(card.kind)}
      </div>
      <div className="pointer-events-none relative z-10 mt-3 flex items-center gap-1.5">
        <span className="truncate text-card font-medium">{card.name}</span>
        {pending > 0 && (
          // Same dot the notebook cards use for unread reports: something
          // here is waiting on you.
          <span
            className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
            title={`${pending} document${pending === 1 ? "" : "s"} waiting`}
            aria-label={`${pending} waiting to be confirmed`}
          />
        )}
      </div>
      <div className="pointer-events-none relative z-10 mt-1 flex items-center gap-1.5 text-micro text-subtle-foreground">
        <Badge className="gap-1">
          <FileText className="h-2.5 w-2.5" />
          {docs}
        </Badge>
        <span>·</span>
        <span className="truncate">{kindLabel(card.kind)}</span>
        {isOrphan(card) && (
          <span
            className="pointer-events-auto shrink-0 rounded border border-border px-1"
            title={ORPHAN_HINT}
          >
            orphaned
          </span>
        )}
      </div>
      <div className="absolute right-2 top-2 z-20 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
        <RowMenu
          contextItems={onContextItems}
          items={[
            { label: "Open", onClick: onOpen },
            {
              label: "Delete card",
              danger: true,
              onClick: () => void deleteCardsUndoable([card], onChanged),
            },
          ]}
        />
      </div>
    </div>
  );
}

function NewCardModal({
  open,
  onClose,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: (c: RegistryCard) => void;
}) {
  const [kind, setKind] = useState<CardKind>("asset");
  const [name, setName] = useState("");
  const [identifiers, setIdentifiers] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (open) {
      setKind("asset");
      setName("");
      setIdentifiers("");
    }
  }, [open]);

  const submit = async () => {
    if (!name.trim() || busy) return;
    setBusy(true);
    try {
      onCreated(await api.addRegistryCard(kind, name.trim(), identifiers));
      onClose();
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="New card"
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => void submit()}
            loading={busy}
            disabled={!name.trim()}
          >
            Create
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-3">
        <div className="flex flex-wrap gap-1">
          {KINDS.map((k) => (
            <button
              key={k.id}
              type="button"
              onClick={() => setKind(k.id)}
              aria-pressed={kind === k.id}
              className={cn(
                "flex items-center gap-1.5 rounded-md px-2 py-1 text-caption transition-colors",
                kind === k.id
                  ? "bg-surface-2 font-medium text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {k.icon}
              {kindLabel(k.id)}
            </button>
          ))}
        </div>
        <Input
          autoFocus
          placeholder="What is it called?"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void submit()}
        />
        <div>
          <Input
            placeholder="Identifiers — VIN, policy no., serial (optional)"
            value={identifiers}
            onChange={(e) => setIdentifiers(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void submit()}
          />
          <p className="mt-1.5 text-caption text-muted-foreground">
            Documents containing one of these are filed here automatically. Only
            put strings that could not belong to anything else — 6+ characters
            with a digit. Everything else is proposed for you to confirm.
          </p>
        </div>
      </div>
    </Modal>
  );
}

function CardDetail({
  card,
  onBack,
  onChanged,
}: {
  card: RegistryCard;
  onBack: () => void;
  onChanged: () => void;
}) {
  const notebooks = useStore((s) => s.notebooks);
  const [titles, setTitles] = useState<Map<string, Source>>(new Map());

  // Escape backs out, the way it leaves the reader — a detail view you can
  // only exit by finding the right button is a dead end.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !document.querySelector('[role="dialog"]')) {
        onBack();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onBack]);

  // Resolve attached documents across every notebook they live in — a card
  // spans notebooks, so the open notebook's source list isn't enough. Keyed
  // on the attachment identities, not the array — every registry bump
  // returns fresh objects, and array identity refired one listSources per
  // touched notebook even when nothing changed.
  const attachmentsKey = card.attachments
    .map((a) => `${a.sourceId}:${a.status}`)
    .join("|");
  useEffect(() => {
    let alive = true;
    const nbs = [
      ...new Set(card.attachments.map((a) => a.notebookId).filter(Boolean)),
    ];
    void Promise.all(nbs.map((id) => api.listSources(id))).then((lists) => {
      if (!alive) return;
      const m = new Map<string, Source>();
      for (const list of lists) for (const s of list) m.set(s.id, s);
      setTitles(m);
    });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [attachmentsKey]);

  const pending = proposed(card);
  const docs = confirmed(card);

  const setStatus = async (sourceId: string, status: string) => {
    await api.setAttachmentStatus(card.id, sourceId, status);
    onChanged();
  };

  // ---- Selection over the filed documents (docs/RFC-multi-select.md) ----
  const docIds = [...pending, ...docs].map((a) => a.sourceId);
  const docPick = usePickList("attachments", docIds);
  const docsRef = useRef<HTMLDivElement>(null);
  const docBase = useRef<string[]>([]);
  const {
    onPointerDown: docMarqueeDown,
    marquee: docMarquee,
    justEnded: docJustEnded,
  } = useMarquee({
    containerRef: docsRef,
    onStart: (additive) => {
      const p = useStore.getState().picked;
      docBase.current = additive && p?.kind === "attachments" ? p.ids : [];
    },
    onSelect: (ids) =>
      docPick.pickSet(
        "attachments",
        [...new Set([...docBase.current, ...ids])],
        false,
      ),
    onClearBackground: docPick.clearPicked,
  });

  const docBatchItems = (ids: string[]): RowMenuItem[] => [
    {
      label: `Unfile ${ids.length} documents`,
      icon: <Trash2 className="h-3.5 w-3.5" />,
      onClick: () =>
        void (async () => {
          for (const id of ids)
            await api.setAttachmentStatus(card.id, id, "rejected");
          useStore.getState().clearPicked();
          onChanged();
        })(),
    },
    {
      label: `Unlink ${ids.length} only`,
      icon: <X className="h-3.5 w-3.5" />,
      onClick: () =>
        void (async () => {
          for (const id of ids)
            await api.setAttachmentStatus(card.id, id, "remove");
          useStore.getState().clearPicked();
          onChanged();
        })(),
    },
  ];

  const openDoc = (a: CardAttachment) => {
    const s = titles.get(a.sourceId);
    // A card spans notebooks, so opening its document means switching to
    // the notebook that holds it first.
    void useStore
      .getState()
      .selectNotebook(a.notebookId)
      .then(() =>
        useStore.getState().openSourceViewer(a.sourceId, s?.title ?? ""),
      );
  };

  const row = (a: CardAttachment, isPending: boolean) => {
    const s = titles.get(a.sourceId);
    return (
      <div
        key={a.sourceId}
        data-pick-id={a.sourceId}
        // Click selects, double-click opens — Finder's split. Opening on a
        // single click made the row impossible to select by clicking, and a
        // title button spanning the row left nowhere to begin a drag.
        onClick={(e) => {
          if (docJustEnded()) return;
          docPick.handleClick(e, a.sourceId);
        }}
        onDoubleClick={() => openDoc(a)}
        className={cn(
          "group flex cursor-pointer items-center gap-2 border-b border-border py-2 last:border-b-0",
          docPick.pickedIds.has(a.sourceId) && "bg-primary/10",
        )}
      >
        <FileText className="h-3.5 w-3.5 shrink-0 text-subtle-foreground" />
        <span
          className="min-w-0 flex-1 truncate text-left text-body"
          title={s?.title}
        >
          {s?.title ?? "Untitled document"}
        </span>
        <span className="shrink-0 text-micro text-subtle-foreground">
          {notebooks.find((n) => n.id === a.notebookId)?.title}
        </span>
        <span
          className="shrink-0 rounded border border-border px-1.5 py-0.5 text-micro text-muted-foreground"
          title="Why this document is filed here"
        >
          {receipt(a)}
        </span>
        <span className="shrink-0 text-micro text-subtle-foreground">
          {relativeTime(a.at)}
        </span>
        {isPending ? (
          <span className="flex shrink-0 items-center gap-1">
            <Button size="sm" onClick={() => void setStatus(a.sourceId, "confirmed")}>
              <Check className="h-3.5 w-3.5" />
              Confirm
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void setStatus(a.sourceId, "rejected")}
              title="Won't be suggested again"
            >
              <X className="h-3.5 w-3.5" />
              Not this
            </Button>
          </span>
        ) : (
          <span
            // Always rendered, never revealed: this menu sits inline in the
            // row, so fading it in on hover reflowed the metadata beside it
            // every time the pointer crossed a row.
            className="shrink-0"
            onClick={(e) => e.stopPropagation()}
          >
            <RowMenu
              alwaysVisible
              label={`Filing options for "${s?.title ?? "this document"}"`}
              contextItems={() =>
                docPick.contextItems(a.sourceId, docBatchItems)
              }
              items={[
                {
                  label: "Open document",
                  icon: <FileText className="h-3.5 w-3.5" />,
                  onClick: () => openDoc(a),
                },
                {
                  label: "Unfile — don't re-attach",
                  icon: <Trash2 className="h-3.5 w-3.5" />,
                  // Rejection, not deletion: the pair is remembered, so the
                  // sweep never re-attaches this document to this card.
                  onClick: () => void setStatus(a.sourceId, "rejected"),
                },
                {
                  label: "Unlink only",
                  icon: <X className="h-3.5 w-3.5" />,
                  // Forgets the pair entirely, so auto-filing may propose it
                  // again — the right choice when the filing was a mistake
                  // rather than a judgement about this document.
                  onClick: () => void setStatus(a.sourceId, "remove"),
                },
              ]}
            />
          </span>
        )}
      </div>
    );
  };

  return (
    <div className="relative z-10 min-h-0 flex-1 overflow-y-auto">
      {/* 960px to match the grid and the page header — at 760 the detail
          jumped left of everything else on Home. */}
      <div className="mx-auto w-full max-w-[960px] px-6 pb-10">
        {/* Sticky: flush against the scroller's top edge the button was
            clipped by the header above it, and scrolling a long document
            list used to carry the only way out off-screen. */}
        <div className="sticky top-0 z-10 -mx-6 mb-3 bg-background/95 px-6 pb-3 pt-4 backdrop-blur">
          <Button variant="secondary" size="sm" onClick={onBack}>
            <ArrowLeft className="h-3.5 w-3.5" />
            All cards
          </Button>
        </div>

        <div className="flex items-start gap-3">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-muted-foreground">
            {kindIcon(card.kind)}
          </div>
          <div className="min-w-0 flex-1">
            <h1 className="text-title font-semibold">{card.name}</h1>
            <div className="mt-0.5 text-caption text-muted-foreground">
              {kindLabel(card.kind)} · {docs.length} document
              {docs.length === 1 ? "" : "s"}
            </div>
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={() =>
              void deleteCardsUndoable([card], () => {
                onBack();
                onChanged();
              })
            }
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </div>

        <CardFacts card={card} onChanged={onChanged} />

        {/* One marquee container over both document lists: a drag that
            begins among the waiting rows should select there too. */}
        <div ref={docsRef} onPointerDown={docMarqueeDown} className="select-none">
        {pending.length > 0 && (
          <section className="mt-6">
            <h2 className="text-badge font-medium uppercase tracking-wider text-subtle-foreground">
              Waiting for you
            </h2>
            <p className="mt-1 text-caption text-muted-foreground">
              These matched this card&rsquo;s name, which is a guess, not proof.
              Confirming only files the document here. Filing changes nothing
              in the document.
            </p>
            <div className="mt-2">{pending.map((a) => row(a, true))}</div>
          </section>
        )}

        <section className="mt-6">
          <h2 className="text-badge font-medium uppercase tracking-wider text-subtle-foreground">
            Documents
          </h2>
          {docs.length === 0 ? (
            <EmptyState
              compact
              title="Nothing filed here yet"
              hint="Attach a source from its ⋯ menu in the gallery or sources list."
            />
          ) : (
            <div className="mt-2">{docs.map((a) => row(a, false))}</div>
          )}
        </section>
        </div>
      </div>
      {docMarquee}
    </div>
  );
}

/** Key facts + identifiers, edited in place — the reader's doc-properties
    grid shape, with the same click-to-edit contract. */
function CardFacts({
  card,
  onChanged,
}: {
  card: RegistryCard;
  onChanged: () => void;
}) {
  const [adding, setAdding] = useState(false);
  const [label, setLabel] = useState("");
  const [value, setValue] = useState("");

  const save = async (facts: CardFact[]) => {
    await api.updateRegistryCard(card.id, { facts });
    onChanged();
  };

  return (
    <div className="mt-5 border-t border-border pt-4">
      <div className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-caption">
        <span className="text-subtle-foreground">Identifiers</span>
        <IdentifierField card={card} onChanged={onChanged} />
        {card.facts.map((f, i) => (
          <FactRow
            key={`${f.label}-${i}`}
            fact={f}
            onRemove={() => void save(card.facts.filter((_, j) => j !== i))}
          />
        ))}
      </div>
      {adding ? (
        <div className="mt-2 flex items-center gap-2">
          <Input
            autoFocus
            className="w-40"
            placeholder="Label"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
          />
          <Input
            className="flex-1"
            placeholder="Value"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key !== "Enter" || !label.trim()) return;
              void save([...card.facts, { label: label.trim(), value }]);
              setLabel("");
              setValue("");
              setAdding(false);
            }}
          />
          <Button variant="ghost" size="sm" onClick={() => setAdding(false)}>
            Cancel
          </Button>
        </div>
      ) : (
        <button
          className="mt-2 text-caption text-muted-foreground transition-colors hover:text-foreground"
          onClick={() => setAdding(true)}
        >
          + Add a fact
        </button>
      )}
    </div>
  );
}

function FactRow({
  fact,
  onRemove,
}: {
  fact: CardFact;
  onRemove: () => void;
}) {
  return (
    <>
      <span className="text-subtle-foreground">{fact.label}</span>
      <span className="group flex items-center gap-2 text-foreground">
        <span className="min-w-0 flex-1">{fact.value}</span>
        <button
          className="shrink-0 text-subtle-foreground opacity-0 transition hover:text-destructive group-hover:opacity-100"
          onClick={onRemove}
          title="Remove this fact"
        >
          <X className="h-3 w-3" />
        </button>
      </span>
    </>
  );
}

/** Identifiers are the auto-attach key, so editing them is the one field
    here with real consequences — the hint says so at the point of editing. */
function IdentifierField({
  card,
  onChanged,
}: {
  card: RegistryCard;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(card.identifiers);
  const currentId = useStore((s) => s.currentId);

  const commit = async () => {
    setEditing(false);
    if (draft.trim() === card.identifiers) return;
    await api.updateRegistryCard(card.id, { identifiers: draft.trim() });
    // Re-file the open notebook against the new identifiers: adding a VIN to
    // a card that already had documents should pick them up now, not on the
    // next import.
    if (currentId) await api.rematchRegistry(currentId);
    onChanged();
  };

  if (!editing) {
    return (
      <button
        className="text-left text-foreground hover:text-primary"
        onClick={() => {
          setDraft(card.identifiers);
          setEditing(true);
        }}
      >
        {card.identifiers || (
          <span className="text-subtle-foreground">
            Add a VIN, policy no., or serial…
          </span>
        )}
      </button>
    );
  }
  return (
    <Input
      autoFocus
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => void commit()}
      onKeyDown={(e) => {
        if (e.key === "Enter") void commit();
        if (e.key === "Escape") setEditing(false);
      }}
    />
  );
}
