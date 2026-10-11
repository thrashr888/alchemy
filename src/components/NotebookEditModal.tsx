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
import {
  COVER_STYLES,
  COVER_STYLE_LABEL,
  coverChoice,
  NO_COVER,
  useNotebookCover,
  type CoverStyle,
} from "@/lib/cover";

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
  const [cover, setCover] = useState("");
  useEffect(() => {
    if (!notebook) return;
    setTitle(notebook.title);
    setIcon(notebook.icon);
    setColor(notebook.color || NOTEBOOK_PALETTE[0]);
    setCover(notebook.cover ?? "");
  }, [notebook]);

  const save = () => {
    if (!notebook) return;
    const next = { title: title.trim(), icon, color, cover };
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
      if ((notebook.cover ?? "") !== next.cover)
        await st.setNotebookCover(notebook.id, next.cover);
      if (next.title && notebook.title !== next.title)
        await st.renameNotebook(notebook.id, next.title);
    })();
    onClose();
  };

  return (
    <Modal
      open={!!notebook}
      onClose={onClose}
      title="Edit notebook"
      width="max-w-lg"
    >
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
        <NotebookLookFields
          icon={icon}
          color={color}
          onIcon={setIcon}
          onColor={setColor}
        />
        {notebook && (
          <CoverPicker
            notebookId={notebook.id}
            color={color}
            cover={cover}
            onCover={setCover}
          />
        )}
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

/** How many pictures each style shows per page of the picker. */
const COVER_PAGE = 4;

/** The cover row: a tile per picture in each of the three styles, in the
 *  notebook's colour, drawn by the same renderer the shelf uses so what is
 *  picked is what is shown. Pictures are seeded names (the id, then the id
 *  with a suffix), so "More" turns a page of new ones and a choice survives
 *  as a string. "Automatic" is the shelf's own spread. */
function CoverPicker({
  notebookId,
  color,
  cover,
  onCover,
}: {
  notebookId: string;
  color: string;
  cover: string;
  onCover: (cover: string) => void;
}) {
  const [page, setPage] = useState(0);
  const seeds = Array.from({ length: COVER_PAGE }, (_, i) => {
    const n = page * COVER_PAGE + i;
    return n === 0 ? notebookId : `${notebookId}-${n}`;
  });
  const auto = coverChoice(notebookId);
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <span className="text-caption font-semibold uppercase tracking-[0.04em] text-muted-foreground">
          Cover
        </span>
        <div className="flex items-center gap-2">
          <button
            type="button"
            aria-pressed={cover === NO_COVER}
            onClick={() => onCover(NO_COVER)}
            className={cn(
              "rounded-md px-2 py-0.5 text-caption transition-colors",
              cover === NO_COVER
                ? "bg-primary/10 text-foreground"
                : "text-muted-foreground hover:text-foreground",
            )}
            title="No cover on the card or the notebook page"
          >
            None
          </button>
          <button
            type="button"
            aria-pressed={cover === ""}
            onClick={() => onCover("")}
            className={cn(
              "rounded-md px-2 py-0.5 text-caption transition-colors",
              cover === ""
                ? "bg-primary/10 text-foreground"
                : "text-muted-foreground hover:text-foreground",
            )}
            title={`The shelf's own choice: ${COVER_STYLE_LABEL[auto.style]}`}
          >
            Automatic
          </button>
          <button
            type="button"
            onClick={() => setPage((p) => p + 1)}
            className="rounded-md px-2 py-0.5 text-caption text-muted-foreground transition-colors hover:text-foreground"
          >
            More…
          </button>
        </div>
      </div>
      {COVER_STYLES.map((style) => (
        <div key={style} className="flex items-center gap-2">
          <span className="w-12 shrink-0 text-caption text-muted-foreground">
            {COVER_STYLE_LABEL[style]}
          </span>
          <div className="grid flex-1 grid-cols-4 gap-2">
            {seeds.map((seed) => (
              <CoverTile
                key={`${style}:${seed}`}
                seed={seed}
                style={style}
                color={color}
                active={cover === `${style}:${seed}`}
                onPick={() => onCover(`${style}:${seed}`)}
              />
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

function CoverTile({
  seed,
  style,
  color,
  active,
  onPick,
}: {
  seed: string;
  style: CoverStyle;
  color: string;
  active: boolean;
  onPick: () => void;
}) {
  // Rendered at the thumb's own aspect, half size; the choice string is
  // passed so the hook draws exactly this style and picture.
  const url = useNotebookCover(seed, color, 106, 70, `${style}:${seed}`);
  return (
    <button
      type="button"
      aria-pressed={active}
      aria-label={`${COVER_STYLE_LABEL[style]} cover ${seed.slice(-8)}`}
      onClick={onPick}
      className={cn(
        "h-[70px] rounded-md border bg-surface-2 transition-shadow",
        active
          ? "border-primary/60 ring-2 ring-foreground ring-offset-1 ring-offset-surface"
          : "border-border hover:border-border-strong",
      )}
      style={
        url
          ? { backgroundImage: `url(${url})`, backgroundSize: "100% 100%" }
          : undefined
      }
    />
  );
}

/** The look half of the dialog — the color row and the icon grid — shared
 *  with the New notebook dialog so both offer the same choice up front.
 *  `autoIcon` relabels the empty choice: on an existing notebook "" is the
 *  plain book, on a new one it means "pick from the title". */
export function NotebookLookFields({
  icon,
  color,
  onIcon,
  onColor,
  autoIcon = false,
}: {
  icon: string;
  color: string;
  onIcon: (icon: string) => void;
  onColor: (color: string) => void;
  autoIcon?: boolean;
}) {
  return (
    <>
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
          onClick={() => onColor(c)}
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
                name
                  ? `Icon: ${name.replace(/-/g, " ")}`
                  : autoIcon
                    ? "Icon from the title"
                    : "Default icon"
              }
              title={
                name ? name.replace(/-/g, " ") : autoIcon ? "Auto" : "Default"
              }
              onClick={() => onIcon(name)}
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
    </>
  );
}
