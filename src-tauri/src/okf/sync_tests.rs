//! Isolated replicas exercise the real database, writer and reconciler.
//! No app handle, iCloud account, installed model or personal data is used.
use super::*;
use std::sync::{Arc, Mutex};

struct Lab(PathBuf);

impl Lab {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("alchemy-sync-{}", new_id()));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    async fn replica(&self, name: &str, bundle: &Path) -> AppState {
        let dir = self.0.join(name);
        let db = crate::db::Db::open(&dir.join("db")).await.unwrap();
        if db.list_notebooks().await.unwrap().is_empty() {
            db.create_notebook(&Notebook {
                id: "shared-notebook".into(),
                title: "Shared notebook".into(),
                color: String::new(),
                icon: String::new(),
                created_at: 1,
                updated_at: 1,
                status: String::new(),
                growth_web: false,
                source_count: 0,
                note_count: 0,
                report_count: 0,
            })
            .await
            .unwrap();
        }
        let config = crate::ai::AiConfig {
            base_url: "http://127.0.0.1:1".into(),
            embedder: "ollama".into(),
            ..Default::default()
        };
        let state = AppState {
            db: Arc::new(db),
            ai: tokio::sync::RwLock::new(crate::ai::Ai::new(
                config,
                crate::ai::AiRuntime {
                    data_dir: dir.clone(),
                    ..Default::default()
                },
            )),
            config_path: dir.join("config.json"),
            stats_path: dir.join("stats.json"),
            trace_dir: dir.join("traces"),
            model_stats: Mutex::new(HashMap::new()),
            cancel: Mutex::new(HashMap::new()),
            folder_scan_lock: tokio::sync::Mutex::new(()),
            glass_applied: Mutex::new(HashMap::new()),
            open_notebooks: Mutex::new(HashMap::new()),
            gen_queue: crate::genqueue::GenQueue::load(&dir),
        };
        std::fs::create_dir_all(bundle).unwrap();
        set_binding(
            &dir,
            "shared-notebook",
            Some(OkfBinding {
                path: bundle.to_string_lossy().into(),
                id: name.into(),
                last_write_at: 0,
                lost: false,
            }),
        );
        state
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn seed_notes(state: &AppState) {
    for i in 0..5 {
        state
            .db
            .add_note(&Note {
                id: format!("note-{i}"),
                notebook_id: "shared-notebook".into(),
                title: format!("Note {i}"),
                content: format!("Original content {i}"),
                // This kind deliberately skips retrieval embedding; sync semantics
                // are identical and the harness does not need an inference server.
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
    write_bound(state, "shared-notebook").await.unwrap();
}

#[tokio::test]
async fn repeated_delivery_is_quiet_after_remote_edit() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    let b = lab.replica("b", &bundle).await;
    seed_notes(&a).await;
    assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().created, 5);
    let path = bundle.join("notes/note-0.md");
    let before = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        before.replace("Original content 0", "Remote edit with new content"),
    )
    .unwrap();
    assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().updated, 1);
    assert!(
        !reconcile(&b, "shared-notebook").await.unwrap().changed(),
        "redelivery must not repeat an update"
    );
    assert_eq!(b.db.list_notes("shared-notebook").await.unwrap().len(), 5);
}

#[tokio::test]
async fn overlapping_reconciliation_imports_each_file_once() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    let b = lab.replica("b", &bundle).await;
    seed_notes(&a).await;
    let (first, second) = tokio::join!(
        reconcile(&b, "shared-notebook"),
        reconcile(&b, "shared-notebook")
    );
    assert_eq!(first.unwrap().created + second.unwrap().created, 5);
    assert_eq!(b.db.list_notes("shared-notebook").await.unwrap().len(), 5);
}

#[tokio::test]
async fn unrelated_write_does_not_resurrect_remote_deletion() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    let b = lab.replica("b", &bundle).await;
    seed_notes(&a).await;
    reconcile(&b, "shared-notebook").await.unwrap();
    write_bound(&b, "shared-notebook").await.unwrap();
    a.db.delete_note("note-0").await.unwrap();
    write_bound(&a, "shared-notebook").await.unwrap();
    assert!(!bundle.join("notes/note-0.md").exists());
    write_bound(&b, "shared-notebook").await.unwrap();
    assert!(
        !bundle.join("notes/note-0.md").exists(),
        "a stale replica must not recreate an absent known file"
    );
    reconcile(&b, "shared-notebook").await.unwrap();
    let manifest_at = manifest_path(&app_data_dir(&b), "b");
    let mut manifest = load_manifest(&manifest_at);
    for entry in manifest.concepts.values_mut() {
        if entry.missing_since > 0 {
            entry.missing_since -= OKF_MISSING_GRACE_MS + 1;
        }
    }
    save_manifest(&manifest_at, &manifest);
    assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().deleted, 1);
    write_bound(&b, "shared-notebook").await.unwrap();
    assert_eq!(b.db.list_notes("shared-notebook").await.unwrap().len(), 4);
    assert!(!reconcile(&b, "shared-notebook").await.unwrap().changed());
}

