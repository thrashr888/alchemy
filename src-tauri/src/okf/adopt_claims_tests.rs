//! A claim keyed to a row that is gone belongs to the live row holding the
//! same document (`adopt_claims`); only a claim with no counterpart holds
//! the export.
use super::adopt_claims::{heal_orphaned_claims, heal_orphaned_claims_checked};
use super::sync_tests::Lab;
use super::*;
use sha2::{Digest, Sha256};

/// One note and one source, both titled "Original", read in from files
/// and written back — the shape every stale claim here starts from.
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

/// Reproduce unexplained row loss from legacy code: the row goes, and so
/// does the deletion receipt the store wrote for it.
async fn lose_row(state: &AppState, kind: &str, id: &str) {
    if kind == "note" {
        state.db.delete_note(id).await.unwrap();
    } else {
        state.db.delete_source(id).await.unwrap();
    }
    let key = format!("{kind}\0{id}");
    std::fs::remove_file(
        app_data_dir(state)
            .join("db/sync-deletions")
            .join(format!("{:x}.json", Sha256::digest(key.as_bytes()))),
    )
    .unwrap();
    assert!(!state.db.was_deleted(kind, id).unwrap());
}

async fn content_of(state: &AppState, kind: &str, id: &str) -> String {
    if kind == "note" {
        state.db.get_note(id).await.unwrap().unwrap().content
    } else {
        state.db.get_source(id).await.unwrap().unwrap().content
    }
}

/// A row added under a fresh id, the way the consolidation's re-imports
/// were: same document, new identity.
async fn add_row(state: &AppState, notebook: &str, kind: &str, id: &str, title: &str, text: &str) {
    if kind == "note" {
        state
            .db
            .add_note(&Note {
                id: id.into(),
                notebook_id: notebook.into(),
                title: title.into(),
                content: text.into(),
                kind: "audio_overview".into(),
                prompt: String::new(),
                origin: "human:test".into(),
                status: String::new(),
                created_at: 5,
                updated_at: 5,
            })
            .await
            .unwrap();
    } else {
        state
            .db
            .insert_sources_bulk(&[Source {
                id: id.into(),
                notebook_id: notebook.into(),
                title: title.into(),
                source_type: "text".into(),
                url: String::new(),
                content: text.into(),
                char_count: text.len() as i64,
                chunk_count: 0,
                created_at: 5,
                status: "ready".into(),
                error: String::new(),
                parent_id: String::new(),
                mtime: 0,
                author: String::new(),
                image_url: String::new(),
                tags: String::new(),
                note: String::new(),
                fetched_at: 5,
                fetch_failures: 0,
                origin_device: String::new(),
                remote: false,
            }])
            .await
            .unwrap();
    }
}

fn claimed_id(text: &str) -> String {
    parse_okf_doc(text).nested("alchemy", "id").unwrap()
}

