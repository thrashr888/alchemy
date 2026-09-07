//! Live-folder discovery uses the same journaled importer as later sync.
//! Keep the reservation after completion: a stale folder must not resurrect
//! a notebook that the user deleted, or rebind one they explicitly unbound.
use super::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
enum Phase {
    Reserved,
    Creating,
    Created,
    Bound,
    Complete,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Reservation {
    folder: PathBuf,
    notebook: Notebook,
    binding_id: String,
    phase: Phase,
}

fn record_path(data_dir: &Path, folder: &Path) -> PathBuf {
    data_dir.join("okf-discovery").join(format!(
        "{}.json",
        okf_hash(&same_folder(folder).to_string_lossy())
    ))
}

fn load(path: &Path) -> Result<Option<Reservation>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|err| {
            format!(
                "Could not read discovery reservation {}: {err}",
                path.display()
            )
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!(
            "Could not read discovery reservation {}: {err}",
            path.display()
        )),
    }
}

fn save(path: &Path, reservation: &Reservation) -> Result<(), String> {
    use std::io::Write;
    let bytes = serde_json::to_vec_pretty(reservation).map_err(|err| err.to_string())?;
    let result = (|| -> std::io::Result<()> {
        let dir = path.parent().expect("discovery records have a parent");
        std::fs::create_dir_all(dir)?;
        let mut staged = tempfile::NamedTempFile::new_in(dir)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        staged.persist(path).map_err(|err| err.error)?;
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    })();
    result.map_err(|err| {
        format!(
            "Could not save discovery reservation {}: {err}",
            path.display()
        )
    })
}

pub(super) fn has_reservation(data_dir: &Path, folder: &Path) -> Result<bool, String> {
    Ok(load(&record_path(data_dir, folder))?.is_some())
}

pub(super) fn unfinished_folders(data_dir: &Path, root: &Path) -> Result<Vec<PathBuf>, String> {
    let dir = data_dir.join("okf-discovery");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("Could not read discovery reservations: {err}")),
    };
    let root = same_folder(root);
    let mut folders = Vec::new();
    for entry in entries {
        let path = entry.map_err(|err| err.to_string())?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        if let Some(record) = load(&path)? {
            if record.phase != Phase::Complete
                && record.folder.parent() == Some(root.as_path())
                && record.folder.is_dir()
            {
                folders.push(record.folder);
            }
        }
    }
    Ok(folders)
}

