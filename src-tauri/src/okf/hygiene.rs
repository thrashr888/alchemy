//! Keeping the bundle's own bookkeeping small (docs/RFC-okf-live.md §5.6,
//! "Tracking overhead stays bounded").
//!
//! A bundle is the user's portable notebook. `log.md`, `conflicts/` and the
//! listings exist for their sake, so every one of them has a ceiling here:
//! the log keeps a bounded number of entries and never carries a document
//! body; a conflict copy whose text is back in the notebook is cleared
//! after a grace period; and the `<name> 2.md` twins a cloud race leaves
//! behind are resolved for the files Alchemy owns outright. One 11 MB
//! `log.md`, 718 conflict copies and an `index 2.md` in a single notebook
//! is what this is sized against.
use super::*;
use std::collections::{BTreeMap, HashSet};
use std::io::Write;

/// How many dated entries `log.md` keeps. Around five hundred is a season
/// of ordinary use at a few passes a day, and a day of a runaway writer.
pub(crate) const LOG_CAP: usize = 500;

/// How long a conflict copy stays after its text is known to be elsewhere.
pub(crate) const CONFLICT_GRACE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// How many redundant copies per path may wait out the grace period.
pub(crate) const CONFLICTS_PER_PATH: usize = 3;

/// How often a pass looks at `conflicts/` on its own.
const PRUNE_EVERY_MS: i64 = 60 * 60 * 1000;

/// What the launch pass understands how to repair. The number is raised
/// when there is something new to put right: 1 collapsed the marker and
/// bulleted-fence shapes; 2 also collapses a fenced dump whose bullet has
/// already rolled off.
const BLOAT_HEAL_VERSION: &str = "2";

// ---- log.md -----------------------------------------------------------------

/// A per-writer day heading (§5.6): `## 2026-09-10 — thrashr888`.
fn is_day_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("## ") else {
        return false;
    };
    let bytes = rest.as_bytes();
    bytes.len() > 11
        && bytes[..10].iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
        && rest[10..].starts_with(" \u{2014} ")
}

fn is_entry(line: &str) -> bool {
    line.starts_with("- ")
}

