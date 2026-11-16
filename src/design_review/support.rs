//! Where Design Review is offered. Every surface (creation dialog, presets,
//! GUI form, harness change, instruction injection, review room) asks
//! [`design_review_supported`], so widening the mode is a change here only.

use crate::project::{AgentKind, Feature, Project};

/// Shown wherever the mode is offered or opened on a non-Claude feature.
pub const DESIGN_REVIEW_CLAUDE_ONLY: &str = "Claude only for now";

/// Shown for a containerized project: the sandbox image does not ship `amf`,
/// so the agent could not record decisions there.
pub const DESIGN_REVIEW_CONTAINER_UNAVAILABLE: &str = "not available in container projects yet";

/// Whether Design Review can run for a feature on `agent` in a project that
/// is (or is not) containerized.
pub fn design_review_supported(agent: &AgentKind, containerized: bool) -> bool {
    design_review_unavailable_reason(agent, containerized).is_none()
}

/// Why Design Review is unavailable, or `None` when it is available. The
/// container reason wins, since switching harness would not fix it.
pub fn design_review_unavailable_reason(
    agent: &AgentKind,
    containerized: bool,
) -> Option<&'static str> {
    if containerized {
        Some(DESIGN_REVIEW_CONTAINER_UNAVAILABLE)
    } else if !matches!(agent, AgentKind::Claude) {
        Some(DESIGN_REVIEW_CLAUDE_ONLY)
    } else {
        None
    }
}

/// A preset's review choices as applied on every creation surface: Review
/// Mode wins when a preset sets both, and an unavailable mode stays off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetReview {
    pub review: bool,
    pub design_review: bool,
    /// The warning to show when the preset's `design_review` was dropped.
    pub notice: Option<String>,
}

/// Resolve a preset's `review` / `design_review` flags. `unavailable` is
/// [`design_review_unavailable_reason`] for the chosen harness and project.
/// The GUI's `CreateFeatureForm.tsx` mirrors this rule.
pub fn resolve_preset_review(
    preset_name: &str,
    review: bool,
    design_review: bool,
    unavailable: Option<&str>,
) -> PresetReview {
    let notice = match (design_review, review, unavailable) {
        (false, _, _) => None,
        (true, true, _) => Some("the preset also enables Review Mode"),
        (true, false, reason) => reason,
    }
    .map(|reason| format!("Preset '{preset_name}': Design Review ignored ({reason})"));
    PresetReview {
        review,
        design_review: design_review && notice.is_none(),
        notice,
    }
}

/// [`design_review_supported`] for an existing feature of `project`.
pub fn feature_design_review_supported(project: &Project, feature: &Feature) -> bool {
    design_review_supported(&feature.agent, project.containerized)
}

/// Whether the feature has the mode on *and* it can run there. A stale flag on
/// an unsupported feature is treated as off.
pub fn feature_design_review_active(project: &Project, feature: &Feature) -> bool {
    feature.design_review && feature_design_review_supported(project, feature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_review_mode_wins_and_unavailable_design_review_is_dropped() {
        let on = resolve_preset_review("p", false, true, None);
        assert_eq!(
            (on.review, on.design_review, on.notice),
            (false, true, None)
        );
        let both = resolve_preset_review("p", true, true, None);
        assert!(both.review && !both.design_review);
        assert!(both.notice.unwrap().contains("Review Mode"));
        let codex = resolve_preset_review("p", false, true, Some(DESIGN_REVIEW_CLAUDE_ONLY));
        assert!(!codex.design_review);
        assert!(codex.notice.unwrap().contains(DESIGN_REVIEW_CLAUDE_ONLY));
        let off = resolve_preset_review("p", true, false, Some(DESIGN_REVIEW_CLAUDE_ONLY));
        assert_eq!(
            (off.review, off.design_review, off.notice),
            (true, false, None)
        );
    }

    #[test]
    fn only_claude_outside_containers_is_supported() {
        assert!(design_review_supported(&AgentKind::Claude, false));
        for agent in [AgentKind::Codex, AgentKind::Opencode, AgentKind::Pi] {
            assert!(!design_review_supported(&agent, false), "{agent:?}");
            assert_eq!(
                design_review_unavailable_reason(&agent, false),
                Some(DESIGN_REVIEW_CLAUDE_ONLY)
            );
        }
        assert!(!design_review_supported(&AgentKind::Claude, true));
        assert_eq!(
            design_review_unavailable_reason(&AgentKind::Claude, true),
            Some(DESIGN_REVIEW_CONTAINER_UNAVAILABLE)
        );
        assert_eq!(
            design_review_unavailable_reason(&AgentKind::Codex, true),
            Some(DESIGN_REVIEW_CONTAINER_UNAVAILABLE)
        );
        assert_eq!(
            design_review_unavailable_reason(&AgentKind::Claude, false),
            None
        );
    }
}
