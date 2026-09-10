//! Durable machine-local binding transactions. Keep the lock file separate
//! from the JSON inode so atomic replacement cannot split competing writers.
use super::*;
use std::io::Write;

type Bindings = HashMap<String, OkfBinding>;
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn report(error: impl std::fmt::Display) -> String {
    let message = format!("Notebook binding records could not be accessed: {error}");
    crate::diagnostics::error("okf-bindings", &message);
    message
}

pub(super) fn load(data_dir: &Path) -> Result<Bindings, String> {
    if !data_dir.try_exists().map_err(report)? {
        return Ok(Bindings::new());
    }
    let _guard = LOCK
        .lock()
        .map_err(|_| report("Binding transaction lock is poisoned"))?;
    let _file = process_lock(data_dir)?;
    load_unlocked(data_dir)
}

fn load_unlocked(data_dir: &Path) -> Result<Bindings, String> {
    let path = bindings_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(report),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if path
                .with_extension("initialized")
                .try_exists()
                .map_err(report)?
            {
                Err(report(
                    "The established binding record is missing; restore it before syncing",
                ))
            } else {
                Ok(Bindings::new())
            }
        }
        Err(error) => Err(report(error)),
    }
}

fn persist(data_dir: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    persist_at(data_dir, &bindings_path(data_dir), value)
}

fn persist_at(data_dir: &Path, path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    // Serialization failure must not change even the initialization marker.
    let bytes = serde_json::to_vec_pretty(value).map_err(report)?;
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let mut staged = tempfile::NamedTempFile::new_in(data_dir)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        let marker = path.with_extension("initialized");
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)
        {
            Ok(file) => file.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        // A crash during first initialization must never make established
        // records appear to be an unused installation on the next startup.
        std::fs::File::open(data_dir)?.sync_all()?;
        staged.persist(path).map_err(|error| error.error)?;
        std::fs::File::open(data_dir)?.sync_all()?;
        Ok(())
    })();
    result.map_err(report)
}

fn process_lock(data_dir: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(data_dir).map_err(report)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join("okf-bindings.lock"))
        .map_err(report)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        loop {
            // SAFETY: the owned file descriptor stays open for this call and
            // for the entire transaction. Dropping File releases the flock.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(report(error));
            }
        }
    }
    Ok(file)
}

pub(super) fn update<R>(
    data_dir: &Path,
    update: impl FnOnce(&mut Bindings) -> Result<R, String>,
) -> Result<R, String> {
    let _guard = LOCK
        .lock()
        .map_err(|_| report("Binding transaction lock is poisoned"))?;
    let _file = process_lock(data_dir)?;
    update_unlocked(data_dir, update)
}

fn update_unlocked<R>(
    data_dir: &Path,
    update: impl FnOnce(&mut Bindings) -> Result<R, String>,
) -> Result<R, String> {
    let mut current = load_unlocked(data_dir)?;
    let original = current.clone();
    let result = update(&mut current)?;
    // A successful transaction also upgrades legacy records on a no-op,
    // establishing missing-file detection without requiring a field change.
    if current != original
        || !bindings_path(data_dir).exists()
        || !bindings_path(data_dir)
            .with_extension("initialized")
            .exists()
    {
        persist(data_dir, &current)?;
    }
    Ok(result)
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Detached {
    notebooks: std::collections::BTreeMap<String, Option<OkfBinding>>,
    folders: std::collections::BTreeSet<PathBuf>,
}

fn detached_path(data_dir: &Path) -> PathBuf {
    data_dir.join("okf-detached.json")
}

fn detached_unlocked(data_dir: &Path) -> Result<Detached, String> {
    let path = detached_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(report),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if path
                .with_extension("initialized")
                .try_exists()
                .map_err(report)?
            {
                return Err(report("The established notebook detach record is missing; restore it before discovering notebooks"));
            }
            Ok(Detached::default())
        }
        Err(error) => Err(report(error)),
    }
}

fn blocked(detached: &Detached, notebook_id: Option<&str>, folder: &Path) -> bool {
    notebook_id.is_some_and(|id| detached.notebooks.contains_key(id))
        || detached.folders.contains(&same_folder(folder))
}