/// `HH:MM:SSZ`, the time token every entry starts with.
fn is_time(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 9
        && b[8] == b'Z'
        && b.iter().take(8).enumerate().all(|(i, c)| {
            if i == 2 || i == 5 {
                *c == b':'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// One entry line taken apart: when it first happened, what it says, and
/// how many times in a row.
struct Entry<'a> {
    first: &'a str,
    text: &'a str,
    times: usize,
}

fn parse_entry(line: &str) -> Option<Entry<'_>> {
    let rest = line.strip_prefix("- ")?;
    let (first, text) = rest.split_once(' ')?;
    if !is_time(first) {
        return None;
    }
    // `… ×7, last 20:45:02Z` is the coalesced form.
    if let Some((head, tail)) = text.rsplit_once(" \u{d7}") {
        if let Some((n, last)) = tail.split_once(", last ") {
            if let Ok(times) = n.parse::<usize>() {
                if is_time(last) {
                    return Some(Entry {
                        first,
                        text: head,
                        times,
                    });
                }
            }
        }
    }
    Some(Entry {
        first,
        text,
        times: 1,
    })
}

fn render_entry(first: &str, text: &str, times: usize, last: &str) -> String {
    if times <= 1 {
        format!("- {first} {text}")
    } else {
        format!("- {first} {text} \u{d7}{times}, last {last}")
    }
}

/// The log with one more entry under `heading`, as `okf_log_append` writes
/// it: pure, so the rules are testable without a clock.
///
/// Three rules on top of the append. A repeat of the entry just written —
/// the same words, another minute — becomes a count on that line rather
/// than a new one, which is what turns a writer stuck in a loop into one
/// line a day instead of a firehose. The file keeps at most `LOG_CAP`
/// entries, the oldest rolling off whole. And a heading whose entries have
/// all rolled off goes with them.
pub(crate) fn log_with_entry(
    existing: &str,
    heading: &str,
    at: &str,
    entry: &str,
    writer: &str,
) -> String {
    let mut out = if existing.trim().is_empty() {
        String::from("# Log\n")
    } else {
        existing.to_string()
    };
    if !out.ends_with('\n') {
        out.push('\n');
    }
    let text = format!("{entry} ({writer})");
    let heading_line = format!("\n{heading}\n");
    if out.contains(&heading_line) {
        // This writer's newest day is its last block, so the last entry in
        // the file is the one a repeat coalesces with — but only when it is
        // the last *line* and the last heading is this writer's own, so a
        // note from someone else, or the other Mac's block landing after
        // ours, still starts a fresh entry rather than counting onto theirs.
        let last_line_start = out.trim_end_matches('\n').rfind('\n').map_or(0, |i| i + 1);
        let last_line = out[last_line_start..].trim_end_matches('\n').to_string();
        let ours_is_last = out
            .lines()
            .rev()
            .find(|l| is_day_heading(l))
            .is_some_and(|l| l == heading);
        if let Some(prev) = parse_entry(&last_line).filter(|_| ours_is_last) {
            if prev.text == text {
                let merged = render_entry(prev.first, prev.text, prev.times + 1, at);
                out.truncate(last_line_start);
                out.push_str(&merged);
                out.push('\n');
                return cap_log(&out, LOG_CAP);
            }
        }
    } else {
        out.push_str(&format!("\n{heading}\n\n"));
    }
    out.push_str(&render_entry(at, &text, 1, at));
    out.push('\n');
    cap_log(&out, LOG_CAP)
}

/// The log with only its newest `cap` entries.
///
/// Entries roll off from the top; a day heading left with nothing under it
/// goes too. Lines that are neither — a note somebody else left, a title —
/// stay where they are: they are not ours to bound.
pub(crate) fn cap_log(text: &str, cap: usize) -> String {
    let total = text.lines().filter(|l| is_entry(l)).count();
    if total <= cap {
        return text.to_string();
    }
    let mut excess = total - cap;
    // Pass one: drop the oldest entries.
    let mut kept: Vec<&str> = Vec::with_capacity(text.lines().count());
    for line in text.lines() {
        if excess > 0 && is_entry(line) {
            excess -= 1;
            continue;
        }
        kept.push(line);
    }
    // Pass two: drop day headings whose block is now empty of content.
    let mut out: Vec<&str> = Vec::with_capacity(kept.len());
    let mut i = 0;
    while i < kept.len() {
        let line = kept[i];
        if is_day_heading(line) {
            let end = kept[i + 1..]
                .iter()
                .position(|l| is_day_heading(l))
                .map_or(kept.len(), |p| i + 1 + p);
            if kept[i + 1..end].iter().all(|l| l.trim().is_empty()) {
                i = end;
                continue;
            }
        }
        out.push(line);
        i += 1;
    }
    let mut joined = out.join("\n");
    // Collapse the blank runs the removals leave behind.
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    joined.push('\n');
    joined
}

/// A conflict entry the 0.58 log carried whole: the marker, its side and
/// path, and the preserved text — which may be the only copy left if the
/// file under `conflicts/` is gone.
#[derive(Debug, PartialEq)]
pub(crate) struct InlinedConflict {
    pub id: String,
    pub side: String,
    pub rel: String,
    pub text: String,
}

/// The old log with each inlined conflict entry collapsed to one line that
/// names the copy under `conflicts/`, and the entries themselves, so the
/// caller can put back any copy that is missing before the text is gone
/// from the log.
///
/// Three shapes. A 0.58 entry runs from its `<!-- alchemy-conflict:… -->`
/// marker to the next marker or the next per-writer day heading. The
/// document text in between has headings of its own — `## Facts`,
/// `## Documents` — which is why the end is not "the next heading". Before
/// that (0.55–0.57, the read-back in 99a9a90) an overruled disk edit went
/// in as `Kept the app's newer version of N file(s); the disk text
/// follows.` and a fenced block of `path`, text, `---`, `path`, text,
/// closed by the fence with the writer after it; each file in the block
/// becomes one entry, preserved as the remote side, since there was no
/// `conflicts/` to hold it then. The third shape is that block with its
/// bullet gone — the cap rolled the bullet off as an ordinary entry while
/// the block it introduced stayed, since it is not an entry — so a bare
/// fence whose first line is a bundle path is a dump too, and each file in
/// it becomes an `Old copy of <path>` line.
pub(crate) fn collapse_inlined_conflicts(text: &str) -> (String, Vec<InlinedConflict>) {
    const MARKER: &str = "<!-- alchemy-conflict:";
    const OLD_HEAD: &str = "Kept the app's newer version of ";
    const OLD_TAIL: &str = "; the disk text follows.";
    if !text.contains(MARKER) && !text.contains(OLD_TAIL) && !text.contains("\n```\n") {
        return (text.to_string(), Vec::new());
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut found = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // The 0.56 shape: the bullet, a blank, the opening fence, the
        // files, and the closing fence carrying the writer.
        if let Some(old) =
            parse_entry(line).filter(|e| e.text.starts_with(OLD_HEAD) && e.text.ends_with(OLD_TAIL))
        {
            let open = lines
                .get(i + 1..=i + 2)
                .and_then(|w| w.iter().position(|l| *l == "```"))
                .map(|p| i + 1 + p);
            if let Some((open, close)) = open.and_then(|o| Some((o, dump_close(&lines, o)?))) {
                let writer = lines[close]
                    .trim_start_matches("``` (")
                    .trim_end_matches(')');
                let count = old.text[OLD_HEAD.len()..old.text.len() - OLD_TAIL.len()]
                    .trim_end_matches(" file(s)")
                    .to_string();
                for (rel, body) in split_old_losers(&lines[open + 1..close].join("\n")) {
                    found.push(old_loser(rel, body));
                }
                out.push(format!(
                    "- {} {OLD_HEAD}{count} file(s); the disk text is under conflicts/. ({writer})",
                    old.first
                ));
                i = close + 1;
                continue;
            }
        }
        // The same block with no bullet in front of it.
        if line == "```" && opens_dump(&lines, i) {
            if let Some(close) = dump_close(&lines, i) {
                for (rel, body) in split_old_losers(&lines[i + 1..close].join("\n")) {
                    let entry = old_loser(rel, body);
                    out.push(dump_line_put(&entry));
                    found.push(entry);
                }
                i = close + 1;
                continue;
            }
        }
        let Some(id) = line
            .strip_prefix(MARKER)
            .and_then(|r| r.strip_suffix(" -->"))
        else {
            out.push(line.to_string());
            i += 1;
            continue;
        };
        let end = lines[i + 1..]
            .iter()
            .position(|l| l.starts_with(MARKER) || is_day_heading(l))
            .map_or(lines.len(), |p| i + 1 + p);
        let block = &lines[i + 1..end];
        // `## Recovered {side} version of {rel}`, then the link line, then
        // the text.
        let mut side = String::new();
        let mut rel = String::new();
        let mut text_from = block.len();
        for (k, l) in block.iter().enumerate() {
            if let Some(rest) = l.strip_prefix("## Recovered ") {
                if let Some((s, r)) = rest.split_once(" version of ") {
                    side = s.to_string();
                    rel = r.to_string();
                }
            } else if l.starts_with("[Preserved copy](conflicts/") {
                text_from = k + 1;
                break;
            }
        }
        let body = block
            .get(text_from..)
            .map(|b| b.join("\n"))
            .unwrap_or_default();
        let body = body.trim_matches('\n').to_string();
        out.push(format!(
            "- Losing {side} version of {rel} kept in conflicts/{id}.md"
        ));
        found.push(InlinedConflict {
            id: id.to_string(),
            side,
            rel,
            text: body,
        });
        i = end;
    }
    let mut joined = out.join("\n");
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    joined.push('\n');
    (joined, found)
}

/// The files in a 0.56 fenced block: `path`, blank, text, joined by a
/// blank-`---`-blank rule. A document's own rule splits the same way, so a
/// piece that does not open with a concept path is the tail of the piece
/// before it — and a document that *ends* with a rule leaves a piece that
/// opens with `---` and then the next path, which is that document's last
/// line and a new file.
pub(crate) fn split_old_losers(block: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for piece in block.split("\n\n---\n\n") {
        let first_line = |p: &str| p.lines().next().unwrap_or_default().trim().to_string();
        let mut first = first_line(piece);
        let mut piece = piece;
        if first == "---" {
            let rest = piece[3..].trim_start_matches('\n');
            if is_bundle_path(&first_line(rest)) {
                if let Some((_, body)) = out.last_mut() {
                    body.push_str("\n\n---");
                }
                first = first_line(rest);
                piece = rest;
            }
        }
        let first = first.as_str();
        match (is_bundle_path(first), out.last_mut()) {
            (true, _) => {
                let body = piece[first.len()..].trim_matches('\n').to_string();
                out.push((first.to_string(), body));
            }
            (false, Some((_, body))) => {
                body.push_str("\n\n---\n\n");
                body.push_str(piece);
            }
            (false, None) => {}
        }
    }
    for (_, body) in &mut out {
        *body = body.trim_matches('\n').to_string();
    }
    out
}

/// A concept path as the dumps name one: `sources/<slug>.md`,
/// `notes/<slug>.md`, or the root listing.
fn is_bundle_path(s: &str) -> bool {
    s == "index.md"
        || ((s.starts_with("sources/") || s.starts_with("notes/"))
            && s.ends_with(".md")
            && !s.contains(' '))
}

/// Is the bare fence at `open` the start of a dump — its first line a
/// bundle path?
fn opens_dump(lines: &[&str], open: usize) -> bool {
    lines[open + 1..]
        .iter()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| is_bundle_path(l.trim()))
}

/// Where the dump opened at `open` closes. The writer put its name after
/// the closing fence — `\`\`\` (alchemy/0.57.0)` — which no document
/// carries, so that line is the close. A dumped Markdown source has fences
/// of its own, so a bare fence closes the dump only when what follows it
/// (after blank lines) is the end of the file, another dump, a dated
/// entry, or a writer day heading — none of which a document contains.
fn dump_close(lines: &[&str], open: usize) -> Option<usize> {
    let mut k = open + 1;
    while k < lines.len() {
        let l = lines[k];
        if l.starts_with("``` (") && l.ends_with(')') {
            return Some(k);
        }
        if l == "```" {
            let next = lines[k + 1..]
                .iter()
                .position(|n| !n.trim().is_empty())
                .map(|p| k + 1 + p);
            let ends = match next {
                None => true,
                Some(n) => {
                    is_day_heading(lines[n])
                        || parse_entry(lines[n]).is_some()
                        || (lines[n] == "```" && opens_dump(lines, n))
                }
            };
            if ends {
                return Some(k);
            }
        }
        k += 1;
    }
    None
}

/// One file out of a dump, preserved as the remote side with the id the
/// conflict writer would have minted for the same text.
fn old_loser(rel: String, text: String) -> InlinedConflict {
    let id = okf_hash(&serde_json::json!([rel, "remote", text]).to_string());
    InlinedConflict {
        id,
        side: "remote".into(),
        rel,
        text,
    }
}

/// The line a bulletless dump collapses to, before the notebook has been
/// asked whether it already holds the text.
fn dump_line_put(entry: &InlinedConflict) -> String {
    format!(
        "- Old copy of {} ({} chars) put under conflicts/{}.md",
        entry.rel,
        entry.text.chars().count(),
        entry.id
    )
}

/// The same line once the notebook turned out to hold the text already.
fn dump_line_known(entry: &InlinedConflict) -> String {
    format!("- Old copy of {} already in the notebook", entry.rel)
}

/// The bytes of a conflict copy, as `conflicts::Context::preserve` writes
/// them, so a copy put back from the log is the copy that was lost.
pub(crate) fn conflict_copy_text(rel: &str, side: &str, text: &str) -> String {
    format!("# Recovered sync conflict\n\nOriginal document: `{rel}`\n\nPreserved version: {side}\n\n{text}")
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("no parent directory")?;
    let save = || -> std::io::Result<()> {
        let mut staged = tempfile::NamedTempFile::new_in(dir)?;
        staged.write_all(text.as_bytes())?;
        staged.as_file().sync_all()?;
        staged.persist(path).map_err(|err| err.error)?;
        Ok(())
    };
    save().map_err(|err| format!("Could not write {}: {err}", path.display()))
}

/// What trimming one bundle's log did.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct LogTrim {
    pub collapsed: usize,
    /// Inlined texts with no copy under `conflicts/` and no match in the
    /// notebook, written there before they left the log.
    pub copies_restored: usize,
    /// Inlined texts the notebook already holds, dropped with the entry.
    pub copies_dropped: usize,
    pub entries_dropped: usize,
    pub chars_removed: usize,
}

