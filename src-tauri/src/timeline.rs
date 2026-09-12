//! The corpus by when it arrived (docs/RFC-timeline.md): every source and
//! note on one axis by `created_at`, grouped into the batches they came in.
//!
//! Additions only. `updated_at` and `source_events` churn (a folder re-reads
//! on every FSEvent, a URL re-fetches on its cadence), so plotting them puts
//! the same document on the axis dozens of times. `created_at` is written
//! once, at import, and is what a person means by "when I brought this in".
//!
//! The unit is the batch, not the row: the store is bursty (a folder import
//! lands hundreds of rows in seconds, then a single note lands two days
//! later), and a timeline of individual marks is a few dense smears and
//! long empty runs. Batching is pure and read-time — no stored grouping.

use crate::models::{CorpusTimeline, TimelineBatch, TimelineItem};
use std::collections::HashMap;

/// Rows within this many milliseconds of the previous row in the same
/// notebook belong to one batch. Half an hour: a folder import spans
/// seconds, a reading session spans an evening — both stay one batch, while
/// morning and evening sessions stay two.
pub const BATCH_GAP_MS: i64 = 30 * 60 * 1000;

/// Sample titles kept per batch for surfaces that can't show every item
/// (the hover card, the MCP tool).
const SAMPLES: usize = 8;

/// One document as the scan yields it — the batch it joins is decided here.
#[derive(Debug, Clone)]
pub struct TimelineRow {
    pub notebook_id: String,
    pub item: TimelineItem,
}

/// What the batcher needs to know about a notebook: its title and color
/// for the lane, and whether it is a system notebook (Briefs), which stays
/// off the axis the way it stays off the shelf.
#[derive(Debug, Clone)]
pub struct LaneInfo {
    pub title: String,
    pub color: String,
    pub system: bool,
}

