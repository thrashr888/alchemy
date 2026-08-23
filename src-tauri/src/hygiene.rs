//! Source hygiene (docs/RFC-source-hygiene.md): the check that notices
//! drifted sources and the budgeted sweep that fixes the reversible half.
//!
//! Split by reversibility, exactly like the note curator: re-fetching is
//! reversible, so aging url sources refresh automatically (budgeted through
//! the scheduler pass, the gist-sweep shape); removal is not, so dead links,
//! missing files, duplicates, and errored husks are only ever *proposed* —
//! `classify` buckets them, the sources panel badges them, and the review
//! modal (or an agent, via the `source_hygiene` MCP tool) decides.
//!
//! The sweep's refresh is non-destructive on failure: a transient timeout
//! keeps the last-good content and bumps `fetch_failures`, where the
//! user-initiated refresh path deliberately hard-fails (the errored row is
//! its retry affordance).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{self, AppState};
use crate::models::Source;

/// Consecutive background-probe failures before a url source stops being
/// retried and is proposed for removal instead.
pub const UNREACHABLE_AFTER: i64 = 3;
/// An errored, contentless source older than this is a husk worth clearing.
const HUSK_AFTER_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Url refreshes per sweep pass — a full re-fetch plus re-embed is the
/// expensive end of background work, so the budget is smaller than the gist
/// sweep's; anything left over waits for the next pass.
const SWEEP_BUDGET: usize = 3;
/// Minimum spacing between probe attempts on one source, so a failing URL
/// collects its `UNREACHABLE_AFTER` strikes across hours, not consecutive
/// minute ticks (an outage is not a dead link).
const RETRY_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// One sweep at a time, process-wide — the gist sweep's single-flight shape.
static SWEEPING: AtomicBool = AtomicBool::new(false);

/// (source_id → last probe attempt) this app run — the `remote_probe_due`
/// idiom, kept in-memory on purpose: probe pacing is scheduling state, not
/// data worth a column.
static LAST_ATTEMPT: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn attempt_due(source_id: &str) -> bool {
    let mut guard = LAST_ATTEMPT.lock().unwrap_or_else(|p| p.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    match map.get(source_id) {
        Some(last) if now.duration_since(*last) < RETRY_EVERY => false,
        _ => {
            map.insert(source_id.to_string(), now);
            true
        }
    }
}

/// One flagged source. `bucket` is the disposition class the UI and MCP
/// report group by: "unreachable" | "missing-file" | "duplicate" | "husk"
/// (proposed removals) and "stale" (informational — the sweep handles it).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HygieneIssue {
    pub source_id: String,
    pub title: String,
    pub bucket: String,
    pub detail: String,
}

/// Does this source's origin point at a local file on disk (vs. web or
/// cider://)? The type is asked first and the string second: a `url` source
/// whose stored origin doesn't parse as http (stray whitespace, a bare
/// domain) is still a web page, and calling it a missing file would flag a
/// perfectly live source for removal.
fn is_file_path(source: &Source) -> bool {
    let url = source.url.trim();
    !url.is_empty()
        && !matches!(source.source_type.as_str(), "url" | "mac")
        && !commands::is_web_url(url)
        && !crate::mac::is_mac_uri(url)
}

fn is_folder_like(source: &Source) -> bool {
    matches!(
        source.source_type.as_str(),
        "folder" | "obsidian" | "git" | "notion"
    )
}

