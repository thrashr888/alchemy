import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { Button, CardAction, Textarea, useConfirm } from "./ui";
import { Markdown } from "./Markdown";
import { cn, chatReadingClass, isWebUrl } from "@/lib/utils";
import { DitherBackground } from "./DitherBackground";
import { AlchemySymbol } from "./AlchemyHero";
import { DEFAULT_VERBS, THEMES, resolveThemeId } from "@/lib/themes";
import { generatedEpigraph } from "@/lib/epigraph";
import type { Citation, Message } from "@/lib/types";
import {
  MessageSquare,
  ArrowDown,
  ArrowUp,
  Square,
  Eraser,
  Quote,
  StickyNote,
  Sparkles,
  Telescope,
  Check,
  Copy,
  NotebookPen,
  RefreshCw,
  CornerDownRight,
  ExternalLink,
  SlidersHorizontal,
} from "lucide-react";

export function ChatPanel() {
  const currentId = useStore((s) => s.currentId);
  const messages = useStore((s) => s.messages);
  const sources = useStore((s) => s.sources);
  const sending = useStore((s) => s.sending);
  const streamingText = useStore((s) => s.streamingText);
  const steps = useStore((s) => s.steps);
  const agentMode = useStore((s) => s.agentMode);
  const toggleAgentMode = useStore((s) => s.toggleAgentMode);
  const send = useStore((s) => s.sendMessage);
  const cancelGeneration = useStore((s) => s.cancelGeneration);
  const reading = useStore((s) => s.reading);
  const clearChat = useStore((s) => s.clearChat);
  const appendToken = useStore((s) => s.appendToken);
  const appendStep = useStore((s) => s.appendStep);
  const theme = useStore((s) => s.theme);
  const followups = useStore((s) => s.followups);
  const summary = useStore((s) => s.summary);
  const summaryLoading = useStore((s) => s.summaryLoading);
  const refreshSummary = useStore((s) => s.refreshSummary);

  const [draft, setDraft] = useState("");
  const failedInput = useStore((s) => s.failedInput);
  const { confirm, dialog: confirmDialog } = useConfirm();
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // A failed send hands its text back — restore it into the composer so the
  // user can retry without retyping.
  useEffect(() => {
    if (failedInput) {
      setDraft((d) => d || failedInput);
      useStore.setState({ failedInput: null });
    }
  }, [failedInput]);

  // Another surface (the source reader's "Ask about this") staged text for
  // the composer — load it and focus so the user can finish their question.
  const pendingInput = useStore((s) => s.pendingInput);
  useEffect(() => {
    if (!pendingInput) return;
    setDraft(pendingInput);
    useStore.setState({ pendingInput: null });
    // Focus after the surface that staged the text (a modal) has closed, and
    // autosize for multi-line prefills (onChange normally handles this).
    setTimeout(() => {
      const el = inputRef.current;
      if (!el) return;
      el.focus();
      el.style.height = "auto";
      el.style.height = `${Math.min(el.scrollHeight, 180)}px`;
      el.selectionStart = el.selectionEnd = el.value.length;
    }, 0);
  }, [pendingInput]);

  // Subscribe once to streaming tokens + agent progress steps from the backend.
  // Events broadcast to every window — only the one with a send in flight
  // should accumulate them.
  useEffect(() => {
    const unToken = listen<{ content: string }>("chat://token", (e) => {
      if (useStore.getState().sending) appendToken(e.payload.content);
    });
    const unStep = listen<{ label: string }>("chat://step", (e) => {
      if (useStore.getState().sending) appendStep(e.payload.label);
    });
    return () => {
      unToken.then((fn) => fn());
      unStep.then((fn) => fn());
    };
  }, [appendToken, appendStep]);

  // "Focus the chat composer" command from the Cmd+K menu.
  useEffect(() => {
    const onFocus = () => inputRef.current?.focus();
    window.addEventListener("nb:focus-composer", onFocus);
    return () => window.removeEventListener("nb:focus-composer", onFocus);
  }, []);

  // Jump straight to the latest message when a notebook's chat first loads —
  // the near-bottom guard below would otherwise leave us stuck at the top.
  const initialScrollDone = useRef(false);
  useEffect(() => {
    initialScrollDone.current = false;
  }, [currentId]);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || initialScrollDone.current || messages.length === 0) return;
    el.scrollTop = el.scrollHeight;
    initialScrollDone.current = true;
  }, [messages, currentId]);

  // Autoscroll on new content — but only when the user is already near the
  // bottom, so scrolling up to re-read mid-stream isn't yanked back down.
  // `atBottom` also drives the "jump to latest" pill when content arrives
  // off-screen.
  const [atBottom, setAtBottom] = useState(true);
  const updateAtBottom = () => {
    const el = scrollRef.current;
    if (!el) return;
    setAtBottom(el.scrollHeight - el.scrollTop - el.clientHeight < 120);
  };
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
    if (nearBottom) el.scrollTo({ top: el.scrollHeight });
    setAtBottom(nearBottom);
  }, [messages, streamingText, steps]);

  // Sending your own message always jumps to it, even from deep in history —
  // the near-bottom guard is for incoming content, not your own action.
  useEffect(() => {
    if (!sending) return;
    const el = scrollRef.current;
    el?.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }, [sending]);

  const jumpToLatest = () => {
    const el = scrollRef.current;
    el?.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  };

  const canChat = !!currentId && sources.length > 0;
  const isBlank = messages.length === 0 && !sending;

  function submit() {
    const text = draft.trim();
    if (!text || sending || !canChat) return;
    setDraft("");
    void send(text);
  }

  return (
    <div className="relative flex h-full flex-1 flex-col min-w-0">
      {isBlank && (
        <>
          <div className="chat-mist pointer-events-none absolute inset-0 z-0">
            <DitherBackground themeKey={theme} />
          </div>
          <div className="chat-mist-fade pointer-events-none absolute inset-0 z-0" />
        </>
      )}
      <div className="relative z-10 flex items-center px-5 h-12 border-b border-border">
        <MessageSquare className="h-4 w-4 text-muted-foreground" />
        <span className="ml-2 text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">
          Chat
        </span>
        <div className="ml-auto flex items-center gap-1">
          {messages.length > 0 && (
            <Button
              variant="ghost"
              size="sm"
              onClick={async () => {
                if (await confirm({ title: "Clear this conversation?", confirmLabel: "Clear", danger: true }))
                  clearChat();
              }}
            >
              <Eraser className="h-3.5 w-3.5" />
              Clear
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon"
            onClick={() => useStore.getState().openSettings("chat")}
            title="Chat settings (style, length, custom prompt)"
            aria-label="Chat settings"
          >
            <SlidersHorizontal className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      <div ref={scrollRef} onScroll={updateAtBottom} className="relative z-10 flex-1 overflow-y-auto">
        <div className={cn("mx-auto flex max-w-[720px] flex-col gap-6 px-5 py-6", chatReadingClass(reading))}>
          {canChat && (
            <SummaryBanner
              summary={summary}
              loading={summaryLoading}
              onRefresh={refreshSummary}
              centered={isBlank}
            />
          )}

          {/* The sigil: full-size welcome on a truly blank notebook; once a
              summary exists it stays as a compact emblem between the summary
              and the start of the thread. */}
          {(isBlank || (canChat && !!summary)) && (
            <ChatHero
              hasNotebook={!!currentId}
              hasSources={sources.length > 0}
              compact={canChat && !!summary}
            />
          )}

          {messages.map((m) => (
            <ChatMessage key={m.id} message={m} />
          ))}

          {sending && (
            <div className="flex flex-col gap-2">
              <RoleLabel role="assistant" />
              {steps.length > 0 && <StepTrail steps={steps} done={!!streamingText} />}
              {streamingText ? (
                <Markdown>{streamingText}</Markdown>
              ) : (
                steps.length === 0 && <ThinkingDots />
              )}
            </div>
          )}

          {!sending && followups.length > 0 && messages.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <span className="text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
                Suggested follow-ups
              </span>
              {followups.map((q, i) => (
                <button
                  key={i}
                  onClick={() => {
                    // Fill the composer instead of firing immediately — the
                    // user can tweak the question, or just hit Enter.
                    setDraft(q);
                    inputRef.current?.focus();
                  }}
                  className="flex items-start gap-2 rounded-lg border border-border bg-surface/60 px-3 py-2 text-left text-[13px] text-foreground/90 transition-colors hover:border-border-strong hover:bg-surface-2"
                >
                  <CornerDownRight className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  {q}
                </button>
              ))}
            </div>
          )}
        </div>

        {!atBottom && !isBlank && (
          <div className="pointer-events-none sticky bottom-3 z-20 flex justify-center">
            <button
              onClick={jumpToLatest}
              className={cn(
                "pointer-events-auto flex h-7 items-center gap-1.5 rounded-full border border-border-strong",
                "bg-elevated/95 px-3 text-[11px] font-medium text-muted-foreground shadow-lg backdrop-blur",
                "transition-colors hover:text-foreground",
              )}
            >
              <ArrowDown className="h-3 w-3" />
              {sending ? "New content below" : "Jump to latest"}
            </button>
          </div>
        )}
      </div>

      <div className="relative z-10 px-5 pb-5 pt-2">
        <div className="mx-auto max-w-[720px]">
          <div
            className={cn(
              "rounded-lg border border-border-strong bg-surface p-2.5 shadow-md transition-colors",
              "focus-within:border-ring/60",
            )}
          >
            <Textarea
              ref={inputRef}
              rows={1}
              className="border-0 bg-transparent focus:ring-0 min-h-[24px] max-h-[180px] px-1.5 py-1"
              placeholder={
                canChat
                  ? "Ask anything about your sources…"
                  : currentId
                    ? "Add a source to start chatting"
                    : "Select or create a notebook"
              }
              value={draft}
              disabled={!canChat}
              onChange={(e) => {
                setDraft(e.target.value);
                e.target.style.height = "auto";
                e.target.style.height = `${Math.min(e.target.scrollHeight, 180)}px`;
              }}
              onKeyDown={(e) => {
                // isComposing: don't send mid-IME-composition (CJK input).
                if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  submit();
                }
              }}
            />
            <div className="flex items-center justify-between px-1.5 pt-1">
              <button
                onClick={toggleAgentMode}
                title="Agentic mode: the model plans multiple searches over your sources before answering"
                className={cn(
                  "inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-[11px] transition-colors",
                  agentMode
                    ? "border-primary/50 bg-primary/15 text-citation"
                    : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
                )}
              >
                <Telescope className="h-3 w-3" />
                {agentMode ? "Deep research: on" : "Deep research: off"}
              </button>
              {sending ? (
                <Button
                  variant="secondary"
                  size="icon"
                  onClick={() => cancelGeneration("chat")}
                  title="Stop"
                  aria-label="Stop generating"
                >
                  <Square className="h-3.5 w-3.5" />
                </Button>
              ) : (
                <Button
                  variant="primary"
                  size="icon"
                  onClick={submit}
                  disabled={!draft.trim() || !canChat}
                  title="Send"
                  aria-label="Send message"
                >
                  <ArrowUp className="h-4 w-4" />
                </Button>
              )}
            </div>
          </div>
        </div>
      </div>

      {confirmDialog}
    </div>
  );
}

