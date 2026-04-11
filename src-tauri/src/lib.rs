mod commands;
mod models;
mod services;
mod store;

/// Run in stdio MCP server mode (no GUI, no Tauri).
pub fn run_mcp_server() -> std::io::Result<()> {
    services::mcp_server::run()
}

/// Run as a Claude Code UserPromptSubmit hook.
/// Reads a JSON event from stdin, writes memory context to stdout.
pub fn run_hook() -> Result<(), String> {
    services::hook::run()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(e) = store::init() {
        eprintln!("Failed to init store: {}", e);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::autopilot::get_bootstrap_status,
            commands::autopilot::run_first_time_setup,
            commands::autopilot::store_list_memories,
            commands::autopilot::store_list_memories_by_topic,
            commands::autopilot::fetch_memory,
            commands::autopilot::search_memories_fts,
            commands::autopilot::list_topics,
            commands::autopilot::store_add_memory,
            commands::autopilot::store_update_memory,
            commands::autopilot::store_delete_memory,
            commands::autopilot::register_mcp_server,
            commands::autopilot::unregister_mcp_server,
            commands::autopilot::get_mcp_server_status,
            commands::autopilot::run_organize_pass,
            commands::autopilot::run_consolidate_topics,
            commands::autopilot::undo_last_organize,
            commands::autopilot::list_history,
            commands::autopilot::get_auto_organize,
            commands::autopilot::set_auto_organize,
            commands::autopilot::get_custom_db_dir,
            commands::autopilot::set_custom_db_dir,
            commands::autopilot::get_hook_status,
            commands::autopilot::install_hook,
            commands::autopilot::uninstall_hook,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
