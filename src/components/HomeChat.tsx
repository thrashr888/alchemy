import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "@/lib/api";
import { openMetaCitation } from "@/lib/citations";
import { useStore } from "@/lib/store";
import { cn, chatReadingClass } from "@/lib/utils";
import type { MetaCitation, MetaTurn } from "@/lib/types";
import { Markdown } from "./Markdown";
import { StepTrail } from "./ui";
import { AlertTriangle, FileText, Package, Sparkles, SquarePen } from "lucide-react";

/**
 * Home chat — the corpus-wide conversation (docs/RFC-meta-chat.md) with room
 * to think. The ⌘K palette answers one question at a glance; Home keeps the
 * thread, so "which notebook holds the SNDK data?" can be followed by "and
 * what did I conclude about it?" without re-establishing the subject.
 *
 * Ephemeral by design: the backend persists no meta-chat turns, and closing
 * the thread throws it away. A rerun is cheap and the corpus has moved on.
 */
export interface HomeChat {
  turns: MetaTurn[];
  /** Tokens of the answer currently arriving. */
  streaming: string;
  /** Completed pipeline stages, then the transient line under them. */
  steps: string[];
  waiting: string;
  loading: boolean;
  /** There is a conversation on screen — Home hands it the center column. */
  active: boolean;
  ask: (question: string) => void;
  stop: () => void;
  close: () => void;
}

function appendTurn(turn: MetaTurn) {
  useStore.setState((s) => ({ homeChat: [...s.homeChat, turn] }));
}

/** One conversation per window, so the run counter is module-wide: a settling
 *  answer checks it before writing, which is how a closed (or superseded)
 *  run stays closed even though the store outlives the view. */
let runSeq = 0;

/** Throw the thread away and give Home its shelf back. Exported because the
 *  section tabs are a way out of the conversation too — a tab that silently
 *  did nothing while a conversation was up would just look broken. */
export function closeHomeChat() {
  if (useStore.getState().homeChat.length === 0) return;
  runSeq++;
  void api.cancelGeneration("meta");
  useStore.setState({ homeChat: [] });
}

/** What the backend sees as prior context: completed exchanges only. A
 *  provider failure leaves a dangling question that would only teach the
 *  model that answers can be error messages. */
function historyOf(turns: MetaTurn[]): { role: string; content: string }[] {
  const out: { role: string; content: string }[] = [];
  for (let i = 0; i + 1 < turns.length; i++) {
    const q = turns[i];
    const a = turns[i + 1];
    if (q.role === "user" && a.role === "assistant" && !a.error) {
      out.push(
        { role: "user", content: q.content },
        { role: "assistant", content: a.content },
      );
    }
  }
  return out;
}

/** The conversation's state machine. The settled turns live in the store so
 *  a citation excursion into a notebook doesn't destroy the thread; the
 *  in-flight run is local, and dies with the view that started it. */
export function useHomeChat(): HomeChat {
  const turns = useStore((s) => s.homeChat);
  const [streaming, setStreaming] = useState("");
  const [steps, setSteps] = useState<string[]>([]);
  const [waiting, setWaiting] = useState("");
  const [loading, setLoading] = useState(false);
  const stopped = useRef(false);

  // Closing the thread (from here, the tabs, or Esc) empties the store; the
  // live run's leftovers go with it.
  useEffect(() => {
    if (turns.length > 0) return;
    setLoading(false);
    setStreaming("");
    setSteps([]);
    setWaiting("");
  }, [turns.length]);

  // Stream tokens into the live answer — batched per frame, or every token
  // re-parses the whole accumulated markdown.
  useEffect(() => {
    if (!loading) return;
    let buffer = "";
    let flush = 0;
    const un = listen<{ content: string }>("meta://token", (e) => {
      buffer += e.payload.content;
      if (flush !== 0) return;
      flush = requestAnimationFrame(() => {
        flush = 0;
        const chunk = buffer;
        buffer = "";
        if (chunk) setStreaming((t) => t + chunk);
      });
    });
    return () => {
      if (flush !== 0) cancelAnimationFrame(flush);
      void un.then((f) => f());
    };
  }, [loading]);

  // The pipeline narrates itself: routing → searching → reading → synthesizing.
  // Transient lines replace each other ("Reading X (2 of 6)"); the rest tick
  // off as a trail.
  useEffect(() => {
    if (!loading) return;
    const un = listen<{ label: string; transient: boolean }>(
      "meta://step",
      (e) => {
        if (e.payload.transient) {
          setWaiting(e.payload.label);
        } else {
          setSteps((s) => [...s, e.payload.label]);
          setWaiting("");
        }
      },
    );
    return () => void un.then((f) => f());
  }, [loading]);

  const ask = useCallback(
    (question: string) => {
      const q = question.trim();
      if (!q || loading) return;
      const id = ++runSeq;
      stopped.current = false;
      const prior = historyOf(useStore.getState().homeChat);
      appendTurn({ role: "user", content: q, citations: [] });
      setStreaming("");
      setSteps([]);
      setWaiting("");
      setLoading(true);
      api
        // No third argument: the backend picks depth per model class (deep
        // rerank on gateways where the extra call is cheap, single-pass local).
        .askEverything(q, prior)
        .then((res) => {
          if (id !== runSeq) return;
          const wasStopped = stopped.current;
          // A stop before the first token leaves nothing to show — the user
          // already knows they cancelled.
          if (wasStopped && !res.answer.trim()) return;
          appendTurn({
            role: "assistant",
            content: res.answer,
            citations: res.citations,
            stopped: wasStopped,
          });
        })
        .catch((e) => {
          if (id !== runSeq) return;
          appendTurn({
            role: "assistant",
            content: e instanceof Error ? e.message : String(e),
            citations: [],
            error: true,
          });
        })
        .finally(() => {
          if (id !== runSeq) return;
          setLoading(false);
          setStreaming("");
          setWaiting("");
        });
    },
    [loading],
  );

  /** Stop streaming but keep what arrived: the backend resolves a cancelled
   *  run with the partial answer and its citations. */
  const stop = useCallback(() => {
    if (!loading) return;
    stopped.current = true;
    void api.cancelGeneration("meta");
  }, [loading]);

  // Leaving Home mid-answer (a citation click opens its notebook) does NOT
  // cancel: the turns land in the store, not in this component, so the answer
  // still arrives and is waiting when the conversation comes back. Only the
  // live token stream is lost, and it was only ever the same text early.
  const active = turns.length > 0;

  // Esc is the universal cancel: it stops a streaming answer, and on a settled
  // thread it closes the conversation. Anything modal owns Esc first.
  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // Anything modal owns Esc first — a dismissal must not also throw away
      // the conversation underneath it.
      const s = useStore.getState();
      if (s.paletteOpen || s.settingsOpen || s.addSourceOpen) return;
      if (document.querySelector('[role="dialog"]')) return;
      e.preventDefault();
      if (loading) stop();
      else closeHomeChat();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active, loading, stop]);

  return {
    turns,
    streaming,
    steps,
    waiting,
    loading,
    active,
    ask,
    stop,
    close: closeHomeChat,
  };
}

