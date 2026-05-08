use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::store::data_dir;

const ENCOUNTERED_FILENAME: &str = "encountered-repos.json";

fn encountered_path() -> PathBuf {
    data_dir().join(ENCOUNTERED_FILENAME)
}

fn read_set() -> HashSet<String> {
    let content = std::fs::read_to_string(encountered_path()).unwrap_or_default();
    if content.is_empty() {
        return HashSet::new();
    }
    serde_json::from_str::<Vec<String>>(&content)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn write_set(set: &HashSet<String>) {
    let mut vec: Vec<&String> = set.iter().collect();
    vec.sort();
    if let Ok(json) = serde_json::to_string_pretty(&vec) {
        let _ = std::fs::write(encountered_path(), json);
    }
}

/// Returns true if this repo has never received a first-encounter nudge.
pub fn is_first_encounter(repo: &Path) -> bool {
    !read_set().contains(&repo.to_string_lossy().to_string())
}

/// Marks a repo as encountered so the nudge won't fire again.
pub fn mark_encountered(repo: &Path) {
    let key = repo.to_string_lossy().to_string();
    let mut set = read_set();
    if set.insert(key) {
        write_set(&set);
    }
}

/// Remove a repo from the encountered set, causing it to be treated as
/// new again on the next hook invocation.
#[allow(dead_code)]
pub fn reset_encountered(repo: &Path) {
    let key = repo.to_string_lossy().to_string();
    let mut set = read_set();
    if set.remove(&key) {
        write_set(&set);
    }
}
