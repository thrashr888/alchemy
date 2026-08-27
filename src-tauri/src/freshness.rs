//! The nightly freshness queue (docs/RFC-night-shift-area.md).
//!
//! Replaces the idea of asking the user to commission work. If a job is worth
//! doing the app should do it, so the whole control surface is one notch -
//! Light, Standard, Generous - and a priority order the user never sees:
//!
//!   1. Freshness      keep the corpus current (re-fetch, re-distill)
//!   2. Verification   judge what arrived against what the user concluded
//!   3. Hygiene        janitorial work with no model behind it
//!
//! The queue spends until its ceiling or until morning, whichever comes
//! first, and reports in *findings*. A token count is a footnote; "2
//! contradictions" is the headline. A night that finds nothing says nothing:
//! silence is the correct output of a healthy night.

use std::sync::atomic::{AtomicI64, Ordering};

/// Tokens spent since the current night began. Reset at the 6 AM boundary,
/// the same hour the overnight pause auto-clears, so "tonight" means one
/// thing everywhere.
static SPENT_TONIGHT: AtomicI64 = AtomicI64::new(0);
/// Epoch ms of the night the counter belongs to; a new one zeroes it.
static NIGHT_STAMP: AtomicI64 = AtomicI64::new(0);

/// Nightly ceilings. Deliberately round numbers: these are a policy, not a
/// measurement, and pretending otherwise would be false precision.
pub fn ceiling(budget: &str) -> i64 {
    match budget {
        "light" => 250_000,
        "generous" => 4_000_000,
        // "standard" and anything unrecognised - an unknown value should
        // behave like the default, never like unlimited.
        _ => 1_000_000,
    }
}

/// The night a moment belongs to: nights roll at 6 AM, so work at 2 AM and
/// work at 11 PM the previous evening share one budget.
fn night_of(now: i64) -> i64 {
    let dt = chrono::DateTime::from_timestamp_millis(now)
        .map(|d| d.with_timezone(&chrono::Local))
        .unwrap_or_else(chrono::Local::now);
    let six = chrono::NaiveTime::from_hms_opt(6, 0, 0).expect("valid time");
    let mut day = dt.date_naive();
    if dt.time() < six {
        day = day.checked_sub_days(chrono::Days::new(1)).unwrap_or(day);
    }
    day.and_time(six)
        .and_local_timezone(chrono::Local)
        .earliest()
        .map(|d| d.timestamp_millis())
        .unwrap_or(now)
}

fn roll_if_new_night(now: i64) {
    let night = night_of(now);
    if NIGHT_STAMP.swap(night, Ordering::Relaxed) != night {
        SPENT_TONIGHT.store(0, Ordering::Relaxed);
    }
}

/// Record tokens spent by background work. Called at the seams that already
/// know a generation finished; unmetered engines report nothing and simply
/// do not advance the counter.
pub fn record_spend(tokens: i64) {
    let now = crate::commands::now();
    roll_if_new_night(now);
    SPENT_TONIGHT.fetch_add(tokens.max(0), Ordering::Relaxed);
}

tokio::task_local! {
    /// Money spent by the run executing in this task, in micro-dollars.
    ///
    /// Task-scoped rather than global on purpose. Reports, the nightly Weave
    /// and the gist sweep each run under their own single-flight guard and
    /// can overlap, so a process-wide counter would bill one job for
    /// another's spend - and a receipt asserting a cost that was not this
    /// run's is the same class of lie as the "overdue by 34h" badge. A task
    /// cannot run concurrently with itself, so this attributes exactly.
    ///
    /// Unset everywhere it has not been armed, which is what keeps a
    /// foreground Studio generation - a user waiting at the keyboard - from
    /// being charged to any run at all.
    static RUN_COST_MICROS: AtomicI64;
}

/// Run `fut` as a metered unit, returning its output and what it spent.
///
/// Only engines that report a price move the number: local models are
/// genuinely free and say 0. A generation that spawns its own task escapes
/// the scope and is undercounted, which is the safe direction to be wrong -
/// a receipt that understates is recoverable from the provider's own
/// billing, one that overstates is not.
pub async fn metered_run<F, T>(fut: F) -> (T, i64)
where
    F: std::future::Future<Output = T>,
{
    RUN_COST_MICROS
        .scope(AtomicI64::new(0), async move {
            let out = fut.await;
            let spent = RUN_COST_MICROS.with(|c| c.load(Ordering::Relaxed));
            (out, spent)
        })
        .await
}

