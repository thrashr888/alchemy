//! Mac items as sources, via the cider crate (https://github.com/thrashr888/cider).
//!
//! A `cider://` origin in a source's `url` names a living Mac item — a
//! Reminders list, a rolling Calendar window, or an Apple Notes folder. The
//! content is fetched, rendered to markdown, and ingested through the normal
//! chunk/embed path; the resync sweep re-fetches on a gentle cadence and
//! re-embeds when the content hash changes (the hash rides in the source's
//! `mtime` column). Sync is the only way data flows in; the narrow write paths
//! (edit a note, add a reminder, check one off) go to the Mac app first and
//! re-sync back — see docs/RFC-cider-tools.md.
//!
//! cider is linked, not spawned (it grew a library target in 0.3.0). It used to
//! be a `brew install cider` binary found on PATH, which meant these sources
//! silently did not exist for anyone who had not installed it — and that a CLI
//! too old for a flag we needed failed at runtime. Neither is possible now: the
//! version is compiled in. cider still shells out to macOS's own `osascript`
//! and `sqlite3`, so TCC still applies and still attributes to Alchemy.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use anyhow::{anyhow, Context};
use cider::sources as cider_lib;
use serde::Serialize;

/// One pickable item in the add-source modal's provider step.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacCollection {
    pub id: String,
    pub label: String,
    pub detail: String,
}

/// Turn a raw cider failure into something a person can act on. TCC denials
/// (Full Disk Access missing for THIS app — grants don't transfer between
/// the dev binary and the installed bundle) all reduce to one instruction.
fn friendly_cider_error(raw: &str) -> String {
    let permission = raw.contains("authorization denied")
        || raw.contains("Operation not permitted")
        || raw.contains("PermissionError")
        || raw.contains("permission_denied")
        || raw.contains("NSAppleScriptErrorNumber=-1743");
    if permission {
        return "macOS is blocking access. Grant Alchemy Full Disk Access \
                (System Settings → Privacy & Security → Full Disk Access), \
                then relaunch Alchemy."
            .to_string();
    }
    // Keep the first line — multi-line tool output isn't toast material.
    raw.lines()
        .next()
        .unwrap_or("cider call failed")
        .chars()
        .take(200)
        .collect()
}

/// Await a cider call and hand back the JSON shape its CLI used to print.
///
/// The renderers below index by field name, and cider's JSON *is*
/// `serde_json::to_value` of these same structs — so linking the crate changed
/// the transport and nothing else. Errors keep going through
/// [`friendly_cider_error`], because a TCC denial reads the same whether it
/// arrives from a subprocess or a function call.
async fn cider<T: serde::Serialize>(
    call: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<serde_json::Value> {
    let value = call
        .await
        .map_err(|e| anyhow!("{}", friendly_cider_error(&format!("{e:#}"))))?;
    serde_json::to_value(value).context("could not read cider's reply")
}

/// Content hash packed into the source's i64 `mtime` column — the sweep's
/// change signal for content that has no file mtime.
pub fn content_stamp(text: &str) -> i64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish() as i64
}

#[tauri::command]
pub fn mac_available() -> bool {
    // cider is linked into the app now, so the integration always exists; the
    // real gate is macOS permission prompts, which fire on first use. The
    // command survives so older frontends keep working.
    true
}

