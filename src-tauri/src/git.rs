//! Git awareness for folder sources (phase 1 of docs/RFC-git-sources.md):
//! detect that a folder lives inside a repository, read its provenance with
//! the user's own git, and render the parent source's folder/repo map.
//!
//! Read-only by design: nothing here ever mutates a user's repo — no fetch,
//! no ref updates, not even `git status` (which can take the index lock).
//! Subprocess shape follows mac.rs: short outer timeout, quiet env.
//! `GIT_TERMINAL_PROMPT=0` so a credential prompt can never hang the sweep.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Provenance for a working tree, read via the user's git.
#[derive(Debug, Clone)]
pub struct RepoInfo {
    /// Repo root (`--show-toplevel`).
    pub root: String,
    /// Current branch, or "HEAD" when detached.
    pub branch: String,
    /// Short sha of HEAD (empty in a repo with no commits yet).
    pub sha: String,
    /// `origin` remote URL, empty for remote-less repos.
    pub remote: String,
    /// Commit date of HEAD (YYYY-MM-DD), empty with no commits.
    pub commit_date: String,
}

/// Run git with a short timeout and no terminal prompts. None = git missing,
/// timed out, or exited non-zero (e.g. not a repo) — callers treat all three
/// the same: no git story here.
async fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Detect whether `path` sits inside a git working tree and collect its
/// provenance. Costs one subprocess when it isn't a repo, five when it is.
pub async fn detect_repo(path: &Path) -> Option<RepoInfo> {
    let root = git_out(path, &["rev-parse", "--show-toplevel"]).await?;
    let root_path = std::path::PathBuf::from(&root);
    let sha = git_out(&root_path, &["rev-parse", "--short", "HEAD"])
        .await
        .unwrap_or_default();
    let branch = git_out(&root_path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .unwrap_or_else(|| "HEAD".to_string());
    let remote = git_out(&root_path, &["remote", "get-url", "origin"])
        .await
        .unwrap_or_default();
    let commit_date = git_out(&root_path, &["log", "-1", "--format=%cs"])
        .await
        .unwrap_or_default();
    Some(RepoInfo {
        root,
        branch,
        sha,
        remote,
        commit_date,
    })
}

/// The minute sweep shouldn't spawn git per folder per tick just to notice a
/// sha that almost never moves without file changes (which force the map
/// refresh anyway). Same in-memory throttle shape as mac::sweep_due.
const PROBE_INTERVAL: Duration = Duration::from_secs(15 * 60);

static PROBES: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// True when this folder source is due a provenance re-probe. Records the
/// probe time as a side effect.
pub fn probe_due(source_id: &str) -> bool {
    let mut guard = PROBES.lock().unwrap_or_else(|p| p.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    match map.get(source_id) {
        Some(last) if now.duration_since(*last) < PROBE_INTERVAL => false,
        _ => {
            map.insert(source_id.to_string(), now);
            true
        }
    }
}

/// One scanned file for the map: folder-relative path, whether its bytes
/// were ingested (false = cloud placeholder awaiting download), and the
/// symbol-outline suffix for code files (outline.rs; empty otherwise).
pub struct MapFile {
    pub rel: String,
    pub ingested: bool,
    pub outline: String,
}

/// How many tree lines / skip rows the map will carry before eliding. The map
/// is embedded content — orientation, not an inventory.
const MAP_TREE_MAX: usize = 500;
const MAP_SKIP_MAX: usize = 200;

/// Render the parent source's content: provenance header (when a repo), file
/// tree, and the skip list with reasons — nothing scanned is silently absent.
pub fn render_map(
    title: &str,
    repo: Option<&RepoInfo>,
    folder_root: &Path,
    files: &[MapFile],
    skipped: &[(String, String)],
    grep_only: usize,
) -> String {
    let mut out = String::new();
    let kind = if repo.is_some() {
        "repository map"
    } else if folder_root.join(".obsidian").is_dir() {
        "Obsidian vault"
    } else {
        "folder map"
    };
    out.push_str(&format!("# {title} — {kind}\n\n"));

    if let Some(r) = repo {
        let origin = if r.remote.is_empty() {
            "local repository".to_string()
        } else {
            r.remote.clone()
        };
        let mut line = format!("> {origin} · {} @ {}", r.branch, sha_or(r));
        if !r.commit_date.is_empty() {
            line.push_str(&format!(" · {}", r.commit_date));
        }
        out.push_str(&line);
        out.push('\n');
        // The folder may be a subtree of the repo — record the scope so the
        // map says what slice of the repo this source actually covers.
        if let Ok(scope) = folder_root.strip_prefix(&r.root) {
            let scope = scope.to_string_lossy();
            if !scope.is_empty() {
                out.push_str(&format!("> Scope: {scope}/\n"));
            }
        }
        out.push('\n');
    }

    let pending = files.iter().filter(|f| !f.ingested).count();
    let mut counts = format!("{} files", files.len());
    if pending > 0 {
        counts.push_str(&format!(" ({pending} awaiting download)"));
    }
    if grep_only > 0 {
        counts.push_str(&format!(" · {grep_only} code files search-only"));
    }
    if !skipped.is_empty() {
        counts.push_str(&format!(" · {} skipped", skipped.len()));
    }
    out.push_str(&format!("## Files ({counts})\n\n```\n"));
    out.push_str(&render_tree(files));
    out.push_str("```\n");

    if !skipped.is_empty() {
        out.push_str("\n## Skipped\n\n");
        for (rel, reason) in skipped.iter().take(MAP_SKIP_MAX) {
            out.push_str(&format!("- {rel} — {reason}\n"));
        }
        if skipped.len() > MAP_SKIP_MAX {
            out.push_str(&format!("- … and {} more\n", skipped.len() - MAP_SKIP_MAX));
        }
    }
    out
}

fn sha_or(r: &RepoInfo) -> &str {
    if r.sha.is_empty() {
        "no commits"
    } else {
        &r.sha
    }
}

/// Indented tree from sorted relative paths. Directories print once as
/// `name/`; files indent beneath them, code files carrying their symbol
/// outline suffix.
fn render_tree(files: &[MapFile]) -> String {
    let mut sorted: Vec<&MapFile> = files.iter().collect();
    sorted.sort_unstable_by(|a, b| a.rel.cmp(&b.rel));
    let mut out = String::new();
    let mut printed_dirs: Vec<String> = Vec::new();
    let mut lines = 0usize;
    for f in &sorted {
        let rel = f.rel.as_str();
        if lines >= MAP_TREE_MAX {
            out.push_str(&format!("… and {} more files\n", sorted.len() - lines));
            break;
        }
        let parts: Vec<&str> = rel.split('/').collect();
        let (dirs, file) = parts.split_at(parts.len() - 1);
        // Print any directory components not already printed at their depth.
        for (depth, dir) in dirs.iter().enumerate() {
            let key = dirs[..=depth].join("/");
            if !printed_dirs.contains(&key) {
                out.push_str(&"  ".repeat(depth));
                out.push_str(dir);
                out.push_str("/\n");
                printed_dirs.push(key);
                lines += 1;
            }
        }
        out.push_str(&"  ".repeat(dirs.len()));
        out.push_str(file[0]);
        out.push_str(&f.outline);
        out.push('\n');
        lines += 1;
    }
    out
}

// ---- Phase 2: remote repositories (docs/RFC-git-sources.md §1–§2) ---------

/// What a pasted URL asks for. The URL's specificity is the intent signal:
/// a repo home means "the page I'm looking at" (README), deeper shapes mean
/// deliberate reaches for content.
#[derive(Debug, Clone, PartialEq)]
pub enum GitTarget {
    /// `https://host/owner/repo` — README only by default.
    RepoHome { remote: String },
    /// `…/tree/<ref>[/<path>]` — subtree at ref (`refpath` still fused).
    Subtree { remote: String, refpath: String },
    /// `…/blob/<ref>/<path>` — one file at ref.
    Blob { remote: String, refpath: String },
    /// Clone URL (scp-like / ssh:// / https…git) — whole repo.
    CloneAll { remote: String },
}

impl GitTarget {
    pub fn remote(&self) -> &str {
        match self {
            GitTarget::RepoHome { remote }
            | GitTarget::Subtree { remote, .. }
            | GitTarget::Blob { remote, .. }
            | GitTarget::CloneAll { remote } => remote,
        }
    }

    /// "owner/repo" for titles and retrieval context.
    pub fn repo_label(&self) -> String {
        repo_label_of(self.remote())
    }
}

fn repo_label_of(remote: &str) -> String {
    let tail = remote
        .rsplit_once(':')
        .map(|(_, t)| t)
        .unwrap_or(remote)
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let mut segs: Vec<&str> = tail.split('/').filter(|s| !s.is_empty()).collect();
    let repo = segs.pop().unwrap_or("repo");
    let owner = segs.pop().unwrap_or("");
    if owner.is_empty() {
        repo.to_string()
    } else {
        format!("{owner}/{repo}")
    }
}

pub fn host_of(remote: &str) -> String {
    if let Some(rest) = remote.strip_prefix("git@") {
        return rest.split(':').next().unwrap_or("").to_string();
    }
    let rest = remote
        .strip_prefix("ssh://")
        .map(|r| r.trim_start_matches("git@"))
        .or_else(|| remote.strip_prefix("https://"))
        .or_else(|| remote.strip_prefix("http://"))
        .unwrap_or(remote);
    rest.split(['/', ':']).next().unwrap_or("").to_string()
}

/// First path segments on github.com (and GHE) that are product surfaces,
/// not owners — URLs under them fall through to page capture.
const RESERVED_FIRST: &[&str] = &[
    "orgs",
    "organizations",
    "settings",
    "marketplace",
    "topics",
    "search",
    "login",
    "join",
    "features",
    "about",
    "pricing",
    "explore",
    "sponsors",
    "notifications",
    "issues",
    "pulls",
    "codespaces",
    "collections",
    "events",
    "trending",
    "new",
    "apps",
    "site",
    "contact",
    "dashboard",
];

/// Parse a URL into a git target by shape alone (host gating is
/// `detect_target`'s job). None = not git-shaped — including repo
/// sub-surfaces like `/releases` and `/issues`, which are API pages.
pub fn parse_git_url(url: &str) -> Option<GitTarget> {
    let u = url.trim().trim_end_matches('/');
    if let Some(rest) = u.strip_prefix("git@") {
        if rest.contains(':') {
            return Some(GitTarget::CloneAll {
                remote: u.to_string(),
            });
        }
    }
    if u.starts_with("ssh://") {
        return Some(GitTarget::CloneAll {
            remote: u.to_string(),
        });
    }
    let web = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))?;
    if web.ends_with(".git") {
        return Some(GitTarget::CloneAll {
            remote: format!("https://{web}"),
        });
    }
    let mut it = web.split('/');
    let host = it.next()?.split(['?', '#']).next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.is_empty() || !host.contains('.') {
        return None;
    }
    let segs: Vec<&str> = it
        .take_while(|s| !s.contains(['?', '#']))
        .filter(|s| !s.is_empty())
        .collect();
    if segs.len() < 2 {
        return None;
    }
    let owner = segs[0];
    let repo = segs[1].trim_end_matches(".git");
    if repo.is_empty() || RESERVED_FIRST.contains(&owner.to_lowercase().as_str()) {
        return None;
    }
    let remote = format!("https://{host}/{owner}/{repo}.git");
    match segs.get(2).copied() {
        None => Some(GitTarget::RepoHome { remote }),
        Some("tree") if segs.len() >= 4 => Some(GitTarget::Subtree {
            remote,
            refpath: segs[3..].join("/"),
        }),
        Some("blob") if segs.len() >= 5 => Some(GitTarget::Blob {
            remote,
            refpath: segs[3..].join("/"),
        }),
        _ => None,
    }
}

