//! Open Knowledge Format: the writer, the reader, and the binding that keeps
//! a notebook on disk as a bundle (docs/RFC-okf-live.md).
//!
//! One format, three jobs. **Export** writes a notebook out as markdown
//! concept files — the share verb, and the nightly escape hatch that survives
//! a Lance-format problem. **Bundle-as-a-source** reads someone else's corpus
//! as a living folder (§4; the ingest side of that lives in `commands.rs`
//! beside the rest of the folder pipeline). **A bound notebook** (§5) keeps
//! its own bundle current: every mutation schedules a debounced write, and
//! the bundle is the notebook's shared surface — an agent edits a note by
//! editing a file.
//!
//! Two pieces of state make the binding work, and both are deliberately not
//! store columns. `<app-data>/okf-bindings.json` maps notebook id to bundle
//! path: paths are per-machine, and a column would sync them somewhere they
//! mean nothing. `<bundle>/.alchemy/manifest.json` maps entity id to the file
//! Alchemy wrote for it, the hash of what it wrote, and the frontmatter keys
//! that came from outside and must go back out untouched. The manifest lives
//! in the bundle because it describes the bundle; a dot-directory is not a
//! concept document (spec §3.1), so every other tool skips it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, State};

use crate::commands::{app_data_dir, e, is_web_url, new_id, AppState};
use crate::ingest;
use crate::models::{Note, Source};
use crate::rag;

// ---- OKF export ------------------------------------------------------------

/// Kebab-case a title into a filesystem/URL-safe slug.
pub(crate) fn okf_slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(60).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "untitled".into()
    } else {
        out
    }
}

/// Double-quote a string for YAML frontmatter.
fn yaml_str(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', " ")
    )
}

