//! robots.txt for the one place Alchemy crawls (docs/RFC-events.md §2):
//! sitemap watches fetch pages the user never pasted, so the site's rules
//! apply. Feeds and pasted URLs are what the site published for readers
//! and are not gated here.
//!
//! One fetch per host, cached in the database for a day. Rules are matched
//! for the `Alchemy` user-agent group first, then `*`; the longest matching
//! `Allow`/`Disallow` path wins (the Google/RFC 9309 rule), `Crawl-delay`
//! is honored between page fetches. A robots.txt that cannot be fetched
//! allows everything except a `5xx`, which is read as "not now".

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::db::Db;

/// Our token in `User-agent:` groups.
pub const AGENT: &str = "Alchemy";
/// Cache lifetime for a host's rules.
const TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// Never wait longer than this between fetches, whatever the site asks.
const MAX_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    /// (allow, path-prefix) in file order; `allow` false is a Disallow.
    pub rules: Vec<(bool, String)>,
    pub crawl_delay_secs: Option<f64>,
    /// The origin answered 5xx (or nothing) — treat as temporarily closed.
    pub unavailable: bool,
}

impl Rules {
    /// Is this path fetchable under these rules? Longest match wins; ties
    /// go to Allow; no match allows.
    pub fn allows(&self, path: &str) -> bool {
        if self.unavailable {
            return false;
        }
        let mut best: Option<(usize, bool)> = None;
        for (allow, prefix) in &self.rules {
            if prefix.is_empty() {
                // "Disallow:" with nothing means allow all; skip it.
                continue;
            }
            if path_matches(path, prefix) {
                let len = prefix.len();
                match best {
                    Some((l, a)) if l > len || (l == len && a) => {}
                    _ => best = Some((len, *allow)),
                }
            }
        }
        best.map(|(_, allow)| allow).unwrap_or(true)
    }

    pub fn delay(&self) -> Option<Duration> {
        self.crawl_delay_secs
            .filter(|d| *d > 0.0)
            .map(|d| Duration::from_secs_f64(d).min(MAX_DELAY))
    }
}

/// robots.txt patterns: `*` wildcards, `$` anchors the end.
fn path_matches(path: &str, pattern: &str) -> bool {
    let anchored = pattern.ends_with('$');
    let pattern = pattern.trim_end_matches('$');
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !path.starts_with(part) {
                return false;
            }
            pos = part.len();
            continue;
        }
        match path[pos..].find(part) {
            Some(at) => pos += at + part.len(),
            None => return false,
        }
    }
    if anchored {
        parts.len() > 1 && pos == path.len() || parts.len() == 1 && path == pattern
    } else {
        true
    }
}

/// Parse a robots.txt for our agent: the `Alchemy` group when the file has
/// one, else the `*` group. Groups are the runs of `User-agent:` lines and
/// the rules that follow them, per RFC 9309.
pub fn parse(text: &str) -> Rules {
    let mut ours = Rules::default();
    let mut any = Rules::default();
    let mut in_ours = false;
    let mut in_any = false;
    let mut collecting_agents = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "user-agent" => {
                // A new group starts at the first agent line after rules.
                if !collecting_agents {
                    in_ours = false;
                    in_any = false;
                    collecting_agents = true;
                }
                let v = value.to_ascii_lowercase();
                if v == AGENT.to_ascii_lowercase() {
                    in_ours = true;
                } else if v == "*" {
                    in_any = true;
                }
            }
            "allow" | "disallow" => {
                collecting_agents = false;
                let allow = key == "allow";
                if in_ours {
                    ours.rules.push((allow, value.to_string()));
                }
                if in_any {
                    any.rules.push((allow, value.to_string()));
                }
            }
            "crawl-delay" => {
                collecting_agents = false;
                let d = value.parse::<f64>().ok();
                if in_ours {
                    ours.crawl_delay_secs = d;
                }
                if in_any {
                    any.crawl_delay_secs = d;
                }
            }
            _ => {
                collecting_agents = false;
            }
        }
    }
    if ours.rules.is_empty() && ours.crawl_delay_secs.is_none() {
        any
    } else {
        ours
    }
}

