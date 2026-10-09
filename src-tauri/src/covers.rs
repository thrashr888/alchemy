//! Cover images: the deeper pick (docs/RFC-canvas.md, "Cover images, in bulk").
//!
//! A URL import used to stop at `og:image` / `twitter:image`, and a page
//! that does not set one got no picture at all. This module is the rest of
//! the ladder, and it is pure: markup (or markdown) in, URLs out, nothing
//! fetched, so every rung has a fixture test.
//!
//! Order for a page: the page's own pick (`og:image` / `twitter:image`),
//! then the first image in the article body that is big enough to be a
//! picture, then the site's `apple-touch-icon`, then its `<link rel=icon>`
//! when that is a PNG of a size worth showing. The same scan, taken whole,
//! is the candidate list the Cover images sheet offers.

use std::collections::HashSet;

use crate::ingest::{decode_entities, og_image};

/// How many candidates one source offers. A sheet row is a thumbnail strip;
/// past a dozen it is a gallery nobody asked for.
pub const MAX_CANDIDATES: usize = 12;

/// Markup is scanned up to this many bytes. Article bodies live well before
/// it; a multi-megabyte SPA shell is not worth walking to the end.
const SCAN_CAP: usize = 1_000_000;

/// Narrower than this, an image is chrome (an avatar, an emoji, a badge).
const MIN_WIDTH: u32 = 120;
/// Shorter than this, it is a divider or a pixel.
const MIN_HEIGHT: u32 = 60;
/// At or above this, an image is a picture rather than a thumbnail.
const GOOD_WIDTH: u32 = 300;
/// A site icon smaller than this is a favicon, not a cover.
const MIN_ICON: u32 = 64;

/// Words in a URL that mean "not content": tracking pixels, spacers, sprite
/// sheets, ad slots, placeholders.
const JUNK_URL: &[&str] = &[
    "pixel",
    "1x1",
    "spacer",
    "tracking",
    "beacon",
    "doubleclick",
    "adsystem",
    "/ads/",
    "sprite",
    "favicon",
    "gravatar.com",
    "blank.",
    "transparent.",
    "loading.",
    "placeholder",
    "facebook.com/tr",
];

/// Words in an `<img class/id/alt>` that mean the same thing.
const JUNK_ATTR: &[&str] = &[
    "avatar", "emoji", "icon", "logo", "badge", "sprite", "pixel", "tracking", "gravatar",
];

/// The best single cover for a page, or `None`. This is what import stamps.
pub fn pick_cover(html: &str, base_url: &str) -> Option<String> {
    og_image(html, base_url)
        .or_else(|| body_images(html, base_url).into_iter().next())
        .or_else(|| touch_icons(html, base_url).into_iter().next())
        .or_else(|| png_icons(html, base_url).into_iter().next())
}

/// Every plausible cover on a page, best first, for the Cover images sheet:
/// the page's own pick, body images (big ones first), then the site icons.
/// Deduped and capped.
pub fn page_cover_candidates(html: &str, base_url: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    out.extend(og_image(html, base_url));
    out.extend(body_images(html, base_url));
    out.extend(touch_icons(html, base_url));
    out.extend(png_icons(html, base_url));
    dedupe_capped(out)
}

