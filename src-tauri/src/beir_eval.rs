//! BEIR datasets against the real retrieval pipeline — public yardsticks
//! beside the synthetic evals in `retrieval_eval.rs`. Same chunker, same
//! built-in embedder, same hybrid search (vector + BM25, RRF-fused) the app
//! ships; scores are comparable to published BEIR numbers, so a chunking or
//! fusion regression shows up as a number the IR community understands.
//!
//! Three domains, three shapes: SciFact (scientific claims, lexical-
//! friendly, BM25's home turf), NFCorpus (medical, graded relevance, many
//! relevant docs per query), FiQA (financial Q&A, paraphrase-heavy — the
//! dense leg's chance to earn its keep).
//!
//! Run explicitly — each downloads its dataset once into `target/beir-cache`
//! and seeds through the built-in embedder (no Ollama):
//!
//!   cargo test --lib beir_ -- --ignored --nocapture

use std::collections::HashMap;

use crate::db::Db;
use crate::evals::builtin_ai;
use crate::ingest;

const BEIR_BASE: &str = "https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets";
/// Docs per embed+insert flush — a handful of Lance commits per corpus.
const SEED_BATCH: usize = 2_048;

/// The cached dataset dir, downloading and unpacking on first use. Lives in
/// target/ so `cargo clean` is the eviction policy.
async fn dataset_dir(name: &str) -> Option<std::path::PathBuf> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/beir-cache");
    let dir = cache.join(name);
    if dir.join("corpus.jsonl").exists() {
        return Some(dir);
    }
    std::fs::create_dir_all(&cache).ok()?;
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

struct BeirRun {
    ndcg: f64,
    recall: f64,
    docs: usize,
    queries: usize,
}

/// Seed a dataset's corpus through the real pipeline and score the shipping
/// hybrid search over its test-split qrels. Returns None when the network
/// (dataset or embedder download) isn't there — callers skip, not fail.
async fn run_beir(name: &str) -> Option<BeirRun> {
    let ai = builtin_ai().await?;
    let dir = dataset_dir(name).await.or_else(|| {
        eprintln!("SKIP: {name} download failed (network?)");
        None
    })?;

    // Corpus: {_id, title, text} per line, chunked exactly like an import.
    let corpus = jsonl(&dir.join("corpus.jsonl"));
    let tmp = std::env::temp_dir().join(format!("alchemy-beir-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&tmp).await.expect("open db");
    db.defer_fts(true);
    let mut rows: Vec<(String, String, i32, String)> = Vec::new();
    let mut inputs: Vec<String> = Vec::new();
    let mut seeded_docs = 0usize;
    for doc in &corpus {
        let id = doc["_id"].as_str().unwrap_or_default().to_string();
        let title = doc["title"].as_str().unwrap_or_default();
        let body = doc["text"].as_str().unwrap_or_default();
        if id.is_empty() || body.is_empty() {
            continue;
        }
        for (j, c) in ingest::chunk_text(title, body).into_iter().enumerate() {
            rows.push((id.clone(), format!("{id}-c{j}"), j as i32, c.text));
            inputs.push(c.embed_text);
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
    db.flush_fts().await.expect("flush fts");

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

    let (mut ndcg_sum, mut recall_sum, mut n) = (0.0f64, 0.0f64, 0usize);
    for (qid, rels) in &qrels {
        let Some(qtext) = queries.get(qid) else {
            continue;
        };
        let qvec = ai.embed_one(qtext).await.expect("embed query");
        // Over-fetch chunks, then collapse to documents in rank order —
        // several chunks of one document may outrank the next document.
        let hits = db
            .search_chunks(name, qvec, qtext, 20, None)
            .await
            .expect("search");
        let mut ranked: Vec<String> = Vec::new();
        for h in hits {
            if !ranked.contains(&h.source_id) {
                ranked.push(h.source_id.clone());
            }
            if ranked.len() == 10 {
                break;
            }
        }
        ndcg_sum += ndcg_at_k(&ranked, rels, 10);
        let found = ranked.iter().filter(|d| rels.contains_key(*d)).count();
        recall_sum += found as f64 / rels.len().min(10) as f64;
        n += 1;
    }
    let _ = std::fs::remove_dir_all(&tmp);
    let run = BeirRun {
        ndcg: ndcg_sum / n as f64,
        recall: recall_sum / n as f64,
        docs: seeded_docs,
        queries: n,
    };
    eprintln!(
        "\nBEIR {name} over {} docs, {} test queries (built-in embedder + BM25, RRF):\n  \
         nDCG@10   {:.4}\n  recall@10 {:.4}\n",
        run.docs, run.queries, run.ndcg, run.recall
    );
    Some(run)
}

// Floors sit a couple of points under each measured baseline — they catch
// regressions in chunking, fusion, or indexing, not model drift. Recalibrate
// deliberately when the embedder or chunker changes on purpose.

#[tokio::test]
#[ignore = "downloads BEIR SciFact and seeds ~5k docs — run with --ignored --nocapture"]
async fn beir_scifact_ndcg() {
    // Measured 2026-08-09: nDCG@10 0.6314, recall@10 0.7861 (published
    // BM25-only ≈ 0.665 — lexical-friendly claims are BM25's home turf).
    let Some(run) = run_beir("scifact").await else {
        return;
    };
    assert!(
        run.docs > 5_000 && run.queries > 250,
        "dataset shape changed"
    );
    assert!(run.ndcg > 0.60, "nDCG@10 regressed: {:.4}", run.ndcg);
    assert!(run.recall > 0.75, "recall@10 regressed: {:.4}", run.recall);
}

#[tokio::test]
#[ignore = "downloads BEIR NFCorpus and seeds ~3.6k docs — run with --ignored --nocapture"]
async fn beir_nfcorpus_ndcg() {
    let Some(run) = run_beir("nfcorpus").await else {
        return;
    };
    assert!(
        run.docs > 3_000 && run.queries > 300,
        "dataset shape changed"
    );
    assert!(run.ndcg > 0.25, "nDCG@10 regressed: {:.4}", run.ndcg);
    assert!(run.recall > 0.10, "recall@10 regressed: {:.4}", run.recall);
}

#[tokio::test]
#[ignore = "downloads BEIR FiQA (~57k docs) — run with --ignored --nocapture"]
async fn beir_fiqa_ndcg() {
    let Some(run) = run_beir("fiqa").await else {
        return;
    };
    assert!(
        run.docs > 50_000 && run.queries > 600,
        "dataset shape changed"
    );
    assert!(run.ndcg > 0.15, "nDCG@10 regressed: {:.4}", run.ndcg);
    assert!(run.recall > 0.15, "recall@10 regressed: {:.4}", run.recall);
}
