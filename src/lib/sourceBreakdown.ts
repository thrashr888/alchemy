/* What the notebook is made of, by source type (alchemy-release-2j9).

   The Sources panel's size strip says how much text a notebook holds. This
   says what that text is: a GitHub-Languages-style split of the source list
   into typed slices, one segment each, biggest first.

   Share is by source COUNT, not by characters. A folder, a cloud
   placeholder, and a still-indexing import all carry zero characters, and a
   size-weighted bar would erase them from a panel whose own row list shows
   them. "Nine of your twelve sources are web pages" is also the question the
   strip's neighbors already answer in counts.

   Types are folded the way the row icons already fold them (sourceIcon.tsx):
   url/html read as Web, and office files -- which extraction flattens into
   plain text sources, so only the path remembers -- read as Office. Folder
   children are classified by their own file type, since that is what they
   are. Anything too small to earn a legend line joins Other. */
import type { Source } from "./types";

/** One segment of the bar and one legend line. */
export interface BreakdownSlice {
  /** Stable key for React and for tests; "other" for the folded tail. */
  key: string;
  label: string;
  count: number;
  /** Characters indexed across this slice; 0 for folders and placeholders. */
  chars: number;
  /** Percent of the notebook's sources, 0-100, unrounded. */
  share: number;
}

type Row = Pick<Source, "sourceType" | "charCount"> & { url?: string };

// Application families recognized by extension, mirroring sourceIcon.tsx --
// docx/pptx/xlsx land as "text" sources, so the file path is the only
// surviving evidence of where they came from.
const OFFICE_EXTS = new Set([
  "doc", "docx", "docm", "rtf", "odt", "gdoc",
  "ppt", "pptx", "pptm", "odp", "gslides", "key",
  "xls", "xlsx", "xlsm", "xlsb", "ods", "csv", "tsv", "gsheet",
]);

const LABELS: Record<string, string> = {
  web: "Web",
  pdf: "PDF",
  markdown: "Markdown",
  text: "Text",
  code: "Code",
  office: "Office",
  image: "Images",
  folder: "Folders",
  mac: "Apple",
  feed: "Feeds",
  git: "Git",
  notion: "Notion",
  obsidian: "Obsidian",
  okf: "OpenKnowledge",
  other: "Other",
};

/** Extension of a local file path; "" for web and cider URLs. */
function fileExt(url?: string): string {
  if (!url || /^[a-z][a-z0-9+.-]*:\/\//i.test(url)) return "";
  const m = /\.([a-z0-9]+)$/i.exec(url);
  return m ? m[1].toLowerCase() : "";
}

/** The slice a single source belongs to. */
export function breakdownKey(s: Row): string {
  if (s.sourceType === "url" || s.sourceType === "html") return "web";
  if (s.sourceType === "text" && OFFICE_EXTS.has(fileExt(s.url)))
    return "office";
  return s.sourceType;
}

export function breakdownLabel(key: string): string {
  return LABELS[key] ?? key;
}

export interface BreakdownOptions {
  /** Slices at or above this percent always keep their own line. */
  minShare?: number;
  /** Most lines to draw before the tail folds into Other. */
  limit?: number;
}

/** Sources split by type, share-descending, rare types folded into Other.
 *  Other sorts last however big it is, so the tail never leads the legend. */
export function sourceBreakdown(
  sources: readonly Row[],
  { minShare = 2, limit = 7 }: BreakdownOptions = {},
): BreakdownSlice[] {
  if (sources.length === 0) return [];
  const acc = new Map<string, { count: number; chars: number }>();
  for (const s of sources) {
    const key = breakdownKey(s);
    const cur = acc.get(key) ?? { count: 0, chars: 0 };
    cur.count += 1;
    cur.chars += s.charCount ?? 0;
    acc.set(key, cur);
  }
  const total = sources.length;
  const ranked = [...acc.entries()]
    .map(([key, v]) => ({
      key,
      label: breakdownLabel(key),
      count: v.count,
      chars: v.chars,
      share: (v.count / total) * 100,
    }))
    .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));

  const kept: BreakdownSlice[] = [];
  const folded: BreakdownSlice[] = [];
  for (const slice of ranked) {
    if (kept.length < limit && slice.share >= minShare) kept.push(slice);
    else folded.push(slice);
  }
  // A one-type tail keeps its own name: calling a single slice "Other"
  // hides a fact the card has room to state.
  if (folded.length === 1) return [...kept, folded[0]];
  if (folded.length === 0) return kept;
  return [
    ...kept,
    {
      key: "other",
      label: LABELS.other,
      count: folded.reduce((n, f) => n + f.count, 0),
      chars: folded.reduce((n, f) => n + f.chars, 0),
      share: folded.reduce((n, f) => n + f.share, 0),
    },
  ];
}

/** Percent to one decimal, with the trailing ".0" kept so the legend column
 *  stays even. Shares under 0.05% would round to nothing, so they floor at
 *  "<0.1%" rather than reading as absent. */
export function formatShare(share: number): string {
  if (share > 0 && share < 0.05) return "<0.1%";
  return `${share.toFixed(1)}%`;
}
