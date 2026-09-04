//! Source hygiene (docs/RFC-source-hygiene.md): the check that notices
//! drifted sources and the budgeted sweep that fixes the reversible half.
//!
//! Split by reversibility, exactly like the note curator: re-fetching is
//! reversible, so aging url sources refresh automatically (budgeted through
//! the scheduler pass, the gist-sweep shape); removal is not, so dead links,
//! missing files, duplicates, and errored husks are only ever *proposed* —
//! `classify` buckets them, the sources panel badges them, and Grow's
//! "Needs attention" review (or an agent, via the `source_hygiene` MCP
//! tool) decides.
//!
//! Notes are in the check too (`classify_notes`), on the same terms: an
//! empty note is proposed for removal and nothing else happens to it.
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
use crate::models::{Note, Source, SourceEvent};

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

/// One flagged object. `bucket` is the disposition class the UI and MCP
/// report group by: "unreachable" | "missing-file" | "duplicate" | "husk" |
/// "empty-note" (proposed removals) and "stale" (informational — the sweep
/// handles it).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HygieneIssue {
    /// "source" or "note" — which table `source_id` points into, and so
    /// which verbs the review can offer. A note has nothing to re-fetch.
    pub kind: String,
    /// The flagged object's id. Named for the case that has always been
    /// here; a note issue carries its note id.
    pub source_id: String,
    pub title: String,
    pub bucket: String,
    pub detail: String,
    /// For "duplicate": the id of the copy being kept — the oldest of the
    /// group, which carries the history. Empty for every other bucket, so
    /// a surface can offer "remove the extras, keep that one" without
    /// re-deriving the grouping.
    #[serde(default)]
    pub keeper_id: String,
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
        "folder" | "obsidian" | "okf" | "git" | "notion" | "feed"
    )
}

/// When this source was last known-fresh. A zero stamp means "nobody
/// recorded one", not "the epoch": rows written by a binary that predates
/// these columns come back zeroed (the shared store fills unknown columns
/// with defaults — see `Db::add_batch`), and any future writer that misses
/// the field would do the same. Reading that as maximally stale would make
/// the sweep re-fetch a source added minutes ago and show "20688 days ago"
/// in the review. Fall back to the import time, which is the same floor the
/// migration backfills with.
fn effective_fetched_at(source: &Source) -> i64 {
    if source.fetched_at > 0 {
        source.fetched_at
    } else {
        source.created_at
    }
}

/// Text short enough to be furniture — a stub `__init__.py`, a one-line
/// README, a note holding a single heading — is not evidence that two rows
/// are the same import. Below this, only an origin can prove a duplicate.
const DUPLICATE_MIN_CHARS: usize = 60;

/// A hash of the text, for grouping only: compared inside one classify pass
/// and never stored, so a fast non-cryptographic hasher is the right tool.
fn content_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.trim().hash(&mut h);
    h.finish()
}

/// What makes two sources the same source twice — the identity the 0.55.0
/// double-import produced under two ids.
///
/// A web source goes by its canonical URL: the same page under `http://`,
/// `www.`, a trailing slash or a tracking param is one page, and
/// `growth::canonical_key` already owns those rules. A file or pasted text
/// goes by its text instead — a file imported twice from two folders is
/// still one document — falling back to the path when there is too little
/// text to be sure.
///
/// Folder children are excluded: two files with identical contents under one
/// parent are the tree the user chose, and the rescan owns them. `None`
/// means "nothing here proves sameness" — never a guess.
fn duplicate_key(s: &Source) -> Option<String> {
    if !s.parent_id.is_empty() {
        return None;
    }
    let url = s.url.trim();
    if !url.is_empty() && (s.source_type == "url" || commands::is_web_url(url)) {
        return Some(format!("url:{}", crate::growth::canonical_key(url)));
    }
    let text = s.content.trim();
    if text.chars().count() >= DUPLICATE_MIN_CHARS {
        return Some(format!("text:{}", content_hash(text)));
    }
    // Too little text to judge by. A shared path still proves it.
    (!url.is_empty()).then(|| format!("url:{url}"))
}

