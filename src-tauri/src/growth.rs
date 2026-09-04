//! Proactive growth (docs/RFC-living-notebook.md Pillar 2, phase 2): the
//! frontier already inside the notebook. Standing queries come from
//! retrieval traces that returned thin evidence; candidates are outbound
//! links found in existing sources' extracted text; ranking is
//! deterministic — mention count, spread across sources, and overlap with
//! the standing queries' tokens. No model call, no network: the proposal
//! tray is the only thing that ever fetches, and only on an explicit Add.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::models::{Note, RegistryCard, Source, SourceEvent};

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
        // Where the URL stops. Markdown delimiters end it as surely as
        // whitespace does: `[http://a](http://a)` used to run the scan
        // straight through the `](` and propose the whole splice as one
        // "url" (Reminders d566d11d). A bracket, a backtick or a pipe is
        // never part of an unescaped URL, so each one is a terminator.
        let end = rest
            .find(|c: char| {
                c.is_whitespace()
                    || matches!(
                        c,
                        ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '<' | '>' | '`' | '|' | '\\'
                    )
            })
            .unwrap_or(rest.len());
        let raw = rest[..end].trim_end_matches(['.', ',', ';', ']', '}']);
        // Markdown anchor: the "](url" shape puts "[anchor]" just before.
        // Emphasis wraps the anchor, not the words ("**The Mart**" is a bold
        // link to The Mart) — strip it so the proposal reads as a title.
        let anchor = if start >= 2 && &bytes[start - 2..start] == b"](" {
            text[..start - 2]
                .rfind('[')
                .map(|open| {
                    text[open + 1..start - 2]
                        .trim_matches(['*', '_', '`', ' '])
                        .to_string()
                })
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
    /// Current pacing: days between fresh Firecrawl rounds for this
    /// notebook, derived from budget and enabled-notebook count.
    pub refresh_every_days: u32,
}

pub const WEB_CREDIT_CAP: u32 = 800;
const CREDITS_PER_SEARCH: u32 = 2;
const GROWTH_TRACE: &str = "growth.jsonl";
const WEB_CACHE: &str = "growth-web-cache.json";

/// The per-notebook web-search opt-in is a real column on the notebook
/// row (`growth_web`), so it travels with the data and the background
/// sweep can act on it. It began life as a growth-web.json sidecar;
/// migrate_web_flags moves old installs over once at boot.
pub async fn web_enabled(db: &crate::db::Db, notebook_id: &str) -> bool {
    db.list_notebooks()
        .await
        .ok()
        .and_then(|nbs| nbs.into_iter().find(|n| n.id == notebook_id))
        .map(|n| n.growth_web)
        .unwrap_or(false)
}

pub async fn set_web_enabled(
    db: &crate::db::Db,
    notebook_id: &str,
    on: bool,
) -> anyhow::Result<()> {
    db.set_notebook_growth_web(notebook_id, on).await
}

/// Web-enabled notebooks, floored at 1 so the budget pacer never
/// divides the month by zero notebooks.
pub async fn web_enabled_count(db: &crate::db::Db) -> usize {
    db.list_notebooks()
        .await
        .map(|nbs| nbs.iter().filter(|n| n.growth_web).count())
        .unwrap_or(0)
        .max(1)
}

/// One-time migration off the growth-web.json sidecar into prefs; the
/// file is renamed so the copy never runs twice.
pub async fn migrate_web_flags(db: &crate::db::Db, trace_dir: &Path) {
    let path = trace_dir.join("growth-web.json");
    let Some(map) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    else {
        return;
    };
    if let Some(obj) = map.as_object() {
        for (id, v) in obj {
            if let Some(b) = v.as_bool() {
                let _ = set_web_enabled(db, id, b).await;
            }
        }
    }
    let _ = std::fs::rename(&path, trace_dir.join("growth-web.json.migrated"));
}

/// Refresh as fast as the budget allows: spread the credits left this
/// month evenly across the enabled notebooks over the days left in the
/// month. One notebook on a full budget hits the 1-day floor; forty
/// notebooks — or a nearly spent budget — stretch toward the 30-day
/// ceiling on their own. The cache also keys on the query set, so new
/// hunger still busts it immediately; the pacer only governs repeats.
const WEB_CACHE_TTL_MIN_MS: i64 = 86_400_000;
const WEB_CACHE_TTL_MAX_MS: i64 = 30 * 86_400_000;

