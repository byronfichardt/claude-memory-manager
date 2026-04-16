//! Auto-organization engine. Classifies untopiced memories into topics,
//! merges near-duplicates within topics, and consolidates overlapping topics.
//! Uses `claude -p` under the hood.

use serde::{Deserialize, Serialize};

use crate::services::claude_api::ClaudeClient;
use crate::store::{edges, history, memories, settings, topics};

const SETTING_LAST_ORGANIZE_TS: &str = "last_organize_ts";
const SETTING_INITIAL_RELATE_DONE: &str = "initial_relate_done";

const CLASSIFY_BATCH_SIZE: usize = 25;
const CLASSIFY_MAX_CONTENT: usize = 300;
const DEDUP_MAX_CONTENT: usize = 1000;
const DEDUP_MIN_TOPIC_SIZE: usize = 2;
const CONSOLIDATE_SAMPLE_PER_TOPIC: usize = 6;
const RELATE_BATCH_SIZE: usize = 20;
const RELATE_MAX_CONTENT: usize = 400;

const CLASSIFY_SYSTEM: &str = r#"You classify memories from a knowledge base into topics.

Rules:
1. Assign each memory to exactly one topic.
2. STRONGLY prefer broad, reusable topics. Don't create a topic that will only have 1 member — find a broader bucket.
3. Topics are FUNCTIONAL categories (what kind of knowledge), NOT project names. Memories about specific projects (e.g. "launchpad", "hearth", "vigil") should be classified by WHAT they're about (deployment, architecture, setup) — never by the project name itself.
4. Topics should be short (1-2 words), lowercase, kebab-case if multi-word.
5. Prefer existing topics when applicable. Existing topics listed in the prompt should be reused unless the memory truly doesn't fit any of them.
6. Good topic examples: "deployment", "testing", "user-profile", "docker", "database", "git-workflow", "projects", "architecture", "security".
7. Bad topic examples: "general", "misc", "other", "stuff", "notes", "launchpad" (project name), "hearth" (project name), "work-project-x" (too specific).

Respond with ONLY valid JSON in this exact format (no prose, no markdown fences):
{"assignments": [{"id": "memory-id", "topic": "topic-name"}]}"#;

const CONSOLIDATE_SYSTEM: &str = r#"You consolidate topic buckets in a memory knowledge base. Identify topics that should merge into broader parent topics.

MERGE THESE:
1. Single-member topics that fit naturally into an existing broader topic (e.g. "file-safety" → "safety").
2. Clearly overlapping / semantically redundant topic pairs (e.g. "workflow" + "git-workflow").
3. Project-name topics (named after a specific project/product like "launchpad", "hearth", "vigil") — consolidate into a broader topic like "projects".

DO NOT MERGE:
1. Any topic with 5 or more members — these are healthy, leave them alone even if semantically adjacent to another topic.
2. Topics that represent genuinely different functional categories, even if conceptually related. For example:
   - "docker" and "deployment" are related but distinct — keep them separate.
   - "testing" and "code-quality" are related but distinct — keep them separate.
3. Do NOT put a memory in a topic where it clearly doesn't belong just to avoid a single-member topic. If a memory doesn't fit any existing topic, leave it in its current topic — a single-member topic is better than a wrong classification.

BE CONSERVATIVE: When in doubt, DON'T merge. It's safer to leave a topic alone than to force a bad merge.

For each merge, pick the best target topic name. Prefer existing topics over inventing new umbrella names.

Respond with ONLY valid JSON (no prose, no markdown fences):
{"merges": [{"sources": ["topic-a", "topic-b"], "target": "topic-a", "reason": "brief justification"}]}

If nothing should merge, return {"merges": []}."#;

const RELATE_SYSTEM: &str = r#"You identify semantic relationships between memories in a knowledge base.

For each pair of memories that have a meaningful relationship, output an edge.

Edge types:
- "relates-to": memories discuss related concepts or share common context
- "supersedes": memory A replaces/updates memory B (B is outdated)
- "depends-on": memory A only makes sense if you also know memory B
- "contradicts": memories contain conflicting information

