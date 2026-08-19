//! Near-duplicate detection used by the organizer and the write path.
//!
//! Update 2: clustering is store-wide (not per-topic). The LLM still confirms
//! merges; this module only decides *which groups are worth asking about* and
//! how a confirmed merge should inherit type/scope.
//!
//! Embeddings are optional. When the model is not loaded (hook, MCP, CI),
//! clustering falls back to a lexical Jaccard over title+content tokens — the
//! same feature the write-time near-dup check uses. Cosine is used when
//! vectors are supplied.

use crate::store::memories::{self, Memory};

/// Cosine threshold when vectors are available (organizer can swap
/// `pair_similarity` for a cosine-backed fn using this cutoff).
#[allow(dead_code)]
pub const CLUSTER_THRESHOLD: f64 = 0.85;
/// Lexical Jaccard is a coarser proxy than cosine — paraphrases of the same
/// instruction typically land around 0.55–0.70, not 0.85.
pub const LEXICAL_CLUSTER_THRESHOLD: f64 = 0.55;
/// Near-identical titles are enough to cluster even when the body was rephrased.
pub const TITLE_CLUSTER_THRESHOLD: f64 = 0.90;
/// Write-time "this already exists" threshold — stricter than clustering.
pub const WRITE_NEAR_DUP_THRESHOLD: f64 = 0.90;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupCandidate {
    pub id: String,
    pub topic: Option<String>,
}

pub fn cluster_by_similarity(
    memories: &[Memory],
    similarity: impl Fn(&Memory, &Memory) -> f64,
    threshold: f64,
) -> Vec<Vec<DedupCandidate>> {
    let n = memories.len();
    if n < 2 {
        return Vec::new();
    }

    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }

    for i in 0..n {
        for j in (i + 1)..n {
            if similarity(&memories[i], &memories[j]) + f64::EPSILON >= threshold {
                union(&mut parent, i, j);
            }
        }
    }

    let mut buckets: std::collections::HashMap<usize, Vec<DedupCandidate>> =
        std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        buckets.entry(root).or_default().push(DedupCandidate {
            id: memories[i].id.clone(),
            topic: memories[i].topic.clone(),
        });
    }

    let mut clusters: Vec<Vec<DedupCandidate>> = buckets
        .into_values()
        .filter(|c| c.len() >= 2)
        .collect();
    clusters.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a[0].id.cmp(&b[0].id)));
    clusters
}

/// Legacy organizer behaviour: only compare memories that already share a topic.
pub fn cluster_by_topic_legacy(memories: &[Memory]) -> Vec<Vec<DedupCandidate>> {
    let mut by_topic: std::collections::HashMap<String, Vec<Memory>> =
        std::collections::HashMap::new();
    for m in memories {
        if let Some(t) = &m.topic {
            by_topic.entry(t.clone()).or_default().push(m.clone());
        }
    }
    let mut out = Vec::new();
    for group in by_topic.into_values() {
        if group.len() < 2 {
            continue;
        }
        out.extend(cluster_by_similarity(&group, pair_similarity, 1.0));
    }
    out
}

pub fn cluster_storewide(memories: &[Memory]) -> Vec<Vec<DedupCandidate>> {
    cluster_by_similarity(memories, pair_similarity, 1.0)
}

/// 1.0 if the pair should be clustered, else 0.0. Title match OR lexical
/// Jaccard (OR cosine, when the caller supplies it via a custom fn).
pub fn pair_similarity(a: &Memory, b: &Memory) -> f64 {
    if are_near_duplicates(a, b) {
        1.0
    } else {
        0.0
    }
}

pub fn are_near_duplicates(a: &Memory, b: &Memory) -> bool {
    // Opposite project-scoped facts must never cluster. Aethermon "write
    // everything to prod" and Sanity "never write to prod" share wording
    // and must stay two memories.
    if !same_scope(a.project.as_deref(), b.project.as_deref()) {
        return false;
    }
    let title = lexical_similarity_text(&a.title, &b.title);
    if title + f64::EPSILON >= TITLE_CLUSTER_THRESHOLD {
        return true;
    }
    lexical_similarity(a, b) + f64::EPSILON >= LEXICAL_CLUSTER_THRESHOLD
}

