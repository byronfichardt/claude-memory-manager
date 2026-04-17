//! Claude Code UserPromptSubmit hook handler.
//!
//! Invoked by Claude Code on every user message (before Claude processes it).
//! Reads a JSON event from stdin, queries the memory store with the prompt text,
//! and writes relevant memories to stdout as additional context.
//!
//! This gives deterministic memory retrieval — Claude sees relevant memories in
//! every turn without having to choose to call the memory_search tool.
//!
//! Exit 0 = allow the prompt to proceed (always, even on error — we never block).
//! Stdout text is injected into Claude's context for this turn.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::services::project;
use crate::store::{self, edges, memories};

const MAX_RESULTS: u32 = 5;
const MAX_SNIPPET_CHARS: usize = 300;
const MIN_PROMPT_LEN: usize = 4;
const MAX_TITLE_LEN: usize = 80;

#[derive(serde::Deserialize)]
struct HookEvent {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    transcript_path: String,
    #[allow(dead_code)]
    #[serde(default)]
    hook_event_name: String,
}

/// Resolve the active project for this hook invocation.
/// Priority: transcript inference > cwd git root > None.
fn detect_active_project(event: &HookEvent) -> Option<PathBuf> {
    if !event.transcript_path.is_empty() {
        if let Some(p) = project::infer_project_from_transcript(Path::new(&event.transcript_path)) {
            return Some(p);
        }
    }
    if !event.cwd.is_empty() {
        if let Some(p) = project::resolve_project(Path::new(&event.cwd)) {
            return Some(p);
        }
    }
    None
}

pub fn run() -> Result<(), String> {
    // Read JSON event from stdin
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("read stdin: {}", e))?;

    if input.trim().is_empty() {
        return Ok(());
    }

    let event: HookEvent = serde_json::from_str(&input)
        .map_err(|e| format!("parse hook event: {}", e))?;

    // Don't bother with very short prompts ("ok", "yes", etc.)
    if event.prompt.trim().len() < MIN_PROMPT_LEN {
        return Ok(());
    }

    // Hook runs as a fresh process per prompt — use a single raw connection
    // rather than spinning up a pool with a WAL checkpoint on every invoke.
    let conn = store::open_hook_connection().map_err(|e| format!("store init: {}", e))?;

    // Detect active project once per invocation — reused for retrieval and save.
    let active_project = detect_active_project(&event);

    // Publish the pointer so the MCP server (which doesn't see transcript_path)
    // can scope memory_add / memory_search to the same project the hook sees.
    project::write_active_project(&event.session_id, active_project.as_deref());

    let mut out = String::new();

    // Correction nudge goes first so it's the first thing Claude reads.
    // Fires independently of retrieval — even on empty memory context, a
    // correction should still prompt an immediate memory_add.
    if detect_correction_signal(&event.prompt) {
        out.push_str(CORRECTION_NUDGE);
        out.push('\n');
    }

    // Check for explicit "remember this" directive. If found, save it directly
    // as a deterministic manual memory.
    if let Some(text_to_save) = extract_remember_directive(&event.prompt) {
        match save_user_memory(&conn, text_to_save, active_project.as_deref()) {
            Ok(title) => {
                out.push_str("<memory-saved>\n");
                out.push_str(&format!(
                    "✓ Memory saved automatically via the user's `remember:` directive.\nTitle: {}\n",
                    title
                ));
                out.push_str("You do NOT need to call memory_add for this — it's already saved.\n");
                out.push_str("Acknowledge the save briefly and proceed with any other part of the user's request.\n");
                out.push_str("</memory-saved>\n\n");
            }
            Err(e) => {
                eprintln!("[hook] remember save failed: {}", e);
                // Fall through to normal retrieval anyway
            }
        }
    }

    // Hybrid retrieval: FTS5 + graph expansion + re-ranking + project affinity
    let final_hits = hybrid_search(&conn, &event.prompt, active_project.as_deref())?;

    if !final_hits.is_empty() {
        out.push_str("<memory-context>\n");
        out.push_str("Relevant memories from your persistent memory store (retrieved automatically):\n\n");

        for (i, hit) in final_hits.iter().enumerate() {
            let topic = hit.topic.as_deref().unwrap_or("untopiced");
            out.push_str(&format!("{}. **{}** _{}_", i + 1, hit.title, topic));
            if !hit.description.is_empty() {
                out.push_str(&format!(" — {}", hit.description));
            }
            out.push('\n');

            let snippet = clean_snippet(&hit.snippet);
            if !snippet.is_empty() {
                let truncated = truncate_chars(&snippet, MAX_SNIPPET_CHARS);
                out.push_str(&format!("   {}\n", truncated));
            }
            out.push('\n');
        }

        out.push_str("</memory-context>\n");

        // Strengthen co-access edges between results — one batched upsert
        // inside a single transaction instead of 2×N² sequential writes.
        if final_hits.len() > 1 {
            let ids: Vec<&str> = final_hits.iter().map(|h| h.id.as_str()).collect();
            let _ = edges::strengthen_co_access_batch(
                &conn,
                &ids,
                CO_ACCESS_INITIAL_WEIGHT,
                CO_ACCESS_DELTA,
            );
        }
    }

    // Always-on save checklist. Appended last so Claude sees it right before
    // composing its reply — the intended trigger point for the self-check.
    // Fires on every prompt (≥ MIN_PROMPT_LEN, already filtered above).
    out.push_str(SAVE_CHECKLIST);
    out.push('\n');

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    handle
        .write_all(out.as_bytes())
        .map_err(|e| format!("write stdout: {}", e))?;
    handle.flush().map_err(|e| format!("flush: {}", e))?;

    Ok(())
}

