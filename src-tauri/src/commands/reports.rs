use super::*;

#[tauri::command]
pub async fn list_report_schedules(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Vec<ReportSchedule>, String> {
    e(state.db.list_report_schedules(&notebook_id).await)
}

/// "interval" | "change", defaulted so pre-trigger callers keep working.
fn resolve_trigger(trigger: Option<String>) -> String {
    match trigger.as_deref() {
        Some("change") => "change".to_string(),
        _ => "interval".to_string(),
    }
}

/// The change-trigger filters (docs/RFC-events.md §5), normalized: source
/// ids as given, kinds restricted to `EVENT_KINDS`. Empty means "any".
pub(crate) fn resolve_watch(sources: Option<String>, kinds: Option<String>) -> (String, String) {
    (
        crate::models::normalize_watch_list(sources.as_deref().unwrap_or(""), None),
        crate::models::normalize_watch_list(
            kinds.as_deref().unwrap_or(""),
            Some(&crate::models::EVENT_KINDS),
        ),
    )
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn create_report_schedule(
    state: State<'_, AppState>,
    notebook_id: String,
    name: String,
    kind: String,
    prompt: String,
    trigger: Option<String>,
    interval_secs: i64,
    watch_sources: Option<String>,
    watch_kinds: Option<String>,
) -> Result<ReportSchedule, String> {
    let (watch_sources, watch_kinds) = resolve_watch(watch_sources, watch_kinds);
    let schedule = ReportSchedule {
        watch_sources,
        watch_kinds,
        id: new_id(),
        notebook_id,
        name: name.trim().to_string(),
        kind,
        prompt,
        trigger: resolve_trigger(trigger),
        not_before: 0,
        interval_secs,
        enabled: true,
        last_run_at: 0,
        created_at: now(),
    };
    e(state.db.add_report_schedule(&schedule).await)?;
    Ok(schedule)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_report_schedule(
    state: State<'_, AppState>,
    id: String,
    name: String,
    kind: String,
    prompt: String,
    trigger: Option<String>,
    interval_secs: i64,
    enabled: bool,
    watch_sources: Option<String>,
    watch_kinds: Option<String>,
) -> Result<(), String> {
    let (watch_sources, watch_kinds) = resolve_watch(watch_sources, watch_kinds);
    e(state
        .db
        .update_report_schedule(
            &id,
            name.trim(),
            &kind,
            &prompt,
            &resolve_trigger(trigger),
            interval_secs,
            enabled,
            &watch_sources,
            &watch_kinds,
        )
        .await)
}

#[tauri::command]
pub async fn delete_report_schedule(state: State<'_, AppState>, id: String) -> Result<(), String> {
    e(state.db.delete_report_schedule(&id).await)
}

/// Sources re-fetched within this window skip the pre-report refresh — a
/// burst of due schedules used to re-fetch the same pages back to back
/// (docs/RFC-source-hygiene.md; `fetched_at` is stamped by every reingest).
const REPORT_REFRESH_FLOOR_MS: i64 = 60 * 60 * 1000;

async fn refresh_notebook_urls(app: &AppHandle, state: &AppState, notebook_id: &str) {
    let sources = state.db.list_sources(notebook_id).await.unwrap_or_default();
    let now = crate::commands::now();
    for source in sources.iter().filter(|source| {
        source.source_type == "url"
            && !source.url.is_empty()
            && now - source.fetched_at > REPORT_REFRESH_FLOOR_MS
    }) {
        let _ = app.emit("report://step", format!("Refreshing: {}", source.title));
        if let Ok(Some(existing)) = state.db.get_source(&source.id).await {
            if let Ok(extracted) = crate::capture::extract_url_rescued(&existing.url).await {
                let _ = reingest(state, &existing, extracted, None, true).await;
            }
        }
    }
}

fn report_notes_for<'a>(notes: &'a [Note], name: &str) -> Vec<&'a Note> {
    let prefix = format!("{name} — ");
    notes
        .iter()
        .filter(|note| {
            note.kind == "report" && (note.title == name || note.title.starts_with(&prefix))
        })
        .collect()
}

/// Select the prior run without deleting or renaming independent artifacts.
/// Matching titles identify candidates for a schedule, not duplicate notes.
pub(super) async fn latest_report_note(
    db: &crate::db::Db,
    notebook_id: &str,
    name: &str,
) -> anyhow::Result<Option<Note>> {
    let notes = db.list_notes(notebook_id).await?;
    let mut matches = report_notes_for(&notes, name);
    matches.sort_by_key(|note| std::cmp::Reverse(note.updated_at));
    Ok(matches.into_iter().next().cloned())
}

#[tauri::command]
pub async fn run_report(
    app: AppHandle,
    state: State<'_, AppState>,
    schedule_id: String,
) -> Result<Note, String> {
    run_report_inner(&app, &state, &schedule_id).await
}

/// The command's body, callable from the resident scheduler (scheduler.rs).
pub(crate) async fn run_report_inner(
    app: &AppHandle,
    state: &AppState,
    schedule_id: &str,
) -> Result<Note, String> {
    let app = app.clone();
    let schedule_id = schedule_id.to_string();
    let schedule = e(state.db.get_report_schedule(&schedule_id).await)?
        .ok_or_else(|| "Report schedule not found".to_string())?;

    // Briefs are schedules too (kind "brief"), but they read across every
    // notebook instead of generating from one — see commands/brief.rs.
    if schedule.kind == super::brief::BRIEF_KIND {
        return super::brief::run_brief(&app, state, schedule).await;
    }

    refresh_notebook_urls(&app, state, &schedule.notebook_id).await;

    // Read the most recent report as the prior run without changing history —
    // its content lets the model report changes since last time (its first
    // line is the `_Run …_` stamp, so the date travels with it).
    let existing = e(latest_report_note(&state.db, &schedule.notebook_id, &schedule.name).await)?;
    let prior_content = existing.as_ref().map(|note| note.content.clone());

    let _ = app.emit("report://step", "Generating report".to_string());
    // The indicator names the report, so an unattended run at 6am is legible
    // in the morning as well as while it happens.
    let (_title, content) = e(crate::inference::labeled(
        format!("Report: {}", schedule.name),
        generate_content(
            state,
            None,
            &schedule.notebook_id,
            &schedule.kind,
            &schedule.prompt,
            None,
            prior_content.as_deref(),
            None,
            None,
        ),
    )
    .await)?;

    persist_report_run(&app, state, &schedule, existing, content).await
}

/// The write side of any scheduled run — reports and briefs share it: stamp
/// the run, update the living note (or create it), re-index, mark the
/// schedule run, and announce. The selected note doubles as the next run's
/// prior for change tracking.
pub(super) async fn persist_report_run(
    app: &AppHandle,
    state: &AppState,
    schedule: &ReportSchedule,
    existing: Option<Note>,
    content: String,
) -> Result<Note, String> {
    let timestamp = now();
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let content = format!("_Run {stamp}_\n\n{content}");
    let note = match existing {
        Some(prior) => {
            e(state
                .db
                .update_note(&prior.id, &schedule.name, &content, timestamp)
                .await)?;
            match e(state.db.get_note(&prior.id).await)? {
                Some(note) => {
                    index_note(state, &note).await;
                    note
                }
                None => return Err("Report note vanished mid-update".into()),
            }
        }
        None => {
            let note = Note {
                id: new_id(),
                notebook_id: schedule.notebook_id.clone(),
                title: schedule.name.clone(),
                content,
                kind: "report".into(),
                prompt: schedule.prompt.clone(),
                origin: String::new(),
                status: String::new(),
                created_at: timestamp,
                updated_at: timestamp,
            };
            e(add_note_indexed(state, &note).await)?;
            note
        }
    };
    e(state.db.set_report_last_run(&schedule.id, timestamp).await)?;
    e(state
        .db
        .touch_notebook(&schedule.notebook_id, timestamp)
        .await)?;
    let _ = app.emit("generate://done", &note);
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prior_report_selection_preserves_distinct_versions_and_provenance_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).await.unwrap();
        let name = "portfolio value review";
        let original = Note {
            id: "original-report".into(),
            notebook_id: "finance".into(),
            title: name.into(),
            content: "Original portfolio analysis and its evidence.".into(),
            kind: "report".into(),
            prompt: "Original review scope".into(),
            origin: "alchemy/0.56.2".into(),
            status: String::new(),
            created_at: 10,
            updated_at: 20,
        };
        let newer = Note {
            id: "newer-report".into(),
            content: "Different, newer portfolio analysis.".into(),
            prompt: "New review scope".into(),
            origin: "alchemy/0.56.0".into(),
            created_at: 30,
            updated_at: 40,
            ..original.clone()
        };
        let historic = Note {
            id: "historic-report".into(),
            title: format!("{name} — 2026-07-13 09:00"),
            content: "Timestamped historical analysis.".into(),
            updated_at: 25,
            ..original.clone()
        };
        let other_provenance = Note {
            id: "separate-provenance".into(),
            origin: "human:reviewer".into(),
            prompt: "Different evidence scope".into(),
            status: "archived".into(),
            ..original.clone()
        };
        let regular_note = Note {
            id: "regular-note".into(),
            kind: "note".into(),
            updated_at: 100,
            ..original.clone()
        };
        let other_report = Note {
            id: "other-report".into(),
            title: format!("{name} extended"),
            updated_at: 100,
            ..original.clone()
        };
        for note in [
            &original,
            &newer,
            &historic,
            &other_provenance,
            &regular_note,
            &other_report,
        ] {
            db.add_note(note).await.unwrap();
        }
        let before = serde_json::to_value(db.list_notes("finance").await.unwrap()).unwrap();
        for _ in 0..2 {
            let prior = latest_report_note(&db, "finance", name)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(prior.id, newer.id);
            assert_eq!(prior.content, newer.content);
            assert_eq!(prior.origin, newer.origin);
            assert_eq!(
                serde_json::to_value(db.list_notes("finance").await.unwrap()).unwrap(),
                before
            );
            for id in [&original.id, &newer.id, &historic.id, &other_provenance.id] {
                assert!(!db.was_deleted("note", id).unwrap());
            }
        }
        drop(db);
        let reopened = crate::db::Db::open(dir.path()).await.unwrap();
        assert_eq!(
            latest_report_note(&reopened, "finance", name)
                .await
                .unwrap()
                .unwrap()
                .id,
            newer.id
        );
        assert_eq!(
            serde_json::to_value(reopened.list_notes("finance").await.unwrap()).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn choosing_latest_timestamped_report_does_not_rename_its_historical_title() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).await.unwrap();
        let note = Note {
            id: "historic".into(),
            notebook_id: "notebook".into(),
            title: "Review — 2026-07-13 09:00".into(),
            content: "Original stamped report.".into(),
            kind: "report".into(),
            prompt: "Original scope".into(),
            origin: "human:author".into(),
            status: String::new(),
            created_at: 1,
            updated_at: 2,
        };
        db.add_note(&note).await.unwrap();
        let selected = latest_report_note(&db, "notebook", "Review")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&selected).unwrap(),
            serde_json::to_value(&note).unwrap()
        );
        assert_eq!(
            serde_json::to_value(db.get_note(&note.id).await.unwrap().unwrap()).unwrap(),
            serde_json::to_value(&note).unwrap()
        );
    }
}
