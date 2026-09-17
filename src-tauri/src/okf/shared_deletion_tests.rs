//! Two people, one folder: what another person's deletion record means
//! (docs/RFC-shared-notebook.md §3). Two data dirs over one bundle, the way
//! `sync_tests` runs every other sync rule — the only thing these add is a
//! binding marked shared and a record signed by somebody who is not us.
use super::sync_tests::Lab;
use super::*;

const NOTEBOOK: &str = "shared-notebook";

/// One note, written to the bundle, and what the manifest recorded for it.
async fn seeded(lab: &Lab, bundle: &Path, shared: bool) -> (AppState, OkfManifestEntry) {
    let state = lab.replica("a", bundle).await;
    state
        .db
        .add_note(&Note {
            id: "note-0".into(),
            notebook_id: NOTEBOOK.into(),
            title: "Shared thought".into(),
            content: "Worth keeping until somebody says otherwise".into(),
            // Skips retrieval embedding; sync semantics are identical and no
            // inference server is needed.
            kind: "audio_overview".into(),
            prompt: String::new(),
            origin: "human:test".into(),
            status: String::new(),
            created_at: 1,
            updated_at: 1,
        })
        .await
        .unwrap();
    write_bound(&state, NOTEBOOK).await.unwrap();
    if shared {
        mark_binding_shared(&app_data_dir(&state), NOTEBOOK).unwrap();
    }
    let manifest = load_manifest_checked(&manifest_path(&app_data_dir(&state), "a")).unwrap();
    let entry = manifest.concepts.get("note-0").cloned().unwrap();
    assert!(bundle.join(&entry.path).is_file());
    (state, entry)
}

/// The other person's Mac: the file goes, and the record carries their
/// by-line rather than ours.
fn deleted_by(bundle: &Path, entry: &OkfManifestEntry, actor: Option<&str>) {
    std::fs::remove_file(bundle.join(&entry.path)).unwrap();
    portable_deletions::record_deleted(bundle, &entry.portable_id).unwrap();
    let by = bundle
        .join("sync/deletions")
        .join(format!("{}.by", entry.portable_id));
    match actor {
        Some(actor) => std::fs::write(&by, format!("{actor}\n")).unwrap(),
        // A record from an Alchemy that predates by-lines: no file at all.
        None => std::fs::remove_file(&by).unwrap(),
    }
}

#[tokio::test]
async fn an_unshared_folder_still_takes_a_deletion_record_as_the_last_word() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let (state, entry) = seeded(&lab, &bundle, false).await;
    deleted_by(&bundle, &entry, Some("human:kim"));
    assert_eq!(reconcile(&state, NOTEBOOK).await.unwrap().deleted, 1);
    assert!(state.db.get_note("note-0").await.unwrap().is_none());
    assert!(deletion_proposals(&state, NOTEBOOK)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_shared_folder_holds_their_deletion_as_a_question_and_restore_answers_it() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let (state, entry) = seeded(&lab, &bundle, true).await;
    deleted_by(&bundle, &entry, Some("human:kim"));

    assert_eq!(reconcile(&state, NOTEBOOK).await.unwrap().deleted, 0);
    assert!(state.db.get_note("note-0").await.unwrap().is_some());
    let waiting = deletion_proposals(&state, NOTEBOOK).await.unwrap();
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    assert_eq!(waiting[0].by, "kim");
    assert_eq!(waiting[0].kind, "note");
    assert_eq!(waiting[0].title, "Shared thought");

    // Neither a write nor a second pass may answer for the person: the file
    // stays gone (writing it back would overrule her) and the row stays here.
    write_bound(&state, NOTEBOOK).await.unwrap();
    assert!(!bundle.join(&entry.path).exists());
    assert_eq!(reconcile(&state, NOTEBOOK).await.unwrap().deleted, 0);
    assert!(state.db.get_note("note-0").await.unwrap().is_some());

    resolve_deletion_proposal(&state, NOTEBOOK, "note-0", true)
        .await
        .unwrap();
    let back = bundle.join(&entry.path);
    assert!(back.is_file(), "restore puts the file back for both of us");
    let manifest = load_manifest_checked(&manifest_path(&app_data_dir(&state), "a")).unwrap();
    assert!(manifest.proposed_deletions.is_empty());
    // A new lifetime: her record ends the old one for good, so reusing that
    // id would have the file deleted again the moment she syncs.
    let now = manifest.concepts.get("note-0").unwrap();
    assert_ne!(now.portable_id, entry.portable_id);
    assert!(std::fs::read_to_string(&back)
        .unwrap()
        .contains(&now.portable_id));
    assert_eq!(reconcile(&state, NOTEBOOK).await.unwrap().deleted, 0);
    assert!(state.db.get_note("note-0").await.unwrap().is_some());
}

#[tokio::test]
async fn remove_accepts_their_deletion() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let (state, entry) = seeded(&lab, &bundle, true).await;
    deleted_by(&bundle, &entry, Some("human:kim"));
    reconcile(&state, NOTEBOOK).await.unwrap();

    resolve_deletion_proposal(&state, NOTEBOOK, "note-0", false)
        .await
        .unwrap();
    assert!(state.db.get_note("note-0").await.unwrap().is_none());
    assert!(deletion_proposals(&state, NOTEBOOK)
        .await
        .unwrap()
        .is_empty());
    // And nothing comes back: the question was answered, not deferred.
    assert!(!reconcile(&state, NOTEBOOK).await.unwrap().changed());
    write_bound(&state, NOTEBOOK).await.unwrap();
    assert!(!bundle.join(&entry.path).exists());
}

#[tokio::test]
async fn our_own_deletion_is_never_a_question_shared_or_not() {
    // Signed by this account, and the unsigned record an older Alchemy
    // writes: both are ours, and ours stand.
    for actor in [Some(okf_human()), None] {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let (state, entry) = seeded(&lab, &bundle, true).await;
        deleted_by(&bundle, &entry, actor.as_deref());
        assert_eq!(reconcile(&state, NOTEBOOK).await.unwrap().deleted, 1);
        assert!(state.db.get_note("note-0").await.unwrap().is_none());
        assert!(deletion_proposals(&state, NOTEBOOK)
            .await
            .unwrap()
            .is_empty());
    }
}