const FTS_OVERFETCH: u32 = 10;
const FTS_WEIGHT: f64 = 0.7;
const GRAPH_WEIGHT: f64 = 0.3;
const CO_ACCESS_INITIAL_WEIGHT: f64 = 0.1;
const CO_ACCESS_DELTA: f64 = 0.05;

/// Hybrid search: FTS5 keyword search + 1-hop graph expansion + re-ranking + project affinity.
///
/// 1. Over-fetch FTS candidates (10 instead of 5)
/// 2. Walk 1-hop graph neighbors of FTS hits
/// 3. Fetch any neighbor memories not already in FTS results
/// 4. Re-rank using combined FTS + graph + project affinity score
/// 5. Return top MAX_RESULTS
fn hybrid_search(
    conn: &Connection,
    prompt: &str,
    current_project: Option<&Path>,
) -> Result<Vec<memories::SearchHit>, String> {
    // Step 1: Get FTS candidates (over-fetch for re-ranking headroom)
    let fts_hits = memories::search_with_conn(conn, prompt, Some(FTS_OVERFETCH))
        .map_err(|e| format!("search: {}", e))?;

    if fts_hits.is_empty() {
        return Ok(fts_hits);
    }

    // Step 2: Get 1-hop graph neighbors
    let fts_ids: Vec<&str> = fts_hits.iter().map(|h| h.id.as_str()).collect();
    let neighbor_edges = edges::get_neighbors_batch_with_conn(conn, &fts_ids).unwrap_or_default();

    // If no edges exist yet, just return the top FTS hits directly
    if neighbor_edges.is_empty() {
        return Ok(fts_hits.into_iter().take(MAX_RESULTS as usize).collect());
    }

    // Normalize FTS scores (BM25 in SQLite FTS5: lower = better, all negative)
    let min_score = fts_hits.iter().map(|h| h.score).fold(f64::INFINITY, f64::min);
    let max_score = fts_hits.iter().map(|h| h.score).fold(f64::NEG_INFINITY, f64::max);
    let score_range = (max_score - min_score).abs();

    let normalize_fts = |score: f64| -> f64 {
        if score_range < f64::EPSILON {
            1.0
        } else {
            // Invert because lower BM25 = better match
            1.0 - ((score - min_score) / score_range)
        }
    };

    // Build a map of memory_id -> normalized FTS score
    let mut fts_score_map: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();
    for hit in &fts_hits {
        fts_score_map.insert(&hit.id, normalize_fts(hit.score));
    }

    // Step 3: Find neighbor memory IDs not already in FTS results
    let fts_id_set: std::collections::HashSet<&str> = fts_ids.iter().copied().collect();
    let mut neighbor_ids: Vec<String> = Vec::new();
    for edge in &neighbor_edges {
        let other = if fts_id_set.contains(edge.source_id.as_str()) {
            &edge.target_id
        } else {
            &edge.source_id
        };
        if !fts_id_set.contains(other.as_str()) && !neighbor_ids.contains(other) {
            neighbor_ids.push(other.clone());
        }
    }

    // Step 4: Compute graph boost for all candidates
    // graph_boost(memory) = avg(edge.weight * connected_fts_hit_score) for edges connecting to FTS hits
    let mut graph_boost: std::collections::HashMap<String, (f64, usize)> = std::collections::HashMap::new();

    for edge in &neighbor_edges {
        // For each edge, determine which end is an FTS hit and compute boost for the other end
        let (fts_end, other_end) = if fts_score_map.contains_key(edge.source_id.as_str()) {
            (edge.source_id.as_str(), edge.target_id.as_str())
        } else if fts_score_map.contains_key(edge.target_id.as_str()) {
            (edge.target_id.as_str(), edge.source_id.as_str())
        } else {
            continue;
        };

        let fts_score = fts_score_map.get(fts_end).copied().unwrap_or(0.0);
        let boost_value = edge.weight * fts_score;

        let entry = graph_boost.entry(other_end.to_string()).or_insert((0.0, 0));
        entry.0 += boost_value;
        entry.1 += 1;

        // Also boost the FTS hit itself (bidirectional benefit)
        let entry = graph_boost.entry(fts_end.to_string()).or_insert((0.0, 0));
        entry.0 += edge.weight * 0.5; // smaller self-boost
        entry.1 += 1;
    }

    // Step 5: Score and rank all candidates
    struct ScoredHit {
        hit: memories::SearchHit,
        combined_score: f64,
    }

    let mut scored: Vec<ScoredHit> = Vec::new();

    // Score FTS hits
    for hit in fts_hits {
        let norm_fts = normalize_fts(hit.score);
        let g_boost = graph_boost
            .get(&hit.id)
            .map(|(sum, count)| if *count > 0 { sum / *count as f64 } else { 0.0 })
            .unwrap_or(0.0);
        let affinity = project::project_affinity(hit.project.as_deref(), current_project);
        let combined = FTS_WEIGHT * norm_fts + GRAPH_WEIGHT * g_boost + affinity;
        scored.push(ScoredHit { hit, combined_score: combined });
    }

    // Fetch and score graph-only neighbors (no FTS signal)
    if !neighbor_ids.is_empty() {
        let id_refs: Vec<&str> = neighbor_ids.iter().map(|s| s.as_str()).collect();
        if let Ok(neighbor_memories) = memories::get_by_ids_with_conn(conn, &id_refs) {
            for mem in neighbor_memories {
                let g_boost = graph_boost
                    .get(&mem.id)
                    .map(|(sum, count)| if *count > 0 { sum / *count as f64 } else { 0.0 })
                    .unwrap_or(0.0);
                let affinity = project::project_affinity(mem.project.as_deref(), current_project);
                let combined = GRAPH_WEIGHT * g_boost + affinity; // No FTS signal

                // Convert Memory to SearchHit for uniform output
                let snippet = truncate_chars(&mem.content, MAX_SNIPPET_CHARS);
                scored.push(ScoredHit {
                    hit: memories::SearchHit {
                        id: mem.id,
                        title: mem.title,
                        description: mem.description,
                        snippet,
                        topic: mem.topic,
                        memory_type: mem.memory_type,
                        project: mem.project,
                        score: 0.0, // no FTS score
                    },
                    combined_score: combined,
                });
            }
        }
    }

    // Sort by combined score (descending)
    scored.sort_by(|a, b| b.combined_score.partial_cmp(&a.combined_score).unwrap_or(std::cmp::Ordering::Equal));

    // Return top MAX_RESULTS
    let results: Vec<memories::SearchHit> = scored
        .into_iter()
        .take(MAX_RESULTS as usize)
        .map(|s| s.hit)
        .collect();

    Ok(results)
}

