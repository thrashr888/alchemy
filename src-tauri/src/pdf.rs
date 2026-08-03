//! PDF text extraction (pdf-inspector) and page rasterization (PDFium).
//!
//! Two libraries, two jobs. `pdf-inspector` reads the content streams: it
//! classifies a file as text-based or scanned and lays its text items out in
//! reading order as Markdown — headings, lists and tables survive, which is
//! what the structure-aware chunker wants. PDFium renders pixels, for OCR of
//! the pages pdf-inspector says carry no text and for the reader's page view.
//! The libpdfium dynamic library is bundled under `src-tauri/libs/`.

use std::io::Cursor;

use anyhow::{anyhow, Context, Result};
use pdfium_render::prelude::*;

/// Extracted PDF text, one Markdown string per page plus the OCR routing the
/// detector recommends.
pub struct PdfText {
    /// Pages in document order, each already Markdown-shaped.
    pub pages: Vec<String>,
    /// 1-indexed pages carrying no usable text — image-only scans inside an
    /// otherwise readable file. Empty for a clean text PDF.
    pub pages_needing_ocr: Vec<usize>,
}

impl PdfText {
    /// Every page needs OCR (or the file has no pages at all): there is
    /// nothing here worth chunking, so the caller should rasterize instead.
    pub fn is_scanned(&self) -> bool {
        self.pages.is_empty() || self.pages_needing_ocr.len() == self.pages.len()
    }

    /// The document's title, guessed from the largest text on page one.
    ///
    /// For papers this is reliably the title: pdf-inspector assigns heading
    /// levels by font-size ratio, so the biggest thing on the first page wins
    /// (`##` beats `###`). The arXiv stamp printed sideways down the margin
    /// is the exception — it renders enormous, lands at `#`, and is never the
    /// title, so it is skipped by name. Used only when the PDF carries no
    /// /Title metadata, which arXiv's do not.
    pub fn guessed_title(&self) -> Option<String> {
        let first = self.pages.first()?;
        first
            .lines()
            .filter_map(|line| {
                let level = line.chars().take_while(|c| *c == '#').count();
                if level == 0 {
                    return None;
                }
                let text = line[level..].trim();
                let looks_like_a_stamp = text.starts_with("arXiv:")
                    || text.starts_with("doi:")
                    || text.to_lowercase().contains("creativecommons");
                let plausible =
                    text.chars().count() >= 8 && text.chars().count() <= 200 && text.contains(' ');
                (!looks_like_a_stamp && plausible).then(|| (level, text.to_string()))
            })
            // Lowest level = largest font. `min_by_key` keeps the first of a
            // tie, which is the topmost heading on the page.
            .min_by_key(|(level, _)| *level)
            .map(|(_, text)| text)
    }

