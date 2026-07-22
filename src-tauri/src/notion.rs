//! Notion pages as living sources (docs/RFC-obsidian-notion.md §4).
//!
//! The trick is that there is no new ingestion pipeline: a Notion page tree
//! is exported to a cache directory as markdown files — root page at the
//! top, child pages as a mirrored subtree — and the existing folder
//! machinery (rescan, mtime-diffed re-embeds, promote/demote, grep leg,
//! reader tree) ingests it like any folder. Refresh re-exports only pages
//! whose `last_edited_time` moved, so the rescan re-embeds only what changed.
//!
//! Auth is an internal integration token the user creates and shares pages
//! with inside Notion — that sharing step IS the permission model. The token
//! is sent only to api.notion.com. Rate limit is ~3 req/s: fetches serialize
//! through one polite pace with Retry-After backoff on 429.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

const API: &str = "https://api.notion.com/v1";
const NOTION_VERSION: &str = "2022-06-28";
/// Pause between API calls — under Notion's ~3 req/s ceiling with headroom.
const PACE: std::time::Duration = std::time::Duration::from_millis(350);
/// Page-tree recursion guard (Notion trees are shallow; cycles are not).
const MAX_DEPTH: usize = 12;

/// A notion.so / *.notion.site URL's page id (dashless 32-hex tail), if any.
pub fn detect_page(url: &str) -> Option<String> {
    let u = url.trim();
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let host_ok = host == "www.notion.so"
        || host == "notion.so"
        || host == "www.notion.site"
        || host.ends_with(".notion.site")
        || host.ends_with(".notion.so");
    if !host_ok {
        return None;
    }
    // The id is the last 32 hex chars of the final path segment (before any
    // query), tolerant of slug prefixes and dashed UUID forms.
    let last = path.split('?').next()?.split('/').next_back()?;
    let hex: String = last.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() < 32 {
        return None;
    }
    Some(hex[hex.len() - 32..].to_lowercase())
}

/// Cache directory for one Notion parent source's exported markdown.
pub fn cache_dir(data_dir: &Path, source_id: &str) -> PathBuf {
    data_dir.join("notion").join(source_id)
}

/// What an export pass did — the parent row's stamp and title come from here.
pub struct ExportStats {
    pub title: String,
    pub pages: usize,
    /// Max `last_edited_time` across the tree (unix millis) — the content
    /// stamp; a refresh with an unchanged stamp rewrote nothing.
    pub max_edited_ms: i64,
}

pub struct NotionClient {
    http: reqwest::Client,
    token: String,
}

impl NotionClient {
    pub fn new(token: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client");
        Self {
            http,
            token: token.trim().to_string(),
        }
    }

