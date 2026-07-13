use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::NamedTempFile;

use super::*;
use crate::extension::{
    CustomSessionConfig, ExtensionConfig, FeaturePreset, HookConfig, HookPrompt, LifecycleHooks,
    save_project_extension_config,
};
use crate::project::{AgentKind, VibeMode};

impl App {
    pub fn start_config_wizard(&mut self) {
        let (project_repo, project_name) = self
            .selected_project()
            .map(|project| (Some(project.repo.clone()), Some(project.name.clone())))
            .unwrap_or((None, None));

        self.mode = AppMode::ConfigWizard(ConfigWizardState {
            step: ConfigWizardStep::CategoryPicker,
            category: ConfigCategory::CustomSessions,
            scope: ConfigScope::Global,
            selected: 0,
            field_focus: 0,
            input_mode: false,
            sessions: Vec::new(),
            presets: Vec::new(),
            hooks: LifecycleHooks::default(),
            keybindings: HashMap::new(),
            allowed_agents: None,
            editing_index: None,
            field_values: Vec::new(),
            field_editor: None,
            field_toggles: Vec::new(),
            agent_toggles: vec![true; AgentKind::ALL.len()],
            agent_toggles_dirty: false,
            keybinding_actions: Vec::new(),
            capturing_key: false,
            original_json: String::new(),
            modified_json: String::new(),
            confirm_diff: None,
            preview_scroll: 0,
            project_repo,
            project_name,
            error: None,
        });
        self.message = None;
    }

    pub fn config_wizard_select_scope(&mut self) -> Result<()> {
        let scope = match &self.mode {
            AppMode::ConfigWizard(state) => state.scope.clone(),
            _ => return Ok(()),
        };

        let config = match scope {
            ConfigScope::Global => self.config.extension.clone(),
            ConfigScope::Project(ref repo) => load_project_extension_scope(repo)?,
        };
        let original_json = serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".into());
        let keybinding_actions = sorted_keybinding_actions(&config.keybindings);
        let allowed_agents = config.allowed_agents.clone();
        let agent_toggles = agent_toggles_from_allowed(allowed_agents.as_deref());

        if let AppMode::ConfigWizard(state) = &mut self.mode {
            state.sessions = config.custom_sessions;
            state.presets = config.feature_presets;
            state.hooks = config.lifecycle_hooks;
            state.keybindings = config.keybindings;
            state.allowed_agents = allowed_agents;
            state.agent_toggles = agent_toggles;
            state.agent_toggles_dirty = false;
            state.keybinding_actions = keybinding_actions;
            state.original_json = original_json.clone();
            state.modified_json = original_json;
            state.confirm_diff = None;
            state.editing_index = None;
            state.field_values.clear();
            state.field_editor = None;
            state.field_toggles.clear();
            state.field_focus = 0;
            state.input_mode = false;
            state.selected = 0;
            state.preview_scroll = 0;
            state.capturing_key = false;
            state.error = None;
            state.step = ConfigWizardStep::ItemList;
        }

        Ok(())
    }

