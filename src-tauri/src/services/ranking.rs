//! Recall scoring and the labels that expose scope / kind to Claude.
//!
//! Combined score (hook + MCP re-rank):
//!   0.45 * bm25_norm
//! + type_weight            (0.45 for user/feedback, else 0)
//! + project_affinity       (+0.40 / +0.15 / 0 / −0.20)
//! + 0.10 * graph_boost     (only usable edges — see `graph_edge_usable`)
//! + 0.05 * usage_score     (log-scaled access_count of this memory)
//! + 0.05 * recency_score   (half-life ~75 days)

use std::path::Path;

use crate::services::project;

/// Sized just above `project::PROJECT_AFFINITY_EXACT` (0.40) so a global
/// standing rule beats a same-project note when lexical relevance is equal.
/// The reserved slot still covers the case where the note also wins BM25.
pub const TYPE_WEIGHT_RULE: f64 = 0.45;

/// Pre-change type weight, locked for proof tests.
#[cfg(test)]
const TYPE_WEIGHT_RULE_LEGACY: f64 = 0.25;

pub const W_BM25: f64 = 0.45;
pub const W_GRAPH: f64 = 0.10;
pub const W_USAGE: f64 = 0.05;
pub const W_RECENCY: f64 = 0.05;

/// Co-access seeds at 0.1; only pairs retrieved ~3+ times clear this floor.
pub const GRAPH_MIN_WEIGHT: f64 = 0.2;

/// ~75 days. A memory updated today scores 1.0; one last touched 75 days
/// ago scores 0.5. Unknown timestamps get a neutral 0.5.
pub const RECENCY_HALF_LIFE_SECS: f64 = 75.0 * 24.0 * 3600.0;

/// Drop non-rule candidates below this after scoring. Stops the hook from
/// injecting five weakly related notes just because FTS returned something.
pub const MIN_INJECT_SCORE: f64 = 0.20;

/// Standing-rule body injected in full, not as an FTS snippet.
pub const STANDING_RULE_CONTENT_CHARS: usize = 800;

pub fn is_standing_rule(memory_type: Option<&str>) -> bool {
    matches!(memory_type, Some("user") | Some("feedback"))
}

pub fn type_weight(memory_type: Option<&str>) -> f64 {
    if is_standing_rule(memory_type) {
        TYPE_WEIGHT_RULE
    } else {
        0.0
    }
}

pub fn kind_label(memory_type: Option<&str>) -> &'static str {
    if is_standing_rule(memory_type) {
        "standing rule"
    } else if memory_type == Some("reference") {
        "reference"
    } else {
        "project note"
    }
}

/// Pick which score-sorted candidates survive the cut, guaranteeing a slot to
/// an eligible standing rule when one matched at all. Relevance alone lets a
/// cluster of same-project notes fill every slot and hide the rule they are an
/// exception to — the note wins the keyword match precisely because it is the
/// narrower, more specific text.
///
/// `eligible` is a standing rule that also applies here: callers exclude rules
/// scoped to a different project, which otherwise get promoted on an incidental
/// word match and spend the reserved slot on noise.
pub fn reserve_standing_rule_slot(eligible: &[bool], limit: usize) -> Vec<usize> {
    let cut = limit.min(eligible.len());
    let mut kept: Vec<usize> = (0..cut).collect();

    if limit == 0 || kept.iter().any(|&i| eligible[i]) {
        return kept;
    }

    if let Some(promoted) = (cut..eligible.len()).find(|&i| eligible[i]) {
        kept.pop();
        kept.push(promoted);
    }

    kept
}

/// A standing rule that governs work in `current_project` — global, or scoped
/// to this project or a sibling. Negative affinity means it belongs to a
/// different project and says nothing about the work at hand.
pub fn is_applicable_standing_rule(
    memory_type: Option<&str>,
    memory_project: Option<&str>,
    current_project: Option<&std::path::Path>,
) -> bool {
    is_standing_rule(memory_type)
        && crate::services::project::project_affinity(memory_project, current_project) >= 0.0
}

pub fn scope_label(project: Option<&str>) -> String {
    match project {
        None => "global".to_string(),
        Some(p) if p.trim().is_empty() => "global".to_string(),
        Some(p) => Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.to_string()),
    }
}

pub fn context_tag(memory_type: Option<&str>, project: Option<&str>) -> String {
    format!(
        "{} · {}",
        kind_label(memory_type),
        scope_label(project)
    )
}

pub const PRECEDENCE_NOTE: &str = "\
Each entry is tagged [kind · scope]. A \"standing rule\" is a durable instruction from the user and \
takes precedence over a \"project note\", which is incidental context captured while working in one \
repo. A project note is never permission to drop a standing rule — if they conflict, follow the \
standing rule and ask before deviating.";

