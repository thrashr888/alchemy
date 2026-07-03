import { useStore } from "@/lib/store";
import { SourcesPanel } from "./SourcesPanel";
import { ChatPanel } from "./ChatPanel";
import { StudioPanel } from "./StudioPanel";
import { SourcesRail, StudioRail } from "./SidebarRails";
import { HealthBanner } from "./HealthBanner";
import { Button } from "./ui";
import { cn } from "@/lib/utils";
import { ChevronLeft, Settings, BookOpen, PanelLeft, PanelRight } from "lucide-react";

export function Workspace({ onOpenSettings }: { onOpenSettings: () => void }) {
  const currentId = useStore((s) => s.currentId);
  const notebooks = useStore((s) => s.notebooks);
  const close = useStore((s) => s.closeNotebook);
  const sourcesOpen = useStore((s) => s.sourcesOpen);
  const studioOpen = useStore((s) => s.studioOpen);
  const toggleSources = useStore((s) => s.toggleSources);
  const toggleStudio = useStore((s) => s.toggleStudio);

  const notebook = notebooks.find((n) => n.id === currentId);

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
        <button
          onClick={toggleSources}
          title={sourcesOpen ? "Hide sources" : "Show sources"}
          className={cn(
            "rounded-md p-1.5 transition-colors",
            sourcesOpen ? "text-foreground" : "text-subtle-foreground hover:text-foreground",
          )}
        >
          <PanelLeft className="h-4 w-4" />
        </button>
        <div className="flex items-center gap-1.5 min-w-0">
          <BookOpen className="h-3.5 w-3.5 shrink-0 text-primary" />
          <span className="truncate text-[13px] font-semibold" title={notebook?.title}>
            {notebook?.title ?? "Notebook"}
          </span>
        </div>
        <div className="ml-auto flex items-center gap-1">
          <button
            onClick={toggleStudio}
            title={studioOpen ? "Hide studio" : "Show studio"}
            className={cn(
              "rounded-md p-1.5 transition-colors",
              studioOpen ? "text-foreground" : "text-subtle-foreground hover:text-foreground",
            )}
          >
            <PanelRight className="h-4 w-4" />
          </button>
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
    </div>
  );
}