/// Bucket a notebook's sources. Pure classification over the rows (plus
/// cheap fs stats for loose files) — no network, no mutation; callers decide
/// what to do with the result. One issue per source, most actionable bucket
/// wins.
///
/// Notes are classified by `classify_notes`; `classify_all` is what the
/// review surfaces call.
pub fn classify(sources: &[Source], cadence_days: u32, now: i64) -> Vec<HygieneIssue> {
    let cadence_ms = i64::from(cadence_days).saturating_mul(86_400_000);
    // First-seen source per identity; later holders of the same identity are
    // the duplicates (keep the oldest — it carries the history).
    let mut first_seen: HashMap<String, &Source> = HashMap::new();
    let mut by_age: Vec<&Source> = sources.iter().collect();
    by_age.sort_by_key(|s| s.created_at);

    let mut issues = Vec::new();
    for s in by_age {
        if is_folder_like(s) {
            continue; // rescan owns parents; their children are below
        }
        let issue = |bucket: &str, detail: String| HygieneIssue {
            kind: "source".into(),
            source_id: s.id.clone(),
            title: s.title.clone(),
            bucket: bucket.into(),
            detail,
            keeper_id: String::new(),
        };
        if s.fetch_failures >= UNREACHABLE_AFTER {
            issues.push(issue(
                "unreachable",
                format!("{} refresh attempts failed", s.fetch_failures),
            ));
            continue;
        }
        // Loose files only: folder children that vanish are the rescan's to
        // reconcile, and a cloud-evicted file is downloadable, not missing —
        // whether it is a legacy `.icloud` placeholder or, on current macOS
        // and every FileProvider mount, the file itself with no data in it
        // (docs/RFC-okf-live.md §5.7). A remote source is the third of those:
        // its path names a drive on another Mac, so "the file is gone" is not
        // news and there is nothing here to remove (§5.8). Callers mark that
        // with `device::mark_remote` before classifying.
        if is_file_path(s) && s.parent_id.is_empty() && !s.remote {
            let p = std::path::Path::new(&s.url);
            if !p.exists() && !crate::okf::is_evicted_stub(p) {
                issues.push(issue(
                    "missing-file",
                    format!("file no longer exists at {}", s.url),
                ));
                continue;
            }
        }
        if let Some(key) = duplicate_key(s) {
            if let Some(first) = first_seen.get(key.as_str()) {
                let how = if key.starts_with("url:") {
                    "same URL as"
                } else {
                    "same content as"
                };
                issues.push(HygieneIssue {
                    keeper_id: first.id.clone(),
                    ..issue(
                        "duplicate",
                        format!("{how} \u{201c}{}\u{201d}", first.title),
                    )
                });
                continue;
            }
            first_seen.insert(key, s);
        }
        if s.status == "error" && s.char_count == 0 && now - s.created_at > HUSK_AFTER_MS {
            issues.push(issue("husk", "failed import with no content".into()));
            continue;
        }
        let fetched_at = effective_fetched_at(s);
        if s.source_type == "url" && cadence_ms > 0 && now - fetched_at > cadence_ms {
            let days = (now - fetched_at) / 86_400_000;
            issues.push(issue("stale", format!("last fetched {days} days ago")));
        }
    }
    issues
}

/// Bucket a notebook's notes. Sources drift because the world moves under
/// them; a note has no origin to drift from, so only two things can be
/// wrong with one from the outside.
///
/// **Empty**: a generation that never landed, or a blank note nobody came
/// back to, reads on the shelf as a document and answers nothing. The same
/// age guard the husk bucket uses keeps this off a note the user has only
/// just created: a note you are about to type into is not a problem, and
/// every note starts empty.
///
/// **Duplicate**: the same note twice. The 0.55.0 double import wrote each
/// generated document under two ids, so the shelf shows "Briefing Doc"
/// beside "Briefing Doc". Title and text together are the identity — one
/// title over two drafts is a rewrite, and one text under two titles is a
/// deliberate copy. Oldest wins, as it does for sources.
pub fn classify_notes(notes: &[Note], now: i64) -> Vec<HygieneIssue> {
    let mut by_age: Vec<&Note> = notes.iter().collect();
    by_age.sort_by_key(|n| n.created_at);
    let mut first_seen: HashMap<(String, u64), &Note> = HashMap::new();
    let mut issues = Vec::new();
    for n in by_age {
        let text = n.content.trim();
        if text.is_empty() {
            if now - n.updated_at > HUSK_AFTER_MS {
                issues.push(HygieneIssue {
                    kind: "note".into(),
                    source_id: n.id.clone(),
                    title: n.title.clone(),
                    bucket: "empty-note".into(),
                    detail: "no content — nothing was ever written here".into(),
                    keeper_id: String::new(),
                });
            }
            continue;
        }
        let key = (n.title.trim().to_string(), content_hash(text));
        if let Some(first) = first_seen.get(&key) {
            issues.push(HygieneIssue {
                kind: "note".into(),
                source_id: n.id.clone(),
                title: n.title.clone(),
                bucket: "duplicate".into(),
                detail: format!("same note as \u{201c}{}\u{201d}", first.title),
                keeper_id: first.id.clone(),
            });
            continue;
        }
        first_seen.insert(key, n);
    }
    issues
}

