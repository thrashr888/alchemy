//! Typed judgments (docs/RFC-typesafe-jev.md): a decision with a defined
//! answer space, asked over a bounded `state`, answered with probabilities
//! instead of prose. Backed by TypeSafe's Jev when a key is present. When
//! there is none, `available()` is `None` and every call site keeps its
//! Small-role path — nothing in the app depends on this module being live.
//!
//! What leaves the machine: exactly the `state` a call site builds (already
//! retrieved, already clipped) plus the question text. Never ids, paths, or
//! whole sources. Every request appends one line to `traces/judge.jsonl`
//! with the site, latency, token usage, and the answers, so the spend is as
//! inspectable as retrieval is.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
/// Pinned, not `jev-latest`: thresholds below are tuned against a version,
/// and the alias moves without a change on our side. `ALCHEMY_JEV_MODEL`
/// overrides for evals.
const MODEL: &str = "jev-1.13.0";
const TIMEOUT: Duration = Duration::from_secs(15);
/// Total request budget in bytes, well under the model's 32k-token state
/// ceiling. A site that trips this is sending too much; clip first.
const MAX_STATE_BYTES: usize = 60_000;

/// One question over the shared state.
#[derive(Debug, Clone)]
pub enum Question {
    /// Yes/no. Answer is the probability of yes.
    Noul {
        instructions: String,
        /// What yes and no mean, when the boundary is subtle.
        criteria: Option<(String, String)>,
    },
    /// One of a fixed set. Answer is the pick plus the full distribution.
    Choice {
        instructions: String,
        /// Option name → rubric (None when the name is self-explaining).
        options: Vec<(String, Option<String>)>,
    },
    /// A position on ordered, described levels (low → high). Its first
    /// caller is registry card triage (RFC §4); parsing is tested now so
    /// the site can land without touching this module.
    #[allow(dead_code)]
    Score {
        instructions: String,
        levels: Vec<String>,
    },
}

impl Question {
    pub fn noul(instructions: impl Into<String>) -> Self {
        Self::Noul {
            instructions: instructions.into(),
            criteria: None,
        }
    }

    pub fn choice<I, K, V>(instructions: impl Into<String>, options: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self::Choice {
            instructions: instructions.into(),
            options: options
                .into_iter()
                .map(|(k, v)| (k.into(), Some(v.into())))
                .collect(),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Self::Noul {
                instructions,
                criteria,
            } => {
                let mut q = json!({"type": "noul", "instructions": instructions});
                if let Some((yes, no)) = criteria {
                    q["criteria"] = json!({"true": yes, "false": no});
                }
                q
            }
            Self::Choice {
                instructions,
                options,
            } => {
                let criteria: serde_json::Map<String, Value> = options
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone().map_or(Value::Null, Value::String)))
                    .collect();
                json!({"type": "choice", "instructions": instructions, "criteria": criteria})
            }
            Self::Score {
                instructions,
                levels,
            } => json!({"type": "score", "instructions": instructions, "criteria": levels}),
        }
    }
}

/// One validated answer. Probabilities are in [0, 1] and sum to one;
/// confidence summarizes how concentrated the distribution is (Nouls carry
/// none — the probability is the whole answer).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        probabilities: BTreeMap<u8, f64>,
        confidence: f64,
    },
}

/// Answers keyed by the ids the caller chose. Every entry was validated
/// before the map was returned; a malformed response fails the whole call.
#[derive(Debug, Clone, Default)]
pub struct Answers(BTreeMap<String, Answer>);

impl Answers {
    pub fn noul(&self, id: &str) -> Option<f64> {
        match self.0.get(id)? {
            Answer::Noul { noul } => Some(*noul),
            _ => None,
        }
    }

    pub fn choice(&self, id: &str) -> Option<(&str, f64, &BTreeMap<String, f64>)> {
        match self.0.get(id)? {
            Answer::Choice {
                choice,
                confidence,
                probabilities,
            } => Some((choice, *confidence, probabilities)),
            _ => None,
        }
    }

    #[allow(dead_code)] // arrives with the first Score site (RFC §4)
    pub fn score(&self, id: &str) -> Option<(f64, f64)> {
        match self.0.get(id)? {
            Answer::Score {
                score, confidence, ..
            } => Some((*score, *confidence)),
            _ => None,
        }
    }
}

/// Where the key came from — reported in Settings, never the key itself.
#[derive(Debug, Clone)]
pub struct Credential {
    pub key: String,
    pub source: String,
}