function SummaryBanner({
  summary,
  loading,
  onRefresh,
  centered,
}: {
  summary: string;
  loading: boolean;
  onRefresh: () => void;
  /** Blank notebook: the chip sits under the centered hero, so center it. */
  centered?: boolean;
}) {
  if (!summary && !loading) {
    return (
      <button
        onClick={onRefresh}
        className={cn(
          "rounded-lg border border-dashed border-border-strong bg-surface/50 px-3 py-1.5 text-[12px] text-muted-foreground transition-colors hover:text-foreground",
          centered ? "self-center" : "self-start",
        )}
      >
        <Sparkles className="mr-1.5 inline h-3 w-3" />
        Generate notebook summary
      </button>
    );
  }
  return (
    <div className="rounded-lg border border-border bg-surface/60 p-3.5">
      <div className="mb-1 flex items-center justify-between">
        <span className="text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
          Notebook summary
        </span>
        <button
          onClick={onRefresh}
          className="text-muted-foreground transition-colors hover:text-foreground"
          title="Regenerate summary"
        >
          <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
        </button>
      </div>
      {loading && !summary ? (
        <div className="text-[13px] text-muted-foreground">Summarizing sources…</div>
      ) : (
        <div className="text-[13px] leading-relaxed text-foreground/90 selectable">
          {/* Single newlines become markdown hard breaks so the model's line
              breaks survive; double newlines stay paragraphs. */}
          <Markdown>{summary.replace(/\n(?!\n)/g, "  \n")}</Markdown>
        </div>
      )}
    </div>
  );
}

