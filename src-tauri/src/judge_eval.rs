//! Judge benchmark (docs/RFC-typesafe-jev.md): the same labeled claim and
//! excerpt cases through every way Second Look can judge a claim, so the
//! typed path is measured against what runs without it.
//!
//! - **baseline**: the Small-role prompt-and-parse (`judge_small`) on an
//!   Ollama model — three strict lines, malformed replies are unjudged. This
//!   is the "without" column.
//! - **ollama**: the typed judge on the same model — schema-constrained
//!   decoding plus logprobs, a margin confidence per verdict.
//! - **jev**: TypeSafe's judge, when a key resolves.
//! - **fm**: the baseline on the Foundation Models sidecar — what Paul's
//!   own configuration runs today (`JUDGE_FM=1`).
//!
//! Forty cases, ten per class, each with a distractor excerpt
//! (`fixtures/judge_verdicts.json`). Accuracy counts unjudged as wrong,
//! because that is what the report shows the reader. The confidence columns
//! are the point of the exercise: a baseline cannot say it is unsure, so
//! every one of its errors is asserted; a typed judge's errors that fall
//! under the review threshold are flagged instead.
//!
//!   ALCHEMY_OLLAMA_TESTS=1 ALCHEMY_JEV_TESTS=1 JUDGE_MODELS=qwen3.8:27b-mlx,gemma4:12b-mlx \
//!     cargo test --lib eval_judge_verdicts -- --ignored --nocapture
//!
//! Rows append to ~/alchemy-benchmarks.csv.

use std::time::Instant;

use crate::ai::{Ai, AiConfig, AiRuntime};
use crate::commands::{judge_small, judge_typed, ClaimVerdict};
use crate::inference::judge::Judge;
use crate::models::Citation;

/// Mirrors `second_look::REVIEW_BELOW`: verdicts under it are flagged.
const REVIEW_BELOW: f64 = 0.7;
const PER_CASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

#[derive(serde::Deserialize)]
struct Excerpt {
    source: String,
    text: String,
}

#[derive(serde::Deserialize)]
struct Case {
    id: String,
    gold: String,
    claim: String,
    excerpts: Vec<Excerpt>,
}

impl Case {
    fn citations(&self) -> Vec<Citation> {
        self.excerpts
            .iter()
            .enumerate()
            .map(|(i, e)| Citation {
                chunk_id: format!("{}-{i}", self.id),
                source_id: String::new(),
                source_title: e.source.clone(),
                source_path: String::new(),
                note_id: String::new(),
                gist: false,
                section: String::new(),
                snote: false,
                ordinal: i as i32,
                snippet: e.text.clone(),
                distance: 0.0,
            })
            .collect()
    }
}

fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!("../fixtures/judge_verdicts.json"))
        .expect("fixtures/judge_verdicts.json parses")
}

struct Outcome {
    id: String,
    gold: String,
    got: String,
    confidence: Option<f64>,
    ms: u128,
}

#[derive(Default)]
struct Summary {
    n: usize,
    correct: usize,
    unjudged: usize,
    median_ms: u128,
    p90_ms: u128,
    /// Typed judges only.
    confident: usize,
    confident_correct: usize,
    errors: usize,
    errors_flagged: usize,
    misses: Vec<String>,
}

fn summarize(outcomes: &[Outcome]) -> Summary {
    let mut s = Summary {
        n: outcomes.len(),
        ..Default::default()
    };
    let mut lat: Vec<u128> = outcomes.iter().map(|o| o.ms).collect();
    lat.sort_unstable();
    if !lat.is_empty() {
        s.median_ms = lat[lat.len() / 2];
        s.p90_ms = lat[(lat.len() * 9 / 10).min(lat.len() - 1)];
    }
    for o in outcomes {
        let ok = o.got == o.gold;
        if ok {
            s.correct += 1;
        } else {
            s.errors += 1;
            s.misses.push(format!("{}:{}→{}", o.id, o.gold, o.got));
        }
        if o.got == "unjudged" {
            s.unjudged += 1;
        }
        if let Some(c) = o.confidence {
            if c >= REVIEW_BELOW {
                s.confident += 1;
                if ok {
                    s.confident_correct += 1;
                }
            } else if !ok {
                s.errors_flagged += 1;
            }
        }
    }
    s
}

fn pct(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64 * 100.0
    }
}

