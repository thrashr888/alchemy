//! Proactive growth (docs/RFC-living-notebook.md Pillar 2, phase 2): the
//! frontier already inside the notebook. Standing queries come from
//! retrieval traces that returned thin evidence; candidates are outbound
//! links found in existing sources' extracted text; ranking is
//! deterministic — mention count, spread across sources, and overlap with
//! the standing queries' tokens. No model call, no network: the proposal
//! tray is the only thing that ever fetches, and only on an explicit Add.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::models::Source;

/// One proposed addition: a URL the notebook's own sources keep pointing
/// at, or a local file Spotlight matched against a standing query.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthProposal {
    /// "web" (url points at the network) | "local" (url is an on-disk path).
    pub kind: String,
    pub url: String,
    /// Best anchor text seen for the link, or the file's name.
    pub anchor: String,
    /// Total times the URL appears across the notebook (0 for local hits).
    pub mentions: u32,
    /// Distinct sources that point at it (0 for local hits).
    pub source_count: u32,
    /// The standing query it best matches ("" when ranked on spread alone).
    pub matched_query: String,
    pub score: f32,
}

/// A retrieval that came back thin: the notebook was asked and had little
/// to say. Recent ones become standing queries the frontier ranks against.
const THIN_CITATIONS: usize = 3;
const QUERY_WINDOW_MS: i64 = 45 * 86_400_000;
const MAX_QUERIES: usize = 8;

pub fn standing_queries(trace_dir: &Path, notebook_id: &str, now_ms: i64) -> Vec<String> {
    let mut out: Vec<(i64, String)> = Vec::new();
    let mut seen = HashSet::new();
    for file in ["retrieval.jsonl", "retrieval.1.jsonl"] {
        let Ok(text) = std::fs::read_to_string(trace_dir.join(file)) else {
            continue;
        };
        for line in text.lines() {
            let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if rec.get("notebookId").and_then(|v| v.as_str()) != Some(notebook_id) {
                continue;
            }
            let ts = rec.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);
            if now_ms - ts > QUERY_WINDOW_MS {
                continue;
            }
            let cites = rec
                .get("citations")
                .and_then(|c| c.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            if cites >= THIN_CITATIONS {
                continue;
            }
            let Some(query) = rec.get("query").and_then(|v| v.as_str()) else {
                continue;
            };
            let key = query.trim().to_lowercase();
            if key.len() < 8 || !seen.insert(key) {
                continue;
            }
            out.push((ts, query.trim().to_string()));
        }
    }
    // Most recent hunger first.
    out.sort_by_key(|(ts, _)| -*ts);
    out.into_iter().take(MAX_QUERIES).map(|(_, q)| q).collect()
}

