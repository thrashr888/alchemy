import { useEffect, useState } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Button, Modal, Spinner } from "./ui";
import { FileText, Globe, ClipboardPaste, Sparkles } from "lucide-react";

/**
 * "Add to which notebook?" — the filing step for a source that arrived
 * without a home. Two ways in:
 *
 *  - An external workflow (browser extension, deep link, Services, menu bar)
 *    sent something to add but couldn't know which notebook the user meant.
 *  - The home dashboard's Add source button, where there IS no current
 *    notebook — that raises the same modal with an empty payload, so it opens
 *    on a capture field first and files second.
 *
 * Either way the router + Small model suggest the likeliest notebook (or a
 * new one) while the user reads; the dropdown stays live the whole time.
 */
/** Select value prefix marking "create a notebook with this title first".
 *  A prefix rather than a second piece of state so the whole choice — file
 *  here vs. start something new — stays one `<select>` value. */
const NEW_NOTEBOOK = "new:";

/** Active notebooks with the OPEN one first — it's the likeliest target,
 *  so it leads the list and is the default selection. The rest keep their
 *  recency order. */
function pickerNotebooks() {
  const s = useStore.getState();
  const active = s.notebooks.filter((n) => n.status !== "archived");
  const current = active.find((n) => n.id === s.currentId);
  return current
    ? [current, ...active.filter((n) => n.id !== current.id)]
    : active;
}

export function ExternalAddModal() {
  const pending = useStore((s) => s.pendingExternalAdd);
  useStore((s) => s.notebooks); // re-render on notebook changes
  useStore((s) => s.currentId);
  const confirm = useStore((s) => s.confirmExternalAdd);
  // Keyed remount per payload resets the selection to the likeliest notebook.
  if (!pending) return null;
  return (
    <ExternalAddForm
      key={pickerNotebooks()[0]?.id ?? "none"}
      onConfirm={(choice) => void fileInto(choice, confirm)}
      onCancel={() => useStore.setState({ pendingExternalAdd: null })}
    />
  );
}

/** Resolve the picker's choice to a notebook id, creating one first when the
 *  user accepted the "new notebook" suggestion, then hand off to the add. */
async function fileInto(
  choice: string,
  confirm: (notebookId: string) => Promise<void>,
) {
  const store = useStore.getState();
  if (!choice.startsWith(NEW_NOTEBOOK)) {
    await confirm(choice);
    return;
  }
  try {
    const id = await store.createNotebook(choice.slice(NEW_NOTEBOOK.length));
    if (!id) throw new Error("could not create the notebook");
    await confirm(id);
  } catch (e) {
    store.pushToast("error", e instanceof Error ? e.message : String(e));
  }
}