/// Bucket a notebook's sources. Pure classification over the rows (plus
/// cheap fs stats for loose files) — no network, no mutation; callers decide
/// what to do with the result. One issue per source, most actionable bucket
/// wins.
pub fn classify(sources: &[Source], cadence_days: u32, now: i64) -> Vec<HygieneIssue> {
    let cadence_ms = i64::from(cadence_days).saturating_mul(86_400_000);
    // First-seen source per normalized URL; later holders of the same URL
    // are the duplicates (keep the oldest — it carries the history).
    let mut first_by_url: HashMap<String, &Source> = HashMap::new();
    let mut by_age: Vec<&Source> = sources.iter().collect();
    by_age.sort_by_key(|s| s.created_at);

    let mut issues = Vec::new();
    for s in by_age {
        if is_folder_like(s) {
            continue; // rescan owns parents; their children are below
        }
        let issue = |bucket: &str, detail: String| HygieneIssue {
            source_id: s.id.clone(),
            title: s.title.clone(),
            bucket: bucket.into(),
            detail,
        };
        if s.fetch_failures >= UNREACHABLE_AFTER {
            issues.push(issue(
                "unreachable",
                format!("{} refresh attempts failed", s.fetch_failures),
            ));
            continue;
        }
        // Loose files only: folder children that vanish are the rescan's to
        // reconcile, and an iCloud-evicted file (stub present) is
        // downloadable, not missing.
        if is_file_path(s) && s.parent_id.is_empty() {
            let p = std::path::Path::new(&s.url);
            let stub = p
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| p.with_file_name(format!(".{n}.icloud")));
            if !p.exists() && !stub.is_some_and(|st| st.exists()) {
                issues.push(issue(
                    "missing-file",
                    format!("file no longer exists at {}", s.url),
                ));
                continue;
            }
        }
        if s.source_type == "url" && !s.url.is_empty() {
            let key = s.url.trim().trim_end_matches('/').to_string();
            if let Some(first) = first_by_url.get(key.as_str()) {
                issues.push(issue(
                    "duplicate",
                    format!("same URL as \u{201c}{}\u{201d}", first.title),
                ));
                continue;
            }
            first_by_url.insert(key, s);
        }
        if s.status == "error" && s.char_count == 0 && now - s.created_at > HUSK_AFTER_MS {
            issues.push(issue("husk", "failed import with no content".into()));
            continue;
        }
        if s.source_type == "url" && cadence_ms > 0 && now - s.fetched_at > cadence_ms {
            let days = (now - s.fetched_at) / 86_400_000;
            issues.push(issue("stale", format!("last fetched {days} days ago")));
        }
    }
    issues
}

/// Fire-and-forget hygiene pass, called from the scheduler tick behind the
/// `background_enabled` gate. Single-flight and budgeted; a converged corpus
/// makes this a cheap metadata scan.
pub fn spawn_sweep(app: &AppHandle) {
    if SWEEPING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = run_sweep(&app).await {
            crate::diagnostics::error("hygiene", format!("sweep failed: {err:#}"));
        }
        SWEEPING.store(false, Ordering::SeqCst);
    });
}

async fn run_sweep(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    let (enabled, cadence_days) = {
        let ai = state.ai.read().await;
        let config = ai.config();
        (config.source_hygiene, config.hygiene_refresh_days)
    };
    if !enabled || cadence_days == 0 {
        return Ok(());
    }
    let cadence_ms = i64::from(cadence_days) * 86_400_000;
    let now = commands::now();
    let archived = state.db.archived_notebook_ids().await.unwrap_or_default();
    let mut urls = state.db.all_url_sources().await?;
    // Oldest fetch first: the budget goes to whatever has waited longest.
    urls.sort_by_key(|s| s.fetched_at);

    let mut refreshed: HashMap<String, u32> = HashMap::new();
    let mut budget = SWEEP_BUDGET;
    for src in &urls {
        if budget == 0 {
            break;
        }
        if archived.contains(&src.notebook_id)
            || src.status == "processing"
            || src.fetch_failures >= UNREACHABLE_AFTER
            || now - src.fetched_at < cadence_ms
            || !attempt_due(&src.id)
        {
            continue;
        }
        budget -= 1;
        match refresh_stale_url(&state, src).await {
            Ok(true) => *refreshed.entry(src.notebook_id.clone()).or_default() += 1,
            Ok(false) => {} // unchanged upstream — freshness stamped, nothing to announce
            // A single source failing is the expected shape of an outage or
            // a dead link, not an app fault: it is already recorded as a
            // strike on the row, so this is a note, not an error event.
            Err(err) => crate::note!(
                "hygiene: refresh of \u{201c}{}\u{201d} failed: {err:#}",
                src.title
            ),
        }
    }
    // Announce only successes: failures become badges (via fetch_failures)
    // on the next natural re-list rather than toasting every strike.
    for (notebook_id, updated) in refreshed {
        let _ = app.emit(
            "sources://changed",
            serde_json::json!({
                "notebookId": notebook_id,
                "added": 0, "updated": updated, "removed": 0, "failed": 0,
            }),
        );
    }
    Ok(())
}

/// Previous content large enough to be worth protecting from a collapse.
const COLLAPSE_FLOOR: usize = 500;
/// A re-fetch keeping less than this share of the previous content is
/// treated as a bad page rather than an edit.
const COLLAPSE_KEEP_RATIO: f64 = 0.5;

