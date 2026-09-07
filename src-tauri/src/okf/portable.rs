//! Portable entity lifetimes are independent of machine-local database IDs.
use super::*;
use std::collections::{HashMap, HashSet};

fn deterministic_id(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

pub(super) fn explicit_id(doc: &OkfDoc) -> Result<Option<String>, String> {
    doc.nested("alchemy", "sync_id")
        .map(|id| {
            let parsed = uuid::Uuid::parse_str(&id)
                .map_err(|_| "Invalid portable notebook entity identity".to_string())?;
            if parsed.is_nil() || parsed.to_string() != id {
                return Err("Noncanonical portable notebook entity identity".into());
            }
            Ok(id)
        })
        .transpose()
}

pub(super) fn identity(doc: &OkfDoc, rel: &str, hash: &str) -> Result<String, String> {
    if let Some(id) = explicit_id(doc)? {
        return Ok(id);
    }
    let kind = rel.split('/').next().unwrap_or_default();
    Ok(match doc.nested("alchemy", "id") {
        Some(id) => deterministic_id(&format!("alchemy-sync-v1\0{kind}\0{id}")),
        None => deterministic_id(&format!("alchemy-sync-v1\0{rel}\0{hash}")),
    })
}

pub(super) fn local_identity(kind: &str, id: &str) -> String {
    deterministic_id(&format!("alchemy-sync-v1\0{kind}\0{id}"))
}

/// Migration changes only frontmatter; the original body and all metadata
/// survive even when Alchemy does not understand their meaning.
pub(super) fn attach_identity(text: &str, id: &str) -> Result<String, String> {
    let (mut front, body) = if let Some(rest) = text.strip_prefix("---\n") {
        let end = rest
            .find("\n---")
            .ok_or("Unclosed frontmatter during sync migration")?;
        let front: serde_yaml_ng::Mapping = serde_yaml_ng::from_str(&rest[..end])
            .map_err(|err| format!("Cannot migrate malformed frontmatter: {err}"))?;
        (front, &rest[end + 4..])
    } else {
        (serde_yaml_ng::Mapping::new(), text)
    };
    let alchemy = front
        .entry(serde_yaml_ng::Value::String("alchemy".into()))
        .or_insert_with(|| serde_yaml_ng::Value::Mapping(Default::default()));
    let mapping = alchemy
        .as_mapping_mut()
        .ok_or("Alchemy frontmatter must be a mapping before sync migration")?;
    mapping.insert("sync_id".into(), id.into());
    let header = serde_yaml_ng::to_string(&front).map_err(|err| err.to_string())?;
    let separator = if body.starts_with('\n') { "" } else { "\n" };
    Ok(format!("---\n{header}---{separator}{body}"))
}

pub(super) fn drop_dead_aliases(manifest: &mut OkfManifest, live: &HashSet<String>) -> bool {
    let live_paths: HashSet<_> = manifest
        .concepts
        .iter()
        .filter(|(id, _)| live.contains(*id))
        .map(|(_, entry)| entry.path.clone())
        .collect();
    let before = manifest.concepts.len();
    manifest
        .concepts
        .retain(|id, entry| live.contains(id) || !live_paths.contains(&entry.path));
    before != manifest.concepts.len()
}

pub(super) async fn repair_dead_aliases(
    state: &AppState,
    notebook_id: &str,
    manifest: &mut OkfManifest,
) -> Result<bool, String> {
    let mut paths: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, entry) in &manifest.concepts {
        paths.entry(&entry.path).or_default().push(id);
    }
    let mut dead = Vec::new();
    for (path, ids) in paths.into_iter().filter(|(_, ids)| ids.len() > 1) {
        let mut absent = Vec::new();
        let mut present = false;
        for id in ids {
            let owner = if path.starts_with("notes/") {
                e(state.db.get_note(id).await)?.map(|row| row.notebook_id)
            } else {
                e(state.db.get_source(id).await)?.map(|row| row.notebook_id)
            };
            match owner {
                Some(owner) if owner == notebook_id => present = true,
                Some(_) => return Err("A duplicate sync claim belongs to another notebook".into()),
                None => absent.push(id.to_string()),
            }
        }
        if present {
            dead.extend(absent);
        }
    }
    for id in &dead {
        manifest.concepts.remove(id);
    }
    Ok(!dead.is_empty())
}

