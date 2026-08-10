//! BEIR datasets against the real retrieval pipeline — public yardsticks
//! beside the synthetic evals in `retrieval_eval.rs`. Same chunker, same
//! embedder tiers, same hybrid search (vector + BM25, RRF-fused) the app
//! ships; scores are comparable to published BEIR numbers, so a chunking or
//! fusion regression shows up as a number the IR community understands.
//!
//! Every run also DIAGNOSES: each leg is scored alone (is fusion earning
//! its keep?) and an offline RRF weight sweep re-fuses the captured legs at
//! several vector weights — tuning evidence without touching the shipping
//! path. Variants measure the Ollama embedder tier (`_nomic`) and the
//! model reranker (`_rerank`) on top of the same corpora.
//!
//!   cargo test --lib beir_ -- --ignored --nocapture

use std::collections::HashMap;

use crate::ai::{Ai, AiConfig, AiRuntime};
use crate::db::Db;
use crate::evals::builtin_ai;
use crate::ingest;
use crate::models::Citation;

const BEIR_BASE: &str = "https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets";
/// Docs per embed+insert flush — a handful of Lance commits per corpus.
const SEED_BATCH: usize = 2_048;
/// Vector weights swept offline against the captured legs (BM25 fixed at 1).
const SWEEP: &[f64] = &[0.0, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0];
/// Queries sampled for the rerank variant — one model call per query.
const RERANK_SAMPLE: usize = 100;

/// The cached dataset dir, downloading and unpacking on first use. Lives in
/// target/ so `cargo clean` is the eviction policy. Names beginning with
/// "Nano" fetch the matching zeta-alpha-ai NanoBEIR dataset from the
/// HuggingFace rows API and land in the same BEIR file layout, so the rest
/// of the harness never knows the difference.
async fn dataset_dir(name: &str) -> Option<std::path::PathBuf> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/beir-cache");
    let dir = cache.join(name);
    if dir.join("corpus.jsonl").exists() {
        return Some(dir);
    }
    std::fs::create_dir_all(&cache).ok()?;
    if name.starts_with("Nano") {
        return fetch_nano(name, &dir).await;
    }
    let url = format!("{BEIR_BASE}/{name}.zip");
    eprintln!("beir: downloading {name} ({url})…");
    // Own client, generous timeout: ingest::fetch_bytes caps at 15s (right
    // for page fetches, fatal for FiQA's 18 MB on a slow moment).
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .ok()?;
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?.to_vec();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    zip.extract(&cache).ok()?;
    dir.join("corpus.jsonl").exists().then_some(dir)
}

/// Pull one NanoBEIR dataset through the HF datasets-server rows API and
/// write it in BEIR's file layout (corpus.jsonl / queries.jsonl /
/// qrels/test.tsv). Nano qrels carry no score column — binary 1s.
async fn fetch_nano(name: &str, dir: &std::path::Path) -> Option<std::path::PathBuf> {
    eprintln!("beir: fetching {name} from HuggingFace…");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .ok()?;
    let fetch = |config: &'static str| {
        let client = client.clone();
        async move {
            let mut rows: Vec<serde_json::Value> = Vec::new();
            loop {
                let url = format!(
                    "https://datasets-server.huggingface.co/rows?dataset=zeta-alpha-ai%2F{name}\
                     &config={config}&split=train&offset={}&length=100",
                    rows.len()
                );
                // Anonymous datasets-server rate limits bite after ~30
                // rapid pages — pace requests and back off on 429.
                let mut attempt = 0u32;
                let batch: serde_json::Value = loop {
                    let resp = client.get(&url).send().await.ok()?;
                    if resp.status().as_u16() == 429 && attempt < 6 {
                        attempt += 1;
                        tokio::time::sleep(std::time::Duration::from_secs(5 * u64::from(attempt)))
                            .await;
                        continue;
                    }
                    if !resp.status().is_success() {
                        return None;
                    }
                    break resp.json().await.ok()?;
                };
                let page = batch["rows"].as_array()?.to_vec();
                if page.is_empty() {
                    break;
                }
                rows.extend(page.into_iter().map(|r| r["row"].clone()));
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }
            Some(rows)
        }
    };
    let corpus = fetch("corpus").await?;
    let queries = fetch("queries").await?;
    let qrels = fetch("qrels").await?;
    std::fs::create_dir_all(dir.join("qrels")).ok()?;
    let dump = |rows: &[serde_json::Value]| {
        rows.iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    };
    std::fs::write(dir.join("corpus.jsonl"), dump(&corpus)).ok()?;
    std::fs::write(dir.join("queries.jsonl"), dump(&queries)).ok()?;
    let tsv = std::iter::once("query-id\tcorpus-id\tscore".to_string())
        .chain(qrels.iter().filter_map(|r| {
            Some(format!(
                "{}\t{}\t1",
                r["query-id"].as_str()?,
                r["corpus-id"].as_str()?
            ))
        }))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("qrels/test.tsv"), tsv).ok()?;
    Some(dir.to_path_buf())
}

