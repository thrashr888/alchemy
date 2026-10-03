import { useEffect, useRef, useState } from "react";
import { openMetaCitation } from "@/lib/citations";
import { api } from "@/lib/api";
import { homeAgentKey, runForThread } from "@/lib/homeChatRun";
import { navAtomic, useStore } from "@/lib/store";
import { cn, chatReadingClass } from "@/lib/utils";
import type {
  AcpPermissionEvent,
  AgentChange,
  MetaCitation,
  MetaTurn,
} from "@/lib/types";
import { Markdown } from "./Markdown";
import {
  CitationsToggle,
  GrammarActions,
  MenuRow,
  ModelPill,
  ToolRow,
  TurnActions,
  copyAction,
  type TurnAction,
} from "./ChatPanel";
import { AlchemySymbol } from "./AlchemyHero";
import { PermissionPrompt } from "./PermissionPrompt";
import { THEMES, resolveThemeId } from "@/lib/themes";
import { CHAT_LENGTHS, CHAT_STYLES } from "./settings/SettingsTabs";
import { RowMenu, StepTrail, Textarea, useConfirm } from "./ui";
import {
  AlertTriangle,
  FileText,
  Package,
  RefreshCw,
  Sparkles,
  SquarePen,
  Trash2,
  Undo2,
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
  /** The hosted agent writing this answer, when it isn't Alchemy's own
   *  pipeline (RFC-unified-chat §6). */
  agent: string | null;
  /** The agent's tool calls still going — its trail's spinner. */
  toolsRunning: number;
  /** A tool the agent is waiting to be allowed to run. */
  permission: AcpPermissionEvent | null;
  answerPermission: (optionId: string | null) => void;
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
  const answerPermission = useStore((s) => s.answerHomePermission);

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
    agent: mine?.agent ?? null,
    toolsRunning: mine?.toolsRunning ?? 0,
    permission: mine?.permission ?? null,
    answerPermission,
    // Home's own asks may go to the hosted agent when it's the chat
    // provider; the store decides per question (the palette never does).
    ask: (q: string) => void ask(q, { allowAgent: true }),
    stop,
  };
}

/** The composer's one section-label heading, styled exactly like `ModelPill`'s
 *  own "Model"/"Effort" headers — duplicated rather than exported from
 *  ChatPanel because it's three lines of class names, not a component. */
function menuSection(text: string) {
  return (
    <div className="px-2.5 pb-0.5 pt-2 text-micro font-medium uppercase tracking-wide text-subtle-foreground">
      {text}
    </div>
  );
}

/** Home's composer pop-up: how the answer is written, how long it runs, and
 *  which model writes it — folded into sections of `ModelPill`'s one menu
 *  (RFC-mac-chrome, "Sheet (chat)"), the same way the notebook composer
 *  hangs Chat settings and Clear conversation off it. Three separate pills
 *  used to carry these; one pill does now, matching the notebook page.
 *
 *  Style and length are Home's OWN (`homeChatConfig`, persisted per surface):
 *  asking across everything is a different job from asking inside one
 *  notebook, and neither should quietly reset the other. The model is the
 *  app's single choice — `ModelPill` writes `AiConfig`, exactly as it does in
 *  a notebook — so picking here is picking everywhere, as it already was. */
export function HomeChatMenu() {
  const config = useStore((s) => s.homeChatConfig);
  const setConfig = useStore((s) => s.setHomeChatConfig);

  return (
    <ModelPill
      scope="every notebook"
      extraRows={(close) => (
        <>
          <div className="mx-2 my-1 h-px bg-border" />
          {menuSection("Style")}
          {CHAT_STYLES.map((s) => (
            <MenuRow
              key={s.id}
              label={s.label}
              selected={config.style === s.id}
              onPick={() => {
                setConfig({ ...config, style: s.id });
                // Custom needs somewhere to type; every other pick is done.
                if (s.id !== "custom") close();
              }}
            />
          ))}
          {config.style === "custom" && (
            <div className="px-2 pb-1.5 pt-1">
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
          )}

          {menuSection("Length")}
          {CHAT_LENGTHS.map((l) => (
            <MenuRow
              key={l.id}
              label={l.label}
              selected={config.length === l.id}
              onPick={() => {
                setConfig({ ...config, length: l.id });
                close();
              }}
            />
          ))}
        </>
      )}
    />
  );
}

