mod chunk;
mod commands;
mod config;
mod export;
mod gateway;
mod model;
mod parse;
mod results;
mod rules;
mod scan;
mod secrets;
mod store;

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::Manager;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir().expect("无法获取应用数据目录");
            std::fs::create_dir_all(&data_dir).ok();
            let workspace_root = data_dir.join("workspace");
            std::fs::create_dir_all(&workspace_root).ok();
            app.manage(AppState {
                db_path: data_dir.join("reviewer.db"),
                workspace_root,
                config_path: data_dir.join("config.json"),
                cancels: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_config,
            commands::save_settings,
            commands::has_api_key,
            commands::clear_api_key,
            commands::scan_directory,
            commands::scan_uploads,
            commands::scan_text,
            commands::test_connection,
            commands::cancel_review,
            commands::run_review,
            commands::summarize_system,
            commands::export_xlsx,
            commands::save_task,
            commands::list_history,
            commands::get_history,
            commands::delete_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
