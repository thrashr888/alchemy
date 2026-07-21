//! Minimal OpenAI-compatible chat client (LM Studio, vLLM, LiteLLM-style
//! enterprise gateways, or Ollama's own /v1). Bearer-token auth by default,
//! with automatic handling of LiteLLM-style static-key schemes; SSE streaming.

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
    /// Gateway instance/team headers, fetched once from /admin/v1/profile
    /// (LiteLLM-style gateways that resolve billing from them).
    team_ctx: std::sync::Arc<tokio::sync::OnceCell<Option<TeamContext>>>,
}

#[derive(Clone)]
struct TeamContext {
    instance_id: String,
    team_id: String,
}

impl OpenAiClient {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("failed to build reqwest client");
        // An empty base is inferred from the key format for well-known
        // providers; otherwise it stays a clearly-invalid placeholder host so
        // request errors read as "configure the gateway URL", never as a
        // panic on a relative (schemeless) URL.
        let base = base_url.trim().trim_end_matches('/');
        let base = if base.is_empty() {
            default_base_for_key(api_key).unwrap_or("http://gateway-url-not-set.invalid/v1")
        } else {
            base
        };
        Self {
            http,
            base_url: base.to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            team_ctx: std::sync::Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    /// Some LiteLLM-style gateways resolve the caller's "team user" from
    /// x-instance-id / x-team-id headers. Fetch the first instance+team from
    /// the profile endpoint once per client; None everywhere else (detected
    /// by the gateway's static-key format).
    async fn team_context(&self) -> Option<TeamContext> {
        if !self.api_key.trim().starts_with("bob_") {
            return None;
        }
        self.team_ctx
            .get_or_init(|| async {
                let url = reqwest::Url::parse(&self.base_url).ok()?;
                let origin = format!(
                    "{}://{}{}",
                    url.scheme(),
                    url.host_str()?,
                    url.port().map(|p| format!(":{p}")).unwrap_or_default()
                );
                let req = self.apply_auth(
                    self.http
                        .get(format!("{origin}/admin/v1/profile"))
                        .timeout(std::time::Duration::from_secs(10)),
                );
                let resp = req.send().await.ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                let v: serde_json::Value = resp.json().await.ok()?;
                let inst = v["instances"].as_array()?.first()?;
                let instance_id = inst["instance_id"].as_str()?.to_string();
                let team_id = inst["teams"]
                    .as_array()
                    .and_then(|t| t.first())
                    .and_then(|t| t["id"].as_str())
                    .unwrap_or_default()
                    .to_string();
                Some(TeamContext {
                    instance_id,
                    team_id,
                })
            })
            .await
            .clone()
    }

    /// Attach gateway team headers when applicable.
    async fn with_team_headers(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ctx) = self.team_context().await {
            req = req.header("x-instance-id", &ctx.instance_id);
            if !ctx.team_id.is_empty() {
                req = req.header("x-team-id", &ctx.team_id);
            }
        }
        req
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        self.apply_auth(self.http.post(self.url(path)))
    }