/// Bring one bundle's `log.md` under the cap: inlined conflict bodies become
/// one line each, and the oldest entries past `LOG_CAP` roll off. A log
/// that is already in shape is not rewritten.
///
/// The log was the fallback record, so before its copy of a text goes the
/// durable one has to exist: a text with no file under `conflicts/` is
/// written there — unless the notebook already holds it, on disk or in the
/// row, in which case the log's copy was never the only one and nothing is
/// written for it.
pub(crate) async fn trim_log(
    state: &AppState,
    bundle: &Path,
    manifest: &OkfManifest,
) -> Result<LogTrim, String> {
    let path = bundle.join("log.md");
    if is_dataless(&path) {
        return Ok(LogTrim::default());
    }
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(LogTrim::default()),
        Err(err) => return Err(format!("Could not read {}: {err}", path.display())),
    };
    let (mut collapsed, inlined) = collapse_inlined_conflicts(&existing);
    let mut done = LogTrim {
        collapsed: inlined.len(),
        ..Default::default()
    };
    let rels: Vec<&str> = inlined.iter().map(|e| e.rel.as_str()).collect();
    let known = known_texts(state, bundle, manifest, &rels).await;
    let empty = empty_key();
    for entry in &inlined {
        let copy = bundle.join("conflicts").join(format!("{}.md", entry.id));
        if copy.exists() {
            continue;
        }
        let key = text_key(&entry.text);
        if key == empty || known.get(&entry.rel).is_some_and(|k| k.contains(&key)) {
            done.copies_dropped += 1;
            // A bulletless dump's line names the copy it would have put
            // under conflicts/; say instead that there was no need.
            collapsed = collapsed.replace(&dump_line_put(entry), &dump_line_known(entry));
            continue;
        }
        std::fs::create_dir_all(bundle.join("conflicts")).map_err(|err| err.to_string())?;
        write_atomic(
            &copy,
            &conflict_copy_text(&entry.rel, &entry.side, &entry.text),
        )?;
        done.copies_restored += 1;
    }
    let before = collapsed.lines().filter(|l| is_entry(l)).count();
    let capped = cap_log(&collapsed, LOG_CAP);
    done.entries_dropped = before.saturating_sub(LOG_CAP);
    if capped == existing {
        return Ok(LogTrim::default());
    }
    done.chars_removed = existing
        .chars()
        .count()
        .saturating_sub(capped.chars().count());
    write_atomic(&path, &capped)?;
    Ok(done)
}

