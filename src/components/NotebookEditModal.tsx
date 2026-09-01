import { useEffect, useState } from "react";
import { useStore } from "@/lib/store";
import type { Notebook } from "@/lib/types";
import {
  NOTEBOOK_ICONS,
  NOTEBOOK_PALETTE,
  notebookIcon,
} from "@/lib/notebookIcons";
import { cn } from "@/lib/utils";
import { Button, Input, Modal } from "./ui";

/** One dialog owns a notebook's look — name, icon, and color together.
 *  The Home row menus' Rename and the workspace title bar both open it;
 *  color moved in here from its own palette pop-over. */
export function NotebookEditModal({
  notebook,
  onClose,
}: {
  notebook: Notebook | null;
  onClose: () => void;
}) {
  const [title, setTitle] = useState("");
  const [icon, setIcon] = useState("");
  const [color, setColor] = useState("");
  useEffect(() => {
    if (!notebook) return;
    setTitle(notebook.title);
    setIcon(notebook.icon);
    setColor(notebook.color || NOTEBOOK_PALETTE[0]);
  }, [notebook]);

  const save = () => {
    if (!notebook) return;
    const next = { title: title.trim(), icon, color };
    // Icon and color first, sequenced: rename() ends with a full refresh,
    // and firing writes unordered let that refresh read the DB before the
    // other writes landed — reverting the optimistic values until some
    // later refresh ("shows up two edits later").
    void (async () => {
      const st = useStore.getState();
      if (notebook.icon !== next.icon)
        await st.setNotebookIcon(notebook.id, next.icon);
      if ((notebook.color || NOTEBOOK_PALETTE[0]) !== next.color)
        await st.setNotebookColor(notebook.id, next.color);
      if (next.title && notebook.title !== next.title)
        await st.renameNotebook(notebook.id, next.title);
    })();
    onClose();
  };

  return (
    <Modal open={!!notebook} onClose={onClose} title="Edit notebook">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
        className="flex flex-col gap-3"
      >
        <Input
          autoFocus
          name="notebook-title"
          aria-label="Notebook title"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
        />
        {/* Color first: it reads as part of the name row above (the dot the
            title bar and cards wear), where the icon grid is a bigger,
            slower choice below it. */}
        <div className="flex items-center justify-between py-1">
          {NOTEBOOK_PALETTE.map((c) => (
            <button
              key={c}
              type="button"
              aria-pressed={color === c}
              aria-label={`Color ${c}`}
              onClick={() => setColor(c)}
              className={cn(
                "h-6 w-6 rounded-full border border-border transition-shadow",
                color === c &&
                  "ring-2 ring-foreground ring-offset-1 ring-offset-surface",
              )}
              style={{ backgroundColor: c }}
            />
          ))}
        </div>
        {/* Icon picker: the auto-picked icon can always be overridden
            here; the plain book is a first-class choice, not an absence. */}
        <div className="grid grid-cols-8 gap-1">
          {["", ...Object.keys(NOTEBOOK_ICONS).filter((k) => k !== "book-open")].map(
            (name) => {
              const Icon = notebookIcon(name);
              const active = icon === name;
              return (
                <button
                  key={name || "default"}
                  type="button"
                  aria-pressed={active}
                  aria-label={
                    name ? `Icon: ${name.replace(/-/g, " ")}` : "Default icon"
                  }
                  title={name ? name.replace(/-/g, " ") : "Default"}
                  onClick={() => setIcon(name)}
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
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary">
            Save
          </Button>
        </div>
      </form>
    </Modal>
  );
}
