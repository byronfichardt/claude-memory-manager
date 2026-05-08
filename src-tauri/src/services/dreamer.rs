//! Dreaming engine — reviews recent session transcripts to mine new memories
//! and detect stale ones. Modelled on Anthropic's "dreaming" feature for agents.
//!
//! Phases:
//! 1. Collect recent session transcripts from ~/.claude/projects/
//! 2. Mine new memories — patterns/facts/decisions not yet in the store
//! 3. Detect stale memories — existing memories contradicted by recent activity
//!
//! Results are saved as DreamProposals for the user to review and apply/dismiss.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

use crate::services::claude_api::ClaudeClient;
use crate::store::{dreams, memories};

pub const PROGRESS_EVENT: &str = "dreamer:progress";

/// How many transcript files to sample
const TRANSCRIPT_LIMIT: usize = 20;
/// Max chars extracted from a single transcript
const TRANSCRIPT_MAX_CHARS: usize = 4000;
/// Max chars from a single user or assistant turn
const TURN_MAX_CHARS: usize = 600;
/// Max memories included in the stale-detection prompt
const STALE_BATCH_SIZE: usize = 40;
/// Max chars of memory content shown to Claude during stale detection
const STALE_MEMORY_MAX_CHARS: usize = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamReport {
    pub new_proposals: usize,
    pub stale_flags: usize,
    pub transcripts_reviewed: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizerProgress {
    pub phase: &'static str,
    pub message: String,
    pub current: usize,
    pub total: usize,
}

fn emit(handle: Option<&AppHandle>, phase: &'static str, message: impl Into<String>, current: usize, total: usize) {
    if let Some(h) = handle {
        let _ = h.emit(
            PROGRESS_EVENT,
            OrganizerProgress { phase, message: message.into(), current, total },
        );
    }
}

// ── Transcript collection ─────────────────────────────────────────────────────

struct Transcript {
    path: PathBuf,
    mtime: std::time::SystemTime,
    project_slug: String,
}

/// Find and summarise the N most recent session transcripts.
fn collect_recent_transcripts(limit: usize) -> Vec<(String, String)> {
    let projects_dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("projects"),
        None => return Vec::new(),
    };

    if !projects_dir.is_dir() {
        return Vec::new();
    }

    let mut files: Vec<Transcript> = Vec::new();

    let project_entries = match std::fs::read_dir(&projects_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for project_entry in project_entries.flatten() {
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let project_slug = project_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let session_entries = match std::fs::read_dir(&project_path) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for session_entry in session_entries.flatten() {
            let p = session_entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            // Skip subagent files
            if p.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|n| n.to_str())
                == Some("subagents")
            {
                continue;
            }
            if let Ok(meta) = p.metadata() {
                if let Ok(mtime) = meta.modified() {
                    files.push(Transcript {
                        path: p,
                        mtime,
                        project_slug: project_slug.clone(),
                    });
                }
            }
        }
    }

    // Sort by most recent first
    files.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    files.truncate(limit);

    files
        .into_iter()
        .filter_map(|t| {
            let text = extract_transcript_text(&t.path)?;
            if text.trim().is_empty() {
                return None;
            }
            // Convert slug back to readable project name
            let project_label = t.project_slug
                .trim_start_matches('-')
                .replace("--", "/")
                .replace('-', "/");
            Some((project_label, text))
        })
        .collect()
}

