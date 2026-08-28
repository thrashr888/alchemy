import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "@/lib/api";
import { openMetaCitation } from "@/lib/citations";
import { useStore } from "@/lib/store";
import { cn, chatReadingClass, relativeTime } from "@/lib/utils";
import type { MetaCitation, MetaTurn } from "@/lib/types";
import { Markdown } from "./Markdown";
import { Button, EmptyState, RowMenu, StepTrail, useConfirm } from "./ui";
import {
  AlertTriangle,
  FileText,
  MessagesSquare,
  Package,
  Plus,
  Sparkles,
  SquarePen,
  Trash2,
} from "lucide-react";

/**
 * Home chat — the corpus-wide conversation (docs/RFC-meta-chat.md) with room
 * to think. The ⌘K palette answers one question at a glance; Home keeps the
 * thread, so "which notebook holds the SNDK data?" can be followed by "and
 * what did I conclude about it?" without re-establishing the subject.
 *
 * Threads are durable (the `meta_turns` table): the Chat tab can be left and
 * returned to, back/forward lands on a conversation, and a relaunch reopens
 * the one that was on screen. The turns live in the store, not here, so a
 * citation excursion into a notebook doesn't throw the thread away either.
 */
export interface HomeChat {
  turns: MetaTurn[];
  /** Tokens of the answer currently arriving. */
  streaming: string;
  /** Completed pipeline stages, then the transient line under them. */
  steps: string[];
  waiting: string;
  loading: boolean;
  /** An answer is out there for this thread — this view's run, or one that
   *  outlived a trip into a notebook. */
  pending: boolean;
  ask: (question: string) => void;
  stop: () => void;
}

/** One conversation per window, so the run counter is module-wide: a settling
 *  answer checks it before writing, which is how a superseded run stays
 *  superseded even though the store outlives the view. */
let runSeq = 0;
/** The thread a live run is answering into, or null when nothing is running.
 *  A question with no answer under it means "still working" only while this
 *  is set — after a relaunch it isn't, and a dangling question is just the
 *  last thing that happened. */
let liveThread: string | null = null;

/** What the backend sees as prior context: completed exchanges only. A
 *  provider failure leaves a dangling question that would only teach the
 *  model that answers can be error messages. */
function historyOf(turns: MetaTurn[]): { role: string; content: string }[] {
  const out: { role: string; content: string }[] = [];
  for (let i = 0; i + 1 < turns.length; i++) {
    const q = turns[i];
    const a = turns[i + 1];
    if (q.role === "user" && a.role === "assistant" && a.kind !== "error") {
      out.push(
        { role: "user", content: q.content },
        { role: "assistant", content: a.content },
      );
    }
  }
  return out;
}

/** The conversation's state machine. The settled turns live in the store
 *  (and in the database under them); the in-flight run is local, and dies
 *  with the view that started it. */
export function useHomeChat(): HomeChat {
  const turns = useStore((s) => s.homeChat.turns);
  const threadId = useStore((s) => s.homeChat.threadId);
  const [streaming, setStreaming] = useState("");
  const [steps, setSteps] = useState<string[]>([]);
  const [waiting, setWaiting] = useState("");
  const [loading, setLoading] = useState(false);
  // `ask` must not be rebuilt on every token of the answer, but still has to
  // see whether a run is up; a ref carries the flag across renders.
  const loadingRef = useRef(false);
  loadingRef.current = loading;
  const stopped = useRef(false);

  // Switching to another conversation supersedes the run writing into this
  // one: its tokens would otherwise stream in under someone else's question.
  // Compared against the previous value rather than fired on mount, because
  // remounting Home mid-answer must NOT cancel — the answer is still coming.
  const shownThread = useRef(threadId);
  useEffect(() => {
    if (shownThread.current === threadId) return;
    shownThread.current = threadId;
    // Only a run answering into the thread we just LEFT is superseded.
    // Cancelling with nothing in flight (or with the run that belongs to the
    // thread we just arrived at — asking from the shelf opens a new thread
    // and asks into it in one move) would kill the answer being asked for.
    if (liveThread === null || liveThread === threadId) return;
    runSeq++;
    liveThread = null;
    void api.cancelGeneration("meta");
    setLoading(false);
    setStreaming("");
    setSteps([]);
    setWaiting("");
  }, [threadId]);

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

  const ask = useCallback((question: string) => {
    const q = question.trim();
    if (!q || loadingRef.current) return;
    const id = ++runSeq;
    stopped.current = false;
    const store = useStore.getState();
    const prior = historyOf(store.homeChat.turns);
    // The thread id exists before the question does (openHomeThread mints
    // it), so the run is keyed to a conversation that can't change under it.
    liveThread = store.homeChat.threadId;
    void store.appendHomeTurn("user", q, [], "chat");
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
        void useStore
          .getState()
          .appendHomeTurn(
            "assistant",
            res.answer,
            res.citations,
            wasStopped ? "stopped" : "chat",
          );
      })
      .catch((e) => {
        if (id !== runSeq) return;
        void useStore
          .getState()
          .appendHomeTurn(
            "assistant",
            e instanceof Error ? e.message : String(e),
            [],
            "error",
          );
      })
      .finally(() => {
        if (id !== runSeq) return;
        liveThread = null;
        setLoading(false);
        setStreaming("");
        setWaiting("");
      });
  }, []);

  /** Stop streaming but keep what arrived: the backend resolves a cancelled
   *  run with the partial answer and its citations. */
  const stop = useCallback(() => {
    if (!loadingRef.current) return;
    stopped.current = true;
    void api.cancelGeneration("meta");
  }, []);

  // Leaving Home mid-answer (a citation click opens its notebook) does NOT
  // cancel: the turns land in the store and the database, not in this
  // component, so the answer still arrives and is waiting when the
  // conversation comes back. Only the live token stream is lost.
  const pending =
    loading ||
    (liveThread !== null &&
      liveThread === threadId &&
      turns[turns.length - 1]?.role === "user");

  // Esc is the universal cancel: it stops a streaming answer. It no longer
  // throws the conversation away — the thread is a place now, and you leave
  // a place by going somewhere else.
  useEffect(() => {
    if (!loading) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // Anything modal owns Esc first.
      const s = useStore.getState();
      if (s.paletteOpen || s.settingsOpen || s.addSourceOpen) return;
      if (document.querySelector('[role="dialog"]')) return;
      e.preventDefault();
      stop();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [loading, stop]);

  return { turns, streaming, steps, waiting, loading, pending, ask, stop };
}

