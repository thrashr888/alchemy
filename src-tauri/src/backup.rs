//! Data trust (docs/RFC-night-shift-area.md §7): the nightly snapshot, the
//! store-version stamp, and the restore path.
//!
//! The store is one embedded LanceDB directory holding everything the user
//! has. Losing it is the only unrecoverable failure in the app, which is why
//! this is the first real work the Night Shift does and why it is mechanical:
//! gated by `background_enabled` alone, never by AI spend or the overnight
//! pause.
//!
//! Snapshots use APFS `clonefile(2)` (via `cp -c`), so a multi-gigabyte store
//! costs almost no time and almost no disk until its blocks diverge. On a
//! non-APFS volume the same call falls back to a real copy — slower, still
//! correct.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::Datelike;

/// Bumped whenever a migration makes the store unreadable by older binaries.
/// A stamp higher than this means the library was opened by a newer Alchemy;
/// see `check_store_version`.
pub const STORE_VERSION: u32 = 1;

const STAMP_FILE: &str = "store_version";

/// Nightly snapshots kept, then one per week beyond that.
const KEEP_DAILY: usize = 7;
const KEEP_WEEKLY: usize = 4;

pub fn store_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("lancedb")
}

pub fn backups_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

fn snapshots_dir(data_dir: &Path) -> PathBuf {
    backups_dir(data_dir).join("store")
}

/// Where the nightly OKF escape hatch writes (docs/RFC-night-shift-area.md §7,
/// docs/RFC-okf-live.md §3): one bundle directory per notebook, replaced each
/// night. Unlike the store snapshots this is not dated and never pruned —
/// there is one copy, always current, and it costs kilobytes of markdown.
pub fn okf_latest_dir(data_dir: &Path) -> PathBuf {
    backups_dir(data_dir).join("okf").join("latest")
}

/// The day the OKF pass last ran, so an hourly tick writes the bundles once.
/// A file rather than an in-memory stamp: a relaunch should not cost the day
/// a second full rewrite of every notebook.
fn okf_stamp_file(data_dir: &Path) -> PathBuf {
    backups_dir(data_dir).join("okf").join("last-run")
}

pub fn okf_last_run(data_dir: &Path) -> String {
    std::fs::read_to_string(okf_stamp_file(data_dir))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn set_okf_last_run(data_dir: &Path, day: &str) {
    let path = okf_stamp_file(data_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, day);
}

/// Copy a directory tree, preferring APFS clones. Returns whether the clone
/// path was taken — worth knowing, because a fallback copy on a big store is
/// the difference between milliseconds and minutes.
fn clone_tree(src: &Path, dst: &Path) -> Result<bool> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).context("failed to create the snapshot directory")?;
    }
    // -c clones on APFS; -R recurses. Failure here is not fatal: fall back to
    // a plain recursive copy so non-APFS volumes still get a snapshot.
    let cloned = std::process::Command::new("/bin/cp")
        .arg("-Rc")
        .arg(src)
        .arg(dst)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if cloned {
        return Ok(true);
    }
    let _ = std::fs::remove_dir_all(dst);
    let ok = std::process::Command::new("/bin/cp")
        .arg("-R")
        .arg(src)
        .arg(dst)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Ok(false)
    } else {
        Err(anyhow!("could not copy the store to {}", dst.display()))
    }
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        match entry.file_type() {
            Ok(t) if t.is_dir() => total += dir_size(&entry.path()),
            Ok(_) => total += entry.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => {}
        }
    }
    total
}

/// Existing snapshots, newest first. Names are `YYYY-MM-DD`, so lexical
/// ordering is chronological — no metadata read needed to sort.
fn existing_snapshots(data_dir: &Path) -> Vec<PathBuf> {
    let dir = snapshots_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    out.sort();
    out.reverse();
    out
}