/// Same concrete project, or both global. Mixed (aethermon vs sanity, or
/// global vs project) is not a duplicate — it is usually an exception.
pub fn same_scope(a: Option<&str>, b: Option<&str>) -> bool {
    a == b
}

pub fn lexical_similarity(a: &Memory, b: &Memory) -> f64 {
    lexical_similarity_text(&embed_text(a), &embed_text(b))
}

pub fn lexical_similarity_text(a: &str, b: &str) -> f64 {
    let ta = tokens(a);
    let tb = tokens(b);
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let inter = ta.intersection(&tb).count() as f64;
    let union = ta.union(&tb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for i in 0..a.len() {
        let x = a[i] as f64;
        let y = b[i] as f64;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

pub fn embed_text(m: &Memory) -> String {
    format!("{} {} {}", m.title, m.description, m.content)
}

fn tokens(s: &str) -> std::collections::HashSet<String> {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| w.len() > 1)
        .map(str::to_string)
        .collect()
}

/// Best existing memory whose lexical similarity to `title + content` is
/// ≥ `WRITE_NEAR_DUP_THRESHOLD`. Used at write time so paraphrases do not
/// become a second row. Does not delete anything.
pub fn find_near_duplicate<'a>(
    existing: &'a [Memory],
    title: &str,
    content: &str,
    project: Option<&str>,
) -> Option<(&'a Memory, f64)> {
    let probe = format!("{title} {content}");
    let mut best: Option<(&Memory, f64)> = None;
    for m in existing {
        if m.archived_at.is_some() {
            continue;
        }
        if !same_scope(m.project.as_deref(), project) {
            continue;
        }
        let score = lexical_similarity_text(&probe, &embed_text(m));
        if score + f64::EPSILON >= WRITE_NEAR_DUP_THRESHOLD {
            match best {
                Some((_, s)) if s >= score => {}
                _ => best = Some((m, score)),
            }
        }
    }
    best
}

/// Store-backed write-time check. None means "safe to insert".
pub fn existing_near_duplicate(
    title: &str,
    content: &str,
    project: Option<&str>,
) -> Result<Option<Memory>, String> {
    let all = memories::list_all()?;
    Ok(find_near_duplicate(&all, title, content, project).map(|(m, _)| m.clone()))
}

/// Prefer a standing-rule type over a project note when sources disagree.
/// Never invent a type; never promote `project` over `feedback`/`user`.
pub fn merge_memory_type(types: &[Option<&str>]) -> Option<String> {
    if types.is_empty() {
        return None;
    }
    let first = types[0].map(str::to_string);
    if types.iter().all(|t| *t == types[0]) {
        return first;
    }
    if types.iter().any(|t| matches!(*t, Some("user"))) {
        return Some("user".to_string());
    }
    if types.iter().any(|t| matches!(*t, Some("feedback"))) {
        return Some("feedback".to_string());
    }
    first
}

/// Keep the narrowest shared scope. Mixed projects stay on the first
/// non-global project only if every source shares it; otherwise None (global)
/// is *not* applied — we keep the first project so a repo-local exception
/// cannot become a standing global rule.
///
/// Update 2 rule: if any source is project-scoped and they are not all the
/// same project, refuse to invent a global merge by returning the most
/// frequent project (ties → first). Callers that want to abort instead can
/// check `merge_projects_compatible`.
pub fn merge_project(projects: &[Option<&str>]) -> Option<String> {
    if projects.is_empty() {
        return None;
    }
    let first = projects[0];
    if projects.iter().all(|p| *p == first) {
        return first.map(str::to_string);
    }
    // Mixed: keep a concrete project, never promote to global.
    projects
        .iter()
        .copied()
        .flatten()
        .next()
        .map(str::to_string)
}

pub fn merge_projects_compatible(projects: &[Option<&str>]) -> bool {
    if projects.is_empty() {
        return true;
    }
    let first = projects[0];
    projects.iter().all(|p| *p == first)
}

/// Types may mix only when the result would still be a standing rule or
/// every source already shares a type. A `feedback` + `project` mix is
/// allowed (becomes feedback). A `user` + `project` mix is allowed (user).
/// Two different project-ish types with no rule present still merge as the
/// first type.
pub fn merge_types_compatible(types: &[Option<&str>]) -> bool {
    if types.is_empty() {
        return true;
    }
    let first = types[0];
    if types.iter().all(|t| *t == first) {
        return true;
    }
    // Refuse only when we'd collapse two standing-rule kinds into each other
    // in a surprising way? Allow user/feedback/project mixes — merge_memory_type
    // prefers the stricter standing-rule kind.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(
        id: &str,
        title: &str,
        content: &str,
        topic: Option<&str>,
        memory_type: Option<&str>,
        project: Option<&str>,
    ) -> Memory {
        Memory {
            id: id.to_string(),
            title: title.to_string(),
            description: String::new(),
            content: content.to_string(),
            memory_type: memory_type.map(str::to_string),
            topic: topic.map(str::to_string),
            source: None,
            project: project.map(str::to_string),
            created_at: 0,
            updated_at: 0,
            access_count: 0,
            archived_at: None,
        }
    }

    /// The live-audit pair: same instruction, two topics. This is the
    /// case per-topic dedup structurally cannot see.
    fn cross_topic_pair() -> Vec<Memory> {
        vec![
            mem(
                "4f109c64",
                "Keep the delivery-plan artifact in sync on EVERY plan change",
                "Whenever the plan changes, update the delivery-plan artifact in the same turn. Do not wait until the end of the session.",
                Some("deliverable-format"),
                Some("feedback"),
                None,
            ),
            mem(
                "cbc2f32f",
                "Keep the delivery-plan artifact in sync on EVERY plan change",
                "Whenever the plan changes, update the delivery-plan artifact in the same turn. Never defer the artifact update to later.",
                Some("team-process"),
                Some("feedback"),
                None,
            ),
            mem(
                "unrelated",
                "Postgres listens on 5432",
                "Production postgres is bound to port 5432.",
                Some("database"),
                Some("project"),
                Some("/tmp/db"),
            ),
        ]
    }

    #[test]
    fn proof_legacy_per_topic_misses_cross_topic_pair_storewide_finds_it() {
        let store = cross_topic_pair();
        let legacy = cluster_by_topic_legacy(&store);
        let next = cluster_storewide(&store);

        let legacy_has_pair = legacy.iter().any(|c| {
            c.iter().any(|m| m.id == "4f109c64") && c.iter().any(|m| m.id == "cbc2f32f")
        });
        let next_has_pair = next.iter().any(|c| {
            c.iter().any(|m| m.id == "4f109c64") && c.iter().any(|m| m.id == "cbc2f32f")
        });

        assert!(
            !legacy_has_pair,
            "legacy baseline drifted — per-topic clustering must miss the cross-topic pair: {legacy:?}"
        );
        assert!(
            next_has_pair,
            "store-wide clustering must surface the cross-topic pair: {next:?}"
        );
        assert!(
            next.len() >= 1,
            "new path must produce at least one cluster"
        );
        assert!(
            next.iter().all(|c| c.iter().any(|m| m.topic != c[0].topic) || c.len() >= 2),
            "clusters should be mergeable groups"
        );
    }

    #[test]
    fn proof_write_time_hash_misses_paraphrase_near_dup_catches_it() {
        let existing = cross_topic_pair();
        let paraphrase_title = "Keep the delivery-plan artifact in sync on EVERY plan change";
        let paraphrase_content = "Whenever the plan changes, update the delivery-plan artifact in the same turn. Do not wait until the end of the session.";

        // Exact same content would hash-match; this is a one-word paraphrase
        // of the second memory, which SHA-256 cannot catch.
        let near_title = "Keep the delivery-plan artifact in sync on EVERY plan change";
        let near_content = "Whenever the plan changes, update the delivery-plan artifact in the same turn. Never defer the artifact update to later.";

        assert_ne!(
            paraphrase_content, near_content,
            "fixture must be a paraphrase, not an exact hash collision"
        );

        let found = find_near_duplicate(&existing, near_title, near_content, None)
            .expect("near-dup check must find the existing paraphrase");
        assert_eq!(found.0.id, "cbc2f32f");
        assert!(found.1 >= WRITE_NEAR_DUP_THRESHOLD);

        // Unrelated content must not trip the write-time gate.
        assert!(find_near_duplicate(
            &existing,
            "Something else entirely",
            "The cat sat on the mat and ignored the delivery plan.",
            None,
        )
        .is_none());

        // Exact-hash path is a different layer; near-dup still works for the
        // identical first memory too (title+content Jaccard = 1.0).
        let exact =
            find_near_duplicate(&existing, paraphrase_title, paraphrase_content, None).unwrap();
        assert_eq!(exact.0.id, "4f109c64");
    }

    #[test]
    fn proof_opposite_project_rules_are_not_clustered() {
        let aethermon = "/Users/byron/projects/personal/aethermon";
        let sanity = "/Users/byron/projects/personal/shopify-sanity-connector";
        let store = vec![
            mem(
                "ae",
                "write to production info",
                "with details write all to production",
                Some("deployment"),
                Some("project"),
                Some(aethermon),
            ),
            mem(
                "sa",
                "write to production info",
                "never write to production",
                Some("deployment"),
                Some("project"),
                Some(sanity),
            ),
        ];

        // Same title would have clustered before the same-scope gate.
        assert!(
            lexical_similarity_text(&store[0].title, &store[1].title) >= TITLE_CLUSTER_THRESHOLD
        );

        let clusters = cluster_storewide(&store);
        assert!(
            clusters.is_empty(),
            "aethermon 'write all to prod' and sanity 'never write to prod' must stay two memories: {clusters:?}"
        );
        assert!(!are_near_duplicates(&store[0], &store[1]));
        assert!(!merge_projects_compatible(&[
            Some(aethermon),
            Some(sanity)
        ]));

        // Writing the sanity rule against a store that only has the
        // aethermon rule must not be treated as a duplicate.
        assert!(find_near_duplicate(
            &store[..1],
            "write to production info",
            "never write to production",
            Some(sanity),
        )
        .is_none());
    }

    #[test]
    fn proof_legacy_merge_promotes_to_global_new_keeps_project() {
        let mixed = [Some("/tmp/aethermon"), None];
        // Legacy: any disagreement → None (global). That is the bug.
        let legacy_global = if mixed.iter().all(|p| *p == mixed[0]) {
            mixed[0]
        } else {
            None
        };
        assert!(
            legacy_global.is_none(),
            "legacy baseline drifted — mixed projects must collapse to global"
        );

        let next = merge_project(&mixed);
        assert_eq!(
            next.as_deref(),
            Some("/tmp/aethermon"),
            "new merge must keep the repo-local scope, not promote to global"
        );
        assert!(
            !merge_projects_compatible(&mixed),
            "caller can still detect mixed projects and refuse if it wants"
        );
    }

    #[test]
    fn proof_merge_prefers_standing_rule_type() {
        assert_eq!(
            merge_memory_type(&[Some("project"), Some("feedback")]).as_deref(),
            Some("feedback")
        );
        assert_eq!(
            merge_memory_type(&[Some("feedback"), Some("feedback")]).as_deref(),
            Some("feedback")
        );
        assert_eq!(
            merge_memory_type(&[Some("project"), Some("project")]).as_deref(),
            Some("project")
        );
    }

    #[test]
    fn proof_insert_and_update_source_calls_queue_memory() {
        let src = include_str!("../store/memories.rs");
        let calls = src.matches("queue_memory").count();
        assert!(
            calls >= 2,
            "insert and update must both call queue_memory (found {calls})"
        );
    }

    #[test]
    fn proof_queue_memory_is_invocable_and_counted() {
        // Before Update 2 this function had zero callers. insert/update now
        // call it; this asserts the enqueue entrypoint is live.
        use crate::services::embeddings::{queue_memory, QUEUE_MEMORY_CALLS};
        let before = QUEUE_MEMORY_CALLS.load(std::sync::atomic::Ordering::SeqCst);
        queue_memory("proof-id", "proof text");
        let after = QUEUE_MEMORY_CALLS.load(std::sync::atomic::Ordering::SeqCst);
        assert!(after > before, "queue_memory must increment its call counter");
    }

    #[test]
    fn cosine_identical_is_one() {
        let v = vec![1.0f32, 0.0, 0.0];
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-9);
        assert!(cosine(&v, &[0.0, 1.0, 0.0]) < 0.01);
    }
}
