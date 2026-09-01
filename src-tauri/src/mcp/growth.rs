//! Grow tools — what a notebook is hungry for, and where it could be fed
//! (docs/RFC-living-notebook.md Pillar 2). The same tiers the Grow pane
//! shows: standing queries from thin retrievals, outbound links the
//! notebook's own sources keep pointing at, Spotlight matches on this Mac,
//! and — only when the notebook has it switched on — the metered open-web
//! search. Nothing here fetches; hand a proposal's url to add_source.

use super::*;
use crate::growth::{self, GrowthProposal, GrowthWebSearch};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct GrowReq {
    /// Notebook id (from list_notebooks).
    notebook_id: String,
    /// Also run the open-web tier (a Firecrawl search per standing query,
    /// metered against the monthly cap). Honored only when the notebook has
    /// web growth switched on in its Grow pane; otherwise `web` is omitted
    /// and `webEnabled` is false.
    #[serde(default)]
    web: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GrowResult {
    /// Recent questions the notebook answered thinly — what it is hungry for.
    queries: Vec<String>,
    /// Link tier: URLs the notebook's own sources point at but don't contain,
    /// ranked by mentions, spread across sources, and overlap with `queries`.
    proposals: Vec<GrowthProposal>,
    /// Local tier: files on this Mac that Spotlight matched to the queries
    /// (url is an on-disk path — pass it to add_source as file_path).
    local: Vec<GrowthProposal>,
    web_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    web: Option<GrowthWebSearch>,
}

#[tool_router(router = growth_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "What a notebook is hungry for and where to feed it — the Grow pane over MCP. Returns queries (recent questions the notebook answered thinly), proposals (URLs its own sources keep linking to but that aren't in the notebook, ranked), local (files on this Mac Spotlight matched to those questions; their url is a path), and webEnabled. Pass web:true to also run the open-web search tier when the notebook has it switched on — it costs metered credits, so only when the link and local tiers came up empty. Nothing is fetched here: add a proposal with add_source (url for web, file_path for local), and prefer proposals that name a matchedQuery."
    )]
    async fn grow(
        &self,
        Parameters(GrowReq { notebook_id, web }): Parameters<GrowReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let now = commands::now();
        let queries = growth::standing_queries(&state.trace_dir, &notebook_id, now);
        // The link frontier lives in the extracted text, which list_sources
        // strips; the other tiers only need metadata.
        let with_content = state
            .db
            .sources_with_content(&notebook_id)
            .await
            .map_err(internal)?;
        let proposals = growth::proposals(&with_content, &queries);
        let sources = state
            .db
            .list_sources(&notebook_id)
            .await
            .map_err(internal)?;
        let local = growth::local_proposals(&sources, &queries).await;
        let web_enabled = growth::web_enabled(&state.db, &notebook_id).await;
        let web = if web && web_enabled {
            let enabled = growth::web_enabled_count(&state.db).await;
            Some(
                growth::web_search(
                    &state.trace_dir,
                    &notebook_id,
                    &sources,
                    &queries,
                    enabled,
                    now,
                )
                .await,
            )
        } else {
            None
        };
        json_result(&GrowResult {
            queries,
            proposals,
            local,
            web_enabled,
            web,
        })
    }
}
