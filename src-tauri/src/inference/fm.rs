//! Apple Foundation Models engine: drives the `alchemy-fm` sidecar
//! (sidecar/alchemy-fm) — the on-device system model over NDJSON stdio.
//! One-shot per request, stateless, `kill_on_drop` throughout; the base API
//! is macOS 26+, and every failure here is soft — the router falls through
//! to the configured chat engine (RFC-inference-providers §7).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::OnceCell;

use super::{ChatOutcome, ChatTurn};

/// How long one Small-role generation may take end to end. The on-device
/// model answers title-sized prompts in ~1–2 s; anything past this is a hung
/// sidecar, not a slow model.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

pub struct FmEngine {
    binary: PathBuf,
    /// Probed once per engine build (one `--probe` spawn); `false` means the
    /// model is unavailable (old macOS, Apple Intelligence off, model not
    /// downloaded) and callers should fall through.
    available: OnceCell<bool>,
}

impl FmEngine {
    pub fn new(binary: PathBuf) -> Self {
        Self {
            binary,
            available: OnceCell::new(),
        }
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
                    Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                        .any(|v| v["available"].as_bool() == Some(true)),
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
                        return Ok(ChatOutcome { text, stats: None });
                    }
                    Some("error") => {
                        let msg = v["message"].as_str().unwrap_or("sidecar error");
                        return Err(anyhow!("foundation models: {msg}"));
                    }
                    _ => {}
                }
            }
            if text.is_empty() {
                Err(anyhow!("sidecar closed without output"))
            } else {
                Ok(ChatOutcome { text, stats: None })
            }
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
