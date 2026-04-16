use std::path::PathBuf;

use crate::services::{bootstrap, scanner};
use crate::store::memories::{self, NewMemory};

#[derive(Debug, serde::Serialize)]
pub struct IngestionReport {
    pub files_scanned: usize,
    pub memories_imported: usize,
    pub memories_skipped: usize,
    pub errors: Vec<String>,
}

/// Scans every discovered Claude Code config dir's `projects/*/memory/`
/// subdirectories and imports all memory files into the SQLite store.
/// Idempotent: content-hash dedup prevents double-imports.
pub fn ingest_existing_files() -> Result<IngestionReport, String> {
    let mut report = IngestionReport {
        files_scanned: 0,
        memories_imported: 0,
        memories_skipped: 0,
        errors: Vec::new(),
    };

    let candidates = discover_memory_dirs();

    for (source_label, memory_dir) in candidates {
        match scanner::list_memory_files(&memory_dir) {
            Ok(files) => {
                for file in files {
                    report.files_scanned += 1;

                    let (title, description, memory_type) = if let Some(ref fm) = file.frontmatter {
                        (
                            fm.name.clone(),
                            fm.description.clone(),
                            Some(fm.memory_type.to_string()),
                        )
                    } else {
                        // Derive a title from the file name
                        let derived = file
                            .relative_path
                            .trim_end_matches(".md")
                            .replace(['_', '-'], " ");
                        (derived, String::new(), None)
                    };

                    let new = NewMemory {
                        title,
                        description,
                        content: file.body,
                        memory_type,
                        topic: None, // organizer assigns later
                        source: Some(format!("{}: {}", source_label, file.relative_path)),
                        project: None, // imported memories stay global; user classifies later
                    };

                    match memories::insert(new) {
                        Ok(m) => {
                            // Check if it was a new row or a dedup hit
                            if m.source.as_deref().unwrap_or("") != format!("{}: {}", source_label, file.relative_path) {
                                report.memories_skipped += 1;
                            } else {
                                report.memories_imported += 1;
                            }
                        }
                        Err(e) => report.errors.push(format!("{}: {}", file.relative_path, e)),
                    }
                }
            }
            Err(e) => report.errors.push(format!("{}: {}", memory_dir.display(), e)),
        }
    }

    Ok(report)
}

fn discover_memory_dirs() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();

    for (label, base) in bootstrap::list_claude_config_dirs() {
        let projects_dir = base.join("projects");
        if !projects_dir.exists() {
            continue;
        }

        if let Ok(entries) = std::fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let memory_dir = entry.path().join("memory");
                if memory_dir.exists() && memory_dir.is_dir() {
                    let project_name = entry.file_name().to_string_lossy().to_string();
                    out.push((
                        format!("{}:{}", label, project_name),
                        memory_dir,
                    ));
                }
            }
        }
    }

    out
}
