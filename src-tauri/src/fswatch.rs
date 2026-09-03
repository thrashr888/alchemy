//! Detection cost (docs/RFC-events.md §4): the kernel tells us when an open
//! notebook's folder changes, and nobody walks a folder nobody is looking at
//! more than once in ten minutes.
//!
//! Two halves, both feeding `commands::resync_sources_filtered`:
//!
//! - **FSEvents for open notebooks.** Windows report the notebook they have
//!   open (`set_open_notebook`); this module watches the roots of the *local*
//!   folder sources (`folder`, `obsidian`) of those notebooks and nothing
//!   else — git and Notion parents scan app-managed caches the app itself
//!   writes, so watching them would only echo our own work. A burst of raw
//!   events debounces two seconds per notebook into one scoped resync, which
//!   already coalesces its `added`/`removed` events and emits one
//!   `sources://changed`.
//! - **The closed sweep.** The scheduler's minute tick passes
//!   [`Sweep::Tick`]; local folder parents of notebooks that are not open
//!   are skipped unless the ten-minute closed window has elapsed. Open
//!   notebooks keep the minute walk as a belt to FSEvents' braces. Git and
//!   Notion probes and Mac hash checks keep their own cadences regardless —
//!   they are not directory walks.
//!
//! The watcher is best-effort by design: if it fails to start, the sweeps
//! still run and the failure is recorded once. Detection must never take
//! the app down.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;

use crate::commands::{self, AppState};

/// Quiet period after the last raw event before a notebook's resync fires.
/// A sync tool writing 400 files must land as one rescan, not 400.
pub const DEBOUNCE: Duration = Duration::from_secs(2);

/// Ceiling on how long a continuously-busy folder can hold its window open
/// before a resync fires anyway, so a folder that never goes quiet still
/// makes progress.
pub const MAX_HOLD: Duration = Duration::from_secs(30);

/// How often the scheduler walks the folders of notebooks nobody has open.
/// A constant, not a setting: the policy is the number.
pub const CLOSED_SWEEP_MS: i64 = 10 * 60 * 1000;

/// Which folder parents a resync pass walks. Everything but the minute tick
/// uses [`Sweep::All`]: the manual Resync command, the notebook-open catch-up,
/// and the watcher's own scoped fires all rescan what they always did.
#[derive(Debug, Clone, Copy)]
pub enum Sweep<'a> {
    /// Walk every folder parent the notebook scope allows.
    All,
    /// The scheduler tick: local folder parents of notebooks outside `open`
    /// walk only when `closed_due` (once per [`CLOSED_SWEEP_MS`]).
    Tick {
        open: &'a HashSet<String>,
        closed_due: bool,
    },
}

/// Whether a folder parent belongs in this pass. Pure, so the policy sits
/// beside its tests rather than inside the walk.
pub fn in_sweep(source_type: &str, notebook_id: &str, sweep: Sweep<'_>) -> bool {
    match sweep {
        Sweep::All => true,
        Sweep::Tick { open, closed_due } => {
            !is_local_folder(source_type) || closed_due || open.contains(notebook_id)
        }
    }
}

/// Folder parents whose root is a user directory on disk — the only ones
/// FSEvents can usefully watch and the only ones the closed sweep throttles.
pub fn is_local_folder(source_type: &str) -> bool {
    matches!(source_type, "folder" | "obsidian" | "okf")
}

// ---- Debounce -----------------------------------------------------------

struct Window {
    first: Instant,
    last: Instant,
}

impl Window {
    fn deadline(&self) -> Instant {
        (self.last + DEBOUNCE).min(self.first + MAX_HOLD)
    }
}

/// Per-notebook trailing debounce with a hold ceiling: a window opens at the
/// first raw event, extends with each further event, and fires once the
/// folder has been quiet for [`DEBOUNCE`] or busy for [`MAX_HOLD`]. Pure —
/// it is handed the clock — so the policy is testable without a filesystem.
#[derive(Default)]
pub struct Debouncer {
    pending: HashMap<String, Window>,
}

