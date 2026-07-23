//! Local-file search backing the Add Source → "Search your Mac" step.
//!
//! Spotlight is queried through the `mdfind` CLI — there is no stable Rust
//! binding to the MDQuery C API worth the surface area, and a subprocess keeps
//! this out of the app's TCC story (mdfind inherits the app's index access).
//! Two passes run concurrently: a name match (`-name`) and a general
//! content/metadata match. Name hits rank first, ingestible types before the
//! rest, most-recently-modified within a tier. Junk paths (hidden/dotfiles,
//! Trash, caches, node_modules, most of ~/Library) are filtered out before we
//! ever stat them.
//!
//! Note: this indexes *and reads* the live Spotlight database — it is entirely
//! separate from `spotlight.rs`, which only *publishes* notebook items into
//! Core Spotlight for the OS to index.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;

/// One local file (or folder) hit, shaped for the results list.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHit {
    pub name: String,
    pub path: String,
    /// Lowercased extension without the dot ("" for extensionless / folders).
    pub ext: String,
    /// Coarse kind label for the row's chip ("Folder", "PDF", "Code", …).
    pub kind: String,
    pub is_dir: bool,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// mtime in unix millis (0 when unknown).
    pub mtime: i64,
    /// True when Alchemy has an extractor for this type — ranked first, and the
    /// only rows the frontend lets you click.
    pub ingestible: bool,
    /// Whether this came from the name pass (vs content/metadata). Internal to
    /// ranking; never crosses the IPC boundary.
    #[serde(skip)]
    pub name_hit: bool,
}

/// mdfind is fast against a warm index; this ceiling only catches a wedged
/// process (e.g. the index rebuilding).
const MDFIND_TIMEOUT: Duration = Duration::from_secs(4);

/// How many surviving paths to stat before ranking. Name hits fill this first,
/// so a flood of content hits can't starve them; the final list is capped far
/// below this by the caller's `limit`.
const POOL_CAP: usize = 160;

/// Live search over local files. Empty/whitespace query returns empty with no
/// subprocess spawned. `limit` is clamped to a sane window.
pub async fn search(query: &str, limit: usize) -> Vec<FileHit> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let limit = limit.clamp(1, 100);
    // GUI-spawned apps still inherit HOME; when absent we search the whole
    // index and simply can't apply the ~/Library filter.
    let home = std::env::var("HOME").unwrap_or_default();

    let (name_paths, content_paths) = tokio::join!(
        run_mdfind(name_args(q, &home)),
        run_mdfind(content_args(q, &home)),
    );

    let pool = merge_and_filter(name_paths, content_paths, &home, POOL_CAP);
    let hits: Vec<FileHit> = pool
        .into_iter()
        .filter_map(|(path, name_hit)| hit_from_path(&path, name_hit))
        .collect();
    rank_and_cap(hits, limit)
}

/// `mdfind` argv for the name pass. `-name` matches the display name only.
fn name_args(query: &str, home: &str) -> Vec<String> {
    let mut args = Vec::new();
    if !home.is_empty() {
        args.push("-onlyin".to_string());
        args.push(home.to_string());
    }
    args.push("-name".to_string());
    args.push(query.to_string());
    args
}

/// `mdfind` argv for the general content/metadata pass. A bare query string is
/// interpreted by Spotlight as a content + metadata search.
fn content_args(query: &str, home: &str) -> Vec<String> {
    let mut args = Vec::new();
    if !home.is_empty() {
        args.push("-onlyin".to_string());
        args.push(home.to_string());
    }
    args.push(query.to_string());
    args
}

/// Run `mdfind` and return its stdout as newline-split paths. Any failure
/// (missing binary off-macOS, timeout, non-zero exit) degrades to an empty
/// list — live search should never surface a subprocess error mid-keystroke.
async fn run_mdfind(args: Vec<String>) -> Vec<String> {
    let mut cmd = tokio::process::Command::new("mdfind");
    cmd.args(&args).kill_on_drop(true);
    match tokio::time::timeout(MDFIND_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        // Timeout (Err) or spawn/exec failure (Ok(Err)) — no results.
        _ => Vec::new(),
    }
}

/// Merge the two passes into a deduped candidate pool: name hits first (so the
/// cap keeps them), each junk-filtered, each path only once. A path present in
/// both passes keeps `name_hit = true`.
fn merge_and_filter(
    name_paths: Vec<String>,
    content_paths: Vec<String>,
    home: &str,
    pool_cap: usize,
) -> Vec<(String, bool)> {
    let mut seen = HashSet::new();
    let mut pool: Vec<(String, bool)> = Vec::new();
    for (paths, name_hit) in [(name_paths, true), (content_paths, false)] {
        for p in paths {
            if pool.len() >= pool_cap {
                break;
            }
            if is_junk(&p, home) {
                continue;
            }
            if !seen.insert(p.clone()) {
                continue;
            }
            pool.push((p, name_hit));
        }
    }
    pool
}

