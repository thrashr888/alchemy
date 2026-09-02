# RFC: Outline Index — long documents by their own structure

Status: implementing (Phase 1). Companion to `RFC-retrieval-maturity.md`
(how we search) and `RFC-infinite-context.md` (gists, enrichment). Gated
end to end on `evals/datasets/fixture-outline.json`: each phase merges
only if the outline numbers move.

## Problem

A 300-page structured document — a maintenance manual, a policy binder,
a standard — is the case hybrid search handles worst. Every chapter
repeats the same subsections in the same words; only the numbers and
the chapter name differ. The chapter name lives in a heading the chunker
had already replaced by the time the subsection's chunk was cut, so
forty chunks embed and index as near-duplicates and the one the question
wants is a coin flip.

Measured (v0.53.0 baseline, hybrid, outline kind, @10): R@5 0.55, R@10
0.68, MRR 0.23, nDCG 0.34 — against 1.00 on every control kind in the
same corpus. Vector alone does better (R@10 0.82) than hybrid, which
says the BM25 leg is actively confused: the subsection words match
everywhere.

PageIndex's insight applies: a long document has a tree, and reasoning
over the tree beats searching a flat bag of look-alike chunks. We keep
the embedding floor — local models need it, and it is what makes short
documents instant — and add the tree on top.

## Design

Three phases, each a Reminders item, each behind the gate.

### Phase 1 — the heading chain (persist the outline at ingest)

`chunk_text` tracks the open heading chain by level instead of the last
heading seen. A chunk under `## Torque Values` inside `# Chapter 2:
Landing Gear` embeds as `[Manual › Chapter 2: Landing Gear › Torque
Values]`, and `Chunk.section` carries the chain. Layout-derived, no
model, free at import.

The outline itself is *derivable*: heading chunks keep their heading
line verbatim (`## …`), so a source's outline is a scan of its chunk
rows in ordinal order — level from the `#` count, span from the
ordinals between headings. Phase 1 stores nothing new; `outline_of`
builds the tree on demand from `source_chunk_rows`. A persisted table
arrives only if Phase 3's escalation needs it across a whole notebook
faster than a scan gives it.

What this buys on the gate: the vector leg now carries the chapter
name. What it cannot buy: the BM25 leg still indexes chunk `text`,
which does not contain the chain. That is deliberate — display text is
what a citation quotes and what click-to-highlight matches; polluting
it with a heading path would show up in the reader.

### Phase 2 — per-section summaries (hierarchical gists)

`gist.rs` distills one gist per source. For a long structured source
(outline depth ≥ 2 and ≥ N sections), distill one short summary per
top-level section with the Small role, stored as chunk rows with owner
`section:<source_id>` and `ordinal` = section index, text
`"<chain>: <summary>"`. Embedded like gists, so a section summary is
itself searchable; retrieval treats them like gist rows (capped,
flagged) and a hit on one expands to the section's chunks the way
neighbor expansion widens a hit today.

Budget: sections, not chunks — a 300-page manual with 14 chapters is
14 calls, once, refreshed on content hash like gists. Runs in the same
sweep slot as gists (event on import; hourly catch-up; never per
minute — `RFC-night-shift-area.md`, background-settle rules).

### Phase 3 — outline-guided retrieval as silent escalation

Hybrid search stays the fast default. Escalate when the answer would be
thin (the existing `citations.len() < 3 || snippet_chars < 700` gate
in `rag::build_chat_messages`) or the top hits are all from one long
structured source with near-tied scores: hand the Small role the
notebook's outlines (chains + section summaries, capped), ask it to
pick sections, pull those sections' chunks by `(source_id, ordinal
range)`, and merge them into the excerpt pool ahead of the rerank.
Same escalation shape as `second_look` and the deep rerank: invisible
to the user, one extra small-model call, traced in
`traces/retrieval.jsonl` with `stage: "outline"`.

Citations gain `section` (the chain) so a long-document citation reads
"Fleet Maintenance Manual › Chapter 2: Landing Gear › Torque Values"
instead of a page of look-alike prose.

## Non-goals

- Not vectorless. The tree is a second signal, not a replacement.
- No UI in Phases 1–3. The citation chain is the only visible change.
- No re-chunking of existing stores on upgrade. Existing chunks keep
  their old embed context until the source is refreshed; the eval seeds
  fresh, so the gate measures the new path.

## Gate

`ALCHEMY_EVALS=1 cargo test --lib eval_retrieval_datasets -- --nocapture`

| phase | hybrid outline R@5 | R@10 | MRR | nDCG |
| --- | --- | --- | --- | --- |
| baseline (v0.53.0) | 0.55 | 0.68 | 0.23 | 0.34 |
| 1 — heading chain | 0.91 | 1.00 | 0.62 | 0.71 |
| 2 — section summaries | | | | |
| 3 — escalation | | | | |

Controls (`chapter`, `exact`) must stay at 1.00; the golden datasets'
floors are unchanged.

Phase 1 note: the vector leg alone reaches R@10 1.00 / MRR 0.95 on the
outline kind once it carries the chain; hybrid lands lower (MRR 0.62)
because the BM25 leg, which indexes chunk `text` without the chain, is
still confused by look-alike subsections and fusion averages that in.
Giving BM25 the chain (a `context` column, or the chain in the FTS
document) is the obvious Phase 1.5 and is measurable the same way.
