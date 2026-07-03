//! Minimal OpenAI-compatible chat client (IBM Bob gateway, LM Studio, vLLM,
//! LiteLLM, or Ollama's own /v1). Bearer-token auth, SSE streaming.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde_json::json;

use super::{ChatOutcome, ChatTurn, GenStats};

#[derive(Clone)]
pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiClient {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("failed to build reqwest client");
        // Empty base falls back to Bob's gateway here too, so no caller can
        // construct a client that builds relative (schemeless) request URLs.
        let base = base_url.trim().trim_end_matches('/');
        let base = if base.is_empty() {
            super::DEFAULT_GATEWAY_URL
        } else {
            base
        };
        Self {
            http,
            base_url: base.to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        self.apply_auth(self.http.post(self.url(path)))
    }

    /// Bob Shell's auth scheme: JWT-shaped tokens use `Bearer`; static keys
    /// (bob_…) use `Apikey` plus `X-API-KEY` on Bob hosts. Non-Bob gateways
    /// keep the standard Bearer scheme.
    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let key = self.api_key.trim();
        if key.is_empty() {
            return req;
        }
        if looks_like_jwt(key) || !self.base_url.contains("bob.ibm.com") {
            return req.bearer_auth(key);
        }
        req.header("Authorization", format!("Apikey {key}"))
            .header("X-API-KEY", key)
    }

    /// Non-streaming completion. Token throughput is wall-clock based since
    /// OpenAI-style responses carry token counts but not decode duration.
    pub async fn chat(&self, messages: &[ChatTurn]) -> Result<ChatOutcome> {
        let started = std::time::Instant::now();
        let resp = self
            .request("/chat/completions")
            .json(&json!({ "model": self.model, "messages": messages, "stream": false }))
            .send()
            .await
            .context("gateway chat request failed — check the base URL")?;

        if !resp.status().is_success() {
            return Err(gateway_error(resp).await);
        }
        let value: serde_json::Value = resp.json().await.context("invalid gateway response")?;
        let text = value["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let stats = wall_clock_stats(&value, started);
        Ok(ChatOutcome { text, stats })
    }

    /// Streaming completion via SSE `data:` lines.
    pub async fn chat_stream<F>(
        &self,
        messages: &[ChatTurn],
        mut on_token: F,
    ) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
    {
        let started = std::time::Instant::now();
        let resp = self
            .request("/chat/completions")
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "stream": true,
                // Most gateways honor this; ones that don't just omit usage.
                "stream_options": { "include_usage": true },
            }))
            .send()
            .await
            .context("gateway chat request failed — check the base URL")?;

        if !resp.status().is_success() {
            return Err(gateway_error(resp).await);
        }

        let mut full = String::new();
        let mut usage_tokens: Option<u64> = None;
        let mut buf: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.context("error reading gateway stream")?;
            buf.extend_from_slice(&bytes);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                let Some(payload) = line.strip_prefix("data:") else {
                    continue;
                };
                let payload = payload.trim();
                if payload == "[DONE]" {
                    let stats = usage_tokens.map(|t| GenStats {
                        eval_count: t,
                        eval_duration_ns: started.elapsed().as_nanos() as u64,
                    });
                    return Ok(ChatOutcome { text: full, stats });
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) else {
                    continue;
                };
                if let Some(delta) = v["choices"][0]["delta"]["content"].as_str() {
                    if !delta.is_empty() {
                        on_token(delta);
                        full.push_str(delta);
                    }
                }
                if let Some(t) = v["usage"]["completion_tokens"].as_u64() {
                    usage_tokens = Some(t);
                }
            }
        }
        let stats = usage_tokens.map(|t| GenStats {
            eval_count: t,
            eval_duration_ns: started.elapsed().as_nanos() as u64,
        });
        Ok(ChatOutcome { text: full, stats })
    }

    /// Model ids from the gateway. Tries OpenAI's GET /models, then falls back
    /// to LiteLLM's GET /model/info (IBM Bob's shape: data[].model_name).
    pub async fn list_models(&self) -> Result<Vec<String>> {
        match self.get_json("/models").await {
            Ok(value) => {
                let models: Vec<String> = value["data"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m["id"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if !models.is_empty() {
                    return Ok(models);
                }
            }
            Err(e) => {
                // Fall through to /model/info; keep this error if both fail.
                let fallback = self.model_info_names().await;
                return fallback.map_err(|_| e);
            }
        }
        self.model_info_names().await
    }

    /// LiteLLM's /model/info listing.
    async fn model_info_names(&self) -> Result<Vec<String>> {
        let value = self.get_json("/model/info").await?;
        let models: Vec<String> = value["data"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m["model_name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }

    async fn get_json(&self, path: &str) -> Result<serde_json::Value> {
        let req = self.apply_auth(
            self.http
                .get(self.url(path))
                .timeout(std::time::Duration::from_secs(10)),
        );
        let resp = req
            .send()
            .await
            .with_context(|| format!("gateway {path} request failed"))?;
        if !resp.status().is_success() {
            return Err(gateway_error(resp).await);
        }
        resp.json()
            .await
            .with_context(|| format!("invalid {path} response"))
    }
}

/// Wall-clock GenStats from a non-streaming response's usage block.
fn wall_clock_stats(value: &serde_json::Value, started: std::time::Instant) -> Option<GenStats> {
    let tokens = value["usage"]["completion_tokens"].as_u64()?;
    if tokens == 0 {
        return None;
    }
    Some(GenStats {
        eval_count: tokens,
        eval_duration_ns: started.elapsed().as_nanos() as u64,
    })
}

/// Three dot-separated non-empty segments — the shape of a JWT.
fn looks_like_jwt(key: &str) -> bool {
    let parts: Vec<&str> = key.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty())
}

async fn gateway_error(resp: reqwest::Response) -> anyhow::Error {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let hint = match status.as_u16() {
        401 | 403 => " (check the API key and that it has the Inference scope)",
        404 => " (check the base URL — it usually ends in /v1)",
        _ => "",
    };
    anyhow!("gateway {}{}: {}", status, hint, truncate(&body, 300))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}