impl Debouncer {
    /// Record a raw event for `notebook` at `now`.
    pub fn touch(&mut self, notebook: &str, now: Instant) {
        match self.pending.get_mut(notebook) {
            Some(w) => w.last = now,
            None => {
                self.pending.insert(
                    notebook.to_string(),
                    Window {
                        first: now,
                        last: now,
                    },
                );
            }
        }
    }

    /// The earliest instant at which some notebook becomes due; `None` when
    /// nothing is pending, so the loop can sleep on the receiver alone.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(Window::deadline).min()
    }

    /// Drain and return every notebook whose window has closed by `now`,
    /// sorted for deterministic firing order.
    pub fn due(&mut self, now: Instant) -> Vec<String> {
        let mut out: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, w)| w.deadline() <= now)
            .map(|(nb, _)| nb.clone())
            .collect();
        for nb in &out {
            self.pending.remove(nb);
        }
        out.sort();
        out
    }
}

// ---- Path filtering -----------------------------------------------------

/// Whether a notify event could reflect a change to folder contents. Reads
/// (`Access`) and inode-metadata touches (`Modify(Metadata)`) never do; a
/// spurious rescan costs an mtime walk, a missed one costs a stale source,
/// so everything else is kept — cider's watcher draws the same line.
pub fn is_content_change(kind: &EventKind) -> bool {
    !matches!(
        kind,
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_))
    )
}

/// Whether the folder scanner would even see `path` under `root`. Mirrors
/// `commands::scan_folder`: dot entries are invisible except iCloud eviction
/// stubs (`.name.icloud`, whose appearance and disappearance *is* the
/// change), and vendored directories are pruned. Without this an Obsidian
/// vault rescans every two seconds while `.obsidian/workspace.json` churns.
pub fn is_scannable(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let mut components = rel.components().peekable();
    while let Some(c) = components.next() {
        let name = c.as_os_str().to_string_lossy();
        let is_last = components.peek().is_none();
        if let Some(rest) = name.strip_prefix('.') {
            let icloud_stub = is_last && rest.ends_with(".icloud") && rest.len() > ".icloud".len();
            if !icloud_stub {
                return false;
            }
        } else if !is_last && commands::SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
            return false;
        }
    }
    true
}

/// Which watched root `path` falls under, if any. Roots never nest in
/// practice (a folder inside a folder source is a child, not a parent), so
/// first match wins.
fn notebook_for<'a>(roots: &'a HashMap<PathBuf, String>, path: &Path) -> Option<&'a str> {
    roots
        .iter()
        .find(|(root, _)| path.starts_with(root) && is_scannable(root, path))
        .map(|(_, nb)| nb.as_str())
}

// ---- The live watcher ---------------------------------------------------

struct Live {
    watcher: notify::RecommendedWatcher,
    /// Root → notebook id, shared with the FSEvents callback so a rearm
    /// changes routing without rebuilding the watcher.
    roots: Arc<Mutex<HashMap<PathBuf, String>>>,
    /// Roots whose `watch` failed, so the failure is recorded once.
    failed: HashSet<PathBuf>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);
/// Set when the watcher failed to construct: recorded once, then the sweeps
/// carry detection alone rather than retrying (and re-logging) every tick.
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Raw notebook ids from the callback thread into the debounce loop.
static TX: OnceLock<mpsc::UnboundedSender<String>> = OnceLock::new();

/// Spawn the debounce loop. Called once from setup; the watcher itself is
/// built lazily on the first rearm that has something to watch.
pub fn start(app: AppHandle) {
    let (tx, rx) = mpsc::unbounded_channel();
    if TX.set(tx).is_err() {
        return;
    }
    tauri::async_runtime::spawn(run(app, rx));
}