/// Attribute one generation's price to the metered run, if any is armed.
///
/// Dollars arrive as f64 from the provider and are stored as integer
/// micro-dollars: money in floats accumulates error, and a receipt is a
/// record rather than an estimate. Nothing to do when no run is armed.
pub fn record_cost(cost_usd: Option<f64>) {
    let Some(usd) = cost_usd else { return };
    if !usd.is_finite() || usd <= 0.0 {
        return;
    }
    let micros = (usd * 1_000_000.0).round() as i64;
    let _ = RUN_COST_MICROS.try_with(|c| c.fetch_add(micros, Ordering::Relaxed));
}

/// Fold one background generation's token count into tonight's spend.
/// Only background work is metered here: a user waiting at the keyboard is
/// not spending the night's budget.
pub fn record_outcome(out: &crate::inference::ChatOutcome) {
    if let Some(stats) = out.stats.as_ref() {
        record_spend(stats.eval_count as i64);
    }
    record_cost(out.cost_usd);
}

pub fn spent_tonight() -> i64 {
    roll_if_new_night(crate::commands::now());
    SPENT_TONIGHT.load(Ordering::Relaxed)
}

/// Is there budget left for another unit of work at this tier? Checked
/// between stages rather than mid-generation: stopping a run half-written
/// would waste what it already spent.
pub fn has_budget(budget: &str) -> bool {
    spent_tonight() < ceiling(budget)
}

/// What a night produced, in the units a person cares about. Counts of
/// *deltas*, never of attempts: "re-read 40 sources, 38 unchanged" is noise,
/// so only the 2 are counted here.
#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Findings {
    /// Sources whose content actually changed.
    pub refreshed: u32,
    /// Ledger rows the Weave moved to contradicted or superseded.
    pub contradictions: u32,
    /// Scheduled work that could not run, or ran and failed.
    pub problems: u32,
    pub tokens: i64,
}

impl Findings {
    /// Did the night produce anything a person would want to hear about?
    /// Tokens alone do not count - spending without finding is not news.
    pub fn worth_reporting(&self) -> bool {
        self.refreshed + self.contradictions + self.problems > 0
    }