const CORRECTION_NUDGE: &str = "\
<memory-correction-detected>
The user's message reads as a correction or pushback. Before responding to the rest of it, call memory_add with type=feedback to capture: (1) the rule, (2) why it matters. Save first, reply second — the save is the point, the reply is the easy part.
</memory-correction-detected>";

const SAVE_CHECKLIST: &str = "\
<memory-save-checklist>
Before replying, scan this turn for memory-worthy signals:
- user correction or stated preference/convention
- non-obvious project fact, debugging gotcha + fix, or architecture decision + rationale
- project state change (status, ownership, deadlines)
If any apply, call memory_add NOW — do not batch to end of session. Saves are cheap; reconstructing lost context is not.
</memory-save-checklist>";

/// Heuristic detector for correction/pushback signals in a user prompt.
///
/// Two classes of trigger:
/// 1. Start-of-message words: "no", "don't", "do not", "stop", "actually",
///    "wait", "nope" — corrections usually lead the message.
/// 2. Anywhere phrases: "you missed", "you misunderstood", "that's wrong",
///    "that is wrong", "not quite", "that's not right", "that's not what",
///    "i already" — strong signals regardless of position.
///
/// Kept tight on purpose: false positives force Claude to waste a turn
/// deciding not to save, which defeats the nudge.
fn detect_correction_signal(prompt: &str) -> bool {
    let lower = prompt.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }

    const START_TRIGGERS: &[&str] = &[
        "no", "don't", "dont", "do not", "stop", "actually", "wait", "nope",
    ];
    for trigger in START_TRIGGERS {
        if starts_with_word(&lower, trigger) {
            return true;
        }
    }

    const ANYWHERE_TRIGGERS: &[&str] = &[
        "you missed",
        "you misunderstood",
        "that's wrong",
        "that is wrong",
        "not quite",
        "that's not right",
        "that's not what",
        "i already",
    ];
    for phrase in ANYWHERE_TRIGGERS {
        if contains_phrase(&lower, phrase) {
            return true;
        }
    }

    false
}

