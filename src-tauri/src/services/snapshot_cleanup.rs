use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::services::claude_api::ClaudeClient;
use crate::store::memories::{self, Memory};
use crate::store::{history, snapshots};

pub const BATCH_SIZE: usize = 25;
pub const MAX_AGE_SECS: i64 = 14 * 24 * 3600;
const MAX_CONTENT: usize = 400;

const MARKER_WORDS: &[&str] = &[
    "done", "shipped", "implemented", "released", "handoff", "merged", "status", "wip",
    "completed", "progress", "finished", "deployed",
];
const MARKER_PHRASES: &[&str] = &["state of", "hand-off", "hand off", "in progress", "next steps"];

pub const SYSTEM: &str = r#"You decide which memories in a long-term knowledge base are point-in-time status snapshots rather than lasting knowledge.

A status snapshot records where some piece of work stood at one moment: progress logs, "X is done / shipped / merged / implemented", handoff notes, what is left to do, the current state of a ticket or investigation. It goes stale as soon as the work moves on.

Lasting knowledge stays true and useful later: how something works, a decision and its reason, a gotcha and its fix, a convention, where something lives. A memory that mentions a ticket key or a date can still be lasting knowledge.

If a memory mixes both, it is NOT a snapshot. When in doubt, it is NOT a snapshot.

Respond with ONLY valid JSON (no prose, no markdown fences):
{"snapshot_ids": ["id1", "id2"]}"#;

#[derive(Deserialize)]
struct ClassifyResponse {
    #[serde(default)]
    snapshot_ids: Vec<String>,
}

pub fn has_snapshot_marker(text: &str) -> bool {
    let lower = text.to_lowercase();
    if MARKER_PHRASES.iter().any(|p| lower.contains(p)) {
        return true;
    }
    if lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| MARKER_WORDS.contains(&w))
    {
        return true;
    }
    has_iso_date(text) || has_ticket_key(text)
}

fn has_iso_date(text: &str) -> bool {
    text.as_bytes().windows(10).any(|w| {
        w.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        })
    })
}

fn has_ticket_key(text: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter_map(|w| w.split_once('-'))
        .any(|(key, num)| {
            key.len() >= 2
                && key.chars().all(|c| c.is_ascii_uppercase())
                && !num.is_empty()
                && num.chars().all(|c| c.is_ascii_digit())
        })
}

pub fn looks_like_snapshot(m: &Memory) -> bool {
    m.memory_type.as_deref() == Some("project")
        && has_snapshot_marker(&format!("{} {}", m.title, m.description))
}

pub fn is_stale(m: &Memory, now: i64, superseded: bool) -> bool {
    superseded || now - m.created_at >= MAX_AGE_SECS
}

pub fn needs_review<'a>(live: &'a [Memory], verdicts: &HashMap<String, String>) -> Vec<&'a Memory> {
    live.iter()
        .filter(|m| m.archived_at.is_none())
        .filter(|m| !verdicts.contains_key(&m.id))
        .filter(|m| looks_like_snapshot(m))
        .collect()
}

pub fn excluding_snapshots<'a>(batch: &'a [Memory], verdicts: &HashMap<String, String>) -> Vec<&'a Memory> {
    batch
        .iter()
        .filter(|m| verdicts.get(&m.id).map(String::as_str) != Some(snapshots::SNAPSHOT))
        .collect()
}

pub async fn classify_batch(client: &ClaudeClient, batch: &[&Memory]) -> Result<HashSet<String>, String> {
    let mut prompt = String::from("Which of these memories are status snapshots?\n\n");
    for m in batch {
        prompt.push_str(&format!("[id={}]\ntitle: {}\n", m.id, m.title));
        if !m.description.is_empty() {
            prompt.push_str(&format!("description: {}\n", m.description));
        }
        let preview: String = m.content.chars().take(MAX_CONTENT).collect();
        prompt.push_str(&format!("content: {}\n\n", preview));
    }

    let response = client.analyze(SYSTEM, &prompt).await?;
    parse_response(&response.text)
}

fn parse_response(text: &str) -> Result<HashSet<String>, String> {
    let trimmed = text.trim();
    let json = match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(start), Some(end)) if end > start => &trimmed[start..=end],
        _ => trimmed,
    };
    let parsed: ClassifyResponse =
        serde_json::from_str(json).map_err(|e| format!("parse snapshot response: {}", e))?;
    Ok(parsed.snapshot_ids.into_iter().collect())
}

