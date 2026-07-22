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

use crate::db::{Db, SearchOptions};
use crate::evals::{builtin_ai, seed_corpus, seed_docs, CORPUS};
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

    // "meta" is the corpus-wide path (search_chunks_all_opts with the
    // production meta-chat caps) — the surface gists and cross-notebook
    // questions ride. Here it runs over the same single-notebook corpus, so
    // its numbers are directly comparable to the per-notebook variants.
    const VARIANTS: [&str; 4] = ["vector", "fts", "hybrid", "meta"];
    let meta_opts = SearchOptions {
        pool_multiplier: 4,
        max_per_source: 2,
        max_per_notebook: 3,
        max_notes: 4,
        max_gists: 2,
    };
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
        let mut rows: [Vec<(String, Metrics)>; 4] = Default::default();
        let mut misses: Vec<String> = Vec::new();
        for (q, qvec) in ds.queries.iter().zip(&qvecs) {
            let vector = db
                .search_chunks(nb, qvec.clone(), "", K, None)
                .await
                .expect("vector search");
            // One traced search yields both the FTS-only leg (in-notebook,
            // same population and pool as the other variants) and the
            // production hybrid result.
            let trace = db
                .search_chunks_trace(nb, qvec.clone(), &q.query, K, None)
                .await
                .expect("hybrid search");
            assert!(
                trace.warnings.is_empty(),
                "search degraded: {:?}",
                trace.warnings
            );
            let fts: Vec<Citation> = trace.fts_hits.into_iter().take(K).collect();
            let hybrid = trace.final_hits;
            let meta: Vec<Citation> = db
                .search_chunks_all_opts(qvec.clone(), &q.query, K, None, meta_opts)
                .await
                .expect("meta search")
                .into_iter()
                .map(|(_, c)| c)
                .collect();
            for (vi, hits) in [vector, fts, hybrid, meta].into_iter().enumerate() {
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

/// A chatty near-topic source: twelve heading-separated entries about home
/// networking that embed close to network queries but answer none of them.
/// Seeded as its own notebook so flat corpus-wide search lets it crowd the
/// top-k with near-duplicates — the failure mode diversity caps exist for.
const DOMINATOR: &[(&str, &str)] = &[(
    "Network Tinkering Log",
    "# Entry 1\n\nMoved the mesh node off the bookshelf; wifi bars looked the \
     same in the kitchen but the speed test felt snappier.\n\n\
     # Entry 2\n\nRebooted the router twice this week. Coverage in the garage \
     is still spotty; might need another access point.\n\n\
     # Entry 3\n\nRenamed the network SSIDs so the 2.4GHz and 5GHz bands are \
     easier to tell apart when connecting new devices.\n\n\
     # Entry 4\n\nLooked at the port forwarding table and cleaned out two \
     stale entries from the old console. Everything else untouched.\n\n\
     # Entry 5\n\nThe smart plugs kept dropping off wifi; pinned them to the \
     2.4GHz band and they have been stable since.\n\n\
     # Entry 6\n\nTested wifi throughput near the office window: solid \
     downstream, weaker upstream. Router placement experiment pending.\n\n\
     # Entry 7\n\nSwapped the ethernet cable on the desk switch for a shorter \
     one and tidied the cable run behind the monitor.\n\n\
     # Entry 8\n\nChecked the router admin page for firmware notes; nothing \
     new this month, so no update applied.\n\n\
     # Entry 9\n\nGuest devices seemed slow at the party; probably just \
     too many phones on the band at once, not a config issue.\n\n\
     # Entry 10\n\nMapped which wall jacks are live back to the patch panel \
     and labeled them with tape.\n\n\
     # Entry 11\n\nThe printer fell off the network again; static DHCP \
     reservation added so it stops wandering.\n\n\
     # Entry 12\n\nBrief outage upstream around noon; the ISP status page \
     confirmed it, nothing local to fix.",
)];

/// One breadth query for the diversity eval: relevance spans sources in
/// several notebooks while the dominator crowds the same topic.
struct MetaGolden {
    query: &'static str,
    specs: &'static [(&'static str, &'static str)], // (source_title, must_contain)
}

const META_GOLDEN: &[MetaGolden] = &[
    MetaGolden {
        query: "what do I have set up for wifi networks and port forwarding?",
        specs: &[
            ("Home Network Guide", "32400"),
            ("Office Network Runbook", "8443"),
            ("Cabin WiFi Setup", "8080"),
            ("VPN Access Guide", "51820"),
        ],
    },
    MetaGolden {
        query: "how has the invoice retry error policy changed over time?",
        specs: &[
            ("Acme Invoices Q3", "ERR-503-BACKOFF"),
            ("Acme Invoices Q2 (archive)", "ERR-429-THROTTLE"),
            ("Acme Invoices Q1 (archive)", "ERR-500-RETRY"),
        ],
    },
    MetaGolden {
        query: "what oven temperatures and proofing times do my baking notes use?",
        specs: &[
            ("Sourdough Notes", "four hundred fifty"),
            ("Focaccia Experiments", "four hundred twenty five"),
        ],
    },
];

/// Meta-chat (corpus-wide) eval: flat search vs diversity caps over a corpus
/// where one chatty notebook crowds the topic. Reports spec recall plus
/// distinct sources/notebooks in the top-k.
#[tokio::test]
async fn eval_meta_diversity() {
    let Some(ai) = builtin_ai().await else { return };
    let dir = std::env::temp_dir().join(format!("nbl-eval-meta-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    seed_docs(&ai, &db, "meta-a", DOMINATOR, "dom-").await;
    seed_docs(&ai, &db, "meta-b", CORPUS, "b-").await;
    seed_docs(&ai, &db, "meta-c", EXTRA_CORPUS, "c-").await;

    let mut titles: HashMap<String, String> = HashMap::new();
    for nb in ["meta-a", "meta-b", "meta-c"] {
        for s in db.list_sources(nb).await.expect("list sources") {
            titles.insert(s.id, s.title);
        }
    }

    const K: usize = 8;
    let diverse_opts = SearchOptions {
        pool_multiplier: 4,
        max_per_source: 2,
        max_per_notebook: 3,
        max_notes: 4,
        max_gists: 2,
    };

    // Per query: (recall, distinct sources, distinct notebooks) per variant.
    let mut flat_stats: Vec<(f64, usize, usize)> = Vec::new();
    let mut diverse_stats: Vec<(f64, usize, usize)> = Vec::new();
    eprintln!("\nmeta diversity @{K} (recall / distinct sources / distinct notebooks):");
    for g in META_GOLDEN {
        let qvec = ai.embed_one(g.query).await.expect("embed query");
        let specs: Vec<RelevantSpec> = g
            .specs
            .iter()
            .map(|(t, m)| RelevantSpec {
                source_title: t.to_string(),
                must_contain: m.to_string(),
            })
            .collect();
        let run = |name: &'static str, hits: Vec<(String, Citation)>| {
            let nbs: std::collections::HashSet<&String> = hits.iter().map(|(nb, _)| nb).collect();
            let owners: std::collections::HashSet<&str> = hits
                .iter()
                .map(|(_, c)| {
                    if c.note_id.is_empty() {
                        c.source_id.as_str()
                    } else {
                        c.note_id.as_str()
                    }
                })
                .collect();
            let cs: Vec<Citation> = hits.iter().map(|(_, c)| c.clone()).collect();
            let (ranks, _) = matched_ranks(&cs, &titles, &specs);
            let recall = ranks.len() as f64 / specs.len() as f64;
            eprintln!(
                "  {:<8} {:<58} {:.2} / {} / {}",
                name,
                g.query.chars().take(58).collect::<String>(),
                recall,
                owners.len(),
                nbs.len()
            );
            (recall, owners.len(), nbs.len())
        };
        let flat = db
            .search_chunks_all_opts(qvec.clone(), g.query, K, None, SearchOptions::default())
            .await
            .expect("flat search");
        flat_stats.push(run("flat", flat));
        let diverse = db
            .search_chunks_all_opts(qvec, g.query, K, None, diverse_opts)
            .await
            .expect("diverse search");
        diverse_stats.push(run("diverse", diverse));
    }

    let mean_of = |v: &[(f64, usize, usize)]| {
        let n = v.len() as f64;
        (
            v.iter().map(|s| s.0).sum::<f64>() / n,
            v.iter().map(|s| s.1 as f64).sum::<f64>() / n,
            v.iter().map(|s| s.2 as f64).sum::<f64>() / n,
        )
    };
    let (fr, fs, fn_) = mean_of(&flat_stats);
    let (dr, ds, dn) = mean_of(&diverse_stats);
    eprintln!("  flat    mean: recall {fr:.2}, sources {fs:.1}, notebooks {fn_:.1}");
    eprintln!("  diverse mean: recall {dr:.2}, sources {ds:.1}, notebooks {dn:.1}\n");

    assert!(
        dr >= fr,
        "diversity caps reduced mean spec recall ({dr:.2} < {fr:.2})"
    );
    assert!(
        ds >= fs,
        "diversity caps reduced mean distinct sources ({ds:.1} < {fs:.1})"
    );
    assert!(
        dr >= 0.75,
        "diverse mean spec recall {dr:.2} below 0.75 floor"
    );
}

/// Deep-search eval. Deterministic part: how much recall headroom a wide
/// pool gives a reranker (recall@4 in fusion order vs recall within the
/// 16-candidate pool — the ceiling a perfect reranker reaches). Ollama-gated
/// part: run the real rerank over the pool and score it against fusion
/// order at the same cutoff.
#[tokio::test]
async fn eval_deep_rerank() {
    let Some(ai) = builtin_ai().await else { return };
    let dir = std::env::temp_dir().join(format!("nbl-eval-deep-{}", uuid::Uuid::new_v4()));
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

    const KEEP: usize = 4;
    const POOL: usize = 16;
    let chat = crate::ai::Ai::new(
        crate::ai::AiConfig {
            chat_model: "digitsflow/bonsai-8b:latest".into(),
            ..Default::default()
        },
        crate::ai::AiRuntime::default(),
    );
    let ollama_up = chat.list_models().await.is_ok();
    if !ollama_up {
        eprintln!("NOTE: Ollama not reachable — reporting deterministic headroom only");
    }

    let mut n = 0usize;
    let mut fusion_sum = 0.0f64;
    let mut ceiling_sum = 0.0f64;
    let mut rerank_sum = 0.0f64;
    for ds in &load_datasets() {
        for q in &ds.queries {
            let qvec = ai.embed_one(&q.query).await.expect("embed query");
            let pool = db
                .search_chunks(nb, qvec, &q.query, POOL, None)
                .await
                .expect("pool search");
            let recall_at = |hits: &[Citation], k: usize| {
                let (ranks, _) = matched_ranks(&hits[..hits.len().min(k)], &titles, &q.relevant);
                ranks.len() as f64 / q.relevant.len() as f64
            };
            n += 1;
            fusion_sum += recall_at(&pool, KEEP);
            ceiling_sum += recall_at(&pool, POOL);
            if ollama_up {
                let snippets: Vec<(String, String)> = pool
                    .iter()
                    .map(|c| {
                        (
                            c.source_title.clone(),
                            c.snippet.chars().take(300).collect(),
                        )
                    })
                    .collect();
                let reranked: Vec<Citation> =
                    match crate::agent::rerank_indices(&chat, &q.query, &snippets, KEEP).await {
                        Some(picked) => picked.into_iter().map(|i| pool[i].clone()).collect(),
                        None => pool.iter().take(KEEP).cloned().collect(),
                    };
                rerank_sum += recall_at(&reranked, KEEP);
            }
        }
    }
    let fusion = fusion_sum / n as f64;
    let ceiling = ceiling_sum / n as f64;
    eprintln!("\ndeep search over {n} queries (keep {KEEP} of pool {POOL}):");
    eprintln!("  fusion-order recall@{KEEP}   {fusion:.2}");
    eprintln!(
        "  pool ceiling recall@{POOL}   {ceiling:.2}   (perfect-reranker headroom {:+.2})",
        ceiling - fusion
    );
    if ollama_up {
        let rerank = rerank_sum / n as f64;
        eprintln!(
            "  LLM rerank   recall@{KEEP}   {rerank:.2}   ({:+.2} vs fusion)\n",
            rerank - fusion
        );
        assert!(
            rerank >= fusion - 0.1,
            "rerank recall@{KEEP} {rerank:.2} fell more than 0.1 below fusion {fusion:.2}"
        );
    } else {
        eprintln!();
    }
    assert!(
        ceiling >= fusion,
        "pool ceiling {ceiling:.2} below fusion recall {fusion:.2} — impossible unless judging broke"
    );
}

/// Semantic-router eval: build the notebook router over the three-notebook
/// corpus, check the index is incremental (second sync is a no-op), measure
/// routing accuracy on every dataset query, and compare routed retrieval
/// (top-2 notebooks) against flat.
#[tokio::test]
async fn eval_router() {
    let Some(ai) = builtin_ai().await else { return };
    let dir = std::env::temp_dir().join(format!("nbl-eval-router-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let notebooks = [
        ("meta-a", "Network Tinkering", DOMINATOR, "dom-"),
        ("meta-b", "Personal Reference", CORPUS, "b-"),
        ("meta-c", "Household Archive", EXTRA_CORPUS, "c-"),
    ];
    for (id, title, docs, prefix) in notebooks {
        db.create_notebook(&crate::models::Notebook {
            id: id.into(),
            title: title.into(),
            created_at: 0,
            updated_at: 0,
            color: String::new(),
            source_count: 0,
        })
        .await
        .expect("create notebook");
        seed_docs(&ai, &db, id, docs, prefix).await;
    }

    // Index builds once (one route per source), then syncs are no-ops until
    // the corpus changes.
    let n_sources = DOMINATOR.len() + CORPUS.len() + EXTRA_CORPUS.len();
    let (embedded, deleted) = crate::router::ensure_router(&db, &ai)
        .await
        .expect("build router");
    assert_eq!(
        (embedded, deleted),
        (n_sources, 0),
        "expected one fresh route per source"
    );
    let resync = crate::router::ensure_router(&db, &ai)
        .await
        .expect("resync router");
    assert_eq!(resync, (0, 0), "unchanged corpus must re-embed nothing");

    // Source title -> owning notebook, for judging routing accuracy.
    let mut titles: HashMap<String, String> = HashMap::new();
    let mut source_nb: HashMap<String, String> = HashMap::new();
    for (id, _, _, _) in notebooks {
        for s in db.list_sources(id).await.expect("list sources") {
            source_nb.insert(s.title.clone(), id.to_string());
            titles.insert(s.id, s.title);
        }
    }

    let datasets = load_datasets();
    let mut total = 0usize;
    let mut acc1 = 0usize;
    let mut acc2 = 0usize;
    let mut flat_recall_sum = 0.0f64;
    let mut routed_recall_sum = 0.0f64;
    for ds in &datasets {
        for q in &ds.queries {
            let expected: std::collections::HashSet<&String> = q
                .relevant
                .iter()
                .filter_map(|s| source_nb.get(&s.source_title))
                .collect();
            let qvec = ai.embed_one(&q.query).await.expect("embed query");
            let routes = crate::router::route_notebooks(&db, qvec.clone(), 3)
                .await
                .expect("route");
            total += 1;
            if expected.iter().all(|nb| routes.first() == Some(*nb)) {
                acc1 += 1;
            }
            if expected
                .iter()
                .all(|nb| routes.iter().take(2).any(|r| &r == nb))
            {
                acc2 += 1;
            } else {
                eprintln!(
                    "  MISROUTE: {:?} expected {:?}, routed {:?}",
                    q.query, expected, routes
                );
            }

            let judge = |hits: Vec<(String, Citation)>| {
                let cs: Vec<Citation> = hits.into_iter().map(|(_, c)| c).collect();
                let (ranks, _) = matched_ranks(&cs, &titles, &q.relevant);
                ranks.len() as f64 / q.relevant.len() as f64
            };
            let flat = db
                .search_chunks_all_opts(qvec.clone(), &q.query, K, None, SearchOptions::default())
                .await
                .expect("flat search");
            flat_recall_sum += judge(flat);
            let top2: Vec<String> = routes.into_iter().take(2).collect();
            let routed = db
                .search_chunks_all_opts(qvec, &q.query, K, Some(&top2), SearchOptions::default())
                .await
                .expect("routed search");
            routed_recall_sum += judge(routed);
        }
    }
    let acc1 = acc1 as f64 / total as f64;
    let acc2 = acc2 as f64 / total as f64;
    let flat_recall = flat_recall_sum / total as f64;
    let routed_recall = routed_recall_sum / total as f64;
    eprintln!("\nrouter over {total} dataset queries, 3 notebooks:");
    eprintln!("  accuracy@1 {acc1:.2}   accuracy@2 {acc2:.2}");
    eprintln!("  recall@{K}: flat {flat_recall:.2}   routed(top-2) {routed_recall:.2}\n");

    assert!(
        acc2 >= 0.9,
        "routing accuracy@2 {acc2:.2} below 0.9 — router summaries too weak"
    );
    assert!(
        routed_recall >= flat_recall - 0.05,
        "routed recall {routed_recall:.2} fell more than 0.05 below flat {flat_recall:.2}"
    );
}

/// Context-profile eval (RFC-inference-providers §2): the on-device model's
/// tight profile retrieves k=4 where the default retrieves k=8 — measure how
/// much relevant-source coverage the small profile keeps, so "decent results
/// for everything" is a number, not a vibe. Records both recalls and gates
/// on retention: the k=4 profile must keep ≥75% of k=8's hits.
#[tokio::test]
async fn eval_context_profiles() {
    let Some(ai) = builtin_ai().await else { return };
    let datasets = load_datasets();
    assert!(!datasets.is_empty(), "no datasets in evals/datasets/");

    let dir = std::env::temp_dir().join(format!("nbl-eval-prof-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let nb = "eval-nb-prof";
    seed_corpus(&ai, &db, nb).await;
    seed_docs(&ai, &db, nb, EXTRA_CORPUS, "x-").await;
    let titles: HashMap<String, String> = db
        .list_sources(nb)
        .await
        .expect("list sources")
        .into_iter()
        .map(|s| (s.id, s.title))
        .collect();

    let profiles: [(usize, &str); 2] = [
        (
            crate::inference::ContextProfile::default().retrieve_k,
            "default",
        ),
        (4, "on-device"),
    ];
    let mut hits_at: HashMap<&str, (f64, f64)> = HashMap::new(); // name -> (hit, total)
    for ds in &datasets {
        let query_texts: Vec<String> = ds.queries.iter().map(|q| q.query.clone()).collect();
        let qvecs = ai.embed(&query_texts).await.expect("embed queries");
        for (q, qvec) in ds.queries.iter().zip(&qvecs) {
            for (k, name) in profiles {
                let hits = db
                    .search_chunks(nb, qvec.clone(), &q.query, k, None)
                    .await
                    .expect("profile search");
                let found: std::collections::HashSet<&str> = hits
                    .iter()
                    .filter_map(|c| titles.get(&c.source_id))
                    .map(String::as_str)
                    .collect();
                for rel in &q.relevant {
                    let e = hits_at.entry(name).or_insert((0.0, 0.0));
                    e.1 += 1.0;
                    if found.contains(rel.source_title.as_str()) {
                        e.0 += 1.0;
                    }
                }
            }
        }
    }
    let recall = |name: &str| {
        let (h, t) = hits_at[name];
        if t == 0.0 {
            0.0
        } else {
            h / t
        }
    };
    let (full, tight) = (recall("default"), recall("on-device"));
    let retention = if full == 0.0 { 1.0 } else { tight / full };
    println!(
        "context profiles: default k={} recall {:.3} · on-device k=4 recall {:.3} · retention {:.1}%",
        crate::inference::ContextProfile::default().retrieve_k,
        full,
        tight,
        retention * 100.0
    );
    assert!(
        retention >= 0.75,
        "on-device profile keeps only {:.1}% of default-profile recall",
        retention * 100.0
    );
}

/// Handwritten gist fixtures (RFC-infinite-context §1). The eval seeds
/// these as stored gist rows exactly as the production sweep would write
/// them — generation quality is gated and unit-tested in `gist.rs`; this
/// eval covers what stored gists do to retrieval: overview queries find
/// them, exact-identifier queries are not displaced by them, and the
/// `max_gists` cap holds.
const GIST_FIXTURES: &[(&str, &str)] = &[
    (
        "Media Server Maintenance",
        "This runbook describes the routine upkeep of the home media server: \
         when the library scan runs, how many transcode streams are allowed \
         at once, and the monthly restart schedule tied to backups. It can \
         answer when scans happen, what the transcoding cap is, and when the \
         server container restarts. Key terms: media server, library scan, \
         transcoding, restarts",
    ),
    (
        "Home Insurance Policy",
        "This policy document lays out the homeowner's coverage terms: the \
         deductibles that apply to wind, hail, and theft losses, and the \
         claims process with its filing window. It can answer how large each \
         deductible is and how and when to file a claim through the agent \
         portal. Key terms: deductible, wind and hail, theft, claims, agent \
         portal",
    ),
    (
        "Vendor Payment Runbook",
        "This runbook covers how vendor invoices get paid: wire payments on \
         net-forty-five terms with same-day remittance advice, and the \
         escalation path for disputed invoices through procurement. It can \
         answer how vendors are paid, on what terms, and what happens when \
         an invoice is disputed. Key terms: wires, net-forty-five, \
         remittance advice, disputes, procurement",
    ),
];

/// Seed one stored gist row the way `gist::ensure_gists` writes them:
/// verbatim gist in `text`, title-context prefix on the embedded form, and
/// the source's content hash in `ordinal`.
async fn seed_gist(ai: &crate::ai::Ai, db: &Db, notebook_id: &str, title: &str, gist: &str) {
    let sources = db.list_sources(notebook_id).await.expect("list sources");
    let s = sources
        .iter()
        .find(|s| s.title == title)
        .unwrap_or_else(|| panic!("gist fixture names unknown source {title:?}"));
    let full = db
        .get_source(&s.id)
        .await
        .expect("get source")
        .expect("source exists");
    let embed_input = format!("[{} — overview]\n{gist}", s.title);
    let embeddings = ai.embed(&[embed_input]).await.expect("embed gist");
    db.add_chunks(
        notebook_id,
        &format!("{}{}", crate::db::GIST_CHUNK_PREFIX, s.id),
        &[(
            format!("gist-{}", s.id),
            crate::gist::content_hash(&full.content),
            gist.to_string(),
        )],
        &embeddings,
    )
    .await
    .expect("write gist row");
}

/// Corpus-wide retrieval with stored gists (RFC-infinite-context §1):
/// overview questions surface the gist row; exact-identifier questions keep
/// their verbatim winner; the gist class cap holds; and every corpus-wide
/// read (including FTS-only) returns filled titles — the shared
/// title-filling pass this RFC required.
#[tokio::test]
async fn eval_gist_rows() {
    let Some(ai) = builtin_ai().await else { return };
    let dir = std::env::temp_dir().join(format!("nbl-eval-gist-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    seed_docs(&ai, &db, "gist-a", CORPUS, "a-").await;
    seed_docs(&ai, &db, "gist-b", EXTRA_CORPUS, "b-").await;
    let gist_notebook = |title: &str| {
        if CORPUS.iter().any(|(t, _)| *t == title) {
            "gist-a"
        } else {
            "gist-b"
        }
    };
    for (title, gist) in GIST_FIXTURES {
        seed_gist(&ai, &db, gist_notebook(title), title, gist).await;
    }

    const K: usize = 8;
    let opts = SearchOptions {
        pool_multiplier: 4,
        max_per_source: 2,
        max_per_notebook: 3,
        max_notes: 4,
        max_gists: 2,
    };
    // Overview intent → the gist row surfaces, titled after its source.
    let q = "which document explains how the media server is maintained overall?";
    let qv = ai.embed_one(q).await.expect("embed");
    let hits = db
        .search_chunks_all_opts(qv, q, K, None, opts)
        .await
        .expect("search");
    let gist_rank = hits
        .iter()
        .position(|(_, c)| c.gist && c.source_title == "Media Server Maintenance");
    eprintln!(
        "gist eval: overview query gist rank = {:?} of {}",
        gist_rank.map(|r| r + 1),
        hits.len()
    );
    assert!(
        gist_rank.is_some_and(|r| r < 5),
        "media-server gist should rank in the top 5 for an overview query"
    );

    // Exact identifier → verbatim chunk stays the winner (the fence: gists
    // must never displace exact evidence).
    let q = "what is the status of INV-2024-0042?";
    let qv = ai.embed_one(q).await.expect("embed");
    let hits = db
        .search_chunks_all_opts(qv, q, K, None, opts)
        .await
        .expect("search");
    let top = &hits.first().expect("hits").1;
    assert!(
        !top.gist && top.snippet.contains("INV-2024-0042"),
        "exact-identifier winner must be a verbatim chunk, got gist={} snippet={:?}",
        top.gist,
        top.snippet.chars().take(60).collect::<String>()
    );

    // Cap: an all-overviews query keeps at most `max_gists` gist rows.
    let q = "give me an overview of what these documents cover";
    let qv = ai.embed_one(q).await.expect("embed");
    let hits = db
        .search_chunks_all_opts(qv, q, K, None, opts)
        .await
        .expect("search");
    let gist_hits = hits.iter().filter(|(_, c)| c.gist).count();
    eprintln!(
        "gist eval: overview-of-everything gist hits = {gist_hits}/{}",
        hits.len()
    );
    assert!(
        gist_hits <= 2,
        "max_gists=2 must cap gist rows, got {gist_hits}"
    );

    // FTS-only corpus-wide read fills titles for every row kind — the
    // empty-title gap this RFC's Phase 1 required fixed.
    let fts = db
        .search_chunks_fts_all("remittance", K)
        .await
        .expect("fts all");
    assert!(
        !fts.is_empty(),
        "fts should match the vendor runbook corpus"
    );
    for (_, c) in &fts {
        assert!(
            !c.source_title.is_empty(),
            "corpus-wide FTS returned an empty title for chunk {}",
            c.chunk_id
        );
    }
}

// ---- Global source selection (RFC-infinite-context §4) --------------------
//
// The global answer route retrieves the standing gist layer corpus-wide, then
// fans out per source. This eval covers the retrieval half — `search_gists` —
// with handwritten gists: a global question must cover the sources it touches
// while the per-notebook cap keeps one notebook from owning the fan-out.
// Extract quality is a live-model concern, out of the deterministic suite.

/// Network-heavy notebook: four network sources plus a baking distractor, so
/// the per-notebook gist cap has a fifth candidate to trim.
const GLOBAL_NET: &[(&str, &str)] = &[
    (
        "Home Network Guide",
        "The home router lives in the hallway closet. Port 32400 forwards to \
         the Plex media server. The guest WiFi is isolated from home devices \
         and rotates its passphrase every quarter.",
    ),
    (
        "Office Network Runbook",
        "Office switches are patched on the last Friday of the quarter. Port \
         8443 forwards to the badge system console. The conference network \
         uses a captive portal with a daily rotating code.",
    ),
    (
        "Cabin WiFi Setup",
        "The cabin router sits above the wood stove shelf. Port 8080 forwards \
         to the trail camera system. The guest passphrase is taped inside the \
         pantry door.",
    ),
    (
        "VPN Access Guide",
        "The VPN uses WireGuard listening on port 51820. Shell access to home \
         machines goes over the VPN only; nothing listens on the public \
         interface.",
    ),
    (
        "Sourdough Notes",
        "Feed the starter twice daily at room temperature. Bulk fermentation \
         runs four to six hours. A dutch oven preheated to four hundred fifty \
         degrees gives the best oven spring.",
    ),
];

/// Payments notebook: exactly the sources the payments query expects, so the
/// cap returns all of them and recall does not hinge on a ranking tie-break.
const GLOBAL_PAY: &[(&str, &str)] = &[
    (
        "Acme Invoices Q3",
        "INV-2024-0042 for Acme Corp is paid; INV-2024-0051 for Globex is \
         overdue. Retries that fail with ERR-503-BACKOFF wait sixty seconds. \
         Contact billing for escalations.",
    ),
    (
        "Vendor Payment Runbook",
        "Vendor invoices are paid by wire on net-forty-five terms with \
         same-day remittance advice. Disputed invoices escalate to \
         procurement within five business days.",
    ),
    (
        "Contractor Agreement Summary",
        "Contractors invoice monthly on net-thirty terms. Late payments \
         accrue one percent interest per month. Unlogged hours past thirty \
         days are not billable.",
    ),
];

/// Benefits notebook: exactly the two sources the benefits query expects.
const GLOBAL_LIFE: &[(&str, &str)] = &[
    (
        "Employee Handbook",
        "Employees accrue one and a half days of paid time off per month, \
         available after the first ninety days. Expense reports are due by \
         the fifth business day of the following month.",
    ),
    (
        "Benefits FAQ",
        "The company observes eleven paid holidays per year. Sick time is \
         unlimited within reason. Parental leave is sixteen weeks paid after \
         six months of service.",
    ),
];

/// One handwritten gist per source, keyed by title — seeded exactly as the
/// production sweep writes them via `seed_gist`.
const GLOBAL_GISTS: &[(&str, &str)] = &[
    (
        "Home Network Guide",
        "This guide covers the home network: where the router lives, the Plex \
         port forward on 32400, and the isolated guest WiFi. Key terms: \
         router, port 32400, guest WiFi.",
    ),
    (
        "Office Network Runbook",
        "This runbook covers the office network: the switch patching cadence, \
         the badge console port forward on 8443, and the conference captive \
         portal. Key terms: switches, port 8443, captive portal.",
    ),
    (
        "Cabin WiFi Setup",
        "This note covers the cabin network: the router location, the \
         trail-camera port forward on 8080, and the guest passphrase. Key \
         terms: cabin router, port 8080, trail camera.",
    ),
    (
        "VPN Access Guide",
        "This guide covers remote access: the WireGuard VPN on port 51820 and \
         SSH restricted to the VPN. Key terms: WireGuard, port 51820, SSH.",
    ),
    (
        "Sourdough Notes",
        "These notes cover sourdough baking: starter feeding, bulk \
         fermentation timing, and oven temperature. Key terms: starter, \
         fermentation, oven spring.",
    ),
    (
        "Acme Invoices Q3",
        "This sheet lists Q3 invoices and their status, plus the \
         ERR-503-BACKOFF retry wait. It can answer which invoices are paid or \
         overdue. Key terms: INV-2024-0042, ERR-503-BACKOFF, billing.",
    ),
    (
        "Vendor Payment Runbook",
        "This runbook covers vendor payments: wire payments on net-forty-five \
         terms and the dispute escalation path. Key terms: wires, \
         net-forty-five, procurement.",
    ),
    (
        "Contractor Agreement Summary",
        "This summary covers contractor payment terms: monthly net-thirty \
         invoicing, late-payment interest, and time-tracking rules. Key \
         terms: net-thirty, interest, time tracking.",
    ),
    (
        "Employee Handbook",
        "This handbook covers time off and expenses: paid-time-off accrual \
         and the expense-report deadline. Key terms: paid time off, expense \
         reports.",
    ),
    (
        "Benefits FAQ",
        "This FAQ covers employee benefits: paid holidays, sick time, and \
         parental leave. Key terms: holidays, sick time, parental leave.",
    ),
];

/// One global question and the source titles it should cover.
struct GlobalCase {
    query: &'static str,
    expected: &'static [&'static str],
}

const GLOBAL_CASES: &[GlobalCase] = &[
    GlobalCase {
        query: "compare all my invoice and payment terms across my notebooks",
        expected: &[
            "Acme Invoices Q3",
            "Vendor Payment Runbook",
            "Contractor Agreement Summary",
        ],
    },
    GlobalCase {
        query: "what do all my sources say about employee time off and benefits?",
        expected: &["Employee Handbook", "Benefits FAQ"],
    },
];

/// Corpus-wide gist selection (RFC-infinite-context §4): a global question
/// covers the labeled sources it touches, and the per-notebook cap trims a
/// notebook that over-supplies. Deterministic (builtin embedder + handwritten
/// gists), so a failure is a selection regression, not flakiness.
#[tokio::test]
async fn eval_global_source_selection() {
    let Some(ai) = builtin_ai().await else { return };
    let dir = std::env::temp_dir().join(format!("nbl-eval-global-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    seed_docs(&ai, &db, "glob-net", GLOBAL_NET, "gn-").await;
    seed_docs(&ai, &db, "glob-pay", GLOBAL_PAY, "gp-").await;
    seed_docs(&ai, &db, "glob-life", GLOBAL_LIFE, "gl-").await;
    let notebook_of = |title: &str| -> &'static str {
        if GLOBAL_NET.iter().any(|(t, _)| *t == title) {
            "glob-net"
        } else if GLOBAL_PAY.iter().any(|(t, _)| *t == title) {
            "glob-pay"
        } else {
            "glob-life"
        }
    };
    for (title, gist) in GLOBAL_GISTS {
        seed_gist(&ai, &db, notebook_of(title), title, gist).await;
    }

    const MAX_PER_NB: usize = 3;
    let select = |q: &'static str| async {
        let qv = ai.embed_one(q).await.expect("embed query");
        db.search_gists(qv, 12)
            .await
            .expect("search gists")
            .into_iter()
            .map(|(nb, c)| (nb, c.source_title))
            .collect::<Vec<_>>()
    };
    let cap_holds = |sel: &[(String, String)]| {
        let mut per_nb: HashMap<&str, usize> = HashMap::new();
        for (nb, _) in sel {
            *per_nb.entry(nb.as_str()).or_default() += 1;
        }
        per_nb.values().all(|&n| n <= MAX_PER_NB)
    };

    eprintln!("\nglobal source selection (search_gists, cap {MAX_PER_NB}/notebook):");
    for c in GLOBAL_CASES {
        let sel = select(c.query).await;
        assert!(cap_holds(&sel), "per-notebook cap exceeded: {sel:?}");
        let titles: std::collections::HashSet<&str> = sel.iter().map(|(_, t)| t.as_str()).collect();
        let missing: Vec<&str> = c
            .expected
            .iter()
            .copied()
            .filter(|t| !titles.contains(t))
            .collect();
        eprintln!(
            "  {:<58} covered {}/{}",
            c.query.chars().take(58).collect::<String>(),
            c.expected.len() - missing.len(),
            c.expected.len()
        );
        assert!(
            missing.is_empty(),
            "global selection for {:?} missed {missing:?}; got {sel:?}",
            c.query
        );
    }

    // Cap-trim query: five gists in glob-net, only three come back, and the
    // baking distractor never displaces a network source.
    let q = "summarize everything about my home, office, cabin, and vpn networks";
    let sel = select(q).await;
    assert!(cap_holds(&sel), "per-notebook cap exceeded: {sel:?}");
    let net: Vec<&str> = sel
        .iter()
        .filter(|(nb, _)| nb == "glob-net")
        .map(|(_, t)| t.as_str())
        .collect();
    eprintln!("  network query → glob-net kept {net:?}");
    assert_eq!(
        net.len(),
        MAX_PER_NB,
        "cap should trim glob-net's five gists to {MAX_PER_NB}, got {net:?}"
    );
    assert!(
        !net.contains(&"Sourdough Notes"),
        "cap kept the baking distractor over a network source: {net:?}"
    );
}

// ---- Scale fence (RFC-infinite-context §3) --------------------------------
//
// Deterministic synthetic corpora at 1M/3M (and, behind --ignored, 10M)
// chars, with 12 fixed needle documents planted among distractor docs whose
// identifier spaces are disjoint from every needle. The fence that makes
// "infinite context" falsifiable: exact-identifier recall stays 1.00 at
// every size, and mean recall must not sag as the corpus grows.

/// xorshift64 — deterministic distractor generation without a rand dep.
/// (`Date`/`rand` would make corpora differ across runs; the whole point is
/// a byte-stable corpus per size.)
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn pick<'a>(&mut self, xs: &'a [&'a str]) -> &'a str {
        xs[self.below(xs.len() as u64) as usize]
    }
}

const SCALE_HOSTS: &[&str] = &[
    "maple", "birch", "walnut", "aspen", "cedar", "willow", "rowan", "alder",
];
const SCALE_CUSTOMERS: &[&str] = &[
    "Acme Corp",
    "Globex",
    "Initech",
    "Umbrella",
    "Stark Industrial",
    "Wayne Logistics",
];
const SCALE_SERVICES: &[&str] = &[
    "billing",
    "ingest",
    "metrics",
    "auth",
    "search",
    "archive",
    "reporting",
    "queue",
];

/// One large distractor document (~24k chars, ten sections) — long enough
/// that a 3M-char corpus is ~125 inserts, structured like the real PDFs and
/// runbooks users import. Identifier spaces are deliberately disjoint from
/// every needle: invoices 2015–2023, errors ERR-100..899, ports below
/// 50000, project codes PRJ-1000..6999, hosts from the tree word bank.
fn scale_doc(rng: &mut XorShift, i: usize) -> (String, String) {
    let mut body = String::with_capacity(26_000);
    for sec in 0..10 {
        match rng.below(4) {
            0 => {
                body.push_str(&format!("# Invoice batch {sec}\n\n"));
                for _ in 0..6 {
                    body.push_str(&format!(
                        "INV-20{}-{:04} | {} | ${},{:03} | {}\n",
                        15 + rng.below(9),
                        rng.below(9000) + 100,
                        rng.pick(SCALE_CUSTOMERS),
                        rng.below(40) + 1,
                        rng.below(1000),
                        if rng.below(3) == 0 { "overdue" } else { "paid" },
                    ));
                }
                body.push_str(&format!(
                    "\nRetries for this batch use ERR-{}-BACKOFF with a {} second wait.\n\n",
                    rng.below(800) + 100,
                    rng.below(50) + 5
                ));
            }
            1 => {
                body.push_str(&format!("# Service runbook section {sec}\n\n"));
                body.push_str(&format!(
                    "The {} service listens on port {} of host {}-{:02}. Deploys roll \
                     Tuesday; on-call rotates weekly. Health checks poll every {} \
                     seconds and page after three consecutive failures. Project code \
                     PRJ-{}.\n\n",
                    rng.pick(SCALE_SERVICES),
                    rng.below(46_000) + 3_000,
                    rng.pick(SCALE_HOSTS),
                    rng.below(90) + 1,
                    rng.below(50) + 10,
                    rng.below(6_000) + 1_000,
                ));
            }
            2 => {
                body.push_str(&format!("# Meeting notes, week {sec}\n\n"));
                body.push_str(&format!(
                    "Attendees reviewed the {} migration and agreed to defer the \
                     {} cleanup until after the freeze. Action items: audit the \
                     {} dashboards, refresh the {} credentials, and close out \
                     stale tickets older than {} days.\n\n",
                    rng.pick(SCALE_SERVICES),
                    rng.pick(SCALE_SERVICES),
                    rng.pick(SCALE_SERVICES),
                    rng.pick(SCALE_SERVICES),
                    rng.below(90) + 30,
                ));
            }
            _ => {
                body.push_str(&format!("# Policy appendix {sec}\n\n"));
                body.push_str(&format!(
                    "Expense reports above ${} require director approval and are \
                     reimbursed within {} business days. Travel booked through the \
                     portal earns no points but is covered by the corporate policy \
                     for {} staff.\n\n",
                    (rng.below(40) + 1) * 250,
                    rng.below(20) + 3,
                    rng.pick(SCALE_SERVICES),
                ));
            }
        }
    }
    (format!("Corpus binder {i:04}"), body)
}

/// Fixed needle documents. Every identifier here is outside the distractor
/// generator's ranges, so exactly one document in any corpus answers each
/// query — recall is unambiguous at every size.
const SCALE_NEEDLES: &[(&str, &str)] = &[
    (
        "Zephyr Project Charter",
        "Project PRJ-7741-ZEPHYR covers the migration of archival storage to \
         cold tier. The charter owner is the platform group and the budget \
         line closes at fiscal year end.",
    ),
    (
        "Invoice Exceptions 2077",
        "INV-2077-0420 for Vandelay Import/Export remains disputed at \
         $83,000. The exception ages out of the ledger after two quarters.",
    ),
    (
        "Frost Incident Postmortem",
        "The outage tracked as ERR-9917-FROST began with a stuck mutex in \
         the scheduler and was mitigated by draining the standby pool.",
    ),
    (
        "Kestrel Host Bringup",
        "Host kestrel-40k exposes the debug console on port 55731. Serial \
         access requires the lab bastion and a hardware token.",
    ),
    (
        "Vulnerability Note 31337",
        "CVE-2099-31337 affects the legacy image resizer; the fix pins the \
         codec and disables remote profile loading.",
    ),
    (
        "Aurora Contract Rider",
        "The Aurora rider sets penalty clause AUR-2088-PENALTY at twelve \
         percent for missed delivery windows in winter months.",
    ),
    (
        "Solar Array Field Report",
        "Solar array output peaked at 4.2 kilowatts during the June heat \
         wave, and the inverter clipped for two afternoons in a row.",
    ),
    (
        "Beekeeping Season Log",
        "The north hive swarmed in late spring; a captured swarm was rehomed \
         into the cedar box and accepted the new queen within a week.",
    ),
    (
        "Sourdough Hydration Study",
        "Raising hydration to eighty two percent opened the crumb \
         dramatically but made shaping harder on warm days.",
    ),
    (
        "Glacier Hike Journal",
        "The crossing took nine hours with crampons required above the \
         moraine; the hut warden stamped permits at the saddle.",
    ),
    (
        "Cello Practice Plan",
        "Thumb position drills come before the Elgar concerto excerpts; \
         scales run three octaves with a drone on the open string.",
    ),
    (
        "Heirloom Tomato Trial",
        "The Cherokee Purple plants outyielded the Brandywine rows despite \
         later transplanting and half the fertilizer.",
    ),
];

struct ScaleQuery {
    query: &'static str,
    kind: &'static str, // "exact" | "paraphrase"
    title: &'static str,
}

const SCALE_QUERIES: &[ScaleQuery] = &[
    ScaleQuery {
        query: "what is PRJ-7741-ZEPHYR about?",
        kind: "exact",
        title: "Zephyr Project Charter",
    },
    ScaleQuery {
        query: "what is the status of INV-2077-0420?",
        kind: "exact",
        title: "Invoice Exceptions 2077",
    },
    ScaleQuery {
        query: "what caused ERR-9917-FROST?",
        kind: "exact",
        title: "Frost Incident Postmortem",
    },
    ScaleQuery {
        query: "which host uses port 55731?",
        kind: "exact",
        title: "Kestrel Host Bringup",
    },
    ScaleQuery {
        query: "what does CVE-2099-31337 affect?",
        kind: "exact",
        title: "Vulnerability Note 31337",
    },
    ScaleQuery {
        query: "what is the AUR-2088-PENALTY clause?",
        kind: "exact",
        title: "Aurora Contract Rider",
    },
    ScaleQuery {
        query: "how much power did the panels produce at their peak?",
        kind: "paraphrase",
        title: "Solar Array Field Report",
    },
    ScaleQuery {
        query: "what happened when the bees swarmed?",
        kind: "paraphrase",
        title: "Beekeeping Season Log",
    },
    ScaleQuery {
        query: "what did the wetter bread dough change?",
        kind: "paraphrase",
        title: "Sourdough Hydration Study",
    },
    ScaleQuery {
        query: "how long did the glacier crossing take?",
        kind: "paraphrase",
        title: "Glacier Hike Journal",
    },
    ScaleQuery {
        query: "what should I practice before the concerto?",
        kind: "paraphrase",
        title: "Cello Practice Plan",
    },
    ScaleQuery {
        query: "which tomato variety produced more fruit?",
        kind: "paraphrase",
        title: "Heirloom Tomato Trial",
    },
];

/// Build a corpus of ~`target_chars`, run the needle queries corpus-wide at
/// the adaptive k, and return (exact recall, paraphrase recall, k used).
async fn run_scale(ai: &crate::ai::Ai, target_chars: usize) -> (f64, f64, usize) {
    let dir = std::env::temp_dir().join(format!("nbl-eval-scale-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let mut rng = XorShift(0x5EED_0000 + target_chars as u64);

    // Generate distractors up to the target, needles interleaved evenly.
    let mut docs: Vec<(String, String)> = Vec::new();
    let mut chars = 0usize;
    let mut i = 0usize;
    while chars < target_chars {
        let (t, b) = scale_doc(&mut rng, i);
        chars += b.chars().count();
        docs.push((t, b));
        i += 1;
    }
    let stride = (docs.len() / SCALE_NEEDLES.len()).max(1);
    for (ni, (t, b)) in SCALE_NEEDLES.iter().enumerate() {
        let at = (ni * stride + stride / 2).min(docs.len());
        docs.insert(at, (t.to_string(), b.to_string()));
    }

    // Chunk everything, embed in large batches, insert per doc.
    let notebooks = ["s-a", "s-b", "s-c", "s-d", "s-e", "s-f", "s-g", "s-h"];
    let mut total_chars = 0i64;
    let mut pending: Vec<(usize, crate::ingest::Chunk)> = Vec::new(); // (doc idx, chunk)
    let chunked: Vec<Vec<crate::ingest::Chunk>> = docs
        .iter()
        .map(|(t, b)| crate::ingest::chunk_text(t, &normalize_ws(b)))
        .collect();
    for (di, chunks) in chunked.iter().enumerate() {
        for c in chunks {
            pending.push((
                di,
                crate::ingest::Chunk {
                    text: c.text.clone(),
                    embed_text: c.embed_text.clone(),
                },
            ));
        }
    }
    let inputs: Vec<String> = pending.iter().map(|(_, c)| c.embed_text.clone()).collect();
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(inputs.len());
    for batch in inputs.chunks(256) {
        vectors.extend(ai.embed(&batch.to_vec()).await.expect("embed scale corpus"));
    }
    let mut cursor = 0usize;
    for (di, (title, body)) in docs.iter().enumerate() {
        let n = chunked[di].len();
        let tuples: Vec<(String, i32, String)> = chunked[di]
            .iter()
            .enumerate()
            .map(|(ci, c)| (format!("d{di}-c{ci}"), ci as i32, c.text.clone()))
            .collect();
        let vecs = vectors[cursor..cursor + n].to_vec();
        cursor += n;
        let source = crate::models::Source {
            id: format!("scale-src-{di}"),
            notebook_id: notebooks[di % notebooks.len()].to_string(),
            title: title.clone(),
            source_type: "text".into(),
            url: String::new(),
            content: body.clone(),
            char_count: body.chars().count() as i64,
            chunk_count: n as i64,
            created_at: 1_700_000_000_000 + di as i64,
            status: "ready".into(),
            error: String::new(),
            parent_id: String::new(),
            mtime: 0,
        };
        total_chars += source.char_count;
        db.insert_source(&source, &tuples, &vecs)
            .await
            .expect("insert scale doc");
    }

    let k = crate::inference::ContextProfile::default().retrieve_k_for(total_chars);
    let opts = SearchOptions {
        pool_multiplier: 4,
        max_per_source: 2,
        max_per_notebook: 3,
        max_notes: 4,
        max_gists: 2,
    };
    let queries: Vec<String> = SCALE_QUERIES.iter().map(|q| q.query.to_string()).collect();
    let qvecs = ai.embed(&queries).await.expect("embed scale queries");
    let (mut exact_hit, mut exact_n, mut para_hit, mut para_n) = (0f64, 0f64, 0f64, 0f64);
    for (q, qv) in SCALE_QUERIES.iter().zip(qvecs) {
        let hits = db
            .search_chunks_all_opts(qv, q.query, k, None, opts)
            .await
            .expect("scale search");
        let found = hits.iter().any(|(_, c)| c.source_title == q.title);
        if q.kind == "exact" {
            exact_n += 1.0;
            exact_hit += found as u8 as f64;
        } else {
            para_n += 1.0;
            para_hit += found as u8 as f64;
        }
        if !found {
            eprintln!(
                "  scale MISS ({} chars, {}): {}",
                total_chars, q.kind, q.query
            );
        }
    }
    (exact_hit / exact_n, para_hit / para_n, k)
}

/// Collapse the escaped-continuation whitespace in generated bodies so the
/// chunker sees ordinary paragraphs.
fn normalize_ws(s: &str) -> String {
    s.replace("  ", " ")
}

/// The fence at 1M and 3M chars (10M below): exact recall holds at 1.00 at
/// every size, and recall does not sag with scale. Last verified run:
/// 1M exact 1.00 / para 1.00 (k=10) · 3M exact 1.00 / para 1.00 (k=11).
/// Ignored by default: per-doc LanceDB inserts put seeding at ~10 minutes —
/// run on retrieval changes via
/// `cargo test --lib eval_scale_fence -- --ignored --nocapture`
/// (a bulk-insert seeding path would bring this into the default suite).
#[tokio::test]
#[ignore = "scale corpus seeding takes ~10 minutes; run explicitly on retrieval changes"]
async fn eval_scale_fence() {
    let Some(ai) = builtin_ai().await else { return };
    let (e1, p1, k1) = run_scale(&ai, 1_000_000).await;
    let (e3, p3, k3) = run_scale(&ai, 3_000_000).await;
    eprintln!("scale fence: 1M exact {e1:.2} para {p1:.2} (k={k1}) · 3M exact {e3:.2} para {p3:.2} (k={k3})");
    assert!(
        k3 > k1,
        "adaptive k must grow with the corpus ({k1} → {k3})"
    );
    assert_eq!(e1, 1.0, "1M: every exact-identifier needle must be found");
    assert_eq!(e3, 1.0, "3M: every exact-identifier needle must be found");
    assert!(
        (p1 + e1) / 2.0 - (p3 + e3) / 2.0 <= 0.05,
        "recall sagged with corpus growth: 1M mean {:.2} → 3M mean {:.2}",
        (p1 + e1) / 2.0,
        (p3 + e3) / 2.0
    );
}

/// The 10M-char fence — minutes of embedding, so opt-in:
/// `cargo test --lib eval_scale_fence_10m -- --ignored --nocapture`
#[tokio::test]
#[ignore = "10M-char corpus takes minutes to embed; run explicitly"]
async fn eval_scale_fence_10m() {
    let Some(ai) = builtin_ai().await else { return };
    let (e10, p10, k10) = run_scale(&ai, 10_000_000).await;
    eprintln!("scale fence: 10M exact {e10:.2} para {p10:.2} (k={k10})");
    assert_eq!(e10, 1.0, "10M: every exact-identifier needle must be found");
    assert!(p10 >= 0.5, "10M: paraphrase recall collapsed to {p10:.2}");
}