#[tokio::test]
async fn imported_replica_does_not_rewrite_unchanged_notes() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    let b = lab.replica("b", &bundle).await;
    seed_notes(&a).await;
    reconcile(&b, "shared-notebook").await.unwrap();
    for _ in 0..3 {
        assert_eq!(write_bound(&b, "shared-notebook").await.unwrap().written, 0);
        assert!(!reconcile(&a, "shared-notebook").await.unwrap().changed());
        assert_eq!(write_bound(&a, "shared-notebook").await.unwrap().written, 0);
        assert!(!reconcile(&b, "shared-notebook").await.unwrap().changed());
    }
}

#[tokio::test]
async fn delayed_file_delivery_and_restart_keep_one_copy_per_file() {
    let lab = Lab::new();
    let folder_a = lab.0.join("folder-a");
    let folder_b = lab.0.join("folder-b");
    let a = lab.replica("a", &folder_a).await;
    let b = lab.replica("b", &folder_b).await;
    seed_notes(&a).await;
    std::fs::create_dir_all(folder_b.join("notes")).unwrap();
    for i in [4, 1, 3, 0, 2] {
        let rel = format!("notes/note-{i}.md");
        std::fs::copy(folder_a.join(&rel), folder_b.join(&rel)).unwrap();
        assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().created, 1);
        // Deliver identical bytes again as a sync client might after a retry.
        std::fs::copy(folder_a.join(&rel), folder_b.join(&rel)).unwrap();
        assert!(!reconcile(&b, "shared-notebook").await.unwrap().changed());
        assert_eq!(write_bound(&b, "shared-notebook").await.unwrap().written, 0);
    }
    assert_eq!(b.db.list_notes("shared-notebook").await.unwrap().len(), 5);
    drop(b);
    // Open the same database and persisted manifest, with no in-memory state.
    let b = lab.replica("b", &folder_b).await;
    assert!(!reconcile(&b, "shared-notebook").await.unwrap().changed());
    assert_eq!(b.db.list_notes("shared-notebook").await.unwrap().len(), 5);
}

#[tokio::test]
async fn writer_does_not_hide_an_unread_remote_edit() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    seed_notes(&a).await;
    let path = bundle.join("notes/note-0.md");
    let before = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        before.replace("Original content 0", "Remote changed content"),
    )
    .unwrap();
    write_bound(&a, "shared-notebook").await.unwrap();
    reconcile(&a, "shared-notebook").await.unwrap();
    assert_eq!(
        a.db.get_note("note-0")
            .await
            .unwrap()
            .unwrap()
            .content
            .trim(),
        "Remote changed content"
    );
}

#[tokio::test]
async fn source_replication_converges_and_records_each_edit_once() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    let b = lab.replica("b", &bundle).await;
    crate::commands::store_new_source(
        &a,
        "shared-notebook",
        ingest::Extracted {
            title: "Source one".into(),
            text: "The original shared source content.".into(),
            source_type: "text".into(),
            url: String::new(),
            feeds: Vec::new(),
            image_url: String::new(),
            author: String::new(),
        },
        "",
        0,
        None,
        false,
    )
    .await
    .unwrap();
    write_bound(&a, "shared-notebook").await.unwrap();
    assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().created, 1);
    for _ in 0..3 {
        assert_eq!(write_bound(&b, "shared-notebook").await.unwrap().written, 0);
        assert!(!reconcile(&a, "shared-notebook").await.unwrap().changed());
        assert!(!reconcile(&b, "shared-notebook").await.unwrap().changed());
    }
    let path = bundle.join("sources/source-one.md");
    let before = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        before.replace(
            "The original shared source content.",
            "An edited shared source, delivered by the other laptop.",
        ),
    )
    .unwrap();
    assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().updated, 1);
    assert!(!reconcile(&b, "shared-notebook").await.unwrap().changed());
    assert_eq!(write_bound(&b, "shared-notebook").await.unwrap().written, 0);
    assert_eq!(b.db.list_sources("shared-notebook").await.unwrap().len(), 1);
}

#[tokio::test]
async fn unreadable_claim_store_prevents_database_imports() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    let b = lab.replica("b", &bundle).await;
    seed_notes(&a).await;
    let manifest_at = manifest_path(&app_data_dir(&b), "b");
    std::fs::create_dir_all(&manifest_at).unwrap();
    for _ in 0..2 {
        assert!(reconcile(&b, "shared-notebook").await.is_err());
        assert!(b.db.list_notes("shared-notebook").await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn older_remote_edit_is_preserved_in_log_and_overruled_once() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    seed_notes(&a).await;
    a.db.update_note("note-0", "Note 0", "Original content 0", now_ms() + 60_000)
        .await
        .unwrap();
    write_bound(&a, "shared-notebook").await.unwrap();
    let path = bundle.join("notes/note-0.md");
    let local = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        local.replace("Original content 0", "Older conflicting version"),
    )
    .unwrap();
    assert_eq!(write_bound(&a, "shared-notebook").await.unwrap().written, 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), local);
    assert!(std::fs::read_to_string(bundle.join("log.md"))
        .unwrap()
        .contains("Older conflicting version"));
    assert!(!reconcile(&a, "shared-notebook").await.unwrap().changed());
    assert_eq!(write_bound(&a, "shared-notebook").await.unwrap().written, 0);
}
