//! What the app has a model doing, right now, in one place.
//!
//! Before this, the only sign a model was working was a small glyph on the
//! chat row that was answering — so everything else the app runs on your
//! behalf (a scheduled report, a queued generation, the gist and hygiene
//! sweeps, an embedding pass) spent your machine invisibly, and even the
//! chat glyph was easy to miss.
//!
//! Every provider call passes through `begin`, which returns a guard; the
//! guard's `Drop` ends the entry, so a call that errors, times out, or is
//! cancelled cannot leave a phantom running. The whole list is emitted on
//! `inference://activity` and served over MCP, so an agent sees exactly what
//! the title bar shows.
//!
//! Two rules keep it honest:
//!
//! * **Debounced, not sampled.** Changes coalesce for `DEBOUNCE`, so a burst
//!   of hundreds of short embed calls is one repaint instead of hundreds —
//!   but the *last* state of a burst is always emitted, so the indicator
//!   never sticks on a stale value.
//! * **The label is the caller's.** `labeled` scopes a human sentence
//!   ("Morning Brief", "Summarizing curated.supply") over an async block;
//!   anything inside it that reaches a model is attributed to it. Without
//!   one, an entry still appears — an unlabeled run is still your machine
//!   working — it just says only which engine is busy.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

/// How long changes coalesce before the front end hears about them. Long
/// enough to swallow a burst of embed calls, short enough that pressing send
/// lights the indicator within a frame or two of the first token request.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// One model call in flight.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityItem {
    pub id: u64,
    /// Provider family: "ollama" | "fm" | "gateway" | "agent-cli" | "builtin".
    pub kind: String,
    /// What this call is for, in the user's words. "" when the caller set no
    /// scope — the indicator then falls back to the engine's own name.
    pub label: String,
    /// The model doing it, when the engine names one.
    pub model: String,
    pub started_at: i64,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Ordered by id, so the emitted list is always oldest-first — a caller
/// reading it sees the run that started the burst at the top.
static RUNNING: Mutex<Option<BTreeMap<u64, ActivityItem>>> = Mutex::new(None);
/// When the last emit went out, and whether a change is waiting behind the
/// debounce. Separate from `RUNNING` so a flush can read one without the
/// other.
static EMIT: Mutex<Option<EmitState>> = Mutex::new(None);

#[derive(Debug)]
struct EmitState {
    last: Instant,
    /// A change arrived inside the window and has not been emitted yet.
    pending: bool,
}

tokio::task_local! {
    static LABEL: String;
}

/// Attribute every model call inside `fut` to `label`.
///
/// Task-local, so it follows the future through `.await` without a parameter
/// on every function between the caller and the provider — and deliberately
/// does *not* cross `tokio::spawn`, because a spawned job is its own piece of
/// work and deserves its own sentence.
pub async fn labeled<F>(label: impl Into<String>, fut: F) -> F::Output
where
    F: std::future::Future,
{
    LABEL.scope(label.into(), fut).await
}

fn current_label() -> String {
    LABEL.try_with(|l| l.clone()).unwrap_or_default()
}

/// A call in flight. Dropping it ends the entry — there is no `end`, on
/// purpose: an explicit one is a thing a `?` can skip past.
#[derive(Debug)]
pub struct Guard(u64);

impl Drop for Guard {
    fn drop(&mut self) {
        let changed = {
            let mut guard = RUNNING.lock().unwrap_or_else(|p| p.into_inner());
            guard
                .as_mut()
                .map(|m| m.remove(&self.0).is_some())
                .unwrap_or(false)
        };
        if changed {
            announce();
        }
    }
}

/// Record that `kind` (optionally `model`) has started work. The label comes
/// from the enclosing `labeled` scope.
pub fn begin(kind: &str, model: &str) -> Guard {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let item = ActivityItem {
        id,
        kind: kind.to_string(),
        label: current_label(),
        model: model.to_string(),
        started_at: crate::commands::now(),
    };
    {
        let mut guard = RUNNING.lock().unwrap_or_else(|p| p.into_inner());
        guard.get_or_insert_with(BTreeMap::new).insert(id, item);
    }
    announce();
    Guard(id)
}