/// Open System Settings straight to Privacy & Security → Full Disk Access —
/// the fix for every TCC denial `friendly_cider_error` reports.
#[tauri::command]
pub fn open_privacy_settings() -> Result<(), String> {
    std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Settings/onboarding "Connect" buttons: one benign read per provider so the
/// macOS consent prompt fires at a predictable moment instead of mid-add.
#[tauri::command]
pub async fn mac_connect(provider: String) -> Result<(), String> {
    match provider.as_str() {
        "reminders" => cider(cider_lib::reminders::list(None)).await,
        "calendar" => cider(cider_lib::calendar::list(Some(0), Some(1), None, None)).await,
        "notes" => cider(cider_lib::notes::folders()).await,
        "stocks" => cider(cider_lib::stocks::watchlists()).await,
        other => return Err(format!("Unknown Mac provider: {other}")),
    }
    .map(|_| ())
    .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn list_mac_collections(provider: String) -> Result<Vec<MacCollection>, String> {
    match provider.as_str() {
        // Calendar offers rolling windows over all calendars — no cider call,
        // so the picker opens instantly and no permission prompt fires early.
        "calendar" => Ok([7u32, 30, 90]
            .iter()
            .map(|d| MacCollection {
                id: d.to_string(),
                label: format!("Next {d} days"),
                detail: "All calendars".to_string(),
            })
            .collect()),
        "reminders" => {
            let data = cider(cider_lib::reminders::list(None))
                .await
                .map_err(|e| format!("{e:#}"))?;
            let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
            for r in data.as_array().unwrap_or(&vec![]) {
                if let Some(list) = r["list"].as_str() {
                    *counts.entry(list.to_string()).or_default() += 1;
                }
            }
            Ok(counts
                .into_iter()
                .map(|(name, n)| MacCollection {
                    id: name.clone(),
                    label: name,
                    detail: format!("{n} open {}", if n == 1 { "reminder" } else { "reminders" }),
                })
                .collect())
        }
        // Individual notes, not folders — one note becomes one source (its
        // full text via `notes get`).
        // --brief (cider >= 0.1.8) skips bodies and returns the whole
        // library fast; the picker searches and groups it by folder
        // client-side.
        "notes" => {
            let data = cider(cider_lib::notes::list_brief(None, None))
                .await
                .map_err(|e| format!("{e:#}"))?;
            Ok(data
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|n| {
                    let id = n["id"].as_str()?;
                    Some(MacCollection {
                        id: id.to_string(),
                        label: n["title"].as_str().unwrap_or("Untitled").to_string(),
                        detail: n["folder"].as_str().unwrap_or("Apple Notes").to_string(),
                    })
                })
                .collect())
        }
        // Stocks watchlists — one list becomes one auto-refreshing source.
        "stocks" => {
            let data = cider(cider_lib::stocks::watchlists())
                .await
                .map_err(|e| format!("{e:#}"))?;
            Ok(data
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|w| {
                    let name = w["name"].as_str()?;
                    let n = w["symbols"].as_array().map(|s| s.len()).unwrap_or(0);
                    Some(MacCollection {
                        id: name.to_string(),
                        label: name.to_string(),
                        detail: format!("{n} {}", if n == 1 { "symbol" } else { "symbols" }),
                    })
                })
                .collect())
        }
        other => Err(format!("Unknown Mac provider: {other}")),
    }
}

/// Build the origin URI stored in the source's `url`. Collection names keep
/// their raw spelling — the field is an opaque origin string, not a real URL.
pub fn mac_uri(provider: &str, collection: &str) -> String {
    match provider {
        "calendar" => format!("cider://calendar/upcoming/{collection}"),
        "reminders" => format!("cider://reminders/list/{collection}"),
        "stocks" => format!("cider://stocks/watchlist/{collection}"),
        _ => format!("cider://notes/note/{collection}"),
    }
}