/// True for edges that should influence recall. Weak co-access `relates-to`
/// and `contradicts` edges are excluded: the former is noise, the latter is
/// a warning (see `contradiction_line`) not a relevance boost.
pub fn graph_edge_usable(edge_type: &str, weight: f64) -> bool {
    weight + f64::EPSILON >= GRAPH_MIN_WEIGHT
        && matches!(edge_type, "relates-to" | "depends-on" | "supersedes")
}

pub fn usage_score(access_count: i64) -> f64 {
    let n = access_count.max(0) as f64;
    (1.0 + n).ln() / (201.0_f64).ln()
}

pub fn recency_score(updated_at: i64, now: i64) -> f64 {
    if updated_at <= 0 {
        return 0.5;
    }
    let age = (now - updated_at).max(0) as f64;
    0.5_f64.powf(age / RECENCY_HALF_LIFE_SECS)
}

pub fn combined_score(
    bm25_norm: f64,
    memory_type: Option<&str>,
    memory_project: Option<&str>,
    current_project: Option<&Path>,
    graph_boost: f64,
    access_count: i64,
    updated_at: i64,
    now: i64,
) -> f64 {
    W_BM25 * bm25_norm
        + type_weight(memory_type)
        + project::project_affinity(memory_project, current_project)
        + W_GRAPH * graph_boost
        + W_USAGE * usage_score(access_count)
        + W_RECENCY * recency_score(updated_at, now)
}

/// Pre-change score: `0.7*bm25 + 0.3*graph + affinity + 0.25 type`.
/// Kept so proof tests can show the new function flips the cases we care about.
#[cfg(test)]
pub fn combined_score_legacy(
    bm25_norm: f64,
    memory_type: Option<&str>,
    memory_project: Option<&str>,
    current_project: Option<&Path>,
    graph_boost: f64,
) -> f64 {
    let type_boost = if is_standing_rule(memory_type) {
        TYPE_WEIGHT_RULE_LEGACY
    } else {
        0.0
    };
    0.7 * bm25_norm
        + 0.3 * graph_boost
        + project::project_affinity(memory_project, current_project)
        + type_boost
}

/// Body shown in `<memory-context>`. Standing rules keep the instruction;
/// everything else stays a short snippet.
pub fn inject_body(memory_type: Option<&str>, content: &str, snippet: &str) -> String {
    if is_standing_rule(memory_type) && !content.trim().is_empty() {
        truncate_chars(content.trim(), STANDING_RULE_CONTENT_CHARS)
    } else {
        snippet.to_string()
    }
}

