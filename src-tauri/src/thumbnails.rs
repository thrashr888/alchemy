//! Gallery images cross IPC only after downsampling. Limit native decoders
//! across all windows, not just requests within one gallery.
use anyhow::{Context, Result};
use std::io::Write;

pub static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

pub async fn downsample(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut input = tempfile::NamedTempFile::new()?;
    input.write_all(bytes)?;
    let output = tempfile::NamedTempFile::with_suffix(".png")?;
    let mut child = tokio::process::Command::new("sips")
        .args(["-s", "format", "png", "-Z", "480"])
        .arg(input.path())
        .arg("-o")
        .arg(output.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let status = tokio::time::timeout(std::time::Duration::from_secs(20), child.wait())
        .await
        .context("Thumbnail conversion timed out")??;
    anyhow::ensure!(status.success(), "Thumbnail conversion failed");
    let png = std::fs::read(output.path())?;
    // Reject a malformed/oversized result instead of sending the original.
    anyhow::ensure!(png.len() <= 2 * 1024 * 1024, "Thumbnail exceeds byte limit");
    let image = image::ImageReader::new(std::io::Cursor::new(&png))
        .with_guessed_format()?
        .into_dimensions()?;
    anyhow::ensure!(
        image.0 <= 480 && image.1 <= 480,
        "Thumbnail exceeds dimensions"
    );
    Ok(png)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn large_transparent_image_becomes_small_thumbnail() {
        let _slot = SLOTS.acquire().await.unwrap();
        let image = image::RgbaImage::from_fn(2400, 1600, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x * y) % 239) as u8, 128])
        });
        let mut original = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut original, image::ImageFormat::Png)
            .unwrap();
        let thumb = downsample(original.get_ref()).await.unwrap();
        let decoded = image::load_from_memory(&thumb).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (480, 320));
        assert!(decoded.color().has_alpha());
        assert!(thumb.len() < original.get_ref().len());
        eprintln!(
            "thumbnail encoded bytes: {} -> {}; decoded pixels: {} -> {}",
            original.get_ref().len(),
            thumb.len(),
            2400 * 1600,
            480 * 320
        );
    }
    #[tokio::test]
    async fn invalid_image_never_falls_back_to_original() {
        let _slot = SLOTS.acquire().await.unwrap();
        assert!(downsample(b"not an image").await.is_err());
    }
}