fn ms_left_in_month(now_ms: i64) -> i64 {
    use chrono::{Datelike, TimeZone};
    let now = chrono::DateTime::from_timestamp_millis(now_ms).unwrap_or_default();
    let (y, m) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    chrono::Utc
        .with_ymd_and_hms(y, m, 1, 0, 0, 0)
        .single()
        .map(|d| d.timestamp_millis() - now_ms)
        .unwrap_or(0)
        .max(0)
}

fn web_cache_ttl_ms(queries: &[String], spent: u32, enabled_notebooks: usize, now_ms: i64) -> i64 {
    let remaining = WEB_CREDIT_CAP.saturating_sub(spent) as i64;
    let cost = CREDITS_PER_SEARCH as i64 * queries.len().clamp(1, 2) as i64;
    if remaining < cost {
        return WEB_CACHE_TTL_MAX_MS;
    }
    (ms_left_in_month(now_ms) * cost * enabled_notebooks as i64 / remaining)
        .clamp(WEB_CACHE_TTL_MIN_MS, WEB_CACHE_TTL_MAX_MS)
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
    ttl_ms: i64,
    now_ms: i64,
) -> Option<Vec<GrowthProposal>> {
    let cache = read_web_cache(trace_dir);
    let entry = cache.get(notebook_id)?;
    let ts = entry.get("ts").and_then(|v| v.as_i64())?;
    if now_ms - ts > ttl_ms {
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
        "ts": now_ms,
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
    enabled_notebooks: usize,
    now_ms: i64,
) -> GrowthWebSearch {
    let mut spent = credits_this_month(trace_dir, now_ms);
    let ttl_ms = web_cache_ttl_ms(queries, spent, enabled_notebooks, now_ms);
    let refresh_every_days = (ttl_ms / 86_400_000).max(1) as u32;
    if let Some(cached) = cached_web(trace_dir, notebook_id, queries, ttl_ms, now_ms) {
        return GrowthWebSearch {
            proposals: cached,
            credits_this_month: spent,
            capped: false,
            refresh_every_days,
        };
    }
    if spent >= WEB_CREDIT_CAP {
        return GrowthWebSearch {
            proposals: Vec::new(),
            credits_this_month: spent,
            capped: true,
            refresh_every_days,
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
            refresh_every_days,
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
        refresh_every_days,
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

const FOLDER_TYPES: [&str; 5] = ["folder", "git", "notion", "obsidian", "okf"];

/// One proposed tag merge (phase 5): two tags that are almost certainly
/// the same word — plural/singular or separator variants. Proposal only;
/// apply rewrites the `from` tag to `to` on every source carrying it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagMergeProposal {
    pub from: String,
    pub to: String,
    pub from_count: u32,
    pub to_count: u32,
}

pub fn tag_merge_proposals(sources: &[Source]) -> Vec<TagMergeProposal> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for s in sources {
        for t in s.tags.split_whitespace() {
            *counts.entry(t.to_string()).or_default() += 1;
        }
    }
    let canon = |t: &str| t.replace(['-', '_'], "");
    let mut tags: Vec<&String> = counts.keys().collect();
    tags.sort();
    let mut out: Vec<TagMergeProposal> = Vec::new();
    let mut taken: HashSet<String> = HashSet::new();
    for a in &tags {
        for b in &tags {
            if a == b || taken.contains(*a) || taken.contains(*b) {
                continue;
            }
            // Plural folds into singular; separator variants fold into the
            // more common spelling (ties: the shorter one).
            let (from, to) = if a.as_str() == format!("{b}s") || a.as_str() == format!("{b}es") {
                ((*a).clone(), (*b).clone())
            } else if canon(a) == canon(b) {
                let (ca, cb) = (counts[*a], counts[*b]);
                if cb > ca || (cb == ca && b.len() < a.len()) {
                    ((*a).clone(), (*b).clone())
                } else {
                    continue; // the mirrored iteration handles it
                }
            } else {
                continue;
            };
            taken.insert(from.clone());
            taken.insert(to.clone());
            out.push(TagMergeProposal {
                from_count: counts[&from],
                to_count: counts[&to],
                from,
                to,
            });
        }
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.from_count + m.to_count));
    out.truncate(8);
    out
}

/// The wiki index (Pillar 3's north star, deterministic v1): one generated
/// note that maps the notebook — sources grouped by tag, linked by title
/// (the reader resolves title links), untagged and never-cited called out. Plain
/// markdown in an ordinary note, so it round-trips through OKF and any
/// agent can edit it; no model call, so it always works and costs nothing.
pub const WIKI_INDEX_TITLE: &str = "Notebook index";

/// Confirmed registry cards with confirmed attachments in this notebook —
/// the entities whose pages the wiki carries.
pub fn notebook_entities<'c>(
    cards: &'c [RegistryCard],
    sources: &[Source],
    notebook_id: &str,
) -> Vec<(&'c RegistryCard, Vec<String>)> {
    let ids: HashSet<&str> = sources.iter().map(|s| s.id.as_str()).collect();
    cards
        .iter()
        .filter(|c| c.origin.is_empty())
        .filter_map(|c| {
            let attached: Vec<String> = c
                .attachments
                .iter()
                .filter(|a| {
                    a.status == "confirmed"
                        && a.notebook_id == notebook_id
                        && ids.contains(a.source_id.as_str())
                })
                .map(|a| a.source_id.clone())
                .collect();
            (!attached.is_empty()).then_some((c, attached))
        })
        .collect()
}

