//! The live event stream for agents (docs/RFC-events.md §8).
//!
//! `GET /events?since=<ms>` beside the MCP endpoint, bearer-gated by the
//! same middleware: Server-Sent Events, one JSON `SourceEvent` per frame.
//! The connection replays the rolling window from `since` (the table is the
//! replay; the broadcast is in-memory and lossy by design), then tails every
//! event `db.add_source_event` writes. A disconnected client costs nothing —
//! the channel drops frames nobody is reading. `alchemy events --follow` is
//! this endpoint with a pretty-printer.

use std::convert::Infallible;
use std::sync::OnceLock;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{self, Stream, StreamExt};
use tauri::{AppHandle, Manager};
use tokio::sync::broadcast;

use crate::commands::AppState;
use crate::models::SourceEvent;

/// Frames buffered per subscriber before a slow reader starts losing the
/// oldest; a reader that lags simply skips to the newest, and the table has
/// the rest.
const BUFFER: usize = 256;

static TX: OnceLock<broadcast::Sender<SourceEvent>> = OnceLock::new();

fn sender() -> &'static broadcast::Sender<SourceEvent> {
    TX.get_or_init(|| broadcast::channel(BUFFER).0)
}

/// Fan an event out to every live stream. Called from the one write point
/// (`Db::add_source_event`); a send with no receivers is a no-op.
pub fn publish(event: &SourceEvent) {
    let _ = sender().send(event.clone());
}

/// `?since=<ms>` — only events after this millisecond timestamp; absent or
/// unparseable means the whole rolling window. Parsed by hand: axum's
/// `Query` extractor is a feature this build does not enable.
fn since_of(uri: &axum::http::Uri) -> i64 {
    uri.query()
        .unwrap_or("")
        .split('&')
        .find_map(|pair| pair.strip_prefix("since="))
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
}

fn frame(e: &SourceEvent) -> Result<Event, Infallible> {
    let data = serde_json::to_string(e).unwrap_or_else(|_| "{}".to_string());
    Ok(Event::default().id(e.id.clone()).data(data))
}

/// The SSE handler: replay, then tail.
pub async fn sse(
    axum::extract::State(app): axum::extract::State<AppHandle>,
    uri: axum::http::Uri,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe before reading the table so nothing written between the
    // read and the tail is missed; duplicates are filtered by id below.
    let rx = sender().subscribe();
    let state = app.state::<AppState>();
    let mut replay = state
        .db
        .source_events_since(since_of(&uri))
        .await
        .unwrap_or_default();
    replay.reverse(); // oldest first — a log reads forward
    let seen: std::collections::HashSet<String> = replay.iter().map(|e| e.id.clone()).collect();
    let since = since_of(&uri);
    let live = stream::unfold((rx, seen), move |(mut rx, mut seen)| async move {
        loop {
            match rx.recv().await {
                Ok(e) => {
                    if e.at <= since || !seen.insert(e.id.clone()) {
                        continue;
                    }
                    return Some((e, (rx, seen)));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    let frames = stream::iter(replay).chain(live).map(|e| frame(&e));
    Sse::new(frames).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, at: i64) -> SourceEvent {
        SourceEvent {
            id: id.into(),
            notebook_id: "nb".into(),
            source_id: "src".into(),
            source_title: "A feed".into(),
            kind: "added".into(),
            detail: "new entry".into(),
            diff: String::new(),
            at,
        }
    }

    #[test]
    fn since_is_read_from_the_query_string() {
        let uri: axum::http::Uri = "/events?since=1788300000000&x=1".parse().unwrap();
        assert_eq!(since_of(&uri), 1_788_300_000_000);
        let bare: axum::http::Uri = "/events".parse().unwrap();
        assert_eq!(since_of(&bare), 0);
        let junk: axum::http::Uri = "/events?since=yesterday".parse().unwrap();
        assert_eq!(since_of(&junk), 0, "unparseable means the whole window");
    }

    #[tokio::test]
    async fn published_events_reach_a_subscriber_as_json_frames() {
        let mut rx = sender().subscribe();
        publish(&event("e1", 5));
        let got = rx.recv().await.expect("one frame");
        assert_eq!(got.id, "e1");
        let json = serde_json::to_string(&got).unwrap();
        assert!(json.contains("\"kind\":\"added\""), "{json}");
        assert!(frame(&got).is_ok());
    }
}