Rules:
1. Only emit edges where the relationship is clear and useful for retrieval.
2. Don't emit "relates-to" for every pair in the same topic — only genuinely connected ones.
3. For "supersedes" and "contradicts", order matters: source supersedes/contradicts target.
4. Aim for precision over recall — fewer strong edges beat many weak ones.

Respond with ONLY valid JSON (no prose, no markdown fences):
{"edges": [{"source_id": "id1", "target_id": "id2", "edge_type": "relates-to"}]}"#;

const DEDUP_SYSTEM: &str = r#"You identify groups of memories that are near-duplicates — different phrasings of the same information, or memories that overlap enough that merging would lose nothing.

Rules:
1. Only flag STRONG duplicates. When in doubt, don't merge.
2. Memories that discuss the same topic but contain different facts are NOT duplicates.
3. For each duplicate group, provide a merged version that preserves all unique information.
4. Skip singletons — they are not duplicates.

Respond with ONLY valid JSON (no prose, no markdown fences):
{"merges": [{"source_ids": ["id1", "id2"], "merged_title": "...", "merged_description": "...", "merged_content": "..."}]}"#;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OrganizerReport {
    pub classified_count: usize,
    pub new_topics_created: Vec<String>,
    pub merged_count: usize,
    pub edges_created: usize,
    pub consolidated_topics: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Deserialize)]
struct AiConsolidateResponse {
    #[serde(default)]
    merges: Vec<TopicMerge>,
}

#[derive(Deserialize)]
struct TopicMerge {
    sources: Vec<String>,
    target: String,
    #[serde(default)]
    reason: String,
}

#[derive(Deserialize)]
struct AiClassifyResponse {
    assignments: Vec<Assignment>,
}

#[derive(Deserialize)]
struct Assignment {
    id: String,
    topic: String,
}

#[derive(Deserialize)]
struct AiRelateResponse {
    #[serde(default)]
    edges: Vec<AiEdge>,
}

#[derive(Deserialize, Serialize, Clone)]
struct AiEdge {
    source_id: String,
    target_id: String,
    edge_type: String,
}

#[derive(Deserialize)]
struct AiDedupResponse {
    #[serde(default)]
    merges: Vec<Merge>,
}

#[derive(Deserialize)]
struct Merge {
    source_ids: Vec<String>,
    merged_title: String,
    #[serde(default)]
    merged_description: String,
    merged_content: String,
}

/// Run a full organization pass: classify untopiced memories, dedup within topics,
/// then consolidate overlapping topics.
///
/// Only dedup/consolidate run on topics that changed since the last pass.
pub async fn run_full_pass() -> Result<OrganizerReport, String> {
    let mut report = OrganizerReport::default();
    let client = ClaudeClient::new(None);

    let last_ts: i64 = settings::get(SETTING_LAST_ORGANIZE_TS, "0")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .unwrap_or(0);
    let now_ts = chrono::Utc::now().timestamp();

    // Phase 1: classify untopiced memories
    match classify_untopiced(&client, &mut report).await {
        Ok(()) => {}
        Err(e) => report.errors.push(format!("classify: {}", e)),
    }

    // Phase 1.5: discover relationships.
    // First run: relate ALL topics to seed the graph (one-time).
    // Subsequent runs: only relate topics that got new classifications this pass.
    let initial_relate_done = settings::get_bool(SETTING_INITIAL_RELATE_DONE, false)
        .unwrap_or(false);

    if !initial_relate_done {
        match relate_all_topics(&client, &mut report).await {
            Ok(()) => {
                let _ = settings::set_bool(SETTING_INITIAL_RELATE_DONE, true);
            }
            Err(e) => report.errors.push(format!("relate (initial): {}", e)),
        }
    } else if report.classified_count > 0 {
        match relate_newly_classified_topics(&client, &mut report).await {
            Ok(()) => {}
            Err(e) => report.errors.push(format!("relate: {}", e)),
        }
    }

    // Phase 2: dedup only topics with new/updated memories since last pass
    match dedup_changed_topics(&client, &mut report, last_ts).await {
        Ok(()) => {}
        Err(e) => report.errors.push(format!("dedup: {}", e)),
    }

    // Phase 3: consolidate only if new classifications happened
    if report.classified_count > 0 || !report.new_topics_created.is_empty() {
        match consolidate_topics(&client, &mut report).await {
            Ok(()) => {}
            Err(e) => report.errors.push(format!("consolidate: {}", e)),
        }
    }

    // Record the timestamp for next pass
    let _ = settings::set(SETTING_LAST_ORGANIZE_TS, &now_ts.to_string());

    Ok(report)
}

