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

**What field notes shipped.** `src-tauri/src/fieldnotes.rs` keeps
`<app-data>/field-notes.md`, one `- tool: error` line each (error collapsed
to one line, 200 characters), deduplicated by exact text with a repeat moving
to the newest slot, newest 40 kept, written by temp file and rename. Two
things are captured: a Home-loop tool reply that starts with `error:`, and an
MCP tool call that fails with INVALID_PARAMS (internal errors are bugs and go
to diagnostics). The block rides the Home loop's system prompt and both ACP
preambles. The file is re-read on every use, so a line the user deletes stays
deleted. Deferred: the Settings view beside `recent_errors`, the off switch,
and the per-line turn tag.

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
4. **One thread, two brains** — the user's agent answers in the Home
   thread. *First slice built — see "What phase 4 shipped" below.* The
   notebook Agent pane and its toggle remain for now.
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

**What the first live runs found.** Driven in the dev app on
`muse-glimmer:30b-mlx` (Ollama), one question whose answer is not in the
corpus — "What did I conclude about the Guerneville house purchase date?" —
run three times as each fix landed:

| run | what happened |
| --- | --- |
| 1 | Round 2 failed every time: the history echoed round 1's call with `arguments` as a JSON *string* (OpenAI's dialect), and Ollama answers that with `400 "Value looks like object, but can't find closing '}'"`. The single-round mock test could not see it. The loop fell back to synthesis, so an answer still arrived. |
| 2 | Dialect fixed; now it chained — and did not stop. 8 rounds, 20 tool calls, 15 searches, **6.2 minutes**, 71 excerpts handed to synthesis, which then timed out before its first token. No answer at all. It also reached for `open_notebook` mid-research on a question that never asked to go anywhere. |
| 3 | With the fixes below: 5 searches, stopped as `sufficient` at **94s**, 27 excerpts, answered in ~76s more. |

The research itself was good from run 2 on: the model found the property's
street address in one result and searched by it, and tried the house's
nickname. The single-shot path can do neither.

What changed in response:

- **Sufficiency, not the round budget, bounds a turn** (`EVIDENCE_CAP` = 24
  passages). A model hunting for something absent never runs out of reasons
  to look again; a ceiling on what it may carry back does. This also caps
  what synthesis sees, which was the actual failure in run 2.
- **Every write needs the user's words.** `add_source` keeps only URLs whose
  host the user typed — the rule the classifier route always had and the
  loop's first cut did not carry over. `save_note` needs an instruction to
  save; `open_notebook` needs a navigation verb. A refused call goes back to
  the model as a correctable error.
- **Two dialects, one conversion.** The loop speaks Ollama's (strict) form;
  `inference::to_openai_dialect` rewrites it for gateways.
- **A step at the start of every round**, so the first minute is not the
  front end's "Searching every notebook…" placeholder while nothing searches.
- **The trace records why gathering ended** (`stop`: settled, sufficient,
  budget, error, cancelled, skipped).

Also verified live: a command ("switch chat to ollama") short-circuits on the
classifier fast path with no loop round; an enumerative question skips the
loop and keeps the global path; Stop pressed mid-round-1 with the model call
in flight settles the turn in 0.1s.

**Decision 6, answered.** The round budget was never the constraint that
mattered; sufficiency is. The remaining cost is per-round model latency —
roughly 19s a round on a 30b MLX model — so a pointed question that settles
in two or three rounds spends 40–60s gathering before synthesis begins. That
is the number to watch, and the lever if it is too slow is the model, not
the loop.

**One finding outside this RFC:** "What are my notebooks about, broadly?"
answered as if the corpus were only about Alchemy, from 7 excerpts, across
22 notebooks that cover far more. That is the pre-existing global path,
which the loop deliberately leaves alone; it deserves its own look.

## What phase 4 shipped

**Decision (2026-10-01): the agent is Home's brain, not a hand-off target.**
When the chat provider is an agent with an ACP adapter (`claude-code`,
`codex`, `opencode` — the provider kind and the ACP agent id are the same
string), Home turns run in a thread-scoped ACP session instead of the tool
loop. The hand-off chip the draft described would have done nothing for a
user whose provider is an agent: the loop is skipped for agents
(`supports_tools()` is false), so nothing ever fails over to them.

