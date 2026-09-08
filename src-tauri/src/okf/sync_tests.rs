//! Isolated replicas exercise the real database, writer and reconciler.
//! No app handle, iCloud account, installed model or personal data is used.
use super::*;
use std::sync::{Arc, Mutex};

pub(super) struct Lab(pub(super) PathBuf);

impl Lab {
    pub(super) fn new() -> Self {
        let root = std::env::temp_dir().join(format!("alchemy-sync-{}", new_id()));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    pub(super) async fn replica(&self, name: &str, bundle: &Path) -> AppState {
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
async fn replay_of_a_deleted_note_stays_deleted_after_restart() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    seed_notes(&a).await;
    let path = bundle.join("notes/note-0.md");
    let old = std::fs::read(&path).unwrap();
    a.db.delete_note("note-0").await.unwrap();
    write_bound(&a, "shared-notebook").await.unwrap();
    drop(a);
    let a = lab.replica("a", &bundle).await;
    std::fs::write(&path, &old).unwrap();
    assert!(!reconcile(&a, "shared-notebook").await.unwrap().changed());
    assert_eq!(a.db.list_notes("shared-notebook").await.unwrap().len(), 4);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        old,
        "replayed bytes stay recoverable on disk"
    );
}

#[tokio::test]
async fn missing_established_manifest_stops_before_importing() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    seed_notes(&a).await;
    std::fs::remove_file(manifest_path(&app_data_dir(&a), "a")).unwrap();
    assert!(reconcile(&a, "shared-notebook").await.is_err());
    assert_eq!(a.db.list_notes("shared-notebook").await.unwrap().len(), 5);
}

#[tokio::test]
async fn interrupted_note_import_recovers_reserved_row_after_restart() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    let b = lab.replica("b", &bundle).await;
    seed_notes(&a).await;
    std::fs::write(app_data_dir(&b).join("test-interrupt-import"), "").unwrap();
    assert!(reconcile(&b, "shared-notebook")
        .await
        .unwrap_err()
        .contains("test interruption"));
    let manifest = load_manifest_checked(&manifest_path(&app_data_dir(&b), "b")).unwrap();
    assert_eq!(manifest.imports.len(), 1);
    let pending = manifest.imports.values().next().unwrap();
    let id = pending.id.clone();
    assert!(b.db.get_note(&id).await.unwrap().is_some());
    // A user edit after the interruption must not be acknowledged against the
    // old file bytes when the claim is recovered.
    b.db.update_note(&id, "Local edit", "Survives recovery", now_ms() + 60_000)
        .await
        .unwrap();
    drop(b);
    let b = lab.replica("b", &bundle).await;
    assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().created, 4);
    assert_eq!(b.db.list_notes("shared-notebook").await.unwrap().len(), 5);
    write_bound(&b, "shared-notebook").await.unwrap();
    assert_eq!(
        b.db.get_note(&id).await.unwrap().unwrap().content,
        "Survives recovery"
    );
    assert!(!reconcile(&b, "shared-notebook").await.unwrap().changed());
    assert!(
        load_manifest_checked(&manifest_path(&app_data_dir(&b), "b"))
            .unwrap()
            .imports
            .is_empty()
    );
}

