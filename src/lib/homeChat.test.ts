import { describe, expect, it } from "vitest";
import {
  appendHomeChatTurn,
  HOME_CHAT_MAX_TURNS,
  homeChatHistory,
  parseHomeChatTurns,
  type HomeChatTurn,
} from "./homeChat";

function turn(i: number, status: HomeChatTurn["status"] = "complete"): HomeChatTurn {
  return {
    id: `turn-${i}`,
    question: `Question ${i}`,
    answer: status === "complete" ? `Answer ${i}` : "Provider unavailable",
    citations: [],
    status,
    createdAt: i,
  };
}

describe("Home Chat history", () => {
  it("restores valid turns while dropping malformed data and citations", () => {
    const raw = JSON.stringify([
      {
        ...turn(1),
        citations: [
          {
            kind: "source",
            notebookId: "nb-1",
            notebookTitle: "Research",
            id: "source-1",
            title: "Brief",
            snippet: "Evidence",
          },
          { kind: "source", id: 42 },
        ],
      },
      { question: "missing the rest" },
    ]);

    expect(parseHomeChatTurns(raw)).toEqual([
      {
        ...turn(1),
        citations: [
          {
            kind: "source",
            notebookId: "nb-1",
            notebookTitle: "Research",
            id: "source-1",
            title: "Brief",
            snippet: "Evidence",
          },
        ],
      },
    ]);
    expect(parseHomeChatTurns("not json")).toEqual([]);
  });

  it("keeps the newest bounded window", () => {
    const turns = Array.from({ length: HOME_CHAT_MAX_TURNS + 3 }, (_, i) => turn(i));
    const kept = appendHomeChatTurn(turns.slice(0, -1), turns[turns.length - 1]);
    expect(kept).toHaveLength(HOME_CHAT_MAX_TURNS);
    expect(kept[0].id).toBe("turn-3");
    expect(kept[kept.length - 1]?.id).toBe(`turn-${HOME_CHAT_MAX_TURNS + 2}`);
  });

  it("builds follow-up context from completed answers only", () => {
    expect(homeChatHistory([turn(1), turn(2, "error"), { ...turn(3), answer: "" }])).toEqual([
      { role: "user", content: "Question 1" },
      { role: "assistant", content: "Answer 1" },
    ]);
  });
});
