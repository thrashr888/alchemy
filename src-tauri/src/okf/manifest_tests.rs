use super::*;

#[test]
fn a_removed_initialized_manifest_is_not_a_new_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    save_manifest_checked(&path, &OkfManifest::default()).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(load_manifest_checked(&path)
        .unwrap_err()
        .contains("missing"));
}

#[test]
fn source_provenance_save_preserves_other_records_and_rejects_corruption() {
    let dir = tempfile::tempdir().unwrap();
    crate::device::note_origin_device_checked(dir.path(), "notebook", "first", "Other laptop")
        .unwrap();
    crate::device::note_origin_device_checked(dir.path(), "notebook", "second", "Third laptop")
        .unwrap();
    assert_eq!(
        crate::device::load_origin_devices(dir.path(), "notebook").len(),
        2
    );
    let path = dir.path().join("okf_devices/notebook.json");
    std::fs::write(&path, "{broken").unwrap();
    assert!(crate::device::note_origin_device_checked(
        dir.path(),
        "notebook",
        "third",
        "Fourth laptop"
    )
    .is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "{broken");
}

#[test]
fn checked_load_distinguishes_new_records_from_corruption_and_unreadable_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    assert!(load_manifest_checked(&path).unwrap().concepts.is_empty());
    std::fs::write(&path, "{ truncated").unwrap();
    assert!(load_manifest_checked(&path).is_err());
    assert!(load_manifest_checked(dir.path()).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ truncated");
}

#[test]
fn atomic_save_replaces_complete_records_and_cleans_up_staging_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    let mut manifest = OkfManifest::default();
    manifest
        .concepts
        .insert("first".into(), OkfManifestEntry::default());
    save_manifest_checked(&path, &manifest).unwrap();
    manifest
        .concepts
        .insert("second".into(), OkfManifestEntry::default());
    save_manifest_checked(&path, &manifest).unwrap();
    assert_eq!(load_manifest_checked(&path).unwrap().concepts.len(), 2);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn failed_serialization_preserves_the_previous_claims() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    let mut manifest = OkfManifest::default();
    manifest
        .concepts
        .insert("kept".into(), OkfManifestEntry::default());
    save_manifest_checked(&path, &manifest).unwrap();
    let before = std::fs::read(&path).unwrap();
    manifest.concepts.get_mut("kept").unwrap().extra.insert(
        serde_yaml_ng::Value::Sequence(vec![]),
        serde_yaml_ng::Value::Null,
    );
    assert!(save_manifest_checked(&path, &manifest).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(load_manifest_checked(&path)
        .unwrap()
        .concepts
        .contains_key("kept"));
}

#[test]
fn save_preflight_rejects_a_directory_destination_without_leaving_staging_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    std::fs::create_dir(&path).unwrap();
    assert!(save_manifest_checked(&path, &OkfManifest::default()).is_err());
    assert!(path.is_dir());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
