//! A sync claim keyed to a row the notebook no longer has is not always a
//! lost row. The 2026-09-03 folder doubling and the consolidation after it
//! left records whose ids named rows that had been removed without a
//! deletion receipt — while the same documents sat in the same notebook
//! under other ids, and the nightly copy's record swapped folders with a
//! twin's every time the notebook table was rewritten. `missing_rows` held
//! every export of those notebooks, correctly, for a week: it could not tell
//! a claim whose row was gone from a claim whose row had merely been
//! re-keyed. This pass can.
//!
//! The rule: a claim whose row is gone, with no deletion recorded anywhere,
//! belongs to the live row of the same kind in the same notebook that has
//! no claim of its own and whose title the writer would place at that very
//! path. The claim is re-keyed to that row, and the file is rewritten with
//! the row's identity on the next write. When the file's text is not the
//! row's, the file's version goes under `conflicts/` first (§5.4), unless
//! the id the claim named is still a live row in another notebook — then
//! the text is not the last copy of anything and the claim simply moves. A
//! claim naming a row that lives in another notebook and has no counterpart
//! here is left for the writer, which retires the file the way it retires
//! any concept that left. Only a claim with neither still holds the export.
use super::*;
use std::collections::HashSet;

/// How a stale claim's file compared with the row that took it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Match {
    /// The bodies are the same bytes, trailing whitespace aside.
    Exact,
    /// The same words with different whitespace between them.
    Normalized,
    /// Different text: the file's version is kept under `conflicts/`.
    Conflict,
    /// The claimed file is not on disk; there is nothing to compare.
    Absent,
}

impl Match {
    fn of(file_body: Option<&str>, row_body: &str) -> Self {
        let Some(file_body) = file_body else {
            return Match::Absent;
        };
        if file_body.trim_end() == row_body.trim_end() {
            Match::Exact
        } else if file_body.split_whitespace().eq(row_body.split_whitespace()) {
            Match::Normalized
        } else {
            Match::Conflict
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Match::Exact => "same text",
            Match::Normalized => "same text, different whitespace",
            Match::Conflict => "different text; the file's version is under conflicts/",
            Match::Absent => "file not on disk",
        }
    }
}

/// What one pass over a record did, for the log and the tests.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Adoption {
    /// Claims re-keyed to the live row that holds the same document.
    pub adopted: usize,
    /// Of those, claims whose file already had the row's text.
    pub identical: usize,
    /// Of those, claims whose file text was kept under `conflicts/`.
    pub conflicts: usize,
    /// Claims naming a row that lives in another notebook, with no
    /// counterpart here: left for the writer to retire.
    pub elsewhere: usize,
    /// Claims with no live counterpart anywhere. These still hold the export.
    pub orphaned: Vec<String>,
}

/// The body the writer would put in this concept's file (§5.3): a source's
/// text peeled of its document block, a note's text verbatim.
fn row_body(concept: &OkfConcept) -> &str {
    if concept.type_label == "Source" {
        peel_frontmatter(&concept.content).1
    } else {
        &concept.content
    }
}

async fn lives_elsewhere(
    state: &AppState,
    kind: &str,
    id: &str,
    notebook_id: &str,
) -> Result<bool, String> {
    let owner = if kind == "note" {
        e(state.db.get_note(id).await)?.map(|row| row.notebook_id)
    } else {
        e(state.db.get_source(id).await)?.map(|row| row.notebook_id)
    };
    Ok(owner.is_some_and(|owner| owner != notebook_id))
}

