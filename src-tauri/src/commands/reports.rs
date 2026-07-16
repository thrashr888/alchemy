use super::*;

#[tauri::command]
pub async fn list_report_schedules(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Vec<ReportSchedule>, String> {
    e(state.db.list_report_schedules(&notebook_id).await)
}

#[tauri::command]
pub async fn list_all_report_schedules(
    state: State<'_, AppState>,
) -> Result<Vec<ReportSchedule>, String> {
    e(state.db.all_report_schedules().await)
}

#[tauri::command]
pub async fn create_report_schedule(
    state: State<'_, AppState>,
    notebook_id: String,
    name: String,
    kind: String,
    prompt: String,
    interval_secs: i64,
) -> Result<ReportSchedule, String> {
    let schedule = ReportSchedule {
        id: new_id(),
        notebook_id,
        name: name.trim().to_string(),
        kind,
        prompt,
        interval_secs,
        enabled: true,
        last_run_at: 0,
        created_at: now(),
    };
    e(state.db.add_report_schedule(&schedule).await)?;
    Ok(schedule)
}

#[tauri::command]
pub async fn update_report_schedule(
    state: State<'_, AppState>,
    id: String,
    name: String,
    kind: String,
    prompt: String,
    interval_secs: i64,
    enabled: bool,
) -> Result<(), String> {
    e(state
        .db
        .update_report_schedule(&id, name.trim(), &kind, &prompt, interval_secs, enabled)
        .await)
}

#[tauri::command]
pub async fn delete_report_schedule(state: State<'_, AppState>, id: String) -> Result<(), String> {
    e(state.db.delete_report_schedule(&id).await)
}

async fn refresh_notebook_urls(app: &AppHandle, state: &AppState, notebook_id: &str) {
    let sources = state.db.list_sources(notebook_id).await.unwrap_or_default();
    for source in sources
        .iter()
        .filter(|source| source.source_type == "url" && !source.url.is_empty())
    {
        let _ = app.emit("report://step", format!("Refreshing: {}", source.title));
        if let Ok(Some(existing)) = state.db.get_source(&source.id).await {
            if let Ok(extracted) = ingest::extract_url(&existing.url).await {
                let _ = reingest(state, &existing, extracted).await;
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

pub(super) async fn collapse_report_notes(
    state: &AppState,
    notebook_id: &str,
    name: &str,
) -> anyhow::Result<Option<Note>> {
    let notes = state.db.list_notes(notebook_id).await?;
    let mut matches = report_notes_for(&notes, name);
    matches.sort_by_key(|note| std::cmp::Reverse(note.updated_at));
    let mut iter = matches.into_iter();
    let Some(survivor) = iter.next() else {
        return Ok(None);
    };
    for stale in iter {
        state.db.delete_note(&stale.id).await?;
    }
    if survivor.title != name {
        state
            .db
            .update_note(&survivor.id, name, &survivor.content, survivor.updated_at)
            .await?;
    }
    state.db.get_note(&survivor.id).await
}

#[tauri::command]
pub async fn run_report(
    app: AppHandle,
    state: State<'_, AppState>,
    schedule_id: String,
) -> Result<Note, String> {
    let schedule = e(state.db.get_report_schedule(&schedule_id).await)?
        .ok_or_else(|| "Report schedule not found".to_string())?;

    refresh_notebook_urls(&app, &state, &schedule.notebook_id).await;

    // Collapse before generating so the survivor doubles as the prior run —
    // its content lets the model report changes since last time (its first
    // line is the `_Run …_` stamp, so the date travels with it).
    let existing = e(collapse_report_notes(&state, &schedule.notebook_id, &schedule.name).await)?;
    let prior_content = existing.as_ref().map(|note| note.content.clone());

    let _ = app.emit("report://step", "Generating report".to_string());
    let (_title, content) = e(generate_content(
        &state,
        None,
        &schedule.notebook_id,
        &schedule.kind,
        &schedule.prompt,
        None,
        prior_content.as_deref(),
    )
    .await)?;

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
                    index_note(&state, &note).await;
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
            e(add_note_indexed(&state, &note).await)?;
            note
        }
    };
    e(state.db.set_report_last_run(&schedule_id, timestamp).await)?;
    e(state
        .db
        .touch_notebook(&schedule.notebook_id, timestamp)
        .await)?;
    let _ = app.emit("generate://done", &note);
    Ok(note)
}
