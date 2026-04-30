//! Auto-organization engine. Classifies untopiced memories into topics,
//! merges near-duplicates within topics, and consolidates overlapping topics.
//! Uses `claude -p` under the hood.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::services::claude_api::ClaudeClient;
use crate::store::{edges, history, memories, settings, topics};

pub const PROGRESS_EVENT: &str = "organizer:progress";

#[derive(Serialize, Clone)]
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
            OrganizerProgress {
                phase,
                message: message.into(),
                current,
                total,
            },
        );
    }
}

const SETTING_LAST_ORGANIZE_TS: &str = "last_organize_ts";
const SETTING_INITIAL_RELATE_DONE: &str = "initial_relate_done";
pub const SETTING_SPLIT_THRESHOLD: &str = "split_threshold";
const SETTING_SPLIT_LAST_SIZE_PREFIX: &str = "split_last_size:";
const SETTING_SPLIT_LAST_THRESHOLD_PREFIX: &str = "split_last_threshold:";

const CLASSIFY_BATCH_SIZE: usize = 25;
const CLASSIFY_MAX_CONTENT: usize = 300;
const DEDUP_MAX_CONTENT: usize = 1000;
const DEDUP_MIN_TOPIC_SIZE: usize = 2;
const CONSOLIDATE_SAMPLE_PER_TOPIC: usize = 6;
const RELATE_BATCH_SIZE: usize = 20;
const RELATE_MAX_CONTENT: usize = 400;
pub const SPLIT_DEFAULT_THRESHOLD: usize = 15;
pub const SPLIT_THRESHOLD_MIN: usize = 5;
const SPLIT_MIN_SUB_TOPIC_SIZE: usize = 3;
const SPLIT_GROWTH_RATIO: f64 = 1.3;
const SPLIT_MAX_CONTENT: usize = 300;

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

const SPLIT_SYSTEM: &str = r#"You evaluate whether a topic bucket has grown too broad and should be split into narrower sub-topics.

You are shown one topic and all memories currently classified under it. Decide whether the memories form two or more coherent sub-groups that deserve their own topics.

SPLIT when:
1. The memories clearly cluster into 2+ distinct themes (e.g. a "deployment" bucket that holds both "kubernetes" and "kamal" content).
2. Each candidate sub-group has at least 3 members with meaningfully different focus from the others.
3. The new sub-topic names would be as reusable as a normal top-level topic (short, functional, not project-specific).

DO NOT SPLIT when:
1. The memories all share one coherent theme, even if the bucket is large.
2. A proposed sub-group would have fewer than 3 members.
3. The split would just create a project-name topic (e.g. "hearth-deployment") — project-specific buckets are explicitly discouraged.
4. You're uncertain. A large coherent topic is better than a bad split.

When splitting, you may leave some members in the original topic by naming it as one of the sub-topics. Every member_id you list must currently be in the topic being evaluated.

Respond with ONLY valid JSON (no prose, no markdown fences). If no split is warranted:
{"split": false, "reason": "brief justification"}

If splitting:
{"split": true, "sub_topics": [{"name": "topic-a", "member_ids": ["id1", "id2", "id3"], "reason": "brief"}, {"name": "topic-b", "member_ids": ["id4", "id5", "id6"], "reason": "brief"}]}"#;

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
    pub split_topics: Vec<String>,
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

#[derive(Deserialize)]
struct AiSplitResponse {
    #[serde(default)]
    split: bool,
    #[serde(default)]
    sub_topics: Vec<SplitSubTopic>,
    #[serde(default)]
    reason: String,
}

#[derive(Deserialize, Clone)]
struct SplitSubTopic {
    name: String,
    member_ids: Vec<String>,
    #[serde(default)]
    reason: String,
}

