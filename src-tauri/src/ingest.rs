//! Source ingestion: pull plain text out of files/URLs and split it into
//! overlapping chunks suitable for embedding.

use anyhow::{anyhow, Context, Result};
use std::path::Path;

/// Roughly target ~280 words per chunk with ~40 words of overlap. Word-based
/// rather than token-based keeps it model-agnostic and good enough for RAG.
/// ALCHEMY_CHUNK_WORDS overrides the target — an eval-only knob for the
/// BEIR chunk-size sweep (read per call, not cached: the sweep sets it
/// between runs in one process). The app never sets it.
const OVERLAP_WORDS: usize = 40;

fn chunk_words() -> usize {
    std::env::var("ALCHEMY_CHUNK_WORDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(280)
}

#[derive(Debug)]
pub struct Extracted {
    pub title: String,
    pub source_type: String,
    /// Origin of the content: the URL for `url` sources, the local file path
    /// for file imports (stamped by the command layer), empty for pasted text.
    pub url: String,
    pub text: String,
    /// Embedded document authorship (see `file_author`); empty when absent.
    pub author: String,
    /// Lead image (og:image / twitter:image) for `url` sources; "" when the
    /// page has none. Powers the source gallery.
    pub image_url: String,
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
        "png"
            | "jpg"
            | "jpeg"
            | "jpe"
            | "webp"
            | "gif"
            | "bmp"
            | "tif"
            | "tiff"
            | "heic"
            | "heif"
            | "avif"
            | "ico"
            | "jp2"
    )
}

/// Source-code and config extensions ingested verbatim (no whitespace
/// normalization — indentation is structure) and chunked by `chunk_code`.
/// Prose formats (md/txt) deliberately stay on the document path.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "rb", "java", "kt", "kts", "swift",
    "c", "h", "cc", "cpp", "hpp", "hh", "m", "mm", "php", "sh", "bash", "zsh", "fish", "sql",
    "scala", "lua", "r", "ex", "exs", "erl", "zig", "nix", "proto", "graphql", "vue", "svelte",
    "css", "scss", "less", "toml", "yaml", "yml", "json", "jsonc", "hcl", "tf", "tfvars", "ini",
    "cfg", "conf", "env", "xml", "plist", "gradle", "cmake", "asm", "s", "d", "dart", "hs", "ml",
    "clj", "cljs", "el", "vim", "ps1", "bat", "cmd",
];

/// Extension-less files that are still code/config by convention.
const CODE_FILENAMES: &[&str] = &[
    "dockerfile",
    "makefile",
    "justfile",
    "rakefile",
    "gemfile",
    "procfile",
    "brewfile",
    "vagrantfile",
];

/// Is this path source code (or code-shaped config) that should skip prose
/// normalization and use the code chunker?
pub fn is_code_path(path: &str) -> bool {
    let p = Path::new(path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if CODE_EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }
    let name = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    CODE_FILENAMES.contains(&name.as_str())
}

/// Is this path a PDF?
pub fn is_pdf(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// Final path component for user-facing errors — the full path is noise in a toast.
fn err_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
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
///
/// Panic-contained at this boundary: extractor crates can panic (not error)
/// on malformed files — the PDF reader of the day panicked on "unexpected
/// encoding NULL" mid folder-import, and the unwound worker hung the import.
/// One guard here turns any extractor panic, for every current and future
/// format, into an ordinary failed source instead of a stuck app.
pub fn extract_file(path: &str) -> Result<Extracted> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| extract_file_inner(path)))
        .unwrap_or_else(|_| {
            Err(anyhow!(
                "failed to parse {path} — the file may be malformed or corrupt"
            ))
        })
}

fn extract_file_inner(path: &str) -> Result<Extracted> {
    let p = Path::new(path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut title = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();

    // Code reads verbatim: normalize() strips the indentation that makes
    // code legible and retrievable, and chunk_code needs real lines. The
    // filename keeps its extension — `db.rs` and `db.ts` are different files.
    if is_code_path(path) {
        let text = read_text_lossy(path)?.replace('\r', "");
        if text.trim().is_empty() {
            return Err(anyhow!("No readable text in {}", err_name(path)));
        }
        return Ok(Extracted {
            image_url: String::new(),
            author: String::new(),
            title: p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Untitled")
                .to_string(),
            source_type: "code".to_string(),
            url: String::new(),
            text,
        });
    }

    // RFC-import-pipeline §1: one extractor, markdown out. anydoc converts
    // the whole office/document family — including formats we never had
    // extractors for (doc, rtf, odt, odp, ppt) — to GFM, which is both the
    // retrieval text and the faithful render. PDFs deliberately keep the
    // pdf-inspector + OCR path (scanned PDFs need the vision fallback
    // anydoc refuses); markdown/html/plain text are already their own best
    // extraction; TSVs aren't an anydoc format and keep ours.
    let finish = |md: String| -> Result<Extracted> {
        let text = normalize(&tidy_markdown_tables(&md));
        if text.trim().is_empty() {
            return Err(anyhow!("No readable text in {}", err_name(path)));
        }
        Ok(Extracted {
            image_url: String::new(),
            author: file_author(path),
            title: title.clone(),
            source_type: "text".to_string(),
            url: String::new(),
            text,
        })
    };
    match anydoc::Format::from_path(std::path::Path::new(path)) {
        None | Some(anydoc::Format::Pdf) => {}
        // CSVs keep one fallback below: Excel-exported CSVs are often
        // Windows-1252, which anydoc (strict UTF-8) refuses but our lossy
        // reader absorbs. The trace line is how we know the fallback still
        // earns its keep.
        Some(anydoc::Format::Csv) => match anydoc::to_markdown(path) {
            Ok(md) if !md.trim().is_empty() => return finish(md),
            Ok(_) => crate::note!("anydoc fallback: empty output for {path}"),
            Err(err) => crate::note!("anydoc fallback: {path}: {err}"),
        },
        // The office family is anydoc's alone (the bespoke extractors are
        // gone) — a failure here is the import's failure, reported as such.
        Some(_) => {
            let md =
                anydoc::to_markdown(path).with_context(|| format!("could not extract {path}"))?;
            return finish(md);
        }
    }

    let (source_type, text) = match ext.as_str() {
        "html" | "htm" | "xhtml" => {
            // Saved pages run through the same readability extraction as
            // fetched URLs — article body out, nav and boilerplate dropped —
            // and take the document's own title over the filename.
            let body = read_text_lossy(path)?;
            let (doc_title, text) = readable_text(&body, &format!("file://{path}"));
            // Readability found no article title? The <title> tag still beats
            // the filename stem.
            if let Some(t) = doc_title
                .or_else(|| extract_title(&body))
                .filter(|t| !t.trim().is_empty())
            {
                title = t;
            }
            ("html".to_string(), text)
        }
        "pdf" => {
            // Panic containment for a malformed PDF lives on `extract_file`.
            // pdf-inspector lays the content stream out in reading order as
            // Markdown, so headings and tables reach the chunker intact.
            let extracted = crate::pdf::extract_text(path)?;
            if extracted.is_scanned() {
                // Not an error the user can act on — the caller catches this
                // and rasterizes the pages through the vision model instead.
                return Err(anyhow!(
                    "No selectable text in {}; it looks like a scanned PDF.",
                    err_name(path)
                ));
            }
            ("pdf".to_string(), extracted.markdown())
        }
        "md" | "markdown" => (
            "markdown".to_string(),
            std::fs::read_to_string(path).context("failed to read markdown file")?,
        ),
        // csv reaches here only when anydoc refused it (see above) — the
        // lossy read absorbs the Windows-1252 exports anydoc can't.
        "csv" | "tsv" => {
            let delim = if ext == "csv" { ',' } else { '\t' };
            (
                "text".to_string(),
                tidy_markdown_tables(&delimited_to_rows(&read_text_lossy(path)?, delim)),
            )
        }
        "boxnote" => ("markdown".to_string(), extract_boxnote(path)?),
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
        return Err(anyhow!("No readable text in {}", err_name(path)));
    }
    Ok(Extracted {
        image_url: String::new(),
        // Stamped here — the one chokepoint every local-file ingest passes
        // through — so refresh and folder resync re-capture it for free.
        author: file_author(path),
        title,
        source_type,
        url: String::new(),
        text,
    })
}

/// Drop table columns and rows that hold nothing. Spreadsheet exports pad
/// rows with trailing delimiters, and those phantom columns render as a
/// strip of empty cells down the table's right edge (observed live: a
/// brokerage CSV with four of them). Applied to EXTRACTED output only —
/// a markdown file the user wrote renders exactly as written.
fn tidy_markdown_tables(text: &str) -> String {
    let is_row = |l: &str| {
        let t = l.trim();
        t.len() >= 2 && t.starts_with('|') && t.ends_with('|')
    };
    let is_sep_cell = |c: &str| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':'));
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if !is_row(lines[i]) {
            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && is_row(lines[i]) {
            i += 1;
        }
        let rows: Vec<Vec<&str>> = lines[start..i]
            .iter()
            .map(|l| {
                let t = l.trim();
                t[1..t.len() - 1].split('|').map(str::trim).collect()
            })
            .collect();
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        // A column lives if any non-separator row has something in it.
        let keep: Vec<bool> = (0..width)
            .map(|col| {
                rows.iter()
                    .any(|r| r.get(col).is_some_and(|c| !c.is_empty() && !is_sep_cell(c)))
            })
            .collect();
        for (ri, r) in rows.iter().enumerate() {
            let cells: Vec<&str> = (0..width)
                .filter(|c| keep[*c])
                .map(|c| r.get(c).copied().unwrap_or(""))
                .collect();
            let is_sep_row = r.iter().any(|c| is_sep_cell(c));
            // The header row is whatever sits directly above the separator —
            // GFM grammar demands it, blank or not. Dropping a blank header
            // left the separator first in line, which un-tables the whole
            // block into paragraph soup (observed live on a CSV whose first
            // line is a title, so anydoc emits an empty header).
            let is_header = rows
                .get(ri + 1)
                .is_some_and(|next| next.iter().any(|c| is_sep_cell(c)));
            // Fully blank rows (spacer lines in exports) vanish with the
            // phantom columns.
            if cells.is_empty() || (!is_sep_row && !is_header && cells.iter().all(|c| c.is_empty()))
            {
                continue;
            }
            out.push(format!("| {} |", cells.join(" | ")));
        }
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// A cell, made safe for a markdown table: pipes escaped, line breaks
/// flattened — a cell that closes its row early shears the whole column
/// grid sideways.
fn md_cell(cell: &str) -> String {
    cell.replace(['\r', '\n'], " ")
        .replace('|', "\\|")
        .trim()
        .to_string()
}

fn push_md_row(out: &mut String, cells: &[String], width: usize) {
    out.push('|');
    for i in 0..width {
        out.push(' ');
        out.push_str(&md_cell(cells.get(i).map(String::as_str).unwrap_or("")));
        out.push_str(" |");
    }
    out.push('\n');
}

/// Rows → a GitHub-flavored markdown table, first row as the header (the
/// way spreadsheets mean it). Valid GFM — header plus separator — is what
/// makes the reader paint a real table instead of pipe-riddled prose, and
/// the same pipes still read fine as plain text for retrieval. Ragged rows
/// are padded to the widest row so no column shears.
fn rows_to_markdown_table(rows: &[Vec<String>]) -> String {
    let Some(first) = rows.first() else {
        return String::new();
    };
    let width = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let mut out = String::new();
    push_md_row(&mut out, first, width);
    out.push('|');
    for _ in 0..width {
        out.push_str(" --- |");
    }
    out.push('\n');
    for row in &rows[1..] {
        push_md_row(&mut out, row, width);
    }
    out
}

/// Read a file as UTF-8, replacing invalid bytes. Excel-exported CSVs are
/// often Windows-1252 — importing with a few replacement characters beats
/// failing the whole file.
fn read_text_lossy(path: &str) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("failed to read {path}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Convert delimiter-separated text (CSV/TSV) into a markdown table.
/// The csv crate handles RFC 4180 quoting, CRLF, and ragged rows.
fn delimited_to_rows(text: &str, delim: char) -> String {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim as u8)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let rows: Vec<Vec<String>> = rdr
        .records()
        .flatten()
        .map(|rec| rec.iter().map(str::to_string).collect())
        .filter(|cells: &Vec<String>| cells.iter().any(|c| !c.trim().is_empty()))
        .collect();
    rows_to_markdown_table(&rows)
}

/// Read a single entry from a zip (Office files are zip archives).
/// Embedded authorship for a local file, best-effort: PDF /Author via
/// PDFium, Office documents' OPC docProps/core.xml dc:creator (docx, xlsx,
/// pptx share the format), EXIF Artist for images. Empty when the format has
/// no author concept or the field is blank — callers store it as-is.
pub fn file_author(path: &str) -> String {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let author = match ext.as_str() {
        "pdf" => crate::pdf::pdf_author(path),
        "docx" | "xlsx" | "xlsm" | "pptx" => read_zip_entry(path, "docProps/core.xml")
            .ok()
            .and_then(|xml| tag_text(&xml, "dc:creator")),
        _ if is_image(path) => exif_artist(path),
        _ => None,
    };
    author.unwrap_or_default()
}

/// First <tag>…</tag> text content, entity-decoded enough for names.
fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = xml.find(&open)?;
    let body_at = xml[start..].find('>')? + start + 1;
    let end = xml[body_at..].find(&close)? + body_at;
    let v = xml[body_at..end]
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .trim()
        .to_string();
    (!v.is_empty()).then_some(v)
}

