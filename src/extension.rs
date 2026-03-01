use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project::{AgentKind, VibeMode};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CustomSessionConfig {
    pub name: String,
    pub description: Option<String>,
    pub command: Option<String>,
    pub window_name: Option<String>,
    pub working_dir: Option<PathBuf>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FeaturePreset {
    pub name: String,
    pub branch_prefix: Option<String>,
    pub mode: VibeMode,
    pub agent: AgentKind,
    pub review: bool,
    pub enable_chrome: bool,
    pub enable_notes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ExtensionConfig {
    pub custom_sessions: Vec<CustomSessionConfig>,
    pub lifecycle_hooks: LifecycleHooks,
    pub keybindings: HashMap<String, char>,
    pub feature_presets: Vec<FeaturePreset>,
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

    std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| serde_json::from_str::<GlobalConfigPartial>(&s).ok())
        .map(|c| c.extension)
        .unwrap_or_default()
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

    ExtensionConfig {
        custom_sessions,
        lifecycle_hooks: LifecycleHooks {
            on_start,
            on_stop,
            on_worktree_created,
        },
        keybindings,
        feature_presets,
    }
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
        let merged =
            merge_project_extension_config(&global, tmp.path());
        assert_eq!(merged.custom_sessions.len(), 1);
        assert_eq!(merged.custom_sessions[0].name, "test");
    }

    #[test]
    fn project_hook_overrides_global_hook() {
        let global = ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_start: Some(HookConfig::Script(
                    "global-start.sh".to_string(),
                )),
                ..Default::default()
            },
            ..Default::default()
        };
        let project_config = ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_start: Some(HookConfig::Script(
                    "project-start.sh".to_string(),
                )),
                ..Default::default()
            },
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged =
            merge_project_extension_config(&global, tmp.path());
        let on_start =
            merged.lifecycle_hooks.on_start.unwrap();
        assert_eq!(on_start.script(), "project-start.sh");
    }

    #[test]
    fn global_hook_used_when_project_does_not_set_it() {
        let global = ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_start: Some(HookConfig::Script(
                    "global-start.sh".to_string(),
                )),
                ..Default::default()
            },
            ..Default::default()
        };
        // Project config present but no on_start
        let project_config = ExtensionConfig::default();
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged =
            merge_project_extension_config(&global, tmp.path());
        let on_start =
            merged.lifecycle_hooks.on_start.unwrap();
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

        let merged =
            merge_project_extension_config(&global, tmp.path());
        // "shared" must appear exactly once (project version)
        let shared: Vec<_> = merged
            .custom_sessions
            .iter()
            .filter(|s| s.name == "shared")
            .collect();
        assert_eq!(shared.len(), 1);
        assert_eq!(
            shared[0].command.as_deref(),
            Some("project-cmd")
        );
        // Both unique entries should be present
        assert!(merged
            .custom_sessions
            .iter()
            .any(|s| s.name == "global-only"));
        assert!(merged
            .custom_sessions
            .iter()
            .any(|s| s.name == "project-only"));
        assert_eq!(merged.custom_sessions.len(), 3);
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

        let merged =
            merge_project_extension_config(&global, tmp.path());
        assert_eq!(merged.keybindings.get("quit"), Some(&'Q'));
        // Global key preserved when not overridden
        assert_eq!(
            merged.keybindings.get("delete"),
            Some(&'d')
        );
    }
}
