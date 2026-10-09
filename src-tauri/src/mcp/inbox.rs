//! The Inbox tools: captures that arrived without a home, and the verbs that
//! file or dismiss them. Filing is the same add path a notebook's own Add
//! source uses; the row's suggestion (with the typed judge's confidence) rides
//! along so an agent can show it, or accept only what the judge was sure of.

use super::*;
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct InboxAcceptReq {
    /// The capture's id, from inbox_list.
    id: String,
    /// File into this notebook instead of the suggestion.
    #[serde(default)]
    notebook_id: Option<String>,
    /// Create a notebook with this title and file into it. Ignored when
    /// notebook_id is given. Needed to accept a suggestion that has no name.
    #[serde(default)]
    new_title: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct InboxIdReq {
    /// The capture's id, from inbox_list.
    id: String,
}

#[tool_router(router = inbox_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "List the Inbox: captures from the browser clipper, Services and \
                       alchemy:// links that are waiting to be filed, newest first. Nothing is \
                       imported until a capture is accepted. Each has the url, text or files, \
                       a short excerpt, and the suggestion: suggestedNotebookId and \
                       suggestedTitle (isNew=true means a new notebook with that name), \
                       probability (the judge's confidence; null when no judge answered), \
                       auto (the judge was confident and trusted, so it is safe to file \
                       without asking), alternatives (other notebooks, best first), and \
                       suggested=false while the suggestion is still being worked out."
    )]
    async fn inbox_list(&self) -> Result<CallToolResult, McpError> {
        let items = self.state().db.list_inbox().await.map_err(internal)?;
        json_result(&items)
    }

    #[tool(
        description = "File an Inbox capture: into the suggested notebook, into notebook_id, or \
                       into a new notebook named new_title. Imports it exactly as adding a \
                       source would, then removes it from the Inbox. Returns {notebookId, \
                       title}. File only suggestions the user would expect; a suggestion with \
                       auto=false is a proposal to show them first."
    )]
    async fn inbox_accept(
        &self,
        Parameters(InboxAcceptReq {
            id,
            notebook_id,
            new_title,
        }): Parameters<InboxAcceptReq>,
    ) -> Result<CallToolResult, McpError> {
        let landed =
            crate::inbox::accept(&self.app, &id, notebook_id.as_deref(), new_title.as_deref())
                .await
                .map_err(internal)?;
        json_result(&landed)
    }

    #[tool(
        description = "Dismiss an Inbox capture. It was never imported, so nothing else changes."
    )]
    async fn inbox_dismiss(
        &self,
        Parameters(InboxIdReq { id }): Parameters<InboxIdReq>,
    ) -> Result<CallToolResult, McpError> {
        crate::inbox::dismiss(&self.app, &id)
            .await
            .map_err(internal)?;
        json_result(&serde_json::json!({ "dismissed": id }))
    }
}