pub fn record_verdicts(
    batch: &[&Memory],
    snapshot_ids: &HashSet<String>,
    verdicts: &mut HashMap<String, String>,
) -> Result<(), String> {
    for m in batch {
        let verdict = if snapshot_ids.contains(&m.id) { snapshots::SNAPSHOT } else { snapshots::KNOWLEDGE };
        snapshots::set_verdict(&m.id, verdict)?;
        verdicts.insert(m.id.clone(), verdict.to_string());
    }
    Ok(())
}

/// Archive every live confirmed snapshot that is old enough or already
/// superseded, logging each so `organizer::undo_last` can restore it.
pub fn archive_stale(
    live: &[Memory],
    verdicts: &HashMap<String, String>,
    superseded: &HashSet<String>,
    now: i64,
) -> Result<usize, String> {
    let mut archived = 0;
    for m in live {
        if verdicts.get(&m.id).map(String::as_str) != Some(snapshots::SNAPSHOT) {
            continue;
        }
        if !is_stale(m, now, superseded.contains(&m.id)) {
            continue;
        }
        if memories::archive(&m.id)? {
            let logged = history::record(
                "archive",
                serde_json::json!({
                    "action": "archive",
                    "reason": "snapshot",
                    "memory_id": m.id,
                }),
            );
            if let Err(e) = logged {
                memories::unarchive(&m.id)?;
                return Err(e);
            }
            archived += 1;
        }
    }
    Ok(archived)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(title: &str, memory_type: &str, created_at: i64) -> Memory {
        Memory {
            id: title.to_string(),
            title: title.to_string(),
            description: String::new(),
            content: String::new(),
            memory_type: Some(memory_type.to_string()),
            topic: None,
            source: None,
            project: None,
            created_at,
            updated_at: created_at,
            access_count: 0,
            archived_at: None,
        }
    }

    #[test]
    fn prefilter_matches_status_titles() {
        for title in [
            "TAF-2671 + TAF-2676 implemented locally — completion state",
            "Aethermon Town-tier skeleton (Phase 1) DONE",
            "TAF-3632 root cause not proven",
            "Family-read vs running-read status per service (2026-08-25)",
            "State of the billing migration",
        ] {
            assert!(looks_like_snapshot(&mem(title, "project", 0)), "{title}");
        }
    }

    #[test]
    fn prefilter_skips_knowledge_and_standing_rules() {
        assert!(!looks_like_snapshot(&mem("Deploys go through Kamal on Hetzner", "project", 0)));
        assert!(!looks_like_snapshot(&mem("Abandoned carts sync nightly", "project", 0)));
        assert!(!looks_like_snapshot(&mem("Status checks must pass before merge", "feedback", 0)));
        assert!(!looks_like_snapshot(&mem("Byron's status updates go in Slack", "user", 0)));
        assert!(!looks_like_snapshot(&mem("utf-8 handling in x-forwarded headers", "project", 0)));
    }

    #[test]
    fn staleness_uses_age_or_supersession() {
        let now = 100 * 24 * 3600;
        let fresh = mem("fresh", "project", now - MAX_AGE_SECS + 60);
        let old = mem("old", "project", now - MAX_AGE_SECS);
        let old_but_touched = Memory { updated_at: now, ..mem("touched", "project", now - MAX_AGE_SECS) };
        assert!(is_stale(&old_but_touched, now, false));
        assert!(!is_stale(&fresh, now, false));
        assert!(is_stale(&fresh, now, true));
        assert!(is_stale(&old, now, false));
    }

    #[test]
    fn needs_review_skips_reviewed_memories() {
        let live = vec![mem("TAF-1 DONE", "project", 0), mem("TAF-2 DONE", "project", 0)];
        let verdicts = HashMap::from([("TAF-1 DONE".to_string(), snapshots::KNOWLEDGE.to_string())]);
        let ids: Vec<&str> = needs_review(&live, &verdicts).iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["TAF-2 DONE"]);
    }

    #[test]
    fn stale_snapshot_is_archived_blocks_resave_and_undo_keeps_it() {
        let _store = crate::store::test_store();
        let add = |title: &str, content: &str| {
            memories::insert(memories::NewMemory {
                title: title.to_string(),
                description: String::new(),
                content: content.to_string(),
                memory_type: Some("project".to_string()),
                topic: None,
                source: None,
                project: Some("/tmp/quokka".to_string()),
            })
            .unwrap()
        };
        let stale = add(
            "QKA-9001 quokka checkout rewrite DONE",
            "Quokka checkout rewrite finished and merged, waiting on QA sign-off before release",
        );
        let fresh = add("QKA-9002 quokka invoice export status", "Quokka invoice export half built, CSV writer pending");
        let lesson = add("QKA-9003 quokka invoices need UTC", "Quokka invoice timestamps must be stored in UTC");

        let mut verdicts = HashMap::new();
        record_verdicts(&[&stale, &fresh, &lesson], &HashSet::from([stale.id.clone(), fresh.id.clone()]), &mut verdicts)
            .unwrap();
        let live = vec![stale.clone(), fresh.clone(), lesson.clone()];
        let superseded = HashSet::from([stale.id.clone()]);
        assert_eq!(archive_stale(&live, &verdicts, &superseded, chrono::Utc::now().timestamp()).unwrap(), 1);

        assert!(memories::get(&stale.id).unwrap().unwrap().archived_at.is_some());
        assert!(memories::get(&fresh.id).unwrap().unwrap().archived_at.is_none());
        assert!(memories::get(&lesson.id).unwrap().unwrap().archived_at.is_none());
        assert!(snapshots::is_snapshot(&fresh.id).unwrap());

        let resaved = snapshots::tombstone_for(
            "QKA-9001 quokka checkout rewrite done",
            "",
            "Quokka checkout rewrite finished and merged; waiting on QA sign-off before the release",
        )
        .unwrap();
        assert_eq!(resaved.map(|m| m.id), Some(stale.id.clone()));
        assert!(snapshots::tombstone_for("Quokka invoices", "", "Quokka invoice timestamps must be stored in UTC")
            .unwrap()
            .is_none());

        crate::services::organizer::undo_last().unwrap();
        assert!(memories::get(&stale.id).unwrap().unwrap().archived_at.is_none());
        assert_eq!(snapshots::verdicts().unwrap().get(&stale.id).map(String::as_str), Some(snapshots::KEPT));
        assert!(snapshots::tombstone_for("QKA-9001 quokka checkout rewrite DONE", "", &stale.content)
            .unwrap()
            .is_none());
    }

    #[test]
    fn confirmed_snapshots_are_left_out_of_merges() {
        let batch = vec![mem("snap", "project", 0), mem("lesson", "project", 0), mem("unreviewed", "project", 0)];
        let verdicts = HashMap::from([
            ("snap".to_string(), snapshots::SNAPSHOT.to_string()),
            ("lesson".to_string(), snapshots::KNOWLEDGE.to_string()),
        ]);
        let ids: Vec<&str> = excluding_snapshots(&batch, &verdicts).iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["lesson", "unreviewed"]);
    }

    #[test]
    fn archive_is_rolled_back_when_undo_entry_cannot_be_written() {
        let _store = crate::store::test_store();
        let snap = memories::insert(memories::NewMemory {
            title: "Dingo cache warmup DONE".to_string(),
            description: String::new(),
            content: "Dingo cache warmup shipped".to_string(),
            memory_type: Some("project".to_string()),
            topic: None,
            source: None,
            project: None,
        })
        .unwrap();
        let verdicts = HashMap::from([(snap.id.clone(), snapshots::SNAPSHOT.to_string())]);
        let block_history = |sql: &str| {
            history::ensure_table().unwrap();
            crate::store::with_conn(|c| c.execute_batch(sql).map_err(|e| e.to_string())).unwrap();
        };

        block_history("CREATE TRIGGER block_history BEFORE INSERT ON history BEGIN SELECT RAISE(FAIL, 'blocked'); END;");
        let result = archive_stale(&[snap.clone()], &verdicts, &HashSet::from([snap.id.clone()]), 0);
        block_history("DROP TRIGGER block_history;");

        assert!(result.is_err());
        assert!(memories::get(&snap.id).unwrap().unwrap().archived_at.is_none());
    }

    #[test]
    fn parses_fenced_response() {
        let ids = parse_response("```json\n{\"snapshot_ids\": [\"a\", \"b\"]}\n```").unwrap();
        assert_eq!(ids, HashSet::from(["a".to_string(), "b".to_string()]));
        assert!(parse_response("{}").unwrap().is_empty());
    }
}
