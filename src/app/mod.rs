mod automation;
mod claude_session_picker;
mod claude_sessions;
mod codex_live;
mod codex_session_picker;
mod codex_sessions;
pub mod commands;
mod diff;
mod feature_ops;
mod harpoon;
mod hooks;
mod navigation;
mod notifications;
mod opencode;
pub(crate) mod opencode_storage;
mod project_ops;
mod rename;
mod review;
mod search;
mod session_config;
mod session_ops;
pub mod setup;
mod state;
mod steering;
mod switcher;
mod sync;
mod syntax;
pub mod util;
mod view;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use crate::debug::DebugLog;
use crate::extension::{
    ExtensionConfig, FeaturePreset, load_global_extension_config, merge_project_extension_config,
};
use crate::project::{
    AgentKind, Feature, FeatureSession, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
};
use crate::tmux::TmuxManager;
use crate::token_tracking::{SessionTokenTracker, TokenPricingConfig};
use crate::traits::{TmuxOps, WorktreeOps};
use crate::usage::UsageManager;
use crate::worktree::WorktreeManager;

pub use self::setup::load_config;
pub use codex_live::CodexLiveThreadState;
pub use codex_sessions::sidebar_metadata_for_session_id as codex_sidebar_metadata_for_session_id;
pub use state::*;
pub use steering::{PromptAnalysis, analyze_prompt};

