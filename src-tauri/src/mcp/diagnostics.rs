//! The `recent_errors` tool (docs/RFC-diagnostics.md): the app's own error
//! log, readable by the agent that is being asked to fix it. Read-only — an
//! agent can see what broke, never edit the record of it.

use super::*;
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct RecentErrorsReq {
    /// How many records to return, newest first. Default 30, capped at 200.
    #[serde(default)]
    limit: Option<usize>,
    /// Lowest level to include: info, warn, error, fatal. Default "error".
    #[serde(default)]
    min_level: Option<String>,
}

#[tool_router(router = diagnostics_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "Read Alchemy's own error log — what has crashed, failed, or panicked \
                       recently, newest first. Each record has a level (info|warn|error|fatal), \
                       an origin (rust = backend, js = front-end), a kind (panic, ipc, render, \
                       unhandled-rejection, startup), the message, and often a backtrace in \
                       `detail` plus structured `context`. Use it when the user reports that \
                       something broke, when a command failed for reasons the error string \
                       doesn't explain, or before concluding a bug can't be reproduced — the \
                       failure is usually already recorded here. The same records are in \
                       ~/Library/Logs/com.thrashr888.alchemy/alchemy.log and in Console.app \
                       under the com.thrashr888.alchemy subsystem. Read-only."
    )]
    async fn recent_errors(
        &self,
        Parameters(RecentErrorsReq { limit, min_level }): Parameters<RecentErrorsReq>,
    ) -> Result<CallToolResult, McpError> {
        let level = min_level.as_deref().unwrap_or("error");
        let min = match level {
            "info" => None,
            "warn" => Some(crate::diagnostics::Level::Warn),
            "error" => Some(crate::diagnostics::Level::Error),
            "fatal" => Some(crate::diagnostics::Level::Fatal),
            other => {
                return Err(invalid(format!(
                    "unknown min_level \"{other}\" — use info, warn, error, or fatal"
                )))
            }
        };
        json_result(&serde_json::json!({
            "summary": crate::diagnostics::summary(),
            "records": crate::diagnostics::recent(limit.unwrap_or(30), min),
        }))
    }
}
