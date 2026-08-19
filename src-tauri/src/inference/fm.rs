//! Apple Foundation Models engine: drives the `alchemy-fm` sidecar
//! (sidecar/alchemy-fm) — the on-device system model over NDJSON stdio.
//! One-shot per request, stateless, `kill_on_drop` throughout; the base API
//! is macOS 26+, and every failure here is soft — the router falls through
//! to the configured chat engine (RFC-inference-providers §7).

use std::borrow::Cow;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::OnceCell;

use super::{budget, ChatOutcome, ChatTurn};

/// How long one Small-role generation may take end to end. The on-device
/// model answers title-sized prompts in ~1–2 s; anything past this is a hung
/// sidecar, not a slow model.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct FmEngine {
    binary: PathBuf,
    // Arc'd so clones share one probe (requests snapshot the engine and
    // stream outside the config lock — same pattern as LocalEmbedder).
    probe_detail: Arc<OnceCell<String>>,
    /// Probed once per engine build (one `--probe` spawn); `false` means the
    /// model is unavailable (old macOS, Apple Intelligence off, model not
    /// downloaded) and callers should fall through.
    available: Arc<OnceCell<bool>>,
}

impl FmEngine {
    pub fn new(binary: PathBuf) -> Self {
        Self {
            binary,
            available: Arc::new(OnceCell::new()),
            probe_detail: Arc::new(OnceCell::new()),
        }
    }

    /// The probe's reason string (availability enum text from the sidecar) —
    /// lets the UI distinguish "downloading" from "unsupported".
    pub async fn probe_detail(&self) -> String {
        self.available().await; // ensure the probe ran
        self.probe_detail.get().cloned().unwrap_or_default()
    }

    /// One cached availability probe per engine lifetime.
    pub async fn available(&self) -> bool {
        *self
            .available
            .get_or_init(|| async {
                let out = tokio::time::timeout(
                    Duration::from_secs(10),
                    tokio::process::Command::new(&self.binary)
                        .arg("--probe")
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .kill_on_drop(true)
                        .output(),
                )
                .await;
                match out {
                    Ok(Ok(o)) => {
                        let mut ok = false;
                        for v in String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                        {
                            if v["available"].as_bool() == Some(true) {
                                ok = true;
                            }
                            if let Some(d) = v["detail"].as_str() {
                                let _ = self.probe_detail.set(d.to_string());
                            }
                        }
                        ok
                    }
                    _ => false,
                }
            })
            .await
    }