/// Run a full organization pass: classify untopiced memories, dedup within topics,
/// then consolidate overlapping topics.
///
/// Only dedup/consolidate run on topics that changed since the last pass.
pub async fn run_full_pass(handle: Option<AppHandle>) -> Result<OrganizerReport, String> {
    let h = handle.as_ref();
    let mut report = OrganizerReport::default();
    let client = ClaudeClient::new(None);

    let last_ts: i64 = settings::get(SETTING_LAST_ORGANIZE_TS, "0")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .unwrap_or(0);
    let now_ts = chrono::Utc::now().timestamp();

    emit(h, "starting", "Preparing organizer", 0, 0);

    // Phase 1: classify untopiced memories
    match classify_untopiced(h, &client, &mut report).await {
        Ok(()) => {}
        Err(e) => report.errors.push(format!("classify: {}", e)),
    }

    // Phase 1.5: discover relationships.
    let initial_relate_done = settings::get_bool(SETTING_INITIAL_RELATE_DONE, false)
        .unwrap_or(false);

    if !initial_relate_done {
        match relate_all_topics(h, &client, &mut report).await {
            Ok(()) => {
                let _ = settings::set_bool(SETTING_INITIAL_RELATE_DONE, true);
            }
            Err(e) => report.errors.push(format!("relate (initial): {}", e)),
        }
    } else if report.classified_count > 0 {
        match relate_newly_classified_topics(h, &client, &mut report).await {
            Ok(()) => {}
            Err(e) => report.errors.push(format!("relate: {}", e)),
        }
    }

    // Phase 2: dedup only topics with new/updated memories since last pass
    match dedup_changed_topics(h, &client, &mut report, last_ts).await {
        Ok(()) => {}
        Err(e) => report.errors.push(format!("dedup: {}", e)),
    }

    // Phase 3: consolidate only if new classifications happened
    if report.classified_count > 0 || !report.new_topics_created.is_empty() {
        emit(h, "consolidate", "Consolidating overlapping topics", 0, 1);
        match consolidate_topics(&client, &mut report).await {
            Ok(()) => {}
            Err(e) => report.errors.push(format!("consolidate: {}", e)),
        }
    }

    // Phase 4: split oversized topics. Runs every pass — the per-topic
    // growth guard keeps cost bounded when nothing has changed meaningfully.
    match split_oversized_topics(h, &client, &mut report).await {
        Ok(()) => {}
        Err(e) => report.errors.push(format!("split: {}", e)),
    }

    let _ = settings::set(SETTING_LAST_ORGANIZE_TS, &now_ts.to_string());

    emit(h, "done", "Organize complete", 0, 0);

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

/// Split oversized topics (Phase 4). For each topic whose size exceeds the
/// configured threshold AND has grown meaningfully since the last evaluation,
/// ask the AI whether to split into narrower sub-topics and apply the result.
///
/// Growth guard: per-topic `split_last_size:<topic>` is stored after each
/// evaluation. A topic is only re-asked once its size reaches `last * 1.3`,
/// so cost stays bounded across recurring passes.
pub async fn split_oversized_topics(
    handle: Option<&AppHandle>,
    client: &ClaudeClient,
    report: &mut OrganizerReport,
) -> Result<(), String> {
    let threshold = load_split_threshold();
    let all_topics = topics::list_all()?;

    let candidates: Vec<&topics::Topic> = all_topics
        .iter()
        .filter(|t| (t.memory_count as usize) >= threshold)
        .filter(|t| should_reevaluate_split(&t.name, t.memory_count as usize, threshold))
        .collect();

    if candidates.is_empty() {
        return Ok(());
    }

    let total = candidates.len();
    for (i, topic) in candidates.iter().enumerate() {
        emit(
            handle,
            "split",
            format!("Evaluating split for '{}'", topic.name),
            i,
            total,
        );

        let mems = memories::list_by_topic(&topic.name)?;
        match evaluate_split(client, &topic.name, &mems).await {
            Ok(Some(response)) => match apply_split(&topic.name, &mems, &response) {
                Ok(Some(summary)) => report.split_topics.push(summary),
                Ok(None) => {}
                Err(e) => report
                    .errors
                    .push(format!("apply split {}: {}", topic.name, e)),
            },
            Ok(None) => {}
            Err(e) => report
                .errors
                .push(format!("evaluate split {}: {}", topic.name, e)),
        }

        // Record the size and threshold we evaluated at, so we don't re-ask
        // until the topic grows past the growth ratio OR threshold is lowered.
        record_split_evaluation(&topic.name, mems.len(), threshold);
    }

    Ok(())
}

fn load_split_threshold() -> usize {
    settings::get(SETTING_SPLIT_THRESHOLD, &SPLIT_DEFAULT_THRESHOLD.to_string())
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(SPLIT_DEFAULT_THRESHOLD)
}

fn should_reevaluate_split(topic: &str, current_size: usize, current_threshold: usize) -> bool {
    let size_key = format!("{}{}", SETTING_SPLIT_LAST_SIZE_PREFIX, topic);
    let last_size: usize = settings::get(&size_key, "0")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    if last_size == 0 {
        return true; // never evaluated
    }

    // If threshold was lowered since last eval, force a fresh look.
    let thresh_key = format!("{}{}", SETTING_SPLIT_LAST_THRESHOLD_PREFIX, topic);
    let last_threshold: usize = settings::get(&thresh_key, "0")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);

    if current_threshold < last_threshold {
        return true;
    }

    // Otherwise gate on meaningful growth.
    current_size as f64 >= (last_size as f64) * SPLIT_GROWTH_RATIO
}

