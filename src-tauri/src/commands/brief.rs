//! The Brief (docs/RFC-brief.md): one synthesized arrival point per cadence,
//! ranked needs-you → changed → for-the-record. A brief is a report schedule
//! whose kind is "brief"; runs land as report-kind notes in the "Briefs"
//! system notebook — an ordinary notebook by design, so Reader, search, OKF
//! export, and MCP all work on it for free. The collector reads what Night
//! Shift v1 already produces; it grows richer as later pillars land, the
//! shape doesn't change.

use super::*;

pub(crate) const BRIEF_KIND: &str = "brief";
const BRIEFS_NOTEBOOK: &str = "Briefs";
const DEFAULT_BRIEF_NAME: &str = "Morning Brief";

/// Find or create the Briefs notebook.
async fn ensure_briefs_notebook(state: &AppState) -> Result<Notebook, String> {
    let notebooks = e(state.db.list_notebooks().await)?;
    if let Some(nb) = notebooks.iter().find(|n| n.title == BRIEFS_NOTEBOOK) {
        return Ok(nb.clone());
    }
    let ts = now();
    let color = NOTEBOOK_PALETTE[notebooks.len() % NOTEBOOK_PALETTE.len()];
    let nb = Notebook {
        id: new_id(),
        title: BRIEFS_NOTEBOOK.into(),
        created_at: ts,
        updated_at: ts,
        color: color.to_string(),
        source_count: 0,
    };
    e(state.db.create_notebook(&nb).await)?;
    Ok(nb)
}

/// Create the default daily brief once, ever — smart defaults on; deleting
/// the schedule is respected (the marker file means "offered already", so a
/// deleted brief never resurrects).
pub(crate) async fn ensure_default_brief(state: &AppState) {
    let marker = app_data_dir(state).join("brief-default-created");
    if marker.exists() {
        return;
    }
    let Ok(nb) = ensure_briefs_notebook(state).await else {
        return; // transient — retry next pass, marker unwritten
    };
    let schedule = ReportSchedule {
        id: new_id(),
        notebook_id: nb.id,
        name: DEFAULT_BRIEF_NAME.into(),
        kind: BRIEF_KIND.into(),
        prompt: String::new(),
        interval_secs: 86_400,
        enabled: true,
        // Aligned so the first run lands at the next 7 AM local rather than
        // 24h from whenever the app happened to first launch this build.
        last_run_at: crate::scheduler::next_local_hour_ms(7) - 86_400_000,
        created_at: now(),
    };
    if state.db.add_report_schedule(&schedule).await.is_ok() {
        let _ = std::fs::write(&marker, b"1");
    }
}

/// Everything collected for one brief window, already in rank order.
struct Collected {
    context: String,
    item_count: usize,
}

/// Collect what happened since the last brief: plain queries, no model.
/// Rank is structural — the section order IS the ranking, and the writer is
/// told to preserve it.
async fn collect(state: &AppState, briefs_notebook_id: &str, since: i64) -> Collected {
    let notebooks = state.db.list_notebooks().await.unwrap_or_default();
    let mut attention = String::new();
    let mut changed = String::new();
    let mut quiet: Vec<String> = Vec::new();
    let mut items = 0usize;

    for nb in notebooks.iter().filter(|n| n.id != briefs_notebook_id) {
        let sources = state.db.list_sources(&nb.id).await.unwrap_or_default();
        let notes = state.db.list_notes(&nb.id).await.unwrap_or_default();

        let mut nb_changed = String::new();
        for s in &sources {
            if s.status == "error" {
                let reason = if s.error.is_empty() {
                    "import failed".into()
                } else {
                    s.error.chars().take(200).collect::<String>()
                };
                attention.push_str(&format!(
                    "- [{}] source \u{201c}{}\u{201d} is in an error state: {}\n",
                    nb.title, s.title, reason
                ));
                items += 1;
            } else if s.created_at > since {
                nb_changed.push_str(&format!("  - new source: \u{201c}{}\u{201d}\n", s.title));
                items += 1;
            } else if s.mtime > since {
                nb_changed.push_str(&format!(
                    "  - source updated on disk: \u{201c}{}\u{201d}\n",
                    s.title
                ));
                items += 1;
            }
        }
        for note in notes
            .iter()
            .filter(|n| n.kind == "report" && n.updated_at > since)
        {
            // Skip the run stamp line; give the writer the report's own head.
            let body: String = note
                .content
                .lines()
                .skip(2)
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .take(600)
                .collect();
            nb_changed.push_str(&format!(
                "  - scheduled report ran: \u{201c}{}\u{201d} — excerpt:\n    {}\n",
                note.title,
                body.replace('\n', "\n    ")
            ));
            items += 1;
        }
        if nb_changed.is_empty() {
            quiet.push(nb.title.clone());
        } else {
            changed.push_str(&format!("- [{}]\n{nb_changed}", nb.title));
        }
    }

    let mut context = String::new();
    if !attention.is_empty() {
        context.push_str(&format!("## Needs attention (rank first)\n{attention}\n"));
    }
    if !changed.is_empty() {
        context.push_str(&format!("## Changed since the last brief\n{changed}\n"));
    }
    if !quiet.is_empty() {
        context.push_str(&format!(
            "## Quiet notebooks (one line at most, or omit)\n{}\n",
            quiet.join(", ")
        ));
    }
    Collected {
        context,
        item_count: items,
    }
}

