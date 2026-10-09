//! Cover images, in bulk (docs/RFC-canvas.md, "Cover images, in bulk").
//!
//! A source with no cover draws as a grey rectangle in the Gallery. The
//! import-time pick (`covers::pick_cover`) closes most of that gap for new
//! pages; this is the hand tool for the ones already in the store. A scan
//! looks for candidates in what the source already holds (or, for a web
//! page, the page itself, fetched once and bounded), proposes them, and
//! changes nothing. Apply writes only the picks a person made, and only
//! onto sources that still have no cover.

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::State;

use super::*;
use crate::covers;

/// Sources scanned per call. A web source costs a page fetch, so a call is
/// bounded; the sheet calls again for the next batch.
const SCAN_DEFAULT: usize = 12;
const SCAN_MAX: usize = 25;
/// Page fetches in flight at once.
const SCAN_PARALLEL: usize = 4;

/// One source's scan: what was found and, when it is short of what was
/// hoped, why in a sentence.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoverScan {
    pub source_id: String,
    pub title: String,
    pub source_type: String,
    /// Absolute image URLs, best first.
    pub candidates: Vec<String>,
    /// "" when the scan went as planned.
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverScanReport {
    pub rows: Vec<CoverScan>,
    /// Sources in the notebook with no cover, before this call's batch.
    pub missing: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverPick {
    pub source_id: String,
    /// A web image URL to use as the source's cover.
    pub image_url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoverSkip {
    pub source_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverApplyReport {
    pub applied: u32,
    pub skipped: Vec<CoverSkip>,
}

/// Does this source draw a cover from `image_url` and have none? PDFs and
/// local images render their own thumbnails, and a folder is only a parent.
pub(crate) fn needs_cover(s: &Source) -> bool {
    !matches!(
        s.source_type.as_str(),
        "pdf" | "image" | "folder" | "git" | "notion" | "obsidian" | "okf" | "feed"
    ) && matches!(s.image_url.trim(), "" | "-")
}

/// Candidates from a source's own saved text: image references in the
/// Markdown or HTML it holds, resolved against its web address when it has
/// one.
fn stored_candidates(src: &Source, content: &str) -> Vec<String> {
    let base = Some(src.url.as_str()).filter(|u| is_web_url(u));
    covers::content_images(content, base)
}

async fn scan_one(state: &AppState, src: Source) -> CoverScan {
    let mut row = CoverScan {
        source_id: src.id.clone(),
        title: src.title.clone(),
        source_type: src.source_type.clone(),
        ..CoverScan::default()
    };
    let content = match state.db.get_source(&src.id).await {
        Ok(Some(full)) => full.content,
        _ => String::new(),
    };
    let mut found: Vec<String> = Vec::new();
    let mut why = "";
    if src.source_type == "url" && is_web_url(&src.url) {
        match ingest::fetch_page_html(&src.url).await {
            Ok(html) => found.extend(covers::page_cover_candidates(&html, &src.url)),
            Err(reason) => why = reason,
        }
    }
    found.extend(stored_candidates(&src, &content));
    let mut seen = std::collections::HashSet::new();
    found.retain(|u| seen.insert(u.clone()));
    found.truncate(covers::MAX_CANDIDATES);

    row.note = match (found.is_empty(), why.is_empty()) {
        (true, true) => "No images found".to_string(),
        (true, false) => why.to_string(),
        (false, true) => String::new(),
        (false, false) => format!("{why}; showing images from the saved text"),
    };
    row.candidates = found;
    row
}

/// Scan a notebook's cover-less sources for candidate images. `source_ids`
/// narrows the scan (the sheet asks for a few at a time so it can show
/// progress); without it the first `limit` cover-less sources are scanned.
/// Web pages are fetched, a handful at a time, each with its own short
/// timeout. Nothing is changed.
pub(crate) async fn scan_cover_images_impl(
    state: &AppState,
    notebook_id: &str,
    source_ids: Option<&[String]>,
    limit: Option<usize>,
) -> anyhow::Result<CoverScanReport> {
    let mut targets: Vec<Source> = state
        .db
        .list_sources(notebook_id)
        .await?
        .into_iter()
        .filter(needs_cover)
        .collect();
    // Web pages first: they are where the covers went missing.
    targets.sort_by_key(|s| s.source_type != "url");
    let missing = targets.len() as u32;
    if let Some(ids) = source_ids {
        targets.retain(|s| ids.contains(&s.id));
    }
    targets.truncate(limit.unwrap_or(SCAN_DEFAULT).clamp(1, SCAN_MAX));
    let rows = futures::stream::iter(targets)
        .map(|s| scan_one(state, s))
        .buffered(SCAN_PARALLEL)
        .collect::<Vec<_>>()
        .await;
    Ok(CoverScanReport { rows, missing })
}

/// Why a pick cannot be applied, or `None` when it can.
fn refusal(src: Option<&Source>, notebook_id: &str, image_url: &str) -> Option<&'static str> {
    let Some(src) = src else {
        return Some("Source not found");
    };
    if src.notebook_id != notebook_id {
        return Some("Source is in another notebook");
    }
    if !needs_cover(src) {
        return Some("Already has a cover");
    }
    if !is_web_url(image_url.trim()) {
        return Some("The image must be a web URL");
    }
    None
}

/// Write the chosen images onto their sources. Each pick is checked against
/// the source as it stands now: one that gained a cover since the scan, moved
/// to another notebook or points at something that is not a web image is
/// skipped and reported, never forced.
pub(crate) async fn apply_cover_images_impl(
    state: &AppState,
    notebook_id: &str,
    picks: &[CoverPick],
) -> anyhow::Result<CoverApplyReport> {
    let mut applied = 0u32;
    let mut skipped = Vec::new();
    for pick in picks {
        let src = state.db.get_source_summary(&pick.source_id).await?;
        if let Some(reason) = refusal(src.as_ref(), notebook_id, &pick.image_url) {
            skipped.push(CoverSkip {
                source_id: pick.source_id.clone(),
                reason: reason.to_string(),
            });
            continue;
        }
        state
            .db
            .set_source_image(&pick.source_id, pick.image_url.trim())
            .await?;
        let _ = std::fs::remove_file(og_cache_path(state, &pick.source_id));
        let _ = std::fs::remove_file(legacy_og_cache_path(state, &pick.source_id));
        applied += 1;
    }
    if applied > 0 {
        state.db.touch_notebook(notebook_id, now()).await?;
    }
    Ok(CoverApplyReport { applied, skipped })
}

#[tauri::command]
pub async fn scan_cover_images(
    state: State<'_, AppState>,
    notebook_id: String,
    source_ids: Option<Vec<String>>,
    limit: Option<u32>,
) -> Result<CoverScanReport, String> {
    e(scan_cover_images_impl(
        &state,
        &notebook_id,
        source_ids.as_deref(),
        limit.map(|n| n as usize),
    )
    .await)
}

#[tauri::command]
pub async fn apply_cover_images(
    state: State<'_, AppState>,
    notebook_id: String,
    picks: Vec<CoverPick>,
) -> Result<CoverApplyReport, String> {
    e(apply_cover_images_impl(&state, &notebook_id, &picks).await)
}

/// A stock photo for a notebook cover, seeded so the same notebook always
/// draws the same picture. picsum.photos serves Unsplash photos with no key;
/// it is fetched here rather than in the webview because a cross-origin
/// image cannot be read back off a canvas, and the front end stylizes the
/// photo (dither, ASCII) before it is shown. A data URL never taints.
#[tauri::command]
pub async fn cover_photo(seed: String, width: u32, height: u32) -> Result<String, String> {
    if !(16..=1024).contains(&width) || !(16..=1024).contains(&height) {
        return Err("Cover size out of range".into());
    }
    let seed: String = seed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect();
    let url = format!("https://picsum.photos/seed/{seed}/{width}/{height}");
    let bytes = crate::ingest::fetch_bytes(&url, 2 * 1024 * 1024)
        .await
        .ok_or("Couldn't fetch a cover photo")?;
    use base64::Engine;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(source_type: &str, image_url: &str) -> Source {
        Source {
            id: "s1".into(),
            notebook_id: "nb".into(),
            source_type: source_type.into(),
            image_url: image_url.into(),
            ..Source::default()
        }
    }

    #[test]
    fn who_needs_a_cover() {
        assert!(needs_cover(&src("url", "")));
        assert!(needs_cover(&src("url", "-")));
        assert!(needs_cover(&src("markdown", "")));
        assert!(!needs_cover(&src("url", "https://x.example.com/a.png")));
        // Rendered thumbnails and containers are not cover-less.
        assert!(!needs_cover(&src("pdf", "")));
        assert!(!needs_cover(&src("image", "")));
        assert!(!needs_cover(&src("folder", "")));
    }

    #[test]
    fn stored_text_candidates_resolve_against_the_page() {
        let mut s = src("url", "-");
        s.url = "https://blog.example.com/p/one".into();
        let got = stored_candidates(
            &s,
            "Intro\n\n![a](/img/a.png)\n\n![b](https://cdn.example.com/b.jpg)",
        );
        assert_eq!(
            got,
            vec![
                "https://blog.example.com/img/a.png",
                "https://cdn.example.com/b.jpg"
            ]
        );
        // A file's relative image has nothing to resolve against.
        let mut f = src("markdown", "");
        f.url = "/Users/me/notes/a.md".into();
        assert!(stored_candidates(&f, "![x](pics/x.png)").is_empty());
    }

    #[test]
    fn apply_refuses_what_changed_since_the_scan() {
        let ok = src("url", "-");
        let img = "https://x.example.com/a.png";
        assert_eq!(refusal(Some(&ok), "nb", img), None);
        assert_eq!(refusal(None, "nb", img), Some("Source not found"));
        assert_eq!(
            refusal(Some(&ok), "other", img),
            Some("Source is in another notebook")
        );
        let covered = src("url", "https://x.example.com/had.png");
        assert_eq!(
            refusal(Some(&covered), "nb", img),
            Some("Already has a cover")
        );
        assert_eq!(
            refusal(Some(&ok), "nb", "file:///etc/passwd"),
            Some("The image must be a web URL")
        );
    }
}