fn outcome(case: &Case, verdict: ClaimVerdict, started: Instant) -> Outcome {
    Outcome {
        id: case.id.clone(),
        gold: case.gold.clone(),
        got: verdict.verdict,
        confidence: verdict.confidence,
        ms: started.elapsed().as_millis(),
    }
}

async fn run_baseline(ai: &Ai, cases: &[Case]) -> Vec<Outcome> {
    let mut out = Vec::with_capacity(cases.len());
    for case in cases {
        let started = Instant::now();
        let cites = case.citations();
        let verdict = match tokio::time::timeout(
            PER_CASE_TIMEOUT,
            judge_small(ai, &case.claim, &cites),
        )
        .await
        {
            Ok(v) => v,
            Err(_) => ClaimVerdict {
                claim: case.claim.clone(),
                verdict: "unjudged".into(),
                reason: "timeout".into(),
                evidence_title: String::new(),
                evidence_snippet: String::new(),
                confidence: None,
            },
        };
        out.push(outcome(case, verdict, started));
    }
    out
}

async fn run_typed(judge: &Judge, cases: &[Case]) -> Vec<Outcome> {
    let mut out = Vec::with_capacity(cases.len());
    for case in cases {
        let started = Instant::now();
        let cites = case.citations();
        let verdict =
            match tokio::time::timeout(PER_CASE_TIMEOUT, judge_typed(judge, &case.claim, &cites))
                .await
            {
                Ok(Ok(v)) => v,
                Ok(Err(err)) => {
                    eprintln!("  {} {}: {err:#}", judge.label(), case.id);
                    ClaimVerdict {
                        claim: case.claim.clone(),
                        verdict: "unjudged".into(),
                        reason: String::new(),
                        evidence_title: String::new(),
                        evidence_snippet: String::new(),
                        confidence: None,
                    }
                }
                Err(_) => ClaimVerdict {
                    claim: case.claim.clone(),
                    verdict: "unjudged".into(),
                    reason: "timeout".into(),
                    evidence_title: String::new(),
                    evidence_snippet: String::new(),
                    confidence: None,
                },
            };
        out.push(outcome(case, verdict, started));
    }
    out
}

fn report(label: &str, s: &Summary) -> String {
    let confidence = if s.confident > 0 || s.errors_flagged > 0 {
        format!(
            " | confident {:>3}/{:<3} acc {:>5.1}% | errors flagged {:>2}/{:<2}",
            s.confident_correct,
            s.confident,
            pct(s.confident_correct, s.confident),
            s.errors_flagged,
            s.errors
        )
    } else {
        format!(
            " | (no confidence)                   | errors flagged  0/{:<2}",
            s.errors
        )
    };
    format!(
        "{label:<28} acc {:>5.1}% ({}/{}) unjudged {:>2} | median {:>5} ms p90 {:>5} ms{confidence}",
        pct(s.correct, s.n),
        s.correct,
        s.n,
        s.unjudged,
        s.median_ms,
        s.p90_ms
    )
}

fn append_csv(label: &str, s: &Summary) {
    let home = std::env::var("HOME").unwrap_or_default();
    let today = chrono::Local::now().format("%Y-%m-%d");
    let row = format!(
        "{today},judge,{label},second-look verdicts,{:.4},,,{},unjudged={} median_ms={} p90_ms={} confident={}/{} confident_acc={:.3} errors_flagged={}/{}\n",
        s.correct as f64 / s.n.max(1) as f64,
        s.n,
        s.unjudged,
        s.median_ms,
        s.p90_ms,
        s.confident_correct,
        s.confident,
        pct(s.confident_correct, s.confident) / 100.0,
        s.errors_flagged,
        s.errors
    );
    let _ = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(format!("{home}/alchemy-benchmarks.csv"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, row.as_bytes()));
}

fn ollama_ai(model: &str) -> Ai {
    Ai::new(
        AiConfig {
            embedder: "builtin".into(),
            chat_model: model.to_string(),
            // The Small role is what Second Look's baseline runs on; set it
            // explicitly so the baseline carries production's Small-engine
            // settings (no thinking, bounded output, held warm).
            small_model: model.to_string(),
            ..Default::default()
        },
        AiRuntime::default(),
    )
}