/// Consolidate overlapping topics (Phase 3). Reads all topics with samples,
/// asks the AI which should merge, then bulk-reassigns memories.
pub async fn consolidate_topics(
    client: &ClaudeClient,
    report: &mut OrganizerReport,
) -> Result<(), String> {
    let all_topics = topics::list_all()?;
    if all_topics.len() < 2 {
        return Ok(());
    }

    // Build prompt: each topic with a sample of its memories
    let mut prompt = String::from("Here are the current topics with sample memories:\n\n");
    for topic in &all_topics {
        prompt.push_str(&format!(
            "## {} ({} memor{})\n",
            topic.name,
            topic.memory_count,
            if topic.memory_count == 1 { "y" } else { "ies" }
        ));
        let mems = memories::list_by_topic(&topic.name)?;
        for m in mems.iter().take(CONSOLIDATE_SAMPLE_PER_TOPIC) {
            if m.description.is_empty() {
                prompt.push_str(&format!("- {}\n", m.title));
            } else {
                prompt.push_str(&format!("- {}: {}\n", m.title, m.description));
            }
        }
        if mems.len() > CONSOLIDATE_SAMPLE_PER_TOPIC {
            prompt.push_str(&format!(
                "  (... and {} more)\n",
                mems.len() - CONSOLIDATE_SAMPLE_PER_TOPIC
            ));
        }
        prompt.push('\n');
    }

    let response = client.analyze(CONSOLIDATE_SYSTEM, &prompt).await?;
    let json = extract_json(&response.text);
    let parsed: AiConsolidateResponse = serde_json::from_str(json).map_err(|e| {
        format!(
            "parse consolidate response: {} (raw: {})",
            e,
            truncate(json, 300)
        )
    })?;

    for merge in parsed.merges {
        match apply_topic_merge(&merge) {
            Ok(moved) => {
                let summary = format!(
                    "{} → {} ({} memories)",
                    merge.sources.join("+"),
                    merge.target,
                    moved
                );
                report.consolidated_topics.push(summary);
            }
            Err(e) => report
                .errors
                .push(format!("consolidate merge failed: {}", e)),
        }
    }

    Ok(())
}

/// Apply a topic merge: move all memories from source topics to the target topic,
/// snapshot the before-state for undo, and remove now-empty source topics.
fn apply_topic_merge(merge: &TopicMerge) -> Result<usize, String> {
    let target = normalize_topic(&merge.target);
    if target.is_empty() {
        return Err("empty target topic".to_string());
    }

    // Collect memories currently in the source topics (for snapshot + move)
    let mut affected = Vec::new();
    for source in &merge.sources {
        let source_norm = normalize_topic(source);
        if source_norm.is_empty() || source_norm == target {
            continue;
        }
        let mems = memories::list_by_topic(&source_norm)?;
        for m in mems {
            affected.push((source_norm.clone(), m));
        }
    }

    if affected.is_empty() {
        return Ok(0);
    }

    // Snapshot BEFORE any mutation
    let snapshot = serde_json::json!({
        "action": "consolidate",
        "target": target,
        "sources": merge.sources,
        "reason": merge.reason,
        "affected": affected.iter().map(|(src, m)| serde_json::json!({
            "source_topic": src,
            "memory_id": m.id,
        })).collect::<Vec<_>>(),
    });
    history::record("consolidate", snapshot)?;

    // Ensure target topic exists
    topics::ensure(&target, None, None)?;

    // Move each affected memory to the target topic
    let moved_count = affected.len();
    for (_src, m) in &affected {
        memories::update(&m.id, &m.title, &m.description, &m.content, Some(&target))?;
    }

    // Remove now-empty source topics (best-effort — won't hurt if some remain)
    for source in &merge.sources {
        let source_norm = normalize_topic(source);
        if source_norm.is_empty() || source_norm == target {
            continue;
        }
        let remaining = memories::list_by_topic(&source_norm)?;
        if remaining.is_empty() {
            let _ = topics::delete_empty(&source_norm);
        }
    }

    Ok(moved_count)
}