// ---- Host memory: which non-github hosts speak git ------------------------

const HOST_TTL_SECS: i64 = 30 * 24 * 60 * 60;

static HOSTS: Mutex<Option<HashMap<String, (bool, i64)>>> = Mutex::new(None);

fn hosts_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("git_hosts.json")
}

fn host_verdict(data_dir: &Path, host: &str) -> Option<bool> {
    let mut guard = HOSTS.lock().unwrap_or_else(|p| p.into_inner());
    let map = guard.get_or_insert_with(|| {
        std::fs::read_to_string(hosts_path(data_dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    });
    let (verdict, at) = map.get(host)?;
    let age = chrono::Utc::now().timestamp() - at;
    (age < HOST_TTL_SECS).then_some(*verdict)
}

fn record_host(data_dir: &Path, host: &str, verdict: bool) {
    let mut guard = HOSTS.lock().unwrap_or_else(|p| p.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(host.to_string(), (verdict, chrono::Utc::now().timestamp()));
    if let Ok(json) = serde_json::to_string_pretty(&*map) {
        let _ = std::fs::write(hosts_path(data_dir), json);
    }
}

/// Auth-shaped failures prove a git endpoint exists (the wall is
/// credentials, not protocol).
fn looks_auth_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("terminal prompts disabled")
        || s.contains("authentication failed")
        || s.contains("could not read username")
        || s.contains("permission denied")
        || s.contains("403")
}

/// Shape-parse plus host gating: github.com is trusted, clone URLs are
/// unambiguous, and unknown hosts get one remembered `ls-remote` probe —
/// zero-config GHE (github.ibm.com works because the user's git does).
pub async fn detect_target(data_dir: &Path, url: &str) -> Option<GitTarget> {
    let target = parse_git_url(url)?;
    if matches!(target, GitTarget::CloneAll { .. }) {
        return Some(target);
    }
    let host = host_of(target.remote());
    if host == "github.com" {
        return Some(target);
    }
    if let Some(v) = host_verdict(data_dir, &host) {
        return v.then_some(target);
    }
    let verdict = match run_git(None, &["ls-remote", target.remote()], 8).await {
        Ok(_) => true,
        Err(stderr) => looks_auth_error(&stderr),
    };
    record_host(data_dir, &host, verdict);
    verdict.then_some(target)
}

// ---- Clone plumbing --------------------------------------------------------

/// Run git and return stdout, or Err(stderr) — callers need stderr to spot
/// auth-shaped failures. `dir: None` runs without `-C` (ls-remote by URL).
async fn run_git(dir: Option<&Path>, args: &[&str], timeout_secs: u64) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("git");
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    let out = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        cmd.args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output(),
    )
    .await
    .map_err(|_| format!("git {} timed out", args.first().unwrap_or(&"")))?
    .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn cache_dir(data_dir: &Path, source_id: &str) -> std::path::PathBuf {
    data_dir.join("git").join(source_id)
}

/// Root the folder rescan should walk for a `git` parent: the cache
/// checkout, plus the sparse scope recorded at clone time.
pub fn checkout_root(data_dir: &Path, source_id: &str) -> std::path::PathBuf {
    let dir = cache_dir(data_dir, source_id);
    match std::fs::read_to_string(dir.join(".git").join("alchemy-scope")) {
        Ok(scope) if !scope.trim().is_empty() => dir.join(scope.trim()),
        _ => dir,
    }
}

/// Remove a source's cache checkout (source deletion / failed adopt).
pub fn remove_cache(data_dir: &Path, source_id: &str) {
    let dir = cache_dir(data_dir, source_id);
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Move a staged clone into its permanent home once the source row exists.
pub fn adopt_cache(staged: &Path, data_dir: &Path, source_id: &str) -> anyhow::Result<()> {
    let dest = cache_dir(data_dir, source_id);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    std::fs::rename(staged, &dest)?;
    Ok(())
}

pub struct Staged {
    pub dir: std::path::PathBuf,
    pub sha: String,
    pub kind: StagedKind,
}

pub enum StagedKind {
    /// One file (README default or blob URL), repo-relative.
    Single { file_rel: String },
    /// A checkout to rescan (sparse scope, if any, rides the sidecar).
    Tree,
}

/// `git clone` with an ssh retry when https hits an auth wall — GHE users
/// almost always have keys where they don't have https helpers.
async fn clone_with_fallback(remote: &str, extra: &[&str], dest: &Path) -> anyhow::Result<String> {
    let dest_s = dest.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["clone"];
    args.extend_from_slice(extra);
    args.push(remote);
    args.push(&dest_s);
    match run_git(None, &args, 120).await {
        Ok(_) => Ok(remote.to_string()),
        Err(stderr) if looks_auth_error(&stderr) && remote.starts_with("https://") => {
            let host = host_of(remote);
            let tail = remote
                .trim_start_matches("https://")
                .split_once('/')
                .map(|(_, t)| t)
                .unwrap_or("");
            let ssh = format!("git@{host}:{tail}");
            let mut args: Vec<&str> = vec!["clone"];
            args.extend_from_slice(extra);
            args.push(&ssh);
            args.push(&dest_s);
            run_git(None, &args, 120).await.map_err(|e| {
                anyhow::anyhow!("git clone failed — check your git credentials for {host} ({e})")
            })?;
            Ok(ssh)
        }
        Err(stderr) => Err(anyhow::anyhow!("git clone failed: {stderr}")),
    }
}

/// Split a fused `<ref>/<path>` using the remote's real ref list (branch
/// names contain slashes), longest match first; bare hex falls through as a
/// pinned sha.
async fn resolve_refpath(remote: &str, refpath: &str) -> (String, String) {
    let segs: Vec<&str> = refpath.split('/').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return (String::new(), String::new());
    }
    if let Ok(out) = run_git(None, &["ls-remote", "--heads", "--tags", remote], 15).await {
        let refs: Vec<String> = out
            .lines()
            .filter_map(|l| l.split_whitespace().nth(1))
            .map(|r| {
                r.trim_start_matches("refs/heads/")
                    .trim_start_matches("refs/tags/")
                    .trim_end_matches("^{}")
                    .to_string()
            })
            .collect();
        for k in (1..=segs.len()).rev() {
            let candidate = segs[..k].join("/");
            if refs.iter().any(|r| r == &candidate) {
                return (candidate, segs[k..].join("/"));
            }
        }
    }
    (segs[0].to_string(), segs[1..].join("/"))
}

fn pick_readme(listing: &str) -> Option<String> {
    let names: Vec<&str> = listing.lines().map(str::trim).collect();
    names
        .iter()
        .find(|n| n.eq_ignore_ascii_case("readme.md"))
        .or_else(|| {
            names
                .iter()
                .find(|n| n.to_lowercase().starts_with("readme"))
        })
        .map(|s| s.to_string())
}

fn record_scope(dir: &Path, scope: &str) {
    let _ = std::fs::write(dir.join(".git").join("alchemy-scope"), scope);
}

/// The include-ladder rung chosen at add time (docs/RFC-git-sources.md §1):
/// absent = docs & code; "docs" = prose only, code out of scope entirely.
/// Rides inside .git/ so the scanner never sees it.
pub fn record_include(dir: &Path, include: &str) {
    let _ = std::fs::write(dir.join(".git").join("alchemy-include"), include);
}

pub fn read_include(data_dir: &Path, source_id: &str) -> Option<String> {
    let dir = cache_dir(data_dir, source_id);
    std::fs::read_to_string(dir.join(".git").join("alchemy-include"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Clone a target into a staging dir under `<data>/git/`. Blobless + sparse
/// wherever the scope allows, so a one-file source fetches kilobytes.
pub async fn clone_target(data_dir: &Path, target: &GitTarget) -> anyhow::Result<Staged> {
    let staging = data_dir
        .join("git")
        .join(format!("tmp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(data_dir.join("git"))?;
    let result = clone_target_inner(&staging, target).await;
    if result.is_err() && staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

async fn clone_target_inner(dir: &Path, target: &GitTarget) -> anyhow::Result<Staged> {
    let blobless: &[&str] = &["--filter=blob:none", "--depth", "1", "--no-checkout"];
    match target {
        GitTarget::RepoHome { remote } => {
            clone_with_fallback(remote, blobless, dir).await?;
            let listing = run_git(Some(dir), &["ls-tree", "--name-only", "HEAD"], 15)
                .await
                .map_err(|e| anyhow::anyhow!("git ls-tree failed: {e}"))?;
            let readme = pick_readme(&listing).ok_or_else(|| {
                anyhow::anyhow!(
                    "no README found in {remote} — paste a /blob/ URL to a specific file, \
                     or a /tree/ URL for the whole repo"
                )
            })?;
            sparse_checkout(dir, &format!("/{readme}"), false).await?;
            record_scope(dir, &readme);
            Ok(Staged {
                dir: dir.to_path_buf(),
                sha: short_sha(dir).await,
                kind: StagedKind::Single { file_rel: readme },
            })
        }
        GitTarget::Blob { remote, refpath } => {
            let (reff, path) = resolve_refpath(remote, refpath).await;
            if path.is_empty() {
                anyhow::bail!("blob URL has no file path after the ref");
            }
            clone_at_ref(remote, &reff, blobless, dir).await?;
            sparse_checkout(dir, &format!("/{path}"), false).await?;
            if !dir.join(&path).exists() {
                anyhow::bail!("{path} does not exist at {reff} in {remote}");
            }
            record_scope(dir, &path);
            Ok(Staged {
                dir: dir.to_path_buf(),
                sha: short_sha(dir).await,
                kind: StagedKind::Single { file_rel: path },
            })
        }
        GitTarget::Subtree { remote, refpath } => {
            let (reff, path) = resolve_refpath(remote, refpath).await;
            if path.is_empty() {
                // Whole tree at a ref: a plain shallow clone (children need
                // blobs anyway).
                clone_at_ref(remote, &reff, &["--depth", "1"], dir).await?;
            } else {
                clone_at_ref(remote, &reff, blobless, dir).await?;
                sparse_checkout(dir, &path, true).await?;
                if !dir.join(&path).is_dir() {
                    anyhow::bail!("{path}/ does not exist at {reff} in {remote}");
                }
                record_scope(dir, &path);
            }
            Ok(Staged {
                dir: dir.to_path_buf(),
                sha: short_sha(dir).await,
                kind: StagedKind::Tree,
            })
        }
        GitTarget::CloneAll { remote } => {
            clone_with_fallback(remote, &["--depth", "1"], dir).await?;
            Ok(Staged {
                dir: dir.to_path_buf(),
                sha: short_sha(dir).await,
                kind: StagedKind::Tree,
            })
        }
    }
}

async fn clone_at_ref(remote: &str, reff: &str, base: &[&str], dir: &Path) -> anyhow::Result<()> {
    let looks_sha = reff.len() >= 7 && reff.chars().all(|c| c.is_ascii_hexdigit());
    if looks_sha {
        // Pinned sha: clone the default tip, then fetch the exact commit
        // (GitHub allows reachable-sha fetches).
        clone_with_fallback(remote, base, dir).await?;
        run_git(Some(dir), &["fetch", "--depth", "1", "origin", reff], 120)
            .await
            .map_err(|e| anyhow::anyhow!("git fetch {reff} failed: {e}"))?;
        run_git(Some(dir), &["update-ref", "HEAD", reff], 10)
            .await
            .map_err(|e| anyhow::anyhow!("git update-ref failed: {e}"))?;
        return Ok(());
    }
    let mut args: Vec<&str> = base.to_vec();
    args.push("--branch");
    args.push(reff);
    clone_with_fallback(remote, &args, dir).await?;
    Ok(())
}

async fn sparse_checkout(dir: &Path, pattern: &str, cone: bool) -> anyhow::Result<()> {
    let mode = if cone { "--cone" } else { "--no-cone" };
    run_git(Some(dir), &["sparse-checkout", "set", mode, pattern], 30)
        .await
        .map_err(|e| anyhow::anyhow!("git sparse-checkout failed: {e}"))?;
    run_git(Some(dir), &["checkout"], 120)
        .await
        .map_err(|e| anyhow::anyhow!("git checkout failed: {e}"))?;
    Ok(())
}

async fn short_sha(dir: &Path) -> String {
    git_out(dir, &["rev-parse", "--short", "HEAD"])
        .await
        .unwrap_or_default()
}

/// One-line provenance for single-file git sources, matching the repo map's
/// header and the capture pipeline's `> By … · Published …` convention.
pub async fn provenance_header(dir: &Path) -> Option<String> {
    let r = detect_repo(dir).await?;
    let origin = if r.remote.is_empty() {
        "local repository".to_string()
    } else {
        r.remote.clone()
    };
    let mut line = format!("> {origin} · {} @ {}", r.branch, sha_or(&r));
    if !r.commit_date.is_empty() {
        line.push_str(&format!(" · {}", r.commit_date));
    }
    Some(line)
}

// ---- Remote resync ---------------------------------------------------------

/// Per-source throttle for network probes (the map probe above is the cheap
/// local one; this one crosses the network). The interval is the user's
/// auto-sync cadence setting; 0 never gets here (callers skip).
static REMOTE_PROBES: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

pub fn remote_probe_due(source_id: &str, interval_minutes: u32) -> bool {
    let interval = Duration::from_secs(u64::from(interval_minutes) * 60);
    let mut guard = REMOTE_PROBES.lock().unwrap_or_else(|p| p.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    match map.get(source_id) {
        Some(last) if now.duration_since(*last) < interval => false,
        _ => {
            map.insert(source_id.to_string(), now);
            true
        }
    }
}

/// One cheap round-trip: has the tracked branch moved upstream? On change,
/// refetch and hard-reset the cache checkout; returns the new short sha.
/// Detached checkouts (tag/sha pins) never move. Never touches user repos —
/// this only ever runs against Alchemy's own cache clones.
pub async fn sync_remote(dir: &Path) -> anyhow::Result<Option<String>> {
    let branch = git_out(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .unwrap_or_else(|| "HEAD".to_string());
    if branch == "HEAD" {
        return Ok(None);
    }
    let spec = format!("refs/heads/{branch}");
    let Ok(out) = run_git(Some(dir), &["ls-remote", "origin", &spec], 20).await else {
        return Ok(None); // offline — try again next hour
    };
    let remote_sha = out.split_whitespace().next().unwrap_or_default();
    let local_sha = git_out(dir, &["rev-parse", "HEAD"])
        .await
        .unwrap_or_default();
    if remote_sha.is_empty() || remote_sha == local_sha {
        return Ok(None);
    }
    run_git(
        Some(dir),
        &["fetch", "--depth", "1", "origin", &branch],
        120,
    )
    .await
    .map_err(|e| anyhow::anyhow!("git fetch failed: {e}"))?;
    run_git(Some(dir), &["reset", "--hard", "FETCH_HEAD"], 60)
        .await
        .map_err(|e| anyhow::anyhow!("git reset failed: {e}"))?;
    Ok(Some(short_sha(dir).await))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mf(rel: &str) -> MapFile {
        MapFile {
            rel: rel.to_string(),
            ingested: true,
            outline: String::new(),
        }
    }

    #[test]
    fn tree_carries_outline_suffixes() {
        let files = vec![
            MapFile {
                rel: "src/db.rs".into(),
                ingested: true,
                outline: " — Db, search_chunks_trace".into(),
            },
            mf("README.md"),
        ];
        let tree = render_tree(&files);
        assert!(
            tree.contains("  db.rs — Db, search_chunks_trace\n"),
            "{tree}"
        );
    }

    #[test]
    fn tree_prints_dirs_once_with_indent() {
        let files = vec![mf("src/lib.rs"), mf("src/db.rs"), mf("README.md")];
        let tree = render_tree(&files);
        assert_eq!(tree, "README.md\nsrc/\n  db.rs\n  lib.rs\n");
    }

    #[test]
    fn map_includes_provenance_and_skips() {
        let repo = RepoInfo {
            root: "/tmp/r".into(),
            branch: "main".into(),
            sha: "abc1234".into(),
            remote: "git@github.com:o/r.git".into(),
            commit_date: "2026-07-19".into(),
        };
        let map = render_map(
            "r",
            Some(&repo),
            Path::new("/tmp/r/src"),
            &[mf("lib.rs")],
            &[("big.bin".into(), "binary".into())],
            12,
        );
        assert!(map.contains("# r — repository map"));
        assert!(map.contains("> git@github.com:o/r.git · main @ abc1234 · 2026-07-19"));
        assert!(map.contains("> Scope: src/"));
        assert!(map.contains("12 code files search-only"));
        assert!(map.contains("- big.bin — binary"));
    }

    #[test]
    fn url_grammar_maps_shapes_to_targets() {
        let t = parse_git_url("https://github.com/thrashr888/alchemy").unwrap();
        assert_eq!(
            t,
            GitTarget::RepoHome {
                remote: "https://github.com/thrashr888/alchemy.git".into()
            }
        );
        let t = parse_git_url("https://github.com/o/r/tree/main/src/retrieval").unwrap();
        assert_eq!(
            t,
            GitTarget::Subtree {
                remote: "https://github.com/o/r.git".into(),
                refpath: "main/src/retrieval".into()
            }
        );
        let t = parse_git_url("https://github.com/o/r/blob/main/src/db.rs").unwrap();
        assert_eq!(
            t,
            GitTarget::Blob {
                remote: "https://github.com/o/r.git".into(),
                refpath: "main/src/db.rs".into()
            }
        );
        for clone in [
            "git@github.ibm.com:org/repo.git",
            "ssh://git@host.co/o/r.git",
            "https://gitlab.com/o/r.git",
        ] {
            assert!(matches!(
                parse_git_url(clone),
                Some(GitTarget::CloneAll { .. })
            ));
        }
        // Trailing slash and www are tolerated on repo homes.
        assert!(matches!(
            parse_git_url("https://www.github.com/o/r/"),
            Some(GitTarget::RepoHome { .. })
        ));
    }

    #[test]
    fn url_grammar_rejects_non_repo_shapes() {
        for url in [
            "https://github.com/thrashr888/alchemy/releases",
            "https://github.com/o/r/issues/12",
            "https://github.com/o/r/pulls",
            "https://github.com/o/r/wiki",
            "https://github.com/orgs/anthropics/repositories",
            "https://github.com/settings/profile",
            "https://github.com/thrashr888",
            "https://example.com",
            "https://example.com/one-segment",
            "not a url at all",
        ] {
            assert!(parse_git_url(url).is_none(), "should reject {url}");
        }
    }

    #[test]
    fn helpers_extract_hosts_and_labels() {
        assert_eq!(host_of("git@github.ibm.com:org/repo.git"), "github.ibm.com");
        assert_eq!(host_of("https://github.com/o/r.git"), "github.com");
        assert_eq!(host_of("ssh://git@host.co/o/r.git"), "host.co");
        assert_eq!(
            repo_label_of("https://github.com/thrashr888/alchemy.git"),
            "thrashr888/alchemy"
        );
        assert_eq!(repo_label_of("git@github.ibm.com:org/repo.git"), "org/repo");
    }

    #[test]
    fn readme_picker_prefers_markdown() {
        assert_eq!(
            pick_readme("LICENSE\nREADME.md\nREADME.rst\nsrc").as_deref(),
            Some("README.md")
        );
        assert_eq!(
            pick_readme("license\nReadme.rst\nsrc").as_deref(),
            Some("Readme.rst")
        );
        assert_eq!(pick_readme("LICENSE\nsrc"), None);
    }

    #[test]
    fn auth_errors_are_recognized() {
        assert!(looks_auth_error(
            "fatal: could not read Username for 'https://github.ibm.com': terminal prompts disabled"
        ));
        assert!(looks_auth_error("git@host: Permission denied (publickey)."));
        assert!(!looks_auth_error("fatal: repository 'x' not found"));
    }
}