async fn run(app: AppHandle, mut rx: mpsc::UnboundedReceiver<String>) {
    let mut deb = Debouncer::default();
    loop {
        // Idle costs nothing: with no window open the loop parks on the
        // receiver alone. The far deadline stands in for "never".
        let deadline = deb
            .next_deadline()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(24 * 3600));
        tokio::select! {
            ev = rx.recv() => match ev {
                Some(nb) => deb.touch(&nb, Instant::now()),
                None => return,
            },
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {}
        }
        for nb in deb.due(Instant::now()) {
            let state = app.state::<AppState>();
            // The Notebooks root, not a notebook: something arrived in the
            // folder, so look for bundles nobody here has opened (§5.7).
            if nb.is_empty() {
                crate::okf::open_found_bundles(&app, &state).await;
                continue;
            }
            // A bound notebook's bundle root is in the watched set too, so a
            // change under it means "read the folder back" as well as "rescan
            // the folder sources" (docs/RFC-okf-live.md §5.3). Unbound
            // notebooks return immediately.
            match crate::okf::reconcile(&state, &nb).await {
                Ok(r) if r.changed() => crate::note!(
                    "okf: {nb}: +{} ~{} -{} ({} overruled)",
                    r.created,
                    r.updated,
                    r.deleted,
                    r.overruled
                ),
                Ok(_) => {}
                Err(err) => crate::diagnostics::error("okf", format!("{nb}: reconcile: {err}")),
            }
            match commands::resync_sources_filtered(&app, &state, Some(&nb), Sweep::All).await {
                Ok(Some(scan)) => {
                    if scan.changed() {
                        crate::note!(
                            "fswatch: {nb}: +{} ~{} -{} ({} failed)",
                            scan.added,
                            scan.updated,
                            scan.removed,
                            scan.failed
                        );
                    }
                }
                // A manual import holds the scan lock: the change is real
                // and must not be dropped, so re-open the window and try
                // again after the next quiet period.
                Ok(None) => deb.touch(&nb, Instant::now()),
                Err(err) => crate::diagnostics::error("fswatch", format!("{nb}: resync: {err}")),
            }
        }
    }
}

/// Re-derive the watched set from the open notebooks and their local folder
/// sources. Cheap (one folder-table read, no content), so it runs on every
/// open-set change and every scheduler tick; a watcher that already matches
/// is left alone.
pub async fn rearm(app: &AppHandle) {
    let state = app.state::<AppState>();
    prune_closed_windows(app, &state);
    let open = state.open_notebook_ids();
    let mut desired: HashMap<PathBuf, String> = HashMap::new();
    // The Notebooks folder itself, whether or not a notebook is open: a
    // bundle that lands there — from the other Mac, from a share, from
    // Finder — is opened without waiting for the ten-minute sweep
    // (docs/RFC-okf-live.md §5.7). Its notebook id is the empty string,
    // which no notebook has; the debounce loop reads that as "look at the
    // root, not at one notebook".
    {
        let dir = {
            let ai = state.ai.read().await;
            PathBuf::from(ai.config().notebooks_dir.clone())
        };
        if dir.is_dir() {
            desired.insert(dir, String::new());
        }
    }
    if !open.is_empty() {
        match state.db.all_folder_sources().await {
            Ok(folders) => {
                for f in folders {
                    if !is_local_folder(&f.source_type) || !open.contains(&f.notebook_id) {
                        continue;
                    }
                    let root = PathBuf::from(&f.url);
                    if root.is_dir() {
                        desired.insert(root, f.notebook_id);
                    }
                }
            }
            // The folder table is not the bound roots' business: a read
            // that failed must not cost them their watch.
            Err(err) => crate::note!("fswatch: folder list failed: {err:#}"),
        }
    }
    // A bound notebook's bundle is watched for the same reason its folder
    // sources are: it is the notebook's shared surface, and an edit made in a
    // text editor should land here in seconds, not on the next sweep
    // (docs/RFC-okf-live.md §5.3). Rearm runs on every open-set change and on
    // every bind, so the watch is live from the moment there is something to
    // watch rather than up to a minute later.
    if let Ok(data_dir) = app.path().app_data_dir() {
        for (notebook_id, binding) in crate::okf::load_bindings(&data_dir) {
            if !open.contains(&notebook_id) {
                continue;
            }
            let root = PathBuf::from(&binding.path);
            if root.is_dir() {
                desired.insert(root, notebook_id);
            }
        }
    }
    apply(desired);
}

/// Drop registry entries for windows that no longer exist. `Destroyed`
/// evicts on the hot path; this catches anything that slipped past it.
fn prune_closed_windows(app: &AppHandle, state: &AppState) {
    let live: HashSet<String> = app.webview_windows().keys().cloned().collect();
    let mut open = state
        .open_notebooks
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    open.retain(|label, _| live.contains(label));
}

