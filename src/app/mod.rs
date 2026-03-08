mod claude_session_picker;
mod claude_sessions;
mod codex_session_picker;
mod codex_sessions;
pub mod commands;
mod feature_ops;
mod harpoon;
mod hooks;
mod navigation;
mod notifications;
mod opencode;
mod project_ops;
mod rename;
mod review;
mod search;
mod session_config;
mod session_ops;
pub mod setup;
mod state;
mod switcher;
mod sync;
pub mod util;
mod view;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::debug::DebugLog;
use crate::extension::{
    ExtensionConfig, FeaturePreset, load_global_extension_config, merge_project_extension_config,
};
use crate::project::{
    AgentKind, Feature, FeatureSession, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
};
use crate::tmux::TmuxManager;
use crate::traits::{TmuxOps, WorktreeOps};
use crate::usage::UsageManager;
use crate::worktree::WorktreeManager;

pub use self::setup::load_config;
pub use state::*;

pub struct CommandEntry {
    pub name: String,
    pub source: String,
    pub path: PathBuf,
}

pub struct CommandPickerState {
    pub commands: Vec<CommandEntry>,
    pub selected: usize,
    pub from_view: Option<ViewState>,
}

pub struct SwitcherEntry {
    pub tmux_window: String,
    pub kind: SessionKind,
    pub label: String,
    pub icon: Option<String>,
    pub icon_nerd: Option<String>,
}

pub struct SessionSwitcherState {
    pub project_name: String,
    pub feature_name: String,
    pub tmux_session: String,
    pub sessions: Vec<SwitcherEntry>,
    pub selected: usize,
    pub return_window: String,
    pub return_label: String,
    pub vibe_mode: VibeMode,
    pub review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ZaiPlanConfig {
    pub plan: String,
    pub monthly_token_limit: Option<u64>,
    pub weekly_token_limit: Option<u64>,
    pub five_hour_token_limit: Option<u64>,
}

impl Default for ZaiPlanConfig {
    fn default() -> Self {
        Self {
            plan: "free".to_string(),
            monthly_token_limit: None,
            weekly_token_limit: None,
            five_hour_token_limit: None,
        }
    }
}

impl ZaiPlanConfig {
    pub fn get_monthly_limit(&self) -> Option<u64> {
        self.monthly_token_limit.or(match self.plan.as_str() {
            "free" => Some(10_000_000),
            "coding-plan" => Some(500_000_000),
            "unlimited" => None,
            _ => None,
        })
    }

    pub fn get_weekly_limit(&self) -> Option<u64> {
        self.weekly_token_limit.or(match self.plan.as_str() {
            "free" => Some(2_500_000),
            "coding-plan" => Some(125_000_000),
            "unlimited" => None,
            _ => None,
        })
    }

