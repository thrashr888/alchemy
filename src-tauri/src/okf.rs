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

use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::{app_data_dir, e, is_web_url, new_id, AppState};
use crate::ingest;
use crate::models::{Note, Notebook, Source};
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

/// The app acting on its own initiative: a generation, a curator move, a
/// refresh, an import. Never a person (see `okf_human`).
pub(crate) fn okf_writer() -> String {
    concat!("alchemy/", env!("CARGO_PKG_VERSION")).to_string()
}

/// This Mac's account, the macOS short name. `USER` is set for GUI apps by
/// launchd as well as for shells; `id -un` is the fallback for the odd
/// environment that clears it.
pub(crate) fn okf_account() -> String {
    if let Ok(user) = std::env::var("USER") {
        if !user.trim().is_empty() {
            return user.trim().to_string();
        }
    }
    if let Ok(user) = std::env::var("LOGNAME") {
        if !user.trim().is_empty() {
            return user.trim().to_string();
        }
    }
    std::process::Command::new("/usr/bin/id")
        .arg("-un")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// A person, on this Mac. Two Macs sharing a folder are usually one person
/// with one short name, which is the point: an edit made on either reads as
/// the same human, not as a stranger to be attributed.
pub(crate) fn okf_human() -> String {
    format!("human:{}", okf_account())
}

/// Is this by-line one of ours — this app, or this person? Everything else
/// is somebody else, and §5.3 attributes their edits to them.
pub(crate) fn okf_is_ours(by: &str) -> bool {
    by == okf_writer() || by == okf_human()
}

/// Does this actor read as a machine? `name/version` is the shape both
/// `alchemy/<version>` and other producers use; `human:` is explicitly not.
pub(crate) fn okf_actor_is_machine(actor: &str) -> bool {
    actor == "auto" || (!actor.starts_with("human:") && actor.contains('/'))
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
    /// `generated.by` — who made this version. A person on this Mac
    /// (`human:<account>`) or the app on its own (`alchemy/<version>`), per
    /// §5.6. Empty falls back to the app.
    pub generated_by: String,
    /// `status:` — "" | "draft" | "deprecated".
    pub status: String,
    /// `sources:` — ids of the concepts this one was derived from. Resolved
    /// to bundle-relative paths at write time, so an id that did not make it
    /// into the bundle simply does not appear.
    pub derived_from: Vec<String>,
    /// `alchemy:` — everything the spec has no home for, under one key so it
    /// collides with nothing (OKF §4.1 lets a producer add its own). Ordered
    /// pairs, because the order is the reading order; empty values are
    /// dropped at emission. A reader that does not know the key ignores it;
    /// ours uses it to restore a notebook faithfully.
    pub alchemy: Vec<(String, String)>,
    /// `alchemy.parent` — the folder/git/notion parent this source is a child
    /// of, as an entity id here and the parent's slug in the file, so a
    /// folder source's shape survives a round trip.
    pub parent: String,
    /// The machine path this concept was made from, if any. Stays in
    /// `alchemy.origin` whether or not the bytes travel, so a bind-back can
    /// re-link (§6).
    pub origin_uri: String,
    /// What to do with the original. Resolved at gather time, acted on at
    /// write time — only the writer knows where the bundle is.
    pub reference: Option<ReferencePlan>,
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
            generated_by: String::new(),
            status: String::new(),
            derived_from: Vec::new(),
            alchemy: Vec::new(),
            parent: String::new(),
            origin_uri: String::new(),
            reference: None,
            extra: serde_yaml_ng::Mapping::new(),
        }
    }
}

/// The notebook a bundle describes, as the root `index.md` needs it. Until
/// now that file had no frontmatter at all, so a round trip lost the
/// notebook's own identity — its colour, its icon, and which notebook it was.
pub struct OkfNotebook {
    pub id: String,
    pub title: String,
    pub color: String,
    pub icon: String,
    pub generated_at: i64,
}

