import { useEffect, useRef, useState } from "react";
import { openMetaCitation } from "@/lib/citations";
import { runForThread } from "@/lib/homeChatRun";
import { useStore } from "@/lib/store";
import { cn, chatReadingClass, relativeTime } from "@/lib/utils";
import type { MetaCitation, MetaTurn } from "@/lib/types";
import { Markdown } from "./Markdown";
import { CitationsToggle, MenuPill, MenuRow, ModelPill } from "./ChatPanel";
import { CHAT_LENGTHS, CHAT_STYLES } from "./settings/SettingsTabs";
import {
  Button,
  EmptyState,
  RowMenu,
  StepTrail,
  Textarea,
  useConfirm,
} from "./ui";
import {
  AlertTriangle,
  FileText,
  MessagesSquare,
  Package,
  PanelLeftClose,
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
  /** Tokens of the answer currently arriving into THIS conversation. */
  streaming: string;
  /** Completed pipeline stages, then the transient line under them. */
  steps: string[];
  waiting: string;
  /** An answer is being written into the conversation on screen. */
  loading: boolean;
  /** Asked, but still waiting for the previous answer to hand the channel
   *  back — the backend answers one corpus question at a time. */
  queued: boolean;
  /** The question that run is answering, shown when the thread's turns
   *  haven't finished loading back in. */
  question: string;
  ask: (question: string) => void;
  stop: () => void;
}

/** A view over the store's conversation, not a state machine.
 *
 *  The run used to live here, in component state, keyed to whoever was on
 *  screen — so switching threads cancelled it and threw its trail away. It
 *  belongs to the CONVERSATION now (`homeRun`, driven by `askHome`), and this
 *  hook only decides how much of it the open thread is entitled to see. */
export function useHomeChat(): HomeChat {
  const turns = useStore((s) => s.homeChat.turns);
  const threadId = useStore((s) => s.homeChat.threadId);
  const run = useStore((s) => s.homeRun);
  const ask = useStore((s) => s.askHome);
  const stop = useStore((s) => s.stopHome);

  // A run belongs to one thread. Looking at another conversation shows that
  // conversation, not someone else's answer arriving.
  const mine = runForThread(run, threadId);
  const loading = !!mine;

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

  return {
    turns,
    streaming: mine?.streaming ?? "",
    steps: mine?.steps ?? [],
    waiting: mine?.waiting ?? "",
    loading,
    queued: !!mine?.queued,
    question: mine?.question ?? "",
    ask: (q: string) => void ask(q),
    stop,
  };
}

/** Home composer controls: how the answer is written, how long it runs, and
 *  which model writes it — the same three-pill grammar the notebook composer
 *  uses, so the two chats are operated the same way.
 *
 *  Style and length are Home's OWN (`homeChatConfig`, persisted per surface):
 *  asking across everything is a different job from asking inside one
 *  notebook, and neither should quietly reset the other. The model is the
 *  app's single choice — `ModelPill` writes `AiConfig`, exactly as it does in
 *  a notebook — so picking here is picking everywhere, as it already was. */
export function HomeChatControls() {
  const config = useStore((s) => s.homeChatConfig);
  const setConfig = useStore((s) => s.setHomeChatConfig);
  const [open, setOpen] = useState<null | "style" | "length">(null);
  const style = CHAT_STYLES.find((s) => s.id === config.style);
  const length = CHAT_LENGTHS.find((l) => l.id === config.length);

  return (
    <span className="inline-flex flex-wrap items-center gap-1">
      <MenuPill
        label={style?.label ?? "Default"}
        muted={config.style === "default"}
        open={open === "style"}
        onToggle={() => setOpen((o) => (o === "style" ? null : "style"))}
        onClose={() => setOpen(null)}
        title="How answers across your notebooks are written"
        menuLabel="Style"
        // The custom prompt needs a field wide enough to read back.
        wide={config.style === "custom"}
      >
        {CHAT_STYLES.map((s) => (
          <MenuRow
            key={s.id}
            label={s.label}
            selected={config.style === s.id}
            autoFocus={s.id === config.style}
            onPick={() => {
              setConfig({ ...config, style: s.id });
              // Custom needs somewhere to type; every other pick is done.
              if (s.id !== "custom") setOpen(null);
            }}
          />
        ))}
        {config.style === "custom" && (
          <>
            <div className="mx-2 my-1 h-px bg-border" />
            <div className="px-2 pb-1.5">
              <Textarea
                rows={3}
                aria-label="Custom conversational style"
                placeholder="Act as a skeptical peer reviewer…"
                value={config.customPrompt}
                onChange={(e) =>
                  setConfig({ ...config, customPrompt: e.target.value })
                }
              />
            </div>
          </>
        )}
      </MenuPill>

      <MenuPill
        label={length?.label ?? "Balanced"}
        muted={config.length === "default"}
        open={open === "length"}
        onToggle={() => setOpen((o) => (o === "length" ? null : "length"))}
        onClose={() => setOpen(null)}
        title="How long those answers run"
        menuLabel="Length"
      >
        {CHAT_LENGTHS.map((l) => (
          <MenuRow
            key={l.id}
            label={l.label}
            selected={config.length === l.id}
            autoFocus={l.id === config.length}
            onPick={() => {
              setConfig({ ...config, length: l.id });
              setOpen(null);
            }}
          />
        ))}
      </MenuPill>

      <ModelPill scope="every notebook" />
    </span>
  );
}

/** Past conversations, as the second card of Home's left rail — the same
 *  stacked side-card the Brief and the reports feed make on the right, under
 *  Staff rather than beside the answer. What you asked, when, and how far it
 *  went; clicking one reopens it in the center column. */
