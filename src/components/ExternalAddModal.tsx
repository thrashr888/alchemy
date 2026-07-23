import { useState } from "react";
import { useStore } from "@/lib/store";
import { Button, Modal } from "./ui";
import { FileText, Globe, ClipboardPaste } from "lucide-react";

/**
 * An external workflow (deep link, Services, menu bar) sent something to add,
 * but couldn't know which notebook the user meant — ask. The dropdown is
 * sorted by recently updated and defaults to the most recent notebook, so the
 * common case is a single Return keypress.
 */
export function ExternalAddModal() {
  const pending = useStore((s) => s.pendingExternalAdd);
  const notebooks = useStore((s) => s.notebooks);
  const confirm = useStore((s) => s.confirmExternalAdd);
  // Keyed remount per payload resets the selection to the freshest notebook.
  if (!pending) return null;
  return (
    <ExternalAddForm
      key={notebooks[0]?.id ?? "none"}
      onConfirm={(nb) => void confirm(nb)}
      onCancel={() => useStore.setState({ pendingExternalAdd: null })}
    />
  );
}

function ExternalAddForm({
  onConfirm,
  onCancel,
}: {
  onConfirm: (notebookId: string) => void;
  onCancel: () => void;
}) {
  const pending = useStore((s) => s.pendingExternalAdd);
  const notebooks = useStore((s) => s.notebooks);
  const [notebookId, setNotebookId] = useState(notebooks[0]?.id ?? "");

  if (!pending) return null;
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
          onChange={(e) => setNotebookId(e.target.value)}
          className="h-8 w-full rounded-md border border-input bg-surface-2 px-2 text-body text-foreground outline-none focus:border-ring/70 focus:ring-1 focus:ring-ring/40"
        >
          {notebooks.map((nb) => (
            <option key={nb.id} value={nb.id}>
              {nb.title}
            </option>
          ))}
        </select>
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