    pub fn config_wizard_start_edit(&mut self, index: Option<usize>) {
        if let AppMode::ConfigWizard(state) = &mut self.mode {
            state.editing_index = index;
            state.field_focus = 0;
            state.input_mode = false;
            state.field_editor = None;
            state.preview_scroll = 0;
            state.capturing_key = false;
            state.error = None;

            match state.category {
                ConfigCategory::CustomSessions => {
                    let session = index
                        .and_then(|i| state.sessions.get(i).cloned())
                        .unwrap_or_default();
                    state.field_values = vec![
                        session.name,
                        session.description.unwrap_or_default(),
                        session.command.unwrap_or_default(),
                        session.window_name.unwrap_or_default(),
                        session
                            .working_dir
                            .map(|path| path.display().to_string())
                            .unwrap_or_default(),
                        session.icon.unwrap_or_default(),
                        session.icon_nerd.unwrap_or_default(),
                        session.on_stop.unwrap_or_default(),
                        session.pre_check.unwrap_or_default(),
                    ];
                    state.field_toggles = vec![session.autolaunch.unwrap_or(false)];
                }
                ConfigCategory::FeaturePresets => {
                    let preset = index
                        .and_then(|i| state.presets.get(i).cloned())
                        .unwrap_or_default();
                    state.field_values = vec![
                        preset.name,
                        preset.branch_prefix.unwrap_or_default(),
                        mode_to_index(&preset.mode).to_string(),
                        agent_to_index(&preset.agent).to_string(),
                    ];
                    state.field_toggles = vec![
                        preset.review,
                        preset.plan_mode,
                        preset.enable_chrome,
                        preset.remote_control,
                    ];
                }
                ConfigCategory::LifecycleHooks => {
                    let hook =
                        selected_hook(&state.hooks, index.unwrap_or(state.selected)).cloned();
                    let (script, prompt_title, prompt_options, prompted) = match hook {
                        Some(HookConfig::Script(script)) => {
                            (script, String::new(), String::new(), false)
                        }
                        Some(HookConfig::WithPrompt { script, prompt }) => {
                            (script, prompt.title, prompt.options.join(", "), true)
                        }
                        None => (String::new(), String::new(), String::new(), false),
                    };
                    state.field_values = vec![script, prompt_title, prompt_options];
                    state.field_toggles = vec![prompted];
                }
                ConfigCategory::Keybindings => {
                    let action = index
                        .and_then(|i| state.keybinding_actions.get(i).cloned())
                        .unwrap_or_default();
                    let key = state
                        .keybindings
                        .get(&action)
                        .copied()
                        .or_else(|| crate::handlers::default_key_for_action(&action))
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    state.field_values = vec![action, key];
                    state.field_toggles = Vec::new();
                    state.field_focus = usize::from(index.is_some());
                }
                ConfigCategory::AllowedAgents => return,
            }

            state.step = ConfigWizardStep::EditItem;
        }
    }

    pub fn config_wizard_finish_edit(&mut self) -> bool {
        if let AppMode::ConfigWizard(state) = &mut self.mode {
            state.error = None;

            match state.category {
                ConfigCategory::CustomSessions => {
                    let name = state
                        .field_values
                        .first()
                        .map(|value| value.trim())
                        .unwrap_or_default()
                        .to_string();
                    if name.is_empty() {
                        state.error = Some("Session name cannot be empty".into());
                        return false;
                    }

                    let session = CustomSessionConfig {
                        name,
                        description: option_string(state.field_values.get(1)),
                        command: option_string(state.field_values.get(2)),
                        window_name: option_string(state.field_values.get(3)),
                        working_dir: option_pathbuf(state.field_values.get(4)),
                        icon: option_string(state.field_values.get(5)),
                        icon_nerd: option_string(state.field_values.get(6)),
                        on_stop: option_string(state.field_values.get(7)),
                        autolaunch: bool_to_option(*state.field_toggles.first().unwrap_or(&false)),
                        pre_check: option_string(state.field_values.get(8)),
                    };

                    upsert_or_push(&mut state.sessions, state.editing_index, session);
                }
                ConfigCategory::FeaturePresets => {
                    let name = state
                        .field_values
                        .first()
                        .map(|value| value.trim())
                        .unwrap_or_default()
                        .to_string();
                    if name.is_empty() {
                        state.error = Some("Preset name cannot be empty".into());
                        return false;
                    }

                    let preset = FeaturePreset {
                        name,
                        branch_prefix: option_string(state.field_values.get(1)),
                        mode: index_to_mode(
                            state
                                .field_values
                                .get(2)
                                .and_then(|value| value.parse::<usize>().ok())
                                .unwrap_or(0),
                        ),
                        agent: index_to_agent(
                            state
                                .field_values
                                .get(3)
                                .and_then(|value| value.parse::<usize>().ok())
                                .unwrap_or(0),
                        ),
                        review: *state.field_toggles.first().unwrap_or(&false),
                        plan_mode: *state.field_toggles.get(1).unwrap_or(&false),
                        enable_chrome: *state.field_toggles.get(2).unwrap_or(&false),
                        remote_control: *state.field_toggles.get(3).unwrap_or(&false),
                    };

                    upsert_or_push(&mut state.presets, state.editing_index, preset);
                }
                ConfigCategory::LifecycleHooks => {
                    let script = state
                        .field_values
                        .first()
                        .map(|value| value.trim())
                        .unwrap_or_default()
                        .to_string();
                    let prompted = *state.field_toggles.first().unwrap_or(&false);
                    let hook = if script.is_empty() {
                        None
                    } else if prompted {
                        let title = option_string(state.field_values.get(1))
                            .unwrap_or_else(|| "Choose an option".to_string());
                        let options = split_prompt_options(state.field_values.get(2));
                        Some(HookConfig::WithPrompt {
                            script,
                            prompt: HookPrompt { title, options },
                        })
                    } else {
                        Some(HookConfig::Script(script))
                    };

                    set_selected_hook(
                        &mut state.hooks,
                        state.editing_index.unwrap_or(state.selected),
                        hook,
                    );
                }
                ConfigCategory::Keybindings => {
                    let action = state
                        .field_values
                        .first()
                        .map(|value| value.trim())
                        .unwrap_or_default()
                        .to_string();
                    if action.is_empty() {
                        state.error = Some("Action name cannot be empty".into());
                        return false;
                    }

                    let key = state
                        .field_values
                        .get(1)
                        .and_then(|value| value.chars().next());
                    let Some(key) = key else {
                        state.error = Some("Press a key to bind".into());
                        return false;
                    };

                    state.keybindings.insert(action, key);
                    state.keybinding_actions = sorted_keybinding_actions(&state.keybindings);
                }
                ConfigCategory::AllowedAgents => {}
            }

            state.field_values.clear();
            state.field_editor = None;
            state.field_toggles.clear();
            state.field_focus = 0;
            state.input_mode = false;
            state.capturing_key = false;
            state.selected =
                adjusted_selected_index(state.selected, item_count_for_category(state));
            state.step = ConfigWizardStep::ItemList;
            return true;
        }

        false
    }

