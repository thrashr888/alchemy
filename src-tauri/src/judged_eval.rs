//! Judged end-to-end answer evals (docs/RFC-judged-evals.md) — the layer
//! above `beir_eval.rs`: instead of scoring retrieval, run the FULL chat
//! chain (retrieve → xenc rerank → grounded prompt → generate → cite) and
//! score the answer itself. Phase 1+2: deterministic L0 requirements and
//! the cross-encoder as an on-device faithfulness verifier (L1). The
//! sampled LLM judge (L2) exists to calibrate L1's threshold and comes
//! next.
//!
//!   cargo test --lib judged_ -- --ignored --nocapture
//!
//! Env knobs (targeted-run discipline): JUDGED_SUITES ("scifact_claims,
//! nano_hotpot,unanswerable"), JUDGED_ENGINE ("bonsai"|"fm"|"codex"),
//! JUDGED_SAMPLE (default 25), JUDGED_SUPPORT_THRESHOLD (default 0.0,
//! uncalibrated until L2). Results append to ~/alchemy-benchmarks.csv.

use std::collections::HashMap;

use crate::ai::Ai;
use crate::beir_eval::{seeded_dataset, EmbedStyle, SeededDataset};
use crate::evals::builtin_ai;
use crate::inference::rerank::{CrossEncoder, XencModel};
use crate::models::Citation;

const DEFAULT_SAMPLE: usize = 25;
/// Retrieval depth for the answer chain — the MCP search default, small
/// enough that every excerpt plausibly matters.
const K: usize = 6;

/// Questions the SciFact corpus provably cannot answer — non-biomedical
/// topics verified absent. Right behavior: say so, cite nothing.
const UNANSWERABLE: &[&str] = &[
    "Who won the 2018 FIFA World Cup final?",
    "What is the capital of Australia?",
    "How do I make a proper French omelette?",
    "What year did the Apollo 11 mission land on the moon?",
    "Which composer wrote the Goldberg Variations?",
    "What is the maximum depth of the Mariana Trench?",
    "Who painted the ceiling of the Sistine Chapel?",
    "What programming language was Linux originally written in?",
    "How tall is the Eiffel Tower?",
    "What is the airspeed of an unladen European swallow?",
];

struct Question {
    text: String,
    /// Gold evidence doc ids; empty = the corpus cannot answer this.
    gold: Vec<String>,
    /// Multi-hop: require EVERY gold doc cited, not just one.
    all_gold: bool,
}

/// One scored answer.
struct Scored {
    answered: bool,
    /// Markers that resolve to a shown excerpt / all markers used.
    citation_validity: f64,
    /// Any gold doc cited (or all, when `all_gold`).
    gold_recall: Option<bool>,
    /// L1: supported cited sentences / cited sentences. None when the
    /// answer had no cited sentences to score.
    faithfulness: Option<f64>,
    /// Cited sentences / all sentences — how much of the answer even
    /// claims grounding.
    coverage: f64,
    tokens: u64,
    ms: f64,
}

/// Extract `[n]` markers from one sentence and return the sentence with
/// markers stripped. Hand-rolled — not worth a regex dependency.
fn strip_markers(s: &str) -> (String, Vec<usize>) {
    let (mut clean, mut markers) = (String::new(), Vec::new());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let (before, bracketed) = rest.split_at(open);
        clean.push_str(before);
        match bracketed[1..].find(']') {
            Some(close) if bracketed[1..close + 1].chars().all(|c| c.is_ascii_digit()) => {
                if let Ok(n) = bracketed[1..close + 1].parse::<usize>() {
                    markers.push(n);
                }
                rest = &bracketed[close + 2..];
            }
            _ => {
                clean.push('[');
                rest = &bracketed[1..];
            }
        }
    }
    clean.push_str(rest);
    (clean.trim().to_string(), markers)
}

/// Sentences with the 1-based excerpt markers each one carries.
fn cited_sentences(answer: &str) -> Vec<(String, Vec<usize>)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in answer.chars() {
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') {
            let s = cur.trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
            cur.clear();
        }
    }
    let s = cur.trim().to_string();
    if !s.is_empty() {
        out.push(s);
    }
    out.into_iter().map(|s| strip_markers(&s)).collect()
}

