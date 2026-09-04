/* The Sources panel's facet axis, as pure functions over the live source
   list (alchemy-release-zhk).

   Every number a facet chip shows and every value a facet can hold is
   derived here from the array the panel is rendering right now. The bug
   this fixes had two faces and one cause — a facet that outlived the
   sources it described: removing the last Web source dropped the chip but
   left `kindFacet = "web"` in force, so the list stayed filtered with
   nothing left to click. Counts and selections come from the same pass, so
   they cannot disagree. */
import type { Source } from "./types";

/** Source types that are containers: their children carry the text, they
 *  carry none. Rows indent under them and they sit out of content counts. */
export const FOLDER_TYPES = [
  "folder",
  "git",
  "notion",
  "obsidian",
  "okf",
  "feed",
];

/** Coarse buckets a narrow panel can offer as chips. */
export type SourceKind = "web" | "files" | "images" | "apple" | "folders";

export const KIND_LABEL: Record<SourceKind, string> = {
  web: "Web",
  files: "Files",
  images: "Images",
  apple: "Apple",
  folders: "Folders",
};

export function sourceKind(s: Pick<Source, "sourceType">): SourceKind {
  if (FOLDER_TYPES.includes(s.sourceType)) return "folders";
  if (s.sourceType === "url" || s.sourceType === "html") return "web";
  if (s.sourceType === "image") return "images";
  if (s.sourceType === "mac") return "apple";
  return "files";
}

/** How many sources of each kind the notebook holds right now. A kind with
 *  no sources is absent, which is also what makes its chip disappear. */
export function kindCounts(
  sources: readonly Pick<Source, "sourceType">[],
): Map<SourceKind, number> {
  const m = new Map<SourceKind, number>();
  for (const s of sources) {
    const k = sourceKind(s);
    m.set(k, (m.get(k) ?? 0) + 1);
  }
  return m;
}

/** Tag chips, count-desc then alphabetical, capped — plus `keep` wherever it
 *  ranks, so the tag currently filtering the list is always on screen to
 *  turn off. */
export function tagCounts(
  sources: readonly Pick<Source, "tags">[],
  keep: string | null = null,
  limit = 6,
): [string, number][] {
  const m = new Map<string, number>();
  for (const s of sources)
    for (const t of s.tags.split(" ")) if (t) m.set(t, (m.get(t) ?? 0) + 1);
  const ranked = [...m.entries()].sort(
    (a, b) => b[1] - a[1] || a[0].localeCompare(b[0]),
  );
  const top = ranked.slice(0, limit);
  if (keep && m.has(keep) && !top.some(([t]) => t === keep))
    top.push([keep, m.get(keep)!]);
  return top;
}

/** A selection is only in force while the thing it selects still exists.
 *  Returns null once the last source under it is gone, which unfilters the
 *  list in the same render that drops the chip. */
export function liveFacet<T extends string>(
  selected: T | null,
  available: { has(value: T): boolean },
): T | null {
  if (selected === null) return null;
  return available.has(selected) ? selected : null;
}

/** Sources whose bytes aren't here: the file was moved or deleted out from
 *  under the notebook (the hygiene sweep's `missing-file`), or it is a cloud
 *  stub that has never been downloaded (`status: "placeholder"` — iCloud,
 *  Dropbox, Google Drive). Two causes, one question the reader is asking:
 *  which of these can't I actually read? (alchemy-release-0z2) */
export function missingSourceIds(
  sources: readonly Pick<Source, "id" | "status">[],
  hygiene: readonly { sourceId: string; bucket: string }[],
): Set<string> {
  const known = new Set(sources.map((s) => s.id));
  const out = new Set<string>();
  for (const s of sources) if (s.status === "placeholder") out.add(s.id);
  for (const h of hygiene)
    if (h.bucket === "missing-file" && known.has(h.sourceId)) out.add(h.sourceId);
  return out;
}
