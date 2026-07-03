use std::path::Path;
use std::process::Command;

fn main() {
    ensure_pdfium();
    tauri_build::build()
}

/// The bundled PDFium dylib (scanned-PDF OCR) isn't tracked in git — fetch it on
/// first build so a fresh clone just works. The script is idempotent, so this is
/// a cheap file-exists check on every subsequent build. macOS targets only.
fn ensure_pdfium() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let dylib = Path::new(&manifest).join("libs/libpdfium.dylib");
    if dylib.exists() {
        return;
    }
    let script = Path::new(&manifest).join("../scripts/fetch-pdfium.sh");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    match Command::new("sh").arg(&script).arg(&arch).status() {
        Ok(s) if s.success() => {}
        _ => println!(
            "cargo:warning=Could not fetch libpdfium.dylib — scanned-PDF OCR will be \
             unavailable. Run scripts/fetch-pdfium.sh manually."
        ),
    }
}