/// The whole review in one list: what is wrong with the notebook's sources,
/// then with its notes. The two halves share a shape so one surface (and one
/// MCP report) can render both.
pub fn classify_all(
    sources: &[Source],
    notes: &[Note],
    cadence_days: u32,
    now: i64,
) -> Vec<HygieneIssue> {
    let mut issues = classify(sources, cadence_days, now);
    issues.extend(classify_notes(notes, now));
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
    urls.sort_by_key(effective_fetched_at);

    let mut refreshed: HashMap<String, u32> = HashMap::new();
    let mut budget = SWEEP_BUDGET;
    for src in &urls {
        if budget == 0 {
            break;
        }
        if archived.contains(&src.notebook_id)
            || src.status == "processing"
            || src.fetch_failures >= UNREACHABLE_AFTER
            || now - effective_fetched_at(src) < cadence_ms
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
                let why = format!(
                    "page came back gutted ({} chars, was {}) — keeping the stored copy",
                    extracted.text.chars().count(),
                    existing.content.chars().count()
                );
                strike(state, &existing, &why).await?;
                anyhow::bail!(why)
            } else {
                commands::reingest(state, &existing, extracted, None, true).await?;
                Ok(true)
            }
        }
        Err(err) => {
            strike(state, &existing, &format!("{err:#}")).await?;
            Err(err)
        }
    }
}

