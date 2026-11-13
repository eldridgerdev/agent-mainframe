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