export function HomeThreadsSidebar({
  className,
  style,
  resizeHandle,
  onCollapse,
}: {
  className?: string;
  style?: React.CSSProperties;
  /** The left column's width handle, rendered on this card's edge. */
  resizeHandle?: React.ReactNode;
  /** Fold the card down to the rail, as Staff above it and Brief opposite. */
  onCollapse?: () => void;
}) {
  const threads = useStore((s) => s.homeThreads);
  const openId = useStore((s) => s.homeChat.threadId);
  const runningId = useStore((s) => s.homeRun?.threadId ?? null);
  const openThread = useStore((s) => s.openHomeThread);
  const removeThread = useStore((s) => s.deleteHomeThread);
  const { confirm, dialog } = useConfirm();

  // The open thread may not be in the list yet (nothing asked into it), and
  // that's the state the New-chat button leaves you in — so it reads as
  // pressed only when there is genuinely nothing to go back to.
  const openIsSaved = threads.some((t) => t.id === openId);

  return (
    <section
      className={cn("side-card relative flex min-h-0 flex-col", className)}
      style={style}
    >
      {resizeHandle}
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-4">
        <MessagesSquare className="h-3.5 w-3.5 text-muted-foreground" />
        <span className="text-caption font-semibold uppercase tracking-wide text-muted-foreground">
          Chats
        </span>
        {threads.length > 0 && (
          <span className="text-micro tabular-nums text-subtle-foreground">
            {threads.length}
          </span>
        )}
        <div className="ml-auto flex items-center gap-1">
          <Button
            variant="ghost"
            size="sm"
            disabled={!openIsSaved}
            onClick={() => void openThread(null)}
            title="Start a new conversation"
          >
            <Plus className="h-3.5 w-3.5" />
            New chat
          </Button>
          {onCollapse && (
            <button
              type="button"
              onClick={onCollapse}
              title="Collapse Chats"
              aria-label="Collapse the Chats sidebar"
              className="rounded p-1 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
            >
              <PanelLeftClose className="h-4 w-4" />
            </button>
          )}
        </div>
      </div>
      {/* Rows were flush against each other, which read as one block of text
          rather than a list. A gap (the reports feed's idiom) separates them
          without adding a rule between every pair. */}
      <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 py-2">
        {threads.length === 0 ? (
          <p className="px-1 py-3 text-caption text-subtle-foreground">
            Past conversations are listed here.
          </p>
        ) : (
          threads.map((t) => (
            <div
              key={t.id}
              className={cn(
                "group relative flex shrink-0 items-start rounded-md transition-colors",
                t.id === openId ? "bg-surface-2" : "hover:bg-surface-2",
              )}
            >
              <button
                type="button"
                onClick={() => void openThread(t.id)}
                title={t.title}
                aria-current={t.id === openId}
                className="min-w-0 flex-1 px-2 py-2 text-left"
              >
                <span
                  className={cn(
                    "block truncate text-body",
                    t.id === openId
                      ? "font-medium text-foreground"
                      : "text-muted-foreground",
                  )}
                >
                  {t.title}
                </span>
                <span className="mt-0.5 block truncate text-micro text-subtle-foreground">
                  {/* A run keeps going in the thread it was asked in, so the
                      list says which conversation is still being answered. */}
                  {runningId === t.id ? (
                    <span className="text-muted-foreground">Answering…</span>
                  ) : (
                    <>
                      {relativeTime(t.updatedAt)} · {t.turnCount}{" "}
                      {t.turnCount === 1 ? "turn" : "turns"}
                    </>
                  )}
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
    </section>
  );
}

/** The conversation itself: the scrolling middle, between Home's heading and
 *  the composer docked below it. */
export function HomeChatThread({ chat }: { chat: HomeChat }) {
  const reading = useStore((s) => s.reading);
  const endRef = useRef<HTMLDivElement>(null);

  // Follow the answer down. Streaming updates are already batched per frame,
  // so this rides along with them rather than scheduling its own.
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [chat.turns.length, chat.streaming, chat.steps.length, chat.waiting]);

  if (chat.turns.length === 0 && !chat.loading) {
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
        {/* A queued question hasn't been written to the thread yet — show it
            where it will land, so the conversation doesn't stall on a
            composer that already emptied itself. */}
        {chat.queued && chat.question && (
          <div className="flex justify-end">
            <div className="max-w-[85%] min-w-0 wrap-anywhere rounded-lg rounded-br-sm border border-border bg-surface-2 px-3.5 py-2 text-body text-muted-foreground selectable">
              {chat.question}
            </div>
          </div>
        )}
        {/* The run's own state, read from the store: leaving this thread and
            coming back finds the trail and the partial answer where they
            were, because neither ever belonged to this component. */}
        {chat.loading && (
          <div className="flex flex-col gap-2" aria-busy="true">
            <AnswerLabel />
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
 *  each one click from the source reader or note card that holds it.
 *
 *  Folded away by default, the way a notebook answer's citations are: a
 *  corpus answer can cite a dozen sources, and the list was costing more
 *  vertical space than the answer itself. The inline [n] chips in the prose
 *  stay clickable either way, so nothing is behind the fold that a reader
 *  needs — this is the receipts, not the route. */
function MetaCitations({ citations }: { citations: MetaCitation[] }) {
  const [open, setOpen] = useState(false);
  if (citations.length === 0) return null;
  return (
    <div className="mt-1">
      <CitationsToggle
        count={citations.length}
        open={open}
        onToggle={() => setOpen((o) => !o)}
      />
      {open && (
        <div className="mt-2 flex flex-col gap-0.5">
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
      )}
    </div>
  );
}
