use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const BEGIN_MARKER: &str = "<!-- claude-memory-manager:start -->";
const END_MARKER: &str = "<!-- claude-memory-manager:end -->";

const MANAGED_SECTION: &str = r#"<!-- claude-memory-manager:start -->
## Memory

You have a persistent memory system. Relevant memories for each user message are automatically injected into your context as a `<memory-context>` block at the start of the turn — you don't need to fetch them. Use what's already there.

### Saving memories — BE AGGRESSIVE

Call `memory_add` IMMEDIATELY when any of these happen. Don't wait for permission, don't ask, just save:

- **User correction**: "no, actually...", "don't do X", "that's wrong because...", "stop doing Y"
- **Stated preference or convention**: "we always use Y", "I prefer Z", "never do W", "our style is..."
- **Non-obvious project fact**: file layouts, service names, credentials locations, IP addresses, how subsystems connect
- **Debugging finding**: a gotcha you hit and solved, an environment-specific issue, a version mismatch
- **Cross-session context** the user told you: their role, their goals, their team conventions

Rules for good memories:
- Be specific and terse. Prefer several small focused memories over one big one.
- Include enough context that the memory makes sense out-of-session.
- Set `type` appropriately: `user` (about the user), `feedback` (rules/conventions), `project` (project-specific), `reference` (external resource).
- Skip `topic` — the organizer auto-classifies it later.

### User shortcut

If the user's message starts with `remember:`, `/remember`, or `!remember`, a `<memory-saved>` block will appear in your context — the text has ALREADY been saved automatically. Just acknowledge briefly and proceed with any other part of their message. Do NOT call memory_add in that case.

### Targeted lookup tools

For cases where the auto-injected context isn't enough, you can also call:
- `memory_search(query, limit)` — FTS search when you need more results
- `memory_get(id)` — fetch a specific memory's full content
- `memory_list(topic, limit)` — browse by topic

Use these sparingly; the auto-injection usually has what you need.
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
    // Back-compat summary fields
    pub claude_md_exists: bool,
    pub claude_md_path: String,
    pub managed_section_present: bool,
}

/// Discover all Claude Code config directories for the current user.
///
/// Looks for any directory in `$HOME` whose name starts with `.claude` and
/// contains a `projects/` subdirectory (the standard Claude Code layout).
/// This covers the default `~/.claude` as well as user-created variants like
/// `~/.claude-personal` or `~/.claude-work` (common when users run multiple
/// Claude Code accounts via `CLAUDE_CONFIG_DIR`).
pub fn list_claude_config_dirs() -> Vec<(String, PathBuf)> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let mut results = Vec::new();

    // Walk $HOME looking for .claude* dirs
    if let Ok(entries) = std::fs::read_dir(&home) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(".claude") {
                continue;
            }
            // Skip non-Claude-Code dirs by requiring the projects/ subdir
            // OR a .claude.json (Claude Code's main config file)
            let looks_like_claude = path.join("projects").is_dir()
                || path.join(".claude.json").exists();
            if !looks_like_claude {
                continue;
            }
            // Derive a label from the suffix:
            //   .claude          → "default"
            //   .claude-personal → "personal"
            //   .claude-work     → "work"
            let label = if name == ".claude" {
                "default".to_string()
            } else if let Some(suffix) = name.strip_prefix(".claude-") {
                suffix.to_string()
            } else if let Some(suffix) = name.strip_prefix(".claude_") {
                suffix.to_string()
            } else {
                name.trim_start_matches('.').to_string()
            };
            results.push((label, path));
        }
    }

    results.sort_by(|a, b| {
        // "default" always first
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

    std::fs::write(&path, new_content)
        .map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(())
}

/// Call ensure_claude_md_in() for every detected config dir.
pub fn ensure_claude_md_all() -> Result<(), String> {
    let dirs = list_claude_config_dirs();
    if dirs.is_empty() {
        return Err("No Claude config directories found".to_string());
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
            std::fs::write(&path, out).map_err(|e| e.to_string())?;
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
    std::fs::write(&path, output + "\n")
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
    std::fs::write(&path, output + "\n").map_err(|e| e.to_string())?;
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

/// Ensure our MCP server is pre-approved in this config dir's settings.json.
pub fn ensure_mcp_permissions_in(config_dir: &Path) -> Result<(), String> {
    let path = config_dir.join("settings.json");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
    }

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
    std::fs::write(&path, output + "\n")
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
    std::fs::write(&path, output + "\n").map_err(|e| e.to_string())?;
    Ok(())
}

pub fn get_status() -> Result<BootstrapStatus, String> {
    let dirs = list_claude_config_dirs();
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

    Ok(BootstrapStatus {
        config_dirs,
        memory_count,
        ingestion_done: memory_count > 0,
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
