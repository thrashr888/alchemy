# RFC: Outline Index — long documents by their own structure

Status: implemented through Phase 3, on branch for review. Companion to `RFC-retrieval-maturity.md`
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

`gist.rs` distills one gist per source. For a long structured source,
distill one short summary per top-level section with the Small role —
what the section covers in plain words *plus the other names a reader
might use for it* — stored as chunk rows with owner `section:<source_id>`,
chunk id `section:<source_id>:<start>-<end>` (the passage span), the
chain as context, the summary as text. Embedded and BM25-indexed like a
gist, so a section summary is itself a hit.

**Expansion is scored, not positional.** A section hit is swapped, in
place, for the top two passages of a mini hybrid search (both legs,
filtered to the span, RRF) — "the answer is in this section" becomes
"this passage". Two things measured wrong first: dumping the section in
document order (topic MRR 0.50 → 0.44), and leaving a vouched-for
passage "at its own rank" when the flat search had it deep in the pool
(the promotion is the point). Every section hit expands; gating on the
row's rank measured no better. The rows themselves never leave
`search_chunks_trace`: the per-notebook chat path stays verbatim-only.

Mechanics landed with handwritten fixture summaries so the measurement
is deterministic; the model-written variant is the generator's own
Ollama-gated eval. The generator (`gist::ensure_section_gists`) runs in
the gist sweep right after gists converge: sources with 3–40 top-level
sections, six sources per pass, stamped on disk by heading-and-span hash
(`section-gists.json`) so an unchanged source costs nothing and a
re-chunk reopens it. Section rows go with the chunks on refresh and with
the source on delete.

Budget: sections, not chunks — a 300-page manual with 14 chapters is
14 calls, once, refreshed on content hash like gists. Runs in the same
sweep slot as gists (event on import; hourly catch-up; never per
minute — `RFC-night-shift-area.md`, background-settle rules).

Found on the way: appending rows after an FTS index exists and taking
the incremental `optimize` path ranked one BM25-dependent query
differently run to run, and differently from a from-scratch index over
the same rows. The eval builds its index once, from scratch, after
seeding; production takes the incremental path after every import
(issue filed).

### Phase 3 — outline-guided retrieval as silent escalation

Hybrid search stays the fast default. `outline_index::escalate` runs in
the chat retrieval path beside the gap query, after fusion and before
the rerank, and fires when the pool is thin (the same `< 3 citations ||
< 700 chars` bar `rag::build_chat_messages` uses to ask a clarifying
question) or the top five hits are the same subsection of different
chapters — chains from one source ending alike, the look-alike shape.
It hands the Small role the notebook's outline (`db::notebook_outline`:
one line per section row, chain plus the first line of its summary,
capped at 80), asks for at most two section numbers or NONE, pulls each
pick's two best passages with the same in-span scored search Phase 2
expands with (`db::section_passages`), and fuses them into the pool.
One extra small-model call, silent, traced as `outlinePick` on the
chat retrieval line; a failure leaves the pool untouched.

Three shapes measured wrong before the numbers below (the on/off eval
is `eval_outline_escalation`, Ollama-gated, handwritten summaries so
the escalation is measured and not the generator):