/// Run one question through the real answer chain and score it.
#[allow(clippy::too_many_arguments)]
async fn score_question(
    q: &Question,
    retrieval: &Ai,
    generator: &Ai,
    db: &crate::db::Db,
    corpus_name: &str,
    xenc: &CrossEncoder,
    threshold: f32,
) -> Option<Scored> {
    let qvec = retrieval.embed_one(&q.text).await.ok()?;
    let fetch_k = if retrieval.has_xenc() { K * 3 } else { K };
    let trace = db
        .search_chunks_trace(corpus_name, qvec, &q.text, fetch_k, None)
        .await
        .ok()?;
    let hits: Vec<Citation> = retrieval.rerank_hits(&q.text, trace.final_hits, K).await;

    let profile = generator.profile(crate::inference::Role::Chat);
    let messages = crate::rag::build_chat_messages(
        &[],
        &q.text,
        crate::rag::Excerpts {
            citations: &hits,
            expanded: &HashMap::new(),
        },
        &[],
        "",
        "",
        &profile,
    );
    let t0 = std::time::Instant::now();
    let out = generator.chat(&messages).await.ok()?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let tokens = out.stats.as_ref().map(|s| s.eval_count).unwrap_or(0);

    let sentences = cited_sentences(&out.text);
    let all_markers: Vec<usize> = sentences.iter().flat_map(|(_, m)| m.clone()).collect();
    let valid = all_markers
        .iter()
        .filter(|&&m| m >= 1 && m <= hits.len())
        .count();
    let citation_validity = if all_markers.is_empty() {
        1.0
    } else {
        valid as f64 / all_markers.len() as f64
    };
    let answered = !all_markers.is_empty();

    let cited_docs: Vec<&str> = all_markers
        .iter()
        .filter(|&&m| m >= 1 && m <= hits.len())
        .map(|&m| hits[m - 1].source_id.as_str())
        .collect();
    let gold_recall = (!q.gold.is_empty()).then(|| {
        if q.all_gold {
            q.gold.iter().all(|g| cited_docs.contains(&g.as_str()))
        } else {
            q.gold.iter().any(|g| cited_docs.contains(&g.as_str()))
        }
    });

    // L1: a cited sentence is supported when the cross-encoder scores it
    // above threshold against AT LEAST ONE of the excerpts it cites.
    let (mut supported, mut scored) = (0usize, 0usize);
    for (sentence, markers) in &sentences {
        let cited: Vec<String> = markers
            .iter()
            .filter(|&&m| m >= 1 && m <= hits.len())
            .map(|&m| hits[m - 1].snippet.clone())
            .collect();
        if cited.is_empty() || sentence.split_whitespace().count() < 4 {
            continue;
        }
        if let Ok(scores) = xenc.scores(sentence, &cited).await {
            scored += 1;
            if scores.iter().any(|&s| s > threshold) {
                supported += 1;
            }
        }
    }
    let faithfulness = (scored > 0).then(|| supported as f64 / scored as f64);
    let coverage = if sentences.is_empty() {
        0.0
    } else {
        sentences.iter().filter(|(_, m)| !m.is_empty()).count() as f64 / sentences.len() as f64
    };

    Some(Scored {
        answered,
        citation_validity,
        gold_recall,
        faithfulness,
        coverage,
        tokens,
        ms,
    })
}

/// Build a suite's question list, deterministic (sorted qids, first N).
async fn suite_questions(
    suite: &str,
    retrieval: &Ai,
    sample: usize,
) -> Option<(SeededDataset, String, Vec<Question>)> {
    match suite {
        "scifact_claims" => {
            let ds = seeded_dataset("scifact", retrieval, EmbedStyle::default(), "builtin").await?;
            let mut qids: Vec<&String> = ds.qrels.keys().collect();
            qids.sort();
            let qs = qids
                .into_iter()
                .filter_map(|qid| {
                    let claim = ds.queries.get(qid)?;
                    let mut gold: Vec<String> = ds.qrels[qid].keys().cloned().collect();
                    gold.sort();
                    Some(Question {
                        text: format!(
                            "Is the following claim supported or refuted by the sources? \
                             Claim: {claim}"
                        ),
                        gold,
                        all_gold: false,
                    })
                })
                .take(sample)
                .collect();
            Some((ds, "scifact".into(), qs))
        }
        "nano_hotpot" => {
            let ds =
                seeded_dataset("NanoHotpotQA", retrieval, EmbedStyle::default(), "builtin").await?;
            let mut qids: Vec<&String> = ds.qrels.keys().collect();
            qids.sort();
            let qs = qids
                .into_iter()
                .filter_map(|qid| {
                    let text = ds.queries.get(qid)?.clone();
                    let mut gold: Vec<String> = ds.qrels[qid].keys().cloned().collect();
                    // Multi-hop questions need both evidence docs.
                    if gold.len() < 2 {
                        return None;
                    }
                    gold.sort();
                    Some(Question {
                        text,
                        gold,
                        all_gold: true,
                    })
                })
                .take(sample)
                .collect();
            Some((ds, "NanoHotpotQA".into(), qs))
        }
        "unanswerable" => {
            let ds = seeded_dataset("scifact", retrieval, EmbedStyle::default(), "builtin").await?;
            let qs = UNANSWERABLE
                .iter()
                .take(sample)
                .map(|q| Question {
                    text: (*q).to_string(),
                    gold: Vec::new(),
                    all_gold: false,
                })
                .collect();
            Some((ds, "scifact".into(), qs))
        }
        other => {
            eprintln!("SKIP: unknown suite {other}");
            None
        }
    }
}