fn conflict_copies(bundle: &Path) -> Vec<String> {
    std::fs::read_dir(bundle.join("conflicts"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_live_row_with_the_same_text_takes_over_the_stale_claim() {
    for kind in ["note", "source"] {
        for whitespace in [false, true] {
            let lab = Lab::new();
            let bundle = lab.0.join("shared");
            let a = lab.replica("a", &bundle).await;
            let (note_id, source_id) = seed(&a, &bundle).await;
            let nightly = crate::backup::okf_latest_dir(&app_data_dir(&a));
            export_all(&a, &nightly).await.unwrap();
            let lost = if kind == "note" { &note_id } else { &source_id };
            let text = content_of(&a, kind, lost).await;
            let text = if whitespace {
                format!("{}  \n\n", text.replace(' ', "   "))
            } else {
                text
            };
            lose_row(&a, kind, lost).await;
            add_row(&a, "shared-notebook", kind, "again", "Original", &text).await;
            let at = manifest_path(&app_data_dir(&a), "a");
            let claim = load_manifest_checked(&at).unwrap().concepts[lost].clone();

            write_bound(&a, "shared-notebook").await.unwrap();
            let after = load_manifest_checked(&at).unwrap();
            assert!(!after.concepts.contains_key(lost));
            assert_eq!(after.concepts["again"].path, claim.path);
            let file = std::fs::read_to_string(bundle.join(&claim.path)).unwrap();
            assert_eq!(claimed_id(&file), "again");
            assert!(conflict_copies(&bundle).is_empty(), "{kind} {whitespace}");
            // The row's id is in the file; the row is still the row.
            assert_eq!(content_of(&a, kind, "again").await, text);

            // The nightly copy's record had the same stale claim.
            export_all(&a, &nightly).await.unwrap();
            let copy =
                std::fs::read_to_string(nightly.join("shared-notebook").join(&claim.path)).unwrap();
            assert_eq!(claimed_id(&copy), "again");
        }
    }
}

#[tokio::test]
async fn a_live_row_with_different_text_takes_over_and_the_file_version_is_kept() {
    for kind in ["note", "source"] {
        let lab = Lab::new();
        let bundle = lab.0.join("shared");
        let a = lab.replica("a", &bundle).await;
        let (note_id, source_id) = seed(&a, &bundle).await;
        let lost = if kind == "note" { &note_id } else { &source_id };
        lose_row(&a, kind, lost).await;
        add_row(
            &a,
            "shared-notebook",
            kind,
            "july",
            "Original",
            "The July original\n",
        )
        .await;
        let at = manifest_path(&app_data_dir(&a), "a");
        let claim = load_manifest_checked(&at).unwrap().concepts[lost].clone();

        write_bound(&a, "shared-notebook").await.unwrap();
        let after = load_manifest_checked(&at).unwrap();
        assert!(!after.concepts.contains_key(lost));
        assert_eq!(after.concepts["july"].path, claim.path);
        let file = std::fs::read_to_string(bundle.join(&claim.path)).unwrap();
        assert_eq!(claimed_id(&file), "july");
        assert!(file.ends_with("The July original\n"), "{file}");
        let copies = conflict_copies(&bundle);
        assert_eq!(copies.len(), 1, "{kind}");
        assert!(
            copies[0].contains("Preserved version: remote"),
            "{}",
            copies[0]
        );
        assert!(copies[0].contains("Sole exported content"), "{}", copies[0]);
        assert!(std::fs::read_to_string(bundle.join("log.md"))
            .unwrap()
            .contains("conflicts/"));
        assert_eq!(content_of(&a, kind, "july").await, "The July original\n");
    }
}

#[tokio::test]
async fn a_claim_with_no_counterpart_still_holds_the_export() {
    for kind in ["note", "source"] {
        let lab = Lab::new();
        let bundle = lab.0.join("shared");
        let a = lab.replica("a", &bundle).await;
        let (note_id, source_id) = seed(&a, &bundle).await;
        let lost = if kind == "note" { &note_id } else { &source_id };
        lose_row(&a, kind, lost).await;
        add_row(
            &a,
            "shared-notebook",
            kind,
            "other",
            "Something else",
            "Other text\n",
        )
        .await;
        let at = manifest_path(&app_data_dir(&a), "a");
        let before = load_manifest_checked(&at).unwrap();
        let bytes = std::fs::read(bundle.join(&before.concepts[lost].path)).unwrap();

        let error = write_bound(&a, "shared-notebook").await.unwrap_err();
        assert!(error.contains("without a recorded deletion"), "{error}");
        let after = load_manifest_checked(&at).unwrap();
        assert_eq!(after.concepts[lost].path, before.concepts[lost].path);
        assert_eq!(after.concepts[lost].hash, before.concepts[lost].hash);
        assert!(!after.concepts.contains_key("other"));
        assert_eq!(
            std::fs::read(bundle.join(&before.concepts[lost].path)).unwrap(),
            bytes
        );
        assert!(conflict_copies(&bundle).is_empty());
        let done = heal_orphaned_claims_checked(&a).await;
        assert_eq!(done.orphaned, 1, "{done:?}");
        assert_eq!(done.adopted, 0);
    }
}

#[tokio::test]
async fn the_launch_pass_adopts_once_and_a_second_run_finds_nothing() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    let (note_id, source_id) = seed(&a, &bundle).await;
    let nightly = crate::backup::okf_latest_dir(&app_data_dir(&a));
    export_all(&a, &nightly).await.unwrap();
    lose_row(&a, "note", &note_id).await;
    lose_row(&a, "source", &source_id).await;
    add_row(
        &a,
        "shared-notebook",
        "note",
        "n2",
        "Original",
        "Sole exported content\n",
    )
    .await;
    add_row(
        &a,
        "shared-notebook",
        "source",
        "s2",
        "Original",
        "Rewritten\n",
    )
    .await;

    let done = heal_orphaned_claims_checked(&a).await;
    // The binding's record and the nightly copy's, two claims each.
    assert_eq!(done.records, 2, "{done:?}");
    assert_eq!(done.adopted, 4);
    assert_eq!(done.identical, 2);
    assert_eq!(done.conflicts, 2);
    assert_eq!(done.orphaned, 0);
    assert_eq!(done.failed, 0);
    let at = manifest_path(&app_data_dir(&a), "a");
    let after = load_manifest_checked(&at).unwrap();
    assert_eq!(after.concepts["n2"].path, "notes/original.md");
    assert_eq!(after.concepts["s2"].path, "sources/original.md");
    assert_eq!(conflict_copies(&bundle).len(), 1);
    assert_eq!(conflict_copies(&nightly.join("shared-notebook")).len(), 1);

    let again = heal_orphaned_claims_checked(&a).await;
    assert_eq!(
        again,
        super::adopt_claims::ClaimsHeal::default(),
        "{again:?}"
    );
    assert_eq!(conflict_copies(&bundle).len(), 1);

    // The files carry the rows' identity once written.
    write_bound(&a, "shared-notebook").await.unwrap();
    export_all(&a, &nightly).await.unwrap();
    for root in [&bundle, &nightly.join("shared-notebook")] {
        let note = std::fs::read_to_string(root.join("notes/original.md")).unwrap();
        assert_eq!(claimed_id(&note), "n2");
        let source = std::fs::read_to_string(root.join("sources/original.md")).unwrap();
        assert_eq!(claimed_id(&source), "s2");
        assert!(source.ends_with("Rewritten\n"), "{source}");
    }

    // The stamped entry point runs once per version.
    let stamp = app_data_dir(&a).join("okf-claims-adopted");
    assert!(!stamp.exists());
    heal_orphaned_claims(&a).await;
    assert_eq!(std::fs::read_to_string(&stamp).unwrap(), "1");
}

#[tokio::test]
async fn a_nightly_record_that_swapped_folders_with_a_twin_heals_on_the_next_copy() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    let (_, source_id) = seed(&a, &bundle).await;
    // A second notebook with the same title, created later: its copy goes
    // to `shared-notebook-2` whatever the table order says.
    a.db.create_notebook(&Notebook {
        id: "twin".into(),
        title: "Shared notebook".into(),
        color: String::new(),
        icon: String::new(),
        created_at: 2,
        updated_at: 2,
        status: "archived".into(),
        growth_web: false,
        source_count: 0,
        note_count: 0,
        report_count: 0,
    })
    .await
    .unwrap();
    add_row(
        &a,
        "twin",
        "source",
        "twin-original",
        "Original",
        "The twin's text\n",
    )
    .await;
    add_row(
        &a,
        "twin",
        "source",
        "twin-only",
        "Only here",
        "Only in the twin\n",
    )
    .await;
    let nightly = crate::backup::okf_latest_dir(&app_data_dir(&a));
    export_all(&a, &nightly).await.unwrap();
    let first = nightly.join("shared-notebook");
    let second = nightly.join("shared-notebook-2");
    assert_eq!(
        claimed_id(&std::fs::read_to_string(first.join("sources/original.md")).unwrap()),
        source_id
    );
    assert!(second.join("sources/only-here.md").exists());

    // What table order used to do: each folder's record ends up claiming
    // the other notebook's rows.
    let data_dir = app_data_dir(&a);
    let one = manifest_path(&data_dir, "nightly-shared-notebook");
    let two = manifest_path(&data_dir, "nightly-shared-notebook-2");
    let swap = data_dir.join("swap.json");
    std::fs::rename(&one, &swap).unwrap();
    std::fs::rename(&two, &one).unwrap();
    std::fs::rename(&swap, &two).unwrap();

    let done = heal_orphaned_claims_checked(&a).await;
    assert_eq!(done.records, 2, "{done:?}");
    // `original.md` in each folder goes to that notebook's own row; the
    // twin's `only-here.md` and the first notebook's note have no
    // counterpart in the other, and their rows live on where they are.
    assert_eq!(done.adopted, 2);
    assert_eq!(done.elsewhere, 2);
    assert_eq!(done.conflicts, 0);
    assert_eq!(done.orphaned, 0);
    assert_eq!(done.failed, 0);

    export_all(&a, &nightly).await.unwrap();
    assert_eq!(
        claimed_id(&std::fs::read_to_string(first.join("sources/original.md")).unwrap()),
        source_id
    );
    assert_eq!(
        claimed_id(&std::fs::read_to_string(second.join("sources/original.md")).unwrap()),
        "twin-original"
    );
    assert!(second.join("sources/only-here.md").exists());
    assert!(!first.join("sources/only-here.md").exists());
    assert!(first.join("notes/original.md").exists());
    assert!(!second.join("notes/original.md").exists());
    assert!(conflict_copies(&first).is_empty());
    assert!(conflict_copies(&second).is_empty());
}

#[tokio::test]
async fn records_nothing_reads_are_set_aside_and_the_rest_stay() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;
    seed(&a, &bundle).await;
    let nightly = crate::backup::okf_latest_dir(&app_data_dir(&a));
    export_all(&a, &nightly).await.unwrap();
    let data_dir = app_data_dir(&a);
    let stray = manifest_path(&data_dir, "stray-lineage");
    std::fs::copy(manifest_path(&data_dir, "a"), &stray).unwrap();
    std::fs::write(stray.with_extension("initialized"), b"").unwrap();

    let done = heal_orphaned_claims_checked(&a).await;
    assert_eq!(done.set_aside, 1, "{done:?}");
    assert_eq!(done.failed, 0);
    assert!(!stray.exists());
    assert!(!stray.with_extension("initialized").exists());
    let aside = data_dir.join("okf/orphaned");
    assert!(aside.join("stray-lineage.json").exists());
    assert!(aside.join("stray-lineage.initialized").exists());
    assert!(manifest_path(&data_dir, "a").exists());
    assert!(manifest_path(&data_dir, "nightly-shared-notebook").exists());
    assert_eq!(heal_orphaned_claims_checked(&a).await.set_aside, 0);
}