fn jsonl(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// nDCG@k with GRADED gains (gain = qrel score, the trec_eval convention
/// BEIR reports) — SciFact's all-1 qrels make this identical to binary.
fn ndcg_at_k(ranked: &[String], rels: &HashMap<String, i32>, k: usize) -> f64 {
    let dcg: f64 = ranked
        .iter()
        .take(k)
        .enumerate()
        .filter_map(|(i, d)| rels.get(d).map(|s| *s as f64 / (i as f64 + 2.0).log2()))
        .sum();
    let mut ideal_gains: Vec<i32> = rels.values().copied().collect();
    ideal_gains.sort_unstable_by(|a, b| b.cmp(a));
    let ideal: f64 = ideal_gains
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, s)| *s as f64 / (i as f64 + 2.0).log2())
        .sum();
    if ideal == 0.0 {
        0.0
    } else {
        dcg / ideal
    }
}

/// Chunk hits → first-appearance document order, capped.
fn collapse_docs(hits: &[Citation], cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in hits {
        if !out.contains(&h.source_id) {
            out.push(h.source_id.clone());
        }
        if out.len() == cap {
            break;
        }
    }
    out
}

/// The nomic-embed Ollama tier, pinned explicitly — the config DEFAULT
/// moved to mxbai-embed-large after the A/B, and these baselines predate
/// that. None (skip, not fail) when Ollama isn't reachable.
async fn nomic_ai() -> Option<Ai> {
    let ai = Ai::new(
        AiConfig {
            embed_model: "nomic-embed-text:latest".into(),
            ..Default::default()
        },
        AiRuntime::default(),
    );
    match ai.test_embed().await {
        Ok(_) => Some(ai),
        Err(_) => {
            eprintln!("SKIP: Ollama embedder unavailable");
            None
        }
    }
}

/// Built-in embedder + a small live chat model for the rerank leg.
async fn rerank_ai() -> Option<Ai> {
    let ai = Ai::new(
        AiConfig {
            embedder: "builtin".into(),
            // The repo's fast-local pick (tests.rs uses it for chat steps).
            chat_model: "digitsflow/bonsai-8b:latest".into(),
            ..Default::default()
        },
        AiRuntime::default(),
    );
    ai.test_embed().await.ok()?;
    Some(ai)
}

/// A chat tier by explicit provider entry — the flat `provider` field is
/// legacy and does NOT route "fm" or agent kinds (the engine-id guard in
/// the comparison test caught exactly that: both silently resolved to
/// Ollama and would have been measured under the wrong label).
fn chat_tier(kind: &str, runtime: AiRuntime) -> Ai {
    let mut config = AiConfig {
        embedder: "builtin".into(),
        ..Default::default()
    };
    config.providers.push(crate::ai::ProviderEntry {
        id: kind.to_string(),
        kind: kind.to_string(),
        label: kind.to_string(),
        base_url: String::new(),
        api_key: String::new(),
        chat_model: String::new(),
    });
    config.chat_provider = kind.to_string();
    Ai::new(config, runtime)
}

/// Apple's on-device model for the rerank leg, via the repo-built sidecar.
fn fm_ai() -> Option<Ai> {
    let sidecar = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sidecar/alchemy-fm/.build/release/alchemy-fm");
    if !sidecar.exists() {
        eprintln!("SKIP: FM sidecar not built");
        return None;
    }
    Some(chat_tier(
        "fm",
        AiRuntime {
            fm_sidecar: Some(sidecar),
            ..Default::default()
        },
    ))
}

