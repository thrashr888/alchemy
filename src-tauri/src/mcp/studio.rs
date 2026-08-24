//! Studio tools: generate documents, save templates, schedule reports —
//! the same registry every other surface uses (rag::ARTIFACT_KINDS + the
//! user's templates), operated from any terminal.

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

use super::*;
use crate::models::ReportSchedule;

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct GenerateReq {
    /// Notebook to generate from.
    notebook_id: String,
    /// A generator kind (see the error message for the list), "template:<id>"
    /// for one of the user's templates, or "custom" with instructions.
    kind: String,
    /// Extra instructions (required for "custom", optional otherwise).
    #[serde(default)]
    instructions: Option<String>,
    /// Optional provider override for THIS generation: the id of one of the
    /// user's configured providers (list them via provider errors — an
    /// unknown id fails fast naming the valid ones). Leave unset to use the
    /// app's configured Studio provider, which is almost always right; set it
    /// when that provider is failing (out of credit, offline) or the user
    /// asked for a specific one.
    #[serde(default)]
    provider: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SaveTemplateReq {
    /// Template name (shown on its Studio tile and in report pickers).
    name: String,
    /// One-line description of what it produces.
    #[serde(default)]
    description: Option<String>,
    /// The reusable generation instruction.
    prompt: String,
    /// Existing template id to update; omit to create a new one.
    #[serde(default)]
    template_id: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CommissionReq {
    /// Notebook the job runs in (from list_notebooks).
    notebook_id: String,
    /// Short name for the job, shown in Tonight and on its receipt.
    name: String,
    /// Generator kind, a template (template:<id> or name), or "custom".
    kind: String,
    /// What to do, required when kind is "custom".
    #[serde(default)]
    prompt: Option<String>,
    /// "tonight" (default, 2 AM local) or "now" (the next scheduler pass).
    #[serde(default)]
    when: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ScheduleReportReq {
    /// Notebook the report runs against.
    notebook_id: String,
    /// Report name (also the title prefix of each run's note).
    name: String,
    /// A generator kind, "template:<id>", a template's name, or "custom"
    /// with a prompt.
    kind: String,
    /// "hourly", "daily", or "weekly".
    interval: String,
    /// What the report should cover (required for "custom").
    #[serde(default)]
    prompt: Option<String>,
    /// "interval" (default — the clock fires it) or "change" (a standing
    /// question: it runs when sources in the notebook change, with the
    /// interval as the minimum time between runs).
    #[serde(default)]
    trigger: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ScheduleIdReq {
    /// Schedule id (from list_schedules).
    schedule_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TemplateIdReq {
    /// Template id (from list_templates).
    template_id: String,
}

#[tool_router(router = studio_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "List a notebook's scheduled reports (id, name, kind, interval, enabled, last run). Every create deserves a way to inspect and undo — clean up schedules you created that the user didn't ask to keep."
    )]
    async fn list_schedules(
        &self,
        Parameters(NotebookIdReq { notebook_id }): Parameters<NotebookIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let schedules = self
            .state()
            .db
            .list_report_schedules(&notebook_id)
            .await
            .map_err(internal)?;
        json_result(&schedules)
    }

    #[tool(
        description = "Delete a scheduled report. The notes its past runs produced stay; only the recurring schedule goes."
    )]
    async fn delete_schedule(
        &self,
        Parameters(ScheduleIdReq { schedule_id }): Parameters<ScheduleIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        // Resolve the notebook before deleting so the change event can name
        // it — the Reports panel only refreshes for the open notebook.
        let notebook_id = state
            .db
            .list_notebooks()
            .await
            .map_err(internal)?
            .into_iter()
            .map(|nb| nb.id)
            .collect::<Vec<_>>();
        let mut owner = None;
        for nb in &notebook_id {
            if let Ok(scheds) = state.db.list_report_schedules(nb).await {
                if scheds.iter().any(|s| s.id == schedule_id) {
                    owner = Some(nb.clone());
                    break;
                }
            }
        }
        state
            .db
            .delete_report_schedule(&schedule_id)
            .await
            .map_err(internal)?;
        self.changed("reports", owner.as_deref());
        json_result(&serde_json::json!({ "ok": true }))
    }

    #[tool(
        description = "Delete a generator template. Report schedules that reference it will error on their next run naming the missing template — check list_schedules first."
    )]
    async fn delete_template(
        &self,
        Parameters(TemplateIdReq { template_id }): Parameters<TemplateIdReq>,
    ) -> Result<CallToolResult, McpError> {
        crate::templates::delete_template(template_id).map_err(invalid)?;
        self.changed("templates", None);
        json_result(&serde_json::json!({ "ok": true }))
    }

    #[tool(
        description = "Start generating a document (summary, briefing, RFC, slide deck, one of the user's templates via template:<id>, or custom with instructions) from a notebook's sources. Returns the placeholder note IMMEDIATELY with status \"generating\" — generation takes seconds to minutes, so poll get_note until status is \"\" (done) or \"error\" (the content then holds the reason). List available templates with list_templates from the Studio group, or use a kind from the error message's list."
    )]
    async fn generate(
        &self,
        Parameters(GenerateReq {
            notebook_id,
            kind,
            instructions,
            provider,
        }): Parameters<GenerateReq>,
    ) -> Result<CallToolResult, McpError> {
        let prompt = instructions.unwrap_or_default();
        let note =
            commands::start_generation_detached(&self.app, &notebook_id, &kind, &prompt, provider)
                .await
                .map_err(|e| invalid(format!("{e:#}")))?;
        self.changed("notes", Some(&notebook_id));
        json_result(&note)
    }

    #[tool(
        description = "List the user's generator templates (id, name, description, prompt). Templates are reusable generation instructions; run one with generate kind:\"template:<id>\" or schedule it as a recurring report."
    )]
    async fn list_templates(&self) -> Result<CallToolResult, McpError> {
        let templates = crate::templates::list_templates().map_err(invalid)?;
        json_result(&templates)
    }

    #[tool(
        description = "Create or update a reusable generator template. The prompt is the generation instruction the template will run with; write it to stand alone (the sources are appended after it). Pass template_id to update an existing template in place."
    )]
    async fn save_template(
        &self,
        Parameters(SaveTemplateReq {
            name,
            description,
            prompt,
            template_id,
        }): Parameters<SaveTemplateReq>,
    ) -> Result<CallToolResult, McpError> {
        if prompt.trim().is_empty() {
            return Err(invalid("prompt is empty — a template is its prompt"));
        }
        let t = crate::templates::save_template(
            template_id,
            name,
            description.unwrap_or_default(),
            prompt,
        )
        .map_err(invalid)?;
        self.changed("templates", None);
        json_result(&t)
    }

    #[tool(
        description = "Hand ONE job to the Night Shift instead of running it now: a deep read, a rebuild, a re-gist, any generator kind or \"custom\" with a prompt. It runs unattended (default: tonight at 2 AM local; pass when=\"now\" for the next scheduler pass), writes its result as a note, and retires itself. Use this for work too slow to wait on. It writes notes and reports only — it never acts outward."
    )]
    async fn commission_run(
        &self,
        Parameters(CommissionReq {
            notebook_id,
            name,
            kind,
            prompt,
            when,
        }): Parameters<CommissionReq>,
    ) -> Result<CallToolResult, McpError> {
        let prompt = prompt.unwrap_or_default();
        let kind = commands::resolve_report_kind(&kind, &prompt).map_err(invalid)?;
        let name = if name.trim().is_empty() {
            "Commissioned run".to_string()
        } else {
            name.trim().to_string()
        };
        let not_before = match when.as_deref() {
            Some("now") => 0,
            _ => crate::scheduler::next_local_hour_ms(2),
        };
        let schedule = ReportSchedule {
            id: commands::new_id(),
            notebook_id,
            name,
            kind,
            prompt,
            trigger: "once".into(),
            not_before,
            interval_secs: 86_400,
            enabled: true,
            last_run_at: 0,
            created_at: commands::now(),
        };
        self.state()
            .db
            .add_report_schedule(&schedule)
            .await
            .map_err(|e| invalid(format!("{e:#}")))?;
        json_result(&schedule)
    }

    #[tool(
        description = "Schedule a recurring report in a notebook: any generator kind, one of the user's templates (by template:<id> or name), \"custom\" with a prompt, or \"brief\" — the cross-notebook morning brief (reads across ALL notebooks, ranked needs-you → changed → record; schedule it in the \"Briefs\" notebook). Interval is hourly, daily, or weekly. Each run refreshes URL sources first, then writes a timestamped note the user sees in Studio → Reports."
    )]
    async fn schedule_report(
        &self,
        Parameters(ScheduleReportReq {
            notebook_id,
            name,
            kind,
            interval,
            prompt,
            trigger,
        }): Parameters<ScheduleReportReq>,
    ) -> Result<CallToolResult, McpError> {
        let prompt = prompt.unwrap_or_default();
        let trigger = match trigger.as_deref() {
            Some("change") => "change".to_string(),
            _ => "interval".to_string(),
        };
        // Same validator as the chat tool: registry kinds, live templates,
        // custom-with-prompt; anything else gets the message naming options.
        let kind = commands::resolve_report_kind(&kind, &prompt).map_err(invalid)?;
        let interval_secs = match interval.as_str() {
            "hourly" => 3_600,
            "daily" => 86_400,
            "weekly" => 604_800,
            other => {
                return Err(invalid(format!(
                    "interval must be hourly, daily, or weekly (got \"{other}\")"
                )))
            }
        };
        let name = if name.trim().is_empty() {
            "Scheduled report".to_string()
        } else {
            name.trim().to_string()
        };
        let schedule = ReportSchedule {
            id: commands::new_id(),
            notebook_id: notebook_id.clone(),
            name,
            kind,
            prompt,
            trigger,
            not_before: 0,
            interval_secs,
            enabled: true,
            last_run_at: 0,
            created_at: commands::now(),
        };
        self.state()
            .db
            .add_report_schedule(&schedule)
            .await
            .map_err(internal)?;
        self.changed("reports", Some(&notebook_id));
        json_result(&schedule)
    }

    #[tool(
        description = "Second Look: claim-by-claim verification of a draft against the notebook, searched fresh (docs/RFC-second-look.md). Pass a note_id to check an existing note, or text (with a title) to check a draft before filing it. Each claim is re-retrieved with hybrid search — excluding the draft itself — and judged supported/weak/unsupported/contradicted by a different engine than the author. Writes a verdict report note and returns the structured verdicts."
    )]
    async fn second_look(
        &self,
        Parameters(SecondLookReq {
            notebook_id,
            note_id,
            title,
            text,
        }): Parameters<SecondLookReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let (title, text, exclude) = match (note_id, text) {
            (Some(id), _) => {
                let Some(note) = state.db.get_note(&id).await.map_err(internal)? else {
                    return Err(invalid("no note with that id"));
                };
                (note.title.clone(), note.content.clone(), Some(note.id))
            }
            (None, Some(text)) => (title.unwrap_or_else(|| "draft".into()), text, None),
            (None, None) => return Err(invalid("pass note_id or text")),
        };
        let (report, verdicts) =
            commands::second_look_pass(&state, &notebook_id, exclude.as_deref(), &title, &text)
                .await
                .map_err(internal)?;
        self.changed("notes", Some(&notebook_id));
        json_result(&serde_json::json!({
            "reportNoteId": report.id,
            "summary": commands::count_line(&verdicts),
            "verdicts": verdicts,
        }))
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SecondLookReq {
    /// Notebook whose corpus verifies the draft.
    notebook_id: String,
    /// Existing note to check (mutually exclusive with text).
    #[serde(default)]
    note_id: Option<String>,
    /// Title for a raw-text draft.
    #[serde(default)]
    title: Option<String>,
    /// Raw draft text to check without filing it as a note first.
    #[serde(default)]
    text: Option<String>,
}
