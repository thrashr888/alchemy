//! Source ingestion: pull plain text out of files/URLs and split it into
//! overlapping chunks suitable for embedding.

use anyhow::{anyhow, Context, Result};
use std::path::Path;

/// Roughly target ~280 words per chunk with ~40 words of overlap. Word-based
/// rather than token-based keeps it model-agnostic and good enough for RAG.
const CHUNK_WORDS: usize = 280;
const OVERLAP_WORDS: usize = 40;

pub struct Extracted {
    pub title: String,
    pub source_type: String,
    /// Original URL for `url` sources; empty for local files / pasted text.
    pub url: String,
    pub text: String,
}

/// Is this path an image we should OCR rather than read as text?
pub fn is_image(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "heic"
    )
}

/// Is this path a PDF?
pub fn is_pdf(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// File stem as a display title.
pub fn file_title(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string()
}

/// Extract text from a local file, inferring type from the extension.
pub fn extract_file(path: &str) -> Result<Extracted> {
    let p = Path::new(path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let title = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();

    let (source_type, text) = match ext.as_str() {
        "pdf" => {
            let text = pdf_extract::extract_text(path)
                .with_context(|| format!("failed to extract text from PDF {path}"))?;
            if normalize(&text).trim().is_empty() {
                return Err(anyhow!(
                    "no selectable text in {path} — it looks like a scanned/image PDF. \
                     Export its pages as images to OCR them."
                ));
            }
            ("pdf".to_string(), text)
        }
        "md" | "markdown" => (
            "markdown".to_string(),
            std::fs::read_to_string(path).context("failed to read markdown file")?,
        ),
        "xlsx" | "xls" | "xlsm" | "ods" => ("text".to_string(), extract_spreadsheet(path)?),
        "docx" => ("text".to_string(), extract_docx(path)?),
        "pptx" => ("text".to_string(), extract_pptx(path)?),
        "txt" | "text" | "" => (
            "text".to_string(),
            std::fs::read_to_string(path).context("failed to read text file")?,
        ),
        other => {
            // Best-effort: treat unknown extensions as UTF-8 text.
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("unsupported file type .{other}"))?;
            ("text".to_string(), text)
        }
    };

    let text = normalize(&text);
    if text.trim().is_empty() {
        return Err(anyhow!("no extractable text found in {path}"));
    }
    Ok(Extracted {
        title,
        source_type,
        url: String::new(),
        text,
    })
}

/// Extract text from a spreadsheet (xlsx/xls/ods) — sheet by sheet, row by row.
fn extract_spreadsheet(path: &str) -> Result<String> {
    use calamine::{open_workbook_auto, Data, Reader};
    let mut workbook =
        open_workbook_auto(path).with_context(|| format!("failed to open spreadsheet {path}"))?;
    let mut out = String::new();
    for name in workbook.sheet_names() {
        let Ok(range) = workbook.worksheet_range(&name) else {
            continue;
        };
        if range.is_empty() {
            continue;
        }
        out.push_str(&format!("# Sheet: {name}\n"));
        for row in range.rows() {
            let cells: Vec<String> = row
                .iter()
                .map(|c| match c {
                    Data::Empty => String::new(),
                    other => other.to_string(),
                })
                .collect();
            if cells.iter().any(|c| !c.trim().is_empty()) {
                out.push_str(&cells.join(" | "));
                out.push('\n');
            }
        }
        out.push('\n');
    }
    Ok(out)
}

/// Read a single entry from a zip (Office files are zip archives).
fn read_zip_entry(path: &str, name: &str) -> Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).with_context(|| format!("failed to open {path}"))?;
    let mut zip = zip::ZipArchive::new(file).context("not a valid Office (zip) file")?;
    let mut entry = zip
        .by_name(name)
        .with_context(|| format!("{name} not found in archive"))?;
    let mut s = String::new();
    entry.read_to_string(&mut s)?;
    Ok(s)
}

