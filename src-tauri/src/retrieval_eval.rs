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
