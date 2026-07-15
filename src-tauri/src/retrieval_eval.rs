//! Dataset-driven retrieval evals: ranked-relevance metrics (Recall@5/10,
//! MRR@10, MAP@10, nDCG@10) over JSON query datasets, comparing retrieval
//! variants through the same search paths the app uses. This is Phase 1 of
//! the retrieval maturity roadmap (docs/RFC-retrieval-maturity.md): make
//! quality measurable before touching ranking. No runtime behavior changes.
//!
//! Datasets live in `src-tauri/evals/datasets/*.json`. Each query names the
//! source(s) that answer it, optionally with a substring the winning chunk
//! must contain. A per-run JSON report is written to
//! `target/retrieval-eval-report.json`.
//!
//! Run with:  cargo test --lib retrieval_eval -- --nocapture

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::evals::{builtin_ai, seed_corpus};
use crate::models::Citation;

/// Retrieval depth for all metrics; recall is additionally reported at 5.
const K: usize = 10;

#[derive(Deserialize)]
struct Dataset {
    name: String,
    #[allow(dead_code)]
    description: String,
    queries: Vec<EvalQuery>,
}

#[derive(Deserialize)]
struct EvalQuery {
    query: String,
    /// Buckets the metrics: exact, paraphrase, section, metadata, multihop.
    kind: String,
    relevant: Vec<RelevantSpec>,
}

/// One distinct information need. A retrieved chunk satisfies it when the
/// chunk comes from the named source and (when set) its snippet contains
/// `must_contain`, case-insensitively. Only the first chunk to satisfy a spec
/// counts as relevant — duplicates from the same source are neutral — so the
/// judged list has exactly `relevant.len()` relevant items and the standard
/// metric formulas apply unmodified.
#[derive(Deserialize)]
struct RelevantSpec {
    source_title: String,
    #[serde(default)]
    must_contain: String,
}

#[derive(Serialize, Clone, Copy, Default)]
struct Metrics {
    recall_at_5: f64,
    recall_at_10: f64,
    mrr_at_10: f64,
    map_at_10: f64,
    ndcg_at_10: f64,
}

#[derive(Serialize)]
struct VariantReport {
    overall: Metrics,
    by_kind: HashMap<String, Metrics>,
}

#[derive(Serialize)]
struct DatasetReport {
    dataset: String,
    k: usize,
    queries: usize,
    variants: HashMap<String, VariantReport>,
}

fn datasets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("evals")
        .join("datasets")
}

fn load_datasets() -> Vec<Dataset> {
    let dir = datasets_dir();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| {
            let raw = std::fs::read_to_string(p).expect("read dataset");
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", p.display()))
        })
        .collect()
}

/// Ranks (1-based) at which each spec was first satisfied, in rank order.
/// Ranks are distinct: a chunk consumes at most one spec, a spec at most one
/// chunk.
fn matched_ranks(
    hits: &[Citation],
    titles: &HashMap<String, String>,
    specs: &[RelevantSpec],
) -> Vec<usize> {
    let mut open: Vec<bool> = vec![true; specs.len()];
    let mut ranks = Vec::new();
    for (idx, c) in hits.iter().enumerate() {
        let title = titles
            .get(&c.source_id)
            .map(String::as_str)
            .unwrap_or(&c.source_title);
        let snippet = c.snippet.to_lowercase();
        let found = specs.iter().enumerate().position(|(si, s)| {
            open[si]
                && s.source_title == title
                && (s.must_contain.is_empty() || snippet.contains(&s.must_contain.to_lowercase()))
        });
        if let Some(si) = found {
            open[si] = false;
            ranks.push(idx + 1);
        }
    }
    ranks
}

/// Standard binary-relevance metrics from the matched ranks, with
/// R = total_relevant as the recall/AP/IDCG denominator.
fn query_metrics(ranks: &[usize], total_relevant: usize) -> Metrics {
    let r = total_relevant.max(1) as f64;
    let in_k = |k: usize| ranks.iter().filter(|&&rk| rk <= k).count() as f64;
    let mrr = ranks
        .iter()
        .min()
        .filter(|&&rk| rk <= K)
        .map_or(0.0, |&rk| 1.0 / rk as f64);
    let mut hits = 0.0;
    let mut ap_sum = 0.0;
    let mut dcg = 0.0;
    for &rk in ranks.iter().filter(|&&rk| rk <= K) {
        hits += 1.0;
        ap_sum += hits / rk as f64;
        dcg += 1.0 / ((rk + 1) as f64).log2();
    }
    let idcg: f64 = (1..=total_relevant.min(K))
        .map(|i| 1.0 / ((i + 1) as f64).log2())
        .sum();
    Metrics {
        recall_at_5: in_k(5) / r,
        recall_at_10: in_k(K) / r,
        mrr_at_10: mrr,
        map_at_10: ap_sum / total_relevant.clamp(1, K) as f64,
        ndcg_at_10: if idcg > 0.0 { dcg / idcg } else { 0.0 },
    }
}