/// Extract text from a .docx (WordprocessingML).
fn extract_docx(path: &str) -> Result<String> {
    let xml = read_zip_entry(path, "word/document.xml")?;
    // Paragraph and break boundaries become newlines; then strip all tags.
    let xml = xml
        .replace("</w:p>", "\n")
        .replace("<w:br/>", "\n")
        .replace("<w:tab/>", "\t");
    Ok(strip_html(&xml))
}

/// Extract text from a .pptx (PresentationML), one slide at a time, in order.
fn extract_pptx(path: &str) -> Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).with_context(|| format!("failed to open {path}"))?;
    let mut zip = zip::ZipArchive::new(file).context("not a valid .pptx file")?;

    // Collect slide entries with their numeric index for correct ordering.
    let mut slides: Vec<(u32, String)> = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            let num: u32 = name
                .trim_start_matches("ppt/slides/slide")
                .trim_end_matches(".xml")
                .parse()
                .unwrap_or(0);
            let mut xml = String::new();
            entry.read_to_string(&mut xml)?;
            slides.push((num, xml));
        }
    }
    slides.sort_by_key(|(n, _)| *n);

    let mut out = String::new();
    for (n, xml) in slides {
        let xml = xml.replace("</a:p>", "\n");
        let text = strip_html(&xml);
        if !text.trim().is_empty() {
            out.push_str(&format!("# Slide {n}\n{}\n\n", text.trim()));
        }
    }
    Ok(out)
}

/// Fetch a URL and strip it down to readable text (naive tag removal).
pub async fn extract_url(raw_url: &str) -> Result<Extracted> {
    let url = normalize_url(raw_url);

    let client = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Alchemy/0.1 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")?;

    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("{url} returned HTTP {}", status.as_u16()));
    }
    let body = resp.text().await.context("failed to read response body")?;

    let (article_title, text) = readable_text(&body, &url);
    if text.trim().is_empty() {
        return Err(anyhow!(
            "no readable text found at {url} (the page may be JavaScript-rendered)"
        ));
    }
    let title = article_title
        .or_else(|| extract_title(&body))
        .unwrap_or_else(|| url.clone());
    Ok(Extracted {
        title,
        source_type: "url".to_string(),
        url,
        text,
    })
}

/// Readability-style article extraction (drops nav, footers, comments, hidden
/// elements) with a plain tag-strip fallback for pages that don't look like
/// articles (dashboards, listings, bot walls). Returns the article title, if
/// one was found, alongside the text.
fn readable_text(body: &str, url: &str) -> (Option<String>, String) {
    let cfg = dom_smoothie::Config {
        text_mode: dom_smoothie::TextMode::Formatted,
        ..Default::default()
    };
    let article = dom_smoothie::Readability::new(body, Some(url), Some(cfg))
        .ok()
        .and_then(|mut r| r.parse().ok());
    if let Some(article) = article {
        let text = normalize(&article.text_content);
        // Same threshold as looks_blocked: shorter than this means the
        // article extraction probably picked the wrong (or no) node, so
        // whole-page extraction is the safer bet.
        if text.chars().count() >= 200 {
            let title = Some(article.title.trim().to_string()).filter(|t| !t.is_empty());
            return (title, text);
        }
    }
    (None, normalize(&strip_html(body)))
}

/// Heuristic: does this extracted text look like a bot wall / login page /
/// JS-only shell rather than real article content? Returns a reason if so.
pub fn looks_blocked(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let chars = trimmed.chars().count();
    if chars < 200 {
        return Some(format!(
            "Only {chars} characters extracted — the page may require login, block bots, or render with JavaScript."
        ));
    }
    let lower = trimmed.to_lowercase();
    const MARKERS: &[&str] = &[
        "enable javascript",
        "verify you are human",
        "are you a robot",
        "checking your browser",
        "just a moment",
        "access denied",
        "captcha",
        "sign in to continue",
        "log in to continue",
        "please log in",
    ];
    if let Some(m) = MARKERS.iter().find(|m| lower.contains(**m)) {
        return Some(format!("The page looks blocked or gated (\"{m}\")."));
    }
    None
}

