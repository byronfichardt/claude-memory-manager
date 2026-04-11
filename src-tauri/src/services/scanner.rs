use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::models::memory::MemoryFile;
use crate::services::frontmatter;

/// List all memory files in a project's memory directory.
/// Used only by the one-shot ingestion service.
pub fn list_memory_files(memory_dir: &Path) -> Result<Vec<MemoryFile>, String> {
    if !memory_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();

    for entry in walkdir::WalkDir::new(memory_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        if path.extension().map_or(true, |ext| ext != "md") {
            continue;
        }

        if entry.file_name() == "MEMORY.md" {
            continue;
        }

        let relative_path = path
            .strip_prefix(memory_dir)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string();

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

        let metadata = std::fs::metadata(path)
            .map_err(|e| format!("Failed to get metadata for {}: {}", path.display(), e))?;

        let last_modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let size_bytes = metadata.len();

        let (fm, body) = frontmatter::parse(&content);

        files.push(MemoryFile {
            path: path.to_path_buf(),
            relative_path,
            frontmatter: fm,
            body,
            last_modified,
            size_bytes,
        });
    }

    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}