/// Image references in text a person (or a converter) wrote: Markdown
/// `![alt](url)` and any inline `<img src>`. Relative references resolve
/// against `base_url` when there is one; a local path with nothing to
/// resolve against is dropped, since the card cannot fetch a file the
/// source merely names. Order of appearance, deduped, capped.
pub fn content_images(text: &str, base_url: Option<&str>) -> Vec<String> {
    let cap = floor_boundary(text, SCAN_CAP);
    let text = &text[..cap];
    let base = base_url.and_then(|b| reqwest::Url::parse(b).ok());
    let mut found: Vec<(usize, String)> = Vec::new();

    // ![alt](target "title")
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(off) = text[at..].find("![") {
        let start = at + off;
        at = start + 2;
        // The alt text and the target stay on one line: a stray "![" in
        // prose must not swallow the next paragraph's link, and every
        // lookahead stops at the line end so a page of stray "![" lines
        // is read once, not once per line.
        let line_end = text[at..].find('\n').map(|e| at + e).unwrap_or(text.len());
        let Some(close) = text[at..line_end].find("](") else {
            at = line_end;
            continue;
        };
        let target = at + close + 2;
        let mut i = target;
        while i < line_end && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        let (raw, end) = if bytes.get(i) == Some(&b'<') && i < line_end {
            match text[i + 1..line_end].find('>') {
                Some(e) => (&text[i + 1..i + 1 + e], i + 2 + e),
                None => {
                    at = line_end;
                    continue;
                }
            }
        } else {
            let e = text[i..line_end]
                .find(|c: char| c.is_whitespace() || c == ')')
                .map(|e| i + e)
                .unwrap_or(line_end);
            (&text[i..e], e)
        };
        at = end;
        found.push((start, raw.to_string()));
    }

    // <img src="…"> written inline.
    let lower = text.to_ascii_lowercase();
    for tag in open_tags(text, &lower, "img") {
        if let Some(src) = tag_attr(tag.text, "src") {
            found.push((tag.start, src));
        }
    }

    found.sort_by_key(|(pos, _)| *pos);
    let out = found
        .into_iter()
        .filter_map(|(_, raw)| resolve(&base, &raw))
        .filter(|u| !is_junk_url(u))
        .collect();
    dedupe_capped(out)
}

/// Body images worth showing, best first: the article's own pictures in
/// document order, ones that declare (or whose URL or `srcset` implies) at
/// least 300px across ahead of ones that declare nothing, ahead of
/// mid-sized ones. Tracking pixels, icons, data URIs and SVGs never appear.
fn body_images(html: &str, base_url: &str) -> Vec<String> {
    let base = reqwest::Url::parse(base_url).ok();
    let cap = floor_boundary(html, SCAN_CAP);
    let html = &html[..cap];
    let region = article_region(html);
    let lower = region.to_ascii_lowercase();

    let mut sized: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut small: Vec<String> = Vec::new();
    for tag in open_tags(&region, &lower, "img") {
        let Some((url, declared)) = read_img(tag.text, &base) else {
            continue;
        };
        match declared.or_else(|| url_width_hint(&url)) {
            Some(w) if w >= GOOD_WIDTH => sized.push(url),
            Some(w) if w >= MIN_WIDTH => small.push(url),
            Some(_) => {}
            None => unknown.push(url),
        }
    }
    sized.extend(unknown);
    sized.extend(small);
    sized
}

/// Parse one `<img>` into its absolute URL and the width the markup
/// declares for it, or `None` when it is chrome.
fn read_img(tag: &str, base: &Option<reqwest::Url>) -> Option<(String, Option<u32>)> {
    let hints = ["class", "id", "role"]
        .iter()
        .filter_map(|a| tag_attr(tag, a))
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if JUNK_ATTR.iter().any(|w| hints.contains(w)) || hints.contains("presentation") {
        return None;
    }

    let mut width = tag_attr(tag, "width").and_then(|v| px(&v));
    let height = tag_attr(tag, "height").and_then(|v| px(&v));
    // A declared size under the floor is a pixel or an icon whatever the
    // file turns out to be.
    if width.is_some_and(|w| w < MIN_WIDTH) || height.is_some_and(|h| h < MIN_HEIGHT) {
        return None;
    }

    let srcset = ["srcset", "data-srcset", "data-lazy-srcset"]
        .iter()
        .find_map(|a| tag_attr(tag, a));
    let mut raw = None;
    if let Some(set) = srcset {
        if let Some((url, w)) = best_of_srcset(&set) {
            raw = Some(url);
            width = width.max(w);
        }
    }
    let raw = raw.or_else(|| {
        ["data-src", "data-lazy-src", "data-original", "src"]
            .iter()
            .filter_map(|a| tag_attr(tag, a))
            .find(|v| !v.trim().is_empty() && !v.trim_start().starts_with("data:"))
    })?;
    let url = resolve(base, &raw)?;
    if is_junk_url(&url) {
        return None;
    }
    Some((url, width))
}