#[derive(Debug, Clone)]
pub struct CodexSidebarMetadataResult {
    pub cache_key: String,
    pub title: Option<String>,
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexDebugCommand {
    PlanDemo,
    WorkChangeReasonDemo,
    WorkDiffReviewDemo,
    WorkCommandDemo,
    WorkFileDemo,
    WorkInputDemo,
    ClearInputDemo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCommand {
    OpenDebugLog,
    RefreshNotifications,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    SlashCommand,
    Local { command: LocalCommand },
    CodexLiveDemo(CodexDebugCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    pub name: String,
    pub source: String,
    pub path: Option<PathBuf>,
    pub action: CommandAction,
}

pub struct CommandPickerState {
    pub commands: Vec<CommandEntry>,
    pub selected: usize,
    pub from_view: Option<ViewState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPickerFocus {
    Default,
    Local,
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
    pub diff_review_viewer: DiffReviewViewer,
    pub diff_viewer_layout: DiffViewerLayout,
    pub zai: Option<ZaiPlanConfig>,
    pub opencode_theme: Option<String>,
    pub projects: ProjectsConfig,
    pub extension: ExtensionConfig,
    pub theme: crate::theme::ThemeName,
    pub transparent_background: bool,
    #[serde(default)]
    pub token_pricing: TokenPricingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum DiffReviewViewer {
    #[default]
    #[serde(rename = "amf", alias = "custom")]
    Amf,
    #[serde(rename = "nvim", alias = "legacy")]
    Nvim,
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
            diff_review_viewer: DiffReviewViewer::default(),
            diff_viewer_layout: DiffViewerLayout::Unified,
            zai: None,
            opencode_theme: Some("catppuccin-frappe".to_string()),
            projects: ProjectsConfig::default(),
            extension: ExtensionConfig::default(),
            theme: crate::theme::ThemeName::default(),
            transparent_background: false,
            token_pricing: TokenPricingConfig::default(),
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
    pub viewport_cols: u16,
    pub viewport_rows: u16,
    pub tmux_cursor: Option<(u16, u16)>,
    pub leader_active: bool,
    pub leader_activated_at: Option<Instant>,
    pub pending_inputs: Vec<PendingInput>,
    pub latest_prompt_cache: HashMap<String, String>,
    pub sidebar_plan_cache: HashMap<String, String>,
    pub codex_session_title_cache: HashMap<String, Option<String>>,
    pub codex_session_prompt_cache: HashMap<String, Option<String>>,
    pub codex_live_threads: HashMap<String, CodexLiveThreadState>,
    pub codex_sidebar_metadata_tx: std::sync::mpsc::Sender<CodexSidebarMetadataResult>,
    pub codex_sidebar_metadata_rx: std::sync::mpsc::Receiver<CodexSidebarMetadataResult>,
    pub codex_sidebar_metadata_inflight: std::collections::HashSet<String>,
    pub opencode_sidebar_cache: HashMap<String, opencode_storage::OpencodeSidebarData>,
    sidebar_load_tx: Sender<SidebarLoadResult>,
    sidebar_load_rx: Receiver<SidebarLoadResult>,
    sidebar_load_signatures: HashMap<String, u64>,
    pending_sidebar_loads: std::collections::HashSet<String>,
    pub usage: UsageManager,
    pub token_tracker: SessionTokenTracker,
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
    pub last_file_notification_fingerprint: Option<u64>,
    pub vscode_available: bool,
}

struct SidebarLoadResult {
    tmux_session: String,
    signature: u64,
    latest_prompt: Option<String>,
    opencode_sidebar: Option<opencode_storage::OpencodeSidebarData>,
}

impl App {
    pub fn new(store_path: PathBuf) -> Result<Self> {
        setup::ensure_notify_scripts();
        crate::project::migrate_from_old_path();
        let store = ProjectStore::load(&store_path)?;
        let (sidebar_load_tx, sidebar_load_rx) = std::sync::mpsc::channel();
        let latest_prompt_cache = Self::build_latest_prompt_cache(&store);
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
        let sidebar_plan_cache = Self::build_sidebar_plan_cache(&store);
        let (codex_sidebar_metadata_tx, codex_sidebar_metadata_rx) = std::sync::mpsc::channel();
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
            viewport_cols: 0,
            viewport_rows: 0,
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            pending_inputs: Vec::new(),
            latest_prompt_cache,
            sidebar_plan_cache,
            codex_session_title_cache: HashMap::new(),
            codex_session_prompt_cache: HashMap::new(),
            codex_live_threads: HashMap::new(),
            codex_sidebar_metadata_tx,
            codex_sidebar_metadata_rx,
            codex_sidebar_metadata_inflight: std::collections::HashSet::new(),
            opencode_sidebar_cache: HashMap::new(),
            sidebar_load_tx,
            sidebar_load_rx,
            sidebar_load_signatures: HashMap::new(),
            pending_sidebar_loads: std::collections::HashSet::new(),
            usage: UsageManager::new(zai_enabled, zai_monthly, zai_weekly, zai_five_hour),
            token_tracker: SessionTokenTracker::default(),
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
            last_file_notification_fingerprint: None,
            vscode_available: std::process::Command::new("code")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok(),
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
        let (sidebar_load_tx, sidebar_load_rx) = std::sync::mpsc::channel();
        let latest_prompt_cache = Self::build_latest_prompt_cache(&store);
        let sidebar_plan_cache = Self::build_sidebar_plan_cache(&store);
        let (codex_sidebar_metadata_tx, codex_sidebar_metadata_rx) = std::sync::mpsc::channel();
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
            viewport_cols: 0,
            viewport_rows: 0,
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            pending_inputs: Vec::new(),
            latest_prompt_cache,
            sidebar_plan_cache,
            codex_session_title_cache: HashMap::new(),
            codex_session_prompt_cache: HashMap::new(),
            codex_live_threads: HashMap::new(),
            codex_sidebar_metadata_tx,
            codex_sidebar_metadata_rx,
            codex_sidebar_metadata_inflight: std::collections::HashSet::new(),
            opencode_sidebar_cache: HashMap::new(),
            sidebar_load_tx,
            sidebar_load_rx,
            sidebar_load_signatures: HashMap::new(),
            pending_sidebar_loads: std::collections::HashSet::new(),
            usage: UsageManager::new(false, None, None, None),
            token_tracker: SessionTokenTracker::default(),
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
            last_file_notification_fingerprint: None,
            vscode_available: false,
        }
    }

    pub(crate) fn has_active_sidebar(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::Viewing(view) if view.sidebar_session_kind().is_some()
        )
    }

    pub(crate) fn refresh_sidebar_for_current_view(&mut self) {
        let Some((project_name, feature_name, window, session_kind)) = (match &self.mode {
            AppMode::Viewing(view) => view.sidebar_session_kind().map(|session_kind| {
                (
                    view.project_name.clone(),
                    view.feature_name.clone(),
                    view.window.clone(),
                    session_kind,
                )
            }),
            _ => None,
        }) else {
            return;
        };

        let Some((pi, fi)) = self
            .store
            .projects
            .iter()
            .enumerate()
            .find(|(_, project)| project.name == project_name)
            .and_then(|(pi, project)| {
                project
                    .features
                    .iter()
                    .enumerate()
                    .find(|(_, feature)| feature.name == feature_name)
                    .map(|(fi, _)| (pi, fi))
            })
        else {
            return;
        };

        self.refresh_latest_prompt_for_feature(pi, fi);
        self.refresh_sidebar_plan_for_feature(pi, fi);
        self.request_codex_sidebar_metadata_for_view(
            &project_name,
            &feature_name,
            &window,
            &session_kind,
        );
        self.schedule_sidebar_load_for_feature(pi, fi);
    }

    fn build_latest_prompt_cache(store: &ProjectStore) -> HashMap<String, String> {
        let mut cache = HashMap::new();

        for project in &store.projects {
            for feature in &project.features {
                if let Some(prompt) = latest_prompt_text_for_feature(feature)
                    .map(|prompt| prompt.trim().to_string())
                    .filter(|prompt| !prompt.is_empty())
                {
                    cache.insert(feature.tmux_session.clone(), prompt);
                }
            }
        }

        cache
    }

    fn build_sidebar_plan_cache(store: &ProjectStore) -> HashMap<String, String> {
        let mut cache = HashMap::new();

        for project in &store.projects {
            for feature in &project.features {
                if let Some(plan) =
                    crate::markdown::read_plan_preview(&feature.workdir, Some(&project.repo))
                {
                    cache.insert(feature.tmux_session.clone(), plan);
                }
            }
        }

        cache
    }

    pub(crate) fn schedule_sidebar_load_for_feature(&mut self, pi: usize, fi: usize) {
        let Some(feature) = self
            .store
            .projects
            .get(pi)
            .and_then(|project| project.features.get(fi))
        else {
            return;
        };

        let request = SidebarLoadRequest::from_feature(feature);
        let signature = request.signature();

        if self
            .sidebar_load_signatures
            .get(&request.tmux_session)
            .is_some_and(|cached| *cached == signature)
        {
            return;
        }

        if !self
            .pending_sidebar_loads
            .insert(request.tmux_session.clone())
        {
            return;
        }

        let tx = self.sidebar_load_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(request.load(signature));
        });
    }

    pub(crate) fn schedule_sidebar_loads_for_all_features(&mut self) {
        let mut targets = Vec::new();
        for (pi, project) in self.store.projects.iter().enumerate() {
            for (fi, _feature) in project.features.iter().enumerate() {
                targets.push((pi, fi));
            }
        }
        for (pi, fi) in targets {
            self.schedule_sidebar_load_for_feature(pi, fi);
        }
    }

    pub(crate) fn poll_sidebar_load_results(&mut self) {
        while let Ok(result) = self.sidebar_load_rx.try_recv() {
            self.pending_sidebar_loads.remove(&result.tmux_session);
            self.sidebar_load_signatures
                .insert(result.tmux_session.clone(), result.signature);

            if let Some(prompt) = result.latest_prompt {
                let prompt = prompt.trim().to_string();
                if prompt.is_empty() {
                    self.latest_prompt_cache.remove(&result.tmux_session);
                } else {
                    self.latest_prompt_cache
                        .insert(result.tmux_session.clone(), prompt);
                }
            } else {
                self.latest_prompt_cache.remove(&result.tmux_session);
            }

            if let Some(data) = result.opencode_sidebar {
                self.opencode_sidebar_cache
                    .insert(result.tmux_session, data);
            } else {
                self.opencode_sidebar_cache.remove(&result.tmux_session);
            }
        }
    }

    pub(crate) fn clear_sidebar_state_for_session(&mut self, tmux_session: &str) {
        self.pending_sidebar_loads.remove(tmux_session);
        self.sidebar_load_signatures.remove(tmux_session);
        self.latest_prompt_cache.remove(tmux_session);
        self.sidebar_plan_cache.remove(tmux_session);
        self.codex_live_threads.remove(tmux_session);
        self.opencode_sidebar_cache.remove(tmux_session);
    }

    pub(crate) fn refresh_latest_prompt_for_feature(&mut self, pi: usize, fi: usize) {
        let Some(feature) = self
            .store
            .projects
            .get(pi)
            .and_then(|project| project.features.get(fi))
        else {
            return;
        };

        if let Some(prompt) = latest_prompt_text_for_feature(feature)
            .map(|prompt| prompt.trim().to_string())
            .filter(|prompt| !prompt.is_empty())
        {
            self.latest_prompt_cache
                .insert(feature.tmux_session.clone(), prompt);
        } else {
            self.latest_prompt_cache.remove(&feature.tmux_session);
        }
    }

    pub(crate) fn refresh_sidebar_plan_for_feature(&mut self, pi: usize, fi: usize) {
        let Some((project, feature)) = self
            .store
            .projects
            .get(pi)
            .and_then(|project| project.features.get(fi).map(|feature| (project, feature)))
        else {
            return;
        };

        if let Some(plan) =
            crate::markdown::read_plan_preview(&feature.workdir, Some(&project.repo))
        {
            self.sidebar_plan_cache
                .insert(feature.tmux_session.clone(), plan);
        } else {
            self.sidebar_plan_cache.remove(&feature.tmux_session);
        }
    }

    pub fn latest_prompt_for_session(&self, tmux_session: &str) -> Option<&str> {
        self.latest_prompt_cache
            .get(tmux_session)
            .map(String::as_str)
    }

    pub fn sidebar_plan_for_session(&self, tmux_session: &str) -> Option<&str> {
        self.sidebar_plan_cache
            .get(tmux_session)
            .map(String::as_str)
    }

    fn codex_sidebar_cache_key(workdir: &Path, session_id: &str) -> String {
        format!("{}::{session_id}", workdir.display())
    }

    pub(crate) fn request_codex_sidebar_metadata_for_session(
        &mut self,
        workdir: &Path,
        session_id: &str,
    ) {
        let cache_key = Self::codex_sidebar_cache_key(workdir, session_id);
        if self.codex_sidebar_metadata_inflight.contains(&cache_key)
            || (self.codex_session_title_cache.contains_key(&cache_key)
                && self.codex_session_prompt_cache.contains_key(&cache_key))
        {
            return;
        }

        self.codex_sidebar_metadata_inflight
            .insert(cache_key.clone());
        let tx = self.codex_sidebar_metadata_tx.clone();
        let workdir = workdir.to_path_buf();
        let session_id = session_id.to_string();
        std::thread::spawn(move || {
            let metadata = crate::app::codex_sidebar_metadata_for_session_id(&workdir, &session_id)
                .ok()
                .flatten();
            let result = CodexSidebarMetadataResult {
                cache_key,
                title: metadata
                    .as_ref()
                    .and_then(|metadata| metadata.title.clone()),
                prompt: metadata.and_then(|metadata| metadata.latest_prompt),
            };
            let _ = tx.send(result);
        });
    }

    pub(crate) fn request_codex_sidebar_metadata_for_view(
        &mut self,
        project_name: &str,
        feature_name: &str,
        window: &str,
        session_kind: &SessionKind,
    ) {
        if *session_kind != SessionKind::Codex {
            return;
        }

        let context = self
            .store
            .projects
            .iter()
            .find(|project| project.name == project_name)
            .and_then(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.name == feature_name)
            })
            .and_then(|feature| {
                feature
                    .sessions
                    .iter()
                    .find(|session| session.tmux_window == window)
                    .and_then(|session| {
                        session
                            .token_usage_source
                            .as_ref()
                            .filter(|source| {
                                source.provider == crate::token_tracking::TokenUsageProvider::Codex
                            })
                            .map(|source| (feature.workdir.clone(), source.id.clone()))
                    })
            });

