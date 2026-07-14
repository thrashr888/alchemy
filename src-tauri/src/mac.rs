//! Mac items as sources, via the cider CLI (https://github.com/thrashr888/cider).
//!
//! A `cider://` origin in a source's `url` names a living Mac item — a
//! Reminders list, a rolling Calendar window, or an Apple Notes folder. The
//! content is fetched over cider's JSON stdout, rendered to markdown, and
//! ingested through the normal chunk/embed path; the resync sweep re-fetches
//! on a gentle cadence and re-embeds when the content hash changes (the hash
//! rides in the source's `mtime` column). Sync is the only way data flows
//! in; the narrow write paths (edit a note, add a reminder) go to the Mac
//! app first and re-sync back — see docs/RFC-cider-tools.md.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use anyhow::{anyhow, Context};
use serde::Serialize;

/// One pickable item in the add-source modal's provider step.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacCollection {
    pub id: String,
    pub label: String,
    pub detail: String,
}

/// GUI-spawned apps get a minimal PATH without Homebrew, so check the known
/// install locations before falling back to PATH.
fn cider_path() -> Option<PathBuf> {
    for p in ["/opt/homebrew/bin/cider", "/usr/local/bin/cider"] {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    which_cider()
}

fn which_cider() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("cider"))
        .find(|c| c.exists())
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
    // Keep the first line — subprocess tracebacks aren't toast material.
    raw.lines()
        .next()
        .unwrap_or("cider call failed")
        .chars()
        .take(200)
        .collect()
}

/// Pull the message out of cider's `{"ok": false, "error": …}` envelope, or
/// fall back to the raw text.
fn envelope_message(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text.trim())
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| text.trim().to_string())
}

/// Run cider and unwrap its `{"ok": bool, "data"/"error": …}` envelope.
/// cider has internal JXA/osascript timeouts (15–30s); this outer one only
/// catches a wedged process.
async fn cider(args: &[&str]) -> anyhow::Result<serde_json::Value> {
    let bin = cider_path().ok_or_else(|| anyhow!("cider is not installed (brew install cider)"))?;
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        tokio::process::Command::new(bin).args(args).output(),
    )
    .await
    .map_err(|_| anyhow!("cider timed out"))?
    .context("failed to run cider")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        // cider puts data on stdout and errors (including its JSON error
        // envelope) on stderr — parse the envelope rather than echoing it.
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.trim().is_empty() {
            anyhow::bail!("cider produced no output");
        }
        anyhow::bail!("{}", friendly_cider_error(&envelope_message(&stderr)));
    }
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).with_context(|| {
        format!(
            "unexpected cider output: {}",
            stdout.chars().take(200).collect::<String>()
        )
    })?;
    // Success is BARE JSON (an array or object); only failures wear the
    // {"ok": false, "error": …} envelope.
    if v["ok"].as_bool() == Some(false) {
        let msg = v["error"]["message"]
            .as_str()
            .or_else(|| v["error"].as_str())
            .unwrap_or("cider call failed");
        return Err(anyhow!("{}", friendly_cider_error(msg)));
    }
    if v["ok"].as_bool() == Some(true) {
        return Ok(v["data"].clone());
    }
    Ok(v)
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
    cider_path().is_some()
}

