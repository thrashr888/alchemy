//! Feed sources (docs/RFC-events.md §2): RSS, Atom, and JSON Feed as living
//! sources, plus the feed-shaped hosts (GitHub releases, Wikipedia page
//! history, arXiv queries, Substack, Reddit, Medium).
//!
//! A feed is a folder source whose root is a URL: the parent row carries
//! `source_type: "feed"` and a **rolling index** of its kept entries as its
//! text; each entry is an ordinary `url` child with `parent_id` set and
//! `mtime` = the entry's published time. Retrieval, gallery, tags, and
//! hygiene work unchanged because children are just sources.
//!
//! A **sitemap watch** is the same parent with different arrival rules: a
//! pasted `sitemap.xml` snapshots every URL it lists as already seen and
//! ingests nothing — only pages that appear later arrive, each fetched
//! through the normal page path. Watching a site is not copying it.
//!
//! Cost rules (RFC "Cost rules"): a poll is one conditional GET; a 304 costs
//! nothing downstream. New entries ingest through the normal chunk/embed
//! path, capped per feed and per pass. A poll writes at most one `added`
//! event per feed. Nothing here calls a model.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{self, AppState};
use crate::models::{Source, SourceEvent};

/// New entries ingested per feed per pass; the rest wait for the next one.
const MAX_NEW_PER_FEED: usize = 5;
/// New entries ingested across every feed per pass.
const MAX_NEW_PER_PASS: usize = 20;
/// Summary-only entries whose pages get fetched per pass — a page fetch is
/// the expensive end of a poll; the rest keep the feed's summary as text.
const MAX_PAGE_FETCHES_PER_PASS: usize = 3;
/// Entries the rolling index lists (older children stay; the retirement
/// pass in RFC-living-notebook proposes them for archive).
pub const KEEP: usize = 50;
/// Poll cadence bounds. Derived per feed from its own timestamps.
const MIN_CADENCE_MS: i64 = 30 * 60 * 1000;
const MAX_CADENCE_MS: i64 = 24 * 60 * 60 * 1000;
/// Strikes before a feed is reported unreachable (mirrors hygiene).
const UNREACHABLE_AFTER: i64 = 3;
/// Bytes read when probing a well-known path — enough to see `<rss`/`<feed`.
const PROBE_BYTES: usize = 4 * 1024;
/// Entry text handed to the child source, capped.
const ENTRY_TEXT_CAP: usize = 40_000;

const WELL_KNOWN: [&str; 8] = [
    "/feed",
    "/rss",
    "/rss.xml",
    "/atom.xml",
    "/feed.xml",
    "/index.xml",
    "/feed.json",
    // Last, and only a watch: a site with no feed still lists its pages.
    "/sitemap.xml",
];

// ---- Sniffing and discovery (pure) ---------------------------------------

/// Does this response body read as a feed rather than a page? Checked
/// before readability ever sees it (ingest::extract_url).
pub fn looks_like_feed(content_type: &str, body: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    let head: String = body.trim_start().chars().take(2_000).collect();
    let lower = head.to_ascii_lowercase();
    if ct.contains("rss+xml") || ct.contains("atom+xml") || ct.contains("feed+json") {
        return true;
    }
    if lower.starts_with('{') {
        return lower.contains("\"version\"") && lower.contains("jsonfeed.org");
    }
    let xml_ish = lower.starts_with("<?xml")
        || lower.starts_with("<rss")
        || lower.starts_with("<feed")
        || lower.starts_with("<urlset")
        || lower.starts_with("<sitemapindex");
    xml_ish
        && (lower.contains("<rss")
            || lower.contains("<feed")
            || lower.contains("<rdf:rdf")
            || lower.contains("<urlset")
            || lower.contains("<sitemapindex"))
        && !lower.contains("<html")
}

/// Is this feed document a sitemap (RFC §2, the watch shape)?
fn is_sitemap(body: &str) -> bool {
    let head: String = body.trim_start().chars().take(2_000).collect();
    let lower = head.to_ascii_lowercase();
    lower.contains("<urlset") || lower.contains("<sitemapindex")
}

/// Tier 1: `<link rel="alternate" type="application/rss+xml|atom+xml|
/// feed+json">` in a page already in hand. Costs no fetch.
pub fn discover_in_html(html: &str, base: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut at = 0usize;
    while let Some(start) = lower[at..].find("<link") {
        let start = at + start;
        let Some(end_rel) = lower[start..].find('>') else {
            break;
        };
        let end = start + end_rel;
        let tag = &html[start..=end];
        at = end + 1;
        let tl = tag.to_ascii_lowercase();
        if !tl.contains("alternate") {
            continue;
        }
        let ty = attr(&tl, "type").unwrap_or_default();
        if !(ty.contains("rss+xml") || ty.contains("atom+xml") || ty.contains("feed+json")) {
            continue;
        }
        let Some(href) = attr(tag, "href") else {
            continue;
        };
        if let Some(abs) = resolve(base, href.trim()) {
            if !out.contains(&abs) {
                out.push(abs);
            }
        }
    }
    out
}

/// Value of `name="…"` (or `name='…'`) inside one tag; case-insensitive on
/// the attribute name because `tag` may be lowercased by the caller.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let i = from + i;
        let after = &lower[i + name.len()..];
        let trimmed = after.trim_start();
        if !trimmed.starts_with('=') {
            from = i + name.len();
            continue;
        }
        // Position of the value start in the original string.
        let eq = i + name.len() + (after.len() - trimmed.len());
        let rest = &tag[eq + 1..].trim_start();
        let quote = rest.chars().next()?;
        let value = if quote == '"' || quote == '\'' {
            rest[1..].split(quote).next()?
        } else {
            rest.split(|c: char| c.is_whitespace() || c == '>').next()?
        };
        return Some(value.to_string());
    }
    None
}

