//! Explicit folder binding stages imports under durable local ownership before
//! changing the notebook's active binding. Imported rows remain recoverable on
//! failure; the original folder is never used as a staging destination.
use super::*;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Phase {
    Importing,
    Committed,
    Cancelled,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Session {
    notebook_id: String,
    target: PathBuf,
    binding_id: String,
    original: Option<OkfBinding>,
    phase: Phase,
}

fn directory(data_dir: &Path, notebook_id: &str) -> PathBuf {
    data_dir.join("okf-bind-import").join(okf_hash(notebook_id))
}

fn session_path(data_dir: &Path, notebook_id: &str, target: &Path) -> PathBuf {
    directory(data_dir, notebook_id).join(format!("{}.json", okf_hash(&target.to_string_lossy())))
}

fn read(path: &Path) -> Result<Option<Session>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|err| {
            format!(
                "Could not read binding import session {}: {err}",
                path.display()
            )
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if path.with_extension("initialized").exists() {
                Err(format!(
                    "Established binding import session is missing: {}",
                    path.display()
                ))
            } else {
                Ok(None)
            }
        }
        Err(err) => Err(format!(
            "Could not read binding import session {}: {err}",
            path.display()
        )),
    }
}

fn save(path: &Path, session: &Session) -> Result<(), String> {
    use std::io::Write;
    let bytes = serde_json::to_vec_pretty(session).map_err(|err| err.to_string())?;
    let result = (|| -> std::io::Result<()> {
        let dir = path
            .parent()
            .expect("binding import session has a directory");
        std::fs::create_dir_all(dir)?;
        let mut staged = tempfile::NamedTempFile::new_in(dir)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        staged.persist(path).map_err(|err| err.error)?;
        let marker = path.with_extension("initialized");
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)
        {
            Ok(file) => file.sync_all()?,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    })();
    result.map_err(|err| {
        format!(
            "Could not save binding import session {}: {err}",
            path.display()
        )
    })
}

