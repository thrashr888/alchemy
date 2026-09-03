// Shared source-row icon. Lives outside the component files so SourcesPanel,
// SidebarRails, AmbientRail, and ReaderPane can all use it while keeping
// their own exports components-only — Vite Fast Refresh bails ("hmr
// invalidate") on any module mixing component and non-component exports.
import type { Source } from "@/lib/types";
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

export function sourceIcon(t: Source["sourceType"], url?: string) {
  // Mac sources show the app they mirror (same icons as the add-source
  // modal's provider tiles), in that app's signature color.
  if (t === "mac" && url) {
    if (url.startsWith("cider://calendar/"))
      return <Calendar className="h-3.5 w-3.5 text-muted-foreground" />;
    if (url.startsWith("cider://reminders/"))
      return <ListChecks className="h-3.5 w-3.5 text-muted-foreground" />;
    if (url.startsWith("cider://notes/"))
      return <NotebookText className="h-3.5 w-3.5 text-muted-foreground" />;
    if (url.startsWith("cider://stocks/"))
      return <TrendingUp className="h-3.5 w-3.5 text-muted-foreground" />;
  }
  // File-backed sources show the application family the file came from —
  // Word, PowerPoint, Excel, Box, EPUB — not just "text".
  const ext = fileExt(url);
  if (WORD_EXTS.has(ext))
    return <FileType2 className="h-3.5 w-3.5 text-muted-foreground" />;
  if (SLIDES_EXTS.has(ext))
    return <Presentation className="h-3.5 w-3.5 text-muted-foreground" />;
  if (SHEET_EXTS.has(ext))
    return <FileSpreadsheet className="h-3.5 w-3.5 text-muted-foreground" />;
  if (ext === "epub")
    return <BookOpen className="h-3.5 w-3.5 text-muted-foreground" />;
  if (ext === "boxnote")
    return <Box className="h-3.5 w-3.5 text-muted-foreground" />;
  switch (t) {
    case "git":
      return <GitBranch className="h-3.5 w-3.5 text-muted-foreground" />;
    case "notion":
      return <Blocks className="h-3.5 w-3.5 text-muted-foreground" />;
    case "obsidian":
      return <Gem className="h-3.5 w-3.5 text-muted-foreground" />;
    case "okf":
      return <Library className="h-3.5 w-3.5 text-muted-foreground" />;
    case "code":
      return <FileCode className="h-3.5 w-3.5 text-muted-foreground" />;
    case "pdf":
      return <FileType className="h-3.5 w-3.5 text-muted-foreground" />;
    case "url":
      return <Globe className="h-3.5 w-3.5 text-muted-foreground" />;
    case "markdown":
      return <Hash className="h-3.5 w-3.5 text-muted-foreground" />;
    case "image":
      return <ImageIcon className="h-3.5 w-3.5 text-muted-foreground" />;
    case "folder":
      return <Folder className="h-3.5 w-3.5 text-muted-foreground" />;
    case "mac":
      return <Command className="h-3.5 w-3.5 text-muted-foreground" />;
    case "html":
      return <CodeXml className="h-3.5 w-3.5 text-muted-foreground" />;
    default:
      return <FileText className="h-3.5 w-3.5 text-muted-foreground" />;
  }
}
