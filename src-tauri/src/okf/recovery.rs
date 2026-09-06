//! Local write-ahead identities and observed-version deletion history.
//! These records stay outside the shared bundle and never duplicate its text.
use super::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct PendingImport {
    pub id: String,
    pub entry: OkfManifestEntry,
}

pub(super) fn reserve_import(
    manifest: &mut OkfManifest,
    manifest_at: &Path,
    rel: &str,
    hash: &str,
    clock: (i64, u64),
    doc: &OkfDoc,
) -> Result<String, String> {
    let id = manifest
        .imports
        .get(rel)
        .map(|p| p.id.clone())
        .unwrap_or_else(new_id);
    let mut claim = OkfManifest::default();
    adopt(&mut claim, &id, rel, hash, clock.0, clock.1, doc);
    let entry = claim.concepts.remove(&id).expect("adopt creates its claim");
    manifest.imports.insert(
        rel.to_string(),
        PendingImport {
            id: id.clone(),
            entry,
        },
    );
    save_manifest_checked(manifest_at, manifest)?;
    Ok(id)
}

/// Recover only the reserved row. Content matching would collapse deliberately
/// distinct notes, and overwriting that row could discard a later local edit.
pub(super) async fn recover_imports(
    state: &AppState,
    notebook_id: &str,
    manifest: &mut OkfManifest,
    manifest_at: &Path,
) -> Result<(), String> {
    let mut recovered = Vec::new();
    for (rel, pending) in &manifest.imports {
        let owner = if rel.starts_with("notes/") {
            e(state.db.get_note(&pending.id).await)?.map(|note| note.notebook_id)
        } else if rel.starts_with("sources/") {
            e(state.db.get_source(&pending.id).await)?.map(|source| source.notebook_id)
        } else {
            return Err("Invalid path in pending notebook import".into());
        };
        if let Some(owner) = owner {
            if owner != notebook_id || pending.entry.path != *rel {
                return Err("Pending notebook import does not match its reserved item".into());
            }
            let mut entry = pending.entry.clone();
            // The existing row may have been edited since insertion. Do not
            // acknowledge its current version against old incoming bytes.
            entry.local_hash.clear();
            manifest.concepts.insert(pending.id.clone(), entry);
            recovered.push(rel.clone());
        }
    }
    if !recovered.is_empty() {
        for rel in recovered {
            manifest.imports.remove(&rel);
        }
        save_manifest_checked(manifest_at, manifest)?;
    }
    Ok(())
}

pub(super) fn remember_deleted(manifest: &mut OkfManifest, id: &str) {
    if let Some(entry) = manifest.concepts.get(id) {
        let hashes = manifest.tombstones.entry(entry.path.clone()).or_default();
        hashes.extend(entry.seen_hashes.iter().cloned());
        if !entry.hash.is_empty() {
            hashes.insert(entry.hash.clone());
        }
    }
}

pub(super) fn is_deleted_replay(manifest: &OkfManifest, rel: &str, hash: &str) -> bool {
    manifest
        .tombstones
        .get(rel)
        .is_some_and(|hashes| hashes.contains(hash))
}

/// An isolated on-disk fault point lets restart tests interrupt the real
/// transaction boundary without global flags affecting parallel tests.
#[cfg(test)]
pub(super) fn interrupt_import(state: &AppState) -> Result<(), String> {
    let flag = app_data_dir(state).join("test-interrupt-import");
    if flag.is_file() {
        std::fs::remove_file(flag).map_err(|err| err.to_string())?;
        return Err("test interruption after import insertion".into());
    }
    Ok(())
}