/// EXIF Artist tag (the photographer/creator field cameras and editors write).
fn exif_artist(path: &str) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif.get_field(exif::Tag::Artist, exif::In::PRIMARY)?;
    let v = field
        .display_value()
        .to_string()
        .trim_matches('"')
        .trim()
        .to_string();
    (!v.is_empty()).then_some(v)
}

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

/// Extract text from a Box Note (`.boxnote`). Box Notes are JSON: the modern
/// editor (2022+) stores a ProseMirror document under `doc`; the original
/// Etherpad-derived editor stored the flat text under `atext.text`. Both are
/// parsed best-effort into markdown-ish plain text.
fn extract_boxnote(path: &str) -> Result<String> {
    let raw = read_text_lossy(path)?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("{path} is not a valid Box Note (JSON)"))?;

    // Modern format: { "doc": { "type": "doc", "content": [ … ] } } — a
    // ProseMirror tree. Walk it collecting text with block breaks.
    if let Some(doc) = v.get("doc").filter(|d| d.is_object()) {
        let text = boxnote_prosemirror_text(doc);
        if !text.trim().is_empty() {
            return Ok(text);
        }
    }

    // Legacy format: Etherpad-style { "atext": { "text": "…" }, "pool": … } —
    // the text field is already newline-delimited plain text.
    if let Some(text) = v
        .get("atext")
        .and_then(|a| a.get("text"))
        .and_then(|t| t.as_str())
        .filter(|t| !t.trim().is_empty())
    {
        return Ok(text.to_string());
    }

    Err(anyhow!(
        "No readable text in the Box Note {}",
        err_name(path)
    ))
}

/// Walk a Box Note ProseMirror document, concatenating text nodes and inserting
/// breaks at block boundaries so paragraphs, headings and list items survive.
fn boxnote_prosemirror_text(doc: &serde_json::Value) -> String {
    let mut out = String::new();
    boxnote_walk(doc, &mut out);
    out
}

fn boxnote_walk(node: &serde_json::Value, out: &mut String) {
    let ty = node.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        // Leaf text run.
        "text" => {
            if let Some(t) = node.get("text").and_then(|t| t.as_str()) {
                out.push_str(t);
            }
            return;
        }
        // Explicit in-paragraph line break.
        "hard_break" | "hardBreak" => {
            out.push('\n');
            return;
        }
        // A rule renders as nothing meaningful for retrieval.
        "horizontal_rule" | "horizontalRule" => {
            boxnote_break(out, "\n\n");
            return;
        }
        _ => {}
    }

    // Markdown-ish prefixes so structure reads (and chunks) sensibly.
    if ty == "heading" {
        let level = node
            .get("attrs")
            .and_then(|a| a.get("level"))
            .and_then(|l| l.as_u64())
            .unwrap_or(1)
            .clamp(1, 6) as usize;
        out.push_str(&"#".repeat(level));
        out.push(' ');
    }
    let is_list_item = matches!(
        ty,
        "list_item" | "listItem" | "check_list_item" | "checkListItem"
    );
    if is_list_item {
        out.push_str("- ");
    }

    if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
        for child in content {
            boxnote_walk(child, out);
        }
    }

    // Close the block. Paragraph-level nodes get a blank line; tighter
    // structures (list items, table rows) get a single newline.
    match ty {
        "paragraph" | "heading" | "code_block" | "codeBlock" | "blockquote" => {
            boxnote_break(out, "\n\n")
        }
        "list_item" | "listItem" | "check_list_item" | "checkListItem" | "table_row"
        | "tableRow" => boxnote_break(out, "\n"),
        _ => {}
    }
}

/// Append a block separator without piling blank lines onto an empty buffer or
/// one that already ends with the same break.
fn boxnote_break(out: &mut String, sep: &str) {
    if out.is_empty() || out.ends_with(sep) {
        return;
    }
    out.push_str(sep);
}

/// If `path` is a Dropbox Paper stub (`.paper`) that carries a link to the
/// online document, return that URL so it can be fetched like any web page —
/// the same treatment `.gdoc` placeholders get. Modern `.paper` files are
/// usually opaque online-only placeholders with no embedded URL; those return
/// None and the folder scan skips them with a clear reason.
pub fn dropbox_paper_url(path: &str) -> Option<String> {
    if !Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("paper"))
    {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    // JSON stub: a field holding the doc's dropbox.com URL.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
        for key in ["url", "link", "href", "web_url"] {
            if let Some(u) = v.get(key).and_then(|x| x.as_str()) {
                if is_dropbox_paper_url(u) {
                    return Some(u.trim().to_string());
                }
            }
        }
    }
    // Bare weblink stub (e.g. a .webloc-style `URL=…` line): the first Dropbox
    // Paper URL anywhere in the text.
    raw.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '=' | '('))
        .map(|tok| tok.trim_end_matches(['/', ',', ')']))
        .find(|tok| is_dropbox_paper_url(tok))
        .map(str::to_string)
}

/// A Dropbox Paper document URL: paper.dropbox.com/…, or a dropbox.com link
/// into a Paper doc (`/paper` path, or a modern `…Name.paper` share link).
fn is_dropbox_paper_url(u: &str) -> bool {
    let u = u.trim();
    u.starts_with("https://")
        && (u.contains("paper.dropbox.com")
            || (u.contains("dropbox.com") && (u.contains("/paper") || u.contains(".paper"))))
}

/// Fetch a URL and strip it down to readable text (naive tag removal).
pub async fn extract_url(raw_url: &str) -> Result<Extracted> {
    let url = normalize_url(raw_url);

    // A complete, self-consistent Chrome header set. Several listing sites
    // (e.g. carfax.com) reject requests whose headers don't look like a real
    // browser navigation; a bare or branded UA is the usual giveaway.
    // TLS-fingerprinting walls (Cloudflare et al.) still block regardless.
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"
            .parse()
            .unwrap(),
    );
    headers.insert(
        reqwest::header::ACCEPT_LANGUAGE,
        "en-US,en;q=0.9".parse().unwrap(),
    );
    headers.insert(
        "sec-ch-ua",
        "\"Google Chrome\";v=\"137\", \"Chromium\";v=\"137\", \"Not/A)Brand\";v=\"24\""
            .parse()
            .unwrap(),
    );
    headers.insert("sec-ch-ua-mobile", "?0".parse().unwrap());
    headers.insert("sec-ch-ua-platform", "\"macOS\"".parse().unwrap());
    headers.insert("Sec-Fetch-Dest", "document".parse().unwrap());
    headers.insert("Sec-Fetch-Mode", "navigate".parse().unwrap());
    headers.insert("Sec-Fetch-Site", "none".parse().unwrap());
    headers.insert("Sec-Fetch-User", "?1".parse().unwrap());
    headers.insert("Upgrade-Insecure-Requests", "1".parse().unwrap());

    let client = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36",
        )
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")?;

    // Google editor documents can't be scraped (JS-rendered), but every kind
    // has a public export endpoint that works for link-shared docs.
    if let Some((kind, export_url)) = google_export(&url) {
        return extract_google(&client, &url, kind, &export_url).await;
    }

    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;

    // The status is advisory, not gating: some sites serve the complete
    // article with a 500 (broken SSR that still renders — cerebras.ai).
    // Fetch the body regardless and let readability decide; give up when
    // there's nothing to read, or when a failing status comes with a body
    // that reads as the error page itself (`looks_like_error_page`).
    let status = resp.status();

    // Not every URL serves HTML. A link straight to a PDF (arxiv.org/pdf/...
    // is the everyday case) used to be decoded as lossy UTF-8 and fed to
    // readability, which produced an enormous source of binary garbage and
    // then hung the app chunking and embedding it. Sniff the type and hand
    // PDFs to the real reader.
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    if content_type.contains("application/pdf") || looks_like_pdf_url(&url) {
        let bytes = resp
            .bytes()
            .await
            .with_context(|| format!("could not download {url}"))?;
        if crate::pdf::looks_like_pdf(&bytes) {
            let extracted = crate::pdf::extract_text_mem(&bytes)?;
            if extracted.is_scanned() {
                return Err(anyhow!(
                    "No selectable text in this PDF; it looks like a scanned PDF."
                ));
            }
            return Ok(Extracted {
                author: String::new(),
                // Anything beats the URL's last path segment, which on arXiv
                // is a bare id ("2307.03172"): the /Title tag first, then the
                // biggest heading on page one.
                title: crate::pdf::pdf_title_mem(&bytes)
                    .or_else(|| extracted.guessed_title())
                    .unwrap_or_else(|| url_file_title(&url)),
                source_type: "pdf".to_string(),
                url,
                image_url: String::new(),
                text: normalize(&extracted.markdown()),
            });
        }
        // Advertised as a PDF but isn't one — fall through and read it as a
        // page, which is what the server actually sent.
        let body = String::from_utf8_lossy(&bytes).into_owned();
        return readable_page(body, status, url);
    }

    let body = resp.text().await.unwrap_or_default();
    readable_page(body, status, url)
}