// ---- conflicts/ -------------------------------------------------------------

/// One copy under `conflicts/`, as much of it as the pruning rules need.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConflictCopy {
    pub path: PathBuf,
    /// The concept file it was a version of, bundle-relative.
    pub rel: String,
    /// Hash of the body with every frontmatter block peeled — what "the
    /// same text" means here, since a copy from the 0.58 loop carries a
    /// stack of blocks over the same words.
    pub text_key: String,
    /// Epoch ms; the file's own clock.
    pub mtime: i64,
}

/// The identity of a text for "is it anywhere else": the body under any
/// frontmatter, trimmed. Empty is never somebody's text.
pub(crate) fn text_key(text: &str) -> String {
    let (_, body) = peel_frontmatter(text.trim_start());
    okf_hash(body.trim())
}

/// The empty body's key; a copy of nothing is never the only copy.
fn empty_key() -> String {
    okf_hash("")
}

/// Read one conflict copy's header. `None` for a file that is not one of
/// ours (somebody else's Markdown in the folder is left alone).
pub(crate) fn parse_conflict_copy(path: &Path, text: &str, mtime: i64) -> Option<ConflictCopy> {
    let mut lines = text.lines();
    if lines.next()? != "# Recovered sync conflict" {
        return None;
    }
    let mut rel = None;
    let mut body_from = 0;
    for (i, line) in text.lines().enumerate().skip(1) {
        if let Some(r) = line.strip_prefix("Original document: ") {
            rel = Some(r.trim_matches('`').to_string());
        } else if line.starts_with("Preserved version: ") {
            body_from = text
                .lines()
                .take(i + 1)
                .map(|l| l.len() + 1)
                .sum::<usize>()
                .min(text.len());
            break;
        }
    }
    Some(ConflictCopy {
        path: path.to_path_buf(),
        rel: rel?,
        text_key: text_key(&text[body_from..]),
        mtime,
    })
}