/// The `srcset` entry to use and the widest width the set declares. A
/// thumbnail does not need the 4000px original, so the smallest entry of at
/// least 800px wins; failing that, the largest there is. `2x` style
/// descriptors carry no width, so the last entry stands.
fn best_of_srcset(set: &str) -> Option<(String, Option<u32>)> {
    let mut entries: Vec<(String, Option<u32>)> = Vec::new();
    for part in set.split(',') {
        let mut it = part.split_whitespace();
        let Some(url) = it.next() else {
            continue;
        };
        if url.starts_with("data:") {
            continue;
        }
        let w = it
            .next()
            .and_then(|d| d.strip_suffix('w'))
            .and_then(|n| n.parse::<u32>().ok());
        entries.push((url.to_string(), w));
    }
    let widest = entries.iter().filter_map(|(_, w)| *w).max();
    let chosen = entries
        .iter()
        .filter(|(_, w)| w.is_some_and(|w| w >= 800))
        .min_by_key(|(_, w)| *w)
        .or_else(|| entries.iter().max_by_key(|(_, w)| w.unwrap_or(0)))?;
    Some((chosen.0.clone(), widest))
}

/// A width a URL admits to: `?w=1200`, `?width=800`, `-1200x630.jpg`, or a
/// `/1200x/` path segment. Image CDNs say it this way far more often than
/// they say it in a `width` attribute.
fn url_width_hint(url: &str) -> Option<u32> {
    let lower = url.to_ascii_lowercase();
    for key in ["?w=", "&w=", "?width=", "&width="] {
        if let Some(at) = lower.find(key) {
            let digits: String = lower[at + key.len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = digits.parse() {
                return Some(n);
            }
        }
    }
    // …-1200x630.jpg and /1200x630/ share a shape: digits, 'x', digits.
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    let b = path.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if b.get(j) == Some(&b'x') && b.get(j + 1).is_some_and(|c| c.is_ascii_digit()) {
                if let Ok(n) = path[i..j].parse::<u32>() {
                    return Some(n);
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    None
}

/// The part of a page that is the article: the first `<article>`, else
/// `<main>`, else the whole body. Navigation, footers and asides are cut out
/// of any of them, and so are page headers when there is no article to
/// anchor on (an article's own `<header>` often holds its hero image).
fn article_region(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let slice_of = |name: &str| -> Option<&str> {
        let start = open_tags(html, &lower, name).first()?.start;
        let close = format!("</{name}>");
        let end = lower[start..]
            .find(&close)
            .map(|e| start + e)
            .unwrap_or(html.len());
        Some(&html[start..end])
    };
    if let Some(article) = slice_of("article").or_else(|| slice_of("main")) {
        return cut_blocks(article, &["nav", "footer", "aside"]);
    }
    let body_at = open_tags(html, &lower, "body")
        .first()
        .map(|t| t.start)
        .unwrap_or(0);
    cut_blocks(&html[body_at..], &["nav", "header", "footer", "aside"])
}

/// `html` without any `<name>…</name>` block for the given names.
fn cut_blocks(html: &str, names: &[&str]) -> String {
    let mut current = html.to_string();
    for name in names {
        let mut out = String::with_capacity(current.len());
        let lower = current.to_ascii_lowercase();
        let close = format!("</{name}>");
        let mut at = 0;
        for tag in open_tags(&current, &lower, name) {
            if tag.start < at {
                continue; // nested inside a block already cut
            }
            out.push_str(&current[at..tag.start]);
            at = match lower[tag.start..].find(&close) {
                Some(e) => tag.start + e + close.len(),
                None => current.len(),
            };
        }
        out.push_str(&current[at..]);
        current = out;
    }
    current
}

/// `<link rel="apple-touch-icon">` hrefs, largest first.
fn touch_icons(html: &str, base_url: &str) -> Vec<String> {
    let mut icons = link_icons(html, base_url, |rel| {
        rel.iter().any(|t| t.starts_with("apple-touch-icon"))
    });
    icons.sort_by_key(|(_, size, _)| std::cmp::Reverse(size.unwrap_or(0)));
    icons.into_iter().map(|(url, _, _)| url).collect()
}

/// `<link rel="icon">` hrefs that are PNGs declaring at least 64px, largest
/// first. A favicon that does not say how big it is is a 16px favicon.
fn png_icons(html: &str, base_url: &str) -> Vec<String> {
    let mut icons = link_icons(html, base_url, |rel| {
        rel.contains(&"icon") && !rel.iter().any(|t| t.starts_with("apple-touch-icon"))
    });
    icons.retain(|(url, size, mime)| {
        let path = url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .to_ascii_lowercase();
        let png = path.ends_with(".png") || mime.as_deref() == Some("image/png");
        png && size.is_some_and(|s| s >= MIN_ICON)
    });
    icons.sort_by_key(|(_, size, _)| std::cmp::Reverse(size.unwrap_or(0)));
    icons.into_iter().map(|(url, _, _)| url).collect()
}

/// `(absolute href, largest declared edge, type)` for every `<link>` whose
/// space-separated `rel` tokens satisfy `want`.
fn link_icons(
    html: &str,
    base_url: &str,
    want: impl Fn(&[&str]) -> bool,
) -> Vec<(String, Option<u32>, Option<String>)> {
    let base = reqwest::Url::parse(base_url).ok();
    let cap = floor_boundary(html, SCAN_CAP);
    let html = &html[..cap];
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    for tag in open_tags(html, &lower, "link") {
        let Some(rel) = tag_attr(tag.text, "rel") else {
            continue;
        };
        let rel = rel.to_ascii_lowercase();
        let tokens: Vec<&str> = rel.split_whitespace().collect();
        if !want(&tokens) {
            continue;
        }
        let Some(url) = tag_attr(tag.text, "href").and_then(|h| resolve(&base, &h)) else {
            continue;
        };
        let size = tag_attr(tag.text, "sizes").and_then(|s| {
            s.to_ascii_lowercase()
                .split_whitespace()
                .filter_map(|d| {
                    let (w, h) = d.split_once('x')?;
                    Some(w.parse::<u32>().ok()?.min(h.parse::<u32>().ok()?))
                })
                .max()
        });
        let mime = tag_attr(tag.text, "type").map(|t| t.to_ascii_lowercase());
        out.push((url, size, mime));
    }
    out
}

// ---- tiny markup helpers ---------------------------------------------------

/// One opening tag located in a document.
struct OpenTag<'a> {
    start: usize,
    text: &'a str,
}

/// Every `<name …>` in `html`. `lower` is `html.to_ascii_lowercase()` (same
/// byte length, so one set of offsets serves both). The tag name must end at
/// whitespace, `/` or `>`: `<a` does not match `<article`.
fn open_tags<'a>(html: &'a str, lower: &str, name: &str) -> Vec<OpenTag<'a>> {
    let needle = format!("<{name}");
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(off) = lower[pos..].find(&needle) {
        let start = pos + off;
        let after = start + needle.len();
        let boundary = lower
            .as_bytes()
            .get(after)
            .is_some_and(|c| c.is_ascii_whitespace() || *c == b'/' || *c == b'>');
        let Some(end) = lower[start..].find('>').map(|e| start + e + 1) else {
            break;
        };
        pos = end;
        if boundary {
            out.push(OpenTag {
                start,
                text: &html[start..end],
            });
        }
    }
    out
}

