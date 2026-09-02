//! Retrieval-quality evals: a golden question set over a fixture corpus,
//! measuring recall for vector-only vs. hybrid (vector + BM25) search, plus
//! model-dependent checks for the distillation and rerank sub-calls.
//!
//! The retrieval eval needs only the built-in embedder (downloads ~30 MB on
//! first run, cached afterwards) — no Ollama, so it runs everywhere including
//! CI. The distill/rerank evals need live Ollama and are #[ignore]d: they
//! previously skipped silently when Ollama was down, which meant they passed
//! green on every CI run without measuring anything. Ignored is honest;
//! run them explicitly when touching distill or rerank:
//!
//!   cargo test --lib evals -- --ignored --nocapture
//!
//! Run the CI-safe eval with:  cargo test --lib evals -- --nocapture

use crate::ai::{Ai, AiConfig, AiRuntime};
use crate::db::Db;
use crate::ingest;
use crate::models::Citation;

/// (title, body) fixture documents: prose for paraphrase queries, tables and
/// identifiers for exact-match queries, markdown sections for section queries,
/// and distractors so retrieval has something to get wrong.
pub(crate) const CORPUS: &[(&str, &str)] = &[
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

/// Documents that contradict a golden fact, for the grounded-chat conflict
/// cases in `eval_chat_grounding_across_models` ONLY — never seeded into
/// the retrieval corpus, where a second answer would muddy every metric.
/// Excerpt numbering there is CORPUS then these, so the first is [10].
pub(crate) const CONFLICT_CORPUS: &[(&str, &str)] = &[
    // Never names the value it contradicts — a model must read BOTH
    // excerpts to know there is a conflict at all.
    (
        "Billing Retry Policy (draft, 2025)",
        "# Retries\n\nUnder the revised policy, a request that fails with \
         ERR-503-BACKOFF waits ninety seconds before the next attempt. Shorter \
         waits produced retry storms during the March incident.",
    ),
    (
        "Benefits Update Memo",
        "# Paid Time Off\n\nStarting next fiscal year, employees accrue two days \
         of paid time off per month of service. The prior accrual rate no longer \
         applies to new hires.",
    ),
    // A flat contradiction with no recency cue either way.
    (
        "Media Server Notes",
        "# Ports\n\nPlex answers on port 32469 behind the reverse proxy; the \
         media server has never used the default port.",
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

/// Do the corpus evals get to run?
///
/// The eval suite seeds fixture corpora and embeds every document through the
/// built-in embedder — real work, and the bulk of `cargo test`'s wall clock
/// and CPU (roughly 23s of a 36s run, with the fans to match). It measures
/// retrieval quality rather than guarding correctness, so it does not need to
/// run on every local edit. CI sets `ALCHEMY_EVALS=1`, which is where the
/// numbers actually need watching.
///
///   ALCHEMY_EVALS=1 cargo test --lib -- --nocapture
pub(crate) fn evals_enabled() -> bool {
    flag_set("ALCHEMY_EVALS")
}

/// Do the Ollama-backed tests get to run?
///
/// A handful of tests reach for a live Ollama *whenever one happens to be
/// listening* rather than when someone asked for a model run. On any machine
/// with Ollama up — which is most developer machines — that quietly turns a
/// fast deterministic `cargo test` into a minutes-long model sweep with the
/// fans on. Reachability is not consent, so it takes an explicit opt-in:
///
///   ALCHEMY_OLLAMA_TESTS=1 cargo test --lib -- --nocapture
///
/// Unset (or `0`/`false`/empty), those tests report their deterministic half
/// and skip the model calls.
pub(crate) fn ollama_tests_enabled() -> bool {
    flag_set("ALCHEMY_OLLAMA_TESTS")
}

/// Truthy-env test shared by the opt-in gates: set and not `0`/`false`.
fn flag_set(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => {
            let v = v.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        }
        Err(_) => false,
    }
}

/// A raw Ollama engine for eval use, with the same discipline the app now
/// ships: `num_predict` bounds a runaway reply (one was traced holding the
/// server's single slot for 12k+ tokens, wedging everything behind it — an
/// eval sweep hits the identical failure), and a SHORT residency window so
/// a swept model expires right after its turn instead of stacking in
/// unified memory at Ollama's 5m default while the next model loads.
pub(crate) fn eval_ollama(model: &str) -> crate::ai::Ollama {
    crate::ai::Ollama::new(crate::ai::OllamaConfig {
        base_url: "http://localhost:11434".into(),
        chat_model: model.into(),
        keep_alive: Some("1m".into()),
        num_predict: Some(2_048),
        ..Default::default()
    })
}

/// Release swept models at the end of a live eval — best-effort unloads so
/// the machine comes back to the developer instead of holding 10-20 GB of
/// eval models until their residency windows lapse. "fm" (the sidecar
/// spelling in the planner A/B envs) is skipped — it isn't an Ollama tag.
pub(crate) async fn release_models(models: &[&str]) {
    for m in models {
        if m.is_empty() || m.eq_ignore_ascii_case("fm") {
            continue;
        }
        eval_ollama(m).unload_chat_model().await;
    }
}

/// The embedder for a corpus eval: `builtin_ai`, behind [`evals_enabled`].
/// The `#[ignore]`d evals call `builtin_ai` directly — `--ignored` is already
/// an explicit ask, and gating those twice would make them silently no-op.
pub(crate) async fn eval_ai() -> Option<Ai> {
    if !evals_enabled() {
        eprintln!("SKIP: set ALCHEMY_EVALS=1 to run the corpus evals");
        return None;
    }
    builtin_ai().await
}

pub(crate) async fn builtin_ai() -> Option<Ai> {
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

/// Ingest fixture documents through the real chunk → embed → store path.
/// `id_prefix` keeps ids distinct when seeding multiple document sets into
/// one notebook.
pub(crate) async fn seed_docs(
    ai: &Ai,
    db: &Db,
    notebook_id: &str,
    docs: &[(&str, &str)],
    id_prefix: &str,
) {
    for (i, (title, body)) in docs.iter().enumerate() {
        let extracted = ingest::extract_pasted(title, body).expect("extract fixture");
        let chunks = ingest::chunk_text(&extracted.title, &extracted.text);
        let embed_inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
        let embeddings = ai.embed(&embed_inputs).await.expect("embed fixture");
        let tuples: Vec<(String, i32, String)> = chunks
            .iter()
            .enumerate()
            .map(|(j, c)| (format!("{id_prefix}c{i}-{j}"), j as i32, c.text.clone()))
            .collect();
        let contexts: Vec<String> = chunks.iter().map(|c| c.context.clone()).collect();
        let source = crate::models::Source {
            image_url: String::new(),
            author: String::new(),
            id: format!("{id_prefix}src-{i}"),
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
            parent_id: String::new(),
            mtime: 0,
            tags: String::new(),
            note: String::new(),
            fetched_at: 0,
            fetch_failures: 0,
        };
        db.insert_source_ctx(&source, &tuples, &contexts, &embeddings)
            .await
            .expect("store fixture");
    }
    // Chunk writes only mark the BM25 index dirty now (the app's debounced
    // flusher rebuilds it); tests have no flusher, so seeding flushes.
    db.flush_fts().await.expect("flush fixture fts");
}

/// Ingest the golden fixture corpus.
pub(crate) async fn seed_corpus(ai: &Ai, db: &Db, notebook_id: &str) {
    seed_docs(ai, db, notebook_id, CORPUS, "").await;
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
    let Some(ai) = eval_ai().await else { return };
    let dir = std::env::temp_dir().join(format!("nbl-eval-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let nb = "eval-nb";
    seed_corpus(&ai, &db, nb).await;

    const K: usize = 4;
    let mut rows: Vec<(&str, &str, bool, bool)> = Vec::new(); // kind, q, vec, hybrid
    for g in GOLDEN {
        let qvec = ai.embed_one(g.question).await.expect("embed question");
        let vec_only = db
            .search_chunks(nb, qvec.clone(), "", K, None)
            .await
            .expect("vector search");
        let hybrid = db
            .search_chunks(nb, qvec, g.question, K, None)
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
#[ignore = "needs live Ollama; run explicitly — the silent skip made this pass green on CI without measuring anything"]
async fn eval_distill_quality() {
    // small_model, not just chat_model: distill runs on the Small role in
    // the app, and that engine carries the runaway cap + residency window —
    // the eval must exercise the same engine the loop does.
    let ai = Ai::new(
        AiConfig {
            chat_model: "digitsflow/bonsai-8b:latest".into(),
            small_model: "digitsflow/bonsai-8b:latest".into(),
            ..Default::default()
        },
        AiRuntime::default(),
    );
    // This test only runs when explicitly requested (#[ignore]), so a
    // missing Ollama is a failed precondition, not a skip — the silent
    // return here is what let it pass green for months without measuring.
    assert!(
        ai.list_models().await.is_ok(),
        "Ollama not reachable on localhost:11434 — start it, then rerun"
    );

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
    release_models(&["digitsflow/bonsai-8b:latest"]).await;
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
#[ignore = "needs live Ollama; run explicitly — the silent skip made this pass green on CI without measuring anything"]
async fn eval_rerank_surfaces_buried_hit() {
    // small_model too — the loop's rerank call runs on the Small role, and
    // only that engine carries the runaway cap + residency window.
    let ai = Ai::new(
        AiConfig {
            chat_model: "digitsflow/bonsai-8b:latest".into(),
            small_model: "digitsflow/bonsai-8b:latest".into(),
            ..Default::default()
        },
        AiRuntime::default(),
    );
    // This test only runs when explicitly requested (#[ignore]), so a
    // missing Ollama is a failed precondition, not a skip — the silent
    // return here is what let it pass green for months without measuring.
    assert!(
        ai.list_models().await.is_ok(),
        "Ollama not reachable on localhost:11434 — start it, then rerun"
    );

    let mut hits: Vec<Citation> = (0..12)
        .map(|i| Citation {
            chunk_id: format!("d{i}"),
            source_id: format!("s{i}"),
            source_title: "Garden Notes".into(),
            source_path: String::new(),
            note_id: String::new(),
            gist: false,
            snote: false,
            ordinal: i,
            snippet: format!(
                "Entry {i}: tomatoes prefer full sun and weekly deep watering in raised beds."
            ),
            distance: 0.1 + i as f32 * 0.01,
            section: String::new(),
        })
        .collect();
    // The only passage that answers the question, buried at rank 10.
    hits.insert(
        10,
        Citation {
            chunk_id: "needle".into(),
            source_id: "s-needle".into(),
            source_title: "Insurance Policy".into(),
            source_path: String::new(),
            note_id: String::new(),
            gist: false,
            snote: false,
            ordinal: 0,
            snippet: "The homeowner's policy deductible is two thousand five hundred dollars \
                      for wind and hail damage."
                .into(),
            distance: 0.3,
            section: String::new(),
        },
    );

    let kept = crate::agent::rerank(&ai, "what is the deductible for hail damage?", hits).await;
    release_models(&["digitsflow/bonsai-8b:latest"]).await;
    eprintln!(
        "rerank kept: {:?}",
        kept.iter().map(|c| c.chunk_id.as_str()).collect::<Vec<_>>()
    );
    assert!(
        kept.iter().any(|c| c.chunk_id == "needle"),
        "reranker failed to surface the buried relevant passage"
    );
}

/// Ollama-gated: the grounded-chat contract across candidate default chat
/// models. Each model gets the fixture corpus as numbered excerpts through
/// the real prompt (`rag::build_chat_messages`) and must state the fact AND
/// carry bracketed markers `verify::strip_markers` parses — the contract a
/// model has to clear before `ai::recommended_chat_model` may name it,
/// because switching model families is exactly when citation style breaks.
///
/// Models come from ALCHEMY_EVAL_CHAT_MODELS (comma-separated Ollama tags);
/// unset, it evaluates the tier table's picks that are actually installed.
#[tokio::test]
#[ignore = "needs live Ollama; run explicitly — this measures candidate models, it does not guard correctness"]
async fn eval_chat_grounding_across_models() {
    use crate::ai::{Ollama, OllamaConfig};

    let probe = Ollama::new(OllamaConfig {
        base_url: "http://localhost:11434".into(),
        ..Default::default()
    });
    let installed = probe
        .list_models()
        .await
        .expect("Ollama not reachable on localhost:11434 — start it, then rerun");

    let models: Vec<String> = match std::env::var("ALCHEMY_EVAL_CHAT_MODELS") {
        Ok(v) if !v.trim().is_empty() => v.split(',').map(|s| s.trim().to_string()).collect(),
        _ => [8u64, 16, 24, 32, 96, 192]
            .iter()
            .map(|gib| crate::ai::recommended_chat_model(*gib).to_string())
            .filter(|m| installed.iter().any(|i| i == m))
            .collect(),
    };
    assert!(
        !models.is_empty(),
        "no candidate models installed — pull one or set ALCHEMY_EVAL_CHAT_MODELS"
    );

    // (question, any-of answer substrings, 1-based excerpt holding the fact)
    let qa: &[(&str, &[&str], usize)] = &[
        (
            "what should happen after an ERR-503-BACKOFF error?",
            &["sixty", "60 second"],
            1,
        ),
        ("what service uses port 32400?", &["plex"], 2),
        (
            "how much paid time off do employees accrue per month?",
            &["one and a half", "1.5"],
            4,
        ),
        (
            "how hot should the dutch oven be preheated for bread?",
            &["four hundred fifty", "450"],
            5,
        ),
    ];

    // Conflict cases (docs: Reminders "surface contradictions"): two excerpts
    // disagree on the asked-for fact. Grounded behavior is to surface BOTH
    // values and cite both excerpts, not to silently pick one. Each entry is
    // (question, both values that must appear, the two 1-based excerpts).
    let conflicts: &[(&str, [&str; 2], [usize; 2])] = &[
        (
            "what is the wait after an ERR-503-BACKOFF before retrying?",
            ["sixty", "ninety"],
            [1, 10],
        ),
        (
            "how much paid time off do employees accrue per month?",
            ["one and a half", "two days"],
            [4, 11],
        ),
        ("what port does Plex use?", ["32400", "32469"], [2, 12]),
    ];

    let corpus: Vec<(&str, &str)> = CORPUS
        .iter()
        .copied()
        .chain(CONFLICT_CORPUS.iter().copied())
        .collect();
    let citations: Vec<Citation> = corpus
        .iter()
        .enumerate()
        .map(|(i, (title, body))| Citation {
            chunk_id: format!("doc{i}"),
            source_id: format!("src{i}"),
            source_title: title.to_string(),
            source_path: String::new(),
            note_id: String::new(),
            gist: false,
            snote: false,
            ordinal: i as i32,
            snippet: body.to_string(),
            distance: 0.1,
            section: String::new(),
        })
        .collect();
    let sources: Vec<(String, String, String)> = corpus
        .iter()
        .map(|(title, _)| (title.to_string(), String::new(), String::new()))
        .collect();
    let no_expansion = std::collections::HashMap::new();

    let mut failures: Vec<String> = Vec::new();
    for model in &models {
        // The eval-tier engine: capped output (a rambling candidate must
        // not wedge the whole sweep) and a short residency window.
        let ai = eval_ollama(model);
        let (mut facts, mut marked, mut cited, mut tok_s) = (0, 0, 0, Vec::new());
        for (question, expects, excerpt) in qa {
            let messages = crate::rag::build_chat_messages(
                &[],
                question,
                crate::rag::Excerpts {
                    citations: &citations,
                    expanded: &no_expansion,
                },
                &sources,
                "",
                "",
                &crate::inference::ContextProfile::default(),
            );
            let out = ai.chat(&messages).await.expect("chat");
            if let Some(s) = &out.stats {
                tok_s.push(s.tokens_per_sec());
            }
            let lower = out.text.to_lowercase();
            let (_, markers) = crate::verify::strip_markers(&out.text);
            facts += expects.iter().any(|e| lower.contains(e)) as u32;
            marked += !markers.is_empty() as u32;
            cited += markers.contains(excerpt) as u32;
        }
        // Conflicts: both values named AND both excerpts cited. "One-sided"
        // counts answers that picked a side silently — the failure mode the
        // disagreement rule in CHAT_SYSTEM exists to prevent.
        let (mut both_values, mut both_cited, mut one_sided) = (0, 0, 0);
        for (question, values, excerpts) in conflicts {
            let messages = crate::rag::build_chat_messages(
                &[],
                question,
                crate::rag::Excerpts {
                    citations: &citations,
                    expanded: &no_expansion,
                },
                &sources,
                "",
                "",
                &crate::inference::ContextProfile::default(),
            );
            let out = ai.chat(&messages).await.expect("chat");
            let lower = out.text.to_lowercase();
            let (_, markers) = crate::verify::strip_markers(&out.text);
            let values_hit = values.iter().filter(|v| lower.contains(*v)).count();
            let cited_hit = excerpts.iter().filter(|e| markers.contains(e)).count();
            both_values += (values_hit == 2) as u32;
            both_cited += (cited_hit == 2) as u32;
            one_sided += (values_hit == 1) as u32;
            let head: String = out.text.chars().take(240).collect();
            eprintln!(
                "  conflict {question:?}: values {values_hit}/2, excerpts cited {cited_hit}/2\n    {}",
                head.replace('\n', " ")
            );
        }
        // Release this candidate before the next loads — a sweep used to
        // stack every swept model in unified memory for 5 minutes each.
        ai.unload_chat_model().await;
        let speed = tok_s.iter().sum::<f64>() / tok_s.len().max(1) as f64;
        eprintln!(
            "{model}: facts {facts}/{n}, cited-at-all {marked}/{n}, cited-right-excerpt {cited}/{n}, \
             conflicts both-values {both_values}/{c}, both-cited {both_cited}/{c}, one-sided {one_sided}/{c}, \
             {speed:.0} tok/s",
            n = qa.len(),
            c = conflicts.len()
        );
        // A default has to ground reliably: near-perfect facts with markers
        // present. Right-excerpt is reported, not gated — several excerpts
        // legitimately touch some answers.
        if facts < 3 || marked < 3 {
            failures.push(format!("{model} (facts {facts}/4, markers {marked}/4)"));
        }
    }
    assert!(
        failures.is_empty(),
        "models below the grounded-chat floor: {}",
        failures.join(", ")
    );
}