/// Add a scheme if the user typed a bare host like "example.com/article".
pub fn normalize_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

/// Build a source directly from pasted text.
pub fn extract_pasted(title: &str, text: &str) -> Result<Extracted> {
    let text = normalize(text);
    if text.trim().is_empty() {
        return Err(anyhow!("pasted text is empty"));
    }
    let title = if title.trim().is_empty() {
        "Pasted text".to_string()
    } else {
        title.trim().to_string()
    };
    Ok(Extracted {
        title,
        source_type: "text".to_string(),
        url: String::new(),
        text,
    })
}

/// Split normalized text into overlapping word-window chunks.
pub fn chunk_text(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![];
    }
    if words.len() <= CHUNK_WORDS {
        return vec![words.join(" ")];
    }

    let mut chunks = Vec::new();
    let step = CHUNK_WORDS - OVERLAP_WORDS;
    let mut start = 0;
    while start < words.len() {
        let end = (start + CHUNK_WORDS).min(words.len());
        chunks.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        start += step;
    }
    chunks
}

fn normalize(text: &str) -> String {
    // Collapse runs of whitespace while preserving paragraph breaks.
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
        } else {
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

fn strip_html(html: &str) -> String {
    // Drop comments, script/style blocks, and elements marked hidden, then
    // remove all remaining tags. Operates on char boundaries throughout so
    // Unicode pages can't trigger a slice panic. Tag names are ASCII, so
    // case-insensitive comparison is done byte-wise (avoids `to_lowercase`,
    // which can shift byte offsets).
    let mut cleaned = String::with_capacity(html.len());
    let len = html.len();
    let mut i = 0; // always a char boundary
    while i < len {
        let rest = &html[i..];
        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => {
                    i += end + 3;
                    cleaned.push(' ');
                    continue;
                }
                None => break,
            }
        }
        if starts_with_ci(rest, "<script") || starts_with_ci(rest, "<style") {
            let close = if starts_with_ci(rest, "<script") {
                "</script>"
            } else {
                "</style>"
            };
            match find_ci(rest, close) {
                Some(end) => {
                    i += end + close.len();
                    continue;
                }
                None => break,
            }
        }
        let ch = rest.chars().next().unwrap();
        if ch == '<' {
            match rest.find('>') {
                Some(end) => {
                    if let Some(skip) = hidden_element_end(rest, &rest[1..end], end + 1) {
                        i += skip;
                    } else {
                        i += end + 1;
                    }
                    cleaned.push(' ');
                    continue;
                }
                None => break,
            }
        }
        cleaned.push(ch);
        i += ch.len_utf8();
    }
    collapse_blank_lines(&decode_entities(&cleaned))
}

/// If `tag` (the text between '<' and '>') opens an element marked hidden,
/// return the offset in `rest` just past its matching close tag. `rest` starts
/// at the element's '<'; `after_open` is the offset just past its opening '>'.
/// Returns None for visible, self-closing, void, or unclosed elements (the
/// caller then drops only the tag itself).
fn hidden_element_end(rest: &str, tag: &str, after_open: usize) -> Option<usize> {
    if tag.starts_with('/') || tag.ends_with('/') || !tag_is_hidden(tag) {
        return None;
    }
    let name: String = tag
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    if name.is_empty() || is_void_element(&name) {
        return None;
    }
    let open = format!("<{name}");
    let close = format!("</{name}");
    let mut depth = 1usize;
    let mut i = after_open;
    while i < rest.len() {
        let lt = rest[i..].find('<')? + i;
        let at = &rest[lt..];
        if starts_with_ci(at, &close) && !next_is_alnum(at, close.len()) {
            let gt = at.find('>')? + lt + 1;
            depth -= 1;
            if depth == 0 {
                return Some(gt);
            }
            i = gt;
        } else if starts_with_ci(at, &open) && !next_is_alnum(at, open.len()) {
            let gt = at.find('>')? + lt + 1;
            if !rest[lt..gt - 1].ends_with('/') {
                depth += 1;
            }
            i = gt;
        } else {
            i = lt + 1;
        }
    }
    None
}