/** The conversation itself: scrolls under Home's pinned composer. */
export function HomeChatThread({ chat }: { chat: HomeChat }) {
  const reading = useStore((s) => s.reading);
  const endRef = useRef<HTMLDivElement>(null);
  // A trailing question with no answer under it means a run is still out
  // there — either this view's, or one that outlived a trip into a notebook.
  const pending =
    chat.loading || chat.turns[chat.turns.length - 1]?.role === "user";

  // Follow the answer down. Streaming updates are already batched per frame,
  // so this rides along with them rather than scheduling its own.
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [chat.turns.length, chat.streaming, chat.steps.length, chat.waiting]);

  return (
    <div className="relative z-10 min-h-0 flex-1 overflow-y-auto">
      <div
        className={cn(
          "mx-auto flex w-full max-w-[760px] flex-col gap-6 px-6 pb-10 pt-1",
          chatReadingClass(reading),
        )}
      >
        {chat.turns.map((turn, i) =>
          turn.role === "user" ? (
            <div key={i} className="flex justify-end">
              {/* wrap-anywhere: a pasted URL has no break opportunities, so
                  without it the bubble sizes to the URL. */}
              <div className="max-w-[85%] min-w-0 wrap-anywhere rounded-lg rounded-br-sm border border-border bg-surface-2 px-3.5 py-2 text-body selectable">
                {turn.content}
              </div>
            </div>
          ) : turn.error ? (
            <div
              key={i}
              role="alert"
              className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-3.5 py-2.5 text-body text-foreground"
            >
              <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" />
              <span className="selectable min-w-0 whitespace-pre-line">
                {turn.content}
              </span>
            </div>
          ) : (
            <div key={i} className="flex flex-col gap-2">
              <AnswerLabel stopped={turn.stopped} />
              <Markdown
                citations={turn.citations}
                onCitation={openCitation}
                citationLabel={(c) =>
                  `${c.title || "Untitled"} · ${c.notebookTitle}`
                }
              >
                {turn.content}
              </Markdown>
              <MetaCitations citations={turn.citations} />
            </div>
          ),
        )}
        {pending && (
          <div className="flex flex-col gap-2" aria-busy="true">
            <AnswerLabel />
            {chat.loading ? (
              <>
                {(chat.steps.length > 0 || chat.waiting) && (
                  <StepTrail
                    steps={chat.steps}
                    waiting={chat.waiting}
                    done={!!chat.streaming}
                  />
                )}
                {chat.streaming ? (
                  <Markdown>{chat.streaming}</Markdown>
                ) : (
                  chat.steps.length === 0 &&
                  !chat.waiting && (
                    <div className="text-caption text-muted-foreground">
                      Searching every notebook…
                    </div>
                  )
                )}
              </>
            ) : (
              // Returned to the thread while its answer was still being
              // written: the run outlived this view, so say so rather than
              // leave the question hanging with nothing under it.
              <div className="text-caption text-muted-foreground">
                Still working on this answer…
              </div>
            )}
          </div>
        )}
        <div ref={endRef} />
      </div>
    </div>
  );
}

/** Same role label the notebook chat uses — the header above already says
 *  what the scope is, so each turn only has to say who is speaking. */
function AnswerLabel({ stopped }: { stopped?: boolean }) {
  return (
    <div className="flex items-center gap-1.5 text-micro font-medium text-muted-foreground">
      <Sparkles className="h-3 w-3 text-primary" />
      Assistant
      {stopped && <span className="text-subtle-foreground">· stopped</span>}
    </div>
  );
}

function openCitation(c: MetaCitation) {
  void openMetaCitation(c);
}

/** The passages behind an answer, each naming the notebook it lives in and
 *  each one click from the source reader or note card that holds it. */
function MetaCitations({ citations }: { citations: MetaCitation[] }) {
  if (citations.length === 0) return null;
  return (
    <div className="mt-1 flex flex-col gap-0.5 border-t border-border pt-2">
      {citations.map((c, i) => (
        <button
          key={`${c.kind}-${c.id}-${i}`}
          onClick={() => openCitation(c)}
          title={c.snippet}
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
  );
}