/// Does this URL point at a PDF by its path? A backstop for servers that
/// mislabel the content type (`application/octet-stream` is common) — the
/// bytes are still checked for the magic number before anything is parsed.
fn looks_like_pdf_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.to_lowercase().ends_with(".pdf")
}

/// A readable title for a file served straight off a URL: the last path
/// segment without its extension, falling back to the URL itself.
fn url_file_title(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/')
        .find(|seg| !seg.is_empty())
        .map(|seg| seg.trim_end_matches(".pdf").to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| url.to_string())
}

/// A 4xx/5xx response whose body reads as the server's error page. Its own
/// type so the rendered-capture rescue (capture.rs) can recognize it and
/// stop: rendering a 404 layout yields the same 404 layout, only longer,
/// and the rescue's "strictly better than the fast path" rule would take it.
#[derive(Debug)]
pub struct HttpErrorPage {
    pub status: u16,
    pub url: String,
}

impl std::fmt::Display for HttpErrorPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} returned HTTP {} and the page is an error page, not the content",
            self.url, self.status
        )
    }
}

impl std::error::Error for HttpErrorPage {}

/// Does a failing response's extracted page read as the error page rather
/// than content that merely shipped with a broken status? The title is the
/// tell: a soft-deleted page keeps its whole layout — nav, footer, sidebars,
/// thousands of chars — around a "Page Not Found" heading, so length alone
/// can't catch it. A short body under a failing status is an error page too.
/// Only consulted once the status already says failure, so a real article
/// titled "404" served with a 200 is never touched.
pub fn looks_like_error_page(title: &str, text: &str) -> bool {
    const THIN: usize = 2_000;
    if text.trim().chars().count() < THIN {
        return true;
    }
    let lower = title.trim().to_lowercase();
    const MARKERS: &[&str] = &[
        "not found",
        "404",
        "410",
        "doesn't exist",
        "does not exist",
        "can't be found",
        "cannot be found",
        "no longer available",
        "something went wrong",
        "server error",
    ];
    lower.starts_with("error") || MARKERS.iter().any(|m| lower.contains(m))
}

/// The HTML arm of `extract_url`: readability over a fetched page body.
fn readable_page(body: String, status: reqwest::StatusCode, url: String) -> Result<Extracted> {
    let (article_title, text) = readable_text(&body, &url);
    if text.trim().is_empty() {
        if !status.is_success() {
            return Err(anyhow!("{url} returned HTTP {}", status.as_u16()));
        }
        return Err(anyhow!(
            "no readable text found at {url} (the page may be JavaScript-rendered)"
        ));
    }
    let title = article_title
        .or_else(|| extract_title(&body))
        .unwrap_or_else(|| url.clone());
    if !status.is_success() && looks_like_error_page(&title, &text) {
        return Err(HttpErrorPage {
            status: status.as_u16(),
            url,
        }
        .into());
    }
    let image_url = og_image(&body, &url).unwrap_or_default();
    Ok(Extracted {
        author: String::new(),
        title,
        source_type: "url".to_string(),
        url,
        text,
        image_url,
    })
}

/// Kinds of Google editor documents reachable via their export endpoints.
#[derive(Clone, Copy, PartialEq, Debug)]
enum GoogleDocKind {
    Doc,
    Sheet,
    Slides,
}

impl GoogleDocKind {
    fn product(self) -> &'static str {
        match self {
            GoogleDocKind::Doc => "Google Doc",
            GoogleDocKind::Sheet => "Google Sheet",
            GoogleDocKind::Slides => "Google Slides deck",
        }
    }
}

/// Detect a docs.google.com editor URL and build its export endpoint.
/// Export works without auth for documents shared "Anyone with the link".
fn google_export(url: &str) -> Option<(GoogleDocKind, String)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let mut segs = rest.split(['/', '?', '#']);
    if segs.next()? != "docs.google.com" {
        return None;
    }
    let kind = match segs.next()? {
        "document" => GoogleDocKind::Doc,
        "spreadsheets" => GoogleDocKind::Sheet,
        "presentation" => GoogleDocKind::Slides,
        _ => return None,
    };
    // Skip the optional account selector (`/u/0/`) to reach the `d/<id>` pair.
    let mut segs = segs.skip_while(|s| *s != "d");
    segs.next()?; // "d"
    let id = segs.next()?;
    // Published-to-web links (`/d/e/2PACX-…/pub`) have no export endpoint —
    // they are plain HTML, which the generic page scraper handles fine.
    if id == "e"
        || id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let export = match kind {
        GoogleDocKind::Doc => {
            format!("https://docs.google.com/document/d/{id}/export?format=txt")
        }
        GoogleDocKind::Sheet => {
            format!("https://docs.google.com/spreadsheets/d/{id}/export?format=xlsx")
        }
        GoogleDocKind::Slides => {
            format!("https://docs.google.com/presentation/d/{id}/export/txt")
        }
    };
    Some((kind, export))
}

/// Is this a Google editor URL we ingest via export (plain text, not scraped
/// HTML)? The bot-wall heuristics don't apply to these sources.
pub fn is_google_doc_url(url: &str) -> bool {
    google_export(url).is_some()
}

/// If `path` is a Google Drive desktop placeholder (.gdoc/.gsheet/.gslides),
/// return the document's editor URL. These files are tiny JSON stubs — the
/// real content lives in Google's cloud and is fetched via the export path.
pub fn google_placeholder_url(path: &str) -> Option<String> {
    let product = match Path::new(path)
        .extension()?
        .to_str()?
        .to_lowercase()
        .as_str()
    {
        "gdoc" => "document",
        "gsheet" => "spreadsheets",
        "gslides" => "presentation",
        _ => return None,
    };
    placeholder_doc_url(product, &std::fs::read_to_string(path).ok()?)
}

/// Parse a placeholder's JSON into an editor URL. Newer stubs carry `doc_id`;
/// older ones a `url` of the form `…/open?id=<id>`.
fn placeholder_doc_url(product: &str, json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let id = v
        .get("doc_id")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let url = v.get("url")?.as_str()?;
            let (_, after) = url.split_once("id=")?;
            let id = after.split('&').next().unwrap_or(after);
            (!id.is_empty()).then(|| id.to_string())
        })?;
    Some(format!("https://docs.google.com/{product}/d/{id}/edit"))
}

/// Fetch a Google Doc/Sheet/Slides via its export endpoint.
async fn extract_google(
    client: &reqwest::Client,
    original_url: &str,
    kind: GoogleDocKind,
    export_url: &str,
) -> Result<Extracted> {
    let denied = || {
        anyhow!(
            "This {} isn't accessible — it may be private or deleted. If it's yours, \
             set sharing to \"Anyone with the link\" and try again.",
            kind.product()
        )
    };
    let resp = client
        .get(export_url)
        .send()
        .await
        .with_context(|| format!("could not reach {export_url}"))?;
    // Private docs redirect the export endpoint to a Google sign-in page.
    if resp
        .url()
        .host_str()
        .is_some_and(|h| h.contains("accounts.google"))
    {
        return Err(denied());
    }
    let status = resp.status();
    if matches!(status.as_u16(), 401 | 403 | 404) {
        return Err(denied());
    }
    if !status.is_success() {
        return Err(anyhow!("{export_url} returned HTTP {}", status.as_u16()));
    }
    // The export filename carries the document's real title.
    let title = title_from_content_disposition(resp.headers())
        .unwrap_or_else(|| kind.product().to_string());

    let text = match kind {
        GoogleDocKind::Sheet => {
            let bytes = resp
                .bytes()
                .await
                .context("failed to download spreadsheet")?;
            // Same GFM the file path produces — anydoc converts the export
            // in memory (RFC-import-pipeline §1), phantom columns trimmed.
            tidy_markdown_tables(
                &anydoc::to_markdown_bytes(&bytes, anydoc::Format::Excel)
                    .map_err(|e| anyhow!("could not parse the exported spreadsheet: {e}"))?,
            )
        }
        _ => resp.text().await.context("failed to read export body")?,
    };
    let text = normalize(&text);
    if text.trim().is_empty() {
        return Err(anyhow!("this {} exported no text", kind.product()));
    }
    Ok(Extracted {
        image_url: String::new(),
        author: String::new(),
        title,
        source_type: "url".to_string(),
        url: original_url.to_string(),
        text,
    })
}