pub(super) fn migrate_pending(bundle: &Path, manifest: &mut OkfManifest) -> Result<bool, String> {
    let mut changed = false;
    for pending in manifest.imports.values_mut() {
        if !pending.entry.portable_id.is_empty() {
            continue;
        }
        let path = bundle.join(&pending.entry.path);
        if is_dataless(&path) {
            return Err(
                "Download the interrupted import before upgrading its sync identity".into(),
            );
        }
        let text = std::fs::read_to_string(path).map_err(|err| {
            format!("Restore the interrupted import before upgrading sync: {err}")
        })?;
        if okf_hash(&text) != pending.entry.hash {
            return Err(
                "Restore the interrupted import's original file before upgrading its sync identity"
                    .into(),
            );
        }
        let doc = parse_okf_doc(&text);
        pending.entry.portable_id = identity(&doc, &pending.entry.path, &pending.entry.hash)?;
        pending.entry.portable_written = explicit_id(&doc)?.is_some();
        changed = true;
    }
    Ok(changed)
}

pub(super) fn migration_ready(bundle: &Path, manifest: &OkfManifest) -> Result<bool, String> {
    if check_protocol(bundle)? {
        return Ok(true);
    }
    if manifest
        .concepts
        .values()
        .any(|entry| !entry.portable_written)
    {
        return Ok(false);
    }
    for dir in ["notes", "sources"] {
        if !evicted_concepts(bundle, dir).is_empty() {
            return Ok(false);
        }
        for path in concept_files(bundle, dir) {
            if is_dataless(&path) {
                return Ok(false);
            }
            let text = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
            let doc = parse_okf_doc(&text);
            if doc.nested("alchemy", "id").is_some() && explicit_id(&doc)?.is_none() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Protocol {
    version: u32,
}

pub(super) fn check_protocol(bundle: &Path) -> Result<bool, String> {
    let path = bundle.join("sync/protocol.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(format!("Could not read sync protocol: {err}")),
    };
    let protocol: Protocol = serde_json::from_slice(&bytes)
        .map_err(|err| format!("Invalid notebook sync protocol: {err}"))?;
    if protocol.version != 1 {
        return Err(
            "This notebook requires a different sync protocol; update Alchemy before syncing"
                .into(),
        );
    }
    Ok(true)
}

pub(super) fn publish_protocol(bundle: &Path) -> Result<(), String> {
    if check_protocol(bundle)? {
        return Ok(());
    }
    use std::io::Write;
    let dir = bundle.join("sync");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(|err| err.to_string())?;
    temp.write_all(b"{\"version\":1}\n")
        .map_err(|err| err.to_string())?;
    temp.as_file().sync_all().map_err(|err| err.to_string())?;
    match temp.persist_noclobber(dir.join("protocol.json")) {
        Ok(_) => {}
        Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => {
            check_protocol(bundle)?;
        }
        Err(err) => return Err(err.to_string()),
    }
    std::fs::File::open(&dir)
        .and_then(|file| file.sync_all())
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// A pre-portable manifest may already own a returning file under a different
/// path. Only its original row identity or exact observed file bytes prove
/// that relationship; title and extracted-content similarity do not.
fn recover_legacy_paths(
    bundle: &Path,
    candidate: &mut OkfManifest,
    deleted: &HashSet<String>,
) -> Result<bool, String> {
    let missing: Vec<_> = candidate
        .concepts
        .iter()
        .filter(|(_, entry)| entry.portable_id.is_empty() && !bundle.join(&entry.path).exists())
        .map(|(id, entry)| (id.clone(), entry.clone()))
        .collect();
    if missing.is_empty() {
        return Ok(false);
    }
    let mut assigned = HashMap::new();
    let mut unresolved = Vec::new();
    for kind in ["notes", "sources"] {
        for path in concept_files(bundle, kind) {
            let rel = path
                .strip_prefix(bundle)
                .map_err(|err| err.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if candidate.concepts.values().any(|entry| entry.path == rel) || is_dataless(&path) {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .map_err(|err| format!("Could not inspect returning legacy file {rel}: {err}"))?;
            let hash = okf_hash(&text);
            if recovery::is_deleted_replay(candidate, &rel, &hash) {
                continue;
            }
            let doc = parse_okf_doc(&text);
            let legacy_id = doc.nested("alchemy", "id");
            let explicit = explicit_id(&doc)?;
            let portable_id = identity(&doc, &rel, &hash)?;
            let matches: Vec<_> = missing
                .iter()
                .filter(|(id, entry)| {
                    entry.path.split('/').next() == Some(kind)
                        && (legacy_id.as_ref() == Some(id)
                            || entry.hash == hash
                            || entry.seen_hashes.contains(&hash))
                })
                .collect();
            if matches.len() > 1 {
                return Err(format!("Cannot identify returning legacy file {rel}: multiple existing rows match its identity or file history. Restore the original paths before syncing; all rows and files were preserved"));
            }
            if let Some((id, _)) = matches.first() {
                if let Some(previous) = assigned.insert(id.clone(), rel.clone()) {
                    return Err(format!("Returning legacy files {previous} and {rel} both match one existing row. Restore the original paths before syncing; all rows and files were preserved"));
                }
                let entry = candidate.concepts.get_mut(id).unwrap();
                entry.portable_id = portable_id;
                entry.portable_written = explicit.is_some();
                entry.path = rel;
                entry.adopted = true;
                entry.file_mtime = 0;
                entry.file_len = 0;
                entry.missing_since = 0;
            } else if (explicit.is_some() || legacy_id.is_some())
                && !deleted.contains(&portable_id)
                && !candidate
                    .concepts
                    .values()
                    .any(|entry| entry.portable_id == portable_id)
            {
                unresolved.push((kind, rel));
            }
        }
    }
    for (kind, rel) in unresolved {
        if missing.iter().any(|(id, entry)| {
            entry.path.split('/').next() == Some(kind) && !assigned.contains_key(id)
        }) {
            return Err(format!("Cannot identify returning legacy file {rel} while older {kind} are missing. Restore their original paths or original file versions before syncing; no new rows were imported"));
        }
    }
    Ok(!assigned.is_empty())
}

/// Validate the whole set before importing any row. A moved file takes its
/// existing local row and conflict baseline with it. Simultaneous copies of
/// one identity are ambiguous, so keep both files and require repair.
pub(super) fn prepare(
    bundle: &Path,
    manifest: &mut OkfManifest,
    deleted: &HashSet<String>,
) -> Result<bool, String> {
    let mut changed = false;
    let versioned = check_protocol(bundle)? || manifest.protocol_version == 1;
    let mut candidate = manifest.clone();
    changed |= recover_legacy_paths(bundle, &mut candidate, deleted)?;
    if versioned && candidate.protocol_version != 1 {
        candidate.protocol_version = 1;
        changed = true;
    }
    let mut paths: HashMap<String, Vec<String>> = HashMap::new();
    for dir in ["notes", "sources"] {
        for path in concept_files(bundle, dir) {
            let rel = path
                .strip_prefix(bundle)
                .map_err(|err| err.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let known = candidate
                .concepts
                .iter()
                .find(|(_, entry)| entry.path == rel && !deleted.contains(&entry.portable_id))
                .map(|(id, entry)| (id.clone(), entry.clone()));
            if let Some((_, entry)) = &known {
                if !entry.portable_id.is_empty()
                    && (is_dataless(&path)
                        || is_untouched(&rel, file_clock(&path).0, file_clock(&path).1, &candidate))
                {
                    paths
                        .entry(entry.portable_id.clone())
                        .or_default()
                        .push(rel);
                    continue;
                }
            }
            if is_dataless(&path) {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|err| {
                format!("Could not read {} before syncing: {err}", path.display())
            })?;
            let hash = okf_hash(&text);
            if recovery::is_deleted_replay(&candidate, &rel, &hash) {
                continue;
            }
            let doc = parse_okf_doc(&text);
            let explicit = explicit_id(&doc)?;
            let mut portable_id = identity(&doc, &rel, &hash)?;
            if deleted.contains(&portable_id) {
                if let Some((id, entry)) = &known {
                    if entry.portable_id.is_empty() {
                        candidate.concepts.get_mut(id).unwrap().portable_id = portable_id;
                        changed = true;
                    }
                }
                continue;
            }
            if let Some((_, entry)) = &known {
                if !entry.portable_id.is_empty() {
                    if explicit.as_ref().is_some_and(|id| *id != entry.portable_id) {
                        return Err(format!(
                            "Sync identity changed at {rel}; both versions were left intact"
                        ));
                    }
                    portable_id = entry.portable_id.clone();
                }
            }
            if versioned
                && explicit.is_none()
                && doc.nested("alchemy", "id").is_some()
                && known
                    .as_ref()
                    .is_none_or(|(_, entry)| entry.portable_written)
            {
                return Err(format!("{rel} lost its portable sync identity. Update every Alchemy client and restore the file identity before syncing"));
            }
            if let Some((id, _)) = known {
                let entry = candidate.concepts.get_mut(&id).unwrap();
                changed |= entry.portable_id != portable_id
                    || entry.portable_written != explicit.is_some();
                entry.portable_id = portable_id.clone();
                entry.portable_written = explicit.is_some();
            }
            paths.entry(portable_id).or_default().push(rel);
        }
    }
    for (portable_id, locations) in &paths {
        if locations.len() > 1 {
            return Err(format!("Multiple files have sync identity {portable_id}: {}. Both were preserved; give an intentional copy a new sync_id", locations.join(", ")));
        }
        let rel = &locations[0];
        let owners: Vec<_> = candidate
            .concepts
            .iter()
            .filter(|(_, entry)| entry.portable_id == *portable_id)
            .map(|(id, _)| id.clone())
            .collect();
        if owners.len() > 1 {
            return Err("Multiple local rows claim one portable sync identity".into());
        }
        if let Some(id) = owners.first() {
            let entry = candidate.concepts.get_mut(id).unwrap();
            if entry.path != *rel {
                if entry.path.split('/').next() != rel.split('/').next() {
                    return Err("A sync identity moved between notes and sources; original data was preserved".into());
                }
                if bundle.join(&entry.path).exists() {
                    return Err(format!(
                        "Cannot move sync identity from {} to {rel}: the original still exists",
                        entry.path
                    ));
                }
                changed = true;
                entry.path = rel.clone();
                entry.adopted = true;
                entry.file_mtime = 0;
                entry.file_len = 0;
                entry.missing_since = 0;
            }
        }
    }
    *manifest = candidate;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::super::sync_tests::Lab;
    use super::*;

    async fn seed(state: &AppState) {
        state
            .db
            .add_note(&Note {
                id: "local-note".into(),
                notebook_id: "shared-notebook".into(),
                title: "Original".into(),
                content: "First version".into(),
                kind: "audio_overview".into(),
                prompt: String::new(),
                origin: "human:test".into(),
                status: String::new(),
                created_at: 1,
                updated_at: 1,
            })
            .await
            .unwrap();
        write_bound(state, "shared-notebook").await.unwrap();
    }

    async fn legacy_fixture(remote_id: &str) -> (Lab, AppState, PathBuf, PathBuf, String) {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let state = lab.replica("a", &bundle).await;
        seed(&state).await;
        let path = bundle.join("notes/original.md");
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|line| !line.contains("sync_id:"))
            .collect::<Vec<_>>()
            .join("\n")
            .replace("local-note", remote_id);
        std::fs::write(&path, &text).unwrap();
        std::fs::remove_file(bundle.join("sync/protocol.json")).unwrap();
        let at = manifest_path(&app_data_dir(&state), "a");
        let mut manifest = load_manifest_checked(&at).unwrap();
        manifest.protocol_version = 0;
        let entry = manifest.concepts.get_mut("local-note").unwrap();
        entry.portable_id.clear();
        entry.portable_written = false;
        entry.hash = okf_hash(&text);
        entry.seen_hashes = [entry.hash.clone()].into();
        entry.file_mtime = 0;
        save_manifest_checked(&at, &manifest).unwrap();
        (lab, state, bundle, at, text)
    }

    #[tokio::test]
    async fn first_upgrade_recovers_renamed_edited_legacy_note_by_exact_row_identity() {
        let (_lab, state, bundle, at, text) = legacy_fixture("local-note").await;
        state
            .db
            .update_note("local-note", "Original", "Unpublished local version", 2)
            .await
            .unwrap();
        std::fs::remove_file(bundle.join("notes/original.md")).unwrap();
        std::fs::write(
            bundle.join("notes/returned.md"),
            text.replace("First version", "Returned edited version"),
        )
        .unwrap();
        let result = reconcile(&state, "shared-notebook").await.unwrap();
        assert_eq!(result.created, 0);
        assert_eq!(result.updated, 1);
        let rows = state.db.list_notes("shared-notebook").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "local-note");
        assert!(rows[0].content.contains("Returned edited version"));
        let manifest = load_manifest_checked(&at).unwrap();
        assert_eq!(manifest.concepts["local-note"].path, "notes/returned.md");
        let copies: Vec<_> = std::fs::read_dir(bundle.join("conflicts"))
            .unwrap()
            .collect();
        assert_eq!(copies.len(), 1);
        assert!(std::fs::read_to_string(copies[0].as_ref().unwrap().path())
            .unwrap()
            .contains("Unpublished local version"));
        assert!(!reconcile(&state, "shared-notebook")
            .await
            .unwrap()
            .changed());
    }

    #[tokio::test]
    async fn first_upgrade_recovers_remote_legacy_identity_by_exact_observed_file_hash() {
        let (_lab, state, bundle, at, _) = legacy_fixture("other-machine-row").await;
        let before = load_manifest_checked(&at).unwrap().concepts["local-note"]
            .local_hash
            .clone();
        std::fs::rename(
            bundle.join("notes/original.md"),
            bundle.join("notes/returned.md"),
        )
        .unwrap();
        assert_eq!(
            reconcile(&state, "shared-notebook").await.unwrap().created,
            0
        );
        let rows = state.db.list_notes("shared-notebook").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "local-note");
        let manifest = load_manifest_checked(&at).unwrap();
        let entry = &manifest.concepts["local-note"];
        assert_eq!(entry.path, "notes/returned.md");
        assert_eq!(entry.local_hash, before);
        assert_eq!(
            entry.portable_id,
            local_identity("notes", "other-machine-row")
        );
    }

    #[tokio::test]
    async fn first_upgrade_refuses_unproven_returning_legacy_file_without_mutating_rows_or_files() {
        let (_lab, state, bundle, at, text) = legacy_fixture("other-machine-row").await;
        std::fs::remove_file(bundle.join("notes/original.md")).unwrap();
        let returned = text.replace("First version", "Different unproven version");
        let path = bundle.join("notes/returned.md");
        std::fs::write(&path, &returned).unwrap();
        let manifest = std::fs::read(&at).unwrap();
        let rows =
            serde_json::to_value(state.db.list_notes("shared-notebook").await.unwrap()).unwrap();
        assert!(reconcile(&state, "shared-notebook")
            .await
            .unwrap_err()
            .contains("Cannot identify returning legacy file"));
        assert_eq!(std::fs::read(&at).unwrap(), manifest);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), returned);
        assert_eq!(
            serde_json::to_value(state.db.list_notes("shared-notebook").await.unwrap()).unwrap(),
            rows
        );
    }

    #[test]
    fn first_upgrade_refuses_ambiguous_exact_file_history() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sources")).unwrap();
        let text = "---\nalchemy:\n  id: remote-row\n---\nExact observed version";
        std::fs::write(dir.path().join("sources/returned.md"), text).unwrap();
        let mut manifest = OkfManifest::default();
        for id in ["local-one", "local-two"] {
            manifest.concepts.insert(
                id.into(),
                OkfManifestEntry {
                    path: format!("sources/{id}.md"),
                    hash: okf_hash(text),
                    ..Default::default()
                },
            );
        }
        let before = serde_json::to_value(&manifest).unwrap();
        assert!(prepare(dir.path(), &mut manifest, &HashSet::new())
            .unwrap_err()
            .contains("multiple existing rows"));
        assert_eq!(serde_json::to_value(&manifest).unwrap(), before);
    }

    #[tokio::test]
    async fn first_upgrade_refuses_unknown_modern_identity_while_legacy_rows_are_missing() {
        let (_lab, state, bundle, at, text) = legacy_fixture("other-machine-row").await;
        std::fs::remove_file(bundle.join("notes/original.md")).unwrap();
        let returned = attach_identity(
            &text.replace("First version", "Newer unknown version"),
            &new_id(),
        )
        .unwrap();
        let path = bundle.join("notes/returned.md");
        std::fs::write(&path, &returned).unwrap();
        let manifest = std::fs::read(&at).unwrap();
        let rows =
            serde_json::to_value(state.db.list_notes("shared-notebook").await.unwrap()).unwrap();
        assert!(reconcile(&state, "shared-notebook")
            .await
            .unwrap_err()
            .contains("Cannot identify returning legacy file"));
        assert_eq!(std::fs::read(&at).unwrap(), manifest);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), returned);
        assert_eq!(
            serde_json::to_value(state.db.list_notes("shared-notebook").await.unwrap()).unwrap(),
            rows
        );
    }

    #[test]
    fn first_upgrade_refuses_multiple_files_proving_the_same_local_identity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sources")).unwrap();
        let mut manifest = OkfManifest::default();
        manifest.concepts.insert(
            "local-row".into(),
            OkfManifestEntry {
                path: "sources/missing.md".into(),
                local_hash: "original baseline".into(),
                ..Default::default()
            },
        );
        for filename in ["one.md", "two.md"] {
            std::fs::write(
                dir.path().join("sources").join(filename),
                format!("---\nalchemy:\n  id: local-row\n---\nDifferent body for {filename}"),
            )
            .unwrap();
        }
        let before = serde_json::to_value(&manifest).unwrap();
        assert!(prepare(dir.path(), &mut manifest, &HashSet::new())
            .unwrap_err()
            .contains("both match one existing row"));
        assert_eq!(serde_json::to_value(&manifest).unwrap(), before);
        assert_eq!(
            std::fs::read_dir(dir.path().join("sources"))
                .unwrap()
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn renamed_file_keeps_each_replicas_local_row() {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let a = lab.replica("a", &bundle).await;
        let b = lab.replica("b", &bundle).await;
        seed(&a).await;
        reconcile(&b, "shared-notebook").await.unwrap();
        let before = b.db.list_notes("shared-notebook").await.unwrap()[0]
            .id
            .clone();
        std::fs::rename(
            bundle.join("notes/original.md"),
            bundle.join("notes/renamed.md"),
        )
        .unwrap();
        assert_eq!(reconcile(&b, "shared-notebook").await.unwrap().created, 0);
        assert_eq!(reconcile(&a, "shared-notebook").await.unwrap().created, 0);
        write_bound(&b, "shared-notebook").await.unwrap();
        write_bound(&a, "shared-notebook").await.unwrap();
        let notes = b.db.list_notes("shared-notebook").await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, before);
        assert!(!bundle.join("notes/original.md").exists());
        assert!(bundle.join("notes/renamed.md").exists());
    }

    #[tokio::test]
    async fn unseen_revision_cannot_resurrect_deleted_entity_on_fresh_replica() {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let a = lab.replica("a", &bundle).await;
        seed(&a).await;
        let original = std::fs::read_to_string(bundle.join("notes/original.md")).unwrap();
        a.db.delete_note("local-note").await.unwrap();
        write_bound(&a, "shared-notebook").await.unwrap();
        std::fs::write(
            bundle.join("notes/delayed.md"),
            original.replace("First version", "Unseen offline revision"),
        )
        .unwrap();
        let c = lab.replica("c", &bundle).await;
        assert_eq!(reconcile(&c, "shared-notebook").await.unwrap().created, 0);
        assert!(c.db.list_notes("shared-notebook").await.unwrap().is_empty());
        // A genuinely new lifetime at the same path is allowed.
        let id = new_id();
        let old = explicit_id(&parse_okf_doc(&original)).unwrap().unwrap();
        std::fs::write(bundle.join("notes/delayed.md"), original.replace(&old, &id)).unwrap();
        assert_eq!(reconcile(&c, "shared-notebook").await.unwrap().created, 1);
    }

    #[tokio::test]
    async fn duplicate_identity_and_older_client_downgrade_fail_before_import() {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let a = lab.replica("a", &bundle).await;
        seed(&a).await;
        let path = bundle.join("notes/original.md");
        std::fs::copy(&path, bundle.join("notes/copy.md")).unwrap();
        let b = lab.replica("b", &bundle).await;
        assert!(reconcile(&b, "shared-notebook")
            .await
            .unwrap_err()
            .contains("Multiple files"));
        assert!(b.db.list_notes("shared-notebook").await.unwrap().is_empty());
        std::fs::remove_file(bundle.join("notes/copy.md")).unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        let downgraded = original
            .lines()
            .filter(|line| !line.contains("sync_id:"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &downgraded).unwrap();
        assert!(reconcile(&b, "shared-notebook")
            .await
            .unwrap_err()
            .contains("lost its portable sync identity"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), downgraded);
    }

    #[test]
    fn legacy_identity_is_stable_across_paths_and_rejects_invalid_new_ids() {
        let doc = parse_okf_doc("---\nalchemy:\n  id: legacy-row\n---\nBody");
        assert_eq!(
            identity(&doc, "notes/old.md", "v1").unwrap(),
            identity(&doc, "notes/new.md", "v2").unwrap()
        );
        let bad = parse_okf_doc("---\nalchemy:\n  sync_id: invalid\n---\nBody");
        assert!(identity(&bad, "notes/bad.md", "v1").is_err());
    }

    #[test]
    fn pending_legacy_identity_requires_the_reserved_file_version() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("notes")).unwrap();
        let text = "---\nalchemy:\n  id: original-remote-id\n---\nBody";
        std::fs::write(dir.path().join("notes/pending.md"), text).unwrap();
        let mut manifest = OkfManifest::default();
        manifest.imports.insert(
            "notes/pending.md".into(),
            recovery::PendingImport {
                id: "local-reserved-id".into(),
                entry: OkfManifestEntry {
                    path: "notes/pending.md".into(),
                    hash: okf_hash(text),
                    ..Default::default()
                },
            },
        );
        assert!(migrate_pending(dir.path(), &mut manifest).unwrap());
        assert_eq!(
            manifest.imports["notes/pending.md"].entry.portable_id,
            identity(&parse_okf_doc(text), "notes/pending.md", &okf_hash(text)).unwrap()
        );
        manifest
            .imports
            .get_mut("notes/pending.md")
            .unwrap()
            .entry
            .portable_id
            .clear();
        std::fs::write(dir.path().join("notes/pending.md"), "Different document").unwrap();
        assert!(migrate_pending(dir.path(), &mut manifest).is_err());
    }

    #[test]
    fn identity_migration_preserves_unknown_metadata_and_waits_for_cloud_stubs() {
        let text = "---\ntitle: Original\nalchemy:\n  custom: [one, two]\nverified: true\n---\n\nOriginal body\n";
        let id = new_id();
        let migrated = attach_identity(text, &id).unwrap();
        assert!(migrated.ends_with("---\n\nOriginal body\n"));
        let mut doc = parse_okf_doc(&migrated);
        doc.front
            .get_mut("alchemy")
            .unwrap()
            .as_mapping_mut()
            .unwrap()
            .remove("sync_id");
        assert_eq!(doc.front, parse_okf_doc(text).front);
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("notes")).unwrap();
        std::fs::write(dir.path().join("notes/.pending.md.icloud"), b"stub").unwrap();
        assert!(!migration_ready(dir.path(), &OkfManifest::default()).unwrap());
    }

    #[tokio::test]
    async fn stale_duplicate_claim_is_removed_only_when_its_row_is_absent() {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let a = lab.replica("a", &bundle).await;
        seed(&a).await;
        let at = manifest_path(&app_data_dir(&a), "a");
        let mut manifest = load_manifest(&at);
        manifest.concepts.insert(
            "missing-row".into(),
            OkfManifestEntry {
                path: "notes/original.md".into(),
                ..Default::default()
            },
        );
        save_manifest_checked(&at, &manifest).unwrap();
        reconcile(&a, "shared-notebook").await.unwrap();
        assert_eq!(load_manifest(&at).concepts.len(), 1);
        assert!(a.db.get_note("local-note").await.unwrap().is_some());
        assert!(bundle.join("notes/original.md").exists());
    }

    #[tokio::test]
    async fn writer_rejects_remote_bytes_arriving_after_reconciliation() {
        let lab = Lab::new();
        let bundle = lab.0.join("bundle");
        let a = lab.replica("a", &bundle).await;
        seed(&a).await;
        let mut local = a.db.get_note("local-note").await.unwrap().unwrap();
        local.content = "Unpublished local content".into();
        local.updated_at = now_ms();
        a.db.update_note(&local.id, &local.title, &local.content, local.updated_at)
            .await
            .unwrap();
        let (notebook, sources, notes) = gather_bundle_for(&a, "shared-notebook", &bundle)
            .await
            .unwrap();
        let path = bundle.join("notes/original.md");
        let remote = std::fs::read_to_string(&path)
            .unwrap()
            .replace("First version", "Incoming remote content");
        std::fs::write(&path, &remote).unwrap();
        let at = manifest_path(&app_data_dir(&a), "a");
        assert!(
            write_bundle(&notebook, &sources, &notes, &bundle, Some(&at))
                .unwrap_err()
                .contains("changed after reconciliation")
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), remote);
        assert_eq!(
            a.db.get_note("local-note").await.unwrap().unwrap().content,
            local.content
        );
    }
}