/// Every copy under `conflicts/`. An evicted one is skipped, not read: a
/// read is what downloads it.
pub(crate) fn conflict_copies(bundle: &Path) -> Vec<ConflictCopy> {
    let dir = bundle.join("conflicts");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || !name.ends_with(".md") || is_dataless(&path) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as i64);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(copy) = parse_conflict_copy(&path, &text, mtime) {
            out.push(copy);
        }
    }
    out
}

/// What pruning decided: the copies to remove, and the ones that hold text
/// found nowhere else, which stay.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct PrunePlan {
    pub remove: Vec<PathBuf>,
    pub unique: Vec<ConflictCopy>,
}

/// The pruning rule, pure over the listing (§5.6).
///
/// `known` maps a concept path to the keys of every text the notebook holds
/// for it right now — the file on disk and the row in the store. Per path,
/// newest first: a copy whose text is in `known`, or in a newer copy being
/// kept, is *redundant*; it goes once it is older than the grace period,
/// or at once when more than `CONFLICTS_PER_PATH` redundant copies are
/// already waiting for that path. A copy whose text is nowhere else is the
/// only record of somebody's words and is never removed by this pass.
pub(crate) fn prune_plan(
    copies: &[ConflictCopy],
    known: &HashMap<String, HashSet<String>>,
    now: i64,
) -> PrunePlan {
    let mut by_rel: BTreeMap<&str, Vec<&ConflictCopy>> = BTreeMap::new();
    for copy in copies {
        by_rel.entry(copy.rel.as_str()).or_default().push(copy);
    }
    let mut plan = PrunePlan::default();
    let empty = empty_key();
    for (rel, mut group) in by_rel {
        group.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.path.cmp(&b.path)));
        let mut seen: HashSet<String> = known.get(rel).cloned().unwrap_or_default();
        seen.insert(empty.clone());
        let mut waiting = 0usize;
        for copy in group {
            if !seen.contains(&copy.text_key) {
                seen.insert(copy.text_key.clone());
                plan.unique.push(copy.clone());
                continue;
            }
            let aged = now.saturating_sub(copy.mtime) >= CONFLICT_GRACE_MS;
            if aged || waiting >= CONFLICTS_PER_PATH {
                plan.remove.push(copy.path.clone());
            } else {
                waiting += 1;
            }
        }
    }
    plan
}

/// What one prune did.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Pruned {
    pub removed: usize,
    pub unique: Vec<ConflictCopy>,
    pub failed: usize,
}

