//! BEIR SciFact against the real retrieval pipeline — a public yardstick
//! beside the synthetic evals in `retrieval_eval.rs`. Same chunker, same
//! built-in embedder, same hybrid search (vector + BM25, RRF-fused) the app
//! ships; the score is comparable to published BEIR numbers, so a chunking
//! or fusion regression shows up as a number the IR community understands.
//!
//! Run explicitly — it downloads the ~3 MB dataset once into
//! `target/beir-cache` and seeds ~5k documents (built-in embedder, no
//! Ollama):
//!
//!   cargo test --lib beir_scifact -- --ignored --nocapture

use std::collections::{HashMap, HashSet};

use crate::db::Db;
use crate::evals::builtin_ai;
use crate::ingest;

const SCIFACT_URL: &str =
    "https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/scifact.zip";
/// Docs per embed+insert batch: ~25 embed calls and a handful of Lance
/// commits for the whole corpus.
const SEED_BATCH: usize = 512;
const NOTEBOOK: &str = "beir-scifact";

/// The cached dataset dir, downloading and unpacking on first use. Lives in
/// target/ so `cargo clean` is the eviction policy.
async fn scifact_dir() -> Option<std::path::PathBuf> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/beir-cache");
    let dir = cache.join("scifact");
    if dir.join("corpus.jsonl").exists() {
        return Some(dir);
    }
    std::fs::create_dir_all(&cache).ok()?;
    eprintln!("beir: downloading SciFact ({SCIFACT_URL})…");
    let bytes = ingest::fetch_bytes(SCIFACT_URL, 50 * 1024 * 1024).await?;
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

/// Binary-gain nDCG@k over a ranked doc list.
fn ndcg_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    let dcg: f64 = ranked
        .iter()
        .take(k)
        .enumerate()
        .filter(|(_, d)| relevant.contains(*d))
        .map(|(i, _)| 1.0 / (i as f64 + 2.0).log2())
        .sum();
    let ideal: f64 = (0..relevant.len().min(k))
        .map(|i| 1.0 / (i as f64 + 2.0).log2())
        .sum();
    if ideal == 0.0 {
        0.0
    } else {
        dcg / ideal
    }
}

#[tokio::test]
#[ignore = "downloads BEIR SciFact and seeds ~5k docs — run with --ignored --nocapture"]
async fn beir_scifact_ndcg() {
    let Some(ai) = builtin_ai().await else { return };
    let Some(dir) = scifact_dir().await else {
        eprintln!("SKIP: SciFact download failed (network?)");
        return;
    };

    // Corpus: {_id, title, text} per line, chunked exactly like an import.
    let corpus = jsonl(&dir.join("corpus.jsonl"));
    assert!(
        corpus.len() > 5_000,
        "unexpected corpus size {}",
        corpus.len()
    );
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
        if rows.len() >= SEED_BATCH * 4 {
            let embeddings = ai.embed(&inputs).await.expect("embed corpus batch");
            db.add_chunk_rows(NOTEBOOK, &rows, &embeddings)
                .await
                .expect("seed chunk rows");
            rows.clear();
            inputs.clear();
            eprintln!("beir: seeded {seeded_docs}/{} docs", corpus.len());
        }
    }
    if !rows.is_empty() {
        let embeddings = ai.embed(&inputs).await.expect("embed corpus tail");
        db.add_chunk_rows(NOTEBOOK, &rows, &embeddings)
            .await
            .expect("seed chunk tail");
    }
    db.defer_fts(false);
    db.flush_fts().await.expect("flush scifact fts");

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
    let mut qrels: HashMap<String, HashSet<String>> = HashMap::new();
    for line in std::fs::read_to_string(dir.join("qrels/test.tsv"))
        .expect("qrels")
        .lines()
        .skip(1)
    {
        let mut f = line.split('\t');
        let (Some(qid), Some(did), Some(score)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if score.trim().parse::<i32>().unwrap_or(0) > 0 {
            qrels.entry(qid.to_string()).or_default().insert(did.into());
        }
    }
    assert!(qrels.len() > 250, "unexpected qrels size {}", qrels.len());

    let (mut ndcg_sum, mut recall_sum, mut n) = (0.0f64, 0.0f64, 0usize);
    for (qid, relevant) in &qrels {
        let Some(qtext) = queries.get(qid) else {
            continue;
        };
        let qvec = ai.embed_one(qtext).await.expect("embed query");
        // Over-fetch chunks, then collapse to documents in rank order —
        // several chunks of one paper may outrank the next paper.
        let hits = db
            .search_chunks(NOTEBOOK, qvec, qtext, 20, None)
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
        ndcg_sum += ndcg_at_k(&ranked, relevant, 10);
        let found = ranked.iter().filter(|d| relevant.contains(*d)).count();
        recall_sum += found as f64 / relevant.len().min(10) as f64;
        n += 1;
    }
    let ndcg = ndcg_sum / n as f64;
    let recall = recall_sum / n as f64;
    eprintln!(
        "\nBEIR SciFact over {} docs, {n} test queries (built-in embedder + BM25, RRF):\n  \
         nDCG@10  {ndcg:.4}\n  recall@10 {recall:.4}\n",
        seeded_docs
    );
    let _ = std::fs::remove_dir_all(&tmp);

    // Floors sit a couple of points under the measured run (nDCG@10 0.6314,
    // recall@10 0.7861 on 2026-08-09; published BM25 ≈ 0.665) — they catch
    // regressions in chunking, fusion, or indexing, not model drift.
    assert!(ndcg > 0.60, "nDCG@10 regressed: {ndcg:.4}");
    assert!(recall > 0.75, "recall@10 regressed: {recall:.4}");
}