/// The judge matrix over the labeled verdict cases.
#[tokio::test]
#[ignore = "live models — run with --ignored --nocapture and the ALCHEMY_*_TESTS flags"]
async fn eval_judge_verdicts() {
    let cases = cases();
    let ollama_on = crate::evals::ollama_tests_enabled();
    let jev_on = std::env::var("ALCHEMY_JEV_TESTS").is_ok();
    if !ollama_on && !jev_on {
        eprintln!("SKIP: set ALCHEMY_OLLAMA_TESTS=1 and/or ALCHEMY_JEV_TESTS=1");
        return;
    }
    let models: Vec<String> = std::env::var("JUDGE_MODELS")
        .unwrap_or_else(|_| "qwen3.8:27b-mlx".into())
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    let only: Option<String> = std::env::var("JUDGE_ONLY").ok(); // baseline | ollama | jev | fm
    let runs = |which: &str| only.as_deref().is_none_or(|o| o == which);
    let base_url = crate::ai::ollama_config(&AiConfig::default()).base_url;

    eprintln!(
        "\n{} verdict cases, review threshold {REVIEW_BELOW}\n",
        cases.len()
    );
    let mut lines = Vec::new();
    let mut emit = |label: String, outcomes: &[Outcome]| {
        let s = summarize(outcomes);
        let line = report(&label, &s);
        eprintln!("{line}");
        if !s.misses.is_empty() {
            eprintln!("    misses: {}", s.misses.join("  "));
        }
        append_csv(&label, &s);
        lines.push(line);
    };

    if ollama_on {
        for model in &models {
            if runs("baseline") {
                let ai = ollama_ai(model);
                let outcomes = run_baseline(&ai, &cases).await;
                emit(format!("baseline:{model}"), &outcomes);
            }
            if runs("ollama") {
                let judge = Judge::ollama(&base_url, model);
                let outcomes = run_typed(&judge, &cases).await;
                emit(judge.label(), &outcomes);
            }
        }
    }
    if ollama_on && runs("fm") && std::env::var("JUDGE_FM").is_ok() {
        match crate::beir_eval::fm_ai() {
            Some(ai) => {
                let outcomes = run_baseline(&ai, &cases).await;
                emit("baseline:fm".into(), &outcomes);
            }
            None => eprintln!("baseline:fm skipped (no sidecar)"),
        }
    }
    if jev_on && runs("jev") {
        match Judge::jev() {
            Some(judge) => {
                let outcomes = run_typed(&judge, &cases).await;
                emit(judge.label(), &outcomes);
            }
            None => eprintln!("jev skipped (no credential)"),
        }
    }
    eprintln!("\n{}\n", lines.join("\n"));
}

// ---- Tool gate -----------------------------------------------------------

#[derive(serde::Deserialize)]
struct RouteCase {
    id: String,
    surface: String,
    gold: String,
    msg: String,
}

fn route_cases() -> Vec<RouteCase> {
    serde_json::from_str(include_str!("../fixtures/judge_routes.json"))
        .expect("fixtures/judge_routes.json parses")
}

struct RouteOutcome {
    id: String,
    gold: String,
    got: String,
    confidence: f64,
    ms: u128,
}

/// What the gate's policy would do with each answer, split by the two
/// failures that matter: a command judged `none` (a silent drop if the
/// loop were skipped) and a question judged a tool (a wasted hint).
#[derive(Default)]
struct RouteSummary {
    n: usize,
    correct: usize,
    median_ms: u128,
    /// gold ≠ none, got == none, confident: the skip policy would drop it.
    drops_confident: usize,
    /// gold ≠ none, got == none, under threshold: the loop still runs.
    drops_caught: usize,
    /// gold == none, got a tool, confident: a wrong hint.
    wrong_hints: usize,
    /// Correct answers the policy acts on (confident).
    acted_correct: usize,
    acted: usize,
    misses: Vec<String>,
}