/// Classify all untopiced memories in batches.
pub async fn classify_untopiced(
    client: &ClaudeClient,
    report: &mut OrganizerReport,
) -> Result<(), String> {
    let untopiced = memories::list_untopiced()?;
    if untopiced.is_empty() {
        return Ok(());
    }

    let existing_topics: Vec<String> =
        topics::list_all()?.into_iter().map(|t| t.name).collect();

    // Process in batches
    for chunk in untopiced.chunks(CLASSIFY_BATCH_SIZE) {
        match classify_batch(client, chunk, &existing_topics).await {
            Ok(assignments) => {
                apply_classifications(&assignments, &existing_topics, report)?;
            }
            Err(e) => {
                report
                    .errors
                    .push(format!("classify batch: {}", e));
            }
        }
    }

    Ok(())
}

async fn classify_batch(
    client: &ClaudeClient,
    batch: &[memories::Memory],
    existing_topics: &[String],
) -> Result<Vec<Assignment>, String> {
    let mut prompt = String::new();
    if !existing_topics.is_empty() {
        prompt.push_str("Existing topics: ");
        prompt.push_str(&existing_topics.join(", "));
        prompt.push_str("\n\n");
    }
    prompt.push_str("Classify these memories:\n\n");

    for m in batch {
        prompt.push_str(&format!("[id={}]\n", m.id));
        prompt.push_str(&format!("title: {}\n", m.title));
        if !m.description.is_empty() {
            prompt.push_str(&format!("description: {}\n", m.description));
        }
        if let Some(t) = &m.memory_type {
            prompt.push_str(&format!("type: {}\n", t));
        }
        let content_preview = truncate(&m.content, CLASSIFY_MAX_CONTENT);
        prompt.push_str(&format!("content: {}\n\n", content_preview));
    }

    let response = client.analyze(CLASSIFY_SYSTEM, &prompt).await?;
    let json = extract_json(&response.text);
    let parsed: AiClassifyResponse = serde_json::from_str(json)
        .map_err(|e| format!("parse classify response: {} (raw: {})", e, truncate(json, 200)))?;

    Ok(parsed.assignments)
}

fn apply_classifications(
    assignments: &[Assignment],
    existing_topics: &[String],
    report: &mut OrganizerReport,
) -> Result<(), String> {
    for a in assignments {
        // Ensure topic exists
        let topic_name = normalize_topic(&a.topic);
        if topic_name.is_empty() {
            continue;
        }

        if !existing_topics.contains(&topic_name)
            && !report.new_topics_created.contains(&topic_name)
        {
            topics::ensure(&topic_name, None, None)?;
            report.new_topics_created.push(topic_name.clone());
        }

        // Get current memory to preserve title/description/content
        if let Some(m) = memories::get(&a.id)? {
            memories::update(&m.id, &m.title, &m.description, &m.content, Some(&topic_name))?;
            report.classified_count += 1;
        }
    }
    Ok(())
}

/// Dedup only topics that have memories created/updated since `since_ts`.
async fn dedup_changed_topics(
    client: &ClaudeClient,
    report: &mut OrganizerReport,
    since_ts: i64,
) -> Result<(), String> {
    let changed_topics = memories::list_topics_changed_since(since_ts)?;
    if changed_topics.is_empty() {
        return Ok(());
    }

    for topic_name in changed_topics {
        let mems = memories::list_by_topic(&topic_name)?;
        if mems.len() < DEDUP_MIN_TOPIC_SIZE {
            continue;
        }

        match dedup_batch(client, &mems).await {
            Ok(merges) => {
                for merge in merges {
                    match apply_merge(&merge, &topic_name) {
                        Ok(()) => report.merged_count += 1,
                        Err(e) => report
                            .errors
                            .push(format!("apply merge in {}: {}", topic_name, e)),
                    }
                }
            }
            Err(e) => report.errors.push(format!("dedup {}: {}", topic_name, e)),
        }
    }
    Ok(())
}

