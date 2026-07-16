use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::plan_interview::{PlanQuestion, PlanQuestionKind, QuestionSource, builtin_questions};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfiguredPlanQuestion {
    /// Stable identifier used when global, project, and built-in questions are
    /// merged. A project question with the same ID replaces the earlier one.
    pub id: String,
    pub text: String,
    /// An empty list produces a free-text question; a non-empty list produces
    /// a native select question, matching the flat option shape used by hook
    /// prompts in config.json.
    pub options: Vec<String>,
    pub optional: bool,
    /// Runtime-only provenance used by the interview UI. Config files do not
    /// need to declare their own scope; the global/project loader supplies it.
    #[serde(skip)]
    pub(crate) source: QuestionSource,
}

impl Default for ConfiguredPlanQuestion {
    fn default() -> Self {
        Self {
            id: String::new(),
            text: String::new(),
            options: Vec::new(),
            optional: true,
            source: QuestionSource::Template,
        }
    }
}

impl ConfiguredPlanQuestion {
    fn to_plan_question(&self) -> Option<PlanQuestion> {
        let id = self.id.trim();
        let text = self.text.trim();
        if id.is_empty() || text.is_empty() {
            return None;
        }

        let options = self
            .options
            .iter()
            .map(|option| option.trim())
            .filter(|option| !option.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();

        Some(PlanQuestion {
            id: id.to_string(),
            text: text.to_string(),
            kind: if options.is_empty() {
                PlanQuestionKind::FreeText
            } else {
                PlanQuestionKind::Select(options)
            },
            source: self.source.clone(),
            optional: self.optional,
        })
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
    /// Questions layered onto the built-in plan interview bank. IDs are stable
    /// merge keys; project entries replace global entries with the same ID.
    pub plan_questions: Vec<ConfiguredPlanQuestion>,
    /// `None` means inherit from the broader config scope. This keeps an empty
    /// project config from accidentally undoing a global opt-out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_builtin_questions: Option<bool>,
    /// Shell command run (via `bash -c` in the feature's workdir) when
    /// finishing a final review — a build/test gate. `None`/empty skips it
    /// entirely (the default). Project overrides global, same as
    /// `lifecycle_hooks`, so each repo can point this at its own proof / CI
    /// script rather than a hardcoded `cargo build`.
    pub final_review_check_command: Option<String>,
    /// Repo-relative (or absolute) path to the review-findings memory doc
    /// (Epic E of `pr-comment-review-plan.md`), overriding
    /// `AppConfig::review_memory_path` for this project only. Project
    /// overrides global, same as `final_review_check_command`. Falls back to
    /// `AppConfig::review_memory_path`, then
    /// [`crate::app::review_memory::DEFAULT_REVIEW_MEMORY_PATH`], when unset.
    pub review_memory_path: Option<String>,
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

    /// Build the static interview bank after applying configured overrides.
    /// Replacing a built-in keeps its original position; new questions append
    /// in merged config order.
    pub fn plan_interview_questions(&self) -> Vec<PlanQuestion> {
        let mut questions = if self.skip_builtin_questions.unwrap_or(false) {
            Vec::new()
        } else {
            builtin_questions()
        };

        for configured in &self.plan_questions {
            let Some(question) = configured.to_plan_question() else {
                continue;
            };
            if let Some(index) = questions
                .iter()
                .position(|existing| existing.id == question.id)
            {
                questions[index] = question;
            } else {
                questions.push(question);
            }
        }

        questions
    }

    fn normalize_legacy_review_modes(&mut self) {
        for preset in &mut self.feature_presets {
            preset.normalize_legacy_review_mode();
        }
    }

    fn mark_global_plan_questions(&mut self) {
        for question in &mut self.plan_questions {
            question.source = QuestionSource::GlobalTemplate;
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
    config.mark_global_plan_questions();
    config
}

/// Load `{repo}/.amf/config.json` and merge it onto
/// `base` according to the plan merge rules:
/// - custom_sessions: project appends; name collision →
///   project wins
/// - feature_presets: same rules
/// - lifecycle_hooks: project fields override global
/// - keybindings: project overrides global per-action
/// - plan_questions: project appends; ID collision → project wins
/// - skip_builtin_questions: project overrides global when explicitly set
pub fn merge_project_extension_config(base: &ExtensionConfig, repo: &Path) -> ExtensionConfig {
    let project_path = repo.join(".amf").join("config.json");

    let project: ExtensionConfig = if project_path.exists() {
        std::fs::read_to_string(&project_path)
            .ok()
            .and_then(|s| serde_json::from_str::<ExtensionConfig>(&s).ok())
            .unwrap_or_default()
    } else {
        let mut merged = base.clone();
        merged.mark_global_plan_questions();
        return merged;
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

    // Merge plan questions by stable ID (project wins).
    let mut plan_questions = project.plan_questions.clone();
    for entry in &base.plan_questions {
        if !plan_questions
            .iter()
            .any(|question| question.id.trim() == entry.id.trim())
        {
            let mut global_question = entry.clone();
            global_question.source = QuestionSource::GlobalTemplate;
            plan_questions.push(global_question);
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

    let final_review_check_command = project
        .final_review_check_command
        .clone()
        .or_else(|| base.final_review_check_command.clone());

    let review_memory_path = project
        .review_memory_path
        .clone()
        .or_else(|| base.review_memory_path.clone());

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
        plan_questions,
        skip_builtin_questions: project
            .skip_builtin_questions
            .or(base.skip_builtin_questions),
        final_review_check_command,
        review_memory_path,
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
    fn project_check_command_overrides_global() {
        let global = ExtensionConfig {
            final_review_check_command: Some("cargo build".to_string()),
            ..Default::default()
        };
        let project_config = ExtensionConfig {
            final_review_check_command: Some("./proof.sh".to_string()),
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());
        assert_eq!(
            merged.final_review_check_command.as_deref(),
            Some("./proof.sh")
        );
    }

    #[test]
    fn global_check_command_used_when_project_does_not_set_it() {
        let global = ExtensionConfig {
            final_review_check_command: Some("cargo build".to_string()),
            ..Default::default()
        };
        // Project config present but doesn't set a check command.
        let project_config = ExtensionConfig::default();
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());
        assert_eq!(
            merged.final_review_check_command.as_deref(),
            Some("cargo build")
        );
    }

    #[test]
    fn project_review_memory_path_overrides_global() {
        let global = ExtensionConfig {
            review_memory_path: Some("notes/review.md".to_string()),
            ..Default::default()
        };
        let project_config = ExtensionConfig {
            review_memory_path: Some(".amf/team-review-memory.md".to_string()),
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());
        assert_eq!(
            merged.review_memory_path.as_deref(),
            Some(".amf/team-review-memory.md")
        );
    }

    #[test]
    fn global_review_memory_path_used_when_project_does_not_set_it() {
        let global = ExtensionConfig {
            review_memory_path: Some("notes/review.md".to_string()),
            ..Default::default()
        };
        // Project config present but doesn't set a review-memory path.
        let project_config = ExtensionConfig::default();
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());
        assert_eq!(
            merged.review_memory_path.as_deref(),
            Some("notes/review.md")
        );
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
    fn plan_questions_merge_by_id_and_project_wins() {
        let global = ExtensionConfig {
            plan_questions: vec![
                ConfiguredPlanQuestion {
                    id: "shared".into(),
                    text: "Global wording?".into(),
                    ..Default::default()
                },
                ConfiguredPlanQuestion {
                    id: "global-only".into(),
                    text: "Global only?".into(),
                    ..Default::default()
                },
            ],
            skip_builtin_questions: Some(true),
            ..Default::default()
        };
        let project_config = ExtensionConfig {
            plan_questions: vec![
                ConfiguredPlanQuestion {
                    id: " shared ".into(),
                    text: "Project wording?".into(),
                    ..Default::default()
                },
                ConfiguredPlanQuestion {
                    id: "project-only".into(),
                    text: "Project only?".into(),
                    ..Default::default()
                },
            ],
            // An omitted project flag inherits the global setting.
            skip_builtin_questions: None,
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap();
        write_extension_config(&tmp, &project_config);

        let merged = merge_project_extension_config(&global, tmp.path());

        assert_eq!(merged.plan_questions.len(), 3);
        assert_eq!(merged.plan_questions[0].id, " shared ");
        assert_eq!(merged.plan_questions[0].text, "Project wording?");
        assert_eq!(merged.plan_questions[1].id, "project-only");
        assert_eq!(merged.plan_questions[2].id, "global-only");
        assert_eq!(merged.skip_builtin_questions, Some(true));

        let questions = merged.plan_interview_questions();
        assert_eq!(questions.len(), 3);
        assert_eq!(questions[0].id, "shared");
        assert_eq!(questions[0].source, QuestionSource::Template);
        assert_eq!(questions[2].source, QuestionSource::GlobalTemplate);
    }

    #[test]
    fn global_questions_keep_global_source_without_project_config() {
        let global = ExtensionConfig {
            plan_questions: vec![ConfiguredPlanQuestion {
                id: "audience".into(),
                text: "Who is this for?".into(),
                ..Default::default()
            }],
            skip_builtin_questions: Some(true),
            ..Default::default()
        };
        let tmp = TempDir::new().unwrap();

        let merged = merge_project_extension_config(&global, tmp.path());
        let questions = merged.plan_interview_questions();

        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].source, QuestionSource::GlobalTemplate);
    }

    #[test]
    fn configured_plan_questions_parse_free_text_and_select_options() {
        let raw = r#"{
            "plan_questions": [
                {
                    "id": "audience",
                    "text": "Who is this for?"
                },
                {
                    "id": "delivery",
                    "text": "Where should this ship?",
                    "options": [" Desktop ", "Web"],
                    "optional": false
                }
            ],
            "skip_builtin_questions": true
        }"#;

        let config: ExtensionConfig = serde_json::from_str(raw).unwrap();
        let questions = config.plan_interview_questions();

        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].source, QuestionSource::Template);
        assert_eq!(questions[0].kind, PlanQuestionKind::FreeText);
        assert!(questions[0].optional);
        assert_eq!(
            questions[1].kind,
            PlanQuestionKind::Select(vec!["Desktop".into(), "Web".into()])
        );
        assert!(!questions[1].optional);
    }

    #[test]
    fn configured_question_can_override_a_builtin_in_place() {
        let config = ExtensionConfig {
            plan_questions: vec![ConfiguredPlanQuestion {
                id: "scope".into(),
                text: "What should we deliberately leave out?".into(),
                options: vec!["Nothing".into(), "Decide later".into()],
                optional: false,
                ..Default::default()
            }],
            ..Default::default()
        };

        let questions = config.plan_interview_questions();

        assert_eq!(questions.len(), builtin_questions().len());
        assert_eq!(questions[0].id, "scope");
        assert_eq!(questions[0].source, QuestionSource::Template);
        assert_eq!(
            questions[0].kind,
            PlanQuestionKind::Select(vec!["Nothing".into(), "Decide later".into()])
        );
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