/// Outbound links in extracted text: markdown `[anchor](https://…)` plus
/// bare URLs. Returned as (normalized url, anchor).
fn extract_links(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(pos) = text[i..].find("http") {
        let start = i + pos;
        let rest = &text[start..];
        if !rest.starts_with("http://") && !rest.starts_with("https://") {
            i = start + 4;
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || c == ')' || c == '"' || c == '<' || c == '\'')
            .unwrap_or(rest.len());
        let raw = rest[..end].trim_end_matches(['.', ',', ';', ']', '}']);
        // Markdown anchor: the "](url" shape puts "[anchor]" just before.
        let anchor = if start >= 2 && &bytes[start - 2..start] == b"](" {
            text[..start - 2]
                .rfind('[')
                .map(|open| text[open + 1..start - 2].to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        if let Some(url) = normalize_url(raw) {
            out.push((url, anchor));
        }
        i = start + end.max(1);
    }
    out
}

/// Strip fragments and tracking params; drop obvious non-documents. None
/// means "not worth proposing" (media files, localhost, too short).
fn normalize_url(raw: &str) -> Option<String> {
    let no_frag = raw.split('#').next().unwrap_or(raw);
    if no_frag.len() < 12 || no_frag.contains("localhost") || no_frag.contains("127.0.0.1") {
        return None;
    }
    // Keep the query string minus tracking params.
    let (base, query) = no_frag.split_once('?').unwrap_or((no_frag, ""));
    const SKIP_EXT: [&str; 8] = [
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".ico", ".css", ".js",
    ];
    let path = base.to_lowercase();
    if SKIP_EXT.iter().any(|ext| path.ends_with(ext)) {
        return None;
    }
    let kept: Vec<&str> = query
        .split('&')
        .filter(|kv| !kv.is_empty() && !kv.starts_with("utm_") && !kv.starts_with("ref="))
        .collect();
    let mut url = base.trim_end_matches('/').to_string();
    if !kept.is_empty() {
        url.push('?');
        url.push_str(&kept.join("&"));
    }
    Some(url)
}

fn tokens(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 3)
        .map(|w| w.to_string())
        .collect()
}

/// Rank the notebook's outbound links against its standing queries.
pub fn proposals(sources: &[Source], queries: &[String]) -> Vec<GrowthProposal> {
    let existing: HashSet<String> = sources
        .iter()
        .filter(|s| !s.url.is_empty())
        .filter_map(|s| normalize_url(&s.url))
        .collect();
    struct Cand {
        anchor: String,
        mentions: u32,
        sources: HashSet<String>,
    }
    let mut cands: HashMap<String, Cand> = HashMap::new();
    for s in sources {
        if s.content.is_empty() {
            continue;
        }
        for (url, anchor) in extract_links(&s.content) {
            if existing.contains(&url) {
                continue;
            }
            // A source linking to itself under a variant URL is noise.
            if !s.url.is_empty() && url.contains(s.url.trim_end_matches('/')) {
                continue;
            }
            let c = cands.entry(url).or_insert(Cand {
                anchor: String::new(),
                mentions: 0,
                sources: HashSet::new(),
            });
            c.mentions += 1;
            c.sources.insert(s.id.clone());
            if anchor.len() > c.anchor.len() && anchor.len() < 120 {
                c.anchor = anchor;
            }
        }
    }
    let query_tokens: Vec<(String, HashSet<String>)> =
        queries.iter().map(|q| (q.clone(), tokens(q))).collect();
    let mut out: Vec<GrowthProposal> = cands
        .into_iter()
        .map(|(url, c)| {
            let cand_tokens = tokens(&format!("{} {}", c.anchor, url));
            let (matched_query, overlap) = query_tokens
                .iter()
                .map(|(q, qt)| (q.clone(), qt.intersection(&cand_tokens).count()))
                .max_by_key(|(_, n)| *n)
                .filter(|(_, n)| *n > 0)
                .unwrap_or_default();
            let score = c.mentions as f32 + 2.0 * c.sources.len() as f32 + 3.0 * overlap as f32;
            GrowthProposal {
                kind: "web".into(),
                url,
                anchor: c.anchor,
                mentions: c.mentions,
                source_count: c.sources.len() as u32,
                matched_query,
                score,
            }
        })
        // A link one source mentions once, matching nothing, is not a
        // signal — 1 mention + 1 source scores exactly 3.0 and stays out.
        .filter(|p| p.score > 3.0)
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.url.cmp(&b.url)));
    out.truncate(12);
    out
}