    /// The document as one Markdown string, pages separated by a blank line.
    /// Pages awaiting OCR contribute nothing rather than a stray heading.
    pub fn markdown(&self) -> String {
        self.pages
            .iter()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Extract a PDF's text as per-page Markdown.
///
/// Errors only when the file cannot be parsed at all (not a PDF, encrypted,
/// broken xref). A file that parses but holds no text is *not* an error — it
/// comes back with every page in `pages_needing_ocr` so the caller can route
/// it to the vision model.
pub fn extract_text(path: &str) -> Result<PdfText> {
    let result = pdf_inspector::extract_pages_markdown(path, None)
        .map_err(|e| anyhow!("failed to read PDF {path}: {e}"))?;
    Ok(collect_pages(result))
}

/// Split pdf-inspector's per-page output into text we can chunk and pages the
/// vision model still has to look at.
fn collect_pages(result: pdf_inspector::PagesExtractionResult) -> PdfText {
    let mut pages = Vec::with_capacity(result.pages.len());
    let mut pages_needing_ocr = Vec::new();
    for page in &result.pages {
        // `page` is 0-indexed on the wire; everything user-facing is 1-indexed.
        if page.needs_ocr || page.markdown.trim().is_empty() {
            pages_needing_ocr.push(page.page as usize + 1);
            pages.push(String::new());
        } else {
            pages.push(tidy_headings(&page.markdown));
        }
    }
    PdfText {
        pages,
        pages_needing_ocr,
    }
}

/// Demote the "headings" that are not headings.
///
/// pdf-inspector assigns heading levels by font-size ratio, which is the
/// right call for a document with no structure tags — but it promotes three
/// things that then poison the reader's table of contents: the arXiv/DOI
/// stamp printed sideways down the margin (huge type, `#`), bold run-in lead
/// sentences ("**Our results indicate that prompting language** models…"),
/// and table header rows. Real headings survive; the rest become body text,
/// which is what they always were. The text itself is never dropped.
fn tidy_headings(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    for line in markdown.lines() {
        let level = line.chars().take_while(|c| *c == '#').count();
        if level == 0 || level > 6 {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let text = line[level..].trim();
        // Inline markup leaks into headings from table cells (`<u>10 docs</u>`)
        // and emphasis runs; judge, and print, the words themselves.
        let plain = strip_inline_markup(text);
        let words: Vec<&str> = plain.split_whitespace().collect();
        let stamp = plain.starts_with("arXiv:") || plain.starts_with("doi:");
        // A line broken mid-word is body text, always.
        let hyphenated = plain.ends_with('-');
        // Prose gives itself away by trailing off: a real heading ends on a
        // capitalized word, a number, or terminal punctuation — never on a
        // lowercase word part-way through a sentence. Short lines are exempt;
        // one- to three-word lowercase headings are ordinary.
        let trails_off = words.len() >= 4
            && words
                .last()
                .and_then(|w| w.chars().find(|c| c.is_alphabetic()))
                .is_some_and(|c| c.is_lowercase());
        if stamp || hyphenated || trails_off {
            out.push_str(&plain);
        } else {
            out.push_str(&"#".repeat(level));
            out.push(' ');
            out.push_str(&plain);
        }
        out.push('\n');
    }
    out
}

/// Drop HTML tags and markdown emphasis from a heading's text, leaving the
/// words. Headings are rendered as plain text in the TOC, so `<u>` and `**`
/// only ever show up as noise there.
fn strip_inline_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            '*' | '_' if !in_tag => {}
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Extract per-page Markdown from PDF bytes already in memory — the path a
/// PDF fetched over HTTP takes, with no temp file in between.
pub fn extract_text_mem(bytes: &[u8]) -> Result<PdfText> {
    let result = pdf_inspector::extract_pages_markdown_mem(bytes, None)
        .map_err(|e| anyhow!("failed to read PDF: {e}"))?;
    Ok(collect_pages(result))
}

/// Do these bytes begin with the PDF magic number? `%PDF-` may sit a few
/// bytes in on files with a leading BOM or stray whitespace.
pub fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(1024)]
        .windows(5)
        .any(|w| w == b"%PDF-")
}

/// Render page one of an in-memory PDF — the gallery thumbnail for a PDF
/// that lives at a URL rather than on disk.
pub fn render_first_page_mem(bytes: &[u8], target_width: i32) -> Result<Vec<u8>> {
    let bindings = bind_pdfium()?;
    let pdfium = Pdfium::new(bindings);
    let document = pdfium
        .load_pdf_from_byte_slice(bytes, None)
        .context("failed to open PDF")?;
    let rendered = document
        .pages()
        .first()
        .context("PDF has no pages")?
        .render_with_config(&PdfRenderConfig::new().set_target_width(target_width))
        .context("failed to render the first page")?
        .as_image();
    let mut png = Vec::new();
    rendered
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .context("failed to encode page image")?;
    Ok(png)
}

/// The document's /Title tag, cleaned up. Best-effort like `pdf_author`.
/// Worth asking for on a PDF pulled off a URL, where the alternative title is
/// whatever the path happens to end in ("2307.03172").
pub fn pdf_title_mem(bytes: &[u8]) -> Option<String> {
    let bindings = bind_pdfium().ok()?;
    let pdfium = Pdfium::new(bindings);
    let document = pdfium.load_pdf_from_byte_slice(bytes, None).ok()?;
    let title = document
        .metadata()
        .get(PdfDocumentMetadataTagType::Title)?
        .value()
        .trim()
        .to_string();
    // Producers love to leave the LaTeX job name or a temp path in /Title;
    // anything with no space in it is more likely that than a real title.
    (!title.is_empty() && title.contains(' ')).then_some(title)
}

/// How many pages the document has, without extracting any text (~10-50ms).
/// Best-effort: an unreadable file reads as no pages.
pub fn page_count(path: &str) -> usize {
    pdf_inspector::detect_pdf(path)
        .map(|info| info.page_count as usize)
        .unwrap_or(0)
}

/// Render up to `max_pages` pages of a PDF to PNG-encoded images, scaled to
/// roughly `target_width` pixels wide (good detail for OCR).
pub fn render_pdf_pages(path: &str, max_pages: usize, target_width: i32) -> Result<Vec<Vec<u8>>> {
    let bindings = bind_pdfium()?;
    let pdfium = Pdfium::new(bindings);
    let document = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("failed to open PDF {path}"))?;

