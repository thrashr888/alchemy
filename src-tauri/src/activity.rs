//! Read-time aggregation for Settings → Activity (docs/RFC-activity-view.md).
//!
//! Nothing here writes anything: every number derives from timestamps the app
//! has always recorded (table `created_at`s, retrieval trace `ts`), so the
//! view is fully retroactive and there are no counters to keep consistent.

use crate::models::{ActivityCount, ActivityDay, ActivityStats};
use chrono::{Local, NaiveDate, TimeZone, Timelike};
use std::collections::HashMap;
use std::path::Path;

/// Cap for the "most used" lists — enough to be interesting, short enough
/// to stay a glance.
const TOP_N: usize = 8;

/// Millis → local calendar date. None for timestamps chrono can't map
/// (garbage rows far outside the representable range).
fn local_date(ms: i64) -> Option<NaiveDate> {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.date_naive())
}

fn local_hour(ms: i64) -> Option<usize> {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.hour() as usize)
}

/// Strip the ` · $0.04` cost suffix chat captions carry so "Claude Code"
/// aggregates as one model regardless of what each turn cost.
fn model_label(caption: &str) -> &str {
    caption.split(" · $").next().unwrap_or(caption).trim()
}

/// Timestamps from retained retrieval traces (current file + the one rotated
/// generation trace.rs keeps). A few MB at most; unparseable lines skip.
pub fn trace_times(dir: &Path) -> Vec<i64> {
    let mut out = Vec::new();
    for file in ["retrieval.1.jsonl", "retrieval.jsonl"] {
        let Ok(text) = std::fs::read_to_string(dir.join(file)) else {
            continue;
        };
        for line in text.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(ts) = v.get("ts").and_then(|t| t.as_i64()) {
                    out.push(ts);
                }
            }
        }
    }
    out
}