/// First ~140 chars of content, flattened, for `description:` and index lines.
pub(crate) fn okf_description(content: &str) -> String {
    let flat = content
        .replace(['#', '*', '`', '>', '|'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out: String = flat.chars().take(140).collect();
    if flat.chars().count() > 140 {
        out.push('…');
    }
    out
}

fn okf_timestamp(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// Titles go into markdown link text; keep them from breaking the link.
fn link_text(s: &str) -> String {
    s.replace(['[', ']'], " ").trim().to_string()
}

/// Who wrote a concept file: the `generated.by` actor, and the attribution
/// on every `log.md` entry (OKF v0.2 §5.2).
pub(crate) fn okf_writer() -> String {
    concat!("alchemy/", env!("CARGO_PKG_VERSION")).to_string()
}

/// One concept file's worth of what the bundle writer needs. Decoupled from
/// `Source`/`Note` so the writer runs — and is tested — without a database.
#[derive(Clone)]
pub(crate) struct OkfConcept {
    pub id: String,
    pub title: String,
    pub content: String,
    /// The human `type:` label: "Source", "Note", or an artifact's own title.
    pub type_label: String,
    /// `resource:` — where the concept came from. Empty writes no key.
    pub resource: String,
    /// `tags:` — a source's type, a note's kind. Empty writes no key.
    pub tags: Vec<String>,
    /// `generated.at`: `created_at` for sources, `updated_at` for notes.
    pub generated_at: i64,
    /// `status:` — "" | "draft" | "deprecated".
    pub status: String,
    /// `sources:` — ids of the concepts this one was derived from. Resolved
    /// to bundle-relative paths at write time, so an id that did not make it
    /// into the bundle simply does not appear.
    pub derived_from: Vec<String>,
    /// Frontmatter keys Alchemy does not itself write, carried in from an
    /// outside edit and re-emitted verbatim — the spec's round-trip rule.
    pub extra: serde_yaml_ng::Mapping,
}

impl OkfConcept {
    /// The same concept carrying the frontmatter keys someone else put in its
    /// file, so a write re-emits them instead of dropping them.
    fn clone_with_extra(&self, extra: serde_yaml_ng::Mapping) -> Self {
        Self {
            extra: if extra.is_empty() {
                self.extra.clone()
            } else {
                extra
            },
            ..self.clone()
        }
    }

    fn blank() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            content: String::new(),
            type_label: "Note".into(),
            resource: String::new(),
            tags: Vec::new(),
            generated_at: 0,
            status: String::new(),
            derived_from: Vec::new(),
            extra: serde_yaml_ng::Mapping::new(),
        }
    }
}

/// What one bundle write did — the log line, and the caller's receipt.
#[derive(Debug, Default, PartialEq)]
pub struct OkfWrite {
    /// Concepts in the bundle after the write.
    pub sources: usize,
    pub notes: usize,
    /// Files this pass actually touched.
    pub written: usize,
    pub moved: usize,
    pub removed: usize,
}

impl OkfWrite {
    pub fn changed(&self) -> bool {
        self.written + self.moved + self.removed > 0
    }
}

/// A concept's place in the bundle: its slug and the path other concepts
/// cite it by.
struct OkfPlacement {
    slug: String,
    /// Bundle-relative, e.g. `sources/orders.md`.
    path: String,
    title: String,
}

/// Emit one concept's v0.2 frontmatter. Hand-written rather than serialized:
/// key order is part of the document's readability, and only the values need
/// escaping. Unknown keys go through `serde_yaml_ng` so nested maps and
/// sequences survive verbatim.
fn okf_frontmatter(
    concept: &OkfConcept,
    description: &str,
    placements: &std::collections::HashMap<String, OkfPlacement>,
) -> String {
    let mut fm = String::from("---\n");
    fm.push_str(&format!("type: {}\n", concept.type_label));
    fm.push_str(&format!("title: {}\n", yaml_str(&concept.title)));
    if !description.is_empty() {
        fm.push_str(&format!("description: {}\n", yaml_str(description)));
    }
    if !concept.resource.is_empty() {
        fm.push_str(&format!("resource: {}\n", yaml_str(&concept.resource)));
    }
    if !concept.tags.is_empty() {
        fm.push_str(&format!("tags: [{}]\n", concept.tags.join(", ")));
    }
    if !concept.status.is_empty() {
        fm.push_str(&format!("status: {}\n", concept.status));
    }
    fm.push_str("generated:\n");
    fm.push_str(&format!("  by: {}\n", yaml_str(&okf_writer())));
    fm.push_str(&format!(
        "  at: {}\n",
        yaml_str(&okf_timestamp(concept.generated_at))
    ));
    // Provenance: every concept in this bundle that the body actually refers
    // to. A reader can follow a summary back to what it summarized.
    let cited: Vec<&OkfPlacement> = concept
        .derived_from
        .iter()
        .filter_map(|id| placements.get(id))
        .collect();
    if !cited.is_empty() {
        fm.push_str("sources:\n");
        for place in cited {
            fm.push_str(&format!("  - id: {}\n", yaml_str(&place.slug)));
            fm.push_str(&format!("    resource: {}\n", yaml_str(&place.path)));
            fm.push_str(&format!("    title: {}\n", yaml_str(&place.title)));
        }
    }
    // Keys from an outside edit, re-emitted as they came in.
    if !concept.extra.is_empty() {
        if let Ok(text) = serde_yaml_ng::to_string(&concept.extra) {
            fm.push_str(&text);
            if !fm.ends_with('\n') {
                fm.push('\n');
            }
        }
    }
    fm.push_str(&format!(
        "timestamp: {}\n---\n\n",
        yaml_str(&okf_timestamp(concept.generated_at))
    ));
    fm
}

/// Append one dated entry to the bundle's `log.md` (spec §9). A day already
/// present gains a bullet; a new day gains a heading. The file is a history,
/// not a stamp — which is what makes a bundle rewritten every night worth
/// reading.
pub(crate) fn okf_log_append(bundle: &std::path::Path, entry: &str) -> Result<(), String> {
    let path = bundle.join("log.md");
    let now = chrono::Utc::now();
    let day = now.format("%Y-%m-%d").to_string();
    let at = now.format("%H:%M:%SZ").to_string();

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = if existing.trim().is_empty() {
        String::from("# Log\n")
    } else {
        existing
    };
    if !out.ends_with('\n') {
        out.push('\n');
    }
    // Entries arrive in order, so the newest day is always the last heading.
    if !out.contains(&format!("\n## {day}\n")) {
        out.push_str(&format!("\n## {day}\n\n"));
    }
    out.push_str(&format!("- {at} {entry} ({})\n", okf_writer()));
    std::fs::write(&path, out).map_err(|err| format!("Failed to write {path:?}: {err}"))
}

/// Write (or refresh) an OKF v0.2 bundle at `bundle`.
///
/// The manifest is what makes this incremental. A concept whose file hashes
/// the same as the record is skipped; a concept whose title re-slugs is
/// `rename`d, which git reads as a move rather than a delete and an add; and
/// only files the manifest says Alchemy wrote are ever removed, so a document
/// someone else left in the bundle stays. The three index listings regenerate
/// whole — they are cheap and deterministic — and `log.md` gains one dated
/// entry, but only when something actually changed.
///
/// Called with an empty manifest this is the seed pass: everything is new,
/// which is exactly what a first export means.
pub fn write_bundle(
    notebook_title: &str,
    sources: &[OkfConcept],
    notes: &[OkfConcept],
    bundle: &Path,
) -> Result<OkfWrite, String> {
    std::fs::create_dir_all(bundle).map_err(|err| format!("Failed to create {bundle:?}: {err}"))?;
    let write = |path: &Path, text: &str| -> Result<(), String> {
        std::fs::write(path, text).map_err(|err| format!("Failed to write {path:?}: {err}"))
    };
    let mut manifest = load_manifest(bundle);
    let mut out = OkfWrite::default();

    // Slugs are claimed per directory, so two sources called "Notes" become
    // notes.md and notes-2.md rather than one clobbering the other.
    let mut used: HashMap<String, u32> = HashMap::new();
    let mut claim = |dir: &str, title: &str| -> String {
        let s = okf_slug(title);
        let count = used.entry(format!("{dir}/{s}")).or_insert(0);
        *count += 1;
        if *count == 1 {
            s
        } else {
            format!("{s}-{count}")
        }
    };

    // Place everything first: a note's `sources:` entries cite bundle paths,
    // which are only known once every slug is claimed.
    let mut placements: HashMap<String, OkfPlacement> = HashMap::new();
    let order: Vec<(&str, Vec<&OkfConcept>)> = vec![
        ("sources", sources.iter().collect()),
        ("notes", notes.iter().collect()),
    ];
    for (dir, concepts) in &order {
        for concept in concepts {
            let slug = claim(dir, &concept.title);
            placements.insert(
                concept.id.clone(),
                OkfPlacement {
                    path: format!("{dir}/{slug}.md"),
                    slug,
                    title: concept.title.clone(),
                },
            );
        }
    }

    let mut listings: HashMap<&str, Vec<(String, String, String)>> = HashMap::new();
    let mut still_ours: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (dir, concepts) in &order {
        if concepts.is_empty() {
            continue;
        }
        std::fs::create_dir_all(bundle.join(dir)).map_err(|err| err.to_string())?;
        let mut entries = Vec::new();
        for concept in concepts {
            let Some(place) = placements.get(&concept.id) else {
                continue;
            };
            let description = okf_description(&concept.content);
            // Unknown keys from an earlier outside edit ride back out on
            // every write, not just the one that read them.
            let mut concept = concept.clone_with_extra(
                manifest
                    .concepts
                    .get(&concept.id)
                    .map(|m| m.extra.clone())
                    .unwrap_or_default(),
            );
            let text = format!(
                "{}{}\n",
                okf_frontmatter(&concept, &description, &placements),
                concept.content
            );
            let hash = okf_hash(&text);
            let prior = manifest.concepts.get(&concept.id);
            // A retitled concept moves rather than being deleted and rewritten.
            if let Some(prior) = prior {
                if prior.path != place.path {
                    let from = bundle.join(&prior.path);
                    let to = bundle.join(&place.path);
                    if from.exists() && std::fs::rename(&from, &to).is_ok() {
                        out.moved += 1;
                    }
                }
            }
            let unchanged =
                prior.is_some_and(|p| p.hash == hash) && bundle.join(&place.path).exists();
            if !unchanged {
                write(&bundle.join(&place.path), &text)?;
                out.written += 1;
            }
            manifest.concepts.insert(
                concept.id.clone(),
                OkfManifestEntry {
                    path: place.path.clone(),
                    hash,
                    wrote_at: concept.generated_at,
                    extra: std::mem::take(&mut concept.extra),
                },
            );
            still_ours.insert(concept.id.clone());
            entries.push((place.slug.clone(), concept.title.clone(), description));
        }
        listings.insert(dir, entries);
    }

    // A concept the notebook no longer has takes its file with it — but only
    // if the manifest says the file was ours to begin with.
    let gone: Vec<String> = manifest
        .concepts
        .keys()
        .filter(|id| !still_ours.contains(*id))
        .cloned()
        .collect();
    for id in gone {
        if let Some(entry) = manifest.concepts.remove(&id) {
            if std::fs::remove_file(bundle.join(&entry.path)).is_ok() {
                out.removed += 1;
            }
        }
    }

    for (dir, heading) in [("sources", "Sources"), ("notes", "Notes")] {
        let Some(entries) = listings.get(dir) else {
            continue;
        };
        let listing = entries
            .iter()
            .map(|(slug, title, desc)| format!("- [{}]({slug}.md) — {desc}", link_text(title)))
            .collect::<Vec<_>>()
            .join("\n");
        write(
            &bundle.join(dir).join("index.md"),
            &format!("# {heading}\n\n{listing}\n"),
        )?;
    }

    // Root index.md: progressive-disclosure listing of the whole bundle.
    let mut index = format!("# {notebook_title}\n\n");
    index.push_str(
        "A research notebook exported from Alchemy as an Open Knowledge Format bundle.\n",
    );
    for (dir, heading) in [("sources", "Sources"), ("notes", "Notes")] {
        let Some(entries) = listings.get(dir) else {
            continue;
        };
        if entries.is_empty() {
            continue;
        }
        index.push_str(&format!("\n# {heading}\n\n"));
        for (slug, title, desc) in entries {
            index.push_str(&format!(
                "- [{}]({dir}/{slug}.md) — {desc}\n",
                link_text(title)
            ));
        }
    }
    write(&bundle.join("index.md"), &index)?;

    out.sources = listings.get("sources").map(Vec::len).unwrap_or(0);
    out.notes = listings.get("notes").map(Vec::len).unwrap_or(0);
    save_manifest(bundle, &manifest);
    // A pass that changed nothing says nothing: a log of "no change" every
    // night is not a history, it is noise.
    if out.changed() {
        let mut parts = Vec::new();
        if out.written > 0 {
            parts.push(format!("{} written", out.written));
        }
        if out.moved > 0 {
            parts.push(format!("{} moved", out.moved));
        }
        if out.removed > 0 {
            parts.push(format!("{} removed", out.removed));
        }
        okf_log_append(
            bundle,
            &format!(
                "{} \u{2014} {} sources, {} notes.",
                parts.join(", "),
                out.sources,
                out.notes
            ),
        )?;
    }
    Ok(out)
}

/// A note's lifecycle in the bundle (§3): the curator's archive is the
/// spec's `deprecated`, and anything the app wrote on its own initiative is
/// a `draft` until a person has touched it.
fn okf_note_status(note: &Note) -> String {
    if note.status == "archived" {
        "deprecated".into()
    } else if !note.origin.is_empty() {
        "draft".into()
    } else {
        String::new()
    }
}

/// Read a notebook out of the store as concept files waiting to be written.
///
/// **Provenance:** a `Note` records no source ids — the selection a
/// generation ran over lives on the in-flight `GenJob` and is discarded when
/// the job finishes (docs/RFC-okf-live.md §3, "what we know"). What is
/// recorded, in the note's own text, is which documents it refers to: the
/// same URLs, filenames, and wikilinks the link graph already reads. So
/// `sources:` is the graph's outbound source edges for each note — the
/// citations that are actually there, never a guess at what was in scope.
pub(crate) async fn gather_bundle(
    state: &AppState,
    notebook_id: &str,
) -> Result<(String, Vec<OkfConcept>, Vec<OkfConcept>), String> {
    let notebook = e(state.db.list_notebooks().await)?
        .into_iter()
        .find(|n| n.id == notebook_id)
        .ok_or_else(|| "Notebook not found".to_string())?;
    let sources = e(state.db.list_sources(notebook_id).await)?;
    let notes = e(state.db.list_notes(notebook_id).await)?;

    let mut source_concepts = Vec::with_capacity(sources.len());
    for s in &sources {
        let content = e(state.db.source_content(&s.id).await)?;
        let resource = if s.url.is_empty() {
            String::new()
        } else if is_web_url(&s.url) {
            s.url.clone()
        } else {
            format!("file://{}", s.url)
        };
        source_concepts.push(OkfConcept {
            id: s.id.clone(),
            title: s.title.clone(),
            content,
            type_label: "Source".into(),
            resource,
            tags: vec![s.source_type.clone()],
            generated_at: s.created_at,
            ..OkfConcept::blank()
        });
    }

    // One Aho-Corasick pass over the whole notebook, the same one the graph
    // view runs — cheap enough to do on every export.
    let docs: Vec<crate::graph::GraphDoc> = sources
        .iter()
        .zip(source_concepts.iter())
        .map(|(s, c)| crate::graph::GraphDoc {
            id: s.id.clone(),
            kind: "source".into(),
            title: s.title.clone(),
            source_type: s.source_type.clone(),
            url: s.url.clone(),
            content: c.content.clone(),
        })
        .chain(notes.iter().map(|n| crate::graph::GraphDoc {
            id: n.id.clone(),
            kind: "note".into(),
            title: n.title.clone(),
            source_type: String::new(),
            url: String::new(),
            content: n.content.clone(),
        }))
        .collect();
    let graph = crate::graph::build(&docs);
    let source_ids: std::collections::HashSet<&str> =
        sources.iter().map(|s| s.id.as_str()).collect();
    let mut cites: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for edge in &graph.edges {
        if source_ids.contains(edge.to.as_str()) {
            cites
                .entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
        }
    }

    let note_concepts = notes
        .iter()
        .map(|note| OkfConcept {
            id: note.id.clone(),
            title: note.title.clone(),
            content: note.content.clone(),
            type_label: match note.kind.as_str() {
                "note" => "Note",
                "report" => "Report",
                kind => rag::artifact_spec(kind).map(|(t, _)| t).unwrap_or("Note"),
            }
            .to_string(),
            generated_at: note.updated_at,
            status: okf_note_status(note),
            derived_from: cites.get(&note.id).cloned().unwrap_or_default(),
            ..OkfConcept::blank()
        })
        .collect();

    Ok((notebook.title, source_concepts, note_concepts))
}

/// Export a notebook as an Open Knowledge Format bundle: a directory of
/// markdown concept files with YAML frontmatter (sources/ and notes/), plus
/// index.md listings and a log.md — per the OKF v0.2 spec.
#[tauri::command]
pub async fn export_notebook_okf(
    state: State<'_, AppState>,
    notebook_id: String,
    dest_dir: String,
) -> Result<String, String> {
    let (title, sources, notes) = gather_bundle(&state, &notebook_id).await?;

    // A fresh directory per export — never merge into (or clobber) one the
    // user already has.
    let base = std::path::Path::new(&dest_dir);
    let nb_slug = okf_slug(&title);
    let mut bundle = base.join(&nb_slug);
    let mut n = 2;
    while bundle.exists() {
        bundle = base.join(format!("{nb_slug}-{n}"));
        n += 1;
    }
    write_bundle(&title, &sources, &notes, &bundle)?;
    Ok(bundle.display().to_string())
}

/// The nightly escape hatch (docs/RFC-night-shift-area.md §7): every notebook
/// written into `backups/okf/latest/<slug>/` as a v0.2 bundle. Markdown
/// survives a Lance-format problem entirely, which is the whole point. Each
/// night replaces the previous copy in place — except `log.md`, which
/// accumulates, so the directory says what changed and when.
pub(crate) async fn export_all(
    state: &AppState,
    dest: &std::path::Path,
) -> Result<(usize, usize), String> {
    std::fs::create_dir_all(dest).map_err(|err| format!("Failed to create {dest:?}: {err}"))?;
    let notebooks = e(state.db.list_notebooks().await)?;
    let mut concepts = 0usize;
    let mut kept: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut used: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for nb in &notebooks {
        // Two notebooks may share a title; the slug must not collide, and it
        // must be stable across nights, so it is claimed in list order.
        let base = okf_slug(&nb.title);
        let count = used.entry(base.clone()).or_insert(0);
        *count += 1;
        let slug = if *count == 1 {
            base
        } else {
            format!("{base}-{count}")
        };
        let (title, sources, notes) = gather_bundle(state, &nb.id).await?;
        let written = write_bundle(&title, &sources, &notes, &dest.join(&slug))?;
        concepts += written.sources + written.notes;
        kept.insert(slug);
    }
    // A notebook deleted since last night takes its copy with it.
    if let Ok(entries) = std::fs::read_dir(dest) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.')
                && !kept.contains(&name)
                && entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok((notebooks.len(), concepts))
}

/// Export the bundle and zip it into a single shareable `.okf.zip` file at
/// `dest_path` (the coworker / other-laptop case — one file to send, and
/// import_notebook_okf on the other side recreates the notebook).
#[tauri::command]
pub async fn export_notebook_okf_zip(
    state: State<'_, AppState>,
    notebook_id: String,
    dest_path: String,
) -> Result<String, String> {
    let staging = std::env::temp_dir().join(format!("alchemy-okf-export-{}", new_id()));
    std::fs::create_dir_all(&staging).map_err(|e2| e2.to_string())?;
    let bundle = export_notebook_okf(state, notebook_id, staging.display().to_string()).await?;
    let result = zip_dir(
        std::path::Path::new(&bundle),
        std::path::Path::new(&dest_path),
    );
    let _ = std::fs::remove_dir_all(&staging);
    result?;
    Ok(dest_path)
}

/// Zip a bundle directory (bundle-name-rooted entries, so unzipping yields
/// the folder, matching what the exporter writes on disk).
fn zip_dir(dir: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    use std::io::Write as _;
    let file = std::fs::File::create(dest).map_err(|e| format!("Failed to create zip: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = Default::default();
    let root_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("notebook")
        .to_string();
    fn walk(
        zip: &mut zip::ZipWriter<std::fs::File>,
        opts: zip::write::SimpleFileOptions,
        dir: &std::path::Path,
        prefix: &str,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            let entry_name = format!("{prefix}/{name}");
            if path.is_dir() {
                walk(zip, opts, &path, &entry_name)?;
            } else {
                zip.start_file(&entry_name, opts)
                    .map_err(|e| e.to_string())?;
                let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                zip.write_all(&bytes).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    walk(&mut zip, opts, dir, &root_name)?;
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

// ---- OKF as a source (docs/RFC-okf-live.md §4) ------------------------------

/// A bundle concept names itself: `title:` in the frontmatter beats whatever
/// the filename or the first heading would have given it (§4). True when the
/// frontmatter settled the title, so no model retitle is queued behind it.
pub(crate) fn okf_title_from_frontmatter(extracted: &mut ingest::Extracted) -> bool {
    match parse_okf_doc(&extracted.text).str("title") {
        Some(title) => {
            extracted.title = title;
            true
        }
        None => false,
    }
}

/// Bundle listings, not concepts (spec §3.1): `index.md` is a table of
/// contents and `log.md` is the bundle's history. Neither ingests, and
/// neither counts toward what the folder holds.
pub(crate) fn is_okf_reserved(path: &str) -> bool {
    matches!(
        std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str()),
        Some("index.md") | Some("log.md")
    )
}

/// What a concept file says about its own standing, beyond its prose.
/// Machine-local and derived from the file, so it lives in a sidecar rather
/// than a store column — the same shape `EmbedOverrides` uses for repo tiers.
#[derive(Clone, Default, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkfLifecycle {
    /// The spec's `status:` — "" (current) | "draft" | "deprecated".
    #[serde(default)]
    pub status: String,
    /// `stale_after` as epoch ms; 0 when the file names no expiry.
    #[serde(default)]
    pub stale_after: i64,
    /// Trust tier (spec §5.3): "" unverified | "machine" | "human".
    #[serde(default)]
    pub trust: String,
}

impl OkfLifecycle {
    /// Nothing worth showing — the common case, and not worth a sidecar row.
    fn is_plain(&self) -> bool {
        self.status.is_empty() && self.stale_after == 0 && self.trust.is_empty()
    }
}

/// Read a concept's lifecycle out of its frontmatter.
///
/// The trust tier is a reading of `verified:`, which the spec leaves as a
/// list of attestations without saying who counts as a machine. An actor
/// written `name/version` is a tool (that is the shape `generated.by` uses);
/// anything else is a person. A human review outranks a machine one.
pub(crate) fn okf_lifecycle_of(doc: &OkfDoc) -> OkfLifecycle {
    let mut out = OkfLifecycle {
        status: doc.str("status").unwrap_or_default(),
        stale_after: doc
            .str("stale_after")
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.timestamp_millis())
            .unwrap_or(0),
        trust: String::new(),
    };
    if let Some(serde_yaml_ng::Value::Sequence(entries)) = doc.get("verified") {
        for entry in entries {
            let by = entry.get("by").and_then(|v| v.as_str()).unwrap_or("");
            let tier = if by.contains('/') { "machine" } else { "human" };
            if out.trust != "human" {
                out.trust = tier.to_string();
            }
        }
    }
    out
}

fn okf_lifecycle_path(data_dir: &std::path::Path, parent_id: &str) -> std::path::PathBuf {
    data_dir
        .join("okf_lifecycle")
        .join(format!("{parent_id}.json"))
}

/// Every lifecycle-bearing child of one bundle source, by child source id.
pub(crate) fn load_okf_lifecycle(
    data_dir: &std::path::Path,
    parent_id: &str,
) -> std::collections::HashMap<String, OkfLifecycle> {
    std::fs::read_to_string(okf_lifecycle_path(data_dir, parent_id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_okf_lifecycle(
    data_dir: &std::path::Path,
    parent_id: &str,
    map: &std::collections::HashMap<String, OkfLifecycle>,
) {
    let path = okf_lifecycle_path(data_dir, parent_id);
    if map.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(path, json);
    }
}

/// Re-read every bundle child's frontmatter and record what it says about
/// itself. Runs at the end of an `okf` parent's scan, so the panel and the
/// reader have the lifecycle without paying to read each source's full text
/// on every listing.
pub(crate) async fn refresh_okf_lifecycle(state: &AppState, folder: &Source) {
    let Ok(sources) = state.db.list_sources(&folder.notebook_id).await else {
        return;
    };
    let mut map = std::collections::HashMap::new();
    for child in sources.iter().filter(|s| s.parent_id == folder.id) {
        let Ok(text) = std::fs::read_to_string(&child.url) else {
            continue;
        };
        let life = okf_lifecycle_of(&parse_okf_doc(&text));
        if !life.is_plain() {
            map.insert(child.id.clone(), life);
        }
    }
    save_okf_lifecycle(&app_data_dir(state), &folder.id, &map);
}

/// The lifecycle of every OKF concept in a notebook, by source id — what the
/// panel badges and the reader header read. Empty for a notebook holding no
/// bundles, which is the common case and costs one directory miss.
#[tauri::command]
pub async fn okf_lifecycle(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<std::collections::HashMap<String, OkfLifecycle>, String> {
    let sources = e(state.db.list_sources(&notebook_id).await)?;
    let data_dir = app_data_dir(&state);
    let mut out = std::collections::HashMap::new();
    for parent in sources.iter().filter(|s| s.source_type == "okf") {
        out.extend(load_okf_lifecycle(&data_dir, &parent.id));
    }
    Ok(out)
}

/// The frontmatter keys Alchemy writes itself (see `okf_frontmatter`).
/// Everything else in a concept file came from somewhere else and is the
/// bound notebook's to carry back out untouched — the spec's round-trip rule.
#[cfg_attr(not(test), allow(dead_code))]
const OKF_OWN_KEYS: &[&str] = &[
    "type",
    "title",
    "description",
    "resource",
    "tags",
    "status",
    "generated",
    "sources",
    "timestamp",
];

/// A parsed OKF concept file: its frontmatter as real YAML, and its body.
pub(crate) struct OkfDoc {
    front: serde_yaml_ng::Mapping,
    pub body: String,
}

impl OkfDoc {
    /// A frontmatter value, whatever its shape.
    pub fn get(&self, key: &str) -> Option<&serde_yaml_ng::Value> {
        self.front.get(serde_yaml_ng::Value::String(key.into()))
    }

    /// A scalar field as a string. `None` when the key is absent, empty, or
    /// carries a map or a sequence rather than a scalar.
    pub fn str(&self, key: &str) -> Option<String> {
        let text = match self.get(key)? {
            serde_yaml_ng::Value::String(s) => s.clone(),
            serde_yaml_ng::Value::Number(n) => n.to_string(),
            serde_yaml_ng::Value::Bool(b) => b.to_string(),
            _ => return None,
        };
        (!text.is_empty()).then_some(text)
    }

    /// A nested field, one level down (`generated.by`).
    pub fn nested(&self, key: &str, inner: &str) -> Option<String> {
        match self.get(key)?.get(inner)? {
            serde_yaml_ng::Value::String(s) => Some(s.clone()).filter(|s| !s.is_empty()),
            _ => None,
        }
    }

    /// `tags:` as a list, however it was written — `[a, b]`, a block
    /// sequence, or one bare scalar.
    pub fn tags(&self) -> Vec<String> {
        match self.get("tags") {
            Some(serde_yaml_ng::Value::Sequence(items)) => items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            Some(serde_yaml_ng::Value::String(s)) => {
                s.split(',').map(|t| t.trim().to_string()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// When the bundle says this concept was last written, as epoch ms.
    /// `generated.at` first, then the older `timestamp:` — so a v0.1 file and
    /// a v0.2 one both keep their real age across an import instead of being
    /// stamped with the moment they arrived.
    pub fn written_at(&self) -> Option<i64> {
        let raw = self
            .nested("generated", "at")
            .or_else(|| self.str("timestamp"))?;
        chrono::DateTime::parse_from_rfc3339(&raw)
            .ok()
            .map(|d| d.timestamp_millis())
    }

    /// The keys Alchemy did not write. The bound notebook's manifest carries
    /// these back out on the next write (docs/RFC-okf-live.md §5.1), which is
    /// the only consumer — phase 0 reads them so the round trip is testable
    /// before there is anywhere to keep them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn extra(&self) -> serde_yaml_ng::Mapping {
        self.front
            .iter()
            .filter(|(k, _)| !k.as_str().is_some_and(|k| OKF_OWN_KEYS.contains(&k)))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Split a concept file into frontmatter and body, parsing the frontmatter as
/// real YAML — nested `generated`, lists of `verified` entries, and anything
/// else a bundle carries. The v0.1 files Alchemy already wrote are a valid
/// subset, so they parse unchanged; a file whose YAML does not parse falls
/// back to the quoted-scalar reader rather than losing its title.
pub(crate) fn parse_okf_doc(text: &str) -> OkfDoc {
    let Some(rest) = text.strip_prefix("---\n") else {
        return OkfDoc {
            front: serde_yaml_ng::Mapping::new(),
            body: text.to_string(),
        };
    };
    let Some(end) = rest.find("\n---") else {
        return OkfDoc {
            front: serde_yaml_ng::Mapping::new(),
            body: text.to_string(),
        };
    };
    let head = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('\n').to_string();
    let front = match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(head) {
        Ok(serde_yaml_ng::Value::Mapping(map)) => map,
        // Hand-edited frontmatter that is not valid YAML still has readable
        // `key: value` lines; take those rather than dropping the document's
        // title on the floor.
        _ => parse_okf_scalars(head),
    };
    OkfDoc { front, body }
}

/// The v0.1 reader, kept as the fallback: `key: "quoted"` or bare values,
/// one per line, nothing nested.
fn parse_okf_scalars(head: &str) -> serde_yaml_ng::Mapping {
    let mut map = serde_yaml_ng::Mapping::new();
    for line in head.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim();
            let v = if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
                v[1..v.len() - 1]
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\")
            } else {
                v.to_string()
            };
            map.insert(
                serde_yaml_ng::Value::String(k.trim().to_string()),
                serde_yaml_ng::Value::String(v),
            );
        }
    }
    map
}

// ---- The binding (docs/RFC-okf-live.md §5.1) --------------------------------

/// Where a notebook keeps itself on disk. Machine-local: a path means nothing
/// on another machine, so this never becomes a store column.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkfBinding {
    pub path: String,
    /// Epoch ms of the last successful write; 0 until the seed pass lands.
    #[serde(default)]
    pub last_write_at: i64,
}

fn bindings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("okf-bindings.json")
}

pub fn load_bindings(data_dir: &Path) -> HashMap<String, OkfBinding> {
    std::fs::read_to_string(bindings_path(data_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_bindings(data_dir: &Path, map: &HashMap<String, OkfBinding>) {
    let path = bindings_path(data_dir);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(path, json);
    }
}

pub fn binding_for(data_dir: &Path, notebook_id: &str) -> Option<OkfBinding> {
    load_bindings(data_dir).remove(notebook_id)
}

/// Bind, rebind, or (with `None`) unbind. Unbinding leaves the files where
/// they are — the folder is the user's, and stopping the sync is not a
/// reason to take it away.
pub fn set_binding(data_dir: &Path, notebook_id: &str, binding: Option<OkfBinding>) {
    let mut map = load_bindings(data_dir);
    match binding {
        Some(b) => map.insert(notebook_id.to_string(), b),
        None => map.remove(notebook_id),
    };
    save_bindings(data_dir, &map);
}

// ---- The manifest (§5.1) ----------------------------------------------------

/// One concept file Alchemy manages, as of the last write.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkfManifestEntry {
    /// Bundle-relative path, so a moved bundle's manifest still reads.
    pub path: String,
    /// Hash of the whole file as written. Both the "did this change" test and
    /// the echo test the reconciler needs (§5.3).
    pub hash: String,
    /// Epoch ms of the entity when we wrote it — the conflict clock (§5.4).
    #[serde(default)]
    pub wrote_at: i64,
    /// Frontmatter keys Alchemy does not write, carried in from an outside
    /// edit and re-emitted verbatim on every write since.
    #[serde(default)]
    pub extra: serde_yaml_ng::Mapping,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkfManifest {
    /// Entity id → the file written for it.
    #[serde(default)]
    pub concepts: HashMap<String, OkfManifestEntry>,
}

fn manifest_path(bundle: &Path) -> PathBuf {
    bundle.join(".alchemy").join("manifest.json")
}

pub fn load_manifest(bundle: &Path) -> OkfManifest {
    std::fs::read_to_string(manifest_path(bundle))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_manifest(bundle: &Path, manifest: &OkfManifest) {
    let path = manifest_path(bundle);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(manifest) {
        let _ = std::fs::write(path, json);
    }
}

/// FNV-1a over the file's bytes, hex. Not a crate: this only ever compares
/// Alchemy's own writes against Alchemy's own record, so what matters is that
/// the function is cheap and stable across releases — which a `DefaultHasher`
/// explicitly is not.
pub fn okf_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

// ---- Write-through (§5.2) ---------------------------------------------------

/// Two seconds, the same number and the same reason as `fswatch::DEBOUNCE`:
/// a sweep touching forty sources should reach disk as one write.
const DEBOUNCE_MS: i64 = 2_000;

fn pending() -> &'static std::sync::Mutex<HashMap<String, i64>> {
    static PENDING: std::sync::OnceLock<std::sync::Mutex<HashMap<String, i64>>> =
        std::sync::OnceLock::new();
    PENDING.get_or_init(Default::default)
}

fn flushing() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static FLUSHING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    FLUSHING.get_or_init(Default::default)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Note that a bound notebook changed, and write its bundle once the changes
/// stop arriving. Safe to call from any mutation: an unbound notebook costs
/// one file read and returns.
pub fn schedule_write(notebook_id: &str) {
    let Some(app) = crate::commands::app_handle() else {
        return;
    };
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };
    if binding_for(&data_dir, notebook_id).is_none() {
        return;
    }
    let id = notebook_id.to_string();
    if let Ok(mut map) = pending().lock() {
        map.insert(id.clone(), now_ms() + DEBOUNCE_MS);
    }
    // One flusher per notebook: later changes move its deadline rather than
    // stacking a second writer behind it.
    match flushing().lock() {
        Ok(mut running) => {
            if !running.insert(id.clone()) {
                return;
            }
        }
        Err(_) => return,
    }
    tauri::async_runtime::spawn(async move {
        loop {
            let deadline = pending()
                .lock()
                .ok()
                .and_then(|m| m.get(&id).copied())
                .unwrap_or(0);
            let wait = deadline - now_ms();
            if wait <= 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(wait as u64)).await;
        }
        if let Ok(mut map) = pending().lock() {
            map.remove(&id);
        }
        let state = app.state::<AppState>();
        if let Err(err) = write_bound(&state, &id).await {
            crate::diagnostics::error("okf", format!("bundle write failed: {err}"));
        }
        if let Ok(mut running) = flushing().lock() {
            running.remove(&id);
        }
    });
}

/// Bring a bound notebook's bundle up to date. The seed pass and every write
/// after it are the same pass — a bundle nobody has written yet simply has an
/// empty manifest, so every concept counts as changed.
pub async fn write_bound(state: &AppState, notebook_id: &str) -> Result<OkfWrite, String> {
    let data_dir = app_data_dir(state);
    let binding = binding_for(&data_dir, notebook_id)
        .ok_or_else(|| "This notebook isn't kept on disk".to_string())?;
    let bundle = PathBuf::from(&binding.path);
    let (title, sources, notes) = gather_bundle(state, notebook_id).await?;
    let written = write_bundle(&title, &sources, &notes, &bundle)?;
    set_binding(
        &data_dir,
        notebook_id,
        Some(OkfBinding {
            path: binding.path,
            last_write_at: now_ms(),
        }),
    );
    Ok(written)
}

// ---- Surfaces (§5.5) --------------------------------------------------------

/// Where this notebook keeps itself on disk, if anywhere. The header chip and
/// the ⋯ menu both ask this.
#[tauri::command]
pub async fn notebook_okf_binding(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Option<OkfBinding>, String> {
    Ok(binding_for(&app_data_dir(&state), &notebook_id))
}

/// Keep a notebook on disk as an OKF bundle at `path`.
///
/// An empty folder gets the seed pass. A folder that already is a bundle is
/// imported first and then bound, so binding to a colleague's checkout adds
/// what it holds rather than overwriting it — duplicates skip, as import
/// always does. Returns the path, so the caller can say where it went.
pub(crate) async fn bind_impl(
    app: &AppHandle,
    state: &AppState,
    notebook_id: &str,
    path: &str,
) -> Result<String, String> {
    let bundle = PathBuf::from(path);
    if !bundle.is_dir() {
        return Err(format!("Not a folder: {path}"));
    }
    // A bundle already living here has content the notebook does not; take it
    // in before the writer starts treating this folder as its own.
    if crate::commands::find_bundle_root(bundle.clone()).is_ok() {
        crate::commands::import_bundle(app, state, bundle.clone(), Some(notebook_id.to_string()))
            .await?;
    }
    let data_dir = app_data_dir(state);
    set_binding(
        &data_dir,
        notebook_id,
        Some(OkfBinding {
            path: path.to_string(),
            last_write_at: 0,
        }),
    );
    write_bound(state, notebook_id).await?;
    Ok(path.to_string())
}

#[tauri::command]
pub async fn bind_notebook_okf(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: String,
    path: String,
) -> Result<String, String> {
    bind_impl(&app, &state, &notebook_id, &path).await
}

/// Stop keeping a notebook on disk. The files stay where they are — the
/// folder is the user's, and ending the sync is no reason to take it away.
#[tauri::command]
pub async fn unbind_notebook_okf(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<(), String> {
    set_binding(&app_data_dir(&state), &notebook_id, None);
    Ok(())
}

/// Write a bound notebook's bundle now, rather than waiting out the debounce.
/// The ⋯ menu's "Write now" and the agent-facing equivalent.
#[tauri::command]
pub async fn write_notebook_okf(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<i64, String> {
    write_bound(&state, &notebook_id).await?;
    Ok(now_ms())
}
