//! Project scoping utilities — resolving the active project from cwd or transcript,
//! and applying scope to memories on insert.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How long an active-project pointer stays valid. After this, the MCP server
/// ignores it and falls back to its startup cwd.
const ACTIVE_PROJECT_TTL_SECS: u64 = 600;

/// Filename (inside the memory manager data dir) for the hook→MCP project pointer.
const ACTIVE_PROJECT_FILENAME: &str = "active-project.json";

/// Walk upward from `start` looking for a `.git` directory or file (worktree).
/// Stops at `$HOME` or filesystem root.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir();
    let mut current: Option<&Path> = Some(start);

    while let Some(dir) = current {
        let git = dir.join(".git");
        // .git can be a directory (normal repo) or a file (worktree/submodule)
        if git.exists() {
            return Some(dir.to_path_buf());
        }

        // Stop at $HOME — don't resolve "anything under ~" as a project
        if let Some(ref h) = home {
            if dir == h.as_path() {
                return None;
            }
        }

        current = dir.parent();
    }

    None
}

/// Resolve a directory to its project identifier.
/// Returns the git root if inside a git repo, else None (we treat ambiguity as "no project").
pub fn resolve_project(cwd: &Path) -> Option<PathBuf> {
    find_git_root(cwd)
}

/// True if two paths share their immediate parent directory.
/// e.g. /Users/byron/projects/personal/hearth and /Users/byron/projects/personal/classifyhq
/// both have parent /Users/byron/projects/personal — returns true.
pub fn shared_parent(a: &Path, b: &Path) -> bool {
    match (a.parent(), b.parent()) {
        (Some(pa), Some(pb)) => pa == pb,
        _ => false,
    }
}

// Project-affinity scoring — additive re-rank boosts applied by the hook and
// the MCP memory_search tool when a current project is known.
pub const PROJECT_AFFINITY_EXACT: f64 = 0.40;
pub const PROJECT_AFFINITY_SHARED_PARENT: f64 = 0.15;
pub const PROJECT_AFFINITY_UNRELATED: f64 = -0.20;

/// Compute the project-affinity boost for a memory given the current active project.
/// - No active project → 0 (graceful degradation)
/// - Memory is global (NULL project) → 0 (globals always apply, neutral)
/// - Exact match → +0.40
/// - Shared immediate parent directory → +0.15
/// - Different project → -0.20
pub fn project_affinity(memory_project: Option<&str>, current: Option<&Path>) -> f64 {
    match (memory_project, current) {
        (_, None) => 0.0,
        (None, Some(_)) => 0.0,
        (Some(mp), Some(cp)) => {
            let mp_path = Path::new(mp);
            if mp_path == cp {
                PROJECT_AFFINITY_EXACT
            } else if shared_parent(mp_path, cp) {
                PROJECT_AFFINITY_SHARED_PARENT
            } else {
                PROJECT_AFFINITY_UNRELATED
            }
        }
    }
}

/// Compute the final scope for a new memory.
///
/// Rules (priority order):
/// 1. `memory_type == "user"` → always `None` (global, hard rule)
/// 2. Explicit override: `"global"` → None; absolute path → Some(path)
/// 3. Type-driven default:
///    - `feedback` / `reference` → None (global default)
///    - `project` / None / other → `detected_project` (or None if not detected)
pub fn resolve_memory_scope(
    memory_type: Option<&str>,
    explicit: Option<&str>,
    detected_project: Option<&Path>,
) -> Option<String> {
    // Hard rule: user type is always global
    if memory_type == Some("user") {
        return None;
    }

    // Explicit override takes precedence
    if let Some(e) = explicit {
        let trimmed = e.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("global") {
            return None;
        }
        return Some(trimmed.to_string());
    }

    // Type-driven defaults
    match memory_type {
        Some("feedback") | Some("reference") => None,
        _ => detected_project.map(|p| p.to_string_lossy().to_string()),
    }
}

/// Parse a Claude Code session transcript (JSONL) and infer the active project.
/// Reads up to the last `max_entries` lines, extracts file paths from tool uses
/// (Read/Write/Edit/Glob/Grep, plus absolute paths in Bash commands), walks each
/// to its git root, and returns the most common root.
///
/// Returns None if the transcript is missing/malformed or no project-affine
/// activity is found.
pub fn infer_project_from_transcript(transcript_path: &Path) -> Option<PathBuf> {
    infer_project_from_transcript_with_limit(transcript_path, 50)
}

