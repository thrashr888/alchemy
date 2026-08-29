import {
  Fragment,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { navAtomic, useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { openMetaCitation } from "@/lib/citations";
import { citedNotebooks, runForThread } from "@/lib/homeChatRun";
import { SYSTEM_THEME, THEMES } from "@/lib/themes";
import { cn } from "@/lib/utils";
import type { MetaCitation, MetaTurn, SearchHit } from "@/lib/types";
import { ARTIFACTS, AUDIO_OVERVIEW } from "./studioArtifacts";
import { Markdown } from "./Markdown";
import { Spinner, useConfirm } from "./ui";
import {
  AlertTriangle,
  AppWindow,
  BookOpen,
  ChevronLeft,
  Eraser,
  FileText,
  FolderOutput,
  LayoutGrid,
  Library,
  Link2,
  MessageSquare,
  MessagesSquare,
  Palette,
  PanelLeft,
  PanelRight,
  Plus,
  Search,
  Settings,
  Sparkles,
  Logs,
  Package,
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
  //
  // The answer is NOT this component's to run. There is one corpus channel per
  // window, and the store owns it (`askHome`/`stopHome`, app-lifetime meta://
  // listeners): the palette used to listen for the same tokens and cancel the
  // same scope on its own, so a palette question asked over a live Home answer
  // put two owners on one stream. A palette ask is a Home conversation now —
  // it supersedes whatever was running exactly as asking from another thread
  // does, it persists, and it keeps going if the palette closes.
  const [mode, setMode] = useState<"search" | "ask">("search");
  const [followup, setFollowup] = useState("");
  // The conversation this palette session is asking into, minted on the first
  // question. A ref, not state: `startAsk` has to know within the same tick
  // whether it is opening a conversation or continuing one, and every change
  // to it rides along with a store change that re-renders anyway.
  const askThread = useRef<string | null>(null);
  const askBodyRef = useRef<HTMLDivElement>(null);

  const homeThreadId = useStore((s) => s.homeChat.threadId);
  const homeTurns = useStore((s) => s.homeChat.turns);
  const homeRun = useStore((s) => s.homeRun);
  // Only ever the palette's own conversation: if something moved the open
  // thread out from under us, this shows nothing rather than someone else's.
  const askThreadId =
    askThread.current && homeThreadId === askThread.current
      ? homeThreadId
      : null;
  const askTurns = askThreadId ? homeTurns : [];
  const askRun = runForThread(homeRun, askThreadId);

  useEffect(() => {
    if (!paletteOpen) return;
    setQuery("");
    setSelected(0);
    setMode("search");
    setFollowup("");
    // A reopened palette starts a new session — and hence, on the next
    // question, a new conversation. Any answer still being written keeps
    // going in the thread it was asked in.
    askThread.current = null;
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

  // Follow the answer down as it arrives, the way the Chat tab's thread does.
  useEffect(() => {
    const body = askBodyRef.current;
    if (body) body.scrollTop = body.scrollHeight;
  }, [
    askTurns.length,
    askRun?.streaming,
    askRun?.steps.length,
    askRun?.waiting,
  ]);

  function startAsk(question: string) {
    const q = question.trim();
    if (!q) return;
    const s = useStore.getState();
    // A run in the palette's own conversation blocks the follow-up; Stop is
    // the way out of it. (A run in ANY OTHER thread doesn't: asking here
    // supersedes it, and the store keeps its partial under its own thread.)
    if (askThread.current && s.homeRun?.threadId === askThread.current) return;
    setMode("ask");
    setFollowup("");
    // The first question of a session opens a conversation of its own: asked
    // at the launcher, it is a fresh subject, and grafting it onto whatever
    // Home had open would feed that thread's history to the model as context.
    // Follow-ups typed here continue this one, history and all.
    if (!askThread.current || s.homeChat.threadId !== askThread.current)
      askThread.current = s.newHomeThread();
    void s.askHome(q);
  }

  /** Esc out of ask mode. A live answer is stopped the way Home's Stop stops
   *  it — cancelled, and whatever it had written kept as a stopped turn.
   *  Closing the palette any other way leaves it running in its thread. */
  function exitAsk() {
    if (askRun) useStore.getState().stopHome();
    askThread.current = null;
    setMode("search");
    requestAnimationFrame(() => inputRef.current?.focus());
  }

  /** The whole conversation, in the place built for reading it. */
  function openInChat() {
    const threadId = askThread.current;
    if (!threadId) return;
    setPaletteOpen(false);
    void navAtomic(async () => {
      const s = useStore.getState();
      if (s.currentId) s.closeNotebook();
      await useStore.getState().openHomeThread(threadId);
    });
  }

  const commands = useMemo<Command[]>(() => {
    // Read fresh store state at execution time — panel/agent flags may have
    // changed since the palette opened.
    const state = () => useStore.getState();
    const close = () => state().setPaletteOpen(false);
    /** A jump to Home: close the palette, leave the notebook if one is open,
     *  then land — recorded as the single navigation it reads as. */
    const goHome = (go: () => Promise<void> | void) => {
      close();
      void navAtomic(async () => {
        const s = state();
        if (s.currentId) s.closeNotebook();
        await go();
      });
    };
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
          label: "Export notebook…",
          keywords:
            "share send coworker zip package okf open knowledge format markdown backup download",
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
          id: "open-gallery",
          group: "View",
          label: "Browse source gallery",
          keywords: "images cards grid explore visual masonry",
          icon: <LayoutGrid className="h-3.5 w-3.5" />,
          run: () => {
            close();
            useStore.setState({ galleryOpen: true, ledgerOpen: false });
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
        .filter((n) => n.id !== currentId && n.status !== "archived")
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
      ...notebooks
        .filter((n) => n.status !== "archived")
        .map((n): Command => ({
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
        id: "import-okf",
        group: "Navigate",
        label: "Import notebook from OKF…",
        keywords: "zip bundle share receive upload okf",
        icon: <FolderOutput className="h-3.5 w-3.5" />,
        run: () => {
          close();
          useStore.setState({ importOkfOpen: true });
        },
      },
      // Home's conversations and its sections, reachable from anywhere the
      // palette is: each hop leaves an open notebook behind, and navAtomic
      // keeps that to one back-stack entry instead of a stop-over in the
      // notebook's own chat.
      {
        id: "home-new-chat",
        group: "Navigate",
        label: "New chat",
        keywords: "ask everything corpus conversation thread meta question",
        icon: <MessageSquare className="h-3.5 w-3.5" />,
        hint: "⌥⌘N",
        run: () => goHome(() => state().openHomeThread(null)),
      },
      {
        id: "home-chat",
        group: "Navigate",
        label: "Go to chats",
        keywords: "conversations threads history ask everything meta",
        icon: <MessagesSquare className="h-3.5 w-3.5" />,
        // The conversation last on screen, minting one only if there has
        // never been one — what Home's own Chat tab does.
        run: () =>
          goHome(() => state().openHomeThread(state().homeChat.threadId)),
      },
      {
        id: "home-registry",
        group: "Navigate",
        label: "Go to registry",
        keywords: "cards people places things entities identifiers",
        icon: <Package className="h-3.5 w-3.5" />,
        run: () =>
          goHome(() =>
            useStore.setState({ homeSection: "registry", openCardId: null }),
          ),
      },
      // Only from Home: inside a notebook, "Back to all notebooks" above is
      // already this row, and two ways to say it in one list is one too many.
      ...(currentId
        ? []
        : [
            {
              id: "home-notebooks",
              group: "Navigate",
              label: "Go to notebooks",
              keywords: "shelf library home grid all",
              icon: <Library className="h-3.5 w-3.5" />,
              run: () =>
                goHome(() =>
                  useStore.setState({
                    homeSection: "notebooks",
                    openCardId: null,
                  }),
                ),
            } satisfies Command,
          ]),
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
      group:
        h.kind === "card"
          ? "Registry"
          : h.kind === "ledger"
            ? "Ledger"
            : "Search sources & notes",
      label: h.title || h.snippet.slice(0, 60) || "Untitled",
      keywords: h.snippet,
      icon:
        h.kind === "card" ? (
          <Package className="h-3.5 w-3.5" />
        ) : h.kind === "ledger" ? (
          <Logs className="h-3.5 w-3.5" />
        ) : h.kind === "note" ? (
          <SquarePen className="h-3.5 w-3.5" />
        ) : (
          <FileText className="h-3.5 w-3.5" />
        ),
      run: () => {
        close();
        void (async () => {
          const s = state();
          if (h.kind === "card") {
            // Cards are corpus-scoped: leave the notebook and open the card
            // on Home rather than switching notebooks.
            s.closeNotebook();
            useStore.setState({
              homeSection: "registry",
              openCardId: h.id,
            });
          } else if (h.kind === "ledger") {
            // Open the notebook's Ledger tab — where the row can be acted on.
            await s.selectNotebook(h.notebookId);
            useStore.setState({ ledgerOpen: true, galleryOpen: false });
          } else if (h.kind === "note") {
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
      // then closes the palette as usual. Tab is left alone so focus flows
      // input → notebook chips → citations (all real buttons); Enter only
      // re-asks from the input — on a focused citation it activates it.
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        exitAsk();
      } else if (e.key === "Enter" && e.target === inputRef.current) {
        e.preventDefault();
        startAsk(followup);
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

  /** Jump to a cited passage — shared routing with the Home chat. */
  function openCitation(c: MetaCitation) {
    setPaletteOpen(false);
    void openMetaCitation(c);
  }

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
              "menu-glass flex w-full flex-col overflow-hidden rounded-lg outline-none",
              // Ask mode gets real reading room; search stays launcher-sized.
              mode === "ask"
                ? "max-h-[78vh] max-w-[780px]"
                : "max-h-[52vh] max-w-[560px]",
              "transition-[max-width,max-height] duration-200",
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
                className="h-11 w-full bg-transparent text-card text-foreground placeholder:text-subtle-foreground outline-none"
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
              <kbd className="shrink-0 rounded border border-border-strong bg-surface-2 px-1.5 py-0.5 text-badge text-subtle-foreground">
                esc
              </kbd>
            </div>
            {mode === "ask" ? (
              // Keyed so React swaps the container instead of reconciling the
              // listbox's keyed children into this branch's unkeyed ones.
              //
              // Everything below is read from the store's conversation: the
              // settled turns of this palette's thread, then whatever its run
              // has written so far. The palette displays a conversation now;
              // it doesn't run one.
              <div
                key="ask-body"
                ref={askBodyRef}
                className="flex flex-1 flex-col gap-3.5 overflow-y-auto px-4 py-3.5"
              >
                {askTurns.map((turn) =>
                  turn.role === "user" ? (
                    <div
                      key={turn.id}
                      className="text-body font-medium text-foreground"
                    >
                      {turn.content}
                    </div>
                  ) : turn.kind === "error" ? (
                    <div
                      key={turn.id}
                      role="alert"
                      className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-body text-foreground"
                    >
                      <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" />
                      <span className="min-w-0 whitespace-pre-line">
                        {turn.content}
                      </span>
                    </div>
                  ) : (
                    <AskAnswer
                      key={turn.id}
                      turn={turn}
                      onCitation={openCitation}
                      onNotebook={(id) => {
                        setPaletteOpen(false);
                        void useStore.getState().selectNotebook(id);
                      }}
                    />
                  ),
                )}
                {/* Queued behind the answer it displaced: the question isn't
                    in the thread yet, so show it where it will land. */}
                {askRun?.queued && askRun.question && (
                  <div className="text-body font-medium text-muted-foreground">
                    {askRun.question}
                  </div>
                )}
                {askRun && (
                  <div className="flex flex-col gap-2" aria-busy="true">
                    {askRun.streaming && (
                      <div className="text-body leading-relaxed">
                        {/* Citations arrive with the settled answer, so inline
                            [n] markers are plain text until then. */}
                        <Markdown>{askRun.streaming}</Markdown>
                      </div>
                    )}
                    {/* Live stage, not a static spinner: the backend narrates
                        the real pipeline (routing → searching → reading →
                        synthesizing). One quiet line — the current step plus a
                        subtle count of finished stages, never a log dump. */}
                    <div className="flex items-center gap-2 text-caption text-muted-foreground">
                      <Spinner className="h-3.5 w-3.5 shrink-0" />
                      <span className="min-w-0 truncate">
                        {askRun.waiting ||
                          askRun.steps[askRun.steps.length - 1] ||
                          "Searching every notebook…"}
                      </span>
                      {askRun.steps.length > 1 && (
                        <span className="shrink-0 text-micro tabular-nums text-subtle-foreground">
                          {askRun.steps.length} steps
                        </span>
                      )}
                      <button
                        type="button"
                        onClick={() => useStore.getState().stopHome()}
                        className="ml-auto shrink-0 rounded border border-border-strong px-1.5 py-px text-micro text-muted-foreground transition-colors hover:text-foreground"
                      >
                        Stop
                      </button>
                    </div>
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
                  <div className="px-3 py-8 text-center text-body text-muted-foreground">
                    No matching commands
                  </div>
                ) : (
                  results.map((cmd, index) => (
                    <Fragment key={cmd.id}>
                      {(index === 0 ||
                        results[index - 1].group !== cmd.group) && (
                        <div className="px-2.5 pb-1 pt-2 text-micro font-semibold uppercase tracking-wide text-subtle-foreground">
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
                          "flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-1.5 text-body",
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
                          <span className="shrink-0 rounded border border-border-strong bg-surface-2 px-1 py-px text-badge text-subtle-foreground">
                            {cmd.hint}
                          </span>
                        )}
                      </div>
                    </Fragment>
                  ))
                )}
              </div>
            )}
            {/* A palette answer is a real conversation now — kept, listed, and
                continuable somewhere with room to read it. */}
            {mode === "ask" && (
              <div className="flex items-center gap-2 border-t border-border px-3.5 py-2">
                <span className="min-w-0 truncate text-micro text-subtle-foreground">
                  Kept in your chats
                </span>
                <button
                  type="button"
                  onClick={openInChat}
                  className="ml-auto flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 text-caption text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
                >
                  <MessageSquare className="h-3.5 w-3.5" />
                  Open in Chat
                </button>
              </div>
            )}
          </div>
        </div>
      )}
      {confirmDialog}
    </>
  );
}

/** One settled corpus answer, as the palette shows it: the notebooks it drew
 *  from, the prose with its inline [n] chips, then the passages behind it. */
function AskAnswer({
  turn,
  onCitation,
  onNotebook,
}: {
  turn: MetaTurn;
  onCitation: (c: MetaCitation) => void;
  onNotebook: (notebookId: string) => void;
}) {
  const notebooks = citedNotebooks(turn.citations);
  return (
    <div className="flex flex-col gap-2.5">
      {notebooks.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {notebooks.map(([id, title]) => (
            <button
              key={id}
              onClick={() => onNotebook(id)}
              className="rounded-full border border-border bg-surface-2/60 px-2 py-0.5 text-micro text-muted-foreground transition-colors hover:border-border-strong hover:text-foreground"
            >
              {title || "Untitled"}
            </button>
          ))}
        </div>
      )}
      <div className="text-body leading-relaxed">
        <Markdown
          citations={turn.citations}
          onCitation={onCitation}
          citationLabel={(c) => `${c.title || "Untitled"} · ${c.notebookTitle}`}
        >
          {turn.content}
        </Markdown>
      </div>
      {turn.kind === "stopped" && (
        <div className="text-micro text-subtle-foreground">stopped</div>
      )}
      {turn.citations.length > 0 && (
        <div className="flex flex-col gap-0.5 border-t border-border pt-2.5">
          {turn.citations.map((c, i) => (
            <button
              key={`${c.kind}-${c.id}-${i}`}
              onClick={() => onCitation(c)}
              className="flex items-center gap-2 rounded-md px-1.5 py-1 text-left text-caption text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
            >
              <span className="shrink-0 text-badge text-subtle-foreground">
                [{i + 1}]
              </span>
              {c.kind === "card" ? (
                <Package className="h-3 w-3 shrink-0" />
              ) : c.kind === "note" ? (
                <SquarePen className="h-3 w-3 shrink-0" />
              ) : (
                <FileText className="h-3 w-3 shrink-0" />
              )}
              <span className="min-w-0 truncate">{c.title || "Untitled"}</span>
              <span className="ml-auto shrink-0 text-micro text-subtle-foreground">
                {c.notebookTitle}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