/// The local tier (RFC-living-notebook Pillar 2): standing queries swept
/// through Spotlight via filesearch — files already on this Mac that speak
/// to what the notebook was asked and couldn't answer. No network at all,
/// and they rank above web proposals: the cheapest fetch never leaves the
/// machine. At most three queries run (mdfind is a subprocess) and six
/// hits return.
pub async fn local_proposals(sources: &[Source], queries: &[String]) -> Vec<GrowthProposal> {
    let existing: HashSet<&str> = sources.iter().map(|s| s.url.as_str()).collect();
    let mut out: Vec<GrowthProposal> = Vec::new();
    let mut seen_names = HashSet::new();
    for query in queries.iter().take(4) {
        let qt = tokens(query);
        // Spotlight ANDs bare-query words, so a whole question matches
        // nothing. Sweep the two longest tokens separately ("temperature",
        // "gaggia") and let the name-overlap filter below supply precision.
        let mut kw: Vec<&String> = qt.iter().collect();
        kw.sort_by_key(|w| std::cmp::Reverse(w.len()));
        let mut kept = 0;
        for token in kw.into_iter().take(2) {
            for hit in crate::filesearch::search(token, 6).await {
                if hit.is_dir || !hit.ingestible || existing.contains(hit.path.as_str()) {
                    continue;
                }
                // Only files whose NAME shares words with the query — two
                // of them when the query has two to give. One shared token
                // proposed roof invoices for an autopilot question; a
                // content-only match is worse still. Same-named copies
                // across the disk collapse to one hit.
                let need = 2.min(qt.len());
                if tokens(&hit.name).intersection(&qt).count() < need {
                    continue;
                }
                if !seen_names.insert(hit.name.to_lowercase()) {
                    continue;
                }
                out.push(GrowthProposal {
                    kind: "local".into(),
                    // Above every web score; earlier queries (more recent
                    // hunger) rank first, then Spotlight's order holds.
                    score: 100.0 - out.len() as f32,
                    url: hit.path,
                    anchor: hit.name,
                    mentions: 0,
                    source_count: 0,
                    matched_query: query.clone(),
                });
                kept += 1;
                if kept >= 2 || out.len() >= 6 {
                    break;
                }
            }
            if kept >= 2 || out.len() >= 6 {
                break;
            }
        }
        if out.len() >= 6 {
            break;
        }
    }
    out
}

/// The open-web tier: standing queries through Firecrawl's keyless search
/// (docs.firecrawl.dev — free tier is 1,000 credits/month, search costs 2
/// credits per query at our result size). Only search metadata comes back;
/// the page itself is fetched by the existing import path when the user
/// accepts a proposal. Usage is metered in traces/growth.jsonl against a
/// soft cap kept well inside the free tier.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthWebSearch {
    pub proposals: Vec<GrowthProposal>,
    pub credits_this_month: u32,
    pub capped: bool,
}

pub const WEB_CREDIT_CAP: u32 = 800;
const CREDITS_PER_SEARCH: u32 = 2;
const GROWTH_TRACE: &str = "growth.jsonl";
const WEB_CACHE: &str = "growth-web-cache.json";

/// One Firecrawl round per notebook per day is plenty: standing queries
/// move slowly, and reopening the pane must not spend credits. The cache
/// keys on the query set too, so new hunger busts it early.
fn day_of(now_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(now_ms)
        .unwrap_or_default()
        .format("%Y-%m-%d")
        .to_string()
}

