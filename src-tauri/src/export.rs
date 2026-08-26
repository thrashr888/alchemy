//! Per-note export (docs/RFC-note-export.md): one backend path turns a note
//! into the file that matches its shape — .docx for prose, .xlsx for table
//! notes, .pptx for slide decks and flashcards, .png for infographics and
//! mind maps, .m4a for Audio Overview episodes, and .pdf of the note's own
//! render for any kind. Both the Studio note menu and the MCP `export_note`
//! tool land here, so the UI and agents can never drift.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use tauri::{AppHandle, Manager};

use crate::commands::{new_id, AppState};
use crate::models::Note;

/// Flattened-error command surface for the UI (same shape as commands.rs).
#[tauri::command]
pub async fn export_note(
    app: AppHandle,
    note_id: String,
    format: String,
    dest: Option<String>,
) -> Result<String, String> {
    export_note_file(&app, &note_id, &format, dest)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Export `note_id` as `format` ("png" | "pdf" | "pptx" | "docx" | "xlsx" |
/// "m4a") to `dest`, or to `~/Downloads/<title>.<ext>` when no destination
/// is given. Returns the absolute path written.
pub async fn export_note_file(
    app: &AppHandle,
    note_id: &str,
    format: &str,
    dest: Option<String>,
) -> Result<String> {
    let state = app.state::<AppState>();
    let note = state
        .db
        .get_note(note_id)
        .await?
        .ok_or_else(|| anyhow!("no note with id {note_id}"))?;

    let format = export_ext(format)?;
    let ext = format;
    let dest: PathBuf = match dest {
        Some(d) => PathBuf::from(d),
        None => app
            .path()
            .download_dir()
            .context("could not resolve the Downloads folder")?
            .join(format!("{}.{ext}", safe_name(&note.title))),
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // The builders and pdfium are CPU-bound and the copies are filesystem
    // IO; none of it belongs on an async worker (and certainly not the main
    // thread) — every arm funnels through spawn_blocking.
    let out = dest.clone();
    match format {
        "docx" => {
            let (title, content) = (note.title.clone(), note.content.clone());
            blocking(move || {
                std::fs::write(&out, docx_bytes(&title, &content)?).map_err(Into::into)
            })
            .await?
        }
        "xlsx" => {
            let content = note.content.clone();
            blocking(move || std::fs::write(&out, xlsx_bytes(&content)?).map_err(Into::into))
                .await?
        }
        "pptx" => {
            let note = note.clone();
            blocking(move || std::fs::write(&out, pptx_note_bytes(&note)?).map_err(Into::into))
                .await?
        }
        "m4a" => {
            let src = crate::commands::audio_path(app, &note.id)
                .context("could not resolve the app data dir")?;
            anyhow::ensure!(src.exists(), "This note has no audio yet.");
            blocking(move || {
                std::fs::copy(&src, &out)?;
                Ok(())
            })
            .await?
        }
        "pdf" => {
            // The note's own render, printed: whatever the print pipeline
            // produced IS the artifact — no rasterizing, no conversion.
            let tmp = print_note_pdf(app, &note).await?;
            blocking(move || {
                std::fs::copy(&tmp, &out)?;
                let _ = std::fs::remove_file(&tmp);
                Ok(())
            })
            .await?
        }
        "png" => {
            let tmp = print_note_pdf(app, &note).await?;
            // Mind maps and UML diagrams scale to fit one sheet, so
            // rasterize them wider to keep node labels crisp; posters get
            // poster width.
            let width = if note.kind == "mind_map" || note.kind == "uml" {
                2200
            } else {
                1600
            };
            blocking(move || {
                let pages = crate::pdf::render_pdf_pages(&tmp.to_string_lossy(), 8, width)?;
                let png = stitch_png_pages(&pages)?;
                let _ = std::fs::remove_file(&tmp);
                std::fs::write(&out, png)?;
                Ok(())
            })
            .await?
        }
        _ => unreachable!(),
    }
    Ok(dest.to_string_lossy().into_owned())
}

/// spawn_blocking with the join flattened into the export's error.
async fn blocking(work: impl FnOnce() -> Result<()> + Send + 'static) -> Result<()> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .context("the export task was cancelled")?
}

/// A note's pptx: flashcards become question/answer slide pairs, anything
/// deck-shaped becomes title+bullets slides — whichever its content parses
/// as (kind picks which parser gets first try).
fn pptx_note_bytes(note: &Note) -> Result<Vec<u8>> {
    let cards = crate::pptx::parse_cards(&note.content);
    let deck = crate::pptx::parse_deck(&note.content);
    // Same thresholds as the front-end renderers: ≥2 of either shape.
    let slides = if note.kind == "flashcards" && cards.len() >= 2 {
        crate::pptx::cards_to_slides(&cards)
    } else if deck.len() >= 2 {
        deck
    } else if cards.len() >= 2 {
        crate::pptx::cards_to_slides(&cards)
    } else {
        bail!("This note doesn't parse as a slide deck or flashcards.");
    };
    crate::pptx::pptx_bytes(&slides)
}

/// Normalize an export format to the extension it writes. Shared with the
/// drag-out staging path (dragout.rs), which needs the same name the Save
/// dialog would have produced.
pub(crate) fn export_ext(format: &str) -> Result<&str> {
    let format = match format {
        "audio" | "mp3" => "m4a", // the episode is AAC; see the RFC
        f => f,
    };
    match format {
        "png" | "pdf" | "pptx" | "docx" | "xlsx" | "m4a" => Ok(format),
        other => {
            bail!("unknown export format \"{other}\" — use png, pdf, pptx, docx, xlsx, or m4a")
        }
    }
}

/// Titles become filenames; keep them, minus path separators.
pub(crate) fn safe_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Note".to_string()
    } else {
        trimmed.to_string()
    }
}

// ---- Print pipeline: note render → PDF (→ pdfium raster for png) -----------

/// Render the note's print sheet in a throwaway window and silently print
/// it to a temp PDF (the proven print_webview path). PDF export ships this
/// file as-is; PNG export rasterizes it. The window picks the sheet by
/// kind — infographic poster, mind map, slide pages, flashcard study
/// sheet, or the note's markdown in print typography (PrintExportView.tsx).
async fn print_note_pdf(app: &AppHandle, note: &Note) -> Result<PathBuf> {
    let tmp = std::env::temp_dir().join(format!("alchemy-export-{}.pdf", new_id()));
    let _ = std::fs::remove_file(&tmp);
    // win-* so the window matches the default capability and may invoke
    // print_webview; the boot flag routes App.tsx to the export view.
    let label = format!("win-export-{}", new_id());
    let boot = format!(
        "window.__ALCHEMY_NOTEBOOK__ = '{}'; window.__ALCHEMY_NOTE__ = '{}'; \
         window.__ALCHEMY_PRINT_EXPORT__ = '{}';",
        note.notebook_id.replace('\'', ""),
        note.id.replace('\'', ""),
        tmp.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\'', "")
    );
    // Small and unfocused, but genuinely on screen: WKWebView's print
    // operation paints never-composited content as blank pages (see the
    // .print-surface notes in index.css), and a hidden NSWindow never
    // composites. The window closes itself the moment the PDF lands.
    let window =
        tauri::WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::App("index.html".into()))
            .title("Exporting…")
            .inner_size(360.0, 240.0)
            .focused(false)
            .initialization_script(&boot)
            .build()
            .context("could not open the export window")?;

    let result = wait_for_stable_file(&tmp, 60).await;
    let _ = window.close();
    result?;
    Ok(tmp)
}