fn sessions(data_dir: &Path, notebook_id: &str) -> Result<Vec<(PathBuf, Session)>, String> {
    let entries = match std::fs::read_dir(directory(data_dir, notebook_id)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("Could not inspect pending binding imports: {err}")),
    };
    let mut records = std::collections::BTreeSet::new();
    for entry in entries {
        let path = entry.map_err(|err| err.to_string())?.path();
        if path
            .extension()
            .is_some_and(|ext| ext == "json" || ext == "initialized")
        {
            records.insert(path.with_extension("json"));
        }
    }
    records
        .into_iter()
        .filter_map(|path| match read(&path) {
            Ok(Some(session)) => Some(Ok((path, session))),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

pub(super) fn blocks_existing_write(
    data_dir: &Path,
    notebook_id: &str,
    binding_id: &str,
) -> Result<bool, String> {
    Ok(sessions(data_dir, notebook_id)?.iter().any(|(_, session)| {
        session.phase == Phase::Importing
            && session
                .original
                .as_ref()
                .is_some_and(|original| original.id == binding_id)
    }))
}

/// Explicit unbind may cancel the pause. Keep session identities so a later
/// deliberate retry still adopts its previously inserted rows without cleanup.
pub(super) fn cancel_imports(data_dir: &Path, notebook_id: &str) -> Result<(), String> {
    for (path, mut session) in sessions(data_dir, notebook_id)? {
        if session.phase == Phase::Importing {
            session.phase = Phase::Cancelled;
            save(&path, &session)?;
        }
    }
    Ok(())
}

fn interrupted_message(target: &Path, error: String) -> String {
    format!("Binding import into {} is incomplete: {error}. Existing items were kept. Retry binding this folder to resume, or unbind the notebook to cancel the pending import.", target.display())
}

/// No AppHandle is needed until the caller rearms filesystem watching.
pub(crate) async fn bind_folder(
    state: &AppState,
    notebook_id: &str,
    path: &str,
) -> Result<String, String> {
    let lock = notebook_sync_lock(state, notebook_id);
    let guard = lock.lock().await;
    let data_dir = app_data_dir(state);
    let current = binding_for_checked(&data_dir, notebook_id)?;
    if !e(state.db.list_notebooks().await)?
        .iter()
        .any(|notebook| notebook.id == notebook_id)
    {
        return Err("Notebook not found".into());
    }
    let target = PathBuf::from(path);
    if !target.exists() {
        std::fs::create_dir_all(&target)
            .map_err(|err| format!("Could not create {path}: {err}"))?;
    }
    if !target.is_dir() {
        return Err(format!("Not a folder: {path}"));
    }
    let target = same_folder(&target);
    if e(state.db.list_sources(notebook_id).await)?
        .iter()
        .any(|source| source.url == path)
    {
        return Err(
            "This notebook already reads that folder as a source — pick a different one".into(),
        );
    }
    if load_bindings_checked(&data_dir)?
        .iter()
        .any(|(id, binding)| id != notebook_id && same_folder(&binding.path) == target)
    {
        return Err("This folder is already bound to another notebook".into());
    }
    let session_at = session_path(&data_dir, notebook_id, &target);
    let prior_session = read(&session_at)?;
    // Rebinding the active location never reimports its owned files. This also
    // finishes a crash after binding publication but before session completion.
    if let Some(active) = current
        .as_ref()
        .filter(|binding| same_folder(&binding.path) == target)
    {
        if let Some(mut session) = prior_session {
            if session.binding_id != active.id {
                return Err("The binding import session belongs to a different binding".into());
            }
            session.phase = Phase::Committed;
            save(&session_at, &session)?;
        }
        let manifest_at = manifest_path(&data_dir, &active.id);
        adopt_legacy_manifest(&target, &manifest_at);
        load_manifest_checked(&manifest_at)?;
        bindings::allow_explicit(&data_dir, notebook_id, &target, &active.id)?;
        drop(guard);
        write_bound(state, notebook_id).await?;
        return Ok(path.into());
    }
    for (_, pending) in sessions(&data_dir, notebook_id)? {
        if pending.phase == Phase::Importing
            && pending.target != target
            && pending.original.as_ref().map(|binding| &binding.id)
                == current.as_ref().map(|binding| &binding.id)
        {
            return Err(interrupted_message(
                &pending.target,
                "another target is already pending".into(),
            ));
        }
    }
    let detached = bindings::detached_binding(&data_dir, notebook_id, &target)?;
    let mut session = if let Some(binding) = detached {
        Session {
            notebook_id: notebook_id.into(),
            target: target.clone(),
            binding_id: binding.id,
            original: current.clone(),
            phase: Phase::Importing,
        }
    } else {
        prior_session.unwrap_or_else(|| Session {
            notebook_id: notebook_id.into(),
            target: target.clone(),
            binding_id: new_id(),
            original: current.clone(),
            phase: Phase::Importing,
        })
    };
    if session.notebook_id != notebook_id || session.target != target {
        return Err("Binding import session names a different notebook or folder".into());
    }
    // This call is a fresh explicit request; a previous cancellation/rebind
    // does not revoke its existing per-file identities, but CAS uses today's
    // active binding so a concurrent mutation still cannot be overwritten.
    session.original = current.clone();
    session.phase = Phase::Importing;
    save(&session_at, &session)?;
    let manifest_at = manifest_path(&data_dir, &session.binding_id);
    let imported = import_files(state, notebook_id, &target, &manifest_at).await;
    if let Err(error) = imported {
        return Err(interrupted_message(&target, error));
    }
    #[cfg(test)]
    fault(state, "before-binding")?;
    replace_binding_checked(
        &data_dir,
        notebook_id,
        current.as_ref(),
        Some(OkfBinding {
            id: session.binding_id.clone(),
            path: target.to_string_lossy().into(),
            last_write_at: 0,
            lost: false,
        }),
    )?;
    #[cfg(test)]
    fault(state, "after-binding")?;
    session.phase = Phase::Committed;
    save(&session_at, &session)?;
    bindings::allow_explicit(&data_dir, notebook_id, &target, &session.binding_id)?;
    drop(guard);
    write_bound(state, notebook_id).await?;
    Ok(path.into())
}

async fn import_files(
    state: &AppState,
    notebook_id: &str,
    bundle: &Path,
    manifest_at: &Path,
) -> Result<(), String> {
    let mut manifest = load_manifest_checked(manifest_at)?;
    for dir in ["sources", "notes"] {
        if !evicted_concepts(bundle, dir).is_empty() {
            return Err(format!(
                "Download the notebook's {dir} before finishing this binding"
            ));
        }
    }
    let mut deleted = portable_deletions::read_deleted(bundle)?;
    deleted.extend(manifest.deleted_entities.iter().cloned());
    manifest.deleted_entities.extend(deleted.iter().cloned());
    portable::prepare(bundle, &mut manifest, &deleted)?;
    save_manifest_checked(manifest_at, &manifest)?;
    recovery::recover_imports(state, notebook_id, &mut manifest, manifest_at).await?;
    for dir in ["sources", "notes"] {
        for path in concept_files(bundle, dir) {
            let rel = path
                .strip_prefix(bundle)
                .map_err(|err| err.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if is_dataless(&path) || is_evicted_stub(&path) {
                return Err(format!("Download {rel} before finishing this binding"));
            }
            let (mtime, len) = file_clock(&path);
            let text = std::fs::read_to_string(&path)
                .map_err(|err| format!("Could not read {rel}: {err}"))?;
            let hash = okf_hash(&text);
            let doc = parse_okf_doc(&text);
            let portable_id = portable::identity(&doc, &rel, &hash)?;
            if deleted.contains(&portable_id) {
                continue;
            }
            if manifest.concepts.values().any(|entry| entry.path == rel) {
                // Retain the acknowledged baseline and clock. After binding
                // publication, normal reconciliation handles later edits and
                // preserves conflicts; staging never overwrites either side.
                continue;
            }
            if recovery::is_deleted_replay(&manifest, &rel, &hash) {
                continue;
            }
            if doc.body.trim().is_empty() {
                continue;
            }
            let id = recovery::reserve_import(
                &mut manifest,
                manifest_at,
                &rel,
                &hash,
                (mtime, len),
                &doc,
            )?;
            let taken = if dir == "notes" {
                take_in_note(state, notebook_id, &doc, &path, Some(&id)).await?
            } else {
                take_in_source(state, notebook_id, &doc, &path, bundle, Some(&id)).await?
            }
            .ok_or_else(|| format!("{rel} did not create its reserved notebook item"))?;
            #[cfg(test)]
            fault(state, "after-row")?;
            manifest.imports.remove(&rel);
            adopt(&mut manifest, &taken.id, &rel, &hash, mtime, len, &doc);
            manifest
                .concepts
                .get_mut(&taken.id)
                .expect("adopt creates its claim")
                .local_hash = taken.local_hash;
            save_manifest_checked(manifest_at, &manifest)?;
        }
    }
    // An interruption is not permission to remove an imported row. Changed
    // known files are reconciled after publication; missing or unreadable files
    // still prevent a complete import so no grace-period delete can run yet.
    for entry in manifest.concepts.values() {
        if deleted.contains(&entry.portable_id) {
            continue;
        }
        let path = bundle.join(&entry.path);
        if is_dataless(&path) || is_evicted_stub(&path) {
            return Err(format!(
                "Download {} before finishing this binding",
                entry.path
            ));
        }
        std::fs::read_to_string(&path)
            .map_err(|err| format!("Could not verify imported file {}: {err}", entry.path))?;
    }
    Ok(())
}

#[cfg(test)]
fn fault(state: &AppState, stage: &str) -> Result<(), String> {
    let flag = app_data_dir(state).join(format!("test-bind-import-{stage}"));
    if flag.is_file() {
        let remaining = std::fs::read_to_string(&flag)
            .unwrap_or_default()
            .trim()
            .parse::<usize>()
            .unwrap_or(1);
        if remaining > 1 {
            std::fs::write(&flag, (remaining - 1).to_string()).map_err(|err| err.to_string())?;
        } else {
            std::fs::remove_file(flag).map_err(|err| err.to_string())?;
            return Err(format!("test interruption {stage}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::okf::sync_tests::Lab;

    async fn seed(state: &AppState) {
        state
            .db
            .add_note(&Note {
                id: "existing-note".into(),
                notebook_id: "shared-notebook".into(),
                title: "Existing".into(),
                content: "Existing user content".into(),
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

    fn incoming(lab: &Lab, sources: bool) -> PathBuf {
        let folder = lab.0.join("incoming");
        let dir = if sources { "sources" } else { "notes" };
        std::fs::create_dir_all(folder.join(dir)).unwrap();
        std::fs::write(folder.join("index.md"), "# Incoming\n").unwrap();
        for i in 0..2 {
            let metadata = if sources {
                "  source_type: text\n  tags: important\n  author: Original Author\n  device: Other Mac\n"
            } else {
                "  kind: audio_overview\n"
            };
            std::fs::write(folder.join(dir).join(format!("document-{i}.md")), format!("---\ntitle: Deliberate duplicate\nalchemy:\n{metadata}---\nIdentical document body\n")).unwrap();
        }
        folder
    }

    fn originals(folder: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = vec![(
            folder.join("index.md"),
            std::fs::read(folder.join("index.md")).unwrap(),
        )];
        for dir in ["sources", "notes"] {
            for path in concept_files(folder, dir) {
                files.push((path.clone(), std::fs::read(path).unwrap()));
            }
        }
        files
    }

    #[tokio::test]
    async fn interrupted_explicit_import_preserves_original_binding_files_and_duplicate_identities()
    {
        for sources in [false, true] {
            for after in [1, 2] {
                let lab = Lab::new();
                let old = lab.0.join("old");
                let state = lab.replica("a", &old).await;
                seed(&state).await;
                if sources {
                    crate::commands::store_extracted_with_id(
                        &state,
                        "shared-notebook",
                        ingest::Extracted {
                            title: "Existing source".into(),
                            source_type: "text".into(),
                            text: "Identical document body".into(),
                            url: String::new(),
                            feeds: Vec::new(),
                            image_url: String::new(),
                            author: "Local Author".into(),
                        },
                        "existing-source",
                        "local-tag",
                        "Local Mac",
                    )
                    .await
                    .unwrap();
                    write_bound(&state, "shared-notebook").await.unwrap();
                }
                let old_files = originals(&old);
                let target = incoming(&lab, sources);
                let incoming_files = originals(&target);
                std::fs::write(
                    app_data_dir(&state).join("test-bind-import-after-row"),
                    after.to_string(),
                )
                .unwrap();
                assert!(
                    bind_folder(&state, "shared-notebook", target.to_str().unwrap())
                        .await
                        .is_err()
                );
                assert_eq!(
                    binding_for_checked(&app_data_dir(&state), "shared-notebook")
                        .unwrap()
                        .unwrap()
                        .id,
                    "a"
                );
                assert!(
                    blocks_existing_write(&app_data_dir(&state), "shared-notebook", "a").unwrap()
                );
                let _ = write_bound(&state, "shared-notebook").await;
                assert_eq!(originals(&old), old_files);
                for (path, bytes) in old_files.iter().chain(incoming_files.iter()) {
                    assert_eq!(std::fs::read(path).unwrap(), *bytes);
                }
                let landed: std::collections::HashSet<_> = if sources {
                    state
                        .db
                        .list_sources("shared-notebook")
                        .await
                        .unwrap()
                        .into_iter()
                        .filter(|source| source.id != "existing-source")
                        .map(|source| source.id)
                        .collect()
                } else {
                    state
                        .db
                        .list_notes("shared-notebook")
                        .await
                        .unwrap()
                        .into_iter()
                        .filter(|note| note.id != "existing-note")
                        .map(|note| note.id)
                        .collect()
                };
                assert_eq!(landed.len(), after);
                drop(state);
                let state = lab.replica("a", &old).await;
                bind_folder(&state, "shared-notebook", target.to_str().unwrap())
                    .await
                    .unwrap();
                assert_eq!(state.db.list_notebooks().await.unwrap().len(), 1);
                assert_eq!(
                    state
                        .db
                        .get_note("existing-note")
                        .await
                        .unwrap()
                        .unwrap()
                        .content,
                    "Existing user content"
                );
                if sources {
                    let current = state.db.list_sources("shared-notebook").await.unwrap();
                    assert_eq!(current.len(), 3);
                    let existing = current
                        .iter()
                        .find(|source| source.id == "existing-source")
                        .unwrap();
                    assert_eq!(
                        (existing.tags.as_str(), existing.author.as_str()),
                        ("local-tag", "Local Author")
                    );
                    assert!(landed
                        .iter()
                        .all(|id| current.iter().any(|source| &source.id == id)));
                    assert!(current
                        .iter()
                        .filter(|source| source.id != "existing-source")
                        .all(|source| source.tags == "important"
                            && source.author == "Original Author"));
                } else {
                    let current = state.db.list_notes("shared-notebook").await.unwrap();
                    assert_eq!(current.len(), 3);
                    assert!(landed
                        .iter()
                        .all(|id| current.iter().any(|note| &note.id == id)));
                }
                assert_eq!(originals(&old), old_files);
            }
        }
    }

    #[tokio::test]
    async fn staging_validates_portable_identities_before_creating_any_row() {
        let lab = Lab::new();
        let old = lab.0.join("old");
        let state = lab.replica("a", &old).await;
        seed(&state).await;
        let target = incoming(&lab, false);
        let id = new_id();
        for file in concept_files(&target, "notes") {
            let text = std::fs::read_to_string(&file)
                .unwrap()
                .replace("  kind:", &format!("  sync_id: {id}\n  kind:"));
            std::fs::write(file, text).unwrap();
        }
        let snapshot = originals(&target);
        assert!(
            bind_folder(&state, "shared-notebook", target.to_str().unwrap())
                .await
                .is_err()
        );
        assert_eq!(
            state.db.list_notes("shared-notebook").await.unwrap().len(),
            1
        );
        assert_eq!(originals(&target), snapshot);
        assert!(!target.join("sync/protocol.json").exists());
    }

    #[tokio::test]
    async fn staging_does_not_import_a_portably_deleted_entity() {
        let lab = Lab::new();
        let old = lab.0.join("old");
        let state = lab.replica("a", &old).await;
        seed(&state).await;
        let target = incoming(&lab, false);
        let file = target.join("notes/document-0.md");
        let text = std::fs::read_to_string(&file).unwrap();
        let id = portable::identity(
            &parse_okf_doc(&text),
            "notes/document-0.md",
            &okf_hash(&text),
        )
        .unwrap();
        portable_deletions::record_deleted(&target, &id).unwrap();
        std::fs::write(
            app_data_dir(&state).join("test-bind-import-before-binding"),
            "1",
        )
        .unwrap();
        assert!(
            bind_folder(&state, "shared-notebook", target.to_str().unwrap())
                .await
                .is_err()
        );
        assert_eq!(
            state.db.list_notes("shared-notebook").await.unwrap().len(),
            2
        );
        assert_eq!(std::fs::read_to_string(file).unwrap(), text);
        assert!(!target.join("sync/protocol.json").exists());
    }

    #[tokio::test]
    async fn paused_target_edit_resumes_one_row_and_preserves_local_conflict() {
        let lab = Lab::new();
        let old = lab.0.join("old");
        let state = lab.replica("a", &old).await;
        seed(&state).await;
        let old_snapshot = originals(&old);
        let target = incoming(&lab, false);
        std::fs::write(app_data_dir(&state).join("test-bind-import-after-row"), "1").unwrap();
        assert!(
            bind_folder(&state, "shared-notebook", target.to_str().unwrap())
                .await
                .is_err()
        );
        let id = state
            .db
            .list_notes("shared-notebook")
            .await
            .unwrap()
            .into_iter()
            .find(|note| note.id != "existing-note")
            .unwrap()
            .id;
        state
            .db
            .update_note(&id, "Local title", "Local edit made while paused", now_ms())
            .await
            .unwrap();
        let changed = target.join("notes/document-0.md");
        std::fs::write(&changed, "# A later external edit\n").unwrap();
        std::fs::File::open(&changed)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new().set_modified(
                    std::time::SystemTime::now() + std::time::Duration::from_secs(10),
                ),
            )
            .unwrap();
        assert!(!target.join("conflicts").exists());
        bind_folder(&state, "shared-notebook", target.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            state.db.list_notes("shared-notebook").await.unwrap().len(),
            3
        );
        assert_eq!(
            state
                .db
                .get_note(&id)
                .await
                .unwrap()
                .unwrap()
                .content
                .trim(),
            "# A later external edit"
        );
        assert!(std::fs::read_to_string(&changed)
            .unwrap()
            .contains("# A later external edit"));
        let conflicts: Vec<_> = std::fs::read_dir(target.join("conflicts"))
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect();
        assert!(conflicts
            .iter()
            .any(|text| text.contains("Local edit made while paused")));
        assert_eq!(originals(&old), old_snapshot);
    }

    #[tokio::test]
    async fn binding_publication_interruption_and_cancel_retry_keep_the_session_identity() {
        for stage in ["before-binding", "after-binding"] {
            let lab = Lab::new();
            let old = lab.0.join("old");
            let state = lab.replica("a", &old).await;
            seed(&state).await;
            let target = incoming(&lab, false);
            std::fs::write(
                app_data_dir(&state).join(format!("test-bind-import-{stage}")),
                "1",
            )
            .unwrap();
            assert!(
                bind_folder(&state, "shared-notebook", target.to_str().unwrap())
                    .await
                    .is_err()
            );
            let record = read(&session_path(
                &app_data_dir(&state),
                "shared-notebook",
                &same_folder(&target),
            ))
            .unwrap()
            .unwrap();
            if stage == "before-binding" {
                cancel_imports(&app_data_dir(&state), "shared-notebook").unwrap();
                assert!(
                    !blocks_existing_write(&app_data_dir(&state), "shared-notebook", "a").unwrap()
                );
            }
            bind_folder(&state, "shared-notebook", target.to_str().unwrap())
                .await
                .unwrap();
            assert_eq!(
                binding_for_checked(&app_data_dir(&state), "shared-notebook")
                    .unwrap()
                    .unwrap()
                    .id,
                record.binding_id
            );
            assert_eq!(
                state.db.list_notes("shared-notebook").await.unwrap().len(),
                3
            );
        }
    }
}
