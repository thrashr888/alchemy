//! Immutable, portable lifetime tombstones. A delete names an entity, never a
//! filename or content hash, so old revisions cannot recreate that lifetime.
use super::*;
use std::collections::HashSet;
use std::io::Write;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Deletion {
    version: u32,
    id: String,
}

fn validate_id(id: &str) -> Result<(), String> {
    let parsed =
        uuid::Uuid::parse_str(id).map_err(|_| "Invalid portable deletion UUID".to_string())?;
    if parsed.is_nil() || parsed.to_string() != id {
        return Err("Portable deletion UUID must be nonzero and canonical".into());
    }
    Ok(())
}

fn read_record(path: &Path, id: &str) -> Result<(), String> {
    validate_id(id)?;
    if is_evicted_stub(path) {
        hydrate_if_evicted(path);
        return Err(format!(
            "Waiting for deletion record {} to download",
            path.display()
        ));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|err| format!("Could not inspect deletion record: {err}"))?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err("Invalid portable deletion record file".into());
    }
    let bytes =
        std::fs::read(path).map_err(|err| format!("Could not read deletion record: {err}"))?;
    let record: Deletion = serde_json::from_slice(&bytes)
        .map_err(|err| format!("Invalid deletion record {}: {err}", path.display()))?;
    if record.version != 1 || record.id != id {
        return Err(format!(
            "Deletion record identity or version mismatch: {}",
            path.display()
        ));
    }
    Ok(())
}

pub(super) fn read_deleted(bundle: &Path) -> Result<HashSet<String>, String> {
    let directory = bundle.join("sync/deletions");
    for path in [bundle.join("sync"), directory.clone()] {
        if is_evicted_stub(&path) {
            hydrate_if_evicted(&path);
            return Err("Waiting for notebook deletion records to download".into());
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if !metadata.is_dir() {
                return Err("Invalid notebook deletion record directory".into());
            }
        }
    }
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(err) => return Err(format!("Could not list notebook deletion records: {err}")),
    };
    let mut deleted = HashSet::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| format!("Could not inspect notebook deletion records: {err}"))?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("Invalid deletion record filename")?;
        let path = entry.path();
        if let Some(legacy) = name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".icloud"))
        {
            if let Some(id) = legacy.strip_suffix(".json") {
                validate_id(id)?;
                hydrate_if_evicted(&directory.join(legacy));
                return Err(format!("Waiting for deletion record {id} to download"));
            }
        }
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        read_record(&path, id)?;
        deleted.insert(id.to_owned());
    }
    Ok(deleted)
}

pub(super) fn record_deleted(bundle: &Path, id: &str) -> Result<(), String> {
    validate_id(id)?;
    if !bundle.is_dir() {
        return Err("Cannot publish a deletion into a missing notebook folder".into());
    }
    let sync = bundle.join("sync");
    let directory = sync.join("deletions");
    for path in [&sync, &directory] {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if !meta.is_dir() => {
                return Err(format!(
                    "Deletion record directory is not a directory: {}",
                    path.display()
                ))
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = std::fs::create_dir(path) {
                    if error.kind() != std::io::ErrorKind::AlreadyExists
                        || !std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
                    {
                        return Err(format!(
                            "Could not create deletion record directory: {error}"
                        ));
                    }
                }
            }
            Err(err) => {
                return Err(format!(
                    "Could not inspect deletion record directory: {err}"
                ))
            }
        }
    }
    let path = directory.join(format!("{id}.json"));
    if path.exists() {
        read_record(&path, id)?;
        for durable in [&path, &directory, &sync, &bundle.to_path_buf()] {
            std::fs::File::open(durable)
                .and_then(|file| file.sync_all())
                .map_err(|err| format!("Could not persist existing deletion record: {err}"))?;
        }
        return Ok(());
    }
    let save = || -> std::io::Result<()> {
        std::fs::File::open(bundle)?.sync_all()?;
        std::fs::File::open(&sync)?.sync_all()?;
        let mut staged = tempfile::NamedTempFile::new_in(&directory)?;
        let record = Deletion {
            version: 1,
            id: id.to_string(),
        };
        staged.write_all(&serde_json::to_vec(&record).map_err(std::io::Error::other)?)?;
        staged.as_file().sync_all()?;
        if let Err(err) = staged.persist_noclobber(&path) {
            if err.error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(err.error);
            }
        }
        std::fs::File::open(&directory)?.sync_all()?;
        Ok(())
    };
    save().map_err(|err| format!("Could not publish deletion record {id}: {err}"))?;
    read_record(&path, id)
}

