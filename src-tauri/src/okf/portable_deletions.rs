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

/// Who wrote a deletion record: `sync/deletions/<uuid>.by`, holding one
/// actor line — `human:<account>` for a person (§5.6's by-line grammar).
///
/// A file beside the record rather than a field inside it. The record is
/// `deny_unknown_fields`, so an extra key would make every Alchemy already
/// installed call it invalid — and an invalid record stops that notebook's
/// whole pass, on the other person's Mac, over a by-line. Older clients skip
/// any name in this folder that is not `<uuid>.json` without looking, so a
/// by-line is invisible to them. A record with none is read as ours, which is
/// what every record written before this one is.
fn actor_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.by"))
}

/// One line, trimmed and capped: this ends up in a sentence in somebody's
/// sidebar, and a file in a shared folder is not a place to trust length.
fn clean_actor(raw: &str) -> String {
    raw.lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(120)
        .collect()
}

/// Every deletion record's by-line in this bundle, by the entity it ends.
pub(super) fn read_actors(bundle: &Path) -> std::collections::HashMap<String, String> {
    let directory = bundle.join("sync/deletions");
    let mut out = std::collections::HashMap::new();
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".by") else {
            continue;
        };
        if validate_id(id).is_err() || is_evicted_stub(&entry.path()) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let actor = clean_actor(&raw);
        if !actor.is_empty() {
            out.insert(id.to_string(), actor);
        }
    }
    out
}

/// Sign a deletion record. Best-effort on purpose: the record is the delete,
/// and a by-line that could not be written is a delete that reads as ours —
/// which is exactly how every record before this one reads.
fn record_actor(directory: &Path, id: &str) {
    let path = actor_path(directory, id);
    if path.exists() {
        return;
    }
    if let Err(err) = std::fs::write(&path, format!("{}\n", okf_human())) {
        crate::note!("okf: could not sign deletion record {id}: {err}");
    }
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
        record_actor(&directory, id);
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
    record_actor(&directory, id);
    read_record(&path, id)
}

/// Hold another person's deletions as proposals (docs/RFC-shared-notebook.md
/// §3), and say who asked.
///
/// Only ever called for a binding the user marked shared. Within one person's
/// Macs a deletion record is authoritative and this never runs; between two
/// people it is a question, because the corpus is not only the deleter's. The
/// row stays, the claim stays (so the writer neither rewrites the file nor
/// reads its absence as our own delete), and the sidebar offers Restore or
/// Remove. Returns whether the manifest changed.
pub(super) fn hold_foreign(
    bundle: &Path,
    manifest: &mut OkfManifest,
    deleted: &mut HashSet<String>,
) -> bool {
    let actors = read_actors(bundle);
    let mut open: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (id, entry) in &manifest.concepts {
        if entry.portable_id.is_empty() || !deleted.contains(&entry.portable_id) {
            continue;
        }
        // A deletion this Mac already agreed to is finished business.
        if manifest.deleted_entities.contains(&entry.portable_id) {
            continue;
        }
        // No by-line is ours: every record written before by-lines existed,
        // and every record this app writes when it cannot sign one.
        let Some(actor) = actors.get(&entry.portable_id) else {
            continue;
        };
        if okf_is_ours(actor) {
            continue;
        }
        open.insert(id.clone(), actor.clone());
    }
    for id in open.keys() {
        if let Some(entry) = manifest.concepts.get(id) {
            deleted.remove(&entry.portable_id);
        }
    }
    for (id, actor) in &open {
        if manifest.proposed_deletions.contains_key(id) {
            continue;
        }
        let path = manifest
            .concepts
            .get(id)
            .map(|entry| entry.path.clone())
            .unwrap_or_default();
        okf_notice(format!(
            "{path} was deleted by {} in this shared folder. It is still here: Restore puts it back for both of you, Remove accepts the deletion.",
            okf_person(actor)
        ));
    }
    if manifest.proposed_deletions == open {
        return false;
    }
    manifest.proposed_deletions = open;
    true
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
