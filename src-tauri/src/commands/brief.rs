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
        icon: String::new(),
        // "system": briefs land here behind the scenes — the notebook works
        // like any other (Reader, search, OKF, MCP) but stays off the shelf,
        // because a permanently source-less notebook in the grid reads as
        // clutter, not a feature.
        status: "system".into(),
        source_count: 0,
        note_count: 0,
        report_count: 0,
    };
    e(state.db.create_notebook(&nb).await)?;
    Ok(nb)
}

/// Create the default daily brief once, ever — smart defaults on; deleting
/// the schedule is respected (the marker file means "offered already", so a
/// deleted brief never resurrects).
pub(crate) async fn ensure_default_brief(state: &AppState) {
    // Self-healing upgrade, every pass (one cheap list): a Briefs notebook
    // that is visible ("") or archived moves to "system". Visible was the
    // old default and read as shelf clutter; archived is worse — the user
    // hiding the clutter by archiving silently killed their brief, because
    // the scheduler's background gate skips archived notebooks.
    if let Ok(notebooks) = state.db.list_notebooks().await {
        if let Some(nb) = notebooks
            .iter()
            .find(|n| n.title == BRIEFS_NOTEBOOK && n.status != "system")
        {
            let _ = state.db.set_notebook_status(&nb.id, "system").await;
        }
    }
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
        trigger: "interval".into(),
        not_before: 0,
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
    // Updates come from the events the refresh paths write (accurate for
    // every source class — mac/git content stamps aren't timestamps, so the
    // old mtime heuristic missed them). Newest-first, one line per source.
    let events = state
        .db
        .source_events_since(since)
        .await
        .unwrap_or_default();
    let mut attention = String::new();
    let mut changed = String::new();
    let mut quiet: Vec<String> = Vec::new();
    let mut items = 0usize;

    for nb in notebooks.iter().filter(|n| n.id != briefs_notebook_id) {
        let sources = state.db.list_sources(&nb.id).await.unwrap_or_default();
        let notes = state.db.list_notes(&nb.id).await.unwrap_or_default();
        let ledger = state.db.list_ledger(&nb.id).await.unwrap_or_default();

        let mut nb_changed = String::new();
        // The Weave's verdicts lead the brief: a freshly contradicted row is
        // exactly what needs a human; answered/superseded is news.
        for entry in ledger.iter().filter(|l| l.updated_at > since) {
            let last_why = entry.why.lines().last().unwrap_or("").trim();
            match entry.status.as_str() {
                "contradicted" => {
                    attention.push_str(&format!(
                        "- [{}] ledger {} now CONTRADICTED: \u{201c}{}\u{201d} ({})\n",
                        nb.title, entry.kind, entry.text, last_why
                    ));
                    items += 1;
                }
                "superseded" | "answered" => {
                    nb_changed.push_str(&format!(
                        "  - ledger {} {}: \u{201c}{}\u{201d} ({})\n",
                        entry.kind, entry.status, entry.text, last_why
                    ));
                    items += 1;
                }
                _ => {}
            }
        }
        let mut fresh: std::collections::HashSet<&str> = std::collections::HashSet::new();
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
                fresh.insert(s.id.as_str());
                nb_changed.push_str(&format!("  - new source: \u{201c}{}\u{201d}\n", s.title));
                items += 1;
            }
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for event in events.iter().filter(|e| {
            e.notebook_id == nb.id && e.kind == "updated" && !fresh.contains(e.source_id.as_str())
        }) {
            if !seen.insert(event.source_id.as_str()) {
                continue; // several updates in the window — newest line wins
            }
            nb_changed.push_str(&format!(
                "  - source updated ({}): \u{201c}{}\u{201d}\n",
                event.detail, event.source_title
            ));
            // A taste of the actual change grounds the writer — the diff's
            // first lines, already ±-prefixed and capped at write time.
            for line in event.diff.lines().take(3) {
                nb_changed.push_str(&format!("      {line}\n"));
            }
            items += 1;
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

    // The Registry is corpus-scoped, so it is collected once rather than per
    // notebook. A pending proposal is the one thing here that is actually
    // waiting on a human — it ranks with the errors.
    let cards = state.db.list_registry().await.unwrap_or_default();
    for card in &cards {
        let n = card
            .attachments
            .iter()
            .filter(|a| a.status == "proposed")
            .count();
        if n > 0 {
            attention.push_str(&format!(
                "- registry card \u{201c}{}\u{201d} ({}) has {n} document{} waiting to be confirmed or turned down\n",
                card.name,
                card.kind,
                if n == 1 { "" } else { "s" }
            ));
            items += 1;
        }
    }
    let filed: Vec<&crate::models::RegistryCard> = cards
        .iter()
        .filter(|c| {
            c.attachments
                .iter()
                .any(|a| a.status == "confirmed" && a.at > since)
        })
        .collect();
    if !filed.is_empty() {
        changed.push_str("- [Registry]\n");
        for card in filed {
            let n = card
                .attachments
                .iter()
                .filter(|a| a.status == "confirmed" && a.at > since)
                .count();
            changed.push_str(&format!(
                "  - {n} new document{} filed under \u{201c}{}\u{201d}\n",
                if n == 1 { "" } else { "s" },
                card.name
            ));
            items += 1;
        }
    }
    let mut context = String::new();
    // What the freshness queue produced, denominated in findings with the
    // token count as a footnote (freshness.rs). A quiet night contributes
    // nothing at all rather than a line saying so.
    if let Some(line) = crate::freshness::collect_findings(&state.db, since)
        .await
        .brief_line()
    {
        context.push_str(&format!("## Overnight\n{line}\n\n"));
    }
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
    let content = run_generation_chat(state, None, &messages, None)
        .await
        .map_err(|e| format!("{e:#}"))?;

    let note = persist_report_run(app, state, &schedule, existing, content).await?;

    // The audio edition rides behind the note, fire-and-forget: the text
    // brief is the deliverable, audio is the commute upgrade. Any failure
    // degrades to text, silently (RFC-brief §2.4).
    let app = app.clone();
    let spoken = note.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = synthesize_brief_audio(&app, &spoken).await {
            crate::note!("brief audio: {err:#}");
        }
    });
    Ok(note)
}

