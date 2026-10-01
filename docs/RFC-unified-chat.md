# RFC: Unified chat — one thread, every tool

## Problem

Home chat can do seven things. The MCP server can do sixty-seven.

`try_global_tool_route` (`commands.rs:9279`) is a keyword gate feeding one
LLM classification into a closed enum: settings, Night Shift, add URLs, add
text, save note, open notebook, rename/delete chat. It dispatches once and
returns. `grep -rn "tool_calls\|ToolCall" src-tauri/src` matches nothing:
there is no loop in the chat path, no tool result ever returns to the model,
nothing chains, nothing plans.

Meanwhile `src-tauri/src/mcp/` exposes 67 tools — `grow`, `commission_run`,
`second_look`, `schedule_report`, `corpus_timeline`, `ast_search`,
`suggest_cards`, the whole notebook and source CRUD — to any agent that
connects. And `AgentPane.tsx` hosts the user's own coding agent over ACP in
a second pane with a second transcript, kept in the Zustand store rather
than `meta_turns`.

So an agent that connects to Alchemy from outside has more hands than
Alchemy's own chat, and the one surface in the app that *does* have a tool
loop is the one that doesn't share the conversation.

This RFC makes Home chat the surface that can do everything the app can do,
in one durable thread, without adding a capability the app didn't already
have.

### Background

Guillermo Rauch's decomposition of agents into brain (model + harness),
hands (tools) and files (durable state) is a useful scoring rubric here:

- **Files** — done, and ahead. LanceDB plus OKF markdown folders plus sync
  is exactly the detachable agent storage other stacks are only now
  building. Night Shift is already the nightly consolidation job.
- **Hands** — strong, but pointed outward. 67 MCP tools, `cider` into Notes
  / Reminders / Calendar, fetch, git sources, the generation queue, the
  scheduler.
- **Brain** — missing. One-shot classifier. The real loop lives in another
  pane.

Everything below is the brain, plus the plumbing to let it reach the hands
we already built.

## 1. The loop

A new module, `src-tauri/src/agentloop.rs`, owns one thing: run a turn as
model → tool → result → model until the model answers or the round budget
is spent.

- **Round budget**, not a token budget: 8 rounds on a local model, 12 on a
  gateway. Shift's evals found round limits, not context, to be the binding
  constraint on hard turns, and a per-step budget beats a per-turn one as
  soon as steps exist.
- **A round-limited turn keeps its exchanges.** This is the one rule worth
  writing down before anything else: shift threw away the whole turn when
  it hit the ceiling, the next turn's model had no record of the reads it
  had done, and it invented an account of work it couldn't see. A turn that
  runs out of rounds settles as a turn — partial, labelled, with its tool
  rows intact.
- **Provider capability gate.** `ChatEngine` grows `supports_tools()`,
  and an engine that returns false falls back to today's classifier path
  unchanged. Nobody loses a working chat because they picked the wrong
  model. The four engines, as investigated:

  | engine | verdict |
  | --- | --- |
  | **Ollama** | Yes. `/api/chat` takes `tools` and returns `tool_calls`; `run_stream` already parses newline-delimited JSON objects, so the tool-call arm is a branch in the existing loop. |
  | **Gateway** | Yes. OpenAI-compatible `/chat/completions`; tool-call deltas arrive in the same SSE `data:` lines `chat_stream` already drains. |
  | **Foundation Models** | No, and not a near-term yes. Apple's framework does expose tools (`LanguageModelSession(tools:)`), but our sidecar is **one-shot, stateless, spawned per request** over NDJSON — `respond()` builds a session, streams one answer, exits. A tool loop needs either a session that survives across tool results or a reverse channel where Swift asks Rust to run a tool, because the tool bodies are in Rust. That is a sidecar protocol inversion for a ~3B model that will choose badly among 67 tools. FM stays the Small role it already is (`REQUEST_TIMEOUT` is 60s and the doc comment says "title-sized prompts"). Classifier path. |
  | **Agent CLIs** | They bypass our loop entirely — they *are* loops, and they already receive Alchemy's MCP server. Wrapping one in ours would nest two loops around the same 67 tools. This is phase 4's answer, and the code already carries the warning: `agent_cli.rs:531` records that copilot auto-loading Alchemy's own MCP server means "a copilot that calls back into the process waiting on it is a deadlock". Handing a turn to an agent must never block the loop that handed it over. |
- **Cancellation keeps its current contract.** The cancel scope is claimed
  before retrieval, as it is today; each round races the token, and
  dispatch stays deliberately outside the race — a mutation abandoned
  halfway is worse than one you wait out.
