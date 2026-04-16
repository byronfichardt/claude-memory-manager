//! Project scoping utilities — resolving the active project from cwd or transcript,
//! and applying scope to memories on insert.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
