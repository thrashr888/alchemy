//! Which Mac a source came from, and what that means when its file is not
//! here (docs/RFC-okf-live.md §5.8).
//!
//! A bundle that travels through a shared folder carries every source's text,
//! but not the drive the text was read off. On the far Mac those paths point
//! at a OneDrive mount that will never exist, and the app used to call that a
//! missing file: flagged for removal, listed under Missing, and offered a
//! Refresh that could only ever fail. The origin device is the one fact that
//! tells the two cases apart — a file deleted out from under the notebook,
//! and a file that was never on this machine to begin with.
//!
//! The record lives beside the store, not in it: one file per notebook in the
//! same per-parent sidecar shape §5.6's edits use. One store serves the
//! installed app and every dev build, so a column is a release-timing hazard,
//! and this is a fact only listings and the bundle writer read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::models::Source;

/// This Mac's name, as its owner would say it — "Paul's MacBook Pro", not a
/// Bonjour host name. That is what a hint has to name for the sentence to be
/// worth reading, and `scutil` is where macOS keeps it.
///
/// Read once: the answer is a subprocess, and every source listing asks.
pub fn this_device() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        for (bin, args) in [
            ("/usr/sbin/scutil", &["--get", "ComputerName"][..]),
            ("/bin/hostname", &["-s"][..]),
        ] {
            if let Some(name) = std::process::Command::new(bin)
                .args(args)
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                return name;
            }
        }
        "unknown".to_string()
    })
}

