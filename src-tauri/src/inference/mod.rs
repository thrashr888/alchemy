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
pub mod budget;
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
    /// Ceiling for corpus-size-adaptive retrieval (`retrieve_k_for`): the
    /// budget line the corpus can grow k toward but never past.
    pub retrieve_k_max: usize,
    /// Char budget for the full-corpus source manifest in the chat prompt.
    /// Big notebooks (hundreds of git-source files) would otherwise stuff
    /// tens of KB of titles into every prompt — fatal for the ~4k-token
    /// on-device model, waste for everyone else.
    pub manifest_chars: usize,
    /// Rolling window of prior conversation turns included in the prompt.
    pub history_turns: usize,
    /// Post-rank ordinal ±1 excerpt expansion in the chat prompt
    /// (RFC-infinite-context §3): ON where the window affords context, OFF
    /// where the window itself is the constraint.
    pub neighbor_expansion: bool,
    /// Gist rows admitted to fusion as their own capped class
    /// (RFC-infinite-context §1, §5). One distilled overview orients the
    /// answer; a second spends the tight budget twice on redundancy — its
    /// source's verbatim chunks already carry the specifics — so on-device
    /// takes one, everyone else two.
    pub max_gists: usize,
    /// Max Small-role extracts in the Phase 4 global map-reduce
    /// (RFC-infinite-context §4, §5). Each extract is a sequential Small call
    /// against one source; the on-device tier runs them on the same
    /// single-tenant engine that answers, so a narrower fan-out keeps the
    /// global route from starving the synthesis it feeds.
    pub global_fan_out: usize,
    /// Hard-cap prompt excerpt bodies at 500 chars (RFC-infinite-context §5).
    /// The frontier tier reads full passages; the ~4k-token on-device window
    /// cannot afford long excerpts and answers better from tight, numbered
    /// evidence. Prompt text only — persisted citations/snippets never change.
    pub compact_excerpts: bool,
}

impl Default for ContextProfile {
    fn default() -> Self {
        Self {
            retrieve_k: 8,
            retrieve_k_max: 16,
            manifest_chars: 24_000,
            history_turns: 6,
            neighbor_expansion: true,
            max_gists: 2,
            global_fan_out: 6,
            compact_excerpts: false,
        }
    }
}

impl ContextProfile {
    /// Corpus-size-adaptive retrieval depth (RFC-infinite-context §3): a
    /// fixed k=8 covers 0.13% of a 10M-char corpus, so k grows by one per
    /// doubling of corpus text beyond ~200k chars, capped by the profile
    /// budget. 50k chars → base k; 3M → base+3; 10M → base+5.
    pub fn retrieve_k_for(&self, corpus_chars: i64) -> usize {
        let extra = if corpus_chars > 200_000 {
            (corpus_chars as f64 / 200_000.0).log2().floor() as usize
        } else {
            0
        };
        (self.retrieve_k + extra).min(self.retrieve_k_max)
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

/// Result of a chat: the assistant text plus optional generation stats and,
/// for metered providers (agent CLIs report it), the dollar cost.
#[derive(Default)]
pub struct ChatOutcome {
    pub text: String,
    pub stats: Option<GenStats>,
    pub cost_usd: Option<f64>,
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
#[derive(Clone)]
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
#[derive(Clone)]
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
#[derive(Clone)]
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
            // On-device model: ~4k-token window. k=4 holds ≥75% recall in
            // eval_context_profiles; the tight manifest/history budgets keep
            // big notebooks from overflowing the window before the question.
            ChatEngine::FoundationModels(_) => ContextProfile {
                retrieve_k: 4,
                // Even a huge corpus can't push the ~4k-token window past
                // six passages, and neighbor expansion would blow it — the
                // on-device model gets depth from ranking, not breadth.
                retrieve_k_max: 6,
                manifest_chars: 2_000,
                history_turns: 2,
                neighbor_expansion: false,
                // One overview is orientation; a second is budget spent twice.
                max_gists: 1,
                // Extracts share the single-tenant engine that synthesizes.
                global_fan_out: 3,
                // The ~4k-token window answers better from tight excerpts.
                compact_excerpts: true,
            },
            _ => ContextProfile::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC-infinite-context §3: k grows one per doubling past ~200k chars,
    /// never past the profile budget, never below the base.
    #[test]
    fn retrieve_k_scales_with_corpus_inside_budget() {
        let p = ContextProfile::default(); // k=8, max 16
        assert_eq!(p.retrieve_k_for(0), 8);
        assert_eq!(
            p.retrieve_k_for(50_000),
            8,
            "small notebooks keep the base k"
        );
        assert_eq!(p.retrieve_k_for(200_000), 8, "the knee is exclusive");
        assert_eq!(p.retrieve_k_for(800_000), 10);
        assert_eq!(p.retrieve_k_for(3_000_000), 11);
        assert_eq!(p.retrieve_k_for(10_000_000), 13);
        assert_eq!(
            p.retrieve_k_for(i64::MAX),
            16,
            "budget ceiling holds at any corpus size"
        );

        let tight = ContextProfile {
            retrieve_k: 4,
            retrieve_k_max: 6,
            manifest_chars: 2_000,
            history_turns: 2,
            neighbor_expansion: false,
            max_gists: 1,
            global_fan_out: 3,
            compact_excerpts: true,
        };
        assert_eq!(tight.retrieve_k_for(50_000), 4);
        assert_eq!(
            tight.retrieve_k_for(10_000_000),
            6,
            "on-device budget caps a 10M corpus at six passages"
        );
    }

    /// RFC-infinite-context §5: the two tiers diverge only where the eval
    /// evidence says so. The default tier reproduces today's fixed constants
    /// exactly (byte-for-byte prompt/retrieval equivalence depends on it); the
    /// on-device tier tightens each evidence-shape knob.
    #[test]
    fn evidence_shape_tiers_match_their_constants() {
        // Default tier: today's hardcoded values (the meta SearchOptions
        // literal's max_gists 2, the old global fan-out 6, uncapped excerpts).
        let d = ContextProfile::default();
        assert_eq!(d.max_gists, 2, "default gist budget is today's constant");
        assert_eq!(d.global_fan_out, 6, "default fan-out is today's constant");
        assert!(!d.compact_excerpts, "default reads full excerpts");

        // On-device tier: one overview, half the fan-out, capped excerpts.
        // Construction is pure (probe/model load are lazy), so a dummy path
        // is enough to resolve the FoundationModels tier's profile.
        let router = Router::new(
            ChatEngine::FoundationModels(FmEngine::new("fm-sidecar".into())),
            Embedder::Builtin(LocalEmbedder::new("data".into(), None)),
            None,
            None,
        );
        let p = router.profile(Role::Chat);
        assert_eq!(p.max_gists, 1, "on-device admits one overview gist");
        assert_eq!(p.global_fan_out, 3, "on-device narrows the global fan-out");
        assert!(p.compact_excerpts, "on-device caps excerpt bodies");
    }
}
