# RFC: Foundation learnings — sessions, links, multi-hop, memory evals

Status: proposed. Fold-in RFC: each pillar lands inside an existing track
(import pipeline, registry/ledger, retrieval maturity, infinite context)
rather than as a new subsystem.

## Summary

Chroma's Foundation (trychroma.com/foundation) is a research preview of
"self-improving memory": watch agent sessions, extract knowledge into a
hyperlinked / tagged / versioned wiki with lineage and citations, serve it
back to any agent over MCP. It is cloud- and team-shaped; Alchemy is
local-first and personal — that's our differentiation, not a gap. But four
of its ideas map cleanly onto infrastructure we already have, and one open
question (cortex as per-notebook chat memory) turns out to be the same
project. This RFC folds all five in.

Guardrails:

- No new storage engine, no new daemon. Everything lands in LanceDB tables
  and existing pipelines (gists, registry, ledger, router).
- Intelligent behavior default-ON per house rule; toggles are cost control.
- Each pillar independently shippable; nothing below depends on another
  pillar landing first.

## Pillar 1: agent sessions as a source type

Foundation's core insight: agent transcripts are the richest untapped
knowledge source. Alchemy already distills sources into gists and
auto-extracts registry facts — the missing piece is just ingestion.

- Add **"Agent session"** to the add-source modal: browse/pick sessions from
  known transcript locations (`~/.claude/projects/*/…jsonl` for Claude Code;
  Codex/opencode equivalents as they're verified). Show project + date +
  first-prompt preview in the picker.
- Extraction (`ingest.rs`, new filetype): parse the JSONL turn stream into a
  readable transcript — prompts, assistant text, tool calls collapsed to
  one-line summaries, results elided past a cap. Skip binary/image payloads.
  Chunking is structure-aware on turn boundaries.
- The existing pipeline does the rest: gist distillation gives the "what did
  we learn" summary, registry auto-facts pick up identifiers, ledger entries
  can anchor to transcript quotes.
- Later (not v1): a "watch this project" mode that re-imports new sessions —
  needs the async-import + refresh plumbing from RFC-import-pipeline, and
  cost thinking; one-shot import first.

## Pillar 2: bidirectional links as metadata

Foundation's data model is "a wiki — hyperlinked, tagged, versioned."
Alchemy has tags and versioned notes; it lacks links. Constraint that
settles the design: sources are sometimes raw files referenced in place
(folder imports, placeholders) rather than copied content we own — so links
**cannot live in content**. They are metadata about pairs of entities.

- New `links` LanceDB table: `{ id, notebook_id, from_id, from_kind, to_id,
  to_kind, link_kind, created_by, created_at }` where kind ∈ {source, note,
  ledger_entry}. `link_kind` starts with "references" (explicit) and
  "mentions" (derived); one row serves both directions — "bidirectional" is
  a query property (`from_id = X OR to_id = X`), not two rows.
- Derived links come from machinery that already exists: registry
  identifiers appearing in two sources is a "mentions" edge; a ledger
  anchor's quote pins entry→source; note citations pin note→source. Backfill
  derives these from current data — day one the graph is non-empty.
- Note content MAY carry `[[title]]` syntax (notes are ours, unlike raw
  files); the editor resolves it to a link row + renders a chip. Raw sources
  never get rewritten.
- UI: a backlinks section on the source viewer and note editor — "linked
  from" list, hairline rows, no graph visualization (that's decoration until
  proven otherwise).
- MCP parity: `link`/`unlink`/`list_links` tools beside the existing note
  and ledger tools.

## Pillar 3: bounded multi-hop retrieval

Foundation's Context-1 is a self-editing search agent: retrieve, assess,
reformulate, hop. We don't need their 20B model — we need a bounded loop
over the retrieval stack we already measure (RFC-retrieval-maturity), and we
have two hop-sources Foundation doesn't: the **ledger** (typed claims with
anchors) and the **registry** (identifiers with attached facts).

- Loop, cap ≤ 3 hops, on the chat path only when hop 1 is weak (reuse the
  router's confidence signals; strong first-pass answers pay zero extra):
  1. hybrid search as today;
  2. small-model assess: "answerable from these excerpts? if not, what's
     missing?" — emits a reformulated query and/or a registry identifier or
     ledger entry to pivot through;
  3. pivot hops query by identifier/anchor (cheap, exact) before falling
     back to another embedding search.
- Every hop appends to the retrieval trace (`traces/retrieval.jsonl`) so the
  eval harness can score multi-hop against single-shot on the existing
  `multihop`-kind dataset queries — that dataset kind exists precisely
  because single-shot loses there.
- Guardrail: never mandatory on the normal path (same rule as reranking);
  wall-clock budget, and coalesced scans per the Lance scan-storm lesson.

## Pillar 4: memory evals (BEAM) — and cortex

BEAM is the long-horizon agent-memory benchmark Foundation claims SOTA on
(10M-token variant). It tests exactly the capability the last two open
threads circle around:

- The infinite-context branch's open follow-up is a **live-model eval
  harness** — BEAM's shape (long multi-session histories, questions whose
  answers were established far in the past) is the right dataset shape for
  it. Rather than inventing a bespoke memory eval, adapt BEAM-style tasks
  into `src-tauri/evals/datasets/` alongside the retrieval datasets, scored
  through the same runner conventions (sampled short runs first, per the
  targeted-eval-runs practice; results to ~/alchemy-benchmarks.csv).
- **Cortex (the P6 question) folds in here — same project, one decision.**
  cortex (thrashr888/cortex) is a two-store memory: episodic raw.db →
  consolidated long-term via sleep/dream, plus skills export. That *is*
  Foundation's session→wiki loop, repo-local. As per-notebook chat memory it
  would consolidate chat turns into durable notebook knowledge — which in
  Alchemy terms should write **ledger entries and notes**, not a parallel
  SQLite store, or it violates the one-store guardrail and agent parity.
  Two viable shapes:
  1. cortex as a library: needs an upstream lib target (it's binary-only
     today — same fix-upstream pattern as cider), then its consolidation
     passes run in-process writing to LanceDB;
  2. cortex as inspiration: implement wake/sleep-style consolidation
     natively on the ledger (a Night-Shift-adjacent pass distilling recent
     chat turns into facts/decisions/questions).
  Recommendation: (2), with (1) revisited if the consolidation logic proves
  hairy — the value is the loop, not the binary. Either way BEAM-style evals
  are the acceptance metric, which is why cortex belongs in this RFC and not
  its own: build the eval first, then the memory pass has a scoreboard.

## Sequencing

1. Pillar 1 (sessions in add-source) — smallest, pure ingest, immediately
   useful for dogfooding: Alchemy sessions researching Alchemy.
2. Pillar 4a (BEAM-style eval harness) — establishes the scoreboard.
3. Pillar 2 (links table + backfill + backlinks UI).
4. Pillar 3 (bounded multi-hop) — measured against the Pillar-4 harness.
5. Pillar 4b (consolidation pass / cortex decision) — last, scored.
