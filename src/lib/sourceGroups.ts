/**
 * How sources are grouped for filtering, and what colour each group reads as.
 *
 * Extracted from GalleryPane when the graph became the second surface that
 * needed to filter by type — two copies of this mapping would drift the
 * first time a source type is added.
 */
import type { Source } from "./types";

export type TypeGroup =
  | "all"
  | "urls"
  | "docs"
  | "images"
  | "text"
  | "code"
  | "mac"
  | "folders"
  /** Graph only: notes are nodes there, but they are not sources and never
   *  appear in the gallery's grid. */
  | "notes";

export const GROUP_OF: Record<Source["sourceType"], Exclude<TypeGroup, "all">> =
  {
    url: "urls",
    html: "text",
    pdf: "docs",
    image: "images",
    text: "text",
    markdown: "text",
    code: "code",
    mac: "mac",
    folder: "folders",
    git: "folders",
    notion: "folders",
    obsidian: "folders",
    feed: "folders",
  };

export const GROUP_LABEL: Record<Exclude<TypeGroup, "all">, string> = {
  // "URLs", not "Pages": this group is exactly the web-link sources, and
  // "Pages" read ambiguously next to the PDF group (paper pages? text?).
  urls: "URLs",
  docs: "PDFs",
  images: "Images",
  text: "Text",
  code: "Code",
  mac: "Mac",
  folders: "Folders",
  notes: "Notes",
};

/**
 * One colour per group, for surfaces where type is worth seeing at a glance
 * — currently the graph, where a node has no room for an icon or a label.
 *
 * Hexes rather than CSS tokens, and deliberately: these are categorical
 * identity colours, the same kind of thing as NOTEBOOK_PALETTE (which is
 * itself kept in sync with Rust). A semantic token means "this is a border"
 * or "this is danger"; there is no semantic token for "this one is a PDF".
 * The hues are drawn from the same family as the notebook palette so the app
 * stays coherent, and they are stated once, here, not in a component —
 * which is what DESIGN.md's rule is actually protecting against.
 *
 * Eight hues, evenly spaced around the wheel at one saturation and
 * lightness. Picked by search rather than taste: hand-assigning from the
 * notebook palette twice produced a pair only ~42 apart in RGB (two mid
 * blues, then a teal and a green), which at node size reads as one group.
 * Even spacing maximises the thing that actually matters for a categorical
 * encoding — hue difference — and lifts the closest pair to 63.
 *
 * Contrast is 4.7:1 against a dark background and 1.8:1 against white. The
 * low light-theme figure is deliberate and safe here: these fill small
 * circles that also carry a foreground-derived stroke, so the outline gives
 * the shape its definition and the fill only has to say which group. Do not
 * reuse these for text.
 */
export const GROUP_COLOR: Record<Exclude<TypeGroup, "all">, string> = {
  docs: "#d16161",
  text: "#d1b561",
  mac: "#99d161",
  notes: "#61d17d",
  code: "#61d1d1",
  urls: "#617dd1",
  folders: "#9961d1",
  images: "#d161b5",
};

/** The group a graph node belongs to — notes are their own, sources map by
 *  type. Kept here so the graph and the grid agree on what a "PDF" is. */
export function groupOfNode(kind: string, sourceType: string): TypeGroup {
  if (kind === "note") return "notes";
  return GROUP_OF[sourceType as Source["sourceType"]] ?? "text";
}
