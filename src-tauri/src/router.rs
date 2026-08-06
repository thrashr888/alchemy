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
use crate::inference::{ChatTurn, Role};

/// How many notebooks the picker sees. Small-role models lose the thread on
/// long option lists, and the router's tail is noise by this depth anyway.
const SUGGEST_CANDIDATES: usize = 5;
/// How much of the incoming document the picker reads. Enough to tell an
/// invoice from a paper; short enough for a 3B model's context.
const SUGGEST_EXCERPT_CHARS: usize = 700;
/// Source titles listed per candidate notebook, as its description.
const SUGGEST_TITLES_PER_NOTEBOOK: usize = 6;

/// Where an unfiled source should go: an existing notebook, or a new one.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookSuggestion {
    /// Empty when proposing a new notebook — `title` is then the proposal.
    pub notebook_id: String,
    pub title: String,
    /// True when nothing on hand fits and `title` is a proposed new name.
    pub is_new: bool,
}

/// Suggest the notebook an incoming source belongs in (the "drop a link and
/// it files itself" path).
///
/// Two stages, because neither alone is good enough: the router narrows the
/// corpus to its nearest few notebooks by embedding — cheap, and it scales
/// past what any prompt could list — then the Small model picks among them,
/// which is what catches "this is a recipe, and none of these five are about
/// food". A raw distance threshold would have to guess at the embedding
/// metric; asking the model sidesteps that and yields a name for the new
/// notebook for free.
///
/// Never errors into the caller's face: any failure (no embedder, no model,
/// empty corpus) falls back to the most recently updated notebook, which is
/// exactly the default the picker used before this existed.
pub async fn suggest_notebook(
    db: &Db,
    ai: &Ai,
    title: &str,
    body: &str,
) -> Result<NotebookSuggestion> {
    let notebooks = db.list_notebooks().await?;
    let active: Vec<_> = notebooks
        .into_iter()
        .filter(|n| n.status != "archived")
        .collect();
    let Some(fallback) = active.first().cloned() else {
        // No notebook to file into: propose one named after the document.
        return Ok(NotebookSuggestion {
            notebook_id: String::new(),
            title: proposed_title(ai, title, body).await,
            is_new: true,
        });
    };
    let fallback = NotebookSuggestion {
        notebook_id: fallback.id,
        title: fallback.title,
        is_new: false,
    };

    let excerpt: String = body.chars().take(SUGGEST_EXCERPT_CHARS).collect();
    let query = format!("{title}\n{excerpt}");

    // Route to candidates. An empty or unbuilt index just means "consider
    // everything" — with a handful of notebooks that is the same answer.
    let mut candidates: Vec<_> = match ai.embed(std::slice::from_ref(&query)).await {
        Ok(vecs) if !vecs.is_empty() => {
            let ranked = route_notebooks(db, vecs[0].clone(), SUGGEST_CANDIDATES).await?;
            ranked
                .iter()
                .filter_map(|id| active.iter().find(|n| &n.id == id).cloned())
                .collect()
        }
        _ => vec![],
    };
    if candidates.is_empty() {
        candidates = active.iter().take(SUGGEST_CANDIDATES).cloned().collect();
    }

    // Describe each candidate by what is actually in it — a title alone
    // ("Research") tells the model nothing.
    let mut listing = String::new();
    for (i, nb) in candidates.iter().enumerate() {
        let titles: Vec<String> = db
            .list_sources(&nb.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .take(SUGGEST_TITLES_PER_NOTEBOOK)
            .map(|s| s.title)
            .collect();
        listing.push_str(&format!("{}. {}", i + 1, nb.title));
        if !titles.is_empty() {
            listing.push_str(&format!(" — contains: {}", titles.join("; ")));
        }
        listing.push('\n');
    }

    // Plain-text reply, one token's worth: Small-role models (3-8B, Apple FM)
    // do not parse JSON reliably (same finding as gist.rs).
    let messages = vec![
        ChatTurn::system(
            "You file incoming documents into the right notebook. Reply with ONLY a \
             single number from the list, or the word NEW. No explanation.",
        ),
        ChatTurn::user(format!(
            "Incoming document:\nTitle: {title}\nExcerpt: {excerpt}\n\n\
             Notebooks:\n{listing}\n\
             Which notebook does this document belong in? Reply with its number. \
             If none of them is a good fit, reply NEW."
        )),
    ];
    let reply = match ai.chat_role(Role::Small, &messages).await {
        Ok(out) => out.text,
        // No Small model configured, or the engine is down — the recency
        // default is still a reasonable answer.
        Err(_) => return Ok(fallback),
    };

    let answer = reply.trim().to_uppercase();
    if answer.starts_with("NEW") {
        return Ok(NotebookSuggestion {
            notebook_id: String::new(),
            title: proposed_title(ai, title, body).await,
            is_new: true,
        });
    }
    // First integer anywhere in the reply: small models like to say
    // "2." or "Notebook 2" however firmly the prompt asks for a bare number.
    let picked = answer
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n >= 1 && *n <= candidates.len())
        .map(|n| &candidates[n - 1]);
    Ok(match picked {
        Some(nb) => NotebookSuggestion {
            notebook_id: nb.id.clone(),
            title: nb.title.clone(),
            is_new: false,
        },
        None => fallback,
    })
}

