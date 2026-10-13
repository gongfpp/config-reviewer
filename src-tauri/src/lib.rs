mod chunk;
mod commands;
mod config;
mod export;
mod gateway;
mod git_sync;
mod model;
mod parse;
mod results;
mod rules;
mod scan;
mod scheduler;
mod secrets;
mod store;

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            #[cfg(debug_assertions)]
            let data_dir = config::qa_data_dir().unwrap_or(data_dir);
            std::fs::create_dir_all(&data_dir)?;
            let workspace_root = data_dir.join("workspace");
            // 启动时清理上次会话残留的工作区。
            let _ = std::fs::remove_dir_all(&workspace_root);
            let _ = std::fs::remove_dir_all(data_dir.join("scheduled-workspace"));
            std::fs::create_dir_all(&workspace_root)?;
            app.manage(scheduler::SchedulerState::load(
                &data_dir.join("reviewer.db"),
            ));
            app.manage(AppState {
                db_path: data_dir.join("reviewer.db"),
                workspace_root,
                config_path: data_dir.join("config.json"),
                cancels: Mutex::new(HashMap::new()),
                shutting_down: std::sync::atomic::AtomicBool::new(false),
            });
            let show = MenuItem::with_id(app, "show", "显示配置评审台", true, None::<&str>)?;
            let quit =
                MenuItem::with_id(app, "quit", "退出程序（停止定时任务）", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("reviewer")
                .tooltip("配置评审台 · 关闭窗口后定时任务继续运行")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => scheduler::request_exit(app),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            scheduler::start(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_config,
            commands::load_defaults,
            commands::save_settings,
            commands::has_api_key,
            commands::clear_api_key,
            commands::scan_directory,
            commands::scan_uploads,
            commands::scan_text,
            commands::cancel_scan,
            commands::test_connection,
            commands::cancel_review,
            commands::run_review,
            commands::summarize_system,
            commands::export_xlsx,
            commands::export_text,
            commands::save_task,
            commands::list_history,
            commands::get_history,
            commands::delete_history,
            scheduler::list_schedules,
            scheduler::save_schedule,
            scheduler::delete_schedule,
            scheduler::run_schedule_now,
            scheduler::cancel_schedule,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested {
                api, code: None, ..
            } = event
            {
                api.prevent_exit();
                scheduler::request_exit(app);
            }
        });
}

#[cfg(test)]
mod tests;
