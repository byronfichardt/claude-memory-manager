use serde::Serialize;
use std::process::Command;

use crate::services::installer::{
    self, ConfigDirRegistration, SetupResult, UninstallReport, MCP_SERVER_NAME,
    SETTING_CUSTOM_DB_DIR, SETTING_HOOK_ENABLED,
};
use crate::services::{bootstrap, embeddings, organizer, portable};
use crate::store::{edges, history, memories, repo_edges, settings, topics};

const SETTING_AUTO_ORGANIZE: &str = "auto_organize";

/// Run a blocking closure on tauri's blocking thread pool. Use for any
/// command that touches the SQLite pool or other blocking I/O — keeps the
/// async runtime worker (and the IPC bridge) free.
async fn blocking<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce() -> Result<R, String> + Send + 'static,
    R: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("blocking task join error: {}", e))?
}

#[tauri::command]
pub async fn get_bootstrap_status() -> Result<bootstrap::BootstrapStatus, String> {
    blocking(bootstrap::get_status).await
}

#[tauri::command]
pub async fn get_startup_errors() -> Vec<String> {
    blocking(|| Ok(crate::store::get_startup_errors()))
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub async fn run_first_time_setup() -> Result<SetupResult, String> {
    blocking(installer::run_first_time_setup).await
}

#[tauri::command]
pub async fn uninstall_everything() -> Result<UninstallReport, String> {
    blocking(|| Ok(installer::uninstall_everything())).await
}

#[tauri::command]
pub async fn store_list_memories() -> Result<Vec<memories::Memory>, String> {
    blocking(memories::list_all).await
}

#[tauri::command]
pub async fn store_list_memories_by_topic(topic: String) -> Result<Vec<memories::Memory>, String> {
    blocking(move || memories::list_by_topic(&topic)).await
}

#[tauri::command]
pub async fn fetch_memory(id: String) -> Result<Option<memories::Memory>, String> {
    blocking(move || memories::get(&id)).await
}

#[tauri::command]
pub async fn search_memories_fts(
    query: String,
    limit: Option<u32>,
) -> Result<Vec<memories::SearchHit>, String> {
    blocking(move || memories::search(&query, limit)).await
}

#[tauri::command]
pub async fn list_topics() -> Result<Vec<topics::Topic>, String> {
    blocking(topics::list_all).await
}

#[tauri::command]
pub async fn store_add_memory(
    title: String,
    description: String,
    content: String,
    memory_type: Option<String>,
    topic: Option<String>,
) -> Result<memories::Memory, String> {
    blocking(move || {
        memories::insert(memories::NewMemory {
            title,
            description,
            content,
            memory_type,
            topic,
            source: Some("manual".to_string()),
            project: None,
        })
    })
    .await
}

#[tauri::command]
pub async fn store_update_memory(
    id: String,
    title: String,
    description: String,
    content: String,
    topic: Option<String>,
) -> Result<memories::Memory, String> {
    blocking(move || {
        memories::update(&id, &title, &description, &content, topic.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn store_delete_memory(id: String) -> Result<(), String> {
    blocking(move || memories::delete(&id)).await
}

#[tauri::command]
pub async fn store_memory_count() -> Result<i64, String> {
    blocking(memories::count).await
}

#[derive(Serialize)]
pub struct ExportSummary {
    pub path: String,
    pub memory_count: usize,
    pub bytes_written: usize,
}

/// Serialize the entire memory store to a JSON bundle and write it to `path`.
/// The frontend picks the path via the dialog plugin's `save()` and passes it
/// here — we do the actual I/O in Rust to keep capability surface minimal.
#[tauri::command]
pub fn export_memories(path: String) -> Result<ExportSummary, String> {
    let json = portable::build_export()?;
    let bytes_written = json.len();
    std::fs::write(&path, &json).map_err(|e| format!("write {}: {}", path, e))?;

    // Re-parse just enough to report the count back to the UI without a DB
    // roundtrip. Cheap since we already have the string.
    let memory_count = serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|v| v.get("memory_count").and_then(|n| n.as_u64()))
        .unwrap_or(0) as usize;

    Ok(ExportSummary {
        path,
        memory_count,
        bytes_written,
    })
}

/// Read a JSON bundle from `path` and import it. `mode` must be `"merge"` or
/// `"replace"`.
#[tauri::command]
pub fn import_memories(
    path: String,
    mode: String,
) -> Result<portable::ImportReport, String> {
    let mode = match mode.as_str() {
        "merge" => portable::ImportMode::Merge,
        "replace" => portable::ImportMode::Replace,
        other => return Err(format!("invalid import mode '{}'", other)),
    };
    let json = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {}", path, e))?;
    portable::import_bundle(&json, mode)
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
    if bootstrap::list_claude_config_dirs().is_empty() {
        return Err(bootstrap::ERR_NO_CLAUDE_INSTALL.to_string());
    }
    if !bootstrap::is_claude_cli_available() {
        return Err(bootstrap::ERR_NO_CLAUDE_CLI.to_string());
    }
    let results = installer::register_in_all_configs();
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
            let result = Command::new(bootstrap::claude_binary_path())
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
pub async fn run_organize_pass(app: tauri::AppHandle, force: Option<bool>) -> Result<organizer::OrganizerReport, String> {
    organizer::run_full_pass(Some(app), force.unwrap_or(false)).await
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
pub async fn undo_last_organize() -> Result<String, String> {
    blocking(organizer::undo_last).await
}

#[tauri::command]
pub async fn list_history(limit: Option<i64>) -> Result<Vec<history::HistoryEntry>, String> {
    blocking(move || history::list_recent(limit.unwrap_or(20))).await
}

#[tauri::command]
pub async fn get_auto_organize() -> Result<bool, String> {
    blocking(|| settings::get_bool(SETTING_AUTO_ORGANIZE, false)).await
}

#[tauri::command]
pub async fn set_auto_organize(enabled: bool) -> Result<(), String> {
    blocking(move || settings::set_bool(SETTING_AUTO_ORGANIZE, enabled)).await
}

#[tauri::command]
pub async fn get_split_threshold() -> Result<u32, String> {
    blocking(|| {
        let raw = settings::get(organizer::SETTING_SPLIT_THRESHOLD, "")?;
        if raw.is_empty() {
            return Ok(organizer::SPLIT_DEFAULT_THRESHOLD as u32);
        }
        Ok(raw
            .parse::<u32>()
            .unwrap_or(organizer::SPLIT_DEFAULT_THRESHOLD as u32))
    })
    .await
}

#[tauri::command]
pub async fn set_split_threshold(threshold: u32) -> Result<(), String> {
    blocking(move || {
        if threshold < organizer::SPLIT_THRESHOLD_MIN as u32 {
            return Err(format!(
                "threshold must be at least {}",
                organizer::SPLIT_THRESHOLD_MIN
            ));
        }
        settings::set(organizer::SETTING_SPLIT_THRESHOLD, &threshold.to_string())
    })
    .await
}

#[tauri::command]
pub async fn get_custom_db_dir() -> Result<String, String> {
    blocking(|| settings::get(SETTING_CUSTOM_DB_DIR, "")).await
}

#[tauri::command]
pub async fn set_custom_db_dir(path: String) -> Result<(), String> {
    blocking(move || settings::set(SETTING_CUSTOM_DB_DIR, &path)).await
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
pub async fn get_hook_status() -> Result<HookStatus, String> {
    blocking(|| {
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
    })
    .await
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

#[derive(Serialize)]
pub struct RelatedMemoryEntry {
    pub edge: edges::MemoryEdge,
    pub memory: memories::Memory,
}

#[derive(Serialize)]
pub struct RelatedMemoriesResponse {
    pub edges: Vec<edges::MemoryEdge>,
    pub related: Vec<RelatedMemoryEntry>,
}

#[tauri::command]
pub async fn get_related_memories(id: String, depth: Option<u32>) -> Result<RelatedMemoriesResponse, String> {
    blocking(move || {
        let depth = depth.unwrap_or(1).min(3);

        let edge_list = if depth <= 1 {
            edges::get_neighbors(&id)?
        } else {
            edges::get_neighbors_deep(&id, depth)?
        };

        let mut neighbor_ids: Vec<String> = Vec::new();
        for edge in &edge_list {
            for candidate in [&edge.source_id, &edge.target_id] {
                if candidate.as_str() != id && !neighbor_ids.contains(candidate) {
                    neighbor_ids.push(candidate.clone());
                }
            }
        }

        let id_refs: Vec<&str> = neighbor_ids.iter().map(|s| s.as_str()).collect();
        let neighbor_memories = memories::get_by_ids(&id_refs)?;
        let mem_map: std::collections::HashMap<String, memories::Memory> = neighbor_memories
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();

        let mut related = Vec::new();
        for edge in &edge_list {
            let other_id = if edge.source_id == id {
                &edge.target_id
            } else {
                &edge.source_id
            };
            if let Some(mem) = mem_map.get(other_id) {
                related.push(RelatedMemoryEntry {
                    edge: edge.clone(),
                    memory: mem.clone(),
                });
            }
        }

        Ok(RelatedMemoriesResponse {
            edges: edge_list,
            related,
        })
    })
    .await
}

#[tauri::command]
pub async fn get_mcp_server_status() -> Result<McpStatus, String> {
    blocking(get_mcp_server_status_sync).await
}

fn get_mcp_server_status_sync() -> Result<McpStatus, String> {
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

#[tauri::command]
pub async fn get_repo_graph() -> Result<repo_edges::RepoGraph, String> {
    blocking(repo_edges::full_graph).await
}

#[tauri::command]
pub async fn bulk_delete_memories(ids: Vec<String>) -> Result<usize, String> {
    blocking(move || memories::bulk_delete(&ids)).await
}

#[tauri::command]
pub async fn list_memories_since(since_ts: i64, limit: Option<usize>) -> Result<Vec<memories::Memory>, String> {
    blocking(move || memories::list_since(since_ts, limit.unwrap_or(100))).await
}

#[tauri::command]
pub async fn get_embedding_status() -> Result<embeddings::EmbeddingStatus, String> {
    blocking(|| Ok(embeddings::get_status())).await
}

#[tauri::command]
pub async fn enable_semantic_search() -> Result<(), String> {
    blocking(embeddings::enable).await
}

#[tauri::command]
pub async fn disable_semantic_search() -> Result<(), String> {
    blocking(embeddings::disable).await
}

#[tauri::command]
pub async fn trigger_embedding_sweep() -> Result<(), String> {
    blocking(|| {
        embeddings::trigger_sweep();
        Ok(())
    })
    .await
}
