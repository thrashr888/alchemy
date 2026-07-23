// Shared source-row icon. Lives outside the component files so SourcesPanel,
// SidebarRails, AmbientRail, and ReaderPane can all use it while keeping
// their own exports components-only — Vite Fast Refresh bails ("hmr
// invalidate") on any module mixing component and non-component exports.
import type { Source } from "@/lib/types";
import {
  Calendar,
  CodeXml,
  Command,
  FileCode,
  FileText,
  FileType,
  Folder,
  GitBranch,
  Globe,
  Hash,
  Image as ImageIcon,
  ListChecks,
  NotebookText,
  TrendingUp,
} from "lucide-react";

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
  switch (t) {
    case "git":
      return <GitBranch className="h-3.5 w-3.5 text-muted-foreground" />;
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
