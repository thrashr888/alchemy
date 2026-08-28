import type { MetaCitation } from "./types";

/** Home Chat is intentionally compact local history, not another unbounded
 * artifact store. The backend still fits the resulting prompt to the active
 * model; this cap keeps the app snapshot and relaunch restore predictable. */
export const HOME_CHAT_MAX_TURNS = 24;

export interface HomeChatTurn {
  id: string;
  question: string;
  answer: string;
  citations: MetaCitation[];
  status: "complete" | "error";
  createdAt: number;
}

function isCitation(value: unknown): value is MetaCitation {
  if (!value || typeof value !== "object") return false;
  const c = value as Record<string, unknown>;
  return (
    (c.kind === "source" || c.kind === "note" || c.kind === "card") &&
    typeof c.notebookId === "string" &&
    typeof c.notebookTitle === "string" &&
    typeof c.id === "string" &&
    typeof c.title === "string" &&
    typeof c.snippet === "string"
  );
}

function isTurn(value: unknown): value is HomeChatTurn {
  if (!value || typeof value !== "object") return false;
  const turn = value as Record<string, unknown>;
  return (
    typeof turn.id === "string" &&
    typeof turn.question === "string" &&
    turn.question.trim().length > 0 &&
    typeof turn.answer === "string" &&
    (turn.status === "complete" || turn.status === "error") &&
    typeof turn.createdAt === "number" &&
    Number.isFinite(turn.createdAt) &&
    Array.isArray(turn.citations)
  );
}

/** Parse the relaunch snapshot defensively. A corrupt or older shape must not
 * keep Home from rendering; malformed citations are simply unavailable. */
export function parseHomeChatTurns(raw: string | null): HomeChatTurn[] {
  if (!raw) return [];
  try {
    const value: unknown = JSON.parse(raw);
    if (!Array.isArray(value)) return [];
    return value
      .filter(isTurn)
      .map((turn) => ({
        ...turn,
        citations: turn.citations.filter(isCitation),
      }))
      .slice(-HOME_CHAT_MAX_TURNS);
  } catch {
    return [];
  }
}

export function appendHomeChatTurn(
  turns: HomeChatTurn[],
  turn: HomeChatTurn,
): HomeChatTurn[] {
  return [...turns, turn].slice(-HOME_CHAT_MAX_TURNS);
}

/** Only successful answers become model context. Error copy is UI state, not
 * something the next answer should treat as a claim about the user's corpus. */
export function homeChatHistory(turns: HomeChatTurn[]) {
  return turns
    .filter((turn) => turn.status === "complete" && turn.answer.trim())
    .flatMap((turn) => [
      { role: "user", content: turn.question },
      { role: "assistant", content: turn.answer },
    ]);
}