/// Markdown brief → a single-narrator HOST script for the existing Kokoro
/// pipeline. Deterministic — no model call: the brief is already written to
/// be read, so the audio edition is the brief, verbatim, with section
/// headers spoken as transitions and notebook tags spoken as "In X:".
fn brief_script(content: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for raw in content.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with("_Run ") {
            continue;
        }
        if let Some(header) = trimmed.strip_prefix("## ") {
            lines.push(format!("HOST: {header}."));
            continue;
        }
        let body = trimmed.trim_start_matches(['-', '*', ' ']).trim();
        if body.is_empty() {
            continue;
        }
        let spoken = match body.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
            Some((notebook, tail)) => format!(
                "In {notebook}: {}",
                tail.trim_start_matches([':', ' ', '\u{2014}', '-'])
            ),
            None => body.to_string(),
        };
        lines.push(format!("HOST: {spoken}"));
    }
    lines.join("\n")
}

/// Voice the brief on-device. Quietly a no-op when the Kokoro voices aren't
/// set up — the text edition stands alone.
async fn synthesize_brief_audio(app: &AppHandle, note: &Note) -> anyhow::Result<()> {
    let dir = kokoro_dir(app)?;
    if !crate::tts::kokoro_files_present(&dir) {
        return Ok(());
    }
    let script = brief_script(&note.content);
    if script.is_empty() {
        return Ok(());
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    synthesize_audio(app, &note.id, &script, &cancel).await?;
    // Open windows refresh their player; no window, no listener, no-op.
    let _ = app.emit("audio://ready", &note.id);
    Ok(())
}