/// Everything in flight, oldest first.
pub fn running() -> Vec<ActivityItem> {
    let guard = RUNNING.lock().unwrap_or_else(|p| p.into_inner());
    guard
        .as_ref()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
pub fn count() -> usize {
    let guard = RUNNING.lock().unwrap_or_else(|p| p.into_inner());
    guard.as_ref().map(|m| m.len()).unwrap_or(0)
}

/// Decide what a change should do to the emit clock: send now, or mark a
/// flush owed. Split out from `announce` so the debounce can be tested
/// without a Tauri handle or a clock to sleep on.
fn debounce_step(now: Instant) -> Debounced {
    let mut guard = EMIT.lock().unwrap_or_else(|p| p.into_inner());
    match guard.as_mut() {
        Some(state) if now.duration_since(state.last) < DEBOUNCE => {
            let first_in_window = !state.pending;
            state.pending = true;
            if first_in_window {
                Debounced::ScheduleFlush
            } else {
                Debounced::Hold
            }
        }
        _ => {
            *guard = Some(EmitState {
                last: now,
                pending: false,
            });
            Debounced::EmitNow
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Debounced {
    /// Outside the window: emit immediately.
    EmitNow,
    /// Inside the window and nothing owed yet: arm one trailing flush, so the
    /// end of the burst is never the state that gets dropped.
    ScheduleFlush,
    /// Inside the window with a flush already armed.
    Hold,
}

/// Clear the pending flag and say whether a trailing emit is owed.
fn take_pending(now: Instant) -> bool {
    let mut guard = EMIT.lock().unwrap_or_else(|p| p.into_inner());
    match guard.as_mut() {
        Some(state) if state.pending => {
            state.pending = false;
            state.last = now;
            true
        }
        _ => false,
    }
}

fn emit_now() {
    let Some(app) = crate::commands::app_handle() else {
        return;
    };
    use tauri::Emitter;
    let _ = app.emit("inference://activity", running());
}

fn announce() {
    // Nothing to tell before setup (and nothing to tell in tests): the list
    // is still correct, it just has no listener yet.
    if crate::commands::app_handle().is_none() {
        return;
    }
    match debounce_step(Instant::now()) {
        Debounced::EmitNow => emit_now(),
        Debounced::ScheduleFlush => {
            tauri::async_runtime::spawn(async {
                tokio::time::sleep(DEBOUNCE).await;
                if take_pending(Instant::now()) {
                    emit_now();
                }
            });
        }
        Debounced::Hold => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is process-wide by design, so its tests have to take
    /// turns. Each one starts from empty.
    static TEST_TURN: Mutex<()> = Mutex::new(());

    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        let turn = TEST_TURN.lock().unwrap_or_else(|p| p.into_inner());
        *RUNNING.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *EMIT.lock().unwrap_or_else(|p| p.into_inner()) = None;
        turn
    }

    /// Guards are the whole contract: what begins ends, and only when the
    /// guard goes away.
    #[tokio::test]
    async fn guards_open_and_close_entries() {
        let _turn = exclusive();
        let a = begin("ollama", "muse-glimmer:30b");
        let b = begin("builtin", "");
        assert_eq!(count(), 2);
        let items = running();
        assert_eq!(items[0].kind, "ollama");
        assert_eq!(items[0].model, "muse-glimmer:30b");
        // Oldest first, whatever order they end in.
        assert!(items[0].id < items[1].id);
        drop(a);
        assert_eq!(count(), 1);
        assert_eq!(running()[0].kind, "builtin");
        drop(b);
        assert_eq!(count(), 0);
        assert!(running().is_empty());
    }

    /// A call that panics or errors out mid-flight still ends: the guard is
    /// dropped by unwinding, not by a line somebody remembered to write.
    #[tokio::test]
    async fn an_abandoned_call_does_not_linger() {
        let _turn = exclusive();
        {
            let _g = begin("gateway", "gpt-x");
            assert_eq!(count(), 1);
        }
        assert_eq!(count(), 0);
    }

    /// The caller's sentence reaches the entry through the task-local scope,
    /// and an unlabeled call still registers.
    // The turn lock is a test harness, not app state: holding it across the
    // scoped await is exactly what serializes these tests.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn the_enclosing_scope_names_the_work() {
        let _turn = exclusive();
        labeled("Morning Brief", async {
            let _g = begin("ollama", "m");
            assert_eq!(running()[0].label, "Morning Brief");
        })
        .await;
        let _g = begin("ollama", "m");
        assert_eq!(running()[0].label, "");
    }

    /// A burst inside one window emits once at its head and arms exactly one
    /// trailing flush, so hundreds of short embed calls cost two repaints —
    /// and the last state still gets out.
    #[test]
    fn the_debounce_coalesces_a_burst_but_keeps_its_tail() {
        let _turn = exclusive();
        let t0 = Instant::now();
        assert_eq!(debounce_step(t0), Debounced::EmitNow);
        assert_eq!(debounce_step(t0), Debounced::ScheduleFlush);
        for _ in 0..50 {
            assert_eq!(debounce_step(t0), Debounced::Hold);
        }
        // The armed flush finds the owed emit, and takes it exactly once.
        let flush_at = t0 + DEBOUNCE;
        assert!(take_pending(flush_at));
        assert!(!take_pending(flush_at));
        // Past the window, the next change goes out immediately again.
        assert_eq!(
            debounce_step(flush_at + DEBOUNCE + Duration::from_millis(1)),
            Debounced::EmitNow
        );
    }
}