    /// One line for the Morning Brief, denominated in findings. Returns None
    /// when the night was quiet, because the right thing to say then is
    /// nothing at all.
    pub fn brief_line(&self) -> Option<String> {
        if !self.worth_reporting() {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if self.contradictions > 0 {
            parts.push(format!(
                "found {} {}",
                self.contradictions,
                plural(self.contradictions, "contradiction", "contradictions")
            ));
        }
        if self.refreshed > 0 {
            parts.push(format!(
                "refreshed {} {}",
                self.refreshed,
                plural(self.refreshed, "source", "sources")
            ));
        }
        if self.problems > 0 {
            parts.push(format!(
                "{} {} needs you",
                self.problems,
                plural(self.problems, "item", "items")
            ));
        }
        // Tokens are the footnote, never the headline.
        let spend = if self.tokens > 0 {
            format!(" ({}K tokens)", self.tokens / 1000)
        } else {
            String::new()
        };
        Some(format!("Last night: {}{spend}.", join_prose(&parts)))
    }
}

/// What the night produced, read back out of persisted state rather than
/// tallied as it went. Source events, ledger status changes, and receipts
/// are all written anyway, so the report cannot drift from what happened -
/// and a crash mid-night loses no accounting.
pub async fn collect_findings(db: &crate::db::Db, since: i64) -> Findings {
    let events = db.source_events_since(since).await.unwrap_or_default();
    let receipts = db.list_receipts(since, 500).await.unwrap_or_default();
    let contradictions = db.ledger_upsets_since(since).await.unwrap_or(0);

    // One source that changed three times is one refreshed source, not three.
    let mut changed: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for e in &events {
        changed.insert(e.source_id.as_str());
    }

    Findings {
        refreshed: changed.len() as u32,
        contradictions,
        problems: receipts.iter().filter(|r| r.status == "failed").count() as u32,
        tokens: spent_tonight(),
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    if n == 1 { one } else { many }.to_string()
}

/// "a", "a and b", "a, b, and c" - the shapes a person writes.
fn join_prose(parts: &[String]) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let head = parts[..parts.len() - 1].join(", ");
            format!("{head}, and {}", parts[parts.len() - 1])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_night_says_nothing() {
        let quiet = Findings::default();
        assert!(!quiet.worth_reporting());
        assert_eq!(quiet.brief_line(), None);

        // Spending without finding is still not news - the whole point of
        // reporting deltas rather than attempts.
        let busy_but_empty = Findings {
            tokens: 900_000,
            ..Default::default()
        };
        assert!(!busy_but_empty.worth_reporting());
        assert_eq!(busy_but_empty.brief_line(), None);
    }

    #[test]
    fn findings_lead_and_tokens_trail() {
        let night = Findings {
            refreshed: 12,
            contradictions: 2,
            problems: 0,
            tokens: 740_000,
        };
        let line = night.brief_line().expect("worth reporting");
        assert_eq!(
            line,
            "Last night: found 2 contradictions and refreshed 12 sources (740K tokens)."
        );
        // The finding comes before the price.
        assert!(line.find("contradiction").unwrap() < line.find("tokens").unwrap());
    }

    #[test]
    fn singulars_read_like_english() {
        let one = Findings {
            refreshed: 1,
            contradictions: 1,
            problems: 1,
            ..Default::default()
        };
        assert_eq!(
            one.brief_line().unwrap(),
            "Last night: found 1 contradiction, refreshed 1 source, and 1 item needs you."
        );
    }

    #[test]
    fn an_unknown_budget_is_standard_not_unlimited() {
        assert_eq!(ceiling("standard"), 1_000_000);
        assert_eq!(ceiling("light"), 250_000);
        assert_eq!(ceiling("generous"), 4_000_000);
        assert_eq!(ceiling("wide-open"), ceiling("standard"));
        assert_eq!(ceiling(""), ceiling("standard"));
    }

    #[test]
    fn the_night_rolls_at_six_not_at_midnight() {
        // 2 AM belongs to the night that began the previous evening, so work
        // either side of midnight shares one budget.
        let two_am = chrono::Local::now()
            .date_naive()
            .and_hms_opt(2, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let eleven_pm_before = two_am - 3 * 60 * 60 * 1000;
        assert_eq!(night_of(two_am), night_of(eleven_pm_before));

        // ...and the following afternoon is a different night.
        let next_afternoon = two_am + 12 * 60 * 60 * 1000;
        assert_ne!(night_of(two_am), night_of(next_afternoon));
    }

    #[test]
    fn spending_accumulates_within_a_night() {
        SPENT_TONIGHT.store(0, Ordering::Relaxed);
        NIGHT_STAMP.store(0, Ordering::Relaxed);
        record_spend(100_000);
        record_spend(50_000);
        assert_eq!(spent_tonight(), 150_000);
        assert!(has_budget("standard"));

        record_spend(900_000);
        assert!(!has_budget("standard"), "past the standard ceiling");
        assert!(has_budget("generous"), "generous still has room");

        // A negative report cannot claw budget back.
        record_spend(-500_000);
        assert_eq!(spent_tonight(), 1_050_000);
    }

    /// A run is billed for its own generations and no one else's. The whole
    /// reason this is task-scoped: reports, the Weave and the gist sweep
    /// overlap, and a receipt naming another job's spend would be a lie
    /// stated as a measurement.
    #[tokio::test]
    async fn concurrent_runs_do_not_bill_each_other() {
        let a = metered_run(async {
            record_cost(Some(1.50));
            tokio::task::yield_now().await;
            record_cost(Some(0.25));
        });
        let b = metered_run(async {
            record_cost(Some(0.10));
            tokio::task::yield_now().await;
        });
        let ((), a_cost) = a.await;
        let ((), b_cost) = b.await;
        assert_eq!(a_cost, 1_750_000, "1.50 + 0.25 in micro-dollars");
        assert_eq!(b_cost, 100_000, "0.10 in micro-dollars");
    }

    /// Foreground work is charged to nobody. `record_cost` outside a metered
    /// run must be a no-op rather than a panic or a stray global.
    #[tokio::test]
    async fn spend_outside_a_run_is_attributed_nowhere() {
        record_cost(Some(9.99));
        let ((), cost) = metered_run(async {}).await;
        assert_eq!(cost, 0, "an unarmed generation bills no run");
    }

    /// Local models are free and must say 0, not an invented figure. Junk
    /// from a provider is refused for the same reason.
    #[tokio::test]
    async fn unpriced_and_nonsense_costs_are_ignored() {
        let ((), cost) = metered_run(async {
            record_cost(None);
            record_cost(Some(0.0));
            record_cost(Some(-5.0));
            record_cost(Some(f64::NAN));
            record_cost(Some(f64::INFINITY));
        })
        .await;
        assert_eq!(cost, 0, "only a real, positive price counts");
    }
}