/// Reserve, create, bind, and reconcile one live bundle. Archive import is a
/// separate explicit operation; discovery must never mint fresh row identities
/// after an interrupted pass.
pub(crate) async fn discover_bundle(state: &AppState, folder: &Path) -> Result<String, String> {
    let folder = same_folder(folder);
    let data_dir = app_data_dir(state);
    let path = record_path(&data_dir, &folder);
    let lock = notebook_sync_lock(
        state,
        &format!("discovery-{}", okf_hash(&folder.to_string_lossy())),
    );
    let _guard = lock.lock().await;
    let existing = e(state.db.list_notebooks().await)?;
    let mut record = if let Some(record) = load(&path)? {
        if record.folder != folder {
            return Err("Discovery reservation names a different folder".into());
        }
        record
    } else {
        let index = std::fs::read_to_string(folder.join("index.md"))
            .map_err(|err| format!("Could not read discovered notebook: {err}"))?;
        let doc = parse_okf_doc(&index);
        let title = doc
            .str("title")
            .or_else(|| {
                doc.body
                    .lines()
                    .find_map(|line| line.strip_prefix("# ").map(str::trim).map(str::to_owned))
            })
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| {
                folder
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into()
            });
        let id = doc
            .nested("alchemy", "id")
            .filter(|id| !id.is_empty())
            .unwrap_or_else(new_id);
        if existing.iter().any(|notebook| notebook.id == id) {
            return Err(
                "Discovered notebook already exists without a reservation; bind it explicitly"
                    .into(),
            );
        }
        if load_bindings_checked(&data_dir)?
            .values()
            .any(|binding| same_folder(&binding.path) == folder)
        {
            return Err("Discovered folder already belongs to another binding".into());
        }
        let ts = now_ms();
        let reservation = Reservation {
            folder: folder.clone(),
            notebook: Notebook {
                id,
                title: title.clone(),
                color: doc.nested("alchemy", "color").unwrap_or_else(|| {
                    crate::db::NOTEBOOK_PALETTE[existing.len() % crate::db::NOTEBOOK_PALETTE.len()]
                        .into()
                }),
                icon: doc
                    .nested("alchemy", "icon")
                    .unwrap_or_else(|| crate::commands::auto_notebook_icon(&title)),
                created_at: ts,
                updated_at: ts,
                status: String::new(),
                growth_web: false,
                source_count: 0,
                note_count: 0,
                report_count: 0,
            },
            binding_id: new_id(),
            phase: Phase::Reserved,
        };
        save(&path, &reservation)?;
        reservation
    };
    let notebook_id = record.notebook.id.clone();
    if declared_id(&folder).is_some_and(|id| id != notebook_id) {
        return Err("Reserved folder now declares a different notebook identity".into());
    }
    let notebook_lock = notebook_sync_lock(state, &notebook_id);
    let notebook_guard = notebook_lock.lock().await;
    let exists = e(state.db.list_notebooks().await)?
        .iter()
        .any(|notebook| notebook.id == notebook_id);
    if !exists {
        if e(state.db.was_deleted("notebook", &notebook_id))? {
            return Err("Reserved notebook was deleted; discovery will not recreate it".into());
        }
        if !matches!(record.phase, Phase::Reserved | Phase::Creating) {
            return Err("Reserved notebook is missing after creation began; refusing to recreate a possibly deleted notebook".into());
        }
        record.phase = Phase::Creating;
        save(&path, &record)?;
        #[cfg(test)]
        {
            let fault = data_dir.join("test-discovery-before-notebook");
            if fault.exists() {
                std::fs::remove_file(fault).map_err(|err| err.to_string())?;
                return Err("test interruption before discovered notebook creation".into());
            }
        }
        e(state.db.create_notebook(&record.notebook).await)?;
        #[cfg(test)]
        {
            let fault = data_dir.join("test-discovery-after-notebook");
            if fault.exists() {
                std::fs::remove_file(fault).map_err(|err| err.to_string())?;
                return Err("test interruption after discovered notebook creation".into());
            }
        }
    }
    if matches!(record.phase, Phase::Reserved | Phase::Creating) {
        record.phase = Phase::Created;
        save(&path, &record)?;
    }
    let manifest_at = manifest_path(&data_dir, &record.binding_id);
    if record.phase == Phase::Created {
        let manifest = load_manifest_checked(&manifest_at)?;
        save_manifest_checked(&manifest_at, &manifest)?;
    }
    update_bindings_checked(&data_dir, |bindings| {
        if let Some(binding) = bindings.get(&notebook_id) {
            if binding.id != record.binding_id || same_folder(&binding.path) != folder {
                return Err(
                    "Reserved notebook now has a different binding; discovery stopped".into(),
                );
            }
        } else {
            if matches!(record.phase, Phase::Bound | Phase::Complete) {
                return Err(
                    "Reserved notebook was unbound; discovery will not restore its binding".into(),
                );
            }
            if bindings
                .values()
                .any(|binding| same_folder(&binding.path) == folder)
            {
                return Err("Reserved folder now belongs to another notebook".into());
            }
            bindings.insert(
                notebook_id.clone(),
                OkfBinding {
                    path: folder.to_string_lossy().into(),
                    id: record.binding_id.clone(),
                    last_write_at: 0,
                    lost: false,
                },
            );
        }
        Ok(())
    })?;
    if record.phase == Phase::Complete {
        return Ok(notebook_id);
    }
    record.phase = Phase::Bound;
    save(&path, &record)?;
    drop(notebook_guard);
    reconcile(state, &notebook_id).await?;
    record.phase = Phase::Complete;
    save(&path, &record)?;
    Ok(notebook_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::okf::sync_tests::Lab;

    fn bundle(lab: &Lab, identified: bool) -> PathBuf {
        let folder = lab.0.join("incoming");
        std::fs::create_dir_all(folder.join("notes")).unwrap();
        std::fs::write(folder.join("index.md"), format!("---\ntitle: Imported notebook\nalchemy:\n{}  color: teal\n  icon: FlaskConical\n---\n# Legacy title\n", if identified { "  id: imported-notebook\n" } else { "" })).unwrap();
        for i in 0..3 {
            std::fs::write(
                folder.join(format!("notes/note-{i}.md")),
                format!("---\ntitle: Note {i}\nalchemy:\n  kind: audio_overview\n---\nBody {i}\n"),
            )
            .unwrap();
        }
        folder
    }

    #[tokio::test]
    async fn discovery_resumes_after_notebook_creation_without_minting_another_id() {
        for identified in [false, true] {
            let lab = Lab::new();
            let folder = bundle(&lab, identified);
            let a = lab.replica("a", &lab.0.join("existing")).await;
            std::fs::write(app_data_dir(&a).join("test-discovery-after-notebook"), "").unwrap();
            assert!(discover_bundle(&a, &folder).await.is_err());
            let reserved = load(&record_path(&app_data_dir(&a), &folder))
                .unwrap()
                .unwrap();
            drop(a);
            let a = lab.replica("a", &lab.0.join("existing")).await;
            let id = discover_bundle(&a, &folder).await.unwrap();
            assert_eq!(id, reserved.notebook.id);
            assert_eq!(a.db.list_notebooks().await.unwrap().len(), 2);
            assert_eq!(a.db.list_notes(&id).await.unwrap().len(), 3);
            let notebook =
                a.db.list_notebooks()
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|notebook| notebook.id == id)
                    .unwrap();
            assert_eq!(
                (
                    notebook.title.as_str(),
                    notebook.color.as_str(),
                    notebook.icon.as_str()
                ),
                ("Imported notebook", "teal", "FlaskConical")
            );
            if identified {
                assert_eq!(id, "imported-notebook");
            }
        }
    }

    #[tokio::test]
    async fn discovery_resumes_first_row_with_the_same_binding_and_identity() {
        let lab = Lab::new();
        let folder = bundle(&lab, false);
        let a = lab.replica("a", &lab.0.join("existing")).await;
        std::fs::write(app_data_dir(&a).join("test-interrupt-import"), "").unwrap();
        assert!(discover_bundle(&a, &folder).await.is_err());
        let record = load(&record_path(&app_data_dir(&a), &folder))
            .unwrap()
            .unwrap();
        let first = a.db.list_notes(&record.notebook.id).await.unwrap()[0]
            .id
            .clone();
        assert_eq!(
            unfinished_folders(&app_data_dir(&a), &lab.0).unwrap(),
            vec![same_folder(&folder)]
        );
        drop(a);
        let a = lab.replica("a", &lab.0.join("existing")).await;
        let id = discover_bundle(&a, &folder).await.unwrap();
        let notes = a.db.list_notes(&id).await.unwrap();
        assert_eq!(notes.len(), 3);
        assert!(notes.iter().any(|note| note.id == first));
        assert_eq!(
            binding_for_checked(&app_data_dir(&a), &id)
                .unwrap()
                .unwrap()
                .id,
            record.binding_id
        );
        assert!(unfinished_folders(&app_data_dir(&a), &lab.0)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn preinsert_interruption_resumes_but_recorded_deletion_does_not() {
        for deleted in [false, true] {
            let lab = Lab::new();
            let folder = bundle(&lab, true);
            let a = lab.replica("a", &lab.0.join("existing")).await;
            let fault = if deleted {
                "test-discovery-after-notebook"
            } else {
                "test-discovery-before-notebook"
            };
            std::fs::write(app_data_dir(&a).join(fault), "").unwrap();
            assert!(discover_bundle(&a, &folder).await.is_err());
            if deleted {
                a.db.delete_notebook("imported-notebook").await.unwrap();
            }
            drop(a);
            let a = lab.replica("a", &lab.0.join("existing")).await;
            let result = discover_bundle(&a, &folder).await;
            if deleted {
                assert!(result.is_err());
                assert_eq!(a.db.list_notebooks().await.unwrap().len(), 1);
            } else {
                assert_eq!(result.unwrap(), "imported-notebook");
                assert_eq!(a.db.list_notes("imported-notebook").await.unwrap().len(), 3);
            }
        }
    }

    #[tokio::test]
    async fn completed_reservation_does_not_recreate_deleted_or_unbound_notebooks() {
        let lab = Lab::new();
        let folder = bundle(&lab, true);
        let a = lab.replica("a", &lab.0.join("existing")).await;
        let id = discover_bundle(&a, &folder).await.unwrap();
        set_binding_checked(&app_data_dir(&a), &id, None).unwrap();
        assert!(discover_bundle(&a, &folder).await.is_err());
        assert!(binding_for_checked(&app_data_dir(&a), &id)
            .unwrap()
            .is_none());
        a.db.delete_notebook(&id).await.unwrap();
        assert!(discover_bundle(&a, &folder).await.is_err());
        assert_eq!(a.db.list_notebooks().await.unwrap().len(), 1);
    }
}
