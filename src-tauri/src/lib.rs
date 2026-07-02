mod agent;
mod ai;
mod commands;
mod db;
mod ingest;
mod models;
mod pdf;
mod rag;

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
        .plugin(tauri_plugin_notification::init());

    #[cfg(feature = "debug")]
    let builder = builder.plugin(tauri_plugin_debug_bridge::init());

    builder
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("could not resolve app data dir");
            std::fs::create_dir_all(&data_dir).ok();

            let db_dir = data_dir.join("lancedb");
            let config_path = data_dir.join("ai_config.json");
            let stats_path = data_dir.join("model_stats.json");

            let config = std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|s| serde_json::from_str::<ai::AiConfig>(&s).ok())
                .unwrap_or_default();

            let model_stats = std::fs::read_to_string(&stats_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();

            let db = tauri::async_runtime::block_on(db::Db::open(&db_dir))
                .expect("failed to open LanceDB");

            app.manage(AppState {
                db: Arc::new(db),
                ai: tokio::sync::RwLock::new(ai::Ollama::new(config)),
                config_path,
                stats_path,
                model_stats: std::sync::Mutex::new(model_stats),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_notebooks,
            commands::create_notebook,
            commands::rename_notebook,
            commands::delete_notebook,
            commands::list_sources,
            commands::add_source_file,
            commands::add_source_url,
            commands::add_source_text,
            commands::update_source_text,
            commands::refresh_source_url,
            commands::get_source_content,
            commands::delete_source,
            commands::reembed_all,
            commands::list_messages,
            commands::clear_chat,
            commands::send_message,
            commands::send_message_agentic,
            commands::list_notes,
            commands::create_note,
            commands::update_note,
            commands::delete_note,
            commands::convert_note_to_source,
            commands::generate_artifact,
            commands::rebuild_note,
            commands::get_ai_config,
            commands::set_ai_config,
            commands::list_models,
            commands::check_ollama,
            commands::check_models,
            commands::get_model_stats,
            commands::suggest_followups,
            commands::generate_notebook_summary,
            commands::list_report_schedules,
            commands::list_all_report_schedules,
            commands::create_report_schedule,
            commands::update_report_schedule,
            commands::delete_report_schedule,
            commands::run_report,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
