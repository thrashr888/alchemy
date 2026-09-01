//! The `settings` tool (docs/RFC-self-resolve.md phase 3, grown by
//! docs/RFC-conversational-setup.md): get/set over the safe `AiConfig`
//! allowlist plus the onboarding verbs — models/test/pull, profile (via
//! set), style, theme, connect, and setup — same cores as the chat tool
//! router's versions. Secrets are refused on write and redacted out of
//! every read; `pull` only returns the validated command; `connect` is the
//! one verb that never applies without `confirm: true`.

use super::*;
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SettingsReq {
    /// One of: get, set, models, test, pull, style, theme, connect, setup.
    op: String,
    /// For set: chatProvider, studioProvider, chatModel, effort, baseUrl
    /// (bare fields target the active chat provider), smallModel, embedder,
    /// provider.<id>.chatModel / .effort / .baseUrl, or profile.name /
    /// profile.profession / profile.instructions / profile.assistantName.
    #[serde(default)]
    field: String,
    /// For set: the new value. Provider fields accept an id or a label.
    #[serde(default)]
    value: String,
    /// For test: a provider id/label or an Ollama model name (empty probes
    /// the active chat provider). For connect: the agent client to connect
    /// (empty lists the clients and their state).
    #[serde(default)]
    target: String,
    /// For pull: the Ollama model name to stage a download for.
    #[serde(default)]
    model: String,
    /// For style: which notebook's chat voice to change (styles are per
    /// notebook).
    #[serde(default)]
    notebook_id: String,
    /// For style: a style id or name (friendly, bffs, kids, professional, scientific,
    /// adhd, ste100, govuk, plain, gdev, learning, custom, default). Empty
    /// keeps the current style.
    #[serde(default)]
    style: String,
    /// For style: shorter | longer | default. Empty keeps the current length.
    #[serde(default)]
    length: String,
    /// For theme: a theme name or fuzzy description ("gruvbox", "the dark
    /// rust one"). Empty lists the roster.
    #[serde(default)]
    theme: String,
    /// For connect: must be true to actually write the client's config —
    /// connecting edits ANOTHER app's files, so it never applies without
    /// explicit confirmation.
    #[serde(default)]
    confirm: bool,
}

