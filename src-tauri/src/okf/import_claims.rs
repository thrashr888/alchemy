//! Initial import/binding migration only. Ordinary reconciliation must never
//! deduplicate a newly created file merely because its text matches a row.
use super::*;

pub(super) async fn adopt_imported_files(
    state: &AppState,
    notebook_id: &str,
    bundle: &Path,
    manifest_at: &Path,
) -> Result<(), String> {
    let lock = notebook_sync_lock(state, notebook_id);
    let _guard = lock.lock().await;
    let mut manifest = load_manifest_checked(manifest_at)?;
    let (_, sources, notes) = gather_bundle_for(state, notebook_id, bundle).await?;
    claim_imported_files(bundle, &mut manifest, &sources, &notes)?;
    save_manifest_checked(manifest_at, &manifest)
}

/// Validate every pairing before changing the record. Both the row and its
/// file must be unique; matching the first of two equal rows/files would
/// silently decide which document owns the other one's edits and deletion.
fn claim_imported_files(
    bundle: &Path,
    manifest: &mut OkfManifest,
    sources: &[OkfConcept],
    notes: &[OkfConcept],
) -> Result<(), String> {
    let mut claims = Vec::new();
    let mut matched_ids = std::collections::HashSet::new();
    for (dir, concepts) in [("sources", sources), ("notes", notes)] {
        for path in concept_files(bundle, dir) {
            let rel = path
                .strip_prefix(bundle)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if manifest.concepts.values().any(|entry| entry.path == rel) {
                continue;
            }
            // Keep the clock from before the read. A remote save during this
            // pass must remain visible to the following reconciliation.
            let (mtime, len) = file_clock(&path);
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("Couldn't read imported document {rel}: {error}"))?;
            let doc = parse_okf_doc(&text);
            if doc.body.trim().is_empty() {
                continue;
            }
            let title = doc.str("title").unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string()
            });
            let expected_type = if dir == "notes" {
                doc.nested("alchemy", "kind").unwrap_or_else(|| {
                    crate::commands::note_kind_from_label(
                        doc.str("type").as_deref().unwrap_or("Note"),
                    )
                })
            } else {
                const SOURCE_TYPES: &[&str] =
                    &["pdf", "text", "markdown", "html", "url", "image", "mac"];
                doc.nested("alchemy", "source_type")
                    .into_iter()
                    .chain(doc.tags())
                    .find(|kind| SOURCE_TYPES.contains(&kind.as_str()))
                    .unwrap_or_else(|| "text".into())
            };
            let type_key = if dir == "notes" {
                "kind"
            } else {
                "source_type"
            };
            let matches: Vec<_> = concepts.iter().filter(|concept| {
                !manifest.concepts.contains_key(&concept.id)
                    && concept.title == title
                    // Import normalizes only surrounding whitespace. Preserve
                    // every interior byte when identifying the landed row.
                    && concept.content.trim() == doc.body.trim()
                    && concept.alchemy.iter().any(|(key, value)| key == type_key && value == &expected_type)
            }).collect();
            if matches.len() != 1 {
                return Err(format!(
                    "Couldn't safely match imported document {rel} to one notebook item ({} matches). Its file was left unchanged.",
                    matches.len()
                ));
            }
            let concept = matches[0];
            if !matched_ids.insert(concept.id.clone()) {
                return Err(format!(
                    "More than one imported file matches the same notebook item, including {rel}. The files were left unchanged."
                ));
            }
            claims.push((concept, rel, okf_hash(&text), mtime, len, doc));
        }
    }
    for (concept, rel, hash, mtime, len, doc) in claims {
        adopt(manifest, &concept.id, &rel, &hash, mtime, len, &doc);
        if let Some(entry) = manifest.concepts.get_mut(&concept.id) {
            entry.local_hash = local_concept_hash(concept);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str) -> OkfConcept {
        OkfConcept {
            id: id.into(),
            title: "Original title".into(),
            content: "Exact body".into(),
            type_label: "Note".into(),
            alchemy: vec![("kind".into(), "note".into())],
            ..OkfConcept::blank()
        }
    }

    fn file(bundle: &Path, name: &str, body: &str) -> PathBuf {
        let dir = bundle.join("notes");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!("---\ntitle: Original title\ntype: Note\n---\n{body}\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn initial_import_claims_noncanonical_path_and_writer_leaves_bytes_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "outside-name.md", "Exact body");
        let before = std::fs::read(&path).unwrap();
        let mut manifest = OkfManifest::default();
        let notes = [note("local-id")];
        claim_imported_files(dir.path(), &mut manifest, &[], &notes).unwrap();
        let entry = manifest.concepts.get("local-id").unwrap();
        assert_eq!(entry.path, "notes/outside-name.md");
        assert_eq!(entry.local_hash, local_concept_hash(&notes[0]));
        claim_imported_files(dir.path(), &mut manifest, &[], &notes).unwrap();
        assert_eq!(manifest.concepts.len(), 1);
        let manifest_at = dir.path().join("record.json");
        save_manifest_checked(&manifest_at, &manifest).unwrap();
        let notebook = OkfNotebook {
            id: "nb".into(),
            title: "Notebook".into(),
            color: String::new(),
            icon: String::new(),
            generated_at: 1,
        };
        write_bundle(&notebook, &[], &notes, dir.path(), Some(&manifest_at)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!dir.path().join("notes/original-title.md").exists());
    }

    #[test]
    fn duplicate_rows_or_files_never_receive_arbitrary_claims() {
        let dir = tempfile::tempdir().unwrap();
        file(dir.path(), "one.md", "Exact body");
        let mut manifest = OkfManifest::default();
        assert!(
            claim_imported_files(dir.path(), &mut manifest, &[], &[note("a"), note("b")]).is_err()
        );
        assert!(manifest.concepts.is_empty());
        file(dir.path(), "two.md", "Exact body");
        assert!(claim_imported_files(dir.path(), &mut manifest, &[], &[note("a")]).is_err());
        assert!(manifest.concepts.is_empty());
    }

    #[test]
    fn unmatched_file_blocks_claims_without_changing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "different.md", "Different body");
        let before = std::fs::read(&path).unwrap();
        let mut manifest = OkfManifest::default();
        assert!(claim_imported_files(dir.path(), &mut manifest, &[], &[note("a")]).is_err());
        assert!(manifest.concepts.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