function ChatMessage({ message }: { message: Message }) {
  if (message.role === "user") {
    return (
      <div className="flex flex-col items-end gap-1">
        <div className="max-w-[85%] rounded-lg rounded-br-sm bg-surface-2 px-3.5 py-2 text-[13px] selectable border border-border">
          {message.content}
        </div>
      </div>
    );
  }
  return (
    <div className="group flex flex-col gap-2">
      <RoleLabel role="assistant" />
      <Markdown
        citations={message.citations}
        onCitation={openCitationTarget}
      >
        {message.content}
      </Markdown>
      {message.citations.length > 0 && <Citations citations={message.citations} />}
      <MessageActions content={message.content} />
    </div>
  );
}

function noteTitleFrom(content: string): string {
  const line = content.split("\n").map((l) => l.trim()).find(Boolean) ?? "";
  const clean = line.replace(/^#+\s*/, "").replace(/[*_`>#]/g, "").trim();
  return clean.slice(0, 60) || "Chat response";
}

function MessageActions({ content }: { content: string }) {
  const createNote = useStore((s) => s.createNote);
  const [copied, setCopied] = useState(false);
  const [saved, setSaved] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(content);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }
  async function save() {
    await createNote(noteTitleFrom(content), content);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  }

  return (
    <div className="flex items-center gap-1 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
      <button
        onClick={copy}
        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-surface-2 hover:text-foreground"
        title="Copy to clipboard"
      >
        {copied ? <Check className="h-3.5 w-3.5 text-success" /> : <Copy className="h-3.5 w-3.5" />}
        {copied ? "Copied" : "Copy"}
      </button>
      <button
        onClick={save}
        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-surface-2 hover:text-foreground"
        title="Save this response as a note"
      >
        {saved ? <Check className="h-3.5 w-3.5 text-success" /> : <NotebookPen className="h-3.5 w-3.5" />}
        {saved ? "Saved" : "Save as note"}
      </button>
    </div>
  );
}