    let config = PdfRenderConfig::new().set_target_width(target_width);
    let mut pages = Vec::new();
    for (i, page) in document.pages().iter().enumerate() {
        if i >= max_pages {
            break;
        }
        let image = page
            .render_with_config(&config)
            .with_context(|| format!("failed to render PDF page {}", i + 1))?
            .as_image();
        let mut png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .context("failed to encode page image")?;
        pages.push(png);
    }
    Ok(pages)
}

/// Render one page (1-indexed) to a PNG, scaled to roughly `target_width`
/// pixels. Backs the reader's page view, which asks for pages as they scroll
/// into sight rather than rasterizing a 300-page document up front.
pub fn render_page(path: &str, page: usize, target_width: i32) -> Result<Vec<u8>> {
    let bindings = bind_pdfium()?;
    let pdfium = Pdfium::new(bindings);
    let document = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("failed to open PDF {path}"))?;
    let index = u16::try_from(page.saturating_sub(1))
        .map_err(|_| anyhow!("page {page} is out of range for {path}"))?;
    let rendered = document
        .pages()
        .get(index)
        .with_context(|| format!("no page {page} in {path}"))?
        .render_with_config(&PdfRenderConfig::new().set_target_width(target_width))
        .with_context(|| format!("failed to render PDF page {page}"))?
        .as_image();
    let mut png = Vec::new();
    rendered
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .context("failed to encode page image")?;
    Ok(png)
}

/// The document's /Author tag, if the PDF carries one. Best-effort: any
/// failure (no PDFium, malformed file) reads as "no author" — authorship is
/// garnish on the properties panel, never worth failing an ingest over.
pub fn pdf_author(path: &str) -> Option<String> {
    let bindings = bind_pdfium().ok()?;
    let pdfium = Pdfium::new(bindings);
    let document = pdfium.load_pdf_from_file(path, None).ok()?;
    let author = document
        .metadata()
        .get(PdfDocumentMetadataTagType::Author)?
        .value()
        .trim()
        .to_string();
    (!author.is_empty()).then_some(author)
}

