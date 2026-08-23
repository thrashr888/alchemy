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
    /// Reasoning effort for this provider, empty = the provider's own default.
    /// Only meaningful where the engine has somewhere to put it (see
    /// `inference::efforts_for`); every other provider hides the control.
    /// `serde(default)` on the struct keeps older configs loading.
    pub effort: String,
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
    /// Ollama model answering the Small role — gists, tags, Weave verdicts,
    /// registry suggestions. Empty keeps the previous behaviour: Apple
    /// Foundation Models when the sidecar is available, otherwise the chat
    /// provider. A local 8–12B here is usually the right call: the Small
    /// role is high-volume and its jobs are short and structured, so paying
    /// chat-model latency for them is waste.
    #[serde(default)]
    pub small_model: String,
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
    /// Which hosted coding agent the notebook Agent view opens with
    /// (docs/RFC-acp-agents.md); an `acp_agents` id. Empty means "whichever
    /// is installed", so a fresh machine needs no choice made for it.
    #[serde(default)]
    pub hosted_agent: String,
    /// Browser-extension clip receiver (localhost-only; accepts a rendered
    /// DOM from the user's logged-in tab, see docs/RFC-page-capture.md §8).
    /// Default-on, same as MCP — the toggle exists for anyone who wants no
    /// localhost surface, not for safety (the endpoint is origin-gated).
    #[serde(default = "default_true")]
    pub clip_enabled: bool,
    #[serde(default = "default_clip_port")]
    pub clip_port: u16,
    /// Menu bar extra (tray icon). Settings → General toggles it live.
    /// Also the residency switch: with the tray on, closing the last window
    /// leaves Alchemy running in the menu bar (docs/RFC-night-shift.md);
    /// with it off, window close quits as before.
    #[serde(default = "default_true")]
    pub tray_enabled: bool,
    /// The Night Shift master switch (docs/RFC-night-shift.md): scheduled
    /// reports and automatic source resyncs from the resident scheduler.
    /// Off means on-demand only — Run Now and manual Refresh still work.
    #[serde(default = "default_true")]
    pub background_enabled: bool,
    /// Desktop notifications (report ready, work finishing). Lives in config
    /// rather than webview localStorage so the resident scheduler can honor
    /// it with no window open; the frontend mirrors it for its own checks.
    #[serde(default = "default_true")]
    pub show_notifications: bool,
    /// Quiet-while-focused rule: skip notifications (and the frontend's
    /// sound cues) while an Alchemy window is focused — the user is already
    /// looking. On by default; off means always deliver. Checked at send
    /// time by `scheduler::notifications_wanted`.
    #[serde(default = "default_true")]
    pub quiet_when_focused: bool,
    /// Weekly LLM consolidation of auto-created evidence notes (the note
    /// curator's phase-5 pass, docs/RFC-note-curator.md). On by default —
    /// smart defaults over opt-ins; the pass is idle-gated, capped, and
    /// fully recoverable, so the toggle exists for cost control, not safety.
    #[serde(default = "default_true")]
    pub curator_consolidate: bool,
    /// Master switch for the background distillation family
    /// (docs/RFC-infinite-context.md): source gists (Phase 1) and per-chunk
    /// embedding enrichment for low-density page captures (Phase 2) both ride
    /// this one flag — they share the same fire-and-forget sweep. On by
    /// default — smart defaults over opt-ins; the sweep is budgeted, gated,
    /// and self-healing, so the toggle exists for cost control, not safety.
    #[serde(default = "default_true")]
    pub source_gists: bool,
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
    /// Notion internal-integration token (`ntn_…`) — sent only to
    /// api.notion.com; empty means Notion URLs fall through to page capture.
    #[serde(default)]
    pub notion_token: String,
    /// Diagnose-and-suggest on unclassified provider errors
    /// (docs/RFC-self-resolve.md phase 2): one Small-role call turns the raw
    /// error into a plain-language diagnosis plus clickable fixes. On by
    /// default — the toggle exists for cost control, not opt-in; phase 1's
    /// deterministic classifier keeps working either way.
    #[serde(default = "default_true")]
    pub self_diagnose: bool,
    /// Source hygiene (docs/RFC-source-hygiene.md): the budgeted background
    /// sweep that re-fetches aging url sources and flags unreachable ones.
    /// On by default — the toggle is cost control, not opt-in; removals are
    /// only ever proposed, never automatic.
    #[serde(default = "default_true")]
    pub source_hygiene: bool,
    /// Days before a url source counts as stale and the hygiene sweep
    /// re-fetches it from its origin.
    #[serde(default = "default_hygiene_days")]
    pub hygiene_refresh_days: u32,
}

fn default_git_sync_minutes() -> u32 {
    60
}

