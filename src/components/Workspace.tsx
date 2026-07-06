import { useEffect } from "react";
import { useStore } from "@/lib/store";
import { SourcesPanel } from "./SourcesPanel";
import { ChatPanel } from "./ChatPanel";
import { StudioPanel } from "./StudioPanel";
import { SourceViewer } from "./SourceViewer";
import { SourcesRail, StudioRail } from "./SidebarRails";
import { HealthBanner } from "./HealthBanner";
import { Button } from "./ui";
import { shortcutBlocked } from "@/lib/utils";
import { ChevronLeft, Search, Settings, BookOpen } from "lucide-react";

export function Workspace({ onOpenSettings }: { onOpenSettings: () => void }) {
  const currentId = useStore((s) => s.currentId);
  const notebooks = useStore((s) => s.notebooks);
  const close = useStore((s) => s.closeNotebook);
  const sourcesOpen = useStore((s) => s.sourcesOpen);
  const studioOpen = useStore((s) => s.studioOpen);

  const notebook = notebooks.find((n) => n.id === currentId);

  // Panel + note shortcuts: Cmd+1 sources, Cmd+2 studio, Cmd+N new note
  // (opening the studio panel first when it's collapsed).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || shortcutBlocked(e)) return;
      const { studioOpen, toggleSources, toggleStudio } = useStore.getState();
      if (e.key === "1") {
        e.preventDefault();
        toggleSources();
      } else if (e.key === "2") {
        e.preventDefault();
        toggleStudio();
      } else if (e.key === "n" && !studioOpen) {
        e.preventDefault();
        // Open the panel; StudioPanel opens the composer when it mounts.
        useStore.setState({ pendingNewNote: true });
        toggleStudio();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
      <header
        data-tauri-drag-region
        className="flex items-center gap-2 h-12 border-b border-border pl-[84px] pr-3"
      >
        <Button variant="ghost" size="sm" onClick={close} title="Back to notebooks">
          <ChevronLeft className="h-4 w-4" />
          Notebooks
        </Button>
        <div className="mx-1 h-4 w-px bg-border" />
        <div className="flex items-center gap-1.5 min-w-0">
          <BookOpen className="h-3.5 w-3.5 shrink-0 text-primary" />
          <span className="truncate text-[13px] font-semibold" title={notebook?.title}>
            {notebook?.title ?? "Notebook"}
          </span>
        </div>
        <div className="ml-auto flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => useStore.getState().setPaletteOpen(true)}
            title="Search & commands (⌘K)"
            aria-label="Open the command menu"
          >
            <Search className="h-4 w-4" />
          </Button>
          <Button variant="ghost" size="icon" onClick={onOpenSettings} title="Settings">
            <Settings className="h-4 w-4" />
          </Button>
        </div>
      </header>

      <HealthBanner onOpenSettings={onOpenSettings} />

      <div className="flex flex-1 overflow-hidden">
        {sourcesOpen ? <SourcesPanel /> : <SourcesRail />}
        <ChatPanel />
        {studioOpen ? <StudioPanel /> : <StudioRail />}
      </div>

      <SourceViewer />
    </div>
  );
}
