use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project::{AgentKind, StoredVibeMode, VibeMode};
use crate::prompt_library::PromptTemplate;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CustomSessionConfig {
    pub name: String,
    pub description: Option<String>,
    pub command: Option<String>,
    pub window_name: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub icon: Option<String>,
    pub icon_nerd: Option<String>,
    pub on_stop: Option<String>,
    pub autolaunch: Option<bool>,
    pub pre_check: Option<String>,
}

impl CustomSessionConfig {
    /// Run the `pre_check` command (if any) and return
    /// `Ok(())` on success or `Err(message)` with the
    /// command output when it fails / is not found.
    pub fn run_pre_check(&self, workdir: &std::path::Path) -> std::result::Result<(), String> {
        let cmd = match &self.pre_check {
            Some(c) if !c.is_empty() => c,
            _ => return Ok(()),
        };
        match std::process::Command::new("bash")
            .arg("-c")
            .arg(cmd)
            .current_dir(workdir)
            .output()
        {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                let msg = String::from_utf8_lossy(&output.stdout);
                let err = String::from_utf8_lossy(&output.stderr);
                let combined = if !msg.is_empty() && !err.is_empty() {
                    format!("{}\n{}", msg.trim(), err.trim())
                } else if !msg.is_empty() {
                    msg.trim().to_string()
                } else if !err.is_empty() {
                    err.trim().to_string()
                } else {
                    format!("pre_check failed (exit {})", output.status)
                };
                Err(combined)
            }
            Err(e) => Err(format!("pre_check failed to run: {}", e)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPrompt {
    pub title: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HookConfig {
    Script(String),
    WithPrompt { script: String, prompt: HookPrompt },
}

impl HookConfig {
    pub fn script(&self) -> &str {
        match self {
            HookConfig::Script(s) => s,
            HookConfig::WithPrompt { script, .. } => script,
        }
    }

    pub fn prompt(&self) -> Option<&HookPrompt> {
        match self {
            HookConfig::Script(_) => None,
            HookConfig::WithPrompt { prompt, .. } => Some(prompt),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LifecycleHooks {
    pub on_start: Option<HookConfig>,
    pub on_stop: Option<HookConfig>,
    pub on_worktree_created: Option<HookConfig>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(default)]
pub struct FeaturePreset {
    pub name: String,
    pub branch_prefix: Option<String>,
    pub mode: VibeMode,
    pub agent: AgentKind,
    pub review: bool,
    pub plan_mode: bool,
    pub enable_chrome: bool,
    pub remote_control: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct FeaturePresetDe {
    name: String,
    branch_prefix: Option<String>,
    mode: StoredVibeMode,
    agent: AgentKind,
    review: bool,
    plan_mode: bool,
    enable_chrome: bool,
    remote_control: bool,
}

impl<'de> Deserialize<'de> for FeaturePreset {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let preset = FeaturePresetDe::deserialize(deserializer)?;
        let (mode, legacy_review) = preset.mode.into_mode_and_review();
        Ok(Self {
            name: preset.name,
            branch_prefix: preset.branch_prefix,
            mode,
            agent: preset.agent,
            review: preset.review || legacy_review,
            plan_mode: preset.plan_mode,
            enable_chrome: preset.enable_chrome,
            remote_control: preset.remote_control,
        })
    }
}

impl FeaturePreset {
    pub fn normalize_legacy_review_mode(&mut self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ExtensionConfig {
    pub custom_sessions: Vec<CustomSessionConfig>,
    pub lifecycle_hooks: LifecycleHooks,
    pub keybindings: HashMap<String, char>,
    pub feature_presets: Vec<FeaturePreset>,
    pub allowed_agents: Option<Vec<AgentKind>>,
    /// Declarative prompt-library templates (read-only at load time).
    /// The library view can export user templates here. Phase 3 surfaces
    /// these in the picker with a `Global` / `Project` source badge.
    pub prompt_templates: Vec<PromptTemplate>,
}

impl ExtensionConfig {
    pub fn allowed_agents(&self) -> Vec<AgentKind> {
        AgentKind::allowed_list(self.allowed_agents.as_deref())
    }

    pub fn allows_agent(&self, agent: &AgentKind) -> bool {
        self.allowed_agents().contains(agent)
    }

    pub fn allowed_feature_presets(&self) -> Vec<FeaturePreset> {
        self.feature_presets
            .iter()
            .filter(|preset| self.allows_agent(&preset.agent))
            .cloned()
            .collect()
    }

    fn normalize_legacy_review_modes(&mut self) {
        for preset in &mut self.feature_presets {
            preset.normalize_legacy_review_mode();
        }
    }
}

/// Thin wrapper used only for deserializing the
/// `extension` field out of the global config file.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct GlobalConfigPartial {
    extension: ExtensionConfig,
}

/// Load the `extension` block from
/// `~/.config/amf/config.json`.
/// Returns a default (empty) config on any failure.
pub fn load_global_extension_config() -> ExtensionConfig {
    let config_path = crate::project::amf_config_dir().join("config.json");

    if !config_path.exists() {
        return ExtensionConfig::default();
    }

    let mut config = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| serde_json::from_str::<GlobalConfigPartial>(&s).ok())
        .map(|c| c.extension)
        .unwrap_or_default();
    config.normalize_legacy_review_modes();
    config
}

/// Load `{repo}/.amf/config.json` and merge it onto
/// `base` according to the plan merge rules:
/// - custom_sessions: project appends; name collision →
///   project wins
/// - feature_presets: same rules
/// - lifecycle_hooks: project fields override global
/// - keybindings: project overrides global per-action
pub fn merge_project_extension_config(base: &ExtensionConfig, repo: &Path) -> ExtensionConfig {
    let project_path = repo.join(".amf").join("config.json");

    let project: ExtensionConfig = if project_path.exists() {
        std::fs::read_to_string(&project_path)
            .ok()
            .and_then(|s| serde_json::from_str::<ExtensionConfig>(&s).ok())
            .unwrap_or_default()
    } else {
        return base.clone();
    };

    // Merge custom_sessions: start with project, then
    // append global entries whose name doesn't collide.
    let mut custom_sessions = project.custom_sessions.clone();
    for entry in &base.custom_sessions {
        if !custom_sessions.iter().any(|e| e.name == entry.name) {
            custom_sessions.push(entry.clone());
        }
    }

    // Merge feature_presets: same strategy.
    let mut feature_presets = project.feature_presets.clone();
    for entry in &base.feature_presets {
        if !feature_presets.iter().any(|e| e.name == entry.name) {
            feature_presets.push(entry.clone());
        }
    }

    // Merge prompt_templates by name (project wins).
    let mut prompt_templates = project.prompt_templates.clone();
    for entry in &base.prompt_templates {
        if !prompt_templates.iter().any(|e| e.name == entry.name) {
            prompt_templates.push(entry.clone());
        }
    }

    // Merge lifecycle_hooks: project fields take priority.
    let on_start = project
        .lifecycle_hooks
        .on_start
        .clone()
        .or_else(|| base.lifecycle_hooks.on_start.clone());
    let on_stop = project
        .lifecycle_hooks
        .on_stop
        .clone()
        .or_else(|| base.lifecycle_hooks.on_stop.clone());
    let on_worktree_created = project
        .lifecycle_hooks
        .on_worktree_created
        .clone()
        .or_else(|| base.lifecycle_hooks.on_worktree_created.clone());

    let mut keybindings = base.keybindings.clone();
    for (action, key) in &project.keybindings {
        keybindings.insert(action.clone(), *key);
    }

    let mut merged = ExtensionConfig {
        custom_sessions,
        lifecycle_hooks: LifecycleHooks {
            on_start,
            on_stop,
            on_worktree_created,
        },
        keybindings,
        feature_presets,
        allowed_agents: project
            .allowed_agents
            .clone()
            .or_else(|| base.allowed_agents.clone()),
        prompt_templates,
    };
    merged.normalize_legacy_review_modes();
    merged
}

/// Write an `ExtensionConfig` to `{repo}/.amf/config.json`.
pub fn save_project_extension_config(repo: &Path, config: &ExtensionConfig) -> anyhow::Result<()> {
    let amf_dir = repo.join(".amf");
    std::fs::create_dir_all(&amf_dir)?;
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(amf_dir.join("config.json"), json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn write_extension_config(dir: &TempDir, config: &ExtensionConfig) {
        let amf_dir = dir.path().join(".amf");
        std::fs::create_dir_all(&amf_dir).unwrap();
        let json = serde_json::to_string(config).unwrap();
        std::fs::write(amf_dir.join("config.json"), json).unwrap();
    }

    // ── merge_project_extension_config ────────────────────────

    #[test]
    fn no_project_config_returns_base_unchanged() {
        let global = ExtensionConfig {
            custom_sessions: vec![CustomSessionConfig {
                name: "test".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap(); // no .amf/config.json
        let merged = merge_project_extension_config(&global, tmp.path());
        assert_eq!(merged.custom_sessions.len(), 1);
        assert_eq!(merged.custom_sessions[0].name, "test");
    }

    #[test]
    fn project_hook_overrides_global_hook() {
        let global = ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_start: Some(HookConfig::Script("global-start.sh".to_string())),
                ..Default::default()
            },
            ..Default::default()
        };
        let project_config = ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_start: Some(HookConfig::Script("project-start.sh".to_string())),
                ..Default::default()
            },
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());
        let on_start = merged.lifecycle_hooks.on_start.unwrap();
        assert_eq!(on_start.script(), "project-start.sh");
    }

    #[test]
    fn global_hook_used_when_project_does_not_set_it() {
        let global = ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_start: Some(HookConfig::Script("global-start.sh".to_string())),
                ..Default::default()
            },
            ..Default::default()
        };
        // Project config present but no on_start
        let project_config = ExtensionConfig::default();
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());
        let on_start = merged.lifecycle_hooks.on_start.unwrap();
        assert_eq!(on_start.script(), "global-start.sh");
    }

    #[test]
    fn custom_sessions_deduplicated_by_name_project_wins() {
        let global = ExtensionConfig {
            custom_sessions: vec![
                CustomSessionConfig {
                    name: "shared".to_string(),
                    command: Some("global-cmd".to_string()),
                    ..Default::default()
                },
                CustomSessionConfig {
                    name: "global-only".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let project_config = ExtensionConfig {
            custom_sessions: vec![
                CustomSessionConfig {
                    name: "shared".to_string(),
                    command: Some("project-cmd".to_string()),
                    ..Default::default()
                },
                CustomSessionConfig {
                    name: "project-only".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());
        // "shared" must appear exactly once (project version)
        let shared: Vec<_> = merged
            .custom_sessions
            .iter()
            .filter(|s| s.name == "shared")
            .collect();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].command.as_deref(), Some("project-cmd"));
        // Both unique entries should be present
        assert!(
            merged
                .custom_sessions
                .iter()
                .any(|s| s.name == "global-only")
        );
        assert!(
            merged
                .custom_sessions
                .iter()
                .any(|s| s.name == "project-only")
        );
        assert_eq!(merged.custom_sessions.len(), 3);
    }

    #[test]
    fn prompt_templates_merged_by_name_project_wins() {
        let global = ExtensionConfig {
            prompt_templates: vec![
                PromptTemplate::new("shared".to_string(), "global body".to_string()),
                PromptTemplate::new("global-only".to_string(), "g".to_string()),
            ],
            ..Default::default()
        };
        let project_config = ExtensionConfig {
            prompt_templates: vec![
                PromptTemplate::new("shared".to_string(), "project body".to_string()),
                PromptTemplate::new("project-only".to_string(), "p".to_string()),
            ],
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());
        // "shared" must appear exactly once (project version).
        let shared: Vec<_> = merged
            .prompt_templates
            .iter()
            .filter(|t| t.name == "shared")
            .collect();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].body, "project body");
        // Both unique entries should be present.
        assert!(
            merged
                .prompt_templates
                .iter()
                .any(|t| t.name == "global-only")
        );
        assert!(
            merged
                .prompt_templates
                .iter()
                .any(|t| t.name == "project-only")
        );
        assert_eq!(merged.prompt_templates.len(), 3);
    }

    #[test]
    fn keybindings_project_overrides_per_action() {
        let mut global_bindings = HashMap::new();
        global_bindings.insert("quit".to_string(), 'q');
        global_bindings.insert("delete".to_string(), 'd');

        let global = ExtensionConfig {
            keybindings: global_bindings,
            ..Default::default()
        };
        let mut project_bindings = HashMap::new();
        // Override quit only
        project_bindings.insert("quit".to_string(), 'Q');

        let project_config = ExtensionConfig {
            keybindings: project_bindings,
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());
        assert_eq!(merged.keybindings.get("quit"), Some(&'Q'));
        // Global key preserved when not overridden
        assert_eq!(merged.keybindings.get("delete"), Some(&'d'));
    }

    #[test]
    fn project_allowed_agents_override_global_allowed_agents() {
        let global = ExtensionConfig {
            allowed_agents: Some(vec![AgentKind::Claude]),
            ..Default::default()
        };
        let project_config = ExtensionConfig {
            allowed_agents: Some(vec![AgentKind::Opencode]),
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());
        assert_eq!(merged.allowed_agents(), vec![AgentKind::Opencode]);
    }

    #[test]
    fn empty_allowed_agents_means_allow_all() {
        let config = ExtensionConfig {
            allowed_agents: Some(vec![]),
            ..Default::default()
        };

        assert_eq!(config.allowed_agents(), AgentKind::ALL.to_vec());
    }

    #[test]
    fn merge_normalizes_legacy_review_preset_mode() {
        let global = ExtensionConfig::default();
        let tmp = TempDir::new().unwrap();
        let raw = r#"{
            "feature_presets": [
                {
                    "name": "review-preset",
                    "mode": "review",
                    "agent": "claude",
                    "review": false,
                    "enable_chrome": false
                }
            ]
        }"#;
        std::fs::create_dir_all(tmp.path().join(".amf")).unwrap();
        std::fs::write(tmp.path().join(".amf").join("config.json"), raw).unwrap();

        let merged = merge_project_extension_config(&global, tmp.path());
        assert_eq!(merged.feature_presets.len(), 1);
        assert_eq!(merged.feature_presets[0].mode, VibeMode::Vibeless);
        assert!(merged.feature_presets[0].review);
    }

    #[test]
    fn save_project_extension_config_writes_raw_extension_json() {
        let tmp = TempDir::new().unwrap();
        let config = ExtensionConfig {
            custom_sessions: vec![CustomSessionConfig {
                name: "lint".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        save_project_extension_config(tmp.path(), &config).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join(".amf").join("config.json")).unwrap();
        let loaded: ExtensionConfig = serde_json::from_str(&raw).unwrap();
        assert_eq!(loaded.custom_sessions.len(), 1);
        assert_eq!(loaded.custom_sessions[0].name, "lint");
    }
}
