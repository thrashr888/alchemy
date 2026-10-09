//! The Inbox: save now, organize later.
//!
//! An external arrival (browser clipper, Services, an `alchemy://add` link,
//! the menu bar) used to raise a blocking "Add to which notebook?" question.
//! Now it lands here instantly. The router's pick (the typed judge's, when
//! one is configured: docs/RFC-typesafe-jev.md "Notebook suggestion, typed")
//! is computed behind it and stored on the row, and the user files it later
//! with one key. Nothing is imported until then, so dismissing a row leaves
//! no trace, there is no source to move, and OKF never hears of it.
//!
//! Accepting runs the same add path a notebook's own Add source runs
//! (`add_source_file`, `ingest_url`, `store_extracted`); this module only
//! decides where the row goes.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use tauri::{AppHandle, Manager, State};

use crate::commands::{self, AppState};
use crate::models::{InboxAccepted, InboxAlternative, InboxItem};
use crate::router::NotebookSuggestion;

/// Characters of content kept on the row.
pub const EXCERPT_CHARS: usize = 600;
/// Other notebooks stored beside the suggestion (keys 1-4 in the section).
pub const MAX_ALTERNATIVES: usize = 4;
/// Longest a suggestion may take, fetch included. A row that overruns it is
/// still shown, with no suggestion; the user can choose a notebook by hand.
const SUGGEST_BUDGET: Duration = Duration::from_secs(60);

/// Where an accepted row goes.
#[derive(Debug, Clone, PartialEq)]
pub enum AcceptTarget {
    Existing(String),
    New(String),
}

