//! Dataset-driven retrieval evals: ranked-relevance metrics (Recall@5/10,
//! MRR@10, MAP@10, nDCG@10) over JSON query datasets, comparing retrieval
//! variants through the same search paths the app uses. This is Phase 1 of
//! the retrieval maturity roadmap (docs/RFC-retrieval-maturity.md): make
//! quality measurable before touching ranking.
//!
//! Datasets live in `src-tauri/evals/datasets/*.json`. Each query names the
//! source(s) that answer it, optionally with a substring the winning chunk
//! must contain. A per-run JSON report is written to
//! `target/retrieval-eval-report.json`.
//!
//! Run with:  cargo test --lib retrieval_eval -- --nocapture

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::evals::{builtin_ai, seed_corpus, seed_docs};
use crate::models::Citation;

/// Retrieval depth for all metrics; recall is additionally reported at 5.
const K: usize = 10;

/// Extra distractors seeded only for the dataset evals, on top of the golden
/// corpus in evals.rs (which its own test keeps unchanged). Each one crowds a
/// golden or hard query — look-alike identifiers, near-topic prose, competing
/// numbers — so top-k is contested and recall/ranking metrics can actually
/// move.
const EXTRA_CORPUS: &[(&str, &str)] = &[
    (
        "Acme Invoices Q1 (archive)",
        "# Sheet: Closed\n\
         invoice | customer | amount | status\n\
         INV-2023-0087 | Acme Corp | $11,200 | paid\n\
         INV-2023-0091 | Globex | $6,050 | paid\n\
         INV-2023-0095 | Initech | $2,750 | paid\n\n\
         # Sheet: Notes\n\
         The original retry policy used ERR-500-RETRY with a ten second wait \
         before the next attempt. Superseded in Q2.",
    ),
    (
        "Vendor Payment Runbook",
        "# Wires\n\nVendor invoices are paid by wire on net-forty-five terms. \
         Remittance advice goes out the same day.\n\n\
         # Disputes\n\nDisputed vendor invoices are escalated to procurement \
         within five business days.",
    ),
    (
        "Cabin WiFi Setup",
        "# Router\n\nThe cabin router sits above the wood stove shelf. The guest \
         passphrase is taped inside the pantry door.\n\n\
         # Port Forwarding\n\nPort 8080 forwards to the trail camera system. \
         Everything else stays closed.",
    ),
    (
        "Tokyo Business Trip",
        "Three nights at a hotel in Shinjuku for the conference. Breakfast at the \
         station, late ramen most evenings, and one free afternoon in the Meiji \
         shrine gardens before the flight home.",
    ),
    (
        "Focaccia Experiments",
        "Overnight cold proof in the fridge, then two hours at room temperature. \
         Bake at four hundred twenty five degrees on a sheet pan with plenty of \
         olive oil. Flaky salt goes on after the oven, not before.",
    ),
    (
        "Benefits FAQ",
        "# Holidays\n\nThe company observes eleven paid holidays per year.\n\n\
         # Sick Days\n\nSick time is unlimited within reason and does not draw \
         from vacation balances.\n\n\
         # Parental Leave\n\nSixteen weeks paid, available after six months of \
         service.",
    ),
    (
        "Media Server Maintenance",
        "# Library Scans\n\nThe media server scans its library nightly at 3am. \
         Transcoding is capped at two simultaneous streams.\n\n\
         # Restarts\n\nThe server container restarts on the first Sunday of the \
         month after backups complete.",
    ),
    (
        "Home Insurance Policy",
        "# Deductibles\n\nThe homeowner's policy deductible is two thousand five \
         hundred dollars for wind and hail damage, and five hundred dollars for \
         theft.\n\n\
         # Claims\n\nClaims are filed through the agent portal within sixty days \
         of the loss.",
    ),
    (
        "VPN Access Guide",
        "# WireGuard\n\nThe VPN uses WireGuard listening on port 51820. Peer \
         configs are issued per device.\n\n\
         # SSH\n\nShell access to home machines goes over the VPN only; nothing \
         listens on the public interface.",
    ),
];