    /// GET with auth + version headers, pacing, and 429 Retry-After backoff.
    async fn get(&self, url: &str) -> Result<Value> {
        for attempt in 0..3 {
            tokio::time::sleep(PACE).await;
            let resp = self
                .http
                .get(url)
                .bearer_auth(&self.token)
                .header("Notion-Version", NOTION_VERSION)
                .send()
                .await
                .context("couldn't reach Notion — check your connection")?;
            let status = resp.status();
            if status.as_u16() == 429 && attempt < 2 {
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(2.0);
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait.min(30.0))).await;
                continue;
            }
            if !status.is_success() {
                let body: Value = resp.json().await.unwrap_or_default();
                let msg = body["message"].as_str().unwrap_or("");
                anyhow::bail!(match status.as_u16() {
                    401 => "Notion rejected the token — re-check it in Settings → Sources".into(),
                    404 => "Notion can't see this page — share it with your integration \
                            (page ••• menu → Connections)"
                        .to_string(),
                    _ => format!("Notion API {status}: {msg}"),
                });
            }
            return resp.json().await.context("invalid Notion response");
        }
        Err(anyhow!("Notion kept rate-limiting — try again in a minute"))
    }

    /// Validate the token against `/users/me`; returns a human label (the
    /// integration's workspace or bot name) on success. Powers the Settings
    /// field's live "key works" check.
    pub async fn check_token(&self) -> Result<String> {
        let v = self.get(&format!("{API}/users/me")).await?;
        let name = v["bot"]["workspace_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| v["name"].as_str())
            .unwrap_or("your workspace")
            .to_string();
        Ok(name)
    }

    /// A page's title and `last_edited_time` (unix millis).
    async fn page_meta(&self, page_id: &str) -> Result<(String, i64)> {
        let v = self.get(&format!("{API}/pages/{page_id}")).await?;
        let title = v["properties"]
            .as_object()
            .and_then(|props| {
                props
                    .values()
                    .find(|p| p["type"].as_str() == Some("title"))
                    .and_then(|p| p["title"].as_array())
                    .map(|parts| rich_text(parts))
            })
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Untitled".to_string());
        Ok((title, edited_ms(&v)))
    }

    /// All child blocks of a block/page, following pagination.
    async fn block_children(&self, block_id: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut url = format!("{API}/blocks/{block_id}/children?page_size=100");
            if let Some(c) = &cursor {
                url.push_str(&format!("&start_cursor={c}"));
            }
            let v = self.get(&url).await?;
            if let Some(results) = v["results"].as_array() {
                out.extend(results.iter().cloned());
            }
            match v["next_cursor"].as_str() {
                Some(c) if v["has_more"].as_bool() == Some(true) => cursor = Some(c.to_string()),
                _ => break,
            }
        }
        Ok(out)
    }

    /// Export a page tree as markdown files under `dir`. The root page
    /// becomes `<Title>.md`; child pages mirror into `<Title>/…`. Pages
    /// whose `last_edited_time` hasn't moved past the existing file's mtime
    /// are skipped (their subtrees still walk — Notion timestamps don't
    /// bubble up from children).
    pub async fn export_tree(&self, page_id: &str, dir: &Path) -> Result<ExportStats> {
        std::fs::create_dir_all(dir).context("failed to create notion cache dir")?;
        let mut stats = ExportStats {
            title: String::new(),
            pages: 0,
            max_edited_ms: 0,
        };
        self.export_page(page_id, dir, 0, &mut stats).await?;
        Ok(stats)
    }

    async fn export_page(
        &self,
        page_id: &str,
        dir: &Path,
        depth: usize,
        stats: &mut ExportStats,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let (title, edited) = self.page_meta(page_id).await?;
        if stats.title.is_empty() {
            stats.title = title.clone();
        }
        stats.max_edited_ms = stats.max_edited_ms.max(edited);
        let file = dir.join(format!("{}.md", safe_name(&title)));

        // Unchanged since last export: keep the file (and its mtime) so the
        // rescan skips re-embedding; still descend for changed children.
        let fresh = file
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64 >= edited)
            .unwrap_or(false);

        let blocks = self.block_children(page_id).await?;
        let mut md = String::new();
        let mut children: Vec<(String, String)> = Vec::new();
        render_blocks(&blocks, 0, &safe_name(&title), &mut md, &mut children);
        // Nested non-page blocks (toggles, list children) fetch lazily here:
        // v1 renders only top-level block children plus one nesting level
        // already present in the payload — deep block nesting degrades to
        // what the API inlined.

        if !fresh {
            let body = format!("# {title}\n\n{}", md.trim());
            std::fs::write(&file, body).context("failed to write notion export")?;
            stats.pages += 1;
        }

        if !children.is_empty() {
            let child_dir = dir.join(safe_name(&title));
            std::fs::create_dir_all(&child_dir).ok();
            for (child_id, _) in &children {
                if let Err(err) =
                    Box::pin(self.export_page(child_id, &child_dir, depth + 1, stats)).await
                {
                    // One unshared/deleted child shouldn't sink the tree.
                    eprintln!("notion: child {child_id} skipped: {err:#}");
                }
            }
        }
        Ok(())
    }
}

