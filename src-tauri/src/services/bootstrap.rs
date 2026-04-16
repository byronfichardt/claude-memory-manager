use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const BEGIN_MARKER: &str = "<!-- claude-memory-manager:start -->";
const END_MARKER: &str = "<!-- claude-memory-manager:end -->";

const MANAGED_SECTION: &str = r#"<!-- claude-memory-manager:start -->
## Memory

Relevant memories are auto-injected as a `<memory-context>` block at the start of each turn. Treat it as authoritative: scan it first, start your answer from it, and never propose a procedure that contradicts it. If a memory names a specific file, flag, or function, verify it still exists before acting — trust current observation over stale memory and update it.

For procedural questions (how to deploy/test/commit/release X), cite the memory that informed your answer or state "no relevant memory found" before proceeding.

### Saving — call `memory_add` proactively

Save immediately when you observe any of: a user correction, a stated preference or convention, a non-obvious project fact, a debugging gotcha and its fix, an architecture decision with its rationale, a project state change, or a workflow discovery. A typical session produces 3–10 memories. Self-check before commits and at session end.

Good memories are specific, terse, and self-contained. Prefer several small focused memories over one big one. Set `type` to `user` (about the human), `feedback` (rules/conventions), `project` (project-specific facts), or `reference` (external resources). Skip `topic` — it's auto-classified.

### Project scoping

- `type=user` → always global (enforced; `project` is ignored).
- `type=feedback` / `type=reference` → global unless you pass an explicit `project` path.
- `type=project` → defaults to the current project (git root of the MCP spawn dir). Pass `project: "global"` to override.

If Claude Code was launched from a non-project dir (e.g. `~/projects/`) but you've been editing files in a specific project, pass `project: "<git-root-absolute-path>"` explicitly on both `memory_add` and `memory_search`.

### User shortcut

If the user's message starts with `remember:`, `/remember`, or `!remember`, a `<memory-saved>` block confirms it was already saved — acknowledge briefly, don't call `memory_add`.

### Lookup tools

`memory_search(query, limit)`, `memory_get(id)`, `memory_list(topic, limit)` — use sparingly; auto-injection usually covers it.
<!-- claude-memory-manager:end -->"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDirStatus {
    pub path: String,
    pub label: String,
    pub claude_md_present: bool,
    pub managed_section_present: bool,
    pub permissions_granted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapStatus {
    pub config_dirs: Vec<ConfigDirStatus>,
    pub memory_count: i64,
    pub ingestion_done: bool,
    /// True if we found at least one `~/.claude*` directory that looks like a
    /// Claude Code install. When false, the UI should prompt the user to
    /// install Claude Code first.
    pub claude_code_installed: bool,
    /// True if `claude --version` succeeded. When false, MCP registration
    /// cannot proceed and the UI should surface a "Claude CLI not found"
    /// state.
    pub claude_cli_available: bool,
    /// Captured errors from startup (directory creation, DB init, etc.).
    pub startup_errors: Vec<String>,
    // Back-compat summary fields
    pub claude_md_exists: bool,
    pub claude_md_path: String,
    pub managed_section_present: bool,
}

/// Discover all Claude Code config directories for the current user.
///
/// Reads `$HOME` with `read_dir` and keeps entries whose name starts with
/// `.claude` and that contain a `projects/` subdirectory OR a `.claude.json`
/// file (the standard Claude Code layout). This covers the default `~/.claude`
/// as well as user-created variants like `~/.claude-personal`,
/// `~/.claude-work`, `.claude-staging`, etc.
///
/// `$HOME` itself does not trigger macOS TCC prompts — only traversing into
/// protected subdirectories (Documents, Downloads, Desktop) does. We only stat
/// the top-level entries of `$HOME`, not recurse into them, so this is safe.
pub fn list_claude_config_dirs() -> Vec<(String, PathBuf)> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    fn label_for(name: &str) -> String {
        if name == ".claude" {
            "default".to_string()
        } else if let Some(suffix) = name.strip_prefix(".claude-") {
            suffix.to_string()
        } else if let Some(suffix) = name.strip_prefix(".claude_") {
            suffix.to_string()
        } else {
            name.trim_start_matches('.').to_string()
        }
    }

    fn looks_like_claude(path: &Path) -> bool {
        path.join("projects").is_dir() || path.join(".claude.json").exists()
    }

    let mut results: Vec<(String, PathBuf)> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&home) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = match name.to_str() {
                Some(s) => s,
                None => continue,
            };
            if !name.starts_with(".claude") {
                continue;
            }
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if !looks_like_claude(&path) {
                continue;
            }
            results.push((label_for(name), path));
        }
    }

    // Also include CLAUDE_CONFIG_DIR if set and not already covered
    if let Ok(custom) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !custom.is_empty() {
            let path = PathBuf::from(&custom);
            if path.is_dir() && !results.iter().any(|(_, p)| p == &path) {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("custom")
                    .to_string();
                results.push((label_for(&name), path));
            }
        }
    }

    results.sort_by(|a, b| {
        if a.0 == "default" {
            std::cmp::Ordering::Less
        } else if b.0 == "default" {
            std::cmp::Ordering::Greater
        } else {
            a.0.cmp(&b.0)
        }
    });

    results
}

