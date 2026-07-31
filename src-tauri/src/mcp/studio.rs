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
}

#[tool_router(router = studio_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "Start generating a document (summary, briefing, RFC, slide deck, one of the user's templates via template:<id>, or custom with instructions) from a notebook's sources. Returns the placeholder note IMMEDIATELY with status \"generating\" — generation takes seconds to minutes, so poll get_note until status is \"\" (done) or \"error\" (the content then holds the reason). List available templates with list_templates from the Studio group, or use a kind from the error message's list."
    )]
    async fn generate(
        &self,
        Parameters(GenerateReq {
            notebook_id,
            kind,
            instructions,
        }): Parameters<GenerateReq>,
    ) -> Result<CallToolResult, McpError> {
        let prompt = instructions.unwrap_or_default();
        let note = commands::start_generation_detached(&self.app, &notebook_id, &kind, &prompt)
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
        description = "Schedule a recurring report in a notebook: any generator kind, one of the user's templates (by template:<id> or name), or \"custom\" with a prompt. Interval is hourly, daily, or weekly. Each run refreshes URL sources first, then writes a timestamped note the user sees in Studio → Reports."
    )]
    async fn schedule_report(
        &self,
        Parameters(ScheduleReportReq {
            notebook_id,
            name,
            kind,
            interval,
            prompt,
        }): Parameters<ScheduleReportReq>,
    ) -> Result<CallToolResult, McpError> {
        let prompt = prompt.unwrap_or_default();
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
}