    pub fn get_five_hour_limit(&self) -> Option<u64> {
        self.five_hour_token_limit.or(match self.plan.as_str() {
            "free" => Some(500_000),
            "coding-plan" => Some(25_000_000),
            "unlimited" => None,
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub nerd_font: bool,
    pub leader_timeout_seconds: u64,
    pub zai: Option<ZaiPlanConfig>,
    pub opencode_theme: Option<String>,
    pub projects: ProjectsConfig,
    pub extension: ExtensionConfig,
    pub theme: crate::theme::ThemeName,
    pub transparent_background: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProjectsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_preferred_agent: Option<AgentKind>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            nerd_font: true,
            leader_timeout_seconds: 5,
            zai: None,
            opencode_theme: Some("catppuccin-frappe".to_string()),
            projects: ProjectsConfig::default(),
            extension: ExtensionConfig::default(),
            theme: crate::theme::ThemeName::default(),
            transparent_background: false,
        }
    }
}

pub struct App {
    pub store: ProjectStore,
    pub store_path: PathBuf,
    pub config: AppConfig,
    pub active_extension: ExtensionConfig,
    pub theme: crate::theme::Theme,
    pub selection: Selection,
    pub mode: AppMode,
    pub message: Option<String>,
    pub should_quit: bool,
    pub should_switch: Option<String>,
    pub pane_content: String,
    pub pane_content_cols: u16,
    pub pane_content_rows: u16,
    pub tmux_cursor: Option<(u16, u16)>,
    pub leader_active: bool,
    pub leader_activated_at: Option<Instant>,
    pub pending_inputs: Vec<PendingInput>,
    pub usage: UsageManager,
    pub scroll_offset: usize,
    pub session_filter: SessionFilter,
    pub throbber_state: throbber_widgets_tui::ThrobberState,
    pub thinking_features: std::collections::HashSet<String>,
    pub ipc_thinking_sessions: std::collections::HashSet<String>,
    pub ipc_tool_sessions: std::collections::HashSet<String>,
    pub summary_state: SummaryState,
    pub summary_rx: Option<std::sync::mpsc::Receiver<(String, Result<String, anyhow::Error>)>>,
    pub tmux: Box<dyn TmuxOps>,
    pub worktree: Box<dyn WorktreeOps>,
    pub debug_log: DebugLog,
    pub background_deletions: HashMap<String, BackgroundDeletion>,
    pub background_hooks: HashMap<String, BackgroundHook>,
    pub ipc: Option<crate::ipc::IpcGuard>,
    pub ipc_fallback_logged: bool,
    pub last_file_notification_count: usize,
}

impl App {
    pub fn new(store_path: PathBuf) -> Result<Self> {
        setup::ensure_notify_scripts();
        crate::project::migrate_from_old_path();
        let store = ProjectStore::load(&store_path)?;
        let config = load_config();
        let zai_enabled = config.zai.is_some();
        let zai_monthly = config.zai.as_ref().and_then(|z| z.get_monthly_limit());
        let zai_weekly = config.zai.as_ref().and_then(|z| z.get_weekly_limit());
        let zai_five_hour = config.zai.as_ref().and_then(|z| z.get_five_hour_limit());
        let global_ext = load_global_extension_config();
        let active_extension = store
            .projects
            .first()
            .map(|p| merge_project_extension_config(&global_ext, &p.repo))
            .unwrap_or(global_ext);
        let mut theme = crate::theme::Theme::load(&config.theme);
        theme.set_transparent(config.transparent_background);
        Ok(Self {
            store,
            store_path,
            config,
            active_extension,
            theme,
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            message: None,
            should_quit: false,
            should_switch: None,
            pane_content: String::new(),
            pane_content_cols: 0,
            pane_content_rows: 0,
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            pending_inputs: Vec::new(),
            usage: UsageManager::new(zai_enabled, zai_monthly, zai_weekly, zai_five_hour),
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            thinking_features: std::collections::HashSet::new(),
            ipc_thinking_sessions: std::collections::HashSet::new(),
            ipc_tool_sessions: std::collections::HashSet::new(),
            summary_state: SummaryState::new(),
            summary_rx: None,
            tmux: Box::new(TmuxManager),
            worktree: Box::new(WorktreeManager),
            debug_log: DebugLog::default(),
            background_deletions: HashMap::new(),
            background_hooks: HashMap::new(),
            ipc: None,
            ipc_fallback_logged: false,
            last_file_notification_count: 0,
        })
    }

    pub fn log_startup(&mut self) {
        self.debug_log.info("amf", "AMF started".to_string());
        self.debug_log
            .debug("amf", format!("Store path: {}", self.store_path.display()));
        self.debug_log.debug(
            "amf",
            format!("Projects loaded: {}", self.store.projects.len()),
        );
    }

    /// Lightweight constructor for unit/integration tests.
    ///
    /// Accepts a pre-built `ProjectStore` and injected trait objects so
    /// tests can drive App logic without touching the filesystem or
    /// spawning real tmux sessions.
    #[cfg(test)]
    pub fn new_for_test(
        store: ProjectStore,
        tmux: Box<dyn TmuxOps>,
        worktree: Box<dyn WorktreeOps>,
    ) -> Self {
        use crate::extension::ExtensionConfig;
        Self {
            store,
            store_path: PathBuf::new(),
            config: AppConfig::default(),
            active_extension: ExtensionConfig::default(),
            theme: crate::theme::Theme::default(),
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            message: None,
            should_quit: false,
            should_switch: None,
            pane_content: String::new(),
            pane_content_cols: 0,
            pane_content_rows: 0,
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            pending_inputs: Vec::new(),
            usage: UsageManager::new(false, None, None, None),
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            thinking_features: std::collections::HashSet::new(),
            ipc_thinking_sessions: std::collections::HashSet::new(),
            ipc_tool_sessions: std::collections::HashSet::new(),
            summary_state: SummaryState::new(),
            summary_rx: None,
            tmux,
            worktree,
            debug_log: DebugLog::default(),
            background_deletions: HashMap::new(),
            background_hooks: HashMap::new(),
            ipc: None,
            ipc_fallback_logged: false,
            last_file_notification_count: 0,
        }
    }