/// Re-key every claim in the record at `manifest_at` whose row this
/// notebook no longer has onto the live row that holds the same document,
/// and say which claims are left. The record is saved when anything moved.
/// Runs ahead of every write and every nightly copy (`missing_rows`), so a
/// nightly record that swapped folders with a twin's heals on the next pass
/// rather than holding the copy.
pub(super) async fn adopt(
    state: &AppState,
    notebook_id: &str,
    sources: &[OkfConcept],
    notes: &[OkfConcept],
    bundle: &Path,
    manifest_at: &Path,
) -> Result<(OkfManifest, Adoption), String> {
    let mut manifest = load_manifest_checked(manifest_at)?;
    let mut out = Adoption::default();
    let live: HashSet<&str> = sources
        .iter()
        .chain(notes)
        .map(|row| row.id.as_str())
        .collect();
    let live_paths: HashSet<&str> = manifest
        .concepts
        .iter()
        .filter(|(id, _)| live.contains(id.as_str()))
        .map(|(_, entry)| entry.path.as_str())
        .collect();
    // The writer discards obsolete aliases of a still-owned path without
    // deleting that path. They do not authorize a portable tombstone.
    let mut stale: Vec<(String, OkfManifestEntry)> = manifest
        .concepts
        .iter()
        .filter(|(id, entry)| {
            !live.contains(id.as_str()) && !live_paths.contains(entry.path.as_str())
        })
        .map(|(id, entry)| (id.clone(), entry.clone()))
        .collect();
    if stale.is_empty() {
        return Ok((manifest, out));
    }
    stale.sort_by(|a, b| a.1.path.cmp(&b.1.path));
    let mut deleted: Option<HashSet<String>> = None;
    // Rows that took a claim this pass; one claim per row.
    let mut taken: HashSet<String> = HashSet::new();
    let mut changed = false;
    for (id, entry) in stale {
        let (kind, dir, pool) = if entry.path.starts_with("notes/") {
            ("note", "notes", notes)
        } else if entry.path.starts_with("sources/") {
            ("source", "sources", sources)
        } else {
            return Err("Invalid concept path in notebook sync record".into());
        };
        if e(state.db.was_deleted(kind, &id))?
            || manifest.deleted_entities.contains(&entry.portable_id)
        {
            continue;
        }
        if deleted.is_none() {
            deleted = Some(portable_deletions::read_deleted(bundle)?);
        }
        if deleted
            .as_ref()
            .is_some_and(|ids| ids.contains(&entry.portable_id))
        {
            continue;
        }
        let path = bundle.join(&entry.path);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(format!("Could not read {}: {err}", entry.path)),
        };
        let file_body = text.as_deref().map(|text| peel_frontmatter(text).1);
        // The row this claim belongs to: unplaced, same kind, and titled so
        // the writer would put it at this path. Ties go to the row whose
        // text is the file's, then to the older row.
        let heir = pool
            .iter()
            .filter(|row| !manifest.concepts.contains_key(&row.id) && !taken.contains(&row.id))
            .filter(|row| keeps_slug(&entry.path, dir, &okf_slug(&row.title)))
            .map(|row| (Match::of(file_body, row_body(row)), row))
            .min_by(|a, b| {
                a.0.cmp(&b.0)
                    .then(a.1.generated_at.cmp(&b.1.generated_at))
                    .then(a.1.id.cmp(&b.1.id))
            });
        let Some((how, heir)) = heir else {
            if lives_elsewhere(state, kind, &id, notebook_id).await? {
                crate::note!(
                    "okf: {} is claimed for a row that now lives in another notebook; leaving it to the writer",
                    entry.path
                );
                out.elsewhere += 1;
            } else {
                out.orphaned.push(entry.path.clone());
            }
            continue;
        };
        if how == Match::Conflict && !lives_elsewhere(state, kind, &id, notebook_id).await? {
            conflicts::Context {
                bundle,
                rel: &entry.path,
                base_local_hash: "",
            }
            .preserve_remote(&parse_okf_doc(text.as_deref().unwrap_or_default()))?;
            out.conflicts += 1;
        }
        // The file keeps its claim, its identity and its observed clock —
        // the next read-back must not take a snapshot in over the row —
        // and loses its projection, so the next write puts the row's text
        // and id over it. A file that is not there is put back at once.
        let mut adopted = entry.clone();
        adopted.local_hash.clear();
        adopted.links_hash.clear();
        adopted.rewrite_pending = true;
        if text.is_some() {
            let (mtime, len) = file_clock(&path);
            adopted.file_mtime = mtime;
            adopted.file_len = len;
            adopted.missing_since = 0;
        } else {
            adopted.file_mtime = 0;
            adopted.file_len = 0;
            adopted.missing_since = 1;
        }
        manifest.concepts.remove(&id);
        manifest.outgoing.remove(&id);
        manifest.concepts.insert(heir.id.clone(), adopted);
        taken.insert(heir.id.clone());
        changed = true;
        out.adopted += 1;
        if matches!(how, Match::Exact | Match::Normalized) {
            out.identical += 1;
        }
        crate::note!(
            "okf: {} was claimed for a row this notebook no longer has; the claim now belongs to \u{201c}{}\u{201d} ({})",
            entry.path,
            heir.title,
            how.describe()
        );
    }
    if changed {
        save_manifest_checked(manifest_at, &manifest)?;
        okf_notice(format!(
            "adopted {} stale sync claim{} in {} onto the rows that hold the same documents ({} with the same text, {} with the file's version kept under conflicts/){}",
            out.adopted,
            if out.adopted == 1 { "" } else { "s" },
            bundle.display(),
            out.identical,
            out.conflicts,
            if out.orphaned.is_empty() {
                String::new()
            } else {
                format!(
                    "; {} still name{} no row anywhere: {}",
                    out.orphaned.len(),
                    if out.orphaned.len() == 1 { "s" } else { "" },
                    out.orphaned.join(", ")
                )
            }
        ));
    }
    Ok((manifest, out))
}

