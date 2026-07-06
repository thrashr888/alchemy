//! Retrieval-quality evals: a golden question set over a fixture corpus,
//! measuring recall for vector-only vs. hybrid (vector + BM25) search, plus
//! model-dependent checks for the distillation and rerank sub-calls.
//!
//! The retrieval eval needs only the built-in embedder (downloads ~30 MB on
//! first run, cached afterwards) — no Ollama. The distill/rerank evals skip
//! unless Ollama is reachable, mirroring the round-trip tests.
//!
//! Run with:  cargo test --lib evals -- --nocapture

use crate::ai::{Ai, AiConfig, AiRuntime};
use crate::db::Db;
use crate::ingest;
use crate::models::Citation;

/// (title, body) fixture documents: prose for paraphrase queries, tables and
/// identifiers for exact-match queries, markdown sections for section queries,
/// and distractors so retrieval has something to get wrong.
const CORPUS: &[(&str, &str)] = &[
    (
        "Acme Invoices Q3",
        "# Sheet: Outstanding\n\
         invoice | customer | amount | status\n\
         INV-2024-0042 | Acme Corp | $12,400 | paid\n\
         INV-2024-0051 | Globex | $8,150 | overdue\n\
         INV-2024-0057 | Initech | $3,300 | disputed\n\n\
         # Sheet: Notes\n\
         Retries that fail with ERR-503-BACKOFF should wait sixty seconds before \n\
         the next attempt. Contact billing for escalations.",
    ),
    (
        "Home Network Guide",
        "# Router Setup\n\nThe router lives in the hallway closet. Firmware updates \
         are applied on the first Monday of each month.\n\n\
         # Guest WiFi\n\nVisitors get internet access through the guest network. The \
         guest network is isolated from home devices and rotates its passphrase \
         every quarter.\n\n\
         # Port Forwarding\n\nPort 32400 forwards to the media server for Plex. \
         Port 22 stays closed from the outside; use the VPN instead.",
    ),
    (
        "Kyoto Trip Journal",
        "We spent the first three nights at a small ryokan in the Gion district, \
         sleeping on tatami mats and eating breakfast in the garden. The owner \
         recommended the early-morning walk to Kiyomizu-dera before the crowds. \
         Later in the week we took the train to Nara to see the deer park, and \
         finished the trip with a kaiseki dinner near the Kamo river.",
    ),
    (
        "Employee Handbook",
        "# Time Off\n\nEmployees accrue one and a half days of paid time off per \
         month of service, available after the first ninety days.\n\n\
         # Expenses\n\nExpense reports are due by the fifth business day of the \
         following month. Receipts are required for anything over twenty dollars.",
    ),
    (
        "Sourdough Notes",
        "Feed the starter twice daily at room temperature. Bulk fermentation runs \
         four to six hours depending on kitchen warmth. A dutch oven preheated to \
         four hundred fifty degrees gives the best oven spring.",
    ),
    // Distractors: near-topic content that makes top-k competitive, including
    // look-alike identifiers so exact-match queries can actually fail.
    (
        "Acme Invoices Q2 (archive)",
        "# Sheet: Closed\n\
         invoice | customer | amount | status\n\
         INV-2024-0012 | Acme Corp | $9,900 | paid\n\
         INV-2024-0019 | Globex | $4,700 | paid\n\
         INV-2024-0023 | Hooli | $15,250 | written off\n\n\
         # Sheet: Notes\n\
         Older retries used ERR-429-THROTTLE handling with a five second wait. \
         That policy was replaced in Q3.",
    ),
    (
        "Office Network Runbook",
        "# Switch Rack\n\nThe office switches are patched on the last Friday of the \
         quarter. Spare cables live in the supply room.\n\n\
         # Conference WiFi\n\nThe conference network uses a captive portal and a \
         daily rotating code printed at reception.\n\n\
         # Firewall\n\nPort 8443 forwards to the badge system console. All other \
         inbound ports are closed by default.",
    ),
    (
        "Osaka Weekend Notes",
        "A quick weekend in Osaka: street food in Dotonbori, an afternoon at the \
         aquarium, and a capsule hotel near the station. The okonomiyaki place \
         the concierge suggested had a line around the block.",
    ),
    (
        "Contractor Agreement Summary",
        "# Payment Terms\n\nContractors invoice monthly with net-thirty terms. Late \
         payments accrue one percent interest per month.\n\n\
         # Time Tracking\n\nHours are logged weekly in the portal; unlogged hours \
         past thirty days are not billable.",
    ),
];

/// A golden question: the retrieval is correct if any of the top-k snippets
/// contains `expect` (case-insensitive). `kind` buckets the metrics.
struct Golden {
    kind: &'static str,
    question: &'static str,
    expect: &'static str,
}

