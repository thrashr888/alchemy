use super::*;
use crate::okf::sync_tests::Lab;

#[test]
fn deletion_records_are_checked_and_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    assert!(read_deleted(dir.path()).unwrap().is_empty());
    record_deleted(dir.path(), &id).unwrap();
    let path = dir.path().join(format!("sync/deletions/{id}.json"));
    let first = std::fs::read(&path).unwrap();
    record_deleted(dir.path(), &id).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), first);
    assert_eq!(
        read_deleted(dir.path()).unwrap(),
        HashSet::from([id.clone()])
    );
    std::fs::write(&path, "truncated json").unwrap();
    assert!(read_deleted(dir.path()).is_err());
    assert!(record_deleted(dir.path(), &id).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "truncated json");
}

#[test]
fn malformed_identity_version_and_filenames_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    record_deleted(dir.path(), &id).unwrap();
    let path = dir.path().join(format!("sync/deletions/{id}.json"));
    for bad in [
        serde_json::json!({"version":2,"id":id}),
        serde_json::json!({"version":1,"id":uuid::Uuid::new_v4().to_string()}),
        serde_json::json!({"version":1,"id":id,"unexpected":true}),
    ] {
        std::fs::write(&path, bad.to_string()).unwrap();
        assert!(read_deleted(dir.path()).is_err());
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::write(dir.path().join("sync/deletions/not-a-uuid.json"), "{}").unwrap();
    assert!(read_deleted(dir.path()).is_err());
    assert!(record_deleted(dir.path(), "../escape").is_err());
    assert!(record_deleted(&dir.path().join("missing"), &id).is_err());
    assert!(!dir.path().join("missing").exists());
}

async fn fixture() -> (Lab, AppState, PathBuf, OkfManifest, String, Note) {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    let note = Note {
        id: "local-note".into(),
        notebook_id: "shared-notebook".into(),
        title: "Example".into(),
        content: "Acknowledged body.".into(),
        kind: "audio_overview".into(),
        prompt: String::new(),
        origin: String::new(),
        status: String::new(),
        created_at: 1,
        updated_at: 1,
    };
    state.db.add_note(&note).await.unwrap();
    let portable = uuid::Uuid::new_v4().to_string();
    let edits = load_okf_edits(&app_data_dir(&state), "shared-notebook");
    let mut manifest = OkfManifest::default();
    manifest.concepts.insert(
        note.id.clone(),
        OkfManifestEntry {
            portable_id: portable.clone(),
            path: "notes/example.md".into(),
            hash: "observed".into(),
            local_hash: local_concept_hash(&note_concept(&note, &edits)),
            ..Default::default()
        },
    );
    (lab, state, bundle, manifest, portable, note)
}

#[tokio::test]
async fn remote_delete_retains_changed_note_and_remembers_lifetime_after_restart() {
    let (lab, state, bundle, mut manifest, portable, note) = fixture().await;
    state
        .db
        .update_note(&note.id, &note.title, "Local unpublished body.", 2)
        .await
        .unwrap();
    record_deleted(&bundle, &portable).unwrap();
    let at = manifest_path(&app_data_dir(&state), "a");
    assert_eq!(
        apply_deleted(
            &state,
            "shared-notebook",
            &bundle,
            &mut manifest,
            &at,
            &read_deleted(&bundle).unwrap()
        )
        .await
        .unwrap(),
        1
    );
    assert!(state.db.get_note(&note.id).await.unwrap().is_none());
    let copies: Vec<_> = std::fs::read_dir(bundle.join("conflicts"))
        .unwrap()
        .collect();
    assert_eq!(copies.len(), 1);
    assert!(std::fs::read_to_string(copies[0].as_ref().unwrap().path())
        .unwrap()
        .contains("Local unpublished body."));
    assert!(manifest.deleted_entities.contains(&portable));
    drop(state);
    let state = lab.replica("a", &bundle).await;
    let mut manifest = load_manifest_checked(&at).unwrap();
    assert!(manifest.deleted_entities.contains(&portable));
    assert_eq!(
        apply_deleted(
            &state,
            "shared-notebook",
            &bundle,
            &mut manifest,
            &at,
            &HashSet::new()
        )
        .await
        .unwrap(),
        0
    );
    assert!(state.db.was_deleted("note", &note.id).unwrap());
}