/** How many threads the Chats disclosure shows before folding the rest
 *  behind "Show N more…" — a long history must not push Registry and Tags
 *  off the bottom of a 220px sidebar. */
const CHATS_SIDEBAR_CAP = 12;

/** Sessions nested under the Library's Chats row, Mail-mailbox style
 *  (DESIGN.md §9, "Chats is a NavigationSplitView" — now a sidebar
 *  disclosure rather than a second column). Each is a 28px indented row;
 *  the date/turn-count detail that used to be a second line lives in the
 *  row's tooltip now, since a sidebar row has no room for two lines.
 *
 *  Picking a row switches sections on its way — `openHomeThread` does both,
 *  and `navAtomic` keeps it to one entry in the back stack. The parent
 *  Chats row (drawn by `HomeView`) owns the disclosure's open/closed state
 *  and the New chat button; this component only draws what is open. */
export function HomeChatSidebarThreads() {
  const threads = useStore((s) => s.homeThreads);
  const openId = useStore((s) => s.homeChat.threadId);
  const chatSelected = useStore((s) => s.homeSection === "chat");
  const runningId = useStore((s) => s.homeRun?.threadId ?? null);
  const openThread = useStore((s) => s.openHomeThread);
  const removeThread = useStore((s) => s.deleteHomeThread);
  const { confirm, dialog } = useConfirm();
  const [expanded, setExpanded] = useState(false);

  if (threads.length === 0) {
    return (
      <p className="px-8 py-1 text-caption text-subtle-foreground">
        No conversations yet.
      </p>
    );
  }

  const shown = expanded ? threads : threads.slice(0, CHATS_SIDEBAR_CAP);
  const hidden = threads.length - shown.length;

  return (
    <div className="flex flex-col gap-px">
      {shown.map((t) => {
        const selected = chatSelected && t.id === openId;
        return (
          <div
            key={t.id}
            className={cn(
              "group relative flex h-[30px] shrink-0 items-center rounded-md pl-8 pr-2 transition-colors",
              selected ? "bg-[var(--selection)]" : "hover:bg-surface-2",
            )}
          >
            <button
              type="button"
              onClick={() => void navAtomic(() => openThread(t.id))}
              // The row shows the short name the small model gave the
              // conversation; the tooltip keeps what was actually asked and
              // when, which is what a truncated title is always a lossy
              // stand-in for.
              title={
                runningId === t.id
                  ? t.question || t.title
                  : `${t.question || t.title} — ${new Date(
                      t.updatedAt,
                    ).toLocaleDateString()} · ${t.turnCount} ${
                      t.turnCount === 1 ? "turn" : "turns"
                    }`
              }
              aria-current={selected}
              className={cn(
                "min-w-0 flex-1 truncate text-left text-[0.875rem]",
                selected
                  ? "font-medium text-foreground"
                  : "text-muted-foreground",
              )}
            >
              {runningId === t.id ? (
                <span className="text-muted-foreground">Answering…</span>
              ) : (
                t.title
              )}
            </button>
            <RowMenu
              label={`Options for ${t.title}`}
              items={[
                {
                  label: "Delete…",
                  symbol: "trash",
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
        );
      })}
      {hidden > 0 && (
        <button
          type="button"
          onClick={() => setExpanded(true)}
          className="flex h-[30px] w-full shrink-0 items-center rounded-md pl-8 pr-2 text-left text-[0.875rem] text-subtle-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
        >
          Show {hidden} more…
        </button>
      )}
      {dialog}
    </div>
  );
}

/** The conversation itself: the scrolling middle, between Home's heading and
 *  the composer docked below it. */
export function HomeChatThread({ chat }: { chat: HomeChat }) {
  const undo = useAgentUndo(chat);
  const reading = useStore((s) => s.reading);
  const theme = useStore((s) => s.theme);
  const endRef = useRef<HTMLDivElement>(null);

  // Follow the answer down. Streaming updates are already batched per frame,
  // so this rides along with them rather than scheduling its own.
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [
    chat.turns.length,
    chat.streaming,
    chat.steps.length,
    chat.waiting,
    chat.permission,
  ]);

  // The same blank state the notebook's Chat page shows on a truly empty
  // transcript (`ChatHero`, isBlank): the sigil and one line, no summary
  // card — there's no per-notebook summary to show here anyway.
  if (chat.turns.length === 0 && !chat.loading) {
    return (
      <div className="relative z-10 flex min-h-0 flex-1 flex-col items-center justify-center gap-4 px-6 pb-10">
        <AlchemySymbol
          className="h-16 w-16 text-citation/60"
          strokeWidth={1}
          preferred={THEMES[resolveThemeId(theme)]?.sigil}
        />
        <div className="flex max-w-[360px] flex-col items-center gap-1.5 text-center">
          <div className="text-body font-semibold text-foreground/90">
            Ask across everything
          </div>
          <p className="text-body text-muted-foreground">
            One question, every notebook. Answers cite the notebook and
            source they came from, and the conversation is kept.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="relative z-10 min-h-0 flex-1 overflow-y-auto">
      <div
        className={cn(
          "mx-auto flex w-full max-w-[680px] flex-col gap-6 px-4 py-6",
          chatReadingClass(reading),
        )}
      >
        {undo.dialog}
        {chat.turns.map((turn, i) =>
          turn.role === "user" ? (
            <div key={turn.id} className="group flex flex-col items-end gap-1">
              {/* wrap-anywhere: a pasted URL has no break opportunities, so
                  without it the bubble sizes to the URL. */}
              <div className="max-w-[85%] min-w-0 wrap-anywhere rounded-lg rounded-br-sm border border-border bg-surface-2 px-3.5 py-2 text-body selectable">
                {turn.content}
              </div>
              <TurnActions
                createdAt={turn.createdAt}
                actions={[
                  copyAction(turn.content),
                  rerunAction(turn.content, chat),
                ]}
              />
            </div>
          ) : turn.kind === "tool" ? (
            // A command Home carried out — the same quiet row a notebook
            // transcript gives its tools, so "switch chat to ollama" reads
            // identically wherever it was asked.
            <ToolRow
              key={turn.id}
              content={turn.content}
              actions={
                i === chat.turns.length - 1 && (
                  <GrammarActions content={turn.content} surface="home" />
                )
              }
            />
          ) : turn.kind === "error" ? (
            <div key={turn.id} className="group flex flex-col gap-1">
              <div
                role="alert"
                className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-3.5 py-2.5 text-body text-foreground"
              >
                <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" />
                <span className="selectable min-w-0 whitespace-pre-line">
                  {turn.content}
                </span>
              </div>
              {/* A failure's one useful verb is the question again — the one
                  above it, not this row's own text. */}
              <TurnActions
                createdAt={turn.createdAt}
                actions={retryActions(chat, i, turn.content)}
                model={turn.model}
              />
            </div>
          ) : (
            <div key={turn.id} className="group flex flex-col gap-2">
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
              <TurnActions
                createdAt={turn.createdAt}
                actions={[
                  copyAction(turn.content),
                  ...undo.actionFor(chat.turns, i),
                ]}
                model={turn.model}
              />
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
                // The pipeline's stages are over once the answer starts
                // arriving. An agent's are tools, which it can still be
                // calling between paragraphs, so its trail is done only
                // when none is running.
                done={
                  chat.agent
                    ? !chat.waiting && chat.toolsRunning === 0
                    : !!chat.streaming
                }
              />
            )}
            {chat.streaming ? (
              <Markdown>{chat.streaming}</Markdown>
            ) : (
              chat.steps.length === 0 &&
              !chat.waiting && (
                <div className="text-caption text-muted-foreground">
                  {chat.agent
                    ? `${chat.agent} is working…`
                    : "Searching every notebook…"}
                </div>
              )
            )}
            {chat.permission && (
              <PermissionPrompt
                request={chat.permission}
                agent={chat.agent}
                onAnswer={chat.answerPermission}
              />
            )}
          </div>
        )}
        <div ref={endRef} />
      </div>
    </div>
  );
}

/** Re-run, as the notebook transcript means it: ask the same question again
 *  as a fresh turn at the end of this conversation. The earlier exchange
 *  stays exactly where it is — nothing in the thread is rewritten. */
/** Undo for what an agent changed in this thread (RFC-unified-chat, phase 2).
 *
 *  Alchemy keeps a note's or source's state when the user says Yes to an
 *  agent's update or delete; an answer whose turn made such changes offers
 *  to put them back. A thread's changes are read once and split into turns
 *  by time — each was allowed between its turn's question and its answer. */
function useAgentUndo(chat: HomeChat) {
  const threadId = useStore((s) => s.homeChat.threadId);
  const pushToast = useStore((s) => s.pushToast);
  const { confirm, dialog } = useConfirm();
  const [changes, setChanges] = useState<AgentChange[]>([]);
  const key = threadId ? homeAgentKey(threadId) : null;
  // Re-read when the thread changes or a turn settles: that is when a new
  // change can have been recorded.
  const turnCount = chat.turns.length;
  useEffect(() => {
    if (!key) {
      setChanges([]);
      return;
    }
    let live = true;
    api
      .agentChanges(key, 0, Number.MAX_SAFE_INTEGER)
      .then((c) => {
        if (live) setChanges(c);
      })
      .catch(() => {
        if (live) setChanges([]);
      });
    return () => {
      live = false;
    };
  }, [key, turnCount]);

  const actionFor = (turns: MetaTurn[], i: number): TurnAction[] => {
    if (!key) return [];
    let q = i - 1;
    while (q >= 0 && turns[q].role !== "user") q--;
    if (q < 0) return [];
    const from = turns[q].createdAt;
    const to = turns[i].createdAt;
    const mine = changes.filter(
      (c) => c.at >= from && c.at <= to && !c.undone,
    );
    if (mine.length === 0) return [];
    const agent = mine[0].agent;
    const line = (c: AgentChange) =>
      c.tool.startsWith("delete")
        ? `Bring back “${c.title}”`
        : `Restore “${c.title}”`;
    return [
      {
        label: "Undo changes",
        doneLabel: "Undone",
        icon: <Undo2 className="h-3.5 w-3.5" />,
        title: `Put back what ${agent} changed in this answer`,
        onClick: async () => {
          const ok = await confirm({
            title: `Undo what ${agent} changed?`,
            message:
              "Anything you've edited since is left as it is. Deleted items come back as new copies.",
            items: mine.map(line),
            confirmLabel: "Undo",
          });
          if (!ok) return;
          try {
            const report = await api.undoAgentChanges(key, from, to);
            const done = report.restored.length;
            const summary =
              done > 0
                ? `Put back ${done} ${done === 1 ? "change" : "changes"}.`
                : "Nothing was put back.";
            pushToast(
              report.skipped.length ? "error" : "success",
              report.skipped.length
                ? `${summary} ${report.skipped.join(". ")}.`
                : summary,
            );
          } finally {
            const fresh = await api
              .agentChanges(key, 0, Number.MAX_SAFE_INTEGER)
              .catch(() => [] as AgentChange[]);
            setChanges(fresh);
          }
        },
      },
    ];
  };

  return { actionFor, dialog };
}

function rerunAction(question: string, chat: HomeChat): TurnAction {
  return {
    label: "Re-run",
    icon: <RefreshCw className="h-3.5 w-3.5" />,
    disabled: chat.loading,
    title: "Ask this again as a new turn",
    onClick: () => chat.ask(question),
  };
}

/** What a failed turn offers: copy the message, and retry the question it
 *  failed to answer — which is the turn above it, since the error row's own
 *  text is a provider complaint, not something to re-ask. */
function retryActions(
  chat: HomeChat,
  i: number,
  content: string,
): TurnAction[] {
  const actions = [copyAction(content)];
  for (let j = i - 1; j >= 0; j--) {
    const turn = chat.turns[j];
    if (turn.role !== "user") continue;
    actions.push({
      ...rerunAction(turn.content, chat),
      label: "Retry",
      title: "Ask the question again",
    });
    break;
  }
  return actions;
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
