Chat is a conversation; the Ledger is a record. It is the notebook's typed memory — the third mode of the center column, next to Chat and Reader — and it holds the things you have actually concluded, decided, or still need to answer, as structured rows rather than prose scrolled away in a transcript.

## What a ledger entry is

Every entry has a **kind**, a date, a status, and optionally a **why** and an anchor to source text. The five kinds:

- **Assertion** — a claim you're carrying forward. Statuses: asserted, corroborated, contradicted, stale.
- **Fact** — something established. Statuses: current, superseded.
- **Decision** — a call you made. Statuses: decided, superseded.
- **Question** — something open. Statuses: open, answered.
- **Log** — a dated note for the record.

The statuses are a vocabulary, not a state machine: you (or an agent working for you) are the authority, and moving an entry between statuses is always allowed within its kind. Anchors pin an entry to verbatim source text, so a fact can point at the exact sentence it came from — the same citation contract chat answers follow.

## Why bother

A research notebook accumulates two different kinds of knowledge. The sources hold what *the documents* say. The Ledger holds what *you* say about them: "we decided to go with the smaller vendor", "the Q3 number in the deck contradicts the filing", "still waiting on the licensing answer". Six weeks later, those rows are the difference between re-reading everything and picking up where you left off.

Some patterns that work well:

- **Decisions on file.** Every time a discussion in chat ends with a call, add a decision entry with a one-line why. When circumstances change, mark it superseded rather than deleting it — the history is the value.
- **Open questions as a queue.** Add questions as they occur to you; resolve them to *answered* as sources arrive that settle them.
- **Assertions under test.** When a source makes a claim you are not sure about, record it as an assertion. As other sources weigh in, promote it to corroborated or flag it contradicted.

## Agents can keep it too

The Ledger is fully agent-reachable through Alchemy's MCP server: an agent can list entries, add them, and update statuses. That means an agent doing research for you can leave behind structured findings — dated, typed, and anchored — instead of a wall of text, and you can audit exactly what it concluded. See the "Use Cases & Power Tips" source for how to connect agents.

## Where it shows up

Ledger entries are visible in the Ledger pane, and the daily Brief can call out when new activity touches a decision you have on file. Entries are deliberately kept out of retrieval until they are woven in carefully — a wrong merge would poison the notebook's memory worse than no memory at all.

Start small: one decision, one open question. The habit compounds.
