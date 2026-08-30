mod acp;
mod activity;
mod agent;
mod ai;
mod backup;
mod capture;
mod clip;
mod commands;
mod connectors;
mod db;
mod diagnostics;
#[cfg(target_os = "macos")]
mod dockmenu;
#[cfg(target_os = "macos")]
mod dragout;
mod examples;
mod export;
mod filesearch;
mod freshness;
mod gist;
mod git;
mod graph;
mod grepsearch;
mod growth;
mod hygiene;
mod inference;
mod ingest;
mod integrations;
mod mac;
mod mcp;
mod menu;
mod models;
mod notion;
mod outline;
mod pdf;
mod pptx;
mod rag;
mod router;
mod scheduler;
mod selfheal;
#[cfg(target_os = "macos")]
mod services;
#[cfg(target_os = "macos")]
mod spotlight;
mod templates;
mod textsize;
mod trace;
mod tts;
mod verify;

#[cfg(test)]
mod beir_eval;
#[cfg(test)]
mod evals;
#[cfg(test)]
mod fidelity;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod judged_eval;
#[cfg(test)]
mod perf_budgets;
#[cfg(test)]
mod retrieval_eval;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use commands::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before anything else: a panic in setup used to be a silent bounce in
    // the Dock. From here on it lands in ~/Library/Logs (docs/RFC-diagnostics.md).
    diagnostics::install_panic_hook();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // Window geometry survives relaunch. Pop-outs share one state slot
        // per kind (workspace / note reader) via label mapping; render-only
        // windows (export, capture) are denylisted. VISIBLE stays excluded:
        // close-to-tray hides the main window, and saving visible=false
        // would relaunch the app with no window at all.
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED
                        | tauri_plugin_window_state::StateFlags::FULLSCREEN,
                )
                .map_label(|label| {
                    if label.starts_with("win-export-") || label.starts_with("capture-") {
                        "ephemeral"
                    } else if label.starts_with("win-note-") {
                        "note"
                    } else if label.starts_with("win-") {
                        "workspace"
                    } else {
                        label
                    }
                })
                .with_denylist(&["ephemeral"])
                .build(),
        )
        .plugin(tauri_plugin_liquid_glass::init());

    #[cfg(feature = "debug")]
    let builder = builder.plugin(tauri_plugin_debug_bridge::init());

    builder
        .on_window_event(|window, event| match event {
            // Close-to-tray (docs/RFC-night-shift.md): with the menu bar icon
            // on, closing the main window hides it and Alchemy stays resident
            // — the scheduler keeps running. Tray off = close quits as before.
            // Child windows (notes, mind maps) close normally either way.
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if window.label() != "main" {
                    return;
                }
                let app = window.app_handle();
                let resident = app
                    .try_state::<commands::AppState>()
                    .and_then(|s| s.ai.try_read().ok().map(|ai| ai.config().tray_enabled))
                    .unwrap_or(true);
                if resident {
                    api.prevent_close();
                    let _ = window.hide();
                    scheduler::first_close_notice(app);
                }
            }
            // Evict per-window glass memos when a window is destroyed so a
            // recreated window with the same label re-applies from scratch.
            tauri::WindowEvent::Destroyed => {
                if let Some(state) = window.app_handle().try_state::<commands::AppState>() {
                    state.glass_applied.lock().unwrap().remove(window.label());
                }
            }
            // Refocusing Alchemy is the moment the accessibility text size can
            // have just changed — the user came back from System Settings.
            // Re-query and broadcast to every window if it actually moved.
            tauri::WindowEvent::Focused(true) => {
                textsize::publish_if_changed(window.app_handle());
            }
            _ => {}
        })
        .setup(|app| {
            // Hand background sweeps their announce channel before anything
            // can spawn one (commands::notify_changed).
            commands::set_app_handle(app.handle().clone());
            // Point the diagnostics log at the app log dir and open the
            // session. Everything below can now fail out loud.
            diagnostics::init(&app.handle().clone());
            let data_dir = match app.path().app_data_dir() {
                Ok(dir) => dir,
                Err(err) => diagnostics::fatal_startup("could not resolve the app data dir", &err),
            };
            if let Err(err) = std::fs::create_dir_all(&data_dir) {
                diagnostics::fatal_startup("could not create the app data dir", &err);
            }
            // Boot-phase stamps -> traces/startup.jsonl (docs/RFC-professional-grade.md
            // Pillar 2). See trace::Startup for exactly what the clock covers.
            let startup = trace::Startup::begin(data_dir.join("traces"));

            let db_dir = data_dir.join("lancedb");
            let config_path = data_dir.join("ai_config.json");
            let stats_path = data_dir.join("model_stats.json");

            let mut config = std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|s| serde_json::from_str::<ai::AiConfig>(&s).ok())
                .unwrap_or_default();
            // Legacy flat configs become provider lists; flat fields stay
            // mirrored for the call sites that key off them.
            config.normalize();

            let model_stats = std::fs::read_to_string(&stats_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();

            // The one startup failure users actually hit: a database left
            // half-written by a hard kill, or a schema a downgraded binary
            // can't read. Dying here with no window and no message is the
            // worst possible answer — say what happened and where to look.
            // Data trust (docs/RFC-night-shift-area.md §7). Three guards, in
            // the order they can save the library:
            //
            // 1. A stamp from a NEWER Alchemy means this binary would read
            //    columns it has never heard of. Refuse with a sentence the
            //    user can act on instead of a Lance panic.
            if let Some(stamp) = backup::check_store_version(&data_dir) {
                diagnostics::fatal_startup(
                    "this library was written by a newer version of Alchemy",
                    &format!(
                        "the store is at version {stamp}, this build reads version {}. Update Alchemy to open it.",
                        backup::STORE_VERSION
                    ),
                );
            }
            // 2. Clone the store aside before a migration touches it, so an
            //    upgrade is rehearsable and a downgrade has somewhere to go.
            //    A store already at this version has nothing to rehearse.
            if db_dir.exists() && backup::read_stamp(&data_dir) < backup::STORE_VERSION {
                match backup::snapshot_pre_migrate(&data_dir, env!("CARGO_PKG_VERSION")) {
                    Ok(path) => note!("pre-migration snapshot at {}", path.display()),
                    Err(err) => {
                        // Not fatal: a first run has no store to copy, and a
                        // full disk should not block the app from opening.
                        diagnostics::error(
                            "backup",
                            format!("pre-migration snapshot skipped: {err:#}"),
                        )
                    }
                }
            }

            let db = match tauri::async_runtime::block_on(db::Db::open(&db_dir)) {
                Ok(db) => db,
                Err(err) => diagnostics::fatal_startup("could not open the database", &err),
            };
            // 3. The store opened and migrated cleanly — stamp it, so an
            //    older binary meeting it later gets guard 1 instead of a panic.
            backup::write_stamp(&data_dir);
            // Db::open both connects and ensures every table, so the two phases
            // the RFC names collapse into this one stamp.
            startup.stamp("db_open");

            // App menu, built exactly once (rebuilding would clear AppKit's
            // auto-managed Window list). Open Recent mutates in place later.
            let recents: Vec<(String, String)> =
                tauri::async_runtime::block_on(db.list_notebooks())
                    .map(|nbs| nbs.into_iter().map(|n| (n.id, n.title)).collect())
                    .unwrap_or_default();
            // First read back through the ensured tables — the honest "tables
            // are readable" signal, not just "the directory opened".
            startup.stamp("tables_ready");
            let handles = menu::build(&app.handle().clone(), &recents)?;
            app.set_menu(handles.menu)?;
            // Deep links, tray, global hotkey (docs/RFC-macos-integrations.md).
            integrations::setup(app, &recents, config.tray_enabled)?;
            #[cfg(target_os = "macos")]
            services::setup(app);
            #[cfg(target_os = "macos")]
            {
                // Seed the Dock's list from the same recents the app menu was
                // just built with; rebuild_app_menu keeps it current after.
                dockmenu::set_recents(&recents);
                dockmenu::setup(app);
            }
            // Only after set_menu does the NSMenu exist — now AppKit can be
            // told this is the windows menu and start listing open windows.
            #[cfg(target_os = "macos")]
            handles.window.set_as_windows_menu_for_nsapp()?;
            app.manage(menu::RecentMenu(handles.recent));
            // View menu's per-view toggle groups; the frontend flips these
            // with `set_menu_context` as it enters and leaves a notebook.
            app.manage(handles.context);
            app.manage(menu::ThemeMenu(handles.themes));
            app.manage(menu::GenerateMenu(handles.generate));

            let runtime = commands::ai_runtime(app.handle().clone(), data_dir.clone());
            let (mcp_enabled, mcp_port) = (config.mcp_enabled, config.mcp_port);
            let (clip_enabled, clip_port) = (config.clip_enabled, config.clip_port);
            app.manage(AppState {
                db: Arc::new(db),
                ai: tokio::sync::RwLock::new(ai::Ai::new(config, runtime)),
                config_path,
                stats_path,
                trace_dir: data_dir.join("traces"),
                model_stats: std::sync::Mutex::new(model_stats),
                cancel: std::sync::Mutex::new(std::collections::HashMap::new()),
                folder_scan_lock: tokio::sync::Mutex::new(()),
                glass_applied: std::sync::Mutex::new(std::collections::HashMap::new()),
            });

            // Studio templates: write the default pack on first run so
            // ~/Documents/Alchemy/templates exists before anything lists it.
            templates::seed_on_startup(&data_dir);

            // Rendered page capture needs an app handle to open its hidden
            // webview windows (docs/RFC-page-capture.md).
            capture::init(app.handle().clone(), data_dir.clone());

            // Spotlight needs AppState (it reads the db to build the index).
            #[cfg(target_os = "macos")]
            spotlight::setup(app);

            // Agent access: embedded MCP server (see docs/RFC-mcp-server.md).
            app.manage(mcp::McpState::default());
            app.manage(acp::AcpState::default());
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                mcp::apply_config(&handle, mcp_enabled, mcp_port).await;
            });

            // Skills the user connected to their agent clients are ours to
            // keep current — Connect writes them once, and a release that
            // changes the skill would otherwise leave every agent on the
            // machine reading last year's copy. Blocking pool: a dozen small
            // file comparisons that must not sit on the setup thread.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                connectors::refresh_installed_skills(&handle);
            });

            // Browser-extension clip receiver (docs/RFC-page-capture.md §8).
            app.manage(clip::ClipState::default());
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                clip::apply_config(&handle, clip_enabled, clip_port).await;
            });

            // The Night Shift's resident scheduler (docs/RFC-night-shift.md):
            // source resync + due report runs, window or no window.
            scheduler::start(app.handle().clone());
            startup.stamp("scheduler_up");

            // Fusion follows the embedder tier (BEIR-measured; db.rs): the
            // built-in leg fuses at 0.25, nomic-class at full weight.
            {
                let state = app.state::<AppState>();
                let f =
                    tauri::async_runtime::block_on(async { state.ai.read().await.fusion_params() });
                state.db.set_fusion(f);
            }

            // Imports stranded mid-embed by a quit or crash restart from
            // their stored content (docs/RFC-import-pipeline.md §2).
            commands::resume_stranded_imports(&app.handle().clone());

            // Debounced BM25 rebuilds: chunk writers mark the FTS index
            // dirty and nudge this task; one Tantivy rebuild lands ~2s
            // after the last write of a burst, instead of one whole-corpus
            // rebuild inline per write.
            {
                let db = app.state::<AppState>().db.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        db.fts_write_notified().await;
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        if let Err(err) = db.flush_fts().await {
                            crate::note!("fts flush: {err:#}");
                        }
                    }
                });
            }

            // Database housekeeping: compact fragments, prune dead versions.
            // Lance never cleans up after itself, and an unpruned install
            // grows to gigabytes of stale FTS indices. Delayed so launch
            // stays snappy.
            {
                let db = app.state::<AppState>().db.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    match db.maintain().await {
                        Ok((bytes, versions)) if versions > 0 => crate::note!(
                            "db maintenance: pruned {versions} old versions, reclaimed {} MB",
                            bytes / (1024 * 1024)
                        ),
                        Ok(_) => {}
                        Err(err) => crate::note!("db maintenance failed: {err:#}"),
                    }
                });
            }

            // Seed the current accessibility text scale so the first window
            // focus doesn't spuriously broadcast; the frontend reads it at boot
            // via get_system_text_scale, and window-focus republishes changes.
            textsize::prime();

            // Last backend phase: state is managed, background tasks are
            // spawned, and the webview has been loading in parallel.
            startup.stamp("setup_done");
            Ok(())
        })
        .on_menu_event(|app, event| menu::handle_event(app, event.id().0.as_str()))
        .invoke_handler(tauri::generate_handler![
            commands::log_client_error,
            commands::recent_errors,
            commands::pending_fatal,
            commands::reveal_log,
            #[cfg(target_os = "macos")]
            dragout::start_file_drag,
            #[cfg(target_os = "macos")]
            dragout::stage_note_for_drag,
            commands::list_notebooks,
            commands::create_notebook,
            commands::rename_notebook,
            commands::set_notebook_color,
            commands::set_notebook_icon,
            commands::set_notebook_status,
            commands::delete_notebook,
            commands::list_sources,
            commands::add_source_file,
            commands::add_source_folder,
            commands::list_cloud_folders,
            commands::search_mac_files,
            commands::add_source_mac,
            integrations::integrations_ready,
            integrations::locate_note,
            commands::mac_note_body,
            commands::update_mac_note,
            commands::add_mac_reminder,
            commands::complete_mac_reminder,
            mac::mac_available,
            mac::open_privacy_settings,
            mac::mac_connect,
            mac::list_mac_collections,
            commands::agent_cli_status,
            commands::provider_readiness,
            commands::provider_readiness_one,
            commands::resync_sources,
            commands::add_source_url,
            commands::add_source_text,
            commands::update_source_text,
            commands::set_source_tags,
            commands::set_source_note,
            commands::refresh_source_url,
            commands::refresh_sources,
            commands::get_source_content,
            commands::delete_sources,
            commands::set_sources_tags,
            commands::delete_notes,
            commands::source_hygiene,
            commands::hygiene_keep,
            commands::set_child_embedded,
            commands::reembed_all,
            commands::list_messages_page,
            commands::clear_chat,
            commands::add_note_to_chat,
            commands::send_message,
            commands::send_message_agentic,
            commands::cancel_generation,
            commands::open_in_terminal,
            commands::start_ollama,
            commands::suggest_notebook,
            commands::pdf_page_count,
            commands::pdf_page_image,
            commands::pdf_local_path,
            commands::notion_check,
            commands::delete_message,
            commands::list_notes,
            commands::activity_stats,
            commands::home_activity,
            commands::source_thumbnail,
            commands::source_snippets,
            commands::backfill_source_images,
            commands::export_notebook_okf,
            commands::fix_traffic_lights,
            commands::get_audio_path,
            commands::kokoro_status,
            commands::setup_kokoro,
            commands::remove_kokoro,
            commands::export_audio,
            export::export_note,
            commands::new_window,
            commands::print_webview,
            commands::set_window_glass,
            commands::source_backlinks,
            commands::notebook_graph,
            commands::related_passages,
            commands::live_view_open,
            commands::live_view_bounds,
            commands::live_view_visible,
            commands::live_view_close,
            commands::rebuild_app_menu,
            commands::fill_menu_lists,
            menu::set_menu_context,
            commands::list_shortcuts,
            commands::search_everything,
            commands::grep_sources,
            commands::export_notebook_okf_zip,
            commands::import_notebook_okf,
            commands::probe_okf,
            commands::ask_everything,
            commands::list_meta_threads,
            commands::list_meta_turns,
            commands::add_meta_turn,
            commands::delete_meta_thread,
            commands::create_note,
            commands::restore_note,
            commands::build_info,
            commands::release_history,
            commands::cited_source_ids,
            commands::seed_scale_fixture,
            commands::growth_proposals,
            commands::growth_web_search,
            commands::growth_local,
            commands::growth_retire,
            commands::generate_wiki_index,
            commands::live_view_back,
            commands::live_view_forward,
            commands::live_view_url,
            commands::update_note,
            commands::note_opened,
            commands::convert_note_to_source,
            commands::generate_artifact,
            commands::rebuild_note,
            commands::get_ai_config,
            commands::set_ai_config,
            commands::apply_settings_fix,
            commands::apply_connect_fix,
            commands::send_notification,
            commands::list_models,
            commands::check_ollama,
            commands::check_models,
            commands::list_gateway_models,
            commands::provider_models,
            commands::get_model_stats,
            commands::suggest_followups,
            commands::generate_epigraph,
            commands::generate_notebook_summary,
            commands::list_report_schedules,
            commands::list_source_events,
            commands::night_shift_status,
            commands::snapshot_status,
            commands::snapshot_now,
            commands::restore_snapshot,
            commands::toggle_night_shift_pause,
            commands::list_ledger,
            commands::add_ledger_entry,
            commands::update_ledger_entry,
            commands::delete_ledger_entry,
            commands::list_registry,
            commands::add_registry_card,
            commands::update_registry_card,
            commands::delete_registry_card,
            commands::attach_source_to_card,
            commands::set_attachment_status,
            commands::cards_for_source,
            commands::suggest_cards_now,
            commands::set_card_origin,
            commands::rule_all_suggested,
            commands::rematch_registry,
            commands::run_second_look,
            commands::create_report_schedule,
            commands::update_report_schedule,
            commands::delete_report_schedule,
            commands::run_report,
            templates::list_templates,
            templates::open_templates_folder,
            templates::install_default_templates,
            templates::save_template,
            templates::delete_template,
            mcp::mcp_status,
            acp::acp_agents,
            acp::acp_check,
            acp::acp_status,
            acp::acp_start,
            acp::acp_prompt,
            acp::acp_cancel,
            acp::acp_stop,
            acp::acp_permission,
            connectors::list_agent_connectors,
            connectors::connect_agent,
            textsize::get_system_text_scale,
            textsize::dump_text_size_signals,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // Residency (docs/RFC-night-shift.md): with the main window
            // hidden rather than destroyed this rarely fires, but child-
            // window-only exits and platform quirks land here. Explicit quit
            // paths (⌘Q, tray Quit) set QUIT_REQUESTED or exit with a code.
            tauri::RunEvent::ExitRequested { code, api, .. } => {
                let resident = app
                    .try_state::<commands::AppState>()
                    .and_then(|s| s.ai.try_read().ok().map(|ai| ai.config().tray_enabled))
                    .unwrap_or(true);
                if resident
                    && code.is_none()
                    && !scheduler::QUIT_REQUESTED.load(std::sync::atomic::Ordering::Relaxed)
                {
                    api.prevent_exit();
                }
            }
            // Dock icon click while the main window is hidden. (Verified
            // live: fires on real Dock clicks; the synthetic AppleScript
            // `reopen` event does NOT reach tao's delegate — don't let a
            // scripted test convince you this is broken.)
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => integrations::focus_main(app),
            _ => {}
        });
}