async fn dedup_batch(
    client: &ClaudeClient,
    batch: &[memories::Memory],
) -> Result<Vec<Merge>, String> {
    let mut prompt = String::new();
    prompt.push_str("These memories are in the same topic. Identify near-duplicates:\n\n");

    for m in batch {
        prompt.push_str(&format!("[id={}]\n", m.id));
        prompt.push_str(&format!("title: {}\n", m.title));
        if !m.description.is_empty() {
            prompt.push_str(&format!("description: {}\n", m.description));
        }
        let content_preview = truncate(&m.content, DEDUP_MAX_CONTENT);
        prompt.push_str(&format!("content: {}\n\n", content_preview));
    }

    let response = client.analyze(DEDUP_SYSTEM, &prompt).await?;
    let json = extract_json(&response.text);
    let parsed: AiDedupResponse = serde_json::from_str(json)
        .map_err(|e| format!("parse dedup response: {} (raw: {})", e, truncate(json, 200)))?;

    Ok(parsed.merges)
}

fn apply_merge(merge: &Merge, topic: &str) -> Result<(), String> {
    // Validate that all source ids exist
    let mut source_memories = Vec::new();
    for id in &merge.source_ids {
        if let Some(m) = memories::get(id)? {
            source_memories.push(m);
        }
    }
    if source_memories.len() < 2 {
        return Err("merge requires at least 2 source memories".to_string());
    }

    // Snapshot originals for undo BEFORE any destructive op
    let snapshot = serde_json::json!({
        "action": "merge",
        "topic": topic,
        "originals": source_memories,
        "merged_title": merge.merged_title,
    });
    history::record("merge", snapshot)?;

    // Preserve project scope if all sources share one; otherwise go global
    // (cross-project merges lose their project affinity rather than get wrongly-scoped).
    let shared_project: Option<String> = {
        let first = source_memories[0].project.clone();
        if source_memories.iter().all(|m| m.project == first) {
            first
        } else {
            None
        }
    };

    // Insert the merged memory. Ordering: insert first, then delete sources.
    // If this errors halfway, we can still recover from the history log.
    memories::insert(memories::NewMemory {
        title: merge.merged_title.clone(),
        description: merge.merged_description.clone(),
        content: merge.merged_content.clone(),
        memory_type: source_memories[0].memory_type.clone(),
        topic: Some(topic.to_string()),
        source: Some("auto_merged".to_string()),
        project: shared_project,
    })?;

    // Delete originals
    for m in &source_memories {
        memories::delete(&m.id)?;
    }

    Ok(())
}

/// Undo the most recent merge by restoring the source memories and deleting the merged one.
pub fn undo_last() -> Result<String, String> {
    let entry = history::get_most_recent()?.ok_or_else(|| "no history".to_string())?;

    let snapshot: serde_json::Value = serde_json::from_str(&entry.snapshot)
        .map_err(|e| format!("parse snapshot: {}", e))?;

    let action = snapshot
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match action {
        "merge" => undo_merge(&snapshot, &entry)?,
        "consolidate" => undo_consolidate(&snapshot)?,
        "relate" => undo_relate(&snapshot)?,
        _ => return Err(format!("unknown action: {}", action)),
    };

    history::delete_entry(entry.id)?;
    Ok(format!("Undid: {}", action))
}

/// Undo a topic consolidation by re-assigning memories back to their original topics.
fn undo_consolidate(snapshot: &serde_json::Value) -> Result<(), String> {
    let affected = snapshot
        .get("affected")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "snapshot missing affected list".to_string())?;

    for entry in affected {
        let source_topic = entry
            .get("source_topic")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let memory_id = entry
            .get("memory_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if source_topic.is_empty() || memory_id.is_empty() {
            continue;
        }

        // Ensure the source topic exists (it may have been deleted when emptied)
        topics::ensure(source_topic, None, None)?;

        // Re-assign the memory
        if let Some(m) = memories::get(memory_id)? {
            memories::update(&m.id, &m.title, &m.description, &m.content, Some(source_topic))?;
        }
    }

    Ok(())
}

