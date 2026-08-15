//! Type-based half of the recall score, plus the labels that expose scope and
//! kind to Claude at recall time. The project-based half lives in
//! `project::project_affinity`.

use std::path::Path;

/// Sized against `project::PROJECT_AFFINITY_EXACT` (0.40): without it, a
/// same-project note outscores a global standing rule by the full affinity
/// gap on scope alone, so a one-off exception saved inside a repo buries the
/// rule it contradicts.
pub const TYPE_WEIGHT_RULE: f64 = 0.25;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::project;

    #[test]
    fn rules_outrank_notes_on_type() {
        assert_eq!(type_weight(Some("feedback")), TYPE_WEIGHT_RULE);
        assert_eq!(type_weight(Some("user")), TYPE_WEIGHT_RULE);
        assert_eq!(type_weight(Some("project")), 0.0);
        assert_eq!(type_weight(Some("reference")), 0.0);
        assert_eq!(type_weight(None), 0.0);
    }

    #[test]
    fn global_rule_survives_a_same_project_note_with_equal_relevance() {
        let repo = Path::new("/Users/byron/projects/personal/aethermon");

        let rule = type_weight(Some("feedback")) + project::project_affinity(None, Some(repo));
        let note = type_weight(Some("project"))
            + project::project_affinity(Some("/Users/byron/projects/personal/aethermon"), Some(repo));

        assert!(note > rule, "a same-project note still wins on scope alone");
        assert!(
            note - rule < project::PROJECT_AFFINITY_EXACT,
            "but the gap must be narrow enough for keyword relevance to decide"
        );
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