    /// Re-merge extension config for the currently selected
    /// project/feature. Call this whenever the selection changes.
    pub fn reload_extension_config(&mut self) {
        let global_ext = load_global_extension_config();
        self.active_extension = match &self.selection {
            Selection::Project(pi) => {
                if let Some(project) = self.store.projects.get(*pi) {
                    merge_project_extension_config(&global_ext, &project.repo)
                } else {
                    global_ext
                }
            }
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => {
                if let Some(project) = self.store.projects.get(*pi) {
                    if project.features.get(*fi).is_some() {
                        // Extension config is project-scoped and lives under
                        // `{repo}/.amf/config.json`, so worktree selections
                        // should still reload from the project repo.
                        merge_project_extension_config(&global_ext, &project.repo)
                    } else {
                        merge_project_extension_config(&global_ext, &project.repo)
                    }
                } else {
                    global_ext
                }
            }
        };
    }

    pub(crate) fn extension_for_repo(&self, repo: &Path) -> ExtensionConfig {
        let global_ext = load_global_extension_config();
        merge_project_extension_config(&global_ext, repo)
    }

    pub(crate) fn allowed_agents_for_repo(&self, repo: &Path) -> Vec<AgentKind> {
        self.extension_for_repo(repo).allowed_agents()
    }

    pub(crate) fn repo_for_project_path(&self, path: &Path) -> PathBuf {
        self.worktree
            .repo_root(path)
            .unwrap_or_else(|_| path.to_path_buf())
    }

    pub(crate) fn allowed_agents_for_project_path(&self, path: &Path) -> Vec<AgentKind> {
        let repo = self.repo_for_project_path(path);
        self.allowed_agents_for_repo(&repo)
    }

    pub(crate) fn allows_agent_for_repo(&self, repo: &Path, agent: &AgentKind) -> bool {
        self.extension_for_repo(repo).allows_agent(agent)
    }

    pub(crate) fn allowed_feature_presets_for_repo(&self, repo: &Path) -> Vec<FeaturePreset> {
        self.extension_for_repo(repo).allowed_feature_presets()
    }

    pub(crate) fn normalize_agent_for_repo(
        &self,
        repo: &Path,
        preferred: &AgentKind,
    ) -> (AgentKind, usize) {
        let allowed = self.allowed_agents_for_repo(repo);
        let selected = allowed
            .iter()
            .find(|agent| *agent == preferred)
            .cloned()
            .unwrap_or_else(|| allowed[0].clone());
        let index = AgentKind::index_in(&allowed, &selected);
        (selected, index)
    }

    pub(crate) fn normalize_agent_for_project_path(
        &self,
        path: &Path,
        preferred: &AgentKind,
    ) -> (AgentKind, usize) {
        let repo = self.repo_for_project_path(path);
        self.normalize_agent_for_repo(&repo, preferred)
    }

    pub(crate) fn refresh_create_project_agent_selection(&mut self) {
        let (path, preferred) = match &self.mode {
            AppMode::CreatingProject(state) => (PathBuf::from(&state.path), state.agent.clone()),
            _ => return,
        };
        let (agent, agent_index) = self.normalize_agent_for_project_path(&path, &preferred);
        if let AppMode::CreatingProject(state) = &mut self.mode {
            state.agent = agent;
            state.agent_index = agent_index;
        }
    }

    pub(crate) fn default_project_preferred_agent(&self) -> AgentKind {
        self.config
            .projects
            .default_preferred_agent
            .clone()
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        self.store.save(&self.store_path)
    }

    pub fn start_theme_picker(&mut self) {
        let themes = crate::theme::Theme::list();
        let selected = themes
            .iter()
            .position(|t| *t == self.config.theme)
            .unwrap_or(0);
        self.mode = AppMode::ThemePicker(ThemePickerState { selected, themes });
    }

    pub fn apply_theme(&mut self, theme_name: crate::theme::ThemeName) {
        self.config.theme = theme_name;
        let mut theme = crate::theme::Theme::load(&self.config.theme);
        theme.set_transparent(self.config.transparent_background);
        self.theme = theme;
        self.mode = AppMode::Normal;

        let config_path = crate::project::amf_config_dir().join("config.json");
        let dir = config_path.parent().unwrap();
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&self.config).unwrap_or_default(),
        );
    }

    pub fn log_debug(&mut self, context: &str, message: String) {
        self.debug_log.debug(context, message);
    }

    pub fn log_info(&mut self, context: &str, message: String) {
        self.debug_log.info(context, message);
    }

    pub fn log_warn(&mut self, context: &str, message: String) {
        self.debug_log.warn(context, message);
    }

    pub fn log_error(&mut self, context: &str, message: String) {
        self.debug_log.error(context, message);
    }

    pub fn report_logged_error(&mut self, context: &str, detail: impl Into<String>) {
        let detail = detail.into();
        self.log_error(context, detail.clone());
        self.set_debug_log_error_message(detail);
    }

    pub fn set_debug_log_error_message(&mut self, message: impl Into<String>) {
        self.message = Some(format!(
            "Error: {} Check debug log for details.",
            message.into()
        ));
    }
}