const GOLDEN: &[Golden] = &[
    // Exact identifiers — where BM25 should shine and embeddings often miss.
    Golden {
        kind: "exact",
        question: "what is the status of INV-2024-0042?",
        expect: "INV-2024-0042",
    },
    Golden {
        kind: "exact",
        question: "which invoice is overdue for Globex?",
        expect: "INV-2024-0051",
    },
    Golden {
        kind: "exact",
        question: "what should happen after ERR-503-BACKOFF?",
        expect: "ERR-503-BACKOFF",
    },
    Golden {
        kind: "exact",
        question: "what service uses port 32400?",
        expect: "32400",
    },
    // Paraphrase — where vector similarity should shine.
    Golden {
        kind: "paraphrase",
        question: "how much vacation time do employees earn?",
        expect: "paid time off",
    },
    Golden {
        kind: "paraphrase",
        question: "where did we stay on the Japan trip?",
        expect: "ryokan",
    },
    Golden {
        kind: "paraphrase",
        question: "how do visitors get on the internet at home?",
        expect: "guest network",
    },
    Golden {
        kind: "paraphrase",
        question: "when do I need to turn in receipts for work purchases?",
        expect: "expense reports",
    },
    Golden {
        kind: "paraphrase",
        question: "how warm should the oven be for baking bread?",
        expect: "four hundred fifty",
    },
    // Section-targeted — structure-aware chunks should keep these coherent.
    Golden {
        kind: "section",
        question: "when are router firmware updates applied?",
        expect: "first monday",
    },
    Golden {
        kind: "section",
        question: "is ssh open to the internet?",
        expect: "port 22",
    },
    Golden {
        kind: "section",
        question: "what temple did the ryokan owner recommend visiting early?",
        expect: "kiyomizu",
    },
];

async fn builtin_ai() -> Option<Ai> {
    let ai = Ai::new(
        AiConfig {
            embedder: "builtin".into(),
            ..Default::default()
        },
        AiRuntime::default(),
    );
    match ai.test_embed().await {
        Ok(_) => Some(ai),
        Err(_) => {
            eprintln!("SKIP: built-in embedder unavailable (no network for first download?)");
            None
        }
    }
}

/// Ingest the fixture corpus through the real chunk → embed → store path.
async fn seed_corpus(ai: &Ai, db: &Db, notebook_id: &str) {
    for (i, (title, body)) in CORPUS.iter().enumerate() {
        let extracted = ingest::extract_pasted(title, body).expect("extract fixture");
        let chunks = ingest::chunk_text(&extracted.title, &extracted.text);
        let embed_inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
        let embeddings = ai.embed(&embed_inputs).await.expect("embed fixture");
        let tuples: Vec<(String, i32, String)> = chunks
            .iter()
            .enumerate()
            .map(|(j, c)| (format!("c{i}-{j}"), j as i32, c.text.clone()))
            .collect();
        let source = crate::models::Source {
            id: format!("src-{i}"),
            notebook_id: notebook_id.to_string(),
            title: extracted.title.clone(),
            source_type: "text".into(),
            url: String::new(),
            content: extracted.text.clone(),
            char_count: extracted.text.chars().count() as i64,
            chunk_count: tuples.len() as i64,
            created_at: 0,
            status: "ready".into(),
            error: String::new(),
        };
        db.insert_source(&source, &tuples, &embeddings)
            .await
            .expect("store fixture");
    }
}

fn hit(citations: &[Citation], expect: &str) -> bool {
    let needle = expect.to_lowercase();
    citations
        .iter()
        .any(|c| c.snippet.to_lowercase().contains(&needle))
}