const BRIEF_INSTRUCTION: &str = "You are writing the user's brief: one short document covering \
what happened across their notebooks since the last brief, and what needs them. The collected \
items below are already in rank order — keep that order. Structure the brief as exactly three \
markdown sections, omitting any that would be empty: \"## Needs you\" (errors and anything \
wanting a decision, each with one line on what happens if ignored), \"## What changed\" (new and \
updated sources, scheduled report findings — summarize the findings, don't repeat whole \
reports), and \"## For the record\" (everything quiet, at most a sentence). Name notebooks and \
sources exactly as given. If a previous brief is provided, open with a single italic line noting \
what's new since it, and do not repeat items it already covered. No preamble, no sign-off, \
under 500 words.";

/// Run one brief: collect → write one generation call → persist as a
/// report-kind note in the Briefs notebook (same persistence as any report,
/// so unread state, the reports feed, and prior-run threading are free).
pub(crate) async fn run_brief(
    app: &AppHandle,
    state: &AppState,
    schedule: ReportSchedule,
) -> Result<Note, String> {
    let existing = e(collapse_report_notes(state, &schedule.notebook_id, &schedule.name).await)?;
    let since = existing
        .as_ref()
        .map(|n| n.updated_at)
        .unwrap_or_else(|| now() - 86_400_000);

    let _ = app.emit("report://step", "Collecting for the brief".to_string());
    let collected = collect(state, &schedule.notebook_id, since).await;
    if collected.item_count == 0 && existing.is_some() {
        // A brief with nothing to say is noise, not stewardship: skip the
        // model call, stamp the schedule so it doesn't retry every tick.
        e(state.db.set_report_last_run(&schedule.id, now()).await)?;
        return existing.ok_or_else(|| "unreachable: existing checked above".into());
    }

    let mut corpus = format!("## Collected activity\n\n{}\n", collected.context);
    if let Some(prior) = existing.as_ref().map(|n| n.content.as_str()) {
        let clipped: String = prior.chars().take(8_000).collect();
        corpus.push_str(&format!(
            "## Previous brief (for the since-last-time line — do not repeat its items)\n\n{clipped}\n"
        ));
    }
    let instruction = if schedule.prompt.trim().is_empty() {
        BRIEF_INSTRUCTION.to_string()
    } else {
        format!(
            "{BRIEF_INSTRUCTION}\n\nAdditional instructions from the user (follow these):\n{}",
            schedule.prompt.trim()
        )
    };
    let persona = {
        let ai = state.ai.read().await.clone();
        rag::persona_block(&ai.config().profile)
    };
    let _ = app.emit("report://step", "Writing the brief".to_string());
    let messages = rag::build_artifact_messages(&instruction, &corpus, &persona);
    let content = run_generation_chat(state, None, &messages)
        .await
        .map_err(|e| format!("{e:#}"))?;

    persist_report_run(app, state, &schedule, existing, content).await
}
