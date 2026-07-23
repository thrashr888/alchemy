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
        // 8192 tokens — a prompt past it does not degrade, it hard-errors.
        // Structure-aware callers already budget the prompt before assembly;
        // this catches every path that didn't (agentic retrieval, rerank,
        // distill, tool routing…) and any estimate that drifted.
        //
        // The token estimate (chars/3.5) is calibrated to English prose; dense
        // content (code, RFCs, dense markdown, CJK) tokenizes to MORE tokens per
        // char, so a prompt the estimator calls "in budget" can still exceed
        // 8192 and get rejected up front — before any token — with the exact
        // count ("Content contains N tokens, which exceeds … 8192"). We read
        // that measured count and re-trim to its true ratio, retrying a bounded
        // number of times. Overflow is a pre-generation rejection, so no token
        // has reached `on_token` when we retry. See `inference::budget`.
        const MAX_ATTEMPTS: usize = 3;
        let mut budget_tokens = budget::FM_INPUT_BUDGET_TOKENS;
        for attempt in 0..MAX_ATTEMPTS {
            let fitted = budget::fit_messages(messages, budget_tokens);
            if let Cow::Owned(_) = &fitted {
                eprintln!(
                    "foundation models: trimming prompt to ~{budget_tokens} est input tokens \
                     (assembled ~{} est) to fit the on-device window",
                    budget::messages_tokens(messages),
                );
            }
            match self.run_once(&fitted, &mut on_token).await {
                Ok(outcome) => return Ok(outcome),
                Err(e) => match parse_context_overflow(&e) {
                    // Still over the real window and attempts remain: recalibrate
                    // the budget from the measured count (with headroom) and retry.
                    Some((actual, limit)) if attempt + 1 < MAX_ATTEMPTS => {
                        let target = budget::FM_INPUT_BUDGET_TOKENS * 9 / 10; // 10% headroom
                        let scaled = (budget_tokens as u128 * target as u128
                            / (actual.max(1) as u128))
                            as usize;
                        let next = scaled.min(budget_tokens.saturating_sub(1)).max(256);
                        eprintln!(
                            "foundation models: prompt measured {actual} tokens (limit {limit}); \
                             re-trimming to ~{next} est input tokens and retrying",
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
            budget::FM_CONTEXT_TOKENS,
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
