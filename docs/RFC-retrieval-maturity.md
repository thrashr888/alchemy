# RFC: Retrieval Maturity Roadmap

Status: Phase 1 implemented. Phases 2–6 pending.

Lessons ported from a review of a more mature retrieval stack (its ranked eval
framework, semantic KB routing, trace-corpus packaging). The current design —
LanceDB, structure-aware chunking, hybrid vector + BM25 with reciprocal rank
fusion, notes as prefixed chunks — stays. The goal is to make retrieval
measurable, configurable, and multi-stage without complicating the UX.

Guardrails:

- Keep LanceDB; the gap is strategy and evals, not the backend.
- Never make LLM reranking mandatory on the normal chat path.
- Keep note/source citation separation.
- No user-facing retrieval knobs; internal profiles and debug modes first.
- No training/distillation until the trace + eval loop exists.

## Phase 1: dataset-driven eval harness (done)

Ranked metrics (Recall@5/10, MRR@10, MAP@10, nDCG@10) over JSON query
datasets, comparing vector-only vs FTS-only vs production hybrid through the
real `db.rs` search paths. No runtime behavior change.

- Runner: `src-tauri/src/retrieval_eval.rs`
  (`cargo test --lib retrieval_eval -- --nocapture`)
- Datasets: `src-tauri/evals/datasets/*.json` — queries labeled by kind
  (exact / paraphrase / section / metadata / multihop) with relevant source +
  required-substring specs.
- Report: `src-tauri/target/retrieval-eval-report.json`
- The original fixture test in `evals.rs` is preserved.

Next dataset work: add anonymized real-notebook datasets; add note-as-memory
query kind once note weighting (Phase 3/5 below) is on the table.

## Phase 2: retrieval debug trace

Refactor `search_chunks` internals to return a `SearchTrace` (vector hits,
FTS hits, fused hits, final hits, warnings — e.g. FTS silently failing today)
while keeping the public return type unchanged. Expose via a dev command
and/or MCP `search_debug`.

## Phase 3: diversity post-processing

Post-fusion caps: `max_per_source`, `max_per_notebook` (meta-chat),
`max_notes`, with a larger candidate pool. Apply first to
`retrieve_everything`, where one large notebook can starve the rest. Also:
extend the note-title fallback in `retrieve_everything` to source titles.

## Phase 4: semantic router for notebooks/sources

The highest-leverage idea from that review. Embed per-notebook and per-source summaries
(title + source titles + recent note/report titles; title + type + headings +
excerpt), route the query to top candidates, search within them, fall back to
flat search on low confidence. Compare flat vs routed in the Phase 1 harness
before defaulting on.

## Phase 5: optional rerank / deep-search profile

Larger candidate pool (30–60) → existing LLM reranker → top 6–12, only for
deep search, meta-chat, and agentic retrieval. Prove MRR/nDCG gains on a
buried-hit dataset first. Later candidates: query planning (identifier
detection, metadata intent, multi-part decomposition) and retrieval profiles
(fast / balanced / deep / exact / meta / agent).

## Phase 6: trace export

Export retrieval runs as JSONL (query, scope, per-stage hits, final
citations, user signals: citation clicks, note saves, regenerations).
Local-only by default. This is the raw material for tuning and any future
small-model retrieval planner.