/// Pull the filename out of a Content-Disposition header, minus its extension.
fn title_from_content_disposition(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let value = headers
        .get(reqwest::header::CONTENT_DISPOSITION)?
        .to_str()
        .ok()?;
    // Prefer the RFC 5987 UTF-8 form; fall back to the quoted filename.
    let name = value
        .split(';')
        .find_map(|p| {
            p.trim().strip_prefix("filename*=UTF-8''").map(|f| {
                percent_encoding::percent_decode_str(f)
                    .decode_utf8_lossy()
                    .into_owned()
            })
        })
        .or_else(|| {
            value.split(';').find_map(|p| {
                p.trim()
                    .strip_prefix("filename=")
                    .map(|f| f.trim_matches('"').to_string())
            })
        })?;
    let stem = name
        .rsplit_once('.')
        .map(|(s, _)| s.to_string())
        .unwrap_or(name);
    let stem = stem.trim().to_string();
    (!stem.is_empty()).then_some(stem)
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
        // Prefer the article's HTML converted to markdown: headings, lists,
        // tables, emphasis, and LINKS survive — links are what the reader's
        // wiki-jumping and the backlink graph are built from. Fall back to
        // the plain text extraction when conversion fails or comes up short.
        let markdown = htmd::convert(&article.content)
            .ok()
            .map(|md| tidy_markdown(&md))
            .filter(|md| md.chars().count() >= 200);
        let text = markdown.unwrap_or_else(|| normalize(&article.text_content));
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

/// Page metadata the webview capture recovers from the live DOM (meta tags
/// + JSON-LD) that static readability can't always see.
#[derive(Default)]
pub struct PageMeta {
    pub og_title: String,
    pub byline: String,
    pub published: String,
    pub og_image: String,
}

/// Build an `Extracted` from already-rendered HTML — the webview capture
/// path (capture.rs). Same readability pipeline as fetched URLs and saved
/// pages; the live DOM's `document.title` and OpenGraph title fill in when
/// the markup carries no usable one (SPAs often set titles only via JS),
/// and byline/date become a one-line provenance header so retrieval knows
/// who wrote it and when.
pub fn extracted_from_html(html: &str, url: &str, dom_title: &str, meta: &PageMeta) -> Extracted {
    let (article_title, text) = readable_text(html, url);
    let title = article_title
        .or_else(|| extract_title(html))
        .or_else(|| Some(meta.og_title.trim().to_string()).filter(|t| !t.is_empty()))
        .or_else(|| Some(dom_title.trim().to_string()).filter(|t| !t.is_empty()))
        .unwrap_or_else(|| url.to_string());
    let text = match provenance_line(meta) {
        Some(line) if !text.trim().is_empty() => format!("{line}\n\n{text}"),
        _ => text,
    };
    let image_url = Some(meta.og_image.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| og_image(html, url))
        .unwrap_or_default();
    Extracted {
        author: String::new(),
        title,
        source_type: "url".to_string(),
        url: url.to_string(),
        text,
        image_url,
    }
}

/// `> By Jane Doe · Published 2024-03-12` — compact, only when known.
/// ISO timestamps are trimmed to the date; junk-length bylines dropped.
fn provenance_line(meta: &PageMeta) -> Option<String> {
    let byline = meta.byline.split_whitespace().collect::<Vec<_>>().join(" ");
    let byline = (!byline.is_empty() && byline.chars().count() <= 80).then_some(byline.as_str());
    let published = meta.published.trim();
    let published = published
        .split_once('T')
        .map(|(d, _)| d)
        .unwrap_or(published);
    let published = (!published.is_empty() && published.chars().count() <= 32).then_some(published);
    let parts: Vec<String> = [
        byline.map(|b| format!("By {b}")),
        published.map(|p| format!("Published {p}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        return None;
    }
    Some(format!("> {}", parts.join(" · ")))
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
    blocked_marker(trimmed)
}

/// Marker-only variant of [`looks_blocked`], without the minimum-length
/// heuristic — for text that came from an authoritative export (a tiny public
/// Google Sheet is not a blocked page) but could still be an interstitial.
pub fn blocked_marker(text: &str) -> Option<String> {
    let lower = text.trim().to_lowercase();
    const MARKERS: &[&str] = &[
        "enable javascript",
        "verify you are human",
        "are you a robot",
        "checking your browser",
        "just a moment",
        "attention required",
        "access denied",
        "captcha",
        "sign in to continue",
        "log in to continue",
        "please log in",
        "you need access",
        "request access",
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
        image_url: String::new(),
        author: String::new(),
        title,
        source_type: "text".to_string(),
        url: String::new(),
        text,
    })
}

/// A chunk ready for storage. `text` is the verbatim slice of the source —
/// it's what gets stored, shown as a citation snippet, and matched for
/// click-to-highlight. `embed_text` is the same text prefixed with document
/// and section context, so the vector carries topical signal (which doc,
/// which section) that the raw words may lack.
pub struct Chunk {
    pub text: String,
    pub embed_text: String,
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

/// Split normalized text into structure-aware chunks: whole paragraphs are
/// packed up to ~chunk_words(), markdown-style headings start a new chunk and
/// become section context, and oversized paragraphs fall back to sentence
/// (then word-window) splitting.
pub fn chunk_text(title: &str, text: &str) -> Vec<Chunk> {
    let make = |heading: &str, body: &str| -> Chunk {
        let mut ctx = title.trim().to_string();
        if !heading.is_empty() {
            if !ctx.is_empty() {
                ctx.push_str(" › ");
            }
            ctx.push_str(heading);
        }
        let body = body.trim().to_string();
        // Embed text de-brackets [[wikilinks]] so retrieval reads prose;
        // display text keeps them for the reader to render as links.
        let embed_body = debracket_wikilinks(&body);
        let embed_text = if ctx.is_empty() {
            embed_body
        } else {
            format!("[{ctx}]\n{embed_body}")
        };
        Chunk {
            text: body,
            embed_text,
        }
    };

    let mut chunks: Vec<Chunk> = Vec::new();
    let mut heading = String::new(); // current section heading
    let mut cur = String::new(); // paragraphs packed into the pending chunk
    let mut cur_words = 0usize;
    let mut cur_heading = String::new(); // section the pending chunk started in

    for para in text.split("\n\n") {
        let p = para.trim();
        if p.is_empty() {
            continue;
        }
        let words = word_count(p);

        // Markdown-style heading (including the "# Sheet:" / "# Slide N"
        // markers our extractors emit): new section, new chunk.
        if p.lines().count() == 1 && p.starts_with('#') {
            if !cur.is_empty() {
                chunks.push(make(&cur_heading, &cur));
                cur.clear();
            }
            heading = p.trim_start_matches('#').trim().to_string();
            cur_heading = heading.clone();
            cur.push_str(p); // the heading line stays in the chunk verbatim
            cur_words = words;
            continue;
        }

        // A single paragraph bigger than a whole chunk: flush what's pending
        // and split it by sentences (word windows as a last resort).
        if words > chunk_words() {
            if !cur.is_empty() {
                chunks.push(make(&cur_heading, &cur));
                cur.clear();
                cur_words = 0;
            }
            for piece in split_oversized(p) {
                chunks.push(make(&heading, &piece));
            }
            cur_heading = heading.clone();
            continue;
        }

        if cur_words + words > chunk_words() && !cur.is_empty() {
            chunks.push(make(&cur_heading, &cur));
            cur.clear();
            cur_words = 0;
        }
        if cur.is_empty() {
            cur_heading = heading.clone();
        } else {
            cur.push_str("\n\n");
        }
        cur.push_str(p);
        cur_words += words;
    }
    if !cur.trim().is_empty() {
        chunks.push(make(&cur_heading, &cur));
    }
    chunks
}

/// Split an oversized paragraph at sentence-ish boundaries, packing sentences
/// up to chunk_words(). A single run with no boundaries at all (minified text,
/// giant table row) falls back to overlapping word windows.
fn split_oversized(p: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_words = 0usize;
    for seg in p.split_inclusive(['.', '!', '?', '\n']) {
        let words = word_count(seg);
        if words > chunk_words() {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
                cur.clear();
                cur_words = 0;
            }
            out.extend(word_windows(seg));
            continue;
        }
        if cur_words + words > chunk_words() && !cur.trim().is_empty() {
            out.push(cur.trim().to_string());
            cur.clear();
            cur_words = 0;
        }
        cur.push_str(seg);
        cur_words += words;
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// url/html sources are page captures: low-density prose where a situating
/// sentence measurably helps retrieval (RFC-infinite-context §2) and where
/// nav cruft is worth keeping out of the vector space. pdf/markdown/docx/text
/// are clean prose with near-zero measured headroom, and code keeps its
/// path-prefix trick — none of those is a page capture. Mac (cider) sources
/// are structured Reminders/Calendar/Notes data, not captured pages, so they
/// stay on the plain path too.
pub fn is_page_capture_type(source_type: &str) -> bool {
    matches!(source_type, "url" | "html")
}

/// Is this chunk unmistakable navigation cruft — safe to keep out of the
/// vector index (RFC-infinite-context §2 boilerplate gate)? Only page-capture
/// chunks are ever tested; the verbatim text still lives in `source.content`,
/// so dropping it here never touches the reader or a citation. Deliberately
/// conservative: a chunk is junk only when it is short AND carries no
/// sentence, no heading structure, and no rare/identifier-ish token — anything
/// that could be real content keeps its slot.
pub fn is_boilerplate_chunk(chunk: &Chunk) -> bool {
    let text = chunk.text.trim();
    // Short: real passages run long; menus and breadcrumbs don't.
    if text.chars().count() >= 120 {
        return false;
    }
    // Any sentence punctuation (even a fragment) reads as content, not a link.
    if text.chars().any(|c| matches!(c, '.' | '!' | '?')) {
        return false;
    }
    // Heading structure: the chunk IS a heading line, or sits under a section
    // (its embed prefix carries "title › section"). Structure means keep.
    if text.starts_with('#') || chunk.embed_text.contains(" › ") {
        return false;
    }
    // A rare or identifier-ish token (a name, code, number, or long word)
    // marks real signal; a run of common short words is nav.
    if text
        .split(|c: char| c.is_whitespace() || "|·,:;()[]{}\"'`/".contains(c))
        .any(is_rare_token)
    {
        return false;
    }
    true
}

/// A token a navigation bar is unlikely to contain: it carries a digit, an
/// underscore/hyphen compound, internal capitalization, or is simply long.
/// Mirrors the identifier heuristic gists gate on, plus a length rule for rare
/// words. Short common words ("Home", "About", "Next") match none of these.
fn is_rare_token(t: &str) -> bool {
    let t = t.trim_matches(|c: char| ".:!?".contains(c));
    let n = t.chars().count();
    if n < 4 {
        return false;
    }
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    let compound = t.contains('_') || (t.contains('-') && !t.ends_with('-'));
    let mixed_case =
        t.chars().skip(1).any(|c| c.is_uppercase()) && t.chars().any(|c| c.is_lowercase());
    has_digit || compound || mixed_case || n >= 12
}

/// Chunk dispatch: code sources keep whitespace and split on block
/// boundaries; everything else uses the prose chunker. `code_ctx` is the
/// retrieval context for code chunks — "repo › relative/path.rs" when the
/// caller knows it (folder children), falling back to the title.
///
/// Markdown gets vault-aware prep (RFC-obsidian-notion §3): leading YAML
/// frontmatter is provenance, not prose — stripped from what's chunked, with
/// `tags:` joining the retrieval context the way repo paths do — and
/// `[[wikilinks]]` de-bracket in embed text so retrieval reads prose.
/// Display text keeps the brackets; the reader renders them as links.
/// Page-capture (url/html) sources additionally drop nav-cruft chunks from
/// the index (RFC-infinite-context §2).
pub fn chunk_source(extracted: &Extracted, code_ctx: Option<&str>) -> Vec<Chunk> {
    if extracted.source_type == "code" {
        return chunk_code(code_ctx.unwrap_or(&extracted.title), &extracted.text);
    }
    // PDFs arrive as Markdown too (pdf-inspector reconstructs headings, lists
    // and tables), so they take the same structure-aware path. Frontmatter
    // splitting is a no-op on them — a PDF does not open with `---`.
    if extracted.source_type == "markdown" || extracted.source_type == "pdf" {
        let (tags, body) = split_frontmatter(&extracted.text);
        let ctx = match tags.is_empty() {
            true => extracted.title.clone(),
            false => format!("{} · {}", extracted.title, tags.join(" ")),
        };
        return chunk_text(&ctx, body);
    }
    let chunks = chunk_text(&extracted.title, &extracted.text);
    if is_page_capture_type(&extracted.source_type) {
        chunks
            .into_iter()
            .filter(|c| !is_boilerplate_chunk(c))
            .collect()
    } else {
        chunks
    }
}

/// Split leading YAML frontmatter off markdown. Returns (`#tag`s from any
/// `tags:` key, body after the closing `---`). Not a YAML parser — it reads
/// the two shapes Obsidian writes (inline `[a, b]` and block `- a` lists)
/// and ignores everything else. Unclosed fences are treated as content.
pub(crate) fn split_frontmatter(text: &str) -> (Vec<String>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (vec![], text);
    };
    let Some(end) = rest.find("\n---") else {
        return (vec![], text);
    };
    let (yaml, mut body) = rest.split_at(end);
    body = body.strip_prefix("\n---").unwrap_or(body);
    body = body.strip_prefix('\n').unwrap_or(body);

    let mut tags: Vec<String> = Vec::new();
    let mut in_tags_block = false;
    for line in yaml.lines() {
        if in_tags_block {
            if let Some(item) = line.trim().strip_prefix("- ") {
                push_tag(&mut tags, item);
                continue;
            }
            in_tags_block = false;
        }
        if let Some(val) = line.strip_prefix("tags:") {
            let val = val.trim();
            if val.is_empty() {
                in_tags_block = true;
            } else {
                for item in val.trim_start_matches('[').trim_end_matches(']').split(',') {
                    push_tag(&mut tags, item);
                }
            }
        }
    }
    (tags, body)
}

fn push_tag(tags: &mut Vec<String>, raw: &str) {
    let t = raw.trim().trim_matches(['"', '\'']).trim_start_matches('#');
    if !t.is_empty() {
        tags.push(format!("#{t}"));
    }
}

/// `[[Note]]` → `Note`, `[[Note|alias]]` → `alias`, `[[Note#head]]` →
/// `Note head` — embed text reads as prose instead of bracket soup.
fn debracket_wikilinks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        out.push_str(&rest[..start]);
        let inner = &after[..end];
        let display = match inner.rsplit_once('|') {
            Some((_, alias)) if !alias.trim().is_empty() => alias.trim().to_string(),
            _ => inner.replace('#', " ").trim().to_string(),
        };
        out.push_str(&display);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// How many trailing lines of one code chunk repeat at the start of the next
/// when a single block is bigger than a whole chunk — continuity without
/// prose-style word overlap.
const CODE_OVERLAP_LINES: usize = 8;

/// Split code into chunks on blank-line block boundaries, packing blocks up
/// to ~chunk_words(). Text is verbatim — indentation intact, so citations show
/// real code — and `embed_text` carries a `[context]` path header, the
/// highest-leverage retrieval trick for code (exact file-name hits for BM25,
/// orientation for the embedder). Oversized blocks fall back to line windows,
/// never sentence splits.
pub fn chunk_code(context: &str, text: &str) -> Vec<Chunk> {
    let make = |body: &str| -> Chunk {
        let body = body.trim_end().to_string();
        let ctx = context.trim();
        let embed_text = if ctx.is_empty() {
            body.clone()
        } else {
            format!("[{ctx}]\n{body}")
        };
        Chunk {
            text: body,
            embed_text,
        }
    };

    // Group lines into blocks separated by blank lines.
    let mut blocks: Vec<(String, usize)> = Vec::new(); // (block text, word count)
    let mut cur = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                let words = word_count(&cur);
                blocks.push((std::mem::take(&mut cur), words));
            }
            continue;
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
    }
    if !cur.is_empty() {
        let words = word_count(&cur);
        blocks.push((cur, words));
    }

    let mut chunks: Vec<Chunk> = Vec::new();
    let mut pending = String::new();
    let mut pending_words = 0usize;
    for (block, words) in blocks {
        // A single block bigger than a whole chunk: flush and line-window it.
        if words > chunk_words() {
            if !pending.is_empty() {
                chunks.push(make(&pending));
                pending.clear();
                pending_words = 0;
            }
            for piece in line_windows(&block) {
                chunks.push(make(&piece));
            }
            continue;
        }
        if pending_words + words > chunk_words() && !pending.is_empty() {
            chunks.push(make(&pending));
            pending.clear();
            pending_words = 0;
        }
        if !pending.is_empty() {
            pending.push_str("\n\n");
        }
        pending.push_str(&block);
        pending_words += words;
    }
    if !pending.trim().is_empty() {
        chunks.push(make(&pending));
    }
    chunks
}

/// Split one oversized code block into line runs of ~chunk_words() with a few
/// lines of overlap for continuity.
fn line_windows(block: &str) -> Vec<String> {
    let lines: Vec<&str> = block.lines().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < lines.len() {
        let mut end = start;
        let mut words = 0usize;
        while end < lines.len() {
            let w = word_count(lines[end]);
            // Always take at least one line, however wide (minified guards
            // live upstream in the folder scan's size cap).
            if end > start && words + w > chunk_words() {
                break;
            }
            words += w;
            end += 1;
        }
        out.push(lines[start..end].join("\n"));
        if end == lines.len() {
            break;
        }
        start = end.saturating_sub(CODE_OVERLAP_LINES).max(start + 1);
    }
    out
}

/// Last-resort overlapping word windows for boundary-free text.
fn word_windows(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![];
    }
    if words.len() <= chunk_words() {
        return vec![words.join(" ")];
    }
    let mut chunks = Vec::new();
    let step = chunk_words() - OVERLAP_WORDS;
    let mut start = 0;
    while start < words.len() {
        let end = (start + chunk_words()).min(words.len());
        chunks.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        start += step;
    }
    chunks
}