/// Resolve `href` against `base` (absolute, protocol-relative, root- and
/// path-relative). `None` for anything that is not http(s) once resolved.
fn resolve(base: &str, href: &str) -> Option<String> {
    let out = if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        let (scheme, rest) = base.split_once("://")?;
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        if let Some(root) = href.strip_prefix('/') {
            format!("{scheme}://{host}/{root}")
        } else {
            let dir = match path.rfind('/') {
                Some(i) => &path[..i],
                None => "",
            };
            if dir.is_empty() {
                format!("{scheme}://{host}/{href}")
            } else {
                format!("{scheme}://{host}/{dir}/{href}")
            }
        }
    };
    (out.starts_with("http://") || out.starts_with("https://")).then_some(out)
}

/// One feed the app can offer to follow (docs/RFC-events.md §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedCandidate {
    pub url: String,
    /// Short human label: "Releases", "Page history", "Feed", …
    pub label: String,
    /// "page" (advertised by the page), "host" (a known host's shape),
    /// "well-known" (found at a conventional path).
    pub tier: String,
}

fn candidate(url: impl Into<String>, label: &str, tier: &str) -> FeedCandidate {
    FeedCandidate {
        url: url.into(),
        label: label.to_string(),
        tier: tier.to_string(),
    }
}

/// Tier 3: hosts whose URL shape implies a feed. A table, not a code path.
/// arXiv is the deliberate exception: a paper has no feed and a category is
/// the whole field, so it offers a search feed built from the notebook's
/// standing queries (RFC "Decisions") — nothing when there are none.
pub fn host_rules(page_url: &str, standing_queries: &[String]) -> Vec<FeedCandidate> {
    let Some((_, rest)) = page_url.split_once("://") else {
        return Vec::new();
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.trim_start_matches("www.").to_ascii_lowercase();
    let path = path.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let mut out = Vec::new();
    match host.as_str() {
        "github.com" if parts.len() >= 2 => {
            let (owner, repo) = (parts[0], parts[1].trim_end_matches(".git"));
            out.push(candidate(
                format!("https://github.com/{owner}/{repo}/releases.atom"),
                "Releases",
                "host",
            ));
            out.push(candidate(
                format!("https://github.com/{owner}/{repo}/commits.atom"),
                "Commits",
                "host",
            ));
        }
        h if h.ends_with("wikipedia.org") && parts.len() >= 2 && parts[0] == "wiki" => {
            out.push(candidate(
                format!(
                    "https://{h}/w/index.php?title={}&action=history&feed=atom",
                    parts[1]
                ),
                "Page history",
                "host",
            ));
        }
        "youtube.com" | "m.youtube.com" if parts.len() >= 2 && parts[0] == "channel" => {
            out.push(candidate(
                format!(
                    "https://www.youtube.com/feeds/videos.xml?channel_id={}",
                    parts[1]
                ),
                "New videos",
                "host",
            ));
        }
        "arxiv.org" | "export.arxiv.org"
            if !parts.is_empty() && matches!(parts[0], "abs" | "pdf" | "html") =>
        {
            if let Some(url) = arxiv_query_feed(standing_queries) {
                out.push(candidate(
                    url,
                    "arXiv papers matching your open questions",
                    "host",
                ));
            }
        }
        h if h.ends_with(".substack.com") => {
            out.push(candidate(format!("https://{h}/feed"), "Posts", "host"));
        }
        "reddit.com" | "old.reddit.com" if parts.len() >= 2 && parts[0] == "r" => {
            out.push(candidate(
                format!("https://www.reddit.com/r/{}/.rss", parts[1]),
                "New posts",
                "host",
            ));
        }
        "medium.com" if !parts.is_empty() => {
            out.push(candidate(
                format!("https://medium.com/feed/{}", parts[0]),
                "Posts",
                "host",
            ));
        }
        _ => {}
    }
    out
}

/// The arXiv API query feed for a notebook's standing queries — at most
/// four, quoted, OR-ed, newest first. `None` without queries: a feed of
/// everything would be the noise the decision refused.
fn arxiv_query_feed(queries: &[String]) -> Option<String> {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    let terms: Vec<String> = queries
        .iter()
        .map(|q| q.trim().replace('"', ""))
        .filter(|q| q.len() >= 8)
        .take(4)
        .map(|q| format!("all:%22{}%22", utf8_percent_encode(&q, NON_ALPHANUMERIC)))
        .collect();
    if terms.is_empty() {
        return None;
    }
    Some(format!(
        "https://export.arxiv.org/api/query?search_query={}&sortBy=submittedDate&sortOrder=descending&max_results=25",
        terms.join("+OR+")
    ))
}

/// Tier 2: the conventional paths on the page's origin. Network — only from
/// the explicit "Follow updates…" path, never from a sweep.
pub async fn probe_well_known(page_url: &str) -> Vec<String> {
    let Some((scheme, rest)) = page_url.split_once("://") else {
        return Vec::new();
    };
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() {
        return Vec::new();
    }
    let Ok(client) = client() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for path in WELL_KNOWN {
        let url = format!("{scheme}://{host}{path}");
        let Ok(resp) = client.get(&url).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let Ok(bytes) = resp.bytes().await else {
            continue;
        };
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(PROBE_BYTES)]).into_owned();
        if looks_like_feed(&ct, &head) {
            out.push(url);
        }
    }
    out
}

// ---- Parsing (pure) --------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub link: String,
    pub published_ms: i64,
    /// Plain text: the entry's content when the feed carries it, else its
    /// summary. `full` says which.
    pub text: String,
    pub full: bool,
}

#[derive(Debug, Clone)]
pub struct Parsed {
    pub title: String,
    pub description: String,
    /// Newest first.
    pub entries: Vec<Entry>,
    /// A sitemap watch: entries are bare links, and nothing ingests at
    /// connect (see the module doc).
    pub sitemap: bool,
}

