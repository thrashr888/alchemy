import { useStore } from "@/lib/store";
import { SourcesPanel } from "./SourcesPanel";
import { ChatPanel } from "./ChatPanel";
import { StudioPanel } from "./StudioPanel";
import { HealthBanner } from "./HealthBanner";
import { Button } from "./ui";
import { ChevronLeft, Settings, BookOpen } from "lucide-react";

export function Workspace({ onOpenSettings }: { onOpenSettings: () => void }) {
  const currentId = useStore((s) => s.currentId);
  const notebooks = useStore((s) => s.notebooks);
  const close = useStore((s) => s.closeNotebook);

  const notebook = notebooks.find((n) => n.id === currentId);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
      <header className="flex items-center gap-2 px-3 h-12 border-b border-border">
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
        <div className="ml-auto">
          <Button variant="ghost" size="icon" onClick={onOpenSettings} title="Settings">
            <Settings className="h-4 w-4" />
          </Button>
        </div>
      </header>

      <HealthBanner onOpenSettings={onOpenSettings} />

      <div className="flex flex-1 overflow-hidden">
        <SourcesPanel />
        <ChatPanel />
        <StudioPanel />
      </div>
    </div>
  );
}