pub(super) fn discovery_blocked(
    data_dir: &Path,
    notebook_id: Option<&str>,
    folder: &Path,
) -> Result<bool, String> {
    let _guard = LOCK
        .lock()
        .map_err(|_| report("Binding transaction lock is poisoned"))?;
    let _file = process_lock(data_dir)?;
    Ok(blocked(&detached_unlocked(data_dir)?, notebook_id, folder))
}

/// The detach intent becomes durable before removal of the active binding.
/// A crash between the two must never authorize automatic reattachment.
pub(super) fn detach(data_dir: &Path, notebook_id: &str) -> Result<(), String> {
    let _guard = LOCK
        .lock()
        .map_err(|_| report("Binding transaction lock is poisoned"))?;
    let _file = process_lock(data_dir)?;
    let current = load_unlocked(data_dir)?;
    let mut detached = detached_unlocked(data_dir)?;
    if let Some(binding) = current.get(notebook_id) {
        detached
            .notebooks
            .insert(notebook_id.to_string(), Some(binding.clone()));
        detached.folders.insert(same_folder(&binding.path));
    } else {
        detached
            .notebooks
            .entry(notebook_id.to_string())
            .or_insert(None);
    }
    persist_at(data_dir, &detached_path(data_dir), &detached)?;
    update_unlocked(data_dir, |bindings| {
        bindings.remove(notebook_id);
        Ok(())
    })
}

/// Called only after an explicit binding has been published, while its
/// notebook lock is still held. A failed import leaves detach intent intact.
pub(super) fn allow_explicit(
    data_dir: &Path,
    notebook_id: &str,
    folder: &Path,
    binding_id: &str,
) -> Result<(), String> {
    let _guard = LOCK
        .lock()
        .map_err(|_| report("Binding transaction lock is poisoned"))?;
    let _file = process_lock(data_dir)?;
    if !load_unlocked(data_dir)?
        .get(notebook_id)
        .is_some_and(|binding| {
            binding.id == binding_id && same_folder(&binding.path) == same_folder(folder)
        })
    {
        return Err(report(
            "The explicit binding changed before its detach intent could be cleared",
        ));
    }
    let mut detached = detached_unlocked(data_dir)?;
    let notebook = detached.notebooks.remove(notebook_id).is_some();
    let folder = detached.folders.remove(&same_folder(folder));
    if notebook || folder {
        persist_at(data_dir, &detached_path(data_dir), &detached)?;
    }
    Ok(())
}

/// An explicit resume of the same folder retains the previous manifest and
/// its local row identities, instead of importing its files as fresh items.
pub(super) fn detached_binding(
    data_dir: &Path,
    notebook_id: &str,
    folder: &Path,
) -> Result<Option<OkfBinding>, String> {
    let _guard = LOCK
        .lock()
        .map_err(|_| report("Binding transaction lock is poisoned"))?;
    let _file = process_lock(data_dir)?;
    Ok(detached_unlocked(data_dir)?
        .notebooks
        .remove(notebook_id)
        .flatten()
        .filter(|binding| same_folder(&binding.path) == same_folder(folder)))
}

/// The binding ids a detached notebook could resume under. A record kept
/// for them is still read, and must not be set aside as unread.
pub(super) fn detached_ids(data_dir: &Path) -> Result<Vec<String>, String> {
    let _guard = LOCK
        .lock()
        .map_err(|_| report("Binding transaction lock is poisoned"))?;
    let _file = process_lock(data_dir)?;
    Ok(detached_unlocked(data_dir)?
        .notebooks
        .into_values()
        .flatten()
        .map(|binding| binding.id)
        .collect())
}