/// The texts the notebook holds for every path that has a conflict copy:
/// the concept file on disk, and the row behind it.
async fn known_texts(
    state: &AppState,
    bundle: &Path,
    manifest: &OkfManifest,
    rels: &[&str],
) -> HashMap<String, HashSet<String>> {
    let by_path: HashMap<&str, &str> = manifest
        .concepts
        .iter()
        .map(|(id, entry)| (entry.path.as_str(), id.as_str()))
        .collect();
    let mut known: HashMap<String, HashSet<String>> = HashMap::new();
    let rels: HashSet<&str> = rels.iter().copied().collect();
    for rel in rels {
        let mut keys = HashSet::new();
        let file = bundle.join(rel);
        if !is_dataless(&file) {
            if let Ok(text) = std::fs::read_to_string(&file) {
                keys.insert(text_key(&text));
            }
        }
        if let Some(id) = by_path.get(rel) {
            let row = if rel.starts_with("notes/") {
                state
                    .db
                    .get_note(id)
                    .await
                    .ok()
                    .flatten()
                    .map(|n| n.content)
            } else {
                state
                    .db
                    .get_source(id)
                    .await
                    .ok()
                    .flatten()
                    .map(|s| s.content)
            };
            if let Some(content) = row {
                keys.insert(text_key(&content));
            }
        }
        known.insert(rel.to_string(), keys);
    }
    known
}

/// Prune one bundle's `conflicts/` now, and record what went in the log.
pub(crate) async fn prune_conflicts(
    state: &AppState,
    bundle: &Path,
    manifest: &OkfManifest,
) -> Pruned {
    let copies = conflict_copies(bundle);
    if copies.is_empty() {
        return Pruned::default();
    }
    let rels: Vec<&str> = copies.iter().map(|c| c.rel.as_str()).collect();
    let known = known_texts(state, bundle, manifest, &rels).await;
    let plan = prune_plan(&copies, &known, now_ms());
    let mut done = Pruned {
        unique: plan.unique,
        ..Default::default()
    };
    for path in &plan.remove {
        match std::fs::remove_file(path) {
            Ok(()) => done.removed += 1,
            Err(err) => {
                done.failed += 1;
                crate::note!(
                    "okf: couldn't clear conflict copy {}: {err}",
                    path.display()
                );
            }
        }
    }
    if done.removed > 0 {
        let _ = okf_log_append(
            bundle,
            &format!(
                "Cleared {} conflict cop{} whose text is back in the notebook.",
                done.removed,
                if done.removed == 1 { "y" } else { "ies" }
            ),
        );
    }
    done
}

/// The hourly pass: prune when it has been a while, and note it in the
/// manifest so the next pass can tell.
pub(crate) async fn prune_conflicts_if_due(
    state: &AppState,
    bundle: &Path,
    manifest: &mut OkfManifest,
    manifest_at: &Path,
) {
    let now = now_ms();
    if now.saturating_sub(manifest.conflicts_pruned_at) < PRUNE_EVERY_MS {
        return;
    }
    let done = prune_conflicts(state, bundle, manifest).await;
    if done.removed > 0 {
        crate::note!(
            "okf: cleared {} conflict copies under {}",
            done.removed,
            bundle.display()
        );
    }
    manifest.conflicts_pruned_at = now;
    if let Err(err) = save_manifest_checked(manifest_at, manifest) {
        crate::note!("okf: couldn't record the conflict prune: {err}");
    }
}

// ---- `<name> 2.md` ----------------------------------------------------------

/// A file a cloud race left beside its original.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CloudTwin {
    /// The numbered copy, bundle-relative.
    pub rel: String,
    /// The name it is a copy of, bundle-relative.
    pub canonical: String,
    /// Alchemy writes the canonical file whole: listings, the log, the
    /// sync records. Anything under `sources/` or `notes/` that is not a
    /// listing is somebody's document and is not resolved here.
    pub owned: bool,
}

/// `index 2.md` → `index.md`; `foo 12.md` → `foo.md`; `foo.md` → `None`.
pub(crate) fn twin_canonical(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    let (base, n) = stem.rsplit_once(' ')?;
    if base.is_empty() || n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // A copy is numbered from 2; " 0" and " 1" are somebody's own naming.
    if n.parse::<u32>().ok()? < 2 {
        return None;
    }
    Some(format!("{base}.{ext}"))
}

/// Is this bundle-relative path one Alchemy writes whole?
pub(crate) fn is_owned(rel: &str) -> bool {
    let name = Path::new(rel)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    match rel.split('/').count() {
        1 => matches!(name, "index.md" | "log.md"),
        _ => {
            rel.starts_with("sync/")
                || ((rel.starts_with("sources/") || rel.starts_with("notes/"))
                    && name == "index.md")
        }
    }
}

