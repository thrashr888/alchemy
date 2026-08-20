//! The `settings` tool (docs/RFC-self-resolve.md phase 3): get/set over the
//! safe `AiConfig` allowlist, same core as the chat tool router's version —
//! secrets are refused on write and redacted out of every read.

use super::*;
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SettingsReq {
    /// "get" (redacted snapshot) or "set" (change one field).
    op: String,
    /// For set: chatProvider, studioProvider, chatModel, effort, baseUrl
    /// (bare fields target the active chat provider), smallModel, embedder,
    /// or provider.<id>.chatModel / .effort / .baseUrl.
    #[serde(default)]
    field: String,
    /// For set: the new value. Provider fields accept an id or a label.
    #[serde(default)]
    value: String,
}

#[tool_router(router = settings_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "Read or change Alchemy's AI settings. op:\"get\" returns a redacted snapshot (API keys are never readable). op:\"set\" changes ONE field: chatProvider, studioProvider, chatModel, effort, baseUrl (these three target the active chat provider), smallModel, embedder (ollama|builtin), or provider.<id>.chatModel / .effort / .baseUrl. Provider values accept an id or a label from the get snapshot. API keys and tokens can never be read or set through this tool. Applied changes take effect immediately and open windows refresh live."
    )]
    async fn settings(
        &self,
        Parameters(SettingsReq { op, field, value }): Parameters<SettingsReq>,
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
            other => Err(invalid(format!(
                "unknown op \"{other}\" — use \"get\" or \"set\""
            ))),
        }
    }
}