/// A sitemap's `<url><loc>…</loc><lastmod>…</lastmod></url>` blocks as
/// link-only entries, newest `lastmod` first (undated ones keep file order
/// after the dated ones). A sitemap *index* is refused with the list it
/// points at: the user picks one, the app never crawls a tree.
fn parse_sitemap(body: &str, base: &str) -> Result<Parsed> {
    let lower = body.to_ascii_lowercase();
    if lower.contains("<sitemapindex") {
        let children: Vec<String> = between_all(body, "<loc>", "</loc>")
            .into_iter()
            .take(6)
            .collect();
        anyhow::bail!(
            "{base} is a sitemap index — paste one of the sitemaps it lists instead: {}",
            children.join(", ")
        );
    }
    let host = base
        .split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or(base)
        .trim_start_matches("www.");
    let mut entries: Vec<Entry> = Vec::new();
    for block in body.split("<url>").skip(1) {
        let block = block.split("</url>").next().unwrap_or("");
        let Some(loc) = between(block, "<loc>", "</loc>") else {
            continue;
        };
        let Some(link) = resolve(base, loc.trim()) else {
            continue;
        };
        let published_ms = between(block, "<lastmod>", "</lastmod>")
            .and_then(|d| parse_lastmod(d.trim()))
            .unwrap_or(0);
        entries.push(Entry {
            id: link.clone(),
            title: link.clone(),
            link,
            published_ms,
            text: String::new(),
            full: false,
        });
    }
    let n = entries.len();
    // Stable: dated pages newest first, undated ones after in file order.
    entries.sort_by_key(|e| std::cmp::Reverse(e.published_ms));
    Ok(Parsed {
        title: format!("{host} \u{2014} new pages"),
        description: format!(
            "Watching the {n} pages listed at {base}. Pages added to the site arrive here as sources; nothing already listed is copied."
        ),
        entries,
        sitemap: true,
    })
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let i = s.find(open)? + open.len();
    let j = s[i..].find(close)? + i;
    Some(&s[i..j])
}

fn between_all(s: &str, open: &str, close: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(found) = between(rest, open, close) {
        out.push(found.trim().to_string());
        let skip = rest.find(open).unwrap_or(0) + open.len() + found.len() + close.len();
        rest = &rest[skip.min(rest.len())..];
    }
    out
}

/// `2026-09-01`, `2026-09-01T10:00:00Z`, or with an offset → epoch ms.
fn parse_lastmod(s: &str) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis());
    }
    let date = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis())
}

/// A URL as the compact token the seen-set keeps (docs/RFC-events.md §2):
/// trailing-slash-insensitive, so `/a` and `/a/` are one page.
pub fn url_hash(url: &str) -> u64 {
    crate::mac::content_stamp(url.trim_end_matches('/')) as u64
}

/// Parse RSS / Atom / JSON Feed — or a sitemap. Entries without a link are
/// dropped — a child source needs an origin to refresh from and to dedupe on.
pub fn parse(body: &str, base: &str) -> Result<Parsed> {
    if is_sitemap(body) {
        return parse_sitemap(body, base);
    }
    let feed = feed_rs::parser::parse(body.as_bytes()).context("could not parse feed")?;
    let text_of = |t: &Option<feed_rs::model::Text>| -> String {
        t.as_ref().map(|t| plain(&t.content)).unwrap_or_default()
    };
    let mut entries: Vec<Entry> = feed
        .entries
        .iter()
        .filter_map(|e| {
            let link = e
                .links
                .iter()
                .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate"))
                .or(e.links.first())
                .map(|l| l.href.clone())
                .and_then(|h| resolve(base, &h))?;
            let published = e
                .published
                .or(e.updated)
                .map(|d| d.timestamp_millis())
                .unwrap_or(0);
            let content = e
                .content
                .as_ref()
                .and_then(|c| c.body.as_ref())
                .map(|b| plain(b))
                .filter(|t| !t.trim().is_empty());
            let (text, full) = match content {
                Some(c) => (c, true),
                None => (text_of(&e.summary), false),
            };
            let title = text_of(&e.title);
            let title = if title.trim().is_empty() {
                link.clone()
            } else {
                title
            };
            Some(Entry {
                id: if e.id.is_empty() {
                    link.clone()
                } else {
                    e.id.clone()
                },
                title,
                link,
                published_ms: published,
                text: text.chars().take(ENTRY_TEXT_CAP).collect(),
                full,
            })
        })
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.published_ms));
    Ok(Parsed {
        title: text_of(&feed.title),
        description: text_of(&feed.description),
        entries,
        sitemap: false,
    })
}

/// HTML or text → plain text. Feeds carry HTML in content and often in
/// summaries; the same readability converter pages use keeps structure and
/// links as markdown.
fn plain(s: &str) -> String {
    let t = s.trim();
    if t.contains('<') && t.contains('>') {
        crate::ingest::html_fragment_to_text(t)
    } else {
        t.to_string()
    }
}

fn day(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "undated".to_string())
}

/// The child source's text: a rigid provenance block a card can parse
/// (RFC §7 — `title / published / link` then the body), same idea as the
/// byline header pages get.
pub fn entry_text(e: &Entry) -> String {
    let mut out = format!(
        "# {}\nPublished: {}\nLink: {}\n\n",
        e.title.trim(),
        day(e.published_ms),
        e.link
    );
    out.push_str(e.text.trim());
    out
}

/// The parent's rolling index: description plus one line per kept entry,
/// newest first — `- YYYY-MM-DD — [Title](link)`, the line the feed card
/// parses (src/lib/liveCards.ts). "What's new in the Tauri blog" retrieves
/// this; a specific claim retrieves the child.
pub fn index_text(title: &str, description: &str, entries: &[(i64, String, String)]) -> String {
    let mut out = format!("# {}\n", title.trim());
    if !description.trim().is_empty() {
        out.push_str(description.trim());
        out.push('\n');
    }
    out.push_str(&format!(
        "\nFeed of {} {}, newest first:\n\n",
        entries.len(),
        if entries.len() == 1 {
            "entry"
        } else {
            "entries"
        }
    ));
    for (ms, t, link) in entries.iter().take(KEEP) {
        out.push_str(&format!(
            "- {} \u{2014} [{}]({})\n",
            day(*ms),
            t.trim(),
            link
        ));
    }
    out
}