/// Every numbered twin in the bundle, from the listing alone.
pub(crate) fn cloud_twins(bundle: &Path) -> Vec<CloudTwin> {
    fn names_in(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
        if depth > 8 {
            return;
        }
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            match entry.file_type() {
                Ok(t) if t.is_dir() => {
                    if depth == 0 && !matches!(name.as_str(), "sources" | "notes" | "sync") {
                        continue;
                    }
                    names_in(&entry.path(), &rel, depth + 1, out);
                }
                Ok(t) if t.is_file() => out.push(rel),
                _ => {}
            }
        }
    }
    let mut names = Vec::new();
    names_in(bundle, "", 0, &mut names);
    names.sort();
    let mut out = Vec::new();
    for rel in &names {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let Some(canonical_name) = twin_canonical(name) else {
            continue;
        };
        let canonical = match rel.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/{canonical_name}"),
            None => canonical_name,
        };
        out.push(CloudTwin {
            rel: rel.clone(),
            owned: is_owned(&canonical),
            canonical,
        });
    }
    out
}

/// Where a file set aside from a bundle goes: under the app's own data,
/// never deleted, named after the binding and the moment.
pub(crate) fn set_aside_dir(data_dir: &Path, binding_id: &str) -> PathBuf {
    data_dir.join("okf").join("set-aside").join(binding_id)
}

fn set_aside(path: &Path, into: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(into).map_err(|err| err.to_string())?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let mut target = into.join(format!("{}-{name}", now_ms()));
    let mut n = 2;
    while target.exists() {
        target = into.join(format!("{}-{n}-{name}", now_ms()));
        n += 1;
    }
    if std::fs::rename(path, &target).is_err() {
        std::fs::copy(path, &target).map_err(|err| err.to_string())?;
        std::fs::remove_file(path).map_err(|err| err.to_string())?;
    }
    Ok(target)
}

fn mtime_of(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// What one tidy did.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Tidied {
    /// Owned twins resolved: the numbered copy, or the older canonical,
    /// set aside.
    pub resolved: usize,
    /// Twins of somebody's documents, left where they are and named once
    /// in the log.
    pub noted: Vec<String>,
    pub failed: usize,
}

/// Resolve the numbered twins of the files Alchemy owns, and name the rest.
///
/// For an owned file the newer of the pair keeps the canonical name and the
/// other is set aside under the app's data — a rename, never a delete, so a
/// wrong guess costs nothing. A twin with no original just takes the name.
/// A twin of a concept file is user content and stays: both are notes
/// (§5.6), and the log says so once.
pub(crate) fn tidy_cloud_twins(bundle: &Path, aside: &Path) -> Tidied {
    let mut done = Tidied::default();
    let mut lines: Vec<String> = Vec::new();
    for twin in cloud_twins(bundle) {
        let dup = bundle.join(&twin.rel);
        if is_dataless(&dup) {
            continue;
        }
        if !twin.owned {
            let line = format!(
                "The cloud left a second copy of {} as {}; both are kept as notes, look them over.",
                twin.canonical, twin.rel
            );
            lines.push(line);
            done.noted.push(twin.rel.clone());
            continue;
        }
        let canonical = bundle.join(&twin.canonical);
        let outcome: Result<(), String> = (|| {
            if !canonical.exists() {
                std::fs::rename(&dup, &canonical).map_err(|err| err.to_string())?;
                return Ok(());
            }
            if is_dataless(&canonical) {
                return Err("the original is not downloaded".into());
            }
            let same = std::fs::read(&dup).ok() == std::fs::read(&canonical).ok();
            if !same && mtime_of(&dup) > mtime_of(&canonical) {
                set_aside(&canonical, aside)?;
                std::fs::rename(&dup, &canonical).map_err(|err| err.to_string())?;
            } else {
                set_aside(&dup, aside)?;
            }
            Ok(())
        })();
        match outcome {
            Ok(()) => {
                done.resolved += 1;
                lines.push(format!(
                    "Set aside the cloud's second copy of {} ({}).",
                    twin.canonical, twin.rel
                ));
            }
            Err(err) => {
                done.failed += 1;
                crate::note!("okf: couldn't resolve {}: {err}", dup.display());
            }
        }
    }
    if !lines.is_empty() {
        // The user-content line repeats every pass the twin is there;
        // once in the log is enough.
        let existing = std::fs::read_to_string(bundle.join("log.md")).unwrap_or_default();
        for line in lines {
            if existing.contains(&line) {
                continue;
            }
            let _ = okf_log_append(bundle, &line);
        }
    }
    done
}

/// The per-pass tidy, run before a reconcile reads the folder.
pub(crate) fn tidy_bundle(data_dir: &Path, binding_id: &str, bundle: &Path) -> Tidied {
    let done = tidy_cloud_twins(bundle, &set_aside_dir(data_dir, binding_id));
    if done.resolved > 0 {
        crate::note!(
            "okf: set aside {} cloud twin(s) of files Alchemy owns under {}",
            done.resolved,
            bundle.display()
        );
    }
    done
}