/// Cleanup for converted markdown. Markdown is whitespace-significant
/// (nested lists, code blocks), so unlike `normalize` this keeps leading
/// indentation — it only trims line ends and collapses runs of blank lines.
fn tidy_markdown(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut blank = 0;
    for line in md.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(t);
        out.push('\n');
    }
    out.trim().to_string()
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
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | ':' | '_'))
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

/// Is the byte at `idx` a valid ASCII tag-name character? Used as a tag-name
/// boundary check so `<div` doesn't match `<divx` and `<my-element` doesn't
/// match `<my-element-extra`.
fn next_is_alnum(s: &str, idx: usize) -> bool {
    s.as_bytes()
        .get(idx)
        .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'_'))
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

/// Download an image (og:image cache fill): one GET, 8 MB cap, best-effort.
pub async fn fetch_image_bytes(url: &str) -> Option<Vec<u8>> {
    fetch_bytes(url, 8 * 1024 * 1024).await
}

/// One GET, capped. Shared by the image and PDF thumbnail backfills — a PDF
/// wants a far larger ceiling than an og:image, so the cap is the caller's.
pub async fn fetch_bytes(url: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let client = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.is_empty() || bytes.len() > max_bytes {
        return None;
    }
    Some(bytes.to_vec())
}

/// Fetch a page and return just its lead image — the gallery backfill path
/// for URL sources ingested before `image_url` existed. Lightweight on
/// purpose: one GET, meta-tag parse, no readability, no embedding.
pub async fn fetch_lead_image(url: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;
    let body = client.get(url).send().await.ok()?.text().await.ok()?;
    og_image(&body, url)
}

/// Every plausible lead-image candidate on a page, for the reader's manual
/// picker (the auto `og_image` pick misses some pages): meta og/twitter
/// images first, then body `<img>` sources (data-src lazy-load variants
/// included via the attribute scan), resolved against the page URL, with
/// obvious chrome (SVGs, sprites, favicons, pixels) dropped. Deduped,
/// capped, never fetches anything.
pub fn page_images(html: &str, base_url: &str) -> Vec<String> {
    const MAX_CANDIDATES: usize = 24;
    let cap = html
        .char_indices()
        .nth(1_000_000)
        .map(|(i, _)| i)
        .unwrap_or(html.len());
    let hay = &html[..cap];
    let lower = hay.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    if lower.len() != hay.len() {
        // See og_image: shared byte indices below need same-length lowering.
        out.extend(og_image(html, base_url));
        return out;
    }
    let base = reqwest::Url::parse(base_url).ok();
    let mut seen = std::collections::HashSet::new();
    let mut push = |raw: &str, out: &mut Vec<String>| {
        let candidate = decode_entities(raw.trim());
        if candidate.is_empty() || candidate.starts_with("data:") {
            return;
        }
        let resolved = base
            .as_ref()
            .and_then(|b| b.join(&candidate).ok())
            .map(|u| u.to_string())
            .unwrap_or(candidate);
        if !resolved.starts_with("http://") && !resolved.starts_with("https://") {
            return;
        }
        let l = resolved.to_lowercase();
        if l.ends_with(".svg")
            || ["sprite", "favicon", "pixel.", "1x1", "spacer"]
                .iter()
                .any(|w| l.contains(w))
        {
            return;
        }
        if seen.insert(l) {
            out.push(resolved);
        }
    };
    // Meta tags first — they're the page's own pick of a representative image.
    for key in ["<meta", "<img"] {
        let mut pos = 0;
        while out.len() < MAX_CANDIDATES {
            let Some(off) = lower[pos..].find(key) else {
                break;
            };
            let start = pos + off;
            let Some(end) = lower[start..].find('>').map(|e| start + e + 1) else {
                break;
            };
            pos = end;
            let tag = &hay[start..end];
            if key == "<meta" {
                const KEYS: &[&str] = &[
                    "og:image",
                    "og:image:url",
                    "og:image:secure_url",
                    "twitter:image",
                    "twitter:image:src",
                ];
                let name = meta_attr(tag, "property").or_else(|| meta_attr(tag, "name"));
                if !name.is_some_and(|k| KEYS.contains(&k.to_lowercase().as_str())) {
                    continue;
                }
                if let Some(content) = meta_attr(tag, "content") {
                    push(content, &mut out);
                }
            } else if let Some(src) = meta_attr(tag, "src") {
                push(src, &mut out);
            }
        }
    }
    out
}

/// The page's lead image from raw HTML meta tags — `og:image` (and its
/// `:url`/`:secure_url` variants) or `twitter:image`, first hit wins —
/// resolved against the page URL. Head-only scan; never fetches anything.
pub fn og_image(html: &str, base_url: &str) -> Option<String> {
    // Meta tags live in <head>; cap the scan so multi-MB bodies stay cheap.
    let cap = html
        .char_indices()
        .nth(300_000)
        .map(|(i, _)| i)
        .unwrap_or(html.len());
    let hay = &html[..cap];
    let lower = hay.to_lowercase();
    if lower.len() != hay.len() {
        // Rare lowercasing length change (e.g. İ) would break shared byte
        // indices below — skip rather than risk a mid-ingest panic.
        return None;
    }
    const KEYS: &[&str] = &[
        "og:image",
        "og:image:url",
        "og:image:secure_url",
        "twitter:image",
        "twitter:image:src",
    ];
    let mut pos = 0;
    while let Some(off) = lower[pos..].find("<meta") {
        let start = pos + off;
        let Some(end) = lower[start..].find('>').map(|e| start + e + 1) else {
            break;
        };
        pos = end;
        let tag = &hay[start..end];
        let key = meta_attr(tag, "property").or_else(|| meta_attr(tag, "name"));
        if !key.is_some_and(|k| KEYS.contains(&k.to_lowercase().as_str())) {
            continue;
        }
        let Some(content) = meta_attr(tag, "content") else {
            continue;
        };
        let candidate = decode_entities(content.trim());
        if candidate.is_empty() || candidate.starts_with("data:") {
            continue;
        }
        // Resolve protocol-relative and path-relative candidates.
        let resolved = reqwest::Url::parse(base_url)
            .ok()
            .and_then(|b| b.join(&candidate).ok())
            .map(|u| u.to_string())
            .unwrap_or(candidate);
        if resolved.starts_with("http://") || resolved.starts_with("https://") {
            return Some(resolved);
        }
    }
    None
}

