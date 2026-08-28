//! Home-chat tools: read access to the corpus-wide conversations the user
//! has been having on Home (docs/RFC-meta-chat.md). Read-only on purpose —
//! agents already ask across everything with `ask_everything`; what they
//! can't otherwise see is what the *user* asked and what came back, which is
//! often the shortest route to "what have we already established?".

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

use super::*;

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ThreadIdReq {
    /// Thread id (from list_home_chats).
    thread_id: String,
}

#[tool_router(router = homechat_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "List the user's Home conversations — the questions they asked across every notebook at once — most recently used first. Each thread: id, title (its opening question), turnCount, and timestamps. Read one with get_home_chat."
    )]
    async fn list_home_chats(&self) -> Result<CallToolResult, McpError> {
        let threads = self
            .state()
            .db
            .list_meta_threads()
            .await
            .map_err(internal)?;
        json_result(&threads)
    }

    #[tool(
        description = "Read one Home conversation in full: every turn in order, with the citations behind each answer (each naming the notebook and source it came from). Use it to pick up a line of enquiry the user already started rather than re-deriving it."
    )]
    async fn get_home_chat(
        &self,
        Parameters(ThreadIdReq { thread_id }): Parameters<ThreadIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let turns = self
            .state()
            .db
            .list_meta_turns(&thread_id)
            .await
            .map_err(internal)?;
        if turns.is_empty() {
            return Err(invalid("no Home conversation with that id"));
        }
        json_result(&turns)
    }
}
