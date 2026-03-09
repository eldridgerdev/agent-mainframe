use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteeringConstraintKind {
    FileScope,
    AcceptanceCriteria,
    Invariants,
    ValidationCommands,
    Risks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteeringConstraintConfig {
    pub constraint: SteeringConstraintKind,
    pub label: String,
    pub missing_explanation: String,
    pub teaching_tip: String,
    #[serde(default)]
    pub detection: Option<SteeringDetectionMatcher>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SteeringDetectionMatcher {
    Any {
        matchers: Vec<SteeringDetectionMatcher>,
    },
    All {
        matchers: Vec<SteeringDetectionMatcher>,
    },
    ContainsAny {
        phrases: Vec<String>,
    },
    TokenContainsAny {
        snippets: Vec<String>,
    },
    TokenStartsWithAny {
        prefixes: Vec<String>,
    },
    TokenEndsWithAny {
        suffixes: Vec<String>,
    },
    PromptContains {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteeringCoachConfig {
    pub constraints: Vec<SteeringConstraintConfig>,
}

impl Default for SteeringCoachConfig {
    fn default() -> Self {
        Self {
            constraints: vec![
                SteeringConstraintConfig {
                    constraint: SteeringConstraintKind::FileScope,
                    label: "File scope".to_string(),
                    missing_explanation:
                        "The prompt does not clearly name what files or directories are in scope."
                            .to_string(),
                    teaching_tip: "Add a line like: `Touch only src/app/... and src/ui/...; leave handlers unchanged unless needed.`".to_string(),
                    detection: Some(default_detection_for(SteeringConstraintKind::FileScope)),
                },
                SteeringConstraintConfig {
                    constraint: SteeringConstraintKind::AcceptanceCriteria,
                    label: "Acceptance criteria".to_string(),
                    missing_explanation:
                        "The prompt does not say what \"done\" looks like.".to_string(),
                    teaching_tip: "Add a line like: `Done when the create-feature flow shows coaching before launch and still reaches cargo check.`".to_string(),
                    detection: Some(default_detection_for(
                        SteeringConstraintKind::AcceptanceCriteria,
                    )),
                },
                SteeringConstraintConfig {
                    constraint: SteeringConstraintKind::Invariants,
                    label: "Invariants".to_string(),
                    missing_explanation:
                        "The prompt does not pin down behavior that must stay unchanged."
                            .to_string(),
                    teaching_tip: "Add a line like: `Keep the existing command/dialog flow and do not bypass the current create_feature path.`".to_string(),
                    detection: Some(default_detection_for(SteeringConstraintKind::Invariants)),
                },
                SteeringConstraintConfig {
                    constraint: SteeringConstraintKind::ValidationCommands,
                    label: "Validation commands".to_string(),
                    missing_explanation:
                        "The prompt does not tell the agent how to verify the change."
                            .to_string(),
                    teaching_tip: "Add an explicit command like: `Run cargo check before stopping.`"
                        .to_string(),
                    detection: Some(default_detection_for(
                        SteeringConstraintKind::ValidationCommands,
                    )),
                },
                SteeringConstraintConfig {
                    constraint: SteeringConstraintKind::Risks,
                    label: "Risks".to_string(),
                    missing_explanation:
                        "The prompt does not call out sharp edges or likely regressions."
                            .to_string(),
                    teaching_tip: "Add a watch-out like: `Be careful not to break SuperVibe confirmation or session launch defaults.`".to_string(),
                    detection: Some(default_detection_for(SteeringConstraintKind::Risks)),
                },
            ],
        }
    }
}

pub fn default_detection_for(kind: SteeringConstraintKind) -> SteeringDetectionMatcher {
    match kind {
        SteeringConstraintKind::FileScope => SteeringDetectionMatcher::Any {
            matchers: vec![
                SteeringDetectionMatcher::ContainsAny {
                    phrases: vec![
                        "file scope".to_string(),
                        "touch only".to_string(),
                        "only touch".to_string(),
                        "only edit".to_string(),
                        "limit changes to".to_string(),
                        "keep changes in".to_string(),
                        "in scope".to_string(),
                        "out of scope".to_string(),
                    ],
                },
                SteeringDetectionMatcher::TokenContainsAny {
                    snippets: vec!["/".to_string()],
                },
                SteeringDetectionMatcher::TokenStartsWithAny {
                    prefixes: vec!["src".to_string()],
                },
                SteeringDetectionMatcher::TokenEndsWithAny {
                    suffixes: vec![
                        ".rs".to_string(),
                        ".toml".to_string(),
                        ".md".to_string(),
                        ".json".to_string(),
                        ".yaml".to_string(),
                        ".yml".to_string(),
                    ],
                },
            ],
        },
        SteeringConstraintKind::AcceptanceCriteria => SteeringDetectionMatcher::ContainsAny {
            phrases: vec![
                "acceptance criteria".to_string(),
                "done when".to_string(),
                "success looks like".to_string(),
                "should ".to_string(),
                "must ".to_string(),
                "expected result".to_string(),
                "expected behavior".to_string(),
                "so that ".to_string(),
                "when i ".to_string(),
            ],
        },
        SteeringConstraintKind::Invariants => SteeringDetectionMatcher::ContainsAny {
            phrases: vec![
                "must not".to_string(),
                "do not".to_string(),
                "don't".to_string(),
                "without changing".to_string(),
                "preserve".to_string(),
                "keep existing".to_string(),
                "leave ".to_string(),
                "avoid regression".to_string(),
                "invariant".to_string(),
                "non-goal".to_string(),
            ],
        },
        SteeringConstraintKind::ValidationCommands => SteeringDetectionMatcher::Any {
            matchers: vec![
                SteeringDetectionMatcher::ContainsAny {
                    phrases: vec![
                        "cargo check".to_string(),
                        "cargo test".to_string(),
                        "cargo clippy".to_string(),
                        "npm test".to_string(),
                        "pnpm test".to_string(),
                        "pytest".to_string(),
                        "go test".to_string(),
                        "just ".to_string(),
                        "run ".to_string(),
                        "verify with".to_string(),
                    ],
                },
                SteeringDetectionMatcher::All {
                    matchers: vec![
                        SteeringDetectionMatcher::PromptContains {
                            text: "`".to_string(),
                        },
                        SteeringDetectionMatcher::ContainsAny {
                            phrases: vec![
                                "cargo".to_string(),
                                "npm".to_string(),
                                "pnpm".to_string(),
                                "pytest".to_string(),
                                "go test".to_string(),
                                "just".to_string(),
                                "uv run".to_string(),
                            ],
                        },
                    ],
                },
            ],
        },
        SteeringConstraintKind::Risks => SteeringDetectionMatcher::ContainsAny {
            phrases: vec![
                "risk".to_string(),
                "risks".to_string(),
                "watch out".to_string(),
                "careful".to_string(),
                "edge case".to_string(),
                "gotcha".to_string(),
                "backward compatibility".to_string(),
                "backwards compatibility".to_string(),
                "regression".to_string(),
                "performance".to_string(),
                "perf".to_string(),
                "migration".to_string(),
                "hooks".to_string(),
                "tmux".to_string(),
            ],
        },
    }
}

pub fn steering_config_path(repo: &Path) -> PathBuf {
    repo.join(".amf").join("steering.json")
}

pub fn load_steering_config(repo: &Path) -> SteeringCoachConfig {
    let path = steering_config_path(repo);

    if !path.exists() {
        return SteeringCoachConfig::default();
    }

    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<SteeringCoachConfig>(&raw).ok())
        .unwrap_or_default()
}

pub fn ensure_steering_config(repo: &Path) -> SteeringCoachConfig {
    let path = steering_config_path(repo);
    if path.exists() {
        return load_steering_config(repo);
    }

    let config = SteeringCoachConfig::default();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".to_string()),
    );

    SteeringCoachConfig::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn ensure_steering_config_injects_defaults_when_missing() {
        let repo = TempDir::new().unwrap();

        let config = ensure_steering_config(repo.path());
        let path = steering_config_path(repo.path());

        assert_eq!(config, SteeringCoachConfig::default());
        assert!(path.exists());

        let written: SteeringCoachConfig =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(written, SteeringCoachConfig::default());
    }

    #[test]
    fn load_steering_config_reads_custom_constraints() {
        let repo = TempDir::new().unwrap();
        let path = steering_config_path(repo.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{
  "constraints": [
    {
      "constraint": "risks",
      "label": "Sharp edges",
      "missing_explanation": "Call out regressions.",
      "teaching_tip": "Mention the risky path.",
      "detection": {
        "type": "contains_any",
        "phrases": ["danger zone"]
      }
    }
  ]
}"#,
        )
        .unwrap();

        let config = load_steering_config(repo.path());

        assert_eq!(config.constraints.len(), 1);
        assert_eq!(config.constraints[0].label, "Sharp edges");
        assert_eq!(
            config.constraints[0].detection,
            Some(SteeringDetectionMatcher::ContainsAny {
                phrases: vec!["danger zone".to_string()]
            })
        );
    }

    #[test]
    fn old_constraint_shape_without_detection_still_loads() {
        let repo = TempDir::new().unwrap();
        let path = steering_config_path(repo.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{
  "constraints": [
    {
      "constraint": "risks",
      "label": "Sharp edges",
      "missing_explanation": "Call out regressions.",
      "teaching_tip": "Mention the risky path."
    }
  ]
}"#,
        )
        .unwrap();

        let config = load_steering_config(repo.path());

        assert_eq!(config.constraints.len(), 1);
        assert_eq!(config.constraints[0].detection, None);
    }
}