/// Resolution order shared with clue, so one key serves both:
/// `TYPESAFE_API_KEY`, then the file `TYPESAFE_API_KEY_FILE` names, then
/// `$XDG_CONFIG_HOME/typesafe/api-key`, then `~/.config/typesafe/api-key`.
pub fn credential() -> Option<Credential> {
    resolve(
        std::env::var("TYPESAFE_API_KEY").ok(),
        std::env::var_os("TYPESAFE_API_KEY_FILE").map(PathBuf::from),
        shared_key_path(),
    )
}

fn shared_key_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg).join("typesafe/api-key"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/typesafe/api-key"))
}

fn resolve(
    env_key: Option<String>,
    env_file: Option<PathBuf>,
    shared: Option<PathBuf>,
) -> Option<Credential> {
    if let Some(key) = env_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
    {
        return Some(Credential {
            key,
            source: "environment:TYPESAFE_API_KEY".into(),
        });
    }
    for (path, label) in [
        (env_file, "environment:TYPESAFE_API_KEY_FILE"),
        (shared, "file"),
    ] {
        let Some(path) = path else { continue };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let key = raw.trim();
        if key.is_empty() || key.chars().any(char::is_control) {
            continue;
        }
        return Some(Credential {
            key: key.to_string(),
            source: format!("{label}:{}", path.display()),
        });
    }
    None
}

/// A live judge: present only when a credential resolves right now. Cheap
/// to call — the key is a file read — so call sites ask at the moment of
/// use and a key added while the app runs is picked up on the next call.
pub fn available() -> Option<Judge> {
    let credential = credential()?;
    // Say where the key came from, once per process: the only place the
    // user learns that judgments are leaving the machine, never the key.
    static ANNOUNCED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ANNOUNCED.get_or_init(|| {
        crate::note!(
            "judge: TypeSafe {MODEL} on, key from {} (typed judgments leave the machine; \
             see traces/judge.jsonl)",
            credential.source
        );
    });
    Some(Judge::new(credential))
}

pub struct Judge {
    key: String,
    model: String,
    client: reqwest::Client,
}

impl Judge {
    fn new(credential: Credential) -> Self {
        Self {
            key: credential.key,
            model: std::env::var("ALCHEMY_JEV_MODEL")
                .ok()
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| MODEL.to_string()),
            client: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
        }
    }

    /// Ask every question in `questions` over `state` in one round trip.
    /// `site` names the caller in the trace line ("second_look", "route").
    pub async fn ask(
        &self,
        site: &str,
        state: Value,
        questions: &[(&str, Question)],
    ) -> Result<Answers> {
        if questions.is_empty() {
            bail!("no questions");
        }
        let qmap: serde_json::Map<String, Value> = questions
            .iter()
            .map(|(id, q)| (id.to_string(), q.to_json()))
            .collect();
        let body = json!({"model": self.model, "state": state, "questions": qmap});
        let bytes = body.to_string().len();
        if bytes > MAX_STATE_BYTES {
            bail!("judge request for {site} is {bytes} bytes; clip the state first");
        }
        let started = Instant::now();
        let response = self
            .client
            .post(ENDPOINT)
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                anyhow!(
                    "TypeSafe unreachable ({})",
                    if e.is_timeout() {
                        "timeout"
                    } else {
                        "connection"
                    }
                )
            })?;
        let status = response.status();
        if !status.is_success() {
            // Bodies and headers stay out of the log: they can carry secrets.
            bail!("TypeSafe returned HTTP {}", status.as_u16());
        }
        let raw: Value = response
            .json()
            .await
            .context("TypeSafe returned invalid JSON")?;
        let answers = parse_answers(&raw, questions)?;
        if let Some(dir) = crate::trace::dir() {
            crate::trace::log_file(
                dir,
                "judge.jsonl",
                json!({
                    "ts": std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    "site": site,
                    "model": raw.get("model").and_then(Value::as_str).unwrap_or(&self.model),
                    "questions": questions.len(),
                    "stateBytes": bytes,
                    "usage": raw.get("usage").cloned().unwrap_or(Value::Null),
                    "ms": started.elapsed().as_millis(),
                    "answers": answers.0,
                }),
            );
        }
        Ok(answers)
    }
}

fn unit(v: f64) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}

fn sums_to_one<'a>(values: impl Iterator<Item = &'a f64>) -> bool {
    let mut total = 0.0;
    for v in values {
        if !unit(*v) {
            return false;
        }
        total += v;
    }
    (total - 1.0).abs() <= 0.02
}