/// Paths we never want in the results: anything hidden (a leading-dot
/// component covers `.Trash`, `.git`, dotfiles, caches), vendored dependency
/// trees, and the bulk of `~/Library` — except the two roots that back cloud
/// drives (`CloudStorage` = Dropbox/Drive/OneDrive, `Mobile Documents` =
/// iCloud Drive), which hold real user documents.
fn is_junk(path: &str, home: &str) -> bool {
    for comp in Path::new(path).components() {
        if let std::path::Component::Normal(os) = comp {
            let s = os.to_string_lossy();
            if s.starts_with('.') {
                return true;
            }
            if s == "node_modules" {
                return true;
            }
        }
    }
    if !home.is_empty() {
        let lib = format!("{home}/Library");
        if path == lib || path.starts_with(&format!("{lib}/")) {
            let is_cloud = path.starts_with(&format!("{lib}/CloudStorage"))
                || path.starts_with(&format!("{lib}/Mobile Documents"));
            if !is_cloud {
                return true;
            }
        }
    }
    false
}

/// Stat a candidate into a `FileHit`. Returns `None` when the path has vanished
/// since Spotlight indexed it (a natural staleness filter).
fn hit_from_path(path: &str, name_hit: bool) -> Option<FileHit> {
    let meta = std::fs::metadata(path).ok()?;
    let is_dir = meta.is_dir();
    let ext = ext_of(path);
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some(FileHit {
        name: file_name_of(path),
        path: path.to_string(),
        kind: kind_label(path, is_dir),
        ingestible: is_ingestible(path, is_dir),
        is_dir,
        size: if is_dir { 0 } else { meta.len() },
        mtime,
        ext,
        name_hit,
    })
}

/// Sort the pool into presentation order and cut to `limit`. The sort is
/// stable, so name-first insertion order breaks ties inside a tier.
fn rank_and_cap(mut hits: Vec<FileHit>, limit: usize) -> Vec<FileHit> {
    hits.sort_by_key(rank_key);
    hits.truncate(limit);
    hits
}

/// Ordering key: ingestible-and-named first, then ingestible content hits,
/// then the un-ingestible remainder — newest first within each tier.
fn rank_key(h: &FileHit) -> (u8, std::cmp::Reverse<i64>) {
    let tier = match (h.ingestible, h.name_hit) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    };
    (tier, std::cmp::Reverse(h.mtime))
}

/// Can Alchemy turn this path into a source? Folders always (they become synced
/// folder sources); files when a rich extractor or the code path claims them —
/// mirrors `commands::RICH_EXTENSIONS` + `ingest::is_code_path`.
pub fn is_ingestible(path: &str, is_dir: bool) -> bool {
    if is_dir {
        return true;
    }
    let ext = ext_of(path);
    crate::commands::RICH_EXTENSIONS.contains(&ext.as_str()) || crate::ingest::is_code_path(path)
}

/// Coarse, human kind label for the row chip. Display only.
fn kind_label(path: &str, is_dir: bool) -> String {
    if is_dir {
        return "Folder".to_string();
    }
    let named = match ext_of(path).as_str() {
        "pdf" => Some("PDF"),
        "png" | "jpg" | "jpeg" | "jpe" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "heic"
        | "heif" | "avif" | "ico" | "jp2" => Some("Image"),
        "md" | "markdown" | "txt" | "text" | "rtf" => Some("Text"),
        "doc" | "docx" | "odt" | "pages" | "gdoc" => Some("Document"),
        "ppt" | "pptx" | "key" | "odp" | "gslides" => Some("Slides"),
        "xls" | "xlsx" | "xlsm" | "ods" | "csv" | "tsv" | "numbers" | "gsheet" => {
            Some("Spreadsheet")
        }
        "epub" => Some("Book"),
        "html" | "htm" | "xhtml" => Some("Web page"),
        _ => None,
    };
    if let Some(k) = named {
        return k.to_string();
    }
    if crate::ingest::is_code_path(path) {
        return "Code".to_string();
    }
    let ext = ext_of(path);
    if ext.is_empty() {
        "File".to_string()
    } else {
        ext.to_uppercase()
    }
}

