//! Install / uninstall orchestration for Claude Memory Manager.
//!
//! Wires the app into Claude Code's config directories (CLAUDE.md managed
//! section, MCP server registration, UserPromptSubmit hook) and undoes all
//! of it. Invoked by thin Tauri command wrappers in `commands/autopilot.rs`.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

use crate::services::{bootstrap, ingestion};
use crate::store::settings;

/// MCP server identifier written to `.claude.json` via `claude mcp add`.
pub const MCP_SERVER_NAME: &str = "claude-memory-manager";
/// Preference key for the auto-installed UserPromptSubmit hook.
pub const SETTING_HOOK_ENABLED: &str = "hook_enabled";
/// Optional override for the SQLite data directory (exposed via Settings UI
/// and propagated to the MCP child process via `--env` on registration).
pub const SETTING_CUSTOM_DB_DIR: &str = "custom_db_dir";

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

#[derive(Serialize)]
pub struct UninstallStep {
    pub label: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct UninstallReport {
    pub steps: Vec<UninstallStep>,
    pub data_dir_removed: bool,
    pub data_dir_path: String,
}

/// Full first-time setup: write the managed CLAUDE.md section, ingest existing
/// memory files, and register the MCP server + hook in every detected config
/// directory. Idempotent — safe to re-run.
pub fn run_first_time_setup() -> Result<SetupResult, String> {
    // Step 1: Write the managed section to every CLAUDE.md we can find
    // (also short-circuits with NO_CLAUDE_INSTALL if no ~/.claude* found)
    bootstrap::ensure_claude_md_all()?;

    // Step 2: Ingest existing memory files from all configs
    let report = ingestion::ingest_existing_files()?;

    // Step 3: Register the MCP server in every config dir — but only if the
    // claude CLI is available. Without it, `claude mcp add` would fail per
    // config dir with a confusing error. Probe once up front.
    let mcp_registrations = if bootstrap::is_claude_cli_available() {
        register_in_all_configs()
    } else {
        bootstrap::list_claude_config_dirs()
            .into_iter()
            .map(|(label, dir)| ConfigDirRegistration {
                label,
                path: dir.to_string_lossy().to_string(),
                success: false,
                error: Some(bootstrap::ERR_NO_CLAUDE_CLI.to_string()),
            })
            .collect()
    };

    let status = bootstrap::get_status()?;
    Ok(SetupResult {
        bootstrap: status,
        ingestion: report,
        mcp_registrations,
    })
}

/// Register the MCP server in every detected Claude config directory.
pub fn register_in_all_configs() -> Vec<ConfigDirRegistration> {
    bootstrap::list_claude_config_dirs()
        .into_iter()
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
pub fn register_mcp_in_dir(config_dir: &Path) -> Result<(), String> {
    let binary = std::env::current_exe()
        .map_err(|e| format!("Failed to get binary path: {}", e))?
        .to_string_lossy()
        .to_string();

    // Remove existing (ignore errors)
    let _ = Command::new(bootstrap::claude_binary_path())
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

    let output = Command::new(bootstrap::claude_binary_path())
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

/// Remove every trace of Claude Memory Manager from the user's Claude Code
/// configuration and delete the local data directory. Intended to be called
/// from a "Danger zone" button in Settings before the user drags the app to
/// the Trash on macOS (which otherwise leaves orphans behind).
///
/// Ordering matters: the hook is removed FIRST, so if any step later fails
/// the user isn't left with a broken hook pointing at a binary that may be
/// about to be deleted. Data directory removal is last.
pub fn uninstall_everything() -> UninstallReport {
    let mut steps: Vec<UninstallStep> = Vec::new();
    let dirs = bootstrap::list_claude_config_dirs();

    // Step 1: Remove the hook from every config dir
    for (label, dir) in &dirs {
        let result = bootstrap::remove_memory_hook_in(dir);
        steps.push(UninstallStep {
            label: format!("Remove hook ({})", label),
            success: result.is_ok(),
            error: result.err(),
        });
    }

    // Step 2: Remove MCP permission entry
    for (label, dir) in &dirs {
        let result = bootstrap::remove_mcp_permissions_in(dir);
        steps.push(UninstallStep {
            label: format!("Remove MCP permission ({})", label),
            success: result.is_ok(),
            error: result.err(),
        });
    }

    // Step 3: Unregister the MCP server via `claude mcp remove`
    for (label, dir) in &dirs {
        let output = Command::new(bootstrap::claude_binary_path())
            .env("CLAUDE_CONFIG_DIR", dir)
            .args(["mcp", "remove", MCP_SERVER_NAME, "--scope", "user"])
            .output();
        let (success, error) = match output {
            Ok(o) if o.status.success() => (true, None),
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                if stderr.contains("not found") || stderr.contains("No MCP server") {
                    (true, None)
                } else {
                    (false, Some(stderr.trim().to_string()))
                }
            }
            Err(e) => (false, Some(e.to_string())),
        };
        steps.push(UninstallStep {
            label: format!("Unregister MCP ({})", label),
            success,
            error,
        });
    }

    // Step 4: Remove managed CLAUDE.md section
    for (label, dir) in &dirs {
        let result = bootstrap::remove_claude_md_section_in(dir);
        steps.push(UninstallStep {
            label: format!("Strip CLAUDE.md section ({})", label),
            success: result.is_ok(),
            error: result.err(),
        });
    }

    // Step 5: Checkpoint + remove the data directory. The SQLite connection
    // stays open (it's a process-wide OnceLock) but on macOS/Linux an unlinked
    // file with open fds is fine — the app keeps running against the deleted
    // inode until it exits.
    crate::store::shutdown();

    let data_dir = crate::store::data_dir();
    let data_dir_path = data_dir.to_string_lossy().to_string();
    let data_dir_removed = match std::fs::remove_dir_all(&data_dir) {
        Ok(_) => true,
        Err(e) => {
            steps.push(UninstallStep {
                label: "Delete data dir".to_string(),
                success: false,
                error: Some(format!("{}: {}", data_dir.display(), e)),
            });
            false
        }
    };

    UninstallReport {
        steps,
        data_dir_removed,
        data_dir_path,
    }
}

/// Attempt a first-time setup automatically on launch, so users who install
/// this app and never click "Get started" still get CLAUDE.md injection,
/// ingestion, MCP registration, and hook install.
///
/// Fires once per install — the success marker at `<data_dir>/first-run.json`
/// suppresses future auto-runs. If a run fails (e.g. because Claude Code
/// isn't installed yet, or the CLI isn't on PATH), we deliberately leave the
/// marker absent so the next launch retries.
pub fn maybe_auto_bootstrap() {
    let marker_path = crate::store::data_dir().join("first-run.json");

    // Already completed a successful auto-run — nothing to do.
    if is_successful_marker(&marker_path) {
        return;
    }

    // Already bootstrapped via the UI button? Write the marker and exit.
    let status = match bootstrap::get_status() {
        Ok(s) => s,
        Err(_) => return,
    };

    if status.managed_section_present && status.memory_count > 0 {
        write_success_marker(&marker_path, "already-configured");
        return;
    }

    // Preconditions for auto-run: a Claude Code install must exist. If the
    // user hasn't installed Claude Code yet, there's nothing we can do — the
    // UI will render a "install Claude Code first" card.
    if !status.claude_code_installed {
        crate::store::record_startup_error(
            "Auto-bootstrap skipped: no ~/.claude* directory found. Install Claude Code and reopen.",
        );
        return;
    }

    // Run the same logic as the "Get started" button. Errors are logged;
    // marker is written only on success so transient failures retry later.
    match run_first_time_setup() {
        Ok(result) => {
            let any_mcp_success = result.mcp_registrations.iter().any(|r| r.success);
            let all_mcp_fail = !result.mcp_registrations.is_empty() && !any_mcp_success;

            if all_mcp_fail {
                // CLAUDE.md was still written, but MCP couldn't register.
                // Don't write the success marker — the user may resolve the
                // CLI path issue and benefit from a retry next launch.
                crate::store::record_startup_error(format!(
                    "Auto-bootstrap: CLAUDE.md injected but MCP registration failed in all {} config dir(s). Reopen after fixing the claude CLI.",
                    result.mcp_registrations.len()
                ));
            } else {
                write_success_marker(&marker_path, "ok");
            }
        }
        Err(e) => {
            crate::store::record_startup_error(format!(
                "Auto-bootstrap failed: {}. Will retry next launch.",
                e
            ));
        }
    }
}

fn is_successful_marker(path: &Path) -> bool {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };
    json.get("success").and_then(|v| v.as_bool()) == Some(true)
}

fn write_success_marker(path: &Path, reason: &str) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = serde_json::json!({
        "success": true,
        "reason": reason,
        "timestamp": ts,
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, body.to_string() + "\n");
}
