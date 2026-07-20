//! App-facing AI facade. `AiConfig` is the persisted settings shape; `Ai`
//! delegates every capability through the inference router
//! (docs/RFC-inference-providers.md) — engines and chat types live in
//! `crate::inference`, re-exported here so call sites keep their imports.

use crate::inference::{ChatEngine, Embedder, Role, Router};
pub use crate::inference::{
    ChatOutcome, ChatTurn, EmbedderProgress, GenStats, LocalEmbedder, Ollama, OllamaConfig,
    OpenAiClient,
};

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
    /// curator's phase-5 pass, docs/RFC-note-curator.md). On by default —
    /// smart defaults over opt-ins; the pass is idle-gated, capped, and
    /// fully recoverable, so the toggle exists for cost control, not safety.
    #[serde(default = "default_true")]
    pub curator_consolidate: bool,
    /// Minutes between remote git re-sync probes (docs/RFC-git-sources.md
    /// §8); 0 disables auto-sync (manual Refresh always works). Git sources
    /// themselves have no off switch — the smarter thing is the only thing.
    #[serde(default = "default_git_sync_minutes")]
    pub git_sync_minutes: u32,
}

fn default_git_sync_minutes() -> u32 {
    60
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
            curator_consolidate: default_true(),
            git_sync_minutes: default_git_sync_minutes(),
        }
    }
}

/// Host context for Ai: where to keep downloaded assets and how to report
/// embedder download progress to the UI.
#[derive(Default, Clone)]
pub struct AiRuntime {
    pub data_dir: std::path::PathBuf,
    pub embedder_progress: Option<EmbedderProgress>,
}

/// App-facing capability facade over the inference Router. One instance
/// lives in AppState behind a RwLock and is rebuilt whenever the config is
/// saved.
pub struct Ai {
    config: AiConfig,
    router: Router,
    /// Ollama retained directly for the capabilities that haven't joined the
    /// router yet (OCR fallback, model listing).
    ollama: Ollama,
    /// Gateway client retained for vision + model listing when configured.
    openai: Option<OpenAiClient>,
}

fn ollama_config(config: &AiConfig) -> OllamaConfig {
    OllamaConfig {
        base_url: config.base_url.clone(),
        chat_model: config.chat_model.clone(),
        embed_model: config.embed_model.clone(),
        vision_model: config.vision_model.clone(),
    }
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
        let chat = match &openai {
            Some(gw) => ChatEngine::Gateway(gw.clone()),
            None => ChatEngine::Ollama(Ollama::new(ollama_config(&config))),
        };
        let data_dir = if runtime.data_dir.as_os_str().is_empty() {
            std::env::temp_dir().join("alchemy")
        } else {
            runtime.data_dir.clone()
        };
        let embedder = if config.embedder == "builtin" {
            Embedder::Builtin(LocalEmbedder::new(
                data_dir,
                runtime.embedder_progress.clone(),
            ))
        } else {
            Embedder::Ollama(Ollama::new(ollama_config(&config)))
        };
        let router = Router::new(chat, embedder);
        let ollama = Ollama::new(ollama_config(&config));
        Self {
            config,
            router,
            ollama,
            openai,
        }
    }

    /// Retrieval/context parameters for a role, resolved by the router
    /// against the active model tier (RFC-inference-providers §2).
    pub fn profile(&self, role: Role) -> crate::inference::ContextProfile {
        self.router.profile(role)
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
        self.router.chat_engine(Role::Chat).chat(messages).await
    }

    pub async fn chat_stream<F>(&self, messages: &[ChatTurn], on_token: F) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
    {
        self.router
            .chat_engine(Role::Chat)
            .chat_stream(messages, on_token)
            .await
    }

    /// Gateway model listing (provider == "openai"); Err when not applicable.
    pub async fn list_gateway_models(&self) -> Result<Vec<String>> {
        match &self.openai {
            Some(gw) => gw.list_models().await,
            None => Err(anyhow::anyhow!("no gateway configured")),
        }
    }

    // Embeddings route through the router's dedicated embedder — never a
    // preference ladder (vectors are index-coupled).
    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.router.embedder().embed(texts).await
    }
    pub async fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut v = self.embed(std::slice::from_ref(&text.to_string())).await?;
        v.pop()
            .ok_or_else(|| anyhow::anyhow!("embedder returned no vector"))
    }
    pub async fn test_embed(&self) -> Result<usize> {
        self.router.embedder().test_embed().await
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