fn mean(rows: &[(String, Metrics)], kind: Option<&str>) -> Metrics {
    let sel: Vec<&Metrics> = rows
        .iter()
        .filter(|(k, _)| kind.is_none_or(|want| k == want))
        .map(|(_, m)| m)
        .collect();
    let n = sel.len().max(1) as f64;
    Metrics {
        recall_at_5: sel.iter().map(|m| m.recall_at_5).sum::<f64>() / n,
        recall_at_10: sel.iter().map(|m| m.recall_at_10).sum::<f64>() / n,
        mrr_at_10: sel.iter().map(|m| m.mrr_at_10).sum::<f64>() / n,
        map_at_10: sel.iter().map(|m| m.map_at_10).sum::<f64>() / n,
        ndcg_at_10: sel.iter().map(|m| m.ndcg_at_10).sum::<f64>() / n,
    }
}

/// Ranked-metric comparison of retrieval variants over the JSON datasets.
/// Variants all go through db.rs code paths: vector-only (empty query text
/// skips BM25), FTS-only, and the production hybrid RRF.
#[tokio::test]
async fn eval_retrieval_datasets() {
    let Some(ai) = builtin_ai().await else { return };
    let datasets = load_datasets();
    assert!(!datasets.is_empty(), "no datasets in evals/datasets/");

    let dir = std::env::temp_dir().join(format!("nbl-eval-ds-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let nb = "eval-nb";
    seed_corpus(&ai, &db, nb).await;
    let titles: HashMap<String, String> = db
        .list_sources(nb)
        .await
        .expect("list sources")
        .into_iter()
        .map(|s| (s.id, s.title))
        .collect();

    const VARIANTS: [&str; 3] = ["vector", "fts", "hybrid"];
    let mut reports = Vec::new();
    for ds in &datasets {
        // (kind, metrics) per query, per variant.
        let mut rows: HashMap<&str, Vec<(String, Metrics)>> = HashMap::new();
        for q in &ds.queries {
            let qvec = ai.embed_one(&q.query).await.expect("embed question");
            for variant in VARIANTS {
                let hits: Vec<Citation> = match variant {
                    "vector" => db
                        .search_chunks(nb, qvec.clone(), "", K, None)
                        .await
                        .expect("vector search"),
                    "fts" => db
                        .search_chunks_fts_all(&q.query, K)
                        .await
                        .expect("fts search")
                        .into_iter()
                        .map(|(_, c)| c)
                        .collect(),
                    _ => db
                        .search_chunks(nb, qvec.clone(), &q.query, K, None)
                        .await
                        .expect("hybrid search"),
                };
                let ranks = matched_ranks(&hits, &titles, &q.relevant);
                rows.entry(variant)
                    .or_default()
                    .push((q.kind.clone(), query_metrics(&ranks, q.relevant.len())));
            }
        }

        let mut kinds: Vec<String> = ds.queries.iter().map(|q| q.kind.clone()).collect();
        kinds.sort();
        kinds.dedup();

        eprintln!(
            "\ndataset {} ({} queries), @{K}:",
            ds.name,
            ds.queries.len()
        );
        eprintln!(
            "  {:<10} {:<11} {:>4} {:>4} {:>5} {:>5} {:>5}",
            "variant", "kind", "R@5", "R@10", "MRR", "MAP", "nDCG"
        );
        let mut variants = HashMap::new();
        for variant in VARIANTS {
            let rows = &rows[variant];
            for kind in kinds.iter().map(Some).chain([None]) {
                let m = mean(rows, kind.map(String::as_str));
                eprintln!(
                    "  {:<10} {:<11} {:>4.2} {:>4.2} {:>5.2} {:>5.2} {:>5.2}",
                    variant,
                    kind.map(String::as_str).unwrap_or("overall"),
                    m.recall_at_5,
                    m.recall_at_10,
                    m.mrr_at_10,
                    m.map_at_10,
                    m.ndcg_at_10
                );
            }
            variants.insert(
                variant.to_string(),
                VariantReport {
                    overall: mean(rows, None),
                    by_kind: kinds
                        .iter()
                        .map(|k| (k.clone(), mean(rows, Some(k))))
                        .collect(),
                },
            );
        }
        reports.push(DatasetReport {
            dataset: ds.name.clone(),
            k: K,
            queries: ds.queries.len(),
            variants,
        });
    }

    let report_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("retrieval-eval-report.json");
    std::fs::write(
        &report_path,
        serde_json::to_string_pretty(&reports).expect("serialize report"),
    )
    .expect("write report");
    eprintln!("\nreport: {}\n", report_path.display());

    // Floors, mirroring evals.rs: hybrid must never lag the vector-only
    // baseline, must nail exact identifiers, and must keep overall recall
    // high. The built-in embedder and BM25 are deterministic for fixed
    // inputs, so a failure is a retrieval regression, not flakiness.
    for r in &reports {
        let hybrid = &r.variants["hybrid"];
        let vector = &r.variants["vector"];
        assert!(
            hybrid.overall.recall_at_10 >= vector.overall.recall_at_10,
            "{}: hybrid recall@10 ({:.2}) fell below vector-only ({:.2})",
            r.dataset,
            hybrid.overall.recall_at_10,
            vector.overall.recall_at_10
        );
        if let Some(exact) = hybrid.by_kind.get("exact") {
            assert!(
                (exact.recall_at_10 - 1.0).abs() < f64::EPSILON,
                "{}: hybrid missed an exact-identifier query (recall@10 {:.2})",
                r.dataset,
                exact.recall_at_10
            );
        }
        assert!(
            hybrid.overall.recall_at_10 >= 0.8,
            "{}: overall hybrid recall@10 {:.2} below 0.8 floor",
            r.dataset,
            hybrid.overall.recall_at_10
        );
    }
}