    pub async fn chat_stream<F>(
        &self,
        messages: &[ChatTurn],
        mut on_token: F,
    ) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
    {
        // Hard boundary guard (backstop): this sidecar runs ONLY when the
        // active engine is the on-device model, whose context window is a hard
        // ceiling — a prompt past it does not degrade, it hard-errors.
        // Structure-aware callers already budget the prompt before assembly;
        // this catches every path that didn't (agentic retrieval, rerank,
        // distill, tool routing…) and any estimate that drifted.
        //
        // Two things can put us over that ceiling, and the retry handles both
        // by trusting only what the framework measured:
        //
        //  * The window is smaller than we assumed. It is not the same number
        //    on every machine and OS build (8192 here, 4096 on Paul's), and the
        //    rejection names the real one — so record it (`note_context_limit`)
        //    and re-derive the budget from it, for this call and every later
        //    one. Scaling off the ASSUMED budget instead was the bug behind
        //    "i still get this error": with a true window of 4096, a 4502-token
        //    prompt scaled to 6656 * 5990 / 4502 = 8854, clamped to
        //    budget - 1 — a one-token step that changed nothing, twice, before
        //    surfacing the same error.
        //  * The estimate drifted. `estimate_tokens` (chars/3.5) is calibrated
        //    to English prose; dense content (code, RFCs, dense markdown, CJK)
        //    tokenizes to MORE tokens per char, so a prompt the estimator calls
        //    "in budget" can still overflow. The measured/estimated ratio of
        //    the prompt we just sent converts the token target back into an
        //    estimator-space budget.
        //
        // Overflow is a pre-generation rejection, so no token has reached
        // `on_token` when we retry. See `inference::budget`.
        const MAX_ATTEMPTS: usize = 3;
        let mut budget_tokens = budget::fm_input_budget_tokens();
        for attempt in 0..MAX_ATTEMPTS {
            let fitted = budget::fit_messages(messages, budget_tokens);
            if let Cow::Owned(_) = &fitted {
                eprintln!(
                    "foundation models: trimming prompt to ~{budget_tokens} est input tokens \
                     (assembled ~{} est) to fit the on-device window",
                    budget::messages_tokens(messages),
                );
            }
            // What OUR estimator thinks we are about to send. The rejection
            // reports what the REAL tokenizer counted for this same prompt, and
            // the pair is the conversion factor between the two spaces.
            let sent_est = budget::messages_tokens(&fitted).max(1);
            match self.run_once(&fitted, &mut on_token).await {
                Ok(outcome) => return Ok(outcome),
                Err(e) => match parse_context_overflow(&e) {
                    // Still over the real window and attempts remain: recalibrate
                    // from the framework's own numbers and retry.
                    Some((actual, limit)) if attempt + 1 < MAX_ATTEMPTS => {
                        budget::note_context_limit(limit);
                        let next =
                            retuned_budget(sent_est, actual, budget::fm_input_budget_tokens());
                        eprintln!(
                            "foundation models: prompt measured {actual} tokens (limit {limit}, \
                             ~{sent_est} est); re-trimming to ~{next} est input tokens and retrying",
                        );
                        budget_tokens = next;
                    }
                    // Not an overflow, or attempts exhausted: surface it.
                    _ => return Err(e),
                },
            }
        }
        Err(anyhow!(
            "foundation models: could not fit the prompt within the {}-token window \
             after {MAX_ATTEMPTS} attempts",
            budget::fm_context_tokens(),
        ))
    }