/// Make the live watcher match `desired`, building it on first need.
fn apply(desired: HashMap<PathBuf, String>) {
    let mut guard = LIVE.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        if desired.is_empty() || FAILED.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        match build() {
            Ok(live) => *guard = Some(live),
            Err(err) => {
                FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                crate::diagnostics::error(
                    "fswatch",
                    format!("watcher failed to start; folder sweeps carry detection: {err:#}"),
                );
                return;
            }
        }
    }
    let live = guard.as_mut().expect("watcher built above");
    let mut roots = live.roots.lock().unwrap_or_else(|p| p.into_inner());
    let current: HashSet<PathBuf> = roots.keys().cloned().collect();
    let wanted: HashSet<PathBuf> = desired.keys().cloned().collect();
    for root in current.difference(&wanted) {
        if let Err(err) = live.watcher.unwatch(root) {
            crate::note!("fswatch: unwatch {}: {err}", root.display());
        }
        roots.remove(root);
        crate::note!("fswatch: released {}", root.display());
    }
    let mut watched: HashSet<PathBuf> = current.intersection(&wanted).cloned().collect();
    for root in wanted.difference(&current) {
        match live.watcher.watch(root, RecursiveMode::Recursive) {
            Ok(()) => {
                crate::note!("fswatch: watching {}", root.display());
                watched.insert(root.clone());
                live.failed.remove(root);
            }
            // Recorded once per root: a root that stays unwatchable would
            // otherwise re-log every tick, and the sweep covers it anyway.
            Err(err) if live.failed.insert(root.clone()) => crate::diagnostics::error(
                "fswatch",
                format!("watch {}: {err}; the sweep covers it", root.display()),
            ),
            Err(_) => {}
        }
    }
    // Routing follows the desired map even for roots already watched: a
    // folder source moved between notebooks re-routes without a rewatch.
    for (root, nb) in desired {
        if watched.contains(&root) {
            roots.insert(root, nb);
        }
    }
}