/// The most recent snapshot, if any: its path, when it was taken, and its
/// size on disk. Clones share blocks with the live store, so this number is
/// an upper bound on what the snapshot actually costs.
pub fn latest_snapshot(data_dir: &Path) -> Option<(PathBuf, i64, u64)> {
    let path = existing_snapshots(data_dir).into_iter().next()?;
    let taken = path
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let bytes = dir_size(&path);
    Some((path, taken, bytes))
}

/// Keep the last `KEEP_DAILY` snapshots, then one per ISO week for
/// `KEEP_WEEKLY` weeks. Everything older goes.
fn prune(data_dir: &Path) {
    let snaps = existing_snapshots(data_dir);
    if snaps.len() <= KEEP_DAILY {
        return;
    }
    let mut kept_weeks: Vec<String> = Vec::new();
    for path in snaps.iter().skip(KEEP_DAILY) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // "YYYY-MM-DD" → ISO week key, so one survivor per week is kept.
        let week = chrono::NaiveDate::parse_from_str(&name, "%Y-%m-%d")
            .map(|d| {
                let iso = d.iso_week();
                format!("{}-{}", iso.year(), iso.week())
            })
            .unwrap_or_default();
        let keep =
            !week.is_empty() && kept_weeks.len() < KEEP_WEEKLY && !kept_weeks.contains(&week);
        if keep {
            kept_weeks.push(week);
        } else {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

pub struct SnapshotOutcome {
    pub path: PathBuf,
    pub bytes: u64,
    pub cloned: bool,
}

/// Take today's snapshot. Idempotent within a day: a second call replaces the
/// same dated directory rather than piling up.
pub fn snapshot(data_dir: &Path) -> Result<SnapshotOutcome> {
    let src = store_dir(data_dir);
    if !src.exists() {
        return Err(anyhow!("no store to snapshot yet"));
    }
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let dst = snapshots_dir(data_dir).join(&day);
    if dst.exists() {
        std::fs::remove_dir_all(&dst).context("failed to replace today's snapshot")?;
    }
    let cloned = clone_tree(&src, &dst)?;
    prune(data_dir);
    Ok(SnapshotOutcome {
        bytes: dir_size(&dst),
        path: dst,
        cloned,
    })
}

/// Clone the store aside before a migration appends columns. Named by the
/// version doing the appending, so a downgrade has something to go back to.
pub fn snapshot_pre_migrate(data_dir: &Path, version: &str) -> Result<PathBuf> {
    let src = store_dir(data_dir);
    if !src.exists() {
        return Err(anyhow!("no store to snapshot"));
    }
    let dst = backups_dir(data_dir).join("pre-migrate").join(version);
    if dst.exists() {
        // Already rehearsed this upgrade; the earlier copy is the older, and
        // therefore better, one to keep.
        return Ok(dst);
    }
    clone_tree(&src, &dst)?;
    Ok(dst)
}

/// Read the store's version stamp. A missing stamp means a store written
/// before stamping existed — that is not an error, it is version 0.
pub fn read_stamp(data_dir: &Path) -> u32 {
    std::fs::read_to_string(store_dir(data_dir).join(STAMP_FILE))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

pub fn write_stamp(data_dir: &Path) {
    let path = store_dir(data_dir).join(STAMP_FILE);
    if let Err(err) = std::fs::write(&path, STORE_VERSION.to_string()) {
        crate::diagnostics::error("backup", format!("could not stamp the store: {err}"));
    }
}

/// Refuse a library written by a newer Alchemy instead of letting Lance
/// panic on a column this binary has never heard of. Returns the offending
/// stamp when the store is too new.
pub fn check_store_version(data_dir: &Path) -> Option<u32> {
    let stamp = read_stamp(data_dir);
    (stamp > STORE_VERSION).then_some(stamp)
}

/// Put the most recent snapshot back. The broken store is renamed aside,
/// never deleted — a failed restore must not be the thing that loses the
/// library. Returns where the old store went.
pub fn restore_latest(data_dir: &Path) -> Result<PathBuf> {
    let (snapshot, _, _) =
        latest_snapshot(data_dir).ok_or_else(|| anyhow!("no snapshot to restore"))?;
    let live = store_dir(data_dir);
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let aside = data_dir.join(format!("lancedb.broken-{stamp}"));
    if live.exists() {
        std::fs::rename(&live, &aside).context("failed to move the damaged store aside")?;
    }
    if let Err(err) = clone_tree(&snapshot, &live) {
        // Put the original back rather than leaving the user with nothing.
        let _ = std::fs::rename(&aside, &live);
        return Err(err);
    }
    Ok(aside)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("alchemy-backup-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(store_dir(&dir)).expect("create store");
        std::fs::write(store_dir(&dir).join("data.txt"), b"library").expect("seed");
        dir
    }

    #[test]
    fn snapshot_copies_the_store_and_is_idempotent_per_day() {
        let dir = scratch("snap");
        let first = snapshot(&dir).expect("first snapshot");
        assert!(first.path.join("data.txt").exists(), "content came along");

        // A second call the same day replaces rather than accumulates.
        std::fs::write(store_dir(&dir).join("data.txt"), b"library, revised").expect("edit");
        let second = snapshot(&dir).expect("second snapshot");
        assert_eq!(first.path, second.path, "same dated directory");
        let restored = std::fs::read_to_string(second.path.join("data.txt")).expect("read");
        assert_eq!(
            restored, "library, revised",
            "snapshot reflects the latest store"
        );
        assert_eq!(existing_snapshots(&dir).len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_recent_days_then_one_per_week() {
        let dir = scratch("prune");
        // 30 consecutive days, oldest first.
        let start = chrono::NaiveDate::from_ymd_opt(2026, 6, 1).expect("date");
        for i in 0..30 {
            let day = start + chrono::Duration::days(i);
            let path = snapshots_dir(&dir).join(day.format("%Y-%m-%d").to_string());
            std::fs::create_dir_all(&path).expect("mkdir");
        }
        prune(&dir);
        let left = existing_snapshots(&dir);
        assert!(left.len() >= KEEP_DAILY, "the daily window survives");
        assert!(
            left.len() <= KEEP_DAILY + KEEP_WEEKLY,
            "everything beyond the weekly allowance is pruned, got {}",
            left.len()
        );
        // The newest KEEP_DAILY are always kept, contiguously.
        let newest = left[0].file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(newest, "2026-06-30");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stamp_round_trips_and_only_newer_stores_are_refused() {
        let dir = scratch("stamp");
        assert_eq!(read_stamp(&dir), 0, "an unstamped store is version 0");
        assert!(check_store_version(&dir).is_none(), "older is readable");

        write_stamp(&dir);
        assert_eq!(read_stamp(&dir), STORE_VERSION);
        assert!(check_store_version(&dir).is_none(), "our own stamp is fine");

        std::fs::write(
            store_dir(&dir).join(STAMP_FILE),
            (STORE_VERSION + 1).to_string(),
        )
        .expect("write future stamp");
        assert_eq!(
            check_store_version(&dir),
            Some(STORE_VERSION + 1),
            "a newer store is refused rather than opened"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_puts_the_snapshot_back_and_keeps_the_broken_store() {
        let dir = scratch("restore");
        snapshot(&dir).expect("snapshot");

        // Corrupt the live store, then restore.
        std::fs::write(store_dir(&dir).join("data.txt"), b"corrupted").expect("corrupt");
        let aside = restore_latest(&dir).expect("restore");

        let live = std::fs::read_to_string(store_dir(&dir).join("data.txt")).expect("read live");
        assert_eq!(live, "library", "the good copy is back");
        let kept = std::fs::read_to_string(aside.join("data.txt")).expect("read aside");
        assert_eq!(
            kept, "corrupted",
            "the damaged store is moved aside, never deleted"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