/// Recall@k per question kind and overall, for vector-only vs. hybrid.
/// Passing an empty query text to `search_chunks` skips the BM25 side, which
/// gives us the vector-only baseline through the exact same code path.
#[tokio::test]
async fn eval_retrieval_recall() {
    let Some(ai) = builtin_ai().await else { return };
    let dir = std::env::temp_dir().join(format!("nbl-eval-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let nb = "eval-nb";
    seed_corpus(&ai, &db, nb).await;

    const K: usize = 4;
    let mut rows: Vec<(&str, &str, bool, bool)> = Vec::new(); // kind, q, vec, hybrid
    for g in GOLDEN {
        let qvec = ai.embed_one(g.question).await.expect("embed question");
        let vec_only = db
            .search_chunks(nb, qvec.clone(), "", K)
            .await
            .expect("vector search");
        let hybrid = db
            .search_chunks(nb, qvec, g.question, K)
            .await
            .expect("hybrid search");
        rows.push((
            g.kind,
            g.question,
            hit(&vec_only, g.expect),
            hit(&hybrid, g.expect),
        ));
    }

    let recall = |kind: Option<&str>, hybrid: bool| -> (usize, usize) {
        let sel: Vec<_> = rows
            .iter()
            .filter(|r| kind.is_none_or(|k| r.0 == k))
            .collect();
        let hits = sel
            .iter()
            .filter(|r| if hybrid { r.3 } else { r.2 })
            .count();
        (hits, sel.len())
    };

    eprintln!("\nretrieval recall@{K} (hits/total):");
    for kind in ["exact", "paraphrase", "section"] {
        let (vh, vt) = recall(Some(kind), false);
        let (hh, ht) = recall(Some(kind), true);
        eprintln!("  {kind:<11} vector-only {vh}/{vt}   hybrid {hh}/{ht}");
    }
    let (vh, vt) = recall(None, false);
    let (hh, ht) = recall(None, true);
    eprintln!(
        "  {:<11} vector-only {vh}/{vt}   hybrid {hh}/{ht}\n",
        "overall"
    );
    for r in rows.iter().filter(|r| !r.3) {
        eprintln!("  MISS (hybrid): [{}] {}", r.0, r.1);
    }

    // Floors, not aspirations: hybrid must never lag the vector-only baseline,
    // must nail exact identifiers, and must stay above 80% overall. Failures
    // here mean a retrieval regression, not a flaky model — the built-in
    // embedder and BM25 are deterministic for fixed inputs.
    assert!(
        hh >= vh,
        "hybrid recall ({hh}) fell below vector-only ({vh})"
    );
    let (eh, et) = recall(Some("exact"), true);
    assert_eq!(eh, et, "hybrid missed an exact-identifier query");
    assert!(
        hh as f64 / ht as f64 >= 0.8,
        "overall hybrid recall {hh}/{ht} below 0.8 floor"
    );
}

/// Ollama-gated: the distill sub-call must return the load-bearing fact
/// verbatim and compress its input. Skips when no local chat model is up.
#[tokio::test]
async fn eval_distill_quality() {
    let ai = Ai::new(
        AiConfig {
            chat_model: "digitsflow/bonsai-8b:latest".into(),
            ..Default::default()
        },
        AiRuntime::default(),
    );
    if ai.list_models().await.is_err() {
        eprintln!("SKIP: Ollama not reachable on localhost:11434");
        return;
    }

    // The needle sits mid-document surrounded by on-topic filler.
    let filler = "The committee met quarterly to review routine facilities matters. ".repeat(60);
    let doc = format!(
        "{filler}The emergency generator is tested on the third Thursday of every \
         month at 7am, and the test lasts about twenty minutes. {filler}"
    );
    let out = crate::agent::distill(
        &ai,
        "when is the emergency generator tested?",
        "Facilities Minutes",
        &doc,
    )
    .await;

    eprintln!("distill output ({} chars):\n{out}\n", out.chars().count());
    assert!(
        out.to_lowercase().contains("third thursday"),
        "distillate lost the key fact; got: {out}"
    );
    assert!(
        out.chars().count() < doc.chars().count() / 2,
        "distillate did not compress its input"
    );
}

/// Ollama-gated: the reranker must pull an obviously relevant passage buried
/// deep in the pool into the kept set.
#[tokio::test]
async fn eval_rerank_surfaces_buried_hit() {
    let ai = Ai::new(
        AiConfig {
            chat_model: "digitsflow/bonsai-8b:latest".into(),
            ..Default::default()
        },
        AiRuntime::default(),
    );
    if ai.list_models().await.is_err() {
        eprintln!("SKIP: Ollama not reachable on localhost:11434");
        return;
    }

    let mut hits: Vec<Citation> = (0..12)
        .map(|i| Citation {
            chunk_id: format!("d{i}"),
            source_id: format!("s{i}"),
            source_title: "Garden Notes".into(),
            ordinal: i,
            snippet: format!(
                "Entry {i}: tomatoes prefer full sun and weekly deep watering in raised beds."
            ),
            distance: 0.1 + i as f32 * 0.01,
        })
        .collect();
    // The only passage that answers the question, buried at rank 10.
    hits.insert(
        10,
        Citation {
            chunk_id: "needle".into(),
            source_id: "s-needle".into(),
            source_title: "Insurance Policy".into(),
            ordinal: 0,
            snippet: "The homeowner's policy deductible is two thousand five hundred dollars \
                      for wind and hail damage."
                .into(),
            distance: 0.3,
        },
    );

    let kept = crate::agent::rerank(&ai, "what is the deductible for hail damage?", hits).await;
    eprintln!(
        "rerank kept: {:?}",
        kept.iter().map(|c| c.chunk_id.as_str()).collect::<Vec<_>>()
    );
    assert!(
        kept.iter().any(|c| c.chunk_id == "needle"),
        "reranker failed to surface the buried relevant passage"
    );
}