fn undo_merge(
    snapshot: &serde_json::Value,
    _entry: &history::HistoryEntry,
) -> Result<(), String> {
    let originals = snapshot
        .get("originals")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "snapshot missing originals".to_string())?;

    let merged_title = snapshot
        .get("merged_title")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Delete the merged memory (find by title — not perfect but works for recent undo)
    if !merged_title.is_empty() {
        let all = memories::list_all()?;
        for m in all {
            if m.title == merged_title && m.source.as_deref() == Some("auto_merged") {
                let _ = memories::delete(&m.id);
                break;
            }
        }
    }

    // Re-insert the originals
    for orig in originals {
        let title = orig
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Restored")
            .to_string();
        let description = orig
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let content = orig
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let topic = orig.get("topic").and_then(|v| v.as_str()).map(String::from);
        let memory_type = orig
            .get("memory_type")
            .and_then(|v| v.as_str())
            .map(String::from);
        let project = orig.get("project").and_then(|v| v.as_str()).map(String::from);

        let _ = memories::insert(memories::NewMemory {
            title,
            description,
            content,
            memory_type,
            topic,
            source: Some("restored_by_undo".to_string()),
            project,
        });
    }

    Ok(())
}

/// Undo a relate pass by removing the edges that were created.
fn undo_relate(snapshot: &serde_json::Value) -> Result<(), String> {
    let ai_edges = snapshot
        .get("edges")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "snapshot missing edges list".to_string())?;

    for edge_val in ai_edges {
        let source_id = edge_val.get("source_id").and_then(|v| v.as_str()).unwrap_or("");
        let target_id = edge_val.get("target_id").and_then(|v| v.as_str()).unwrap_or("");
        let edge_type = edge_val.get("edge_type").and_then(|v| v.as_str()).unwrap_or("");

        if source_id.is_empty() || target_id.is_empty() || edge_type.is_empty() {
            continue;
        }

        // Delete the specific edge by its unique constraint fields
        let _ = crate::store::with_conn(|conn| {
            conn.execute(
                "DELETE FROM memory_edges WHERE source_id = ?1 AND target_id = ?2 AND edge_type = ?3 AND source_origin = 'ai_discovered'",
                rusqlite::params![source_id, target_id, edge_type],
            )
            .map_err(|e| format!("undo relate edge: {}", e))?;
            Ok(())
        });
    }

    Ok(())
}

/// Initial full relate pass — runs once to seed the graph across all topics.
async fn relate_all_topics(
    client: &ClaudeClient,
    report: &mut OrganizerReport,
) -> Result<(), String> {
    let all_topics = topics::list_all()?;

    for topic in &all_topics {
        let mems = memories::list_by_topic(&topic.name)?;
        if mems.len() < 2 {
            continue;
        }

        for chunk in mems.chunks(RELATE_BATCH_SIZE) {
            match relate_batch(client, chunk).await {
                Ok(ai_edges) => {
                    if !ai_edges.is_empty() {
                        apply_discovered_edges(&ai_edges, &topic.name, report)?;
                    }
                }
                Err(e) => report.errors.push(format!("relate {}: {}", topic.name, e)),
            }
        }
    }

    Ok(())
}

/// Discover relationships between memories within topics that received
/// new classifications this pass. Only processes topics from the report's
/// new_topics_created list plus any existing topics that got new members.
async fn relate_newly_classified_topics(
    client: &ClaudeClient,
    report: &mut OrganizerReport,
) -> Result<(), String> {
    // Collect unique topic names that were touched by classification
    let mut topics_to_relate: Vec<String> = report.new_topics_created.clone();

    // Also check existing topics that may have received new classifications
    // by scanning for topics with recently updated memories (just the last few seconds)
    let recent_ts = chrono::Utc::now().timestamp() - 30; // last 30 seconds
    if let Ok(changed) = memories::list_topics_changed_since(recent_ts) {
        for t in changed {
            if !topics_to_relate.contains(&t) {
                topics_to_relate.push(t);
            }
        }
    }

    for topic_name in &topics_to_relate {
        let mems = memories::list_by_topic(topic_name)?;
        if mems.len() < 2 {
            continue;
        }

        // Process in batches
        for chunk in mems.chunks(RELATE_BATCH_SIZE) {
            match relate_batch(client, chunk).await {
                Ok(ai_edges) => {
                    if !ai_edges.is_empty() {
                        apply_discovered_edges(&ai_edges, topic_name, report)?;
                    }
                }
                Err(e) => report.errors.push(format!("relate {}: {}", topic_name, e)),
            }
        }
    }

    Ok(())
}

