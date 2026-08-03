# RFC: Deep Research — frontier CLI agents researching your own corpus

> Status: proposed (2026-08-02). Backlog P1/7.
> Depends on: the registry/MCP push (`docs/RFC-mcp-server.md`) — the
> researching agent needs a `generate` tool to close its loop.

## The sentence no competitor can say

Alchemy already runs headless agent CLIs as an inference provider
(`inference/agent_cli.rs`: claude, codex, gemini, cursor, opencode, copilot,
hermes, bob) and already exposes the corpus to agents over an embedded MCP
server (`mcp/`, streamable HTTP on 127.0.0.1:41414). Those two facts have not
yet been pointed at each other.

Put together they give Alchemy something hosted research tools structurally
cannot offer:

> **Your Claude Code / Codex / Gemini subscription drives multi-step research
> over your private sources, on your machine, at zero marginal cost.**

NotebookLM cannot say it — the corpus would have to leave the device.
Perplexity and the deep-research products cannot say it — they meter tokens,
and they research the public web, not your files. A local RAG tool cannot say
it either, because a 7B local model is not a research agent. The combination
is only available to a product that is (a) local-first, (b) already speaks
MCP, and (c) already shells out to a subscription CLI. Alchemy is all three
already; this RFC is the wiring.

## What a run looks like

The user asks a question too big for one retrieval pass — *"How has my
thinking about retrieval evaluation changed across these 40 sources, and
where do I contradict myself?"* Chat's single hybrid-search-then-answer
(`rag.rs`) cannot do this. It retrieves once, against one phrasing, and
answers from what came back.

Deep research instead hands the question to a frontier agent with tools:

1. **Plan.** The agent decomposes the question into sub-questions.
2. **Search.** For each, it calls Alchemy's existing MCP `search` tool —
   hybrid vector + BM25 over the real corpus, the same retrieval chat uses.
3. **Read.** It pulls whole sources with `get_source` when an excerpt is not
   enough. This is the step chat structurally cannot take.
4. **Iterate.** It notices gaps, contradictions, and unfamiliar names, and
   searches again. Multi-step is the entire point.
5. **Write.** It calls `generate` (the MCP push's contribution) to land a
   report as a real note in the notebook, with citations.

The output is a durable artifact in the user's own notebook, not a chat
message that scrolls away.

## Why the agent CLI, not the chat provider

A deep research run is 20-60 tool calls and a lot of reasoning. Through a
metered gateway that is dollars per question, which makes the feature
something users ration. Through `claude -p` on a Max subscription it is
free at the margin, which makes it something they use daily. The economics
change the product, not just the bill.

It also means the reasoning quality is frontier-grade while the corpus never
leaves the machine — the agent runs locally as a subprocess and reaches
Alchemy over localhost MCP. Nothing is uploaded that the user did not
already send to their own subscription.

## Scope

**Ship Claude-Code-only first**, gated behind `agent_status` detection
(already implemented). The other CLIs differ in their tool-use plumbing and
JSON shapes; one working path beats seven flaky ones, and the surface is
identical for the rest later. `claude -p --output-format stream-json` already
streams structured events, which is what a progress UI needs.

**In scope**
- A `deep_research` command taking a question + notebook scope.
- A run loop that spawns the agent CLI with the Alchemy MCP server
  pre-registered (`connectors.rs` already knows how to register it).
- Streamed progress: which sub-question, which tool, how many sources read.
  A multi-minute run with no feedback reads as a hang.
- Cancellation, on the existing `cancel_generation` scope pattern.
- The report lands as a note, with citations resolving to real chunks.
- A trace line per run in `traces/`, like every other retrieval path.

**Out of scope for v1**
- The other seven agent CLIs.
- Web search mixed with corpus search. Tempting, and wrong first: the moat is
  the private corpus, and blending public results makes provenance muddy.
- Scheduled/background research runs (Night Shift's territory — natural v2).

## The opening commit: collapse the AiConfig flat fields

`AiConfig` currently carries both a `providers: Vec<ProviderEntry>` list and
the legacy flat fields it was grown from — `provider`, `embedder`,
`base_url`, `chat_model`, `openai_base_url`, `openai_api_key`,
`openai_chat_model`, `vision_model`, `openai_vision_model` — with
`normalize` synthesizing entries from the flat set for legacy configs.

Deep research adds a *third* role (Research) that must name a provider, and
adding it to the flat set would mean two more fields and another normalize
branch. This is the moment to finish the migration instead: providers become
the only representation, roles reference provider ids, and the flat fields
are read once at load for migration and never written again.

This is the one genuinely risky config migration in the backlog, which is
exactly why it should ride in with a feature that pays for it rather than
happen as a standalone refactor nobody can justify testing.

**Migration safety:** the flat fields keep parsing (serde `default`), a
migration runs once and rewrites config.json in provider form, and the old
binary's reader still understands the result — the shared dev/prod store
rule applies here too (see the shared-store note in the release process).
Ship a release promptly after it lands.

## Open questions

- **Where does a run live in the UI?** Studio (it produces an artifact) or a
  distinct surface? Leaning Studio: the output is a report, and Studio is
  already where reports are made.
- **How much does the agent see?** Registering the whole MCP surface gives it
  notebook/source/note CRUD, including writes. A research run should almost
  certainly get a read-only subset plus `generate`.
- **What stops a runaway?** A wall-clock cap and a tool-call cap, surfaced as
  "stopped after N steps" rather than silently truncated.
- **Does the gist/router layer help the agent?** Per-source gists
  (`gist.rs`) and the semantic router (`router.rs`) are a table of contents
  for the corpus. Handing the agent that map up front may cut its search
  count substantially. Worth measuring, not assuming.