    pub fn config_wizard_prepare_confirm(&mut self) -> Result<()> {
        if let AppMode::ConfigWizard(state) = &mut self.mode
            && state.agent_toggles_dirty
            && !state.agent_toggles.iter().any(|enabled| *enabled)
        {
            state.error = Some("Select at least one harness".into());
            return Ok(());
        }

        let modified = match &self.mode {
            AppMode::ConfigWizard(state) => build_extension_config(state),
            _ => return Ok(()),
        };

        let modified_json = serde_json::to_string_pretty(&modified)?;
        let confirm_diff =
            build_config_confirm_diff(&original_json_for_mode(&self.mode), &modified_json).ok();

        if let AppMode::ConfigWizard(state) = &mut self.mode {
            state.modified_json = modified_json;
            state.confirm_diff = confirm_diff;
            state.preview_scroll = 0;
            state.error = None;
            state.step = ConfigWizardStep::ConfirmSave;
        }

        Ok(())
    }

    pub fn config_wizard_save(&mut self) -> Result<()> {
        let (scope, config) = match &self.mode {
            AppMode::ConfigWizard(state) => (state.scope.clone(), build_extension_config(state)),
            _ => return Ok(()),
        };

        match scope {
            ConfigScope::Global => {
                self.config.extension = config;
                self.save_config();
                self.log_info(
                    "config_wizard",
                    "saved global config wizard changes".to_string(),
                );
            }
            ConfigScope::Project(repo) => {
                save_project_extension_config(&repo, &config)?;
                self.log_info(
                    "config_wizard",
                    format!("saved project config wizard changes to {}", repo.display()),
                );
            }
        }

        self.reload_extension_config();
        self.mode = AppMode::Normal;
        self.message = Some("Saved config changes".into());
        Ok(())
    }
}

fn original_json_for_mode(mode: &AppMode) -> String {
    match mode {
        AppMode::ConfigWizard(state) => state.original_json.clone(),
        _ => "{}".to_string(),
    }
}

fn build_config_confirm_diff(
    original_json: &str,
    modified_json: &str,
) -> Result<crate::diff::DiffFile> {
    let mut original = NamedTempFile::new()?;
    original.write_all(original_json.as_bytes())?;
    let mut modified = NamedTempFile::new()?;
    modified.write_all(modified_json.as_bytes())?;
    crate::diff::load_review_file(original.path(), modified.path(), "config.json")
}

fn load_project_extension_scope(repo: &Path) -> Result<ExtensionConfig> {
    let path = repo.join(".amf").join("config.json");
    if !path.exists() {
        return Ok(ExtensionConfig::default());
    }

    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read project config {}", path.display()))?;
    let config = serde_json::from_str::<ExtensionConfig>(&raw)
        .with_context(|| format!("failed to parse project config {}", path.display()))?;
    Ok(config)
}

