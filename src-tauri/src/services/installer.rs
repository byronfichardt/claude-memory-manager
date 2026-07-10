//! Install / uninstall orchestration for Claude Memory Manager.
//!
//! Wires the app into Claude Code's config directories (CLAUDE.md managed
//! section, MCP server registration, UserPromptSubmit hook) and, when the Pi
//! (pi.dev) coding agent is installed, into `~/.pi/agent` (AGENTS.md managed
//! section + memory MCP server in mcp.json) — then undoes all of it. Invoked by
//! thin Tauri command wrappers in `commands/autopilot.rs`.

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

    // Step 1b: Also wire up Pi (pi.dev) if it's installed — inject the same
    // managed section into ~/.pi/agent/AGENTS.md and register the memory MCP
    // server in ~/.pi/agent/mcp.json. Best-effort: Pi support is additive to the
    // primary Claude Code integration, so failures here must not abort setup.
    setup_pi_integration();

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

    // Step 4b: Remove the managed section from Pi's global AGENTS.md and the
    // memory MCP server from Pi's mcp.json (no-ops if Pi isn't installed or the
    // entries are absent).
    let pi_md = bootstrap::remove_pi_agents_md_section();
    steps.push(UninstallStep {
        label: "Strip Pi AGENTS.md section".to_string(),
        success: pi_md.is_ok(),
        error: pi_md.err(),
    });
    let pi_mcp = remove_pi_mcp_registration();
    steps.push(UninstallStep {
        label: "Unregister MCP (Pi)".to_string(),
        success: pi_mcp.is_ok(),
        error: pi_mcp.err(),
    });

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
    // Backfill Pi support once for installs that predate it (or where Pi was
    // installed after Claude setup completed). Gated by its own marker so it runs
    // at most once automatically: a user who deletes the managed block from
    // AGENTS.md is not clobbered on the next launch, and a persistent failure is
    // not re-logged every launch. The explicit "Get started" path
    // (run_first_time_setup) still re-runs it unconditionally as a retry.
    maybe_backfill_pi_integration();

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

// ---------------------------------------------------------------------------
// Pi (pi.dev) integration
// ---------------------------------------------------------------------------

/// Best-effort: wire the memory manager into Pi if it's installed — inject the
/// managed AGENTS.md section and register the memory MCP server in Pi's mcp.json.
/// Each step is independent and non-fatal; failures are logged as startup errors
/// because Pi support is additive to the primary Claude Code integration.
fn setup_pi_integration() {
    if !bootstrap::is_pi_installed() {
        return;
    }
    if let Err(e) = bootstrap::ensure_pi_agents_md() {
        crate::store::record_startup_error(format!("Pi AGENTS.md injection failed: {}", e));
    }
    if let Err(e) = register_mcp_in_pi() {
        crate::store::record_startup_error(format!("Pi MCP registration failed: {}", e));
    }
}

/// Run [`setup_pi_integration`] at most once automatically, gated by a marker in
/// the data dir. This backfills Pi support on launch for pre-existing installs
/// without clobbering a user who deliberately removed the managed block, and
/// without re-logging a persistent failure on every launch.
fn maybe_backfill_pi_integration() {
    if !bootstrap::is_pi_installed() {
        return;
    }
    let marker = crate::store::data_dir().join("pi-setup.json");
    if marker.exists() {
        return;
    }
    setup_pi_integration();
    // Record the attempt regardless of per-step success so the automatic backfill
    // does not repeat. Explicit re-setup ("Get started") remains a retry path.
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, "{\"done\":true}\n");
}

/// Register (or update) the memory MCP server in Pi's global
/// `~/.pi/agent/mcp.json`, preserving any other servers. No-op when Pi isn't
/// installed. Pi has no registration CLI, so we edit the JSON file directly —
/// this mirrors what `claude mcp add` does for Claude config dirs.
fn register_mcp_in_pi() -> Result<(), String> {
    let dir = match bootstrap::pi_agent_dir() {
        Some(d) if d.is_dir() => d,
        _ => return Ok(()),
    };
    let path = dir.join("mcp.json");

    let binary = std::env::current_exe()
        .map_err(|e| format!("Failed to get binary path: {}", e))?
        .to_string_lossy()
        .to_string();

    let mut env = serde_json::Map::new();
    let custom_db = settings::get(SETTING_CUSTOM_DB_DIR, "").unwrap_or_default();
    if !custom_db.is_empty() {
        env.insert(
            "CLAUDE_MEMORY_DB_DIR".to_string(),
            serde_json::Value::String(custom_db),
        );
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut root: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&existing).map_err(|e| format!("parse {}: {}", path.display(), e))?
    };

    upsert_mcp_server(&mut root, MCP_SERVER_NAME, &binary, env)
        .map_err(|e| format!("{}: {}", path.display(), e))?;

    let serialized =
        serde_json::to_string_pretty(&root).map_err(|e| format!("serialize mcp.json: {}", e))?;
    bootstrap::atomic_write(&path, (serialized + "\n").as_bytes())
        .map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(())
}