pub fn entity_page_title(card: &RegistryCard) -> String {
    format!("Entity: {}", card.name)
}

/// One entity page: the card's registry facts plus the documents filed
/// under it in this notebook, linked by title. Deterministic, like the
/// index — the registry is the source of truth, this is its wiki face.
pub fn build_entity_page(card: &RegistryCard, attached: &[&Source]) -> String {
    let mut md = format!("{} · from the registry\n", card.kind);
    if !card.identifiers.trim().is_empty() {
        md.push_str(&format!("\nIdentifiers: `{}`\n", card.identifiers.trim()));
    }
    if !card.note.trim().is_empty() {
        md.push_str(&format!("\n{}\n", card.note.trim()));
    }
    if !card.facts.is_empty() {
        md.push_str("\n## Facts\n\n");
        for f in &card.facts {
            if f.value.is_empty() {
                md.push_str(&format!("- {}\n", f.label));
            } else {
                md.push_str(&format!("- {}: {}\n", f.label, f.value));
            }
        }
    }
    md.push_str("\n## Documents\n\n");
    for s in attached {
        md.push_str(&title_link_line(s, &format!("{} chars", s.char_count)));
    }
    md
}

/// `- [Title](<Title>) — extra`, falling back to plain text when the title
/// can't be a link destination (see build_wiki_index).
fn title_link_line(s: &Source, extra: &str) -> String {
    let text = s.title.replace('[', "(").replace(']', ")");
    if s.title.contains(['<', '>']) {
        format!("- {text} — {extra}\n")
    } else {
        format!("- [{text}](<{}>) — {extra}\n", s.title)
    }
}