/// Locate and bind to the PDFium library across dev and bundled layouts.
fn bind_pdfium() -> Result<Box<dyn PdfiumLibraryBindings>> {
    let mut dirs: Vec<String> = Vec::new();
    if let Ok(dir) = std::env::var("ALCHEMY_PDFIUM_DIR") {
        dirs.push(dir);
    }
    // Dev: `tauri dev` runs with cwd = src-tauri.
    dirs.push("./libs".to_string());
    // Bundled: alongside or near the executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_string_lossy().into_owned());
            dirs.push(parent.join("libs").to_string_lossy().into_owned());
            // macOS .app: Contents/MacOS/<bin> -> Contents/Resources/libs
            if let Some(contents) = parent.parent() {
                dirs.push(
                    contents
                        .join("Resources/libs")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }

    for dir in &dirs {
        let name = Pdfium::pdfium_platform_library_name_at_path(dir);
        if let Ok(bindings) = Pdfium::bind_to_library(&name) {
            return Ok(bindings);
        }
    }
    Err(anyhow!(
        "could not load PDFium (libpdfium) for PDF rasterization — searched: {}",
        dirs.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    /// Eyeball the extraction on a real document:
    /// `ALCHEMY_TEST_PDF=/path/to.pdf cargo test --lib pdf::tests -- --ignored --nocapture`
    /// Two-column papers are the case worth checking — reading order and
    /// table reconstruction are exactly what the plain-text readers lose.
    #[test]
    #[ignore = "needs a PDF at $ALCHEMY_TEST_PDF"]
    fn dump_extraction() {
        let Ok(path) = std::env::var("ALCHEMY_TEST_PDF") else {
            return;
        };
        let text = super::extract_text(&path).expect("extraction failed");
        println!(
            "pages: {}  needing OCR: {:?}",
            text.pages.len(),
            text.pages_needing_ocr
        );
        let md = text.markdown();
        let out = format!("{path}.md");
        std::fs::write(&out, &md).expect("write markdown");
        println!("chars: {} -> {out}", md.len());
    }

    /// Font-size heading detection promotes things that wreck the reader's
    /// table of contents. Every case here is verbatim from arXiv:2307.03172.
    #[test]
    fn tidy_headings_demotes_non_headings() {
        let tidy = |s: &str| super::tidy_headings(s).trim_end().to_string();

        // The stamp printed sideways down page one — biggest type on the
        // page, never the document's structure.
        assert_eq!(
            tidy("# arXiv:2307.03172v3 [cs.CL] 20 Nov 2023"),
            "arXiv:2307.03172v3 [cs.CL] 20 Nov 2023"
        );
        // A bold run-in lead sentence, trailing off mid-thought.
        assert_eq!(
            tidy("## Our results indicate that prompting language"),
            "Our results indicate that prompting language"
        );
        // Body text broken across a line at a hyphen.
        assert_eq!(
            tidy("## Extended-context models are not necessarily bet-"),
            "Extended-context models are not necessarily bet-"
        );
        // A table header row that reached the markdown as a heading.
        assert_eq!(
            tidy("## <u>10 docs 20 docs 30 docs</u>"),
            "10 docs 20 docs 30 docs"
        );

        // ...and the real headings all survive, at their original level.
        for real in [
            "## Lost in the Middle: How Language Models Use Long Contexts",
            "### Abstract",
            "### 1 Introduction",
            "## 2.1 Experimental Setup",
            "# 3 How Well Can Language Models Retrieve From Input Contexts?",
            "# References",
            "## G.2 20 Total Retrieved Documents",
        ] {
            assert_eq!(tidy(real), real, "should have kept: {real}");
        }
    }

    /// A PDF served straight off a URL (arxiv.org/pdf/... is the everyday
    /// case) must be read as a PDF, not decoded as lossy UTF-8 and handed to
    /// readability — that produced a huge binary-garbage source and hung the
    /// app chunking it.
    #[tokio::test]
    #[ignore = "hits the network"]
    async fn url_pdf_extracts_as_pdf() {
        let ex = crate::ingest::extract_url("https://arxiv.org/pdf/2307.03172")
            .await
            .expect("extraction failed");
        assert_eq!(ex.source_type, "pdf");
        assert_eq!(
            ex.title, "Lost in the Middle: How Language Models Use Long Contexts",
            "the paper's title, not the arXiv id from the URL"
        );
        assert!(
            ex.text.contains("Lost in the Middle"),
            "expected the paper's title in the text, got: {}",
            &ex.text[..ex.text.len().min(300)]
        );
        // The old path produced hundreds of KB of mojibake. Real extracted
        // text from a 15-page paper lands well under that.
        assert!(
            ex.text.len() < 200_000,
            "suspiciously large extraction: {} bytes",
            ex.text.len()
        );
        println!("title: {}\nchars: {}", ex.title, ex.text.len());
    }

    /// The reader's page view end to end: PDFium binds, page 1 rasterizes,
    /// and what comes back is a real PNG rather than an empty buffer.
    /// `ALCHEMY_TEST_PDF=/path/to.pdf cargo test --lib pdf::tests -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a PDF at $ALCHEMY_TEST_PDF and the bundled PDFium"]
    fn renders_a_page() {
        let Ok(path) = std::env::var("ALCHEMY_TEST_PDF") else {
            return;
        };
        let png = super::render_page(&path, 1, 800).expect("render failed");
        assert_eq!(
            &png[..8],
            b"\x89PNG\r\n\x1a\n",
            "not a PNG: {:?}",
            &png[..8.min(png.len())]
        );
        let out = format!("{path}.page1.png");
        std::fs::write(&out, &png).expect("write png");
        println!("page 1: {} bytes -> {out}", png.len());
        assert!(super::page_count(&path) > 0, "page_count should be > 0");
    }
}
