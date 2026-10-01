// Shared source-row icon. Lives outside the component files so SourcesPanel,
// SidebarRails, AmbientRail, and ReaderPane can all use it while keeping
// their own exports components-only — Vite Fast Refresh bails ("hmr
// invalidate") on any module mixing component and non-component exports.
import type { Source } from "@/lib/types";
import type { LucideIcon } from "lucide-react";
import {
  Blocks,
  BookOpen,
  Box,
  Calendar,
  CodeXml,
  Command,
  FileCode,
  FileSpreadsheet,
  FileText,
  FileType,
  FileType2,
  Folder,
  Gem,
  GitBranch,
  Globe,
  Hash,
  Image as ImageIcon,
  Library,
  ListChecks,
  NotebookText,
  Presentation,
  TrendingUp,
} from "lucide-react";

// Application families recognized by file extension. Extraction flattens
// docx/pptx/xlsx and friends into generic text sources, so the origin app
// survives only in the path — sniff it back for the row icon.
const WORD_EXTS = new Set(["doc", "docx", "docm", "rtf", "odt", "gdoc"]);
const SLIDES_EXTS = new Set(["ppt", "pptx", "pptm", "odp", "gslides", "key"]);
const SHEET_EXTS = new Set([
  "xls", "xlsx", "xlsm", "xlsb", "ods", "csv", "tsv", "gsheet",
]);

/** Extension of a local file path; "" for web/cider URLs and bare titles. */
function fileExt(url?: string): string {
  if (!url || /^[a-z][a-z0-9+.-]*:\/\//i.test(url)) return "";
  const m = /\.([a-z0-9]+)$/i.exec(url);
  return m ? m[1].toLowerCase() : "";
}

/** Which glyph a source wears. Separate from `sourceIcon` because Home's
 *  cards draw the same type vocabulary at 12px rather than the rows' 14px —
 *  the CHOICE is shared, the size is the caller's. */
export function sourceGlyph(
  t: Source["sourceType"],
  url?: string,
): LucideIcon {
  // Mac sources show the app they mirror (same icons as the add-source
  // modal's provider tiles).
  if (t === "mac" && url) {
    if (url.startsWith("cider://calendar/")) return Calendar;
    if (url.startsWith("cider://reminders/")) return ListChecks;
    if (url.startsWith("cider://notes/")) return NotebookText;
    if (url.startsWith("cider://stocks/")) return TrendingUp;
  }
  // File-backed sources show the application family the file came from —
  // Word, PowerPoint, Excel, Box, EPUB — not just "text".
  const ext = fileExt(url);
  if (WORD_EXTS.has(ext)) return FileType2;
  if (SLIDES_EXTS.has(ext)) return Presentation;
  if (SHEET_EXTS.has(ext)) return FileSpreadsheet;
  if (ext === "epub") return BookOpen;
  if (ext === "boxnote") return Box;
  switch (t) {
    case "git":
      return GitBranch;
    case "notion":
      return Blocks;
    case "obsidian":
      return Gem;
    case "okf":
      return Library;
    case "code":
      return FileCode;
    case "pdf":
      return FileType;
    case "url":
      return Globe;
    case "markdown":
      return Hash;
    case "image":
      return ImageIcon;
    case "folder":
      return Folder;
    case "mac":
      return Command;
    case "html":
      return CodeXml;
    default:
      return FileText;
  }
}

export function sourceIcon(t: Source["sourceType"], url?: string) {
  const Glyph = sourceGlyph(t, url);
  return <Glyph className="h-3.5 w-3.5 text-muted-foreground" />;
}