#[tokio::test]
async fn unchanged_remote_delete_is_quiet_and_failed_preservation_keeps_edited_row() {
    let (_lab, state, bundle, mut manifest, portable, note) = fixture().await;
    let at = manifest_path(&app_data_dir(&state), "a");
    let deleted = HashSet::from([portable]);
    state
        .db
        .update_note(&note.id, &note.title, "Local unpublished body.", 2)
        .await
        .unwrap();
    std::fs::write(bundle.join("conflicts"), "blocked").unwrap();
    assert!(apply_deleted(
        &state,
        "shared-notebook",
        &bundle,
        &mut manifest,
        &at,
        &deleted
    )
    .await
    .is_err());
    assert!(state.db.get_note(&note.id).await.unwrap().is_some());
    assert!(manifest.concepts.contains_key(&note.id));
    std::fs::remove_file(bundle.join("conflicts")).unwrap();
    state
        .db
        .update_note(&note.id, &note.title, &note.content, note.updated_at)
        .await
        .unwrap();
    assert_eq!(
        apply_deleted(
            &state,
            "shared-notebook",
            &bundle,
            &mut manifest,
            &at,
            &deleted
        )
        .await
        .unwrap(),
        1
    );
    assert!(!bundle.join("conflicts").exists());
}

#[tokio::test]
async fn conditional_delete_does_not_remove_a_changed_note() {
    let (_lab, state, _bundle, _manifest, _portable, note) = fixture().await;
    state
        .db
        .update_note(&note.id, &note.title, "Concurrent edit.", note.updated_at)
        .await
        .unwrap();
    assert!(!state.db.delete_note_if_unchanged(&note).await.unwrap());
    let fresh = state.db.get_note(&note.id).await.unwrap().unwrap();
    assert_eq!(fresh.content, "Concurrent edit.");
    assert!(state.db.delete_note_if_unchanged(&fresh).await.unwrap());
    assert!(state.db.get_note(&note.id).await.unwrap().is_none());
}

#[tokio::test]
async fn pending_deleted_import_publishes_lifetime_before_reservation_recovery() {
    let (_lab, state, bundle, mut manifest, portable, note) = fixture().await;
    let entry = manifest.concepts.remove(&note.id).unwrap();
    manifest.imports.insert(
        entry.path.clone(),
        recovery::PendingImport {
            id: note.id.clone(),
            entry,
        },
    );
    state.db.delete_note(&note.id).await.unwrap();
    publish_pending_deletions(&state, &bundle, &manifest)
        .await
        .unwrap();
    assert!(read_deleted(&bundle).unwrap().contains(&portable));
    let at = manifest_path(&app_data_dir(&state), "a");
    recovery::recover_imports(&state, "shared-notebook", &mut manifest, &at)
        .await
        .unwrap();
    assert!(manifest.imports.is_empty());
    assert!(read_deleted(&bundle).unwrap().contains(&portable));
}

#[tokio::test]
async fn restored_pending_row_is_not_deleted_by_an_old_local_receipt() {
    let (_lab, state, bundle, mut manifest, portable, note) = fixture().await;
    let entry = manifest.concepts.remove(&note.id).unwrap();
    manifest.imports.insert(
        entry.path.clone(),
        recovery::PendingImport {
            id: note.id.clone(),
            entry,
        },
    );
    state.db.delete_note(&note.id).await.unwrap();
    state.db.add_note(&note).await.unwrap();
    publish_pending_deletions(&state, &bundle, &manifest)
        .await
        .unwrap();
    assert!(!read_deleted(&bundle).unwrap().contains(&portable));
}

