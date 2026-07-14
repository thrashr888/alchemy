import {
  Fragment,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { SYSTEM_THEME, THEMES } from "@/lib/themes";
import { cn } from "@/lib/utils";
import type { MetaCitation, SearchHit } from "@/lib/types";
import { ARTIFACTS, AUDIO_OVERVIEW } from "./StudioPanel";
import { Markdown } from "./Markdown";
import { Spinner, useConfirm } from "./ui";
import {
  AppWindow,
  BookOpen,
  ChevronLeft,
  Eraser,
  FileText,
  FolderOutput,
  Link2,
  MessageSquare,
  Palette,
  PanelLeft,
  PanelRight,
  Plus,
  Search,
  Settings,
  Sparkles,
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
  /** Rendered dimmed until selected (the always-there Ask row). */
  muted?: boolean;
  run: () => void;
}

/** Cmd+K command menu: search across navigation, sources, and generation. */
export function CommandPalette() {
  const paletteOpen = useStore((s) => s.paletteOpen);
  const setPaletteOpen = useStore((s) => s.setPaletteOpen);
  const currentId = useStore((s) => s.currentId);
  const notebooks = useStore((s) => s.notebooks);
  const agentMode = useStore((s) => s.agentMode);
  const kokoroReady = useStore((s) => !!s.kokoroStatus?.verified);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // Ask mode (docs/RFC-meta-chat.md): the palette flips into a lightweight
  // corpus-wide chat. `query` is preserved underneath so Esc returns to the
  // search results exactly as they were.
  const [mode, setMode] = useState<"search" | "ask">("search");
  const [followup, setFollowup] = useState("");
  const [askQuestion, setAskQuestion] = useState("");
  const [askText, setAskText] = useState("");
  const [askCitations, setAskCitations] = useState<MetaCitation[]>([]);
  const [askLoading, setAskLoading] = useState(false);
  const askThread = useRef<{ role: string; content: string }[]>([]);

  useEffect(() => {
    if (!paletteOpen) return;
    setQuery("");
    setSelected(0);
    setMode("search");
    setFollowup("");
    setAskText("");
    setAskCitations([]);
    setAskLoading(false);
    askThread.current = [];
    // The homepage's unified ask box seeds a question — open straight into
    // ask mode with it (Esc still drops back to search with it as the query).
    const pending = useStore.getState().pendingAsk;
    if (pending) {
      useStore.setState({ pendingAsk: null });
      setQuery(pending);
      startAsk(pending);
    }
    const trigger = document.activeElement as HTMLElement | null;
    // The input mounts in this same render pass.
    requestAnimationFrame(() => inputRef.current?.focus());
    return () => trigger?.focus?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paletteOpen]);

  // Stream tokens into the answer while a question is in flight.
  useEffect(() => {
    if (!askLoading) return;
    const un = listen<{ content: string }>("meta://token", (e) => {
      setAskText((t) => t + e.payload.content);
    });
    return () => {
      void un.then((f) => f());
    };
  }, [askLoading]);

  function startAsk(question: string) {
    const q = question.trim();
    if (!q || askLoading) return;
    setMode("ask");
    setAskQuestion(q);
    setAskText("");
    setAskCitations([]);
    setAskLoading(true);
    setFollowup("");
    const history = [...askThread.current];
    api
      .askEverything(q, history)
      .then((res) => {
        setAskText(res.answer);
        setAskCitations(res.citations);
        askThread.current = [
          ...history,
          { role: "user", content: q },
          { role: "assistant", content: res.answer },
        ];
      })
      .catch((e) => {
        setAskText(e instanceof Error ? e.message : String(e));
      })
      .finally(() => setAskLoading(false));
  }

  function exitAsk() {
    if (askLoading) void api.cancelGeneration("meta");
    setMode("search");
    setAskLoading(false);
    requestAnimationFrame(() => inputRef.current?.focus());
  }

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
              if (
                await confirm({
                  title: "Clear this conversation?",
                  confirmLabel: "Clear",
                  danger: true,
                })
              )
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
            state().openAddSource("url");
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
        ...(kokoroReady ? [AUDIO_OVERVIEW, ...ARTIFACTS] : ARTIFACTS).map(
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
          id: "export-okf",
          group: "Notebook",
          label: "Export notebook as OKF bundle…",
          keywords: "open knowledge format markdown share backup download",
          icon: <FolderOutput className="h-3.5 w-3.5" />,
          hint: "⌘⇧E",
          run: () => {
            close();
            void state().exportNotebookOkf();
          },
        },
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
        .map((n): Command => ({
          id: `nb-${n.id}`,
          group: "Navigate",
          label: `Open notebook: ${n.title}`,
          keywords: "switch go",
          icon: <BookOpen className="h-3.5 w-3.5" />,
          run: () => {
            close();
            void state().selectNotebook(n.id);
          },
        })),
      ...notebooks.map((n): Command => ({
        id: `nbw-${n.id}`,
        group: "Navigate",
        label: `Open in new window: ${n.title}`,
        keywords: "window parallel side",
        icon: <AppWindow className="h-3.5 w-3.5" />,
        run: () => {
          close();
          void api.newWindow(n.id);
        },
      })),
      {
        id: "new-window",
        group: "Navigate",
        label: "New window",
        keywords: "open another parallel",
        icon: <AppWindow className="h-3.5 w-3.5" />,
        run: () => {
          close();
          void api.newWindow();
        },
      },
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
          for (let i = 2; taken.has(title); i++)
            title = `Untitled notebook ${i}`;
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
      {
        id: "theme-system",
        group: "Settings",
        label: "Theme: System",
        keywords: "appearance color dark light auto os",
        icon: <Palette className="h-3.5 w-3.5" />,
        run: () => {
          state().setTheme(SYSTEM_THEME);
          close();
        },
      },
      ...Object.values(THEMES).map((t): Command => ({
        id: `theme-${t.id}`,
        group: "Settings",
        label: `Theme: ${t.label}`,
        keywords: "appearance color dark light",
        icon: <Palette className="h-3.5 w-3.5" />,
        run: () => {
          state().setTheme(t.id);
          close();
        },
      })),
    );
    return list;
  }, [currentId, notebooks, agentMode, kokoroReady, confirm]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commands;
    const terms = q.split(/\s+/);
    return commands.filter((c) => {
      const hay = `${c.label} ${c.group} ${c.keywords ?? ""}`.toLowerCase();
      return terms.every((t) => hay.includes(t));
    });
  }, [commands, query]);

  // Global content search: debounced BM25 across every notebook's sources and
  // notes. Appended after the command matches as its own group.
  const [hits, setHits] = useState<SearchHit[]>([]);
  useEffect(() => {
    if (!paletteOpen || query.trim().length < 3) {
      setHits([]);
      return;
    }
    const t = setTimeout(() => {
      api
        .searchEverything(query.trim())
        .then(setHits)
        .catch(() => setHits([]));
    }, 200);
    return () => clearTimeout(t);
  }, [paletteOpen, query]);

  const hitCommands = useMemo<Command[]>(() => {
    const state = () => useStore.getState();
    const close = () => state().setPaletteOpen(false);
    return hits.map((h) => ({
      id: `hit-${h.kind}-${h.id}`,
      group: "Search sources & notes",
      label: h.title || h.snippet.slice(0, 60) || "Untitled",
      keywords: h.snippet,
      icon:
        h.kind === "note" ? (
          <SquarePen className="h-3.5 w-3.5" />
        ) : (
          <FileText className="h-3.5 w-3.5" />
        ),
      run: () => {
        close();
        void (async () => {
          const s = state();
          if (h.kind === "note") {
            // StudioPanel auto-opens this id once the notebook's notes load.
            useStore.setState({ justCreatedNoteId: h.id });
            if (!s.studioOpen) s.toggleStudio();
            await s.selectNotebook(h.notebookId);
          } else {
            await s.selectNotebook(h.notebookId);
            // After the switch: the viewer survives because selectNotebook
            // has already reset state by the time we set it.
            useStore
              .getState()
              .openSourceViewer(
                h.id,
                h.title,
                h.kind === "content" ? h.snippet : undefined,
              );
          }
        })();
      },
    }));
  }, [hits]);

  // The Ask row: always the last result whenever there's a query — dimmed
  // until reached (Tab jumps straight to it), so it never competes with
  // command matches but is always one keystroke away.
  const askRow = useMemo<Command[]>(() => {
    const q = query.trim();
    if (!q) return [];
    return [
      {
        id: "ask-everything",
        group: "Ask",
        label: `Ask across all notebooks: “${q}”`,
        keywords: q,
        icon: <Sparkles className="h-3.5 w-3.5" />,
        hint: "tab",
        muted: true,
        run: () => startAsk(q),
      },
    ];
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query]);

  const results = useMemo(
    () => [...filtered, ...hitCommands, ...askRow],
    [filtered, hitCommands, askRow],
  );

  // Clamp the selection whenever the result set changes.
  useEffect(() => {
    setSelected((i) => Math.min(i, Math.max(0, results.length - 1)));
  }, [results.length]);

  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-index="${selected}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    // Committing an IME composition must not run a command.
    if (e.nativeEvent.isComposing) return;
    if (mode === "ask") {
      // Esc steps back to the search results (query intact); a second Esc
      // then closes the palette as usual.
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        exitAsk();
      } else if (e.key === "Enter") {
        e.preventDefault();
        startAsk(followup);
      } else if (e.key === "Tab") {
        e.preventDefault();
      }
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation(); // don't also close a dialog underneath
      setPaletteOpen(false);
    } else if (e.key === "Tab") {
      // Tab jumps to the Ask row (the last result) — the one-keystroke path
      // into corpus-wide answers.
      e.preventDefault();
      if (askRow.length) setSelected(results.length - 1);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelected((i) => (results.length ? (i + 1) % results.length : 0));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelected((i) =>
        results.length ? (i - 1 + results.length) % results.length : 0,
      );
    } else if (e.key === "Enter") {
      e.preventDefault();
      results[selected]?.run();
    }
  };

  /** Jump to a cited passage: select the notebook, then open the note card
   *  or the source reader at the snippet — same routing as search hits. */
  function openCitation(c: MetaCitation) {
    setPaletteOpen(false);
    void (async () => {
      const s = useStore.getState();
      if (c.kind === "note") {
        useStore.setState({ justCreatedNoteId: c.id });
        if (!s.studioOpen) s.toggleStudio();
        await s.selectNotebook(c.notebookId);
      } else {
        await s.selectNotebook(c.notebookId);
        useStore.getState().openSourceViewer(c.id, c.title, c.snippet);
      }
    })();
  }

  const askNotebooks = useMemo(() => {
    const seen = new Map<string, string>();
    for (const c of askCitations) {
      if (!seen.has(c.notebookId)) seen.set(c.notebookId, c.notebookTitle);
    }
    return [...seen.entries()];
  }, [askCitations]);

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
              {mode === "ask" ? (
                <Sparkles className="h-4 w-4 shrink-0 text-citation" />
              ) : (
                <Search className="h-4 w-4 shrink-0 text-subtle-foreground" />
              )}
              <input
                ref={inputRef}
                value={mode === "ask" ? followup : query}
                onChange={(e) =>
                  mode === "ask"
                    ? setFollowup(e.target.value)
                    : setQuery(e.target.value)
                }
                placeholder={
                  mode === "ask"
                    ? "Ask a follow-up…"
                    : "Type a command or search…"
                }
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
                  results[selected]
                    ? `palette-${results[selected].id}`
                    : undefined
                }
              />
              <kbd className="shrink-0 rounded border border-border-strong bg-surface-2 px-1.5 py-0.5 text-[10px] text-subtle-foreground">
                esc
              </kbd>
            </div>
            {mode === "ask" ? (
              // Keyed so React swaps the container instead of reconciling the
              // listbox's keyed children into this branch's unkeyed ones.
              <div
                key="ask-body"
                className="flex-1 overflow-y-auto px-4 py-3.5"
              >
                <div className="mb-2.5 text-[13px] font-medium text-foreground">
                  {askQuestion}
                </div>
                {askNotebooks.length > 0 && (
                  <div className="mb-2.5 flex flex-wrap gap-1.5">
                    {askNotebooks.map(([id, title]) => (
                      <button
                        key={id}
                        onClick={() => {
                          setPaletteOpen(false);
                          void useStore.getState().selectNotebook(id);
                        }}
                        className="rounded-full border border-border bg-surface-2/60 px-2 py-0.5 text-[11px] text-muted-foreground transition-colors hover:border-border-strong hover:text-foreground"
                      >
                        {title || "Untitled"}
                      </button>
                    ))}
                  </div>
                )}
                {askText ? (
                  <div className="text-[13px] leading-relaxed">
                    <Markdown>{askText}</Markdown>
                  </div>
                ) : (
                  <div className="flex items-center gap-2 py-4 text-[12px] text-muted-foreground">
                    <Spinner className="h-3.5 w-3.5" />
                    Searching every notebook…
                  </div>
                )}
                {!askLoading && askCitations.length > 0 && (
                  <div className="mt-3 flex flex-col gap-0.5 border-t border-border pt-2.5">
                    {askCitations.map((c, i) => (
                      <button
                        key={`${c.kind}-${c.id}-${i}`}
                        onClick={() => openCitation(c)}
                        className="flex items-center gap-2 rounded-md px-1.5 py-1 text-left text-[12px] text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
                      >
                        <span className="shrink-0 text-[10px] text-subtle-foreground">
                          [{i + 1}]
                        </span>
                        {c.kind === "note" ? (
                          <SquarePen className="h-3 w-3 shrink-0" />
                        ) : (
                          <FileText className="h-3 w-3 shrink-0" />
                        )}
                        <span className="min-w-0 truncate">
                          {c.title || "Untitled"}
                        </span>
                        <span className="ml-auto shrink-0 text-[11px] text-subtle-foreground">
                          {c.notebookTitle}
                        </span>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            ) : (
              <div
                key="search-list"
                id="palette-list"
                role="listbox"
                ref={listRef}
                className="flex-1 overflow-y-auto p-1.5"
              >
                {results.length === 0 ? (
                  <div className="px-3 py-8 text-center text-[13px] text-muted-foreground">
                    No matching commands
                  </div>
                ) : (
                  results.map((cmd, index) => (
                    <Fragment key={cmd.id}>
                      {(index === 0 ||
                        results[index - 1].group !== cmd.group) && (
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
                          index === selected
                            ? "bg-surface-2 text-foreground"
                            : cmd.muted
                              ? "text-subtle-foreground"
                              : "text-foreground/85",
                        )}
                      >
                        <span className="text-muted-foreground">
                          {cmd.icon}
                        </span>
                        <span className="min-w-0 flex-1 truncate">
                          {cmd.label}
                        </span>
                        {cmd.hint && (
                          <span className="shrink-0 rounded border border-border-strong bg-surface-2 px-1 py-px text-[10px] text-subtle-foreground">
                            {cmd.hint}
                          </span>
                        )}
                      </div>
                    </Fragment>
                  ))
                )}
              </div>
            )}
          </div>
        </div>
      )}
      {confirmDialog}
    </>
  );
}