fn record_split_evaluation(topic: &str, size: usize, threshold: usize) {
    let size_key = format!("{}{}", SETTING_SPLIT_LAST_SIZE_PREFIX, topic);
    let thresh_key = format!("{}{}", SETTING_SPLIT_LAST_THRESHOLD_PREFIX, topic);
    let _ = settings::set(&size_key, &size.to_string());
    let _ = settings::set(&thresh_key, &threshold.to_string());
}

async fn evaluate_split(
    client: &ClaudeClient,
    topic: &str,
    mems: &[memories::Memory],
) -> Result<Option<AiSplitResponse>, String> {
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "Topic: {} ({} memories)\n\n",
        topic,
        mems.len()
    ));
    prompt.push_str("Members:\n\n");
    for m in mems {
        prompt.push_str(&format!("[id={}]\n", m.id));
        prompt.push_str(&format!("title: {}\n", m.title));
        if !m.description.is_empty() {
            prompt.push_str(&format!("description: {}\n", m.description));
        }
        let content_preview = truncate(&m.content, SPLIT_MAX_CONTENT);
        prompt.push_str(&format!("content: {}\n\n", content_preview));
    }

    let response = client.analyze(SPLIT_SYSTEM, &prompt).await?;
    let json = extract_json(&response.text);
    let parsed: AiSplitResponse = serde_json::from_str(json)
        .map_err(|e| format!("parse split response: {} (raw: {})", e, truncate(json, 300)))?;

    if !parsed.split || parsed.sub_topics.is_empty() {
        return Ok(None);
    }

    Ok(Some(parsed))
}