- *Prepending the picks* ahead of the flat pool. Under bonsai's wrong
  picks the exact kind fell 1.00 → 0.20: a guess displaced a rank-one
  hit both legs agreed on. Fusion (RRF of the flat pool with the
  outline's passages) keeps a passage both sides vouch for on top and
  lands an outline-only passage behind the flat leader: topic 0.88 vs
  0.82, exact 0.33 vs 0.20 for bonsai; gemma the same either way.
- *Escalating over a literal match.* "what is part FM-2041?" quotes an
  identifier the flat leader carries verbatim; no summary can beat that,
  and bonsai's guess (already deep in the flat pool as a look-alike)
  still won the fusion. The trigger now stays quiet when a digit-bearing
  token from the question appears in the leader's snippet — exact back
  to 1.00.
- *Trusting a shotgun.* Bonsai answers "1,2,3,…,14" when the summaries
  cannot say; more than four numbers reads as NONE.

Citations gain `section` (the chain) so a long-document citation reads
"Fleet Maintenance Manual › Chapter 2: Landing Gear › Torque Values"
instead of a page of look-alike prose. The page range the Reminders
item asked for is not here: chunk rows carry ordinals, not pages.

## How Phases 2 and 3 get measured

Phase 1 leaves the gate's recall saturated (R@10 1.00) and its ranking
limited by the BM25 leg. Two consequences:

1. **Section summaries cannot move `fixture-outline` by themselves.**
   Its `must_contain` strings are numbers inside chunks; a summary row
   never contains them, and `db.search_chunks` does no expansion. Phase 2
   needs its own cases: *section-topic* queries whose answer is "which
   section covers X" — `kind: "topic"`, relevant = the section summary
   row (or the section's heading chunk). The dataset grows those cases
   before Phase 2 lands, the same way the outline kind preceded Phase 1.
2. **Escalation is a model-in-the-loop change** and measures like
   `eval_deep_rerank`: an `ALCHEMY_OLLAMA_TESTS=1` half that runs the
   real Small role over the outline, plus a deterministic half that
   checks the trigger fires on the thin/near-tied cases and stays quiet
   on the golden ones. The number to report is MRR on the outline kind
   with escalation on versus off, on the same seeded store.

The cheaper lever for ranking is **Phase 1.5**: give the BM25 leg the
chain. The FTS index is on `text`; the chain would need a `context`
column on `chunks` (a schema addition on a store shared by prod and dev
builds — `add_batch` conforms batches to the table it finds, but the
index and the query must tolerate its absence on old tables) or a
second FTS leg over a chain-only column fused at RRF. Measurable on the
existing gate with no new cases; it is the first thing to try before
Phase 2.

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
| 1.5 — chain in the BM25 document | 1.00 | 1.00 | 0.92 | 0.94 |
| 2 — section summaries (mechanics, handwritten fixtures) | 0.95 | 1.00 | 0.89 | 0.92 |
| 3 — escalation, bonsai-8b | 1.00 | 1.00 | 0.91 | 0.93 |
| 3 — escalation, gemma4 12B | 1.00 | 1.00 | 0.93 | 0.95 |

The `topic` kind (chapter paraphrases: undercarriage, de-icing,
Portugal) is Phase 2's own measurement:

| phase | topic R@5 | R@10 | MRR | nDCG |
| --- | --- | --- | --- | --- |
| after 1.5 (no section rows) | 0.69 | 0.85 | 0.50 | 0.59 |
| 2 — section rows + scored expansion, handwritten summaries | 0.85 | 1.00 | 0.79 | 0.83 |
| 2 — the generator, bonsai-8b (Small role) | 0.77 | 0.92 | 0.54–0.61 | 0.63 |
| 2 — the generator, gemma4 12B, thinking off | 0.85 | 0.92 | 0.60 | 0.68 |
| 3 — escalation on, bonsai-8b (handwritten summaries) | 1.00 | 1.00 | 0.88 | 0.91 |
| 3 — escalation on, gemma4 12B (handwritten summaries) | 1.00 | 1.00 | 0.88 | 0.91 |

The generator's eval (`eval_section_gists_model`, Ollama-gated) scores the
topic kind three ways on one store: no rows, the handwritten fixtures,
the model's. Handwritten is the ceiling to report against, not a floor
to fail on; the bar is "beats no rows", and both models clear it. Bonsai
never reaches "undercarriage" or "Portugal" and its runs spread 0.54–0.61
at Ollama's default temperature; gemma writes exactly the names the
queries use and lands 0.60 — the rest of the gap to 0.79 is the
summaries' length and phrasing against BM25, not their content.

Found on the way, and fixed: a thinking model as the Small role (gemma4)
spent the whole `num_predict` cap on hidden reasoning and returned empty
text — 35 s per section, nothing to show. The Small engine now sends
Ollama `think: false` (ignored by models without a thinking mode); the
same prompt answers in under a second.

Controls (`chapter`, `exact`) must stay at 1.00; the golden datasets'
floors are unchanged. Phase 3's rows are on/off on one seeded store
(`eval_outline_escalation`); the escalation fires on every topic and
outline query of this corpus (they all land on look-alike chains) and
on neither control kind.

Phase 1 note: the vector leg alone reaches R@10 1.00 / MRR 0.95 on the
outline kind once it carries the chain; hybrid lands lower (MRR 0.62)
because the BM25 leg, which indexes chunk `text` without the chain, is
still confused by look-alike subsections and fusion averages that in.
Phase 1.5 did that, and the *shape* mattered more than the weight. Two
nullable columns join `chunks` — `context` ("title › chain", read back
onto `Citation.section`) and `bm25` (`"{context}\n{text}"`, the one
document the FTS index covers); older tables gain both through
`add_columns`, with `bm25` seeded from `text` so no existing row drops
out of BM25, and older builds' appends conform with "". Two shapes were
measured:

- *Separate context leg* (a second FTS index, a third RRF list): outline
  MRR 0.72 / 0.79 / 0.87 / 0.86 at weights 0.25 / 0.5 / 1.0 / 2.0 — but
  hard-exact MRR fell 1.00 → 0.78, because a context-only match ("Port
  Forwarding" for "which port does the badge console use") gets a full
  rank-one vote of its own.
- *One BM25 document per chunk* (chain prepended to the text, single
  index, two legs as before): outline MRR 0.92, R@5 and R@10 1.00, and
  the golden datasets return exactly to their Phase 1 numbers. Text and
  chain terms combine in one score, which is the field weighting BM25
  was built for. This shipped.