        let Some((workdir, session_id)) = context else {
            return;
        };

        self.request_codex_sidebar_metadata_for_session(&workdir, &session_id);
    }

    pub fn cached_codex_session_title(&self, workdir: &Path, session_id: &str) -> Option<&str> {
        let cache_key = Self::codex_sidebar_cache_key(workdir, session_id);
        self.codex_session_title_cache
            .get(&cache_key)
            .and_then(|title| title.as_deref())
    }

    pub fn cached_codex_session_prompt(&self, workdir: &Path, session_id: &str) -> Option<&str> {
        let cache_key = Self::codex_sidebar_cache_key(workdir, session_id);
        self.codex_session_prompt_cache
            .get(&cache_key)
            .and_then(|prompt| prompt.as_deref())
    }

    pub fn codex_live_thread(&self, tmux_session: &str) -> Option<&CodexLiveThreadState> {
        self.codex_live_threads.get(tmux_session)
    }

    pub fn apply_codex_live_event(&mut self, tmux_session: &str, raw: &serde_json::Value) -> bool {
        let state = self
            .codex_live_threads
            .entry(tmux_session.to_string())
            .or_default();
        state.apply_event(raw)
    }

    pub(crate) fn viewport_size(&self) -> Option<(u16, u16)> {
        match (self.viewport_cols, self.viewport_rows) {
            (0, _) | (_, 0) => None,
            dims => Some(dims),
        }
    }

    pub(crate) fn resize_session_windows_for_viewport(
        tmux: &dyn TmuxOps,
        viewport: Option<(u16, u16)>,
        tmux_session: &str,
        windows: &[String],
    ) -> Result<()> {
        let Some((cols, rows)) = viewport else {
            return Ok(());
        };

        for window in windows {
            tmux.resize_pane(tmux_session, window, cols, rows)?;
        }

        Ok(())
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

    pub(crate) fn use_custom_diff_review_viewer(&self) -> bool {
        matches!(self.config.diff_review_viewer, DiffReviewViewer::Amf)
    }

    pub(crate) fn ensure_agent_mode_supported(
        &self,
        agent: &AgentKind,
        mode: &VibeMode,
    ) -> Result<()> {
        if matches!(agent, AgentKind::Codex) && matches!(mode, VibeMode::Vibeless) {
            anyhow::bail!(
                "Codex does not support Vibeless diff review. Use Claude/Opencode, or switch to Vibe or SuperVibe."
            );
        }
        Ok(())
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
        let original_theme = self.config.theme;
        self.mode = AppMode::ThemePicker(ThemePickerState {
            selected,
            themes,
            original_theme,
        });
    }

    pub fn preview_theme(&mut self, theme_name: crate::theme::ThemeName) {
        let mut theme = crate::theme::Theme::load(&theme_name);
        theme.set_transparent(self.config.transparent_background);
        self.theme = theme;
    }

    pub fn apply_theme(&mut self, theme_name: crate::theme::ThemeName) {
        self.config.theme = theme_name;
        let mut theme = crate::theme::Theme::load(&self.config.theme);
        theme.set_transparent(self.config.transparent_background);
        self.theme = theme;
        self.mode = AppMode::Normal;
        self.save_config();
    }

    pub fn toggle_transparent_background(&mut self) {
        self.config.transparent_background = !self.config.transparent_background;
        self.theme
            .set_transparent(self.config.transparent_background);
        self.save_config();
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

    pub fn save_config(&self) {
        if self.store_path.as_os_str().is_empty() {
            return;
        }
        let config_path = crate::project::amf_config_dir().join("config.json");
        let dir = config_path.parent().unwrap();
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&self.config).unwrap_or_default(),
        );
    }
}

