/* The Inbox: captures that arrived without a home. The clipper, Services,
   `alchemy://` links and the menu bar save here instantly instead of raising
   a blocking "which notebook?" question; the typed judge's suggestion is
   computed behind them and waits beside each row. One key files it.

   Nothing is imported until a row is filed, so dismissing one leaves no
   trace and filing one is the same add path the notebook's own Add source
   uses. Filing is reversible through the notebook's Remove source, which is
   why "Accept all confident" needs no undo of its own. */
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ClipboardPaste,
  FileText,
  Globe,
  Inbox as InboxIcon,
} from "lucide-react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type { InboxItem } from "@/lib/types";
import {
  confidentItems,
  inboxKind,
  inboxTitle,
  percent,
  verbForKey,
  type InboxKind,
} from "@/lib/inbox";
import { cn, relativeTime, shortcutBlocked } from "@/lib/utils";
import {
  Button,
  Chip,
  EmptyState,
  Input,
  Modal,
  RowMenu,
  Select,
  Spinner,
  type RowMenuItem,
} from "./ui";

const GLYPH: Record<InboxKind, typeof Globe> = {
  url: Globe,
  file: FileText,
  text: ClipboardPaste,
};

export function InboxSection() {
  const items = useStore((s) => s.inbox);
  const notebooks = useStore((s) => s.notebooks);
  const [selId, setSelId] = useState<string | null>(null);
  // The row being given a new notebook's name, and the name so far.
  const [naming, setNaming] = useState<{ id: string; title: string } | null>(
    null,
  );
  const [choosing, setChoosing] = useState<string | null>(null);
  const [busy, setBusy] = useState<Set<string>>(new Set());

  // Fresh on arrival at the section; after that mcp://changed keeps it so.
  useEffect(() => {
    void useStore.getState().refreshInbox();
  }, []);

  const selIndex = Math.max(
    0,
    items.findIndex((i) => i.id === selId),
  );
  const selected: InboxItem | undefined = items[selIndex];
  const confident = useMemo(() => confidentItems(items), [items]);
  const active = useMemo(
    () => notebooks.filter((n) => n.status !== "archived"),
    [notebooks],
  );

  const file = useCallback(
    async (
      item: InboxItem,
      opts: { notebookId?: string; newTitle?: string } = {},
      quiet = false,
    ): Promise<boolean> => {
      const st = useStore.getState();
      setBusy((b) => new Set(b).add(item.id));
      try {
        const landed = await api.inboxAccept(item.id, opts);
        // The row is gone; step the selection to its neighbour.
        const list = useStore.getState().inbox;
        const at = list.findIndex((i) => i.id === item.id);
        setSelId(list[at + 1]?.id ?? list[at - 1]?.id ?? null);
        await Promise.all([st.refreshInbox(), st.refreshNotebooks()]);
        if (!quiet) st.pushToast("success", `Filed in ${landed.title}`);
        return true;
      } catch (e) {
        st.pushToast("error", e instanceof Error ? e.message : String(e));
        return false;
      } finally {
        setBusy((b) => {
          const next = new Set(b);
          next.delete(item.id);
          return next;
        });
      }
    },
    [],
  );

  const dismiss = useCallback(async (item: InboxItem) => {
    const st = useStore.getState();
    const list = st.inbox;
    const at = list.findIndex((i) => i.id === item.id);
    try {
      await api.inboxDismiss(item.id);
      setSelId(list[at + 1]?.id ?? list[at - 1]?.id ?? null);
      await st.refreshInbox();
    } catch (e) {
      st.pushToast("error", e instanceof Error ? e.message : String(e));
    }
  }, []);

  const accept = useCallback(
    (item: InboxItem) => {
      if (!item.suggested) {
        useStore
          .getState()
          .pushToast("info", "Still choosing a notebook for this one");
        return;
      }
      if (!item.isNew && !item.suggestedNotebookId) {
        setChoosing(item.id);
        return;
      }
      void file(item);
    },
    [file],
  );

  const startNew = useCallback((item: InboxItem) => {
    // The suggestion's own name when it is a new notebook; otherwise ask,
    // starting from what the capture is called.
    if (item.isNew && item.suggestedTitle.trim()) {
      void file(item, { newTitle: item.suggestedTitle });
      return;
    }
    setNaming({ id: item.id, title: inboxTitle(item) });
  }, [file]);

  // Keys act on the selected row, anywhere on the section that is not a
  // field or a dialog. Home's type-to-ask listener would otherwise treat N and
  // 1-4 as the start of a question, so this one listens in the capture phase
  // and stops the keys it takes.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || shortcutBlocked(e)) return;
      const verb = verbForKey(e.key, selected);
      if (!verb) return;
      e.preventDefault();
      e.stopPropagation();
      switch (verb.type) {
        case "move": {
          const next = items[selIndex + verb.delta];
          if (next) setSelId(next.id);
          break;
        }
        case "accept":
          if (selected) accept(selected);
          break;
        case "alternative":
          if (selected)
            void file(selected, {
              notebookId: selected.alternatives[verb.index].notebookId,
            });
          break;
        case "new":
          if (selected) startNew(selected);
          break;
        case "dismiss":
          if (selected) void dismiss(selected);
          break;
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [items, selIndex, selected, accept, file, startNew, dismiss]);

  useEffect(() => {
    document
      .querySelector(`[data-inbox-id="${selected?.id}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [selected?.id]);

  const acceptConfident = async () => {
    let filed = 0;
    for (const item of confident) if (await file(item, {}, true)) filed++;
    if (filed > 0)
      useStore
        .getState()
        .pushToast(
          "success",
          `Filed ${filed} ${filed === 1 ? "capture" : "captures"}`,
        );
  };

  if (items.length === 0) {
    return (
      <div className="relative z-10 flex min-h-0 flex-1 flex-col items-center justify-center">
        <EmptyState
          icon={<InboxIcon className="h-5 w-5" />}
          title="Nothing waiting."
          hint="Captures from the clipper, Services and alchemy:// land here."
        />
      </div>
    );
  }

  return (
    <div className="relative z-10 flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center justify-between gap-3 border-b border-border px-6 py-2">
        <span className="text-micro text-subtle-foreground">
          Enter files the suggestion · 1–4 an alternative · N a new notebook ·
          Delete dismisses
        </span>
        <Button
          variant="secondary"
          size="sm"
          className="h-[26px] rounded-lg"
          disabled={confident.length === 0}
          onClick={() => void acceptConfident()}
          title="File every capture the judge is at least 70% sure of. Never creates a notebook."
        >
          Accept {confident.length} confident
        </Button>
      </div>
      <ul
        role="listbox"
        aria-label="Inbox"
        className="min-h-0 flex-1 overflow-y-auto"
      >
        {items.map((item) => (
          <InboxRow
            key={item.id}
            item={item}
            selected={item.id === selected?.id}
            busy={busy.has(item.id)}
            naming={naming?.id === item.id ? naming : null}
            onSelect={() => setSelId(item.id)}
            onAccept={() => accept(item)}
            onFileInto={(notebookId) => void file(item, { notebookId })}
            onNew={() => startNew(item)}
            onChoose={() => setChoosing(item.id)}
            onDismiss={() => void dismiss(item)}
            onNaming={setNaming}
            onNamed={(title) => {
              setNaming(null);
              if (title.trim()) void file(item, { newTitle: title.trim() });
            }}
          />
        ))}
      </ul>
      {choosing && (
        <ChooseNotebook
          notebooks={active}
          initial={
            items.find((i) => i.id === choosing)?.suggestedNotebookId ?? ""
          }
          onCancel={() => setChoosing(null)}
          onChoose={(notebookId) => {
            const item = items.find((i) => i.id === choosing);
            setChoosing(null);
            if (item) void file(item, { notebookId });
          }}
        />
      )}
    </div>
  );
}

function InboxRow({
  item,
  selected,
  busy,
  naming,
  onSelect,
  onAccept,
  onFileInto,
  onNew,
  onChoose,
  onDismiss,
  onNaming,
  onNamed,
}: {
  item: InboxItem;
  selected: boolean;
  busy: boolean;
  naming: { id: string; title: string } | null;
  onSelect: () => void;
  onAccept: () => void;
  onFileInto: (notebookId: string) => void;
  onNew: () => void;
  onChoose: () => void;
  onDismiss: () => void;
  onNaming: (n: { id: string; title: string } | null) => void;
  onNamed: (title: string) => void;
}) {
  const Glyph = GLYPH[inboxKind(item)];
  const menu: RowMenuItem[] = [
    { label: "Accept suggestion", onClick: onAccept, accelerator: "Return" },
    ...item.alternatives.map((a, i) => ({
      label: `File in ${a.title}`,
      onClick: () => onFileInto(a.notebookId),
      accelerator: String(i + 1),
    })),
    { label: "New notebook…", onClick: onNew, accelerator: "N" },
    { label: "Choose notebook…", onClick: onChoose },
    { label: "", onClick: () => {}, separator: true },
    { label: "Dismiss", onClick: onDismiss, danger: true },
  ];
  return (
    <li
      role="option"
      aria-selected={selected}
      data-inbox-id={item.id}
      onClick={onSelect}
      className={cn(
        "group flex cursor-default items-start gap-3 border-b border-border px-6 py-3 transition-colors",
        selected ? "bg-[var(--selection)]" : "hover:bg-surface-2",
        busy && "opacity-60",
      )}
    >
      <Glyph className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-3">
          <span className="min-w-0 flex-1 truncate text-body font-medium text-foreground">
            {inboxTitle(item)}
          </span>
          <span className="shrink-0 text-micro tabular-nums text-subtle-foreground">
            {relativeTime(item.createdAt)}
          </span>
        </div>
        {item.excerpt && (
          <div className="truncate text-caption text-muted-foreground">
            {item.excerpt}
          </div>
        )}
        <div className="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-caption">
          <Suggestion item={item} />
          {item.alternatives.map((a, i) => (
            <Chip
              key={a.notebookId}
              active={false}
              size="xs"
              onClick={(e) => {
                e.stopPropagation();
                onFileInto(a.notebookId);
              }}
              title={`File in ${a.title} (${i + 1})`}
            >
              <span className="tabular-nums text-subtle-foreground">
                {i + 1}
              </span>
              <span className="max-w-[160px] truncate">{a.title}</span>
              <span className="tabular-nums text-subtle-foreground">
                {percent(a.probability)}
              </span>
            </Chip>
          ))}
        </div>
        {naming && (
          <form
            className="mt-2 flex items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              onNamed(naming.title);
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <Input
              autoFocus
              aria-label="New notebook name"
              value={naming.title}
              onChange={(e) => onNaming({ id: item.id, title: e.target.value })}
              onKeyDown={(e) => e.key === "Escape" && onNaming(null)}
              className="max-w-[320px]"
            />
            <Button
              type="submit"
              variant="primary"
              size="sm"
              disabled={!naming.title.trim()}
            >
              Create and file
            </Button>
          </form>
        )}
      </div>
      <RowMenu items={menu} label="Inbox item" />
    </li>
  );
}

/** "→ Ferrari 458/488 Research · 92%", or the new-notebook form, or the wait. */
function Suggestion({ item }: { item: InboxItem }) {
  if (!item.suggested)
    return (
      <span className="inline-flex items-center gap-1.5 text-muted-foreground">
        <Spinner className="h-3 w-3" />
        Choosing…
      </span>
    );
  if (item.isNew)
    return (
      <span className="text-foreground">
        → New notebook: {item.suggestedTitle || "untitled"}
        {item.probability !== null && (
          <span className="text-subtle-foreground">
            {" "}
            · {percent(item.probability)}
          </span>
        )}
      </span>
    );
  if (!item.suggestedNotebookId)
    return (
      <span className="text-muted-foreground">
        No suggestion. Press Return to choose a notebook.
      </span>
    );
  return (
    <span className="text-foreground">
      → {item.suggestedTitle}
      {item.probability !== null && (
        <span className="text-subtle-foreground">
          {" "}
          · {percent(item.probability)}
        </span>
      )}
    </span>
  );
}

function ChooseNotebook({
  notebooks,
  initial,
  onCancel,
  onChoose,
}: {
  notebooks: { id: string; title: string }[];
  initial: string;
  onCancel: () => void;
  onChoose: (notebookId: string) => void;
}) {
  const [value, setValue] = useState(
    notebooks.some((n) => n.id === initial) ? initial : (notebooks[0]?.id ?? ""),
  );
  return (
    <Modal open onClose={onCancel} title="File in which notebook?">
      <form
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (value) onChoose(value);
        }}
      >
        <Select
          autoFocus
          aria-label="Notebook"
          value={value}
          onChange={setValue}
          options={notebooks.map((n) => ({ value: n.id, label: n.title }))}
        />
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!value}>
            File here
          </Button>
        </div>
      </form>
    </Modal>
  );
}