/// Aggregate row-level metadata into everything the Activity tab renders.
///
/// - `messages`: (notebook_id, role, model caption, created_at, words) —
///   chat turns only, tool rows already excluded at the scan.
/// - `sources`: (source_type, created_at, char_count).
/// - `today`: the local date to anchor streaks on — a parameter so tests
///   don't depend on the wall clock.
pub fn aggregate(
    messages: &[(String, String, String, i64, i64)],
    note_times: &[i64],
    sources: &[(String, i64, i64)],
    notebook_titles: &HashMap<String, String>,
    retrieval_times: &[i64],
    today: NaiveDate,
) -> ActivityStats {
    let mut days: HashMap<NaiveDate, ActivityDay> = HashMap::new();
    let mut hours = [0i64; 24];
    let mut model_counts: HashMap<String, (i64, i64)> = HashMap::new(); // count, last ts
    let mut notebook_counts: HashMap<String, i64> = HashMap::new();
    let mut type_counts: HashMap<String, i64> = HashMap::new();
    let mut stats = ActivityStats::default();
    let mut first = i64::MAX;

    for (notebook_id, role, model, ts, words) in messages {
        stats.total_messages += 1;
        first = first.min(*ts);
        if let Some(d) = local_date(*ts) {
            days.entry(d).or_default().messages += 1;
        }
        if let Some(h) = local_hour(*ts) {
            hours[h] += 1;
        }
        *notebook_counts.entry(notebook_id.clone()).or_default() += 1;
        if role == "user" {
            stats.total_user_messages += 1;
        } else {
            stats.assistant_words += words;
            let label = model_label(model);
            if !label.is_empty() {
                let e = model_counts.entry(label.to_string()).or_default();
                e.0 += 1;
                e.1 = e.1.max(*ts);
            }
        }
    }

    for (source_type, ts, chars) in sources {
        stats.total_sources += 1;
        stats.corpus_chars += chars;
        first = first.min(*ts);
        if let Some(d) = local_date(*ts) {
            days.entry(d).or_default().sources += 1;
        }
        *type_counts.entry(source_type.clone()).or_default() += 1;
    }

    for ts in note_times {
        stats.total_notes += 1;
        first = first.min(*ts);
        if let Some(d) = local_date(*ts) {
            days.entry(d).or_default().notes += 1;
        }
    }

    for ts in retrieval_times {
        stats.total_retrievals += 1;
        if let Some(d) = local_date(*ts) {
            days.entry(d).or_default().retrievals += 1;
        }
    }

    stats.total_notebooks = notebook_titles.len() as i64;
    stats.first_activity_at = if first == i64::MAX { 0 } else { first };
    stats.peak_hour = if stats.total_messages > 0 {
        (0..24).max_by_key(|&h| hours[h]).unwrap_or(0) as i64
    } else {
        -1
    };

    // Streaks over the set of active local days.
    let mut dates: Vec<NaiveDate> = days.keys().copied().collect();
    dates.sort();
    stats.active_days = dates.len() as i64;
    let mut longest = 0i64;
    let mut run = 0i64;
    let mut prev: Option<NaiveDate> = None;
    for d in &dates {
        run = match prev {
            Some(p) if (*d - p).num_days() == 1 => run + 1,
            _ => 1,
        };
        longest = longest.max(run);
        prev = Some(*d);
    }
    stats.longest_streak = longest;
    // Current streak anchors on today *or yesterday* — opening the app at
    // 9am shouldn't show a broken streak before you've done anything.
    let anchor = [today, today - chrono::Days::new(1)]
        .into_iter()
        .find(|d| days.contains_key(d));
    if let Some(mut d) = anchor {
        while days.contains_key(&d) {
            stats.current_streak += 1;
            match d.checked_sub_days(chrono::Days::new(1)) {
                Some(p) => d = p,
                None => break,
            }
        }
    }

    // "Most used" lists: count desc, ties toward the more recently used
    // (models) or the alphabetically first (the rest, for stable output).
    let mut models: Vec<(String, i64, i64)> = model_counts
        .into_iter()
        .map(|(label, (count, last))| (label, count, last))
        .collect();
    models.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    stats.favorite_model = models.first().map(|m| m.0.clone()).unwrap_or_default();
    stats.models = models
        .into_iter()
        .take(TOP_N)
        .map(|(label, count, _)| ActivityCount { label, count })
        .collect();

    // Notebook message counts join to titles; deleted notebooks aggregate
    // under one label and sink below every living one.
    let mut nb_label_counts: HashMap<String, i64> = HashMap::new();
    for (id, count) in notebook_counts {
        let label = notebook_titles
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "(deleted)".to_string());
        *nb_label_counts.entry(label).or_default() += count;
    }
    let mut notebooks: Vec<ActivityCount> = nb_label_counts
        .into_iter()
        .map(|(label, count)| ActivityCount { label, count })
        .collect();
    notebooks.sort_by(|a, b| {
        (a.label == "(deleted)")
            .cmp(&(b.label == "(deleted)"))
            .then(b.count.cmp(&a.count))
            .then(a.label.cmp(&b.label))
    });
    notebooks.truncate(TOP_N);
    stats.notebooks = notebooks;

    let mut source_types: Vec<ActivityCount> = type_counts
        .into_iter()
        .map(|(label, count)| ActivityCount { label, count })
        .collect();
    source_types.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
    source_types.truncate(TOP_N);
    stats.source_types = source_types;

    stats.days = dates
        .into_iter()
        .map(|d| {
            let mut day = days.remove(&d).unwrap_or_default();
            day.date = d.format("%Y-%m-%d").to_string();
            day
        })
        .collect();
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Local-noon millis for a date, so bucketing tests hold in any timezone.
    fn noon(y: i32, m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, 12, 0, 0)
            .single()
            .expect("noon is never ambiguous")
            .timestamp_millis()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn msg(
        nb: &str,
        role: &str,
        model: &str,
        ts: i64,
        words: i64,
    ) -> (String, String, String, i64, i64) {
        (nb.into(), role.into(), model.into(), ts, words)
    }

    #[test]
    fn empty_inputs_are_a_fresh_install() {
        let s = aggregate(&[], &[], &[], &HashMap::new(), &[], date(2026, 8, 20));
        assert_eq!(s.total_messages, 0);
        assert_eq!(s.peak_hour, -1);
        assert_eq!(s.current_streak, 0);
        assert_eq!(s.first_activity_at, 0);
        assert!(s.days.is_empty());
    }

    #[test]
    fn streaks_and_day_buckets() {
        // Active Aug 17, 18, 19 (yesterday); today Aug 20 has nothing yet.
        let messages = vec![
            msg("nb1", "user", "", noon(2026, 8, 17), 3),
            msg("nb1", "assistant", "Ollama · $0.01", noon(2026, 8, 17), 50),
            msg("nb1", "user", "", noon(2026, 8, 18), 3),
            msg("nb1", "user", "", noon(2026, 8, 19), 3),
        ];
        // A gap-separated earlier run of 2 days.
        let notes = vec![noon(2026, 8, 1), noon(2026, 8, 2)];
        let s = aggregate(
            &messages,
            &notes,
            &[],
            &HashMap::new(),
            &[],
            date(2026, 8, 20),
        );
        assert_eq!(s.active_days, 5);
        assert_eq!(s.longest_streak, 3);
        assert_eq!(s.current_streak, 3, "anchors on yesterday");
        assert_eq!(s.days.len(), 5);
        assert_eq!(s.days[0].date, "2026-08-01");
        assert_eq!(s.days[4].messages, 1);
    }

    #[test]
    fn current_streak_breaks_after_a_quiet_day() {
        let messages = vec![msg("nb1", "user", "", noon(2026, 8, 17), 1)];
        let s = aggregate(&messages, &[], &[], &HashMap::new(), &[], date(2026, 8, 20));
        assert_eq!(s.longest_streak, 1);
        assert_eq!(s.current_streak, 0, "two days quiet = no live streak");
    }

    #[test]
    fn favorite_model_strips_cost_and_prefers_recent_on_ties() {
        let messages = vec![
            msg(
                "nb1",
                "assistant",
                "Claude Code · $0.04",
                noon(2026, 8, 1),
                10,
            ),
            msg(
                "nb1",
                "assistant",
                "Claude Code · $0.10",
                noon(2026, 8, 2),
                10,
            ),
            msg("nb1", "assistant", "Ollama", noon(2026, 8, 3), 10),
            msg("nb1", "assistant", "Ollama", noon(2026, 8, 4), 10),
        ];
        let s = aggregate(&messages, &[], &[], &HashMap::new(), &[], date(2026, 8, 20));
        assert_eq!(s.favorite_model, "Ollama", "tie breaks to more recent");
        assert_eq!(s.models.len(), 2);
        assert_eq!(s.models[0].count, 2);
        assert_eq!(s.assistant_words, 40);
        assert_eq!(s.total_user_messages, 0);
    }

    #[test]
    fn deleted_notebooks_aggregate_and_sink() {
        let messages = vec![
            msg("gone1", "user", "", noon(2026, 8, 1), 1),
            msg("gone2", "user", "", noon(2026, 8, 1), 1),
            msg("kept", "user", "", noon(2026, 8, 1), 1),
        ];
        let titles = HashMap::from([("kept".to_string(), "Research".to_string())]);
        let s = aggregate(&messages, &[], &[], &titles, &[], date(2026, 8, 20));
        assert_eq!(s.notebooks.len(), 2);
        assert_eq!(s.notebooks[0].label, "Research");
        assert_eq!(s.notebooks[1].label, "(deleted)");
        assert_eq!(s.notebooks[1].count, 2, "deleted sinks even with more");
    }
}