fn latest_prompt_text_for_feature(feature: &Feature) -> Option<String> {
    let preferred_session_kind = match feature.agent {
        AgentKind::Claude => Some(SessionKind::Claude),
        AgentKind::Opencode => Some(SessionKind::Opencode),
        AgentKind::Codex => Some(SessionKind::Codex),
    };

    let preferred_opencode_session_id = if feature.agent == AgentKind::Opencode {
        feature
            .sessions
            .iter()
            .find(|session| session.kind == SessionKind::Opencode)
            .and_then(|session| session.token_usage_source.as_ref())
            .filter(|source| source.provider == crate::token_tracking::TokenUsageProvider::Opencode)
            .map(|source| source.id.as_str())
    } else {
        None
    };

    crate::app::util::read_latest_prompt_for_session(
        &feature.workdir,
        preferred_session_kind.as_ref(),
        preferred_opencode_session_id,
    )
}

struct SidebarLoadRequest {
    tmux_session: String,
    workdir: PathBuf,
    preferred_session_kind: Option<SessionKind>,
    preferred_session_id: Option<String>,
}

impl SidebarLoadRequest {
    fn from_feature(feature: &Feature) -> Self {
        let preferred_session_kind = match feature.agent {
            AgentKind::Claude => Some(SessionKind::Claude),
            AgentKind::Opencode => Some(SessionKind::Opencode),
            AgentKind::Codex => Some(SessionKind::Codex),
        };
        let preferred_session_id = match feature.agent {
            AgentKind::Claude => feature
                .sessions
                .iter()
                .find(|session| session.kind == SessionKind::Claude)
                .and_then(|session| session.claude_session_id.clone()),
            AgentKind::Opencode => feature
                .sessions
                .iter()
                .find(|session| session.kind == SessionKind::Opencode)
                .and_then(|session| session.token_usage_source.as_ref())
                .filter(|source| {
                    source.provider == crate::token_tracking::TokenUsageProvider::Opencode
                })
                .map(|source| source.id.clone()),
            AgentKind::Codex => feature
                .sessions
                .iter()
                .find(|session| session.kind == SessionKind::Codex)
                .and_then(|session| session.token_usage_source.as_ref())
                .filter(|source| {
                    source.provider == crate::token_tracking::TokenUsageProvider::Codex
                })
                .map(|source| source.id.clone()),
        };

        Self {
            tmux_session: feature.tmux_session.clone(),
            workdir: feature.workdir.clone(),
            preferred_session_kind,
            preferred_session_id,
        }
    }