#[tokio::test]
async fn source_conditional_delete_preserves_concurrent_row_then_full_changed_body() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    let landed = crate::commands::store_new_source(
        &state,
        "shared-notebook",
        ingest::Extracted {
            title: "Example".into(),
            text: "Source base.".into(),
            source_type: "text".into(),
            url: String::new(),
            feeds: Vec::new(),
            image_url: String::new(),
            author: "Original author".into(),
        },
        "",
        0,
        None,
        false,
    )
    .await
    .unwrap();
    let original = state.db.get_source(&landed.id).await.unwrap().unwrap();
    state
        .db
        .add_chunks(
            "shared-notebook",
            &original.id,
            &[("chunk".into(), 0, "Source base.".into())],
            &[vec![0.1; 8]],
        )
        .await
        .unwrap();
    let local_body = "Local changed passage.\n".repeat(300);
    state
        .db
        .replace_source_row(&Source {
            content: local_body.clone(),
            tags: "local-label".into(),
            ..original.clone()
        })
        .await
        .unwrap();
    assert!(!state
        .db
        .delete_source_if_unchanged(&original)
        .await
        .unwrap());
    assert_eq!(
        state
            .db
            .source_chunk_rows(&original.id)
            .await
            .unwrap()
            .len(),
        1
    );
    let portable = uuid::Uuid::new_v4().to_string();
    let mut manifest = OkfManifest::default();
    let edits = load_okf_edits(&app_data_dir(&state), "shared-notebook");
    manifest.concepts.insert(
        original.id.clone(),
        OkfManifestEntry {
            portable_id: portable.clone(),
            path: "sources/example.md".into(),
            hash: "observed".into(),
            local_hash: local_concept_hash(&source_concept(
                &original,
                original.content.clone(),
                &bundle,
                0,
                &edits,
            )),
            ..Default::default()
        },
    );
    let at = manifest_path(&app_data_dir(&state), "a");
    assert_eq!(
        apply_deleted(
            &state,
            "shared-notebook",
            &bundle,
            &mut manifest,
            &at,
            &HashSet::from([portable])
        )
        .await
        .unwrap(),
        1
    );
    assert!(state.db.get_source(&original.id).await.unwrap().is_none());
    assert!(state
        .db
        .source_chunk_rows(&original.id)
        .await
        .unwrap()
        .is_empty());
    let copies: Vec<_> = std::fs::read_dir(bundle.join("conflicts"))
        .unwrap()
        .collect();
    assert_eq!(copies.len(), 1);
    let saved = std::fs::read_to_string(copies[0].as_ref().unwrap().path()).unwrap();
    assert!(saved.contains(&local_body));
    assert!(saved.contains("local-label"));
    assert!(saved.contains("Original author"));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while std::sync::Arc::strong_count(&state.db) > 1 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn simultaneous_identical_deletion_publications_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| record_deleted(dir.path(), &id)))
            .collect();
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
    });
    assert_eq!(read_deleted(dir.path()).unwrap(), HashSet::from([id]));
    assert_eq!(
        std::fs::read_dir(dir.path().join("sync/deletions"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn existing_replica_accepts_new_lifetime_at_deleted_path_and_preserves_local_edit() {
    let (lab, a, bundle, _, _, note) = fixture().await;
    write_bound(&a, "shared-notebook").await.unwrap();
    let b = lab.replica("b", &bundle).await;
    reconcile(&b, "shared-notebook").await.unwrap();
    let old_local = b.db.list_notes("shared-notebook").await.unwrap().remove(0);
    b.db.update_note(
        &old_local.id,
        &old_local.title,
        "Unpublished old lifetime.",
        2,
    )
    .await
    .unwrap();
    let path = bundle.join("notes/example.md");
    let original = std::fs::read_to_string(&path).unwrap();
    let old_portable = portable::explicit_id(&parse_okf_doc(&original))
        .unwrap()
        .unwrap();
    a.db.delete_note(&note.id).await.unwrap();
    write_bound(&a, "shared-notebook").await.unwrap();
    let new_portable = new_id();
    let replacement = original
        .replace(&old_portable, &new_portable)
        .replace("Acknowledged body.", "New lifetime body.");
    std::fs::write(&path, &replacement).unwrap();

    let result = reconcile(&b, "shared-notebook").await.unwrap();
    assert_eq!(result.deleted, 1);
    assert_eq!(result.created, 1);
    let notes = b.db.list_notes("shared-notebook").await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_ne!(notes[0].id, old_local.id);
    assert!(notes[0].content.contains("New lifetime body."));
    let copies: Vec<_> = std::fs::read_dir(bundle.join("conflicts"))
        .unwrap()
        .collect();
    assert_eq!(copies.len(), 1);
    assert!(std::fs::read_to_string(copies[0].as_ref().unwrap().path())
        .unwrap()
        .contains("Unpublished old lifetime."));
    assert!(!reconcile(&b, "shared-notebook").await.unwrap().changed());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), replacement);
}

#[tokio::test]
async fn remote_rename_retains_unpublished_local_edit_and_local_row() {
    let (lab, a, bundle, _, _, _) = fixture().await;
    write_bound(&a, "shared-notebook").await.unwrap();
    let b = lab.replica("b", &bundle).await;
    reconcile(&b, "shared-notebook").await.unwrap();
    let local = b.db.list_notes("shared-notebook").await.unwrap().remove(0);
    b.db.update_note(
        &local.id,
        &local.title,
        "Unpublished edit follows rename.",
        2,
    )
    .await
    .unwrap();
    std::fs::rename(
        bundle.join("notes/example.md"),
        bundle.join("notes/renamed.md"),
    )
    .unwrap();
    let result = reconcile(&b, "shared-notebook").await.unwrap();
    assert_eq!(result.created, 0);
    assert_eq!(result.deleted, 0);
    write_bound(&b, "shared-notebook").await.unwrap();
    let notes = b.db.list_notes("shared-notebook").await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].id, local.id);
    assert!(notes[0]
        .content
        .contains("Unpublished edit follows rename."));
    assert!(!bundle.join("notes/example.md").exists());
    assert!(std::fs::read_to_string(bundle.join("notes/renamed.md"))
        .unwrap()
        .contains("Unpublished edit follows rename."));
    reconcile(&a, "shared-notebook").await.unwrap();
    assert_eq!(a.db.list_notes("shared-notebook").await.unwrap().len(), 1);
}