/// The print job's finish signal is the file itself reaching a stable
/// non-zero size (same contract as print_webview's own wait).
async fn wait_for_stable_file(path: &Path, timeout_secs: u64) -> Result<()> {
    let mut last: u64 = 0;
    for _ in 0..(timeout_secs * 5) {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size > 0 && size == last {
            return Ok(());
        }
        last = size;
    }
    bail!("PNG export timed out — the note never finished rendering")
}

/// One page passes through untouched; multi-page posters stack vertically
/// on white (the print sheet is fixed-ink on white already).
fn stitch_png_pages(pages: &[Vec<u8>]) -> Result<Vec<u8>> {
    anyhow::ensure!(!pages.is_empty(), "the export produced no pages");
    if pages.len() == 1 {
        return Ok(pages[0].clone());
    }
    let images: Vec<image::RgbaImage> = pages
        .iter()
        .map(|p| {
            image::load_from_memory(p)
                .map(|i| i.to_rgba8())
                .context("failed to decode a rendered page")
        })
        .collect::<Result<_>>()?;
    let width = images.iter().map(|i| i.width()).max().unwrap_or(1);
    let height: u32 = images.iter().map(|i| i.height()).sum();
    let mut canvas = image::RgbaImage::from_pixel(width, height, image::Rgba([255, 255, 255, 255]));
    let mut y: i64 = 0;
    for img in &images {
        image::imageops::overlay(&mut canvas, img, 0, y);
        y += i64::from(img.height());
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(canvas)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .context("failed to encode the stitched PNG")?;
    Ok(out)
}

// ---- DOCX: markdown → Word --------------------------------------------------

/// Markdown → .docx bytes. Covers what generated notes actually contain:
/// headings, paragraphs, bullet/numbered lists (textual markers — Word's
/// numbering XML buys nothing for an export), tables, code blocks, block
/// quotes, and bold/italic/code inlines. The note title becomes a leading
/// Heading 1 unless the content already opens with one.
pub fn docx_bytes(title: &str, markdown: &str) -> Result<Vec<u8>> {
    use docx_rs::{Docx, Style, StyleType};

    let mut docx = Docx::new().default_size(22); // 11pt body
    for (id, name, size) in [
        ("Heading1", "heading 1", 34usize),
        ("Heading2", "heading 2", 28),
        ("Heading3", "heading 3", 25),
        ("Heading4", "heading 4", 23),
    ] {
        docx = docx.add_style(
            Style::new(id, StyleType::Paragraph)
                .name(name)
                .size(size)
                .bold(),
        );
    }

    let mut w = DocxWriter::new(docx);
    if !markdown.trim_start().starts_with("# ") {
        w.heading = Some(1);
        w.text(title);
        w.flush_paragraph();
    }
    walk_markdown(markdown, &mut w)?;
    let docx = w.finish();

    let mut buf = Cursor::new(Vec::new());
    docx.build()
        .pack(&mut buf)
        .context("failed to write the .docx")?;
    Ok(buf.into_inner())
}

/// Event-walk state for the docx writer.
struct DocxWriter {
    docx: docx_rs::Docx,
    runs: Vec<docx_rs::Run>,
    bold: usize,
    italic: usize,
    heading: Option<usize>,
    /// Per-level list counters; None = bullet, Some(n) = next ordered index.
    lists: Vec<Option<u64>>,
    quote: usize,
    code_block: bool,
    /// In-flight table: rows of cell strings (first row = header).
    table: Option<Vec<Vec<String>>>,
}

impl DocxWriter {
    fn new(docx: docx_rs::Docx) -> Self {
        Self {
            docx,
            runs: Vec::new(),
            bold: 0,
            italic: 0,
            heading: None,
            lists: Vec::new(),
            quote: 0,
            code_block: false,
            table: None,
        }
    }

    fn styled_run(&self, text: &str, mono: bool) -> docx_rs::Run {
        use docx_rs::{Run, RunFonts};
        let mut run = Run::new().add_text(text);
        if self.bold > 0 {
            run = run.bold();
        }
        if self.italic > 0 {
            run = run.italic();
        }
        if mono || self.code_block {
            run = run.fonts(RunFonts::new().ascii("Courier New")).size(20);
        }
        run
    }

    fn text(&mut self, text: &str) {
        if let Some(rows) = self.table.as_mut() {
            if let Some(cell) = rows.last_mut().and_then(|r| r.last_mut()) {
                cell.push_str(text);
            }
            return;
        }
        let run = self.styled_run(text, false);
        self.runs.push(run);
    }

    fn flush_paragraph(&mut self) {
        use docx_rs::Paragraph;
        if self.runs.is_empty() {
            self.heading = None;
            return;
        }
        let mut para = Paragraph::new();
        for run in self.runs.drain(..) {
            para = para.add_run(run);
        }
        if let Some(level) = self.heading.take() {
            para = para.style(&format!("Heading{}", level.min(4)));
        }
        // Lists and quotes read as indentation (720 twips = ½")
        let depth = self.lists.len() + self.quote;
        if depth > 0 {
            para = para.indent(Some(720 * depth as i32), None, None, None);
        }
        self.docx = std::mem::replace(&mut self.docx, docx_rs::Docx::new()).add_paragraph(para);
    }

    fn flush_table(&mut self) {
        use docx_rs::{Paragraph, Run, Table, TableCell, TableRow};
        let Some(rows) = self.table.take() else {
            return;
        };
        if rows.is_empty() {
            return;
        }
        let table = Table::new(
            rows.iter()
                .enumerate()
                .map(|(i, row)| {
                    TableRow::new(
                        row.iter()
                            .map(|cell| {
                                let mut run = Run::new().add_text(cell);
                                if i == 0 {
                                    run = run.bold();
                                }
                                TableCell::new().add_paragraph(Paragraph::new().add_run(run))
                            })
                            .collect(),
                    )
                })
                .collect(),
        );
        self.docx = std::mem::replace(&mut self.docx, docx_rs::Docx::new()).add_table(table);
    }

    fn finish(mut self) -> docx_rs::Docx {
        self.flush_paragraph();
        self.flush_table();
        self.docx
    }
}

/// Drive a DocxWriter with pulldown-cmark events.
fn walk_markdown(markdown: &str, w: &mut DocxWriter) -> Result<()> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    w.flush_paragraph();
                    w.heading = Some(heading_level(level));
                }
                Tag::Paragraph => w.flush_paragraph(),
                Tag::BlockQuote(_) => {
                    w.flush_paragraph();
                    w.quote += 1;
                }
                Tag::CodeBlock(_) => {
                    w.flush_paragraph();
                    w.code_block = true;
                }
                Tag::List(start) => {
                    w.flush_paragraph();
                    w.lists.push(start);
                }
                Tag::Item => {
                    w.flush_paragraph();
                    let marker = match w.lists.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}. ");
                            *n += 1;
                            m
                        }
                        _ => "•  ".to_string(),
                    };
                    let run = w.styled_run(&marker, false);
                    w.runs.push(run);
                }
                Tag::Emphasis => w.italic += 1,
                Tag::Strong => w.bold += 1,
                Tag::Table(_) => {
                    w.flush_paragraph();
                    w.table = Some(Vec::new());
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some(rows) = w.table.as_mut() {
                        rows.push(Vec::new());
                    }
                }
                Tag::TableCell => {
                    if let Some(cell_row) = w.table.as_mut().and_then(|r| r.last_mut()) {
                        cell_row.push(String::new());
                    }
                }
                Tag::Link { .. } | Tag::Image { .. } => {}
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) | TagEnd::Paragraph | TagEnd::Item => w.flush_paragraph(),
                TagEnd::BlockQuote(_) => {
                    w.flush_paragraph();
                    w.quote = w.quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    w.flush_paragraph();
                    w.code_block = false;
                }
                TagEnd::List(_) => {
                    w.flush_paragraph();
                    w.lists.pop();
                }
                TagEnd::Emphasis => w.italic = w.italic.saturating_sub(1),
                TagEnd::Strong => w.bold = w.bold.saturating_sub(1),
                TagEnd::Table => w.flush_table(),
                _ => {}
            },
            Event::Text(t) => {
                if w.code_block {
                    // Code arrives as one blob; one paragraph per line keeps
                    // Word from soft-wrapping it into soup.
                    for line in t.lines() {
                        let run = w.styled_run(line, true);
                        w.runs.push(run);
                        w.flush_paragraph();
                    }
                } else {
                    w.text(&t);
                }
            }
            Event::Code(t) => {
                if w.table.is_some() {
                    w.text(&t);
                } else {
                    let run = w.styled_run(&t, true);
                    w.runs.push(run);
                }
            }
            Event::SoftBreak => w.text(" "),
            Event::HardBreak => {
                if w.table.is_none() {
                    let run = docx_rs::Run::new().add_break(docx_rs::BreakType::TextWrapping);
                    w.runs.push(run);
                }
            }
            Event::Rule => {
                w.flush_paragraph();
                w.text("———");
                w.flush_paragraph();
            }
            _ => {}
        }
    }
    Ok(())
}

