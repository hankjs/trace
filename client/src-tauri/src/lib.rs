mod acp;
mod commands;
mod llm_stream;
mod terminal;
mod tools;

use std::sync::Arc;
use acp::AcpState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let config_path = app_data_dir.join("acp_agents.json");
            let state = Arc::new(AcpState::new(
                config_path.to_string_lossy().to_string(),
            ));

            // Load config in background
            let state_clone = state.clone();
            tauri::async_runtime::spawn(async move {
                let _ = state_clone.load_config().await;
            });

            app.manage(state);
            app.manage(terminal::TermManager::default());
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
            llm_stream::llm_stream,
            llm_stream::llm_stream_test,
            terminal::term_create,
            terminal::term_write,
            terminal::term_resize,
            terminal::term_close,
            terminal::term_read,
            terminal::term_list,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