/// Two device names for the same Mac. Case and stray spacing are not a
/// different machine — "Paul's MacBook Pro" arrives through frontmatter, a
/// file system, and a text editor before it is compared.
pub fn same_device(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Is this source remote — its text here, its origin somewhere else?
///
/// Three facts and no I/O, so the rule can be read in one place and asserted
/// without a file system. `origin_path_exists` is `None` for a source with no
/// local origin at all (a web page, a pasted note, a `cider://` mirror): those
/// work here exactly as they work there, and nothing about them is remote.
///
/// An unknown origin device is this Mac's. Every source that predates the
/// record is one of ours, and guessing the other way would hide a genuinely
/// missing file behind a device name we never had.
///
/// The path is asked second on purpose: when the same drive is mounted on
/// both Macs the source is local in every way that matters, and it becomes
/// local again the moment the path appears — nothing has to be un-marked.
pub fn is_remote(origin_device: &str, this_device: &str, origin_path_exists: Option<bool>) -> bool {
    if origin_device.trim().is_empty() || same_device(origin_device, this_device) {
        return false;
    }
    origin_path_exists == Some(false)
}

/// The path a source depends on being able to reach. `None` means it depends
/// on none: a URL, `cider://`, pasted text.
///
/// A folder child answers with its parent's root rather than its own file.
/// The drive is the thing that is either mounted or not; one file missing
/// underneath a mounted root is a deletion, and the rescan owns that.
pub fn origin_path<'a>(source: &'a Source, parents: &HashMap<&str, &'a str>) -> Option<&'a str> {
    let local = |url: &'a str| -> Option<&'a str> {
        let url = url.trim();
        (!url.is_empty() && !crate::commands::is_web_url(url) && !crate::mac::is_mac_uri(url))
            .then_some(url)
    };
    if !source.parent_id.is_empty() {
        if let Some(root) = parents.get(source.parent_id.as_str()).copied() {
            return local(root);
        }
    }
    local(&source.url)
}

fn devices_path(data_dir: &Path, notebook_id: &str) -> PathBuf {
    data_dir
        .join("okf_devices")
        .join(format!("{notebook_id}.json"))
}

/// Every source in one notebook that came from somewhere else, by id. One
/// file read per listing, and a directory miss for the notebooks that have
/// never left this Mac.
pub fn load_origin_devices(data_dir: &Path, notebook_id: &str) -> HashMap<String, String> {
    std::fs::read_to_string(devices_path(data_dir, notebook_id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Record where a source came from — called by the two import paths when a
/// concept's frontmatter names a device.
///
/// This Mac's own name is not recorded: absent means ours, so the file stays
/// the size of what actually travelled rather than a row per source.
pub fn note_origin_device(data_dir: &Path, notebook_id: &str, source_id: &str, device: &str) {
    if let Err(err) = note_origin_device_checked(data_dir, notebook_id, source_id, device) {
        crate::diagnostics::error("source-device", err.to_string());
    }
}

/// Sync must persist provenance before publishing a source row. A damaged
/// sidecar is an error here, rather than permission to replace its history.
pub(crate) fn note_origin_device_checked(
    data_dir: &Path,
    notebook_id: &str,
    source_id: &str,
    device: &str,
) -> anyhow::Result<()> {
    use std::io::Write;
    let device = device.trim();
    if device.is_empty() || same_device(device, this_device()) {
        return Ok(());
    }
    let path = devices_path(data_dir, notebook_id);
    let mut map: HashMap<String, String> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(err) => return Err(err.into()),
    };
    map.insert(source_id.to_string(), device.to_string());
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Missing device record directory"))?;
    std::fs::create_dir_all(dir)?;
    let mut staged = tempfile::NamedTempFile::new_in(dir)?;
    staged.write_all(&serde_json::to_vec_pretty(&map)?)?;
    staged.as_file().sync_all()?;
    staged.persist(&path).map_err(|err| err.error)?;
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// Fill in `origin_device` and `remote` for a notebook's sources.
///
/// The rows come out of the store without either — the store does not know
/// about devices — so every surface that answers "what is wrong with this
/// source" runs this first: the listing, the hygiene review, the folder
/// rescan, the bundle writer. One sidecar read and one stat per distinct
/// origin path, however many children share it.
pub fn mark_remote(data_dir: &Path, notebook_id: &str, sources: &mut [Source]) {
    let devices = load_origin_devices(data_dir, notebook_id);
    if devices.is_empty() {
        for s in sources.iter_mut() {
            s.origin_device = this_device().to_string();
            s.remote = false;
        }
        return;
    }
    let parents: HashMap<&str, &str> = sources
        .iter()
        .map(|s| (s.id.as_str(), s.url.as_str()))
        .collect();
    // Paths, then verdicts: `parents` borrows the slice, so nothing may be
    // written back while it is alive.
    let mut exists: HashMap<&str, bool> = HashMap::new();
    let verdicts: Vec<(String, bool)> = sources
        .iter()
        .map(|s| {
            let device = devices
                .get(&s.id)
                .cloned()
                .unwrap_or_else(|| this_device().to_string());
            let here = origin_path(s, &parents).map(|p| {
                *exists.entry(p).or_insert_with(|| {
                    Path::new(p).exists() || crate::okf::is_evicted_stub(Path::new(p))
                })
            });
            let remote = is_remote(&device, this_device(), here);
            (device, remote)
        })
        .collect();
    for (s, (device, remote)) in sources.iter_mut().zip(verdicts) {
        s.origin_device = device;
        s.remote = remote;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(id: &str, url: &str, parent: &str) -> Source {
        Source {
            origin_device: String::new(),
            remote: false,
            id: id.into(),
            notebook_id: "nb".into(),
            title: id.into(),
            source_type: "pdf".into(),
            url: url.into(),
            content: String::new(),
            char_count: 0,
            chunk_count: 0,
            created_at: 0,
            status: "ready".into(),
            error: String::new(),
            parent_id: parent.into(),
            mtime: 0,
            author: String::new(),
            image_url: String::new(),
            tags: String::new(),
            note: String::new(),
            fetched_at: 0,
            fetch_failures: 0,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("alchemy-device-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// The whole rule, one table. Both halves have to hold: another Mac's
    /// name is not enough (the same drive mounted here makes the source ours
    /// again), and an absent path is not enough (that is the missing file
    /// this is carefully not hiding).
    #[test]
    fn remote_needs_a_foreign_device_and_an_unreachable_path() {
        let here = "Paul's iMac";
        let away = "Paul's MacBook Pro";
        for (device, exists, want, why) in [
            (away, Some(false), true, "elsewhere, and not mounted here"),
            (away, Some(true), false, "the same drive on both Macs"),
            (away, None, false, "a web page has no drive to miss"),
            (here, Some(false), false, "ours, and genuinely gone"),
            ("", Some(false), false, "unrecorded means ours"),
            ("  ", Some(false), false, "blank means ours"),
            ("paul's MACBOOK pro", Some(false), true, "case is not a Mac"),
        ] {
            assert_eq!(
                is_remote(device, here, exists),
                want,
                "{why}: {device:?} / {exists:?}"
            );
        }
    }

    /// A child asks about its parent's root, not its own file: the drive is
    /// the thing that is mounted or not, and one file missing under a mounted
    /// root is a deletion the rescan owns.
    #[test]
    fn a_child_depends_on_its_parents_root() {
        let sources = [
            src("p", "/Volumes/Work/Docs", ""),
            src("c", "/Volumes/Work/Docs/q3.pdf", "p"),
            src("l", "/Users/paul/notes.md", ""),
            src("t", "", ""),
            src("w", "https://example.com/page", ""),
        ];
        let parents: HashMap<&str, &str> = sources
            .iter()
            .map(|s| (s.id.as_str(), s.url.as_str()))
            .collect();

        assert_eq!(
            origin_path(&sources[1], &parents),
            Some("/Volumes/Work/Docs")
        );
        assert_eq!(
            origin_path(&sources[2], &parents),
            Some("/Users/paul/notes.md")
        );
        assert_eq!(origin_path(&sources[3], &parents), None);
        assert_eq!(origin_path(&sources[4], &parents), None);
    }

    /// The sidecar records only what came from somewhere else, and marking
    /// reads it back: an unrecorded source is this Mac's, and a recorded one
    /// whose path is absent is remote. This Mac's own name is never written —
    /// absent already means ours.
    #[test]
    fn the_sidecar_records_only_what_travelled() {
        let dir = scratch("sidecar");
        let gone = dir.join("never-mounted/plan.pdf");
        let here = dir.join("here.md");
        std::fs::write(&here, "text").expect("write");

        note_origin_device(&dir, "nb", "away", "Paul's MacBook Pro");
        note_origin_device(&dir, "nb", "same-drive", "Paul's MacBook Pro");
        note_origin_device(&dir, "nb", "ours", this_device());
        let recorded = load_origin_devices(&dir, "nb");
        assert_eq!(recorded.len(), 2, "this Mac's own name is not worth a row");
        assert!(!recorded.contains_key("ours"));

        let mut sources = [
            src("away", &gone.to_string_lossy(), ""),
            src("same-drive", &here.to_string_lossy(), ""),
            src("ours", &gone.to_string_lossy(), ""),
        ];
        mark_remote(&dir, "nb", &mut sources);
        assert!(sources[0].remote);
        assert_eq!(sources[0].origin_device, "Paul's MacBook Pro");
        assert!(!sources[1].remote, "the path resolves here, so it is local");
        assert!(!sources[2].remote, "unrecorded is this Mac's");
        assert_eq!(sources[2].origin_device, this_device());

        // The drive turns up: nothing has to be un-marked, the same pass just
        // answers differently.
        std::fs::create_dir_all(gone.parent().expect("parent")).expect("mkdir");
        std::fs::write(&gone, "text").expect("write");
        mark_remote(&dir, "nb", &mut sources);
        assert!(!sources[0].remote);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A notebook that has never left this Mac reads no paths at all — the
    /// sidecar is missing, and every source is ours by definition.
    #[test]
    fn a_notebook_that_never_travelled_costs_nothing() {
        let dir = scratch("untravelled");
        let mut sources = [src("a", "/nope/gone.pdf", "")];
        mark_remote(&dir, "nb", &mut sources);
        assert!(!sources[0].remote);
        assert_eq!(sources[0].origin_device, this_device());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
