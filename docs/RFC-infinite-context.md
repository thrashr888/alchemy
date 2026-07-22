# RFC: Infinite Context — corpus-scale retrieval & distillation

Status: draft. Companion to `RFC-retrieval-maturity.md` (done — how we
search) and `RFC-inference-providers.md` (in flight — who answers). This RFC
covers what we index and how evidence scales with corpus size and answer
model, so a 10M+ char source list feels fully available in every answer.

## Problem

Real notebooks are past 3M chars of sources; the target is 10M+. At
~280-word chunks, 10M chars is only ~6,000 chunks — LanceDB handles that
without ANN tuning, so storage and latency are not the problem. Quality is:

- **Fixed budgets dilute.** Chat retrieves k=8 ≈ 13k chars — 0.13% of a 10M
  corpus. Every imported source adds distractors competing for the same
  eight slots.
- **Global questions have no top-k answer.** "What do my sources disagree
  on?" / "summarize the themes" needs coverage, not similarity.
- **Source types rank on incompatible scales.** Verbatim code, table rows,
  transcripts, and articles embed with different densities; low-density
  conversational text embeds worst raw.
- **Answer models diverge.** Opus-class models digest 20 messy excerpts;
  3–8B locals (Granite, Qwen, Apple FM) lose the thread. One evidence
  packet shape cannot serve both.

"Infinite" is an illusion to maintain, and it decomposes into measurable
claims: retrieval quality flat as the corpus grows, no question class
(pointed, sectional, global) unanswerable, no source type or answer model
left behind.

## Evidence base

| Study | Result | What it justifies here |
| --- | --- | --- |
| Anthropic contextual retrieval (2024) | failed retrievals −35% (embeddings), −49% (+BM25), −67% (+rerank) | context-enriched `embed_text`; we ship the free tier ([title › section]) — the LLM tier is Phase 2 |
| LinkedIn KG-RAG, SIGIR '24 | +77.6% MRR in production | structure conversational data at ingest, don't embed raw |
| Amazon Meta Knowledge, KDD '24 | beats chunk RAG p<0.01; <$20 per 2k docs (Haiku) | gists/synthetic-QA are cheap on the Small role |
| Dense X Retrieval, EMNLP '24 | +10.1 R@20 on weak retrievers vs +2.2 on strong | distillation headroom concentrates in **local** mode |
| RAPTOR, ICLR '24 | +20% absolute on QuALITY | a summary layer above chunks unlocks global questions |
| Doc2Query--, ECIR '23 | filtering generated text: +16% effectiveness, −48% index | gate every generated artifact |
| LazyGraphRAG, MSR '24 | index at 0.1% of GraphRAG cost, parity quality | defer LLM work; never eager-distill a whole corpus |
| Semantic-chunking study, NAACL Findings '25 | clever chunking often ≤ fixed-size | no ingest sophistication without a measured delta |
| Cerebras / Snyk / Slack (2024–26, production) | independent convergence | distill threads; keep raw for FTS + citations |

## Guardrails

Carried from `RFC-retrieval-maturity.md`:

- LanceDB stays. No new storage engine, no external services.
- No mandatory LLM on the normal chat path; every new stage degrades to
  today's behavior on any failure.
- No user-facing retrieval knobs. One Settings toggle ("ingest enrichment"),
  smart-default ON when a capable provider is configured — cost control,
  not a quality switch.
- Every phase lands with eval deltas **against the production baseline**
  (hybrid + prefix + optional deep rerank), not against naive vector search.

New here:

- `Chunk.text` stays verbatim — citations, click-to-highlight, and FTS are
  grounded in what the source actually says. Distillation only ever adds
  rows or changes `embed_text`.
- Import never blocks on an LLM. Distillation is a background pass
  (fire-and-forget queue, the note-curator pattern in `commands.rs`).
- Generated artifacts are gated (Doc2Query-- lesson): identifier-overlap
  with raw text, length bounds, degeneracy checks; on failure, keep
  prefix-only behavior and move on.
- Added rows are capped at ~5% of chunk count so the index never bloats.

## Phase 1: source gists

One distilled row per source, stored in `chunks` with
`source_id = "gist:<source_id>"` (the `note:` prefix pattern), embedded and
FTS-indexed like any chunk:

- Content: 3–6 sentence gist, key entities/identifiers, and the questions
  this source can answer (Meta Knowledge / Snyk shape).
