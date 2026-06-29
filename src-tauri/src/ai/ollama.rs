//! Thin async Ollama HTTP client: embeddings, blocking chat, and streaming chat.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

use super::{AiConfig, ChatTurn};

#[derive(Clone)]
pub struct Ollama {
    http: reqwest::Client,
    config: AiConfig,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    name: String,
}

#[derive(Deserialize)]
struct ChatChunk {
    #[serde(default)]
    message: Option<ChatMessageDelta>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    eval_count: Option<u64>,
    #[serde(default)]
    eval_duration: Option<u64>,
}

/// Generation stats reported by Ollama at the end of a chat.
#[derive(Clone, Copy, Default)]
pub struct GenStats {
    pub eval_count: u64,
    pub eval_duration_ns: u64,
}

impl GenStats {
    pub fn tokens_per_sec(&self) -> f64 {
        if self.eval_duration_ns == 0 {
            0.0
        } else {
            self.eval_count as f64 / (self.eval_duration_ns as f64 / 1e9)
        }
    }

    fn from_parts(count: Option<u64>, duration: Option<u64>) -> Option<Self> {
        match (count, duration) {
            (Some(eval_count), Some(eval_duration_ns)) if eval_count > 0 && eval_duration_ns > 0 => {
                Some(GenStats { eval_count, eval_duration_ns })
            }
            _ => None,
        }
    }
}

/// Result of a chat: the assistant text plus optional generation stats.
pub struct ChatOutcome {
    pub text: String,
    pub stats: Option<GenStats>,
}

#[derive(Deserialize)]
struct ChatMessageDelta {
    #[serde(default)]
    content: String,
}

impl Ollama {
    pub fn new(config: AiConfig) -> Self {
        let http = reqwest::Client::builder()
            // Local models can take a while on the first token; be patient.
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("failed to build reqwest client");
        Self { http, config }
    }

    pub fn config(&self) -> &AiConfig {
        &self.config
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url.trim_end_matches('/'), path)
    }

    /// Embed a batch of texts. Returns one vector per input, in order.
    ///
    /// Texts are sent in bounded sub-batches with a per-request timeout so a
    /// wedged/loading Ollama surfaces a clear error in seconds rather than
    /// hanging on one giant request.
    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        const BATCH: usize = 64;
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH) {
            let parsed = self.embed_request(batch, std::time::Duration::from_secs(120)).await?;
            out.extend(parsed.embeddings);
        }
        Ok(out)
    }

    async fn embed_request(
        &self,
        inputs: &[String],
        timeout: std::time::Duration,
    ) -> Result<EmbedResponse> {
        let resp = self
            .http
            .post(self.url("/api/embed"))
            .timeout(timeout)
            .json(&json!({ "model": self.config.embed_model, "input": inputs }))
            .send()
            .await
            .with_context(|| {
                format!(
                    "embedding request to Ollama failed or timed out — is `ollama serve` running \
                     and is the model `{}` available? (a large chat model loading can also stall this)",
                    self.config.embed_model
                )
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("ollama embed {}: {}", status, body));
        }
        resp.json().await.context("invalid embed response")
    }

    /// Quick liveness probe for the embedding model (short timeout). Returns the
    /// embedding dimension on success.
    pub async fn test_embed(&self) -> Result<usize> {
        let parsed = self
            .embed_request(&["ok".to_string()], std::time::Duration::from_secs(20))
            .await?;
        Ok(parsed.embeddings.first().map(|v| v.len()).unwrap_or(0))
    }

    pub async fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut v = self.embed(&[text.to_string()]).await?;
        v.pop().ok_or_else(|| anyhow!("embedding model returned no vector"))
    }

    /// Non-streaming chat completion; used for one-shot artifact generation.
    pub async fn chat(&self, messages: &[ChatTurn]) -> Result<ChatOutcome> {
        let resp = self
            .http
            .post(self.url("/api/chat"))
            .json(&json!({
                "model": self.config.chat_model,
                "messages": messages,
                "stream": false,
            }))
            .send()
            .await
            .context("ollama chat request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("ollama chat {}: {}", status, body));
        }
        let value: serde_json::Value = resp.json().await?;
        let text = value["message"]["content"].as_str().unwrap_or_default().to_string();
        let stats = GenStats::from_parts(
            value.get("eval_count").and_then(|v| v.as_u64()),
            value.get("eval_duration").and_then(|v| v.as_u64()),
        );
        Ok(ChatOutcome { text, stats })
    }

    /// Streaming chat. `on_token` is called for each content delta as it arrives.
    /// Returns the full concatenated assistant message.
    pub async fn chat_stream<F>(&self, messages: &[ChatTurn], mut on_token: F) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
    {
        let resp = self
            .http
            .post(self.url("/api/chat"))
            .json(&json!({
                "model": self.config.chat_model,
                "messages": messages,
                "stream": true,
            }))
            .send()
            .await
            .context("ollama chat request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("ollama chat {}: {}", status, body));
        }

        let mut full = String::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.context("error reading chat stream")?;
            buf.extend_from_slice(&bytes);

            // Ollama streams newline-delimited JSON objects.
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = &line[..line.len() - 1];
                if line.is_empty() {
                    continue;
                }
                if let Ok(parsed) = serde_json::from_slice::<ChatChunk>(line) {
                    if let Some(delta) = parsed.message {
                        if !delta.content.is_empty() {
                            on_token(&delta.content);
                            full.push_str(&delta.content);
                        }
                    }
                    if parsed.done {
                        let stats = GenStats::from_parts(parsed.eval_count, parsed.eval_duration);
                        return Ok(ChatOutcome { text: full, stats });
                    }
                }
            }
        }
        Ok(ChatOutcome { text: full, stats: None })
    }

    /// List locally available model names (from `/api/tags`).
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let resp = self
            .http
            .get(self.url("/api/tags"))
            .send()
            .await
            .context("ollama tags request failed")?;
        let parsed: TagsResponse = resp.json().await.context("invalid tags response")?;
        Ok(parsed.models.into_iter().map(|m| m.name).collect())
    }
}
