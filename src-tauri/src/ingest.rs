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
    /// Origin of the content: the URL for `url` sources, the local file path
    /// for file imports (stamped by the command layer), empty for pasted text.
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
            return Err(anyhow!("no extractable text found in {path}"));
        }
        return Ok(Extracted {
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
            // pdf-extract PANICS (not errors) on some malformed/encrypted PDFs
            // — e.g. "unexpected encoding NULL". Catch it so one bad file in a
            // folder import fails gracefully instead of unwinding the worker
            // thread and hanging the whole import.
            let text = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pdf_extract::extract_text(path)
            }))
            .map_err(|_| anyhow!("failed to parse PDF {path} — it may be malformed or encrypted"))?
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
        "csv" | "tsv" => {
            let delim = if ext == "csv" { ',' } else { '\t' };
            (
                "text".to_string(),
                delimited_to_rows(&read_text_lossy(path)?, delim),
            )
        }
        "docx" => ("text".to_string(), extract_docx(path)?),
        "pptx" => ("text".to_string(), extract_pptx(path)?),
        "epub" => ("text".to_string(), extract_epub(path)?),
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
    let mut workbook = calamine::open_workbook_auto(path)
        .with_context(|| format!("failed to open spreadsheet {path}"))?;
    Ok(sheets_to_text(&mut workbook))
}

/// Render every sheet of an open workbook as "cell | cell | cell" rows.
fn sheets_to_text<RS>(workbook: &mut calamine::Sheets<RS>) -> String
where
    RS: std::io::Read + std::io::Seek,
{
    use calamine::{Data, Reader};
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
    out
}