- `home_brain` decides per ask, so a provider change takes effect on the
  next question. It falls back to the loop when the agent isn't installed
  or the MCP server isn't running — an agent with no way into the notebooks
  is worse than the excerpt answer it would replace.
- Sessions are keyed `home-<threadId>`; the session map, the working
  directory and the events already took any string. Only the preamble
  differs: Home's points at `ask_everything` and `list_notebooks`, and holds
  writes to the user's word, as the loop does.
- One live agent session per window; a thread left behind resumes from its
  stored session id. A **fresh** session in a thread that already has turns
  is handed the conversation so far, newest first within 6,000 characters,
  so a thread that started on the local model doesn't meet an agent with no
  idea what "it" refers to.
- The agent's narration ("I'll search your notebooks…") goes to the step
  trail; the saved answer is what it wrote after its last tool call.
- `add_meta_turn` takes an optional `model`: an agent turn is captioned with
  the agent that ran it, even if the provider changed mid-answer.
- A sign-in failure carries the command that fixes it.

**What the live run found.** Driven in the dev app with Codex, on the
Guerneville question the local loop had answered "not in the library":

| | |
| --- | --- |
| first try | `mcp__alchemy__startup (failed)`; Codex went probing ports itself and answered that it couldn't connect. |
| cause 1 — a regression since 0.52 | The ACP handoff (2026-08-20) passed the MCP URL without the bearer token the server has required since 2026-08-31 (shipped in 0.52). An agent **not** connected through Settings → Agents had only that handoff, and every call it made was refused; agents connected through Settings carry the key in their own entry and were unaffected. Fixed in its own commit. |
| cause 2 — dev only | All three agents carry their own `alchemy` MCP entry (written by Connect) pointing at 41414, the installed app; in dev it is closed and it shadows the session's. In production it is the same running app, so this doesn't reach users. Dev testing needs CLAUDE.md's documented edit (point the agent's entry at 41415, then restore). |
| with both handled | Codex called `ask_everything` and `search`, found the answer in the *River House* notebook, and noticed the date is recorded as an assumption — with a working `alchemy://note/…` link to the note. |
| Stop | settled 0.7s after the press mid-tool-call; partial kept as `stopped`; the session survives for a follow-up. |

Routing, both directions, was verified: Ollama → loop, Claude Code /
Codex → agent.

### Alchemy decides what may change a notebook

**Decision (2026-10-01), for consistency across providers:** the rule about
what an agent may change is the app's, not the agent's. Before this, each
agent applied its own: Claude Code inherits the user's `defaultMode` (here
`auto`, which approves tools itself — the first live Claude run never
prompted for anything), Codex starts in its own "Auto review", and opencode
follows its own config. The local loop already held writes to the user's
words (§4); agents now meet the same rule.

- **One classification.** `mcp/access.rs` labels each of the 67 tools read
  or write, both lists explicit, and a test fails the build when a served
  tool is in neither, in both, or no longer exists. At runtime anything not
  on the read list is a write, so an unknown name asks.
- **Asking mode for Claude Code.** It is put in `default` ("always ask")
  before the session reports ready — Home's and the notebook Agent pane's
  alike. A mode the adapter doesn't offer is logged, not forced. (Codex and
  opencode: see below.)
- **Reads are answered by Alchemy; writes go to the user.** A permission
  request for one of Alchemy's read tools gets `allow_once` without the
  user seeing it. The tool is identified from Claude's
  `_meta.claudeCode.toolName`, the request title, or — for Codex, whose MCP
  permission requests carry only a call id — the title its earlier
  `tool_call` update gave that id. Another server's `search` is not ours.
