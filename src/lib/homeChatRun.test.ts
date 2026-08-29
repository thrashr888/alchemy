import { describe, expect, it } from "vitest";
import {
  citedNotebooks,
  historyOf,
  homeDraftKey,
  mergeLoadedTurns,
  runForThread,
} from "./homeChatRun";
import type { HomeRun } from "./storeTypes";
import type { MetaCitation, MetaTurn } from "./types";

let seq = 0;
function turn(
  role: MetaTurn["role"],
  content: string,
  kind: MetaTurn["kind"] = "chat",
  id = `turn-${++seq}`,
): MetaTurn {
  return {
    id,
    threadId: "t1",
    role,
    content,
    citations: [],
    kind,
    createdAt: seq,
  };
}

function run(threadId: string): HomeRun {
  return {
    threadId,
    question: "who mentioned SNDK?",
    streaming: "They ",
    steps: ["Searching every notebook"],
    waiting: "",
    stopped: false,
    queued: false,
  };
}

describe("prior context", () => {
  it("pairs completed exchanges", () => {
    expect(
      historyOf([
        turn("user", "one"),
        turn("assistant", "first"),
        turn("user", "two"),
        turn("assistant", "second"),
      ]).map((m) => m.content),
    ).toEqual(["one", "first", "two", "second"]);
  });

  it("drops an exchange that ended in an error", () => {
    expect(
      historyOf([
        turn("user", "one"),
        turn("assistant", "Ollama is unreachable", "error"),
      ]),
    ).toEqual([]);
  });

  it("drops a question with no answer under it", () => {
    expect(historyOf([turn("user", "still running")])).toEqual([]);
  });

  it("drops a command and its tool confirmation", () => {
    expect(
      historyOf([
        turn("user", "switch chat to ollama"),
        turn("assistant", "Chat provider is now Ollama.", "tool"),
      ]),
    ).toEqual([]);
  });

  it("keeps the real exchanges around a tool row", () => {
    expect(
      historyOf([
        turn("user", "one"),
        turn("assistant", "first"),
        turn("user", "add https://example.com"),
        turn("assistant", "Added 1 source to **Japan**.", "tool"),
        turn("user", "two"),
        turn("assistant", "second"),
      ]).map((m) => m.content),
    ).toEqual(["one", "first", "two", "second"]);
  });
});

describe("which run a conversation sees", () => {
  it("shows the run asked into this thread", () => {
    expect(runForThread(run("t1"), "t1")?.threadId).toBe("t1");
  });

  it("hides a run belonging to another thread", () => {
    // The point of the whole model: leaving a conversation leaves its answer
    // running, but the answer never appears under someone else's question.
    expect(runForThread(run("t1"), "t2")).toBeNull();
  });

  it("shows nothing when nothing is running", () => {
    expect(runForThread(null, "t1")).toBeNull();
  });
});

describe("composer draft slots", () => {
  it("keeps a slot per conversation", () => {
    expect(homeDraftKey(true, "t1")).not.toBe(homeDraftKey(true, "t2"));
  });

  it("gives an unasked new chat its own slot", () => {
    expect(homeDraftKey(true, null)).toBe("t:new");
  });

  it("keeps the shelf apart from every thread", () => {
    expect(homeDraftKey(false, "t1")).toBe("shelf");
  });
});

describe("the notebooks behind an answer", () => {
  function cite(
    notebookId: string,
    notebookTitle: string,
    kind: MetaCitation["kind"] = "source",
  ): MetaCitation {
    return {
      kind,
      notebookId,
      notebookTitle,
      id: `c-${++seq}`,
      title: "A source",
      snippet: "…",
    };
  }

  it("names each notebook once, in citation order", () => {
    expect(
      citedNotebooks([
        cite("n2", "Stocks"),
        cite("n1", "Alchemy"),
        cite("n2", "Stocks"),
      ]),
    ).toEqual([
      ["n2", "Stocks"],
      ["n1", "Alchemy"],
    ]);
  });

  it("leaves the registry out — a card lives in no notebook", () => {
    expect(citedNotebooks([cite("", "", "card")])).toEqual([]);
  });
});

describe("merging a thread's turns back in", () => {
  it("keeps an answer that settled while the fetch was in flight", () => {
    const fetched = [turn("user", "one", "chat", "a")];
    const onScreen = [
      turn("user", "one", "chat", "a"),
      turn("assistant", "arrived late", "chat", "b"),
    ];
    expect(mergeLoadedTurns(fetched, onScreen).map((t) => t.id)).toEqual([
      "a",
      "b",
    ]);
  });

  it("does not duplicate what the fetch already knows", () => {
    const both = [turn("user", "one", "chat", "a")];
    expect(mergeLoadedTurns(both, both)).toHaveLength(1);
  });
});