/// Validate and apply a split. Returns Some(summary) on success, None if the
/// AI's split was rejected during validation (treated as a no-op, not an error).
fn apply_split(
    original_topic: &str,
    current_members: &[memories::Memory],
    response: &AiSplitResponse,
) -> Result<Option<String>, String> {
    let member_ids: std::collections::HashSet<&str> =
        current_members.iter().map(|m| m.id.as_str()).collect();

    // Normalize and validate sub-topic shape before touching any data.
    let mut normalized: Vec<(String, Vec<String>, String)> = Vec::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_members: std::collections::HashSet<String> = std::collections::HashSet::new();

    for sub in &response.sub_topics {
        let name = normalize_topic(&sub.name);
        if name.is_empty() {
            return Ok(None);
        }
        if !seen_names.insert(name.clone()) {
            return Ok(None); // duplicate sub-topic name
        }
        let mut filtered: Vec<String> = Vec::new();
        for id in &sub.member_ids {
            if !member_ids.contains(id.as_str()) {
                continue; // unknown id — skip, don't fail
            }
            if !seen_members.insert(id.clone()) {
                continue; // same member listed under two sub-topics — keep first
            }
            filtered.push(id.clone());
        }
        if filtered.len() < SPLIT_MIN_SUB_TOPIC_SIZE {
            return Ok(None);
        }
        normalized.push((name, filtered, sub.reason.clone()));
    }

    if normalized.len() < 2 {
        return Ok(None);
    }

    // Reject trivial splits where everything lands back in the original topic.
    if normalized.len() == 1 && normalized[0].0 == original_topic {
        return Ok(None);
    }

    // Snapshot BEFORE mutating, so undo can restore the original topic.
    let moves: Vec<serde_json::Value> = normalized
        .iter()
        .flat_map(|(target, ids, _)| {
            ids.iter().map(move |id| {
                serde_json::json!({ "memory_id": id, "target_topic": target })
            })
        })
        .collect();

    let snapshot = serde_json::json!({
        "action": "split",
        "original_topic": original_topic,
        "sub_topics": normalized
            .iter()
            .map(|(n, ids, r)| serde_json::json!({
                "name": n,
                "member_ids": ids,
                "reason": r,
            }))
            .collect::<Vec<_>>(),
        "moves": moves,
    });
    history::record("split", snapshot)?;

    // Ensure sub-topics exist.
    for (name, _, _) in &normalized {
        topics::ensure(name, None, None)?;
    }

    // Reassign each member.
    let mut total_moved = 0usize;
    let by_id: std::collections::HashMap<&str, &memories::Memory> =
        current_members.iter().map(|m| (m.id.as_str(), m)).collect();

    for (target, ids, _) in &normalized {
        for id in ids {
            if let Some(m) = by_id.get(id.as_str()) {
                if m.topic.as_deref() == Some(target.as_str()) {
                    continue; // staying in original (target == original_topic case)
                }
                memories::update(&m.id, &m.title, &m.description, &m.content, Some(target))?;
                total_moved += 1;
            }
        }
    }

    if total_moved == 0 {
        return Ok(None);
    }

    // If the original topic is now empty (every member moved elsewhere), remove it.
    let remaining = memories::list_by_topic(original_topic)?;
    if remaining.is_empty() {
        let _ = topics::delete_empty(original_topic);
    }

    // Reset size+threshold tracking for each new sub-topic so its own growth
    // curve starts fresh — otherwise a new sub-topic would inherit the
    // original's threshold and immediately qualify for re-evaluation.
    let current_threshold = load_split_threshold();
    for (name, _, _) in &normalized {
        if name != original_topic {
            let sub_size = memories::list_by_topic(name)?.len();
            record_split_evaluation(name, sub_size, current_threshold);
        }
    }

    let summary = format!(
        "{} → {} ({} memories)",
        original_topic,
        normalized
            .iter()
            .map(|(n, _, _)| n.clone())
            .collect::<Vec<_>>()
            .join("+"),
        total_moved
    );
    Ok(Some(summary))
}