fn summarize_routes(outcomes: &[RouteOutcome]) -> RouteSummary {
    let mut s = RouteSummary {
        n: outcomes.len(),
        ..Default::default()
    };
    let mut lat: Vec<u128> = outcomes.iter().map(|o| o.ms).collect();
    lat.sort_unstable();
    if !lat.is_empty() {
        s.median_ms = lat[lat.len() / 2];
    }
    for o in outcomes {
        let ok = o.got == o.gold;
        let confident = o.confidence >= REVIEW_BELOW;
        if ok {
            s.correct += 1;
        } else {
            s.misses
                .push(format!("{}:{}→{}@{:.2}", o.id, o.gold, o.got, o.confidence));
        }
        if confident {
            s.acted += 1;
            if ok {
                s.acted_correct += 1;
            }
        }
        match (o.gold.as_str(), o.got.as_str()) {
            (g, "none") if g != "none" => {
                if confident {
                    s.drops_confident += 1
                } else {
                    s.drops_caught += 1
                }
            }
            ("none", t) if t != "none" && confident => s.wrong_hints += 1,
            _ => {}
        }
    }
    s
}

fn report_routes(label: &str, s: &RouteSummary) -> String {
    format!(
        "{label:<28} acc {:>5.1}% ({}/{}) | median {:>5} ms | acted {:>2}/{:<2} right {:>5.1}% | commands→none: {} confident, {} caught | wrong hints {}",
        pct(s.correct, s.n),
        s.correct,
        s.n,
        s.median_ms,
        s.acted_correct,
        s.acted,
        pct(s.acted_correct, s.acted),
        s.drops_confident,
        s.drops_caught,
        s.wrong_hints
    )
}

async fn run_routes(judge: &Judge, cases: &[RouteCase]) -> Vec<RouteOutcome> {
    use crate::commands::chatloop::{gate_specs, typed_tool_gate};
    let home_specs = gate_specs(true);
    let notebook_specs = gate_specs(false);
    let mut out = Vec::with_capacity(cases.len());
    for case in cases {
        let specs = if case.surface == "home" {
            &home_specs
        } else {
            &notebook_specs
        };
        let started = Instant::now();
        let (got, confidence) =
            match tokio::time::timeout(PER_CASE_TIMEOUT, typed_tool_gate(judge, specs, &case.msg))
                .await
            {
                Ok(Ok(g)) => (g.tool, g.confidence),
                Ok(Err(err)) => {
                    eprintln!("  {} {}: {err:#}", judge.label(), case.id);
                    ("error".into(), 0.0)
                }
                Err(_) => ("timeout".into(), 0.0),
            };
        out.push(RouteOutcome {
            id: case.id.clone(),
            gold: case.gold.clone(),
            got,
            confidence,
            ms: started.elapsed().as_millis(),
        });
    }
    out
}