/// A short notebook name for a document that fits nowhere. Falls back to the
/// document's own title, which is never wrong, only unambitious.
async fn proposed_title(ai: &Ai, title: &str, body: &str) -> String {
    let excerpt: String = body.chars().take(SUGGEST_EXCERPT_CHARS).collect();
    let messages = vec![
        ChatTurn::system(
            "You name notebooks. Reply with ONLY the name — two to four words, \
             no quotes, no punctuation, no explanation.",
        ),
        ChatTurn::user(format!(
            "Name a notebook that would collect documents like this one.\n\
             Title: {title}\nExcerpt: {excerpt}"
        )),
    ];
    let clean = |s: &str| -> Option<String> {
        let t = s
            .lines()
            .find(|l| !l.trim().is_empty())?
            .trim()
            .trim_matches(['"', '\'', '*', '#', '.'])
            .trim()
            .to_string();
        // A model that ignored the instruction and wrote a sentence is worse
        // than the document's own title.
        (!t.is_empty() && t.chars().count() <= 40).then_some(t)
    };
    match ai.chat_role(Role::Small, &messages).await {
        Ok(out) => clean(&out.text).unwrap_or_else(|| title.to_string()),
        Err(_) => title.to_string(),
    }
}

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

/// One route summary string: `"{title} [{tags}] — {gist}"`, brackets omitted
/// when the source has no tags, the gist arm omitted when there is none,
/// capped to `ROUTE_SUMMARY_CHARS`. Tags are user ground truth
/// (docs/RFC-source-tags.md) — a few tag tokens meaningfully shift a short
/// summary's embedding, and the self-healing diff re-embeds on any change.
fn route_summary(title: &str, tags: &str, gist: Option<&str>) -> String {
    let mut summary = if tags.is_empty() {
        title.to_string()
    } else {
        format!("{title} [{tags}]")
    };
    if let Some(g) = gist {
        summary = format!("{summary} — {g}");
    }
    if summary.chars().count() > ROUTE_SUMMARY_CHARS {
        summary = summary.chars().take(ROUTE_SUMMARY_CHARS).collect();
    }
    summary
}

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
    // User tags ride along the same way (RFC-source-tags §Retrieval).
    let gists: std::collections::HashMap<String, String> = db
        .list_gists()
        .await?
        .into_iter()
        .map(|g| (g.source_id, g.text))
        .collect();
    let mut desired: Vec<Route> = Vec::new();
    for nb in db.list_notebooks().await? {
        for s in db.list_sources(&nb.id).await? {
            let summary = route_summary(&s.title, &s.tags, gists.get(&s.id).map(String::as_str));
            desired.push(Route {
                id: format!("src:{}", s.id),
                kind: "source".into(),
                notebook_id: nb.id.clone(),
                summary,
            });
        }
        // Titles only — the route summary never reads a note's body, and
        // this runs on the ask-everything request path.
        for (id, title, _) in db.list_note_meta(Some(&nb.id)).await? {
            desired.push(Route {
                id: format!("note:{id}"),
                kind: "note".into(),
                notebook_id: nb.id.clone(),
                summary: title,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC-source-tags: tags join the route summary in brackets, vanish
    /// without residue when empty, compose with the gist arm, and the cap
    /// still applies to the combined string.
    #[test]
    fn route_summary_folds_tags_and_gist() {
        assert_eq!(route_summary("Doc", "", None), "Doc");
        assert_eq!(route_summary("Doc", "rust lance", None), "Doc [rust lance]");
        assert_eq!(route_summary("Doc", "", Some("about x")), "Doc — about x");
        assert_eq!(
            route_summary("Doc", "rust", Some("about x")),
            "Doc [rust] — about x"
        );
        let long_gist = "g".repeat(ROUTE_SUMMARY_CHARS * 2);
        let capped = route_summary("Doc", "rust", Some(&long_gist));
        assert_eq!(capped.chars().count(), ROUTE_SUMMARY_CHARS);
        assert!(capped.starts_with("Doc [rust] — g"));
    }
}