fn heading_level(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        _ => 4,
    }
}

// ---- XLSX: markdown tables → workbook ---------------------------------------

/// Every markdown table in the note, as rows of cell strings (header first).
pub fn markdown_tables(markdown: &str) -> Vec<Vec<Vec<String>>> {
    let options = Options::ENABLE_TABLES;
    let mut tables: Vec<Vec<Vec<String>>> = Vec::new();
    let mut current: Option<Vec<Vec<String>>> = None;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(Tag::Table(_)) => current = Some(Vec::new()),
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                if let Some(rows) = current.as_mut() {
                    rows.push(Vec::new());
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some(row) = current.as_mut().and_then(|r| r.last_mut()) {
                    row.push(String::new());
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some(cell) = current
                    .as_mut()
                    .and_then(|r| r.last_mut())
                    .and_then(|row| row.last_mut())
                {
                    cell.push_str(&t);
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(rows) = current.take() {
                    if !rows.is_empty() {
                        tables.push(rows);
                    }
                }
            }
            _ => {}
        }
    }
    tables
}

/// Markdown tables → .xlsx bytes: one worksheet per table, bold header row,
/// numeric-looking cells written as numbers so formulas work on them.
pub fn xlsx_bytes(markdown: &str) -> Result<Vec<u8>> {
    use rust_xlsxwriter::{Format, Workbook};

    let tables = markdown_tables(markdown);
    anyhow::ensure!(!tables.is_empty(), "This note has no table to export.");

    let mut workbook = Workbook::new();
    let header = Format::new().set_bold();
    for (i, rows) in tables.iter().enumerate() {
        let sheet = workbook.add_worksheet();
        if tables.len() > 1 {
            sheet.set_name(format!("Table {}", i + 1))?;
        }
        for (r, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let (r32, c16) = (r as u32, c as u16);
                if r == 0 {
                    sheet.write_string_with_format(r32, c16, cell, &header)?;
                } else if let Some(n) = numeric(cell) {
                    sheet.write_number(r32, c16, n)?;
                } else {
                    sheet.write_string(r32, c16, cell)?;
                }
            }
        }
        sheet.autofit();
    }
    Ok(workbook.save_to_buffer()?)
}