/// Classify all untopiced memories in batches.
pub async fn classify_untopiced(
    handle: Option<&AppHandle>,
    client: &ClaudeClient,
    report: &mut OrganizerReport,
) -> Result<(), String> {
    let untopiced = memories::list_untopiced()?;
    if untopiced.is_empty() {
        return Ok(());
    }

    let existing_topics: Vec<String> =
        topics::list_all()?.into_iter().map(|t| t.name).collect();

    let total_batches = untopiced.chunks(CLASSIFY_BATCH_SIZE).count();
    for (i, chunk) in untopiced.chunks(CLASSIFY_BATCH_SIZE).enumerate() {
        emit(
            handle,
            "classify",
            format!("Classifying {} memories", chunk.len()),
            i,
            total_batches,
        );
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
    handle: Option<&AppHandle>,
    client: &ClaudeClient,
    report: &mut OrganizerReport,
    since_ts: i64,
) -> Result<(), String> {
    let changed_topics = memories::list_topics_changed_since(since_ts)?;
    if changed_topics.is_empty() {
        return Ok(());
    }

    let total = changed_topics.len();
    for (i, topic_name) in changed_topics.into_iter().enumerate() {
        let mems = memories::list_by_topic(&topic_name)?;
        if mems.len() < DEDUP_MIN_TOPIC_SIZE {
            continue;
        }

        emit(
            handle,
            "dedup",
            format!("Deduping topic '{}'", topic_name),
            i,
            total,
        );
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
        "split" => undo_split(&snapshot)?,
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

/// Undo a split by moving every affected memory back to the original topic
/// and removing any sub-topics that were created and are now empty.
fn undo_split(snapshot: &serde_json::Value) -> Result<(), String> {
    let original_topic = snapshot
        .get("original_topic")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "snapshot missing original_topic".to_string())?;

    let moves = snapshot
        .get("moves")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "snapshot missing moves".to_string())?;

    // Ensure the original topic exists again (it may have been removed when emptied).
    topics::ensure(original_topic, None, None)?;

    let mut touched_sub_topics: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for entry in moves {
        let memory_id = entry
            .get("memory_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let target_topic = entry
            .get("target_topic")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if memory_id.is_empty() {
            continue;
        }

        if !target_topic.is_empty() && target_topic != original_topic {
            touched_sub_topics.insert(target_topic.to_string());
        }

        if let Some(m) = memories::get(memory_id)? {
            memories::update(
                &m.id,
                &m.title,
                &m.description,
                &m.content,
                Some(original_topic),
            )?;
        }
    }

    // Drop any sub-topics the split created that are now empty.
    for sub in touched_sub_topics {
        let remaining = memories::list_by_topic(&sub)?;
        if remaining.is_empty() {
            let _ = topics::delete_empty(&sub);
        }
    }

    // Restore the original's size+threshold tracker so the split phase doesn't
    // immediately re-propose the same split on the next pass.
    let current_size = memories::list_by_topic(original_topic)?.len();
    record_split_evaluation(original_topic, current_size, load_split_threshold());

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
    handle: Option<&AppHandle>,
    client: &ClaudeClient,
    report: &mut OrganizerReport,
) -> Result<(), String> {
    let all_topics = topics::list_all()?;

    let total = all_topics.len();
    for (i, topic) in all_topics.iter().enumerate() {
        let mems = memories::list_by_topic(&topic.name)?;
        if mems.len() < 2 {
            continue;
        }

        emit(
            handle,
            "relate",
            format!("Relating memories in '{}'", topic.name),
            i,
            total,
        );
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
    handle: Option<&AppHandle>,
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

    let total = topics_to_relate.len();
    for (i, topic_name) in topics_to_relate.iter().enumerate() {
        let mems = memories::list_by_topic(topic_name)?;
        if mems.len() < 2 {
            continue;
        }

        emit(
            handle,
            "relate",
            format!("Relating memories in '{}'", topic_name),
            i,
            total,
        );
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

    #[test]
    fn test_split_response_no_split() {
        let raw = r#"{"split": false, "reason": "coherent"}"#;
        let parsed: AiSplitResponse = serde_json::from_str(raw).unwrap();
        assert!(!parsed.split);
        assert_eq!(parsed.reason, "coherent");
        assert!(parsed.sub_topics.is_empty());
    }

    #[test]
    fn test_split_response_with_sub_topics() {
        let raw = r#"{
            "split": true,
            "sub_topics": [
                {"name": "kubernetes", "member_ids": ["a", "b", "c"], "reason": "k8s-specific"},
                {"name": "kamal", "member_ids": ["d", "e", "f"], "reason": "kamal-specific"}
            ]
        }"#;
        let parsed: AiSplitResponse = serde_json::from_str(raw).unwrap();
        assert!(parsed.split);
        assert_eq!(parsed.sub_topics.len(), 2);
        assert_eq!(parsed.sub_topics[0].name, "kubernetes");
        assert_eq!(parsed.sub_topics[0].member_ids.len(), 3);
    }

    #[test]
    fn test_growth_guard_math() {
        // With ratio 1.3: last=10 means re-evaluate at 13+; last=15 means re-evaluate at 20+.
        let ratio = SPLIT_GROWTH_RATIO;
        assert!(10.0 * ratio <= 13.0);
        assert!(15.0 * ratio <= 20.0);
        assert!(15.0 * ratio > 19.0); // 19 is NOT enough growth from 15
    }
}