fn default_hygiene_days() -> u32 {
    30
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

fn default_clip_port() -> u16 {
    41500
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
                effort: String::new(),
            });
            if !self.openai_base_url.trim().is_empty() || !self.openai_api_key.is_empty() {
                self.providers.push(ProviderEntry {
                    id: "gateway".into(),
                    kind: "gateway".into(),
                    label: "Gateway".into(),
                    base_url: self.openai_base_url.clone(),
                    api_key: self.openai_api_key.clone(),
                    chat_model: self.openai_chat_model.clone(),
                    effort: String::new(),
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
            notion_token: String::new(),
            provider: default_provider(),
            embedder: default_provider(),
            base_url: "http://localhost:11434".to_string(),
            // 20b, not 120b: the big one is a 65GB download needing ~64GB of
            // RAM, so the shipped default was unrunnable on the 32GB Macs most
            // people have. Same family, so the citation style `verify.rs`
            // parses and the prompt tuning around it still hold. Fresh configs
            // only — an existing config keeps whatever it already had.
            chat_model: "gpt-oss:20b".to_string(),
            small_model: String::new(),
            // A/B'd on BEIR (2026-08-09): mxbai beat nomic-embed-text on
            // every dataset pair tried (scifact 0.743 vs 0.727, fiqa 0.390
            // vs 0.347 fused nDCG@10). Fresh configs only — persisted
            // configs keep the model their stored vectors were built with.
            embed_model: "mxbai-embed-large".to_string(),
            // OCR is opt-in: pick a vision model in Settings to enable it.
            vision_model: String::new(),
            openai_base_url: String::new(),
            openai_api_key: String::new(),
            openai_chat_model: String::new(),
            openai_vision_model: String::new(),
            profile: UserProfile::default(),
            mcp_enabled: default_true(),
            mcp_port: default_mcp_port(),
            hosted_agent: String::new(),
            clip_enabled: default_true(),
            clip_port: default_clip_port(),
            tray_enabled: default_true(),
            background_enabled: default_true(),
            show_notifications: default_true(),
            quiet_when_focused: default_true(),
            curator_consolidate: default_true(),
            source_gists: default_true(),
            vision_provider: String::new(),
            setup_seen: false,
            git_sync_minutes: default_git_sync_minutes(),
            self_diagnose: default_true(),
            source_hygiene: default_true(),
            hygiene_refresh_days: default_hygiene_days(),
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
#[derive(Clone)]
pub struct Ai {
    config: AiConfig,
    router: Router,
    /// Ollama retained directly for the capabilities that haven't joined the
    /// router yet (OCR fallback, model listing).
    ollama: Ollama,
    /// Gateway client retained for vision + model listing when configured.
    openai: Option<OpenAiClient>,
    /// Tier-matched local cross-encoder reranker; None when fusion order
    /// is already the best available (strong embedder tiers).
    xenc: Option<crate::inference::rerank::CrossEncoder>,
    /// Answer verifier (RFC-judged-evals §5) — the same Small model on
    /// EVERY tier: reranking is a tier decision (a 38M model loses to
    /// mxbai's fusion order) but claim-vs-cited-excerpt verification is
    /// engine-independent. Lazy: nothing loads until the first check.
    verifier: crate::inference::rerank::CrossEncoder,
    /// Resolved app-data dir (same one the embedder writes under). The gist
    /// sweep's enrichment marker lives here (RFC-infinite-context §2), so the
    /// distillation family can find it without threading a path through every
    /// `spawn_sweep` call site.
    data_dir: std::path::PathBuf,
    /// The FM sidecar path the constructor resolved, kept so a per-call
    /// provider override can rebuild any entry's engine after construction.
    fm_sidecar: Option<std::path::PathBuf>,
}

pub(crate) fn ollama_config(config: &AiConfig) -> OllamaConfig {
    OllamaConfig {
        base_url: config.base_url.clone(),
        chat_model: config.chat_model.clone(),
        embed_model: config.embed_model.clone(),
        vision_model: config.vision_model.clone(),
        // Effort is a per-provider choice; the shared slice carries none.
        effort: String::new(),
    }
}

/// The chat engine one configured provider entry resolves to. Shared by the
/// constructor (roles) and by per-call provider overrides (the MCP generate
/// tool), so the two can never disagree about what an entry means.
fn engine_for_entry(
    config: &AiConfig,
    fm_sidecar: Option<&std::path::Path>,
    entry: &ProviderEntry,
) -> ChatEngine {
    match entry.kind.as_str() {
        "fm" => match fm_sidecar.filter(|p| p.exists()) {
            Some(p) => ChatEngine::FoundationModels(FmEngine::new(p.to_path_buf())),
            // Sidecar missing (pre-26 macOS, unbundled build): fall
            // back to Ollama so a stale selection can't dead-end.
            None => ChatEngine::Ollama(Ollama::new(ollama_config(config))),
        },
        "gateway" => ChatEngine::Gateway(OpenAiClient::with_effort(
            entry.base_url.trim(),
            &entry.api_key,
            &entry.chat_model,
            &entry.effort,
        )),
        kind => match AgentKind::from_id(kind) {
            // Family B: the vendor CLI carries the subscription. The
            // model and effort are still ours to pass — blank means
            // "the CLI's own default", which is exactly what goes
            // stale when a vendor retires a model out from under it.
            Some(agent) => ChatEngine::Agent(AgentCli::configured(
                agent,
                &entry.chat_model,
                &entry.effort,
            )),
            None => {
                let mut oc = ollama_config(config);
                if !entry.base_url.trim().is_empty() {
                    oc.base_url = entry.base_url.clone();
                }
                if !entry.chat_model.trim().is_empty() {
                    oc.chat_model = entry.chat_model.clone();
                }
                oc.effort = entry.effort.clone();
                ChatEngine::Ollama(Ollama::new(oc))
            }
        },
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
            engine_for_entry(&config, fm_path.as_deref(), entry)
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
                data_dir.clone(),
                runtime.embedder_progress.clone(),
            ))
        } else {
            Embedder::Ollama(Ollama::new(ollama_config(&config)))
        };
        // Small-role rung. An explicitly configured Ollama model wins: it is
        // a deliberate choice, where the FM sidecar is a host capability we
        // merely detected. Otherwise the sidecar when the host found the
        // binary — availability (macOS version, Apple Intelligence state) is
        // probed lazily on first use, and unavailable probes make chat_role
        // fall through, so constructing the engine here is always safe.
        let small = if !config.small_model.trim().is_empty() {
            let mut oc = ollama_config(&config);
            oc.chat_model = config.small_model.trim().to_string();
            Some(ChatEngine::Ollama(Ollama::new(oc)))
        } else {
            runtime
                .fm_sidecar
                .as_ref()
                .filter(|p| p.exists())
                .map(|p| ChatEngine::FoundationModels(FmEngine::new(p.clone())))
        };
        let router = Router::new(chat, embedder, small, generate);
        let ollama = Ollama::new(ollama_config(&config));
        // Tier-matched local reranker (see Router::xenc_model). Lazy: the
        // model downloads/loads on first rerank, not at config time.
        let xenc = router
            .xenc_model()
            .map(|m| crate::inference::rerank::CrossEncoder::new(data_dir.clone(), m));
        let verifier = crate::inference::rerank::CrossEncoder::new(
            data_dir.clone(),
            crate::inference::rerank::XencModel::Small,
        );
        Self {
            config,
            router,
            ollama,
            openai,
            data_dir,
            fm_sidecar: fm_path,
            xenc,
            verifier,
        }
    }

    /// The answer verifier (see the `verifier` field).
    pub fn verifier(&self) -> &crate::inference::rerank::CrossEncoder {
        &self.verifier
    }

    /// The app-data dir this instance writes under — the gist sweep's
    /// enrichment marker lives here (RFC-infinite-context §2).
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// Retrieval/context parameters for a role, resolved by the router
    /// against the active model tier (RFC-inference-providers §2).
    pub fn profile(&self, role: Role) -> crate::inference::ContextProfile {
        self.router.profile(role)
    }

    /// Corpus budget for one generation prompt, sized to the engine that
    /// will read it (see Router::corpus_chars).
    pub fn corpus_chars(&self, role: Role) -> usize {
        self.router.corpus_chars(role)
    }

    /// Stable id of the engine a role resolves to ("ollama", "gateway",
    /// "foundation-models", or an agent kind) — evals verify they measured
    /// the engine they meant to, since unavailable tiers fall through.
    pub fn chat_engine_id(&self, role: Role) -> &'static str {
        self.router.chat_engine(role).id()
    }

    /// The engine currently answering `role` — per-call provider overrides
    /// (the chat fallback buttons, the MCP generate tool) resolve through
    /// `engine_for_provider` instead.
    pub fn engine(&self, role: Role) -> &ChatEngine {
        self.router.chat_engine(role)
    }

    /// The engine allowed to diagnose a failure of `failing_id`
    /// (RFC-self-resolve phase 2): the diagnosing model must never be the
    /// failing engine. The Small engine wins when it isn't the failing
    /// stack; when the local stack IS the failure, the Apple FM sidecar
    /// steps in if it's alive; when neither qualifies, None — the caller
    /// skips the loop and the phase-1 cleaned error stands alone.
    pub async fn diagnosis_engine(&self, failing_id: &str) -> Option<ChatEngine> {
        if self.router.has_small() {
            let small = self.router.chat_engine(Role::Small);
            let usable = match small {
                ChatEngine::FoundationModels(fm) => {
                    failing_id != "foundation-models" && fm.available().await
                }
                other => other.id() != failing_id,
            };
            if usable {
                return Some(small.clone());
            }
        }
        if failing_id != "foundation-models" {
            if let Some(path) = self.fm_sidecar.as_ref().filter(|p| p.exists()) {
                let fm = FmEngine::new(path.clone());
                if fm.available().await {
                    return Some(ChatEngine::FoundationModels(fm));
                }
            }
        }
        None
    }

    /// The embedder tier's RRF (vector weight, k) — see
    /// Router::fusion_params.
    pub fn fusion_params(&self) -> (f32, f32) {
        self.router.fusion_params()
    }

    /// Whether this tier reranks with a local cross-encoder — callers use
    /// this to retrieve a wider pool worth reranking.
    pub fn has_xenc(&self) -> bool {
        self.xenc.is_some()
    }

    /// Rerank citations with the tier's cross-encoder and truncate to `k`.
    /// No reranker, a small pool, or ANY failure returns fusion order
    /// truncated to `k` — reranking can reorder-or-equal, never lose hits.
    pub async fn rerank_hits(
        &self,
        query: &str,
        mut hits: Vec<crate::models::Citation>,
        k: usize,
    ) -> Vec<crate::models::Citation> {
        if let Some(xe) = self.xenc.as_ref().filter(|_| hits.len() > k) {
            let snippets: Vec<String> = hits.iter().map(|c| c.snippet.clone()).collect();
            match xe.rank(query, &snippets).await {
                Ok(order) => {
                    hits = order
                        .into_iter()
                        .filter_map(|i| hits.get(i).cloned())
                        .collect()
                }
                Err(err) => crate::note!("xenc rerank skipped: {err:#}"),
            }
        }
        hits.truncate(k);
        hits
    }

    /// Input-token budget for the engine that will answer `role`, but only when
    /// that engine is the on-device Foundation Models sidecar — its context
    /// window is a hard ceiling (`inference::budget`), and one whose real size
    /// varies by machine, so this reads the live value rather than the assumed
    /// default. `None` for every other engine: Ollama, gateways, and agent CLIs
    /// carry far larger windows and must never have their prompts shrunk to fit
    /// the on-device ceiling. Call sites that assemble large prompts use this to
    /// structure-aware-trim before dispatch; the sidecar itself re-checks as an
    /// unconditional backstop.
    pub fn fm_input_budget(&self, role: Role) -> Option<usize> {
        matches!(
            self.router.chat_engine(role),
            ChatEngine::FoundationModels(_)
        )
        .then(crate::inference::budget::fm_input_budget_tokens)
    }

    pub fn config(&self) -> &AiConfig {
        &self.config
    }

    /// The model name answering chats right now (stats keying, health display).
    pub fn active_chat_model(&self) -> String {
        match self
            .config
            .provider_by_id(&self.config.chat_provider)
            .map(|p| (p.kind.clone(), p.chat_model.clone(), p.label.clone()))
        {
            Some((kind, model, label)) => match kind.as_str() {
                "gateway" | "ollama" if !model.trim().is_empty() => model,
                "gateway" => self.config.openai_chat_model.clone(),
                "ollama" => self.config.chat_model.clone(),
                // Agent CLIs and the on-device model have no user-facing
                // model name; the provider label ("Claude Code", "On this
                // Mac") is the honest caption, not the kind id ("fm").
                _ if !label.trim().is_empty() => label,
                other => other.to_string(),
            },
            None => self.config.chat_model.clone(),
        }
    }

    /// Resolve a per-call provider override: the engine for the configured
    /// entry with this id, plus the model name to key stats under. `Err`
    /// names the valid ids — an agent typo should read as "pick one of
    /// these", not "provider broken". Host settings still own every default;
    /// this narrows one call to one already-configured entry.
    pub fn engine_for_provider(&self, id: &str) -> Result<(ChatEngine, String)> {
        let entry = self
            .config
            .providers
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no provider with id \"{id}\" — configured providers: {}",
                    self.config
                        .providers
                        .iter()
                        .map(|p| format!("\"{}\" ({})", p.id, p.label))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let engine = engine_for_entry(&self.config, self.fm_sidecar.as_deref(), entry);
        let model = if entry.chat_model.trim().is_empty() {
            entry.label.clone()
        } else {
            entry.chat_model.clone()
        };
        Ok((engine, model))
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
            // FM is capability-gated (it may not be usable on this host);
            // an explicitly configured model is simply tried.
            let usable = match engine {
                ChatEngine::FoundationModels(fm) => fm.available().await,
                _ => true,
            };
            if usable {
                match engine.chat(messages).await {
                    Ok(out) => return Ok(out),
                    Err(err) => {
                        crate::note!("small-role engine failed, falling through: {err:#}");
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