/// A removed pending import has no final claim for the ordinary writer to
/// inspect. Publish its lifetime before import recovery discards the receipt.
pub(super) async fn publish_pending_deletions(
    state: &AppState,
    bundle: &Path,
    manifest: &OkfManifest,
) -> Result<(), String> {
    for pending in manifest.imports.values() {
        let kind = if pending.entry.path.starts_with("notes/") {
            "note"
        } else if pending.entry.path.starts_with("sources/") {
            "source"
        } else {
            return Err("Invalid path in pending deletion".into());
        };
        if e(state.db.was_deleted(kind, &pending.id))? {
            let absent = if kind == "note" {
                e(state.db.get_note(&pending.id).await)?.is_none()
            } else {
                e(state.db.get_source(&pending.id).await)?.is_none()
            };
            if absent {
                record_deleted(bundle, &pending.entry.portable_id)?;
            }
        }
    }
    Ok(())
}

pub(super) async fn apply_deleted(
    state: &AppState,
    notebook_id: &str,
    bundle: &Path,
    manifest: &mut OkfManifest,
    manifest_at: &Path,
    deleted: &HashSet<String>,
) -> Result<usize, String> {
    for id in deleted {
        validate_id(id)?;
    }
    if deleted
        .iter()
        .any(|id| !manifest.deleted_entities.contains(id))
    {
        manifest.deleted_entities.extend(deleted.iter().cloned());
        save_manifest_checked(manifest_at, manifest)?;
    }
    let targets: Vec<_> = manifest
        .concepts
        .iter()
        .filter(|(_, entry)| manifest.deleted_entities.contains(&entry.portable_id))
        .map(|(id, entry)| (id.clone(), entry.clone()))
        .collect();
    let mut removed = 0;
    for (id, entry) in targets {
        let conflict = conflicts::Context {
            bundle,
            rel: &entry.path,
            base_local_hash: &entry.local_hash,
        };
        let mut done = false;
        // A hot local editor can change the row between preservation and the
        // conditional delete. Preserve its next version and retry, boundedly.
        for _ in 0..3 {
            if entry.path.starts_with("notes/") {
                if let Some(note) = e(state.db.get_note(&id).await)? {
                    if note.notebook_id != notebook_id {
                        return Err("Portable deletion note belongs to another notebook".into());
                    }
                    let edits = load_okf_edits(&app_data_dir(state), notebook_id);
                    conflict.preserve_local_before_delete(&note_concept(&note, &edits))?;
                    recovery::remember_deleted(manifest, &id);
                    save_manifest_checked(manifest_at, manifest)?;
                    done = e(state.db.delete_note_if_unchanged(&note).await)?;
                } else {
                    done = true;
                }
            } else if entry.path.starts_with("sources/") {
                if let Some(mut source) = e(state.db.get_source(&id).await)? {
                    if source.notebook_id != notebook_id {
                        return Err("Portable deletion source belongs to another notebook".into());
                    }
                    crate::device::mark_remote(
                        &app_data_dir(state),
                        notebook_id,
                        std::slice::from_mut(&mut source),
                    );
                    let edits = load_okf_edits(&app_data_dir(state), notebook_id);
                    let local = source_concept(&source, source.content.clone(), bundle, 0, &edits);
                    conflict.preserve_local_before_delete(&local)?;
                    recovery::remember_deleted(manifest, &id);
                    save_manifest_checked(manifest_at, manifest)?;
                    done = e(state.db.delete_source_if_unchanged(&source).await)?;
                } else {
                    done = true;
                }
            } else {
                return Err("Invalid path in portable deletion claim".into());
            }
            if done {
                break;
            }
        }
        if !done {
            return Err("Item kept changing during remote deletion; retry reconciliation".into());
        }
        recovery::remember_deleted(manifest, &id);
        manifest.concepts.remove(&id);
        save_manifest_checked(manifest_at, manifest)?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests;