/// Validate every answer against its question. Nothing is returned unless
/// all of it checks out: a partial map would let a caller act on one good
/// answer beside a missing one.
fn parse_answers(raw: &Value, questions: &[(&str, Question)]) -> Result<Answers> {
    let answers = raw
        .get("answers")
        .and_then(Value::as_object)
        .context("TypeSafe response has no answers")?;
    let mut out = BTreeMap::new();
    for (id, question) in questions {
        let a = answers
            .get(*id)
            .with_context(|| format!("TypeSafe omitted the answer to `{id}`"))?;
        let kind = a.get("type").and_then(Value::as_str).unwrap_or("");
        let parsed = match question {
            Question::Noul { .. } => {
                let noul = a.get("noul").and_then(Value::as_f64);
                match noul {
                    Some(n) if kind == "noul" && unit(n) => Answer::Noul { noul: n },
                    _ => bail!("malformed noul answer for `{id}`"),
                }
            }
            Question::Choice { options, .. } => {
                let choice = a.get("choice").and_then(Value::as_str).unwrap_or("");
                let confidence = a.get("confidence").and_then(Value::as_f64).unwrap_or(-1.0);
                let probabilities: BTreeMap<String, f64> = a
                    .get("probabilities")
                    .cloned()
                    .and_then(|p| serde_json::from_value(p).ok())
                    .unwrap_or_default();
                let known = |k: &str| options.iter().any(|(o, _)| o == k);
                if kind != "choice"
                    || !known(choice)
                    || !unit(confidence)
                    || probabilities.len() != options.len()
                    || !probabilities.keys().all(|k| known(k))
                    || !sums_to_one(probabilities.values())
                {
                    bail!("malformed choice answer for `{id}`");
                }
                Answer::Choice {
                    choice: choice.to_string(),
                    probabilities,
                    confidence,
                }
            }
            Question::Score { levels, .. } => {
                let score = a.get("score").and_then(Value::as_f64).unwrap_or(-1.0);
                let confidence = a.get("confidence").and_then(Value::as_f64).unwrap_or(-1.0);
                let by_string: BTreeMap<String, f64> = a
                    .get("probabilities")
                    .cloned()
                    .and_then(|p| serde_json::from_value(p).ok())
                    .unwrap_or_default();
                let mut probabilities = BTreeMap::new();
                for (k, v) in &by_string {
                    let Ok(level) = k.parse::<u8>() else {
                        bail!("malformed score level `{k}` for `{id}`");
                    };
                    probabilities.insert(level, *v);
                }
                let top = levels.len().saturating_sub(1) as f64;
                let expected: f64 = probabilities.iter().map(|(k, v)| f64::from(*k) * v).sum();
                if kind != "score"
                    || !unit(confidence)
                    || !score.is_finite()
                    || !(0.0..=top).contains(&score)
                    || probabilities.len() != levels.len()
                    || probabilities
                        .keys()
                        .any(|k| usize::from(*k) >= levels.len())
                    || !sums_to_one(probabilities.values())
                    || (expected - score).abs() > 0.06
                {
                    bail!("malformed score answer for `{id}`");
                }
                Answer::Score {
                    score,
                    probabilities,
                    confidence,
                }
            }
        };
        out.insert((*id).to_string(), parsed);
    }
    Ok(Answers(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict() -> Vec<(&'static str, Question)> {
        vec![
            (
                "verdict",
                Question::choice(
                    "Does `excerpt` support `claim`?",
                    [("supported", "states it"), ("contradicted", "opposite")],
                ),
            ),
            ("relevant", Question::noul("Is `excerpt` about `claim`?")),
        ]
    }

    #[test]
    fn questions_serialize_to_the_api_shape() {
        let qs = verdict();
        let choice = qs[0].1.to_json();
        assert_eq!(choice["type"], "choice");
        assert_eq!(choice["criteria"]["supported"], "states it");
        let noul = qs[1].1.to_json();
        assert_eq!(noul["type"], "noul");
        assert!(noul.get("criteria").is_none());
        let score = Question::Score {
            instructions: "how relevant".into(),
            levels: vec!["none".into(), "some".into()],
        }
        .to_json();
        assert_eq!(score["criteria"], json!(["none", "some"]));
    }

    #[test]
    fn a_good_response_parses_with_every_field() {
        let raw = json!({"model":"jev-1.13.0","answers":{
            "verdict":{"type":"choice","choice":"supported","confidence":0.98,
                       "probabilities":{"supported":0.98,"contradicted":0.02}},
            "relevant":{"type":"noul","noul":0.91}},
            "usage":{"input_tokens":435,"output_tokens":67}});
        let answers = parse_answers(&raw, &verdict()).unwrap();
        let (choice, confidence, probabilities) = answers.choice("verdict").unwrap();
        assert_eq!(choice, "supported");
        assert!((confidence - 0.98).abs() < 1e-9);
        assert_eq!(probabilities.len(), 2);
        assert!((answers.noul("relevant").unwrap() - 0.91).abs() < 1e-9);
        assert!(answers.choice("relevant").is_none());
    }

    #[test]
    fn a_bad_response_fails_whole() {
        let good_noul = json!({"type":"noul","noul":0.5});
        for bad in [
            json!({"type":"choice","choice":"maybe","confidence":0.9,"probabilities":{"supported":0.9,"contradicted":0.1}}),
            json!({"type":"choice","choice":"supported","confidence":0.9,"probabilities":{"supported":0.9}}),
            json!({"type":"choice","choice":"supported","confidence":0.9,"probabilities":{"supported":0.9,"contradicted":0.4}}),
            json!({"type":"noul","noul":0.5}),
        ] {
            let raw = json!({"answers":{"verdict":bad,"relevant":good_noul}});
            assert!(parse_answers(&raw, &verdict()).is_err());
        }
        let missing = json!({"answers":{"verdict":{"type":"choice","choice":"supported","confidence":1.0,"probabilities":{"supported":1.0,"contradicted":0.0}}}});
        assert!(parse_answers(&missing, &verdict()).is_err());
    }

    #[test]
    fn scores_must_match_their_distribution() {
        let q = vec![(
            "s",
            Question::Score {
                instructions: "x".into(),
                levels: vec!["a".into(), "b".into(), "c".into()],
            },
        )];
        let ok = json!({"answers":{"s":{"type":"score","score":1.3,"confidence":0.54,
            "probabilities":{"0":0.0,"1":0.7,"2":0.3}}}});
        assert!((parse_answers(&ok, &q).unwrap().score("s").unwrap().0 - 1.3).abs() < 1e-9);
        let drift = json!({"answers":{"s":{"type":"score","score":2.0,"confidence":0.54,
            "probabilities":{"0":0.0,"1":0.7,"2":0.3}}}});
        assert!(parse_answers(&drift, &q).is_err());
    }

    #[test]
    fn credential_resolution_prefers_env_then_files_and_rejects_junk() {
        let dir = std::env::temp_dir().join(format!("alchemy-judge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shared = dir.join("api-key");
        std::fs::write(&shared, "ts_shared\n").unwrap();
        let named = dir.join("named");
        std::fs::write(&named, "ts_named").unwrap();
        let junk = dir.join("junk");
        std::fs::write(&junk, "line one\nline two").unwrap();

        let c = resolve(
            Some(" ts_env ".into()),
            Some(named.clone()),
            Some(shared.clone()),
        )
        .unwrap();
        assert_eq!(
            (c.key.as_str(), c.source.as_str()),
            ("ts_env", "environment:TYPESAFE_API_KEY")
        );
        let c = resolve(Some("".into()), Some(named.clone()), Some(shared.clone())).unwrap();
        assert_eq!(c.key, "ts_named");
        assert!(c.source.starts_with("environment:TYPESAFE_API_KEY_FILE:"));
        let c = resolve(None, Some(dir.join("missing")), Some(shared.clone())).unwrap();
        assert_eq!(c.key, "ts_shared");
        assert!(resolve(None, None, Some(junk)).is_none());
        assert!(resolve(None, None, None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live round trip — needs a key and the network:
    ///   ALCHEMY_JEV_TESTS=1 cargo test --lib judge::tests::live -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn live_round_trip_answers_a_verdict() {
        if std::env::var("ALCHEMY_JEV_TESTS").is_err() {
            eprintln!("set ALCHEMY_JEV_TESTS=1 to run");
            return;
        }
        let judge = available().expect("no TypeSafe credential");
        let answers = judge
            .ask(
                "test",
                json!({"claim": "Sales fell in Q4.", "excerpt": "Q4 sales climbed 9%."}),
                &verdict(),
            )
            .await
            .unwrap();
        let (choice, confidence, _) = answers.choice("verdict").unwrap();
        eprintln!(
            "verdict={choice} confidence={confidence:.2} relevant={:.2}",
            answers.noul("relevant").unwrap()
        );
        assert_eq!(choice, "contradicted");
    }
}