/// Cheap check for markup that hides an element: inline display/visibility,
/// the bare `hidden` attribute, or aria-hidden="true".
fn tag_is_hidden(tag: &str) -> bool {
    let squished: String = tag
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    if squished.contains("display:none")
        || squished.contains("visibility:hidden")
        || squished.contains("aria-hidden=\"true\"")
        || squished.contains("aria-hidden='true'")
    {
        return true;
    }
    tag.split_whitespace()
        .skip(1)
        .any(|t| t.eq_ignore_ascii_case("hidden") || t.to_ascii_lowercase().starts_with("hidden="))
}

fn is_void_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Is the byte at `idx` an ASCII alphanumeric? Used as a tag-name boundary
/// check so `<div` doesn't match `<divx`.
fn next_is_alnum(s: &str, idx: usize) -> bool {
    s.as_bytes()
        .get(idx)
        .is_some_and(|b| b.is_ascii_alphanumeric())
}

/// Collapse runs of blank (or whitespace-only) lines down to one blank line.
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_blank = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            pending_blank = !out.is_empty();
        } else {
            if pending_blank {
                out.push('\n');
                pending_blank = false;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// ASCII case-insensitive prefix check (safe on any UTF-8 input).
fn starts_with_ci(haystack: &str, prefix: &str) -> bool {
    let h = haystack.as_bytes();
    let p = prefix.as_bytes();
    h.len() >= p.len() && h[..p.len()].eq_ignore_ascii_case(p)
}

/// ASCII case-insensitive substring search; returns a byte offset (always a
/// char boundary because the needle is ASCII).
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&k| h[k..k + n.len()].eq_ignore_ascii_case(n))
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let close = lower[open_end..].find("</title>")? + open_end;
    let title = decode_entities(html[open_end..close].trim());
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_splits_and_overlaps() {
        assert!(chunk_text("").is_empty());
        let short = chunk_text("one two three");
        assert_eq!(short.len(), 1);

        let words = (0..900)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_text(&words);
        assert!(chunks.len() >= 3, "long text splits into multiple chunks");
        // Consecutive chunks overlap (last words of one appear in the next).
        let tail: Vec<&str> = chunks[0].split_whitespace().rev().take(5).collect();
        assert!(
            tail.iter().any(|w| chunks[1].contains(*w)),
            "chunks overlap"
        );
    }

    #[test]
    fn strip_html_is_unicode_safe_and_clean() {
        // Multi-byte content must not panic (regression: byte-index slicing).
        let html = "<p>Café ☕ — <b>büro</b> 日本語</p><script>var x = {a:1};</script>";
        let text = strip_html(html);
        assert!(text.contains("Café"));
        assert!(text.contains("日本語"));
        assert!(!text.contains("var x"), "script contents removed");
        assert!(!text.contains('<'), "tags removed");
    }

    #[test]
    fn strip_html_decodes_entities() {
        assert_eq!(strip_html("a &amp; b &lt;c&gt;").trim(), "a & b <c>");
    }

    #[test]
    fn strip_html_drops_comments_hidden_elements_and_extra_blanks() {
        let html = r#"<html><head><title>Dealer</title></head>
<body>
<!-- OFFICIAL FERRARI DEALER / Ferrari Silicon Valley -->
<p>Visible paragraph.</p>
<!--
<div class="save-bar"><span>Saved</span></div>
-->
<div style="display: none">Hidden inline style.</div>
<div hidden><p>Hidden attr block.</p></div>
<span aria-hidden="true">Decorative</span>
<input type="hidden" value="csrf-token">



<p>After many blank lines.</p>
<!-- unterminated comment swallows the rest
</body></html>"#;
        let text = strip_html(html);
        assert!(text.contains("Visible paragraph."));
        assert!(text.contains("After many blank lines."));
        assert!(!text.contains("-->"), "no comment delimiters: {text}");
        assert!(!text.contains("OFFICIAL FERRARI DEALER"));
        assert!(!text.contains("Saved"), "commented-out markup dropped");
        assert!(!text.contains("Hidden inline style."));
        assert!(!text.contains("Hidden attr block."));
        assert!(!text.contains("Decorative"), "aria-hidden dropped");
        assert!(!text.contains("csrf-token"));
        assert!(!text.contains("unterminated"));
        assert!(!text.contains("\n\n\n"), "blank runs collapsed: {text:?}");
    }

    #[test]
    fn readable_text_extracts_article_and_drops_boilerplate() {
        let para = "The quick brown fox jumps over the lazy dog near the riverbank at dawn, \
                    watching the water drift slowly past the old stone bridge into town.";
        let html = format!(
            r#"<html><head><title>Fox Story — Example News</title></head>
<body>
<nav><a href="/">Home</a> <a href="/about">About</a> <a href="/contact">Contact</a></nav>
<!-- OFFICIAL FERRARI DEALER / Ferrari Silicon Valley -->
<div hidden><span>Saved</span></div>
<article><h1>Fox Story</h1>
<p>{para}</p><p>{para}</p><p>{para}</p><p>{para}</p><p>{para}</p>
</article>
<footer>Copyright 2026 Example News. Privacy Policy. Terms of Service.</footer>
</body></html>"#
        );
        let (title, text) = readable_text(&html, "https://example.com/fox");
        assert!(text.contains("quick brown fox"));
        assert!(!text.contains("Privacy Policy"), "footer dropped: {text}");
        assert!(!text.contains("OFFICIAL FERRARI DEALER"));
        assert!(!text.contains("Saved"));
        assert!(!text.contains("-->"));
        assert!(title.is_some(), "article title extracted");
    }

    #[test]
    fn readable_text_falls_back_to_full_page_on_non_articles() {
        // Too little content for readability — the tag-strip fallback must
        // keep the page's text rather than returning nothing.
        let html = "<html><body><h1>Dashboard</h1><p>3 sources indexed.</p></body></html>";
        let (title, text) = readable_text(html, "https://example.com/app");
        assert!(text.contains("3 sources indexed."));
        assert!(title.is_none(), "fallback leaves title to extract_title");
    }

    #[test]
    fn strip_html_keeps_content_after_hidden_and_nested_hidden() {
        // Nested same-name tags inside a hidden element must not truncate
        // the visible content that follows it.
        let html = r#"<div hidden><div><span>inner</span></div></div><p>still here</p>"#;
        let text = strip_html(html);
        assert!(!text.contains("inner"));
        assert!(text.contains("still here"));

        // A hidden element that never closes falls back to dropping only the
        // tag, keeping the document readable.
        let text = strip_html("<div hidden>orphan <p>tail</p>");
        assert!(text.contains("tail"));
    }

    #[test]
    fn normalize_url_adds_scheme() {
        assert_eq!(normalize_url("example.com/x"), "https://example.com/x");
        assert_eq!(normalize_url("http://a.com"), "http://a.com");
        assert_eq!(normalize_url("  https://b.com  "), "https://b.com");
    }

    #[test]
    fn file_type_detection() {
        assert!(is_pdf("/a/b.PDF"));
        assert!(!is_pdf("/a/b.txt"));
        assert!(is_image("photo.JPEG"));
        assert!(is_image("scan.png"));
        assert!(!is_image("notes.md"));
    }

    #[test]
    fn extract_pasted_titles_and_rejects_empty() {
        assert!(extract_pasted("", "   ").is_err());
        let ex = extract_pasted("", "hello world").unwrap();
        assert_eq!(ex.title, "Pasted text");
        assert_eq!(ex.source_type, "text");
    }
}
