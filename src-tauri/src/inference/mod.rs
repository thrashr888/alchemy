//! Inference seam (docs/RFC-inference-providers.md): roles, engines, and the
//! router that assigns one to the other. Workspace-crate-shaped on purpose —
//! nothing in this module may import Tauri or app types; the `crate::ai`
//! facade is its first consumer, and extraction into a standalone crate is
//! triggered by a second consumer in another repo, not before.
//!
//! Three provider families plug in here: local engines (builtin embedder,
//! Ollama today; MLX next), agent CLIs (claude/codex, phase 3), and
//! OpenAI-compatible gateways. Streaming is an invariant: chat-shaped
//! engines expose `chat_stream`; plain `chat` is just the collected stream.

mod agent_cli;
mod fm;
mod gateway;
mod local_embed;
mod ollama;

pub use agent_cli::{agent_status, AgentCli, AgentKind};
pub use fm::FmEngine;
pub use gateway::OpenAiClient;
pub use local_embed::{EmbedderProgress, LocalEmbedder};
pub use ollama::Ollama;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// What a call site needs, not which model it wants. The router maps each
/// role to the best available engine given user preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // later-phase roles (Agent/Small/Vision…) arrive with their engine families
pub enum Role {
    /// Interactive notebook chat: streaming, quality, citations.
    Chat,
    /// Multi-step tool use (deep research, curator consolidation).
    Agent,
    /// Long-form studio generation (artifacts, reports, audio scripts).
    Generate,
    /// Fast cheap jobs: titles, summaries, router hints.
    Small,
    /// Ingest/retrieval embeddings — index-coupled, never routed dynamically.
    Embed,
    /// Image OCR.
    Vision,
}

/// Retrieval/context parameters tuned to the resolved model, not just the
/// query (RFC §2): a 4B model wants fewer, better-ranked passages in a tight
/// budget; a 120B MoE can afford breadth. Phase-1 values reproduce today's
/// fixed constants exactly; tiers diverge only when the eval suite says so.
#[derive(Debug, Clone, Copy)]
pub struct ContextProfile {
    /// Top-k passages retrieved for chat grounding.
    pub retrieve_k: usize,
}

impl Default for ContextProfile {
    fn default() -> Self {
        Self { retrieve_k: 8 }
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

/// The slice of app config an Ollama engine needs — inference stays free of
/// app-level config types.
#[derive(Debug, Clone, Default)]
pub struct OllamaConfig {
    pub base_url: String,
    pub chat_model: String,
    pub embed_model: String,
    pub vision_model: String,
}

/// Chat-capable engines. Enum dispatch rather than `dyn Trait` because
/// `chat_stream` is generic over its token sink; new families (MLX, agent
/// CLIs) become variants here.
pub enum ChatEngine {
    Ollama(Ollama),
    Gateway(OpenAiClient),
    /// Apple's on-device system model via the alchemy-fm sidecar (macOS 26+).
    FoundationModels(FmEngine),
    /// A subscription-carrying agent CLI run headless (claude, codex).
    Agent(AgentCli),
}

impl ChatEngine {
    /// Stable identifier for stats keying and provider attribution (used by
    /// the availability probes when the preference ladder lands).
    #[allow(dead_code)]
    pub fn id(&self) -> &'static str {
        match self {
            ChatEngine::Ollama(_) => "ollama",
            ChatEngine::Gateway(_) => "gateway",
            ChatEngine::FoundationModels(_) => "foundation-models",
            ChatEngine::Agent(a) => a.kind().id(),
        }
    }

    /// Streaming is the invariant; `chat` below is just the collected stream.
    pub async fn chat_stream<F>(&self, messages: &[ChatTurn], on_token: F) -> Result<ChatOutcome>
    where
        F: FnMut(&str),
    {
        match self {
            ChatEngine::Ollama(o) => o.chat_stream(messages, on_token).await,
            ChatEngine::Gateway(g) => g.chat_stream(messages, on_token).await,
            ChatEngine::FoundationModels(f) => f.chat_stream(messages, on_token).await,
            ChatEngine::Agent(a) => a.chat_stream(messages, on_token).await,
        }
    }

    pub async fn chat(&self, messages: &[ChatTurn]) -> Result<ChatOutcome> {
        match self {
            ChatEngine::Ollama(o) => o.chat(messages).await,
            ChatEngine::Gateway(g) => g.chat(messages).await,
            ChatEngine::FoundationModels(f) => f.chat(messages).await,
            ChatEngine::Agent(a) => a.chat(messages).await,
        }
    }
}

/// Embedding engines. Kept apart from chat: vectors are coupled to the
/// index, so `Embed` never falls through a preference ladder.
pub enum Embedder {
    Builtin(LocalEmbedder),
    Ollama(Ollama),
}

impl Embedder {
    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        match self {
            Embedder::Builtin(le) => le.embed(texts).await,
            Embedder::Ollama(o) => o.embed(texts).await,
        }
    }

    pub async fn test_embed(&self) -> Result<usize> {
        match self {
            Embedder::Builtin(le) => le.test_embed().await,
            Embedder::Ollama(o) => o.test_embed().await,
        }
    }
}

/// Per-role engine assignment. Phase 1 mirrors today's behavior: one chat
/// engine serves every chat-shaped role and profiles are uniform. The
/// preference ladders, availability probing, and per-tier profiles land with
/// the new engine families.
pub struct Router {
    chat: ChatEngine,
    embedder: Embedder,
    /// Small-role engine when one is available (the FM sidecar today).
    /// Callers fall through to `chat` when it is None or errors.
    small: Option<ChatEngine>,
    /// Generate-role engine when the studio provider differs from chat.
    generate: Option<ChatEngine>,
}

impl Router {
    pub fn new(
        chat: ChatEngine,
        embedder: Embedder,
        small: Option<ChatEngine>,
        generate: Option<ChatEngine>,
    ) -> Self {
        Self {
            chat,
            embedder,
            small,
            generate,
        }
    }

    pub fn chat_engine(&self, role: Role) -> &ChatEngine {
        match role {
            Role::Small => self.small.as_ref().unwrap_or(&self.chat),
            Role::Generate => self.generate.as_ref().unwrap_or(&self.chat),
            _ => &self.chat,
        }
    }

    /// Whether Small currently has its own engine (vs falling through).
    pub fn has_small(&self) -> bool {
        self.small.is_some()
    }

    pub fn embedder(&self) -> &Embedder {
        &self.embedder
    }

    pub fn profile(&self, role: Role) -> ContextProfile {
        // The on-device model's small context wants fewer, tighter passages
        // (RFC §2: retrieval tunes to the resolved model).
        match self.chat_engine(role) {
            ChatEngine::FoundationModels(_) => ContextProfile { retrieve_k: 4 },
            _ => ContextProfile::default(),
        }
    }
}
