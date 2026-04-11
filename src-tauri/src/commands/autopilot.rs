use serde::Serialize;
use std::process::Command;

use crate::services::{bootstrap, ingestion, organizer};
use crate::store::{history, memories, settings, topics};

const MCP_SERVER_NAME: &str = "claude-memory-manager";
const SETTING_AUTO_ORGANIZE: &str = "auto_organize";
const SETTING_CUSTOM_DB_DIR: &str = "custom_db_dir";
const SETTING_HOOK_ENABLED: &str = "hook_enabled";

#[derive(Serialize)]
pub struct ConfigDirRegistration {
    pub label: String,
    pub path: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct SetupResult {
    pub bootstrap: bootstrap::BootstrapStatus,
    pub ingestion: ingestion::IngestionReport,
    pub mcp_registrations: Vec<ConfigDirRegistration>,
}

#[tauri::command]
pub fn get_bootstrap_status() -> Result<bootstrap::BootstrapStatus, String> {
    bootstrap::get_status()
}

#[tauri::command]
pub fn run_first_time_setup() -> Result<SetupResult, String> {
    // Step 1: Write the managed section to every CLAUDE.md we can find
    bootstrap::ensure_claude_md_all()?;

    // Step 2: Ingest existing memory files from all configs
    let report = ingestion::ingest_existing_files()?;

    // Step 3: Register the MCP server in every config dir (best-effort)
    let mcp_registrations = register_in_all_configs();

    let status = bootstrap::get_status()?;
    Ok(SetupResult {
        bootstrap: status,
        ingestion: report,
        mcp_registrations,
    })
}

/// Register the MCP server in every detected Claude config directory.
fn register_in_all_configs() -> Vec<ConfigDirRegistration> {
    let dirs = bootstrap::list_claude_config_dirs();
    dirs.into_iter()
        .map(|(label, dir)| {
            let path_str = dir.to_string_lossy().to_string();
            match register_mcp_in_dir(&dir) {
                Ok(_) => ConfigDirRegistration {
                    label,
                    path: path_str,
                    success: true,
                    error: None,
                },
                Err(e) => ConfigDirRegistration {
                    label,
                    path: path_str,
                    success: false,
                    error: Some(e),
                },
            }
        })
        .collect()
}

/// Register the MCP server in a specific config dir by setting CLAUDE_CONFIG_DIR
/// when invoking the claude CLI, AND writing to that dir's settings.json.
/// Also installs the UserPromptSubmit hook for deterministic memory retrieval.
fn register_mcp_in_dir(config_dir: &std::path::Path) -> Result<(), String> {
    let binary = std::env::current_exe()
        .map_err(|e| format!("Failed to get binary path: {}", e))?
        .to_string_lossy()
        .to_string();

    // Remove existing (ignore errors)
    let _ = Command::new("claude")
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .args(["mcp", "remove", MCP_SERVER_NAME, "--scope", "user"])
        .output();

    let mut add_args = vec![
        "mcp".to_string(),
        "add".to_string(),
        MCP_SERVER_NAME.to_string(),
        "--scope".to_string(),
        "user".to_string(),
    ];

    // If user has set a custom DB dir, pass it via --env so the MCP server
    // child process inherits it. This is the WithSecure XFENCE escape hatch.
    let custom_db = settings::get(SETTING_CUSTOM_DB_DIR, "").unwrap_or_default();
    if !custom_db.is_empty() {
        add_args.push("--env".to_string());
        add_args.push(format!("CLAUDE_MEMORY_DB_DIR={}", custom_db));
    }

    add_args.push("--".to_string());
    add_args.push(binary.clone());
    add_args.push("--mcp-server".to_string());

    let output = Command::new("claude")
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .args(&add_args)
        .output()
        .map_err(|e| format!("Failed to run 'claude mcp add' for {}: {}", config_dir.display(), e))?;

    if !output.status.success() {
        return Err(format!(
            "claude mcp add ({}) failed: {}",
            config_dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    bootstrap::ensure_mcp_permissions_in(config_dir)?;

    // Install the UserPromptSubmit hook for deterministic memory retrieval
    // (respect the user's auto_hook preference; default on)
    if settings::get_bool(SETTING_HOOK_ENABLED, true).unwrap_or(true) {
        let _ = bootstrap::ensure_memory_hook_in(config_dir, &binary);
    }

    Ok(())
}

#[tauri::command]
pub fn store_list_memories() -> Result<Vec<memories::Memory>, String> {
    memories::list_all()
}

#[tauri::command]
pub fn store_list_memories_by_topic(topic: String) -> Result<Vec<memories::Memory>, String> {
    memories::list_by_topic(&topic)
}

#[tauri::command]
pub fn fetch_memory(id: String) -> Result<Option<memories::Memory>, String> {
    memories::get(&id)
}

#[tauri::command]
pub fn search_memories_fts(
    query: String,
    limit: Option<u32>,
) -> Result<Vec<memories::SearchHit>, String> {
    memories::search(&query, limit)
}

#[tauri::command]
pub fn list_topics() -> Result<Vec<topics::Topic>, String> {
    topics::list_all()
}

#[tauri::command]
pub fn store_add_memory(
    title: String,
    description: String,
    content: String,
    memory_type: Option<String>,
    topic: Option<String>,
) -> Result<memories::Memory, String> {
    memories::insert(memories::NewMemory {
        title,
        description,
        content,
        memory_type,
        topic,
        source: Some("manual".to_string()),
    })
}

#[tauri::command]
pub fn store_update_memory(
    id: String,
    title: String,
    description: String,
    content: String,
    topic: Option<String>,
) -> Result<memories::Memory, String> {
    memories::update(&id, &title, &description, &content, topic.as_deref())
}

#[tauri::command]
pub fn store_delete_memory(id: String) -> Result<(), String> {
    memories::delete(&id)
}

#[derive(Serialize)]
pub struct ConfigDirMcpStatus {
    pub label: String,
    pub path: String,
    pub registered: bool,
}

#[derive(Serialize)]
pub struct McpStatus {
    pub registered: bool,
    pub binary_path: String,
    pub per_config: Vec<ConfigDirMcpStatus>,
}

/// Register the MCP server in every detected Claude config directory.
#[tauri::command]
pub fn register_mcp_server() -> Result<Vec<ConfigDirRegistration>, String> {
    let results = register_in_all_configs();
    if results.is_empty() {
        return Err("No Claude config directories found".to_string());
    }
    Ok(results)
}

/// Remove registration from every Claude config directory.
#[tauri::command]
pub fn unregister_mcp_server() -> Result<Vec<ConfigDirRegistration>, String> {
    let dirs = bootstrap::list_claude_config_dirs();
    let results = dirs
        .into_iter()
        .map(|(label, dir)| {
            let path_str = dir.to_string_lossy().to_string();
            let result = Command::new("claude")
                .env("CLAUDE_CONFIG_DIR", &dir)
                .args(["mcp", "remove", MCP_SERVER_NAME, "--scope", "user"])
                .output();

            let (success, error) = match result {
                Ok(output) if output.status.success() => (true, None),
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    if stderr.contains("not found") || stderr.contains("No MCP server") {
                        (true, None)
                    } else {
                        (false, Some(stderr.trim().to_string()))
                    }
                }
                Err(e) => (false, Some(e.to_string())),
            };

            // Also remove permissions entry (best-effort)
            let _ = bootstrap::remove_mcp_permissions_in(&dir);

            ConfigDirRegistration {
                label,
                path: path_str,
                success,
                error,
            }
        })
        .collect();
    Ok(results)
}

// Organizer commands

#[tauri::command]
pub async fn run_organize_pass() -> Result<organizer::OrganizerReport, String> {
    organizer::run_full_pass().await
}

/// Run ONLY the topic consolidation phase. Useful for cleaning up an already-classified store.
#[tauri::command]
pub async fn run_consolidate_topics() -> Result<organizer::OrganizerReport, String> {
    let mut report = organizer::OrganizerReport::default();
    let client = crate::services::claude_api::ClaudeClient::new(None);
    organizer::consolidate_topics(&client, &mut report).await?;
    Ok(report)
}

#[tauri::command]
pub fn undo_last_organize() -> Result<String, String> {
    organizer::undo_last()
}

#[tauri::command]
pub fn list_history(limit: Option<i64>) -> Result<Vec<history::HistoryEntry>, String> {
    history::list_recent(limit.unwrap_or(20))
}

#[tauri::command]
pub fn get_auto_organize() -> Result<bool, String> {
    settings::get_bool(SETTING_AUTO_ORGANIZE, true)
}

#[tauri::command]
pub fn set_auto_organize(enabled: bool) -> Result<(), String> {
    settings::set_bool(SETTING_AUTO_ORGANIZE, enabled)
}

#[tauri::command]
pub fn get_custom_db_dir() -> Result<String, String> {
    settings::get(SETTING_CUSTOM_DB_DIR, "")
}

#[tauri::command]
pub fn set_custom_db_dir(path: String) -> Result<(), String> {
    settings::set(SETTING_CUSTOM_DB_DIR, &path)
}

#[derive(Serialize)]
pub struct HookStatus {
    pub enabled: bool,
    pub per_config: Vec<ConfigDirHookStatus>,
}

#[derive(Serialize)]
pub struct ConfigDirHookStatus {
    pub label: String,
    pub path: String,
    pub installed: bool,
}

#[tauri::command]
pub fn get_hook_status() -> Result<HookStatus, String> {
    let enabled = settings::get_bool(SETTING_HOOK_ENABLED, true)?;
    let dirs = bootstrap::list_claude_config_dirs();
    let per_config = dirs
        .into_iter()
        .map(|(label, dir)| ConfigDirHookStatus {
            label,
            path: dir.to_string_lossy().to_string(),
            installed: bootstrap::is_hook_installed_in(&dir),
        })
        .collect();
    Ok(HookStatus {
        enabled,
        per_config,
    })
}

#[tauri::command]
pub fn install_hook() -> Result<Vec<ConfigDirHookStatus>, String> {
    let binary = std::env::current_exe()
        .map_err(|e| format!("binary path: {}", e))?
        .to_string_lossy()
        .to_string();

    settings::set_bool(SETTING_HOOK_ENABLED, true)?;

    let dirs = bootstrap::list_claude_config_dirs();
    let results = dirs
        .into_iter()
        .map(|(label, dir)| {
            let path_str = dir.to_string_lossy().to_string();
            let installed = bootstrap::ensure_memory_hook_in(&dir, &binary).is_ok()
                && bootstrap::is_hook_installed_in(&dir);
            ConfigDirHookStatus {
                label,
                path: path_str,
                installed,
            }
        })
        .collect();
    Ok(results)
}

#[tauri::command]
pub fn uninstall_hook() -> Result<Vec<ConfigDirHookStatus>, String> {
    settings::set_bool(SETTING_HOOK_ENABLED, false)?;
    let dirs = bootstrap::list_claude_config_dirs();
    let results = dirs
        .into_iter()
        .map(|(label, dir)| {
            let path_str = dir.to_string_lossy().to_string();
            let _ = bootstrap::remove_memory_hook_in(&dir);
            ConfigDirHookStatus {
                label,
                path: path_str,
                installed: bootstrap::is_hook_installed_in(&dir),
            }
        })
        .collect();
    Ok(results)
}

#[tauri::command]
pub fn get_mcp_server_status() -> Result<McpStatus, String> {
    let binary = std::env::current_exe()
        .map_err(|e| format!("Failed to get binary path: {}", e))?
        .to_string_lossy()
        .to_string();

    let dirs = bootstrap::list_claude_config_dirs();
    let mut per_config = Vec::new();

    for (label, dir) in &dirs {
        // Read the .claude.json config file directly instead of invoking
        // `claude mcp list`, which does network health checks on every
        // registered MCP server (slow).
        let registered = is_server_registered_in_dir(dir);

        per_config.push(ConfigDirMcpStatus {
            label: label.clone(),
            path: dir.to_string_lossy().to_string(),
            registered,
        });
    }

    let all_registered = !per_config.is_empty() && per_config.iter().all(|c| c.registered);

    Ok(McpStatus {
        registered: all_registered,
        binary_path: binary,
        per_config,
    })
}

/// Check registration by reading the config file directly (fast, no subprocess).
fn is_server_registered_in_dir(config_dir: &std::path::Path) -> bool {
    let config_path = config_dir.join(".claude.json");
    let content = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };

    json.get("mcpServers")
        .and_then(|v| v.as_object())
        .map(|servers| servers.contains_key(MCP_SERVER_NAME))
        .unwrap_or(false)
}