/// Extract readable text from a single JSONL transcript file.
/// Returns user prompts and assistant text responses, truncated sensibly.
fn extract_transcript_text(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut out = String::new();
    let mut char_count = 0usize;

    for line in content.lines() {
        if char_count >= TRANSCRIPT_MAX_CHARS {
            break;
        }
        let obj: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        match obj.get("type").and_then(|v| v.as_str()) {
            Some("user") => {
                let msg_content = obj
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("");

                // Skip system/command messages
                if msg_content.starts_with('<') || msg_content.len() < 10 {
                    continue;
                }

                let truncated = truncate(msg_content, TURN_MAX_CHARS);
                out.push_str("USER: ");
                out.push_str(&truncated);
                out.push('\n');
                char_count += truncated.len();
            }
            Some("assistant") => {
                let blocks = obj
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array());

                if let Some(blocks) = blocks {
                    for block in blocks {
                        if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                // Skip organizer's own JSON responses
                                if text.trim_start().starts_with('{') {
                                    continue;
                                }
                                let truncated = truncate(text, TURN_MAX_CHARS);
                                out.push_str("ASSISTANT: ");
                                out.push_str(&truncated);
                                out.push('\n');
                                char_count += truncated.len();
                                break;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if out.is_empty() { None } else { Some(out) }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

// ── Claude prompts ────────────────────────────────────────────────────────────

const MINE_SYSTEM: &str = r#"You analyze Claude Code session transcripts to discover knowledge worth adding to a long-term memory store.

Look for:
- Recurring patterns (the user repeatedly does X, always prefers Y, consistently asks about Z)
- Decisions made during sessions that should persist (architecture choices, process rules, tool preferences)
- Project facts discovered (gotchas, config values, API quirks, deployment details)
- User preferences revealed by behavior or explicit statements

Do NOT propose:
- Anything already in the memory store (titles listed below)
- One-off events or temporary states
- Obvious or trivial facts
- Anything vague or generic

For each proposal, set memory_type:
- "feedback" — a coding/style/process rule or preference
- "project" — a specific project fact
- "user" — a trait or persistent preference of the user
- "reference" — a pointer to an external resource (URL, tool, system)

Respond with ONLY valid JSON, no prose, no markdown fences:
{"proposals": [{"title": "...", "description": "one-line summary", "content": "full memory content", "memory_type": "...", "reasoning": "why this is worth saving"}]}"#;

const STALE_SYSTEM: &str = r#"You review a memory store for outdated entries given recent session activity.

Flag memories where the transcripts provide clear evidence that the stored information is wrong, superseded, or no longer accurate.

Do NOT flag:
- Memories that simply weren't mentioned recently
- Memories that are still plausibly true
- Global preferences or style rules (these rarely go stale)

Only flag memories where you see a direct contradiction or supersession in the transcript evidence.

Respond with ONLY valid JSON, no prose, no markdown fences:
{"stale": [{"memory_id": "...", "title": "...", "reasoning": "what in the transcripts suggests this is outdated"}]}"#;

#[derive(Deserialize)]
struct MineResponse {
    proposals: Vec<MineProposal>,
}

#[derive(Deserialize)]
struct MineProposal {
    title: String,
    description: String,
    content: String,
    memory_type: String,
    reasoning: String,
}

#[derive(Deserialize)]
struct StaleResponse {
    stale: Vec<StaleFlag>,
}

#[derive(Deserialize)]
struct StaleFlag {
    memory_id: String,
    title: String,
    reasoning: String,
}

async fn mine_new_memories(
    client: &ClaudeClient,
    transcripts: &[(String, String)],
    existing: &[memories::Memory],
) -> Result<Vec<dreams::DreamProposal>, String> {
    let transcript_block = transcripts
        .iter()
        .enumerate()
        .map(|(i, (project, text))| format!("--- Session {} (project: {}) ---\n{}", i + 1, project, text))
        .collect::<Vec<_>>()
        .join("\n\n");

    let existing_titles = existing
        .iter()
        .map(|m| format!("- {}", m.title))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "EXISTING MEMORY STORE (do not re-propose these):\n{}\n\nRECENT SESSION TRANSCRIPTS:\n{}",
        if existing_titles.is_empty() { "(empty)".to_string() } else { existing_titles },
        transcript_block,
    );

    let response = client.analyze(MINE_SYSTEM, &prompt).await?;

    let parsed: MineResponse = serde_json::from_str(&response.text)
        .map_err(|e| format!("parse mine response: {} — raw: {}", e, &response.text[..200.min(response.text.len())]))?;

    let now = dreams::now();
    Ok(parsed.proposals.into_iter().map(|p| dreams::DreamProposal {
        id: uuid::Uuid::new_v4().to_string(),
        proposal_type: "new".to_string(),
        title: p.title,
        content: p.content,
        description: p.description,
        memory_type: p.memory_type,
        reasoning: p.reasoning,
        target_memory_id: None,
        status: "pending".to_string(),
        created_at: now,
    }).collect())
}

async fn detect_stale_memories(
    client: &ClaudeClient,
    transcripts: &[(String, String)],
    existing: &[memories::Memory],
) -> Result<Vec<dreams::DreamProposal>, String> {
    if existing.is_empty() {
        return Ok(Vec::new());
    }

    let transcript_block = transcripts
        .iter()
        .map(|(project, text)| format!("project: {}\n{}", project, text))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    // Only send a sample — most memories aren't stale
    let memories_block = existing
        .iter()
        .take(STALE_BATCH_SIZE)
        .map(|m| format!(
            "id: {}\ntitle: {}\ncontent: {}",
            m.id,
            m.title,
            truncate(&m.content, STALE_MEMORY_MAX_CHARS),
        ))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    let prompt = format!(
        "MEMORY STORE:\n{}\n\nRECENT SESSION TRANSCRIPTS:\n{}",
        memories_block, transcript_block,
    );

    let response = client.analyze(STALE_SYSTEM, &prompt).await?;

    let parsed: StaleResponse = serde_json::from_str(&response.text)
        .map_err(|e| format!("parse stale response: {} — raw: {}", e, &response.text[..200.min(response.text.len())]))?;

    let now = dreams::now();
    Ok(parsed.stale.into_iter().map(|s| dreams::DreamProposal {
        id: uuid::Uuid::new_v4().to_string(),
        proposal_type: "stale".to_string(),
        title: s.title,
        content: String::new(),
        description: s.reasoning.clone(),
        memory_type: String::new(),
        reasoning: s.reasoning,
        target_memory_id: Some(s.memory_id),
        status: "pending".to_string(),
        created_at: now,
    }).collect())
}

// ── Public API ────────────────────────────────────────────────────────────────

pub async fn run_dream_pass(handle: Option<AppHandle>) -> Result<DreamReport, String> {
    let client = ClaudeClient::default();
    let mut report = DreamReport {
        new_proposals: 0,
        stale_flags: 0,
        transcripts_reviewed: 0,
        errors: Vec::new(),
    };

    let h = handle.as_ref();
    emit(h, "collecting", "Collecting recent session transcripts", 0, 3);

    let transcripts = collect_recent_transcripts(TRANSCRIPT_LIMIT);
    report.transcripts_reviewed = transcripts.len();

    if transcripts.is_empty() {
        emit(h, "done", "No transcripts found", 3, 3);
        return Ok(report);
    }

    let existing = memories::list_all().unwrap_or_default();
    let mut proposals: Vec<dreams::DreamProposal> = Vec::new();

    emit(h, "mining", format!("Mining patterns from {} sessions", transcripts.len()), 1, 3);

    match mine_new_memories(&client, &transcripts, &existing).await {
        Ok(mut p) => {
            report.new_proposals = p.len();
            proposals.append(&mut p);
        }
        Err(e) => report.errors.push(format!("pattern mining: {}", e)),
    }

    emit(h, "staleness", "Checking for stale memories", 2, 3);

    match detect_stale_memories(&client, &transcripts, &existing).await {
        Ok(mut p) => {
            report.stale_flags = p.len();
            proposals.append(&mut p);
        }
        Err(e) => report.errors.push(format!("stale detection: {}", e)),
    }

    dreams::save_proposals(&proposals)?;

    emit(
        h,
        "done",
        format!(
            "{} new proposals, {} stale flags",
            report.new_proposals, report.stale_flags
        ),
        3,
        3,
    );

    Ok(report)
}