/// Poll cadence from the feed's own rhythm: the median gap between entry
/// timestamps, clamped to [30 min, 24 h]. Feeds without usable dates poll
/// at the ceiling.
pub fn cadence_ms(published: &[i64]) -> i64 {
    let mut ts: Vec<i64> = published.iter().copied().filter(|t| *t > 0).collect();
    ts.sort_unstable();
    ts.dedup();
    if ts.len() < 2 {
        return MAX_CADENCE_MS;
    }
    let mut gaps: Vec<i64> = ts.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_unstable();
    let median = gaps[gaps.len() / 2];
    median.clamp(MIN_CADENCE_MS, MAX_CADENCE_MS)
}

// ---- Persistent poll state (one JSON sidecar, like git_hosts.json) ---------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FeedState {
    #[serde(default)]
    etag: String,
    #[serde(default)]
    last_modified: String,
    #[serde(default)]
    next_poll_at: i64,
    #[serde(default)]
    failures: i64,
    #[serde(default)]
    cadence_ms: i64,
    /// Sitemap watches only: `url_hash` of every page already seen, so a
    /// connect ingests nothing and a poll ingests only what appeared.
    #[serde(default)]
    seen: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Discovered {
    source_id: String,
    source_title: String,
    seen_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    /// Poll state by feed parent source id.
    #[serde(default)]
    state: HashMap<String, FeedState>,
    /// Tier-1 discoveries by notebook id, then feed url.
    #[serde(default)]
    discovered: HashMap<String, HashMap<String, Discovered>>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("feeds.json")
}