/// Read a file as UTF-8, replacing invalid bytes. Excel-exported CSVs are
/// often Windows-1252 — importing with a few replacement characters beats
/// failing the whole file.
fn read_text_lossy(path: &str) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("failed to read {path}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Convert delimiter-separated text (CSV/TSV) into readable "a | b | c" rows.
/// The csv crate handles RFC 4180 quoting, CRLF, and ragged rows.
fn delimited_to_rows(text: &str, delim: char) -> String {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim as u8)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut out = String::new();
    for rec in rdr.records().flatten() {
        let cells: Vec<&str> = rec.iter().collect();
        if cells.iter().any(|c| !c.trim().is_empty()) {
            out.push_str(&cells.join(" | "));
            out.push('\n');
        }
    }
    out
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

/// Extract a .docx (WordprocessingML) to markdown: heading styles become
/// `#` headings, bold/italic runs keep their emphasis, list paragraphs
/// become bullets, and tables become GFM tables — so Word documents read
/// (and chunk) like their origin instead of flattened text.
fn extract_docx(path: &str) -> Result<String> {
    let xml = read_zip_entry(path, "word/document.xml")?;
    Ok(docx_to_markdown(&xml))
}

fn docx_to_markdown(xml: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < xml.len() {
        let rest = &xml[i..];
        if let Some(tbl_start) = rest.find("<w:tbl>").or_else(|| rest.find("<w:tbl ")) {
            let p_start = rest.find("<w:p>").or_else(|| rest.find("<w:p "));
            if p_start.is_none_or(|p| tbl_start < p) {
                let tbl_rest = &rest[tbl_start..];
                let end = tbl_rest
                    .find("</w:tbl>")
                    .map(|e| e + "</w:tbl>".len())
                    .unwrap_or(tbl_rest.len());
                out.push_str(&docx_table(&tbl_rest[..end]));
                out.push('\n');
                i += tbl_start + end;
                continue;
            }
        }
        match rest.find("<w:p>").or_else(|| rest.find("<w:p ")) {
            Some(p_start) => {
                let p_rest = &rest[p_start..];
                let end = p_rest
                    .find("</w:p>")
                    .map(|e| e + "</w:p>".len())
                    .unwrap_or(p_rest.len());
                let para = docx_paragraph(&p_rest[..end]);
                if !para.trim().is_empty() {
                    out.push_str(&para);
                    out.push('\n');
                    out.push('\n');
                }
                i += p_start + end;
            }
            None => break,
        }
    }
    out.trim().to_string()
}

/// One `<w:p>` → one markdown line: heading prefix from pStyle, list bullet
/// from numPr, bold/italic from each run's properties.
fn docx_paragraph(p: &str) -> String {
    let props = p.find("</w:pPr>").map(|e| &p[..e]).unwrap_or("");
    let style = props
        .find("<w:pStyle")
        .and_then(|at| props[at..].find('>').map(|e| &props[at..at + e]))
        .and_then(|tag| xml_attr(tag, "w:val"))
        .unwrap_or("");
    let prefix = match style {
        "Title" | "Heading1" => "# ",
        "Heading2" => "## ",
        "Heading3" | "Heading4" => "### ",
        _ if props.contains("<w:numPr>") => "- ",
        _ => "",
    };
    let mut text = String::new();
    // Merge consecutive same-format runs so split runs don't emit `****`.
    let mut pending = String::new();
    let mut pending_fmt = (false, false);
    let flush = |text: &mut String, seg: &mut String, fmt: (bool, bool)| {
        if seg.is_empty() {
            return;
        }
        let (bold, italic) = fmt;
        let wrapped = match (bold, italic) {
            (true, true) => format!("***{seg}***"),
            (true, false) => format!("**{seg}**"),
            (false, true) => format!("*{seg}*"),
            (false, false) => seg.clone(),
        };
        text.push_str(&wrapped);
        seg.clear();
    };
    let mut j = 0;
    while let Some(r_at) = p[j..].find("<w:r>").or_else(|| p[j..].find("<w:r ")) {
        let r_rest = &p[j + r_at..];
        let r_end = r_rest
            .find("</w:r>")
            .map(|e| e + "</w:r>".len())
            .unwrap_or(r_rest.len());
        let run = &r_rest[..r_end];
        let rpr = run.find("</w:rPr>").map(|e| &run[..e]).unwrap_or("");
        let flag = |tag: &str| {
            rpr.find(&format!("<w:{tag}"))
                .map(|at| {
                    let t = &rpr[at..rpr[at..].find('>').map(|e| at + e + 1).unwrap_or(rpr.len())];
                    !t.contains("w:val=\"0\"") && !t.contains("w:val=\"false\"")
                })
                .unwrap_or(false)
        };
        let fmt = (flag("b/") || flag("b "), flag("i/") || flag("i "));
        if fmt != pending_fmt {
            flush(&mut text, &mut pending, pending_fmt);
            pending_fmt = fmt;
        }
        let mut k = 0;
        while let Some(t_at) = run[k..].find("<w:t") {
            let t_rest = &run[k + t_at..];
            let Some(open_end) = t_rest.find('>') else {
                break;
            };
            let Some(close) = t_rest.find("</w:t>") else {
                break;
            };
            if close > open_end {
                pending.push_str(&xml_unescape(&t_rest[open_end + 1..close]));
            }
            k += t_at + close + "</w:t>".len();
        }
        if run.contains("<w:br/>") {
            pending.push('\n');
        }
        if run.contains("<w:tab/>") {
            pending.push('\t');
        }
        j += r_at + r_end;
    }
    flush(&mut text, &mut pending, pending_fmt);
    if text.trim().is_empty() {
        return String::new();
    }
    format!("{prefix}{}", text.trim())
}

/// One `<w:tbl>` → a GFM table (first row is the header).
fn docx_table(tbl: &str) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut i = 0;
    while let Some(tr_at) = tbl[i..].find("<w:tr>").or_else(|| tbl[i..].find("<w:tr ")) {
        let tr_rest = &tbl[i + tr_at..];
        let tr_end = tr_rest
            .find("</w:tr>")
            .map(|e| e + "</w:tr>".len())
            .unwrap_or(tr_rest.len());
        let tr = &tr_rest[..tr_end];
        let mut cells: Vec<String> = Vec::new();
        let mut c = 0;
        while let Some(tc_at) = tr[c..].find("<w:tc>").or_else(|| tr[c..].find("<w:tc ")) {
            let tc_rest = &tr[c + tc_at..];
            let tc_end = tc_rest
                .find("</w:tc>")
                .map(|e| e + "</w:tc>".len())
                .unwrap_or(tc_rest.len());
            let mut cell = String::new();
            let mut p_i = 0;
            let tc = &tc_rest[..tc_end];
            while let Some(p_at) = tc[p_i..].find("<w:p>").or_else(|| tc[p_i..].find("<w:p ")) {
                let p_rest = &tc[p_i + p_at..];
                let p_end = p_rest
                    .find("</w:p>")
                    .map(|e| e + "</w:p>".len())
                    .unwrap_or(p_rest.len());
                let para = docx_paragraph(&p_rest[..p_end]);
                if !para.trim().is_empty() {
                    if !cell.is_empty() {
                        cell.push(' ');
                    }
                    cell.push_str(para.trim_start_matches(['#', ' ', '-']).trim());
                }
                p_i += p_at + p_end;
            }
            cells.push(cell.replace('|', "\\|"));
            c += tc_at + tc_end;
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
        i += tr_at + tr_end;
    }
    if rows.is_empty() {
        return String::new();
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut out = String::new();
    for (idx, row) in rows.iter().enumerate() {
        out.push('|');
        for c in 0..cols {
            out.push(' ');
            out.push_str(row.get(c).map(String::as_str).unwrap_or(""));
            out.push_str(" |");
        }
        out.push('\n');
        if idx == 0 {
            out.push('|');
            for _ in 0..cols {
                out.push_str(" --- |");
            }
            out.push('\n');
        }
    }
    out
}

/// Decode the five XML entities WordprocessingML uses in text nodes.
fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
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

/// Pull a double-quoted attribute value out of an XML tag string.
fn xml_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// Chapter paths in reading order: META-INF/container.xml names the OPF
/// package file, whose spine lists manifest ids in the order a reader shows
/// them. None if any of that structure is missing or malformed.
fn epub_spine<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Option<Vec<String>> {
    use std::io::Read;
    let mut container = String::new();
    zip.by_name("META-INF/container.xml")
        .ok()?
        .read_to_string(&mut container)
        .ok()?;
    let rootfile = &container[container.find("<rootfile")?..];
    let opf_path = xml_attr(rootfile, "full-path")?.to_string();
    let mut opf = String::new();
    zip.by_name(&opf_path).ok()?.read_to_string(&mut opf).ok()?;
    // Manifest hrefs resolve relative to the OPF's own directory.
    let base = opf_path
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/"))
        .unwrap_or_default();

    let mut hrefs = std::collections::HashMap::new();
    for tag in opf.split("<item ").skip(1) {
        let Some(end) = tag.find('>') else { continue };
        let tag = &tag[..end];
        if let (Some(id), Some(href)) = (xml_attr(tag, "id"), xml_attr(tag, "href")) {
            hrefs.insert(id.to_string(), href.to_string());
        }
    }
    let mut order = Vec::new();
    for tag in opf[opf.find("<spine")?..].split("<itemref").skip(1) {
        let Some(end) = tag.find('>') else { continue };
        if let Some(href) = xml_attr(&tag[..end], "idref").and_then(|id| hrefs.get(id)) {
            order.push(format!("{base}{href}"));
        }
    }
    Some(order)
}

/// Every HTML-ish entry in archive order — the fallback when an epub's
/// package metadata can't be parsed.
fn epub_html_entries<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Vec<String> {
    (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|e| e.name().to_string()))
        .filter(|n| {
            let l = n.to_lowercase();
            l.ends_with(".xhtml") || l.ends_with(".html") || l.ends_with(".htm")
        })
        .collect()
}

/// Extract text from an .epub (a zip of XHTML chapters), in reading order.
fn extract_epub(path: &str) -> Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).with_context(|| format!("failed to open {path}"))?;
    let mut zip = zip::ZipArchive::new(file).context("not a valid .epub (zip) file")?;

    let chapters = match epub_spine(&mut zip) {
        Some(c) if !c.is_empty() => c,
        _ => epub_html_entries(&mut zip),
    };

    let mut out = String::new();
    for name in chapters {
        let Ok(mut entry) = zip.by_name(&name) else {
            continue;
        };
        let mut html = String::new();
        if entry.read_to_string(&mut html).is_err() {
            continue;
        }
        // Block-level closers become newlines so paragraphs survive stripping.
        for closer in [
            "</p>", "</div>", "</li>", "</h1>", "</h2>", "</h3>", "</h4>", "</h5>", "</h6>",
            "<br/>", "<br />", "<br>",
        ] {
            html = html.replace(closer, &format!("{closer}\n"));
        }
        let text = strip_html(&html);
        if !text.trim().is_empty() {
            out.push_str(text.trim());
            out.push_str("\n\n");
        }
    }
    Ok(out)
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
    // article with a 500 (broken SSR that still renders — cerebras.ai) or a
    // 404 (soft-deleted pages with full layouts). Fetch the body regardless
    // and let readability decide; only give up when there's nothing to read.
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

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
    Ok(Extracted {
        title,
        source_type: "url".to_string(),
        url,
        text,
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
            // Bytes is AsRef<[u8]>, so the cursor reads it without a copy.
            let mut workbook = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes))
                .context("could not parse the exported spreadsheet")?;
            sheets_to_text(&mut workbook)
        }
        _ => resp.text().await.context("failed to read export body")?,
    };
    let text = normalize(&text);
    if text.trim().is_empty() {
        return Err(anyhow!("this {} exported no text", kind.product()));
    }
    Ok(Extracted {
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
    Extracted {
        title,
        source_type: "url".to_string(),
        url: url.to_string(),
        text,
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
/// packed up to ~CHUNK_WORDS, markdown-style headings start a new chunk and
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
        if words > CHUNK_WORDS {
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

        if cur_words + words > CHUNK_WORDS && !cur.is_empty() {
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
/// up to CHUNK_WORDS. A single run with no boundaries at all (minified text,
/// giant table row) falls back to overlapping word windows.
fn split_oversized(p: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_words = 0usize;
    for seg in p.split_inclusive(['.', '!', '?', '\n']) {
        let words = word_count(seg);
        if words > CHUNK_WORDS {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
                cur.clear();
                cur_words = 0;
            }
            out.extend(word_windows(seg));
            continue;
        }
        if cur_words + words > CHUNK_WORDS && !cur.trim().is_empty() {
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
/// caller knows it (folder children), falling back to the title. Page-capture
/// (url/html) sources additionally drop nav-cruft chunks from the index.
pub fn chunk_source(extracted: &Extracted, code_ctx: Option<&str>) -> Vec<Chunk> {
    if extracted.source_type == "code" {
        chunk_code(code_ctx.unwrap_or(&extracted.title), &extracted.text)
    } else {
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
}

/// How many trailing lines of one code chunk repeat at the start of the next
/// when a single block is bigger than a whole chunk — continuity without
/// prose-style word overlap.
const CODE_OVERLAP_LINES: usize = 8;

/// Split code into chunks on blank-line block boundaries, packing blocks up
/// to ~CHUNK_WORDS. Text is verbatim — indentation intact, so citations show
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
        if words > CHUNK_WORDS {
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
        if pending_words + words > CHUNK_WORDS && !pending.is_empty() {
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

/// Split one oversized code block into line runs of ~CHUNK_WORDS with a few
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
            if end > start && words + w > CHUNK_WORDS {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            title: "main.rs".into(),
            source_type: "code".into(),
            url: String::new(),
            text: "fn main() {\n    body();\n}".into(),
        };
        let got = chunk_source(&code, None);
        assert!(got[0].text.contains("    body();"));
        assert!(got[0].embed_text.starts_with("[main.rs]\n"));

        let prose = Extracted {
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
        assert!(chunks.iter().all(|c| word_count(&c.text) <= CHUNK_WORDS));

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

    // Word documents extract to markdown: headings, emphasis, lists, and
    // tables all survive instead of flattening to plain text.
    #[test]
    fn docx_maps_styles_to_markdown() {
        let xml = r#"<w:document><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly Report</w:t></w:r></w:p>
<w:p><w:r><w:t>Plain intro with </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bold words</w:t></w:r><w:r><w:t> and </w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>italic ones</w:t></w:r><w:r><w:t>.</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Findings</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>First bullet</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>Second bullet &amp; more</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Region</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Revenue</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>West</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>$1.2M</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
</w:body></w:document>"#;
        let md = docx_to_markdown(xml);
        assert!(md.contains("# Quarterly Report"), "h1: {md}");
        assert!(md.contains("**bold words**"), "bold: {md}");
        assert!(md.contains("*italic ones*"), "italic: {md}");
        assert!(md.contains("## Findings"), "h2: {md}");
        assert!(md.contains("- First bullet"), "list: {md}");
        assert!(md.contains("- Second bullet & more"), "entities: {md}");
        assert!(md.contains("| Region | Revenue |"), "table header: {md}");
        assert!(md.contains("| --- | --- |"), "separator: {md}");
        assert!(md.contains("| West | $1.2M |"), "row: {md}");
    }

    // Split runs with identical formatting merge — no `****` artifacts.
    #[test]
    fn docx_merges_adjacent_same_format_runs() {
        let xml = r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Hello </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>world</w:t></w:r></w:p>"#;
        let md = docx_to_markdown(xml);
        assert_eq!(md, "**Hello world**");
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
    fn delimited_to_rows_handles_quoting() {
        let csv = "name,note\n\"Doe, Jane\",\"said \"\"hi\"\"\"\nplain,row\n";
        assert_eq!(
            delimited_to_rows(csv, ','),
            "name | note\nDoe, Jane | said \"hi\"\nplain | row\n"
        );
        // Blank rows are dropped; TSV uses tabs.
        assert_eq!(
            delimited_to_rows("a\tb\n\n\nc\td\n", '\t'),
            "a | b\nc | d\n"
        );
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

        let text = extract_epub(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        let first = text.find("First in spine").unwrap();
        let second = text.find("Second in spine & last in text").unwrap();
        assert!(first < second, "spine order should win over archive order");
        assert!(text.contains("Opening"));
        assert!(!text.contains('<'), "no tags survive: {text}");
    }
}
