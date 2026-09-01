//! Thin async Ollama HTTP client: embeddings, blocking chat, and streaming chat.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

use super::{ChatOutcome, ChatTurn, GenStats, OllamaConfig};

#[derive(Clone)]
pub struct Ollama {
    http: reqwest::Client,
    config: OllamaConfig,
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

/// Dedicated OCR models want specific, terse prompts; general vision models do
/// better with an explicit instruction.
fn ocr_prompt(model: &str) -> &'static str {
    let m = model.to_lowercase();
    if m.contains("deepseek-ocr") {
        // Grounding token yields structured markdown (headings, tables).
        "<|grounding|>Convert the document to markdown."
    } else if m.contains("glm-ocr") {
        "Text Recognition:"
    } else {
        "Transcribe ALL text in this image exactly, preserving reading order and line breaks. \
         Output only the transcribed text with no commentary. If there is no text, output nothing."
    }
}

#[derive(Deserialize)]
struct ChatMessageDelta {
    #[serde(default)]
    content: String,
}

impl Ollama {
    pub fn new(config: OllamaConfig) -> Self {
        let http = reqwest::Client::builder()
            // Local models can take a while on the first token; be patient.
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("failed to build reqwest client");
        Self { http, config }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url.trim_end_matches('/'), path)
    }

    /// Gateway-parity backoff for Ollama's idempotent perception calls
    /// (chat, embed, OCR — generations mutate nothing): two short waits on
    /// 429/5xx before giving up, so a model mid-load or a restarting server
    /// doesn't kill a whole agentic run. Transport errors and timeouts are
    /// deliberately NOT retried — the client timeout is long, and doubling
    /// it would hide a wedged server instead of surfacing it.
    async fn backoff_if_retryable(resp: &reqwest::Response, attempt: u32) -> bool {
        let code = resp.status().as_u16();
        if attempt >= 2 || !(code == 429 || (500..=503).contains(&code)) {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2 + 4 * attempt as u64)).await;
        true
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
            let parsed = self
                .embed_request(batch, std::time::Duration::from_secs(120))
                .await?;
            out.extend(parsed.embeddings);
        }
        Ok(out)
    }

    async fn embed_request(
        &self,
        inputs: &[String],
        timeout: std::time::Duration,
    ) -> Result<EmbedResponse> {
        let mut attempt = 0;
        let resp = loop {
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
            if resp.status().is_success() || !Self::backoff_if_retryable(&resp, attempt).await {
                break resp;
            }
            attempt += 1;
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("ollama embed {}: {}", status, body));
        }
        resp.json().await.context("invalid embed response")
    }

    /// OCR an image with the configured vision model via `/api/chat` (the path
    /// the dedicated OCR models document). `image_base64` is the raw image
    /// bytes, base64-encoded. Returns the transcribed text.
    pub async fn ocr(&self, image_base64: &str) -> Result<String> {
        let model = self.config.vision_model.trim();
        if model.is_empty() {
            return Err(anyhow!(
                "no vision model configured for OCR — set one in Settings"
            ));
        }
        let mut attempt = 0;
        let resp = loop {
            let resp = self
                .http
                .post(self.url("/api/chat"))
                .timeout(std::time::Duration::from_secs(180))
                .json(&json!({
                    "model": model,
                    "messages": [{
                        "role": "user",
                        "content": ocr_prompt(model),
                        "images": [image_base64],
                    }],
                    "stream": false,
                }))
                .send()
                .await
                .with_context(|| {
                    format!(
                        "OCR request to Ollama failed — is the vision model `{model}` installed?"
                    )
                })?;
            if resp.status().is_success() || !Self::backoff_if_retryable(&resp, attempt).await {
                break resp;
            }
            attempt += 1;
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("ollama OCR {}: {}", status, body));
        }
        let value: serde_json::Value = resp.json().await?;
        Ok(value["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// Quick liveness probe for the embedding model (short timeout). Returns the
    /// embedding dimension on success.
    pub async fn test_embed(&self) -> Result<usize> {
        let parsed = self
            .embed_request(&["ok".to_string()], std::time::Duration::from_secs(20))
            .await?;
        Ok(parsed.embeddings.first().map(|v| v.len()).unwrap_or(0))
    }

    /// The configured residency window and output cap, as body fragments.
    /// Both absent unless set — the server defaults are what every other
    /// request expects.
    fn apply_keep_alive(&self, body: &mut serde_json::Value) {
        if let Some(ka) = &self.config.keep_alive {
            body["keep_alive"] = json!(ka);
        }
        if let Some(n) = self.config.num_predict {
            body["options"] = json!({ "num_predict": n });
        }
    }

    /// Non-streaming chat completion; used for one-shot artifact generation.
    pub async fn chat(&self, messages: &[ChatTurn]) -> Result<ChatOutcome> {
        let mut body = json!({
            "model": self.config.chat_model,
            "messages": messages,
            "stream": false,
        });
        self.apply_keep_alive(&mut body);
        let mut attempt = 0;
        let resp = loop {
            let resp = self
                .http
                .post(self.url("/api/chat"))
                .json(&body)
                .send()
                .await
                .context("ollama chat request failed")?;
            if resp.status().is_success() || !Self::backoff_if_retryable(&resp, attempt).await {
                break resp;
            }
            attempt += 1;
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("ollama chat {}: {}", status, body));
        }
        let value: serde_json::Value = resp.json().await?;
        let text = value["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let stats = GenStats::from_parts(
            value.get("eval_count").and_then(|v| v.as_u64()),
            value.get("eval_duration").and_then(|v| v.as_u64()),
        );
        Ok(ChatOutcome {
            text,
            stats,
            cost_usd: None,
        })
    }

    /// Streaming chat. `on_token` is called for each content delta as it arrives.
    /// Returns the full concatenated assistant message.
    pub async fn chat_stream<F>(
        &self,
        messages: &[ChatTurn],
        mut on_token: F,
    ) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
    {
        let body = {
            let mut body = json!({
                "model": self.config.chat_model,
                "messages": messages,
                "stream": true,
            });
            // Only when the user asked for one: the parameter is absent by
            // default, which is what every model expects.
            if !self.config.effort.trim().is_empty() {
                body["think"] = json!(self.config.effort.trim());
            }
            self.apply_keep_alive(&mut body);
            body
        };
        // Retries happen strictly before any body is consumed, so a retried
        // stream can never emit duplicate tokens.
        let mut attempt = 0;
        let resp = loop {
            let resp = self
                .http
                .post(self.url("/api/chat"))
                .json(&body)
                .send()
                .await
                .context("ollama chat request failed")?;
            if resp.status().is_success() || !Self::backoff_if_retryable(&resp, attempt).await {
                break resp;
            }
            attempt += 1;
        };

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
                        return Ok(ChatOutcome {
                            text: full,
                            stats,
                            cost_usd: None,
                        });
                    }
                }
            }
        }
        Ok(ChatOutcome {
            text: full,
            stats: None,
            cost_usd: None,
        })
    }

    /// Ask the server to release this engine's chat model now — a chat
    /// request with `keep_alive: 0` and no messages is Ollama's documented
    /// unload. Best-effort: eval sweeps call it between models so a swept
    /// 10-20 GB model doesn't linger in unified memory while the next
    /// loads; a failure just means the residency window expires instead.
    #[cfg_attr(not(test), allow(dead_code))] // eval-harness hygiene, test builds only
    pub async fn unload_chat_model(&self) {
        let _ = self
            .http
            .post(self.url("/api/chat"))
            .timeout(std::time::Duration::from_secs(10))
            .json(&json!({
                "model": self.config.chat_model,
                "messages": [],
                "keep_alive": 0,
            }))
            .send()
            .await;
    }

    /// The chat model this engine answers with — the name a loading status
    /// should show.
    pub fn chat_model_name(&self) -> &str {
        self.config.chat_model.trim()
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

    /// Names of models currently loaded — or mid-load — in the server
    /// (`/api/ps`). Used to tell "cold model still paging in" apart from
    /// "daemon gone" when a probe times out. Short timeout: this runs right
    /// after a failure, and a wedged daemon must not stall the report.
    pub async fn ps(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct PsModel {
            name: String,
        }
        #[derive(Deserialize)]
        struct PsResponse {
            #[serde(default)]
            models: Vec<PsModel>,
        }
        let resp = self
            .http
            .get(self.url("/api/ps"))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
            .context("ollama ps request failed")?;
        let parsed: PsResponse = resp.json().await.context("invalid ps response")?;
        Ok(parsed.models.into_iter().map(|m| m.name).collect())
    }
}
