//! AI provider abstraction. `Ai` routes each capability to a backend:
//! chat/generation goes to Ollama or an OpenAI-compatible gateway (LM Studio,
//! vLLM, LiteLLM, Ollama's own /v1); embeddings, OCR, and model listing stay
//! on Ollama for now.

mod local_embed;
mod ollama;
mod openai;

pub use local_embed::{EmbedderProgress, LocalEmbedder};
pub use ollama::Ollama;
pub use openai::OpenAiClient;

use anyhow::Result;
use serde::{Deserialize, Serialize};

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
    /// Vision-capable gateway model for OCR (empty = OCR disabled).
    #[serde(default)]
    pub openai_vision_model: String,
    /// Who the user is; woven into system prompts so answers fit them.
    #[serde(default)]
    pub profile: UserProfile,
    /// Embedded MCP server for agent access (localhost-only streamable HTTP,
    /// see docs/RFC-mcp-server.md).
    #[serde(default = "default_true")]
    pub mcp_enabled: bool,
    #[serde(default = "default_mcp_port")]
    pub mcp_port: u16,
    /// Menu bar extra (tray icon). Settings → General toggles it live.
    #[serde(default = "default_true")]
    pub tray_enabled: bool,
    /// Weekly LLM consolidation of auto-created evidence notes (the note
    /// curator's phase-5 pass, docs/RFC-note-curator.md). Off by default:
    /// it spends tokens and rewrites note content.
    #[serde(default)]
    pub curator_consolidate: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserProfile {
    pub name: String,
    pub profession: String,
    /// Standing instructions, kept in mind across chats and generations.
    pub instructions: String,
}

fn default_provider() -> String {
    "ollama".to_string()
}

fn default_true() -> bool {
    true
}

fn default_mcp_port() -> u16 {
    41414
}

impl AiConfig {
    /// Is chat routed through the OpenAI-compatible gateway (large-context
    /// remote models) rather than local Ollama? Context-size budgets key off
    /// this in one place instead of scattering provider string comparisons.
    pub fn is_gateway(&self) -> bool {
        self.provider == "openai"
    }
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
            profile: UserProfile::default(),
            mcp_enabled: default_true(),
            mcp_port: default_mcp_port(),
            tray_enabled: default_true(),
            curator_consolidate: false,
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

/// Host context for Ai: where to keep downloaded assets and how to report
/// embedder download progress to the UI.
#[derive(Default, Clone)]
pub struct AiRuntime {
    pub data_dir: std::path::PathBuf,
    pub embedder_progress: Option<EmbedderProgress>,
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
    pub fn new(config: AiConfig, runtime: AiRuntime) -> Self {
        let openai = (config.provider == "openai").then(|| {
            OpenAiClient::new(
                config.openai_base_url.trim(),
                &config.openai_api_key,
                &config.openai_chat_model,
            )
        });
        let ollama = Ollama::new(config.clone());
        let data_dir = if runtime.data_dir.as_os_str().is_empty() {
            std::env::temp_dir().join("alchemy")
        } else {
            runtime.data_dir.clone()
        };
        let local_embed = (config.embedder == "builtin")
            .then(|| LocalEmbedder::new(data_dir, runtime.embedder_progress.clone()));
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
                let model = self.config.openai_vision_model.trim();
                if model.is_empty() {
                    anyhow::bail!(
                        "no vision model configured — set one in Settings → Models to enable OCR"
                    );
                }
                gw.ocr(image_base64, model).await
            }
            None => self.ollama.ocr(image_base64).await,
        }
    }
    pub async fn list_models(&self) -> Result<Vec<String>> {
        self.ollama.list_models().await
    }
}
