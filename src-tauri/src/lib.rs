mod agent;
mod ai;
mod capture;
mod commands;
mod connectors;
mod db;
mod git;
mod grepsearch;
mod inference;
mod ingest;
mod integrations;
mod mac;
mod mcp;
mod menu;
mod models;
mod outline;
mod pdf;
mod rag;
mod router;
#[cfg(target_os = "macos")]
mod services;
#[cfg(target_os = "macos")]
mod spotlight;
mod templates;
mod trace;
mod tts;

#[cfg(test)]
mod evals;
#[cfg(test)]
mod retrieval_eval;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use commands::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_liquid_glass::init());

    #[cfg(feature = "debug")]
    let builder = builder.plugin(tauri_plugin_debug_bridge::init());

    builder
        // Evict per-window glass memos when a window is destroyed so a
        // recreated window with the same label re-applies from scratch.
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                use tauri::Manager;
                if let Some(state) = window.app_handle().try_state::<commands::AppState>() {
                    state.glass_applied.lock().unwrap().remove(window.label());
                }
            }
        })
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("could not resolve app data dir");
            std::fs::create_dir_all(&data_dir).ok();

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

            let db = tauri::async_runtime::block_on(db::Db::open(&db_dir))
                .expect("failed to open LanceDB");

            // App menu, built exactly once (rebuilding would clear AppKit's
            // auto-managed Window list). Open Recent mutates in place later.
            let recents: Vec<(String, String)> =
                tauri::async_runtime::block_on(db.list_notebooks())
                    .map(|nbs| nbs.into_iter().map(|n| (n.id, n.title)).collect())
                    .unwrap_or_default();
            let handles = menu::build(&app.handle().clone(), &recents)?;
            app.set_menu(handles.menu)?;
            // Deep links, tray, global hotkey (docs/RFC-macos-integrations.md).
            integrations::setup(app, &recents, config.tray_enabled)?;
            #[cfg(target_os = "macos")]
            services::setup(app);
            // Only after set_menu does the NSMenu exist — now AppKit can be
            // told this is the windows menu and start listing open windows.
            #[cfg(target_os = "macos")]
            handles.window.set_as_windows_menu_for_nsapp()?;
            app.manage(menu::RecentMenu(handles.recent));

            let runtime = commands::ai_runtime(app.handle().clone(), data_dir.clone());
            let (mcp_enabled, mcp_port) = (config.mcp_enabled, config.mcp_port);
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
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                mcp::apply_config(&handle, mcp_enabled, mcp_port).await;
            });
            Ok(())
        })
        .on_menu_event(|app, event| menu::handle_event(app, event.id().0.as_str()))
        .invoke_handler(tauri::generate_handler![
            commands::list_notebooks,
            commands::create_notebook,
            commands::rename_notebook,
            commands::set_notebook_color,
            commands::delete_notebook,
            commands::list_sources,
            commands::add_source_file,
            commands::add_source_folder,
            commands::add_source_mac,
            integrations::integrations_ready,
            integrations::locate_note,
            commands::mac_note_body,
            commands::update_mac_note,
            commands::add_mac_reminder,
            mac::mac_available,
            mac::open_privacy_settings,
            mac::mac_connect,
            mac::list_mac_collections,
            commands::agent_cli_status,
            commands::provider_readiness,
            commands::resync_sources,
            commands::add_source_url,
            commands::add_source_text,
            commands::update_source_text,
            commands::refresh_source_url,
            commands::get_source_content,
            commands::delete_source,
            commands::set_child_embedded,
            commands::reembed_all,
            commands::list_messages,
            commands::clear_chat,
            commands::add_note_to_chat,
            commands::send_message,
            commands::send_message_agentic,
            commands::cancel_generation,
            commands::open_in_terminal,
            commands::list_notes,
            commands::list_recent_notes,
            commands::list_recent_reports,
            commands::corpus_stats,
            commands::export_notebook_okf,
            commands::fix_traffic_lights,
            commands::get_audio_path,
            commands::kokoro_status,
            commands::setup_kokoro,
            commands::remove_kokoro,
            commands::export_audio,
            commands::new_window,
            commands::print_webview,
            commands::set_window_glass,
            commands::source_backlinks,
            commands::related_passages,
            commands::live_view_open,
            commands::live_view_bounds,
            commands::live_view_visible,
            commands::live_view_close,
            commands::rebuild_app_menu,
            commands::search_everything,
            commands::export_notebook_okf_zip,
            commands::import_notebook_okf,
            commands::probe_okf,
            commands::ask_everything,
            commands::create_note,
            commands::build_info,
            commands::update_note,
            commands::note_opened,
            commands::delete_note,
            commands::convert_note_to_source,
            commands::generate_artifact,
            commands::rebuild_note,
            commands::get_ai_config,
            commands::set_ai_config,
            commands::list_models,
            commands::check_ollama,
            commands::check_models,
            commands::list_gateway_models,
            commands::get_model_stats,
            commands::suggest_followups,
            commands::generate_epigraph,
            commands::generate_notebook_summary,
            commands::list_report_schedules,
            commands::list_all_report_schedules,
            commands::create_report_schedule,
            commands::update_report_schedule,
            commands::delete_report_schedule,
            commands::run_report,
            templates::list_templates,
            templates::open_templates_folder,
            templates::install_default_templates,
            mcp::mcp_status,
            connectors::list_agent_connectors,
            connectors::connect_agent,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