/// The codex subscription CLI for the rerank leg — one `codex exec` per
/// query, so callers sample rather than sweep.
fn codex_ai() -> Option<Ai> {
    Some(chat_tier("codex", AiRuntime::default()))
}

/// How the rerank leg prompts the model. The shipping listwise-JSON shape
/// assumes a model that can hold 20 passages and emit clean JSON — measured
/// destructive on small tiers, so the eval races friendlier shapes.
#[derive(Clone, Copy, PartialEq)]
enum RerankStrategy {
    /// The shipping prompt: fused top-20, 300-char snippets, JSON reply.
    Listwise,
    /// Small-model shape: top-10 only, 150-char snippets, and the reply is
    /// bare comma-separated numbers — no JSON for a 4k-window model to flub.
    ListwiseLite,
    /// One tiny call per passage: "rate 0–10, reply with only the number."
    /// Ties keep fusion order, so a lazy uniform rating degrades to the
    /// fused ranking instead of scrambling it.
    Pointwise,
}

impl RerankStrategy {
    fn label(self) -> &'static str {
        match self {
            RerankStrategy::Listwise => "listwise-json",
            RerankStrategy::ListwiseLite => "listwise-lite",
            RerankStrategy::Pointwise => "pointwise",
        }
    }
}

/// Rerank the fused hits into a doc order under one strategy. None = the
/// model's reply was unusable (counted, so garbage output is visible).
async fn rerank_docs(
    rr: &Ai,
    strategy: RerankStrategy,
    qtext: &str,
    hits: &[Citation],
) -> Option<Vec<String>> {
    match strategy {
        RerankStrategy::Listwise => {
            let snippets: Vec<(String, String)> = hits
                .iter()
                .map(|h| {
                    let head: String = h.snippet.chars().take(300).collect();
                    (h.source_title.clone(), head)
                })
                .collect();
            let picked = crate::agent::rerank_indices(rr, qtext, &snippets, 10).await?;
            let ordered: Vec<Citation> = picked.into_iter().map(|i| hits[i].clone()).collect();
            Some(collapse_docs(&ordered, 10))
        }
        RerankStrategy::ListwiseLite => {
            let top: Vec<&Citation> = hits.iter().take(10).collect();
            let list = top
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let head: String = h.snippet.chars().take(150).collect();
                    format!("{i}: {head}")
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = vec![
                crate::ai::ChatTurn::system(
                    "You rank search snippets by how well they answer a question. \
                     Reply with ONLY the snippet numbers, best first, separated by \
                     commas. Example reply: 3,0,7,1",
                ),
                crate::ai::ChatTurn::user(format!(
                    "Question: {qtext}\n\nSnippets:\n{list}\n\nNumbers:"
                )),
            ];
            let out = rr.chat(&messages).await.ok()?.text;
            let mut seen = std::collections::HashSet::new();
            let picked: Vec<usize> = out
                .split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse::<usize>().ok())
                .filter(|&i| i < top.len() && seen.insert(i))
                .collect();
            if picked.is_empty() {
                return None;
            }
            let ordered: Vec<Citation> = picked.into_iter().map(|i| top[i].clone()).collect();
            Some(collapse_docs(&ordered, 10))
        }
        RerankStrategy::Pointwise => {
            let top: Vec<&Citation> = hits.iter().take(10).collect();
            let mut scored: Vec<(usize, f64)> = Vec::new();
            for (i, h) in top.iter().enumerate() {
                let head: String = h.snippet.chars().take(200).collect();
                let messages = vec![
                    crate::ai::ChatTurn::system(
                        "Rate how directly the snippet answers the question, 0 \
                         (unrelated) to 10 (directly answers). Reply with ONLY \
                         the number.",
                    ),
                    crate::ai::ChatTurn::user(format!(
                        "Question: {qtext}\n\nSnippet: {head}\n\nRating:"
                    )),
                ];
                let score = rr
                    .chat(&messages)
                    .await
                    .ok()?
                    .text
                    .split(|c: char| !c.is_ascii_digit())
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| s.parse::<u32>().ok())
                    .next()?
                    .min(10) as f64;
                scored.push((i, score));
            }
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
            let ordered: Vec<Citation> = scored.into_iter().map(|(i, _)| top[i].clone()).collect();
            Some(collapse_docs(&ordered, 10))
        }
    }
}

