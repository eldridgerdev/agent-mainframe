//! Staff roles: which cheap model does which packet work, and how much input
//! each may be sent.

use serde::{Deserialize, Serialize};

use crate::project::AgentKind;

/// Scribe input cap (brief, summaries), in estimated tokens.
pub const SCRIBE_CAP: u64 = 16_000;
/// Cartographer seed context cap; it then explores read-only.
pub const CARTOGRAPHER_SEED_CAP: u64 = 24_000;
/// Cartographer cap for seed plus everything it reads.
pub const CARTOGRAPHER_TOTAL_CAP: u64 = 90_000;
/// Presenter cap for an ordinary answer.
pub const PRESENTER_CAP: u64 = 8_000;
/// Presenter seed cap for a read-only deep dive.
pub const PRESENTER_DEEP_CAP: u64 = 24_000;
/// Haiku costs five times more above this many prompt tokens; every staff
/// cap stays below it.
pub const HAIKU_TIER_LINE: u64 = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StaffRole {
    Scribe,
    Cartographer,
    Presenter,
}

impl StaffRole {
    pub const ALL: [StaffRole; 3] = [
        StaffRole::Scribe,
        StaffRole::Cartographer,
        StaffRole::Presenter,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            StaffRole::Scribe => "scribe",
            StaffRole::Cartographer => "cartographer",
            StaffRole::Presenter => "presenter",
        }
    }

    /// Input cap for the role's ordinary run (the seed, for roles that
    /// explore).
    pub fn input_cap(self) -> u64 {
        match self {
            StaffRole::Scribe => SCRIBE_CAP,
            StaffRole::Cartographer => CARTOGRAPHER_SEED_CAP,
            StaffRole::Presenter => PRESENTER_CAP,
        }
    }

    /// Hard ceiling including anything the role reads itself.
    pub fn total_cap(self) -> u64 {
        match self {
            StaffRole::Scribe => SCRIBE_CAP,
            StaffRole::Cartographer => CARTOGRAPHER_TOTAL_CAP,
            StaffRole::Presenter => PRESENTER_DEEP_CAP,
        }
    }
}

/// One role's harness, model and reasoning level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffModel {
    #[serde(default)]
    pub harness: AgentKind,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

impl StaffModel {
    fn claude(model: &str) -> Self {
        Self {
            harness: AgentKind::Claude,
            model: model.to_owned(),
            reasoning: None,
        }
    }
}

/// The `design_review` block of `amf.json`. Every role is optional; an absent
/// role uses the default for the feature's harness.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesignReviewConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scribe: Option<StaffModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cartographer: Option<StaffModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presenter: Option<StaffModel>,
}

impl DesignReviewConfig {
    pub fn is_empty(&self) -> bool {
        self.scribe.is_none() && self.cartographer.is_none() && self.presenter.is_none()
    }

    pub fn role(&self, role: StaffRole) -> Option<&StaffModel> {
        match role {
            StaffRole::Scribe => self.scribe.as_ref(),
            StaffRole::Cartographer => self.cartographer.as_ref(),
            StaffRole::Presenter => self.presenter.as_ref(),
        }
    }
}

/// The model for `role`: the configured one if any, else the Claude default
/// (haiku, haiku, sonnet) when the feature runs Claude, else `None`. `None`
/// means the caller must ask once for staff models; other harnesses have no
/// cheap tier to default to.
pub fn resolve(
    role: StaffRole,
    cfg: &DesignReviewConfig,
    feature_agent: &AgentKind,
) -> Option<StaffModel> {
    if let Some(configured) = cfg.role(role) {
        return Some(configured.clone());
    }
    if *feature_agent != AgentKind::Claude {
        return None;
    }
    Some(StaffModel::claude(match role {
        StaffRole::Scribe | StaffRole::Cartographer => "haiku",
        StaffRole::Presenter => "sonnet",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cap_stays_below_the_haiku_tier_line() {
        for cap in [
            SCRIBE_CAP,
            CARTOGRAPHER_SEED_CAP,
            CARTOGRAPHER_TOTAL_CAP,
            PRESENTER_CAP,
            PRESENTER_DEEP_CAP,
        ] {
            assert!(cap < HAIKU_TIER_LINE);
        }
        for role in StaffRole::ALL {
            assert!(role.input_cap() <= role.total_cap());
            assert!(role.total_cap() < 100_000);
        }
    }

    #[test]
    fn claude_defaults_are_haiku_haiku_sonnet() {
        let cfg = DesignReviewConfig::default();
        let model = |role| resolve(role, &cfg, &AgentKind::Claude).unwrap();
        assert_eq!(model(StaffRole::Scribe), StaffModel::claude("haiku"));
        assert_eq!(model(StaffRole::Cartographer), StaffModel::claude("haiku"));
        assert_eq!(model(StaffRole::Presenter), StaffModel::claude("sonnet"));
    }

    #[test]
    fn configured_role_wins_on_any_harness() {
        let cfg = DesignReviewConfig {
            scribe: Some(StaffModel {
                harness: AgentKind::Codex,
                model: "gpt-mini".into(),
                reasoning: Some("low".into()),
            }),
            ..Default::default()
        };
        let got = resolve(StaffRole::Scribe, &cfg, &AgentKind::Pi).unwrap();
        assert_eq!(got.harness, AgentKind::Codex);
        assert_eq!(got.model, "gpt-mini");
        assert_eq!(got.reasoning.as_deref(), Some("low"));
        // Unconfigured roles on Claude still default.
        assert!(resolve(StaffRole::Presenter, &cfg, &AgentKind::Claude).is_some());
    }

    #[test]
    fn non_claude_with_nothing_configured_resolves_to_none() {
        let cfg = DesignReviewConfig::default();
        for agent in [AgentKind::Codex, AgentKind::Opencode, AgentKind::Pi] {
            for role in StaffRole::ALL {
                assert_eq!(resolve(role, &cfg, &agent), None);
            }
        }
    }

    #[test]
    fn config_block_round_trips_and_absent_is_default() {
        let parsed: DesignReviewConfig =
            serde_json::from_str(r#"{"presenter":{"model":"opus","reasoning":"high"}}"#).unwrap();
        assert_eq!(
            parsed.presenter.as_ref().unwrap().harness,
            AgentKind::Claude
        );
        assert!(parsed.scribe.is_none());
        let back = serde_json::to_string(&parsed).unwrap();
        assert!(!back.contains("scribe"));
        let empty: DesignReviewConfig = serde_json::from_str("{}").unwrap();
        assert!(empty.is_empty());
    }
}
