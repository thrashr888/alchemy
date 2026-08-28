import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type { MetaCitation } from "@/lib/types";
import { cn } from "@/lib/utils";
import { AlchemySymbol } from "./AlchemyHero";
import { DitherBackground } from "./DitherBackground";
import { Markdown } from "./Markdown";
import { Button, LiveRegion, Spinner, Textarea, useConfirm } from "./ui";
import {
  AlertTriangle,
  ArrowUp,
  Eraser,
  FileText,
  Package,
  RefreshCw,
  Sparkles,
  Square,
  SquarePen,
} from "lucide-react";

const COMPOSER_MAX_H = 180;

/** Persistent, corpus-wide chat for Home. Retrieval and citation semantics
 * are the existing ask_everything contract; this component is the durable
 * reading/composing surface around it. */
export function HomeChat() {
  const turns = useStore((s) => s.homeChatTurns);
  const sending = useStore((s) => s.homeChatSending);
  const question = useStore((s) => s.homeChatQuestion);
  const streamingText = useStore((s) => s.homeChatStreamingText);
  const steps = useStore((s) => s.homeChatSteps);
  const waiting = useStore((s) => s.homeChatWaiting);
  const send = useStore((s) => s.sendHomeMessage);
  const clear = useStore((s) => s.clearHomeChat);
  const theme = useStore((s) => s.theme);
  const glassOn = useStore((s) => s.reading.glass);
  const [draft, setDraft] = useState(
    () => localStorage.getItem("homeChatDraft") ?? "",
  );
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const initialScrollDone = useRef(false);
  const { confirm, dialog } = useConfirm();

  useEffect(() => {
    if (draft) localStorage.setItem("homeChatDraft", draft);
    else localStorage.removeItem("homeChatDraft");
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_H)}px`;
  }, [draft]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el || initialScrollDone.current) return;
    el.scrollTop = el.scrollHeight;
    initialScrollDone.current = true;
  }, [turns.length]);

  // Follow a live answer only while the reader is already near the bottom;
  // scrolling up to inspect an earlier citation must remain stable.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
    if (nearBottom) el.scrollTop = el.scrollHeight;
  }, [turns, streamingText, steps, waiting]);

  const [announcements, setAnnouncements] = useState<
    { id: number; text: string }[]
  >([]);
  const announcementSeq = useRef(0);
  const wasSending = useRef(false);
  useEffect(() => {
    if (sending === wasSending.current) return;
    wasSending.current = sending;
    if (sending) {
      setAnnouncements([
        { id: ++announcementSeq.current, text: "Searching your library." },
      ]);
      return;
    }
    const last = turns[turns.length - 1];
    if (!last) return;
    if (last.status === "error") {
      setAnnouncements([
        { id: ++announcementSeq.current, text: `The answer failed. ${last.answer}` },
      ]);
      return;
    }
    const count = last.citations.length;
    setAnnouncements([
      {
        id: ++announcementSeq.current,
        text: `Answer ready. ${count === 0 ? "No citations" : count === 1 ? "1 citation" : `${count} citations`}.`,
      },
    ]);
  }, [sending, turns]);

  const citedNotebooks = useMemo(() => {
    const seen = new Map<string, string>();
    for (const turn of turns) {
      for (const citation of turn.citations) {
        if (citation.notebookId && !seen.has(citation.notebookId))
          seen.set(citation.notebookId, citation.notebookTitle);
      }
    }
    return seen;
  }, [turns]);

  function submit() {
    const text = draft.trim();
    if (!text || sending) return;
    const state = useStore.getState();
    if (state.metaAskOwner && state.metaAskOwner !== "home") {
      state.pushToast(
        "info",
        "Another library answer is already running. Stop it before starting Home Chat.",
      );
      return;
    }
    setDraft("");
    void send(text);
    requestAnimationFrame(() => {
      const el = scrollRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    });
  }

  function openCitation(citation: MetaCitation) {
    void (async () => {
      const state = useStore.getState();
      if (citation.kind === "card") {
        state.openHomeSection("registry", citation.id);
      } else if (citation.kind === "note") {
        useStore.setState({ justCreatedNoteId: citation.id });
        if (!state.studioOpen) state.toggleStudio();
        await state.selectNotebook(citation.notebookId);
      } else {
        await state.selectNotebook(citation.notebookId);
        useStore
          .getState()
          .openSourceViewer(citation.id, citation.title, citation.snippet);
      }
    })();
  }

  const blank = turns.length === 0 && !sending;
  const progress =
    waiting || steps[steps.length - 1] || "Searching your entire library…";

  return (
    <div className="relative flex min-h-0 flex-1 flex-col overflow-hidden">
      <LiveRegion announcements={announcements} />
      {blank && !glassOn && (
        <>
          <div className="glass-mist pointer-events-none absolute inset-0 z-0">
            <DitherBackground themeKey={theme} />
          </div>
          <div className="chat-mist-fade glass-mist pointer-events-none absolute inset-0 z-0" />
        </>
      )}

      <div className="relative z-10 flex h-12 shrink-0 items-center border-b border-border px-5">
        <Sparkles className="size-4 text-citation" aria-hidden />
        <span className="ml-2 text-caption font-semibold uppercase text-muted-foreground">
          Home chat
        </span>
        <span className="ml-2 truncate text-caption text-subtle-foreground">
          All notebooks + Registry
        </span>
        {turns.length > 0 && (
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto"
            disabled={sending}
            onClick={() =>
              void confirm({
                title: "Clear Home Chat?",
                message: "This removes the locally saved conversation.",
                confirmLabel: "Clear",
                danger: true,
              }).then((ok) => {
                if (ok) clear();
              })
            }
          >
            <Eraser className="size-3.5" aria-hidden />
            Clear
          </Button>
        )}
      </div>

      <div
        ref={scrollRef}
        className="relative z-10 min-h-0 flex-1 overflow-y-auto"
        aria-busy={sending}
      >
        <div className="mx-auto flex min-h-full w-full max-w-[720px] flex-col gap-6 px-5 py-8">
          {blank && (
            <div className="my-auto flex flex-col items-center px-4 py-12 text-center">
              <AlchemySymbol className="mb-6 size-20 text-citation/70" />
              <h1 className="text-balance text-page font-semibold text-foreground">
                Ask across your whole library
              </h1>
              <p className="mt-2 max-w-md text-pretty text-body leading-relaxed text-muted-foreground">
                Home Chat searches indexed sources, notes, reports, and
                Registry cards across every notebook. Citations take you back
                to the exact place an answer came from.
              </p>
            </div>
          )}

          {turns.map((turn) => (
            <article key={turn.id} className="flex flex-col gap-3">
              <div className="ml-auto max-w-[85%] rounded-lg bg-surface-2 px-3.5 py-2.5 text-body leading-relaxed text-foreground">
                <p className="whitespace-pre-wrap text-pretty">{turn.question}</p>
              </div>
              <div className="flex min-w-0 flex-col gap-2">
                <div className="flex items-center gap-2 text-micro font-medium uppercase text-subtle-foreground">
                  <Sparkles className="size-3.5" aria-hidden />
                  Across Alchemy
                </div>
                {turn.status === "error" ? (
                  <div
                    role="alert"
                    className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2.5 text-body text-destructive"
                  >
                    <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
                    <span className="min-w-0 flex-1 text-pretty">{turn.answer}</span>
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={sending}
                      onClick={() => void send(turn.question)}
                    >
                      <RefreshCw className="size-3.5" aria-hidden />
                      Retry
                    </Button>
                  </div>
                ) : (
                  <>
                    <div className="text-body leading-relaxed">
                      <Markdown
                        citations={turn.citations}
                        onCitation={openCitation}
                        citationLabel={(citation) =>
                          `${citation.title || "Untitled"} · ${citation.notebookTitle}`
                        }
                      >
                        {turn.answer}
                      </Markdown>
                    </div>
                    <HomeChatCitations
                      citations={turn.citations}
                      onOpen={openCitation}
                    />
                  </>
                )}
              </div>
            </article>
          ))}

          {sending && (
            <article className="flex flex-col gap-3">
              <div className="ml-auto max-w-[85%] rounded-lg bg-surface-2 px-3.5 py-2.5 text-body leading-relaxed text-foreground">
                <p className="whitespace-pre-wrap text-pretty">{question}</p>
              </div>
              <div className="flex min-w-0 flex-col gap-2">
                <div className="flex items-center gap-2 text-micro font-medium uppercase text-subtle-foreground">
                  <Sparkles className="size-3.5" aria-hidden />
                  Across Alchemy
                </div>
                {streamingText ? (
                  <div className="text-body leading-relaxed">
                    <Markdown>{streamingText}</Markdown>
                  </div>
                ) : (
                  <div className="flex items-center gap-2 py-2 text-caption text-muted-foreground">
                    <Spinner className="size-3.5 shrink-0" />
                    <span className="min-w-0 truncate">{progress}</span>
                    {steps.length > 1 && (
                      <span className="ml-auto shrink-0 tabular-nums text-micro text-subtle-foreground">
                        {steps.length} steps
                      </span>
                    )}
                  </div>
                )}
              </div>
            </article>
          )}

          {!blank && citedNotebooks.size > 0 && (
            <div className="mt-auto flex flex-wrap items-center gap-1.5 border-t border-border pt-4">
              <span className="mr-1 text-micro uppercase text-subtle-foreground">
                In this thread
              </span>
              {[...citedNotebooks].map(([id, title]) => (
                <button
                  key={id}
                  type="button"
                  onClick={() => void useStore.getState().selectNotebook(id)}
                  className="rounded-full border border-border bg-surface px-2 py-0.5 text-micro text-muted-foreground transition-colors hover:border-border-strong hover:text-foreground"
                >
                  {title || "Untitled"}
                </button>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="relative z-10 shrink-0 px-5 pb-5 pt-2">
        <div className="mx-auto max-w-[720px]">
          <div
            className={cn(
              "relative rounded-lg border border-border-strong bg-surface p-2.5 shadow-md transition-colors",
              "focus-within:border-ring/60",
            )}
          >
            <Textarea
              ref={textareaRef}
              rows={1}
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.nativeEvent.isComposing) return;
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  submit();
                }
              }}
              className="max-h-[180px] min-h-6 border-0 bg-transparent px-1.5 py-1 focus:ring-0"
              placeholder="Ask across all notebooks…"
              aria-label="Ask Home Chat across all notebooks"
            />
            <div className="mt-2 flex items-center gap-2 pl-1.5">
              <span className="min-w-0 flex-1 truncate text-micro text-subtle-foreground">
                Sources, notes, reports, and Registry cards
              </span>
              {sending ? (
                <Button
                  variant="secondary"
                  size="icon"
                  onClick={() => void api.cancelGeneration("meta")}
                  aria-label="Stop Home Chat answer"
                  title="Stop"
                >
                  <Square className="size-3 fill-current" aria-hidden />
                </Button>
              ) : (
                <Button
                  variant="primary"
                  size="icon"
                  disabled={!draft.trim()}
                  onClick={submit}
                  aria-label="Send to Home Chat"
                  title="Send"
                >
                  <ArrowUp className="size-4" aria-hidden />
                </Button>
              )}
            </div>
          </div>
        </div>
      </div>
      {dialog}
    </div>
  );
}

function HomeChatCitations({
  citations,
  onOpen,
}: {
  citations: MetaCitation[];
  onOpen: (citation: MetaCitation) => void;
}) {
  if (citations.length === 0) return null;
  return (
    <div className="mt-1 flex flex-col gap-0.5 border-t border-border pt-2.5">
      {citations.map((citation, index) => (
        <button
          key={`${citation.kind}-${citation.id}-${index}`}
          type="button"
          onClick={() => onOpen(citation)}
          className="group flex items-center gap-2 rounded-md px-1.5 py-1 text-left text-caption text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
        >
          <span className="shrink-0 tabular-nums text-badge text-subtle-foreground">
            [{index + 1}]
          </span>
          {citation.kind === "card" ? (
            <Package className="size-3 shrink-0" aria-hidden />
          ) : citation.kind === "note" ? (
            <SquarePen className="size-3 shrink-0" aria-hidden />
          ) : (
            <FileText className="size-3 shrink-0" aria-hidden />
          )}
          <span className="min-w-0 flex-1 truncate">
            {citation.title || "Untitled"}
          </span>
          <span className="shrink-0 text-micro text-subtle-foreground">
            {citation.notebookTitle}
          </span>
        </button>
      ))}
    </div>
  );
}