/// Create or update the managed section in a specific config dir's CLAUDE.md.
pub fn ensure_claude_md_in(config_dir: &Path) -> Result<(), String> {
    let path = config_dir.join("CLAUDE.md");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {}: {}", parent.display(), e))?;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    let new_content = if existing.is_empty() {
        MANAGED_SECTION.to_string() + "\n"
    } else if let (Some(start), Some(end)) = (existing.find(BEGIN_MARKER), existing.find(END_MARKER))
    {
        let end_idx = end + END_MARKER.len();
        let mut out = String::with_capacity(existing.len());
        out.push_str(&existing[..start]);
        out.push_str(MANAGED_SECTION);
        out.push_str(&existing[end_idx..]);
        out
    } else {
        let mut out = existing.trim_end().to_string();
        out.push_str("\n\n");
        out.push_str(MANAGED_SECTION);
        out.push('\n');
        out
    };

    atomic_write(&path, new_content.as_bytes())
        .map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(())
}

/// Write `contents` to `path` atomically by writing a sibling temp file and
/// renaming it over the target. On macOS this bypasses the provenance-based
/// write protection that blocks ad-hoc-signed apps from modifying existing
/// files in-place — `rename(2)` is permitted even when `open(O_TRUNC)` is not.
fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("tmp");
    let tmp = parent.join(format!(".{}.cmm-tmp", file_name));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// Sentinel error string returned when no Claude Code config directory can
/// be found. The frontend matches against this prefix to render a dedicated
/// "Install Claude Code first" state rather than a generic error.
pub const ERR_NO_CLAUDE_INSTALL: &str = "NO_CLAUDE_INSTALL";
pub const ERR_NO_CLAUDE_CLI: &str = "NO_CLAUDE_CLI";