/// Remove the memory MCP server from Pi's mcp.json (uninstall). Leaves other
/// servers and a malformed/absent file untouched.
fn remove_pi_mcp_registration() -> Result<(), String> {
    let dir = match bootstrap::pi_agent_dir() {
        Some(d) => d,
        None => return Ok(()),
    };
    let path = dir.join("mcp.json");
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    let mut root: serde_json::Value = match serde_json::from_str(&existing) {
        Ok(v) => v,
        Err(_) => return Ok(()), // don't touch a file we can't parse
    };
    if !remove_mcp_server_entry(&mut root, MCP_SERVER_NAME) {
        return Ok(()); // nothing of ours to remove
    }
    let serialized =
        serde_json::to_string_pretty(&root).map_err(|e| format!("serialize mcp.json: {}", e))?;
    bootstrap::atomic_write(&path, (serialized + "\n").as_bytes())
        .map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(())
}

/// Upsert `mcpServers.<name>` in a Pi mcp.json value, preserving sibling servers.
/// Returns an error if the root or `mcpServers` is present but not a JSON object.
fn upsert_mcp_server(
    root: &mut serde_json::Value,
    name: &str,
    command: &str,
    env: serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "mcp.json root is not a JSON object".to_string())?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| "mcpServers is not a JSON object".to_string())?;
    servers.insert(
        name.to_string(),
        serde_json::json!({
            "command": command,
            "args": ["--mcp-server"],
            "env": serde_json::Value::Object(env),
            "lifecycle": "keep-alive",
        }),
    );
    Ok(())
}

/// Remove `mcpServers.<name>` from a Pi mcp.json value. Returns true if an entry
/// was actually removed.
fn remove_mcp_server_entry(root: &mut serde_json::Value, name: &str) -> bool {
    root.get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .map(|servers| servers.remove(name).is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_mcp_server_preserves_siblings() {
        let mut root = serde_json::json!({
            "mcpServers": {
                "other": { "command": "/bin/other", "args": [] }
            }
        });
        upsert_mcp_server(&mut root, "claude-memory-manager", "/app/cmm", serde_json::Map::new())
            .unwrap();

        let servers = root.get("mcpServers").unwrap().as_object().unwrap();
        assert!(servers.contains_key("other"), "sibling server must be preserved");
        let ours = servers.get("claude-memory-manager").unwrap();
        assert_eq!(ours.get("command").unwrap(), "/app/cmm");
        assert_eq!(ours.get("args").unwrap(), &serde_json::json!(["--mcp-server"]));
        assert_eq!(ours.get("lifecycle").unwrap(), "keep-alive");
    }

    #[test]
    fn test_upsert_mcp_server_creates_container_and_env() {
        // Empty object: mcpServers must be created; custom env carried through.
        let mut root = serde_json::json!({});
        let mut env = serde_json::Map::new();
        env.insert("CLAUDE_MEMORY_DB_DIR".to_string(), serde_json::json!("/data"));
        upsert_mcp_server(&mut root, "claude-memory-manager", "/app/cmm", env).unwrap();

        let ours = root
            .pointer("/mcpServers/claude-memory-manager")
            .expect("server entry created");
        assert_eq!(ours.pointer("/env/CLAUDE_MEMORY_DB_DIR").unwrap(), "/data");
    }

    #[test]
    fn test_upsert_mcp_server_rejects_non_object_root() {
        let mut root = serde_json::json!("not an object");
        assert!(upsert_mcp_server(&mut root, "x", "/app", serde_json::Map::new()).is_err());
    }

    #[test]
    fn test_remove_mcp_server_entry() {
        let mut root = serde_json::json!({
            "mcpServers": {
                "claude-memory-manager": { "command": "/app/cmm" },
                "other": { "command": "/bin/other" }
            }
        });
        assert!(remove_mcp_server_entry(&mut root, "claude-memory-manager"));
        let servers = root.get("mcpServers").unwrap().as_object().unwrap();
        assert!(!servers.contains_key("claude-memory-manager"));
        assert!(servers.contains_key("other"), "sibling must survive removal");

        // Second removal is a no-op returning false.
        assert!(!remove_mcp_server_entry(&mut root, "claude-memory-manager"));
        // Absent mcpServers → false, no panic.
        let mut empty = serde_json::json!({});
        assert!(!remove_mcp_server_entry(&mut empty, "claude-memory-manager"));
    }
}
