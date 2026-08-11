//! Retrieval tools: hybrid, corpus-wide, grep, structural, debug.

use super::*;
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AskEverythingReq {
    /// The question to retrieve corpus-wide passages for.
    question: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct GrepReq {
    /// Notebook whose repo- and folder-backed files to search.
    notebook_id: String,
    /// Regular expression (Rust regex syntax); plain literal text works too.
    pattern: String,
    /// Max file windows to return (default 6, max 20).
    #[serde(default)]
    max_results: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AstReq {
    /// Notebook whose repo- and folder-backed files to search.
    notebook_id: String,
    /// ast-grep structural pattern, e.g. `fn $NAME($$$) { $$$ }` or
    /// `$OBJ.unwrap()`. `$NAME` matches one node, `$$$` any number.
    pattern: String,
    /// Max matches to return (default 10, max 30).
    #[serde(default)]
    max_results: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SearchReq {
    /// Notebook to search.
    notebook_id: String,
    /// Natural-language query; hybrid vector + keyword search.
    query: String,
    /// Max passages to return (default 6, max 20).
    #[serde(default)]
    max_results: Option<u32>,
}

#[tool_router(router = search_router, vis = "pub(super)")]
impl AlchemyMcp {
    // -- Search --

    #[tool(
        description = "Hybrid search (vector similarity + BM25 keyword, rank-fused) over a notebook's source chunks AND notes. Runs on the local embedder — cheap, call freely. Returns passages with sourceId/sourceTitle/snippet/distance; a passage with a non-empty noteId came from a note (a prior conclusion — yours or the user's), not a source document: weigh it as secondhand and use get_note for its full text. Use get_source for a source passage's full document. Synthesize answers yourself from the passages."
    )]
    async fn search(
        &self,
        Parameters(SearchReq {
            notebook_id,
            query,
            max_results,
        }): Parameters<SearchReq>,
    ) -> Result<CallToolResult, McpError> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Err(invalid("query is empty"));
        }
        let k = max_results.unwrap_or(6).clamp(1, 20) as usize;
        let state = self.state();
        let query_vec = {
            let ai = state.ai.read().await.clone();
            ai.embed_one(&query).await.map_err(internal)?
        };
        let citations = state
            .db
            .search_chunks(&notebook_id, query_vec, &query, k, None)
            .await
            .map_err(internal)?;
        crate::trace::log(
            &state.trace_dir,
            serde_json::json!({
                "ts": commands::now(),
                "surface": "mcp",
                "notebookId": notebook_id,
                "query": query,
                "citations": crate::trace::cite_summaries(&citations),
            }),
        );
        commands::bump_note_usage(&state.db, &citations, "retrieval_hits").await;
        json_result(&citations)
    }

    #[tool(
        description = "Exact-match search (ripgrep's engine, in-process) over the notebook's repo- and folder-backed files — the full working trees, not just embedded passages. Use for identifiers, error strings, or regex; use `search` for concepts and prose. Returns ranked file windows with sourceId/sourceTitle/path/line/window; get_source fetches a hit's stored text."
    )]
    async fn grep_sources(
        &self,
        Parameters(GrepReq {
            notebook_id,
            pattern,
            max_results,
        }): Parameters<GrepReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let files = commands::repo_backed_files(&state, &notebook_id, None).await;
        if files.is_empty() {
            return Err(invalid(
                "this notebook has no repo- or folder-backed files to grep",
            ));
        }
        let k = max_results.unwrap_or(6).clamp(1, 20) as usize;
        let paths: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
        let hits = tokio::task::spawn_blocking(move || {
            crate::grepsearch::search_pattern(&pattern, &paths, k)
        })
        .await
        .map_err(internal)?
        .map_err(invalid)?;
        let payload: Vec<serde_json::Value> = hits
            .into_iter()
            .map(|h| {
                let (path, id, title) = &files[h.file_index];
                serde_json::json!({
                    "sourceId": id,
                    "sourceTitle": title,
                    "path": path,
                    "line": h.first_line,
                    "window": h.window,
                })
            })
            .collect();
        json_result(&payload)
    }

    #[tool(
        description = "Structural code search (ast-grep) over the notebook's repo- and folder-backed files: match syntax, not text — `fn $NAME($$$) { $$$ }` finds every function, `$X.unwrap()` every unwrap call, across Rust/TS/Python/Go/Ruby/Java and the other bundled grammars. `$NAME` matches one node, `$$$` any number. Use grep_sources for plain text or regex."
    )]
    async fn ast_search(
        &self,
        Parameters(AstReq {
            notebook_id,
            pattern,
            max_results,
        }): Parameters<AstReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let files = commands::repo_backed_files(&state, &notebook_id, None).await;
        if files.is_empty() {
            return Err(invalid(
                "this notebook has no repo- or folder-backed files to search",
            ));
        }
        let k = max_results.unwrap_or(10).clamp(1, 30) as usize;
        let paths: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
        let hits = tokio::task::spawn_blocking(move || {
            crate::outline::ast_search_files(&pattern, &paths, k)
        })
        .await
        .map_err(internal)?;
        let payload: Vec<serde_json::Value> = hits
            .into_iter()
            .map(|h| {
                let (path, id, title) = &files[h.file_index];
                serde_json::json!({
                    "sourceId": id,
                    "sourceTitle": title,
                    "path": path,
                    "line": h.line,
                    "text": h.text,
                })
            })
            .collect();
        json_result(&payload)
    }

    #[tool(
        description = "Search with the working shown: the same hybrid retrieval as `search`, but returning every stage — vector hits, BM25 keyword hits, the rank-fused pool, the final top-k, and warnings (e.g. the keyword index failing and degrading to vector-only, which `search` hides). Use when retrieval quality looks wrong and you need to see WHY a passage did or didn't surface. Snippets are truncated; use `search` or get_source for full text."
    )]
    async fn search_debug(
        &self,
        Parameters(SearchReq {
            notebook_id,
            query,
            max_results,
        }): Parameters<SearchReq>,
    ) -> Result<CallToolResult, McpError> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Err(invalid("query is empty"));
        }
        let k = max_results.unwrap_or(6).clamp(1, 20) as usize;
        let state = self.state();
        let ai = state.ai.read().await.clone();
        let query_vec = ai.embed_one(&query).await.map_err(internal)?;
        // Cross-encoder tiers retrieve a 3x pool and rerank it down to k,
        // same as chat (Router::xenc_model) — agents get the same quality.
        let fetch_k = if ai.has_xenc() { k * 3 } else { k };
        let mut trace = state
            .db
            .search_chunks_trace(&notebook_id, query_vec, &query, fetch_k, None)
            .await
            .map_err(internal)?;
        trace.final_hits = ai.rerank_hits(&query, trace.final_hits, k).await;
        let compact = |hits: &[crate::models::Citation]| -> Vec<serde_json::Value> {
            hits.iter()
                .map(|c| {
                    serde_json::json!({
                        "chunkId": c.chunk_id,
                        "sourceId": c.source_id,
                        "sourceTitle": c.source_title,
                        // On-disk path of the original file (empty for web/
                        // mac/note hits) — lets an agent open the source
                        // directly instead of a get_source round-trip.
                        "sourcePath": c.source_path,
                        "noteId": c.note_id,
                        "distance": c.distance,
                        "snippet": c.snippet.chars().take(200).collect::<String>(),
                    })
                })
                .collect()
        };
        json_result(&serde_json::json!({
            "query": query,
            "k": k,
            "warnings": trace.warnings,
            "vectorHits": compact(&trace.vector_hits),
            "ftsHits": compact(&trace.fts_hits),
            "fusedHits": compact(&trace.fused_hits),
            "finalHits": compact(&trace.final_hits),
        }))
    }

    #[tool(
        description = "Retrieve passages for a question across ALL notebooks at once (hybrid vector + keyword, rank-fused, plus matching notes). Each passage names its notebook — use this to answer 'which notebook has…' questions or to ground corpus-wide answers. Synthesize the answer yourself from the passages."
    )]
    async fn ask_everything(
        &self,
        Parameters(AskEverythingReq { question }): Parameters<AskEverythingReq>,
    ) -> Result<CallToolResult, McpError> {
        let question = question.trim().to_string();
        if question.is_empty() {
            return Err(invalid("question is empty"));
        }
        let state = self.state();
        // No deep rerank here: MCP agents synthesize from raw passages and
        // are better served by fast, wide retrieval they can filter.
        let passages = commands::retrieve_everything(&state, &question, 16, false)
            .await
            .map_err(internal)?;
        json_result(&passages)
    }
}
