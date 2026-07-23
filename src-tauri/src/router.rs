//! Semantic router (docs/RFC-retrieval-maturity.md Phase 4): embed one
//! summary per notebook so corpus-wide questions can be routed to the most
//! likely notebooks before chunk search. Modeled on the classic KB-router pattern.
//!
//! The index is self-healing rather than hooked into every write path:
//! `ensure_router` recomputes the cheap text summaries from current db state,
//! diffs them against what's stored, and re-embeds only what changed —
//! a no-op string comparison on the common path.

use anyhow::Result;

use crate::ai::Ai;
use crate::db::{Db, Route};

/// Notebooks at or below this count skip routing entirely: filtering to the
/// top-N of N notebooks is the flat search with extra steps.
pub const MIN_NOTEBOOKS_TO_ROUTE: usize = 5;
/// How many notebooks a routed meta-chat search keeps.
pub const ROUTE_TOP_K: usize = 4;
/// Route entries consulted per query before aggregating to notebooks.
const ROUTE_POOL: usize = 24;
/// Cap on a route summary ("title — gist"): enough for the full gist body,
/// bounded so one verbose distillate can't dominate embedding time.
const ROUTE_SUMMARY_CHARS: usize = 480;

/// One route per source and per note, not one per notebook: a notebook
/// holding invoices AND travel journals AND recipes has no single point in
/// embedding space, and a merged summary dilutes every topic in it (measured:
/// notebook-level summaries misrouted 17% of dataset queries at top-2). With
/// per-item routes a notebook is as close as its closest item. Titles are
/// the summary — strong signal, and cheap enough to diff on every call.
async fn desired_routes(db: &Db) -> Result<Vec<Route>> {
    // Sources with a gist route on "title — gist" instead of the bare
    // title (RFC-infinite-context §1): the distillate names what the source
    // is ABOUT, which is exactly the signal routing lacks when titles are
    // opaque ("IMG_4032.pdf"). Self-heals through the same summary diff —
    // a new gist changes the summary string, which re-embeds the route.
    let gists: std::collections::HashMap<String, String> = db
        .list_gists()
        .await?
        .into_iter()
        .map(|g| (g.source_id, g.text))
        .collect();
    let mut desired: Vec<Route> = Vec::new();
    for nb in db.list_notebooks().await? {
        for s in db.list_sources(&nb.id).await? {
            let summary = match gists.get(&s.id) {
                Some(g) => {
                    let mut s2 = format!("{} — {}", s.title, g);
                    if s2.chars().count() > ROUTE_SUMMARY_CHARS {
                        s2 = s2.chars().take(ROUTE_SUMMARY_CHARS).collect();
                    }
                    s2
                }
                None => s.title.clone(),
            };
            desired.push(Route {
                id: format!("src:{}", s.id),
                kind: "source".into(),
                notebook_id: nb.id.clone(),
                summary,
            });
        }
        for n in db.list_notes(&nb.id).await? {
            desired.push(Route {
                id: format!("note:{}", n.id),
                kind: "note".into(),
                notebook_id: nb.id.clone(),
                summary: n.title.clone(),
            });
        }
    }
    Ok(desired)
}

/// Bring the router index in line with the corpus. Returns
/// (embedded, deleted) counts — (0, 0) when nothing changed.
pub async fn ensure_router(db: &Db, ai: &Ai) -> Result<(usize, usize)> {
    let desired = desired_routes(db).await?;

    let stored = db.list_routes().await?;
    let stored_by_id: std::collections::HashMap<&str, &Route> =
        stored.iter().map(|r| (r.id.as_str(), r)).collect();
    let changed: Vec<Route> = desired
        .iter()
        .filter(|r| {
            stored_by_id
                .get(r.id.as_str())
                .is_none_or(|s| s.summary != r.summary)
        })
        .cloned()
        .collect();
    let desired_ids: std::collections::HashSet<&str> =
        desired.iter().map(|r| r.id.as_str()).collect();
    let stale: Vec<String> = stored
        .iter()
        .filter(|r| !desired_ids.contains(r.id.as_str()))
        .map(|r| r.id.clone())
        .collect();

    if !changed.is_empty() {
        let inputs: Vec<String> = changed.iter().map(|r| r.summary.clone()).collect();
        let embeddings = ai.embed(&inputs).await?;
        db.upsert_routes(&changed, &embeddings).await?;
    }
    if !stale.is_empty() {
        db.delete_routes(&stale).await?;
    }
    Ok((changed.len(), stale.len()))
}

/// Top notebooks for a query, best first: nearest source/note routes,
/// aggregated to notebooks in first-appearance order (a notebook ranks as
/// high as its closest item). Empty when the router has no index yet —
/// callers fall back to flat search.
pub async fn route_notebooks(db: &Db, query_vec: Vec<f32>, k: usize) -> Result<Vec<String>> {
    let hits = db.route_search(query_vec, None, ROUTE_POOL).await?;
    let mut out: Vec<String> = Vec::new();
    for (r, _) in hits {
        if !out.contains(&r.notebook_id) {
            out.push(r.notebook_id);
            if out.len() >= k {
                break;
            }
        }
    }
    Ok(out)
}