- Produced on `Role::Small`, background queue after import completes;
  re-gisted when the source's content hash changes (the `ensure_router`
  diff pattern — unchanged corpus re-does nothing).
- Gates before write: every identifier in the gist must appear in the raw
  text; length in [200, 1200] chars; no repetition degeneracy.
- Dual use: the gist replaces the title as the router summary (better
  meta-chat routing for free) and can surface on source cards later.

Retrieval: gist rows join fusion as their own capped class
(`SearchOptions.max_gists`, default 2) — RAPTOR's lesson at depth 1. A gist
hit cites the source; drill-down re-searches scoped to it.

Status: **implemented** (`gist.rs`; gates unit-tested there, retrieval
covered by `eval_gist_rows`; dataset-report metrics byte-identical across
the change). Decisions the eval forced:

- **Backfill leaked the cap.** Diversity backfill re-admits capped
  candidates to guarantee count — correct for near-duplicate chunks, wrong
  for gists (3 surfaced past `max_gists: 2` on an overview query). Backfill
  is now two-tier: skipped verbatim chunks first, skipped gists only into
  otherwise-empty slots. Count guarantee intact; the cap is real.
- **Gists are corpus-wide evidence only** (meta-chat, MCP, as-you-type
  FTS). The per-notebook chat path filters them out until the citation
  reader can render a gist hit (its `ordinal` is a content hash, so
  click-to-highlight can't resolve it).
- The FTS empty-title gap is fixed by a shared `corpus_titles` pass
  (sources + notes + gists) used by every corpus-wide read; `eval_gist_rows`
  asserts no empty titles come back.
- Dataset-runner `gist`/`global` query kinds wait for a corpus-wide dataset
  runner — the current runner exercises the per-notebook path, which now
  excludes gists by design. That runner arrives with Phase 3's scale
  corpora.
- Settings toggle is backend-only for now (`AiConfig.source_gists`,
  default ON); the Settings UI row can land with the source-card gist
  surface.

## Phase 2: distilled embeddings for low-density types

Per-type policy, scoped by the counter-evidence:

| Type | Embed treatment |
| --- | --- |
| url, page capture, transcripts, chat-like | per-chunk LLM enrichment: `embed_text` = one situating sentence (what this chunk is, in context of the document) + verbatim text |
| pdf, markdown, docx, text | keep `[title › section]` prefix — clean prose has near-zero measured headroom |
| code | keep path-prefix (already the high-leverage trick) |

Enrichment is lazy: background after gisting, oldest-source-last, skippable
entirely in local-only mode (Dense X says local gains most, but Small-role
throughput is the constraint — gists first, chunks only when idle).
Enrichment hash-invalidates with source refresh.

Also here: the boilerplate gate. URL-type chunks that are all-common-token
nav cruft (no rare term, no heading, short) are ingested to FTS but not
embedded — junk never enters the vector space (Cerebras burst thresholds).

Eval: messy-source datasets built from real captured pages and transcript
fixtures. Target: ≥20% failed-retrieval reduction on messy sets, ≥0% on
clean sets (regression fence).

## Phase 3: scale-adaptive evidence assembly

- `retrieve_k` scales with corpus size (log curve) inside the resolved
  `ContextProfile` budget — 8 for a 50k-char notebook is generous; for 10M
  it's starvation.
- Post-rank neighbor expansion: final citations pull ordinal ±1 as context
  (deduped against overlap), budget-gated per profile — deep/gateway ON,
  small-local OFF.
- Recency joins the RRF tie-break (score, then source recency, then chunk
  id — determinism preserved, Phase-1-of-retrieval-RFC lesson kept).
- Scale eval corpora: 1M / 3M / 10M chars assembled from public fixtures +
  anonymized real notebooks. The "infinite" claim made falsifiable:
  recall@k curves must stay flat (±5%) across the three sizes.

Status: **implemented**.

- Adaptive k (`ContextProfile::retrieve_k_for`: +1 per doubling past ~200k
  chars, capped by `retrieve_k_max`; 16 default, 6 on-device), post-rank
  neighbor expansion (`Db::expand_neighbor_excerpts`: prompt-only,
  profile-gated, higher ranks claim neighbors first, no ordinal included
  twice), and the recency tie-break (`fused_cmp`: score → owner recency →
  chunk id; gists inherit their source's timestamp) are in with unit
  tests; the dataset report was byte-identical across those changes. One
  honesty note: a true RRF tie is a vector-only vs FTS-only hit at equal
  rank — constructing that end to end is fixture-fragile, so the recency
  rule is proven at the extracted comparator, not through the full stack.
- The scale fence (`eval_scale_fence`): deterministic synthetic corpora
  (xorshift-seeded distractor binders, identifier spaces disjoint from 12
  fixed needles). Measured: **1M chars exact 1.00 / paraphrase 1.00
  (k=10) · 3M exact 1.00 / paraphrase 1.00 (k=11)** — recall flat across a
  tripling, adaptive k growing as designed. Ignored by default (per-doc
  LanceDB seeding ≈ 10 min); run on retrieval changes with
  `cargo test --lib eval_scale_fence -- --ignored --nocapture`; the 10M
  variant is `eval_scale_fence_10m`. A bulk-insert seeding path would move
  it into the default suite — noted as follow-up, not blocking.
- The dataset runner gained a **meta** variant (corpus-wide
  `search_chunks_all_opts` with production caps): additive report change,
  existing variants byte-identical; meta scores R@10 1.00 / nDCG 0.95
  (core) and 1.00 / 0.92 (hard) — the corpus-wide surface is on par with
  per-notebook retrieval on the fixtures, and `gist`/`global` dataset
  kinds now have a runner to land in.

## Phase 4: global answers (lazy map-reduce)

Classify the query (heuristics first: enumerative/comparative markers;
`Role::Small` assist only when ambiguous):

- **Pointed** → today's path, untouched.
- **Global** → retrieve gist rows corpus-wide (bounded, diversity-capped)
  → for each covered source that needs depth, one `Role::Small` extract
  scoped to that source → one `Role::Chat` synthesis over the extracts,
  cited at source granularity with chunk drill-down on demand.

This is LazyGraphRAG's shape: the hierarchy (gists) is cheap and standing;
per-question LLM work happens only for the sources the question touches.
Fan-out is bounded by the model's ContextProfile; failure at any step
degrades to the pointed path.

Eval: global-question dataset scored on citation precision/recall against
labeled source sets (objective — no LLM-judge theater), plus a small
hand-checked answer set.

## Phase 5: model-tiered evidence shaping

Extend `ContextProfile` (inference RFC §2) with evidence-shape parameters:
neighbor expansion on/off, gist budget, global fan-out cap, excerpt style
(compact numbered for ≤8B, sectioned-with-headers for frontier). Same
retrieval, different packet. Tiers diverge from today's constants only on
eval evidence, per the profile-eval harness already in tree.

Eval: cross-model matrix (Opus-class gateway, Granite/Qwen via Ollama,
Apple FM) over the shared datasets; per-tier citation precision and latency
budgets. "Extreme quality on disparate models" = the small-model tier loses
≤10% citation precision vs frontier on the same questions, at its own k.

## Costs

- Gist: one Small call per source (~1–3s local 3–8B; fractions of a cent
  gateway). A 200-source notebook backfills in minutes, in the background.
- Chunk enrichment: only messy types, typically 10–30% of a corpus; a 10M
  fully-messy corpus is ~6k Small calls — hours local (hence lazy +
  skippable), single-digit dollars gateway (Meta Knowledge's arithmetic).
- Storage: +≤5% rows; embeddings dominate and scale the same.

## Risks

- Hallucinated gists → the identifier-overlap gate; a bad gist misroutes
  worse than no gist.
- Staleness on edit/refresh → content-hash invalidation everywhere.
- Background queue complexity → reuse the curator's fire-and-forget
  pattern; queue state is re-derivable from hashes, never authoritative.
- Eval overfit to fixtures → anonymized real-notebook datasets (Phase 1 of
  the retrieval RFC flagged this; this RFC needs it delivered).
- No capable Small role available → everything above degrades to today's
  app, by construction.

## Open questions

- Gist rows and the empty-FTS-title gap: fix the shared title-filling pass
  (retrieval RFC Phase 2 known gap) before or with Phase 1?
- Notebook-level rollups (depth-2 RAPTOR) — wait until gists prove out in
  meta-chat, or land with Phase 4?
- Surface gists on source cards now, or leave UI to the document-surface
  RFC?