fn build() -> anyhow::Result<Live> {
    let roots: Arc<Mutex<HashMap<PathBuf, String>>> = Arc::default();
    let callback_roots = roots.clone();
    let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Some(tx) = TX.get() else { return };
        match result {
            Ok(event) if is_content_change(&event.kind) => {
                let roots = callback_roots.lock().unwrap_or_else(|p| p.into_inner());
                // One send per notebook per raw event, not per path: the
                // debouncer folds them anyway, but the channel need not
                // carry a 400-file burst as 400 messages.
                let mut hit: Vec<&str> = Vec::new();
                for path in &event.paths {
                    if let Some(nb) = notebook_for(&roots, path) {
                        if !hit.contains(&nb) {
                            hit.push(nb);
                        }
                    }
                }
                for nb in hit {
                    let _ = tx.send(nb.to_string());
                }
            }
            Ok(_) => {}
            Err(err) => crate::note!("fswatch: {err}"),
        }
    })?;
    Ok(Live {
        watcher,
        roots,
        failed: HashSet::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tick_sweep_skips_closed_local_folders_until_due() {
        let open = open(&["a"]);
        let tick = Sweep::Tick {
            open: &open,
            closed_due: false,
        };
        assert!(
            in_sweep("folder", "a", tick),
            "open notebook walks every tick"
        );
        assert!(in_sweep("obsidian", "a", tick));
        assert!(!in_sweep("folder", "b", tick), "closed notebook waits");
        assert!(!in_sweep("obsidian", "b", tick));
        assert!(in_sweep("git", "b", tick), "git keeps its own cadence");
        assert!(
            in_sweep("notion", "b", tick),
            "notion keeps its own cadence"
        );

        let due = Sweep::Tick {
            open: &open,
            closed_due: true,
        };
        assert!(in_sweep("folder", "b", due), "the ten-minute window opens");
        assert!(in_sweep("folder", "a", Sweep::All));
        assert!(
            in_sweep("folder", "b", Sweep::All),
            "manual resync walks all"
        );
    }

    #[test]
    fn debounce_folds_a_burst_into_one_fire_after_quiet() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        for i in 0..400 {
            d.touch("nb", t0 + Duration::from_millis(i * 3));
        }
        let last = t0 + Duration::from_millis(399 * 3);
        assert_eq!(d.next_deadline(), Some(last + DEBOUNCE));
        assert!(
            d.due(last + Duration::from_secs(1)).is_empty(),
            "still inside the window"
        );
        assert_eq!(d.due(last + DEBOUNCE), vec!["nb".to_string()]);
        assert_eq!(d.next_deadline(), None, "nothing left pending");
    }

    #[test]
    fn debounce_is_per_notebook_and_orders_fires() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.touch("b", t0);
        d.touch("a", t0 + Duration::from_millis(500));
        assert_eq!(
            d.due(t0 + DEBOUNCE),
            vec!["b".to_string()],
            "a is still quiet-waiting"
        );
        assert_eq!(
            d.due(t0 + Duration::from_millis(500) + DEBOUNCE),
            vec!["a".to_string()]
        );
        d.touch("b", t0);
        d.touch("a", t0);
        assert_eq!(
            d.due(t0 + DEBOUNCE),
            vec!["a".to_string(), "b".to_string()],
            "simultaneous fires are sorted"
        );
    }

    #[test]
    fn debounce_hold_ceiling_fires_a_folder_that_never_goes_quiet() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        let mut t = t0;
        while t < t0 + MAX_HOLD + Duration::from_secs(5) {
            d.touch("nb", t);
            let fired = d.due(t);
            if !fired.is_empty() {
                assert!(t >= t0 + MAX_HOLD, "fired before the hold ceiling");
                assert!(t < t0 + MAX_HOLD + Duration::from_secs(2));
                return;
            }
            t += Duration::from_secs(1);
        }
        panic!("a continuously busy folder never fired");
    }

    #[test]
    fn retouch_after_busy_reopens_the_window() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.touch("nb", t0);
        assert_eq!(d.due(t0 + DEBOUNCE), vec!["nb".to_string()]);
        // Resync reported busy: the loop touches again to retry.
        d.touch("nb", t0 + DEBOUNCE);
        assert!(d.due(t0 + DEBOUNCE + Duration::from_secs(1)).is_empty());
        assert_eq!(d.due(t0 + DEBOUNCE * 2), vec!["nb".to_string()]);
    }

    #[test]
    fn content_change_ignores_reads_and_metadata() {
        use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, RemoveKind};
        assert!(!is_content_change(&EventKind::Access(AccessKind::Read)));
        assert!(!is_content_change(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::Any)
        )));
        assert!(is_content_change(&EventKind::Create(CreateKind::File)));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Data(
            DataChange::Content
        ))));
        assert!(is_content_change(&EventKind::Remove(RemoveKind::File)));
        assert!(is_content_change(&EventKind::Any));
    }

    #[test]
    fn scannable_mirrors_the_folder_walker() {
        let root = Path::new("/v/vault");
        assert!(is_scannable(root, Path::new("/v/vault/notes/a.md")));
        assert!(
            !is_scannable(root, Path::new("/v/vault/.obsidian/workspace.json")),
            "dot dirs are invisible to the scanner"
        );
        assert!(!is_scannable(root, Path::new("/v/vault/.DS_Store")));
        assert!(
            is_scannable(root, Path::new("/v/vault/docs/.report.pdf.icloud")),
            "iCloud stubs are the change"
        );
        assert!(!is_scannable(root, Path::new("/v/vault/.icloud")));
        assert!(!is_scannable(
            root,
            Path::new("/v/vault/app/node_modules/x/index.js")
        ));
        assert!(!is_scannable(root, Path::new("/elsewhere/a.md")));
    }

    #[test]
    fn notebook_routing_by_root() {
        let mut roots = HashMap::new();
        roots.insert(PathBuf::from("/a"), "nb-a".to_string());
        roots.insert(PathBuf::from("/b/deep"), "nb-b".to_string());
        assert_eq!(notebook_for(&roots, Path::new("/a/x.md")), Some("nb-a"));
        assert_eq!(
            notebook_for(&roots, Path::new("/b/deep/y/z.pdf")),
            Some("nb-b")
        );
        assert_eq!(notebook_for(&roots, Path::new("/b/other.md")), None);
        assert_eq!(notebook_for(&roots, Path::new("/a/.git/index")), None);
    }
}