#[derive(Serialize, Deserialize)]
struct Cached {
    fetched_at: i64,
    rules: Rules,
}

fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split('/').next()?;
    Some(format!("{scheme}://{host}"))
}

pub fn path_of(url: &str) -> String {
    url.split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|i| rest[i..].to_string()))
        .unwrap_or_else(|| "/".to_string())
}

/// The rules for a URL's host, fetched at most once a day and kept in the
/// database (`robots.<origin>`), never on disk.
pub async fn rules_for(db: &Db, url: &str) -> Rules {
    let Some(origin) = origin_of(url) else {
        return Rules::default();
    };
    let key = format!("robots.{origin}");
    let now = crate::commands::now();
    if let Ok(Some(raw)) = db.kv_get(&key).await {
        if let Ok(c) = serde_json::from_str::<Cached>(&raw) {
            if now - c.fetched_at < TTL_MS {
                return c.rules;
            }
        }
    }
    let rules = fetch(&origin).await;
    if let Ok(json) = serde_json::to_string(&Cached {
        fetched_at: now,
        rules: rules.clone(),
    }) {
        let _ = db.kv_set(&key, &json).await;
    }
    rules
}

async fn fetch(origin: &str) -> Rules {
    let client = match reqwest::Client::builder()
        .user_agent(format!(
            "{AGENT}/{} (+https://thrashr888.github.io/alchemy/)",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Rules::default(),
    };
    match client.get(format!("{origin}/robots.txt")).send().await {
        Ok(resp) if resp.status().is_success() => {
            let text = resp.text().await.unwrap_or_default();
            parse(&text)
        }
        Ok(resp) if resp.status().is_server_error() => Rules {
            unavailable: true,
            ..Rules::default()
        },
        // 404 and friends: no rules, everything allowed.
        Ok(_) => Rules::default(),
        // Unreachable host: the page fetch will fail on its own; do not
        // pretend the site forbade it.
        Err(_) => Rules::default(),
    }
}

/// May Alchemy fetch this URL as a crawler?
pub async fn allowed(db: &Db, url: &str) -> bool {
    rules_for(db, url).await.allows(&path_of(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROBOTS: &str = "\
# comment
User-agent: Googlebot
Disallow: /private/

User-agent: *
Disallow: /admin/
Disallow: /tmp/*.pdf$
Allow: /admin/public/
Crawl-delay: 2

User-agent: Alchemy
Disallow: /products/
Crawl-delay: 5
";

    #[test]
    fn our_group_wins_over_the_wildcard() {
        let r = parse(ROBOTS);
        assert!(!r.allows("/products/hg-2"), "our own group's Disallow");
        assert!(
            r.allows("/admin/"),
            "the * group's rules do not apply once we have our own"
        );
        assert_eq!(r.delay(), Some(Duration::from_secs(5)));
    }

    #[test]
    fn the_wildcard_group_applies_when_we_are_not_named() {
        let text = ROBOTS.replace("User-agent: Alchemy", "User-agent: Other");
        let r = parse(&text);
        assert!(!r.allows("/admin/secret"));
        assert!(
            r.allows("/admin/public/x"),
            "longest match wins: Allow beats the shorter Disallow"
        );
        assert!(r.allows("/products/hg-2"));
        assert!(!r.allows("/tmp/report.pdf"), "wildcard and end anchor");
        assert!(r.allows("/tmp/report.pdf.bak"), "the $ anchor holds");
        assert_eq!(r.delay(), Some(Duration::from_secs(2)));
    }

    #[test]
    fn missing_or_empty_files_allow_everything() {
        assert!(parse("").allows("/anything"));
        assert!(
            parse("User-agent: *\nDisallow:\n").allows("/x"),
            "empty Disallow allows all"
        );
        assert!(!Rules {
            unavailable: true,
            ..Rules::default()
        }
        .allows("/x"));
        assert_eq!(path_of("https://a.b/c/d?e"), "/c/d?e");
        assert_eq!(path_of("https://a.b"), "/");
        assert_eq!(
            origin_of("https://www.x.y/p/q").as_deref(),
            Some("https://www.x.y")
        );
    }
}