/// Count one failed probe on the row. The strike that reaches
/// `UNREACHABLE_AFTER` also becomes an `unreachable` event
/// (docs/RFC-events.md §1) — once, at the threshold, so a dead link is one
/// arrival in the Brief rather than a row per retry for the rest of its life.
async fn strike(state: &AppState, existing: &Source, why: &str) -> anyhow::Result<()> {
    let failures = existing.fetch_failures + 1;
    state
        .db
        .set_source_fetch(&existing.id, effective_fetched_at(existing), failures)
        .await?;
    if failures == UNREACHABLE_AFTER {
        let _ = state
            .db
            .add_source_event(&SourceEvent {
                id: commands::new_id(),
                notebook_id: existing.notebook_id.clone(),
                source_id: existing.id.clone(),
                source_title: existing.title.clone(),
                kind: "unreachable".into(),
                detail: format!("{failures} refresh attempts failed"),
                diff: why.chars().take(200).collect(),
                at: commands::now(),
            })
            .await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    fn src(id: &str, source_type: &str) -> Source {
        Source {
            origin_device: String::new(),
            remote: false,
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

    /// A zero stamp means "unrecorded", not "1970". Rows written by a binary
    /// that predates these columns come back zeroed, and calling such a
    /// source 20,000 days stale would re-fetch it the moment it was added.
    #[test]
    fn a_zero_stamp_falls_back_to_the_import_time() {
        let now = 100 * DAY;
        let mut just_added = src("just-added", "url");
        just_added.url = "https://example.com/new".into();
        just_added.created_at = now - DAY; // imported yesterday
        just_added.fetched_at = 0; // written by an older binary
        assert!(
            classify(&[just_added.clone()], 30, now).is_empty(),
            "a source imported yesterday is not stale"
        );

        // Old enough by its import time, though, and it does flag.
        just_added.created_at = now - 90 * DAY;
        let issues = classify(&[just_added], 30, now);
        assert_eq!(buckets(&issues), vec![("just-added", "stale")]);
        assert!(issues[0].detail.contains("90 days"), "{}", issues[0].detail);
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
        assert_eq!(issues[0].keeper_id, "first");
    }

    /// Two copies of one document with no origin to compare: same text, two
    /// ids. The oldest is the keeper whatever order the rows arrive in, and
    /// every other copy carries its id.
    #[test]
    fn identical_text_is_a_duplicate_and_the_oldest_is_kept() {
        let now = 100 * DAY;
        let body = "The Meridian programme note, imported twice by the 0.55.0 \
                    double import, at ample length to be judged by.";
        let mut copies: Vec<Source> = ["third", "first", "second"]
            .iter()
            .map(|id| {
                let mut s = src(id, "text");
                s.content = body.into();
                s
            })
            .collect();
        copies[0].created_at = 30;
        copies[1].created_at = 10;
        copies[2].created_at = 20;

        let issues = classify(&copies, 0, now);
        assert_eq!(
            buckets(&issues),
            vec![("second", "duplicate"), ("third", "duplicate")]
        );
        assert!(issues.iter().all(|i| i.keeper_id == "first"));

        // A different document is not a duplicate of it.
        let mut other = src("other", "text");
        other.content = body.replace("twice", "once");
        let with_other = [copies[1].clone(), other];
        assert!(classify(&with_other, 0, now).is_empty());
    }

    /// Short text proves nothing — two stub files are not one import — and a
    /// folder child is the rescan's business, not the review's.
    #[test]
    fn thin_text_and_folder_children_are_never_duplicates() {
        let now = 100 * DAY;
        let stub = |id: &str| {
            let mut s = src(id, "text");
            s.content = "TODO".into();
            s
        };
        assert!(classify(&[stub("a"), stub("b")], 0, now).is_empty());

        let body = "A file long enough to hash, sitting under a folder source \
                    that the rescan owns from end to end.";
        let child = |id: &str| {
            let mut s = src(id, "markdown");
            s.parent_id = "parent".into();
            s.url = format!("/tmp/tree/{id}.md");
            s.content = body.into();
            s
        };
        assert!(classify(&[child("a"), child("b")], 0, now).is_empty());
    }

    /// The same generated note under two ids — the shape Paul saw in the
    /// Studio list. Empty notes still flag as before.
    #[test]
    fn duplicate_notes_keep_the_oldest() {
        let now = 100 * DAY;
        let note = |id: &str, title: &str, content: &str, created: i64| Note {
            id: id.into(),
            notebook_id: "nb".into(),
            title: title.into(),
            content: content.into(),
            kind: "briefing".into(),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: created,
            updated_at: created,
        };
        let issues = classify_notes(
            &[
                note("newer", "Briefing Doc", "One brief.", 20),
                note("older", "Briefing Doc", "One brief.", 10),
                note("other", "Briefing Doc", "A different brief.", 30),
                note("blank", "Untitled", "", 0),
            ],
            now,
        );
        assert_eq!(
            buckets(&issues),
            vec![("blank", "empty-note"), ("newer", "duplicate")]
        );
        assert_eq!(issues[1].keeper_id, "older");
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

    /// A file that was never on this Mac cannot have gone missing from it.
    /// The same absent path, twice: local, it is a source to propose for
    /// removal; remote, it is a notebook that travelled through a shared
    /// folder and brought its text with it (docs/RFC-okf-live.md §5.8).
    #[test]
    fn a_remote_source_is_not_a_missing_file() {
        let now = 100 * DAY;
        let path = "/Volumes/OneDrive-Work/Q3/plan.pdf";
        let mut here = src("here", "pdf");
        here.url = path.into();
        here.fetched_at = now;
        let mut away = src("away", "pdf");
        away.url = path.into();
        away.fetched_at = now;
        away.origin_device = "Paul's MacBook Pro".into();
        away.remote = true;

        assert_eq!(
            buckets(&classify(&[here, away], 30, now)),
            vec![("here", "missing-file")]
        );
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

    fn note(id: &str, content: &str, updated_at: i64) -> Note {
        Note {
            id: id.into(),
            notebook_id: "nb".into(),
            title: id.into(),
            content: content.into(),
            kind: "note".into(),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: updated_at,
            updated_at,
        }
    }

    /// An empty note is flagged only once it has had a week to be written.
    /// Every note starts empty, and the one the user is typing into right
    /// now is not a problem to review.
    #[test]
    fn empty_notes_flag_only_after_the_husk_age() {
        let now = 100 * DAY;
        let issues = classify_notes(
            &[
                note("just-made", "", now - DAY),
                note("blank", "   \n ", now - 30 * DAY),
                note("written", "It says here…", now - 30 * DAY),
            ],
            now,
        );
        assert_eq!(buckets(&issues), vec![("blank", "empty-note")]);
        assert_eq!(issues[0].kind, "note");
    }

    /// One list, both halves, and every issue says which it is — the review
    /// has to know whether "Retry" means anything for a row.
    #[test]
    fn classify_all_labels_each_half() {
        let now = 100 * DAY;
        let mut dead = src("dead", "url");
        dead.url = "https://example.com/gone".into();
        dead.fetch_failures = UNREACHABLE_AFTER;
        let issues = classify_all(&[dead], &[note("blank", "", 0)], 30, now);
        assert_eq!(
            buckets(&issues),
            vec![("dead", "unreachable"), ("blank", "empty-note")]
        );
        assert_eq!(issues[0].kind, "source");
        assert_eq!(issues[1].kind, "note");
    }
}