#[tokio::test]
async fn interrupted_source_import_recovers_reserved_row_after_restart() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    std::fs::create_dir_all(bundle.join("sources")).unwrap();
    std::fs::write(
        bundle.join("sources/imported.md"),
        "---\ntitle: Imported source\nalchemy:\n  tags: research,keep\n  device: Other laptop\n---\nA source arriving from the other laptop.\n",
    )
    .unwrap();
    std::fs::write(app_data_dir(&a).join("test-interrupt-source-insert"), "").unwrap();
    assert!(reconcile(&a, "shared-notebook")
        .await
        .unwrap_err()
        .contains("test interruption"));
    let manifest = load_manifest_checked(&manifest_path(&app_data_dir(&a), "a")).unwrap();
    let id = manifest.imports.values().next().unwrap().id.clone();
    assert_eq!(a.db.list_sources("shared-notebook").await.unwrap().len(), 1);
    drop(a);
    let a = lab.replica("a", &bundle).await;
    assert!(!reconcile(&a, "shared-notebook").await.unwrap().changed());
    assert_eq!(a.db.list_sources("shared-notebook").await.unwrap().len(), 1);
    assert_eq!(
        a.db.get_source(&id).await.unwrap().unwrap().tags,
        "research,keep"
    );
    assert_eq!(
        crate::device::load_origin_devices(&app_data_dir(&a), "shared-notebook")
            .get(&id)
            .map(String::as_str),
        Some("Other laptop")
    );
    write_bound(&a, "shared-notebook").await.unwrap();
    let written = std::fs::read_to_string(bundle.join("sources/imported.md")).unwrap();
    assert!(written.contains("research,keep"));
    assert!(written.contains("Other laptop"));
    assert!(!reconcile(&a, "shared-notebook").await.unwrap().changed());
}

#[tokio::test]
async fn reserved_import_without_a_row_retries_the_same_identity() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    std::fs::create_dir_all(bundle.join("notes")).unwrap();
    let path = bundle.join("notes/pending.md");
    let text = "---\ntitle: Pending\nalchemy:\n  kind: audio_overview\n---\nPending content\n";
    std::fs::write(&path, text).unwrap();
    let manifest_at = manifest_path(&app_data_dir(&a), "a");
    let mut manifest = OkfManifest::default();
    let id = recovery::reserve_import(
        &mut manifest,
        &manifest_at,
        "notes/pending.md",
        &okf_hash(text),
        file_clock(&path),
        &parse_okf_doc(text),
    )
    .unwrap();
    drop(a);
    let a = lab.replica("a", &bundle).await;
    assert_eq!(reconcile(&a, "shared-notebook").await.unwrap().created, 1);
    assert_eq!(a.db.list_notes("shared-notebook").await.unwrap().len(), 1);
    assert!(a.db.get_note(&id).await.unwrap().is_some());
}

#[tokio::test]
async fn old_deleted_versions_cannot_overwrite_a_new_item_at_the_same_path() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    seed_notes(&a).await;
    let path = bundle.join("notes/note-0.md");
    let first = std::fs::read_to_string(&path).unwrap();
    let second = first.replace("Original content 0", "Second observed version");
    std::fs::write(&path, &second).unwrap();
    assert_eq!(reconcile(&a, "shared-notebook").await.unwrap().updated, 1);
    a.db.delete_note("note-0").await.unwrap();
    write_bound(&a, "shared-notebook").await.unwrap();
    for old in [&first, &second] {
        std::fs::write(&path, old).unwrap();
        assert!(!reconcile(&a, "shared-notebook").await.unwrap().changed());
        assert_eq!(a.db.list_notes("shared-notebook").await.unwrap().len(), 4);
    }
    let old_identity = portable::explicit_id(&parse_okf_doc(&first))
        .unwrap()
        .unwrap();
    let fresh = first
        .replace("Original content 0", "A new item reuses this path")
        .replace(&old_identity, &new_id());
    std::fs::write(&path, &fresh).unwrap();
    assert_eq!(reconcile(&a, "shared-notebook").await.unwrap().created, 1);
    for old in [&first, &second] {
        std::fs::write(&path, old).unwrap();
        assert!(!reconcile(&a, "shared-notebook").await.unwrap().changed());
        let notes = a.db.list_notes("shared-notebook").await.unwrap();
        assert_eq!(notes.len(), 5);
        assert!(notes
            .iter()
            .any(|note| note.content.trim() == "A new item reuses this path"));
        assert_eq!(write_bound(&a, "shared-notebook").await.unwrap().written, 1);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("A new item reuses this path"));
        assert!(!reconcile(&a, "shared-notebook").await.unwrap().changed());
    }
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
    let deleted_bytes = std::fs::read(bundle.join("notes/note-0.md")).unwrap();
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
    // The explicit shared deletion was already applied by write_bound above;
    // only unmarked filesystem disappearances wait for the grace period.
    assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().deleted, 0);
    write_bound(&b, "shared-notebook").await.unwrap();
    assert_eq!(b.db.list_notes("shared-notebook").await.unwrap().len(), 4);
    assert!(!reconcile(&b, "shared-notebook").await.unwrap().changed());
    // Both the deleting replica and the one that observed the deletion must
    // reject a transport replay of this exact older version.
    std::fs::write(bundle.join("notes/note-0.md"), deleted_bytes).unwrap();
    assert!(!reconcile(&a, "shared-notebook").await.unwrap().changed());
    assert!(!reconcile(&b, "shared-notebook").await.unwrap().changed());
    assert_eq!(b.db.list_notes("shared-notebook").await.unwrap().len(), 4);
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