/// Call ensure_claude_md_in() for every detected config dir.
pub fn ensure_claude_md_all() -> Result<(), String> {
    let dirs = list_claude_config_dirs();
    if dirs.is_empty() {
        return Err(ERR_NO_CLAUDE_INSTALL.to_string());
    }
    for (_, dir) in dirs {
        ensure_claude_md_in(&dir)?;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn remove_claude_md_section_in(config_dir: &Path) -> Result<(), String> {
    let path = config_dir.join("CLAUDE.md");
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };

    if let (Some(start), Some(end)) = (existing.find(BEGIN_MARKER), existing.find(END_MARKER)) {
        let end_idx = end + END_MARKER.len();
        let mut out = String::new();
        out.push_str(existing[..start].trim_end());
        out.push_str(existing[end_idx..].trim_start());
        if out.trim().is_empty() {
            std::fs::remove_file(&path).ok();
        } else {
            atomic_write(&path, out.as_bytes()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Install or remove the UserPromptSubmit hook that auto-injects relevant
/// memories into Claude's context on every message. Idempotent.
pub fn ensure_memory_hook_in(config_dir: &Path, binary_path: &str) -> Result<(), String> {
    let path = config_dir.join("settings.json");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
    }

    backup_settings_once_daily(&path);

    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    let mut settings: serde_json::Value =
        serde_json::from_str(&existing).unwrap_or_else(|_| serde_json::json!({}));

    if !settings.is_object() {
        settings = serde_json::json!({});
    }

    // Build hook command — escape the binary path for shell
    let hook_command = format!("{} --hook", shell_quote(binary_path));

    let obj = settings.as_object_mut().unwrap();
    let hooks = obj
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }

    let hooks_obj = hooks.as_object_mut().unwrap();
    let event_list = hooks_obj
        .entry("UserPromptSubmit".to_string())
        .or_insert_with(|| serde_json::json!([]));
    if !event_list.is_array() {
        *event_list = serde_json::json!([]);
    }

    let event_array = event_list.as_array_mut().unwrap();

    // Remove any existing claude-memory-manager hook (identified by --hook in the command)
    event_array.retain(|entry| {
        !entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|arr| {
                arr.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .map(|c| c.contains("claude-memory-manager") && c.contains("--hook"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    });

    // Add our hook entry
    event_array.push(serde_json::json!({
        "matcher": "",
        "hooks": [
            {
                "type": "command",
                "command": hook_command,
                "timeout": 10
            }
        ]
    }));

    let output =
        serde_json::to_string_pretty(&settings).map_err(|e| format!("serialize: {}", e))?;
    atomic_write(&path, (output + "\n").as_bytes())
        .map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(())
}

/// Remove the memory hook from a config dir's settings.json.
pub fn remove_memory_hook_in(config_dir: &Path) -> Result<(), String> {
    let path = config_dir.join("settings.json");
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };

    let mut settings: serde_json::Value = match serde_json::from_str(&existing) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    if let Some(event_array) = settings
        .get_mut("hooks")
        .and_then(|h| h.get_mut("UserPromptSubmit"))
        .and_then(|e| e.as_array_mut())
    {
        event_array.retain(|entry| {
            !entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .map(|arr| {
                    arr.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| c.contains("claude-memory-manager") && c.contains("--hook"))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        });
    }

    let output = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    atomic_write(&path, (output + "\n").as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

/// Check whether the memory hook is installed in a config dir.
pub fn is_hook_installed_in(config_dir: &Path) -> bool {
    let path = config_dir.join("settings.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };

    json.get("hooks")
        .and_then(|h| h.get("UserPromptSubmit"))
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter().any(|entry| {
                entry
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .map(|inner| {
                        inner.iter().any(|h| {
                            h.get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| {
                                    c.contains("claude-memory-manager") && c.contains("--hook")
                                })
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Shell-quote a path so it can be used in a hook command string.
fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/_.-".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Copy `settings.json` to `settings.json.bak` at most once per 24 hours.
///
/// Guards against destructive edits to a user's existing Claude settings:
/// if our JSON parse fails and we fall back to `{}`, we would otherwise
/// wipe their config. A dated backup gives a clear restore path.
fn backup_settings_once_daily(settings_path: &Path) {
    if !settings_path.exists() {
        return;
    }
    let backup_path = settings_path.with_extension("json.bak");

    if let Ok(meta) = std::fs::metadata(&backup_path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                if elapsed < std::time::Duration::from_secs(24 * 60 * 60) {
                    return;
                }
            }
        }
    }

    let _ = std::fs::copy(settings_path, &backup_path);
}

/// Ensure our MCP server is pre-approved in this config dir's settings.json.
pub fn ensure_mcp_permissions_in(config_dir: &Path) -> Result<(), String> {
    let path = config_dir.join("settings.json");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
    }

    backup_settings_once_daily(&path);

    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    let mut settings: serde_json::Value =
        serde_json::from_str(&existing).unwrap_or_else(|_| serde_json::json!({}));

    if !settings.is_object() {
        settings = serde_json::json!({});
    }

    {
        let obj = settings.as_object_mut().unwrap();
        let permissions = obj
            .entry("permissions".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !permissions.is_object() {
            *permissions = serde_json::json!({});
        }

        let perms_obj = permissions.as_object_mut().unwrap();
        let allow = perms_obj
            .entry("allow".to_string())
            .or_insert_with(|| serde_json::json!([]));
        if !allow.is_array() {
            *allow = serde_json::json!([]);
        }

        let allow_array = allow.as_array_mut().unwrap();
        let entry = "mcp__claude-memory-manager";
        let already = allow_array.iter().any(|v| v.as_str() == Some(entry));
        if !already {
            allow_array.push(serde_json::json!(entry));
        }
    }

    let output =
        serde_json::to_string_pretty(&settings).map_err(|e| format!("serialize: {}", e))?;
    atomic_write(&path, (output + "\n").as_bytes())
        .map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(())
}

#[allow(dead_code)]
pub fn remove_mcp_permissions_in(config_dir: &Path) -> Result<(), String> {
    let path = config_dir.join("settings.json");

    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };

    let mut settings: serde_json::Value = match serde_json::from_str(&existing) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    if let Some(allow) = settings
        .get_mut("permissions")
        .and_then(|p| p.get_mut("allow"))
        .and_then(|a| a.as_array_mut())
    {
        allow.retain(|v| v.as_str() != Some("mcp__claude-memory-manager"));
    }

    let output = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    atomic_write(&path, (output + "\n").as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

/// Check whether the `claude` CLI is present and runnable. Runs
/// `claude --version` with a short timeout; returns true on clean exit.
pub fn is_claude_cli_available() -> bool {
    match std::process::Command::new(claude_binary_path())
        .arg("--version")
        .output()
    {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// Resolve the `claude` binary. macOS GUI apps don't inherit the user's
/// shell PATH (and shell aliases never propagate), so we use the standard
/// Claude Code install location. Falls back to bare `claude` for other
/// platforms or unusual installs.
pub fn claude_binary_path() -> String {
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".local/bin/claude");
        return p.to_string_lossy().to_string();
    }
    "claude".to_string()
}

pub fn get_status() -> Result<BootstrapStatus, String> {
    let dirs = list_claude_config_dirs();
    let claude_code_installed = !dirs.is_empty();
    let mut config_dirs = Vec::new();
    let mut any_managed = false;
    let mut first_path = String::new();
    let mut first_exists = false;

    for (label, dir) in &dirs {
        let claude_md_path = dir.join("CLAUDE.md");
        let content = std::fs::read_to_string(&claude_md_path).unwrap_or_default();
        let claude_md_present = !content.is_empty();
        let managed = content.contains(BEGIN_MARKER) && content.contains(END_MARKER);

        // Check if settings.json has our permission
        let settings_path = dir.join("settings.json");
        let settings_content = std::fs::read_to_string(&settings_path).unwrap_or_default();
        let permissions_granted = settings_content.contains("mcp__claude-memory-manager");

        if managed {
            any_managed = true;
        }
        if first_path.is_empty() {
            first_path = claude_md_path.to_string_lossy().to_string();
            first_exists = claude_md_present;
        }

        config_dirs.push(ConfigDirStatus {
            path: dir.to_string_lossy().to_string(),
            label: label.clone(),
            claude_md_present,
            managed_section_present: managed,
            permissions_granted,
        });
    }

    let memory_count = crate::store::memories::count().unwrap_or(0);
    let claude_cli_available = is_claude_cli_available();
    let startup_errors = crate::store::get_startup_errors();

    Ok(BootstrapStatus {
        config_dirs,
        memory_count,
        ingestion_done: memory_count > 0,
        claude_code_installed,
        claude_cli_available,
        startup_errors,
        claude_md_exists: first_exists,
        claude_md_path: first_path,
        managed_section_present: any_managed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_managed_section_into_empty() {
        let existing = "";
        let result = if existing.is_empty() {
            MANAGED_SECTION.to_string() + "\n"
        } else {
            "UNREACHED".to_string()
        };
        assert!(result.contains(BEGIN_MARKER));
        assert!(result.contains(END_MARKER));
    }

    #[test]
    fn test_merge_replaces_existing_managed_section() {
        let existing = format!(
            "# My global CLAUDE.md\n\nSome user content\n\n{}\nOLD MANAGED CONTENT\n{}\n\nMore user content\n",
            BEGIN_MARKER, END_MARKER
        );

        let (start, end) = (existing.find(BEGIN_MARKER), existing.find(END_MARKER));
        assert!(start.is_some() && end.is_some());

        let start = start.unwrap();
        let end = end.unwrap() + END_MARKER.len();
        let mut out = String::with_capacity(existing.len());
        out.push_str(&existing[..start]);
        out.push_str(MANAGED_SECTION);
        out.push_str(&existing[end..]);

        assert!(out.contains("My global CLAUDE.md"));
        assert!(out.contains("Some user content"));
        assert!(out.contains("More user content"));
        assert!(out.contains("memory_search"));
        assert!(!out.contains("OLD MANAGED CONTENT"));
    }

    #[test]
    fn test_backup_once_daily_roundtrip() {
        let tmp = std::env::temp_dir().join(format!(
            "cmm-backup-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let settings = tmp.join("settings.json");
        std::fs::write(&settings, r#"{"original":true}"#).unwrap();

        // First call creates the backup
        backup_settings_once_daily(&settings);
        let backup = settings.with_extension("json.bak");
        assert!(backup.exists(), "backup should be created on first call");
        let first = std::fs::read_to_string(&backup).unwrap();
        assert!(first.contains("\"original\":true"));

        // Overwrite settings.json and call again in the same second — backup
        // must NOT be replaced (the daily guard).
        std::fs::write(&settings, r#"{"newer":true}"#).unwrap();
        backup_settings_once_daily(&settings);
        let second = std::fs::read_to_string(&backup).unwrap();
        assert_eq!(first, second, "daily guard should prevent re-backup");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_merge_appends_to_unmarked_claude_md() {
        let existing = "# My CLAUDE.md\n\nSome instructions.\n";
        let mut out = existing.trim_end().to_string();
        out.push_str("\n\n");
        out.push_str(MANAGED_SECTION);
        out.push('\n');

        assert!(out.contains("My CLAUDE.md"));
        assert!(out.contains("Some instructions"));
        assert!(out.contains(BEGIN_MARKER));
        assert!(out.contains(END_MARKER));
    }
}
