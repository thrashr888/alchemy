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
    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let resp = self
            .http
            .post(self.url("/api/embed"))
            .json(&json!({ "model": self.config.embed_model, "input": texts }))
            .send()
            .await
            .context("ollama embed request failed (is `ollama serve` running?)")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("ollama embed {}: {}", status, body));
        }
        let parsed: EmbedResponse = resp.json().await.context("invalid embed response")?;
        Ok(parsed.embeddings)
    }

    pub async fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut v = self.embed(&[text.to_string()]).await?;
        v.pop().ok_or_else(|| anyhow!("embedding model returned no vector"))
    }

    /// Non-streaming chat completion; used for one-shot artifact generation.
    pub async fn chat(&self, messages: &[ChatTurn]) -> Result<String> {
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
        Ok(value["message"]["content"].as_str().unwrap_or_default().to_string())
    }

    /// Streaming chat. `on_token` is called for each content delta as it arrives.
    /// Returns the full concatenated assistant message.
    pub async fn chat_stream<F>(&self, messages: &[ChatTurn], mut on_token: F) -> Result<String>
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
                        return Ok(full);
                    }
                }
            }
        }
        Ok(full)
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