fn build_extension_config(state: &ConfigWizardState) -> ExtensionConfig {
    ExtensionConfig {
        custom_sessions: state.sessions.clone(),
        lifecycle_hooks: state.hooks.clone(),
        keybindings: state.keybindings.clone(),
        feature_presets: state.presets.clone(),
        allowed_agents: if state.agent_toggles_dirty {
            agent_toggles_to_allowed(&state.scope, &state.agent_toggles)
        } else {
            state.allowed_agents.clone()
        },
        // The wizard doesn't edit prompt templates; carry the existing
        // ones through from the loaded config so saving doesn't wipe
        // prompts exported via the prompt library.
        prompt_templates: serde_json::from_str::<ExtensionConfig>(&state.original_json)
            .map(|config| config.prompt_templates)
            .unwrap_or_default(),
        // Plan interview questions are declarative-only for now; preserve
        // them when the wizard edits another extension setting.
        plan_questions: serde_json::from_str::<ExtensionConfig>(&state.original_json)
            .map(|config| config.plan_questions)
            .unwrap_or_default(),
        skip_builtin_questions: serde_json::from_str::<ExtensionConfig>(&state.original_json)
            .ok()
            .and_then(|config| config.skip_builtin_questions),
        // Same reasoning: the wizard has no UI for the final-review check
        // command, so carry the loaded value through untouched.
        final_review_check_command: serde_json::from_str::<ExtensionConfig>(&state.original_json)
            .ok()
            .and_then(|config| config.final_review_check_command),
    }
}

fn sorted_keybinding_actions(_map: &HashMap<String, char>) -> Vec<String> {
    // All dashboard actions have a default key, so they are always listed.
    // Extra actions have no default but are always offered so they can be
    // discovered and bound from the wizard.
    crate::handlers::DASHBOARD_KEYBINDING_ACTIONS
        .iter()
        .map(|(action, _)| (*action).to_string())
        .chain(
            crate::handlers::EXTRA_KEYBINDING_ACTIONS
                .iter()
                .map(|action| (*action).to_string()),
        )
        .collect()
}