pub(crate) fn infer_project_from_transcript_with_limit(
    transcript_path: &Path,
    max_entries: usize,
) -> Option<PathBuf> {
    let content = std::fs::read_to_string(transcript_path).ok()?;
    if content.is_empty() {
        return None;
    }

    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(max_entries);
    let recent = &lines[start..];

    // Count occurrences per git root, weighted by recency (later = higher weight)
    let mut counts: HashMap<PathBuf, f64> = HashMap::new();
    let total = recent.len();

    for (i, line) in recent.iter().enumerate() {
        // More recent entries get slightly higher weight
        let recency_weight = 1.0 + (i as f64 / total.max(1) as f64);

        let paths = extract_paths_from_line(line);
        for path in paths {
            if let Some(root) = find_git_root(&path) {
                *counts.entry(root).or_insert(0.0) += recency_weight;
            }
        }
    }

    counts
        .into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(root, _)| root)
}

/// Extract file/directory paths from a single transcript JSONL line.
/// Handles `tool_use` blocks for Read/Write/Edit/Glob/Grep and absolute-path
/// mentions in Bash commands.
fn extract_paths_from_line(line: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return out,
    };

    // Only interested in assistant messages that contain tool_use blocks
    if value.get("type").and_then(|v| v.as_str()) != Some("assistant") {
        return out;
    }

    let content = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array());

    let Some(blocks) = content else { return out };

    for block in blocks {
        if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
            continue;
        }
        let tool = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let input = match block.get("input") {
            Some(v) => v,
            None => continue,
        };

        match tool {
            "Read" | "Write" | "Edit" | "NotebookEdit" => {
                if let Some(p) = input.get("file_path").and_then(|v| v.as_str()) {
                    if let Some(path) = parse_path(p) {
                        out.push(path);
                    }
                }
            }
            "Glob" | "Grep" => {
                if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
                    if let Some(path) = parse_path(p) {
                        out.push(path);
                    }
                }
            }
            "Bash" => {
                if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                    out.extend(extract_abs_paths_from_bash(cmd));
                }
            }
            _ => {}
        }
    }

    out
}

/// Parse a string as an absolute path. Ignores relative paths since we can't
/// know their git root without knowing the cwd at the time of the tool use.
fn parse_path(s: &str) -> Option<PathBuf> {
    let p = PathBuf::from(s);
    if p.is_absolute() {
        Some(p)
    } else {
        None
    }
}

/// Best-effort extraction of absolute paths from a Bash command string.
/// Looks for tokens starting with `/` that look like paths.
fn extract_abs_paths_from_bash(cmd: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // Split on whitespace, also break on common shell separators
    for token in cmd.split(|c: char| c.is_whitespace() || c == '|' || c == ';' || c == '&' || c == '"' || c == '\'') {
        if token.starts_with('/') && token.len() > 1 {
            // Strip trailing punctuation
            let cleaned = token.trim_end_matches(|c: char| matches!(c, ',' | '.' | ';' | ':' | ')' | ']'));
            let p = PathBuf::from(cleaned);
            if p.is_absolute() {
                out.push(p);
            }
        }
    }
    out
}

/// Path to the active-project pointer file. Lives alongside `memories.db` so
/// the hook (writer) and MCP server (reader) agree without extra config.
pub fn active_project_path() -> PathBuf {
    crate::store::data_dir().join(ACTIVE_PROJECT_FILENAME)
}

/// Write the detected active project for `session_id` to the pointer file.
/// Best-effort: returns early on any IO/serialization error. Uses a temp-file
/// rename for atomicity so a concurrent reader never sees a truncated file.
///
/// `project == None` clears this session's pointer (records it explicitly as
/// "no project"), so a session moving from a repo to a non-repo dir doesn't
/// leak the stale project.
pub fn write_active_project(session_id: &str, project: Option<&Path>) {
    write_active_project_at(&active_project_path(), session_id, project, now_unix());
}

fn write_active_project_at(
    path: &Path,
    session_id: &str,
    project: Option<&Path>,
    now: u64,
) {
    if session_id.is_empty() {
        return;
    }

    let mut root = read_json(path).unwrap_or_else(|| serde_json::json!({ "sessions": {} }));

    let sessions = root
        .get_mut("sessions")
        .and_then(|v| v.as_object_mut())
        .cloned()
        .unwrap_or_default();
    let mut sessions = sessions;

    prune_expired_sessions(&mut sessions, now);

    let entry = serde_json::json!({
        "project": project.map(|p| p.to_string_lossy().to_string()),
        "updated_at": now,
    });
    sessions.insert(session_id.to_string(), entry);

    root["sessions"] = serde_json::Value::Object(sessions);

    let _ = atomic_write_json(path, &root);
}

/// Read the freshest non-expired project pointer across all sessions.
/// Returns None if the file is missing, unreadable, or every entry is stale /
/// has `project == null`.
///
/// When multiple Claude sessions are active, we return the most recently
/// updated one — almost always the session that just fired a prompt.
pub fn read_active_project() -> Option<PathBuf> {
    read_active_project_at(&active_project_path(), now_unix())
}