/// Fetch a cider:// origin and render it to markdown for ingestion.
/// Returns (default_title, markdown).
pub async fn fetch(uri: &str) -> anyhow::Result<(String, String)> {
    if let Some(days) = uri.strip_prefix("cider://calendar/upcoming/") {
        let data = cider(cider_lib::calendar::list(
            Some(0),
            Some(days.parse().unwrap_or(7)),
            None,
            None,
        ))
        .await?;
        return Ok((
            format!("Calendar: next {days} days"),
            render_calendar(days, &data),
        ));
    }
    if let Some(list) = uri.strip_prefix("cider://reminders/list/") {
        let data = cider(cider_lib::reminders::list(Some(list))).await?;
        return Ok((format!("Reminders: {list}"), render_reminders(list, &data)));
    }
    if let Some(id) = uri.strip_prefix("cider://notes/note/") {
        let n = cider(cider_lib::notes::get(id)).await?;
        let title = n["title"].as_str().unwrap_or("Untitled").to_string();
        let mut out = format!("# {title}\n\n");
        if let Some(f) = n["folder"].as_str() {
            out.push_str(&format!("_Apple Notes · {f}"));
            if let Some(m) = n["modified"].as_str() {
                out.push_str(&format!(
                    " · modified {}",
                    m.chars().take(10).collect::<String>()
                ));
            }
            out.push_str("_\n\n");
        }
        if let Some(body) = n["body"].as_str() {
            out.push_str(body.trim());
            out.push('\n');
        }
        return Ok((title, out));
    }
    if let Some(list) = uri.strip_prefix("cider://stocks/watchlist/") {
        // Two calls: the watchlist for membership/order, the quote cache for
        // prices. Quotes are as fresh as the Stocks app/widget keeps them.
        let lists = cider(cider_lib::stocks::watchlists()).await?;
        let symbols: Vec<String> = lists
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .find(|w| w["name"].as_str() == Some(list))
            .and_then(|w| w["symbols"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|s| s.as_str().map(String::from))
            .collect();
        if symbols.is_empty() {
            anyhow::bail!("Watchlist \"{list}\" not found in Apple Stocks (was it renamed?)");
        }
        let quotes = cider(cider_lib::stocks::fetch()).await?;
        return Ok((
            format!("Stocks: {list}"),
            render_stocks(list, &symbols, &quotes),
        ));
    }
    // Legacy folder-as-source origins keep syncing.
    if let Some(folder) = uri.strip_prefix("cider://notes/folder/") {
        let data = cider(cider_lib::notes::list(Some(folder), None, None)).await?;
        let mut out = format!("# Apple Notes — {folder}\n\n");
        for n in data.as_array().unwrap_or(&vec![]) {
            out.push_str(&format!(
                "## {}\n",
                n["title"].as_str().unwrap_or("Untitled")
            ));
            if let Some(m) = n["modified"].as_str() {
                out.push_str(&format!(
                    "_Modified {}_\n\n",
                    m.chars().take(10).collect::<String>()
                ));
            }
            if let Some(body) = n["body"].as_str() {
                out.push_str(body.trim());
                out.push_str("\n\n");
            }
        }
        return Ok((format!("Notes: {folder}"), out));
    }
    anyhow::bail!("Unrecognized Mac source origin: {uri}")
}

// ---- Renderers -------------------------------------------------------------
//
// Pure functions from cider's JSON to the markdown a source stores. The shapes
// are a contract, not a style: the chat's live answer cards
// (src/lib/liveCards.ts, docs/RFC-events.md §7) parse this text back into
// native tables, so every line is fixed-order and one item per line. The
// tests below pin each shape; change one only together with its parser.

/// `# Calendar — next N days`, a `## YYYY-MM-DD` heading per day, then one
/// `- HH:MM — Title (Calendar)[ at Location]` per event (`all day` in the
/// time slot for all-day events); notes ride as an indented `  - ` line.
fn render_calendar(days: &str, data: &serde_json::Value) -> String {
    let mut out = format!("# Calendar — next {days} days\n\n");
    let mut last_day = String::new();
    for e in data.as_array().unwrap_or(&vec![]) {
        let start = e["start_date"].as_str().unwrap_or("");
        let day = start.chars().take(10).collect::<String>();
        if day != last_day && !day.is_empty() {
            out.push_str(&format!("## {day}\n"));
            last_day = day;
        }
        let time = if e["is_all_day"].as_bool() == Some(true) {
            "all day".to_string()
        } else {
            start.chars().skip(11).take(5).collect()
        };
        out.push_str(&format!(
            "- {} — {} ({})",
            time,
            e["title"].as_str().unwrap_or("Untitled"),
            e["calendar"].as_str().unwrap_or("Calendar"),
        ));
        if let Some(loc) = e["location"].as_str() {
            out.push_str(&format!(" at {loc}"));
        }
        out.push('\n');
        if let Some(notes) = e["notes"].as_str() {
            if !notes.trim().is_empty() {
                out.push_str(&format!("  - {}\n", notes.trim().replace('\n', " ")));
            }
        }
    }
    out
}

/// `# Reminders — List`, then one `- [ ] Title \`id\`[ — due YYYY-MM-DD]`
/// per open reminder; notes ride as an indented `  - ` line.
fn render_reminders(list: &str, data: &serde_json::Value) -> String {
    let mut out = format!("# Reminders — {list}\n\n");
    for r in data.as_array().unwrap_or(&vec![]) {
        out.push_str(&format!(
            "- [ ] {}",
            r["title"].as_str().unwrap_or("Untitled")
        ));
        // Carry the id into the text: titles repeat (two identical bug
        // reports is the case that prompted this), so it is the only way
        // for a reader — or an agent — to name one reminder exactly when
        // asking to complete it.
        if let Some(id) = r["id"].as_str() {
            out.push_str(&format!(" `{id}`"));
        }
        if let Some(due) = r["due_date"].as_str() {
            out.push_str(&format!(
                " — due {}",
                due.chars().take(10).collect::<String>()
            ));
        }
        out.push('\n');
        if let Some(notes) = r["notes"].as_str() {
            if !notes.trim().is_empty() {
                out.push_str(&format!("  - {}\n", notes.trim().replace('\n', " ")));
            }
        }
    }
    out
}

/// `# Stocks — List`, a five-column table `| Symbol | Name | Price | Change |
/// Status |` in watchlist order (price as `123.45 USD`, change as `+1.23%`,
/// blanks for symbols the quote cache lacks), then
/// `_Prices as of <RFC 3339> (Apple Stocks cache)._` when any quote carried a
/// time. Quotes are as fresh as the Stocks app/widget keeps them.
fn render_stocks(list: &str, symbols: &[String], quotes: &serde_json::Value) -> String {
    let mut out = format!("# Stocks — {list}\n\n");
    let mut as_of = "";
    let empty = vec![];
    let rows = quotes.as_array().unwrap_or(&empty);
    out.push_str("| Symbol | Name | Price | Change | Status |\n");
    out.push_str("|---|---|---|---|---|\n");
    for sym in symbols {
        let q = rows.iter().find(|q| q["symbol"].as_str() == Some(sym));
        let (name, price, pct, status) = match q {
            Some(q) => {
                if let Some(t) = q["as_of"].as_str() {
                    if t > as_of {
                        as_of = t;
                    }
                }
                (
                    q["name"].as_str().unwrap_or("").to_string(),
                    q["price"]
                        .as_f64()
                        .map(|p| format!("{p:.2} {}", q["currency"].as_str().unwrap_or("")))
                        .unwrap_or_default(),
                    q["change_percent"]
                        .as_f64()
                        .map(|c| format!("{c:+.2}%"))
                        .unwrap_or_default(),
                    q["exchange_status"].as_str().unwrap_or("").to_string(),
                )
            }
            None => (String::new(), String::new(), String::new(), String::new()),
        };
        out.push_str(&format!(
            "| {sym} | {name} | {price} | {pct} | {status} |\n"
        ));
    }
    if !as_of.is_empty() {
        out.push_str(&format!("\n_Prices as of {as_of} (Apple Stocks cache)._\n"));
    }
    out
}

// ---- Item events -----------------------------------------------------------
//
// Phase 5 of docs/RFC-events.md: a Mac resync names *which item* changed,
// not that "the list" did. The stored text already carries what a delta
// needs — reminder ids and due dates, calendar days and times, one item per
// line (the renderers above) — so the diff of the old and new renderings
// *is* the delta: no high-water mark to persist, no second cider call, and
// the same answer whether the change arrived through the store watch
// (macwatch.rs), the minute sweep, or one of our own write-backs. cider 0.6's
// `since` filters stay available for a pass that wants the store's own view.

/// Item-level events between two renderings of one `cider://` source, as
/// `(kind, detail)` pairs in the RFC §1 vocabulary (`completed`, `moved`,
/// `added`, `removed`, `updated`). Empty for providers with no items to name
/// — a note or a stocks table — where the generic `updated` diff is the
/// right event, and empty when only sub-lines (reminder notes, event notes)
/// changed, for the same reason.
pub fn item_events(url: &str, old_text: &str, new_text: &str) -> Vec<(&'static str, String)> {
    if url.starts_with("cider://reminders/list/") {
        reminder_events(old_text, new_text)
    } else if url.starts_with("cider://calendar/upcoming/") {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        calendar_events(old_text, new_text, &today)
    } else {
        Vec::new()
    }
}

struct ReminderLine<'a> {
    title: &'a str,
    id: &'a str,
    due: Option<&'a str>,
}