/// The folder each notebook's nightly copy gets, and so the record it is
/// kept under. Two notebooks may share a title; the slug must not collide,
/// and it must be stable across nights, so it is claimed in creation order.
/// It used to be claimed in table order, and a notebook's row moves to the
/// end of the table whenever it is rewritten — three notebooks called "458
/// Spider Purchase" swapped folders night to night, and each folder's
/// record ended up claiming another notebook's rows.
pub(super) fn nightly_slugs(notebooks: &[Notebook]) -> Vec<(&Notebook, String)> {
    let mut ordered: Vec<&Notebook> = notebooks.iter().collect();
    ordered.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let mut used: HashMap<String, u32> = HashMap::new();
    ordered
        .into_iter()
        .map(|nb| {
            let base = okf_slug(&nb.title);
            let count = used.entry(base.clone()).or_insert(0);
            *count += 1;
            let slug = if *count == 1 {
                base
            } else {
                format!("{base}-{count}")
            };
            (nb, slug)
        })
        .collect()
}

// ---- The launch pass --------------------------------------------------------

/// Bumped when the pass below has to run again on a store it already ran on.
const CLAIMS_HEAL_VERSION: &str = "1";

/// What the launch pass did across every record, for the log and the tests.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ClaimsHeal {
    /// Records that had a stale claim to look at.
    pub records: usize,
    pub adopted: usize,
    pub identical: usize,
    pub conflicts: usize,
    pub elsewhere: usize,
    pub orphaned: usize,
    /// Records no binding and no nightly copy reads any more, moved under
    /// `okf/orphaned/`.
    pub set_aside: usize,
    /// Records that could not be read or repaired this launch; the pass
    /// runs again.
    pub failed: usize,
}

/// Re-key stale claims in every record a live lineage reads — each
/// binding's, and each notebook's nightly copy's — and set aside the
/// records nothing reads any more, once per store under the versioned
/// marker `okf-claims-adopted`. `adopt` runs ahead of every write from then
/// on; this is for the exports that have been held since the folder
/// doubling, so they resume at launch rather than at the next write.
pub(crate) async fn heal_orphaned_claims(state: &AppState) {
    let data_dir = app_data_dir(state);
    let stamp = data_dir.join("okf-claims-adopted");
    if std::fs::read_to_string(&stamp).is_ok_and(|v| v.trim() == CLAIMS_HEAL_VERSION) {
        return;
    }
    let done = heal_orphaned_claims_checked(state).await;
    if done.adopted + done.elsewhere + done.orphaned + done.set_aside > 0 {
        okf_notice(format!(
            "looked at {} sync record{} with stale claims: {} claims adopted onto the rows that hold the same documents ({} same text, {} kept under conflicts/), {} left for the writer as rows of another notebook, {} still naming no row anywhere; {} record{} nothing reads any more set aside under okf/orphaned/",
            done.records,
            if done.records == 1 { "" } else { "s" },
            done.adopted,
            done.identical,
            done.conflicts,
            done.elsewhere,
            done.orphaned,
            done.set_aside,
            if done.set_aside == 1 { "" } else { "s" },
        ));
    }
    if done.failed == 0 {
        if let Err(err) = std::fs::write(&stamp, CLAIMS_HEAL_VERSION) {
            crate::note!("okf: couldn't stamp the claims heal: {err}");
        }
    }
}

