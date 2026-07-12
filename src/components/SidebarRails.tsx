import { useEffect, useRef, useState } from "react";
import { useStore } from "@/lib/store";
import { sourceIcon, MenuItem, useCloseOnOutside } from "./SourcesPanel";
import { cn } from "@/lib/utils";
import {
  PanelLeft,
  PanelRight,
  Plus,
  Wand2,
  StickyNote,
  AlertCircle,
  Upload,
  FolderOpen,
  Link2,
  ClipboardPaste,
} from "lucide-react";

/**
 * Thin icon rail shown when the Sources panel is collapsed — mirrors
 * NotebookLM: each source's type icon stacked vertically; click anything to
 * reopen the panel. The + keeps the full add-source menu available: file and
 * folder picks run in place, while the URL/text forms (modals that live in
 * the expanded panel) reopen the panel with the right form up.
 */
export function SourcesRail() {
  const sources = useStore((s) => s.sources);
  const toggleSources = useStore((s) => s.toggleSources);
  const currentId = useStore((s) => s.currentId);
  const pickAndAddFiles = useStore((s) => s.pickAndAddFiles);
  const pickAndAddFolder = useStore((s) => s.pickAndAddFolder);

  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTriggerRef = useRef<HTMLButtonElement>(null);

  // Same menu keyboard behavior as the expanded panel: focus first item on
  // open, arrows cycle, Escape closes.
  useEffect(() => {
    if (menuOpen) menuRef.current?.querySelector<HTMLElement>("button")?.focus();
  }, [menuOpen]);
  useCloseOnOutside(menuOpen, () => setMenuOpen(false), menuRef, menuTriggerRef);

  function onMenuKey(e: React.KeyboardEvent) {
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? []);
    const idx = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") {
      e.stopPropagation();
      setMenuOpen(false);
      menuTriggerRef.current?.focus();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      items[(idx + 1) % items.length]?.focus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      items[(idx - 1 + items.length) % items.length]?.focus();
    }
  }

  return (
    <div className="flex w-12 shrink-0 flex-col items-center border-r border-border bg-surface py-2">
      <button
        onClick={toggleSources}
        title="Show sources"
        className="rounded-md p-1.5 text-subtle-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
      >
        <PanelLeft className="h-4 w-4" />
      </button>
      <div className="my-1.5 h-px w-6 bg-border" />
      <div className="relative">
        <button
          ref={menuTriggerRef}
          onClick={() => setMenuOpen((o) => !o)}
          disabled={!currentId}
          title="Add source"
          aria-label="Add source"
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          className="rounded-md p-1.5 text-subtle-foreground transition-colors hover:bg-surface-2 hover:text-foreground disabled:opacity-40"
        >
          <Plus className="h-4 w-4" />
        </button>
        {menuOpen && (
          <>
            <div className="fixed inset-0 z-10" onClick={() => setMenuOpen(false)} />
            <div
              ref={menuRef}
              role="menu"
              aria-label="Add source"
              onKeyDown={onMenuKey}
              className="absolute left-full top-0 z-20 ml-1 w-44 overflow-hidden rounded-md bg-elevated py-1 shadow-[0_0_0_0.5px_var(--border-strong),0_8px_24px_-6px_rgba(0,0,0,0.4)]"
            >
              <MenuItem
                icon={<Upload className="h-3.5 w-3.5" />}
                label="Upload files"
                onClick={() => {
                  setMenuOpen(false);
                  void pickAndAddFiles();
                }}
              />
              <MenuItem
                icon={<FolderOpen className="h-3.5 w-3.5" />}
                label="Add folder"
                onClick={() => {
                  setMenuOpen(false);
                  void pickAndAddFolder();
                }}
              />
              <MenuItem
                icon={<Link2 className="h-3.5 w-3.5" />}
                label="From URL"
                onClick={() => {
                  setMenuOpen(false);
                  useStore.setState({ pendingAddUrl: true });
                  toggleSources();
                }}
              />
              <MenuItem
                icon={<ClipboardPaste className="h-3.5 w-3.5" />}
                label="Paste text"
                onClick={() => {
                  setMenuOpen(false);
                  useStore.setState({ pendingAddText: true });
                  toggleSources();
                }}
              />
            </div>
          </>
        )}
      </div>
      <div className="flex min-h-0 flex-1 flex-col items-center gap-0.5 overflow-y-auto">
        {sources.map((s) => (
          <button
            key={s.id}
            onClick={toggleSources}
            title={s.title}
            className="relative rounded-md p-1.5 transition-colors hover:bg-surface-2"
          >
            {sourceIcon(s.sourceType)}
            {s.status === "error" && (
              <AlertCircle className="absolute -right-0 -top-0 h-2.5 w-2.5 text-destructive" />
            )}
          </button>
        ))}
      </div>
    </div>
  );
}

/** Thin icon rail shown when the Studio panel is collapsed. */
export function StudioRail() {
  const notes = useStore((s) => s.notes);
  const toggleStudio = useStore((s) => s.toggleStudio);
  return (
    <div className="flex w-12 shrink-0 flex-col items-center border-l border-border bg-surface py-2">
      <button
        onClick={toggleStudio}
        title="Show studio"
        className="rounded-md p-1.5 text-subtle-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
      >
        <PanelRight className="h-4 w-4" />
      </button>
      <div className="my-1.5 h-px w-6 bg-border" />
      <button
        onClick={toggleStudio}
        title="Generate documents"
        className="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
      >
        <Wand2 className="h-4 w-4" />
      </button>
      <button
        onClick={toggleStudio}
        title={`Notes${notes.length ? ` (${notes.length})` : ""}`}
        className={cn(
          "relative rounded-md p-1.5 transition-colors hover:bg-surface-2",
          notes.length ? "text-muted-foreground hover:text-foreground" : "text-subtle-foreground",
        )}
      >
        <StickyNote className="h-4 w-4" />
        {notes.length > 0 && (
          <span className="absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-primary/20 px-0.5 text-[10px] font-medium text-citation">
            {notes.length}
          </span>
        )}
      </button>
      <button
        onClick={toggleStudio}
        title="Add note"
        className="mt-auto rounded-md p-1.5 text-subtle-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
      >
        <Plus className="h-4 w-4" />
      </button>
    </div>
  );
}