/// Settings/onboarding "Connect" buttons: one benign read per provider so the
/// macOS consent prompt fires at a predictable moment instead of mid-add.
#[tauri::command]
pub async fn mac_connect(provider: String) -> Result<(), String> {
    match provider.as_str() {
        "reminders" => cider(&["reminders", "list", "--limit", "1"]).await,
        "calendar" => cider(&["calendar", "list", "--days-back", "0", "--days-ahead", "1"]).await,
        "notes" => cider(&["notes", "folders"]).await,
        "stocks" => cider(&["stocks", "watchlists"]).await,
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
            let data = cider(&["reminders", "list"])
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
        // full text via `notes get`, past the list command's 2000-char cap).
        // --brief (cider >= 0.1.8) skips bodies and returns the whole
        // library fast; the picker searches and groups it by folder
        // client-side.
        "notes" => {
            let data = cider(&["notes", "list", "--brief"])
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
            let data = cider(&["stocks", "watchlists"])
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
        let data = cider(&["calendar", "list", "--days-back", "0", "--days-ahead", days]).await?;
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
        return Ok((format!("Calendar: next {days} days"), out));
    }
    if let Some(list) = uri.strip_prefix("cider://reminders/list/") {
        let data = cider(&["reminders", "list", "--list", list]).await?;
        let mut out = format!("# Reminders — {list}\n\n");
        for r in data.as_array().unwrap_or(&vec![]) {
            out.push_str(&format!(
                "- [ ] {}",
                r["title"].as_str().unwrap_or("Untitled")
            ));
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
        return Ok((format!("Reminders: {list}"), out));
    }
    if let Some(id) = uri.strip_prefix("cider://notes/note/") {
        let n = cider(&["notes", "get", "--id", id]).await?;
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
        let lists = cider(&["stocks", "watchlists"]).await?;
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
        let quotes = cider(&["stocks", "list"]).await?;
        let mut out = format!("# Stocks — {list}\n\n");
        let mut as_of = "";
        let empty = vec![];
        let rows = quotes.as_array().unwrap_or(&empty);
        out.push_str("| Symbol | Name | Price | Change | Status |\n");
        out.push_str("|---|---|---|---|---|\n");
        for sym in &symbols {
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
        let as_of = as_of.to_string();
        if !as_of.is_empty() {
            out.push_str(&format!("\n_Prices as of {as_of} (Apple Stocks cache)._\n"));
        }
        return Ok((format!("Stocks: {list}"), out));
    }
    // Legacy folder-as-source origins keep syncing.
    if let Some(folder) = uri.strip_prefix("cider://notes/folder/") {
        let data = cider(&["notes", "list", "--folder", folder]).await?;
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

/// Is this origin a Mac item?
pub fn is_mac_uri(url: &str) -> bool {
    url.starts_with("cider://")
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
    let n = cider(&["notes", "get", "--id", id]).await?;
    Ok(n["body"].as_str().unwrap_or_default().to_string())
}

/// Replace the note's body (cider renders line breaks to Notes' HTML).
pub async fn update_note(uri: &str, body: &str) -> anyhow::Result<()> {
    let id = uri
        .strip_prefix("cider://notes/note/")
        .ok_or_else(|| anyhow!("Not an Apple Notes source: {uri}"))?;
    cider(&["notes", "update", "--id", id, "--body", body])
        .await
        .map(|_| ())
}

/// Add a reminder to the list this source mirrors.
pub async fn add_reminder(uri: &str, title: &str, notes: Option<&str>) -> anyhow::Result<()> {
    let list = uri
        .strip_prefix("cider://reminders/list/")
        .ok_or_else(|| anyhow!("Not a Reminders source: {uri}"))?;
    let mut args = vec!["reminders", "create", "--title", title, "--list", list];
    if let Some(n) = notes {
        if !n.trim().is_empty() {
            args.push("--notes");
            args.push(n);
        }
    }
    cider(&args).await.map(|_| ())
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
        for raw in [
            r#"{"error":{"code":"operation_failed","message":"sqlite3 failed: Error: unable to open database \"/Users/x/Library/Group Containers/group.com.apple.calendar/Calendar.sqlitedb\": authorization denied\n"},"ok":false}"#,
            r#"{"error":{"code":"operation_failed","message":"ls failed: ls: /Users/x/Library/Group Containers/group.com.apple.reminders/Container_v1/Stores: Operation not permitted\n"},"ok":false}"#,
            r#"{"error":{"code":"permission_denied","message":"python3 failed: Traceback (most recent call last):\n  File \"<string>\", line 24, in <module>\nPermissionError: [Errno 1] Operation not permitted: '/Users/x/…'"},"ok":false}"#,
        ] {
            let msg = friendly_cider_error(&envelope_message(raw));
            assert!(msg.contains("Full Disk Access"), "got: {msg}");
            assert!(!msg.contains('{'), "raw JSON leaked: {msg}");
        }
    }

    #[test]
    fn other_errors_keep_first_line_only() {
        let msg = friendly_cider_error("osascript failed: some error\nline two\nline three");
        assert_eq!(msg, "osascript failed: some error");
    }

    #[test]
    fn plain_stderr_passes_through() {
        assert_eq!(envelope_message("not json at all"), "not json at all");
    }
}