/// The value of attribute `name` on one tag, case-insensitive on the name,
/// quoted or bare, entity-decoded. Unlike a substring search it never mistakes
/// `data-src` for `src`.
fn tag_attr(tag: &str, name: &str) -> Option<String> {
    let b = tag.as_bytes();
    let mut i = 1; // past '<'
    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' {
        i += 1; // the tag name
    }
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() || b[i] == b'>' {
            return None;
        }
        let name_start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let attr = &tag[name_start..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b.get(i) != Some(&b'=') {
            continue; // a bare attribute (`hidden`)
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = match b.get(i) {
            Some(&q) if q == b'"' || q == b'\'' => {
                let rest = &tag[i + 1..];
                let close = rest.find(q as char)?;
                i += close + 2;
                &rest[..close]
            }
            Some(_) => {
                let s = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                &tag[s..i]
            }
            None => return None,
        };
        if attr.eq_ignore_ascii_case(name) {
            return Some(entities(value.trim()));
        }
    }
}

/// `decode_entities` plus the numeric spellings of `&` and `'` that CMSes
/// emit inside URLs.
fn entities(s: &str) -> String {
    decode_entities(s)
        .replace("&#038;", "&")
        .replace("&#38;", "&")
        .replace("&#x26;", "&")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
}

/// `"640"`, `"640px"` → 640. `"100%"`, `"auto"` → `None`.
fn px(v: &str) -> Option<u32> {
    let v = v.trim().trim_end_matches("px");
    v.parse::<f64>()
        .ok()
        .filter(|n| *n >= 0.0)
        .map(|n| n as u32)
}

