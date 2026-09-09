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
        note_id: String::new(),
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

/// Could this schedule write into this note without overwriting someone
/// else's work? Its own runs and the app's own writes (origin "" or an
/// `alchemy/…` by-line, the shape sync stamps on the app's notes) qualify;
/// a note another author or agent signed, or one the curator archived, is
/// a version to keep, not a target.
fn adoptable(schedule: &ReportSchedule, note: &Note) -> bool {
    note.notebook_id == schedule.notebook_id
        && note.kind == "report"
        && note.status != "archived"
        && (note.origin.is_empty() || note.origin.starts_with("alchemy/"))
}

/// The note a schedule updates in place, by identity rather than title:
///
/// 1. the pinned `note_id`, when it still names a live report here — a
///    pin that dangles (the note was deleted or archived) yields None, so
///    the run starts a fresh note instead of adopting a look-alike;
/// 2. otherwise the note this schedule's last successful receipt wrote,
///    the strongest evidence a pre-pin schedule has;
/// 3. otherwise the one same-title report the schedule could safely own
///    (`adoptable`), which is how schedules from before the pin migrate —
///    once, since the run that follows pins its choice. Two or more such
///    candidates is a guess, and this does not guess: the run starts a
///    fresh note and every existing version keeps its text.
///
/// Nothing here deletes, renames, or reorders notes; every same-title
/// version stays exactly where it was.
pub(super) async fn living_report_note(
    db: &crate::db::Db,
    schedule: &ReportSchedule,
) -> anyhow::Result<Option<Note>> {
    if !schedule.note_id.is_empty() {
        return Ok(db
            .get_note(&schedule.note_id)
            .await?
            .filter(|note| adoptable(schedule, note)));
    }
    if let Some(id) = db.last_written_note(&schedule.id).await? {
        if let Some(note) = db.get_note(&id).await? {
            if adoptable(schedule, &note) {
                return Ok(Some(note));
            }
        }
    }
    let notes = db.list_notes(&schedule.notebook_id).await?;
    let candidates: Vec<&Note> = report_notes_for(&notes, &schedule.name)
        .into_iter()
        .filter(|note| adoptable(schedule, note))
        .collect();
    // The living note carries the bare name; a "name — stamp" title is a
    // historical version by construction and only stands in when no bare
    // one exists.
    let exact: Vec<&Note> = candidates
        .iter()
        .copied()
        .filter(|note| note.title == schedule.name)
        .collect();
    let pool = if exact.is_empty() {
        &candidates
    } else {
        &exact
    };
    Ok(match pool.as_slice() {
        [only] => Some((*only).clone()),
        _ => None,
    })
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
    let existing = e(living_report_note(&state.db, &schedule).await)?;
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
    // Pin the identity the run just acted on, so the next run goes by id
    // and never by title again. Only written when it changes.
    if schedule.note_id != note.id {
        e(state.db.set_report_note(&schedule.id, &note.id).await)?;
    }
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

    fn schedule(notebook_id: &str, name: &str) -> ReportSchedule {
        ReportSchedule {
            id: "schedule-1".into(),
            notebook_id: notebook_id.into(),
            name: name.into(),
            kind: "briefing".into(),
            prompt: String::new(),
            trigger: "interval".into(),
            not_before: 0,
            interval_secs: 86_400,
            enabled: true,
            watch_sources: String::new(),
            watch_kinds: String::new(),
            note_id: String::new(),
            last_run_at: 0,
            created_at: 1,
        }
    }

    fn report(id: &str, name: &str, updated_at: i64) -> Note {
        Note {
            id: id.into(),
            notebook_id: "finance".into(),
            title: name.into(),
            content: format!("Report body of {id}."),
            kind: "report".into(),
            prompt: "scope".into(),
            origin: "alchemy/0.56.2".into(),
            status: String::new(),
            created_at: updated_at - 5,
            updated_at,
        }
    }

    fn receipt(schedule_id: &str, note_id: &str, ended_at: i64) -> crate::models::RunReceipt {
        crate::models::RunReceipt {
            id: format!("receipt-{ended_at}"),
            schedule_id: schedule_id.into(),
            notebook_id: "finance".into(),
            name: "portfolio value review".into(),
            kind: "briefing".into(),
            trigger: "interval".into(),
            status: "ok".into(),
            detail: String::new(),
            error: String::new(),
            note_id: note_id.into(),
            provider: "ollama".into(),
            model: String::new(),
            cost_micros: 0,
            due_at: ended_at,
            started_at: ended_at,
            ended_at,
        }
    }

    #[tokio::test]
    async fn pinned_note_wins_over_newer_same_title_report() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).await.unwrap();
        let name = "portfolio value review";
        let original = report("original", name, 20);
        let newer = report("newer", name, 40);
        db.add_note(&original).await.unwrap();
        db.add_note(&newer).await.unwrap();
        let mut s = schedule("finance", name);
        s.note_id = original.id.clone();
        let chosen = living_report_note(&db, &s).await.unwrap().unwrap();
        assert_eq!(chosen.id, original.id);
        assert_eq!(chosen.content, original.content);
    }

    #[tokio::test]
    async fn dangling_archived_or_foreign_pin_starts_fresh_instead_of_guessing() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).await.unwrap();
        let name = "portfolio value review";
        let lookalike = report("lookalike", name, 40);
        db.add_note(&lookalike).await.unwrap();
        let mut s = schedule("finance", name);
        s.note_id = "deleted-long-ago".into();
        assert!(living_report_note(&db, &s).await.unwrap().is_none());

        let archived = Note {
            status: "archived".into(),
            ..report("archived", name, 50)
        };
        db.add_note(&archived).await.unwrap();
        s.note_id = archived.id.clone();
        assert!(living_report_note(&db, &s).await.unwrap().is_none());

        let elsewhere = Note {
            notebook_id: "other-notebook".into(),
            ..report("elsewhere", name, 60)
        };
        db.add_note(&elsewhere).await.unwrap();
        s.note_id = elsewhere.id.clone();
        assert!(living_report_note(&db, &s).await.unwrap().is_none());
        // Nothing was touched to reach that answer.
        assert_eq!(db.list_notes("finance").await.unwrap().len(), 2);
        assert_eq!(db.list_notes("other-notebook").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn pre_pin_schedule_adopts_the_note_its_receipt_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).await.unwrap();
        let name = "portfolio value review";
        let own = report("own-run", name, 20);
        let newer = report("imported-newer", name, 40);
        db.add_note(&own).await.unwrap();
        db.add_note(&newer).await.unwrap();
        let s = schedule("finance", name);
        let now = crate::commands::now();
        db.add_receipt(&receipt(&s.id, &newer.id, now - 3_000))
            .await
            .unwrap();
        db.add_receipt(&receipt(&s.id, &own.id, now - 1_000))
            .await
            .unwrap();
        // A failed run names no note and must not count.
        let mut failed = receipt(&s.id, "", now);
        failed.status = "failed".into();
        db.add_receipt(&failed).await.unwrap();
        // Another schedule's receipt is not evidence for this one.
        db.add_receipt(&receipt("schedule-2", &newer.id, now + 1_000))
            .await
            .unwrap();
        assert_eq!(
            db.last_written_note(&s.id).await.unwrap().as_deref(),
            Some("own-run")
        );
        assert_eq!(
            living_report_note(&db, &s).await.unwrap().unwrap().id,
            own.id
        );
    }

    #[tokio::test]
    async fn pre_pin_fallback_with_two_versions_starts_fresh_and_touches_neither() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).await.unwrap();
        let name = "portfolio value review";
        let original = report("original", name, 20);
        let newer = Note {
            origin: "alchemy/0.56.0".into(),
            ..report("newer", name, 40)
        };
        db.add_note(&original).await.unwrap();
        db.add_note(&newer).await.unwrap();
        let before = serde_json::to_value(db.list_notes("finance").await.unwrap()).unwrap();
        let s = schedule("finance", name);
        assert!(living_report_note(&db, &s).await.unwrap().is_none());
        drop(db);
        let reopened = crate::db::Db::open(dir.path()).await.unwrap();
        assert!(living_report_note(&reopened, &s).await.unwrap().is_none());
        assert_eq!(
            serde_json::to_value(reopened.list_notes("finance").await.unwrap()).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn pre_pin_fallback_skips_archived_and_foreign_versions_without_touching_them() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).await.unwrap();
        let name = "portfolio value review";
        let newer = Note {
            origin: "alchemy/0.56.0".into(),
            ..report("newer", name, 40)
        };
        let historic = Note {
            title: format!("{name} \u{2014} 2026-07-13 09:00"),
            ..report("historic", name, 25)
        };
        let archived = Note {
            status: "archived".into(),
            ..report("archived", name, 100)
        };
        let human = Note {
            origin: "human:reviewer".into(),
            ..report("human", name, 200)
        };
        let agent = Note {
            origin: "codex/1.0".into(),
            ..report("agent", name, 300)
        };
        let regular = Note {
            kind: "note".into(),
            ..report("regular", name, 400)
        };
        let other_title = report("other", &format!("{name} extended"), 500);
        for note in [
            &newer,
            &historic,
            &archived,
            &human,
            &agent,
            &regular,
            &other_title,
        ] {
            db.add_note(note).await.unwrap();
        }
        let before = serde_json::to_value(db.list_notes("finance").await.unwrap()).unwrap();
        let s = schedule("finance", name);
        for _ in 0..2 {
            let prior = living_report_note(&db, &s).await.unwrap().unwrap();
            assert_eq!(prior.id, newer.id);
            assert_eq!(prior.origin, newer.origin);
            assert_eq!(
                serde_json::to_value(db.list_notes("finance").await.unwrap()).unwrap(),
                before
            );
            for id in [&newer.id, &historic.id, &archived.id, &human.id, &agent.id] {
                assert!(!db.was_deleted("note", id).unwrap());
            }
        }
        drop(db);
        let reopened = crate::db::Db::open(dir.path()).await.unwrap();
        assert_eq!(
            living_report_note(&reopened, &s).await.unwrap().unwrap().id,
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
            title: "Review \u{2014} 2026-07-13 09:00".into(),
            content: "Original stamped report.".into(),
            kind: "report".into(),
            prompt: "Original scope".into(),
            origin: String::new(),
            status: String::new(),
            created_at: 1,
            updated_at: 2,
        };
        db.add_note(&note).await.unwrap();
        let selected = living_report_note(&db, &schedule("notebook", "Review"))
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

    #[tokio::test]
    async fn pin_survives_a_reopen_and_reads_empty_on_older_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).await.unwrap();
        let s = schedule("finance", "portfolio value review");
        db.add_report_schedule(&s).await.unwrap();
        assert_eq!(
            db.get_report_schedule(&s.id)
                .await
                .unwrap()
                .unwrap()
                .note_id,
            ""
        );
        db.set_report_note(&s.id, "living-note").await.unwrap();
        drop(db);
        let reopened = crate::db::Db::open(dir.path()).await.unwrap();
        let stored = reopened.get_report_schedule(&s.id).await.unwrap().unwrap();
        assert_eq!(stored.note_id, "living-note");
        assert_eq!(stored.name, s.name);
    }
}