- **The classifier stays** as the zero-cost fast path. "switch chat to
  ollama" should not buy a tool loop. The gate already exists and already
  pays for itself.

## 2. One catalog

The `#[tool]` methods in `src-tauri/src/mcp/*.rs` become the single source
of truth for what Alchemy can do. A `ToolCatalog` is built from the same
definitions and handed to both the MCP server and the chat loop.

The precedent is already in the tree. `mcp::tool_catalog()`
(`mcp/mod.rs:388`) reads the Desktop Extension manifest's tool list off
`all_tools().list_all()` rather than keeping a copy, with the reason in its
doc comment: "a hand-kept copy would start lying one release after someone
added a tool." This RFC extends that function from `(name, description)` to
the full definition — input schema and a permission class — and adds a
dispatcher that calls the same `ToolRouter`.

The rule: **adding an MCP tool makes chat able to do it.** No second list,
no second dispatcher, no drift. A parity test asserts the catalog and the
MCP surface enumerate the same tools, in the shape of the existing
`plugins/claude-code` parity test.

One wrinkle, recorded as open question 1: 11 of the 67 tool bodies take an
rmcp `RequestContext` to derive an OKF by-line, and a chat turn has no MCP
peer. Phase 1 sidesteps it — read tools need no actor — and phase 2 pays
for it properly.

This is the convention the repo already states — new features should be
agent-reachable, not UI-only — pointed inward for the first time.

## 3. Tool search

67 schemas will not fit a local model's prompt, and local is the default.

A `tool_search(query)` tool returns at most 8 matching schemas and enables
them for the rest of the turn. A session with 67 tools costs the same
context as one with none until a tool is wanted. Shift arrived at the same
answer for the same reason (`docs/mcp-client-rfc.md` in that repo).

We have already paid this bill once, in the other direction.
`agent_cli.rs:528` records the measurement: copilot with its default MCP
config loaded **105 tools, ~42k tool-definition tokens, 43k prompt tokens
before the question** — and the fix was to refuse the servers one by one
down to 17 tools and 15k. That is the cost of putting a tool catalog in a
prompt, measured on this codebase. A local 8-27b model has far less room
than copilot did.

**The always-on six**, chosen so that the common Home turn never pays for
a search round:

| tool | why it earns a permanent seat |
| --- | --- |
| `ask_everything` | corpus-wide passages: Home's natural scope, and the thing most turns want first |
| `list_notebooks` | nearly every other tool takes a `notebook_id`; without this the model guesses ids |
| `add_source` | the most common Home command there is ("add this url") |
| `create_note` | the second most common ("save that") |
| `open_notebook` | navigation, which is local to Home and has no MCP equivalent |
| `tool_search` | the door to the other 62 |

Everything else — `search`, `grow`, `list_sources`, `second_look`,
`schedule_report`, the rest — is one `tool_search` round away. The six are
a starting point the evals should tune, not a fixed number.

## 4. Bounded, visible, reversible, stoppable

The product invariant is the design, not a review checklist. Each clause
gets a mechanism:

- **Bounded** — the round budget, and a permission class per tool. Every
  catalog entry is `read`, `write`, `destructive`, or `outside`:
  - `read` runs free.
  - `write` runs, is recorded, is undoable.
  - `destructive` (delete a notebook, retire a source) and `outside`
    (share, connectors, Mac writes via `cider`) become a **proposal in the
    thread** the user confirms. The shape already exists in
    `deletion_proposals` and in the registry's suggested cards.
- **Visible** — the existing step trail shows tool rows as they run, and
  every turn writes a `RunReceipt` (`models.rs:336`) with `trigger: "chat"`:
  status, one human line ("Read 12 sources — wrote 1 note"), provider,
  model, cost. Chat turns join Night Shift runs in the same receipts list
  rather than being a separate kind of history.
- **Reversible** — write tools record pre-images to an undo journal, and
  the turn's receipt carries one Undo. **Scope: notes and sources, and
  nothing else.** They are the two nouns the app is made of; a chat turn
  that changes one of them must be undoable. Settings changes and registry
  edits are deliberately out — they are small, already visible in their own
  surfaces, and each would drag its own journal format in for a case the
  user can fix by hand in one click.
- **Stoppable** — Esc already cancels; the loop makes it bite per round
  instead of only at the first token.

## 5. Results are surfaces

Tools that can do 67 things still produce text, and text will not collapse
the app into one pane.

