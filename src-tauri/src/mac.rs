//! Mac items as sources, via the cider CLI (https://github.com/thrashr888/cider).
//!
//! A `cider://` origin in a source's `url` names a living Mac item — a
//! Reminders list, a rolling Calendar window, or an Apple Notes folder. The
//! content is fetched over cider's JSON stdout, rendered to markdown, and
//! ingested through the normal chunk/embed path; the resync sweep re-fetches
//! on a gentle cadence and re-embeds when the content hash changes (the hash
//! rides in the source's `mtime` column). Read-only by design — see
//! docs/RFC-cider-tools.md.

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
        // Errors that never reach the JSON envelope land on stderr (e.g. a
        // permission-denied reading another app's container).
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = stderr.trim().chars().take(300).collect::<String>();
        anyhow::bail!(
            "cider produced no output{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
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
        // The first call to a data class pops a macOS permission prompt; a
        // timeout here usually means it's waiting on (or was denied) consent.
        return Err(anyhow!(
            "{msg} — if macOS asked for permission, allow it and retry"
        ));
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
        "notes" => {
            let data = cider(&["notes", "list"])
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
        other => Err(format!("Unknown Mac provider: {other}")),
    }
}

/// Build the origin URI stored in the source's `url`. Collection names keep
/// their raw spelling — the field is an opaque origin string, not a real URL.
pub fn mac_uri(provider: &str, collection: &str) -> String {
    match provider {
        "calendar" => format!("cider://calendar/upcoming/{collection}"),
        "reminders" => format!("cider://reminders/list/{collection}"),
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