pub(crate) async fn heal_orphaned_claims_checked(state: &AppState) -> ClaimsHeal {
    let data_dir = app_data_dir(state);
    let mut done = ClaimsHeal::default();
    let Ok(bindings) = load_bindings_checked(&data_dir) else {
        done.failed += 1;
        return done;
    };
    let Ok(notebooks) = state.db.list_notebooks().await else {
        done.failed += 1;
        return done;
    };
    // Every record a live lineage still reads: a binding's — active, detached
    // and resumable, or mid-import — and each current notebook's nightly copy's.
    let mut referenced: HashSet<String> = bindings.values().map(|b| b.id.clone()).collect();
    match bindings::detached_ids(&data_dir) {
        Ok(ids) => referenced.extend(ids),
        Err(_) => done.failed += 1,
    }
    match bind_import::session_binding_ids(&data_dir) {
        Ok(ids) => referenced.extend(ids),
        Err(_) => done.failed += 1,
    }
    let nightly_root = crate::backup::okf_latest_dir(&data_dir);
    let mut targets: Vec<(String, PathBuf, String)> = bindings
        .iter()
        .filter(|(notebook, _)| notebooks.iter().any(|n| &n.id == *notebook))
        .map(|(notebook, b)| (notebook.clone(), PathBuf::from(&b.path), b.id.clone()))
        .collect();
    for (nb, slug) in nightly_slugs(&notebooks) {
        let record = format!("nightly-{slug}");
        referenced.insert(record.clone());
        targets.push((nb.id.clone(), nightly_root.join(&slug), record));
    }
    for (notebook_id, bundle, record) in targets {
        let manifest_at = manifest_path(&data_dir, &record);
        if !manifest_at.exists() {
            continue;
        }
        match heal_record(state, &notebook_id, &bundle, &manifest_at).await {
            Ok(Some(adoption)) => {
                done.records += 1;
                done.adopted += adoption.adopted;
                done.identical += adoption.identical;
                done.conflicts += adoption.conflicts;
                done.elsewhere += adoption.elsewhere;
                done.orphaned += adoption.orphaned.len();
                // The files still carry the old identity; the next write
                // puts the rows' over them.
                if adoption.adopted > 0 && bindings.contains_key(&notebook_id) {
                    schedule_write(&notebook_id);
                }
            }
            Ok(None) => {}
            Err(err) => {
                done.failed += 1;
                crate::note!(
                    "okf: couldn't repair the sync record for {}: {err}",
                    bundle.display()
                );
            }
        }
    }
    match set_aside_unread(&data_dir, &referenced) {
        Ok(moved) => done.set_aside += moved,
        Err(err) => {
            done.failed += 1;
            crate::note!("okf: couldn't set aside unread sync records: {err}");
        }
    }
    done
}

/// One record. Ids first: the full gather reads every source's text, and
/// is only worth it when a claim is actually stale.
async fn heal_record(
    state: &AppState,
    notebook_id: &str,
    bundle: &Path,
    manifest_at: &Path,
) -> Result<Option<Adoption>, String> {
    let lock = notebook_sync_lock(state, notebook_id);
    let _guard = lock.lock().await;
    let manifest = load_manifest_checked(manifest_at)?;
    if manifest.concepts.is_empty() {
        return Ok(None);
    }
    let live: HashSet<String> = e(state.db.list_sources(notebook_id).await)?
        .into_iter()
        .map(|s| s.id)
        .chain(
            e(state.db.list_notes(notebook_id).await)?
                .into_iter()
                .map(|n| n.id),
        )
        .collect();
    if manifest.concepts.keys().all(|id| live.contains(id)) {
        return Ok(None);
    }
    let (_, sources, notes) = gather_bundle_for(state, notebook_id, bundle).await?;
    adopt(state, notebook_id, &sources, &notes, bundle, manifest_at)
        .await
        .map(|(_, adoption)| Some(adoption))
}

/// Records under `okf/` that no binding and no nightly copy reads: moved
/// under `okf/orphaned/` with their markers, never deleted. Three of them
/// sat beside the 458 notebook's — one per lineage of the doubling — and
/// each named rows no notebook has; a restore is one `mv` back.
fn set_aside_unread(data_dir: &Path, referenced: &HashSet<String>) -> Result<usize, String> {
    let dir = data_dir.join("okf");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(format!("Could not list {}: {err}", dir.display())),
    };
    let aside = dir.join("orphaned");
    let mut moved = 0;
    let mut stems: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_string)
        })
        .filter(|stem| !referenced.contains(stem))
        .collect();
    stems.sort();
    for stem in stems {
        std::fs::create_dir_all(&aside)
            .map_err(|err| format!("Could not create {}: {err}", aside.display()))?;
        for ext in ["json", "initialized", "stamped-v2"] {
            let from = dir.join(format!("{stem}.{ext}"));
            if !from.exists() {
                continue;
            }
            let to = aside.join(format!("{stem}.{ext}"));
            std::fs::rename(&from, &to).map_err(|err| {
                format!(
                    "Could not move {} to {}: {err}",
                    from.display(),
                    to.display()
                )
            })?;
        }
        crate::note!(
            "okf: set aside the sync record {stem}.json \u{2014} no notebook binding or nightly copy reads it; it is under okf/orphaned/"
        );
        moved += 1;
    }
    Ok(moved)
}