/// Lowercased extension (no dot); "" when there is none.
fn ext_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// File (or folder) name; the whole path if it somehow has no final component.
fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/tester";

    fn p(rel: &str) -> String {
        format!("{HOME}/{rel}")
    }

    #[test]
    fn junk_filter_drops_hidden_trash_and_vendored() {
        // Real documents survive.
        assert!(!is_junk(&p("Documents/budget.pdf"), HOME));
        assert!(!is_junk(&p("Desktop/notes.md"), HOME));
        // Hidden files/dirs and their contents are dropped.
        assert!(is_junk(&p(".zshrc"), HOME));
        assert!(is_junk(&p(".Trash/old.pdf"), HOME));
        assert!(is_junk(&p("code/repo/.git/config"), HOME));
        assert!(is_junk(&p("Desktop/.hidden/report.pdf"), HOME));
        // Vendored dependency trees.
        assert!(is_junk(&p("code/app/node_modules/left-pad/index.js"), HOME));
    }

    #[test]
    fn junk_filter_excludes_library_but_keeps_cloud_drives() {
        // The bulk of ~/Library is app noise.
        assert!(is_junk(&p("Library/Caches/foo.pdf"), HOME));
        assert!(is_junk(
            &p("Library/Application Support/app/data.txt"),
            HOME
        ));
        assert!(is_junk(&p("Library"), HOME));
        // …but the cloud-drive roots under it hold real user files.
        assert!(!is_junk(&p("Library/CloudStorage/Dropbox/plan.docx"), HOME));
        assert!(!is_junk(
            &p("Library/Mobile Documents/com~apple~CloudDocs/thesis.pdf"),
            HOME
        ));
        // A different app called "Library" outside home is not the OS folder.
        assert!(!is_junk("/opt/Library/thing.pdf", HOME));
    }

    #[test]
    fn ingestible_tracks_rich_code_and_folders() {
        // Rich extractors.
        assert!(is_ingestible(&p("a/report.pdf"), false));
        assert!(is_ingestible(&p("a/photo.HEIC"), false));
        assert!(is_ingestible(&p("a/notes.md"), false));
        // Code + code-by-filename.
        assert!(is_ingestible(&p("a/lib.rs"), false));
        assert!(is_ingestible(&p("a/Dockerfile"), false));
        // Folders always ingest (they become synced folder sources).
        assert!(is_ingestible(&p("a/project"), true));
        // Unknown / unsupported types do not.
        assert!(!is_ingestible(&p("a/movie.mov"), false));
        assert!(!is_ingestible(&p("a/archive.zip"), false));
        assert!(!is_ingestible(&p("a/mystery"), false));
    }

    #[test]
    fn kind_labels_are_human() {
        assert_eq!(kind_label(&p("a/report.pdf"), false), "PDF");
        assert_eq!(kind_label(&p("a/project"), true), "Folder");
        assert_eq!(kind_label(&p("a/pic.jpeg"), false), "Image");
        assert_eq!(kind_label(&p("a/main.rs"), false), "Code");
        assert_eq!(kind_label(&p("a/Makefile"), false), "Code");
        assert_eq!(kind_label(&p("a/sheet.csv"), false), "Spreadsheet");
        assert_eq!(kind_label(&p("a/movie.mov"), false), "MOV");
        assert_eq!(kind_label(&p("a/mystery"), false), "File");
    }

    #[test]
    fn merge_prioritizes_name_hits_and_dedupes() {
        let name = vec![p("Documents/budget.pdf"), p(".secret/x.pdf")];
        let content = vec![
            p("Documents/budget.pdf"), // dup of a name hit → stays name_hit
            p("Reports/q3.pdf"),
            p("node_modules/pkg/readme.md"), // junk
        ];
        let pool = merge_and_filter(name, content, HOME, 100);
        assert_eq!(
            pool,
            vec![
                (p("Documents/budget.pdf"), true),
                (p("Reports/q3.pdf"), false),
            ]
        );
    }

    #[test]
    fn merge_cap_keeps_name_hits_over_content() {
        let name = vec![p("a/one.pdf"), p("a/two.pdf")];
        let content = vec![p("a/three.pdf"), p("a/four.pdf")];
        let pool = merge_and_filter(name, content, HOME, 3);
        // Cap of 3: both name hits, then one content hit.
        assert_eq!(pool.len(), 3);
        assert_eq!(pool[0], (p("a/one.pdf"), true));
        assert_eq!(pool[1], (p("a/two.pdf"), true));
        assert_eq!(pool[2].1, false);
    }

    fn hit(name: &str, ingestible: bool, name_hit: bool, mtime: i64) -> FileHit {
        FileHit {
            name: name.to_string(),
            path: format!("/x/{name}"),
            ext: String::new(),
            kind: String::new(),
            is_dir: false,
            size: 0,
            mtime,
            ingestible,
            name_hit,
        }
    }

    #[test]
    fn ranking_tiers_then_recency() {
        // Deliberately shuffled input across all four tiers + recency.
        let hits = vec![
            hit("content_junk", false, false, 900),
            hit("name_junk", false, true, 100),
            hit("content_good_old", true, false, 10),
            hit("name_good", true, true, 50),
            hit("content_good_new", true, false, 999),
        ];
        let ranked = rank_and_cap(hits, 10);
        let order: Vec<&str> = ranked.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "name_good",        // tier 0: ingestible + name
                "content_good_new", // tier 1: ingestible + content, newest first
                "content_good_old",
                "name_junk",    // tier 2: un-ingestible name hit
                "content_junk", // tier 3: un-ingestible content hit
            ]
        );
    }

    #[test]
    fn ranking_truncates_to_limit() {
        let hits = vec![
            hit("a", true, true, 3),
            hit("b", true, true, 2),
            hit("c", true, true, 1),
        ];
        let ranked = rank_and_cap(hits, 2);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].name, "a");
        assert_eq!(ranked[1].name, "b");
    }

    #[tokio::test]
    async fn empty_query_never_spawns() {
        assert!(search("", 30).await.is_empty());
        assert!(search("   \t ", 30).await.is_empty());
    }
}