A `MetaTurn` gains tool rows carrying a render payload, and the front end
renders the real component inline: a notebook card, a source list, the
gallery grid, a timeline, the diff of a note edit, a settings row. The
renderers exist; `docs/RFC-artifact-renderers.md` is the thread to pull.

This is the half that makes "one interaction pane" true. The loop makes
chat *able* to do everything; renderable results make it *worth looking
at*. Ship only the loop and you get a fast blind terminal.

## 6. One thread, two brains

`meta_turns.model` already captions every turn with who wrote it. That is
most of the work.

The ACP agent stops being a pane and becomes a brain the thread can hand a
turn to. When the local model spends its round budget without settling, or
the user asks for it, the thread offers to hand the turn to Claude Code (or
whichever agent is installed). The agent already receives Alchemy's MCP
endpoint in `session/new` — proven live in `docs/RFC-acp-agents.md` — so it
arrives with the same 67 hands, and its turns land in `meta_turns` with the
agent as the model.

This deletes the Chat/Agent toggle and the second transcript. One
conversation, captioned per turn, cheap by default and expensive when it
has to be.

## Self-improvement: field notes, and nothing else

Shift grew workflows: a named procedure with its own checks, run records,
reflection after a hard turn, distillation after a good run, and promotion
only by measured A/B against the baseline. It is good, and it is not coming
here.

**Alchemy already has the nouns a workflow unifies**, and a fourth would be
a synonym:

| shift | Alchemy, already shipped |
| --- | --- |
| workflow definition (prose steps, data file) | **template** — markdown in `~/Documents/Alchemy/templates`, user-owned, editable, portable (`templates.rs`) |
| scheduled run | **standing order** — `schedule_report`, `commission_run` |
| run record | **receipt** — `RunReceipt`, already has status, detail, cost, model |
| check | **second look** — `second_look_pass` |
| promotion gate | **proposal** — `deletion_proposals`, suggested registry cards |

What is worth taking from that RFC is the part that needs no noun at all,
costs no model call, and was measured to be the most valuable of the three:

**Field notes.** When a tool call is rejected — bad argument shape, an id
that doesn't exist, a notebook that isn't there — the loop appends one
deduplicated line recording the corrected shape. Newest 40 kept, each
tagged with the turn it came from, fed into every chat turn's system
prompt as a `<field-notes>` block.

- Deterministic. No model, no judgment, no scoring.
- Off with one setting.

**Where they live: a file, not the wiki.** The generated wiki
(`growth.rs:1134`, RFC-living-notebook pillar 3) is the right instinct —
in-app, readable, already a place where the app writes for itself — but it
is the wrong container for three reasons:

1. **A wiki page is a note, and notes get retrieved.** Field notes are
   operational memory about argument shapes. "create_note rejected the
   notebook title, pass the id" would start coming back as a citation when
   the user asks a research question. That is the failure the container is
   supposed to prevent.
2. **`upsert_wiki` is deterministic and regenerates.** The index and its
   entity pages are re-derived from sources on every sweep, write-skipping
   when unchanged. Field notes are append-only and stochastic; the next
   refresh would flatten them, and a user's hand-edits with them.
3. **The wiki is per-notebook.** Field notes are about the app's tools, not
   about any notebook's subject.

So: plain markdown at `<app-data>/field-notes.md` — portable, inspectable,
outside the corpus, never embedded. To answer the real want behind the wiki
suggestion (visible in the app, not buried in a file), it gets a surface:
Settings shows the lines beside `recent_errors`, which is where it belongs
anyway — a field note *is* a handled error the loop learned from. The user
edits or deletes lines there; nothing else writes to it.

Shift's finding: most wasted rounds were the same mistake made again in a
new session. That is the whole return, and it is available for a file
append.

**Explicitly deferred: reflection, distillation, and A/B promotion.** Not
because they're bad — because the gate that makes them honest is
measurement against a re-runnable task, and Alchemy doesn't have one. A
workflow pins a task so a candidate and a baseline can run it twice. A
Home chat turn is a one-off question over a corpus that changed since
yesterday; two runs would prove nothing and we would be shipping the
evaluation theater we don't want.

Where measurement *does* belong is where it already lives and already
works: `traces/retrieval.jsonl`, `judged_eval.rs`, and the
`ALCHEMY_EVALS=1` corpus evals watched in CI. Improvement to the loop is an
offline, measured change to the loop — not a thing the loop does to itself
at night.

If distillation ever earns its place, it arrives as an existing noun: a
turn that worked becomes a **proposed template**, disabled, sitting in the
templates folder for the user to accept. Separate RFC, after phase 5 has
data.

## Non-goals

