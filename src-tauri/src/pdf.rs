//! PDF page rasterization via PDFium, used to OCR scanned/image-only PDFs.
//! The libpdfium dynamic library is bundled under `src-tauri/libs/`.

use std::io::Cursor;

use anyhow::{anyhow, Context, Result};
use pdfium_render::prelude::*;

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
                dirs.push(contents.join("Resources/libs").to_string_lossy().into_owned());
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
