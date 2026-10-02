import { describe, expect, it } from "vitest";
import {
  EMPTY_AGENT_FOLD,
  citedNotebooks,
  foldAgentUpdate,
  historyOf,
  homeDraftKey,
  mergeLoadedTurns,
  homeAgentKey,
  runForThread,
  toolsRunning,
  type AgentFold,
  agentFailureMessage,
  isAgentAuthFailure,
  agentAnswer,
  seedAgentPrompt,
} from "./homeChatRun";
import type { HomeRun } from "./storeTypes";
import type { AcpUpdateEvent, MetaCitation, MetaTurn } from "./types";

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

describe("foldAgentUpdate", () => {
  const fold = (updates: AcpUpdateEvent["update"][]): AgentFold =>
    updates.reduce(foldAgentUpdate, EMPTY_AGENT_FOLD);
  const say = (text: string) => ({
    sessionUpdate: "agent_message_chunk",
    content: { type: "text", text },
  });

  it("streams message chunks into one answer", () => {
    expect(fold([say("The SNDK "), say("data is in Finance.")]).streaming).toBe(
      "The SNDK data is in Finance.",
    );
  });

  it("moves narration into the trail and keeps only the answer", () => {
    // The shape of the first live Codex run: talk, tool, talk, tool, answer.
    const f = fold([
      say("I'll search your notebooks."),
      { sessionUpdate: "tool_call", toolCallId: "a", title: "ask_everything" },
      say("I found a River House summary.\nChecking the rest."),
      { sessionUpdate: "tool_call", toolCallId: "b", title: "search" },
      say("It closed in October 2025."),
    ]);
    expect(f.steps).toEqual([
      "I'll search your notebooks.",
      "ask_everything",
      "I found a River House summary. Checking the rest.",
      "search",
    ]);
    expect(agentAnswer(f)).toBe("It closed in October 2025.");
  });

  it("keeps each tool on its own trail line after narration shifts it", () => {
    let f = fold([
      say("Looking."),
      { sessionUpdate: "tool_call", toolCallId: "a", title: "search" },
    ]);
    f = foldAgentUpdate(f, {
      sessionUpdate: "tool_call_update",
      toolCallId: "a",
      status: "failed",
    });
    expect(f.steps).toEqual(["Looking.", "search (failed)"]);
  });

  it("falls back to the last words when a tool call came last", () => {
    const f = fold([
      say("It closed in October 2025. Saving that for you."),
      { sessionUpdate: "tool_call", toolCallId: "a", title: "create_note" },
    ]);
    expect(f.streaming).toBe("");
    expect(agentAnswer(f)).toBe("It closed in October 2025. Saving that for you.");
  });

  it("cuts a long aside to one trail line", () => {
    const f = fold([
      say("x".repeat(400)),
      { sessionUpdate: "tool_call", toolCallId: "a", title: "search" },
    ]);
    expect(f.steps[0].length).toBeLessThanOrEqual(160);
    expect(f.steps[0].endsWith("…")).toBe(true);
  });

  it("turns tool calls into trail lines that finish by id", () => {
    let f = fold([
      { sessionUpdate: "tool_call", toolCallId: "a", title: "search" },
      { sessionUpdate: "tool_call", toolCallId: "b", title: "read_source" },
    ]);
    expect(f.steps).toEqual(["search", "read_source"]);
    expect(toolsRunning(f)).toBe(2);
    f = foldAgentUpdate(f, {
      sessionUpdate: "tool_call_update",
      toolCallId: "a",
      status: "completed",
    });
    expect(toolsRunning(f)).toBe(1);
    // A status-only update without an id means the newest call.
    f = foldAgentUpdate(f, {
      sessionUpdate: "tool_call_update",
      status: "failed",
    });
    expect(f.steps).toEqual(["search", "read_source (failed)"]);
    expect(toolsRunning(f)).toBe(0);
  });

  it("leaves thoughts and replayed user messages out", () => {
    const thought = {
      sessionUpdate: "agent_thought_chunk",
      content: { type: "text", text: "hmm" },
    };
    const replay = {
      sessionUpdate: "user_message_chunk",
      content: { type: "text", text: "old question" },
    };
    // Unchanged by identity, so the store can skip a commit for them.
    expect(foldAgentUpdate(EMPTY_AGENT_FOLD, thought)).toBe(EMPTY_AGENT_FOLD);
    expect(foldAgentUpdate(EMPTY_AGENT_FOLD, replay)).toBe(EMPTY_AGENT_FOLD);
  });

  it("ignores an update for a tool it never saw start", () => {
    const f = foldAgentUpdate(EMPTY_AGENT_FOLD, {
      sessionUpdate: "tool_call_update",
      toolCallId: "ghost",
      status: "completed",
    });
    expect(f).toBe(EMPTY_AGENT_FOLD);
  });
});

describe("homeAgentKey", () => {
  it("prefixes the thread id so it can't collide with a notebook's", () => {
    expect(homeAgentKey("abc")).toBe("home-abc");
  });
});

describe("seedAgentPrompt", () => {
  const q = "and what did I decide?";

  it("sends the question alone when there is nothing to carry over", () => {
    expect(seedAgentPrompt([], q)).toBe(q);
  });

  it("hands a fresh agent the conversation another brain answered", () => {
    const out = seedAgentPrompt(
      [
        { role: "user", content: "where is the SNDK data?" },
        { role: "assistant", content: "In Stocks: Indexes." },
      ],
      q,
    );
    expect(out).toContain("User: where is the SNDK data?");
    expect(out).toContain("Assistant: In Stocks: Indexes.");
    expect(out.endsWith(q)).toBe(true);
  });

  it("keeps the newest exchanges when it has to cut", () => {
    const long = "x".repeat(4_000);
    const out = seedAgentPrompt(
      [
        { role: "user", content: `oldest ${long}` },
        { role: "assistant", content: "old answer" },
        { role: "user", content: `newest ${long}` },
        { role: "assistant", content: "new answer" },
      ],
      q,
    );
    expect(out).toContain("newest");
    expect(out).toContain("new answer");
    expect(out).not.toContain("oldest");
  });
});

describe("agentFailureMessage", () => {
  it("recognizes the sign-in failures agents actually report", () => {
    // The two from the live run that found this, plus the HTTP form.
    expect(isAgentAuthFailure("Authentication required")).toBe(true);
    expect(
      isAgentAuthFailure(
        "Failed to authenticate: OAuth session expired and could not be refreshed",
      ),
    ).toBe(true);
    expect(isAgentAuthFailure("HTTP 401 Unauthorized")).toBe(true);
    expect(isAgentAuthFailure("The agent hit an error.")).toBe(false);
    expect(isAgentAuthFailure("Rate limited, try again")).toBe(false);
  });

  it("adds the fix to a sign-in failure, and only to one", () => {
    const out = agentFailureMessage("Authentication required", "Claude Code", "claude");
    expect(out).toContain("run `claude` in Terminal");
    expect(out.startsWith("Authentication required")).toBe(true);
    expect(agentFailureMessage("Timed out", "Claude Code", "claude")).toBe("Timed out");
    expect(agentFailureMessage("Authentication required", "Codex", "")).toBe(
      "Authentication required",
    );
  });
});