/// The tool gate over the labeled routing cases, per judge. There is no
/// "baseline" row here: without a judge the gate does not exist and every
/// message pays the loop round, so the baseline is 0 drops, 0 hints, and
/// one round per message.
///
///   ALCHEMY_OLLAMA_TESTS=1 ALCHEMY_JEV_TESTS=1 JUDGE_MODELS=qwen3.8:27b-mlx,gemma4:12b-mlx \
///     cargo test --lib eval_judge_routes -- --ignored --nocapture
#[tokio::test]
#[ignore = "live models — run with --ignored --nocapture and the ALCHEMY_*_TESTS flags"]
async fn eval_judge_routes() {
    let cases = route_cases();
    let ollama_on = crate::evals::ollama_tests_enabled();
    let jev_on = std::env::var("ALCHEMY_JEV_TESTS").is_ok();
    if !ollama_on && !jev_on {
        eprintln!("SKIP: set ALCHEMY_OLLAMA_TESTS=1 and/or ALCHEMY_JEV_TESTS=1");
        return;
    }
    let models: Vec<String> = std::env::var("JUDGE_MODELS")
        .unwrap_or_else(|_| "qwen3.8:27b-mlx".into())
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    let base_url = crate::ai::ollama_config(&AiConfig::default()).base_url;
    let home = std::env::var("HOME").unwrap_or_default();
    let today = chrono::Local::now().format("%Y-%m-%d");
    eprintln!(
        "\n{} routing cases ({} notebook, {} home), gate threshold {REVIEW_BELOW}\n",
        cases.len(),
        cases.iter().filter(|c| c.surface == "notebook").count(),
        cases.iter().filter(|c| c.surface == "home").count()
    );
    let mut judges: Vec<Judge> = Vec::new();
    if ollama_on {
        judges.extend(models.iter().map(|m| Judge::ollama(&base_url, m)));
    }
    if jev_on {
        match Judge::jev() {
            Some(j) => judges.push(j),
            None => eprintln!("jev skipped (no credential)"),
        }
    }
    let mut lines = Vec::new();
    for judge in &judges {
        let outcomes = run_routes(judge, &cases).await;
        let s = summarize_routes(&outcomes);
        let label = format!(
            "{}{}",
            judge.label(),
            if judge.trusted_to_skip() {
                ""
            } else {
                " (hint-only)"
            }
        );
        let line = report_routes(&label, &s);
        eprintln!("{line}");
        if !s.misses.is_empty() {
            eprintln!("    misses: {}", s.misses.join("  "));
        }
        let row = format!(
            "{today},judge,{},tool gate,{:.4},,,{},median_ms={} acted={}/{} drops_confident={} drops_caught={} wrong_hints={}\n",
            judge.label(),
            s.correct as f64 / s.n.max(1) as f64,
            s.n,
            s.median_ms,
            s.acted_correct,
            s.acted,
            s.drops_confident,
            s.drops_caught,
            s.wrong_hints
        );
        let _ = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(format!("{home}/alchemy-benchmarks.csv"))
            .and_then(|mut f| std::io::Write::write_all(&mut f, row.as_bytes()));
        lines.push(line);
    }
    eprintln!("\n{}\n", lines.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_is_balanced_and_well_formed() {
        let cases = cases();
        assert_eq!(cases.len(), 40);
        for gold in ["supported", "weak", "unsupported", "contradicted"] {
            assert_eq!(
                cases.iter().filter(|c| c.gold == gold).count(),
                10,
                "{gold}"
            );
        }
        for c in &cases {
            assert!(c.excerpts.len() >= 2, "{} needs a distractor", c.id);
            assert!(c.claim.chars().count() >= 20, "{} claim too short", c.id);
        }
    }

    #[test]
    fn routing_fixture_names_real_tools() {
        use crate::commands::chatloop::gate_specs;
        let cases = route_cases();
        assert!(cases.len() >= 30);
        for c in &cases {
            let specs = gate_specs(c.surface == "home");
            assert!(
                c.gold == "none" || specs.iter().any(|(n, _)| *n == c.gold),
                "{}: {} is not an action tool on {}",
                c.id,
                c.gold,
                c.surface
            );
        }
        assert!(cases.iter().filter(|c| c.gold == "none").count() >= 10);
    }

    #[test]
    fn route_summary_separates_drops_from_hints() {
        let o = |id: &str, gold: &str, got: &str, c: f64| RouteOutcome {
            id: id.into(),
            gold: gold.into(),
            got: got.into(),
            confidence: c,
            ms: 5,
        };
        let s = summarize_routes(&[
            o("a", "generate", "generate", 0.9),
            o("b", "generate", "none", 0.9),
            o("c", "generate", "none", 0.2),
            o("d", "none", "settings", 0.95),
            o("e", "none", "none", 0.1),
        ]);
        assert_eq!((s.n, s.correct), (5, 2));
        assert_eq!(
            (s.drops_confident, s.drops_caught, s.wrong_hints),
            (1, 1, 1)
        );
        assert_eq!((s.acted, s.acted_correct), (3, 1));
        assert!(report_routes("x", &s).contains("commands→none: 1 confident, 1 caught"));
    }

    #[test]
    fn summary_counts_flagged_errors_and_unjudged() {
        let outcomes = vec![
            Outcome {
                id: "a".into(),
                gold: "weak".into(),
                got: "weak".into(),
                confidence: Some(0.9),
                ms: 10,
            },
            Outcome {
                id: "b".into(),
                gold: "weak".into(),
                got: "supported".into(),
                confidence: Some(0.2),
                ms: 20,
            },
            Outcome {
                id: "c".into(),
                gold: "weak".into(),
                got: "supported".into(),
                confidence: Some(0.95),
                ms: 30,
            },
            Outcome {
                id: "d".into(),
                gold: "weak".into(),
                got: "unjudged".into(),
                confidence: None,
                ms: 40,
            },
        ];
        let s = summarize(&outcomes);
        assert_eq!((s.n, s.correct, s.unjudged, s.errors), (4, 1, 1, 3));
        assert_eq!(
            (s.confident, s.confident_correct, s.errors_flagged),
            (2, 1, 1)
        );
        assert_eq!(s.median_ms, 30);
        assert!(report("x", &s).contains("errors flagged  1/3"));
    }
}