    fn signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.workdir.hash(&mut hasher);
        self.preferred_session_id.hash(&mut hasher);
        session_kind_signature(self.preferred_session_kind.as_ref()).hash(&mut hasher);
        sidebar_prompt_input_signature(&self.workdir).hash(&mut hasher);

        if self.preferred_session_kind == Some(SessionKind::Opencode) {
            opencode_storage::sidebar_input_signature(
                &self.workdir,
                self.preferred_session_id.as_deref(),
            )
            .hash(&mut hasher);
        }

        hasher.finish()
    }

    fn load(self, signature: u64) -> SidebarLoadResult {
        let latest_prompt = crate::app::util::read_latest_prompt_for_session(
            &self.workdir,
            self.preferred_session_kind.as_ref(),
            self.preferred_session_id.as_deref(),
        );
        let opencode_sidebar = if self.preferred_session_kind == Some(SessionKind::Opencode) {
            opencode_storage::read_sidebar_data(&self.workdir, self.preferred_session_id.as_deref())
        } else {
            None
        };

        SidebarLoadResult {
            tmux_session: self.tmux_session,
            signature,
            latest_prompt,
            opencode_sidebar,
        }
    }
}

fn session_kind_signature(session_kind: Option<&SessionKind>) -> u8 {
    match session_kind {
        Some(SessionKind::Claude) => 1,
        Some(SessionKind::Opencode) => 2,
        Some(SessionKind::Codex) => 3,
        Some(SessionKind::Terminal) => 4,
        Some(SessionKind::Nvim) => 5,
        Some(SessionKind::Vscode) => 6,
        Some(SessionKind::Custom) => 7,
        None => 0,
    }
}

fn sidebar_prompt_input_signature(workdir: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_path_metadata(&mut hasher, crate::app::util::latest_prompt_path(workdir));
    hash_path_metadata(&mut hasher, workdir.join(".codex").join("latest-prompt.txt"));
    hasher.finish()
}

fn hash_path_metadata(hasher: &mut impl Hasher, path: PathBuf) {
    path.hash(hasher);
    match std::fs::metadata(&path) {
        Ok(metadata) => {
            true.hash(hasher);
            metadata.len().hash(hasher);
            metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .hash(hasher);
        }
        Err(_) => false.hash(hasher),
    }
}