- **Workflows.** Above.
- **A general computer-use agent.** Alchemy is files and research hands.
  Acting outside the app stays proposal-gated through connectors and
  `cider`.
- **Replacing notebook chat.** Notebook chat stays scoped to its sources —
  that's the grounded-answer surface and it works. Home is the surface that
  does everything.
- **New autonomy.** Night Shift gains no powers here. The loop runs when a
  person asks.
- **New capabilities at all in phase 1.** Every tool the loop can call is
  one the app already exposes to outside agents today.

## Phasing

1. **Catalog, loop, tool search.** Home only, read tools plus the writes
   Home already had. Round-limited turns keep their exchanges. Provider
   capability gate with the classifier as fallback. *Built — see "What
   phase 1 shipped" below.*
2. **Permission classes, proposals for destructive and outside tools, undo
   journal.** The full write surface opens here, not before.
3. **Renderable tool results** (`RFC-artifact-renderers`).
4. **One thread, two brains** — fold `AgentPane` into the thread, delete the
   toggle and the second transcript.
5. **Field notes.**

## What phase 1 shipped

`commands/chatloop.rs`, plus `chat_tools` on the Ollama and gateway engines
and `supports_tools` on `ChatEngine`. Two things differ from the draft above,
both discovered in the building:

**The catalog is not yet read off the MCP routers.** rmcp's `Peer::new` is
`pub(crate)`, so a `ToolRouter` cannot be called without a live MCP session:
`ToolCallContext` needs a `RequestContext`, which needs a `Peer`, which we
cannot construct. `list_all()` for *definitions* works fine — it is what
`tool_catalog()` already does — but dispatch does not. So phase 1 has two
lists after all: the advertised catalog and the `dispatch` match. A test
(`catalog_is_covered`) fails when they drift, which catches the problem at CI
instead of preventing it by construction. Phase 2's real work is extracting
each tool body into a plain function both callers share, which also settles
open question 1 — the extracted body takes an actor rather than a context.

**Global queries skip the loop.** RFC-infinite-context's gist route answers
"which notebooks mention X" from whole-corpus coverage; the loop would reach
for a top-12 chunk search and answer it worse. `is_global_query` keeps those
on the specialized path, and they pay no loop round.

**The cost to watch.** An ordinary Home question now pays one extra
non-streaming round before retrieval — the round where the model decides
*what* to search for, which is the point, but it is not free on a local 27b.
Every run writes `rounds_used`, `wall_ms`, `settled` and `empty` to
`traces/chatloop.jsonl`; decision 6 is waiting on those numbers from real
hardware. If the median is bad, the lever is a cost-control toggle in
Settings, default on.

## Decisions

1. **Which engines tool-call.** Ollama yes, gateways yes, Foundation Models
   no (classifier path; see §1 for why the sidecar shape, not Apple's API,
   is the blocker), agent CLIs bypass the loop entirely. A `supports_tools()`
   gate on `ChatEngine` and the classifier as the universal fallback.
2. **The always-on six**, with everything else behind `tool_search` (§3).
   Discernment is the whole design here; the measured 42k-token bill in
   `agent_cli.rs` is the argument.
3. **The palette stays one-shot.** ⌘K is a glance surface and its value is
   that it is fast. A palette ask that wants tools says so and walks over
   through the Open in Chat affordance that already exists. No loop, no
   extra round, no regression in the fastest path in the app.
4. **Undo covers notes and sources only** (§4). Not settings, not registry.
5. **Field notes are a file**, surfaced in Settings beside recent errors,
   not a wiki page (above).
6. **The round budget is an evals question, and ships instrumented to be
   one.** v1 takes the simple default — 8 rounds local, 12 gateway — and
   writes `rounds_used`, `wall_ms` and `settled` into the turn's trace, so
   `traces/` can answer whether the ceiling or the clock is the real
   constraint before we invent a time budget. Shift's evals found round
   limits binding, not token budgets; our models are slower than theirs, so
   the answer may differ and should be measured rather than guessed.

## Open questions

1. **Actor identity for mutating tools called by chat.** 11 of the 67 tools
   take an rmcp `RequestContext` to derive an OKF by-line (`client_actor`).
   A chat turn has no MCP peer. Phase 1 dodges this by exposing read tools
   plus the seven writes that already exist; phase 2 needs those bodies to
   take an actor from either source, and the by-line for a chat-driven write
   has to be decided — the user, or the model that wrote it?
2. **Does a tool round get its own foreground yield?** `crate::foreground`
   stands background work aside for one waiting person; an 8-round turn
   holds that for minutes. Probably fine, probably worth measuring with (6).