async fn relate_batch(
    client: &ClaudeClient,
    batch: &[memories::Memory],
) -> Result<Vec<AiEdge>, String> {
    let mut prompt = String::new();
    prompt.push_str("Identify relationships between these memories:\n\n");

    for m in batch {
        prompt.push_str(&format!("[id={}]\n", m.id));
        prompt.push_str(&format!("title: {}\n", m.title));
        if !m.description.is_empty() {
            prompt.push_str(&format!("description: {}\n", m.description));
        }
        let content_preview = truncate(&m.content, RELATE_MAX_CONTENT);
        prompt.push_str(&format!("content: {}\n\n", content_preview));
    }

    let response = client.analyze(RELATE_SYSTEM, &prompt).await?;
    let json = extract_json(&response.text);
    let parsed: AiRelateResponse = serde_json::from_str(json)
        .map_err(|e| format!("parse relate response: {} (raw: {})", e, truncate(json, 200)))?;

    Ok(parsed.edges)
}

fn apply_discovered_edges(
    ai_edges: &[AiEdge],
    topic: &str,
    report: &mut OrganizerReport,
) -> Result<(), String> {
    // Snapshot before applying (for undo)
    let snapshot = serde_json::json!({
        "action": "relate",
        "topic": topic,
        "edges": ai_edges,
    });
    history::record("relate", snapshot)?;

    let valid_types = ["relates-to", "supersedes", "depends-on", "contradicts"];

    for ai_edge in ai_edges {
        let edge_type = ai_edge.edge_type.trim().to_lowercase();
        if !valid_types.contains(&edge_type.as_str()) {
            continue;
        }

        // Verify both memories exist
        let source_exists = memories::get(&ai_edge.source_id)?.is_some();
        let target_exists = memories::get(&ai_edge.target_id)?.is_some();
        if !source_exists || !target_exists {
            continue;
        }

        match edges::insert(
            &ai_edge.source_id,
            &ai_edge.target_id,
            &edge_type,
            0.5,
            "ai_discovered",
        ) {
            Ok(_) => report.edges_created += 1,
            Err(e) => report.errors.push(format!("edge insert: {}", e)),
        }
    }

    Ok(())
}

fn normalize_topic(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(n).collect::<String>())
    }
}

fn extract_json(text: &str) -> &str {
    let trimmed = text.trim();

    // Strip ```json fences
    if let Some(start) = trimmed.find("```json") {
        let after = &trimmed[start + 7..];
        if let Some(end) = after.find("```") {
            return after[..end].trim();
        }
    }
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let after = if let Some(nl) = after.find('\n') {
            &after[nl + 1..]
        } else {
            after
        };
        if let Some(end) = after.find("```") {
            return after[..end].trim();
        }
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_topic() {
        assert_eq!(normalize_topic("Deployment"), "deployment");
        assert_eq!(normalize_topic("  User Profile  "), "user-profile");
        assert_eq!(normalize_topic("api/design"), "api-design");
        assert_eq!(normalize_topic("--kebab--case--"), "kebab--case");
    }

    #[test]
    fn test_extract_json_plain() {
        assert_eq!(extract_json(r#"{"foo": 1}"#), r#"{"foo": 1}"#);
    }

    #[test]
    fn test_extract_json_fenced() {
        let input = "```json\n{\"foo\": 1}\n```";
        assert_eq!(extract_json(input), r#"{"foo": 1}"#);
    }

    #[test]
    fn test_extract_json_plain_fence() {
        let input = "```\n{\"foo\": 1}\n```";
        assert_eq!(extract_json(input), r#"{"foo": 1}"#);
    }
}