fn option_string(value: Option<&String>) -> Option<String> {
    value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn option_pathbuf(value: Option<&String>) -> Option<PathBuf> {
    option_string(value).map(PathBuf::from)
}

fn bool_to_option(value: bool) -> Option<bool> {
    if value { Some(true) } else { None }
}

fn upsert_or_push<T>(items: &mut Vec<T>, index: Option<usize>, item: T) {
    if let Some(index) = index
        && let Some(slot) = items.get_mut(index)
    {
        *slot = item;
        return;
    }
    items.push(item);
}

fn mode_to_index(mode: &VibeMode) -> usize {
    match mode {
        VibeMode::Vibeless => 0,
        VibeMode::Vibe => 1,
        VibeMode::SuperVibe => 2,
    }
}

fn index_to_mode(index: usize) -> VibeMode {
    match index {
        1 => VibeMode::Vibe,
        2 => VibeMode::SuperVibe,
        _ => VibeMode::Vibeless,
    }
}

fn agent_to_index(agent: &AgentKind) -> usize {
    AgentKind::index_in(&AgentKind::ALL, agent)
}

fn index_to_agent(index: usize) -> AgentKind {
    AgentKind::ALL.get(index).cloned().unwrap_or_default()
}

fn selected_hook(hooks: &LifecycleHooks, selected: usize) -> Option<&HookConfig> {
    match selected {
        0 => hooks.on_start.as_ref(),
        1 => hooks.on_stop.as_ref(),
        _ => hooks.on_worktree_created.as_ref(),
    }
}

fn set_selected_hook(hooks: &mut LifecycleHooks, selected: usize, hook: Option<HookConfig>) {
    match selected {
        0 => hooks.on_start = hook,
        1 => hooks.on_stop = hook,
        _ => hooks.on_worktree_created = hook,
    }
}

fn split_prompt_options(value: Option<&String>) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn adjusted_selected_index(selected: usize, len: usize) -> usize {
    match len {
        0 => 0,
        _ => selected.min(len.saturating_sub(1)),
    }
}

fn item_count_for_category(state: &ConfigWizardState) -> usize {
    match state.category {
        ConfigCategory::CustomSessions => state.sessions.len(),
        ConfigCategory::FeaturePresets => state.presets.len(),
        ConfigCategory::LifecycleHooks => 3,
        ConfigCategory::Keybindings => state.keybinding_actions.len(),
        ConfigCategory::AllowedAgents => AgentKind::ALL.len(),
    }
}

fn agent_toggles_from_allowed(allowed: Option<&[AgentKind]>) -> Vec<bool> {
    AgentKind::ALL
        .iter()
        .map(|agent| allowed.is_none_or(|allowed| allowed.is_empty() || allowed.contains(agent)))
        .collect()
}

pub(crate) fn agent_toggles_to_allowed(
    scope: &ConfigScope,
    toggles: &[bool],
) -> Option<Vec<AgentKind>> {
    let allowed: Vec<_> = AgentKind::ALL
        .iter()
        .zip(toggles.iter().copied())
        .filter_map(|(agent, enabled)| enabled.then_some(agent.clone()))
        .collect();
    match (scope, allowed.len() == AgentKind::ALL.len()) {
        (ConfigScope::Global, true) => None,
        (ConfigScope::Project(_), true) => Some(Vec::new()),
        (_, false) => Some(allowed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_for_allowed_agents(
        scope: ConfigScope,
        allowed_agents: Option<Vec<AgentKind>>,
        agent_toggles_dirty: bool,
    ) -> ConfigWizardState {
        let agent_toggles = agent_toggles_from_allowed(allowed_agents.as_deref());
        ConfigWizardState {
            step: ConfigWizardStep::ItemList,
            category: ConfigCategory::AllowedAgents,
            scope,
            selected: 0,
            field_focus: 0,
            input_mode: false,
            sessions: Vec::new(),
            presets: Vec::new(),
            hooks: LifecycleHooks::default(),
            keybindings: HashMap::new(),
            allowed_agents,
            editing_index: None,
            field_values: Vec::new(),
            field_editor: None,
            field_toggles: Vec::new(),
            agent_toggles,
            agent_toggles_dirty,
            keybinding_actions: Vec::new(),
            capturing_key: false,
            original_json: String::new(),
            modified_json: String::new(),
            confirm_diff: None,
            preview_scroll: 0,
            project_repo: None,
            project_name: None,
            error: None,
        }
    }

    #[test]
    fn agent_toggles_track_all_agent_kinds() {
        let toggles = agent_toggles_from_allowed(None);

        assert_eq!(toggles.len(), AgentKind::ALL.len());
        assert!(toggles.iter().all(|enabled| *enabled));
    }

    #[test]
    fn global_all_agent_toggles_serialize_as_unrestricted() {
        let allowed =
            agent_toggles_to_allowed(&ConfigScope::Global, &vec![true; AgentKind::ALL.len()]);

        assert_eq!(allowed, None);
    }

    #[test]
    fn project_all_agent_toggles_serialize_as_explicit_allow_all() {
        let allowed = agent_toggles_to_allowed(
            &ConfigScope::Project(PathBuf::from("/repo")),
            &vec![true; AgentKind::ALL.len()],
        );

        assert_eq!(allowed, Some(Vec::new()));
    }

    #[test]
    fn untouched_project_allowed_agents_preserve_inheritance() {
        let state =
            state_for_allowed_agents(ConfigScope::Project(PathBuf::from("/repo")), None, false);

        assert_eq!(build_extension_config(&state).allowed_agents, None);
    }

    #[test]
    fn touched_project_allowed_agents_can_override_global_with_allow_all() {
        let state =
            state_for_allowed_agents(ConfigScope::Project(PathBuf::from("/repo")), None, true);

        assert_eq!(
            build_extension_config(&state).allowed_agents,
            Some(Vec::new())
        );
    }

    #[test]
    fn untouched_project_explicit_allow_all_is_preserved() {
        let state = state_for_allowed_agents(
            ConfigScope::Project(PathBuf::from("/repo")),
            Some(Vec::new()),
            false,
        );

        assert_eq!(
            build_extension_config(&state).allowed_agents,
            Some(Vec::new())
        );
    }

    #[test]
    fn saving_unrelated_settings_preserves_plan_question_config() {
        let mut state =
            state_for_allowed_agents(ConfigScope::Project(PathBuf::from("/repo")), None, false);
        let original = ExtensionConfig {
            plan_questions: vec![crate::extension::ConfiguredPlanQuestion {
                id: "delivery".into(),
                text: "Where should this ship?".into(),
                options: vec!["Desktop".into(), "Web".into()],
                optional: false,
            }],
            skip_builtin_questions: Some(true),
            ..Default::default()
        };
        state.original_json = serde_json::to_string(&original).unwrap();

        let rebuilt = build_extension_config(&state);

        assert_eq!(rebuilt.plan_questions, original.plan_questions);
        assert_eq!(rebuilt.skip_builtin_questions, Some(true));
    }
}