function ExternalAddForm({
  onConfirm,
  onCancel,
}: {
  onConfirm: (notebookId: string) => void;
  onCancel: () => void;
}) {
  const pending = useStore((s) => s.pendingExternalAdd);
  useStore((s) => s.notebooks);
  useStore((s) => s.currentId);
  const notebooks = pickerNotebooks();
  const [notebookId, setNotebookId] = useState(notebooks[0]?.id ?? "");
  // Auto-notebooking: the router + Small model pick the likeliest home while
  // the user reads the payload summary. Advisory only — it moves the
  // selection, never the outcome, and the dropdown stays live throughout.
  const [suggesting, setSuggesting] = useState(true);
  const [newTitle, setNewTitle] = useState<string | null>(null);
  const [touched, setTouched] = useState(false);
  // Raised from home with nothing to file: capture first, then pick. An
  // empty payload is the signal — every external entry point supplies one.
  const empty =
    !!pending && !pending.files.length && !pending.url && !pending.text;
  const [captured, setCaptured] = useState("");

  useEffect(() => {
    if (!pending || empty) return;
    let stale = false;
    setSuggesting(true);
    api
      .suggestNotebook({
        title: pending.title ?? "",
        text: pending.text ?? "",
        // Files can't be read from the frontend; their name is the signal
        // we have until the source is actually imported.
        url: pending.url ?? (pending.files[0] ?? ""),
      })
      .then((s) => {
        if (stale) return;
        // A user who already chose keeps their choice — the suggestion lost
        // the race, and overriding a deliberate click would be rude.
        if (s.isNew && s.title) {
          setNewTitle(s.title);
          if (!touched) setNotebookId(`${NEW_NOTEBOOK}${s.title}`);
        } else if (!touched && s.notebookId) {
          setNotebookId(s.notebookId);
        }
      })
      .catch(() => {
        /* suggestion is a nicety; the picker works without it */
      })
      .finally(() => !stale && setSuggesting(false));
    return () => {
      stale = true;
    };
    // Runs once per payload — `touched` is read, not tracked, on purpose.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pending, empty]);

  if (!pending) return null;

  // Capture step. Committing rewrites the pending payload, which re-runs the
  // suggestion effect above and drops us into the picker — one modal, two
  // beats, no second component.
  if (empty) {
    const trimmed = captured.trim();
    const isUrl = /^https?:\/\/\S+$/i.test(trimmed);
    return (
      <Modal open onClose={onCancel} title="Add a source" width="max-w-md">
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (!trimmed) return;
            useStore.setState({
              pendingExternalAdd: {
                files: [],
                url: isUrl ? trimmed : null,
                text: isUrl ? null : trimmed,
                title: null,
              },
            });
          }}
          className="flex flex-col gap-4"
        >
          <textarea
            autoFocus
            value={captured}
            onChange={(e) => setCaptured(e.target.value)}
            onKeyDown={(e) => {
              // Return submits; Shift+Return keeps writing. A pasted link is
              // the common case and shouldn't need a reach for the mouse.
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                e.currentTarget.form?.requestSubmit();
              }
            }}
            rows={5}
            placeholder="Paste a link, or type notes to save…"
            className="w-full resize-none rounded-md border border-input bg-surface-2 px-2.5 py-2 text-body leading-relaxed text-foreground outline-none placeholder:text-subtle-foreground focus:border-ring/70 focus:ring-1 focus:ring-ring/40"
          />
          <div className="flex items-center justify-between gap-2">
            <span className="text-micro text-subtle-foreground">
              {isUrl ? "Looks like a link — the page will be fetched" : ""}
            </span>
            <div className="flex gap-2">
              <Button type="button" variant="ghost" onClick={onCancel}>
                Cancel
              </Button>
              <Button type="submit" variant="primary" disabled={!trimmed}>
                Continue
              </Button>
            </div>
          </div>
        </form>
      </Modal>
    );
  }
  const summary = pending.files.length ? (
    <span className="flex items-start gap-2">
      <FileText className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <span>
        {pending.files.length === 1
          ? (pending.files[0].split("/").pop() ?? "1 file")
          : `${pending.files.length} files`}
      </span>
    </span>
  ) : pending.url ? (
    <span className="flex items-start gap-2">
      <Globe className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <span className="break-all">{pending.url}</span>
    </span>
  ) : (
    <span className="flex items-start gap-2">
      <ClipboardPaste className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <span className="line-clamp-3">{pending.text}</span>
    </span>
  );

  return (
    <Modal
      open
      onClose={onCancel}
      title="Add to which notebook?"
      width="max-w-md"
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (notebookId) onConfirm(notebookId);
        }}
        className="flex flex-col gap-4"
      >
        <div className="rounded-md border border-border bg-surface-2/40 px-3 py-2.5 text-caption leading-relaxed text-foreground/90">
          {summary}
        </div>
        <select
          autoFocus
          value={notebookId}
          onChange={(e) => {
            setTouched(true);
            setNotebookId(e.target.value);
          }}
          className="h-8 w-full rounded-md border border-input bg-surface-2 px-2 text-body text-foreground outline-none focus:border-ring/70 focus:ring-1 focus:ring-ring/40"
        >
          {newTitle && (
            <option value={`${NEW_NOTEBOOK}${newTitle}`}>
              New notebook — “{newTitle}”
            </option>
          )}
          {notebooks.map((nb) => (
            <option key={nb.id} value={nb.id}>
              {nb.id === useStore.getState().currentId
                ? `${nb.title} — current notebook`
                : nb.title}
            </option>
          ))}
        </select>
        <div className="-mt-2 flex h-4 items-center gap-1.5 text-micro text-subtle-foreground">
          {suggesting ? (
            <>
              <Spinner className="h-3 w-3" />
              <span>Finding the best notebook…</span>
            </>
          ) : (
            !touched && (
              <>
                <Sparkles className="h-3 w-3" />
                <span>Suggested. Change it to file this elsewhere.</span>
              </>
            )
          )}
        </div>
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!notebookId}>
            Add source
          </Button>
        </div>
      </form>
    </Modal>
  );
}