// ---- the launch heal --------------------------------------------------------

/// What the launch pass did across every bound bundle.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct BloatHeal {
    pub bundles: usize,
    pub log: LogTrim,
    pub conflicts_removed: usize,
    pub conflicts_unique: usize,
    pub twins_resolved: usize,
    pub failed: usize,
}

/// Put every bound bundle's bookkeeping under its ceiling, once per store
/// under the versioned marker `okf-bloat-healed`. The per-pass rules keep
/// it there from then on; this is for the bundles that already carry the
/// 0.58 state.
pub(crate) async fn heal_bundle_bloat(state: &AppState) {
    let data_dir = app_data_dir(state);
    let stamp = data_dir.join("okf-bloat-healed");
    if std::fs::read_to_string(&stamp).is_ok_and(|v| v.trim() == BLOAT_HEAL_VERSION) {
        return;
    }
    let done = heal_bundle_bloat_checked(state).await;
    if done.log.collapsed > 0
        || done.log.entries_dropped > 0
        || done.conflicts_removed > 0
        || done.twins_resolved > 0
    {
        okf_notice(format!(
            "trimmed the bookkeeping in {} notebook folder{}: {} log entries collapsed ({} characters off, {} rolled off past the cap, {} texts put back under conflicts/, {} already in the notebook), {} conflict copies cleared, {} kept as the only copy of their text, {} cloud twins set aside",
            done.bundles,
            if done.bundles == 1 { "" } else { "s" },
            done.log.collapsed,
            done.log.chars_removed,
            done.log.entries_dropped,
            done.log.copies_restored,
            done.log.copies_dropped,
            done.conflicts_removed,
            done.conflicts_unique,
            done.twins_resolved,
        ));
    }
    if done.failed == 0 {
        if let Err(err) = std::fs::write(&stamp, BLOAT_HEAL_VERSION) {
            crate::note!("okf: couldn't stamp the bundle-bloat heal: {err}");
        }
    }
}

pub(crate) async fn heal_bundle_bloat_checked(state: &AppState) -> BloatHeal {
    let data_dir = app_data_dir(state);
    let mut done = BloatHeal::default();
    let Ok(bindings) = load_bindings_checked(&data_dir) else {
        done.failed += 1;
        return done;
    };
    for binding in bindings.values() {
        let bundle = PathBuf::from(&binding.path);
        if !bundle.is_dir() {
            continue;
        }
        done.bundles += 1;
        let tidied = tidy_bundle(&data_dir, &binding.id, &bundle);
        done.twins_resolved += tidied.resolved;
        done.failed += tidied.failed;
        let manifest_at = manifest_path(&data_dir, &binding.id);
        let mut manifest = match load_manifest_checked(&manifest_at) {
            Ok(m) => m,
            Err(err) => {
                done.failed += 1;
                crate::note!(
                    "okf: couldn't read the record for {}: {err}",
                    bundle.display()
                );
                continue;
            }
        };
        match trim_log(state, &bundle, &manifest).await {
            Ok(trim) => {
                done.log.collapsed += trim.collapsed;
                done.log.copies_restored += trim.copies_restored;
                done.log.copies_dropped += trim.copies_dropped;
                done.log.entries_dropped += trim.entries_dropped;
                done.log.chars_removed += trim.chars_removed;
            }
            Err(err) => {
                done.failed += 1;
                crate::note!("okf: couldn't trim {}/log.md: {err}", bundle.display());
                continue;
            }
        }
        let pruned = prune_conflicts(state, &bundle, &manifest).await;
        done.conflicts_removed += pruned.removed;
        done.conflicts_unique += pruned.unique.len();
        done.failed += pruned.failed;
        if !pruned.unique.is_empty() {
            let mut names: Vec<String> = pruned
                .unique
                .iter()
                .take(10)
                .map(|c| {
                    format!(
                        "conflicts/{} ({})",
                        c.path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or_default(),
                        c.rel
                    )
                })
                .collect();
            if pruned.unique.len() > names.len() {
                names.push(format!("and {} more", pruned.unique.len() - names.len()));
            }
            let _ = okf_log_append(
                &bundle,
                &format!(
                    "Kept {} conflict cop{} holding text found nowhere else: {}.",
                    pruned.unique.len(),
                    if pruned.unique.len() == 1 { "y" } else { "ies" },
                    names.join(", ")
                ),
            );
        }
        manifest.conflicts_pruned_at = now_ms();
        if let Err(err) = save_manifest_checked(&manifest_at, &manifest) {
            crate::note!("okf: couldn't record the conflict prune: {err}");
        }
    }
    done
}

#[cfg(test)]
mod tests;