/// True if `haystack` starts with `needle` followed by a non-word character
/// (or end-of-string). Avoids matching "notebook" when looking for "no".
/// Both args assumed lowercased.
fn starts_with_word(haystack: &str, needle: &str) -> bool {
    if !haystack.starts_with(needle) {
        return false;
    }
    match haystack[needle.len()..].chars().next() {
        None => true,
        Some(c) => !is_word_char(c),
    }
}

/// True if `haystack` contains `needle` with word boundaries on both sides.
/// Both args assumed lowercased.
fn contains_phrase(haystack: &str, needle: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let abs = start + pos;
        let before_ok = abs == 0
            || !is_word_char(haystack[..abs].chars().last().unwrap_or(' '));
        let after_idx = abs + needle.len();
        let after_ok = after_idx >= haystack.len()
            || !is_word_char(haystack[after_idx..].chars().next().unwrap_or(' '));
        if before_ok && after_ok {
            return true;
        }
        start = abs + needle.len().max(1);
    }
    false
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '\'' || c == '_'
}

/// Detects explicit save-to-memory directives at the start of a user prompt.
/// Returns the text content to save if found.
///
/// Recognized patterns (case-insensitive on the prefix):
/// - "remember: <text>"
/// - "remember that <text>"
/// - "/remember <text>"
/// - "!remember <text>"
fn extract_remember_directive(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    let lower = trimmed.to_lowercase();

    const PREFIXES: &[&str] = &[
        "remember: ",
        "remember:",
        "remember that ",
        "/remember ",
        "/remember:",
        "!remember ",
        "!remember:",
    ];

    for prefix in PREFIXES {
        if lower.starts_with(prefix) {
            let rest = trimmed[prefix.len()..].trim();
            if rest.len() >= MIN_PROMPT_LEN {
                return Some(rest.to_string());
            }
        }
    }

    None
}

/// Save a user-directed memory. Derives a title from the first sentence or
/// first MAX_TITLE_LEN chars. Content is the full text. Returns the title.
///
/// NOTE: `remember:` directives are currently classified as type=user, which is
/// always global per scope rules. The `active_project` param is plumbed through
/// for future directives that support scoped saves (e.g. `remember-here:`).
fn save_user_memory(
    conn: &Connection,
    text: String,
    active_project: Option<&Path>,
) -> Result<String, String> {
    let title = derive_title(&text);
    let description = String::new();

    let memory_type = Some("user".to_string());
    let project = project::resolve_memory_scope(memory_type.as_deref(), None, active_project);

    let memory = memories::insert_with_conn(
        conn,
        memories::NewMemory {
            title: title.clone(),
            description,
            content: text,
            memory_type,
            topic: None,
            source: Some("user_remember_directive".to_string()),
            project,
        },
    )?;

    Ok(memory.title)
}

fn derive_title(text: &str) -> String {
    let trimmed = text.trim();
    // Take first sentence or first N chars, whichever is shorter
    let first_sentence = trimmed
        .split(|c| c == '.' || c == '!' || c == '?' || c == '\n')
        .next()
        .unwrap_or(trimmed)
        .trim();

    if first_sentence.chars().count() <= MAX_TITLE_LEN {
        first_sentence.to_string()
    } else {
        let truncated: String = first_sentence.chars().take(MAX_TITLE_LEN).collect();
        format!("{}...", truncated)
    }
}