    /// One sidecar round-trip: spawn, send the assembled prompt, stream tokens.
    /// Factored out of `chat_stream` so the overflow-retry loop can re-invoke it
    /// with a tighter prompt; takes `on_token` by `&mut` so the same callback
    /// spans attempts.
    async fn run_once<F>(&self, messages: &[ChatTurn], on_token: &mut F) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
    {
        let request = serde_json::json!({
            "messages": messages
                .iter()
                .map(|t| serde_json::json!({ "role": t.role, "content": t.content }))
                .collect::<Vec<_>>(),
        });

        let mut child = tokio::process::Command::new(&self.binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn alchemy-fm sidecar")?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("no sidecar stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("no sidecar stdout"))?;

        let run = async {
            stdin.write_all(request.to_string().as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            drop(stdin); // EOF tells the sidecar the request is complete

            let mut lines = BufReader::new(stdout).lines();
            let mut text = String::new();
            while let Some(line) = lines.next_line().await? {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                match v["type"].as_str() {
                    Some("token") => {
                        let t = v["text"].as_str().unwrap_or_default();
                        if v["replace"].as_bool() == Some(true) {
                            // The model revised earlier output (rare): restart
                            // the accumulated text; the token callback has no
                            // un-emit, so downstream sees the tail twice —
                            // acceptable for Small-role jobs.
                            text = t.to_string();
                        } else {
                            text.push_str(t);
                        }
                        on_token(t);
                    }
                    Some("done") => {
                        return Ok(ChatOutcome {
                            text,
                            ..Default::default()
                        });
                    }
                    Some("error") => {
                        let msg = v["message"].as_str().unwrap_or("sidecar error");
                        return Err(anyhow!("foundation models: {msg}"));
                    }
                    _ => {}
                }
            }
            // EOF before a done/error event means the sidecar died mid-stream
            // (a crash, not a completion). Returning the partial text as
            // success once masked a per-token SIGABRT as a 5-char answer —
            // fail loudly instead so the chat surface shows a real error.
            Err(anyhow!(
                "foundation models sidecar exited mid-stream ({} chars in)",
                text.len()
            ))
        };

        let outcome = tokio::time::timeout(REQUEST_TIMEOUT, run)
            .await
            .map_err(|_| anyhow!("foundation models sidecar timed out"))?;
        let _ = child.start_kill();
        outcome
    }

    pub async fn chat(&self, messages: &[ChatTurn]) -> Result<ChatOutcome> {
        self.chat_stream(messages, |_| {}).await
    }
}

/// Estimator-space budget that should land the next attempt at `target` real
/// tokens, given that a prompt we estimated at `sent_est` really measured
/// `actual`. `actual / sent_est` is this prompt's measured tokens-per-estimated
/// -token, so `target * sent_est / actual` is the estimate that maps to
/// `target`.
///
/// Clamped strictly below `sent_est`: whatever the ratio says, the next attempt
/// must be smaller than the one that just overflowed, so the loop can only
/// converge downward.
fn retuned_budget(sent_est: usize, actual: usize, target: usize) -> usize {
    let scaled = (sent_est as u128 * target as u128 / (actual.max(1) as u128)) as usize;
    scaled.min(sent_est.saturating_sub(1)).max(256)
}

/// Parse the framework's over-budget rejection — "Content contains N tokens,
/// which exceeds the maximum allowed context size of M" — into `(actual, limit)`.
/// Returns `None` for any other error, so only a true context overflow triggers
/// a re-trim/retry.
fn parse_context_overflow(err: &anyhow::Error) -> Option<(usize, usize)> {
    let s = err.to_string();
    if !s.contains("exceeds the maximum allowed context size") {
        return None;
    }
    let first_uint = |seg: &str| -> Option<usize> {
        seg.split(|c: char| !c.is_ascii_digit())
            .find(|p| !p.is_empty())
            .and_then(|n| n.parse().ok())
    };
    let actual = first_uint(s.split("contains ").nth(1)?)?;
    let limit = first_uint(s.rsplit("context size of ").next()?)?;
    Some((actual, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paul's live failure: a machine whose real window is 4096, not the 8192
    /// this code assumed. The prompt fit our 6656-token budget, measured 4502,
    /// and was rejected. The old recalibration aimed at 90% of the ASSUMED
    /// budget (5990) — above the true window — so it computed a bigger budget,
    /// got clamped to `budget - 1`, and retried the same prompt until the
    /// attempts ran out. Retuning against the REPORTED limit has to shrink it
    /// enough to actually fit.
    #[test]
    fn retunes_against_the_reported_limit_not_the_assumed_one() {
        let (sent_est, actual, limit) = (6656_usize, 4502_usize, 4096_usize);
        // The budget the reported limit implies (window minus a scaled reserve).
        let target = limit - limit * budget::FM_OUTPUT_RESERVE_TOKENS / budget::FM_CONTEXT_TOKENS;
        assert_eq!(target, 3328);

        let next = retuned_budget(sent_est, actual, target);
        assert_eq!(next, 4920);

        // The point of the exercise: at this prompt's measured ratio, the next
        // attempt lands inside the real window instead of overflowing again.
        let projected = actual * next / sent_est;
        assert!(
            projected <= limit,
            "retuned budget still projects {projected} tokens over a {limit} window"
        );

        // The old formula's target (90% of the assumed 6656 budget) is what
        // made the retry a no-op — it must not be what we aim at any more.
        let old_target = (budget::FM_CONTEXT_TOKENS - budget::FM_OUTPUT_RESERVE_TOKENS) * 9 / 10;
        assert_eq!(retuned_budget(sent_est, actual, old_target), sent_est - 1);
    }

    #[test]
    fn retuned_budget_always_shrinks() {
        // Even a ratio claiming there is room to grow steps down instead.
        assert_eq!(retuned_budget(1000, 10, 6656), 999);
        // And it never collapses below a usable floor.
        assert_eq!(retuned_budget(1000, 1_000_000, 3328), 256);
    }

    #[test]
    fn parses_the_context_overflow_rejection() {
        let e = anyhow!(
            "foundation models: Content contains 9554 tokens, which exceeds the \
             maximum allowed context size of 8192."
        );
        assert_eq!(parse_context_overflow(&e), Some((9554, 8192)));
    }

    #[test]
    fn ignores_unrelated_errors() {
        assert_eq!(
            parse_context_overflow(&anyhow!("foundation models sidecar timed out")),
            None
        );
        assert_eq!(parse_context_overflow(&anyhow!("boom")), None);
    }
}
