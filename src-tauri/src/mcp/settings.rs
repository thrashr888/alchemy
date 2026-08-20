//! The `settings` tool (docs/RFC-self-resolve.md phase 3, grown by
//! docs/RFC-conversational-setup.md phase 1): get/set over the safe
//! `AiConfig` allowlist plus the model verbs `models`, `test`, and `pull` —
//! same core as the chat tool router's version. Secrets are refused on
//! write and redacted out of every read; `pull` only returns the validated
//! command, it never executes anything.

use super::*;
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SettingsReq {
    /// "get" (redacted snapshot), "set" (change one field), "models"
    /// (installed Ollama models + provider readiness), "test" (live-probe
    /// one provider/model), or "pull" (stage an `ollama pull` command).
    op: String,
    /// For set: chatProvider, studioProvider, chatModel, effort, baseUrl
    /// (bare fields target the active chat provider), smallModel, embedder,
    /// or provider.<id>.chatModel / .effort / .baseUrl.
    #[serde(default)]
    field: String,
    /// For set: the new value. Provider fields accept an id or a label.
    #[serde(default)]
    value: String,
    /// For test: a provider id/label or an Ollama model name; empty probes
    /// the active chat provider.
    #[serde(default)]
    target: String,
    /// For pull: the Ollama model name to stage a download for.
    #[serde(default)]
    model: String,
}

#[tool_router(router = settings_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "Read, change, or probe Alchemy's AI settings. op:\"get\" returns a redacted snapshot (API keys are never readable). op:\"set\" changes ONE field: chatProvider, studioProvider, chatModel, effort, baseUrl (these three target the active chat provider), smallModel, embedder (ollama|builtin), or provider.<id>.chatModel / .effort / .baseUrl; provider values accept an id or a label; API keys and tokens can never be read or set. op:\"models\" lists installed Ollama models plus every provider's active model and live readiness. op:\"test\" live-probes one provider or model (pass `target`; empty = active chat provider) with exactly one tiny chat call and, for Ollama targets, one embed call, reporting alive/failed and latency — no config change. op:\"pull\" validates an Ollama model name (pass `model`) and returns the `ollama pull` command string for the USER to run in Terminal — Alchemy never executes it. Applied `set` changes take effect immediately and open windows refresh live."
    )]
    async fn settings(
        &self,
        Parameters(SettingsReq {
            op,
            field,
            value,
            target,
            model,
        }): Parameters<SettingsReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let mut config = { state.ai.read().await.config().clone() };
        match op.trim() {
            "get" => Ok(CallToolResult::success(vec![ContentBlock::text(
                crate::selfheal::settings_get(&config),
            )])),
            "set" => {
                let echo =
                    crate::selfheal::settings_set(&mut config, &field, &value).map_err(invalid)?;
                commands::apply_ai_config(&self.app, &state, config)
                    .await
                    .map_err(internal)?;
                self.changed("settings", None);
                json_result(&serde_json::json!({ "ok": true, "applied": echo }))
            }
            "models" => Ok(CallToolResult::success(vec![ContentBlock::text(
                commands::settings_models_report(&self.app, &state).await,
            )])),
            "test" => Ok(CallToolResult::success(vec![ContentBlock::text(
                commands::settings_test_report(&state, &target).await,
            )])),
            "pull" => {
                // Validation only — the command comes back as text for the
                // user to run; this process never shells out.
                let command = crate::selfheal::pull_command(&model).map_err(invalid)?;
                json_result(&serde_json::json!({
                    "ok": true,
                    "command": command,
                    "note": "Run this in Terminal yourself — Alchemy stages the \
                             command but never executes it. Then call op \"test\" \
                             with the model name.",
                }))
            }
            other => Err(invalid(format!(
                "unknown op \"{other}\" — use get, set, models, test, or pull"
            ))),
        }
    }
}