/** The slim column of past conversations beside the open one: what you
 *  asked, when, and how far it went. Clicking one reopens it in place. */
export function HomeThreadList() {
  const threads = useStore((s) => s.homeThreads);
  const openId = useStore((s) => s.homeChat.threadId);
  const openThread = useStore((s) => s.openHomeThread);
  const removeThread = useStore((s) => s.deleteHomeThread);
  const { confirm, dialog } = useConfirm();

  // The open thread may not be in the list yet (nothing asked into it), and
  // that's the state the New-chat button leaves you in — so it reads as
  // pressed only when there is genuinely nothing to go back to.
  const openIsSaved = threads.some((t) => t.id === openId);

  return (
    <div className="flex w-[190px] shrink-0 flex-col border-r border-border pt-6">
      <div className="px-3 pb-2">
        <Button
          variant="secondary"
          size="sm"
          className="w-full justify-start"
          disabled={!openIsSaved}
          onClick={() => void openThread(null)}
          title="Start a new conversation"
        >
          <Plus className="h-3.5 w-3.5" />
          New chat
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-4">
        {threads.length === 0 ? (
          <p className="px-1 py-3 text-caption text-subtle-foreground">
            Past conversations are listed here.
          </p>
        ) : (
          threads.map((t) => (
            <div
              key={t.id}
              className={cn(
                "group relative flex items-start rounded-md transition-colors",
                t.id === openId ? "bg-surface-2" : "hover:bg-surface-2",
              )}
            >
              <button
                type="button"
                onClick={() => void openThread(t.id)}
                title={t.title}
                aria-current={t.id === openId}
                className="min-w-0 flex-1 px-2 py-1.5 text-left"
              >
                <span
                  className={cn(
                    "block truncate text-caption",
                    t.id === openId
                      ? "font-medium text-foreground"
                      : "text-muted-foreground",
                  )}
                >
                  {t.title}
                </span>
                <span className="mt-0.5 block truncate text-micro text-subtle-foreground">
                  {relativeTime(t.updatedAt)} · {t.turnCount}{" "}
                  {t.turnCount === 1 ? "turn" : "turns"}
                </span>
              </button>
              <div className="pr-1 pt-1">
                <RowMenu
                  label={`Options for ${t.title}`}
                  items={[
                    {
                      label: "Delete…",
                      icon: <Trash2 className="h-3.5 w-3.5" />,
                      danger: true,
                      onClick: async () => {
                        if (
                          await confirm({
                            title: "Delete this conversation?",
                            message: `"${t.title}" and its ${t.turnCount} ${
                              t.turnCount === 1 ? "turn" : "turns"
                            } are deleted permanently.`,
                            confirmLabel: "Delete",
                            danger: true,
                          })
                        )
                          void removeThread(t.id);
                      },
                    },
                  ]}
                />
              </div>
            </div>
          ))
        )}
      </div>
      {dialog}
    </div>
  );
}

/** The conversation itself: scrolls under Home's pinned composer. */
export function HomeChatThread({ chat }: { chat: HomeChat }) {
  const reading = useStore((s) => s.reading);
  const endRef = useRef<HTMLDivElement>(null);

  // Follow the answer down. Streaming updates are already batched per frame,
  // so this rides along with them rather than scheduling its own.
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [chat.turns.length, chat.streaming, chat.steps.length, chat.waiting]);

  if (chat.turns.length === 0 && !chat.pending) {
    return (
      <div className="relative z-10 flex min-h-0 flex-1 items-center justify-center px-6 pb-10">
        <EmptyState
          icon={<MessagesSquare className="h-5 w-5" />}
          title="Ask across everything"
          hint="One question, every notebook. Answers cite the notebook and source they came from, and the conversation is kept."
        />
      </div>
    );
  }

  return (
    <div className="relative z-10 min-h-0 flex-1 overflow-y-auto">
      <div
        className={cn(
          "mx-auto flex w-full max-w-[760px] flex-col gap-6 px-6 pb-10 pt-1",
          chatReadingClass(reading),
        )}
      >
        {chat.turns.map((turn) =>
          turn.role === "user" ? (
            <div key={turn.id} className="flex justify-end">
              {/* wrap-anywhere: a pasted URL has no break opportunities, so
                  without it the bubble sizes to the URL. */}
              <div className="max-w-[85%] min-w-0 wrap-anywhere rounded-lg rounded-br-sm border border-border bg-surface-2 px-3.5 py-2 text-body selectable">
                {turn.content}
              </div>
            </div>
          ) : turn.kind === "error" ? (
            <div
              key={turn.id}
              role="alert"
              className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-3.5 py-2.5 text-body text-foreground"
            >
              <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" />
              <span className="selectable min-w-0 whitespace-pre-line">
                {turn.content}
              </span>
            </div>
          ) : (
            <div key={turn.id} className="flex flex-col gap-2">
              <AnswerLabel stopped={turn.kind === "stopped"} />
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
        {chat.pending && (
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