/// The accept rule. A notebook the caller names wins; failing that a title
/// the caller gives starts a new notebook; failing that, the row's own
/// suggestion. A new notebook is only ever made on one of those three words
/// from a person (or an agent acting for one), never from a bare accept of an
/// existing-notebook suggestion.
pub fn accept_target(
    item: &InboxItem,
    notebook_id: Option<&str>,
    new_title: Option<&str>,
) -> Result<AcceptTarget> {
    fn given(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    if let Some(id) = given(notebook_id) {
        return Ok(AcceptTarget::Existing(id.to_string()));
    }
    if let Some(title) = given(new_title) {
        return Ok(AcceptTarget::New(title.to_string()));
    }
    if item.is_new {
        let title = if item.suggested_title.trim().is_empty() {
            item.title.trim()
        } else {
            item.suggested_title.trim()
        };
        if title.is_empty() {
            bail!("Name the new notebook first");
        }
        return Ok(AcceptTarget::New(title.to_string()));
    }
    if !item.suggested_notebook_id.is_empty() {
        return Ok(AcceptTarget::Existing(item.suggested_notebook_id.clone()));
    }
    bail!("There is no suggestion for this capture yet; choose a notebook")
}

/// The other notebooks the judge ranked: everything but the pick, best
/// first, at most `MAX_ALTERNATIVES`.
pub fn alternatives_of(s: &NotebookSuggestion) -> Vec<InboxAlternative> {
    s.ranked
        .iter()
        .filter(|r| r.notebook_id != s.notebook_id)
        .take(MAX_ALTERNATIVES)
        .map(|r| InboxAlternative {
            notebook_id: r.notebook_id.clone(),
            title: r.title.clone(),
            probability: r.probability,
        })
        .collect()
}

/// Copy a suggestion onto its row.
pub fn apply_suggestion(item: &mut InboxItem, s: &NotebookSuggestion) {
    item.suggested_notebook_id = s.notebook_id.clone();
    item.suggested_title = s.title.clone();
    item.is_new = s.is_new;
    item.probability = s.confidence;
    item.auto = s.auto;
    item.alternatives = alternatives_of(s);
    item.judge = s.judge.clone().unwrap_or_default();
}

/// What the row shows of the content.
pub fn excerpt_of(text: &str) -> String {
    crate::router::clip(text, EXCERPT_CHARS)
}

fn base_name(path: &str) -> String {
    path.rsplit('/')
        .find(|p| !p.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn changed() {
    commands::notify_changed("inbox", None);
}

/// Save a capture. Returns at once; the suggestion arrives behind it.
pub async fn add(
    app: &AppHandle,
    url: &str,
    text: &str,
    title: &str,
    files: Vec<String>,
) -> Result<InboxItem> {
    let state = app.state::<AppState>();
    let (url, text, title) = (url.trim(), text.trim(), title.trim());
    if url.is_empty() && text.is_empty() && files.is_empty() {
        bail!("Nothing to save");
    }
    // The same link twice is one arrival, not two.
    if files.is_empty() && text.is_empty() {
        let wanted = crate::ingest::normalize_url(url);
        let wanted = wanted.trim_end_matches('/');
        for existing in state.db.list_inbox().await? {
            if existing.files.is_empty()
                && existing.text.is_empty()
                && crate::ingest::normalize_url(&existing.url).trim_end_matches('/') == wanted
            {
                return Ok(existing);
            }
        }
    }
    let item = InboxItem {
        id: commands::new_id(),
        created_at: commands::now(),
        url: url.to_string(),
        text: text.to_string(),
        title: title.to_string(),
        files,
        excerpt: excerpt_of(text),
        suggested_notebook_id: String::new(),
        suggested_title: String::new(),
        is_new: false,
        probability: None,
        auto: false,
        alternatives: Vec::new(),
        judge: String::new(),
        suggested: false,
    };
    state.db.add_inbox_item(&item).await?;
    changed();
    let handle = app.clone();
    let id = item.id.clone();
    tauri::async_runtime::spawn(async move {
        fill(&handle, &id).await;
    });
    Ok(item)
}

/// Compute and store one row's suggestion. Always ends with the row marked
/// suggested, so "Choosing..." resolves; a failure leaves it with none.
pub async fn fill(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    let item = match state.db.get_inbox_item(id).await {
        Ok(Some(item)) if !item.suggested => item,
        Ok(_) => return,
        Err(err) => {
            crate::diagnostics::error("inbox", format!("could not read {id}: {err:#}"));
            return;
        }
    };
    let filled = tokio::time::timeout(SUGGEST_BUDGET, compute(&state, &item))
        .await
        .unwrap_or_else(|_| {
            crate::note!("inbox: no suggestion for {id} within {SUGGEST_BUDGET:?}");
            let mut gave_up = item.clone();
            gave_up.suggested = true;
            gave_up
        });
    if let Err(err) = state.db.update_inbox_item(&filled).await {
        crate::diagnostics::error("inbox", format!("could not store a suggestion: {err:#}"));
        return;
    }
    changed();
}

async fn compute(state: &AppState, item: &InboxItem) -> InboxItem {
    let mut out = item.clone();
    let (title, text, location) = if let Some(path) = item.files.first() {
        // A file's name is the signal on hand until it is imported.
        let name = if item.title.is_empty() {
            base_name(path)
        } else {
            item.title.clone()
        };
        (name, String::new(), path.clone())
    } else {
        let (title, text) =
            commands::resolve_incoming(item.title.clone(), item.text.clone(), item.url.clone())
                .await;
        (title, text, item.url.clone())
    };
    // A failed fetch routes on the URL string itself; that is not a title or
    // an excerpt worth keeping.
    if !item.url.is_empty() && item.files.is_empty() {
        if out.title.is_empty() && title != item.url {
            out.title = title.clone();
        }
        if out.excerpt.is_empty() && text != item.url {
            out.excerpt = excerpt_of(&text);
        }
    }
    let ai = state.ai.read().await.clone();
    match crate::router::suggest_notebook(&state.db, &ai, &title, &text, &location).await {
        Ok(s) => apply_suggestion(&mut out, &s),
        Err(err) => crate::diagnostics::error(
            "inbox",
            format!("could not suggest a notebook for {}: {err:#}", item.id),
        ),
    }
    out.suggested = true;
    out
}

/// Suggest for every row that has none: rows saved when no model answered,
/// or still waiting when the app quit.
pub async fn fill_missing(app: &AppHandle) {
    let state = app.state::<AppState>();
    let pending: Vec<String> = match state.db.list_inbox().await {
        Ok(items) => items
            .into_iter()
            .filter(|i| !i.suggested)
            .map(|i| i.id)
            .collect(),
        Err(err) => {
            crate::diagnostics::error("inbox", format!("could not list the inbox: {err:#}"));
            return;
        }
    };
    for id in pending {
        fill(app, &id).await;
    }
}

/// File a row. `notebook_id` and `new_title` override the suggestion (see
/// `accept_target`). On any failure the row stays where it is.
pub async fn accept(
    app: &AppHandle,
    id: &str,
    notebook_id: Option<&str>,
    new_title: Option<&str>,
) -> Result<InboxAccepted> {
    // One filing per row at a time. The row is deleted only after the add
    // lands, so two accepts that overlap (a doubled Enter, an agent and the
    // keyboard at once) would both import it; the second now waits out as
    // an error instead.
    static IN_FLIGHT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::LazyLock::new(Default::default);
    let claimed = IN_FLIGHT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.to_string());
    if !claimed {
        bail!("That capture is already being filed");
    }
    let result = accept_claimed(app, id, notebook_id, new_title).await;
    IN_FLIGHT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
    result
}

async fn accept_claimed(
    app: &AppHandle,
    id: &str,
    notebook_id: Option<&str>,
    new_title: Option<&str>,
) -> Result<InboxAccepted> {
    let state = app.state::<AppState>();
    let item = state
        .db
        .get_inbox_item(id)
        .await?
        .ok_or_else(|| anyhow!("That capture is no longer in the Inbox"))?;
    let (nb_id, nb_title) = match accept_target(&item, notebook_id, new_title)? {
        AcceptTarget::Existing(nb_id) => {
            let nb = state
                .db
                .list_notebooks()
                .await?
                .into_iter()
                .find(|n| n.id == nb_id)
                .ok_or_else(|| anyhow!("That notebook no longer exists"))?;
            (nb.id, nb.title)
        }
        AcceptTarget::New(title) => {
            let nb = commands::new_notebook(&state, title)
                .await
                .map_err(|e| anyhow!(e))?;
            (nb.id, nb.title)
        }
    };
    file_item(app, &nb_id, &item).await?;
    state.db.delete_inbox_item(id).await?;
    commands::notify_changed("notebooks", Some(&nb_id));
    commands::notify_changed("sources", Some(&nb_id));
    changed();
    Ok(InboxAccepted {
        notebook_id: nb_id,
        title: nb_title,
    })
}

/// The add path itself: files, then a URL, then pasted text, the order the
/// picker's `confirmExternalAdd` uses.
async fn file_item(app: &AppHandle, notebook_id: &str, item: &InboxItem) -> Result<()> {
    let state = app.state::<AppState>();
    if !item.files.is_empty() {
        let mut failed = Vec::new();
        for path in &item.files {
            let state: State<'_, AppState> = app.state();
            if let Err(err) =
                commands::add_source_file(app.clone(), state, notebook_id.to_string(), path.clone())
                    .await
            {
                failed.push(format!("{}: {err}", base_name(path)));
            }
        }
        if !failed.is_empty() {
            bail!("Could not add {}", failed.join("; "));
        }
    } else if !item.url.is_empty() {
        if crate::mac::is_mac_uri(&item.url) {
            commands::ingest_mac(&state, notebook_id, &item.url, "").await?;
        } else {
            commands::ingest_url(&state, notebook_id, &item.url, None).await?;
        }
    } else if !item.text.is_empty() {
        let extracted = crate::ingest::extract_pasted(&item.title, &item.text)?;
        commands::store_extracted(&state, notebook_id, extracted).await?;
    }
    Ok(())
}

pub async fn dismiss(app: &AppHandle, id: &str) -> Result<()> {
    app.state::<AppState>().db.delete_inbox_item(id).await?;
    changed();
    Ok(())
}

// ---- IPC ---------------------------------------------------------------------

#[tauri::command]
pub async fn inbox_list(state: State<'_, AppState>) -> Result<Vec<InboxItem>, String> {
    state.db.list_inbox().await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn inbox_add(
    app: AppHandle,
    url: String,
    text: String,
    title: String,
    files: Vec<String>,
) -> Result<InboxItem, String> {
    add(&app, &url, &text, &title, files)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn inbox_accept(
    app: AppHandle,
    id: String,
    notebook_id: Option<String>,
    new_title: Option<String>,
) -> Result<InboxAccepted, String> {
    accept(&app, &id, notebook_id.as_deref(), new_title.as_deref())
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn inbox_dismiss(app: AppHandle, id: String) -> Result<(), String> {
    dismiss(&app, &id).await.map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::RankedNotebook;

    fn item() -> InboxItem {
        InboxItem {
            id: "i".into(),
            created_at: 1,
            url: String::new(),
            text: String::new(),
            title: "A page".into(),
            files: vec![],
            excerpt: String::new(),
            suggested_notebook_id: "nb-suggested".into(),
            suggested_title: "Suggested".into(),
            is_new: false,
            probability: Some(0.9),
            auto: true,
            alternatives: vec![],
            judge: "j".into(),
            suggested: true,
        }
    }

    fn ranked(id: &str, p: f64) -> RankedNotebook {
        RankedNotebook {
            notebook_id: id.into(),
            title: format!("Title {id}"),
            probability: p,
        }
    }

    #[test]
    fn a_bare_accept_files_into_the_suggestion() {
        assert_eq!(
            accept_target(&item(), None, None).unwrap(),
            AcceptTarget::Existing("nb-suggested".into())
        );
        // Blank strings from a form field count as nothing.
        assert_eq!(
            accept_target(&item(), Some("  "), Some("")).unwrap(),
            AcceptTarget::Existing("nb-suggested".into())
        );
    }

    #[test]
    fn a_given_notebook_beats_the_suggestion() {
        assert_eq!(
            accept_target(&item(), Some("other"), None).unwrap(),
            AcceptTarget::Existing("other".into())
        );
        // Even over a title: choosing a notebook is more specific than naming one.
        assert_eq!(
            accept_target(&item(), Some("other"), Some("Fresh")).unwrap(),
            AcceptTarget::Existing("other".into())
        );
    }

    #[test]
    fn a_new_notebook_needs_the_suggestion_to_be_new_or_a_title_given() {
        let mut new = item();
        new.is_new = true;
        new.suggested_notebook_id.clear();
        new.suggested_title = "Fresh idea".into();
        assert_eq!(
            accept_target(&new, None, None).unwrap(),
            AcceptTarget::New("Fresh idea".into())
        );
        // A given title wins over the suggested one, and works on an
        // existing-notebook suggestion too (the N key).
        assert_eq!(
            accept_target(&new, None, Some(" Mine ")).unwrap(),
            AcceptTarget::New("Mine".into())
        );
        assert_eq!(
            accept_target(&item(), None, Some("Mine")).unwrap(),
            AcceptTarget::New("Mine".into())
        );
        // No name anywhere is an error, not an "Untitled notebook".
        new.suggested_title.clear();
        new.title.clear();
        assert!(accept_target(&new, None, None).is_err());
        // A new suggestion with no name falls back to the capture's title.
        new.title = "A page".into();
        assert_eq!(
            accept_target(&new, None, None).unwrap(),
            AcceptTarget::New("A page".into())
        );
    }

    #[test]
    fn nothing_suggested_and_nothing_given_is_an_error() {
        let mut bare = item();
        bare.suggested_notebook_id.clear();
        bare.suggested = false;
        assert!(accept_target(&bare, None, None).is_err());
        assert_eq!(
            accept_target(&bare, Some("nb"), None).unwrap(),
            AcceptTarget::Existing("nb".into())
        );
    }

    #[test]
    fn alternatives_leave_out_the_pick_and_cap_at_four() {
        let mut s = NotebookSuggestion {
            notebook_id: "a".into(),
            title: "A".into(),
            is_new: false,
            confidence: Some(0.8),
            judge: Some("j".into()),
            auto: true,
            ranked: vec![
                ranked("a", 0.7),
                ranked("b", 0.1),
                ranked("c", 0.08),
                ranked("d", 0.05),
                ranked("e", 0.04),
                ranked("f", 0.03),
            ],
        };
        let alts = alternatives_of(&s);
        assert_eq!(
            alts.iter()
                .map(|a| a.notebook_id.as_str())
                .collect::<Vec<_>>(),
            ["b", "c", "d", "e"]
        );
        assert_eq!(alts[0].probability, 0.1);
        // A new-notebook suggestion has no pick to leave out.
        s.notebook_id.clear();
        s.is_new = true;
        assert_eq!(alternatives_of(&s).len(), MAX_ALTERNATIVES);
        assert_eq!(alternatives_of(&s)[0].notebook_id, "a");
    }

    #[test]
    fn applying_a_suggestion_without_a_judge_stores_no_probability() {
        let mut row = item();
        let plain = NotebookSuggestion {
            notebook_id: "nb".into(),
            title: "Router pick".into(),
            is_new: false,
            confidence: None,
            judge: None,
            auto: false,
            ranked: vec![],
        };
        apply_suggestion(&mut row, &plain);
        assert_eq!(row.suggested_notebook_id, "nb");
        assert_eq!(row.probability, None);
        assert!(!row.auto && row.alternatives.is_empty() && row.judge.is_empty());
    }

    #[test]
    fn the_excerpt_is_capped() {
        let long = "word ".repeat(1000);
        assert!(excerpt_of(&long).chars().count() <= EXCERPT_CHARS);
        assert_eq!(excerpt_of("  a\n\n b "), "a b");
    }

    #[tokio::test]
    async fn rows_round_trip_newest_first_and_update_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(dir.path()).await.expect("open db");
        let mut older = item();
        older.id = "older".into();
        older.created_at = 10;
        older.files = vec!["/a/b.pdf".into()];
        older.probability = None;
        older.suggested = false;
        older.title = "it's quoted".into();
        let mut newer = item();
        newer.id = "newer".into();
        newer.created_at = 20;
        newer.alternatives = vec![InboxAlternative {
            notebook_id: "x".into(),
            title: "X".into(),
            probability: 0.25,
        }];
        db.add_inbox_item(&older).await.unwrap();
        db.add_inbox_item(&newer).await.unwrap();
        let listed = db.list_inbox().await.unwrap();
        assert_eq!(
            listed.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["newer", "older"]
        );
        assert_eq!(listed[1].files, vec!["/a/b.pdf".to_string()]);
        assert_eq!(listed[1].probability, None);
        assert_eq!(listed[0].probability, Some(0.9));
        assert_eq!(listed[0].alternatives[0].probability, 0.25);

        let mut filled = db.get_inbox_item("older").await.unwrap().unwrap();
        assert_eq!(filled.title, "it's quoted");
        filled.suggested = true;
        filled.is_new = true;
        filled.suggested_title = "Fresh 'one'".into();
        filled.probability = Some(0.555);
        db.update_inbox_item(&filled).await.unwrap();
        let back = db.get_inbox_item("older").await.unwrap().unwrap();
        assert!(back.suggested && back.is_new);
        assert_eq!(back.suggested_title, "Fresh 'one'");
        assert_eq!(back.probability, Some(0.555));

        db.delete_inbox_item("older").await.unwrap();
        assert!(db.get_inbox_item("older").await.unwrap().is_none());
        assert_eq!(db.list_inbox().await.unwrap().len(), 1);
    }
}