fn edited_ms(page: &Value) -> i64 {
    page["last_edited_time"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

/// Filesystem-safe filename from a page title.
fn safe_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if matches!(c, '/' | ':' | '\\' | '\0') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    let out: String = trimmed.chars().take(80).collect();
    if out.is_empty() {
        "Untitled".to_string()
    } else {
        out
    }
}

/// Concatenate a rich_text array to markdown-ish plain text: code annotations
/// keep backticks, links become [text](url), everything else is plain.
fn rich_text(parts: &[Value]) -> String {
    let mut out = String::new();
    for p in parts {
        let text = p["plain_text"].as_str().unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        let href = p["href"].as_str().unwrap_or_default();
        if p["annotations"]["code"].as_bool() == Some(true) {
            out.push_str(&format!("`{text}`"));
        } else if !href.is_empty() {
            // Notion link spans keep their surrounding spaces inside the
            // span; hoist them out so the markdown reads [docs], not [ docs].
            let lead = text.len() - text.trim_start().len();
            let trail = text.trim_end().len();
            let core = text[lead..trail].trim();
            if core.is_empty() {
                out.push_str(text);
            } else {
                out.push_str(&text[..lead]);
                out.push_str(&format!("[{core}]({href})"));
                out.push_str(&text[trail..]);
            }
        } else {
            out.push_str(text);
        }
    }
    out
}

/// Render a block list to markdown, collecting `child_page` blocks as
/// (id, title) for the exporter to recurse into. `subdir` is the page's own
/// mirrored child directory (its safe name) — child-page links point there.
/// Nested payload children (the API inlines one level for lists/toggles)
/// indent under their parent.
pub fn render_blocks(
    blocks: &[Value],
    indent: usize,
    subdir: &str,
    out: &mut String,
    children: &mut Vec<(String, String)>,
) {
    let pad = "  ".repeat(indent);
    let mut numbered = 0usize;
    for b in blocks {
        let ty = b["type"].as_str().unwrap_or_default();
        if ty != "numbered_list_item" {
            numbered = 0;
        }
        let txt = |key: &str| rich_text(b[key]["rich_text"].as_array().map_or(&[], |v| v));
        match ty {
            "paragraph" => {
                let t = txt("paragraph");
                if !t.is_empty() {
                    out.push_str(&format!("{pad}{t}\n\n"));
                }
            }
            "heading_1" => out.push_str(&format!("## {}\n\n", txt("heading_1"))),
            "heading_2" => out.push_str(&format!("### {}\n\n", txt("heading_2"))),
            "heading_3" => out.push_str(&format!("#### {}\n\n", txt("heading_3"))),
            "bulleted_list_item" => {
                out.push_str(&format!("{pad}- {}\n", txt("bulleted_list_item")))
            }
            "numbered_list_item" => {
                numbered += 1;
                out.push_str(&format!("{pad}{numbered}. {}\n", txt("numbered_list_item")));
            }
            "to_do" => {
                let mark = if b["to_do"]["checked"].as_bool() == Some(true) {
                    "x"
                } else {
                    " "
                };
                out.push_str(&format!("{pad}- [{mark}] {}\n", txt("to_do")));
            }
            "toggle" => out.push_str(&format!("{pad}- {}\n", txt("toggle"))),
            "quote" => out.push_str(&format!("{pad}> {}\n\n", txt("quote"))),
            "callout" => {
                let icon = b["callout"]["icon"]["emoji"].as_str().unwrap_or_default();
                let t = txt("callout");
                out.push_str(&format!("{pad}> {icon} {t}\n\n"));
            }
            "code" => {
                let lang = b["code"]["language"].as_str().unwrap_or_default();
                let t = rich_text(b["code"]["rich_text"].as_array().map_or(&[], |v| v));
                out.push_str(&format!("```{lang}\n{t}\n```\n\n"));
            }
            "divider" => out.push_str("---\n\n"),
            "equation" => {
                let ex = b["equation"]["expression"].as_str().unwrap_or_default();
                out.push_str(&format!("{pad}$${ex}$$\n\n"));
            }
            "table" => { /* rows arrive as nested table_row children below */ }
            "table_row" => {
                let cells: Vec<String> = b["table_row"]["cells"]
                    .as_array()
                    .map(|rows| {
                        rows.iter()
                            .map(|cell| rich_text(cell.as_array().map_or(&[], |v| v)))
                            .collect()
                    })
                    .unwrap_or_default();
                out.push_str(&format!("| {} |\n", cells.join(" | ")));
            }
            "child_page" => {
                let title = b["child_page"]["title"].as_str().unwrap_or("Untitled");
                let id: String = b["id"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| c.is_ascii_hexdigit())
                    .collect();
                // Link into the mirrored subtree; the reader's in-corpus
                // routing makes it a hop.
                out.push_str(&format!(
                    "{pad}- [{title}]({subdir}/{}.md)\n",
                    safe_name(title)
                ));
                if !id.is_empty() {
                    children.push((id, title.to_string()));
                }
            }
            "child_database" => {
                let title = b["child_database"]["title"].as_str().unwrap_or("Untitled");
                out.push_str(&format!("{pad}*Database: {title}* (not imported yet)\n\n"));
            }
            "bookmark" | "embed" | "link_preview" => {
                if let Some(u) = b[ty]["url"].as_str() {
                    out.push_str(&format!("{pad}<{u}>\n\n"));
                }
            }
            "image" => {
                let cap = rich_text(b["image"]["caption"].as_array().map_or(&[], |v| v));
                // File URLs are signed and expire — keep the caption, not a
                // link that will 403 next week.
                if cap.is_empty() {
                    out.push_str(&format!("{pad}*[image]*\n\n"));
                } else {
                    out.push_str(&format!("{pad}*[image: {cap}]*\n\n"));
                }
            }
            _ => {
                // Unknown blocks degrade to their rich_text when present.
                let t = txt(ty);
                if !t.is_empty() {
                    out.push_str(&format!("{pad}{t}\n\n"));
                }
            }
        }
        // The API inlines one level of nested children for some blocks.
        if let Some(nested) = b[ty]["children"].as_array() {
            render_blocks(nested, indent + 1, subdir, out, children);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_notion_urls() {
        assert_eq!(
            detect_page("https://www.notion.so/acme/Roadmap-8a3f05e2c1d94b7fa2d0c9e8b1a2f3d4"),
            Some("8a3f05e2c1d94b7fa2d0c9e8b1a2f3d4".into())
        );
        assert_eq!(
            detect_page("https://acme.notion.site/8a3f05e2-c1d9-4b7f-a2d0-c9e8b1a2f3d4?pvs=4"),
            Some("8a3f05e2c1d94b7fa2d0c9e8b1a2f3d4".into())
        );
        assert_eq!(
            detect_page("https://example.com/8a3f05e2c1d94b7fa2d0c9e8b1a2f3d4"),
            None
        );
        assert_eq!(detect_page("https://www.notion.so/pricing"), None);
    }

    #[test]
    fn renders_common_blocks() {
        let rt = |s: &str| json!([{ "plain_text": s, "annotations": {} }]);
        let blocks = vec![
            json!({ "type": "heading_1", "heading_1": { "rich_text": rt("Brewing") } }),
            json!({ "type": "paragraph", "paragraph": { "rich_text": rt("Start at 93C.") } }),
            json!({ "type": "bulleted_list_item",
                    "bulleted_list_item": { "rich_text": rt("grind finer") } }),
            json!({ "type": "numbered_list_item",
                    "numbered_list_item": { "rich_text": rt("first") } }),
            json!({ "type": "numbered_list_item",
                    "numbered_list_item": { "rich_text": rt("second") } }),
            json!({ "type": "to_do", "to_do": { "rich_text": rt("descale"), "checked": true } }),
            json!({ "type": "code",
                    "code": { "rich_text": rt("let x = 1;"), "language": "rust" } }),
            json!({ "type": "callout",
                    "callout": { "rich_text": rt("watch the temp"),
                                 "icon": { "emoji": "🔥" } } }),
            json!({ "type": "child_page", "id": "abc-123", "child_page": { "title": "Log" } }),
        ];
        let mut md = String::new();
        let mut kids = Vec::new();
        render_blocks(&blocks, 0, "Home", &mut md, &mut kids);
        assert!(md.contains("## Brewing"));
        assert!(md.contains("Start at 93C."));
        assert!(md.contains("- grind finer"));
        assert!(md.contains("1. first\n2. second"));
        assert!(md.contains("- [x] descale"));
        assert!(md.contains("```rust\nlet x = 1;\n```"));
        assert!(md.contains("> 🔥 watch the temp"));
        assert!(md.contains("[Log](Home/Log.md)"));
        assert_eq!(kids, vec![("abc123".to_string(), "Log".to_string())]);
    }

    #[test]
    fn rich_text_keeps_code_and_links() {
        let parts = json!([
            { "plain_text": "use ", "annotations": {} },
            { "plain_text": "cargo", "annotations": { "code": true } },
            { "plain_text": " docs", "annotations": {}, "href": "https://doc.rust-lang.org" },
        ]);
        assert_eq!(
            rich_text(parts.as_array().unwrap()),
            "use `cargo` [docs](https://doc.rust-lang.org)"
        );
    }

    #[test]
    fn safe_names_strip_path_hazards() {
        assert_eq!(safe_name("A/B: C"), "A-B- C");
        assert_eq!(safe_name(""), "Untitled");
        assert_eq!(safe_name("."), "Untitled");
    }
}