pub fn build_wiki_index(
    sources: &[Source],
    cited: &HashSet<String>,
    entities: &[(&RegistryCard, Vec<String>)],
    now_ms: i64,
) -> String {
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
    if !entities.is_empty() {
        md.push_str("\n## Entities\n\n");
        for (card, attached) in entities {
            let title = entity_page_title(card);
            md.push_str(&format!(
                "- [{title}](<{title}>) — {} · {} document{}\n",
                card.kind,
                attached.len(),
                if attached.len() == 1 { "" } else { "s" },
            ));
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

/// Create-or-refresh a notebook's wiki: the index note plus one page per
/// entity the registry files here. Upserts by title, write-skipping
/// unchanged bodies; returns the index note (None when the notebook has
/// no index and `create` is false — having one IS the opt-in) and how
/// many notes actually changed.
pub async fn upsert_wiki(
    db: &crate::db::Db,
    notebook_id: &str,
    cards: &[RegistryCard],
    cited: &HashSet<String>,
    now_ms: i64,
    create: bool,
) -> anyhow::Result<(Option<Note>, usize)> {
    let notes = db.list_notes(notebook_id).await?;
    let existing_index = notes.iter().find(|n| n.title == WIKI_INDEX_TITLE);
    if existing_index.is_none() && !create {
        return Ok((None, 0));
    }
    let sources = db.list_sources(notebook_id).await?;
    let entities = notebook_entities(cards, &sources, notebook_id);
    let mut changed = 0usize;

    let upsert = |title: String, body: String| {
        let found = notes.iter().find(|n| n.title == title).cloned();
        (title, body, found)
    };
    let mut writes: Vec<(String, String, Option<Note>)> = Vec::new();
    writes.push(upsert(
        WIKI_INDEX_TITLE.to_string(),
        build_wiki_index(&sources, cited, &entities, now_ms),
    ));
    let by_id: HashMap<&str, &Source> = sources.iter().map(|s| (s.id.as_str(), s)).collect();
    for (card, attached_ids) in &entities {
        let attached: Vec<&Source> = attached_ids
            .iter()
            .filter_map(|id| by_id.get(id.as_str()).copied())
            .collect();
        writes.push(upsert(
            entity_page_title(card),
            build_entity_page(card, &attached),
        ));
    }

    let mut index_note: Option<Note> = None;
    for (title, body, found) in writes {
        let is_index = title == WIKI_INDEX_TITLE;
        let note = match found {
            Some(mut note) => {
                if note.content != body {
                    db.update_note(&note.id, &note.title, &body, now_ms).await?;
                    note.content = body;
                    note.updated_at = now_ms;
                    changed += 1;
                }
                note
            }
            None => {
                let note = Note {
                    id: uuid::Uuid::new_v4().to_string(),
                    notebook_id: notebook_id.to_string(),
                    title,
                    content: body,
                    kind: "note".into(),
                    prompt: String::new(),
                    origin: String::new(),
                    status: String::new(),
                    created_at: now_ms,
                    updated_at: now_ms,
                };
                db.add_note(&note).await?;
                changed += 1;
                note
            }
        };
        if is_index {
            index_note = Some(note);
        }
    }
    Ok((index_note, changed))
}

/// The nightly web warm (the last deferred phase-5 piece): notebooks
/// whose owner enabled web search get their standing queries run through
/// Firecrawl during the sweep, so the Grow pane opens warm. The weekly
/// cache makes repeat sweeps free and the monthly cap still governs.
pub async fn sweep_web_searches(db: &crate::db::Db) -> anyhow::Result<usize> {
    let Some(trace_dir) = crate::trace::dir() else {
        return Ok(0);
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut warmed = 0usize;
    let notebooks = db.list_notebooks().await?;
    let enabled = notebooks.iter().filter(|n| n.growth_web).count().max(1);
    for nb in notebooks {
        if !nb.growth_web {
            continue;
        }
        let queries = standing_queries(trace_dir, &nb.id, now_ms);
        if queries.is_empty() {
            continue;
        }
        let sources = db.list_sources(&nb.id).await?;
        let before = credits_this_month(trace_dir, now_ms);
        let result = web_search(trace_dir, &nb.id, &sources, &queries, enabled, now_ms).await;
        if !result.proposals.is_empty() {
            warmed += 1;
            // Only a FRESH search (credits moved) earns a feed line — the
            // day-cached rerun every sweep would repeat itself otherwise.
            if result.credits_this_month > before {
                let _ = db
                    .add_source_event(&SourceEvent {
                        id: uuid::Uuid::new_v4().to_string(),
                        notebook_id: nb.id.clone(),
                        source_id: String::new(),
                        source_title: "Grow".into(),
                        kind: "growth".into(),
                        detail: format!(
                            "{} web proposal{} waiting",
                            result.proposals.len(),
                            if result.proposals.len() == 1 { "" } else { "s" }
                        ),
                        diff: String::new(),
                        at: now_ms,
                    })
                    .await;
            }
        }
    }
    Ok(warmed)
}

/// Continuous consolidation (RFC-living-notebook phase 5, WikiSkill's
/// argument made real): notebooks that HAVE an index note — creating one
/// is the opt-in — get their whole wiki (index + entity pages) re-derived
/// on every gist sweep, so the map tracks the shelf without anyone
/// pressing Refresh. Deterministic and write-skipping: an unchanged wiki
/// costs reads, never a commit.
pub async fn refresh_wiki_indexes(db: &crate::db::Db) -> anyhow::Result<usize> {
    let Some(trace_dir) = crate::trace::dir() else {
        // No traces handle (tests, early boot): a refresh without citation
        // data would strip the dust markers and churn every body — skip.
        return Ok(0);
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let cited = cited_ids(trace_dir);
    let cards = db.list_registry().await?;
    let mut refreshed = 0usize;
    for nb in db.list_notebooks().await? {
        let (index, changed) = upsert_wiki(db, &nb.id, &cards, &cited, now_ms, false).await?;
        refreshed += changed;
        // Autonomous work announces itself: one Staff-feed event per
        // notebook whose wiki actually moved (the Brief reads these too).
        if changed > 0 {
            if let Some(index) = index {
                let _ = db
                    .add_source_event(&SourceEvent {
                        id: uuid::Uuid::new_v4().to_string(),
                        notebook_id: nb.id.clone(),
                        source_id: index.id,
                        source_title: WIKI_INDEX_TITLE.into(),
                        kind: "wiki".into(),
                        detail: format!(
                            "wiki refreshed · {changed} note{}",
                            if changed == 1 { "" } else { "s" }
                        ),
                        diff: String::new(),
                        at: now_ms,
                    })
                    .await;
            }
        }
    }
    Ok(refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_cache_ttl_paces_budget_across_notebooks() {
        let queries = vec!["a".to_string(), "b".to_string()];
        // 2026-08-16T00:00Z — 16 days left in August.
        let now = 1_786_838_400_000i64;

        // One notebook, full budget: refresh at the 1-day floor.
        assert_eq!(web_cache_ttl_ms(&queries, 0, 1, now), WEB_CACHE_TTL_MIN_MS);

        // More notebooks split the same budget: TTL scales linearly once
        // above the floor (16d × 4cr × 40nb / 800cr = 3.2 days).
        let ttl = web_cache_ttl_ms(&queries, 0, 40, now);
        assert!(
            ttl > 3 * 86_400_000 && ttl < 4 * 86_400_000,
            "40 notebooks should pace near 3.2 days, got {ttl}"
        );

        // A nearly spent budget stretches toward the ceiling.
        let ttl = web_cache_ttl_ms(&queries, WEB_CREDIT_CAP - 8, 40, now);
        assert!(ttl > 20 * 86_400_000, "thin budget should slow way down");
        // A budget that can't fund one refresh parks at the ceiling.
        assert_eq!(
            web_cache_ttl_ms(&queries, WEB_CREDIT_CAP, 1, now),
            WEB_CACHE_TTL_MAX_MS
        );
    }

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
    fn markdown_links_are_never_split_mid_token() {
        // A self-linking markdown URL — `[http://x](http://x)` — is how the
        // splice got into the Grow list: the scan ran through the `](` and
        // proposed "hub:8450](http://hub:8450" as a page to add.
        let links = extract_links("Dashboard: [http://hub:8450](http://hub:8450) is up.");
        assert_eq!(
            links.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(),
            vec!["http://hub:8450", "http://hub:8450"]
        );
        // The anchor still reads off the "](" shape.
        assert_eq!(links[1].1, "http://hub:8450");

        // Ordinary markdown links keep their anchor, and every delimiter
        // that can wrap a URL ends it.
        let links = extract_links(
            "[Async book](https://rust-lang.github.io/async-book), \
             `https://a.test/tick`, <https://b.test/angle>, |https://c.test/pipe|",
        );
        assert_eq!(links[0].0, "https://rust-lang.github.io/async-book");
        assert_eq!(links[0].1, "Async book");
        assert_eq!(links[1].0, "https://a.test/tick");
        assert_eq!(links[2].0, "https://b.test/angle");
        assert_eq!(links[3].0, "https://c.test/pipe");
        assert!(links
            .iter()
            .all(|(u, _)| !u.contains(['[', ']', '(', '`', '|', '<', '>'])));
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
