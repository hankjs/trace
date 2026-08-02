mod acp;
mod commands;
mod llm_stream;
mod terminal;
mod tools;

use acp::AcpState;
use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let config_path = app_data_dir.join("acp_agents.json");
            let state = Arc::new(AcpState::new(config_path.to_string_lossy().to_string()));

            // Load config in background
            let state_clone = state.clone();
            tauri::async_runtime::spawn(async move {
                let _ = state_clone.load_config().await;
            });

            app.manage(state);
            app.manage(terminal::TermManager::default());

            // macOS 默认菜单的 Close Window (⌘W) 会抢走快捷键关闭整个窗口,
            // 移除它让 ⌘W 透传到前端用于关闭终端 pane
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::MenuItemKind;
                if let Some(menu) = app.menu() {
                    if let Ok(items) = menu.items() {
                        for item in items {
                            if let MenuItemKind::Submenu(sub) = item {
                                if let Ok(sub_items) = sub.items() {
                                    for si in sub_items {
                                        if let MenuItemKind::Predefined(p) = &si {
                                            if p.text().is_ok_and(|t| t == "Close Window") {
                                                let _ = sub.remove(&si);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::acp_new_session,
            commands::acp_prompt,
            commands::acp_cancel,
            commands::acp_stop,
            commands::acp_get_agents,
            commands::acp_add_agent,
            commands::acp_remove_agent,
            commands::acp_test_agent,
            tools::tool_read_file,
            tools::tool_grep,
            tools::tool_glob,
            tools::tool_write_file,
            tools::tool_edit,
            tools::tool_bash,
            tools::tool_read_file_base64,
            llm_stream::llm_stream,
            llm_stream::llm_stream_test,
            terminal::term_create,
            terminal::term_write,
            terminal::term_resize,
            terminal::term_close,
            terminal::term_read,
            terminal::term_list,
            terminal::term_foreground_cwd,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
