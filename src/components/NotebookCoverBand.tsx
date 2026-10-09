import { useNotebookCover } from "@/lib/cover";

/** The notebook's cover across the top of its page: the same picture the
 *  Home card wears, drawn wide and thin under the toolbar and fading into
 *  the pane, so the notebook is recognisable before a word is read. No
 *  text on it — the title is the toolbar's. */
export function NotebookCoverBand({
  id,
  color,
  choice,
}: {
  id: string;
  color?: string;
  /** `Notebook.cover`: the stored choice, or "" for automatic. */
  choice?: string;
}) {
  const cover = useNotebookCover(id, color || "", 1200, 56, choice);
  return (
    <div
      aria-hidden
      className="pointer-events-none relative h-14 w-full shrink-0 overflow-hidden"
      style={
        cover
          ? {
              backgroundImage: `url(${cover})`,
              backgroundSize: "cover",
              backgroundPosition: "center",
            }
          : undefined
      }
    >
      <div
        className="absolute inset-0"
        style={{
          background:
            "linear-gradient(to bottom, transparent 35%, var(--background))",
        }}
      />
    </div>
  );
}