/// A quoted attribute value out of one `<meta …>` tag, order-agnostic.
fn meta_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_lowercase();
    if lower.len() != tag.len() {
        return None; // see og_image: byte indices must stay shared
    }
    let mut search = 0;
    loop {
        let at = lower[search..].find(name)? + search;
        // Must be a standalone attribute name followed by `=`.
        let before_ok = at == 0
            || !lower.as_bytes()[at - 1].is_ascii_alphanumeric()
                && lower.as_bytes()[at - 1] != b':';
        let rest = &tag[at + name.len()..];
        let trimmed = rest.trim_start();
        if before_ok && trimmed.starts_with('=') {
            let after_eq = trimmed[1..].trim_start();
            let quote = after_eq.chars().next()?;
            if quote == '"' || quote == '\'' {
                let inner = &after_eq[1..];
                let close = inner.find(quote)?;
                return Some(&inner[..close]);
            }
            // Unquoted value: read to whitespace or tag end.
            let end = after_eq
                .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .unwrap_or(after_eq.len());
            return Some(&after_eq[..end]);
        }
        search = at + name.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn og_image_finds_property_and_name_variants() {
        let html = r#"<html><head>
            <meta property="og:title" content="T">
            <meta property="og:image" content="https://ex.com/hero.jpg">
        </head><body></body></html>"#;
        assert_eq!(
            og_image(html, "https://ex.com/page"),
            Some("https://ex.com/hero.jpg".to_string())
        );
        let twitter = r#"<meta name="twitter:image" content="https://ex.com/t.png">"#;
        assert_eq!(
            og_image(twitter, "https://ex.com/"),
            Some("https://ex.com/t.png".to_string())
        );
    }

    #[test]
    fn page_images_collects_meta_then_body_and_filters_chrome() {
        let html = r#"<html><head>
            <meta property="og:image" content="https://ex.com/hero.jpg">
        </head><body>
            <img src="/photos/a.png" alt="">
            <img src="https://ex.com/sprite-sheet.png">
            <img src="https://ex.com/logo.svg">
            <img src="data:image/gif;base64,R0lGOD">
            <img data-src="../b.webp" src="https://ex.com/1x1.gif">
            <img src="/photos/a.png">
        </body></html>"#;
        assert_eq!(
            page_images(html, "https://ex.com/articles/page"),
            vec![
                "https://ex.com/hero.jpg".to_string(),
                "https://ex.com/photos/a.png".to_string(),
                "https://ex.com/b.webp".to_string(),
            ]
        );
    }

    #[test]
    fn og_image_resolves_relative_and_skips_junk() {
        // Relative and protocol-relative candidates resolve against the page.
        let rel = r#"<meta property="og:image" content="/img/lead.png">"#;
        assert_eq!(
            og_image(rel, "https://ex.com/articles/1"),
            Some("https://ex.com/img/lead.png".to_string())
        );
        let proto = r#"<meta property="og:image" content="//cdn.ex.com/x.jpg">"#;
        assert_eq!(
            og_image(proto, "https://ex.com/"),
            Some("https://cdn.ex.com/x.jpg".to_string())
        );
        // og:image:width's numeric content must not win; data: URIs skipped.
        let junk = r#"<meta property="og:image:width" content="1200">
                      <meta property="og:image" content="data:image/png;base64,xxx">"#;
        assert_eq!(og_image(junk, "https://ex.com/"), None);
        // Entities in the content attribute decode.
        let ent = r#"<meta property="og:image" content="https://ex.com/a?b=1&amp;c=2">"#;
        assert_eq!(
            og_image(ent, "https://ex.com/"),
            Some("https://ex.com/a?b=1&c=2".to_string())
        );
    }

    #[test]
    fn code_paths_detected_by_extension_and_name() {
        assert!(is_code_path("/repo/src/db.rs"));
        assert!(is_code_path("/repo/src/lib/utils.ts"));
        assert!(is_code_path("/repo/Dockerfile"));
        assert!(is_code_path("/repo/Makefile"));
        assert!(is_code_path("/repo/config.toml"));
        assert!(!is_code_path("/repo/README.md"));
        assert!(!is_code_path("/repo/notes.txt"));
        assert!(!is_code_path("/repo/paper.pdf"));
        assert!(!is_code_path("/repo/LICENSE"));
    }

    #[test]
    fn chunk_code_preserves_whitespace_and_prefixes_context() {
        let code = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n\nfn helper() {\n    todo!()\n}\n";
        let chunks = chunk_code("alchemy › src/main.rs", code);
        assert_eq!(chunks.len(), 1);
        // Indentation survives verbatim in the citation text…
        assert!(chunks[0].text.contains("    let x = 1;"));
        // …blocks are joined with a single blank line…
        assert!(chunks[0].text.contains("}\n\nfn helper()"));
        // …and the embed text carries the path header while the citation
        // text stays clean.
        assert!(chunks[0]
            .embed_text
            .starts_with("[alchemy › src/main.rs]\nfn main()"));
        assert!(!chunks[0].text.starts_with('['));
    }

    #[test]
    fn chunk_code_splits_on_block_boundaries_at_budget() {
        // Many small blocks that can't all fit one chunk: splits happen at
        // blank lines, never mid-block.
        let block = "fn f() {\n    a_line_of_code();\n    another_line_here();\n}";
        let code = vec![block; 60].join("\n\n");
        let chunks = chunk_code("ctx", &code);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.text.starts_with("fn f()"));
            assert!(c.text.ends_with('}'));
        }
    }

    #[test]
    fn chunk_code_line_windows_oversized_blocks() {
        // One giant block with no blank lines falls back to line windows —
        // every chunk still holds whole lines.
        let code = (0..600)
            .map(|i| format!("    call_number_{i}(with, some, args);"))
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = chunk_code("ctx", &code);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.text.lines().all(|l| l.starts_with("    call_number_")));
        }
        // Overlap: the second chunk re-starts before the first one ended.
        let first_last: &str = chunks[0].text.lines().last().unwrap();
        let second_first: &str = chunks[1].text.lines().next().unwrap();
        let n = |l: &str| -> usize {
            l.trim_start()
                .trim_start_matches("call_number_")
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap()
        };
        assert!(n(second_first) <= n(first_last));
    }

    #[test]
    fn chunk_source_dispatches_on_source_type() {
        let code = Extracted {
            image_url: String::new(),
            author: String::new(),
            title: "main.rs".into(),
            source_type: "code".into(),
            url: String::new(),
            text: "fn main() {\n    body();\n}".into(),
        };
        let got = chunk_source(&code, None);
        assert!(got[0].text.contains("    body();"));
        assert!(got[0].embed_text.starts_with("[main.rs]\n"));

        let prose = Extracted {
            image_url: String::new(),
            author: String::new(),
            title: "Notes".into(),
            source_type: "text".into(),
            url: String::new(),
            text: "One paragraph of ordinary prose.".into(),
        };
        let got = chunk_source(&prose, None);
        assert!(got[0].embed_text.starts_with("[Notes]\n"));
    }

    /// Build the Chunk a page-capture chunker would produce for a bare body
    /// under `title` (title-only prefix, no section) — the boilerplate gate's
    /// worst case, where only the text itself can save the chunk.
    fn page_chunk(title: &str, body: &str) -> Chunk {
        Chunk {
            text: body.to_string(),
            embed_text: format!("[{title}]\n{body}"),
        }
    }

    #[test]
    fn boilerplate_gate_drops_nav_keeps_content() {
        // Pure nav: short, no sentence, no heading, all common words.
        assert!(is_boilerplate_chunk(&page_chunk(
            "Acme Blog",
            "Home About Products Services Contact Careers"
        )));
        // A short sentence fragment is content — punctuation saves it.
        assert!(!is_boilerplate_chunk(&page_chunk(
            "Acme Blog",
            "Read our latest pricing update."
        )));
        // A rare/identifier token (version code) marks signal.
        assert!(!is_boilerplate_chunk(&page_chunk(
            "Acme Blog",
            "Download release v2.4.1 arm64"
        )));
        // Heading context (section prefix) keeps the chunk even when short.
        assert!(!is_boilerplate_chunk(&Chunk {
            text: "Overview".into(),
            embed_text: "[Acme Blog › Docs]\nOverview".into(),
        }));
        // Long real passage never trips the gate.
        assert!(!is_boilerplate_chunk(&page_chunk(
            "Acme Blog",
            "The onboarding flow walks a new teammate through account setup, \
             workspace selection, and the first import before handing off"
        )));
    }

    #[test]
    fn boilerplate_gate_spares_clean_fixture_prose() {
        // The golden fixtures are clean article prose: the gate must drop
        // none of them when they are treated as page captures (§2 regression
        // fence — enrichment must never cost recall on clean sets).
        let mut dropped = 0usize;
        for (title, body) in crate::evals::CORPUS {
            for c in chunk_text(title, &normalize(body)) {
                if is_boilerplate_chunk(&c) {
                    dropped += 1;
                }
            }
        }
        assert_eq!(
            dropped, 0,
            "boilerplate gate dropped {dropped} clean chunks"
        );
    }

    #[test]
    fn provenance_line_formats_and_filters() {
        let meta = PageMeta {
            byline: "Jane  Doe".into(),
            published: "2024-03-12T10:00:00Z".into(),
            ..Default::default()
        };
        assert_eq!(
            provenance_line(&meta).as_deref(),
            Some("> By Jane Doe · Published 2024-03-12")
        );
        // Date only.
        let meta = PageMeta {
            published: "2023-01-05".into(),
            ..Default::default()
        };
        assert_eq!(
            provenance_line(&meta).as_deref(),
            Some("> Published 2023-01-05")
        );
        // Nothing known → no line; junk-length byline dropped.
        assert_eq!(provenance_line(&PageMeta::default()), None);
        let meta = PageMeta {
            byline: "x".repeat(200),
            ..Default::default()
        };
        assert_eq!(provenance_line(&meta), None);
    }

    #[test]
    fn extracted_from_html_titles_and_provenance() {
        let body = "Real content sentence. ".repeat(20);
        let html = format!("<html><body><div>{body}</div></body></html>");
        // No <title>, no og:title → live DOM title wins.
        let ex = extracted_from_html(&html, "https://e.com/a", "DOM Title", &PageMeta::default());
        assert_eq!(ex.title, "DOM Title");
        assert!(ex.text.contains("Real content"));
        // og:title beats the DOM title fallback.
        let meta = PageMeta {
            og_title: "OG Title".into(),
            byline: "Jane".into(),
            ..Default::default()
        };
        let ex = extracted_from_html(&html, "https://e.com/a", "DOM Title", &meta);
        assert_eq!(ex.title, "OG Title");
        assert!(ex.text.starts_with("> By Jane\n\n"), "got: {:.60}", ex.text);
        // Empty extraction never gets a dangling provenance header.
        let ex = extracted_from_html("<html></html>", "https://e.com/a", "", &meta);
        assert!(!ex.text.starts_with(">"));
    }

    #[test]
    fn frontmatter_splits_and_reads_both_tag_shapes() {
        // Inline list.
        let (tags, body) =
            split_frontmatter("---\ntitle: X\ntags: [espresso, gear]\n---\nBody here.");
        assert_eq!(tags, vec!["#espresso", "#gear"]);
        assert_eq!(body, "Body here.");

        // Block list, quoted and pre-hashed entries normalize.
        let (tags, body) =
            split_frontmatter("---\ntags:\n  - \"a\"\n  - '#b'\nother: y\n---\n\nText.");
        assert_eq!(tags, vec!["#a", "#b"]);
        assert_eq!(body, "\nText.");

        // No frontmatter, unclosed fence, and a mid-document rule all pass through.
        assert_eq!(split_frontmatter("plain text").1, "plain text");
        assert_eq!(
            split_frontmatter("---\nnever closed").1,
            "---\nnever closed"
        );
        let (tags, body) = split_frontmatter("---\nkey: v\n---\nA\n\n---\n\nB");
        assert!(tags.is_empty());
        assert_eq!(body, "A\n\n---\n\nB");
    }

    #[test]
    fn wikilinks_debracket_in_embed_text_only() {
        assert_eq!(
            debracket_wikilinks("See [[Roast Log]] and [[Gear#Grinder|the grinder]]."),
            "See Roast Log and the grinder."
        );
        assert_eq!(debracket_wikilinks("[[A#B]]"), "A B");
        // Unclosed brackets pass through untouched.
        assert_eq!(debracket_wikilinks("broken [[link"), "broken [[link");

        let chunks = chunk_text("Note", "Compare with [[Other Note|that one]].");
        assert_eq!(chunks[0].text, "Compare with [[Other Note|that one]].");
        assert!(chunks[0].embed_text.ends_with("Compare with that one."));
    }

    #[test]
    fn markdown_chunking_strips_frontmatter_and_carries_tags() {
        let ex = Extracted {
            image_url: String::new(),
            author: String::new(),
            title: "Dialing In".into(),
            text: "---\ntags: [espresso]\n---\nGrind finer when sour. See [[Temps]].".into(),
            source_type: "markdown".into(),
            url: String::new(),
        };
        let chunks = chunk_source(&ex, None);
        assert_eq!(chunks.len(), 1);
        // Frontmatter is gone from both text and embeds; tags join the context.
        assert!(!chunks[0].text.contains("---"));
        assert!(chunks[0]
            .embed_text
            .starts_with("[Dialing In · #espresso]\n"));
        assert!(chunks[0].embed_text.contains("See Temps."));
        assert!(
            chunks[0].text.contains("[[Temps]]"),
            "display keeps brackets"
        );
    }

    #[test]
    fn chunk_text_packs_paragraphs_and_prefixes_context() {
        assert!(chunk_text("Doc", "").is_empty());

        // Small paragraphs pack into one chunk; text stays verbatim while the
        // embed text carries the document title as context.
        let chunks = chunk_text("My Doc", "first paragraph.\n\nsecond paragraph.");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "first paragraph.\n\nsecond paragraph.");
        assert!(chunks[0].embed_text.starts_with("[My Doc]\n"));

        // Headings start a new chunk and become section context.
        let chunks = chunk_text("Guide", "intro text here.\n\n# Setup\n\nsetup steps.");
        assert_eq!(chunks.len(), 2);
        assert!(chunks[1].text.starts_with("# Setup"));
        assert!(chunks[1].embed_text.starts_with("[Guide › Setup]\n"));

        // An oversized paragraph splits at sentence boundaries.
        let long: String = (0..600).map(|i| format!("word{i}. ")).collect();
        let chunks = chunk_text("Doc", &long);
        assert!(chunks.len() >= 2, "oversized paragraph splits");
        assert!(chunks.iter().all(|c| word_count(&c.text) <= chunk_words()));

        // Boundary-free text falls back to overlapping word windows.
        let words = (0..900)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_text("", &words);
        assert!(chunks.len() >= 3, "long text splits into multiple chunks");
        let tail: Vec<&str> = chunks[0].text.split_whitespace().rev().take(5).collect();
        assert!(
            tail.iter().any(|w| chunks[1].text.contains(*w)),
            "windows overlap"
        );
        // No title/heading → no context prefix.
        assert_eq!(chunks[0].text, chunks[0].embed_text);
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

    // Word documents extract to markdown THROUGH THE REAL PATH (anydoc):
    // headings, emphasis, and tables survive instead of flattening to
    // plain text. A minimal but valid .docx package, built in-test.
    #[test]
    fn docx_extracts_styles_as_markdown() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        let path = std::env::temp_dir().join(format!("alchemy-test-{}.docx", uuid::Uuid::new_v4()));
        let file = std::fs::File::create(&path).unwrap();
        let mut z = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let mut add = |name: &str, body: &str| {
            z.start_file(name, opts).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        };
        add(
            "[Content_Types].xml",
            r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#,
        );
        // Heading styles resolve through the styles part, exactly like Word:
        // without it "Heading1" is just a paragraph.
        add(
            "word/styles.xml",
            r#"<?xml version="1.0"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style></w:styles>"#,
        );
        add(
            "_rels/.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        );
        add(
            "word/document.xml",
            r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly Report</w:t></w:r></w:p>
<w:p><w:r><w:t>Plain intro with </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bold words</w:t></w:r><w:r><w:t>.</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Region</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Revenue</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>West</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>$1.2M</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
</w:body></w:document>"#,
        );
        z.finish().unwrap();

        let got = extract_file(path.to_str().unwrap()).expect("docx extracts");
        std::fs::remove_file(&path).ok();
        let md = &got.text;
        assert!(md.contains("Quarterly Report"), "title: {md}");
        assert!(md.contains("# Quarterly Report"), "h1: {md}");
        assert!(md.contains("**bold words**"), "bold: {md}");
        assert!(md.contains("| Region | Revenue |"), "table header: {md}");
        assert!(md.contains("| West | $1.2M |"), "row: {md}");
    }

    // Articles extract to MARKDOWN so structure and links survive — links
    // feed the reader's wiki-jumping and the backlink graph.
    #[test]
    fn readable_text_preserves_structure_and_links_as_markdown() {
        let para = "A long enough paragraph about the topic at hand that clears the minimum \
                    article-length threshold used by the readability extraction fallback logic.";
        let html = format!(
            r#"<html><head><title>Linked Article</title></head><body>
<article><h1>Linked Article</h1>
<h2>Background</h2>
<p>{para}</p>
<p>See <a href="https://example.com/related">the related piece</a> for context.</p>
<ul><li>First point about it</li><li>Second point about it</li></ul>
<p>{para}</p>
</article></body></html>"#
        );
        let (_title, text) = readable_text(&html, "https://example.com/linked");
        assert!(text.contains("## Background"), "heading kept: {text}");
        assert!(
            text.contains("[the related piece](https://example.com/related)"),
            "link kept as markdown: {text}"
        );
        assert!(
            text.lines()
                .any(|l| l.trim_start().starts_with(['-', '*']) && l.contains("First point")),
            "list kept: {text}"
        );
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

    #[test]
    fn google_export_detects_editor_urls() {
        let (kind, export) =
            google_export("https://docs.google.com/document/d/abc-123_X/edit#heading=h.1").unwrap();
        assert_eq!(kind, GoogleDocKind::Doc);
        assert_eq!(
            export,
            "https://docs.google.com/document/d/abc-123_X/export?format=txt"
        );

        let (kind, export) =
            google_export("https://docs.google.com/spreadsheets/d/SHEET?usp=sharing").unwrap();
        assert_eq!(kind, GoogleDocKind::Sheet);
        assert!(export.ends_with("/SHEET/export?format=xlsx"));

        // Account-selector form.
        let (kind, _) =
            google_export("https://docs.google.com/presentation/u/0/d/DECK/edit").unwrap();
        assert_eq!(kind, GoogleDocKind::Slides);

        assert!(google_export("https://docs.google.com/forms/d/abc/edit").is_none());
        assert!(google_export("https://example.com/document/d/abc").is_none());
        // Published-to-web links are plain HTML — leave them to the scraper.
        assert!(google_export("https://docs.google.com/document/d/e/2PACX-abc123/pub").is_none());
        assert!(is_google_doc_url("https://docs.google.com/document/d/abc"));
        assert!(!is_google_doc_url("https://example.com"));
    }

    #[test]
    fn placeholder_doc_url_parses_both_formats() {
        // Newer Drive-for-desktop stubs carry doc_id.
        let modern = r#"{"":"WARNING!","doc_id":"1A_blIDY","resource_key":"","email":"x@y.com"}"#;
        assert_eq!(
            placeholder_doc_url("document", modern).as_deref(),
            Some("https://docs.google.com/document/d/1A_blIDY/edit")
        );
        // Older stubs carry a url with ?id=.
        let legacy = r#"{"url":"https://docs.google.com/open?id=OLD123&x=1","email":"x@y.com"}"#;
        assert_eq!(
            placeholder_doc_url("spreadsheets", legacy).as_deref(),
            Some("https://docs.google.com/spreadsheets/d/OLD123/edit")
        );
        assert!(placeholder_doc_url("document", "{}").is_none());
        assert!(placeholder_doc_url("document", "not json").is_none());
        assert!(google_placeholder_url("/tmp/notes.md").is_none());
    }

    #[test]
    fn anydoc_extracts_the_office_family_as_markdown() {
        let dir = std::env::temp_dir().join(format!("alchemy-anydoc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // The §1 acceptance test: a CSV through the FULL extract path still
        // lands as a GFM table, whichever extractor produced it.
        let csv = dir.join("fleet.csv");
        std::fs::write(&csv, "vessel,berth\nSea Otter,12\nSea Marten,4\n").unwrap();
        let got = extract_file(csv.to_str().unwrap()).expect("csv extracts");
        assert!(
            got.text.lines().nth(1).is_some_and(|l| l.contains("---")),
            "csv must extract as a GFM table, got:\n{}",
            got.text
        );
        assert!(got.text.contains("Sea Otter"));

        // RTF — a format the app never had an extractor for — now extracts
        // through anydoc instead of erroring as unsupported.
        let rtf = dir.join("note.rtf");
        std::fs::write(
            &rtf,
            r"{\rtf1\ansi\deff0 {\fonttbl {\f0 Times;}} Hello from RTF land.}",
        )
        .unwrap();
        let got = extract_file(rtf.to_str().unwrap()).expect("rtf extracts via anydoc");
        assert!(
            got.text.contains("Hello from RTF land"),
            "rtf text missing, got:\n{}",
            got.text
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delimited_to_rows_makes_a_markdown_table() {
        // The reader paints valid GFM as a real table — that's the point.
        let csv = "name,note\n\"Doe, Jane\",\"said \"\"hi\"\"\"\nplain,row\n";
        assert_eq!(
            delimited_to_rows(csv, ','),
            "| name | note |\n| --- | --- |\n| Doe, Jane | said \"hi\" |\n| plain | row |\n"
        );
        // Blank rows are dropped; TSV uses tabs.
        assert_eq!(
            delimited_to_rows("a\tb\n\n\nc\td\n", '\t'),
            "| a | b |\n| --- | --- |\n| c | d |\n"
        );
    }

    #[test]
    fn tidy_drops_phantom_columns_and_blank_rows() {
        // Observed live: a brokerage CSV with trailing commas rendered four
        // empty columns down the table's right edge, plus spacer rows.
        let messy = "| Account | Value |  |  |\n\
                     | --- | --- | --- | --- |\n\
                     | IRA | 1720.41 |  |  |\n\
                     |  |  |  |  |\n\
                     | Cash | 12.00 |  |  |\n";
        assert_eq!(
            tidy_markdown_tables(messy),
            "| Account | Value |\n| --- | --- |\n| IRA | 1720.41 |\n| Cash | 12.00 |\n"
        );
        // A fully-populated table and surrounding prose pass through intact.
        let clean = "# Sheet: One\n| a | b |\n| --- | --- |\n| 1 | 2 |\nafter\n";
        assert_eq!(tidy_markdown_tables(clean), clean);
        // A BLANK header survives: anydoc emits one when a CSV's first line
        // is a title rather than column names, and dropping it left the
        // separator first — invalid GFM, so the whole table fell apart into
        // flowed pipe-riddled text (observed live).
        let blank_header = "|  |  |  |\n\
                            | --- | --- | --- |\n\
                            | Account Summary |  |  |\n\
                            | IRA | 172085.41 |  |\n";
        assert_eq!(
            tidy_markdown_tables(blank_header),
            "|  |  |\n| --- | --- |\n| Account Summary |  |\n| IRA | 172085.41 |\n"
        );
        // Non-table text is untouched, even with stray pipes mid-line.
        let prose = "pipes | in prose | stay\n";
        assert_eq!(tidy_markdown_tables(prose), prose);
    }

    #[test]
    fn markdown_table_cells_cannot_shear_the_grid() {
        // A pipe or newline inside a cell must not open a new column or row.
        let rows = vec![
            vec!["ticker".into(), "note".into()],
            vec!["A|B".into(), "line one\nline two".into()],
            vec!["ragged".into()],
        ];
        assert_eq!(
            rows_to_markdown_table(&rows),
            "| ticker | note |\n| --- | --- |\n| A\\|B | line one line two |\n| ragged |  |\n"
        );
        assert_eq!(rows_to_markdown_table(&[]), "");
    }

    #[test]
    fn content_disposition_title_prefers_utf8_form() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_DISPOSITION,
            "attachment; filename=\"Plan B.txt\"; filename*=UTF-8''Plan%20%E2%9C%93.txt"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            title_from_content_disposition(&headers).as_deref(),
            Some("Plan ✓")
        );

        let mut plain = reqwest::header::HeaderMap::new();
        plain.insert(
            reqwest::header::CONTENT_DISPOSITION,
            "attachment; filename=\"Roadmap.xlsx\"".parse().unwrap(),
        );
        assert_eq!(
            title_from_content_disposition(&plain).as_deref(),
            Some("Roadmap")
        );
        assert!(title_from_content_disposition(&reqwest::header::HeaderMap::new()).is_none());
    }

    /// Local HTML files run through the same readability path as URLs: the
    /// article body survives, chrome is dropped, and the document title wins
    /// over the filename stem.
    #[test]
    fn html_files_extract_like_urls() {
        let body = format!(
            "<html><head><title>The Athanor Manual</title></head><body>\
             <nav><a href=\"/\">Home</a><a href=\"/about\">About</a></nav>\
             <article><h1>The Athanor Manual</h1>{}</article>\
             <footer>Copyright 2026 · Privacy · Terms</footer></body></html>",
            "<p>The athanor holds a steady heat for the long digestion. Keep the \
             vessel sealed and the fire moderate; sudden temperature changes crack \
             the glass and spoil the work entirely.</p>"
                .repeat(4)
        );
        let path = std::env::temp_dir().join(format!("alchemy-test-{}.html", std::process::id()));
        std::fs::write(&path, &body).unwrap();
        let ex = extract_file(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(ex.source_type, "html");
        assert_eq!(ex.title, "The Athanor Manual");
        assert!(ex.text.contains("steady heat for the long digestion"));
        assert!(
            !ex.text.contains("Copyright 2026"),
            "boilerplate dropped: {}",
            ex.text
        );
    }

    #[test]
    fn epub_extracts_chapters_in_spine_order() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        // Minimal epub: container.xml -> OPF -> spine listing ch2 before ch1,
        // proving we honor reading order rather than archive order.
        let path = std::env::temp_dir().join(format!("alchemy-test-{}.epub", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let mut add = |name: &str, body: &str| {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        };
        add(
            "chapter1.xhtml",
            "<html><body><p>Second in spine &amp; last in text.</p></body></html>",
        );
        add(
            "chapter2.xhtml",
            "<html><body><h1>Opening</h1><p>First in spine.</p></body></html>",
        );
        add(
            "META-INF/container.xml",
            r#"<container><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        );
        add(
            "content.opf",
            r#"<package><manifest>
                <item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
                <item id="c2" href="chapter2.xhtml" media-type="application/xhtml+xml"/>
            </manifest><spine><itemref idref="c2"/><itemref idref="c1"/></spine></package>"#,
        );
        zip.finish().unwrap();

        // Through the real path (anydoc owns epub now).
        let text = extract_file(path.to_str().unwrap()).unwrap().text;
        std::fs::remove_file(&path).ok();

        let first = text.find("First in spine").unwrap();
        let second = text.find("Second in spine & last in text").unwrap();
        assert!(first < second, "spine order should win over archive order");
        assert!(text.contains("Opening"));
        assert!(!text.contains("<p>"), "no tags survive: {text}");
    }

    /// The panic boundary on `extract_file`: a malformed file of any type
    /// must come back as Err — never a panic that unwinds the worker (which
    /// hung a live folder import when the PDF reader hit "unexpected encoding
    /// NULL"). Garbage bytes with a .pdf extension exercise the whole path.
    #[test]
    fn extract_file_errors_instead_of_panicking_on_garbage() {
        let path = std::env::temp_dir().join("nbl-malformed-fixture.pdf");
        std::fs::write(&path, b"%PDF-1.4 garbage \x00\x01\x02 not a real pdf").unwrap();
        let res = extract_file(&path.to_string_lossy());
        assert!(res.is_err(), "malformed pdf must error, not panic");
        let _ = std::fs::remove_file(&path);
    }

    fn fixture(name: &str) -> String {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn boxnote_new_format_parses_prosemirror_tree() {
        // The whole pipeline: extension dispatch + ProseMirror walk + normalize.
        let ex = extract_file(&fixture("boxnote_new.boxnote")).unwrap();
        assert_eq!(ex.source_type, "markdown");
        // Heading keeps its markdown level; bold runs join with their siblings.
        assert!(ex.text.contains("# Quarterly Plan"), "heading: {}", ex.text);
        assert!(
            ex.text.contains("We ship cloud folders this week."),
            "paragraph runs join: {}",
            ex.text
        );
        // List items get bullets and land on their own lines.
        assert!(ex.text.contains("- Box Notes"), "bullet: {}", ex.text);
        assert!(ex.text.contains("- Dropbox Paper"), "bullet: {}", ex.text);
        // A hard break inside a paragraph becomes a newline, not a space.
        assert!(
            ex.text.contains("Line one\nLine two"),
            "hard break: {:?}",
            ex.text
        );
    }

    #[test]
    fn boxnote_old_format_reads_etherpad_atext() {
        let ex = extract_file(&fixture("boxnote_old.boxnote")).unwrap();
        assert!(
            ex.text.contains("Legacy Box Note"),
            "title line: {}",
            ex.text
        );
        assert!(
            ex.text
                .contains("This note predates the ProseMirror editor."),
            "body: {}",
            ex.text
        );
    }

    #[test]
    fn boxnote_rejects_non_json() {
        let path = std::env::temp_dir().join("nbl-bad-fixture.boxnote");
        std::fs::write(&path, b"not json at all").unwrap();
        let res = extract_file(&path.to_string_lossy());
        assert!(res.is_err(), "garbage .boxnote must error, not panic");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dropbox_paper_url_reads_json_stub() {
        let path = std::env::temp_dir().join("nbl-stub-json.paper");
        std::fs::write(
            &path,
            br#"{"url":"https://www.dropbox.com/scl/fi/abc123/Plan.paper?rlkey=xyz","title":"Plan"}"#,
        )
        .unwrap();
        assert_eq!(
            dropbox_paper_url(&path.to_string_lossy()).as_deref(),
            Some("https://www.dropbox.com/scl/fi/abc123/Plan.paper?rlkey=xyz")
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dropbox_paper_url_reads_bare_weblink_stub() {
        let path = std::env::temp_dir().join("nbl-stub-link.paper");
        std::fs::write(
            &path,
            b"[InternetShortcut]\nURL=https://paper.dropbox.com/doc/Roadmap--abcDEF\n",
        )
        .unwrap();
        assert_eq!(
            dropbox_paper_url(&path.to_string_lossy()).as_deref(),
            Some("https://paper.dropbox.com/doc/Roadmap--abcDEF")
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dropbox_paper_url_skips_opaque_stub() {
        // An opaque/binary .paper placeholder has no fetchable URL — None means
        // the folder scan skips it with a reason rather than fetching garbage.
        let path = std::env::temp_dir().join("nbl-stub-opaque.paper");
        std::fs::write(&path, b"\x00\x01\x02 opaque box\x00 not a url").unwrap();
        assert_eq!(dropbox_paper_url(&path.to_string_lossy()), None);
        let _ = std::fs::remove_file(&path);
        // Wrong extension never matches, even with a URL inside.
        let other = std::env::temp_dir().join("nbl-not-paper.txt");
        std::fs::write(&other, b"https://paper.dropbox.com/doc/x").unwrap();
        assert_eq!(dropbox_paper_url(&other.to_string_lossy()), None);
        let _ = std::fs::remove_file(&other);
    }
}

#[cfg(test)]
mod error_page_tests {
    use super::*;

    fn page(title: &str, paragraphs: usize) -> String {
        let para = "<p>The membership includes a parking pass and discounts at partner businesses across the county.</p>".repeat(paragraphs);
        format!("<html><head><title>{title}</title></head><body><main><article><h1>{title}</h1>{para}</article></main></body></html>")
    }

    #[test]
    fn failing_status_with_error_title_is_an_error_page() {
        // A soft-404 keeps its whole layout, so the body is long — the title
        // is what gives it away.
        let err = readable_page(
            page("Page Not Found - Error 404 | Regional Parks", 200),
            reqwest::StatusCode::NOT_FOUND,
            "https://example.org/gone".into(),
        )
        .unwrap_err();
        let page = err.downcast_ref::<HttpErrorPage>().expect("typed error");
        assert_eq!(page.status, 404);
        assert!(err.to_string().contains("HTTP 404"), "{err}");
    }

    #[test]
    fn failing_status_with_a_real_article_still_imports() {
        // Broken SSR that still renders (cerebras.ai): the status lies, the
        // body doesn't.
        let ok = readable_page(
            page("Inference at the speed of thought", 200),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "https://example.org/post".into(),
        )
        .unwrap();
        assert!(ok.title.contains("Inference"), "{}", ok.title);
    }

    #[test]
    fn success_status_never_consults_the_title() {
        let ok = readable_page(
            page("404: why the page-not-found problem persists", 200),
            reqwest::StatusCode::OK,
            "https://example.org/essay".into(),
        )
        .unwrap();
        assert!(ok.text.chars().count() > 1_000);
    }

    #[test]
    fn error_page_rule_covers_thin_bodies_and_error_titles() {
        assert!(looks_like_error_page("Error", "Something short."));
        assert!(looks_like_error_page(
            "Ducati Page Not Found",
            &"word ".repeat(1_000)
        ));
        assert!(looks_like_error_page(
            "Error | Costco",
            &"word ".repeat(1_000)
        ));
        assert!(!looks_like_error_page(
            "A fine title",
            &"word ".repeat(1_000)
        ));
        assert!(!looks_like_error_page(
            "Terror at the summit",
            &"word ".repeat(1_000)
        ));
    }
}
