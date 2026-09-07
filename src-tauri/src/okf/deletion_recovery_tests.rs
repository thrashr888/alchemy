use super::sync_tests::Lab;
use super::*;

#[tokio::test]
async fn deleting_interrupted_imports_never_recreates_their_rows() {
    for (kind, bulk) in [
        ("note", false),
        ("note", true),
        ("source", false),
        ("source", true),
    ] {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let state = lab.replica("a", &bundle).await;
        let subdir = if kind == "note" { "notes" } else { "sources" };
        std::fs::create_dir_all(bundle.join(subdir)).unwrap();
        let path = bundle.join(subdir).join("incoming.md");
        let text =
            "---\ntitle: Incoming\nalchemy:\n  kind: audio_overview\n---\nImported then deleted\n";
        std::fs::write(&path, text).unwrap();
        let flag = if kind == "note" {
            "test-interrupt-import"
        } else {
            "test-interrupt-source-insert"
        };
        std::fs::write(app_data_dir(&state).join(flag), "").unwrap();
        assert!(reconcile(&state, "shared-notebook")
            .await
            .unwrap_err()
            .contains("test interruption"));
        let manifest = load_manifest_checked(&manifest_path(&app_data_dir(&state), "a")).unwrap();
        let id = manifest.imports.values().next().unwrap().id.clone();
        match (kind, bulk) {
            ("note", false) => state.db.delete_note(&id).await.unwrap(),
            ("note", true) => state
                .db
                .delete_notes(std::slice::from_ref(&id))
                .await
                .unwrap(),
            ("source", false) => state.db.delete_source(&id).await.unwrap(),
            ("source", true) => state
                .db
                .delete_sources(std::slice::from_ref(&id), &[])
                .await
                .unwrap(),
            _ => unreachable!(),
        }
        drop(state);
        let state = lab.replica("a", &bundle).await;
        assert!(!reconcile(&state, "shared-notebook")
            .await
            .unwrap()
            .changed());
        assert!(state
            .db
            .list_notes("shared-notebook")
            .await
            .unwrap()
            .is_empty());
        assert!(state
            .db
            .list_sources("shared-notebook")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}

#[tokio::test]
async fn explicitly_restored_note_survives_its_old_deletion_receipt() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    std::fs::create_dir_all(bundle.join("notes")).unwrap();
    std::fs::write(
        bundle.join("notes/undo.md"),
        "---\ntitle: Undo\nalchemy:\n  kind: audio_overview\n---\nKeep this restored note\n",
    )
    .unwrap();
    std::fs::write(app_data_dir(&state).join("test-interrupt-import"), "").unwrap();
    assert!(reconcile(&state, "shared-notebook").await.is_err());
    let note = state
        .db
        .list_notes("shared-notebook")
        .await
        .unwrap()
        .pop()
        .unwrap();
    state.db.delete_note(&note.id).await.unwrap();
    state.db.add_note(&note).await.unwrap();
    assert!(!reconcile(&state, "shared-notebook")
        .await
        .unwrap()
        .changed());
    assert_eq!(
        state.db.list_notes("shared-notebook").await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn deletion_receipt_failure_leaves_the_notebook_and_note_intact() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    let note = Note {
        id: "kept".into(),
        notebook_id: "shared-notebook".into(),
        title: "Kept".into(),
        content: "Cannot delete without a receipt".into(),
        kind: "audio_overview".into(),
        prompt: String::new(),
        origin: String::new(),
        status: String::new(),
        created_at: 1,
        updated_at: 1,
    };
    state.db.add_note(&note).await.unwrap();
    std::fs::write(
        app_data_dir(&state).join("db/sync-deletions"),
        "blocks directory",
    )
    .unwrap();
    assert!(state.db.delete_note(&note.id).await.is_err());
    assert!(state.db.delete_notebook("shared-notebook").await.is_err());
    assert!(state.db.get_note(&note.id).await.unwrap().is_some());
    assert_eq!(state.db.list_notebooks().await.unwrap().len(), 1);
}
