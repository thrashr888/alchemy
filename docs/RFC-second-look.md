# RFC: The Second Look

**Status:** implementing (v1)
**Pillar:** RFC-v12-steward §8 — the review culture of a good team, on call for one person.

## Problem

A solo person has no reviewer. Drafts — RFCs, appeal letters, research
notes — ship with claims nobody re-checked. The citations in a draft
are the *author's* citations: retrieval that already agreed with the
sentence it decorates. What's missing is an adversarial pass: does the
corpus, searched fresh, actually support each claim?

## Shape

One verb: **Second Look** takes a note and returns a verdict report.

1. **Split** the note into checkable claims (Small role, strict
   numbered format, parse-or-skip, ≤ 20 claims).
2. **Re-retrieve** per claim: fresh hybrid search (vector + BM25,
   `db.search_chunks`) over the notebook — *excluding the note's own
   chunks*, so a draft can never support itself. Each search appends
   its `SearchTrace` line to `traces/retrieval.jsonl` like every other
   retrieval in the app.
3. **Judge** each claim against its fresh excerpts with a different
   engine than the one that writes prose in this app: the **Small
   role** — different from Chat/Generate by construction in every
   default config. Verdicts: `supported | weak | unsupported |
   contradicted`, strict two-line format, parse-or-skip. A claim whose
   verdict fails to parse is reported as **unjudged** — listed, never
   silently dropped.
4. **Report**: a new note, `Second Look: {title}`, leading with the
   count line (`5 supported · 1 weak · 1 unsupported · 1 contradicted`)
   and then one section per claim: verdict, the claim, the judge's
   reason, and the strongest fresh excerpt with its source title.

## Surfaces

- `run_second_look(notebook_id, note_id)` Tauri command — fire and
  forget; emits the note, `mcp://changed` (scope `notes`), and a
  notification when done.
- `second_look` MCP tool (note id or raw text) returning the
  structured verdicts, so agents can check their own drafts before
  filing them.
- Notes list row menu: **Second Look** action; the report note lands
  beside the draft.

## Discipline (gist rules apply)

- Caps: ≤ 20 claims, k = 6 excerpts per claim, excerpts ≤ 700 chars
  each into the judge.
- Parse-or-skip at both model steps; the report says what was skipped.
- Claims under 40 chars or without checkable content are dropped at
  the split step (the splitter is told to skip pleasantries and
  formatting).
- No config, no toggle: it runs when asked, costs only when asked
  (smart-defaults rule — invocation is the opt-in).

## Non-goals (v1)

- No screenshot/design-crit mode (vision leg comes later).
- No explicit judge-provider setting; the Small role *is* the
  different engine until someone needs finer control.
- No ledger cross-check (a claim contradicted by a ledger row) — that
  arrives with ledger retrieval indexing.
- Not the eval harness itself — but the split/retrieve/judge loop is
  deliberately the same build, so the harness can grow from this code.

## Seams

- `db.search_chunks` / `search_chunks_trace` (db.rs:1025) — fresh
  hybrid retrieval with tracing.
- `Ai::chat_role(Role::Small, …)` (ai/mod.rs:514) — role-routed with
  fallthrough.
- `NOTE_CHUNK_PREFIX` — the self-support exclusion.
- The weave's strict-verdict parse (commands/weave.rs) — same idiom,
  different vocabulary.