/// The JSON `description` field is for humans reading the dataset file;
/// serde ignores it.
#[derive(Deserialize)]
struct Dataset {
    name: String,
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

// BTreeMaps so the JSON report serializes with stable key order — the file
// exists to be diffed across runs.
#[derive(Serialize)]
struct VariantReport {
    overall: Metrics,
    by_kind: BTreeMap<String, Metrics>,
}

#[derive(Serialize)]
struct DatasetReport {
    dataset: String,
    k: usize,
    queries: usize,
    variants: BTreeMap<String, VariantReport>,
}

fn load_datasets() -> Vec<Dataset> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("evals")
        .join("datasets");
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

/// Ranks (1-based) at which each spec was first satisfied, in rank order,
/// plus the indices of specs nothing satisfied. Ranks are distinct: a chunk
/// consumes at most one spec, a spec at most one chunk. Among open specs a
/// chunk satisfies, the most specific (longest `must_contain`) wins, so a
/// bare source spec can't consume the one chunk a constrained sibling needs.
fn matched_ranks(
    hits: &[Citation],
    titles: &HashMap<String, String>,
    specs: &[RelevantSpec],
) -> (Vec<usize>, Vec<usize>) {
    let needles: Vec<String> = specs
        .iter()
        .map(|s| s.must_contain.to_lowercase())
        .collect();
    let mut open: Vec<bool> = vec![true; specs.len()];
    let mut ranks = Vec::new();
    for (idx, c) in hits.iter().enumerate() {
        let title = titles
            .get(&c.source_id)
            .map(String::as_str)
            .unwrap_or(&c.source_title);
        let snippet = c.snippet.to_lowercase();
        let found = specs
            .iter()
            .enumerate()
            .filter(|(si, s)| {
                open[*si]
                    && s.source_title == title
                    && (needles[*si].is_empty() || snippet.contains(&needles[*si]))
            })
            .max_by_key(|(si, _)| needles[*si].len())
            .map(|(si, _)| si);
        if let Some(si) = found {
            open[si] = false;
            ranks.push(idx + 1);
        }
    }
    let unmatched = (0..specs.len()).filter(|&si| open[si]).collect();
    (ranks, unmatched)
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
    seed_docs(&ai, &db, nb, EXTRA_CORPUS, "x-").await;
    let titles: HashMap<String, String> = db
        .list_sources(nb)
        .await
        .expect("list sources")
        .into_iter()
        .map(|s| (s.id, s.title))
        .collect();
    let title_set: std::collections::HashSet<&str> = titles.values().map(String::as_str).collect();

    const VARIANTS: [&str; 3] = ["vector", "fts", "hybrid"];
    let mut reports = Vec::new();
    for ds in &datasets {
        // Fail loudly on dataset authoring errors: an empty query list or a
        // spec naming a source that was never seeded would otherwise score
        // zero forever while the failure message blames retrieval.
        assert!(!ds.queries.is_empty(), "dataset {} has no queries", ds.name);
        for q in &ds.queries {
            for s in &q.relevant {
                assert!(
                    title_set.contains(s.source_title.as_str()),
                    "dataset {}: query {:?} names unknown source_title {:?}",
                    ds.name,
                    q.query,
                    s.source_title
                );
            }
        }

        // One embedder call per dataset instead of one per query.
        let query_texts: Vec<String> = ds.queries.iter().map(|q| q.query.clone()).collect();
        let qvecs = ai.embed(&query_texts).await.expect("embed queries");

        // (kind, metrics) per query, indexed parallel to VARIANTS.
        let mut rows: [Vec<(String, Metrics)>; 3] = Default::default();
        let mut misses: Vec<String> = Vec::new();
        for (q, qvec) in ds.queries.iter().zip(&qvecs) {
            let vector = db
                .search_chunks(nb, qvec.clone(), "", K, None)
                .await
                .expect("vector search");
            // There is no per-notebook FTS-only entry point yet (Phase 2's
            // SearchTrace adds one); filter the corpus-wide results to this
            // notebook so all three variants score the same population.
            let fts: Vec<Citation> = db
                .search_chunks_fts_all(&q.query, K)
                .await
                .expect("fts search")
                .into_iter()
                .filter(|(n, _)| n == nb)
                .map(|(_, c)| c)
                .collect();
            let hybrid = db
                .search_chunks(nb, qvec.clone(), &q.query, K, None)
                .await
                .expect("hybrid search");
            for (vi, hits) in [vector, fts, hybrid].into_iter().enumerate() {
                let (ranks, unmatched) = matched_ranks(&hits, &titles, &q.relevant);
                for si in unmatched {
                    let s = &q.relevant[si];
                    misses.push(format!(
                        "{:<7} [{}] {} — wanted {}{}",
                        VARIANTS[vi],
                        q.kind,
                        q.query,
                        s.source_title,
                        if s.must_contain.is_empty() {
                            String::new()
                        } else {
                            format!(" containing {:?}", s.must_contain)
                        }
                    ));
                }
                rows[vi].push((q.kind.clone(), query_metrics(&ranks, q.relevant.len())));
            }
        }

        let mut kinds: Vec<String> = ds.queries.iter().map(|q| q.kind.clone()).collect();
        kinds.sort();
        kinds.dedup();

        // Build the report first, then print from it, so the console table
        // and the JSON file can't drift apart.
        let variants: BTreeMap<String, VariantReport> = VARIANTS
            .iter()
            .enumerate()
            .map(|(vi, name)| {
                (
                    name.to_string(),
                    VariantReport {
                        overall: mean(&rows[vi], None),
                        by_kind: kinds
                            .iter()
                            .map(|k| (k.clone(), mean(&rows[vi], Some(k))))
                            .collect(),
                    },
                )
            })
            .collect();

        eprintln!(
            "\ndataset {} ({} queries), @{K}:",
            ds.name,
            ds.queries.len()
        );
        eprintln!(
            "  {:<10} {:<11} {:>4} {:>4} {:>5} {:>5} {:>5}",
            "variant", "kind", "R@5", "R@10", "MRR", "MAP", "nDCG"
        );
        for name in VARIANTS {
            let vr = &variants[name];
            let line = |label: &str, m: &Metrics| {
                eprintln!(
                    "  {:<10} {:<11} {:>4.2} {:>4.2} {:>5.2} {:>5.2} {:>5.2}",
                    name,
                    label,
                    m.recall_at_5,
                    m.recall_at_10,
                    m.mrr_at_10,
                    m.map_at_10,
                    m.ndcg_at_10
                );
            };
            for kind in &kinds {
                line(kind, &vr.by_kind[kind]);
            }
            line("overall", &vr.overall);
        }
        for m in &misses {
            eprintln!("  MISS (@{K}): {m}");
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
    std::fs::create_dir_all(report_path.parent().expect("report path has parent"))
        .expect("create report dir");
    std::fs::write(
        &report_path,
        serde_json::to_string_pretty(&reports).expect("serialize report"),
    )
    .expect("write report");
    eprintln!("\nreport: {}\n", report_path.display());

    // Floors, mirroring evals.rs: hybrid must never lag the vector-only
    // baseline, must nail exact identifiers, and must keep overall recall
    // high. Ranked floors guard what recall can't on this small corpus: a
    // regression that keeps relevant chunks in the top-10 but demotes them.
    // The built-in embedder and BM25 are deterministic for fixed inputs, and
    // RRF fusion tie-breaks on chunk id, so a failure is a retrieval
    // regression, not flakiness.
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
        assert!(
            hybrid.overall.mrr_at_10 >= 0.75,
            "{}: overall hybrid MRR@10 {:.2} below 0.75 floor",
            r.dataset,
            hybrid.overall.mrr_at_10
        );
        assert!(
            hybrid.overall.ndcg_at_10 >= 0.8,
            "{}: overall hybrid nDCG@10 {:.2} below 0.8 floor",
            r.dataset,
            hybrid.overall.ndcg_at_10
        );
    }
}