/// Clean FTS5 snippet markers. SearchHit's snippet has `[word]` around matches.
fn clean_snippet(s: &str) -> String {
    s.replace('[', "").replace(']', "").trim().to_string()
}

fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(n).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_remember_directive() {
        assert_eq!(
            extract_remember_directive("remember: we use port 5432 for postgres"),
            Some("we use port 5432 for postgres".to_string())
        );
        assert_eq!(
            extract_remember_directive("Remember: use kebab-case for new files"),
            Some("use kebab-case for new files".to_string())
        );
        assert_eq!(
            extract_remember_directive("/remember the backup runs at 3am"),
            Some("the backup runs at 3am".to_string())
        );
        assert_eq!(
            extract_remember_directive("!remember skip tests locally"),
            Some("skip tests locally".to_string())
        );
        assert_eq!(
            extract_remember_directive("remember that apples are red"),
            Some("apples are red".to_string())
        );
        // Too short
        assert_eq!(extract_remember_directive("remember: a"), None);
        // No directive
        assert_eq!(extract_remember_directive("how do I deploy this"), None);
        // Partial match — "remember" without colon shouldn't trigger
        assert_eq!(extract_remember_directive("I need to remember to update"), None);
    }

    #[test]
    fn test_detect_correction_signal_start_triggers() {
        assert!(detect_correction_signal("no, that's not what I meant"));
        assert!(detect_correction_signal("No that's wrong"));
        assert!(detect_correction_signal("don't do that"));
        assert!(detect_correction_signal("dont do that"));
        assert!(detect_correction_signal("do not use mocks here"));
        assert!(detect_correction_signal("stop adding comments"));
        assert!(detect_correction_signal("actually, use the other approach"));
        assert!(detect_correction_signal("wait, back up"));
        assert!(detect_correction_signal("nope try again"));
    }

    #[test]
    fn test_detect_correction_signal_anywhere_triggers() {
        assert!(detect_correction_signal("you missed the auth check"));
        assert!(detect_correction_signal("i think you misunderstood"));
        assert!(detect_correction_signal("that's wrong — it's port 5433"));
        assert!(detect_correction_signal("hmm that is wrong"));
        assert!(detect_correction_signal("not quite, try again"));
        assert!(detect_correction_signal("that's not right"));
        assert!(detect_correction_signal("that's not what I asked for"));
        assert!(detect_correction_signal("i already said to skip that file"));
    }

    #[test]
    fn test_detect_correction_signal_negatives() {
        // Word boundary: don't fire on "notebook", "notes", "nope" only as prefix word
        assert!(!detect_correction_signal("notebook setup question"));
        assert!(!detect_correction_signal("notes on the API"));
        assert!(!detect_correction_signal("how do I use stopwatch mode"));
        // Benign use of trigger words mid-sentence
        assert!(!detect_correction_signal("can you add a stop button"));
        assert!(!detect_correction_signal("there is no error handling yet"));
        assert!(!detect_correction_signal("how should I implement this"));
        // Empty / whitespace
        assert!(!detect_correction_signal(""));
        assert!(!detect_correction_signal("   "));
    }

    #[test]
    fn test_starts_with_word_boundary() {
        assert!(starts_with_word("no, try again", "no"));
        assert!(starts_with_word("no", "no"));
        assert!(!starts_with_word("notebook is broken", "no"));
        assert!(!starts_with_word("none of this works", "no"));
        assert!(starts_with_word("don't", "don't"));
    }

    #[test]
    fn test_contains_phrase_boundary() {
        assert!(contains_phrase("you missed it", "you missed"));
        assert!(contains_phrase("hmm, you missed.", "you missed"));
        assert!(!contains_phrase("you missedit", "you missed"));
        assert!(!contains_phrase("ayou missed", "you missed"));
    }

    #[test]
    fn test_derive_title() {
        assert_eq!(derive_title("short fact"), "short fact");
        assert_eq!(derive_title("First sentence. More details here"), "First sentence");
        assert_eq!(
            derive_title("Port 5432 is used for postgres on the production server"),
            "Port 5432 is used for postgres on the production server"
        );
        // Long first sentence gets truncated with ellipsis
        let long = "x".repeat(100);
        let title = derive_title(&long);
        assert!(title.ends_with("..."));
        assert!(title.len() <= MAX_TITLE_LEN + 3);
    }
}