fn read_active_project_at(path: &Path, now: u64) -> Option<PathBuf> {
    let root = read_json(path)?;
    let sessions = root.get("sessions")?.as_object()?;

    let mut best: Option<(u64, PathBuf)> = None;
    for (_sid, entry) in sessions {
        let Some(updated_at) = entry.get("updated_at").and_then(|v| v.as_u64()) else {
            continue;
        };
        if now.saturating_sub(updated_at) > ACTIVE_PROJECT_TTL_SECS {
            continue;
        }
        let Some(project_str) = entry.get("project").and_then(|v| v.as_str()) else {
            // project is explicit None for this session — skip
            continue;
        };
        if project_str.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(project_str);
        match &best {
            Some((best_ts, _)) if *best_ts >= updated_at => {}
            _ => best = Some((updated_at, candidate)),
        }
    }
    best.map(|(_, p)| p)
}

fn prune_expired_sessions(sessions: &mut serde_json::Map<String, serde_json::Value>, now: u64) {
    sessions.retain(|_, entry| {
        entry
            .get("updated_at")
            .and_then(|v| v.as_u64())
            .map(|ts| now.saturating_sub(ts) <= ACTIVE_PROJECT_TTL_SECS)
            .unwrap_or(false)
    });
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn atomic_write_json(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cmm-project-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn find_git_root_finds_dot_git_directory() {
        let root = tmpdir();
        let repo = root.join("myrepo");
        let sub = repo.join("src").join("deep");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap();

        let found = find_git_root(&sub);
        assert_eq!(found, Some(repo));
    }

    #[test]
    fn find_git_root_finds_dot_git_file_worktree() {
        let root = tmpdir();
        let repo = root.join("myrepo");
        let sub = repo.join("app");
        fs::create_dir_all(&sub).unwrap();
        // Worktrees use a file, not a directory
        fs::write(repo.join(".git"), "gitdir: /some/other/path").unwrap();

        let found = find_git_root(&sub);
        assert_eq!(found, Some(repo));
    }

    #[test]
    fn find_git_root_returns_none_for_non_git() {
        let root = tmpdir();
        let sub = root.join("no-git").join("here");
        fs::create_dir_all(&sub).unwrap();

        let found = find_git_root(&sub);
        assert!(found.is_none());
    }

    #[test]
    fn shared_parent_exact_match() {
        let a = Path::new("/Users/byron/projects/personal/hearth");
        let b = Path::new("/Users/byron/projects/personal/classifyhq");
        assert!(shared_parent(a, b));
    }

    #[test]
    fn shared_parent_different_depths() {
        let a = Path::new("/Users/byron/projects/personal/hearth");
        let b = Path::new("/Users/byron/projects/work/hobbii-api");
        assert!(!shared_parent(a, b));
    }

    #[test]
    fn shared_parent_identical_paths() {
        let a = Path::new("/Users/byron/projects/personal/hearth");
        let b = Path::new("/Users/byron/projects/personal/hearth");
        // Same path → same parent
        assert!(shared_parent(a, b));
    }

    #[test]
    fn resolve_memory_scope_user_is_always_global() {
        let p = Path::new("/projects/hearth");
        assert_eq!(resolve_memory_scope(Some("user"), None, Some(p)), None);
        assert_eq!(resolve_memory_scope(Some("user"), Some("/override"), Some(p)), None);
        assert_eq!(resolve_memory_scope(Some("user"), Some("global"), None), None);
    }

    #[test]
    fn resolve_memory_scope_explicit_override() {
        let p = Path::new("/projects/hearth");
        assert_eq!(
            resolve_memory_scope(Some("project"), Some("global"), Some(p)),
            None
        );
        assert_eq!(
            resolve_memory_scope(Some("project"), Some("/other/path"), Some(p)),
            Some("/other/path".to_string())
        );
    }

    #[test]
    fn resolve_memory_scope_feedback_defaults_global() {
        let p = Path::new("/projects/hearth");
        assert_eq!(resolve_memory_scope(Some("feedback"), None, Some(p)), None);
        assert_eq!(resolve_memory_scope(Some("reference"), None, Some(p)), None);
    }

    #[test]
    fn resolve_memory_scope_project_defaults_to_detected() {
        let p = Path::new("/projects/hearth");
        assert_eq!(
            resolve_memory_scope(Some("project"), None, Some(p)),
            Some("/projects/hearth".to_string())
        );
        assert_eq!(resolve_memory_scope(Some("project"), None, None), None);
    }

    #[test]
    fn resolve_memory_scope_no_type_defaults_to_detected() {
        let p = Path::new("/projects/hearth");
        assert_eq!(
            resolve_memory_scope(None, None, Some(p)),
            Some("/projects/hearth".to_string())
        );
    }

    #[test]
    fn resolve_memory_scope_explicit_empty_treated_as_global() {
        assert_eq!(resolve_memory_scope(Some("project"), Some(""), None), None);
        assert_eq!(resolve_memory_scope(Some("project"), Some("  "), None), None);
    }

    #[test]
    fn infer_project_handles_missing_file() {
        let fake = Path::new("/nonexistent/path/to/transcript.jsonl");
        assert!(infer_project_from_transcript(fake).is_none());
    }

    #[test]
    fn infer_project_from_transcript_picks_most_common_root() {
        // Build a fake git repo structure
        let root = tmpdir();
        let hearth = root.join("hearth");
        let classify = root.join("classifyhq");
        fs::create_dir_all(hearth.join("src")).unwrap();
        fs::create_dir_all(classify.join("src")).unwrap();
        fs::create_dir_all(hearth.join(".git")).unwrap();
        fs::create_dir_all(classify.join(".git")).unwrap();

        let hearth_file = hearth.join("src/main.rs");
        let classify_file = classify.join("src/app.ts");
        fs::write(&hearth_file, "").unwrap();
        fs::write(&classify_file, "").unwrap();

        // Build a transcript with 3 Hearth tool uses and 1 ClassifyHQ
        let transcript_path = root.join("transcript.jsonl");
        let entries = vec![
            make_tool_use_line("Read", &hearth_file.to_string_lossy()),
            make_tool_use_line("Edit", &hearth_file.to_string_lossy()),
            make_tool_use_line("Read", &classify_file.to_string_lossy()),
            make_tool_use_line("Write", &hearth_file.to_string_lossy()),
        ];
        fs::write(&transcript_path, entries.join("\n")).unwrap();

        let detected = infer_project_from_transcript(&transcript_path);
        assert_eq!(detected, Some(hearth));
    }

    #[test]
    fn infer_project_handles_malformed_jsonl() {
        let root = tmpdir();
        let transcript = root.join("bad.jsonl");
        fs::write(&transcript, "not json\n{also not valid\n").unwrap();
        assert!(infer_project_from_transcript(&transcript).is_none());
    }

    #[test]
    fn infer_project_empty_transcript() {
        let root = tmpdir();
        let transcript = root.join("empty.jsonl");
        fs::write(&transcript, "").unwrap();
        assert!(infer_project_from_transcript(&transcript).is_none());
    }

    #[test]
    fn active_project_roundtrip_returns_written_path() {
        let dir = tmpdir();
        let file = dir.join("active.json");
        let project = PathBuf::from("/Users/byron/projects/personal/hearth");

        write_active_project_at(&file, "sess-1", Some(&project), 1_000);
        let got = read_active_project_at(&file, 1_000);
        assert_eq!(got, Some(project));
    }

    #[test]
    fn active_project_picks_most_recent_session() {
        let dir = tmpdir();
        let file = dir.join("active.json");
        let hearth = PathBuf::from("/projects/hearth");
        let classify = PathBuf::from("/projects/classifyhq");

        write_active_project_at(&file, "sess-old", Some(&hearth), 1_000);
        write_active_project_at(&file, "sess-new", Some(&classify), 1_100);

        let got = read_active_project_at(&file, 1_100);
        assert_eq!(got, Some(classify));
    }

    #[test]
    fn active_project_expires_past_ttl() {
        let dir = tmpdir();
        let file = dir.join("active.json");
        let hearth = PathBuf::from("/projects/hearth");
        write_active_project_at(&file, "sess-1", Some(&hearth), 1_000);

        // Read well past the TTL window
        let got = read_active_project_at(&file, 1_000 + ACTIVE_PROJECT_TTL_SECS + 1);
        assert!(got.is_none());
    }

    #[test]
    fn active_project_none_clears_for_session() {
        let dir = tmpdir();
        let file = dir.join("active.json");
        let hearth = PathBuf::from("/projects/hearth");

        write_active_project_at(&file, "sess-1", Some(&hearth), 1_000);
        write_active_project_at(&file, "sess-1", None, 1_050);

        let got = read_active_project_at(&file, 1_050);
        assert!(got.is_none());
    }

    #[test]
    fn active_project_read_handles_missing_file() {
        let dir = tmpdir();
        let missing = dir.join("nope.json");
        assert!(read_active_project_at(&missing, 1_000).is_none());
    }

    #[test]
    fn active_project_write_ignores_empty_session_id() {
        let dir = tmpdir();
        let file = dir.join("active.json");
        let hearth = PathBuf::from("/projects/hearth");
        write_active_project_at(&file, "", Some(&hearth), 1_000);
        assert!(!file.exists());
    }

    fn make_tool_use_line(tool: &str, file_path: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "name": tool,
                    "input": { "file_path": file_path }
                }]
            }
        })
        .to_string()
    }
}