pub fn contradiction_line(titles: &[String]) -> Option<String> {
    if titles.is_empty() {
        return None;
    }
    Some(format!(
        "   ⚠ contradicts: {}",
        titles.join("; ")
    ))
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
    fn rules_outrank_notes_on_type() {
        assert_eq!(type_weight(Some("feedback")), TYPE_WEIGHT_RULE);
        assert_eq!(type_weight(Some("user")), TYPE_WEIGHT_RULE);
        assert_eq!(type_weight(Some("project")), 0.0);
        assert_eq!(type_weight(Some("reference")), 0.0);
        assert_eq!(type_weight(None), 0.0);
    }

    #[test]
    fn global_rule_beats_same_project_note_when_relevance_is_equal() {
        let repo = Path::new("/Users/byron/projects/personal/aethermon");
        let now = 1_700_000_000;

        let rule = combined_score(1.0, Some("feedback"), None, Some(repo), 0.0, 0, now, now);
        let note = combined_score(
            1.0,
            Some("project"),
            Some("/Users/byron/projects/personal/aethermon"),
            Some(repo),
            0.0,
            0,
            now,
            now,
        );

        assert!(
            rule > note,
            "equal BM25: standing rule must beat same-project note ({rule} vs {note})"
        );
    }

    #[test]
    fn proof_legacy_score_buries_the_rule_new_score_does_not() {
        let repo = Path::new("/Users/byron/projects/personal/aethermon");
        let now = 1_700_000_000;
        let note_project = Some("/Users/byron/projects/personal/aethermon");

        let legacy_rule =
            combined_score_legacy(1.0, Some("feedback"), None, Some(repo), 0.0);
        let legacy_note =
            combined_score_legacy(1.0, Some("project"), note_project, Some(repo), 0.0);
        assert!(
            legacy_note > legacy_rule,
            "legacy baseline drifted — note should still win on +0.40 affinity vs +0.25 type ({legacy_note} vs {legacy_rule})"
        );

        let next_rule = combined_score(1.0, Some("feedback"), None, Some(repo), 0.0, 0, now, now);
        let next_note = combined_score(
            1.0,
            Some("project"),
            note_project,
            Some(repo),
            0.0,
            0,
            now,
            now,
        );
        assert!(
            next_rule > next_note,
            "new score must flip the equal-relevance case ({next_rule} vs {next_note})"
        );
    }

    #[test]
    fn proof_usage_and_recency_break_a_bm25_tie_legacy_cannot() {
        let repo = Path::new("/Users/byron/projects/personal/aethermon");
        let now = 1_700_000_000;
        let stale = now - (180 * 24 * 3600);

        let legacy_hot =
            combined_score_legacy(0.8, Some("project"), Some("/Users/byron/projects/personal/aethermon"), Some(repo), 0.0);
        let legacy_cold =
            combined_score_legacy(0.8, Some("project"), Some("/Users/byron/projects/personal/aethermon"), Some(repo), 0.0);
        assert!(
            (legacy_hot - legacy_cold).abs() < 1e-12,
            "legacy has no usage/recency term, so these must tie"
        );

        let hot = combined_score(
            0.8,
            Some("project"),
            Some("/Users/byron/projects/personal/aethermon"),
            Some(repo),
            0.0,
            80,
            now,
            now,
        );
        let cold = combined_score(
            0.8,
            Some("project"),
            Some("/Users/byron/projects/personal/aethermon"),
            Some(repo),
            0.0,
            0,
            stale,
            now,
        );
        assert!(
            hot > cold,
            "frequently used + recent memory must outrank a stale unused twin ({hot} vs {cold})"
        );
    }

    #[test]
    fn proof_weak_coaccess_and_contradicts_do_not_boost() {
        assert!(!graph_edge_usable("relates-to", 0.1));
        assert!(graph_edge_usable("relates-to", 0.2));
        assert!(graph_edge_usable("depends-on", 0.5));
        assert!(graph_edge_usable("supersedes", 0.5));
        assert!(!graph_edge_usable("contradicts", 0.9));
    }

    #[test]
    fn standing_rule_injects_full_content_notes_keep_snippet() {
        let rule = inject_body(
            Some("feedback"),
            "Always run cargo test before pushing.",
            "Always run…",
        );
        assert_eq!(rule, "Always run cargo test before pushing.");
        let note = inject_body(Some("project"), "long body", "short snippet");
        assert_eq!(note, "short snippet");
    }

    #[test]
    fn a_matching_rule_always_gets_a_slot() {
        let notes_then_rule = [false, false, false, false, false, true];
        assert_eq!(
            reserve_standing_rule_slot(&notes_then_rule, 5),
            vec![0, 1, 2, 3, 5]
        );
    }

    #[test]
    fn nothing_is_promoted_when_a_rule_already_made_the_cut() {
        let rule_inside_cut = [false, true, false, false, false, true];
        assert_eq!(
            reserve_standing_rule_slot(&rule_inside_cut, 5),
            vec![0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn promotion_is_a_no_op_without_candidates_to_promote() {
        assert_eq!(reserve_standing_rule_slot(&[false, false], 5), vec![0, 1]);
        assert_eq!(reserve_standing_rule_slot(&[], 5), Vec::<usize>::new());
        assert_eq!(reserve_standing_rule_slot(&[true], 0), Vec::<usize>::new());
    }

    #[test]
    fn only_rules_that_apply_here_are_eligible_for_the_reserved_slot() {
        let here = Path::new("/Users/byron/projects/work/shopify-sanity-connector");

        assert!(is_applicable_standing_rule(Some("feedback"), None, Some(here)));
        assert!(is_applicable_standing_rule(
            Some("feedback"),
            Some("/Users/byron/projects/work/shopify-sanity-connector"),
            Some(here)
        ));
        assert!(!is_applicable_standing_rule(
            Some("feedback"),
            Some("/Users/byron/projects/personal/aethermon"),
            Some(here)
        ));
        assert!(!is_applicable_standing_rule(Some("project"), None, Some(here)));
    }

    #[test]
    fn labels_describe_kind_and_scope() {
        assert_eq!(kind_label(Some("feedback")), "standing rule");
        assert_eq!(kind_label(Some("project")), "project note");
        assert_eq!(kind_label(None), "project note");
        assert_eq!(scope_label(None), "global");
        assert_eq!(scope_label(Some("")), "global");
        assert_eq!(scope_label(Some("/Users/byron/projects/personal/aethermon")), "aethermon");
        assert_eq!(
            context_tag(Some("feedback"), None),
            "standing rule · global"
        );
    }
}
