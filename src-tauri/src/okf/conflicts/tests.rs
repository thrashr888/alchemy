use super::*;
use crate::okf::sync_tests::Lab;

async fn note_fixture() -> (Lab, AppState, PathBuf, String) {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    state
        .db
        .add_note(&Note {
            id: "note".into(),
            notebook_id: "shared-notebook".into(),
            title: "Example".into(),
            content: "Base content.".into(),
            kind: "audio_overview".into(),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: 1,
            updated_at: 1,
        })
        .await
        .unwrap();
    write_bound(&state, "shared-notebook").await.unwrap();
    let original = std::fs::read_to_string(bundle.join("notes/example.md")).unwrap();
    (lab, state, bundle, original)
}

fn remote_edit(path: &Path, original: &str, body: &str, mtime: i64) {
    std::fs::write(path, original.replace("Base content.", body)).unwrap();
    std::fs::File::open(path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_millis(mtime as u64),
            ),
        )
        .unwrap();
}

fn copies(bundle: &Path) -> Vec<String> {
    let dir = bundle.join("conflicts");
    if !dir.exists() {
        return vec![];
    }
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect()
}

#[tokio::test]
async fn newer_remote_note_preserves_divergent_local_body_once_across_restart() {
    let (lab, state, bundle, original) = note_fixture().await;
    state
        .db
        .update_note("note", "Example", "Local unpublished body.", 2)
        .await
        .unwrap();
    remote_edit(
        &bundle.join("notes/example.md"),
        &original,
        "Remote winning body.",
        now_ms() + 10_000,
    );
    assert_eq!(
        reconcile(&state, "shared-notebook").await.unwrap().updated,
        1
    );
    assert!(state
        .db
        .get_note("note")
        .await
        .unwrap()
        .unwrap()
        .content
        .contains("Remote winning body."));
    assert_eq!(copies(&bundle).len(), 1);
    assert!(copies(&bundle)[0].contains("Local unpublished body."));
    // The log names the copy and carries none of the text (§5.4).
    let log = std::fs::read_to_string(bundle.join("log.md")).unwrap();
    assert!(log.contains("Kept the losing local version of notes/example.md in conflicts/"));
    assert!(!log.contains("Local unpublished body."));
    drop(state);
    let state = lab.replica("a", &bundle).await;
    assert!(!reconcile(&state, "shared-notebook")
        .await
        .unwrap()
        .changed());
    assert_eq!(copies(&bundle).len(), 1);
    assert_eq!(
        state.db.list_notes("shared-notebook").await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn sequential_remote_edits_do_not_create_conflict_copies() {
    let (_lab, state, bundle, original) = note_fixture().await;
    remote_edit(
        &bundle.join("notes/example.md"),
        &original,
        "Remote sequential body.",
        now_ms() + 10_000,
    );
    assert_eq!(
        reconcile(&state, "shared-notebook").await.unwrap().updated,
        1
    );
    assert!(copies(&bundle).is_empty());
    let latest = std::fs::read_to_string(bundle.join("notes/example.md")).unwrap();
    std::fs::write(
        bundle.join("notes/example.md"),
        latest.replace("Remote sequential body.", "Next sequential body."),
    )
    .unwrap();
    std::fs::File::open(bundle.join("notes/example.md"))
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(
            std::time::UNIX_EPOCH + std::time::Duration::from_millis((now_ms() + 20_000) as u64),
        ))
        .unwrap();
    assert_eq!(
        reconcile(&state, "shared-notebook").await.unwrap().updated,
        1
    );
    assert!(copies(&bundle).is_empty());
}

#[tokio::test]
async fn failed_conflict_copy_stops_newer_remote_replacement() {
    let (_lab, state, bundle, original) = note_fixture().await;
    state
        .db
        .update_note("note", "Example", "Local unpublished body.", 2)
        .await
        .unwrap();
    remote_edit(
        &bundle.join("notes/example.md"),
        &original,
        "Remote winning body.",
        now_ms() + 10_000,
    );
    std::fs::write(bundle.join("conflicts"), "blocked directory").unwrap();
    assert!(reconcile(&state, "shared-notebook")
        .await
        .unwrap_err()
        .contains("Could not preserve"));
    assert_eq!(
        state.db.get_note("note").await.unwrap().unwrap().content,
        "Local unpublished body."
    );
    assert!(std::fs::read_to_string(bundle.join("notes/example.md"))
        .unwrap()
        .contains("Remote winning body."));
    std::fs::remove_file(bundle.join("conflicts")).unwrap();
    assert_eq!(
        reconcile(&state, "shared-notebook").await.unwrap().updated,
        1
    );
    assert_eq!(copies(&bundle).len(), 1);
}

#[tokio::test]
async fn failed_log_stops_remote_loser_rewrite_and_retry_is_idempotent() {
    let (_lab, state, bundle, original) = note_fixture().await;
    state
        .db
        .update_note("note", "Example", "Newer local body.", now_ms() + 60_000)
        .await
        .unwrap();
    remote_edit(
        &bundle.join("notes/example.md"),
        &original,
        "Older remote body.",
        now_ms(),
    );
    std::fs::remove_file(bundle.join("log.md")).unwrap();
    std::fs::create_dir(bundle.join("log.md")).unwrap();
    assert!(write_bound(&state, "shared-notebook")
        .await
        .unwrap_err()
        .contains("Could not read conflict log"));
    assert!(std::fs::read_to_string(bundle.join("notes/example.md"))
        .unwrap()
        .contains("Older remote body."));
    assert_eq!(copies(&bundle).len(), 1);
    std::fs::remove_dir(bundle.join("log.md")).unwrap();
    // Two readbacks before the writer repairs the file must not duplicate
    // its recovery artifact or log body.
    assert_eq!(
        reconcile(&state, "shared-notebook")
            .await
            .unwrap()
            .overruled,
        1
    );
    assert_eq!(
        reconcile(&state, "shared-notebook")
            .await
            .unwrap()
            .overruled,
        1
    );
    assert_eq!(copies(&bundle).len(), 1);
    let log = std::fs::read_to_string(bundle.join("log.md")).unwrap();
    assert_eq!(log.matches("in conflicts/").count(), 1);
    // The log names the copy; it never carries the text (§5.4).
    assert!(!log.contains("Older remote body."));
    let before_retry = log;
    reconcile(&state, "shared-notebook").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(bundle.join("log.md")).unwrap(),
        before_retry
    );
    write_bound(&state, "shared-notebook").await.unwrap();
    assert!(std::fs::read_to_string(bundle.join("notes/example.md"))
        .unwrap()
        .contains("Newer local body."));
}