- **Only per-call answers.** Agents also offer "allow always" ("Yes, and
  don't ask again for Create Note commands"), which writes a rule into the
  agent's own settings — after which that agent stops asking Alchemy while
  every other brain still asks. The prompt shows Yes / No / Cancel. A
  remembered answer, if it's wanted, belongs in Alchemy.

**Verified live with Claude Code:** a read (`list_notebooks`,
`ask_everything`) ran with no prompt; "create a note in River House" stopped
at an inline prompt naming `create_note`; No was respected — the agent said
the note wasn't created, and none was.

**Codex and opencode run on their own rules (decided 2026-10-03).** The
release sweep tried Codex in its nearest asking mode, `workspace-write`: it
asked about every shell command ("List files" nine times for two requests),
yet ran some of Alchemy's writes — `add_source`, and `update_source`, a
destructive one — without asking, so no undo snapshot was taken. It neither
left the decision to Alchemy nor stayed out of the way. opencode has no
asking mode at all (a 2026-10-02 decision kept it out of Home; reversed here).

So the rule is: **an agent that hands its tool calls to Alchemy gets Alchemy's
prompt and undo** — Claude Code, verified; **Codex and opencode keep the
judgment the user configured them with**, as in a terminal, and are offered
as Home's brain like any agent. When they do ask, Alchemy's prompt and undo
still apply; what they approve themselves can't be undone from Home. Revisit
if either gains a mode that hands every tool call to the client.

**Dev-only note — don't move the MCP port to test.** The agents' own `alchemy`
MCP entries (from Connect) point at 41414, the installed app, and shadow the
session's server in dev. An earlier version of this note suggested setting
Alchemy's MCP port to 41413 so the dev +1 offset lands on 41414. **Don't:**
every app start runs `connectors::refresh_installed_connectors` with the
configured port and rewrites every connected agent's entry to it. On
2026-10-03 that left Codex refused and Cursor stuck at 41413 (an install-link
connection the refresh doesn't rewrite), which read as a Codex bug until the
config was checked. To test one agent against a dev build, point only that
agent's entry at 41415 and restore it afterwards, as CLAUDE.md says.

## What phase 2 shipped: undo for what an agent changed

Built 2026-10-02 as the release-critical slice of phase 2.

**Capture where the change is known exactly: the permission prompt.**
Nothing kept old content before this — `update_note` replaces a note's
whole title and body and drops the old row; deletion receipts hold ids
only. Since every agent write now stops at Alchemy's prompt, the moment the
user says Yes is when Alchemy knows which session asked, which tool, and
with which arguments (Claude's are on the request; Codex's on the earlier
`tool_call`, cached by id). `acp_permission` snapshots the note or source
then, before answering — the agent is still blocked on that answer, so the
snapshot is never late. No time-window guessing, no MCP or database hooks.

- **Scope:** `update_note`, `delete_note`, `update_source`, `delete_source`
  — where data can be lost. What an agent creates the user can remove by
  hand, and the tool loop only creates, so neither is journaled yet.
- **Journal:** `<app data>/agent-undo/<session key>.json`, newest 200 per
  session. For an update it also keeps a hash of exactly what the agent will
  write (the arguments, normalized the way ingest stores them).
- **Undo never overwrites the user.** An update is restored only while the
  note or source still matches that hash; anything edited since is skipped
  and named in the result. A deleted note or source returns as a new item
  with the old content (a URL source re-added as its URL) — the old id is
  held by the deletion receipts that stop sync from resurrecting it.
- **Where:** Home answers whose turn made such changes show **Undo changes**.
  A thread's changes are read once and split into turns by time, since each
  was allowed between its question and its answer. A confirm dialog lists
  each item; a toast reports what was put back and what was left, and why.

Captured for the notebook Agent pane too (same permission path); its Undo
button waits for after release.

## Initial release scope (2026-10-02)

Shipping the feature means shipping what keeps the user safe and the answers
honest; everything else refines after. In:

- **Phases 1 and 4** (merged): the tool loop for local models, the agent as
  Home's brain for Claude Code and Codex, Alchemy as the permission authority,
  tool names in the user's words.
- **Undo for notes and sources** (phase 2, critical slice). The permission
  prompt asks before a write; it can't take one back. `update_note` replaces
  a note's whole content, so a "Yes" to a bad rewrite is otherwise data the
  user has lost. Undo restores what any brain changed — loop or agent.
- **Field notes** (phase 5): deterministic capture of rejected tool calls,
  fed into every brain's prompt. No Settings view yet; the file is plain
  markdown in the app data folder.

Out, until after release: extracting every tool body into shared functions
(the loop keeps its twelve tools), the loop's full write surface, proposals
for destructive tools (the prompt already gates them), renderable results
(phase 3), citations for agent answers, a remembered "allow always" in
Alchemy, opencode as Home's brain, and the Settings view for field notes.

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
