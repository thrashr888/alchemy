# RFC: Retrieval Maturity Roadmap

Status: implemented (all six phases). Numbers below are from the eval
harness; rerun with `cargo test --lib retrieval_eval -- --nocapture`.

Lessons ported from a review of a more mature retrieval stack (its ranked eval
framework, semantic KB routing, trace-corpus packaging). The original design —
LanceDB, structure-aware chunking, hybrid vector + BM25 with reciprocal rank
fusion, notes as prefixed chunks — stays. The goal was to make retrieval
measurable, configurable, and multi-stage without complicating the UX.

Guardrails (still in force):

- Keep LanceDB; the gap was strategy and evals, not the backend.
- Never make LLM reranking mandatory on the normal chat path.
- Keep note/source citation separation.
- No user-facing retrieval knobs; internal profiles and debug modes first.
- No training/distillation until the trace + eval loop exists.

## Phase 1: dataset-driven eval harness (done)

Ranked metrics (Recall@5/10, MRR@10, MAP@10, nDCG@10) over JSON query
datasets, comparing vector-only vs FTS-only vs production hybrid through the
real `db.rs` search paths.

- Runner: `src-tauri/src/retrieval_eval.rs`
  (`cargo test --lib retrieval_eval -- --nocapture`)
- Datasets: `src-tauri/evals/datasets/*.json` — queries labeled by kind
  (exact / paraphrase / section / metadata / multihop) with relevant source +
  required-substring specs.
- Report: `src-tauri/target/retrieval-eval-report.json` (byte-stable across
  runs; diff it between changes).
- One deliberate runtime touch: RRF fusion tie-breaks equal scores by chunk
  id. Ties are common (a vector-only and an FTS-only hit at the same rank
  score identically) and previously ordered randomly per process — fixing it
  moved hybrid exact-identifier MRR@10 from a coin-flip 0.88 to a stable 1.00.
- The original fixture test in `evals.rs` is preserved.

Next dataset work: anonymized real-notebook datasets; a note-as-memory query
kind (needs the FTS note-title fix below).

## Phase 2: retrieval debug trace (done)

`search_chunks` delegates to `search_chunks_trace`, which returns per-stage
hits (vector, FTS, fused pool, final top-k) plus warnings for degradations
the production path hides (FTS failure → vector-only). MCP `search_debug`
exposes the stages to agents. Eval numbers were byte-identical across the
refactor — no behavior change.

Known gap for later: `search_chunks_fts_all` returns note chunks with empty
titles, which would zero the FTS variant on a note-as-memory dataset. The
uniform fix belongs in a title-filling pass shared by all corpus-wide reads.

## Phase 3: diversity post-processing (done)

`search_chunks_all_opts` takes `SearchOptions` (pool multiplier,
`max_per_source`, `max_per_notebook`, `max_notes`). Caps apply post-fusion in
score order with backfill from skipped candidates, so shaped search never
returns fewer hits than flat — breadth is bought only with near-duplicates.
`retrieve_everything` (meta-chat) uses pool×4, 2/source, 3/notebook, 4 notes,
and gained a source-title fallback beside the note-title one.

Measured (`eval_meta_diversity`, three notebooks + a chatty dominator): mean
spec recall unchanged (0.92 = 0.92, the backfill guarantee), distinct
notebooks in top-8 up 2.7 → 3.0.

## Phase 4: semantic router (done)

`router.rs` + a `routes` LanceDB table: one embedded route per source/note
title, self-healing via summary diff (an unchanged corpus re-embeds nothing).
Meta-chat routes to the top `ROUTE_TOP_K` notebooks when the corpus has more
than `MIN_NOTEBOOKS_TO_ROUTE`; any failure falls back to flat.

Two design decisions the eval forced:

- **Per-item routes, not per-notebook summaries.** One summary vector for a
  topically diverse notebook misroutes (accuracy@2 0.83); ranking a notebook
  by its closest source scores 0.93 (`eval_router`).
- **Routing narrows only the vector side.** BM25 stays corpus-wide as the
  exact-identifier escape hatch (titles carry no error codes). With it,
  routed recall@10 equals flat (1.00) even in a top-2-of-3 stress test;
  without it, 0.93.

## Phase 5: deep-search profile (done)

`retrieve_everything(deep)` retrieves a 3× pool and asks the chat model
(shared `rerank_indices`, extracted from the agentic loop) to keep the k
passages that answer; failure degrades to fusion order, so deep can never
lose the flat result. Defaults: ON for gateway models, OFF for local —
justified by `eval_deep_rerank`: on the fixture corpus fusion order already
scores recall@4 = 1.00 (zero reranker headroom) and a local 8B rerank costs
0.03. Rerank earns its latency on buried/contested pools
(`eval_rerank_surfaces_buried_hit`), not easy ones.

Later candidates: query planning (identifier detection, metadata intent,
multi-part decomposition) and named retrieval profiles
(fast / balanced / deep / exact / meta / agent).

## Phase 6: trace export (done)

`trace.rs` appends one JSONL line per retrieval to
`<app-data>/traces/retrieval.jsonl` (5 MB rotation, one generation kept):
query, surface (chat / meta / mcp), stage hit counts, warnings, routing and
deep flags, ranked final citations. Strictly local. Together with the
`note_usage` table (reads / retrieval_hits / cited), this is the raw material
for tuning and any future small-model retrieval planner. Frontend-only
signals (citation clicks in the reader) are not yet logged; wire them through
a command when the tuning work actually needs them.