#[tokio::test]
async fn stale_manifest_hash_with_matching_clock_does_not_refuse_the_write() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    seed_notes(&a).await;
    // A manifest whose hash predates a clock-only observation of the same
    // bytes (what a heal pass left behind): the file is exactly what the
    // sweep last accepted, but the hash on record is not its hash.
    let at = manifest_path(&app_data_dir(&a), "a");
    let mut manifest = load_manifest(&at);
    let entry = manifest.concepts.get_mut("note-0").unwrap();
    let (mtime, len) = file_clock(&bundle.join(&entry.path));
    assert_eq!((entry.file_mtime, entry.file_len), (mtime, len));
    entry.hash = "0000000000000000".into();
    entry.portable_written = false;
    save_manifest_checked(&at, &manifest).unwrap();
    a.db.update_note("note-0", "Note 0", "Edited locally", now_ms())
        .await
        .unwrap();
    write_bound(&a, "shared-notebook").await.unwrap();
    let text = std::fs::read_to_string(bundle.join("notes/note-0.md")).unwrap();
    assert!(text.ends_with("Edited locally\n"), "{text}");
    assert!(load_manifest(&at).concepts["note-0"].portable_written);
}

#[tokio::test]
async fn one_refused_file_does_not_hold_the_rest_of_the_pass_hostage() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    seed_notes(&a).await;
    // Another Mac's bytes land on note-0 after reconciliation looked; the
    // local row for note-0 changes too, so the writer would project over it.
    let touched = bundle.join("notes/note-0.md");
    let remote = std::fs::read_to_string(&touched)
        .unwrap()
        .replace("Original content 0", "Incoming remote content, longer");
    std::fs::write(&touched, &remote).unwrap();
    a.db.update_note("note-0", "Note 0", "Local edit that must wait", now_ms())
        .await
        .unwrap();
    a.db.update_note("note-1", "Note 1", "Local edit that must land", now_ms())
        .await
        .unwrap();
    a.db.delete_note("note-2").await.unwrap();
    // The writer alone, as the sweep would run it after reconciliation had
    // already looked: the remote bytes arrived after that look.
    let (notebook, sources, notes) = gather_bundle_for(&a, "shared-notebook", &bundle)
        .await
        .unwrap();
    let at = manifest_path(&app_data_dir(&a), "a");
    let err = write_bundle(&notebook, &sources, &notes, &bundle, Some(&at)).unwrap_err();
    assert!(
        err.contains("notes/note-0.md changed after reconciliation"),
        "{err}"
    );
    // The refused file keeps the other Mac's bytes and its claim...
    assert_eq!(std::fs::read_to_string(&touched).unwrap(), remote);
    let manifest = load_manifest(&at);
    assert_eq!(manifest.concepts["note-0"].path, "notes/note-0.md");
    // ...while the edit next to it landed, the deletion published, and the
    // listing still names every note the notebook has.
    let landed = std::fs::read_to_string(bundle.join("notes/note-1.md")).unwrap();
    assert!(landed.ends_with("Local edit that must land\n"), "{landed}");
    assert!(!bundle.join("notes/note-2.md").exists());
    assert_eq!(manifest.deleted_entities.len(), 1);
    assert!(!portable_deletions::read_deleted(&bundle)
        .unwrap()
        .is_empty());
    let index = std::fs::read_to_string(bundle.join("notes/index.md")).unwrap();
    assert!(index.contains("note-0.md") && index.contains("note-1.md"));
    assert!(!index.contains("note-2.md"));
}

