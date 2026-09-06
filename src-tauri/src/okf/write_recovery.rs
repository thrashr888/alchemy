//! Write-ahead path ownership for first exports and concept renames.
use super::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct PendingWrite {
    pub entry: OkfManifestEntry,
    pub prior: Option<OkfManifestEntry>,
}

/// Run before incoming files are classified. An emitted file already belongs
/// to its local row, even when the final writer checkpoint never happened.
pub(super) async fn recover_writes(
    state: &AppState,
    notebook_id: &str,
    bundle: &Path,
    manifest: &mut OkfManifest,
    manifest_at: &Path,
) -> Result<(), String> {
    if manifest.outgoing.is_empty() {
        return Ok(());
    }
    for (id, pending) in manifest.outgoing.clone() {
        let rel = pending.entry.path.clone();
        let owner = if rel.starts_with("notes/") {
            e(state.db.get_note(&id).await)?.map(|row| row.notebook_id)
        } else if rel.starts_with("sources/") {
            e(state.db.get_source(&id).await)?.map(|row| row.notebook_id)
        } else {
            return Err("Invalid destination in pending notebook write".into());
        };
        if owner.as_deref().is_some_and(|owner| owner != notebook_id) {
            return Err("Pending notebook write belongs to another notebook".into());
        }
        let path = bundle.join(&rel);
        if path.exists() {
            if pending
                .prior
                .as_ref()
                .is_some_and(|prior| prior.path != rel && bundle.join(&prior.path).exists())
            {
                return Err(format!(
                    "Interrupted notebook move has both paths present: {rel}"
                ));
            }
            if is_dataless(&path) || is_evicted_stub(&path) {
                hydrate_if_evicted(&path);
                return Err(format!(
                    "Waiting for interrupted notebook write {rel} to download"
                ));
            }
            let (mtime, len) = file_clock(&path);
            let text = std::fs::read_to_string(&path)
                .map_err(|err| format!("Could not recover notebook write {rel}: {err}"))?;
            let hash = okf_hash(&text);
            let mut entry = pending.entry;
            if hash == entry.hash {
                // Acknowledge the exported snapshot, never the current row:
                // a user edit made after the interruption still needs export.
                entry.file_mtime = mtime;
                entry.file_len = len;
            } else if let Some(prior) = pending.prior.filter(|prior| prior.hash == hash) {
                // The rename committed but writing the new title/body did not.
                // The old snapshot still belongs to this same local row.
                entry = OkfManifestEntry {
                    path: rel.clone(),
                    ..prior
                };
                entry.file_mtime = mtime;
                entry.file_len = len;
            } else {
                // The process may have stopped before emitting anything, and
                // another device may have created this same path. Its bytes
                // are not proof that our reserved row owns that file. Keep
                // both versions rather than turning it into a local update.
                return Err(format!(
                    "Interrupted notebook write has unrecognized content at {rel}; both versions were preserved"
                ));
            }
            manifest.concepts.insert(id.clone(), entry);
        }
        // No destination means emission never committed (or the file vanished
        // before recovery). Keep any original claim, but don't invent a missing
        // new claim that could delete the existing local row after the grace.
        manifest.outgoing.remove(&id);
        save_manifest_checked(manifest_at, manifest)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn interrupt_write(manifest_at: &Path, stage: &str) -> Result<(), String> {
    let flag = manifest_at.with_extension(format!("test-{stage}"));
    if flag.is_file() {
        std::fs::remove_file(flag).map_err(|err| err.to_string())?;
        return Err(format!("test interruption {stage}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::okf::sync_tests::Lab;

    async fn add_note(state: &AppState) {
        state
            .db
            .add_note(&Note {
                id: "local-note".into(),
                notebook_id: "shared-notebook".into(),
                title: "Original".into(),
                content: "Original body".into(),
                kind: "audio_overview".into(),
                prompt: String::new(),
                origin: "human:test".into(),
                status: String::new(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .unwrap();
    }

    fn interrupt(state: &AppState, stage: &str) {
        let manifest = manifest_path(&app_data_dir(state), "a");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(manifest.with_extension(format!("test-{stage}")), "").unwrap();
    }

    #[tokio::test]
    async fn interrupted_first_emission_recovers_original_row_and_later_user_edit() {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let a = lab.replica("a", &bundle).await;
        add_note(&a).await;
        interrupt(&a, "after-emit");
        assert!(write_bound(&a, "shared-notebook").await.is_err());
        assert!(bundle.join("notes/original.md").exists());
        a.db.update_note(
            "local-note",
            "Original",
            "Later user edit",
            now_ms() + 10_000,
        )
        .await
        .unwrap();
        drop(a);
        let a = lab.replica("a", &bundle).await;
        assert_eq!(reconcile(&a, "shared-notebook").await.unwrap().created, 0);
        write_bound(&a, "shared-notebook").await.unwrap();
        assert_eq!(a.db.list_notes("shared-notebook").await.unwrap().len(), 1);
        assert!(std::fs::read_to_string(bundle.join("notes/original.md"))
            .unwrap()
            .contains("Later user edit"));
        assert!(
            load_manifest_checked(&manifest_path(&app_data_dir(&a), "a"))
                .unwrap()
                .outgoing
                .is_empty()
        );
    }

    #[tokio::test]
    async fn interruption_before_emission_never_claims_an_absent_file() {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let a = lab.replica("a", &bundle).await;
        add_note(&a).await;
        interrupt(&a, "before-emit");
        assert!(write_bound(&a, "shared-notebook").await.is_err());
        drop(a);
        let a = lab.replica("a", &bundle).await;
        reconcile(&a, "shared-notebook").await.unwrap();
        let manifest = load_manifest_checked(&manifest_path(&app_data_dir(&a), "a")).unwrap();
        assert!(manifest.concepts.is_empty());
        assert!(manifest.outgoing.is_empty());
        assert!(a.db.get_note("local-note").await.unwrap().is_some());
        write_bound(&a, "shared-notebook").await.unwrap();
        assert!(bundle.join("notes/original.md").exists());
    }

    #[tokio::test]
    async fn interrupted_rename_keeps_one_identity_before_and_after_rewrite() {
        for stage in ["before-emit", "after-rename", "after-emit"] {
            let lab = Lab::new();
            let bundle = lab.0.join("bundle");
            let a = lab.replica("a", &bundle).await;
            add_note(&a).await;
            write_bound(&a, "shared-notebook").await.unwrap();
            let original_bytes = std::fs::read(bundle.join("notes/original.md")).unwrap();
            a.db.update_note("local-note", "Renamed", "New body", now_ms() + 10_000)
                .await
                .unwrap();
            interrupt(&a, stage);
            assert!(write_bound(&a, "shared-notebook").await.is_err());
            drop(a);
            let a = lab.replica("a", &bundle).await;
            assert_eq!(reconcile(&a, "shared-notebook").await.unwrap().created, 0);
            write_bound(&a, "shared-notebook").await.unwrap();
            assert_eq!(a.db.list_notes("shared-notebook").await.unwrap().len(), 1);
            assert!(!bundle.join("notes/original.md").exists());
            assert!(std::fs::read_to_string(bundle.join("notes/renamed.md"))
                .unwrap()
                .contains("New body"));
            std::fs::write(bundle.join("notes/original.md"), original_bytes).unwrap();
            assert_eq!(reconcile(&a, "shared-notebook").await.unwrap().created, 0);
            assert_eq!(a.db.list_notes("shared-notebook").await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn unrelated_file_at_a_reserved_destination_never_replaces_the_local_note() {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let a = lab.replica("a", &bundle).await;
        add_note(&a).await;
        interrupt(&a, "before-emit");
        assert!(write_bound(&a, "shared-notebook").await.is_err());
        let remote = "---\ntitle: Someone else's note\n---\nUnrelated remote content\n";
        std::fs::write(bundle.join("notes/original.md"), remote).unwrap();
        drop(a);
        let a = lab.replica("a", &bundle).await;
        assert!(reconcile(&a, "shared-notebook")
            .await
            .unwrap_err()
            .contains("unrecognized content"));
        assert!(write_bound(&a, "shared-notebook").await.is_err());
        assert_eq!(
            a.db.get_note("local-note").await.unwrap().unwrap().content,
            "Original body"
        );
        assert_eq!(
            std::fs::read_to_string(bundle.join("notes/original.md")).unwrap(),
            remote
        );
        assert_eq!(
            load_manifest_checked(&manifest_path(&app_data_dir(&a), "a"))
                .unwrap()
                .outgoing
                .len(),
            1
        );
    }
}