function RoleLabel({ role }: { role: "assistant" | "user" }) {
  return (
    <div className="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
      <Sparkles className="h-3 w-3 text-primary" />
      {role === "assistant" ? "Assistant" : "You"}
    </div>
  );
}

/** A note citation opens the note in Studio (same routing as ⌘K note hits);
 *  a source citation opens the source reader at the passage. */
function openCitationTarget(c: Citation) {
  const s = useStore.getState();
  if (c.noteId) {
    // StudioPanel auto-opens this id once the notebook's notes load.
    useStore.setState({ justCreatedNoteId: c.noteId });
    if (!s.studioOpen) s.toggleStudio();
  } else {
    s.openSourceViewer(c.sourceId, c.sourceTitle, c.snippet);
  }
}

function Citations({ citations }: { citations: Citation[] }) {
  const [open, setOpen] = useState(false);
  const sources = useStore((s) => s.sources);
  // Only web origins get the open-in-browser chip; file paths live in the
  // same field but belong to the source reader's "Show in Finder".
  const urlOf = (sourceId: string) => {
    const url = sources.find((x) => x.id === sourceId)?.url || "";
    return isWebUrl(url) ? url : "";
  };
  return (
    <div className="mt-1">
      <button
        className="inline-flex items-center gap-1.5 rounded-md border border-border bg-surface px-2 py-1 text-[11px] text-muted-foreground hover:text-foreground hover:border-border-strong transition-colors"
        onClick={() => setOpen((o) => !o)}
      >
        <Quote className="h-3 w-3" />
        {citations.length} {citations.length === 1 ? "citation" : "citations"}
      </button>
      {open && (
        <div className="mt-2 flex flex-col gap-2">
          {citations.map((c, i) => (
            <div
              key={c.chunkId}
              title={c.noteId ? "Open the note in Studio" : "Open in the source, highlighted"}
              className="relative cursor-pointer rounded-md border border-border bg-surface px-3 py-2 text-left transition-colors hover:border-border-strong hover:bg-surface-2"
            >
              <CardAction
                label={`${c.noteId ? "Open note" : "Open source"} ${c.sourceTitle}`}
                onClick={() => openCitationTarget(c)}
              />
              <div className="pointer-events-none relative z-10 mb-1 flex items-center gap-2 text-[11px]">
                <span className="flex h-4 min-w-4 items-center justify-center rounded bg-primary/15 px-1 font-semibold text-citation">
                  {i + 1}
                </span>
                <span className="font-medium text-foreground/90 truncate">{c.sourceTitle}</span>
                {c.noteId && (
                  <span
                    className="inline-flex shrink-0 items-center gap-1 rounded bg-surface-2 px-1.5 py-0.5 font-medium text-muted-foreground"
                    title="From a note — a saved conclusion, not a source document"
                  >
                    <StickyNote className="h-3 w-3" />
                    note
                  </span>
                )}
                {urlOf(c.sourceId) && (
                  <button
                    className="pointer-events-auto relative z-20 ml-auto shrink-0 rounded p-0.5 text-citation hover:underline"
                    title={`Open ${urlOf(c.sourceId)}`}
                    aria-label={`Open ${urlOf(c.sourceId)} in browser`}
                    onClick={(e) => {
                      e.stopPropagation();
                      void openUrl(urlOf(c.sourceId));
                    }}
                  >
                    <ExternalLink className="h-3.5 w-3.5" />
                  </button>
                )}
              </div>
              <p
                // Stays hit-testable so the text can be selected; plain
                // clicks (no selection) still open the citation target.
                className="pointer-events-auto relative z-10 line-clamp-4 text-[12px] leading-relaxed text-muted-foreground selectable"
                onClick={() => {
                  if (!window.getSelection()?.toString())
                    openCitationTarget(c);
                }}
              >
                {c.snippet}
              </p>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function StepTrail({ steps, done }: { steps: string[]; done: boolean }) {
  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border bg-surface/60 px-3 py-2">
      {steps.map((s, i) => {
        const isLast = i === steps.length - 1;
        const spinning = isLast && !done;
        return (
          <div key={i} className="flex items-center gap-2 text-[12px]">
            {spinning ? (
              <span
                className="h-2.5 w-2.5 shrink-0 rounded-full border-[1.5px] border-primary border-t-transparent animate-spin"
                aria-hidden
              />
            ) : (
              <Check className="h-3 w-3 shrink-0 text-success" />
            )}
            <span className={cn(spinning ? "text-foreground" : "text-muted-foreground")}>{s}</span>
          </div>
        );
      })}
    </div>
  );
}

function ThinkingDots() {
  const theme = useStore((s) => s.theme);
  // One verb per thinking session, from the theme's set (see Theme.verbs).
  const [verb] = useState(() => {
    const verbs = THEMES[resolveThemeId(theme)]?.verbs ?? DEFAULT_VERBS;
    return verbs[Math.floor(Math.random() * verbs.length)];
  });
  return (
    <div className="flex items-center gap-2 py-1">
      <span className="text-[12px] text-muted-foreground">{verb}</span>
      <div className="flex items-center gap-1">
        {[0, 1, 2].map((i) => (
          <span
            key={i}
            className="h-1.5 w-1.5 rounded-full bg-muted-foreground"
            style={{ animation: "pulse-dot 1.2s ease-in-out infinite", animationDelay: `${i * 0.18}s` }}
          />
        ))}
      </div>
    </div>
  );
}

/**
 * The blank-state sigil. Full-size and vertically centered on an empty
 * notebook; `compact` (a summary exists) shrinks it to a small emblem at the
 * top of the chat column. Same element in both states, so the move animates.
 */
function ChatHero({
  hasNotebook,
  hasSources,
  compact,
}: {
  hasNotebook: boolean;
  hasSources: boolean;
  compact: boolean;
}) {
  const theme = useStore((s) => s.theme);
  // The sigil takes on the notebook's color — the transmutation circle is
  // this notebook's mark, not the app's.
  const notebookColor = useStore(
    (s) => s.notebooks.find((n) => n.id === s.currentId)?.color,
  );
  return (
    <div
      className={cn(
        "flex flex-col items-center gap-4 text-center transition-all duration-700",
        compact ? "pt-1" : "min-h-[62vh] justify-center",
      )}
    >
      <AlchemySymbol
        className={cn(
          "transition-all duration-700",
          notebookColor ? "opacity-85" : "text-citation/60",
          compact ? "h-9 w-9" : "h-16 w-16",
        )}
        style={notebookColor ? { color: notebookColor } : undefined}
        strokeWidth={notebookColor ? 1.5 : 1}
        preferred={THEMES[resolveThemeId(theme)]?.sigil}
      />
      {!compact && (
        <>
          <div className="text-[15px] font-semibold text-foreground/90">
            {!hasNotebook
              ? "Create a notebook to begin"
              : !hasSources
                ? "Add sources to start a grounded chat"
                : "Ask anything about your sources"}
          </div>
          <RotatingQuote theme={theme} />
        </>
      )}
    </div>
  );
}

/** Alchemy-flavored lines about what the chat actually does — a fresh one
 *  each page load, set in proper typographic quotes. */
const QUOTES = [
  "“Solve et coagula” — your sources dissolved, your answers given form.",
  "“Every answer shows its work: citations back to the exact passage.”",
  "“The athanor burns on your own machine; nothing leaves the laboratory.”",
  "“Prima materia in, quintessence out.”",
  "“As above, so below” — every claim traces to a line in your sources.",
  "“Transmutation, with receipts.”",
  "“Distill a hundred pages into one clear draught.”",
  "“The Great Work proceeds one question at a time.”",
  "“Hermetically sealed: your corpus, your model, your machine.”",
];

/** Chosen once per page load — module scope, so remounts don't reshuffle. */
const QUOTE = QUOTES[Math.floor(Math.random() * QUOTES.length)];

function RotatingQuote({ theme }: { theme: string }) {
  // A generated daily epigraph (mood-matched to the theme) takes the slot;
  // the curated product quotes remain the fallback.
  const gen = generatedEpigraph(theme);
  return (
    <p className="max-w-[360px] animate-[quote-fade_0.8s_ease] text-[13px] text-muted-foreground">
      {gen ? `“${gen}”` : QUOTE}
    </p>
  );
}