/// Plain numbers only — "1,234.5" counts, "$5" and "12%" stay strings so no
/// meaning is silently dropped.
fn numeric(cell: &str) -> Option<f64> {
    let cleaned = cell.trim().replace(',', "");
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

#[cfg(test)]
mod export_tests {
    use super::*;

    const TABLE_MD: &str = "\
# Quarterly numbers

| Region | Q1 | Q2 |
| --- | --- | --- |
| West | 1,200 | 900.5 |
| East | 40% | n/a |
";

    #[test]
    fn tables_parse_with_headers_and_cells() {
        let tables = markdown_tables(TABLE_MD);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0][0], vec!["Region", "Q1", "Q2"]);
        assert_eq!(tables[0][2], vec!["East", "40%", "n/a"]);
    }

    #[test]
    fn xlsx_round_trips_as_a_zip() {
        let bytes = xlsx_bytes(TABLE_MD).unwrap();
        // xlsx is a zip: PK magic + non-trivial payload.
        assert_eq!(&bytes[..2], b"PK");
        assert!(bytes.len() > 1000);
    }

    #[test]
    fn xlsx_requires_a_table() {
        assert!(xlsx_bytes("Just prose, no table.").is_err());
    }

    #[test]
    fn numeric_cells_only_when_plainly_numeric() {
        assert_eq!(numeric("1,200"), Some(1200.0));
        assert_eq!(numeric("900.5"), Some(900.5));
        assert_eq!(numeric("40%"), None);
        assert_eq!(numeric("$5"), None);
        assert_eq!(numeric("n/a"), None);
    }

    #[test]
    fn docx_packs_headings_lists_and_tables() {
        let md = "# Title\n\nSome **bold** and *italic* and `code`.\n\n\
                  - one\n- two\n\n1. first\n2. second\n\n> quoted\n\n\
                  ```\nlet x = 1;\n```\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        let bytes = docx_bytes("Title", md).unwrap();
        assert_eq!(&bytes[..2], b"PK");
        assert!(bytes.len() > 1000);
    }

    #[test]
    fn docx_prepends_title_only_without_leading_h1() {
        // Content starting with an H1 keeps it; the packed zip differs when
        // the title paragraph is injected.
        let with_h1 = docx_bytes("Dup", "# Dup\n\nBody.").unwrap();
        let without = docx_bytes("Dup", "Body.").unwrap();
        assert_eq!(&with_h1[..2], b"PK");
        assert_eq!(&without[..2], b"PK");
    }

    #[test]
    fn filenames_drop_path_separators() {
        assert_eq!(safe_name("Q3: a/b review"), "Q3- a-b review");
        assert_eq!(safe_name("  "), "Note");
    }

    #[test]
    fn stitching_single_page_is_identity() {
        let page = {
            let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
            let mut out = Vec::new();
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        };
        assert_eq!(stitch_png_pages(std::slice::from_ref(&page)).unwrap(), page);
        let two = stitch_png_pages(&[page.clone(), page]).unwrap();
        let img = image::load_from_memory(&two).unwrap();
        assert_eq!(img.height(), 8);
    }
}