/// One `render_reminders` item line back into its parts; `None` for
/// anything else (headings, the indented notes lines, a line with no id).
fn parse_reminder_line(line: &str) -> Option<ReminderLine<'_>> {
    let rest = line.strip_prefix("- [ ] ")?;
    let (body, due) = match rest.rsplit_once(" \u{2014} due ") {
        Some((body, due)) => (body, Some(due.trim())),
        None => (rest, None),
    };
    let body = body.strip_suffix('`')?;
    let (title, id) = body.rsplit_once(" `")?;
    Some(ReminderLine { title, id, due })
}

/// cider lists open reminders only, so an id that vanished was completed
/// (or deleted — the store does not say, and "done" is the common case).
fn reminder_events(old: &str, new: &str) -> Vec<(&'static str, String)> {
    let old_items: Vec<ReminderLine<'_>> = old.lines().filter_map(parse_reminder_line).collect();
    let new_items: Vec<ReminderLine<'_>> = new.lines().filter_map(parse_reminder_line).collect();
    let old_by_id: std::collections::HashMap<&str, &ReminderLine<'_>> =
        old_items.iter().map(|r| (r.id, r)).collect();
    let new_by_id: std::collections::HashMap<&str, &ReminderLine<'_>> =
        new_items.iter().map(|r| (r.id, r)).collect();
    let mut out = Vec::new();
    for r in &old_items {
        if !new_by_id.contains_key(r.id) {
            out.push(("completed", format!("\u{2713} {}", r.title)));
        }
    }
    for r in &new_items {
        match old_by_id.get(r.id) {
            None => out.push(("added", format!("new reminder \u{00b7} {}", r.title))),
            Some(o) if o.due != r.due => out.push((
                "moved",
                format!(
                    "{} \u{00b7} due {} \u{2192} {}",
                    r.title,
                    o.due.unwrap_or("no date"),
                    r.due.unwrap_or("no date")
                ),
            )),
            Some(o) if o.title != r.title => out.push((
                "updated",
                format!("renamed \u{00b7} {} \u{2192} {}", o.title, r.title),
            )),
            Some(_) => {}
        }
    }
    out
}

struct CalendarLine<'a> {
    /// `Title (Calendar)` — the location suffix is dropped so a venue change
    /// reads as the same event.
    key: &'a str,
    day: &'a str,
    time: &'a str,
}

