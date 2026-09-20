mod chunk;
mod commands;
mod gateway;
mod model;
mod parse;
mod results;
mod rules;
mod scan;
mod store;

use tauri::Manager;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("无法获取应用数据目录");
            std::fs::create_dir_all(&data_dir).ok();
            let workspace_root = data_dir.join("workspace");
            std::fs::create_dir_all(&workspace_root).ok();
            let db_path = data_dir.join("reviewer.db");
            app.manage(AppState {
                db_path,
                workspace_root,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_defaults,
            commands::scan_directory,
            commands::scan_uploads,
            commands::test_connection,
            commands::run_review,
            commands::save_task,
            commands::list_history,
            commands::get_history,
            commands::delete_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
