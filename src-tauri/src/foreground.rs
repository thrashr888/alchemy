//! Foreground first on the local model queue.
//!
//! Ollama serves one request per model at a time. The gist, section, and
//! tag sweeps issue Small-role calls back to back (3–4 s each), so a helper
//! in front of an answer — the gap query, the outline pick, ten-second
//! ceiling — queued behind them and timed out on a perfectly healthy
//! server (measured: `gapMs 10001` with a sweep in flight).
//!
//! The rule: a person's request holds a [`Guard`] for its whole duration
//! (chat, deep research, ask-everything, studio generation), and a
//! background handle (`Ai::background`) waits for the count to reach zero
//! before each Small-role call. The sweep finishes the call it is on — at
//! most a few seconds — then stands aside until the foreground is quiet.
//! Nothing is cancelled, nothing is lost; the sweep just goes last.

use std::sync::atomic::{AtomicUsize, Ordering};

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// A foreground request in flight. Dropping it releases the queue.
pub struct Guard(());

/// Mark a person's request as in flight until the guard drops.
pub fn begin() -> Guard {
    ACTIVE.fetch_add(1, Ordering::SeqCst);
    Guard(())
}

impl Drop for Guard {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Foreground requests currently in flight.
pub fn active() -> usize {
    ACTIVE.load(Ordering::SeqCst)
}

/// Poll spacing while a background call waits its turn.
const TICK: std::time::Duration = std::time::Duration::from_millis(250);

/// Wait until no foreground request is in flight. Returns at once when
/// the queue is quiet; otherwise polls, noting every ten seconds so a
/// stalled sweep reads as waiting rather than dead.
pub async fn wait_idle() {
    let mut waited = std::time::Duration::ZERO;
    while active() > 0 {
        tokio::time::sleep(TICK).await;
        waited += TICK;
        if waited.as_millis().is_multiple_of(10_000) {
            crate::note!(
                "background: waiting on {} foreground request(s), {}s",
                active(),
                waited.as_secs()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Idle returns at once; a held guard holds `wait_idle` until it drops.
    #[tokio::test]
    async fn background_waits_for_the_foreground() {
        let t = std::time::Instant::now();
        wait_idle().await;
        assert!(t.elapsed() < TICK, "idle queue should not wait");

        let guard = begin();
        assert_eq!(active(), 1);
        let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = released.clone();
        tokio::spawn(async move {
            tokio::time::sleep(TICK * 2).await;
            flag.store(true, Ordering::SeqCst);
            drop(guard);
        });
        let t = std::time::Instant::now();
        wait_idle().await;
        assert!(
            released.load(Ordering::SeqCst),
            "returned before the guard dropped"
        );
        assert!(t.elapsed() >= TICK * 2);
        assert_eq!(active(), 0);
    }
}