    /// Standard Bearer auth, with two key-format-detected exceptions (so any
    /// host works): LiteLLM-style static keys (`bob_…`) expect `Apikey` plus
    /// `X-API-KEY`; Anthropic keys (`sk-ant-…`) get `x-api-key` + version
    /// headers alongside Bearer, because the OpenAI-compat chat endpoint takes
    /// Bearer but native endpoints like GET /models only accept `x-api-key`.
    /// JWT-shaped tokens always use plain Bearer.
    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let key = self.api_key.trim();
        if key.is_empty() {
            return req;
        }
        if looks_like_jwt(key) {
            return req.bearer_auth(key);
        }
        if key.starts_with("bob_") {
            return req
                .header("Authorization", format!("Apikey {key}"))
                .header("X-API-KEY", key);
        }
        if key.starts_with("sk-ant-") {
            return req
                .bearer_auth(key)
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01");
        }
        req.bearer_auth(key)
    }

    /// Non-streaming completion. Token throughput is wall-clock based since
    /// OpenAI-style responses carry token counts but not decode duration.
    pub async fn chat(&self, messages: &[ChatTurn]) -> Result<ChatOutcome> {
        let started = std::time::Instant::now();
        let resp = self
            .with_team_headers(self.request("/chat/completions"))
            .await
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
        Ok(ChatOutcome {
            text,
            stats,
            cost_usd: None,
        })
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
            .with_team_headers(self.request("/chat/completions"))
            .await
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
                    return Ok(ChatOutcome {
                        text: full,
                        stats,
                        cost_usd: None,
                    });
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
        Ok(ChatOutcome {
            text: full,
            stats,
            cost_usd: None,
        })
    }

    /// OCR an image via a vision-capable chat model (OpenAI image_url parts;
    /// LiteLLM translates for Anthropic/Google backends).
    pub async fn ocr(&self, image_base64: &str, model: &str) -> Result<String> {
        use base64::Engine;
        let mime = base64::engine::general_purpose::STANDARD
            .decode(&image_base64.as_bytes()[..image_base64.len().min(24)])
            .ok()
            .map(|head| sniff_mime(&head))
            .unwrap_or("image/png");
        let started = std::time::Instant::now();
        let _ = started;
        let resp = self
            .with_team_headers(self.request("/chat/completions"))
            .await
            .timeout(std::time::Duration::from_secs(180))
            .json(&json!({
                "model": model,
                "messages": [{
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Transcribe ALL text in this image exactly, preserving reading order and line breaks. Output only the transcribed text with no commentary. If there is no text, output nothing."},
                        {"type": "image_url", "image_url": {"url": format!("data:{mime};base64,{image_base64}")}}
                    ]
                }],
            }))
            .send()
            .await
            .context("gateway OCR request failed")?;
        if !resp.status().is_success() {
            return Err(gateway_error(resp).await);
        }
        let value: serde_json::Value = resp.json().await.context("invalid OCR response")?;
        Ok(value["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// Model ids from the gateway. Tries OpenAI's GET /models, then falls back
    /// to LiteLLM's GET /model/info (data[].model_name).
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
        let req = self
            .with_team_headers(
                self.apply_auth(
                    self.http
                        .get(self.url(path))
                        .timeout(std::time::Duration::from_secs(10)),
                ),
            )
            .await;
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

/// Sniff an image mime type from magic bytes (default png).
fn sniff_mime(head: &[u8]) -> &'static str {
    if head.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if head.starts_with(b"GIF8") {
        "image/gif"
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/png"
    }
}

/// Providers recognizable by their key format, so the URL field can stay
/// empty for them. Order matters: the specific `sk-ant-`/`sk-or-` prefixes
/// must match before the generic `sk-`.
fn default_base_for_key(key: &str) -> Option<&'static str> {
    let key = key.trim();
    if key.starts_with("sk-ant-") {
        Some("https://api.anthropic.com/v1")
    } else if key.starts_with("sk-or-") {
        Some("https://openrouter.ai/api/v1")
    } else if key.starts_with("gsk_") {
        Some("https://api.groq.com/openai/v1")
    } else if key.starts_with("bob_") {
        Some("https://api.us-east.bob.ibm.com/inference/v1")
    } else if key.starts_with("sk-") {
        Some("https://api.openai.com/v1")
    } else {
        None
    }
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
        // A 404 naming a function/model means auth worked and the model id
        // is stale or not enabled for this account — a bare 404 is a URL.
        404 if body.contains("unction") || body.contains("model") => {
            " (the selected model isn't available on this account — pick another in Settings)"
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_formats_infer_provider_urls() {
        assert_eq!(
            default_base_for_key("sk-ant-abc"),
            Some("https://api.anthropic.com/v1")
        );
        assert_eq!(
            default_base_for_key("sk-or-v1-abc"),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(
            default_base_for_key("gsk_abc"),
            Some("https://api.groq.com/openai/v1")
        );
        // Generic sk- matches only after the specific prefixes.
        assert_eq!(
            default_base_for_key("sk-abc123"),
            Some("https://api.openai.com/v1")
        );
        assert!(default_base_for_key("bob_prod_x").is_some());
        assert!(default_base_for_key("something-else").is_none());
        assert!(default_base_for_key("").is_none());
    }
}