/// Emit an `alchemy:` block, skipping pairs with nothing to say. Values are
/// quoted scalars, so a colour like `#5e6ad2` cannot read as a comment.
fn okf_alchemy_block(pairs: &[(String, String)]) -> String {
    let kept: Vec<&(String, String)> = pairs.iter().filter(|(_, v)| !v.is_empty()).collect();
    if kept.is_empty() {
        return String::new();
    }
    let mut out = String::from("alchemy:\n");
    for (key, value) in kept {
        out.push_str(&format!("  {key}: {}\n", yaml_str(value)));
    }
    out
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
    /// Originals copied in, and originals left where they were (§6).
    pub referenced: usize,
    pub linked: usize,
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

/// Does this manifest path still belong to `want`'s family — `want.md`, or
/// the `want-2.md` a collision gave it? A path that does is this concept's
/// own and it keeps it; a path that does not means the title re-slugged, and
/// the file moves.
fn keeps_slug(path: &str, dir: &str, want: &str) -> bool {
    let Some(stem) = path
        .strip_prefix(&format!("{dir}/"))
        .and_then(|rest| rest.strip_suffix(".md"))
    else {
        return false;
    };
    stem == want
        || stem
            .strip_prefix(want)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Is some *other* concept's file at this path? The one question the writer
/// has to ask before it renames a file or removes one.
fn claimed_by_another(manifest: &OkfManifest, id: &str, rel: &str) -> bool {
    manifest
        .concepts
        .iter()
        .any(|(other, entry)| other != id && entry.path == rel)
}

/// A placement at a path already decided. The slug is whatever the path says
/// it is — which for a file read-back adopted is the name its author gave it,
/// not one the writer would have chosen.
fn placement_at(dir: &str, path: &str, title: &str) -> OkfPlacement {
    let slug = path
        .strip_prefix(&format!("{dir}/"))
        .unwrap_or(path)
        .strip_suffix(".md")
        .unwrap_or(path)
        .to_string();
    OkfPlacement {
        slug,
        path: path.to_string(),
        title: title.to_string(),
    }
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
    let by = match concept.generated_by.as_str() {
        "" => okf_writer(),
        actor => actor.to_string(),
    };
    fm.push_str(&format!("  by: {}\n", yaml_str(&by)));
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
    let mut alchemy = concept.alchemy.clone();
    if let Some(parent) = placements.get(&concept.parent) {
        alchemy.push(("parent".into(), parent.slug.clone()));
    }
    fm.push_str(&okf_alchemy_block(&alchemy));
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

/// Append one dated entry to the bundle's `log.md` (spec §9). The file is a
/// history, not a stamp — which is what makes a bundle rewritten every night
/// worth reading.
///
/// **The heading names the writer, not just the day** (§5.6). Two Macs
/// sharing one folder appending under one heading is its own newest-wins
/// race, and the entry that records a lost conflict is precisely the one
/// that must not lose. Each install writes under `## <day> — <account>`, so
/// the two sides append to different blocks and a cloud tool has an easy
/// merge instead of a clash.
pub(crate) fn okf_log_append(bundle: &std::path::Path, entry: &str) -> Result<(), String> {
    let path = bundle.join("log.md");
    let now = chrono::Utc::now();
    let heading = format!("## {} \u{2014} {}", now.format("%Y-%m-%d"), okf_account());
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
    // This writer's newest day is always its last block, so a bullet appended
    // at the end lands under the right heading.
    if !out.contains(&format!("\n{heading}\n")) {
        out.push_str(&format!("\n{heading}\n\n"));
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
    notebook: &OkfNotebook,
    sources: &[OkfConcept],
    notes: &[OkfConcept],
    bundle: &Path,
    manifest_at: Option<&Path>,
) -> Result<OkfWrite, String> {
    std::fs::create_dir_all(bundle).map_err(|err| format!("Failed to create {bundle:?}: {err}"))?;
    let write = |path: &Path, text: &str| -> Result<(), String> {
        std::fs::write(path, text).map_err(|err| format!("Failed to write {path:?}: {err}"))
    };
    // `None` is a one-shot export into a fresh directory: there is no last
    // time to compare against and no record worth keeping afterwards.
    let mut manifest = manifest_at.map(load_manifest).unwrap_or_default();
    let mut out = OkfWrite::default();

    // Place everything first: a note's `sources:` entries cite bundle paths,
    // which are only known once every path is claimed.
    let mut placements: HashMap<String, OkfPlacement> = HashMap::new();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let order: Vec<(&str, Vec<&OkfConcept>)> = vec![
        ("sources", sources.iter().collect()),
        ("notes", notes.iter().collect()),
    ];
    // Pass one: paths the manifest already holds for a concept that is still
    // here. Two of them are pinned — a file read-back took in keeps the name
    // it arrived under (the file is the concept, and inventing a slug for it
    // would leave the original unclaimed for the next reconcile to import
    // again), and a concept whose title still slugs to the file it has stays
    // where it is. The second is what keeps a *new* concept of the same name
    // from taking an occupied path and the older one from being renamed on
    // top of it, which used to lose one of the two files outright.
    for (dir, concepts) in &order {
        for concept in concepts {
            let Some(entry) = manifest.concepts.get(&concept.id) else {
                continue;
            };
            if !entry.path.starts_with(&format!("{dir}/")) {
                continue;
            }
            if !entry.adopted && !keeps_slug(&entry.path, dir, &okf_slug(&concept.title)) {
                continue;
            }
            if !taken.insert(entry.path.clone()) {
                continue;
            }
            placements.insert(
                concept.id.clone(),
                placement_at(dir, &entry.path, &concept.title),
            );
        }
    }
    // Pass two: everything else takes the first free slug of its title, so
    // two sources called "Notes" become notes.md and notes-2.md rather than
    // one clobbering the other.
    let mut used: HashMap<String, u32> = HashMap::new();
    for (dir, concepts) in &order {
        for concept in concepts {
            if placements.contains_key(&concept.id) {
                continue;
            }
            let base = okf_slug(&concept.title);
            let key = format!("{dir}/{base}");
            let mut n = *used.get(&key).unwrap_or(&0);
            let path = loop {
                n += 1;
                let slug = if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                };
                let path = format!("{dir}/{slug}.md");
                if taken.insert(path.clone()) {
                    break path;
                }
            };
            used.insert(key, n);
            placements.insert(concept.id.clone(), placement_at(dir, &path, &concept.title));
        }
    }

    let mut listings: HashMap<&str, Vec<(String, String, String)>> = HashMap::new();
    let mut still_ours: std::collections::HashSet<String> = std::collections::HashSet::new();
    // References this pass's concepts point at, so the ones nothing points at
    // any more can go; and the originals that stayed behind, for the log.
    let mut references: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut linked: Vec<String> = Vec::new();
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
            // The original, if the bundle is its sensible home (§6).
            // `resource:` says which way it went — a `references/` path means
            // the bytes are here, anything else is provenance to a place this
            // bundle does not own — and `alchemy.origin` keeps the machine
            // path either way, so a bind-back can re-link.
            match concept.reference.clone() {
                Some(ReferencePlan::Copy { name, hash, from }) => {
                    match place_reference(bundle, &mut manifest, &name, &hash, &from) {
                        Ok(rel) => {
                            references.insert(
                                rel.strip_prefix("references/").unwrap_or(&rel).to_string(),
                            );
                            // The hash is how the bundle dedupes originals, so
                            // it says so out loud rather than hiding in the
                            // filename where only we could read it.
                            concept.alchemy.push(("sha256".into(), hash));
                            concept.resource = rel;
                        }
                        Err(err) => {
                            crate::note!("okf: {err}");
                            out.linked += 1;
                        }
                    }
                }
                Some(ReferencePlan::Inside { rel }) => concept.resource = rel,
                // Only worth saying when the bytes could have travelled and
                // deliberately did not; the rest are links by their nature.
                Some(ReferencePlan::Link { reason }) if reason == "over the size cap" => {
                    linked.push(format!("{} ({reason})", concept.title));
                    out.linked += 1;
                }
                Some(ReferencePlan::Link { .. }) => {}
                None => {}
            }
            if !concept.origin_uri.is_empty() {
                concept
                    .alchemy
                    .push(("origin".into(), concept.origin_uri.clone()));
            }
            let text = format!(
                "{}{}\n",
                okf_frontmatter(&concept, &description, &placements),
                concept.content
            );
            let hash = okf_hash(&text);
            let prior = manifest.concepts.get(&concept.id);
            // A retitled concept moves rather than being deleted and
            // rewritten — but never onto a path another entry still claims.
            // A slug collision used to rename one concept over its
            // neighbour's file and leave the manifest pointing at nothing.
            let mut rel = place.path.clone();
            if let Some(prior) = prior {
                if prior.path != rel {
                    if claimed_by_another(&manifest, &concept.id, &rel) {
                        crate::note!(
                            "okf: {rel} is another concept's file; leaving {} where it is",
                            prior.path
                        );
                        rel = prior.path.clone();
                    } else {
                        let from = bundle.join(&prior.path);
                        if from.exists() && std::fs::rename(&from, bundle.join(&rel)).is_ok() {
                            out.moved += 1;
                        }
                    }
                }
            }
            let at = bundle.join(&rel);
            // A file iCloud has evicted is not missing, it is not downloaded.
            // Writing over it would discard whatever the other Mac put there,
            // so the pass asks for it and leaves it alone (§5.7).
            let slug = placement_at(dir, &rel, &concept.title).slug;
            if is_evicted_stub(&at) {
                hydrate_if_evicted(&at);
                still_ours.insert(concept.id.clone());
                entries.push((slug, concept.title.clone(), description));
                continue;
            }
            let unchanged = prior.is_some_and(|p| p.hash == hash) && at.exists();
            if !unchanged {
                write(&at, &text)?;
                out.written += 1;
            }
            let adopted = prior.is_some_and(|p| p.adopted);
            manifest.concepts.insert(
                concept.id.clone(),
                OkfManifestEntry {
                    path: rel,
                    adopted,
                    hash,
                    wrote_at: concept.generated_at,
                    extra: std::mem::take(&mut concept.extra),
                },
            );
            still_ours.insert(concept.id.clone());
            entries.push((slug, concept.title.clone(), description));
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
            // Only if nothing else claims it. A collision that moved another
            // concept onto this path would otherwise have its file deleted
            // out from under it, leaving `index.md` linking at nothing.
            if manifest.concepts.values().any(|e| e.path == entry.path) {
                continue;
            }
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

    // Root index.md is the bundle's own concept document now. It carries
    // frontmatter like every other file, so the notebook's identity — which
    // notebook this is, its colour, its icon — survives a round trip instead
    // of being guessed back from the H1.
    let description = format!(
        "{} sources and {} notes exported from Alchemy.",
        listings.get("sources").map(Vec::len).unwrap_or(0),
        listings.get("notes").map(Vec::len).unwrap_or(0)
    );
    let root = OkfConcept {
        id: notebook.id.clone(),
        title: notebook.title.clone(),
        type_label: "Notebook".into(),
        generated_at: notebook.generated_at,
        alchemy: vec![
            ("id".into(), notebook.id.clone()),
            ("color".into(), notebook.color.clone()),
            ("icon".into(), notebook.icon.clone()),
        ],
        ..OkfConcept::blank()
    };
    let mut index = okf_frontmatter(&root, &description, &HashMap::new());
    index.push_str(&format!("# {}\n\n", notebook.title));
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

    // An original nothing claims any more goes with the source that owned it.
    let dropped = prune_references(bundle, &mut manifest, &references);
    out.removed += dropped;
    out.referenced = references.len();
    out.sources = listings.get("sources").map(Vec::len).unwrap_or(0);
    out.notes = listings.get("notes").map(Vec::len).unwrap_or(0);
    if let Some(path) = manifest_at {
        save_manifest(path, &manifest);
    }
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
        if out.referenced > 0 {
            parts.push(format!("{} originals", out.referenced));
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
        if !linked.is_empty() {
            okf_log_append(
                bundle,
                &format!(
                    "Left {} original(s) where they were: {}.",
                    linked.len(),
                    linked.join("; ")
                ),
            )?;
        }
    }
    Ok(out)
}

/// A note's lifecycle in the bundle (§3): the curator's archive is the
/// spec's `deprecated`, and anything a *machine* wrote on its own initiative
/// is a `draft` until a person has touched it.
///
/// Since §5.3, an origin can also name a person (`human:kim`, from an edit
/// made on another Mac). A person's note is not a draft, so only machine
/// origins — the curator's `auto`, or a `name/version` producer — earn one.
fn okf_note_status(note: &Note) -> String {
    if note.status == "archived" {
        "deprecated".into()
    } else if okf_actor_is_machine(&note.origin) {
        "draft".into()
    } else {
        String::new()
    }
}

/// Who last made this note, as far as the store records it (§5.6).
///
/// `origin` carries an outside actor verbatim once read-back has attributed
/// one, so that wins. `auto` is the curator or the chat post-pass. A kind
/// other than `note` is something the Studio generated. Everything left is a
/// note a person wrote or edited in the app.
fn okf_note_actor(note: &Note) -> String {
    match note.origin.as_str() {
        "" => {
            if note.kind == "note" {
                okf_human()
            } else {
                okf_writer()
            }
        }
        "auto" => okf_writer(),
        actor => actor.to_string(),
    }
}

/// Who last made this source (§5.6).
///
/// Every source arrives by import, which is the app acting on its own. What
/// says a person has touched one since is the user's own tags and their note
/// — both ground truth from the user — and the sidecar the edit path writes,
/// which is how a bare rename gets a by-line at all: the store keeps no
/// record of who chose a title.
fn okf_source_actor(source: &Source, edits: &OkfHumanEdits) -> String {
    if source.tags.trim().is_empty()
        && source.note.trim().is_empty()
        && !edits.contains_key(&source.id)
    {
        okf_writer()
    } else {
        okf_human()
    }
}

/// One source as the bundle carries it. Split out of `gather_bundle_for` so
/// the by-line rule can be asserted against a real concept without an
/// `AppState`, which a unit test cannot stand up (§5.6, as built).
pub(crate) fn source_concept(
    s: &Source,
    content: String,
    bundle: &Path,
    cap_bytes: u64,
    edits: &OkfHumanEdits,
) -> OkfConcept {
    let resource = if s.url.is_empty() {
        String::new()
    } else if is_web_url(&s.url) {
        s.url.clone()
    } else {
        format!("file://{}", s.url)
    };
    OkfConcept {
        id: s.id.clone(),
        title: s.title.clone(),
        content,
        type_label: "Source".into(),
        resource,
        origin_uri: if s.url.is_empty() || is_web_url(&s.url) {
            String::new()
        } else {
            format!("file://{}", s.url)
        },
        reference: Some(plan_reference(s, bundle, cap_bytes)),
        tags: vec![s.source_type.clone()],
        generated_at: s.created_at,
        generated_by: okf_source_actor(s, edits),
        // What the spec has no field for. `source_type` is the real type
        // (the top-level `tags:` is the spec-facing one); `tags` is the
        // user's own labels; `image_url` spares the gallery a refetch.
        alchemy: vec![
            ("id".into(), s.id.clone()),
            ("source_type".into(), s.source_type.clone()),
            ("tags".into(), s.tags.clone()),
            ("author".into(), s.author.clone()),
            ("image_url".into(), s.image_url.clone()),
        ],
        parent: s.parent_id.clone(),
        ..OkfConcept::blank()
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
/// Told where the bundle will land, so each source's original can be planned
/// against it (§6): a file already inside the bundle is cited where it lies
/// rather than copied beside itself.
pub(crate) async fn gather_bundle_for(
    state: &AppState,
    notebook_id: &str,
    bundle: &Path,
) -> Result<(OkfNotebook, Vec<OkfConcept>, Vec<OkfConcept>), String> {
    let notebook = e(state.db.list_notebooks().await)?
        .into_iter()
        .find(|n| n.id == notebook_id)
        .ok_or_else(|| "Notebook not found".to_string())?;
    let sources = e(state.db.list_sources(notebook_id).await)?;
    let notes = e(state.db.list_notes(notebook_id).await)?;

    // One setting, read once per pass.
    let cap_bytes = {
        let ai = state.ai.read().await;
        ai.config().okf_reference_cap_mb.saturating_mul(1024 * 1024)
    };
    // Who has touched what, read once per pass rather than once per source.
    let human_edits = load_okf_human_edits(&app_data_dir(state), notebook_id);
    let mut source_concepts = Vec::with_capacity(sources.len());
    for s in &sources {
        let content = e(state.db.source_content(&s.id).await)?;
        source_concepts.push(source_concept(s, content, bundle, cap_bytes, &human_edits));
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
            generated_by: okf_note_actor(note),
            status: okf_note_status(note),
            derived_from: cites.get(&note.id).cloned().unwrap_or_default(),
            // `type:` is a human label and several kinds share one; `kind` is
            // the machine name, so a Study Guide comes back a study guide.
            alchemy: vec![
                ("id".into(), note.id.clone()),
                ("kind".into(), note.kind.clone()),
                ("origin".into(), note.origin.clone()),
                ("status".into(), note.status.clone()),
            ],
            ..OkfConcept::blank()
        })
        .collect();

    let meta = OkfNotebook {
        id: notebook.id.clone(),
        title: notebook.title.clone(),
        color: notebook.color.clone(),
        icon: notebook.icon.clone(),
        generated_at: notebook.updated_at,
    };
    Ok((meta, source_concepts, note_concepts))
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
    // Where it lands is settled before anything is read: originals are
    // planned against the bundle path (§6), and the notebook row alone —
    // which carries no source text — is enough to name the directory.
    let title = e(state.db.list_notebooks().await)?
        .into_iter()
        .find(|n| n.id == notebook_id)
        .map(|n| n.title)
        .ok_or_else(|| "Notebook not found".to_string())?;

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
    let (notebook, sources, notes) = gather_bundle_for(&state, &notebook_id, &bundle).await?;
    write_bundle(&notebook, &sources, &notes, &bundle, None)?;
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
        let dir = dest.join(&slug);
        let (notebook, sources, notes) = gather_bundle_for(state, &nb.id, &dir).await?;
        // The nightly copy keeps its own manifest — beside the bindings, not
        // in the bundle — so tonight can drop the concepts last night wrote
        // for a source that has since gone.
        let manifest = manifest_path(&app_data_dir(state), &format!("nightly-{slug}"));
        let written = write_bundle(&notebook, &sources, &notes, &dir, Some(&manifest))?;
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

/// Is this path one of the bundle's concept documents?
///
/// An allowlist, not a skip list, and that is the point. A bundle is often a
/// git repository, and running `ok init` in one grows `.ok/`, `.okignore`,
/// `.claude/`, `.codex/`, `.cursor/`, `.pi/`, `.opencode/`, `.github/`,
/// `.mcp.json`, `opencode.json`, and `.gitignore` — tooling, not knowledge,
/// and a list that keeps growing. Nobody can maintain a blocklist against
/// that. The spec already says where knowledge lives (§3.1), so only
/// `sources/**.md` and `notes/**.md` are read and everything else in the
/// folder is somebody else's business. `.gitignore` is no help here either:
/// the one `ok init` writes excludes a couple of generated skill directories
/// and nothing else.
pub(crate) fn is_okf_concept(root: &Path, path: &str) -> bool {
    let Ok(rel) = Path::new(path).strip_prefix(root) else {
        return false;
    };
    if rel.extension().and_then(|x| x.to_str()) != Some("md") {
        return false;
    }
    // Hidden anywhere along the path is out, whatever a .gitignore says.
    if rel
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
    {
        return false;
    }
    let mut parts = rel.components();
    let top = parts
        .next()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .unwrap_or_default();
    matches!(top.as_str(), "sources" | "notes") && parts.next().is_some() && !is_okf_reserved(path)
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

/// Sources a person has edited in the app, by source id → when (epoch ms).
///
/// The store records how a source arrived and what it says, never who last
/// touched its title, so a bare rename moved no by-line and the concept kept
/// crediting `alchemy/<version>` with a title a person chose (§5.6). This is
/// the missing record, in the same per-parent sidecar shape the lifecycle
/// uses: machine-local, read only by the bundle writer, and no column in a
/// store that older builds still append to.
pub(crate) type OkfHumanEdits = HashMap<String, i64>;

fn okf_human_edits_path(data_dir: &Path, notebook_id: &str) -> PathBuf {
    data_dir
        .join("okf_human_edits")
        .join(format!("{notebook_id}.json"))
}

/// Every source in one notebook a person has edited. One file read per write
/// pass, and a directory miss for the notebooks where nobody has.
pub(crate) fn load_okf_human_edits(data_dir: &Path, notebook_id: &str) -> OkfHumanEdits {
    std::fs::read_to_string(okf_human_edits_path(data_dir, notebook_id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// A person edited this source in the app — renamed it, or rewrote its text.
/// Called by the edit command and the MCP tool behind it, never by a refresh
/// or an import, which are the app acting on its own.
pub(crate) fn note_human_source_edit(data_dir: &Path, source: &Source) {
    let path = okf_human_edits_path(data_dir, &source.notebook_id);
    let mut map = load_okf_human_edits(data_dir, &source.notebook_id);
    map.insert(source.id.clone(), now_ms());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(&map) {
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
    "alchemy",
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

// ---- The Notebooks folder (docs/RFC-okf-live.md §5.7) -----------------------

/// Where notebooks live on disk, and whether new ones go there.
pub(crate) async fn notebooks_home(state: &AppState) -> (PathBuf, bool) {
    let ai = state.ai.read().await;
    let config = ai.config();
    (
        PathBuf::from(config.notebooks_dir.clone()),
        config.keep_on_disk,
    )
}

/// A folder for this notebook under the Notebooks root, deduped the way the
/// exporter's slugs are: `research`, then `research-2`. A notebook already
/// bound keeps the folder it has.
pub(crate) fn claim_notebook_folder(
    root: &Path,
    title: &str,
    taken: &std::collections::HashSet<String>,
) -> PathBuf {
    let base = okf_slug(title);
    let mut slug = base.clone();
    let mut n = 2;
    while taken.contains(&slug) || root.join(&slug).exists() {
        slug = format!("{base}-{n}");
        n += 1;
    }
    root.join(slug)
}

/// A notebook the app keeps for itself (Briefs) is not a document the user
/// files, so it never lands in the Notebooks folder.
pub(crate) fn is_system_notebook(notebook: &Notebook) -> bool {
    notebook.status == "system"
}

/// Give a notebook its folder and seed it (§5.7). Silent and best-effort:
/// creating a notebook must not fail because a disk did.
pub(crate) async fn bind_new_notebook(state: &AppState, notebook: &Notebook) {
    if is_system_notebook(notebook) || crate::examples::is_starter_title(&notebook.title) {
        return;
    }
    let (root, keep) = notebooks_home(state).await;
    if !keep || root.as_os_str().is_empty() {
        return;
    }
    if binding_for(&app_data_dir(state), &notebook.id).is_some() {
        return;
    }
    if let Err(err) = std::fs::create_dir_all(&root) {
        crate::diagnostics::error("okf", format!("could not make the Notebooks folder: {err}"));
        return;
    }
    let taken: std::collections::HashSet<String> = load_bindings(&app_data_dir(state))
        .values()
        .filter_map(|b| {
            Path::new(&b.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })
        .collect();
    let folder = claim_notebook_folder(&root, &notebook.title, &taken);
    if let Err(err) = std::fs::create_dir_all(&folder) {
        crate::diagnostics::error("okf", format!("could not make {folder:?}: {err}"));
        return;
    }
    let data_dir = app_data_dir(state);
    set_binding(
        &data_dir,
        &notebook.id,
        Some(OkfBinding {
            path: folder.to_string_lossy().to_string(),
            id: new_id(),
            last_write_at: 0,
        }),
    );
    if let Err(err) = write_bound(state, &notebook.id).await {
        crate::diagnostics::error("okf", format!("seed pass failed: {err}"));
    }
}

/// Keep every active notebook on disk — the upgrade offer's Keep button
/// (§5.7). Reports how many it bound so the caller can say so.
pub(crate) async fn bind_all_notebooks(app: &AppHandle, state: &AppState) -> Result<usize, String> {
    let notebooks = e(state.db.list_notebooks().await)?;
    let total = notebooks.len();
    let mut bound = 0usize;
    for (done, nb) in notebooks.iter().enumerate() {
        // A starter is the app's own sample, and every Mac seeds its own
        // copy under its own id. Binding them means two installs trade
        // bundles for notebooks neither person asked for (§5.7); the ⋯ verb
        // still binds one if somebody wants it on disk.
        if is_system_notebook(nb)
            || nb.status == "archived"
            || crate::examples::is_starter_title(&nb.title)
        {
            continue;
        }
        let _ = app.emit(
            "okf://binding",
            serde_json::json!({ "done": done, "total": total, "title": nb.title }),
        );
        bind_new_notebook(state, nb).await;
        if binding_for(&app_data_dir(state), &nb.id).is_some() {
            bound += 1;
        }
    }
    let _ = app.emit(
        "okf://binding",
        serde_json::json!({ "done": total, "total": total, "title": "" }),
    );
    Ok(bound)
}

// ---- The self-heal (§5.7) ---------------------------------------------------
//
// Rules are for the state that has not happened yet. The duplication that
// shipped in 0.55.0 already happened, on at least two Macs, and it leaves
// behind exactly four shapes: two notebooks writing into one folder, a
// starter notebook bound at all, a binding whose folder says it belongs to a
// notebook that is bound elsewhere, and a second copy of a starter imported
// from the other Mac's bundle. This is the pass that puts those right, once
// per launch, before anything writes.
//
// It never deletes. Every fix is an unbind (which leaves the files where
// they are, as §5.5 promises) or an archive (which hides a notebook from the
// grid and keeps every row it has). A wrong guess here costs the user a
// visit to the archive, not their notes.

/// One thing the heal decided to do, and the sentence that explains it.
#[derive(Debug, PartialEq)]
pub(crate) enum HealStep {
    /// Stop keeping this notebook on disk. The folder is left alone.
    Unbind { notebook: String, why: String },
    /// Hide this notebook: it is a duplicate of one that stays.
    Archive { notebook: String, why: String },
}

/// A notebook, as the heal needs to see it.
pub(crate) struct HealNotebook {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub archived: bool,
}

/// The plan, as a pure function over what is on disk and in the store.
///
/// Age decides every tie, and age means the notebook's `created_at`, not the
/// binding's: a binding carries no clock, and in every duplication seen the
/// original notebook is the older row and the duplicate was minted by an
/// import a moment ago. So the older notebook keeps the folder.
pub(crate) fn heal_plan(
    bindings: &HashMap<String, OkfBinding>,
    notebooks: &[HealNotebook],
    // `declared` maps a binding path to the `alchemy.id` that folder's
    // `index.md` names — who the folder says it belongs to.
    declared: &HashMap<String, String>,
) -> Vec<HealStep> {
    let by_id: HashMap<&str, &HealNotebook> =
        notebooks.iter().map(|n| (n.id.as_str(), n)).collect();
    // Oldest first, then by id, so a plan is the same plan twice.
    let rank = |id: &str| {
        by_id
            .get(id)
            .map(|n| (n.created_at, n.id.clone()))
            .unwrap_or((i64::MAX, id.to_string()))
    };
    let mut steps: Vec<HealStep> = Vec::new();
    let mut unbound: std::collections::HashSet<String> = std::collections::HashSet::new();
    let unbind = |steps: &mut Vec<HealStep>,
                  unbound: &mut std::collections::HashSet<String>,
                  id: &str,
                  why: String| {
        if unbound.insert(id.to_string()) {
            steps.push(HealStep::Unbind {
                notebook: id.to_string(),
                why,
            });
        }
    };

    // 1. Two notebooks over one folder. Two manifests and two writers over
    //    one file is the thing §5.6 forbids; the older binding keeps it and
    //    the newcomer is a copy, so it is unbound and hidden.
    let mut per_folder: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for (notebook, binding) in bindings {
        per_folder
            .entry(same_folder(&binding.path))
            .or_default()
            .push(notebook.clone());
    }
    let mut folders: Vec<(&PathBuf, &Vec<String>)> = per_folder.iter().collect();
    folders.sort_by(|a, b| a.0.cmp(b.0));
    for (folder, ids) in folders {
        if ids.len() < 2 {
            continue;
        }
        let mut ids = ids.clone();
        ids.sort_by_key(|id| rank(id));
        let keeper = ids[0].clone();
        for id in &ids[1..] {
            let why = format!(
                "{} is notebook {keeper}'s bundle, and two writers must not share one",
                folder.display()
            );
            unbind(&mut steps, &mut unbound, id, why.clone());
            if by_id.get(id.as_str()).is_some_and(|n| !n.archived) {
                steps.push(HealStep::Archive {
                    notebook: id.clone(),
                    why,
                });
            }
        }
    }

    // 2. A starter notebook is not kept on disk by default. Every install
    //    seeds its own, so a bound one is a bundle two Macs will trade.
    let mut bound: Vec<&String> = bindings.keys().collect();
    bound.sort();
    for id in &bound {
        if by_id
            .get(id.as_str())
            .is_some_and(|n| crate::examples::is_starter_title(&n.title))
        {
            unbind(
                &mut steps,
                &mut unbound,
                id,
                "a starter notebook is the app's own sample, not a document to sync".into(),
            );
        }
    }

    // 3. A folder that says it belongs to somebody else, and that somebody
    //    already keeps itself on disk elsewhere. The folder is a duplicate of
    //    their bundle; this notebook has no business writing into it.
    for id in &bound {
        let Some(binding) = bindings.get(*id) else {
            continue;
        };
        let Some(owner) = declared.get(&binding.path) else {
            continue;
        };
        if owner == *id || !by_id.contains_key(owner.as_str()) {
            continue;
        }
        if bindings.contains_key(owner) {
            unbind(
                &mut steps,
                &mut unbound,
                id,
                format!("{} is notebook {owner}'s bundle", binding.path),
            );
        }
    }

    // 4. A second copy of a starter, imported from the other Mac's bundle
    //    before rule 2 existed. The oldest stays; the rest are hidden, never
    //    deleted — a person may have added to one.
    let mut per_title: HashMap<&str, Vec<&HealNotebook>> = HashMap::new();
    for nb in notebooks.iter().filter(|n| !n.archived) {
        if crate::examples::is_starter_title(&nb.title) {
            per_title.entry(nb.title.as_str()).or_default().push(nb);
        }
    }
    let mut titles: Vec<&&str> = per_title.keys().collect();
    titles.sort();
    for title in titles {
        let mut copies = per_title[*title].clone();
        if copies.len() < 2 {
            continue;
        }
        copies.sort_by_key(|n| (n.created_at, n.id.clone()));
        for nb in &copies[1..] {
            let why = format!("a second copy of the starter notebook \u{201c}{title}\u{201d}");
            if !steps
                .iter()
                .any(|s| matches!(s, HealStep::Archive { notebook, .. } if notebook == &nb.id))
            {
                steps.push(HealStep::Archive {
                    notebook: nb.id.clone(),
                    why,
                });
            }
        }
    }
    steps
}

/// Run the plan, once, at launch. Best-effort: a heal that cannot read the
/// store leaves everything as it found it.
pub(crate) async fn heal_bindings(state: &AppState) {
    let data_dir = app_data_dir(state);
    let bindings = load_bindings(&data_dir);
    let Ok(rows) = state.db.list_notebooks().await else {
        return;
    };
    let notebooks: Vec<HealNotebook> = rows
        .into_iter()
        .map(|n| HealNotebook {
            id: n.id,
            title: n.title,
            created_at: n.created_at,
            archived: n.status == "archived",
        })
        .collect();
    // What each bound folder says about itself, read once.
    let declared: HashMap<String, String> = bindings
        .values()
        .filter_map(|b| {
            let text = std::fs::read_to_string(Path::new(&b.path).join("index.md")).ok()?;
            let id = parse_okf_doc(&text).nested("alchemy", "id")?;
            Some((b.path.clone(), id))
        })
        .collect();
    let title_of = |id: &str| {
        notebooks
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.title.clone())
            .unwrap_or_else(|| id.to_string())
    };
    for step in heal_plan(&bindings, &notebooks, &declared) {
        match step {
            HealStep::Unbind { notebook, why } => {
                let path = bindings
                    .get(&notebook)
                    .map(|b| b.path.clone())
                    .unwrap_or_default();
                cancel_pending_write(&notebook);
                set_binding(&data_dir, &notebook, None);
                okf_notice(format!(
                    "stopped keeping \u{201c}{}\u{201d} at {path} \u{2014} {why}. The folder is untouched.",
                    title_of(&notebook)
                ));
            }
            HealStep::Archive { notebook, why } => {
                if state
                    .db
                    .set_notebook_status(&notebook, "archived")
                    .await
                    .is_ok()
                {
                    okf_notice(format!(
                        "archived \u{201c}{}\u{201d} \u{2014} {why}. Nothing was deleted.",
                        title_of(&notebook)
                    ));
                }
            }
        }
    }
}

/// Say something worth reading later. `crate::note!` alone is stderr, which
/// is where 0.55.0's duplication went: the app log's last line that day was
/// the startup entry, and five bundles had been written since.
fn okf_notice(message: String) {
    crate::note!("okf: {message}");
    crate::diagnostics::record(
        crate::diagnostics::Event::new(crate::diagnostics::Level::Warn, "rust", "okf")
            .message(message),
    );
}

/// What this Mac already knows, as the found-bundle rule needs it. Read fresh
/// per folder: a bind that landed a moment ago has to count.
pub(crate) struct KnownNotebooks {
    /// Notebook id → title, for every notebook here.
    pub titles: HashMap<String, String>,
    /// Notebooks that already keep themselves on disk somewhere.
    pub bound: std::collections::HashSet<String>,
    /// The bundle folders those bindings point at, normalized.
    pub folders: std::collections::HashSet<PathBuf>,
}

fn known_notebooks(data_dir: &Path, notebooks: &[Notebook]) -> KnownNotebooks {
    let bindings = load_bindings(data_dir);
    KnownNotebooks {
        titles: notebooks
            .iter()
            .map(|n| (n.id.clone(), n.title.clone()))
            .collect(),
        bound: bindings.keys().cloned().collect(),
        folders: bindings.values().map(|b| same_folder(&b.path)).collect(),
    }
}

/// One path spelling per folder, so a symlinked or trailing-slash binding
/// still reads as the folder it is. Canonicalization is best-effort: a folder
/// that is gone compares by its literal path, which is the only thing left.
pub(crate) fn same_folder(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// What the Notebooks-root watcher should do with a bundle it found (§5.7).
#[derive(Debug, PartialEq)]
pub(crate) enum FoundBundle {
    /// Leave the folder exactly as it is, and say why.
    Skip(String),
    /// The same notebook arriving by another route: bind it here.
    Rebind(String),
    /// A notebook this Mac does not have: import it, then bind.
    Import,
}

/// The rule, as a decision, so the cases that went wrong on 0.55.0 are tested
/// rather than reasoned about.
///
/// Three things it will not do. It will not open a folder some notebook here
/// is already bound to — that is two writers over one file, which §5.6
/// forbids outright. It will not import a bundle whose `alchemy.id` names a
/// notebook this Mac has: that notebook is either unbound (so this folder is
/// its bundle, and it rebinds) or bound somewhere else (so this folder is a
/// duplicate of it, and duplicating the notebook to match would be the wrong
/// half to fix). And it will not open a starter notebook: every install seeds
/// its own copies under its own ids, so trading them between two Macs is a
/// loop that ends with everybody holding everybody's samples.
pub(crate) fn decide_bundle(
    folder: &Path,
    claimed_id: Option<&str>,
    claimed_title: Option<&str>,
    known: &KnownNotebooks,
) -> FoundBundle {
    if known.folders.contains(&same_folder(folder)) {
        return FoundBundle::Skip("a notebook here is already bound to it".into());
    }
    let starter = claimed_id
        .and_then(|id| known.titles.get(id))
        .map(String::as_str)
        .or(claimed_title)
        .is_some_and(crate::examples::is_starter_title);
    if starter {
        return FoundBundle::Skip("it is one of the app's own starter notebooks".into());
    }
    match claimed_id {
        Some(id) if known.bound.contains(id) => FoundBundle::Skip(format!(
            "notebook {id} already keeps itself on disk somewhere else"
        )),
        Some(id) if known.titles.contains_key(id) => FoundBundle::Rebind(id.to_string()),
        _ => FoundBundle::Import,
    }
}

/// Bundles sitting in the Notebooks folder that no notebook here is bound to
/// (§5.7). A second Mac's folder, a share, or simply what was already there
/// at first launch.
pub(crate) fn unopened_bundles(
    root: &Path,
    bound: &std::collections::HashSet<String>,
) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && !p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
                && !bound.contains(&p.to_string_lossy().to_string())
                // A folder that is not a bundle is somebody else's; leave it.
                && crate::commands::find_bundle_root(p.clone()).as_deref() == Ok(p.as_path())
        })
        .collect();
    out.sort();
    out
}

/// Open every bundle in the Notebooks folder that this Mac has not opened
/// yet: import, bind, and say so (§5.7).
///
/// A bundle whose root `index.md` names an `alchemy.id` this machine already
/// has is the same notebook arriving by another route — it rebinds to that
/// notebook rather than making a second copy of it.
pub(crate) async fn open_found_bundles(app: &AppHandle, state: &AppState) -> usize {
    let (root, _) = notebooks_home(state).await;
    if root.as_os_str().is_empty() || !root.is_dir() {
        return 0;
    }
    // One pass at a time. The minute tick and the root watcher's debounce
    // both call this, and two overlapping passes each see the same folder as
    // unbound — which on the 0.55.0 first launch imported bundles the seed
    // pass was still writing, as new notebooks.
    if OPENING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return 0;
    }
    let out = open_found_bundles_inner(app, state, &root).await;
    OPENING.store(false, std::sync::atomic::Ordering::SeqCst);
    out
}

static OPENING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

async fn open_found_bundles_inner(app: &AppHandle, state: &AppState, root: &Path) -> usize {
    let data_dir = app_data_dir(state);
    let bound: std::collections::HashSet<String> = load_bindings(&data_dir)
        .values()
        .map(|b| b.path.clone())
        .collect();
    let found = unopened_bundles(root, &bound);
    if found.is_empty() {
        return 0;
    }
    let notebooks = e(state.db.list_notebooks().await).unwrap_or_default();

    let mut opened = Vec::new();
    for folder in found {
        let path = folder.to_string_lossy().to_string();
        // Re-read the bindings for every folder: a bind may have landed
        // since the listing was taken, and acting on a stale map is how one
        // folder ended up with two notebooks writing into it.
        let known = known_notebooks(&data_dir, &notebooks);
        let index = std::fs::read_to_string(folder.join("index.md")).unwrap_or_default();
        let doc = parse_okf_doc(&index);
        let decision = decide_bundle(
            &folder,
            doc.nested("alchemy", "id").as_deref(),
            doc.str("title").as_deref(),
            &known,
        );
        let outcome = match decision {
            FoundBundle::Skip(why) => {
                crate::note!("okf: left {path} alone: {why}");
                continue;
            }
            // The same notebook by another route — the other Mac's copy, a
            // share, a folder moved — rebinds rather than duplicating.
            FoundBundle::Rebind(id) => {
                set_binding(
                    &data_dir,
                    &id,
                    Some(OkfBinding {
                        path: path.clone(),
                        id: new_id(),
                        last_write_at: 0,
                    }),
                );
                write_bound(state, &id).await.map(|_| id)
            }
            // Import creates the notebook — reusing the bundle's own
            // `alchemy.id` when nothing here claims it — and the binding is
            // recorded before the first write, so the writer's own output
            // can never read as a second arrival.
            FoundBundle::Import => {
                match crate::commands::import_bundle(app, state, folder.clone(), None).await {
                    Ok(nb) => {
                        set_binding(
                            &data_dir,
                            &nb.id,
                            Some(OkfBinding {
                                path: path.clone(),
                                id: new_id(),
                                last_write_at: 0,
                            }),
                        );
                        write_bound(state, &nb.id).await.map(|_| nb.id)
                    }
                    Err(err) => Err(err),
                }
            }
        };
        match outcome {
            Ok(_) => {
                let name = folder
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                crate::note!("okf: opened {name} from the Notebooks folder");
                opened.push(name);
            }
            Err(err) => crate::diagnostics::error("okf", format!("could not open {path}: {err}")),
        }
    }
    if !opened.is_empty() {
        // One announcement, however many arrived: forty folders on a first
        // launch is one event, not forty.
        let _ = app.emit(
            "okf://opened",
            serde_json::json!({ "count": opened.len(), "titles": opened }),
        );
        crate::commands::notify_changed("notebooks", None);
    }
    opened.len()
}

// ---- Originals in `references/` (docs/RFC-okf-live.md §6) -------------------

/// Types worth carrying: the ones whose bytes say something the extracted
/// text cannot. A PDF has pages, an image has a picture, a deck has slides.
/// Plain text and markdown are their own extraction, so copying them would
/// only duplicate the concept body.
const REFERENCE_EXTENSIONS: &[&str] = &[
    "pdf", "docx", "docm", "doc", "rtf", "odt", "pptx", "pptm", "ppt", "odp", "epub", "xlsx",
    "xls", "xlsm", "xlsb", "ods", "png", "jpg", "jpeg", "jpe", "webp", "gif", "bmp", "tif", "tiff",
    "heic", "heif", "avif", "jp2", "m4a", "mp3", "wav", "aiff",
];

/// The first 16 hex characters of the file's SHA-256 — 64 bits, which for a
/// notebook's worth of documents is a collision nobody will see.
///
/// It is an original's identity, not its name. The manifest maps it to the
/// file the bundle carries and `alchemy.sha256` repeats it in the concept, so
/// other tools can dedupe the same way; the filename stays the one the person
/// who made the file chose.
pub fn reference_hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// How long a reference name may get, in bytes. macOS allows 255; a shorter
/// cap leaves room for a `-2` and keeps a log line readable.
const REFERENCE_NAME_MAX: usize = 120;

/// Split a filename into stem and suffix. A name with no dot, and a name that
/// is only a suffix, are all stem.
fn split_reference_name(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, ext),
        _ => (name, ""),
    }
}

/// `paper.pdf` at 2 is `paper-2.pdf`.
fn numbered_reference_name(name: &str, n: u32) -> String {
    let (stem, ext) = split_reference_name(name);
    if ext.is_empty() {
        format!("{stem}-{n}")
    } else {
        format!("{stem}-{n}.{ext}")
    }
}

/// A name from the hash-named layout this branch shipped first: sixteen hex
/// characters and a suffix.
fn is_hash_name(name: &str) -> bool {
    let (stem, _) = split_reference_name(name);
    stem.len() == 16 && stem.chars().all(|c| c.is_ascii_hexdigit())
}

/// The name an original travels under: its own.
///
/// A bundle is meant to be read by a person as well as by a program, and
/// `2018 488 Spider brochure.pdf` says what `14030e98bcc8daf5.pdf` cannot. So
/// the filename is kept as it was — spaces, case and unicode included — and
/// only what a filesystem or a path parser would choke on comes out:
/// separators, control characters, a leading dot that would hide the file,
/// and any length past the cap. A source whose origin has no filename of its
/// own — a clipboard image, a captured page — falls back to its slug.
fn reference_name(path: &Path, fallback_stem: &str, ext: &str) -> String {
    let raw: String = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':'))
        .collect();
    let raw = raw.trim().trim_start_matches('.').trim();
    let (stem, suffix) = split_reference_name(raw);
    let suffix = if suffix.is_empty() { ext } else { suffix };
    let mut stem = stem.trim().to_string();
    if stem.is_empty() {
        stem = fallback_stem.trim().to_string();
    }
    while !stem.is_empty() && stem.len() + suffix.len() + 1 > REFERENCE_NAME_MAX {
        stem.pop();
    }
    let stem = stem.trim_end().to_string();
    let stem = if stem.is_empty() {
        "untitled".to_string()
    } else {
        stem
    };
    if suffix.is_empty() {
        stem
    } else {
        format!("{stem}.{suffix}")
    }
}

/// Why a source's original is, or is not, in the bundle (§6's table).
#[derive(Debug, Clone, PartialEq)]
pub enum ReferencePlan {
    /// Copy the bytes in under this name, deduplicated by this hash.
    Copy {
        name: String,
        hash: String,
        from: PathBuf,
    },
    /// Already inside the bundle: cite it where it lies.
    Inside { rel: String },
    /// Leave the bytes where they are; `resource:` stays provenance.
    Link { reason: &'static str },
}

/// Decide what to do with one source's original.
///
/// The question the table answers is whether the bundle is the sensible home
/// for the bytes. A file the user dragged in has no other home the far side
/// can reach. A clipped page or pasted text *is* its capture, and a URL is
/// re-fetchable. A folder or repo child belongs to a parent that resyncs, and
/// copying a synced folder into a synced folder duplicates it forever.
pub fn plan_reference(source: &Source, bundle: &Path, cap_bytes: u64) -> ReferencePlan {
    if !source.parent_id.is_empty() {
        return ReferencePlan::Link {
            reason: "a folder child; its parent is the origin and resyncs",
        };
    }
    if source.url.is_empty() || is_web_url(&source.url) {
        return ReferencePlan::Link {
            reason: "the concept body is the capture",
        };
    }
    let path = Path::new(&source.url);
    // A file already in the bundle is already here.
    if let Ok(rel) = path.strip_prefix(bundle) {
        return ReferencePlan::Inside {
            rel: rel.to_string_lossy().to_string(),
        };
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !REFERENCE_EXTENSIONS.contains(&ext.as_str()) {
        return ReferencePlan::Link {
            reason: "the text is the whole document",
        };
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return ReferencePlan::Link {
            reason: "the original is not on this machine",
        };
    };
    if cap_bytes == 0 || meta.len() > cap_bytes {
        return ReferencePlan::Link {
            reason: "over the size cap",
        };
    }
    let Ok(bytes) = std::fs::read(path) else {
        return ReferencePlan::Link {
            reason: "the original could not be read",
        };
    };
    ReferencePlan::Copy {
        name: reference_name(path, &okf_slug(&source.title), &ext),
        hash: reference_hash(&bytes),
        from: path.to_path_buf(),
    }
}

/// The name a reference the bundle already holds should be cited under, or
/// `None` when the bundle does not hold it any more and the bytes have to be
/// copied again.
///
/// A `held` that is only a hash takes `want` — the original's own name — in
/// one `rename(2)`, which is the migration off the hash-named layout and
/// which git reads as a move rather than a delete and an add.
fn settle_reference(dir: &Path, held: &str, want: &str, hash: &str) -> Option<String> {
    let at = dir.join(held);
    if is_evicted_stub(&at) {
        // Not downloaded, so not renameable — and not missing either. Ask for
        // it and keep the name; the write after it lands migrates it.
        hydrate_if_evicted(&at);
        return Some(held.to_string());
    }
    if !at.exists() {
        return None;
    }
    if held == want || !is_hash_name(held) {
        return Some(held.to_string());
    }
    let dest = free_reference_name(dir, want, hash);
    match std::fs::rename(&at, dir.join(&dest)) {
        Ok(()) => Some(dest),
        Err(err) => {
            crate::note!("okf: {at:?} would not take its original name ({err})");
            Some(held.to_string())
        }
    }
}

/// The first of `paper.pdf`, `paper-2.pdf`, … that is free — or that already
/// holds exactly these bytes, since one file per original is the point.
fn free_reference_name(dir: &Path, name: &str, hash: &str) -> String {
    let mut candidate = name.to_string();
    for n in 2..100u32 {
        let at = dir.join(&candidate);
        // A stub is the other Mac's copy of the same name, not a rival file.
        if is_evicted_stub(&at) || !at.exists() {
            return candidate;
        }
        if std::fs::read(&at)
            .map(|bytes| reference_hash(&bytes) == hash)
            .unwrap_or(false)
        {
            return candidate;
        }
        candidate = numbered_reference_name(name, n);
    }
    candidate
}

/// Copy an original into `references/` under its own name, unless the bundle
/// already carries those bytes.
///
/// Identity is the content hash and the manifest maps it to the file that
/// holds it: two sources over the same file share one copy, and every write
/// after the first is a lookup rather than a copy. The name is the original
/// file's, so a different file that happens to be called the same thing lands
/// as `<stem>-2.<ext>` instead of overwriting it. Returns the bundle-relative
/// path.
fn place_reference(
    bundle: &Path,
    manifest: &mut OkfManifest,
    name: &str,
    hash: &str,
    from: &Path,
) -> Result<String, String> {
    let dir = bundle.join("references");
    std::fs::create_dir_all(&dir).map_err(|err| format!("Failed to create {dir:?}: {err}"))?;
    // Already carried: cite it where it lies.
    if let Some(prior) = manifest.references.get(hash).cloned() {
        if let Some(rel) = settle_reference(&dir, &prior, name, hash) {
            manifest.references.insert(hash.to_string(), rel.clone());
            return Ok(format!("references/{rel}"));
        }
    }
    // A bundle this branch's earlier builds wrote has the bytes under the
    // hash, and no manifest entry for them. Rename rather than copy a second
    // time, so one write-through migrates the folder and leaves no duplicate.
    let ext = split_reference_name(name).1;
    let legacy = if ext.is_empty() {
        hash.to_string()
    } else {
        format!("{hash}.{ext}")
    };
    if legacy != name {
        if let Some(rel) = settle_reference(&dir, &legacy, name, hash) {
            manifest.references.insert(hash.to_string(), rel.clone());
            return Ok(format!("references/{rel}"));
        }
    }
    let dest_name = free_reference_name(&dir, name, hash);
    let dest = dir.join(&dest_name);
    if is_evicted_stub(&dest) {
        // The bytes are here, just not downloaded. Ask for them rather than
        // writing a second copy over the placeholder.
        hydrate_if_evicted(&dest);
    } else if !dest.exists() {
        std::fs::copy(from, &dest)
            .map_err(|err| format!("Failed to copy {from:?} into the bundle: {err}"))?;
    }
    manifest
        .references
        .insert(hash.to_string(), dest_name.clone());
    Ok(format!("references/{dest_name}"))
}

/// Drop references nothing points at any more.
///
/// "Points at" means a concept this pass wrote, and "ours" means a name the
/// manifest recorded the writer choosing — which is where the claim has to
/// live now that files are named after their originals rather than their
/// bytes. A `handout.pdf` someone put in `references/` by hand is still not
/// ours to remove.
fn prune_references(
    bundle: &Path,
    manifest: &mut OkfManifest,
    claimed: &std::collections::HashSet<String>,
) -> usize {
    let ours: std::collections::HashSet<String> = manifest.references.values().cloned().collect();
    manifest.references.retain(|_, name| claimed.contains(name));
    let dir = bundle.join("references");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || claimed.contains(&name) {
            continue;
        }
        if ours.contains(&name) && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

// ---- The binding (docs/RFC-okf-live.md §5.1) --------------------------------

/// Where a notebook keeps itself on disk. Machine-local: a path means nothing
/// on another machine, so this never becomes a store column.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkfBinding {
    pub path: String,
    /// This binding's own id, and the name of its manifest file. Minted per
    /// bind rather than reusing the notebook id: rebinding to a different
    /// folder must start from a clean record, not inherit the old one's
    /// paths and hashes.
    #[serde(default)]
    pub id: String,
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
    /// The file came in from disk rather than out of the writer's slug, so
    /// the writer keeps writing to it and never renames it (§5.3).
    ///
    /// A hand-added `notes/hand-added.md` used to become a note *and* a
    /// second file at the writer's own slug, leaving the original unclaimed
    /// for the next pass to import again — three notes became fifty-five in
    /// five minutes. Read-back claims the path it read, and this is the flag
    /// that says so: the file is the concept, and its name is its owner's.
    #[serde(default)]
    pub adopted: bool,
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
    /// Content hash → the file in `references/` that holds those bytes (§6).
    /// Originals are named after the originals, so the hash that dedupes them
    /// lives here rather than in the filename — and this map is also what
    /// says which names in `references/` the writer chose and may remove.
    #[serde(default)]
    pub references: HashMap<String, String>,
}

/// Where a binding's manifest lives: `<app-data>/okf/<binding-id>.json`,
/// outside the bundle (§5.6).
///
/// It used to sit in `.alchemy/manifest.json` inside the bundle, which is
/// wrong the moment the folder is shared. Entity ids are this machine's;
/// another Mac binding the same folder must never read them, and two writers
/// must never contend for one file. Out here, each install keeps its own
/// record and the bundle carries nothing machine-shaped at all.
pub fn manifest_path(data_dir: &Path, binding_id: &str) -> PathBuf {
    data_dir.join("okf").join(format!("{binding_id}.json"))
}

pub fn load_manifest(path: &Path) -> OkfManifest {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_manifest(path: &Path, manifest: &OkfManifest) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(manifest) {
        let _ = std::fs::write(path, json);
    }
}

/// The in-bundle manifest this branch's earlier builds wrote. Adopted once
/// on bind and then deleted, so an existing bound folder keeps its hashes
/// instead of rewriting every file — and stops carrying machine state.
pub(crate) fn adopt_legacy_manifest(bundle: &Path, manifest: &Path) {
    let legacy = bundle.join(".alchemy");
    let legacy_file = legacy.join("manifest.json");
    if !legacy_file.exists() {
        return;
    }
    if !manifest.exists() {
        if let Some(dir) = manifest.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::copy(&legacy_file, manifest);
    }
    let _ = std::fs::remove_dir_all(&legacy);
    crate::note!("okf: adopted the in-bundle manifest at {legacy_file:?} and removed it");
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
        // Unbound while the debounce ran: nothing to write, and no error to
        // report — the user asked for exactly this.
        if binding_for(&data_dir, &id).is_some() {
            if let Err(err) = write_bound(&state, &id).await {
                crate::diagnostics::error("okf", format!("bundle write failed: {err}"));
            }
        }
        if let Ok(mut running) = flushing().lock() {
            running.remove(&id);
        }
    });
}

/// Drop a notebook's pending write. The flusher checks the binding again
/// before it writes, so a cancelled notebook costs one map lookup and no
/// file. Used by the unbind and by the self-heal, which must not leave a
/// writer aimed at a folder it just released.
pub(crate) fn cancel_pending_write(notebook_id: &str) {
    if let Ok(mut map) = pending().lock() {
        map.remove(notebook_id);
    }
}

/// Bring a bound notebook's bundle up to date. The seed pass and every write
/// after it are the same pass — a bundle nobody has written yet simply has an
/// empty manifest, so every concept counts as changed.
pub async fn write_bound(state: &AppState, notebook_id: &str) -> Result<OkfWrite, String> {
    let data_dir = app_data_dir(state);
    let binding = binding_for(&data_dir, notebook_id)
        .ok_or_else(|| "This notebook isn't kept on disk".to_string())?;
    let bundle = PathBuf::from(&binding.path);
    let manifest = manifest_path(&data_dir, &binding.id);
    let (notebook, sources, notes) = gather_bundle_for(state, notebook_id, &bundle).await?;
    let written = write_bundle(&notebook, &sources, &notes, &bundle, Some(&manifest))?;
    set_binding(
        &data_dir,
        notebook_id,
        Some(OkfBinding {
            last_write_at: now_ms(),
            ..binding
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
    // The other half of the loop guard in `add_source_folder`: a folder this
    // notebook already reads as a source cannot also be where it writes.
    if e(state.db.list_sources(notebook_id).await)?
        .iter()
        .any(|s| s.url == path)
    {
        return Err(
            "This notebook already reads that folder as a source — pick a different one".into(),
        );
    }
    // A bundle already living here has content the notebook does not; take it
    // in before the writer starts treating this folder as its own.
    if crate::commands::find_bundle_root(bundle.clone()).is_ok() {
        crate::commands::import_bundle(app, state, bundle.clone(), Some(notebook_id.to_string()))
            .await?;
    }
    let data_dir = app_data_dir(state);
    let id = new_id();
    let manifest = manifest_path(&data_dir, &id);
    // Earlier builds of this branch kept the manifest inside the bundle.
    // Take it over so a folder already bound keeps its hashes instead of
    // rewriting every file, then leave the bundle machine-state-free.
    adopt_legacy_manifest(&bundle, &manifest);
    set_binding(
        &data_dir,
        notebook_id,
        Some(OkfBinding {
            path: path.to_string(),
            id,
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

// ---- Read-back (§5.3, §5.4) -------------------------------------------------

/// What one reconcile pass took in from disk.
#[derive(Debug, Default, PartialEq)]
pub struct OkfReconcile {
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
    /// Files that changed on disk but lost the conflict — their text is in
    /// `log.md`, and the next write puts the app's version back.
    pub overruled: usize,
}

impl OkfReconcile {
    pub fn changed(&self) -> bool {
        self.created + self.updated + self.deleted + self.overruled > 0
    }
}

/// What one file on disk asks the reconciler to do — the table in §5.3,
/// pulled out as a decision so it can be tested without a store.
#[derive(Debug, PartialEq)]
pub enum OkfAction {
    /// Not in the manifest: this is somebody's new document.
    Create,
    /// In the manifest, hash moved: an outside edit to a known entity.
    Update(String),
    /// In the manifest, hash matches: our own write echoing back.
    Echo,
}

/// Classify one bundle-relative file against the manifest.
pub fn classify(rel: &str, hash: &str, manifest: &OkfManifest) -> OkfAction {
    match manifest
        .concepts
        .iter()
        .find(|(_, entry)| entry.path == rel)
    {
        Some((_, entry)) if entry.hash == hash => OkfAction::Echo,
        Some((id, _)) => OkfAction::Update(id.clone()),
        None => OkfAction::Create,
    }
}

/// Last writer wins by clock (§5.4). A tie goes to disk: the file is what a
/// person or an agent just saved, and it is the artifact they can see.
pub fn disk_wins(file_mtime: i64, entity_updated_at: i64) -> bool {
    file_mtime >= entity_updated_at
}

/// Is Alchemy's own write for this notebook still in flight? Reconciling
/// mid-write would read half a bundle and call it an outside edit.
fn write_in_flight(notebook_id: &str) -> bool {
    pending()
        .lock()
        .map(|m| m.contains_key(notebook_id))
        .unwrap_or(true)
        || flushing()
            .lock()
            .map(|s| s.contains(notebook_id))
            .unwrap_or(true)
}

/// Every concept file under a bundle directory, at any depth, in a stable
/// order — the same allowlist the source walk uses, so the reconciler and
/// the ingest side agree on what a bundle contains.
fn concept_files(bundle: &Path, dir: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        if depth > 8 {
            return;
        }
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => walk(&path, out, depth + 1),
                Ok(t) if t.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    let mut found = Vec::new();
    walk(&bundle.join(dir), &mut found, 0);
    let mut out: Vec<PathBuf> = found
        .into_iter()
        .filter(|p| is_okf_concept(bundle, &p.to_string_lossy()))
        .collect();
    out.sort();
    out
}

/// Concept files iCloud has evicted, as the stub paths that stand in for
/// them (§5.6).
///
/// An undownloaded file is replaced by a hidden `.name.icloud` stub, which
/// the allowlist skips — correctly, since there is nothing to read. But then
/// a note written on the other Mac would sit unread until someone opened the
/// folder in Finder, which is not a sync story. Folder sources already nudge
/// these; bound roots use the same nudge, and the file reconciles on the
/// pass after it lands.
fn evicted_concepts(bundle: &Path, dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(bundle.join(dir)) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(real) = name
            .strip_prefix('.')
            .and_then(|n| n.strip_suffix(".icloud"))
            .filter(|n| n.ends_with(".md") && !is_okf_reserved(n))
        else {
            continue;
        };
        // Only a stub standing in for a file that is genuinely not here.
        if !bundle.join(dir).join(real).exists() {
            out.push(entry.path().to_string_lossy().to_string());
        }
    }
    out
}

fn file_mtime_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Who a concept file says last wrote it, when that is somebody other than
/// this app or this person.
///
/// Our own writes stamp `alchemy/<version>` or `human:<this account>`, so
/// either by-line means nobody else claimed the file — a deliberate edit,
/// which clears the note's origin exactly as an in-app edit does (§5.3).
/// That covers the two-Mac case on purpose: the same person editing on the
/// other machine is still that person, not a stranger to attribute. Anyone
/// else — `human:kim`, `okf-pipeline/2.1` — keeps their name.
fn outside_actor(doc: &OkfDoc) -> String {
    match doc.nested("generated", "by") {
        Some(by) if !okf_is_ours(&by) => by,
        _ => String::new(),
    }
}

/// Reconcile a bound notebook against its bundle (§5.3).
///
/// Echo suppression is the hash, not a timer: the writer records what it
/// wrote before it writes, so a watcher event for our own file compares equal
/// and stops here. Everything else is the table in §5.3 — a file the manifest
/// has never heard of becomes an entity, a file whose hash moved updates one,
/// and a file that is gone deletes one.
pub async fn reconcile(state: &AppState, notebook_id: &str) -> Result<OkfReconcile, String> {
    let data_dir = app_data_dir(state);
    let Some(binding) = binding_for(&data_dir, notebook_id) else {
        return Ok(OkfReconcile::default());
    };
    if write_in_flight(notebook_id) {
        return Ok(OkfReconcile::default());
    }
    let bundle = PathBuf::from(&binding.path);
    if !bundle.is_dir() {
        return Ok(OkfReconcile::default());
    }
    let manifest_at = manifest_path(&data_dir, &binding.id);
    let mut manifest = load_manifest(&manifest_at);
    // Path → entity id, the direction the reconciler reads in.
    let by_path: HashMap<String, String> = manifest
        .concepts
        .iter()
        .map(|(id, entry)| (entry.path.clone(), id.clone()))
        .collect();
    let mut out = OkfReconcile::default();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut losers: Vec<String> = Vec::new();
    // Did this pass take a file in, and so change the manifest?
    let mut adopted = false;

    // Ask for anything the cloud has evicted before reading what is here, so
    // the next pass finds it (§5.6). Bounded by the same cap folder scans use.
    #[cfg(target_os = "macos")]
    {
        let mut stubs: Vec<String> = ["sources", "notes"]
            .iter()
            .flat_map(|dir| evicted_concepts(&bundle, dir))
            .collect();
        stubs.truncate(crate::commands::ICLOUD_HYDRATE_CAP);
        crate::commands::hydrate_icloud_stubs(stubs);
    }

    for dir in ["sources", "notes"] {
        for path in concept_files(&bundle, dir) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // The whole bundle-relative path, not just the filename: the
            // allowlist reads `sources/**.md`, so two concepts can share a
            // name at different depths and the manifest has to tell them
            // apart — it claims this exact path in a moment.
            let rel = path
                .strip_prefix(&bundle)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| {
                    format!(
                        "{dir}/{}",
                        path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or_default()
                    )
                });
            let hash = okf_hash(&text);
            let action = classify(&rel, &hash, &manifest);
            let known = match &action {
                OkfAction::Update(id) => {
                    seen.insert(id.clone());
                    Some(id.clone())
                }
                // Our own write, echoing back through the watcher.
                OkfAction::Echo => {
                    if let Some(id) = by_path.get(&rel) {
                        seen.insert(id.clone());
                    }
                    continue;
                }
                OkfAction::Create => None,
            };
            let doc = parse_okf_doc(&text);
            let mtime = file_mtime_ms(&path);
            match (dir, known) {
                ("notes", None) => {
                    if let Some(id) = take_in_note(state, notebook_id, &doc, &path).await? {
                        adopt(&mut manifest, &id, &rel, &hash, &doc);
                        adopted = true;
                        out.created += 1;
                    }
                }
                ("sources", None) => {
                    if let Some(id) =
                        take_in_source(state, notebook_id, &doc, &path, &bundle).await?
                    {
                        adopt(&mut manifest, &id, &rel, &hash, &doc);
                        adopted = true;
                        out.created += 1;
                    }
                }
                ("notes", Some(id)) => {
                    match update_note_from_disk(state, &id, &doc, mtime).await? {
                        Verdict::Applied => out.updated += 1,
                        Verdict::Overruled(text) => {
                            out.overruled += 1;
                            losers.push(format!("{rel}\n\n{text}"));
                        }
                        Verdict::Gone => {}
                    }
                }
                ("sources", Some(id)) => {
                    match update_source_from_disk(state, &id, &doc, &path, mtime).await? {
                        Verdict::Applied => out.updated += 1,
                        Verdict::Overruled(text) => {
                            out.overruled += 1;
                            losers.push(format!("{rel}\n\n{text}"));
                        }
                        Verdict::Gone => {}
                    }
                }
                _ => {}
            }
        }
    }

    // A concept file that is gone takes its entity with it.
    let vanished: Vec<(String, String)> = manifest
        .concepts
        .iter()
        .filter(|(id, entry)| !seen.contains(*id) && !bundle.join(&entry.path).exists())
        .map(|(id, entry)| (id.clone(), entry.path.clone()))
        .collect();
    for (id, rel) in vanished {
        let deleted = if rel.starts_with("notes/") {
            state.db.delete_note(&id).await.is_ok()
        } else {
            state.db.delete_source(&id).await.is_ok()
        };
        if deleted {
            manifest.concepts.remove(&id);
            out.deleted += 1;
            crate::note!("okf: {rel} was deleted on disk; removed it here too");
        }
    }
    if out.deleted > 0 || adopted {
        save_manifest(&manifest_at, &manifest);
    }

    // Nothing is lost silently (§5.4): the text that lost the race is written
    // into the log beside the entry that recorded the overwrite.
    if !losers.is_empty() {
        let _ = okf_log_append(
            &bundle,
            &format!(
                "Kept the app's newer version of {} file(s); the disk text follows.\n\n```\n{}\n```",
                losers.len(),
                losers.join("\n\n---\n\n")
            ),
        );
        // The app's version wins, so put it back on disk.
        schedule_write(notebook_id);
    }
    if out.changed() {
        crate::commands::notify_changed("sources", Some(notebook_id));
    }
    Ok(out)
}

/// Claim the file a `Create` came from for the entity it became.
///
/// This is the fix for the duplication loop (§5.3): without it the writer
/// would put the new entity at *its* slug, leave the incoming file
/// unclaimed, and the next reconcile would take the same file in all over
/// again. The path the reconciler read is the entity's path from here on,
/// and `adopted` tells the writer never to re-slug it. The hash is the file
/// as it stands, so the very next pass reads it as an echo rather than an
/// edit, and the frontmatter keys we do not write ride along the way an
/// outside edit's always have.
pub(crate) fn adopt(manifest: &mut OkfManifest, id: &str, rel: &str, hash: &str, doc: &OkfDoc) {
    manifest.concepts.insert(
        id.to_string(),
        OkfManifestEntry {
            path: rel.to_string(),
            adopted: true,
            hash: hash.to_string(),
            wrote_at: 0,
            extra: doc.extra(),
        },
    );
}

/// Which side of a conflict won, and the text the loser had.
enum Verdict {
    Applied,
    Overruled(String),
    Gone,
}

async fn take_in_note(
    state: &AppState,
    notebook_id: &str,
    doc: &OkfDoc,
    path: &Path,
) -> Result<Option<String>, String> {
    if doc.body.trim().is_empty() {
        return Ok(None);
    }
    let ts = file_mtime_ms(path).max(1);
    let note = Note {
        id: new_id(),
        notebook_id: notebook_id.to_string(),
        title: doc.str("title").unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled note")
                .to_string()
        }),
        content: doc.body.clone(),
        // `alchemy.kind` is the machine name; `type:` is a human label
        // several kinds share, and only the fallback for another producer.
        kind: doc.nested("alchemy", "kind").unwrap_or_else(|| {
            crate::commands::note_kind_from_label(doc.str("type").as_deref().unwrap_or("Note"))
        }),
        prompt: String::new(),
        origin: outside_actor(doc),
        status: match doc.nested("alchemy", "status") {
            Some(recorded) => recorded,
            None if doc.str("status").as_deref() == Some("deprecated") => "archived".into(),
            None => String::new(),
        },
        created_at: ts,
        updated_at: ts,
    };
    e(crate::commands::add_note_indexed(state, &note).await)?;
    crate::note!("okf: took in note \"{}\" from disk", note.title);
    Ok(Some(note.id))
}

async fn take_in_source(
    state: &AppState,
    notebook_id: &str,
    doc: &OkfDoc,
    path: &Path,
    bundle: &Path,
) -> Result<Option<String>, String> {
    if doc.body.trim().is_empty() {
        return Ok(None);
    }
    let title = doc.str("title").unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled source")
            .to_string()
    });
    let resource = match doc.str("resource") {
        Some(r) if is_web_url(&r) => r,
        Some(r) => r.strip_prefix("file://").unwrap_or(&r).to_string(),
        None => String::new(),
    };
    // The bundle may carry the original (§6): re-extract it through the
    // ordinary file path so pages stay pages, and point the source at the
    // reference so Refresh and Show in Finder work. A reference the bundle
    // does not actually hold falls back to the concept body.
    let reference = crate::commands::okf_reference_path(bundle, &resource);
    let rich = match &reference {
        Some(file) => crate::commands::extract_any_file(state, &file.to_string_lossy())
            .await
            .ok()
            .map(|mut rich| {
                rich.title = title.clone();
                rich
            }),
        None => None,
    };
    let extracted = rich.unwrap_or(ingest::Extracted {
        feeds: Vec::new(),
        image_url: doc.nested("alchemy", "image_url").unwrap_or_default(),
        author: doc.nested("alchemy", "author").unwrap_or_default(),
        title: title.clone(),
        source_type: doc
            .nested("alchemy", "source_type")
            .unwrap_or_else(|| "markdown".into()),
        // The resource is provenance, and a web one stays refreshable.
        url: resource,
        text: doc.body.clone(),
    });
    // A duplicate is success, not failure — the same rule import follows.
    match crate::commands::store_extracted(state, notebook_id, extracted).await {
        Ok(landed) => {
            if let Some(tags) = doc.nested("alchemy", "tags") {
                let _ = state.db.set_source_tags(&landed.id, &tags).await;
            }
            crate::note!("okf: took in source \"{title}\" from disk");
            Ok(Some(landed.id))
        }
        Err(_) => Ok(None),
    }
}

async fn update_note_from_disk(
    state: &AppState,
    id: &str,
    doc: &OkfDoc,
    mtime: i64,
) -> Result<Verdict, String> {
    let Some(note) = e(state.db.get_note(id).await)? else {
        return Ok(Verdict::Gone);
    };
    // Last writer wins by clock (§5.4). A tie goes to disk: the file is what
    // a person or an agent just saved, and it is the visible artifact.
    if !disk_wins(mtime, note.updated_at) {
        return Ok(Verdict::Overruled(doc.body.clone()));
    }
    let title = doc.str("title").unwrap_or_else(|| note.title.clone());
    e(state.db.update_note(id, &title, &doc.body, mtime).await)?;
    // An edit that names its author keeps that attribution; one that does not
    // is a deliberate edit and takes ownership, exactly as an in-app edit does.
    e(state.db.set_note_origin(id, &outside_actor(doc)).await)?;
    e(state.db.set_note_status(id, "").await)?;
    if let Some(fresh) = e(state.db.get_note(id).await)? {
        crate::commands::index_note(state, &fresh).await;
    }
    crate::note!("okf: took in an edit to note \"{title}\"");
    Ok(Verdict::Applied)
}

async fn update_source_from_disk(
    state: &AppState,
    id: &str,
    doc: &OkfDoc,
    path: &Path,
    mtime: i64,
) -> Result<Verdict, String> {
    let Some(source) = e(state.db.get_source(id).await)? else {
        return Ok(Verdict::Gone);
    };
    // A source has no `updated_at`; `fetched_at` is when its text last came
    // in, which is the same question here.
    if !disk_wins(mtime, source.fetched_at.max(source.created_at)) {
        return Ok(Verdict::Overruled(doc.body.clone()));
    }
    let extracted = ingest::Extracted {
        feeds: Vec::new(),
        image_url: source.image_url.clone(),
        author: source.author.clone(),
        title: doc.str("title").unwrap_or_else(|| source.title.clone()),
        source_type: source.source_type.clone(),
        url: source.url.clone(),
        text: doc.body.clone(),
    };
    let title = extracted.title.clone();
    e(crate::commands::reingest(state, &source, extracted, None, true).await)?;
    crate::note!(
        "okf: re-read source \"{title}\" from disk ({})",
        path.display()
    );
    Ok(Verdict::Applied)
}

/// Reconcile every bound notebook. The closed sweep's share of the work
/// (§5.3): open notebooks get FSEvents, everything else gets the ten-minute
/// window the folder sweep already runs on.
pub async fn reconcile_all(state: &AppState) {
    let data_dir = app_data_dir(state);
    for notebook_id in load_bindings(&data_dir).keys() {
        if let Err(err) = reconcile(state, notebook_id).await {
            crate::diagnostics::error("okf", format!("reconcile failed: {err}"));
        }
    }
}

/// Is this path an iCloud eviction stub standing in for a file that is not
/// downloaded? macOS replaces the file with a hidden `.name.icloud`
/// placeholder, so the file itself simply is not there (§5.7).
pub fn is_evicted_stub(path: &Path) -> bool {
    if path.exists() {
        return false;
    }
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return false;
    };
    dir.join(format!(".{name}.icloud")).is_file()
}

/// Ask iCloud for a file that is not downloaded, and say so. Returns true
/// when a download was started, so the caller can show "Downloading from
/// iCloud…" instead of failing at a file that is only temporarily absent.
pub fn hydrate_if_evicted(path: &Path) -> bool {
    if !is_evicted_stub(path) {
        return false;
    }
    let Some(dir) = path.parent() else {
        return false;
    };
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    #[cfg(target_os = "macos")]
    crate::commands::hydrate_icloud_stubs(vec![dir
        .join(format!(".{name}.icloud"))
        .to_string_lossy()
        .to_string()]);
    crate::note!("okf: asked iCloud for {path:?}");
    true
}

/// The one-time upgrade offer's Keep button (§5.7): every active notebook
/// gets a folder and a seed pass, and the offer is not made again.
#[tauri::command]
pub async fn keep_notebooks_on_disk(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let bound = bind_all_notebooks(&app, &state).await?;
    answer_keep_offer(&app, &state, true).await?;
    Ok(bound)
}

/// Record that the offer has been answered, either way, so it is asked once.
#[tauri::command]
pub async fn dismiss_keep_on_disk_offer(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    answer_keep_offer(&app, &state, false).await
}

async fn answer_keep_offer(app: &AppHandle, state: &AppState, keep: bool) -> Result<(), String> {
    let mut config = state.ai.read().await.config().clone();
    config.keep_on_disk_asked = true;
    if keep {
        config.keep_on_disk = true;
    }
    crate::commands::apply_ai_config(app, state, config).await
}

/// Show a bound notebook's folder in Finder — the "Share folder…" verb
/// (§5.7). iCloud and Dropbox already share any folder, so the useful thing
/// the app can do is put the user in front of the right one; calling the
/// share sheet itself is native code and waits for a sidecar.
#[tauri::command]
pub async fn reveal_notebook_folder(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<String, String> {
    let binding = binding_for(&app_data_dir(&state), &notebook_id)
        .ok_or_else(|| "This notebook isn't kept on disk".to_string())?;
    Ok(binding.path)
}

/// Open every bundle sitting in the Notebooks folder that this Mac has not
/// opened yet (§5.7). Called at launch and whenever the root changes.
#[tauri::command]
pub async fn open_notebooks_folder_bundles(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    Ok(open_found_bundles(&app, &state).await)
}

// ---- The iCloud container (RFC-okf-live.md 5.7, stage two) ------------------
//
// Stage one put the Notebooks folder at `iCloud Drive/Alchemy/`: a plain
// folder, no entitlement, shipped in v0.55.0. Stage two is the app's own
// container, whose `Documents/` Finder shows as "Alchemy" at the iCloud Drive
// root with the app icon — the same container an iPhone app would read. The
// RFC calls the migration between them a folder move, and that is all it is:
// the bundles do not change, only where they sit and what the bindings
// sidecar says about it.

/// What the migration banner needs to know, and nothing else.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IcloudMoveOffer {
    /// Whether to make the offer at all.
    pub available: bool,
    pub from: String,
    pub to: String,
    /// How many bound notebooks would move.
    pub count: usize,
}

/// The bound bundles sitting directly in `root`, by notebook id, sorted so a
/// plan is the same plan twice.
///
/// Direct children only: a bundle deeper down was put there by hand, and its
/// name alone would not say where it belongs.
pub(crate) fn bound_under(root: &Path, bindings: &HashMap<String, OkfBinding>) -> Vec<String> {
    let mut ids: Vec<String> = bindings
        .iter()
        .filter(|(_, b)| Path::new(&b.path).parent() == Some(root))
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// Should the move be offered, and what would move?
///
/// Pure, so the decision can be tested without a signature, a container, or
/// anybody's real iCloud folder: the caller supplies the entitlement answer,
/// the answered flag, and the bindings.
pub(crate) fn icloud_move_plan(
    home: &Path,
    entitled: bool,
    asked: bool,
    notebooks_dir: &Path,
    bindings: &HashMap<String, OkfBinding>,
) -> IcloudMoveOffer {
    let to = crate::ai::icloud_container_documents(home);
    let mut offer = IcloudMoveOffer {
        from: notebooks_dir.to_string_lossy().to_string(),
        to: to.to_string_lossy().to_string(),
        ..Default::default()
    };
    // No entitlement, no container; asked once is asked.
    if !entitled || asked || notebooks_dir == to {
        return offer;
    }
    // Only the folder Alchemy chose is Alchemy's to propose moving. A
    // Notebooks folder the user pointed at Dropbox, at a second drive, or
    // anywhere else is their decision, and stage two is not a reason to
    // overrule it — Settings keeps the picker for anyone who wants this.
    if notebooks_dir != crate::ai::icloud_drive_alchemy(home) {
        return offer;
    }
    offer.count = bound_under(notebooks_dir, bindings).len();
    offer.available = offer.count > 0;
    offer
}

/// Where each bound bundle under `from` lands in the container.
///
/// `taken` carries the names already in the destination, so a collision gets
/// the exporter's `-2` treatment instead of landing on somebody's folder —
/// and so the planner stays a pure function of what it is told.
pub(crate) fn plan_icloud_moves(
    from: &Path,
    to: &Path,
    bindings: &HashMap<String, OkfBinding>,
    taken: &std::collections::HashSet<String>,
) -> Vec<(String, PathBuf, PathBuf)> {
    let mut taken = taken.clone();
    let mut out = Vec::new();
    for id in bound_under(from, bindings) {
        let Some(binding) = bindings.get(&id) else {
            continue;
        };
        let old = PathBuf::from(&binding.path);
        let Some(name) = old.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let mut slug = name.to_string();
        let mut n = 2;
        while taken.contains(&slug) {
            slug = format!("{name}-{n}");
            n += 1;
        }
        taken.insert(slug.clone());
        out.push((id, old, to.join(slug)));
    }
    out
}

/// Point the bindings sidecar at the folders' new homes. The binding id and
/// its manifest are untouched: this is the same bundle at a new path, not a
/// rebind, and a rebind would throw away every hash the reconciler has.
pub(crate) fn rebind_moved(
    bindings: &mut HashMap<String, OkfBinding>,
    moves: &[(String, PathBuf, PathBuf)],
) {
    for (id, _, new) in moves {
        if let Some(binding) = bindings.get_mut(id) {
            binding.path = new.to_string_lossy().to_string();
        }
    }
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// Is the migration on the table right now? Read by the banner at launch.
#[tauri::command]
pub async fn icloud_container_offer(state: State<'_, AppState>) -> Result<IcloudMoveOffer, String> {
    let (dir, asked) = {
        let ai = state.ai.read().await;
        let config = ai.config();
        (
            PathBuf::from(config.notebooks_dir.clone()),
            config.icloud_move_asked,
        )
    };
    Ok(icloud_move_plan(
        &home_dir(),
        crate::ai::bundle_has_icloud_container(),
        asked,
        &dir,
        &load_bindings(&app_data_dir(&state)),
    ))
}

/// Record that the offer has been answered without taking it.
#[tauri::command]
pub async fn dismiss_icloud_container_offer(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.ai.read().await.config().clone();
    config.icloud_move_asked = true;
    crate::commands::apply_ai_config(&app, &state, config).await
}

/// Nothing moves under a write in flight: renaming a bundle mid-write leaves
/// the writer holding a path that is no longer there and the bundle half
/// written. Wait for quiet, and refuse rather than move anyway.
async fn wait_for_quiet(ids: &[String]) -> Result<(), String> {
    for _ in 0..30 {
        if !ids.iter().any(|id| write_in_flight(id)) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err("Alchemy is still saving a notebook to disk. Try again in a moment.".to_string())
}

/// Copy a folder whole. Only reached when the rename could not be done in
/// place (a different volume), and it never removes the original: a move that
/// half-succeeded must leave the user with their files, not a gap.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

/// The offer's Move button: take every bound bundle out of `iCloud
/// Drive/Alchemy/` and into the app's container, repoint the bindings, and
/// make the container the Notebooks folder.
///
/// Returns how many folders moved. Nothing is deleted at any point — a
/// cross-volume move copies and leaves the original where it was, and says so
/// in the log rather than tidying up behind the user's back.
#[tauri::command]
pub async fn move_notebooks_to_icloud_container(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let data_dir = app_data_dir(&state);
    let home = home_dir();
    let (from, asked) = {
        let ai = state.ai.read().await;
        let config = ai.config();
        (
            PathBuf::from(config.notebooks_dir.clone()),
            config.icloud_move_asked,
        )
    };
    let mut bindings = load_bindings(&data_dir);
    let offer = icloud_move_plan(
        &home,
        crate::ai::bundle_has_icloud_container(),
        asked,
        &from,
        &bindings,
    );
    if !offer.available {
        return Err("There's nothing to move into the Alchemy iCloud folder.".to_string());
    }
    let to = PathBuf::from(&offer.to);

    wait_for_quiet(&bound_under(&from, &bindings)).await?;

    // Making the directory is what provisions the container: the entitlement
    // says the app may have it, and the first thing to ask for it gets it made.
    std::fs::create_dir_all(&to).map_err(|err| format!("Couldn't make {}: {err}", offer.to))?;
    let taken: std::collections::HashSet<String> = std::fs::read_dir(&to)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    let moves = plan_icloud_moves(&from, &to, &bindings, &taken);

    let mut done: Vec<(String, PathBuf, PathBuf)> = Vec::new();
    for (id, old, new) in moves {
        if std::fs::rename(&old, &new).is_ok() {
            done.push((id, old, new));
            continue;
        }
        match copy_tree(&old, &new) {
            Ok(()) => {
                crate::note!("okf: copied {old:?} to {new:?}; the original is still there");
                done.push((id, old, new));
            }
            Err(err) => {
                crate::diagnostics::error("okf", format!("could not move {old:?}: {err}"));
            }
        }
    }

    rebind_moved(&mut bindings, &done);
    save_bindings(&data_dir, &bindings);

    let mut config = state.ai.read().await.config().clone();
    config.notebooks_dir = offer.to.clone();
    config.icloud_move_asked = true;
    crate::commands::apply_ai_config(&app, &state, config).await?;
    // The watched root moved with them (5.7: the Notebooks folder is watched
    // whether or not a notebook is open).
    crate::fswatch::rearm(&app).await;
    crate::note!("okf: moved {} notebooks into {}", done.len(), offer.to);
    Ok(done.len())
}
