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
    /// How much overnight work to do: "light" | "standard" | "generous".
    /// One notch rather than a slider, because a token count is not a unit
    /// anyone has intuitions about. Cost control, not an opt-in gate - the
    /// queue runs at Standard unless told otherwise.
    #[serde(default = "default_budget")]
    pub background_budget: String,
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
    /// What the user calls the assistant. People name their agents and talk
    /// to them like a friend; a named assistant answers as itself.
    pub assistant_name: String,
}

fn default_provider() -> String {
    "ollama".to_string()
}

/// Standard is a night's work on a normal corpus without the fans coming on.
fn default_budget() -> String {
    "standard".to_string()
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

/// Placeholder standing in for any stored secret in config snapshots that
/// leave the process (`get_ai_config`). Renders as a masked value if it ever
/// reaches a field, and no real key can collide with it. `absorb_secrets`
/// swaps the stored values back in on save.
pub const REDACTED_KEY: &str = "••••••••";

impl AiConfig {
    /// Is chat routed through the OpenAI-compatible gateway (large-context
    /// remote models) rather than local Ollama? Context-size budgets key off
    /// this in one place instead of scattering provider string comparisons.
    pub fn is_gateway(&self) -> bool {
        self.provider == "openai"
    }

    /// Copy with every secret replaced by [`REDACTED_KEY`] — the shape the
    /// webview gets. Empty fields stay empty so "is a key set" remains
    /// readable.
    pub fn redacted(&self) -> AiConfig {
        let mut c = self.clone();
        for p in &mut c.providers {
            if !p.api_key.is_empty() {
                p.api_key = REDACTED_KEY.into();
            }
        }
        if !c.openai_api_key.is_empty() {
            c.openai_api_key = REDACTED_KEY.into();
        }
        if !c.notion_token.is_empty() {
            c.notion_token = REDACTED_KEY.into();
        }
        c
    }

    /// Replace redaction placeholders with the stored secrets they stand for,
    /// so a config round-tripped through the webview can't wipe keys. A
    /// placeholder with no stored counterpart (provider deleted or re-added
    /// under a new id) becomes empty rather than persisting the placeholder
    /// as if it were a key; a field the user explicitly emptied stays empty,
    /// so clearing a key still works.
    pub fn absorb_secrets(&mut self, stored: &AiConfig) {
        for p in &mut self.providers {
            if p.api_key == REDACTED_KEY {
                p.api_key = stored
                    .provider_by_id(&p.id)
                    .map(|s| s.api_key.clone())
                    .unwrap_or_default();
            }
        }
        if self.openai_api_key == REDACTED_KEY {
            self.openai_api_key = stored.openai_api_key.clone();
        }
        if self.notion_token == REDACTED_KEY {
            self.notion_token = stored.notion_token.clone();
        }
    }

    /// The stored key for an OpenAI-compatible gateway at `base_url`, used to
    /// resolve a redacted placeholder arriving from the webview on probe
    /// commands (`list_gateway_models`), where only the URL identifies the
    /// provider.
    pub fn stored_key_for_url(&self, base_url: &str) -> Option<String> {
        let url = base_url.trim();
        self.providers
            .iter()
            .find(|p| p.base_url.trim() == url && !p.api_key.is_empty())
            .map(|p| p.api_key.clone())
            .or_else(|| {
                (self.openai_base_url.trim() == url && !self.openai_api_key.is_empty())
                    .then(|| self.openai_api_key.clone())
            })
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
            // The un-measurable fallback. A truly fresh config goes through
            // `AiConfig::fresh`, which sizes the chat model to this Mac's
            // unified memory; `Default` is what serde and parse-failure paths
            // land on, so it stays the safe middle-tier pick.
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
            background_budget: default_budget(),
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

impl AiConfig {
    /// A first-run config: the stock defaults with the chat model sized to
    /// this machine's unified memory. Only the missing-config path calls
    /// this — an existing config keeps whatever it already had.
    pub fn fresh() -> Self {
        let mut config = Self::default();
        if let Some(gib) = machine_ram_gib() {
            config.chat_model = recommended_chat_model(gib).to_string();
        }
        config
    }
}

/// Default chat model for a machine with this much unified memory, in GiB
/// (surveyed 2026-08 from the Ollama library; q4-ish weights unless noted).
/// The constraint is macOS's Metal wired limit (~65-75% of RAM): weights
/// plus KV cache plus the embedder must fit under it, so each tier picks
/// the best model whose weights stay near half of RAM.
///
/// Picks are gated on the judged evals (judged_eval.rs, 2026-08-29 run —
/// evidence cited / two-source / faithfulness / abstention) and the
/// grounded-chat contract in `evals::eval_chat_grounding_across_models`:
///
/// - <16GB: lfm2.5:8b — 5.2GB MoE (1B active), 77 tok/s. 52%/44%,
///   0.85/0.98, abstains on all unanswerables. Watch: sometimes emits
///   fullwidth 【n】 markers, which verify.rs cannot parse.
/// - 16-31GB: gemma4:12b — 7.6GB. 64%/72%, 0.80/0.95, abstains on all
///   unanswerables — the honest small default. Nothing q4 between 8 and
///   16GB beats it, so the 24GB tier gets the same pick. (Plain GGUF tag:
///   the -mlx build has no vision projector and hallucinates on images.)
///   gpt-oss:20b, the old default here, left the table with its family:
///   gpt-oss:120b measured 24% evidence cited and answered 4 grounded
///   questions with zero [n] markers.
/// - 32GB: qwen3.8:27b — 18GB, 256K context. 76%/88% with the densest
///   citations measured; the verify pass covers its 0.69 faithfulness.
/// - 96GB: gemma4:31b — 20GB. 68%/80% with the best faithfulness of the
///   large models (0.91/0.98).
/// - 192GB+: qwen3.8-flash-next:125b-mlx — 105GB MoE (6B active), fast at
///   this size with a small KV cache. Unmeasured: nothing else
///   current-generation exists locally at this scale.
///
/// Ornith-1.5:9b cites best in class (76%/84%) but abstained on only 10%
/// of unanswerable questions — a fabrication risk no default should carry.
pub fn recommended_chat_model(ram_gib: u64) -> &'static str {
    match ram_gib {
        0..=15 => "lfm2.5:8b",
        16..=31 => "gemma4:12b",
        32..=95 => "qwen3.8:27b",
        96..=191 => "gemma4:31b",
        _ => "qwen3.8-flash-next:125b-mlx",
    }
}

/// Physical memory in GiB, or None where it can't be read. macOS reports
/// `hw.memsize` in bytes; other platforms just keep the stock default.
fn machine_ram_gib() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        let bytes: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
        Some(bytes >> 30)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
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
        keep_alive: None,
        num_predict: None,
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
            // Hold the small model resident well past Ollama's 5m default:
            // its cold load is the measured tail of deep research and gap
            // retrieval (595s once, under load; 21s typical), and an 8B
            // model kept warm after real use is cheap — unlike predictive
            // preloading, which this deliberately is not.
            oc.keep_alive = Some("30m".into());
            // And cap its output: every small-role call is short by
            // contract (a JSON action, a query line, a distillate whose
            // consumer truncates at 4,000 chars). One runaway response was
            // traced holding Ollama's single slot for 12k+ tokens (~600s)
            // with the whole app queued behind it; 2,048 tokens is roomy
            // for the longest legitimate reply and bounds that tail at
            // ~20s on an 8B model.
            oc.num_predict = Some(2_048);
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

    /// The identity to record speed metrics under, for the configured chat
    /// provider or a named one.
    ///
    /// `active_chat_model` is the caption ("Codex", "Hermes") — right for a
    /// transcript row, but too coarse for a ranking: reasoning effort and the
    /// model an agent CLI routes to move time-to-first-token more than the
    /// choice of CLI does, so pooling `Codex minimal` with `Codex max` under
    /// one average answers nothing. Anything that changes the wait belongs in
    /// the key.
    pub fn chat_metrics_key(&self, provider_id: Option<&str>) -> String {
        let id = provider_id.unwrap_or(&self.config.chat_provider);
        let base = match provider_id {
            // An override names its own provider; resolve that entry's
            // caption the same way the active one is resolved.
            Some(_) => self
                .config
                .provider_by_id(id)
                .map(|p| {
                    let model = p.chat_model.trim();
                    match p.kind.as_str() {
                        "gateway" | "ollama" if !model.is_empty() => model.to_string(),
                        _ if !p.label.trim().is_empty() => p.label.clone(),
                        other => other.to_string(),
                    }
                })
                .unwrap_or_else(|| self.active_chat_model()),
            None => self.active_chat_model(),
        };
        let Some(entry) = self.config.provider_by_id(id) else {
            return base;
        };
        let mut key = base;
        // Agent CLIs caption as their label, so the model they route to is
        // not in the key yet — and it is exactly what the user picked.
        let model = entry.chat_model.trim();
        if !model.is_empty() && !key.contains(model) {
            key = format!("{key} \u{b7} {model}");
        }
        let effort = entry.effort.trim();
        if !effort.is_empty() {
            key = format!("{key} \u{b7} {effort}");
        }
        key
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
    /// Is there a distinct Small-role engine, or would `chat_role(Small)`
    /// fall through to the chat engine? Evals comparing the two roles need
    /// to know the difference is real before reporting a comparison.
    #[cfg(test)]
    pub fn has_small_role(&self) -> bool {
        self.router.has_small()
    }

    /// The Ollama model `chat_role(role)` would run that is NOT currently
    /// resident in the server — so a caller about to pay the cold load can
    /// say "Starting {model}…" instead of letting it read as a hang. None
    /// when the resolved engine isn't Ollama, the model is already loaded,
    /// or the probe fails (status must never block a run).
    pub async fn cold_ollama_model(&self, role: Role) -> Option<String> {
        // Mirror chat_role's resolution: Small falls through to the chat
        // engine when no small engine is configured.
        let engine = match role {
            Role::Small if !self.router.has_small() => self.router.chat_engine(Role::Chat),
            r => self.router.chat_engine(r),
        };
        let ChatEngine::Ollama(o) = engine else {
            return None;
        };
        let model = o.chat_model_name().to_string();
        let loaded = self.ollama.ps().await.ok()?;
        // `ps` reports fully tagged names; configs may omit `:latest`.
        let norm = |s: &str| s.trim_end_matches(":latest").to_string();
        if loaded.iter().any(|m| norm(m) == norm(&model)) {
            None
        } else {
            Some(model)
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_secrets() -> AiConfig {
        AiConfig {
            providers: vec![ProviderEntry {
                id: "gw1".into(),
                kind: "gateway".into(),
                label: "OpenAI".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key: "sk-real-key".into(),
                chat_model: "gpt-5".into(),
                effort: String::new(),
            }],
            openai_api_key: "sk-legacy-key".into(),
            openai_base_url: "https://legacy.example/v1".into(),
            notion_token: "ntn_secret".into(),
            ..AiConfig::default()
        }
    }

    #[test]
    fn chat_model_tiers_follow_unified_memory() {
        assert_eq!(recommended_chat_model(8), "lfm2.5:8b");
        assert_eq!(recommended_chat_model(16), "gemma4:12b");
        assert_eq!(recommended_chat_model(24), "gemma4:12b");
        assert_eq!(recommended_chat_model(32), "qwen3.8:27b");
        assert_eq!(recommended_chat_model(64), "qwen3.8:27b");
        assert_eq!(recommended_chat_model(128), "gemma4:31b");
        assert_eq!(recommended_chat_model(192), "qwen3.8-flash-next:125b-mlx");
        assert_eq!(recommended_chat_model(512), "qwen3.8-flash-next:125b-mlx");
    }

    #[test]
    fn fresh_config_only_differs_from_default_in_chat_model() {
        let fresh = AiConfig::fresh();
        let stock = AiConfig {
            chat_model: fresh.chat_model.clone(),
            ..AiConfig::default()
        };
        assert_eq!(
            serde_json::to_string(&fresh).unwrap(),
            serde_json::to_string(&stock).unwrap()
        );
    }

    #[test]
    fn redacted_strips_every_secret_and_keeps_empty_fields_empty() {
        let mut c = config_with_secrets();
        c.providers.push(ProviderEntry {
            id: "ollama".into(),
            kind: "ollama".into(),
            ..Default::default()
        });
        let r = c.redacted();
        assert_eq!(r.providers[0].api_key, REDACTED_KEY);
        assert_eq!(r.openai_api_key, REDACTED_KEY);
        assert_eq!(r.notion_token, REDACTED_KEY);
        // A keyless provider stays visibly keyless.
        assert_eq!(r.providers[1].api_key, "");
        // Nothing key-shaped survives serialization to the webview.
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("sk-real-key"));
        assert!(!json.contains("sk-legacy-key"));
        assert!(!json.contains("ntn_secret"));
    }

    #[test]
    fn absorb_restores_secrets_on_round_trip() {
        let stored = config_with_secrets();
        let mut round_trip = stored.redacted();
        round_trip.absorb_secrets(&stored);
        assert_eq!(round_trip.providers[0].api_key, "sk-real-key");
        assert_eq!(round_trip.openai_api_key, "sk-legacy-key");
        assert_eq!(round_trip.notion_token, "ntn_secret");
    }

    #[test]
    fn absorb_keeps_explicit_clears_and_new_values() {
        let stored = config_with_secrets();
        let mut edited = stored.redacted();
        edited.providers[0].api_key = "sk-new-key".into();
        edited.notion_token = String::new();
        edited.absorb_secrets(&stored);
        assert_eq!(edited.providers[0].api_key, "sk-new-key");
        assert_eq!(edited.notion_token, "");
    }

    #[test]
    fn absorb_never_persists_the_placeholder_itself() {
        let stored = config_with_secrets();
        let mut edited = stored.redacted();
        // Provider re-added under a fresh id, placeholder carried along.
        edited.providers[0].id = "gw2".into();
        edited.absorb_secrets(&stored);
        assert_eq!(edited.providers[0].api_key, "");
    }

    #[test]
    fn stored_key_resolves_by_url_for_probe_commands() {
        let c = config_with_secrets();
        assert_eq!(
            c.stored_key_for_url(" https://api.openai.com/v1 ".trim()),
            Some("sk-real-key".into())
        );
        assert_eq!(
            c.stored_key_for_url("https://legacy.example/v1"),
            Some("sk-legacy-key".into())
        );
        assert_eq!(c.stored_key_for_url("https://elsewhere.example/v1"), None);
    }
}