#[tool_router(router = settings_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "Read, change, or probe Alchemy's settings. op:\"get\" returns a redacted snapshot (API keys are never readable). op:\"set\" changes ONE field: chatProvider, studioProvider, chatModel, effort, baseUrl (these three target the active chat provider), smallModel, embedder (ollama|builtin), provider.<id>.chatModel / .effort / .baseUrl, or profile.name / profile.profession / profile.instructions / profile.assistantName (the user's persona — free text, never secrets); provider values accept an id or a label; API keys and tokens can never be read or set. op:\"models\" lists installed Ollama models plus every provider's active model and live readiness. op:\"test\" live-probes one provider or model (pass `target`; empty = active chat provider) with exactly one tiny chat call and, for Ollama targets, one embed call — no config change. op:\"pull\" validates an Ollama model name (pass `model`) and returns the `ollama pull` command string for the USER to run in Terminal — Alchemy never executes it. op:\"style\" sets a notebook's answer voice/length (pass `notebook_id` plus `style` and/or `length`). op:\"theme\" switches the app theme (pass `theme`; empty lists them). op:\"connect\" lists agent clients (empty `target`) or registers Alchemy with one — requires `confirm: true` because it writes that client's config file. op:\"setup\" reports the next unmet setup step. Applied changes take effect immediately and open windows refresh live."
    )]
    async fn settings(
        &self,
        Parameters(SettingsReq {
            op,
            field,
            value,
            target,
            model,
            notebook_id,
            style,
            length,
            theme,
            confirm,
        }): Parameters<SettingsReq>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let mut config = { state.ai.read().await.config().clone() };
        let text = |t: String| Ok(CallToolResult::success(vec![ContentBlock::text(t)]));
        match op.trim() {
            "get" => text(crate::selfheal::settings_get(&config)),
            "set" => {
                let echo =
                    crate::selfheal::settings_set(&mut config, &field, &value).map_err(invalid)?;
                commands::apply_ai_config(&self.app, &state, config)
                    .await
                    .map_err(internal)?;
                self.changed("settings", None);
                json_result(&serde_json::json!({ "ok": true, "applied": echo }))
            }
            "models" => text(commands::settings_models_report(&self.app, &state).await),
            "test" => text(commands::settings_test_report(&state, &target).await),
            "pull" => {
                // Validation only — the command comes back as text for the
                // user to run; this process never shells out.
                let command = crate::selfheal::pull_command(&model).map_err(invalid)?;
                json_result(&serde_json::json!({
                    "ok": true,
                    "command": command,
                    "note": "Run this in Terminal yourself — Alchemy stages the \
                             command but never executes it. Then call op \"test\" \
                             with the model name.",
                }))
            }
            "style" => {
                if notebook_id.trim().is_empty() {
                    return Err(invalid(
                        "styles are per notebook — pass notebook_id (from list_notebooks)",
                    ));
                }
                let reply = commands::settings_style_apply(
                    &self.app,
                    &state,
                    commands::StyleTarget::Notebook(notebook_id.trim()),
                    &style,
                    &length,
                )
                .await;
                json_result(&serde_json::json!({ "ok": true, "applied": reply }))
            }
            "theme" => text(commands::settings_theme_apply(&self.app, &theme)),
            "connect" => {
                if target.trim().is_empty() || !confirm {
                    // Read-only roster / the explicit-confirmation refusal.
                    let report = commands::settings_connect_report(&self.app, &target).await;
                    if target.trim().is_empty() {
                        return text(report);
                    }
                    // A named target without confirm never writes: answer
                    // with what WOULD happen and how to proceed.
                    let list = crate::connectors::list_agent_connectors(self.app.clone())
                        .await
                        .unwrap_or_default();
                    let t = target.trim().to_lowercase();
                    if let Some(c) = list
                        .iter()
                        .find(|c| c.id.to_lowercase() == t || c.name.to_lowercase() == t)
                        .filter(|c| c.installed && c.can_auto && !c.configured)
                    {
                        return Err(invalid(crate::selfheal::connect_refusal(
                            &c.name,
                            &c.config_path,
                        )));
                    }
                    return text(report);
                }
                let status = crate::connectors::connect_agent(self.app.clone(), target)
                    .await
                    .map_err(internal)?;
                json_result(&serde_json::json!({
                    "ok": true,
                    "applied": format!("Connected {} — wrote {}", status.name, status.config_path),
                    "file": status.config_path,
                    "restartNeeded": true,
                }))
            }
            "setup" => text(commands::settings_setup_report(&self.app, &state).await),
            other => Err(invalid(format!(
                "unknown op \"{other}\" — use get, set, models, test, pull, style, theme, \
                 connect, or setup"
            ))),
        }
    }

    #[tool(
        description = "App-wide usage statistics (docs/RFC-activity-view.md): totals for \
                       messages, sources, notes, and notebooks; a per-day activity series; \
                       active days and streaks; most-used models, most-active notebooks, and \
                       source-type breakdown. Read-only; the same numbers Settings → Activity \
                       shows. Retrieval counts cover retained trace history only (~months)."
    )]
    async fn activity_stats(&self) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let messages = state.db.message_activity().await.map_err(internal)?;
        let notes = state.db.note_activity().await.map_err(internal)?;
        let sources = state.db.source_activity().await.map_err(internal)?;
        let all_notebooks = state.db.list_notebooks().await.map_err(internal)?;
        // Same ranking rule the Activity pane uses — an agent asking "what am
        // I most active in" should not be told about shelved notebooks.
        let ranked_out: std::collections::HashSet<String> = all_notebooks
            .iter()
            .filter(|n| n.status == "archived" || n.status == "system")
            .map(|n| n.id.clone())
            .collect();
        let titles: std::collections::HashMap<String, String> =
            all_notebooks.into_iter().map(|n| (n.id, n.title)).collect();
        let retrievals = crate::activity::trace_times(&state.trace_dir);
        json_result(&crate::activity::aggregate(
            &messages,
            &notes,
            &sources,
            &titles,
            &ranked_out,
            &retrievals,
            chrono::Local::now().date_naive(),
        ))
    }
}
