import type { AcpUpdateEvent, MetaCitation, MetaTurn } from "./types";
import type { HomeRun } from "./storeTypes";

/**
 * The rules a Home (corpus-wide) conversation is run by, as plain functions
 * — the store wires them to Tauri, and these are the parts worth pinning
 * down on their own.
 *
 * The organising idea: a run belongs to the CONVERSATION it was asked in, not
 * to the view that started it. Everything here follows from that.
 */

/** What the backend sees as prior context: completed exchanges only. A
 *  provider failure leaves a dangling question that would only teach the
 *  model that answers can be error messages — and a tool confirmation
 *  ("Added 2 sources to Japan trip") is process, not conversation: the
 *  notebook transcript keeps kind "tool" out of model context for the same
 *  reason, and the question that triggered one goes with it. */
export function historyOf(
  turns: MetaTurn[],
): { role: string; content: string }[] {
  const out: { role: string; content: string }[] = [];
  for (let i = 0; i + 1 < turns.length; i++) {
    const q = turns[i];
    const a = turns[i + 1];
    if (
      q.role === "user" &&
      a.role === "assistant" &&
      a.kind !== "error" &&
      a.kind !== "tool"
    ) {
      out.push(
        { role: "user", content: q.content },
        { role: "assistant", content: a.content },
      );
    }
  }
  return out;
}

/** How much earlier conversation a fresh agent session is handed. The local
 *  model gets the whole history every turn; an agent session keeps its own
 *  memory after the first prompt, so this is paid once — but it still rides
 *  a prompt, and a long thread shouldn't spend the agent's window on itself. */
const SEED_CHARS = 6_000;

/** The first prompt of a FRESH agent session in a thread that already has a
 *  conversation (RFC-unified-chat §6, "one thread").
 *
 *  An agent session remembers its own turns, but not the turns another brain
 *  answered: a thread that began on the local model and then reaches the
 *  agent would otherwise meet an agent with no idea what "it" or "that"
 *  refers to. So a new session is handed the conversation so far, newest
 *  exchanges kept when it has to be cut. A resumed session already has its
 *  history, and an empty thread has none — both get the question alone. */
export function seedAgentPrompt(
  history: { role: string; content: string }[],
  question: string,
): string {
  if (history.length === 0) return question;
  const lines: string[] = [];
  let used = 0;
  // Walk back from the newest exchange so the cut, if any, drops the oldest.
  for (let i = history.length - 1; i >= 0; i--) {
    const { role, content } = history[i];
    const line = `${role === "user" ? "User" : "Assistant"}: ${content.trim()}`;
    if (used + line.length > SEED_CHARS && lines.length > 0) break;
    lines.unshift(line);
    used += line.length;
  }
  return `<conversation_so_far>\n${lines.join("\n\n")}\n</conversation_so_far>\n\n${question}`;
}

/** Does this agent failure mean "not signed in"? The wording varies by agent
 *  and adapter ("Authentication required", "OAuth session expired",
 *  "401 Unauthorized"), so this matches the family, not one string. */
export function isAgentAuthFailure(message: string): boolean {
  return /\b(auth(entication|enticate|orization)?|unauthori[sz]ed|oauth|sign(ed)?[ -]?in|log(ged)?[ -]?in|401)\b/i.test(
    message,
  );
}

/** The error a Home agent turn records. A sign-in failure carries the command
 *  that fixes it, because "Authentication required" alone sends the user off
 *  to work out which CLI, and how. */
export function agentFailureMessage(
  message: string,
  label: string,
  loginCommand: string,
): string {
  if (!loginCommand || !isAgentAuthFailure(message)) return message;
  return `${message}\n\n${label} needs you to sign in again: run \`${loginCommand}\` in Terminal, then Retry.`;
}

/** How much of the live run the conversation on screen is entitled to see.
 *  An answer being written into another thread is that thread's business:
 *  it keeps running, but it doesn't appear under someone else's question. */
export function runForThread(
  run: HomeRun | null,
  threadId: string | null,
): HomeRun | null {
  return run && threadId && run.threadId === threadId ? run : null;
}

/** Which slot unsent composer text is kept under. Per conversation inside the
 *  Chat tab; the ask box over the notebook grid has its own, because a
 *  question typed there is a fresh subject, not a follow-up. */
export function homeDraftKey(
  chatOpen: boolean,
  threadId: string | null,
): string {
  return chatOpen ? `t:${threadId ?? "new"}` : "shelf";
}

/** The notebooks an answer drew from, in the order it first cited them — the
 *  palette's notebook chips. A citation into the Registry names no notebook,
 *  so the cast isn't a chip. */
export function citedNotebooks(
  citations: MetaCitation[],
): [id: string, title: string][] {
  const seen = new Map<string, string>();
  for (const c of citations)
    if (c.notebookId && !seen.has(c.notebookId))
      seen.set(c.notebookId, c.notebookTitle);
  return [...seen.entries()];
}

