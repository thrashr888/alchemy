use super::sync_tests::Lab;
use super::*;

fn binding(id: &str) -> OkfBinding {
    OkfBinding {
        id: id.into(),
        path: format!("/old/{id}"),
        ..Default::default()
    }
}

#[test]
fn migration_merge_preserves_concurrent_unbind_rebind_and_write_time() {
    let dir = tempfile::tempdir().unwrap();
    for id in ["unbound", "rebound", "written"] {
        set_binding_checked(dir.path(), id, Some(binding(id))).unwrap();
    }
    let before = load_bindings_checked(dir.path()).unwrap();
    let mut after = before.clone();
    for value in after.values_mut() {
        value.path = format!("/new/{}", value.id);
    }
    set_binding_checked(dir.path(), "unbound", None).unwrap();
    set_binding_checked(dir.path(), "rebound", Some(binding("replacement"))).unwrap();
    touch_last_write_checked(dir.path(), "written", "written", 99).unwrap();
    set_binding_checked(dir.path(), "other", Some(binding("other"))).unwrap();
    merge_binding_moves(dir.path(), &before, &after).unwrap();
    let current = load_bindings_checked(dir.path()).unwrap();
    assert!(!current.contains_key("unbound"));
    assert_eq!(current["rebound"].id, "replacement");
    assert_eq!(current["written"].path, "/new/written");
    assert_eq!(current["written"].last_write_at, 99);
    assert!(current.contains_key("other"));
}

#[test]
fn stale_recovery_cannot_restore_an_unbound_notebook() {
    let dir = tempfile::tempdir().unwrap();
    let old = binding("old");
    set_binding_checked(dir.path(), "notebook", Some(old.clone())).unwrap();
    set_binding_checked(dir.path(), "notebook", None).unwrap();
    assert!(replace_binding_checked(
        dir.path(),
        "notebook",
        Some(&old),
        Some(binding("replacement"))
    )
    .is_err());
    assert!(binding_for_checked(dir.path(), "notebook")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn corrupt_and_missing_bindings_stop_live_sync_before_database_changes() {
    for missing in [false, true] {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let state = lab.replica("a", &bundle).await;
        std::fs::create_dir_all(bundle.join("notes")).unwrap();
        let bytes = "---\ntitle: Remote\n---\nDo not duplicate this note\n";
        std::fs::write(bundle.join("notes/remote.md"), bytes).unwrap();
        let path = bindings_path(&app_data_dir(&state));
        if missing {
            std::fs::remove_file(&path).unwrap();
        } else {
            std::fs::write(&path, "{broken").unwrap();
        }
        assert!(reconcile(&state, "shared-notebook").await.is_err());
        assert!(write_bound(&state, "shared-notebook").await.is_err());
        assert!(unbind_impl(&state, "shared-notebook").await.is_err());
        assert!(state
            .db
            .list_notes("shared-notebook")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            std::fs::read_to_string(bundle.join("notes/remote.md")).unwrap(),
            bytes
        );
        if missing {
            assert!(!path.exists());
        } else {
            assert_eq!(std::fs::read_to_string(path).unwrap(), "{broken");
        }
    }
}

#[tokio::test]
async fn unbind_waits_for_the_actual_notebook_writer_lock() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    let lock = notebook_sync_lock(&state, "shared-notebook");
    let guard = lock.lock().await;
    let pending = unbind_impl(&state, "shared-notebook");
    tokio::pin!(pending);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending)
            .await
            .is_err()
    );
    assert!(
        binding_for_checked(&app_data_dir(&state), "shared-notebook")
            .unwrap()
            .is_some()
    );
    drop(guard);
    pending.await.unwrap();
    assert!(
        binding_for_checked(&app_data_dir(&state), "shared-notebook")
            .unwrap()
            .is_none()
    );
}
