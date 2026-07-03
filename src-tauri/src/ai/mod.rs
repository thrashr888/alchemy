//! AI provider abstraction. `Ai` routes each capability to a backend:
//! chat/generation goes to Ollama or an OpenAI-compatible gateway (IBM Bob,
//! LM Studio, vLLM, Ollama's own /v1); embeddings, OCR, and model listing stay
//! on Ollama for now.

mod local_embed;
mod ollama;
mod openai;

pub use local_embed::LocalEmbedder;
pub use ollama::Ollama;
pub use openai::OpenAiClient;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// IBM Bob's OpenAI-compatible gateway (LiteLLM behind /inference/v1, per the
/// Bob Shell bundle's DEFAULT_BACKEND_URL); used whenever the URL field is empty.
pub const DEFAULT_GATEWAY_URL: &str = "https://api.us-east.bob.ibm.com/inference/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    /// Chat/generation backend: "ollama" | "openai".
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Embedding backend: "ollama" | "builtin" (bundled Model2Vec, no Ollama).
    #[serde(default = "default_provider")]
    pub embedder: String,
    pub base_url: String,
    pub chat_model: String,
    pub embed_model: String,
    /// Vision model used to OCR image sources (empty disables OCR).
    #[serde(default)]
    pub vision_model: String,
    /// OpenAI-compatible gateway settings (provider == "openai").
    #[serde(default)]
    pub openai_base_url: String,
    #[serde(default)]
    pub openai_api_key: String,
    #[serde(default)]
    pub openai_chat_model: String,
    /// Vision-capable gateway model for OCR (empty = sonnet-4.6 on Bob).
    #[serde(default)]
    pub openai_vision_model: String,
}

fn default_provider() -> String {
    "ollama".to_string()
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            embedder: default_provider(),
            base_url: "http://localhost:11434".to_string(),
            chat_model: "gpt-oss:120b".to_string(),
            embed_model: "nomic-embed-text:latest".to_string(),
            // OCR is opt-in: pick a vision model in Settings to enable it.
            vision_model: String::new(),
            openai_base_url: String::new(),
            openai_api_key: String::new(),
            openai_chat_model: String::new(),
            openai_vision_model: String::new(),
        }
    }
}

/// A single chat turn handed to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

impl ChatTurn {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    #[allow(dead_code)]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// Generation stats for one completion. Ollama reports true decode duration;
/// OpenAI-style gateways report token counts, timed by wall clock instead.
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

    pub(crate) fn from_parts(count: Option<u64>, duration: Option<u64>) -> Option<Self> {
        match (count, duration) {
            (Some(eval_count), Some(eval_duration_ns))
                if eval_count > 0 && eval_duration_ns > 0 =>
            {
                Some(GenStats {
                    eval_count,
                    eval_duration_ns,
                })
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

/// Capability router. One instance lives in AppState behind a RwLock and is
/// rebuilt whenever the config is saved.
pub struct Ai {
    config: AiConfig,
    ollama: Ollama,
    openai: Option<OpenAiClient>,
    local_embed: Option<LocalEmbedder>,
}

impl Ai {
    pub fn new(config: AiConfig) -> Self {
        let openai = (config.provider == "openai").then(|| {
            let base = if config.openai_base_url.trim().is_empty() {
                DEFAULT_GATEWAY_URL
            } else {
                config.openai_base_url.trim()
            };
            OpenAiClient::new(base, &config.openai_api_key, &config.openai_chat_model)
        });
        let ollama = Ollama::new(config.clone());
        let local_embed = (config.embedder == "builtin").then(LocalEmbedder::new);
        Self {
            config,
            ollama,
            openai,
            local_embed,
        }
    }

    pub fn config(&self) -> &AiConfig {
        &self.config
    }

    /// The model name answering chats right now (stats keying, health display).
    pub fn active_chat_model(&self) -> String {
        match &self.openai {
            Some(_) => self.config.openai_chat_model.clone(),
            None => self.config.chat_model.clone(),
        }
    }

    pub async fn chat(&self, messages: &[ChatTurn]) -> Result<ChatOutcome> {
        match &self.openai {
            Some(gw) => gw.chat(messages).await,
            None => self.ollama.chat(messages).await,
        }
    }

    pub async fn chat_stream<F>(&self, messages: &[ChatTurn], on_token: F) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
    {
        match &self.openai {
            Some(gw) => gw.chat_stream(messages, on_token).await,
            None => self.ollama.chat_stream(messages, on_token).await,
        }
    }

    /// Gateway model listing (provider == "openai"); Err when not applicable.
    pub async fn list_gateway_models(&self) -> Result<Vec<String>> {
        match &self.openai {
            Some(gw) => gw.list_models().await,
            None => Err(anyhow::anyhow!("no gateway configured")),
        }
    }

    // Embeddings route to the built-in Model2Vec embedder or Ollama.
    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        match &self.local_embed {
            Some(le) => le.embed(texts).await,
            None => self.ollama.embed(texts).await,
        }
    }
    pub async fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut v = self.embed(std::slice::from_ref(&text.to_string())).await?;
        v.pop()
            .ok_or_else(|| anyhow::anyhow!("embedder returned no vector"))
    }
    pub async fn test_embed(&self) -> Result<usize> {
        match &self.local_embed {
            Some(le) => le.test_embed().await,
            None => self.ollama.test_embed().await,
        }
    }
    pub async fn ocr(&self, image_base64: &str) -> Result<String> {
        match &self.openai {
            Some(gw) => {
                let model = if self.config.openai_vision_model.trim().is_empty() {
                    "sonnet-4.6"
                } else {
                    self.config.openai_vision_model.trim()
                };
                gw.ocr(image_base64, model).await
            }
            None => self.ollama.ocr(image_base64).await,
        }
    }
    pub async fn list_models(&self) -> Result<Vec<String>> {
        self.ollama.list_models().await
    }
}