/** Turns just fetched for a thread, merged with what is already on screen for
 *  it. An answer that settled while the fetch was in flight is newer than
 *  what came back; overwriting would blink it away and then bring it back. */
export function mergeLoadedTurns(
  fetched: MetaTurn[],
  onScreen: MetaTurn[],
): MetaTurn[] {
  const known = new Set(fetched.map((t) => t.id));
  return [...fetched, ...onScreen.filter((t) => !known.has(t.id))];
}

/** The ACP session key a Home thread's hosted agent runs under. Sessions are
 *  keyed by notebook id on the backend; a thread borrows the namespace with a
 *  prefix no notebook id can carry, which is also how the backend knows to
 *  give it the corpus-wide preamble instead of one notebook's. */
export function homeAgentKey(threadId: string): string {
  return `home-${threadId}`;
}

/** A Home turn the hosted agent is answering, folded from its session
 *  updates into the shape Home already draws: the answer's text, and a trail
 *  of what it did on the way. */
export interface AgentFold {
  streaming: string;
  /** One line per tool call, in the order the agent made them. */
  steps: string[];
  /** Tool calls by id: which step line each one owns, and whether it has
   *  finished. Status-only updates arrive without an id and mean the newest
   *  call, so the newest is remembered too. */
  tools: Record<string, { step: number; title: string; done: boolean }>;
  lastTool: string | null;
  /** The last prose the agent wrote before a tool call, in full — the answer
   *  if it never writes another word (see `agentAnswer`). */
  lastSaid: string;
}

export const EMPTY_AGENT_FOLD: AgentFold = {
  streaming: "",
  steps: [],
  tools: {},
  lastTool: null,
  lastSaid: "",
};

/** Fold one `session/update` into the turn. Thoughts are left out on
 *  purpose: Home shows what the agent did and what it said, and its
 *  scratchpad would be a third stream in a view built for two. Replayed user
 *  messages never belong to a live turn. */
export function foldAgentUpdate(
  fold: AgentFold,
  update: AcpUpdateEvent["update"],
): AgentFold {
  const kind = update.sessionUpdate;
  if (kind === "agent_message_chunk") {
    const text = update.content?.text ?? "";
    if (!text) return fold;
    return { ...fold, streaming: fold.streaming + text };
  }
  if (kind === "tool_call") {
    // Whatever the agent said before reaching for a tool was narration —
    // "I'll search your notebooks", "I found a summary, checking the rest" —
    // not the answer. It becomes a line in the trail, where the work is
    // shown, and the answer starts over from the next words. Run together,
    // they made the saved answer open with the agent talking to itself (the
    // first live Codex run did exactly that).
    const said = fold.streaming.trim();
    const steps = said ? [...fold.steps, narrationLine(said)] : [...fold.steps];
    const id = String(update.toolCallId ?? `tool-${steps.length}`);
    const title = update.title || "Running a tool";
    const done = isFinished(update.status);
    return {
      ...fold,
      streaming: "",
      lastSaid: said || fold.lastSaid,
      steps: [...steps, stepLabel(title, update.status)],
      tools: { ...fold.tools, [id]: { step: steps.length, title, done } },
      lastTool: id,
    };
  }
  if (kind === "tool_call_update") {
    const id = update.toolCallId ? String(update.toolCallId) : fold.lastTool;
    const tool = id ? fold.tools[id] : undefined;
    if (!id || !tool) return fold;
    const title = update.title || tool.title;
    const done = tool.done || isFinished(update.status);
    const steps = [...fold.steps];
    steps[tool.step] = stepLabel(title, update.status);
    return {
      ...fold,
      steps,
      tools: { ...fold.tools, [id]: { ...tool, title, done } },
    };
  }
  return fold;
}

/** One trail line from a stretch of narration: a step is a glance, so it is
 *  one line, and a long aside is cut rather than wrapping the trail. */
function narrationLine(text: string): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > 160 ? `${line.slice(0, 159).trimEnd()}…` : line;
}

/** The answer an agent turn records: what it wrote after its last tool call.
 *  An agent whose final act was a tool (it said its piece, then saved a note)
 *  has nothing after it, and its last words are the answer — better than
 *  recording a turn that "finished without writing an answer". */
export function agentAnswer(fold: AgentFold): string {
  return fold.streaming.trim() ? fold.streaming : fold.lastSaid;
}

/** How many of the agent's tool calls are still going. */
export function toolsRunning(fold: AgentFold): number {
  return Object.values(fold.tools).filter((t) => !t.done).length;
}

function isFinished(status: string | undefined): boolean {
  return status === "completed" || status === "failed";
}

function stepLabel(title: string, status: string | undefined): string {
  return status === "failed" ? `${title} (failed)` : title;
}