/// Group rows into per-notebook batches at `gap_ms`. Rows whose notebook is
/// unknown (a deleted notebook's orphans) or a system notebook are dropped.
/// Batches come back oldest first; `with_items` false keeps only the sample
/// titles, for callers that want the shape without the rows.
pub fn batch(
    mut rows: Vec<TimelineRow>,
    lanes: &HashMap<String, LaneInfo>,
    gap_ms: i64,
    with_items: bool,
) -> CorpusTimeline {
    rows.sort_by_key(|r| r.item.created_at);
    // The open batch per notebook, closed when a row lands past the gap.
    let mut open: HashMap<String, TimelineBatch> = HashMap::new();
    let mut done: Vec<TimelineBatch> = Vec::new();
    let mut sources = 0;
    let mut notes = 0;
    for row in rows {
        let Some(lane) = lanes.get(&row.notebook_id) else {
            continue;
        };
        if lane.system {
            continue;
        }
        let at = row.item.created_at;
        if let Some(b) = open.get(&row.notebook_id) {
            if at - b.end > gap_ms {
                let closed = open.remove(&row.notebook_id).unwrap();
                done.push(closed);
            }
        }
        let b = open
            .entry(row.notebook_id.clone())
            .or_insert_with(|| TimelineBatch {
                notebook_id: row.notebook_id.clone(),
                notebook_title: lane.title.clone(),
                notebook_color: lane.color.clone(),
                start: at,
                end: at,
                sources: 0,
                notes: 0,
                samples: Vec::new(),
                items: Vec::new(),
            });
        b.end = at;
        if row.item.kind == "note" {
            b.notes += 1;
            notes += 1;
        } else {
            b.sources += 1;
            sources += 1;
        }
        if b.samples.len() < SAMPLES {
            b.samples.push(row.item.title.clone());
        }
        if with_items {
            b.items.push(row.item);
        }
    }
    done.extend(open.into_values());
    done.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then(a.notebook_id.cmp(&b.notebook_id))
    });
    let first = done.first().map(|b| b.start).unwrap_or(0);
    let last = done.iter().map(|b| b.end).max().unwrap_or(0);
    CorpusTimeline {
        batches: done,
        first,
        last,
        sources,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lanes() -> HashMap<String, LaneInfo> {
        let mut m = HashMap::new();
        for (id, system) in [("a", false), ("b", false), ("briefs", true)] {
            m.insert(
                id.to_string(),
                LaneInfo {
                    title: id.to_uppercase(),
                    color: "#123456".into(),
                    system,
                },
            );
        }
        m
    }

    fn row(nb: &str, kind: &str, at: i64) -> TimelineRow {
        TimelineRow {
            notebook_id: nb.into(),
            item: TimelineItem {
                id: format!("{nb}-{kind}-{at}"),
                kind: kind.into(),
                title: format!("{kind} at {at}"),
                source_type: if kind == "note" {
                    "note".into()
                } else {
                    "pdf".into()
                },
                origin: String::new(),
                created_at: at,
            },
        }
    }

    const MIN: i64 = 60_000;

    #[test]
    fn a_folder_import_is_one_batch_and_a_later_note_is_another() {
        // 264 files in 20 seconds, then a note two hours on.
        let mut rows: Vec<_> = (0..264).map(|i| row("a", "source", i * 76)).collect();
        rows.push(row("a", "note", 2 * 60 * MIN));
        let t = batch(rows, &lanes(), BATCH_GAP_MS, true);
        assert_eq!(t.batches.len(), 2);
        assert_eq!(t.batches[0].sources, 264);
        assert_eq!(t.batches[0].notes, 0);
        assert_eq!(t.batches[0].items.len(), 264);
        assert_eq!(t.batches[0].samples.len(), 8);
        assert_eq!(t.batches[1].notes, 1);
        assert_eq!((t.sources, t.notes), (264, 1));
        assert_eq!(t.first, 0);
        assert_eq!(t.last, 2 * 60 * MIN);
    }

    #[test]
    fn a_note_every_two_hours_is_a_batch_each() {
        let rows: Vec<_> = (0..5).map(|i| row("a", "note", i * 120 * MIN)).collect();
        let t = batch(rows, &lanes(), BATCH_GAP_MS, true);
        assert_eq!(t.batches.len(), 5);
        assert!(t.batches.iter().all(|b| b.notes == 1 && b.start == b.end));
    }

    #[test]
    fn interleaved_notebooks_keep_their_own_batches() {
        // A and B alternate every ten minutes: each notebook's rows are
        // within the gap of each other, so two batches, not eight.
        let rows: Vec<_> = (0..8)
            .map(|i| row(if i % 2 == 0 { "a" } else { "b" }, "source", i * 10 * MIN))
            .collect();
        let t = batch(rows, &lanes(), BATCH_GAP_MS, true);
        assert_eq!(t.batches.len(), 2);
        assert_eq!(t.batches[0].notebook_id, "a");
        assert_eq!(t.batches[1].notebook_id, "b");
        assert!(t.batches.iter().all(|b| b.sources == 4));
    }

    #[test]
    fn unknown_and_system_notebooks_stay_off_the_axis() {
        let rows = vec![
            row("a", "source", 0),
            row("gone", "source", 1),
            row("briefs", "note", 2),
        ];
        let t = batch(rows, &lanes(), BATCH_GAP_MS, false);
        assert_eq!(t.batches.len(), 1);
        assert_eq!(t.batches[0].notebook_title, "A");
        assert!(t.batches[0].items.is_empty());
        assert_eq!(t.batches[0].samples, vec!["source at 0"]);
    }

    #[test]
    fn unsorted_input_is_batched_by_time_not_arrival() {
        let rows = vec![
            row("a", "source", 5 * MIN),
            row("a", "source", 0),
            row("a", "source", 90 * MIN),
        ];
        let t = batch(rows, &lanes(), BATCH_GAP_MS, true);
        assert_eq!(t.batches.len(), 2);
        assert_eq!(t.batches[0].start, 0);
        assert_eq!(t.batches[0].end, 5 * MIN);
        assert_eq!(t.batches[0].items[0].created_at, 0);
    }
}