#[tokio::test]
async fn newer_remote_source_preserves_full_divergent_local_content() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    let source = crate::commands::store_new_source(
        &state,
        "shared-notebook",
        ingest::Extracted {
            title: "Example".into(),
            text: "Base content.".into(),
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
    write_bound(&state, "shared-notebook").await.unwrap();
    let path = bundle.join("sources/example.md");
    let original = std::fs::read_to_string(&path).unwrap();
    let local_body = format!(
        "Local source beginning.\n{}\nLocal source ending.",
        "full passage\n".repeat(500)
    );
    state
        .db
        .replace_source_row(&Source {
            content: local_body.clone(),
            char_count: local_body.chars().count() as i64,
            ..source.clone()
        })
        .await
        .unwrap();
    remote_edit(
        &path,
        &original,
        "Remote winning source.",
        now_ms() + 10_000,
    );
    assert_eq!(
        reconcile(&state, "shared-notebook").await.unwrap().updated,
        1
    );
    assert_eq!(copies(&bundle).len(), 1);
    assert!(copies(&bundle)[0].contains(&local_body));
    assert!(state
        .db
        .get_source(&source.id)
        .await
        .unwrap()
        .unwrap()
        .content
        .contains("Remote winning source."));
    wait_for_workers(&state).await;
}

#[tokio::test]
async fn stale_note_snapshot_cannot_overwrite_concurrent_edit_or_restore_deleted_row() {
    let (_lab, state, _bundle, _original) = note_fixture().await;
    let expected = state.db.get_note("note").await.unwrap().unwrap();
    let incoming = Note {
        content: "Incoming sync body.".into(),
        ..expected.clone()
    };
    state
        .db
        .update_note(
            "note",
            &expected.title,
            "Concurrent local body.",
            expected.updated_at,
        )
        .await
        .unwrap();
    assert!(!state
        .db
        .update_note_if_unchanged(&expected, &incoming)
        .await
        .unwrap());
    assert_eq!(
        state.db.get_note("note").await.unwrap().unwrap().content,
        "Concurrent local body."
    );
    state.db.delete_note("note").await.unwrap();
    assert!(!state
        .db
        .update_note_if_unchanged(&expected, &incoming)
        .await
        .unwrap());
    assert!(state.db.get_note("note").await.unwrap().is_none());
}

#[tokio::test]
async fn source_sync_reingest_rejects_concurrent_metadata_and_content_changes() {
    let lab = Lab::new();
    let state = lab.replica("a", &lab.0.join("bundle")).await;
    let initial = crate::commands::store_new_source(
        &state,
        "shared-notebook",
        ingest::Extracted {
            title: "Example".into(),
            text: "Base content.".into(),
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
    let expected = state.db.get_source(&initial.id).await.unwrap().unwrap();
    state
        .db
        .set_source_tags(&initial.id, "concurrent-label")
        .await
        .unwrap();
    let incoming = || ingest::Extracted {
        title: "Remote title".into(),
        text: "Incoming body.".into(),
        source_type: "text".into(),
        url: String::new(),
        feeds: Vec::new(),
        image_url: String::new(),
        author: String::new(),
    };
    let error = crate::commands::reingest_if_unchanged(&state, &expected, incoming(), None, true)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Source changed during sync"));
    let fresh = state.db.get_source(&initial.id).await.unwrap().unwrap();
    assert_eq!(fresh.tags, "concurrent-label");
    assert_eq!(fresh.content, "Base content.");
    state
        .db
        .replace_source_row(&Source {
            content: "Concurrent source body.".into(),
            ..fresh.clone()
        })
        .await
        .unwrap();
    assert!(
        crate::commands::reingest_if_unchanged(&state, &fresh, incoming(), None, true)
            .await
            .is_err()
    );
    assert_eq!(
        state
            .db
            .get_source(&initial.id)
            .await
            .unwrap()
            .unwrap()
            .content,
        "Concurrent source body."
    );
    state.db.delete_source(&initial.id).await.unwrap();
    assert!(
        crate::commands::reingest_if_unchanged(&state, &fresh, incoming(), None, true)
            .await
            .is_err()
    );
    assert!(state.db.get_source(&initial.id).await.unwrap().is_none());
    wait_for_workers(&state).await;
}

async fn wait_for_workers(state: &AppState) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while std::sync::Arc::strong_count(&state.db) > 1 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background fixture workers did not finish");
}