/// Every `render_calendar` item line with the `## YYYY-MM-DD` it sits under.
fn parse_calendar(text: &str) -> Vec<CalendarLine<'_>> {
    let mut day = "";
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(d) = line.strip_prefix("## ") {
            day = d.trim();
            continue;
        }
        // Note lines are indented (`  - `), so they miss this prefix.
        let Some(rest) = line.strip_prefix("- ") else {
            continue;
        };
        let Some((time, key)) = rest.split_once(" \u{2014} ") else {
            continue;
        };
        if day.is_empty() {
            continue;
        }
        let key = match key.rfind(") at ") {
            Some(i) => &key[..=i],
            None => key,
        };
        out.push(CalendarLine { key, day, time });
    }
    out
}

fn calendar_title(key: &str) -> &str {
    key.rsplit_once(" (").map(|(t, _)| t).unwrap_or(key)
}

/// `Thu 2 PM`, `Thu Sep 3 2 PM` with the date, `Thu all day`; falls back
/// to the raw fields when a line does not parse.
fn when_label(day: &str, time: &str, with_date: bool) -> String {
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map(|d| {
            if with_date {
                d.format("%a %b %-d").to_string()
            } else {
                d.format("%a").to_string()
            }
        })
        .unwrap_or_else(|_| day.to_string());
    let clock = match time.split_once(':') {
        Some((h, m)) => match (h.parse::<u32>(), m.parse::<u32>()) {
            (Ok(h), Ok(m)) if h < 24 && m < 60 => {
                let (h12, half) = match h {
                    0 => (12, "AM"),
                    1..=11 => (h, "AM"),
                    12 => (12, "PM"),
                    _ => (h - 12, "PM"),
                };
                if m == 0 {
                    format!("{h12} {half}")
                } else {
                    format!("{h12}:{m:02} {half}")
                }
            }
            _ => time.to_string(),
        },
        None => time.to_string(),
    };
    format!("{date} {clock}")
}

/// A rolling window slides every fetch: days before `today` fell off the
/// front and days past the old rendering's last event are new to the window,
/// not new to the calendar. Both are dropped before matching, so a daily
/// standup never reads as seven removals and seven arrivals. Within the
/// overlap, exact `(title, day, time)` matches cancel; a leftover new line
/// pairs with a leftover old line of the same title as `moved`, and what is
/// still left is `added` or `removed`.
fn calendar_events(old: &str, new: &str, today: &str) -> Vec<(&'static str, String)> {
    let old_lines = parse_calendar(old);
    let new_lines = parse_calendar(new);
    let old_last_day = old_lines.iter().map(|l| l.day).max();
    let mut old_left: Vec<&CalendarLine<'_>> =
        old_lines.iter().filter(|l| l.day >= today).collect();
    let mut new_left: Vec<&CalendarLine<'_>> = Vec::new();
    for n in new_lines
        .iter()
        .filter(|l| old_last_day.is_none_or(|last| l.day <= last))
    {
        match old_left
            .iter()
            .position(|o| o.key == n.key && o.day == n.day && o.time == n.time)
        {
            Some(i) => {
                old_left.remove(i);
            }
            None => new_left.push(n),
        }
    }
    let mut out = Vec::new();
    for n in new_left {
        match old_left.iter().position(|o| o.key == n.key) {
            Some(i) => {
                let o = old_left.remove(i);
                let with_date = o.day != n.day;
                out.push((
                    "moved",
                    format!(
                        "{} \u{00b7} {} \u{2192} {}",
                        calendar_title(n.key),
                        when_label(o.day, o.time, with_date),
                        when_label(n.day, n.time, with_date)
                    ),
                ));
            }
            None => out.push((
                "added",
                format!(
                    "new event \u{00b7} {} \u{00b7} {}",
                    calendar_title(n.key),
                    when_label(n.day, n.time, true)
                ),
            )),
        }
    }
    for o in old_left {
        out.push((
            "removed",
            format!(
                "event gone \u{00b7} {} \u{00b7} {}",
                calendar_title(o.key),
                when_label(o.day, o.time, true)
            ),
        ));
    }
    out
}

/// Is this origin a Mac item?
pub fn is_mac_uri(url: &str) -> bool {
    url.starts_with("cider://")
}

/// A reminders fetch can't tell an empty list from a nonexistent one —
/// `--list` filtering just yields no rows either way — so adds check the
/// list's existence explicitly against `reminders lists`.
pub async fn reminders_list_exists(name: &str) -> anyhow::Result<bool> {
    let data = cider(cider_lib::reminders::lists()).await?;
    Ok(data
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .any(|l| l.as_str() == Some(name)))
}

// ---- Write-back ------------------------------------------------------------
//
// Sources stay sync-driven (the Mac item is the truth), but the two providers
// with natural edit affordances accept writes: a note's body can be replaced,
// and reminders can be added to a connected list. Every write is followed by
// a normal re-fetch + re-embed, so Alchemy's copy is always what the app has.