/// Did this re-fetch collapse the source's content?
///
/// A fetch can succeed and still be worthless: a moved page serving a
/// soft-404 ("this page has been moved or removed"), a paywall interstitial,
/// a consent wall. HTTP says 200, the extractor says a couple hundred
/// characters, and an unattended overwrite destroys the real copy — which is
/// the one thing this sweep must never do, because the old text is gone from
/// the row the moment reingest lands. Refusing costs a delayed update and a
/// flag the user can dismiss; accepting costs the source.
///
/// Mechanical on purpose (no model, no heuristics about wording): only a
/// substantial previous body is protected, and only a drastic shrink counts.
fn content_collapsed(before: &str, after: &str) -> bool {
    let before_len = before.chars().count();
    if before_len < COLLAPSE_FLOOR {
        return false;
    }
    let kept = after.chars().count() as f64 / before_len as f64;
    kept < COLLAPSE_KEEP_RATIO
}

/// Re-fetch one aging url source, non-destructively. Returns true when the
/// content changed and was reingested; false when the page is unchanged (the
/// freshness stamp still advances, without paying the re-embed). A fetch
/// failure — or a fetch that succeeds but comes back gutted — keeps the
/// last-good content and counts a strike, the opposite of the user-initiated
/// path's hard fail, because nobody is watching to retry.
async fn refresh_stale_url(state: &AppState, src: &Source) -> anyhow::Result<bool> {
    let existing = state
        .db
        .get_source(&src.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("source vanished mid-sweep"))?;
    match crate::capture::extract_url_rescued(&existing.url).await {
        Ok(extracted) => {
            if extracted.text == existing.content {
                state
                    .db
                    .set_source_fetch(&src.id, commands::now(), 0)
                    .await?;
                Ok(false)
            } else if content_collapsed(&existing.content, &extracted.text) {
                state
                    .db
                    .set_source_fetch(&src.id, existing.fetched_at, existing.fetch_failures + 1)
                    .await?;
                anyhow::bail!(
                    "page came back gutted ({} chars, was {}) — keeping the stored copy",
                    extracted.text.chars().count(),
                    existing.content.chars().count()
                )
            } else {
                commands::reingest(state, &existing, extracted, None, true).await?;
                Ok(true)
            }
        }
        Err(err) => {
            state
                .db
                .set_source_fetch(&src.id, existing.fetched_at, existing.fetch_failures + 1)
                .await?;
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    fn src(id: &str, source_type: &str) -> Source {
        Source {
            id: id.into(),
            notebook_id: "nb".into(),
            title: id.into(),
            source_type: source_type.into(),
            url: String::new(),
            content: String::new(),
            char_count: 100,
            chunk_count: 1,
            created_at: 0,
            status: "ready".into(),
            error: String::new(),
            parent_id: String::new(),
            mtime: 0,
            author: String::new(),
            image_url: String::new(),
            tags: String::new(),
            note: String::new(),
            fetched_at: 0,
            fetch_failures: 0,
        }
    }

    fn buckets(issues: &[HygieneIssue]) -> Vec<(&str, &str)> {
        issues
            .iter()
            .map(|i| (i.source_id.as_str(), i.bucket.as_str()))
            .collect()
    }

    /// A url past its cadence is "stale" (the sweep's own work); one still
    /// inside it is not flagged at all.
    #[test]
    fn stale_urls_flag_only_past_the_cadence() {
        let now = 100 * DAY;
        let mut fresh = src("fresh", "url");
        fresh.url = "https://example.com/a".into();
        fresh.fetched_at = now - 3 * DAY;
        let mut old = src("old", "url");
        old.url = "https://example.com/b".into();
        old.fetched_at = now - 45 * DAY;

        let issues = classify(&[fresh, old], 30, now);
        assert_eq!(buckets(&issues), vec![("old", "stale")]);
        assert!(issues[0].detail.contains("45 days"), "{}", issues[0].detail);
    }

    /// Repeated background failures outrank staleness — the source stops
    /// being retried and is proposed for removal instead.
    #[test]
    fn repeated_failures_flag_unreachable() {
        let now = 100 * DAY;
        let mut dead = src("dead", "url");
        dead.url = "https://example.com/gone".into();
        dead.fetched_at = now - 90 * DAY;
        dead.fetch_failures = UNREACHABLE_AFTER;

        let issues = classify(&[dead], 30, now);
        assert_eq!(buckets(&issues), vec![("dead", "unreachable")]);
    }

    /// The same URL added twice keeps the oldest and flags the newcomer.
    #[test]
    fn duplicate_urls_flag_the_newer_copy() {
        let now = 100 * DAY;
        let mut first = src("first", "url");
        first.url = "https://example.com/page/".into();
        first.created_at = 1;
        first.fetched_at = now;
        let mut second = src("second", "url");
        // Trailing slash and whitespace normalize to the same key.
        second.url = " https://example.com/page ".into();
        second.created_at = 2;
        second.fetched_at = now;

        let issues = classify(&[second, first], 30, now);
        assert_eq!(buckets(&issues), vec![("second", "duplicate")]);
        assert!(issues[0].detail.contains("first"), "{}", issues[0].detail);
    }

    /// A url source whose origin doesn't parse as http — stray whitespace, a
    /// bare domain — is still a web page, not a vanished local file. (Caught
    /// live: such a source was flagged "missing-file" and proposed for
    /// removal while the page was perfectly reachable.)
    #[test]
    fn odd_url_strings_are_never_missing_files() {
        let now = 100 * DAY;
        for origin in [" https://example.com/page ", "www.example.com/page"] {
            let mut s = src("odd", "url");
            s.url = origin.into();
            s.fetched_at = now;
            assert!(
                classify(&[s], 30, now).is_empty(),
                "{origin} must not be flagged"
            );
        }
    }

    /// An old errored import with no content is a husk; a recent one is not
    /// (the user may still be looking at the error).
    #[test]
    fn husks_need_age_and_emptiness() {
        let now = 100 * DAY;
        let mut old_husk = src("old-husk", "url");
        old_husk.status = "error".into();
        old_husk.char_count = 0;
        old_husk.created_at = now - 30 * DAY;
        old_husk.fetched_at = now;
        let mut fresh_error = src("fresh-error", "url");
        fresh_error.status = "error".into();
        fresh_error.char_count = 0;
        fresh_error.created_at = now - DAY;
        fresh_error.fetched_at = now;

        let issues = classify(&[old_husk, fresh_error], 30, now);
        assert_eq!(buckets(&issues), vec![("old-husk", "husk")]);
    }

    /// Folder-like parents are the rescan's business, never hygiene's.
    #[test]
    fn folder_parents_are_never_flagged() {
        let now = 100 * DAY;
        for kind in ["folder", "git", "notion", "obsidian"] {
            let mut parent = src(kind, kind);
            parent.url = "/some/path".into();
            parent.fetched_at = now - 400 * DAY;
            parent.fetch_failures = 99;
            assert!(
                classify(&[parent], 30, now).is_empty(),
                "{kind} parent must not be flagged"
            );
        }
    }

    /// Cadence 0 (the "off" setting) silences staleness without silencing
    /// the broken-source buckets.
    #[test]
    fn cadence_off_keeps_breakage_flags() {
        let now = 100 * DAY;
        let mut ancient = src("ancient", "url");
        ancient.url = "https://example.com/x".into();
        ancient.fetched_at = now - 900 * DAY;
        assert!(classify(&[ancient.clone()], 0, now).is_empty());

        ancient.fetch_failures = UNREACHABLE_AFTER;
        assert_eq!(
            buckets(&classify(&[ancient], 0, now)),
            vec![("ancient", "unreachable")]
        );
    }

    /// The soft-404 guard: a real page replaced by a "moved or removed"
    /// notice must not overwrite the stored copy. Observed live — a listings
    /// page re-fetched to a 226-char notice and took 254 lines with it.
    #[test]
    fn collapse_guard_rejects_soft_404s() {
        let listings = "Ferrari 328 GTS, $144,900, Naples FL\n".repeat(40);
        let soft_404 = "Sorry! The page you are looking for has either been \
                        moved or removed from the website.";
        assert!(content_collapsed(&listings, soft_404));
    }

    /// …while ordinary editing churn passes through. A page that loses a
    /// listing or reorders its sections is a legitimate update.
    #[test]
    fn collapse_guard_allows_ordinary_churn() {
        let before = "Ferrari 328 GTS, $144,900, Naples FL\n".repeat(40);
        let after = "Ferrari 328 GTS, $144,900, Naples FL\n".repeat(32);
        assert!(!content_collapsed(&before, &after));
        // Growth is never a collapse.
        assert!(!content_collapsed(&before, &before.repeat(2)));
    }

    /// Short sources aren't protected: a stub shrinking proves nothing, and
    /// guarding them would freeze every one-line page.
    #[test]
    fn collapse_guard_ignores_short_sources() {
        assert!(!content_collapsed("a short stub of a page", ""));
    }
}
