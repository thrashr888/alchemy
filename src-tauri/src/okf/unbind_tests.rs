use super::sync_tests::Lab;
use super::*;

async fn fixture() -> (Lab, AppState, PathBuf, PathBuf) {
    let lab = Lab::new();
    let root = lab.0.join("notebooks");
    let folder = root.join("research");
    let state = lab.replica("a", &folder).await;
    state
        .db
        .add_note(&Note {
            id: "original-note".into(),
            notebook_id: "shared-notebook".into(),
            title: "Original".into(),
            content: "Keep this original body".into(),
            kind: "audio_overview".into(),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: 1,
            updated_at: 1,
        })
        .await
        .unwrap();
    let source: Source = serde_json::from_value(serde_json::json!({
        "id": "original-source", "notebookId": "shared-notebook",
        "title": "Original source", "sourceType": "text",
        "content": "Keep this source body", "charCount": 21,
        "chunkCount": 0, "createdAt": 1,
    }))
    .unwrap();
    state.db.insert_sources_bulk(&[source]).await.unwrap();
    write_bound(&state, "shared-notebook").await.unwrap();
    (lab, state, root, folder)
}

#[tokio::test]
async fn explicit_unbind_survives_automatic_discovery_and_restart() {
    let (lab, state, root, folder) = fixture().await;
    let note_path = folder.join("notes/original.md");
    let original = std::fs::read(&note_path).unwrap();
    unbind_impl(&state, "shared-notebook").await.unwrap();
    for _ in 0..2 {
        assert_eq!(
            open_found_bundles_inner(None, &state, &root).await.unwrap(),
            0
        );
        assert!(
            binding_for_checked(&app_data_dir(&state), "shared-notebook")
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read(&note_path).unwrap(), original);
        assert_eq!(
            state.db.list_notes("shared-notebook").await.unwrap().len(),
            1
        );
    }
    drop(state);
    let state = lab.replica("a", &folder).await;
    // Lab installs its fixture binding on every open; undo that fixture-only
    // step to preserve the application's real persisted unbound state.
    set_binding_checked(&app_data_dir(&state), "shared-notebook", None).unwrap();
    assert_eq!(
        open_found_bundles_inner(None, &state, &root).await.unwrap(),
        0
    );
    assert!(
        binding_for_checked(&app_data_dir(&state), "shared-notebook")
            .unwrap()
            .is_none()
    );
    // Folder protection also survives a missing or changed declared identity.
    std::fs::write(
        folder.join("index.md"),
        "---\ntitle: Research\n---\n# Research\n",
    )
    .unwrap();
    assert_eq!(
        open_found_bundles_inner(None, &state, &root).await.unwrap(),
        0
    );
    assert_eq!(state.db.list_notebooks().await.unwrap().len(), 1);
    assert_eq!(std::fs::read(&note_path).unwrap(), original);
}

#[tokio::test]
async fn explicit_bind_clears_detach_intent_after_publication() {
    let (_lab, state, root, folder) = fixture().await;
    unbind_impl(&state, "shared-notebook").await.unwrap();
    let target = root.join("chosen");
    bind_import::bind_folder(&state, "shared-notebook", target.to_str().unwrap())
        .await
        .unwrap();
    assert!(
        !bindings::discovery_blocked(&app_data_dir(&state), Some("shared-notebook"), &target)
            .unwrap()
    );
    assert!(bindings::discovery_blocked(&app_data_dir(&state), None, &folder).unwrap());
    assert_eq!(
        state.db.list_notes("shared-notebook").await.unwrap().len(),
        1
    );
    assert!(target.join("notes/original.md").exists());
    assert_eq!(
        open_found_bundles_inner(None, &state, &root).await.unwrap(),
        0
    );
    assert_eq!(
        binding_for_checked(&app_data_dir(&state), "shared-notebook")
            .unwrap()
            .unwrap()
            .path,
        same_folder(&target).to_string_lossy()
    );
}

#[tokio::test]
async fn explicit_resume_same_folder_reuses_original_rows_and_manifest() {
    let (_lab, state, root, folder) = fixture().await;
    let before = binding_for_checked(&app_data_dir(&state), "shared-notebook")
        .unwrap()
        .unwrap();
    let rows = serde_json::to_value(state.db.list_notes("shared-notebook").await.unwrap()).unwrap();
    let sources = serde_json::to_value(
        state
            .db
            .sources_with_content("shared-notebook")
            .await
            .unwrap(),
    )
    .unwrap();
    let text = std::fs::read(folder.join("notes/original.md")).unwrap();
    unbind_impl(&state, "shared-notebook").await.unwrap();
    // Failure before publication must leave the suppression intact.
    std::fs::write(
        app_data_dir(&state).join("test-bind-import-before-binding"),
        "",
    )
    .unwrap();
    assert!(
        bind_import::bind_folder(&state, "shared-notebook", folder.to_str().unwrap())
            .await
            .is_err()
    );
    assert!(
        bindings::discovery_blocked(&app_data_dir(&state), Some("shared-notebook"), &folder)
            .unwrap()
    );
    assert_eq!(
        open_found_bundles_inner(None, &state, &root).await.unwrap(),
        0
    );
    bind_import::bind_folder(&state, "shared-notebook", folder.to_str().unwrap())
        .await
        .unwrap();
    let after = binding_for_checked(&app_data_dir(&state), "shared-notebook")
        .unwrap()
        .unwrap();
    assert_eq!(after.id, before.id);
    assert!(
        !bindings::discovery_blocked(&app_data_dir(&state), Some("shared-notebook"), &folder)
            .unwrap()
    );
    assert_eq!(
        serde_json::to_value(state.db.list_notes("shared-notebook").await.unwrap()).unwrap(),
        rows
    );
    assert_eq!(
        serde_json::to_value(
            state
                .db
                .sources_with_content("shared-notebook")
                .await
                .unwrap()
        )
        .unwrap(),
        sources
    );
    assert_eq!(
        std::fs::read(folder.join("notes/original.md")).unwrap(),
        text
    );
    assert_eq!(
        open_found_bundles_inner(None, &state, &root).await.unwrap(),
        0
    );
}

#[test]
fn stale_discovery_cannot_publish_after_detach_and_corruption_blocks_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("bundle");
    std::fs::create_dir(&folder).unwrap();
    let binding = OkfBinding {
        id: "binding".into(),
        path: folder.to_string_lossy().into(),
        ..Default::default()
    };
    set_binding_checked(dir.path(), "notebook", Some(binding.clone())).unwrap();
    assert!(!bindings::discovery_blocked(dir.path(), Some("notebook"), &folder).unwrap());
    bindings::detach(dir.path(), "notebook").unwrap();
    assert!(bindings::allow_explicit(dir.path(), "notebook", &folder, "binding").is_err());
    assert!(bindings::discovery_blocked(dir.path(), Some("notebook"), &folder).unwrap());
    assert!(
        bindings::update_discovered(dir.path(), "notebook", &folder, |map| {
            map.insert("notebook".into(), binding);
            Ok(())
        })
        .unwrap()
        .is_none()
    );
    assert!(binding_for_checked(dir.path(), "notebook")
        .unwrap()
        .is_none());
    std::fs::write(dir.path().join("okf-detached.json"), "truncated").unwrap();
    assert!(bindings::discovery_blocked(dir.path(), Some("notebook"), &folder).is_err());
    std::fs::remove_file(dir.path().join("okf-detached.json")).unwrap();
    assert!(bindings::discovery_blocked(dir.path(), Some("notebook"), &folder).is_err());
}