/// Task prefixes for asymmetric embedders. nomic-embed is TRAINED on
/// "search_document: " / "search_query: " and measurably underperforms on
/// bare text — the app embeds bare today, so the eval measures the gap
/// before any migration ships (changing document embeddings invalidates
/// every stored vector; mixing spaces is worse than either alone).
#[derive(Clone, Copy, Default)]
struct EmbedStyle {
    doc_prefix: &'static str,
    query_prefix: &'static str,
}

const NOMIC_STYLE: EmbedStyle = EmbedStyle {
    doc_prefix: "search_document: ",
    query_prefix: "search_query: ",
};

struct BeirRun {
    ndcg: f64,
    recall: f64,
    mrr: f64,
    precision: f64,
    vec_ndcg: f64,
    fts_ndcg: f64,
    sweep: Vec<(f64, f64)>,
    /// (scored queries, mean nDCG) for the rerank sample; None = not run or
    /// every rerank call failed (model missing).
    rerank: Option<(usize, f64)>,
    docs: usize,
    queries: usize,
}

/// Seed a dataset's corpus through the real pipeline and score the shipping
/// hybrid search over its test-split qrels — plus the per-leg diagnosis and
/// the offline weight sweep. Returns None when the network isn't there.
async fn run_beir(
    name: &str,
    ai: &Ai,
    rerank_with: Option<(&Ai, RerankStrategy)>,
    style: EmbedStyle,
    // Names the seeded-corpus cache alongside the dataset ("builtin",
    // "nomic", "mxbai-prefixed", …): seeding FiQA through Ollama costs ~10
    // minutes, and every A/B variant after the first should cost none.
    slug: &str,
) -> Option<BeirRun> {
    let dir = dataset_dir(name).await.or_else(|| {
        eprintln!("SKIP: {name} download failed (network?)");
        None
    })?;

    // Corpus: {_id, title, text} per line, chunked exactly like an import.
    let corpus = jsonl(&dir.join("corpus.jsonl"));
    let db_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/beir-cache")
        .join(format!("db-{name}-{slug}"));
    // The marker guards against half-seeded caches from an aborted run.
    let seeded_marker = db_dir.join("seeded.ok");
    let already_seeded = seeded_marker.exists();
    let db = Db::open(&db_dir).await.expect("open db");
    // Fusion follows the embedder tier, exactly as the app stamps it.
    db.set_vector_weight(ai.vector_weight());
    db.defer_fts(true);
    let mut rows: Vec<(String, String, i32, String)> = Vec::new();
    let mut inputs: Vec<String> = Vec::new();
    let mut seeded_docs = 0usize;
    for doc in corpus.iter().take_while(|_| !already_seeded) {
        let id = doc["_id"].as_str().unwrap_or_default().to_string();
        let title = doc["title"].as_str().unwrap_or_default();
        let body = doc["text"].as_str().unwrap_or_default();
        if id.is_empty() || body.is_empty() {
            continue;
        }
        for (j, c) in ingest::chunk_text(title, body).into_iter().enumerate() {
            rows.push((id.clone(), format!("{id}-c{j}"), j as i32, c.text));
            inputs.push(format!("{}{}", style.doc_prefix, c.embed_text));
        }
        seeded_docs += 1;
        if rows.len() >= SEED_BATCH {
            let embeddings = ai.embed(&inputs).await.expect("embed corpus batch");
            db.add_chunk_rows(name, &rows, &embeddings)
                .await
                .expect("seed chunk rows");
            rows.clear();
            inputs.clear();
            eprintln!("beir {name}: seeded {seeded_docs}/{} docs", corpus.len());
        }
    }
    if !rows.is_empty() {
        let embeddings = ai.embed(&inputs).await.expect("embed corpus tail");
        db.add_chunk_rows(name, &rows, &embeddings)
            .await
            .expect("seed chunk tail");
    }
    db.defer_fts(false);
    if already_seeded {
        eprintln!("beir {name}: corpus cache hit ({slug})");
        seeded_docs = corpus.len();
    } else {
        db.flush_fts().await.expect("flush fts");
        std::fs::write(&seeded_marker, b"1").ok();
    }

    // Queries and the test-split qrels: query-id \t corpus-id \t score.
    let queries: HashMap<String, String> = jsonl(&dir.join("queries.jsonl"))
        .into_iter()
        .filter_map(|q| {
            Some((
                q["_id"].as_str()?.to_string(),
                q["text"].as_str()?.to_string(),
            ))
        })
        .collect();
    let mut qrels: HashMap<String, HashMap<String, i32>> = HashMap::new();
    for line in std::fs::read_to_string(dir.join("qrels/test.tsv"))
        .expect("qrels")
        .lines()
        .skip(1)
    {
        let mut f = line.split('\t');
        let (Some(qid), Some(did), Some(score)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let score = score.trim().parse::<i32>().unwrap_or(0);
        if score > 0 {
            qrels
                .entry(qid.to_string())
                .or_default()
                .insert(did.to_string(), score);
        }
    }

    // Deterministic order: HashMap iteration randomizes per process, which
    // silently made every run's rerank SAMPLE a different query subset —
    // two runs of the same engine differed by ±0.18 nDCG before this sort.
    let mut qrel_list: Vec<(&String, &HashMap<String, i32>)> = qrels.iter().collect();
    qrel_list.sort_by(|a, b| a.0.cmp(b.0));

    let (mut ndcg_sum, mut recall_sum, mut vec_sum, mut fts_sum, mut n) =
        (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0usize);
    let (mut mrr_sum, mut precision_sum) = (0.0f64, 0.0f64);
    // Fused nDCG over exactly the reranked queries — the honest baseline
    // for the rerank delta; whole-set fused is a different denominator.
    let mut rr_base_sum = 0.0f64;
    // Captured doc-rank lists per query, for the offline weight sweep.
    type Capture = (Vec<String>, Vec<String>, HashMap<String, i32>);
    let mut captures: Vec<Capture> = Vec::new();
    let (mut rr_sum, mut rr_n) = (0.0f64, 0usize);
    for (qid, rels) in qrel_list {
        let Some(qtext) = queries.get(qid) else {
            continue;
        };
        let qvec = ai
            .embed_one(&format!("{}{qtext}", style.query_prefix))
            .await
            .expect("embed query");
        let trace = db
            .search_chunks_trace(name, qvec, qtext, 20, None)
            .await
            .expect("search");
        // Shipping order (diversity caps applied), and each leg alone.
        let fused = collapse_docs(&trace.final_hits, 10);
        let vec_docs = collapse_docs(&trace.vector_hits, 30);
        let fts_docs = collapse_docs(&trace.fts_hits, 30);
        ndcg_sum += ndcg_at_k(&fused, rels, 10);
        vec_sum += ndcg_at_k(&vec_docs, rels, 10);
        fts_sum += ndcg_at_k(&fts_docs, rels, 10);
        let found = fused.iter().filter(|d| rels.contains_key(*d)).count();
        recall_sum += found as f64 / rels.len().min(10) as f64;
        // MRR@10: how fast the FIRST relevant doc appears — chat stuffs the
        // top passages hardest, so early precision matters most there.
        mrr_sum += fused
            .iter()
            .position(|d| rels.contains_key(d))
            .map(|i| 1.0 / (i as f64 + 1.0))
            .unwrap_or(0.0);
        precision_sum += found as f64 / 10.0;
        // The model reranker, on a sample — one chat call per query.
        if let Some(rr) = rerank_with {
            let sample = std::env::var("BEIR_RERANK_SAMPLE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(RERANK_SAMPLE);
            if rr_n < sample {
                let (rr, strategy) = rr;
                if let Some(ranked) = rerank_docs(rr, strategy, qtext, &trace.final_hits).await {
                    rr_sum += ndcg_at_k(&ranked, rels, 10);
                    rr_base_sum += ndcg_at_k(&fused, rels, 10);
                    rr_n += 1;
                }
            }
        }
        captures.push((vec_docs, fts_docs, rels.clone()));
        n += 1;
    }

    // Offline RRF weight sweep over the captured legs: what WOULD fusion
    // score at each vector weight, BM25 held at 1.0?
    let sweep: Vec<(f64, f64)> = SWEEP
        .iter()
        .map(|&w| {
            let mut total = 0.0f64;
            for (vec_docs, fts_docs, rels) in &captures {
                let mut score: HashMap<&String, f64> = HashMap::new();
                for (r, d) in vec_docs.iter().enumerate() {
                    *score.entry(d).or_default() += w / (60.0 + r as f64);
                }
                for (r, d) in fts_docs.iter().enumerate() {
                    *score.entry(d).or_default() += 1.0 / (60.0 + r as f64);
                }
                let mut ranked: Vec<(&String, f64)> = score.into_iter().collect();
                ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
                let docs: Vec<String> = ranked
                    .into_iter()
                    .take(10)
                    .map(|(d, _)| d.clone())
                    .collect();
                total += ndcg_at_k(&docs, rels, 10);
            }
            (w, total / captures.len() as f64)
        })
        .collect();

    let run = BeirRun {
        ndcg: ndcg_sum / n as f64,
        recall: recall_sum / n as f64,
        mrr: mrr_sum / n as f64,
        precision: precision_sum / n as f64,
        vec_ndcg: vec_sum / n as f64,
        fts_ndcg: fts_sum / n as f64,
        sweep,
        rerank: (rr_n > 0).then_some((rr_n, rr_sum / rr_n as f64)),
        docs: seeded_docs,
        queries: n,
    };
    let sweep_line = run
        .sweep
        .iter()
        .map(|(w, s)| format!("{w:.2}→{s:.4}"))
        .collect::<Vec<_>>()
        .join("  ");
    eprintln!(
        "\nBEIR {name} — {} docs, {} queries\n  \
         fused (shipping)  nDCG@10 {:.4}   recall@10 {:.4}   MRR@10 {:.4}   P@10 {:.4}\n  \
         bm25 leg alone    nDCG@10 {:.4}\n  \
         vector leg alone  nDCG@10 {:.4}\n  \
         sweep w_vec:      {sweep_line}",
        run.docs,
        run.queries,
        run.ndcg,
        run.recall,
        run.mrr,
        run.precision,
        run.fts_ndcg,
        run.vec_ndcg
    );
    match run.rerank {
        Some((m, s)) => {
            let base = if m > 0 { rr_base_sum / m as f64 } else { 0.0 };
            eprintln!(
                "  rerank top20→10   nDCG@10 {s:.4} vs fused {base:.4} on the same \
                 {m}-query sample (Δ{:+.4})\n",
                s - base
            )
        }
        None if rerank_with.is_some() => {
            eprintln!("  rerank            SKIPPED (model unavailable)\n")
        }
        None => eprintln!(),
    }
    Some(run)
}

// Floors sit a couple of points under each measured baseline — they catch
// regressions in chunking, fusion, or indexing, not model drift. Recalibrate
// deliberately when the embedder or chunker changes on purpose.

#[tokio::test]
#[ignore = "downloads BEIR SciFact and seeds ~5k docs — run with --ignored --nocapture"]
async fn beir_scifact_ndcg() {
    // Measured 2026-08-09 with tier-aware fusion (w_vec 0.25 for the
    // built-in leg): nDCG@10 0.6685, recall@10 0.8171 — above published
    // BM25-only (~0.665), up from 0.6314 at equal weights.
    let Some(ai) = builtin_ai().await else { return };
    let Some(run) = run_beir("scifact", &ai, None, EmbedStyle::default(), "builtin").await else {
        return;
    };
    assert!(
        run.docs > 5_000 && run.queries > 250,
        "dataset shape changed"
    );
    assert!(run.ndcg > 0.64, "nDCG@10 regressed: {:.4}", run.ndcg);
    assert!(run.recall > 0.79, "recall@10 regressed: {:.4}", run.recall);
}

#[tokio::test]
#[ignore = "downloads BEIR NFCorpus and seeds ~3.6k docs — run with --ignored --nocapture"]
async fn beir_nfcorpus_ndcg() {
    // Measured 2026-08-09 with tier-aware fusion: nDCG@10 0.3252,
    // recall@10 0.3025 — fusion now beats BM25-alone (0.3177); graded
    // medical relevance, ~38 relevant docs per query keeps recall@10 low
    // by construction.
    let Some(ai) = builtin_ai().await else { return };
    let Some(run) = run_beir("nfcorpus", &ai, None, EmbedStyle::default(), "builtin").await else {
        return;
    };
    assert!(
        run.docs > 3_000 && run.queries > 300,
        "dataset shape changed"
    );
    assert!(run.ndcg > 0.30, "nDCG@10 regressed: {:.4}", run.ndcg);
    assert!(run.recall > 0.28, "recall@10 regressed: {:.4}", run.recall);
}

#[tokio::test]
#[ignore = "downloads BEIR FiQA (~57k docs) — run with --ignored --nocapture"]
async fn beir_fiqa_ndcg() {
    // Measured 2026-08-09 with tier-aware fusion: nDCG@10 0.2456,
    // recall@10 0.3191 — above BM25-alone (0.2426) on the paraphrase-heavy
    // set; ~2 min run. The nomic tier scores 0.3466 here (beir_nomic_all).
    let Some(ai) = builtin_ai().await else { return };
    let Some(run) = run_beir("fiqa", &ai, None, EmbedStyle::default(), "builtin").await else {
        return;
    };
    assert!(
        run.docs > 50_000 && run.queries > 600,
        "dataset shape changed"
    );
    assert!(run.ndcg > 0.22, "nDCG@10 regressed: {:.4}", run.ndcg);
    assert!(run.recall > 0.29, "recall@10 regressed: {:.4}", run.recall);
}

#[tokio::test]
#[ignore = "live Ollama: all three datasets on the nomic-embed tier"]
async fn beir_nomic_all() {
    let Some(ai) = nomic_ai().await else { return };
    for name in ["scifact", "nfcorpus", "fiqa"] {
        run_beir(name, &ai, None, EmbedStyle::default(), "nomic").await;
    }
}

#[tokio::test]
#[ignore = "live Ollama: nomic WITH task prefixes vs the bare-text app default"]
async fn beir_nomic_prefixed() {
    // nomic-embed is trained asymmetric; the app embeds bare text today.
    // Unprefixed baselines (2026-08-09): scifact 0.727, fiqa 0.347.
    let Some(ai) = nomic_ai().await else { return };
    for name in ["scifact", "fiqa"] {
        run_beir(name, &ai, None, NOMIC_STYLE, "nomic-prefixed").await;
    }
}

#[tokio::test]
#[ignore = "live Ollama: rerank sample on all three datasets (builtin embedder)"]
async fn beir_rerank_all() {
    let Some(ai) = builtin_ai().await else { return };
    let Some(rr) = rerank_ai().await else { return };
    for name in ["scifact", "nfcorpus", "fiqa"] {
        run_beir(
            name,
            &ai,
            Some((&rr, RerankStrategy::Listwise)),
            EmbedStyle::default(),
            "builtin",
        )
        .await;
    }
}

/// The 13 NanoBEIR domains in one pass — 50 queries each, small corpora,
/// broad coverage in minutes on the built-in embedder.
#[tokio::test]
#[ignore = "downloads 13 NanoBEIR datasets from HuggingFace — run with --ignored --nocapture"]
async fn beir_nano_all() {
    let Some(ai) = builtin_ai().await else { return };
    let mut lines: Vec<String> = Vec::new();
    for name in [
        "NanoArguAna",
        "NanoClimateFEVER",
        "NanoDBPedia",
        "NanoFEVER",
        "NanoFiQA2018",
        "NanoHotpotQA",
        "NanoMSMARCO",
        "NanoNFCorpus",
        "NanoNQ",
        "NanoQuoraRetrieval",
        "NanoSCIDOCS",
        "NanoSciFact",
        "NanoTouche2020",
    ] {
        if let Some(run) = run_beir(name, &ai, None, EmbedStyle::default(), "builtin").await {
            lines.push(format!(
                "{name:<22} nDCG@10 {:.4}   recall@10 {:.4}   MRR@10 {:.4}",
                run.ndcg, run.recall, run.mrr
            ));
        }
    }
    eprintln!("\n== NanoBEIR summary (built-in embedder) ==");
    for l in &lines {
        eprintln!("  {l}");
    }
    // HF's anonymous rate limits make a few fetch failures weather, not
    // signal — fetched datasets cache forever, so re-runs converge on 13.
    assert!(lines.len() >= 8, "most Nano datasets should have run");
}

/// Embedder A/B over the divergent pair (scifact lexical, fiqa paraphrase).
/// BEIR_EMBED_MODELS overrides the candidate list.
#[tokio::test]
#[ignore = "live Ollama: embedder A/B on scifact + fiqa — pulls compare against nomic"]
async fn beir_embedder_ab() {
    let models = std::env::var("BEIR_EMBED_MODELS")
        .unwrap_or_else(|_| "mxbai-embed-large,bge-m3,snowflake-arctic-embed2".into());
    for model in models.split(',').map(str::trim).filter(|m| !m.is_empty()) {
        let ai = Ai::new(
            AiConfig {
                embed_model: model.to_string(),
                ..Default::default()
            },
            AiRuntime::default(),
        );
        if ai.test_embed().await.is_err() {
            eprintln!("SKIP embedder {model} (not pulled?)");
            continue;
        }
        let slug: String = model
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let datasets = std::env::var("BEIR_AB_DATASETS").unwrap_or_else(|_| "scifact,fiqa".into());
        eprintln!("\n=== embedder: {model} ===");
        for name in datasets.split(',').map(str::trim).filter(|d| !d.is_empty()) {
            run_beir(name, &ai, None, EmbedStyle::default(), &slug).await;
        }
    }
}

#[tokio::test]
#[ignore = "live: rerank-engine comparison on SciFact — bonsai vs Apple FM vs codex"]
async fn beir_rerank_engines_scifact() {
    let Some(ai) = builtin_ai().await else { return };
    // BEIR_RERANK_ENGINE=codex narrows to one engine;
    // BEIR_RERANK_SAMPLE=50 shrinks the per-engine query sample — agent
    // CLIs cost tens of seconds per call.
    let only = std::env::var("BEIR_RERANK_ENGINE").unwrap_or_default();
    let engines: Vec<(&str, Option<Ai>)> = vec![
        ("bonsai-8b", rerank_ai().await),
        ("apple-fm", fm_ai()),
        ("codex", codex_ai()),
    ];
    for (label, rr) in engines {
        if !only.is_empty() && label != only {
            continue;
        }
        let Some(rr) = rr else { continue };
        let resolved = rr.chat_engine_id(crate::inference::Role::Chat);
        eprintln!("\n=== rerank engine: {label} (resolved: {resolved}) ===");
        // Unavailable tiers fall through to another engine silently — that
        // would measure the wrong model and label it wrong.
        if label == "apple-fm" && resolved != "foundation-models" {
            eprintln!("SKIP: FM did not resolve (Apple Intelligence unavailable?)");
            continue;
        }
        if label == "codex" && resolved != "codex" {
            eprintln!("SKIP: codex did not resolve");
            continue;
        }
        // BEIR_RERANK_STRATEGY=listwise-json|listwise-lite|pointwise narrows;
        // default races all three on local tiers. Codex runs listwise only —
        // it already aces that shape, and per-passage CLI spawns are absurd.
        let want = std::env::var("BEIR_RERANK_STRATEGY").unwrap_or_default();
        let strategies: &[RerankStrategy] = if label == "codex" {
            &[RerankStrategy::Listwise]
        } else {
            &[
                RerankStrategy::Listwise,
                RerankStrategy::ListwiseLite,
                RerankStrategy::Pointwise,
            ]
        };
        for s in strategies {
            if !want.is_empty() && s.label() != want {
                continue;
            }
            eprintln!("--- strategy: {} ---", s.label());
            run_beir(
                "scifact",
                &ai,
                Some((&rr, *s)),
                EmbedStyle::default(),
                "builtin",
            )
            .await;
        }
    }
}