/// Raw plaintext of a note source for editing (first line is the title —
/// Apple Notes derives the visible title from it, so editors keep it there).
pub async fn note_body(uri: &str) -> anyhow::Result<String> {
    let id = uri
        .strip_prefix("cider://notes/note/")
        .ok_or_else(|| anyhow!("Not an Apple Notes source: {uri}"))?;
    let n = cider(cider_lib::notes::get(id)).await?;
    Ok(n["body"].as_str().unwrap_or_default().to_string())
}

/// Replace the note's body (cider renders line breaks to Notes' HTML).
pub async fn update_note(uri: &str, body: &str) -> anyhow::Result<()> {
    let id = uri
        .strip_prefix("cider://notes/note/")
        .ok_or_else(|| anyhow!("Not an Apple Notes source: {uri}"))?;
    cider(cider_lib::notes::update(id, body)).await.map(|_| ())
}

/// Add a reminder to the list this source mirrors.
pub async fn add_reminder(uri: &str, title: &str, notes: Option<&str>) -> anyhow::Result<()> {
    let list = uri
        .strip_prefix("cider://reminders/list/")
        .ok_or_else(|| anyhow!("Not a Reminders source: {uri}"))?;
    let notes = notes.map(str::trim).filter(|n| !n.is_empty());
    cider(cider_lib::reminders::create(
        title,
        Some(list),
        None,
        None,
        notes,
    ))
    .await
    .map(|_| ())
}

/// Check off a reminder in the list this source mirrors.
///
/// Always by id, never by title: Reminders lets two items share a name, and a
/// by-title completion picks one of them silently. The id is the one cider
/// prints in `reminders list` and that `fetch` renders into the source text.
/// The id is what cider prints in `reminders list` and what `fetch` renders
/// into the source text.
pub async fn complete_reminder(uri: &str, id: &str) -> anyhow::Result<()> {
    let list = uri
        .strip_prefix("cider://reminders/list/")
        .ok_or_else(|| anyhow!("Not a Reminders source: {uri}"))?;
    if id.trim().is_empty() {
        anyhow::bail!("no reminder id — pick one from the list");
    }
    cider(cider_lib::reminders::complete(
        cider_lib::reminders::Target::Id(id.trim()),
        Some(list),
    ))
    .await
    .map(|_| ())
}

/// The resync sweep runs every minute, but Mac fetches go through osascript
/// and permission-guarded databases — re-checking every 15 minutes is plenty
/// for calendars and reminders. Manual refresh bypasses this.
const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// source_id -> last sweep fetch, in-memory (a fresh app run re-checks once).
static LAST_SWEEP: std::sync::Mutex<Option<std::collections::HashMap<String, std::time::Instant>>> =
    std::sync::Mutex::new(None);