/// An absolute http(s) URL from a possibly relative reference, or `None`.
fn resolve(base: &Option<reqwest::Url>, raw: &str) -> Option<String> {
    let raw = entities(raw.trim());
    if raw.is_empty() || raw.starts_with("data:") || raw.starts_with("javascript:") {
        return None;
    }
    let url = match base {
        Some(b) => b.join(&raw).ok()?,
        None => reqwest::Url::parse(&raw).ok()?,
    };
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

/// A URL that is an SVG, a pixel, a sprite or the like.
fn is_junk_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    path.ends_with(".svg") || JUNK_URL.iter().any(|w| lower.contains(w))
}

fn dedupe_capped(urls: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    urls.into_iter()
        .filter(|u| seen.insert(u.clone()))
        .take(MAX_CANDIDATES)
        .collect()
}

/// The largest char boundary at or below `max` bytes.
fn floor_boundary(s: &str, max: usize) -> usize {
    let mut i = max.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://blog.example.com/posts/2026/rust-edition";

    #[test]
    fn og_image_wins_and_resolves_relative() {
        let html = r#"<html><head>
            <meta property="og:image" content="/social/card.png">
            <link rel="apple-touch-icon" href="/touch.png">
        </head><body><article><img src="/body.jpg" width="900"></article></body></html>"#;
        assert_eq!(
            pick_cover(html, PAGE).as_deref(),
            Some("https://blog.example.com/social/card.png")
        );
    }

    #[test]
    fn twitter_image_counts_as_the_page_pick() {
        let html = r#"<meta name="twitter:image" content="https://cdn.example.com/t.jpg">"#;
        assert_eq!(
            pick_cover(html, PAGE).as_deref(),
            Some("https://cdn.example.com/t.jpg")
        );
    }

    #[test]
    fn body_image_with_srcset_when_no_meta() {
        let html = r#"<html><head><title>x</title></head><body>
            <nav><img src="/nav-banner.jpg" width="900" height="200"></nav>
            <article><h1>Hi</h1>
              <img src="/img/hero-small.jpg"
                   srcset="/img/hero-480.jpg 480w, /img/hero-1200.jpg 1200w, /img/hero-2400.jpg 2400w"
                   alt="A hero">
              <p>text</p>
            </article></body></html>"#;
        // Smallest srcset entry that is at least 800px wide; the nav's
        // image is outside the article.
        assert_eq!(
            pick_cover(html, PAGE).as_deref(),
            Some("https://blog.example.com/img/hero-1200.jpg")
        );
    }

    #[test]
    fn body_prefers_sized_over_unsized_and_skips_tracking_pixels() {
        let html = r#"<body><article>
            <img src="https://stats.example.com/pixel.gif" width="1" height="1">
            <img src="https://ads.example.com/track.png">
            <img src="/tiny.png" width="40" height="40">
            <img src="/diagram.png">
            <img src="/photo.jpg" width="640" height="360">
        </article></body>"#;
        // "track.png" is unsized and not caught by name, so it competes as
        // an unknown; the declared 640px photo outranks every unknown.
        assert_eq!(
            pick_cover(html, PAGE).as_deref(),
            Some("https://blog.example.com/photo.jpg")
        );
    }

    #[test]
    fn body_skips_svg_data_uri_and_icon_classes() {
        let html = r#"<body><article>
            <img src="/logo.svg" width="400">
            <img src="data:image/gif;base64,R0lGODlhAQABAAAAACw=" width="400">
            <img class="avatar" src="/me.jpg" width="400">
            <img src="/emoji/smile.png" class="wp-smiley emoji" width="400">
            <img data-src="/lazy/real.jpg" src="data:image/gif;base64,AAAA" width="500">
        </article></body>"#;
        assert_eq!(
            body_images(html, PAGE),
            vec!["https://blog.example.com/lazy/real.jpg".to_string()]
        );
    }

    #[test]
    fn data_src_is_not_mistaken_for_src() {
        let tag = r#"<img data-src="/a.jpg" src="/b.jpg" alt="x">"#;
        assert_eq!(tag_attr(tag, "src").as_deref(), Some("/b.jpg"));
        assert_eq!(tag_attr(tag, "data-src").as_deref(), Some("/a.jpg"));
        assert_eq!(tag_attr(tag, "SRC").as_deref(), Some("/b.jpg"));
    }

    #[test]
    fn url_hints_stand_in_for_missing_width_attributes() {
        let html = r#"<body><article>
            <img src="https://img.example.com/a.jpg?w=80">
            <img src="https://img.example.com/b-1200x630.jpg">
            </article></body>"#;
        let imgs = body_images(html, PAGE);
        // ?w=80 is under the floor and dropped; the sized one remains.
        assert_eq!(imgs, vec!["https://img.example.com/b-1200x630.jpg"]);
    }

    #[test]
    fn page_header_is_cut_without_an_article_but_kept_inside_one() {
        let no_article = r#"<body><header><img src="/masthead.jpg" width="900"></header>
            <div><img src="/story.jpg" width="900"></div></body>"#;
        assert_eq!(
            body_images(no_article, PAGE),
            vec!["https://blog.example.com/story.jpg"]
        );
        let with_article = r#"<body><header><img src="/masthead.jpg" width="900"></header>
            <article><header><img src="/hero.jpg" width="900"></header></article></body>"#;
        assert_eq!(
            body_images(with_article, PAGE),
            vec!["https://blog.example.com/hero.jpg"]
        );
    }

    #[test]
    fn apple_touch_icon_falls_back_largest_first() {
        let html = r#"<head>
            <link rel="icon" href="/favicon.ico">
            <link rel="apple-touch-icon" sizes="76x76" href="/t-76.png">
            <link rel="apple-touch-icon" sizes="180x180" href="/t-180.png">
        </head><body><p>no images here</p></body>"#;
        assert_eq!(
            pick_cover(html, PAGE).as_deref(),
            Some("https://blog.example.com/t-180.png")
        );
    }

    #[test]
    fn site_icon_only_when_png_of_reasonable_size() {
        let ico = r#"<link rel="icon" href="/favicon.ico" sizes="256x256">"#;
        assert_eq!(pick_cover(ico, PAGE), None);
        let tiny = r#"<link rel="shortcut icon" type="image/png" href="/f.png" sizes="16x16">"#;
        assert_eq!(pick_cover(tiny, PAGE), None);
        let unsized_png = r#"<link rel="icon" href="/f.png">"#;
        assert_eq!(pick_cover(unsized_png, PAGE), None);
        let ok = r#"<link rel="icon" type="image/png" sizes="32x32 192x192" href="/icon-192.png">"#;
        assert_eq!(
            pick_cover(ok, PAGE).as_deref(),
            Some("https://blog.example.com/icon-192.png")
        );
    }

    #[test]
    fn nothing_found_is_none() {
        assert_eq!(
            pick_cover("<html><body><p>words</p></body></html>", PAGE),
            None
        );
    }

    #[test]
    fn candidates_are_ordered_and_deduped() {
        let html = r#"<head>
            <meta property="og:image" content="https://cdn.example.com/og.jpg">
            <link rel="apple-touch-icon" sizes="180x180" href="/t.png">
        </head><body><article>
            <img src="https://cdn.example.com/og.jpg" width="800">
            <img src="/second.jpg" width="800">
        </article></body>"#;
        assert_eq!(
            page_cover_candidates(html, PAGE),
            vec![
                "https://cdn.example.com/og.jpg",
                "https://blog.example.com/second.jpg",
                "https://blog.example.com/t.png",
            ]
        );
    }

    #[test]
    fn entities_in_urls_decode() {
        let html = r#"<article><img src="/i.jpg?a=1&amp;w=900&#038;q=80"></article>"#;
        assert_eq!(
            body_images(html, PAGE),
            vec!["https://blog.example.com/i.jpg?a=1&w=900&q=80"]
        );
    }

    #[test]
    fn markdown_images_resolve_and_skip_local_paths() {
        let md =
            "# Title\n\n![hero](https://cdn.example.com/h.png)\n\nText ![rel](img/a.jpg \"cap\")\n\
                  and ![angle](<https://x.example.com/a b.png>) and ![local](./pics/c.jpg)\n\
                  <img src=\"/inline.webp\">\n![logo](https://x.example.com/logo.svg)";
        // With a page to resolve against, relative paths resolve.
        assert_eq!(
            content_images(md, Some("https://notes.example.com/dir/page")),
            vec![
                "https://cdn.example.com/h.png",
                "https://notes.example.com/dir/img/a.jpg",
                "https://x.example.com/a%20b.png",
                "https://notes.example.com/dir/pics/c.jpg",
                "https://notes.example.com/inline.webp",
            ]
        );
        // With nothing to resolve against, only absolute references remain.
        assert_eq!(
            content_images(md, None),
            vec![
                "https://cdn.example.com/h.png",
                "https://x.example.com/a%20b.png",
            ]
        );
    }

    #[test]
    fn markdown_stray_bang_bracket_does_not_swallow_next_line() {
        let md = "Wow ![ not an image\n\n[link](https://x.example.com/page)\n\n![ok](https://x.example.com/p.png)";
        assert_eq!(
            content_images(md, None),
            vec!["https://x.example.com/p.png"]
        );
    }

    #[test]
    fn a_page_of_stray_bang_brackets_is_read_once() {
        // 20,000 lines that open an image and never close it, then one
        // that does. Rescanning to the real link from every stray line
        // took seconds; bounded to the line it is milliseconds.
        let mut text = "![stray\n".repeat(20_000);
        text.push_str("![ok](https://x.test/y.png)\n");
        let t0 = std::time::Instant::now();
        let found = content_images(&text, None);
        assert_eq!(found, vec!["https://x.test/y.png".to_string()]);
        assert!(t0.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn scan_is_unicode_safe() {
        let html = format!(
            "<body>{}<article><img src=\"/é.jpg\" width=\"500\"></article></body>",
            "é".repeat(10)
        );
        assert_eq!(body_images(&html, PAGE).len(), 1);
        assert!(content_images("日本語 ![x](https://a.example.com/日.png)", None).len() == 1);
    }
}