fn mean(vals: impl Iterator<Item = f64>) -> f64 {
    let v: Vec<f64> = vals.collect();
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// The baseline matrix: every suite through the full chain, one engine.
#[tokio::test]
#[ignore = "live models: full answer chain per question — run with --ignored --nocapture"]
async fn judged_baseline() {
    let Some(retrieval) = builtin_ai().await else {
        return;
    };
    let engine_name = std::env::var("JUDGED_ENGINE").unwrap_or_else(|_| "bonsai".into());
    let generator = match engine_name.as_str() {
        "fm" => crate::beir_eval::fm_ai(),
        "codex" => crate::beir_eval::codex_ai(),
        _ => crate::beir_eval::rerank_ai().await,
    };
    let Some(generator) = generator else {
        eprintln!("SKIP: engine {engine_name} unavailable");
        return;
    };
    let resolved = generator.chat_engine_id(crate::inference::Role::Chat);
    let sample: usize = std::env::var("JUDGED_SAMPLE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SAMPLE);
    let threshold: f32 = std::env::var("JUDGED_SUPPORT_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let suites = std::env::var("JUDGED_SUITES")
        .unwrap_or_else(|_| "scifact_claims,nano_hotpot,unanswerable".into());
    let xenc_cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/beir-cache");
    let xenc = CrossEncoder::new(xenc_cache, XencModel::Small);
    let today = chrono::Local::now().format("%Y-%m-%d");

    for suite in suites.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((ds, corpus_name, questions)) = suite_questions(suite, &retrieval, sample).await
        else {
            continue;
        };
        let mut results: Vec<Scored> = Vec::new();
        for q in &questions {
            if let Some(s) = score_question(
                q,
                &retrieval,
                &generator,
                &ds.db,
                &corpus_name,
                &xenc,
                threshold,
            )
            .await
            {
                results.push(s);
            }
        }
        if results.is_empty() {
            eprintln!("\nJUDGED {suite} [{resolved}] — no results (engine down?)");
            continue;
        }
        let n = results.len();
        let answered = mean(results.iter().map(|r| r.answered as u8 as f64));
        let validity = mean(results.iter().map(|r| r.citation_validity));
        let recall = results.iter().filter_map(|r| r.gold_recall).count();
        let recall_ok = results
            .iter()
            .filter_map(|r| r.gold_recall)
            .filter(|&b| b)
            .count();
        let faith = mean(results.iter().filter_map(|r| r.faithfulness));
        let coverage = mean(results.iter().map(|r| r.coverage));
        let tokens = mean(results.iter().map(|r| r.tokens as f64));
        let ms = mean(results.iter().map(|r| r.ms));
        // The unanswerable suite inverts "answered": citing excerpts for a
        // question the corpus can't answer IS the failure.
        let headline = if suite == "unanswerable" {
            format!("abstained {:.0}%", (1.0 - answered) * 100.0)
        } else if recall > 0 {
            format!(
                "gold cited {recall_ok}/{recall} ({:.0}%)",
                recall_ok as f64 / recall as f64 * 100.0
            )
        } else {
            String::new()
        };
        eprintln!(
            "\nJUDGED {suite} [{resolved}] — {n} questions\n  \
             {headline}\n  \
             answered {:.0}%   citation validity {:.2}   faithfulness(L1) {:.2}   \
             coverage {:.2}\n  \
             mean {tokens:.0} tokens   {ms:.0} ms/answer",
            answered * 100.0,
            validity,
            faith,
            coverage,
        );
        // The ledger row — same file as the retrieval results.
        let home = std::env::var("HOME").unwrap_or_default();
        let row = format!(
            "{today},{suite},builtin,judged {resolved},{faith:.4},,{ms:.0},{n},\
             answered {:.0}% validity {validity:.2} {}\n",
            answered * 100.0,
            headline.replace(',', ";")
        );
        let _ = std::fs::OpenOptions::new()
            .append(true)
            .open(format!("{home}/alchemy-benchmarks.csv"))
            .and_then(|mut f| std::io::Write::write_all(&mut f, row.as_bytes()));
    }
}
