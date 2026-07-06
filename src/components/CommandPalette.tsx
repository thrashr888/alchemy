import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useStore } from "@/lib/store";
import { THEMES } from "@/lib/themes";
import { cn } from "@/lib/utils";
import { ARTIFACTS } from "./StudioPanel";
import { useConfirm } from "./ui";
import {
  BookOpen,
  ChevronLeft,
  Eraser,
  Link2,
  MessageSquare,
  Palette,
  PanelLeft,
  PanelRight,
  Plus,
  Search,
  Settings,
  SquarePen,
  Upload,
  Wand2,
} from "lucide-react";

interface Command {
  id: string;
  group: string;
  label: string;
  /** Extra match terms beyond the label. */
  keywords?: string;
  icon: ReactNode;
  hint?: string;
  run: () => void;
}

/** Cmd+K command menu: search across navigation, sources, and generation. */
export function CommandPalette() {
  const paletteOpen = useStore((s) => s.paletteOpen);
  const setPaletteOpen = useStore((s) => s.setPaletteOpen);
  const currentId = useStore((s) => s.currentId);
  const notebooks = useStore((s) => s.notebooks);
  const agentMode = useStore((s) => s.agentMode);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!paletteOpen) return;
    setQuery("");
    setSelected(0);
    const trigger = document.activeElement as HTMLElement | null;
    // The input mounts in this same render pass.
    requestAnimationFrame(() => inputRef.current?.focus());
    return () => trigger?.focus?.();
  }, [paletteOpen]);

  const commands = useMemo<Command[]>(() => {
    // Read fresh store state at execution time — panel/agent flags may have
    // changed since the palette opened.
    const state = () => useStore.getState();
    const close = () => state().setPaletteOpen(false);
    const list: Command[] = [];

    if (currentId) {
      list.push(
        {
          id: "focus-composer",
          group: "Chat",
          label: "Focus the chat composer",
          keywords: "message ask type",
          icon: <MessageSquare className="h-3.5 w-3.5" />,
          run: () => {
            close();
            window.dispatchEvent(new CustomEvent("nb:focus-composer"));
          },
        },
        {
          id: "agent-mode",
          group: "Chat",
          label: agentMode ? "Agent mode: turn off" : "Agent mode: turn on",
          keywords: "agentic retrieval deep research",
          icon: <Wand2 className="h-3.5 w-3.5" />,
          run: () => {
            state().toggleAgentMode();
            close();
          },
        },
        {
          id: "clear-chat",
          group: "Chat",
          label: "Clear chat history",
          keywords: "delete conversation reset",
          icon: <Eraser className="h-3.5 w-3.5" />,
          run: () => {
            close();
            void (async () => {
              if (await confirm({ title: "Clear this conversation?", confirmLabel: "Clear", danger: true }))
                void state().clearChat();
            })();
          },
        },
        {
          id: "add-files",
          group: "Sources",
          label: "Add sources: upload files…",
          keywords: "import pdf csv image document",
          icon: <Upload className="h-3.5 w-3.5" />,
          run: () => {
            close();
            void state().pickAndAddFiles();
          },
        },
        {
          id: "add-url",
          group: "Sources",
          label: "Add source from URL…",
          keywords: "link website google docs sheets slides",
          icon: <Link2 className="h-3.5 w-3.5" />,
          run: () => {
            close();
            const s = state();
            // Flag, not an event: SourcesPanel may still be mounting.
            useStore.setState({ pendingAddUrl: true });
            if (!s.sourcesOpen) s.toggleSources();
          },
        },
        {
          id: "new-note",
          group: "Studio",
          label: "New note",
          keywords: "write create",
          icon: <SquarePen className="h-3.5 w-3.5" />,
          hint: "⌘N",
          run: () => {
            close();
            const s = state();
            useStore.setState({ pendingNewNote: true });
            if (!s.studioOpen) s.toggleStudio();
          },
        },
        ...ARTIFACTS.map(
          (a): Command => ({
            id: `gen-${a.kind}`,
            group: "Generate",
            label: `Generate ${a.label}`,
            keywords: "artifact note document studio",
            icon: a.icon,
            run: () => {
              close();
              void state().generateArtifact(a.kind);
            },
          }),
        ),
        {
          id: "toggle-sources",
          group: "View",
          label: "Show or hide Sources panel",
          icon: <PanelLeft className="h-3.5 w-3.5" />,
          hint: "⌘1",
          run: () => {
            state().toggleSources();
            close();
          },
        },
        {
          id: "toggle-studio",
          group: "View",
          label: "Show or hide Studio panel",
          icon: <PanelRight className="h-3.5 w-3.5" />,
          hint: "⌘2",
          run: () => {
            state().toggleStudio();
            close();
          },
        },
        {
          id: "close-notebook",
          group: "Navigate",
          label: "Back to all notebooks",
          keywords: "home close exit",
          icon: <ChevronLeft className="h-3.5 w-3.5" />,
          run: () => {
            close();
            state().closeNotebook();
          },
        },
      );
    }

    list.push(
      ...notebooks
        .filter((n) => n.id !== currentId)
        .map(
          (n): Command => ({
            id: `nb-${n.id}`,
            group: "Navigate",
            label: `Open notebook: ${n.title}`,
            keywords: "switch go",
            icon: <BookOpen className="h-3.5 w-3.5" />,
            run: () => {
              close();
              void state().selectNotebook(n.id);
            },
          }),
        ),
      {
        id: "new-notebook",
        group: "Navigate",
        label: "New notebook",
        keywords: "create",
        icon: <Plus className="h-3.5 w-3.5" />,
        run: () => {
          close();
          const s = state();
          // "Untitled notebook", then "Untitled notebook 2", 3, …
          const taken = new Set(s.notebooks.map((n) => n.title));
          let title = "Untitled notebook";
          for (let i = 2; taken.has(title); i++) title = `Untitled notebook ${i}`;
          void s.createNotebook(title);
        },
      },
      {
        id: "settings",
        group: "Settings",
        label: "Open Settings",
        keywords: "preferences models config",
        icon: <Settings className="h-3.5 w-3.5" />,
        hint: "⌘,",
        run: () => {
          close();
          state().openSettings();
        },
      },
      ...Object.values(THEMES).map(
        (t): Command => ({
          id: `theme-${t.id}`,
          group: "Settings",
          label: `Theme: ${t.label}`,
          keywords: "appearance color dark light",
          icon: <Palette className="h-3.5 w-3.5" />,
          run: () => {
            state().setTheme(t.id);
            close();
          },
        }),
      ),
    );
    return list;
  }, [currentId, notebooks, agentMode, confirm]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commands;
    const terms = q.split(/\s+/);
    return commands.filter((c) => {
      const hay = `${c.label} ${c.group} ${c.keywords ?? ""}`.toLowerCase();
      return terms.every((t) => hay.includes(t));
    });
  }, [commands, query]);

  // Clamp the selection whenever the result set changes.
  useEffect(() => {
    setSelected((i) => Math.min(i, Math.max(0, filtered.length - 1)));
  }, [filtered.length]);

  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-index="${selected}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    // Committing an IME composition must not run a command.
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation(); // don't also close a dialog underneath
      setPaletteOpen(false);
    } else if (e.key === "Tab") {
      e.preventDefault(); // keep focus inside; the input is the only field
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelected((i) => (filtered.length ? (i + 1) % filtered.length : 0));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelected((i) => (filtered.length ? (i - 1 + filtered.length) % filtered.length : 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      filtered[selected]?.run();
    }
  };

  return (
    <>
      {paletteOpen && (
        <div
          className="fixed inset-0 z-[60] flex items-start justify-center bg-black/40 backdrop-blur-[2px] pt-[14vh] animate-in fade-in duration-150"
          onMouseDown={() => setPaletteOpen(false)}
        >
          <div
            role="dialog"
            aria-modal="true"
            aria-label="Command menu"
            className={cn(
              "flex max-h-[52vh] w-full max-w-[560px] flex-col overflow-hidden rounded-lg bg-elevated outline-none",
              "shadow-[0_0_0_0.5px_var(--border-strong),0_16px_48px_-8px_rgba(0,0,0,0.45)]",
              "animate-in zoom-in-95 duration-150",
            )}
            onMouseDown={(e) => e.stopPropagation()}
            onKeyDown={onKeyDown}
          >
            <div className="flex items-center gap-2.5 border-b border-border px-3.5">
              <Search className="h-4 w-4 shrink-0 text-subtle-foreground" />
              <input
                ref={inputRef}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Type a command or search…"
                className="h-11 w-full bg-transparent text-[14px] text-foreground placeholder:text-subtle-foreground outline-none"
                // macOS text intelligence draws a focus ring + suggestion pill
                // on this field and its popup steals the arrow keys.
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                {...({ writingsuggestions: "false" } as Record<string, string>)}
                role="combobox"
                aria-expanded="true"
                aria-controls="palette-list"
                aria-activedescendant={
                  filtered[selected] ? `palette-${filtered[selected].id}` : undefined
                }
              />
              <kbd className="shrink-0 rounded border border-border-strong bg-surface-2 px-1.5 py-0.5 text-[10px] text-subtle-foreground">
                esc
              </kbd>
            </div>
            <div id="palette-list" role="listbox" ref={listRef} className="flex-1 overflow-y-auto p-1.5">
              {filtered.length === 0 ? (
                <div className="px-3 py-8 text-center text-[13px] text-muted-foreground">
                  No matching commands
                </div>
              ) : (
                filtered.map((cmd, index) => (
                  <Fragment key={cmd.id}>
                    {(index === 0 || filtered[index - 1].group !== cmd.group) && (
                      <div className="px-2.5 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wide text-subtle-foreground">
                        {cmd.group}
                      </div>
                    )}
                    <div
                      id={`palette-${cmd.id}`}
                      data-index={index}
                      role="option"
                      aria-selected={index === selected}
                      onMouseMove={() => setSelected(index)}
                      onClick={() => cmd.run()}
                      className={cn(
                        "flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-1.5 text-[13px]",
                        index === selected ? "bg-surface-2 text-foreground" : "text-foreground/85",
                      )}
                    >
                      <span className="text-muted-foreground">{cmd.icon}</span>
                      <span className="min-w-0 flex-1 truncate">{cmd.label}</span>
                      {cmd.hint && (
                        <span className="shrink-0 text-[11px] text-subtle-foreground">{cmd.hint}</span>
                      )}
                    </div>
                  </Fragment>
                ))
              )}
            </div>
          </div>
        </div>
      )}
      {confirmDialog}
    </>
  );
}
