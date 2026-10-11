import { BAND_H, BAND_W, useNotebookCover } from "@/lib/cover";

/** The notebook's cover as a backdrop behind the top of its page: the same
 *  picture the Home card wears, drawn wide, pinned to the sheet's top and
 *  fading to nothing, so the pane's own header and content paint over it
 *  from the very top instead of being pushed down. No text on it — the
 *  title is the toolbar's. Nothing at all when the cover is `none`.
 *
 *  The fade is a mask, not a gradient to the background colour, so the
 *  translucent sheet under glass shows through the same way it does
 *  everywhere else. Render it before the pane: the panes' roots are
 *  positioned, so tree order puts them above it. */
export function NotebookCoverBand({
  id,
  color,
  choice,
}: {
  id: string;
  color?: string;
  /** `Notebook.cover`: the stored choice, "" for automatic, "none". */
  choice?: string;
}) {
  const cover = useNotebookCover(id, color || "", BAND_W, BAND_H, choice);
  if (!cover) return null;
  const fade = "linear-gradient(to bottom, black 40%, transparent)";
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute inset-x-0 top-0 z-0"
      style={{
        height: BAND_H,
        backgroundImage: `url(${cover})`,
        backgroundSize: "cover",
        backgroundPosition: "center",
        WebkitMaskImage: fade,
        maskImage: fade,
      }}
    />
  );
}
