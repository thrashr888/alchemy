//! App-facing AI facade. `AiConfig` is the persisted settings shape; `Ai`
//! delegates every capability through the inference router
//! (docs/RFC-inference-providers.md) — engines and chat types live in
//! `crate::inference`, re-exported here so call sites keep their imports.

pub use crate::inference::Role;
use crate::inference::{AgentCli, AgentKind, ChatEngine, Embedder, FmEngine, Router};

pub use crate::inference::{
    ChatOutcome, ChatTurn, EmbedderProgress, GenStats, LocalEmbedder, Ollama, OllamaConfig,
    OpenAiClient,
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// One configured inference provider (RFC-inference-providers §8: a list,
/// not a form). `kind` picks the engine family; gateway/ollama entries carry
/// connection fields, agent entries need none (the CLI is the credential).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderEntry {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub base_url: String,
    pub api_key: String,
    pub chat_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    /// Configured providers; empty on legacy configs until `normalize`
    /// synthesizes entries from the flat fields below.
    #[serde(default)]
    pub providers: Vec<ProviderEntry>,
    /// Provider id answering notebook chat.
    #[serde(default)]
    pub chat_provider: String,
    /// Provider id for studio generation (artifacts, reports, audio
    /// scripts) — the Generate role.
    #[serde(default)]
    pub studio_provider: String,
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
    /// Which engine runs image OCR: "" (off) | "ollama" | "gateway".
    /// Deliberately independent of chat — vision has its own requirements.
    #[serde(default)]
    pub vision_provider: String,
    /// First-run model chooser dismissed (chosen or skipped) — the three-door
    /// pane shows until this flips.
    #[serde(default)]
    pub setup_seen: bool,
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

    pub fn provider_by_id(&self, id: &str) -> Option<&ProviderEntry> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Bring any config into list shape: legacy flat fields synthesize
    /// entries once, and the flat fields are re-mirrored from the selected
    /// chat provider so every existing call site (`is_gateway`, context
    /// budgets, gateway model listing) keeps working unchanged.
    pub fn normalize(&mut self) {
        // "On this Mac" is always listed; readiness is probed live by the
        // UI, and selecting it on an unsupported Mac falls back to Ollama.
        let has_fm = self.providers.iter().any(|p| p.kind == "fm");
        if !has_fm {
            self.providers.insert(
                0,
                ProviderEntry {
                    id: "on-device".into(),
                    kind: "fm".into(),
                    label: "On this Mac".into(),
                    ..Default::default()
                },
            );
        }
        if self.providers.iter().all(|p| p.kind == "fm") {
            self.providers.push(ProviderEntry {
                id: "ollama".into(),
                kind: "ollama".into(),
                label: "Ollama".into(),
                base_url: self.base_url.clone(),
                api_key: String::new(),
                chat_model: self.chat_model.clone(),
            });
            if !self.openai_base_url.trim().is_empty() || !self.openai_api_key.is_empty() {
                self.providers.push(ProviderEntry {
                    id: "gateway".into(),
                    kind: "gateway".into(),
                    label: "Gateway".into(),
                    base_url: self.openai_base_url.clone(),
                    api_key: self.openai_api_key.clone(),
                    chat_model: self.openai_chat_model.clone(),
                });
            }
            for agent in ["claude", "codex"] {
                if self.provider == agent {
                    self.providers.push(ProviderEntry {
                        id: agent.into(),
                        kind: if agent == "claude" {
                            "claude-code".into()
                        } else {
                            "codex".into()
                        },
                        label: if agent == "claude" {
                            "Claude Code".into()
                        } else {
                            "Codex".into()
                        },
                        ..Default::default()
                    });
                }
            }
            self.chat_provider = match self.provider.as_str() {
                "openai" => "gateway".into(),
                "claude" | "codex" => self.provider.clone(),
                _ => "ollama".into(),
            };
        }
        if self.chat_provider.is_empty() || self.provider_by_id(&self.chat_provider).is_none() {
            self.chat_provider = self
                .providers
                .first()
                .map(|p| p.id.clone())
                .unwrap_or_else(|| "ollama".into());
        }
        if self.studio_provider.is_empty() || self.provider_by_id(&self.studio_provider).is_none() {
            self.studio_provider = self.chat_provider.clone();
        }
        if self.vision_provider.is_empty() {
            if self.is_gateway() && !self.openai_vision_model.trim().is_empty() {
                self.vision_provider = "gateway".into();
            } else if !self.vision_model.trim().is_empty() {
                self.vision_provider = "ollama".into();
            }
        }
        // A config that already has real setup predates the first-run pane —
        // never show onboarding to a configured install (the flag ships
        // false in old configs).
        if !self.setup_seen {
            let configured = self.providers.iter().any(|p| {
                !p.api_key.is_empty() || crate::inference::AgentKind::from_id(&p.kind).is_some()
            });
            if configured || self.provider != "ollama" {
                self.setup_seen = true;
            }
        }
        // Mirror the selected chat entry back into the flat legacy fields.
        if let Some(entry) = self.provider_by_id(&self.chat_provider).cloned() {
            match entry.kind.as_str() {
                "gateway" => {
                    self.provider = "openai".into();
                    self.openai_base_url = entry.base_url;
                    self.openai_api_key = entry.api_key;
                    self.openai_chat_model = entry.chat_model;
                }
                "ollama" => {
                    self.provider = "ollama".into();
                    if !entry.base_url.trim().is_empty() {
                        self.base_url = entry.base_url;
                    }
                    if !entry.chat_model.trim().is_empty() {
                        self.chat_model = entry.chat_model;
                    }
                }
                kind => {
                    self.provider = match kind {
                        "claude-code" => "claude".into(),
                        "codex" => "codex".into(),
                        other => other.to_string(),
                    };
                }
            }
        }
    }
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            chat_provider: String::new(),
            studio_provider: String::new(),
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
            vision_provider: String::new(),
            setup_seen: false,
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
    /// Path to the alchemy-fm sidecar binary when the host resolved one
    /// (bundled resource in release, repo build in dev). None = no
    /// Foundation Models rung; Small falls through to the chat engine.
    pub fm_sidecar: Option<std::path::PathBuf>,
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
        let fm_path = runtime.fm_sidecar.clone();
        let engine_for = |entry: &ProviderEntry| -> ChatEngine {
            match entry.kind.as_str() {
                "fm" => match fm_path.as_ref().filter(|p| p.exists()) {
                    Some(p) => ChatEngine::FoundationModels(FmEngine::new(p.clone())),
                    // Sidecar missing (pre-26 macOS, unbundled build): fall
                    // back to Ollama so a stale selection can't dead-end.
                    None => ChatEngine::Ollama(Ollama::new(ollama_config(&config))),
                },
                "gateway" => ChatEngine::Gateway(OpenAiClient::new(
                    entry.base_url.trim(),
                    &entry.api_key,
                    &entry.chat_model,
                )),
                kind => match AgentKind::from_id(kind) {
                    // Family B: the vendor CLI carries the subscription.
                    Some(agent) => ChatEngine::Agent(AgentCli::new(agent)),
                    None => {
                        let mut oc = ollama_config(&config);
                        if !entry.base_url.trim().is_empty() {
                            oc.base_url = entry.base_url.clone();
                        }
                        if !entry.chat_model.trim().is_empty() {
                            oc.chat_model = entry.chat_model.clone();
                        }
                        ChatEngine::Ollama(Ollama::new(oc))
                    }
                },
            }
        };
        let chat = config
            .provider_by_id(&config.chat_provider)
            .map(&engine_for)
            .unwrap_or_else(|| ChatEngine::Ollama(Ollama::new(ollama_config(&config))));
        // Studio (Generate role) gets its own engine only when it differs —
        // same-provider stays one engine, one stats key.
        let generate = (config.studio_provider != config.chat_provider)
            .then(|| {
                config
                    .provider_by_id(&config.studio_provider)
                    .map(&engine_for)
            })
            .flatten();
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
        // Small-role rung: the FM sidecar when the host found the binary.
        // Availability (macOS version, Apple Intelligence state) is probed
        // lazily on first use; unavailable probes make chat_role fall
        // through, so constructing the engine here is always safe.
        let small = runtime
            .fm_sidecar
            .as_ref()
            .filter(|p| p.exists())
            .map(|p| ChatEngine::FoundationModels(FmEngine::new(p.clone())));
        let router = Router::new(chat, embedder, small, generate);
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
        match self
            .config
            .provider_by_id(&self.config.chat_provider)
            .map(|p| (p.kind.clone(), p.chat_model.clone()))
        {
            Some((kind, model)) => match kind.as_str() {
                "gateway" | "ollama" if !model.trim().is_empty() => model,
                "gateway" => self.config.openai_chat_model.clone(),
                "ollama" => self.config.chat_model.clone(),
                other => other.to_string(),
            },
            None => self.config.chat_model.clone(),
        }
    }

    pub async fn chat(&self, messages: &[ChatTurn]) -> Result<ChatOutcome> {
        self.router.chat_engine(Role::Chat).chat(messages).await
    }

    /// Role-routed chat with failure fallthrough (RFC-inference-providers
    /// §7): if the role's engine is unavailable or errors, the configured
    /// chat engine answers instead — one log line, never a dead call.
    pub async fn chat_role(&self, role: Role, messages: &[ChatTurn]) -> Result<ChatOutcome> {
        let engine = self.router.chat_engine(role);
        if role == Role::Generate {
            return engine.chat(messages).await;
        }
        if self.router.has_small() && role == Role::Small {
            if let ChatEngine::FoundationModels(fm) = engine {
                if fm.available().await {
                    match engine.chat(messages).await {
                        Ok(out) => return Ok(out),
                        Err(err) => {
                            eprintln!("small-role engine failed, falling through: {err:#}");
                        }
                    }
                }
            }
        }
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

    /// Streaming, role-routed (studio generation → the Generate provider).
    pub async fn chat_role_stream<F>(
        &self,
        role: Role,
        messages: &[ChatTurn],
        on_token: F,
    ) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
    {
        self.router
            .chat_engine(role)
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
        match self.config.vision_provider.as_str() {
            "gateway" => {
                let model = self.config.openai_vision_model.trim();
                if model.is_empty() {
                    anyhow::bail!(
                        "no vision model configured — set one in Settings → Models to enable OCR"
                    );
                }
                let gw = self.openai.clone().unwrap_or_else(|| {
                    OpenAiClient::new(
                        self.config.openai_base_url.trim(),
                        &self.config.openai_api_key,
                        &self.config.openai_chat_model,
                    )
                });
                gw.ocr(image_base64, model).await
            }
            "ollama" => self.ollama.ocr(image_base64).await,
            _ => anyhow::bail!("OCR is off — pick a vision engine in Settings → Models → Advanced"),
        }
    }
    pub async fn list_models(&self) -> Result<Vec<String>> {
        self.ollama.list_models().await
    }
}