fn with_store<T>(data_dir: &Path, f: impl FnOnce(&mut Store) -> T, save: bool) -> T {
    let mut guard = STORE.lock().unwrap_or_else(|p| p.into_inner());
    let store = guard.get_or_insert_with(|| {
        std::fs::read_to_string(store_path(data_dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    });
    let out = f(store);
    if save {
        if let Ok(json) = serde_json::to_string_pretty(&*store) {
            let _ = std::fs::write(store_path(data_dir), json);
        }
    }
    out
}

/// Remember feeds a just-imported page advertised (tier 1), so the Grow
/// pane can propose them without a second fetch.
pub fn remember_discovered(state: &AppState, notebook_id: &str, source: &Source, feeds: &[String]) {
    if feeds.is_empty() {
        return;
    }
    let dir = commands::app_data_dir(state);
    let now = commands::now();
    with_store(
        &dir,
        |s| {
            let nb = s.discovered.entry(notebook_id.to_string()).or_default();
            for url in feeds {
                nb.insert(
                    url.clone(),
                    Discovered {
                        source_id: source.id.clone(),
                        source_title: source.title.clone(),
                        seen_at: now,
                    },
                );
            }
        },
        true,
    );
}

/// Discovered feeds as growth proposals of kind `feed`, minus the ones the
/// notebook already follows and the ones whose page is gone.
pub fn discovered_proposals(
    state: &AppState,
    notebook_id: &str,
    sources: &[Source],
) -> Vec<crate::growth::GrowthProposal> {
    let dir = commands::app_data_dir(state);
    let followed: std::collections::HashSet<&str> = sources
        .iter()
        .filter(|s| s.source_type == "feed")
        .map(|s| s.url.trim_end_matches('/'))
        .collect();
    let live: std::collections::HashSet<&str> = sources.iter().map(|s| s.id.as_str()).collect();
    let found = with_store(
        &dir,
        |s| s.discovered.get(notebook_id).cloned().unwrap_or_default(),
        false,
    );
    let mut out: Vec<crate::growth::GrowthProposal> = found
        .into_iter()
        .filter(|(url, d)| {
            !followed.contains(url.trim_end_matches('/')) && live.contains(d.source_id.as_str())
        })
        .map(|(url, d)| crate::growth::GrowthProposal {
            kind: "feed".into(),
            url,
            anchor: format!("Follow {}", d.source_title),
            mentions: 0,
            source_count: 1,
            matched_query: String::new(),
            score: 0.5,
        })
        .collect();
    out.sort_by(|a, b| a.anchor.cmp(&b.anchor));
    out
}

/// Everything the app can offer to follow for one source: what its page
/// advertised, what its host's shape implies, and what sits at the
/// conventional paths (the one tier that fetches, so it runs last and only
/// here). Deduped, page tier first.
pub async fn discover_for_source(state: &AppState, source: &Source) -> Vec<FeedCandidate> {
    let dir = commands::app_data_dir(state);
    let mut out: Vec<FeedCandidate> = Vec::new();
    let advertised: Vec<String> = with_store(
        &dir,
        |s| {
            s.discovered
                .get(&source.notebook_id)
                .map(|m| {
                    m.iter()
                        .filter(|(_, d)| d.source_id == source.id)
                        .map(|(u, _)| u.clone())
                        .collect()
                })
                .unwrap_or_default()
        },
        false,
    );
    for url in advertised {
        out.push(candidate(url, "Feed", "page"));
    }
    let queries =
        crate::growth::standing_queries(&state.trace_dir, &source.notebook_id, commands::now());
    for c in host_rules(&source.url, &queries) {
        if !out.iter().any(|o| o.url == c.url) {
            out.push(c);
        }
    }
    if out.is_empty() {
        for url in probe_well_known(&source.url).await {
            if !out.iter().any(|o| o.url == url) {
                let label = if url.ends_with("/sitemap.xml") {
                    "New pages (sitemap watch)"
                } else {
                    "Feed"
                };
                out.push(candidate(url, label, "well-known"));
            }
        }
    }
    out
}

// ---- Fetching --------------------------------------------------------------

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("Alchemy/0.5 (+https://thrashr888.github.io/alchemy/; feed reader)")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")
}

enum Fetched {
    NotModified,
    Body {
        body: String,
        etag: String,
        last_modified: String,
    },
}

async fn fetch(url: &str, st: &FeedState) -> Result<Fetched> {
    let client = client()?;
    let mut req = client.get(url).header(
        reqwest::header::ACCEPT,
        "application/rss+xml, application/atom+xml, application/feed+json, application/xml;q=0.9, */*;q=0.5",
    );
    if !st.etag.is_empty() {
        req = req.header(reqwest::header::IF_NONE_MATCH, st.etag.clone());
    }
    if !st.last_modified.is_empty() {
        req = req.header(reqwest::header::IF_MODIFIED_SINCE, st.last_modified.clone());
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if !resp.status().is_success() {
        anyhow::bail!("{url} returned HTTP {}", resp.status().as_u16());
    }
    let header = |name: reqwest::header::HeaderName| -> String {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let etag = header(reqwest::header::ETAG);
    let last_modified = header(reqwest::header::LAST_MODIFIED);
    let body = resp.text().await.context("could not read feed body")?;
    Ok(Fetched::Body {
        body,
        etag,
        last_modified,
    })
}

// ---- Connect and poll ------------------------------------------------------

fn kept_entries(children: &[&Source]) -> Vec<(i64, String, String)> {
    let mut kept: Vec<(i64, String, String)> = children
        .iter()
        .map(|c| (c.mtime, c.title.clone(), c.url.clone()))
        .collect();
    kept.sort_by_key(|(ms, _, _)| std::cmp::Reverse(*ms));
    kept.truncate(KEEP);
    kept
}

/// Rewrite the parent's rolling index from its current children. The
/// parent is folder-like, so `reingest` writes no `updated` event for it —
/// the poll's own `added` event is the arrival.
async fn rewrite_index(
    state: &AppState,
    parent: &Source,
    title: &str,
    description: &str,
) -> Result<Source> {
    let all = state.db.list_sources(&parent.notebook_id).await?;
    let children: Vec<&Source> = all.iter().filter(|s| s.parent_id == parent.id).collect();
    let text = index_text(title, description, &kept_entries(&children));
    let extracted = crate::ingest::Extracted {
        title: title.to_string(),
        source_type: "feed".to_string(),
        url: parent.url.clone(),
        text,
        author: String::new(),
        image_url: String::new(),
        feeds: Vec::new(),
    };
    let fresh = Source {
        status: "ready".to_string(),
        error: String::new(),
        ..parent.clone()
    };
    commands::reingest(state, &fresh, extracted, None, true).await
}

/// Ingest new entries as children. Returns the titles that landed.
/// `page_budget` is shared across a pass: summary-only entries fetch their
/// page while it lasts, then keep the summary.
async fn ingest_entries(
    state: &AppState,
    parent: &Source,
    entries: &[Entry],
    page_budget: &mut usize,
) -> Vec<(String, String)> {
    let mut landed = Vec::new();
    for e in entries {
        let mut text = entry_text(e);
        let mut title = e.title.clone();
        // A sitemap entry is a bare link: without the page there is nothing
        // to store, so it waits for a pass with budget (or a page that
        // answers) rather than landing empty.
        let link_only = !e.full && e.text.trim().is_empty();
        if link_only && *page_budget == 0 {
            continue;
        }
        let mut fetched = false;
        if !e.full && *page_budget > 0 {
            *page_budget -= 1;
            // The same path Add Source takes: the fast fetch, then the
            // rendered capture when a page comes back as a JS shell.
            if let Ok(page) = crate::capture::extract_url_rescued(&e.link).await {
                if page.source_type != "feed" && page.text.chars().count() > e.text.chars().count()
                {
                    fetched = true;
                    if link_only && !page.title.trim().is_empty() {
                        title = page.title.trim().to_string();
                    }
                    text = format!(
                        "# {}\nPublished: {}\nLink: {}\n\n{}",
                        title.trim(),
                        day(e.published_ms),
                        e.link,
                        page.text.trim()
                    );
                }
            }
        }
        if link_only && !fetched {
            continue;
        }
        let extracted = crate::ingest::Extracted {
            title: title.clone(),
            source_type: "url".to_string(),
            url: e.link.clone(),
            text,
            author: String::new(),
            image_url: String::new(),
            feeds: Vec::new(),
        };
        match commands::store_new_source(
            state,
            &parent.notebook_id,
            extracted,
            &parent.id,
            e.published_ms,
            None,
            true,
        )
        .await
        {
            Ok(_) => landed.push((title, e.link.clone())),
            Err(err) => crate::note!("feed {}: entry {} failed: {err:#}", parent.title, e.link),
        }
    }
    landed
}

/// A feed URL pasted (or discovered and accepted) becomes a parent plus its
/// newest entries. `body` is the feed document `extract_url` already
/// fetched — one GET, not two. No event: the user asked for this.
pub async fn connect(state: &AppState, notebook_id: &str, url: &str, body: &str) -> Result<Source> {
    let parsed = parse(body, url)?;
    if parsed.entries.is_empty() {
        anyhow::bail!("{url} is a feed with no entries");
    }
    let title = if parsed.title.trim().is_empty() {
        url.to_string()
    } else {
        parsed.title.clone()
    };
    let cadence = cadence_ms(
        &parsed
            .entries
            .iter()
            .map(|e| e.published_ms)
            .collect::<Vec<_>>(),
    );
    let stamp = crate::mac::content_stamp(
        &parsed
            .entries
            .first()
            .map(|e| e.id.clone())
            .unwrap_or_default(),
    );
    let extracted = crate::ingest::Extracted {
        title: title.clone(),
        source_type: "feed".to_string(),
        url: url.to_string(),
        text: index_text(&title, &parsed.description, &[]),
        author: String::new(),
        image_url: String::new(),
        feeds: Vec::new(),
    };
    let parent =
        commands::store_new_source(state, notebook_id, extracted, "", stamp, None, false).await?;
    // A feed connects with its newest entries; a sitemap watch connects
    // with none — every page it lists today is marked seen, and only pages
    // that appear from here on arrive.
    let seen: Vec<u64> = if parsed.sitemap {
        parsed.entries.iter().map(|e| url_hash(&e.link)).collect()
    } else {
        let mut page_budget = MAX_PAGE_FETCHES_PER_PASS;
        let first: Vec<Entry> = parsed
            .entries
            .iter()
            .take(MAX_NEW_PER_FEED * 2)
            .cloned()
            .collect();
        ingest_entries(state, &parent, &first, &mut page_budget).await;
        Vec::new()
    };
    let parent = rewrite_index(state, &parent, &title, &parsed.description).await?;
    let dir = commands::app_data_dir(state);
    with_store(
        &dir,
        |s| {
            s.state.insert(
                parent.id.clone(),
                FeedState {
                    etag: String::new(),
                    last_modified: String::new(),
                    next_poll_at: commands::now() + cadence,
                    failures: 0,
                    cadence_ms: cadence,
                    seen,
                },
            );
        },
        true,
    );
    Ok(Source {
        content: String::new(),
        ..parent
    })
}

/// One poll: conditional GET, new entries in, index rewritten, one `added`
/// event. `force` ignores the cadence (manual Refresh). Returns how many
/// entries landed. Errors count a strike and back the feed off.
pub async fn poll_one(
    state: &AppState,
    parent: &Source,
    force: bool,
    page_budget: &mut usize,
) -> Result<usize> {
    let dir = commands::app_data_dir(state);
    let now = commands::now();
    let st = with_store(
        &dir,
        |s| s.state.get(&parent.id).cloned().unwrap_or_default(),
        false,
    );
    if !force && now < st.next_poll_at {
        return Ok(0);
    }
    let cadence = if st.cadence_ms > 0 {
        st.cadence_ms
    } else {
        MAX_CADENCE_MS
    };
    let outcome = async {
        // A forced poll wants the document, not a 304: send no validators,
        // so the index rewrite below always has a body to work from.
        let conditional = if force {
            FeedState::default()
        } else {
            st.clone()
        };
        let fetched = fetch(&parent.url, &conditional).await?;
        let Fetched::Body {
            body,
            etag,
            last_modified,
        } = fetched
        else {
            return Ok::<(usize, FeedState), anyhow::Error>((
                0,
                FeedState {
                    next_poll_at: now + cadence,
                    failures: 0,
                    ..st.clone()
                },
            ));
        };
        let parsed = parse(&body, &parent.url)?;
        // Anything the notebook already holds — under this feed or added
        // by hand — is not new; a sitemap watch also remembers what it
        // saw at connect, since those pages were never ingested.
        let all = state.db.list_sources(&parent.notebook_id).await?;
        let known: std::collections::HashSet<&str> = all
            .iter()
            .filter(|s| !s.url.is_empty())
            .map(|s| s.url.trim_end_matches('/'))
            .collect();
        let seen: std::collections::HashSet<u64> = st.seen.iter().copied().collect();
        let cap = if parsed.sitemap {
            MAX_NEW_PER_FEED.min(*page_budget)
        } else {
            MAX_NEW_PER_FEED
        };
        let fresh: Vec<Entry> = parsed
            .entries
            .iter()
            .filter(|e| {
                !known.contains(e.link.trim_end_matches('/')) && !seen.contains(&url_hash(&e.link))
            })
            .take(cap)
            .cloned()
            .collect();
        let landed = ingest_entries(state, parent, &fresh, page_budget).await;
        let title = if parsed.title.trim().is_empty() {
            parent.title.clone()
        } else {
            parsed.title.clone()
        };
        // A forced poll (manual Refresh) always rewrites the index, so a
        // renamed feed or a changed index format reaches parents that
        // predate it; the sweep rewrites only when something landed.
        if !landed.is_empty() || force {
            rewrite_index(state, parent, &title, &parsed.description).await?;
        }
        if !landed.is_empty() {
            let detail = match landed.len() {
                1 => format!("new entry \u{00b7} {}", landed[0].0),
                n => format!("{n} new entries"),
            };
            let _ = state
                .db
                .add_source_event(&SourceEvent {
                    id: commands::new_id(),
                    notebook_id: parent.notebook_id.clone(),
                    source_id: parent.id.clone(),
                    source_title: title.clone(),
                    kind: "added".into(),
                    detail,
                    diff: landed
                        .iter()
                        .map(|(t, _)| format!("+ {t}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    at: commands::now(),
                })
                .await;
        }
        let cadence = cadence_ms(
            &parsed
                .entries
                .iter()
                .map(|e| e.published_ms)
                .collect::<Vec<_>>(),
        );
        // The watch remembers what landed; what it could not fetch this
        // pass stays unseen and comes round again.
        let mut seen_next = st.seen.clone();
        if parsed.sitemap {
            for (_, link) in &landed {
                let h = url_hash(link);
                if !seen_next.contains(&h) {
                    seen_next.push(h);
                }
            }
        }
        Ok((
            landed.len(),
            FeedState {
                etag,
                last_modified,
                next_poll_at: now + cadence,
                failures: 0,
                cadence_ms: cadence,
                seen: seen_next,
            },
        ))
    }
    .await;
    match outcome {
        Ok((n, next)) => {
            with_store(&dir, |s| s.state.insert(parent.id.clone(), next), true);
            Ok(n)
        }
        Err(err) => {
            let failures = st.failures + 1;
            let backoff = (cadence << failures.min(5)).min(MAX_CADENCE_MS);
            with_store(
                &dir,
                |s| {
                    s.state.insert(
                        parent.id.clone(),
                        FeedState {
                            next_poll_at: now + backoff,
                            failures,
                            ..st.clone()
                        },
                    )
                },
                true,
            );
            if failures == UNREACHABLE_AFTER {
                let _ = state
                    .db
                    .add_source_event(&SourceEvent {
                        id: commands::new_id(),
                        notebook_id: parent.notebook_id.clone(),
                        source_id: parent.id.clone(),
                        source_title: parent.title.clone(),
                        kind: "unreachable".into(),
                        detail: format!("{failures} polls failed"),
                        diff: format!("{err:#}").chars().take(200).collect(),
                        at: commands::now(),
                    })
                    .await;
            }
            Err(err)
        }
    }
}

static POLLING: AtomicBool = AtomicBool::new(false);

/// The scheduler's feed pass: every due feed, single-flight, under the
/// per-pass caps. Announces changed notebooks with `sources://changed` the
/// way the folder sweep does. Not budgeted against the nightly ceiling —
/// polling is not model work.
pub fn spawn_poll(app: &AppHandle) {
    if POLLING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = run_poll(&app).await {
            crate::diagnostics::error("feeds", format!("poll pass failed: {err:#}"));
        }
        POLLING.store(false, Ordering::SeqCst);
    });
}

async fn run_poll(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let archived = state.db.archived_notebook_ids().await.unwrap_or_default();
    let feeds = state.db.all_feed_sources().await?;
    let mut page_budget = MAX_PAGE_FETCHES_PER_PASS;
    let mut landed_total = 0usize;
    let mut changed: HashMap<String, usize> = HashMap::new();
    for feed in &feeds {
        if archived.contains(&feed.notebook_id) || landed_total >= MAX_NEW_PER_PASS {
            continue;
        }
        match poll_one(&state, feed, false, &mut page_budget).await {
            Ok(0) => {}
            Ok(n) => {
                landed_total += n;
                *changed.entry(feed.notebook_id.clone()).or_default() += n;
            }
            Err(err) => crate::note!("feed poll: \u{201c}{}\u{201d} failed: {err:#}", feed.title),
        }
    }
    for (notebook_id, added) in changed {
        let _ = app.emit(
            "sources://changed",
            serde_json::json!({
                "notebookId": notebook_id,
                "scan": { "added": added, "updated": 0, "removed": 0, "failed": 0 },
            }),
        );
    }
    Ok(())
}

/// Manual Refresh on a feed parent (docs/RFC-events.md §2): poll now,
/// ignoring the cadence, with a fresh page budget.
pub async fn refresh(state: &AppState, parent: &Source) -> Result<Source> {
    let mut page_budget = MAX_PAGE_FETCHES_PER_PASS;
    poll_one(state, parent, true, &mut page_budget).await?;
    let fresh = state
        .db
        .get_source(&parent.id)
        .await?
        .ok_or_else(|| anyhow!("Source not found"))?;
    Ok(Source {
        content: String::new(),
        ..fresh
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Tauri Blog</title><description>News</description>
<item><title>Tauri 2.5</title><link>https://v2.tauri.app/blog/tauri-25/</link><guid>a</guid><pubDate>Mon, 01 Sep 2026 10:00:00 GMT</pubDate><description>&lt;p&gt;Short &lt;b&gt;summary&lt;/b&gt;.&lt;/p&gt;</description></item>
<item><title>Tauri 2.4</title><link>/blog/tauri-24/</link><guid>b</guid><pubDate>Mon, 25 Aug 2026 10:00:00 GMT</pubDate><content:encoded xmlns:content="http://purl.org/rss/1.0/modules/content/">&lt;p&gt;Full body here.&lt;/p&gt;</content:encoded></item>
<item><title>No link</title><guid>c</guid></item>
</channel></rss>"#;

    #[test]
    fn sniffs_feeds_but_not_pages() {
        assert!(looks_like_feed("application/rss+xml", ""));
        assert!(looks_like_feed("text/xml", RSS));
        assert!(looks_like_feed(
            "application/xml",
            "<?xml version=\"1.0\"?><feed xmlns=\"http://www.w3.org/2005/Atom\"><title>x</title></feed>"
        ));
        assert!(looks_like_feed(
            "application/json",
            r#"{"version":"https://jsonfeed.org/version/1.1","title":"x","items":[]}"#
        ));
        assert!(!looks_like_feed(
            "text/html",
            "<!doctype html><html><head><title>Hi</title></head></html>"
        ));
        assert!(!looks_like_feed(
            "text/html",
            "<?xml version=\"1.0\"?><html xmlns=\"…\"><rss-like/></html>"
        ));
        assert!(!looks_like_feed(
            "application/json",
            r#"{"version": 2, "data": []}"#
        ));
    }

    #[test]
    fn discovers_alternate_links_and_resolves_them() {
        let html = r#"<html><head>
<link rel="stylesheet" href="/a.css">
<link rel="alternate" type="application/rss+xml" title="RSS" href="/blog/rss.xml">
<LINK REL="alternate" TYPE="application/atom+xml" HREF="https://other.example/atom">
<link rel="alternate" type="application/feed+json" href='feed.json'>
<link rel="alternate" type="text/html" hreflang="fr" href="/fr">
</head></html>"#;
        assert_eq!(
            discover_in_html(html, "https://v2.tauri.app/blog/post/"),
            vec![
                "https://v2.tauri.app/blog/rss.xml",
                "https://other.example/atom",
                "https://v2.tauri.app/blog/post/feed.json",
            ]
        );
        assert!(discover_in_html("<html><body>no links</body></html>", "https://x.y").is_empty());
    }

    #[test]
    fn host_rules_follow_the_table() {
        let q: Vec<String> = Vec::new();
        let gh = host_rules("https://github.com/lancedb/lancedb/tree/main/rust", &q);
        assert_eq!(
            gh.iter().map(|c| c.url.as_str()).collect::<Vec<_>>(),
            vec![
                "https://github.com/lancedb/lancedb/releases.atom",
                "https://github.com/lancedb/lancedb/commits.atom"
            ]
        );
        let wp = host_rules(
            "https://en.wikipedia.org/wiki/California_FAIR_Plan#History",
            &q,
        );
        assert_eq!(
            wp[0].url,
            "https://en.wikipedia.org/w/index.php?title=California_FAIR_Plan&action=history&feed=atom"
        );
        assert_eq!(
            host_rules("https://www.reddit.com/r/pottery/", &q)[0].url,
            "https://www.reddit.com/r/pottery/.rss"
        );
        assert_eq!(
            host_rules("https://alice.substack.com/p/hello", &q)[0].url,
            "https://alice.substack.com/feed"
        );
        assert!(host_rules("https://example.com/article", &q).is_empty());
        // arXiv: a query feed from standing questions, never a category.
        assert!(host_rules("https://arxiv.org/abs/2307.03172", &q).is_empty());
        let qs = vec![
            "retrieval for long documents".to_string(),
            "short".to_string(),
        ];
        let ax = host_rules("https://arxiv.org/pdf/2307.03172", &qs);
        assert_eq!(ax.len(), 1);
        assert!(ax[0]
            .url
            .starts_with("https://export.arxiv.org/api/query?search_query=all:%22retrieval"));
        assert!(
            !ax[0].url.contains("short"),
            "queries under 8 chars are noise"
        );
    }

    #[test]
    fn parses_rss_with_relative_links_and_html() {
        let p = parse(RSS, "https://v2.tauri.app/feed.xml").expect("parses");
        assert_eq!(p.title, "Tauri Blog");
        assert_eq!(p.entries.len(), 2, "the linkless item is dropped");
        assert_eq!(p.entries[0].title, "Tauri 2.5");
        assert_eq!(p.entries[0].link, "https://v2.tauri.app/blog/tauri-25/");
        assert!(!p.entries[0].full);
        assert!(p.entries[0].text.contains("Short"), "{}", p.entries[0].text);
        assert!(!p.entries[0].text.contains("<b>"), "html is flattened");
        assert_eq!(p.entries[1].link, "https://v2.tauri.app/blog/tauri-24/");
        assert!(p.entries[1].full);
        assert!(
            p.entries[1].published_ms < p.entries[0].published_ms,
            "newest first"
        );
    }

    #[test]
    fn entry_and_index_texts_are_rigid() {
        let e = Entry {
            id: "a".into(),
            title: " Tauri 2.5 ".into(),
            link: "https://v2.tauri.app/blog/tauri-25/".into(),
            published_ms: 1_788_256_800_000,
            text: "Body.".into(),
            full: true,
        };
        let t = entry_text(&e);
        assert!(t.starts_with("# Tauri 2.5\nPublished: 2026-09-01\nLink: https://v2.tauri.app/blog/tauri-25/\n\nBody."));
        let idx = index_text(
            "Tauri Blog",
            "News",
            &[(e.published_ms, e.title.clone(), e.link.clone())],
        );
        assert!(idx.contains("Feed of 1 entry, newest first:"));
        assert!(
            idx.contains("- 2026-09-01 \u{2014} [Tauri 2.5](https://v2.tauri.app/blog/tauri-25/)")
        );
    }

    const SITEMAP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
<url><loc>https://www.curated.supply/</loc></url>
<url><loc>https://www.curated.supply/products/imac</loc><lastmod>2026-08-30</lastmod></url>
<url><loc>https://www.curated.supply/products/iphone-pro-17</loc><lastmod>2026-09-01T10:00:00Z</lastmod></url>
<url><loc>/products/relative</loc></url>
</urlset>"#;

    #[test]
    fn sitemaps_are_feeds_of_bare_links_newest_first() {
        assert!(looks_like_feed("text/xml", SITEMAP));
        assert!(is_sitemap(SITEMAP));
        assert!(!is_sitemap(RSS));
        let p = parse(SITEMAP, "https://www.curated.supply/sitemap.xml").expect("parses");
        assert!(p.sitemap);
        assert_eq!(p.title, "curated.supply \u{2014} new pages");
        assert!(p.description.contains("Watching the 4 pages"));
        let links: Vec<&str> = p.entries.iter().map(|e| e.link.as_str()).collect();
        assert_eq!(
            links,
            vec![
                "https://www.curated.supply/products/iphone-pro-17",
                "https://www.curated.supply/products/imac",
                "https://www.curated.supply/",
                "https://www.curated.supply/products/relative",
            ],
            "dated newest first, undated after in file order, relative resolved"
        );
        assert!(
            p.entries.iter().all(|e| !e.full && e.text.is_empty()),
            "bare links"
        );
        assert_eq!(
            p.entries[0].title, p.entries[0].link,
            "titled by the page once fetched"
        );
        assert_eq!(url_hash("https://a.b/x/"), url_hash("https://a.b/x"));
        assert_ne!(url_hash("https://a.b/x"), url_hash("https://a.b/y"));
    }

    #[test]
    fn sitemap_indexes_are_refused_with_their_children() {
        let index = r#"<?xml version="1.0"?><sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
<sitemap><loc>https://x.y/sitemap-products.xml</loc></sitemap>
<sitemap><loc>https://x.y/sitemap-pages.xml</loc></sitemap></sitemapindex>"#;
        let err = parse(index, "https://x.y/sitemap.xml")
            .unwrap_err()
            .to_string();
        assert!(err.contains("sitemap index"), "{err}");
        assert!(err.contains("sitemap-products.xml") && err.contains("sitemap-pages.xml"));
    }

    #[test]
    fn cadence_is_the_median_gap_clamped() {
        let h = 3_600_000;
        assert_eq!(cadence_ms(&[]), MAX_CADENCE_MS, "undated feeds poll daily");
        assert_eq!(cadence_ms(&[10 * h]), MAX_CADENCE_MS);
        assert_eq!(cadence_ms(&[0, 2 * h, 4 * h, 6 * h]), 2 * h);
        assert_eq!(
            cadence_ms(&[0, 60_000, 120_000]),
            MIN_CADENCE_MS,
            "never faster than 30 minutes"
        );
        assert_eq!(
            cadence_ms(&[0, 30 * 24 * h]),
            MAX_CADENCE_MS,
            "never slower than a day"
        );
        // Ordering does not matter; duplicates do not count as gaps.
        assert_eq!(cadence_ms(&[6 * h, 0, 6 * h, 3 * h]), 3 * h);
    }
}