/// Should the sweep re-fetch this Mac source now? Stamps the check time.
pub fn sweep_due(source_id: &str) -> bool {
    let mut guard = LAST_SWEEP.lock().unwrap();
    let map = guard.get_or_insert_with(Default::default);
    let now = std::time::Instant::now();
    match map.get(source_id) {
        Some(t) if now.duration_since(*t) < SWEEP_INTERVAL => false,
        _ => {
            map.insert(source_id.to_string(), now);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The exact stderr shapes seen in the wild (prod app without Full Disk
    // Access) — the user must never see raw JSON or tracebacks.
    #[test]
    fn tcc_denials_become_one_instruction() {
        // The library's anyhow chains, formatted `{e:#}` — the messages the
        // old CLI wrapped in a JSON envelope now arrive bare.
        for raw in [
            "sqlite3 failed: Error: unable to open database \
             \"/Users/x/…/Calendar.sqlitedb\": authorization denied",
            "ls failed: ls: /Users/x/…/Stores: Operation not permitted",
            "python3 failed: PermissionError: [Errno 1] Operation not permitted",
        ] {
            let msg = friendly_cider_error(raw);
            assert!(msg.contains("Full Disk Access"), "got: {msg}");
        }
    }

    #[test]
    fn other_errors_keep_first_line_only() {
        let msg = friendly_cider_error("osascript failed: some error\nline two\nline three");
        assert_eq!(msg, "osascript failed: some error");
    }

    // Agents hand add_source raw cider:// strings (vs the UI's picker), so
    // the constructors and the recognizer must agree on every provider.
    #[test]
    fn mac_uris_round_trip_through_recognizer() {
        for (provider, collection) in [
            ("reminders", "Alchemy"),
            ("calendar", "30"),
            ("stocks", "My Symbols"),
            ("notes", "x-coredata://ABC/ICNote/p1"),
        ] {
            assert!(is_mac_uri(&mac_uri(provider, collection)));
        }
        assert!(!is_mac_uri("https://example.com"));
    }

    // A malformed origin must fail before any cider subprocess runs, so this
    // is safe (and meaningful) without cider installed.
    #[tokio::test]
    async fn fetch_rejects_unrecognized_origin() {
        let err = fetch("cider://bogus/thing").await.unwrap_err();
        assert!(err.to_string().contains("Unrecognized Mac source origin"));
    }

    // The live-card contract (src/lib/liveCards.ts parses these): one item
    // per line, fixed field order. A drift here is a card silently gone.
    #[test]
    fn calendar_renders_one_fixed_line_per_event() {
        let data = serde_json::json!([
            {"title": "Inspection", "calendar": "Home", "location": "12 Elm St",
             "start_date": "2026-09-03T14:00:00", "is_all_day": false,
             "notes": "bring the\nreport"},
            {"title": "Labor Day", "calendar": "US Holidays",
             "start_date": "2026-09-07T00:00:00", "is_all_day": true},
        ]);
        assert_eq!(
            render_calendar("7", &data),
            "# Calendar — next 7 days\n\n\
             ## 2026-09-03\n\
             - 14:00 — Inspection (Home) at 12 Elm St\n\
             \x20 - bring the report\n\
             ## 2026-09-07\n\
             - all day — Labor Day (US Holidays)\n"
        );
    }

    #[test]
    fn reminders_render_title_id_and_due() {
        let data = serde_json::json!([
            {"id": "x-apple-reminder://A1", "title": "Call the insurer",
             "list": "Home", "priority": 0, "due_date": "2026-09-04T16:00:00Z",
             "notes": "policy 4471"},
            {"id": "x-apple-reminder://B2", "title": "Buy stamps", "list": "Home",
             "priority": 0},
        ]);
        assert_eq!(
            render_reminders("Home", &data),
            "# Reminders — Home\n\n\
             - [ ] Call the insurer `x-apple-reminder://A1` — due 2026-09-04\n\
             \x20 - policy 4471\n\
             - [ ] Buy stamps `x-apple-reminder://B2`\n"
        );
    }

    #[test]
    fn stocks_render_a_five_column_table_and_as_of() {
        let symbols = vec!["AAPL".to_string(), "ZZZZ".to_string()];
        let quotes = serde_json::json!([
            {"symbol": "AAPL", "name": "Apple Inc.", "price": 231.456,
             "change_percent": -1.2345, "currency": "USD",
             "exchange_status": "closed", "as_of": "2026-09-02T20:00:00Z"},
        ]);
        assert_eq!(
            render_stocks("My Symbols", &symbols, &quotes),
            "# Stocks — My Symbols\n\n\
             | Symbol | Name | Price | Change | Status |\n\
             |---|---|---|---|---|\n\
             | AAPL | Apple Inc. | 231.46 USD | -1.23% | closed |\n\
             | ZZZZ |  |  |  |  |\n\n\
             _Prices as of 2026-09-02T20:00:00Z (Apple Stocks cache)._\n"
        );
    }

    // ---- Item events (docs/RFC-events.md phase 5) ----

    const REMINDERS_OLD: &str = "# Reminders — Home\n\n\
        - [ ] Call the insurer `x-apple-reminder://A1` — due 2026-09-04\n\
        \x20 - policy 4471\n\
        - [ ] Buy stamps `x-apple-reminder://B2`\n\
        - [ ] Fix the gate `x-apple-reminder://C3` — due 2026-09-10\n";

    #[test]
    fn completing_a_reminder_is_one_completed_event() {
        let new = REMINDERS_OLD.replace(
            "- [ ] Call the insurer `x-apple-reminder://A1` — due 2026-09-04\n\
             \x20 - policy 4471\n",
            "",
        );
        assert_eq!(
            item_events("cider://reminders/list/Home", REMINDERS_OLD, &new),
            vec![("completed", "✓ Call the insurer".to_string())]
        );
    }

    #[test]
    fn reminder_arrivals_moves_and_renames_are_named() {
        let new = "# Reminders — Home\n\n\
            - [ ] Call the insurer `x-apple-reminder://A1` — due 2026-09-06\n\
            - [ ] Buy more stamps `x-apple-reminder://B2`\n\
            - [ ] Fix the gate `x-apple-reminder://C3`\n\
            - [ ] Renew the permit `x-apple-reminder://D4` — due 2026-09-30\n";
        assert_eq!(
            item_events("cider://reminders/list/Home", REMINDERS_OLD, new),
            vec![
                (
                    "moved",
                    "Call the insurer · due 2026-09-04 → 2026-09-06".to_string()
                ),
                (
                    "updated",
                    "renamed · Buy stamps → Buy more stamps".to_string()
                ),
                (
                    "moved",
                    "Fix the gate · due 2026-09-10 → no date".to_string()
                ),
                ("added", "new reminder · Renew the permit".to_string()),
            ]
        );
    }

    #[test]
    fn reminder_note_edits_leave_the_generic_diff_to_speak() {
        let new = REMINDERS_OLD.replace("policy 4471", "policy 4471, ask for Dana");
        assert!(item_events("cider://reminders/list/Home", REMINDERS_OLD, &new).is_empty());
        // Same text, same ids: nothing to say.
        assert!(
            item_events("cider://reminders/list/Home", REMINDERS_OLD, REMINDERS_OLD).is_empty()
        );
    }

    const CALENDAR_OLD: &str = "# Calendar — next 7 days\n\n\
        ## 2026-09-02\n\
        - 09:00 — Standup (Work)\n\
        - 14:00 — Inspection (Home) at 12 Elm St\n\
        \x20 - bring the report\n\
        ## 2026-09-03\n\
        - 09:00 — Standup (Work)\n\
        ## 2026-09-04\n\
        - 09:00 — Standup (Work)\n\
        - all day — Offsite (Work)\n";

    #[test]
    fn a_rescheduled_event_is_one_moved_event() {
        let new = CALENDAR_OLD.replace(
            "- 14:00 — Inspection (Home) at 12 Elm St\n\x20 - bring the report\n## 2026-09-03\n",
            "## 2026-09-03\n- 10:00 — Inspection (Home) at 12 Elm St\n\x20 - bring the report\n",
        );
        assert_eq!(
            calendar_events(CALENDAR_OLD, &new, "2026-09-02"),
            vec![(
                "moved",
                "Inspection · Wed Sep 2 2 PM → Thu Sep 3 10 AM".to_string()
            )]
        );
        // Same day, later: weekday only.
        let later = CALENDAR_OLD.replace("- 14:00 — Inspection", "- 15:30 — Inspection");
        assert_eq!(
            calendar_events(CALENDAR_OLD, &later, "2026-09-02"),
            vec![("moved", "Inspection · Wed 2 PM → Wed 3:30 PM".to_string())]
        );
    }

    #[test]
    fn calendar_arrivals_and_departures_inside_the_window() {
        let new = CALENDAR_OLD
            .replace("- all day — Offsite (Work)\n", "")
            .replace(
                "## 2026-09-03\n",
                "## 2026-09-03\n- 12:00 — Lunch with Sam (Personal)\n",
            );
        assert_eq!(
            calendar_events(CALENDAR_OLD, &new, "2026-09-02"),
            vec![
                (
                    "added",
                    "new event · Lunch with Sam · Thu Sep 3 12 PM".to_string()
                ),
                (
                    "removed",
                    "event gone · Offsite · Fri Sep 4 all day".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_sliding_window_and_a_venue_change_are_not_events() {
        // Two days on: the front two days fell off, two new days rolled in,
        // and the daily standup is still the daily standup.
        let slid = "# Calendar — next 7 days\n\n\
            ## 2026-09-04\n\
            - 09:00 — Standup (Work)\n\
            - all day — Offsite (Work)\n\
            ## 2026-09-05\n\
            - 09:00 — Standup (Work)\n\
            ## 2026-09-06\n\
            - 09:00 — Standup (Work)\n";
        assert!(calendar_events(CALENDAR_OLD, slid, "2026-09-04").is_empty());
        let moved_venue = CALENDAR_OLD.replace("at 12 Elm St", "at 14 Elm St");
        assert!(calendar_events(CALENDAR_OLD, &moved_venue, "2026-09-02").is_empty());
        // An empty old rendering (a calendar with nothing in range) makes
        // every new line an arrival.
        assert_eq!(
            calendar_events(
                "# Calendar — next 7 days\n\n",
                "## 2026-09-03\n- 09:00 — Dentist (Personal)\n",
                "2026-09-02"
            ),
            vec![("added", "new event · Dentist · Thu Sep 3 9 AM".to_string())]
        );
    }

    #[test]
    fn notes_and_stocks_have_no_item_events() {
        assert!(item_events(
            "cider://notes/note/x-coredata://A/ICNote/p1",
            "# A\n\nold",
            "# A\n\nnew"
        )
        .is_empty());
        assert!(item_events(
            "cider://stocks/watchlist/My Symbols",
            "| AAPL | 1 |",
            "| AAPL | 2 |"
        )
        .is_empty());
    }

    #[test]
    fn clock_labels_read_like_a_person_wrote_them() {
        assert_eq!(when_label("2026-09-03", "00:00", false), "Thu 12 AM");
        assert_eq!(when_label("2026-09-03", "12:00", false), "Thu 12 PM");
        assert_eq!(
            when_label("2026-09-03", "23:45", true),
            "Thu Sep 3 11:45 PM"
        );
        assert_eq!(when_label("2026-09-03", "all day", false), "Thu all day");
        assert_eq!(when_label("someday", "soon", false), "someday soon");
    }
}