#[tokio::test]
async fn new_concept_takes_the_next_free_slug_past_an_unclaimed_file() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    seed_notes(&a).await;
    // Another Mac's file sits where "Note 5" would slug to, and a row this
    // pass no longer has still owns note-1.md until cleanup takes it.
    let stranger = "---\ntype: Note\ntitle: Stranger\n---\n\nNot ours\n";
    std::fs::write(bundle.join("notes/note-5.md"), stranger).unwrap();
    a.db.delete_note("note-1").await.unwrap();
    for (id, title) in [("note-5", "Note 5"), ("note-1-again", "Note 1")] {
        a.db.add_note(&Note {
            id: id.into(),
            notebook_id: "shared-notebook".into(),
            title: title.into(),
            content: format!("Body of {id}"),
            kind: "audio_overview".into(),
            prompt: String::new(),
            origin: "human:test".into(),
            status: String::new(),
            created_at: 2,
            updated_at: 2,
        })
        .await
        .unwrap();
    }
    let (notebook, sources, notes) = gather_bundle_for(&a, "shared-notebook", &bundle)
        .await
        .unwrap();
    let at = manifest_path(&app_data_dir(&a), "a");
    let out = write_bundle(&notebook, &sources, &notes, &bundle, Some(&at)).unwrap();
    assert_eq!(out.written, 2, "{out:?}");
    assert_eq!(
        std::fs::read_to_string(bundle.join("notes/note-5.md")).unwrap(),
        stranger
    );
    let manifest = load_manifest(&at);
    assert_eq!(manifest.concepts["note-5"].path, "notes/note-5-2.md");
    assert_eq!(manifest.concepts["note-1-again"].path, "notes/note-1-2.md");
    // The departed row's file went with it; the newcomer never sat on it.
    assert!(!bundle.join("notes/note-1.md").exists());
    assert!(bundle.join("notes/note-1-2.md").exists());
    assert!(!manifest.concepts.contains_key("note-1"));
}

#[tokio::test]
async fn retitled_concept_moves_past_an_occupied_slug_and_leaves_it_alone() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    seed_notes(&a).await;
    let stranger = "---\ntype: Note\ntitle: Stranger\n---\n\nNot ours\n";
    std::fs::write(bundle.join("notes/renamed.md"), stranger).unwrap();
    a.db.update_note("note-0", "Renamed", "Original content 0", now_ms())
        .await
        .unwrap();
    let (notebook, sources, notes) = gather_bundle_for(&a, "shared-notebook", &bundle)
        .await
        .unwrap();
    let at = manifest_path(&app_data_dir(&a), "a");
    let out = write_bundle(&notebook, &sources, &notes, &bundle, Some(&at)).unwrap();
    assert_eq!(out.moved, 1, "{out:?}");
    assert_eq!(
        std::fs::read_to_string(bundle.join("notes/renamed.md")).unwrap(),
        stranger
    );
    assert_eq!(
        load_manifest(&at).concepts["note-0"].path,
        "notes/renamed-2.md"
    );
    assert!(!bundle.join("notes/note-0.md").exists());
    let text = std::fs::read_to_string(bundle.join("notes/renamed-2.md")).unwrap();
    assert!(text.contains("Renamed"), "{text}");
}
