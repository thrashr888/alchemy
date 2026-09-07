//! A missing database row is not proof that the user deleted its export.
use super::*;
use std::collections::HashSet;

/// Check the exact snapshot the writer will project, before it mutates any
/// files. Legacy code or an interrupted database operation can remove a row
/// without a deletion receipt; its Markdown may be the only surviving copy.
pub(super) fn preflight(
    state: &AppState,
    sources: &[OkfConcept],
    notes: &[OkfConcept],
    bundle: &Path,
    manifest_at: &Path,
) -> Result<(), String> {
    let manifest = load_manifest_checked(manifest_at)?;
    let live: HashSet<_> = sources.iter().chain(notes).map(|row| &row.id).collect();
    let live_paths: HashSet<_> = manifest
        .concepts
        .iter()
        .filter(|(id, _)| live.contains(id))
        .map(|(_, entry)| &entry.path)
        .collect();
    let mut deleted = None;
    for (id, entry) in &manifest.concepts {
        // The writer discards obsolete aliases of a still-owned path without
        // deleting that path. They do not authorize a portable tombstone.
        if live.contains(id) || live_paths.contains(&entry.path) {
            continue;
        }
        let kind = if entry.path.starts_with("notes/") {
            "note"
        } else if entry.path.starts_with("sources/") {
            "source"
        } else {
            return Err("Invalid concept path in notebook sync record".into());
        };
        if e(state.db.was_deleted(kind, id))?
            || manifest.deleted_entities.contains(&entry.portable_id)
        {
            continue;
        }
        if deleted.is_none() {
            deleted = Some(portable_deletions::read_deleted(bundle)?);
        }
        if deleted
            .as_ref()
            .is_some_and(|ids| ids.contains(&entry.portable_id))
        {
            continue;
        }
        return Err(format!(
            "{} is missing from the database without a recorded deletion. Its file and sync claim were preserved; restore the missing item before syncing this notebook",
            entry.path
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::okf::sync_tests::Lab;
    use sha2::{Digest, Sha256};

    async fn seed(state: &AppState, bundle: &Path) -> (String, String) {
        for (dir, metadata) in [
            ("notes", "  kind: audio_overview\n"),
            ("sources", "  type: text\n"),
        ] {
            std::fs::create_dir_all(bundle.join(dir)).unwrap();
            std::fs::write(
                bundle.join(dir).join("original.md"),
                format!("---\ntitle: Original\nalchemy:\n{metadata}---\nSole exported content\n"),
            )
            .unwrap();
        }
        reconcile(state, "shared-notebook").await.unwrap();
        write_bound(state, "shared-notebook").await.unwrap();
        (
            state.db.list_notes("shared-notebook").await.unwrap()[0]
                .id
                .clone(),
            state.db.list_sources("shared-notebook").await.unwrap()[0]
                .id
                .clone(),
        )
    }

    async fn remove_row(state: &AppState, kind: &str, id: &str, keep_receipt: bool) {
        if kind == "note" {
            state.db.delete_note(id).await.unwrap();
        } else {
            state.db.delete_source(id).await.unwrap();
        }
        if !keep_receipt {
            // Reproduce unexplained row loss from legacy code, without
            // changing production database APIs.
            let key = format!("{kind}\0{id}");
            std::fs::remove_file(
                app_data_dir(state)
                    .join("db/sync-deletions")
                    .join(format!("{:x}.json", Sha256::digest(key.as_bytes()))),
            )
            .unwrap();
            assert!(!state.db.was_deleted(kind, id).unwrap());
        }
    }

    #[tokio::test]
    async fn unexplained_missing_rows_preserve_exports_and_do_not_delete_remote_rows() {
        for kind in ["note", "source"] {
            for external_edit in [false, true] {
                let lab = Lab::new();
                let bundle = lab.0.join("shared");
                let a = lab.replica("a", &bundle).await;
                let (note_id, source_id) = seed(&a, &bundle).await;
                let id = if kind == "note" { note_id } else { source_id };
                let b = lab.replica("b", &bundle).await;
                reconcile(&b, "shared-notebook").await.unwrap();
                remove_row(&a, kind, &id, false).await;
                let at = manifest_path(&app_data_dir(&a), "a");
                let before = load_manifest_checked(&at).unwrap();
                let claim = before.concepts[&id].clone();
                let path = bundle.join(&claim.path);
                if external_edit {
                    let text = std::fs::read_to_string(&path).unwrap();
                    std::fs::write(&path, format!("{text}\nAnother laptop edit\n")).unwrap();
                }
                let bytes = std::fs::read(&path).unwrap();
                let error = write_bound(&a, "shared-notebook").await.unwrap_err();
                assert!(error.contains("without a recorded deletion"), "{error}");
                drop(a);
                let a = lab.replica("a", &bundle).await;
                assert!(write_bound(&a, "shared-notebook").await.is_err());
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                let after = load_manifest_checked(&at).unwrap();
                assert_eq!(after.concepts[&id].path, claim.path);
                assert_eq!(after.concepts[&id].hash, claim.hash);
                assert!(!after.deleted_entities.contains(&claim.portable_id));
                assert!(portable_deletions::read_deleted(&bundle)
                    .unwrap()
                    .is_empty());
                assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().deleted, 0);
                assert_eq!(b.db.list_notes("shared-notebook").await.unwrap().len(), 1);
                assert_eq!(b.db.list_sources("shared-notebook").await.unwrap().len(), 1);
            }
        }
    }

    #[tokio::test]
    async fn intentional_deletion_receipts_still_remove_exports_and_remote_rows() {
        for kind in ["note", "source"] {
            let lab = Lab::new();
            let bundle = lab.0.join("shared");
            let a = lab.replica("a", &bundle).await;
            let (note_id, source_id) = seed(&a, &bundle).await;
            let id = if kind == "note" { note_id } else { source_id };
            let b = lab.replica("b", &bundle).await;
            reconcile(&b, "shared-notebook").await.unwrap();
            let manifest = load_manifest_checked(&manifest_path(&app_data_dir(&a), "a")).unwrap();
            let claim = &manifest.concepts[&id];
            remove_row(&a, kind, &id, true).await;
            assert_eq!(write_bound(&a, "shared-notebook").await.unwrap().removed, 1);
            assert!(!bundle.join(&claim.path).exists());
            assert!(portable_deletions::read_deleted(&bundle)
                .unwrap()
                .contains(&claim.portable_id));
            assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().deleted, 1);
            let remaining = if kind == "note" {
                b.db.list_notes("shared-notebook").await.unwrap().len()
            } else {
                b.db.list_sources("shared-notebook").await.unwrap().len()
            };
            assert_eq!(remaining, 0);
        }
    }

    #[tokio::test]
    async fn nightly_export_preserves_the_last_copy_of_an_unexplained_missing_row() {
        let lab = Lab::new();
        let bundle = lab.0.join("shared");
        let a = lab.replica("a", &bundle).await;
        let (_, source_id) = seed(&a, &bundle).await;
        let nightly = lab.0.join("nightly");
        export_all(&a, &nightly).await.unwrap();
        let path = nightly.join("shared-notebook/sources/original.md");
        let before = std::fs::read(&path).unwrap();
        remove_row(&a, "source", &source_id, false).await;
        assert!(export_all(&a, &nightly)
            .await
            .unwrap_err()
            .contains("without a recorded deletion"));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}