/// Recheck explicit detach intent under the same process lock that publishes
/// automatic bindings, so an earlier discovery scan cannot undo an unbind.
pub(super) fn update_discovered<R>(
    data_dir: &Path,
    notebook_id: &str,
    folder: &Path,
    update: impl FnOnce(&mut Bindings) -> Result<R, String>,
) -> Result<Option<R>, String> {
    let _guard = LOCK
        .lock()
        .map_err(|_| report("Binding transaction lock is poisoned"))?;
    let _file = process_lock(data_dir)?;
    if blocked(&detached_unlocked(data_dir)?, Some(notebook_id), folder) {
        return Ok(None);
    }
    update_unlocked(data_dir, update).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(id: &str) -> OkfBinding {
        OkfBinding {
            id: id.into(),
            path: format!("/test/{id}"),
            ..Default::default()
        }
    }

    #[test]
    fn corruption_and_missing_initialized_records_block_mutation() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_bindings_checked(dir.path()).unwrap().is_empty());
        set_binding_checked(dir.path(), "one", Some(binding("one"))).unwrap();
        let path = bindings_path(dir.path());
        std::fs::write(&path, b"{ interrupted").unwrap();
        assert!(load_bindings_checked(dir.path()).is_err());
        assert!(set_binding_checked(dir.path(), "two", Some(binding("two"))).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{ interrupted");
        std::fs::remove_file(&path).unwrap();
        assert!(load_bindings_checked(dir.path()).is_err());
        assert!(set_binding_checked(dir.path(), "two", Some(binding("two"))).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn failed_serialization_and_transactions_keep_previous_bytes() {
        struct CannotSerialize;
        impl serde::Serialize for CannotSerialize {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("injected serialization failure"))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        set_binding_checked(dir.path(), "one", Some(binding("one"))).unwrap();
        let before = std::fs::read(bindings_path(dir.path())).unwrap();
        assert!(persist(dir.path(), &CannotSerialize).is_err());
        assert!(update_bindings_checked(dir.path(), |map| {
            map.clear();
            Err::<(), _>("cancel transaction".into())
        })
        .is_err());
        assert_eq!(std::fs::read(bindings_path(dir.path())).unwrap(), before);
    }

    #[test]
    fn stale_touch_cannot_restore_removed_or_replaced_bindings() {
        let dir = tempfile::tempdir().unwrap();
        set_binding_checked(dir.path(), "one", Some(binding("old"))).unwrap();
        set_binding_checked(dir.path(), "one", Some(binding("new"))).unwrap();
        touch_last_write_checked(dir.path(), "one", "old", 55).unwrap();
        assert_eq!(
            binding_for_checked(dir.path(), "one")
                .unwrap()
                .unwrap()
                .last_write_at,
            0
        );
        set_binding_checked(dir.path(), "one", None).unwrap();
        touch_last_write_checked(dir.path(), "one", "new", 99).unwrap();
        assert!(binding_for_checked(dir.path(), "one").unwrap().is_none());
    }

    #[test]
    fn concurrent_transactions_keep_every_record() {
        let dir = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let path = dir.path();
                scope.spawn(move || {
                    for item in 0..8 {
                        let id = format!("{worker}-{item}");
                        set_binding_checked(path, &id, Some(binding(&id))).unwrap();
                    }
                });
            }
        });
        assert_eq!(load_bindings_checked(dir.path()).unwrap().len(), 64);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn legacy_records_gain_missing_file_protection_on_noop_transaction() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(bindings_path(dir.path()), b"{}").unwrap();
        update_bindings_checked(dir.path(), |_| Ok(())).unwrap();
        std::fs::remove_file(bindings_path(dir.path())).unwrap();
        assert!(load_bindings_checked(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn competing_processes_keep_every_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut children = Vec::new();
        for worker in 0..2 {
            children.push(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "okf::bindings::tests::process_worker",
                        "--ignored",
                    ])
                    .env("ALCHEMY_BINDING_TEST_DIR", dir.path())
                    .env("ALCHEMY_BINDING_TEST_WORKER", worker.to_string())
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            );
        }
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(load_bindings_checked(dir.path()).unwrap().len(), 32);
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "Subprocess fixture for competing_processes_keep_every_record"]
    fn process_worker() {
        let Some(dir) = std::env::var_os("ALCHEMY_BINDING_TEST_DIR") else {
            return;
        };
        let worker = std::env::var("ALCHEMY_BINDING_TEST_WORKER").unwrap();
        for item in 0..16 {
            let id = format!("{worker}-{item}");
            set_binding_checked(Path::new(&dir), &id, Some(binding(&id))).unwrap();
        }
    }
}