fn read_web_cache(trace_dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(trace_dir.join(WEB_CACHE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn cached_web(
    trace_dir: &Path,
    notebook_id: &str,
    queries: &[String],
    now_ms: i64,
) -> Option<Vec<GrowthProposal>> {
    let cache = read_web_cache(trace_dir);
    let entry = cache.get(notebook_id)?;
    if entry.get("day")?.as_str()? != day_of(now_ms) {
        return None;
    }
    if entry.get("key")?.as_str()? != queries.join("\n") {
        return None;
    }
    serde_json::from_value(entry.get("proposals")?.clone()).ok()
}

fn store_web_cache(
    trace_dir: &Path,
    notebook_id: &str,
    queries: &[String],
    proposals: &[GrowthProposal],
    now_ms: i64,
) {
    let mut cache = read_web_cache(trace_dir);
    cache[notebook_id] = serde_json::json!({
        "day": day_of(now_ms),
        "key": queries.join("\n"),
        "proposals": proposals,
    });
    let _ = std::fs::create_dir_all(trace_dir);
    let _ = std::fs::write(trace_dir.join(WEB_CACHE), cache.to_string());
}

pub fn credits_this_month(trace_dir: &Path, now_ms: i64) -> u32 {
    use chrono::Datelike;
    let now = chrono::DateTime::from_timestamp_millis(now_ms).unwrap_or_default();
    let mut total = 0u32;
    let Ok(text) = std::fs::read_to_string(trace_dir.join(GROWTH_TRACE)) else {
        return 0;
    };
    for line in text.lines() {
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let ts = rec.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);
        let then = chrono::DateTime::from_timestamp_millis(ts).unwrap_or_default();
        if (then.year(), then.month()) == (now.year(), now.month()) {
            total += rec.get("credits").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        }
    }
    total
}

pub async fn web_search(
    trace_dir: &Path,
    notebook_id: &str,
    sources: &[Source],
    queries: &[String],
    now_ms: i64,
) -> GrowthWebSearch {
    let mut spent = credits_this_month(trace_dir, now_ms);
    if let Some(cached) = cached_web(trace_dir, notebook_id, queries, now_ms) {
        return GrowthWebSearch {
            proposals: cached,
            credits_this_month: spent,
            capped: false,
        };
    }
    if spent >= WEB_CREDIT_CAP {
        return GrowthWebSearch {
            proposals: Vec::new(),
            credits_this_month: spent,
            capped: true,
        };
    }
    let existing: HashSet<String> = sources
        .iter()
        .filter(|s| !s.url.is_empty())
        .filter_map(|s| normalize_url(&s.url))
        .collect();
    let Ok(client) = reqwest::Client::builder()
        .user_agent(concat!("alchemy/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(15))
        .build()
    else {
        return GrowthWebSearch {
            proposals: Vec::new(),
            credits_this_month: spent,
            capped: false,
        };
    };
    let mut out: Vec<GrowthProposal> = Vec::new();
    let mut seen = HashSet::new();
    for query in queries.iter().take(2) {
        if spent + CREDITS_PER_SEARCH > WEB_CREDIT_CAP {
            break;
        }
        // Keyless request — no Authorization header; the free tier answers
        // at low rate limits, which one search per pane-open never troubles.
        let resp = client
            .post("https://api.firecrawl.dev/v2/search")
            .json(&serde_json::json!({ "query": query, "limit": 5 }))
            .send()
            .await;
        let Ok(resp) = resp else { continue };
        let Ok(body) = resp.json::<serde_json::Value>().await else {
            continue;
        };
        spent += CREDITS_PER_SEARCH;
        crate::trace::log_file(
            trace_dir,
            GROWTH_TRACE,
            serde_json::json!({ "ts": now_ms, "credits": CREDITS_PER_SEARCH, "query": query }),
        );
        let hits = body
            .pointer("/data/web")
            .and_then(|w| w.as_array())
            .cloned()
            .unwrap_or_default();
        for (rank, hit) in hits.into_iter().enumerate() {
            let Some(raw) = hit.get("url").and_then(|u| u.as_str()) else {
                continue;
            };
            let Some(url) = normalize_url(raw) else {
                continue;
            };
            if existing.contains(&url) || !seen.insert(url.clone()) {
                continue;
            }
            out.push(GrowthProposal {
                kind: "search".into(),
                url,
                anchor: hit
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string(),
                mentions: 0,
                source_count: 0,
                matched_query: query.clone(),
                score: 10.0 - rank as f32,
            });
        }
    }
    store_web_cache(trace_dir, notebook_id, queries, &out, now_ms);
    GrowthWebSearch {
        proposals: out,
        credits_this_month: spent,
        capped: false,
    }
}

/// Every source id that has appeared as a citation in the retrieval traces
/// (current + one rotated generation — months of history at the 5 MB
/// rotation size). Shared by the uncited facet and the retirement pass.
pub fn cited_ids(trace_dir: &Path) -> HashSet<String> {
    let mut ids = HashSet::new();
    for file in ["retrieval.jsonl", "retrieval.1.jsonl"] {
        let Ok(text) = std::fs::read_to_string(trace_dir.join(file)) else {
            continue;
        };
        for line in text.lines() {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(cites) = record.get("citations").and_then(|c| c.as_array()) else {
                continue;
            };
            for cite in cites {
                if let Some(id) = cite.get("sourceId").and_then(|s| s.as_str()) {
                    if !id.is_empty() {
                        ids.insert(id.to_string());
                    }
                }
            }
        }
    }
    ids
}

/// One retirement candidate (Pillar 3): old enough to have had its chance,
/// never once cited. A proposal, never an action — the pane offers Mute
/// (drop from chat scope, reversible) or Remove.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetireProposal {
    pub source_id: String,
    pub title: String,
    pub age_days: i64,
    pub char_count: i64,
}

const RETIRE_MIN_AGE_DAYS: i64 = 45;

pub fn retire_candidates(
    sources: &[Source],
    cited: &HashSet<String>,
    now_ms: i64,
) -> Vec<RetireProposal> {
    let mut out: Vec<RetireProposal> = sources
        .iter()
        .filter(|s| s.status == "ready" && !FOLDER_TYPES.contains(&s.source_type.as_str()))
        .filter(|s| s.char_count > 0 && !cited.contains(&s.id))
        .filter_map(|s| {
            // A source only counts as passed-over after it has been around
            // (and fresh) long enough to have had its chance.
            let last_alive = s.created_at.max(s.fetched_at);
            let age_days = (now_ms - last_alive) / 86_400_000;
            (age_days >= RETIRE_MIN_AGE_DAYS).then(|| RetireProposal {
                source_id: s.id.clone(),
                title: s.title.clone(),
                age_days,
                char_count: s.char_count,
            })
        })
        .collect();
    out.sort_by_key(|p| std::cmp::Reverse(p.age_days));
    out.truncate(15);
    out
}

const FOLDER_TYPES: [&str; 4] = ["folder", "git", "notion", "obsidian"];

/// The wiki index (Pillar 3's north star, deterministic v1): one generated
/// note that maps the notebook — sources grouped by tag, linked by title
/// (the reader resolves title links), untagged and never-cited called out. Plain
/// markdown in an ordinary note, so it round-trips through OKF and any
/// agent can edit it; no model call, so it always works and costs nothing.
pub const WIKI_INDEX_TITLE: &str = "Notebook index";

pub fn build_wiki_index(sources: &[Source], cited: &HashSet<String>, now_ms: i64) -> String {
    let content: Vec<&Source> = sources
        .iter()
        .filter(|s| !FOLDER_TYPES.contains(&s.source_type.as_str()) && s.char_count > 0)
        .collect();
    let total_chars: i64 = content.iter().map(|s| s.char_count).sum();
    let mut by_tag: HashMap<&str, Vec<&Source>> = HashMap::new();
    let mut untagged: Vec<&Source> = Vec::new();
    for s in &content {
        let mut any = false;
        for tag in s.tags.split_whitespace() {
            by_tag.entry(tag).or_default().push(s);
            any = true;
        }
        if !any {
            untagged.push(s);
        }
    }
    let mut tags: Vec<(&str, Vec<&Source>)> = by_tag.into_iter().collect();
    tags.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));

    let line = |s: &Source| {
        let age_days = (now_ms - s.created_at.max(s.fetched_at)) / 86_400_000;
        let mut extra = format!("{} chars", s.char_count);
        if age_days > RETIRE_MIN_AGE_DAYS && !cited.contains(&s.id) {
            extra.push_str(" · never cited");
        }
        // Standard markdown links, not [[wikilinks]]: the note editor's
        // link routing resolves the href against the corpus by title
        // (ReaderPane::resolveInCorpus), and TipTap parses `[t](<dest>)`
        // where wikilink syntax would stay literal text. Angle-bracket
        // destinations carry spaces and parens; titles holding <> can't
        // be a destination and fall back to plain text.
        let text = s.title.replace('[', "(").replace(']', ")");
        if s.title.contains(['<', '>']) {
            format!("- {text} — {extra}\n")
        } else {
            format!("- [{text}](<{}>) — {extra}\n", s.title)
        }
    };
    let mut md = format!(
        "A living map of this notebook — {} sources, {} characters, grouped \
         by tag. Regenerate it from the Grow pane; links open the source.\n",
        content.len(),
        total_chars,
    );
    for (tag, mut list) in tags {
        list.sort_by_key(|s| std::cmp::Reverse(s.char_count));
        md.push_str(&format!("\n## #{tag}\n\n"));
        for s in list {
            md.push_str(&line(s));
        }
    }
    if !untagged.is_empty() {
        md.push_str("\n## Untagged\n\n");
        untagged.sort_by_key(|s| std::cmp::Reverse(s.char_count));
        for s in untagged {
            md.push_str(&line(s));
        }
    }
    md
}

/// Continuous consolidation (RFC-living-notebook phase 5, WikiSkill's
/// argument made real): notebooks that HAVE an index note — creating one
/// is the opt-in — get it re-derived on every gist sweep, so the map
/// tracks the shelf without anyone pressing Refresh. Deterministic and
/// write-skipping: an unchanged body costs a read, never a commit.
pub async fn refresh_wiki_indexes(db: &crate::db::Db) -> anyhow::Result<usize> {
    let Some(trace_dir) = crate::trace::dir() else {
        // No traces handle (tests, early boot): a refresh without citation
        // data would strip the dust markers and churn every body — skip.
        return Ok(0);
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let cited = cited_ids(trace_dir);
    let mut refreshed = 0usize;
    for nb in db.list_notebooks().await? {
        let Some(note) = db
            .list_notes(&nb.id)
            .await?
            .into_iter()
            .find(|n| n.title == WIKI_INDEX_TITLE)
        else {
            continue;
        };
        let sources = db.list_sources(&nb.id).await?;
        let body = build_wiki_index(&sources, &cited, now_ms);
        if body == note.content {
            continue;
        }
        db.update_note(&note.id, &note.title, &body, now_ms).await?;
        refreshed += 1;
    }
    Ok(refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(id: &str, url: &str, content: &str) -> Source {
        Source {
            id: id.into(),
            notebook_id: "nb".into(),
            title: id.into(),
            source_type: "url".into(),
            url: url.into(),
            content: content.into(),
            char_count: content.len() as i64,
            chunk_count: 0,
            created_at: 0,
            status: "ready".into(),
            error: String::new(),
            parent_id: String::new(),
            mtime: 0,
            author: String::new(),
            image_url: String::new(),
            tags: String::new(),
            note: String::new(),
            fetched_at: 0,
            fetch_failures: 0,
        }
    }

    #[test]
    fn frontier_ranks_spread_and_query_overlap() {
        let sources = vec![
            src(
                "a",
                "https://one.test/a",
                "See [Rust async book](https://rust-lang.github.io/async-book) and \
                 https://tokio.rs/tutorial for background.",
            ),
            src(
                "b",
                "https://two.test/b",
                "The async book (https://rust-lang.github.io/async-book) again.",
            ),
        ];
        let queries = vec!["how does async cancellation work".to_string()];
        let props = proposals(&sources, &queries);
        assert_eq!(props[0].url, "https://rust-lang.github.io/async-book");
        assert_eq!(props[0].source_count, 2);
        assert_eq!(props[0].matched_query, queries[0]);
        // The single-mention tokio link scores below the threshold bar
        // unless boosted by a query match ("tutorial" ∉ query tokens).
        assert!(props.iter().all(|p| p.url != "https://tokio.rs/tutorial"));
    }

    #[test]
    fn existing_sources_and_media_links_are_excluded() {
        let sources = vec![
            src(
                "a",
                "https://one.test/a",
                "Links: https://two.test/b https://two.test/b \
                 https://img.test/pic.png?utm_source=x",
            ),
            src("b", "https://two.test/b", ""),
        ];
        let props = proposals(&sources, &[]);
        assert!(props.iter().all(|p| p.url != "https://two.test/b"));
        assert!(props.iter().all(|p| !p.url.contains("pic.png")));
    }

    #[test]
    fn normalize_strips_fragments_and_tracking() {
        assert_eq!(
            normalize_url("https://a.test/page?utm_source=tw&x=1#sec"),
            Some("https://a.test/page?x=1".into())
        );
        assert_eq!(normalize_url("https://a.test/logo.svg"), None);
    }
}
