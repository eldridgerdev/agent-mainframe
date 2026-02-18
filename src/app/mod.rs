mod state;

use anyhow::Result;
use ratatui_explorer::FileExplorer;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::project::{
    AgentKind, Feature, FeatureSession, Project, ProjectStatus,
    ProjectStore, SessionKind, VibeMode,
};
use crate::tmux::TmuxManager;
use crate::usage::UsageManager;
use crate::worktree::WorktreeManager;

pub use state::*;

fn shorten_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

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
        self.monthly_token_limit.or_else(|| match self.plan.as_str() {
            "free" => Some(10_000_000),
            "coding-plan" => Some(500_000_000),
            "unlimited" => None,
            _ => None,
        })
    }

    pub fn get_weekly_limit(&self) -> Option<u64> {
        self.weekly_token_limit.or_else(|| match self.plan.as_str() {
            "free" => Some(2_500_000),
            "coding-plan" => Some(125_000_000),
            "unlimited" => None,
            _ => None,
        })
    }

    pub fn get_five_hour_limit(&self) -> Option<u64> {
        self.five_hour_token_limit.or_else(|| match self.plan.as_str() {
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
    pub zai: ZaiPlanConfig,
    pub opencode_theme: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            nerd_font: true,
            zai: ZaiPlanConfig::default(),
            opencode_theme: Some("catppuccin-frappe".to_string()),
        }
    }
}

pub fn load_config() -> AppConfig {
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("amf")
        .join("config.json");

    let config = if config_path.exists() {
        std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        let config = AppConfig::default();
        let dir = config_path.parent().unwrap();
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&config)
                .unwrap_or_default(),
        );
        config
    };

    if let Some(ref theme) = config.opencode_theme {
        let _ = update_opencode_theme(theme);
    }

    config
}

fn update_opencode_theme(theme: &str) -> anyhow::Result<()> {
    let opencode_config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("opencode")
        .join("opencode.json");

    let mut config: serde_json::Value = if opencode_config_path.exists() {
        std::fs::read_to_string(&opencode_config_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if let Some(obj) = config.as_object_mut() {
        obj.insert("theme".to_string(), serde_json::json!(theme));
    }

    if let Some(parent) = opencode_config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    std::fs::write(
        &opencode_config_path,
        serde_json::to_string_pretty(&config)?,
    )?;

    Ok(())
}

fn remove_old_diff_review_plugin(repo: &Path) {
    let settings_path =
        repo.join(".claude").join("settings.local.json");
    if !settings_path.exists() {
        return;
    }

    let mut settings: serde_json::Value =
        match std::fs::read_to_string(&settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(v) => v,
            None => return,
        };

    let changed = settings
        .get_mut("enabledPlugins")
        .and_then(|p| p.as_object_mut())
        .map(|obj| {
            obj.remove("diff-review@claude_vibeless")
                .is_some()
        })
        .unwrap_or(false);

    if !changed {
        return;
    }

    if settings
        .get("enabledPlugins")
        .and_then(|p| p.as_object())
        .is_some_and(|obj| obj.is_empty())
    {
        settings
            .as_object_mut()
            .unwrap()
            .remove("enabledPlugins");
    }

    if settings.as_object().is_some_and(|obj| obj.is_empty())
    {
        let _ = std::fs::remove_file(&settings_path);
    } else {
        let _ = std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&settings)
                .unwrap_or_default()
                + "\n",
        );
    }
}

fn ensure_opencode_plugins(
    workdir: &Path,
    repo: &Path,
    mode: &VibeMode,
) {
    let plugins_dir = workdir.join(".opencode").join("plugins");
    let _ = std::fs::create_dir_all(&plugins_dir);

    let src_input_request = repo
        .join(".opencode")
        .join("plugins")
        .join("input-request.js");
    let dst_input_request = plugins_dir.join("input-request.js");

    if src_input_request.exists() {
        let _ = std::fs::copy(&src_input_request, &dst_input_request);
    }

    let dst_diff_review = plugins_dir.join("diff-review.ts");
    let dst_diff_review_sh = plugins_dir.join("diff-review.sh");
    let dst_feedback_prompt = plugins_dir.join("feedback-prompt.sh");
    let dst_explain = plugins_dir.join("explain.sh");
    let _ = std::fs::remove_file(&dst_diff_review);
    let _ = std::fs::remove_file(&dst_diff_review_sh);
    let _ = std::fs::remove_file(&dst_feedback_prompt);
    let _ = std::fs::remove_file(&dst_explain);

    if matches!(mode, VibeMode::Vibeless) {
        let src_diff_review = repo
            .join(".opencode")
            .join("plugins")
            .join("diff-review.ts");
        let src_diff_review_sh = repo
            .join(".opencode")
            .join("plugins")
            .join("diff-review.sh");

        if src_diff_review.exists() {
            let _ = std::fs::copy(&src_diff_review, &dst_diff_review);
        }

        if src_diff_review_sh.exists() {
            let _ = std::fs::copy(&src_diff_review_sh, &dst_diff_review_sh);
        }

        let src_feedback_prompt = repo
            .join(".opencode")
            .join("plugins")
            .join("feedback-prompt.sh");

        if src_feedback_prompt.exists() {
            let _ = std::fs::copy(&src_feedback_prompt, &dst_feedback_prompt);
        }

        let src_explain = repo
            .join(".opencode")
            .join("plugins")
            .join("explain.sh");

        if src_explain.exists() {
            let _ = std::fs::copy(&src_explain, &dst_explain);
        }
    }
}

pub fn ensure_notification_hooks(
    workdir: &Path,
    repo: &Path,
    mode: &VibeMode,
    agent: &AgentKind,
) {
    remove_old_diff_review_plugin(repo);

    if matches!(agent, AgentKind::Opencode) {
        ensure_opencode_plugins(workdir, repo, mode);
        return;
    }

    let config_subdir = match agent {
        AgentKind::Claude => ".claude",
        AgentKind::Opencode => ".opencode",
    };
    let claude_dir = workdir.join(config_subdir);
    let settings_path = claude_dir.join("settings.json");

    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude-super-vibeless");

    let notify_cmd =
        config_dir.join("notify.sh").to_string_lossy().to_string();
    let clear_cmd = config_dir
        .join("clear-notify.sh")
        .to_string_lossy()
        .to_string();
    let diff_review_cmd = repo
        .join("plugins")
        .join("diff-review")
        .join("scripts")
        .join("diff-review.sh")
        .to_string_lossy()
        .to_string();

    let wants_diff_review =
        matches!(mode, VibeMode::Vibeless);

    let mut settings: serde_json::Value =
        if settings_path.exists() {
            std::fs::read_to_string(&settings_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

    let has_notification = settings
        .get("hooks")
        .is_some_and(|h| h.get("Notification").is_some());
    let has_stop = settings
        .get("hooks")
        .is_some_and(|h| h.get("Stop").is_some());
    let has_pre_tool_use = settings
        .get("hooks")
        .is_some_and(|h| h.get("PreToolUse").is_some());
    let has_perms = settings
        .get("permissions")
        .and_then(|p| p.get("allow"))
        .and_then(|a| a.as_array())
        .is_some_and(|arr| {
            arr.iter().any(|v| v.as_str() == Some("Edit"))
                && arr
                    .iter()
                    .any(|v| v.as_str() == Some("Write"))
        });

    let already_correct = has_notification
        && has_stop
        && if wants_diff_review {
            has_pre_tool_use && has_perms
        } else {
            !has_pre_tool_use && !has_perms
        };
    if already_correct {
        return;
    }

    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let notification_hook = serde_json::json!([{
        "matcher": "",
        "hooks": [{ "type": "command", "command": notify_cmd }]
    }]);
    let stop_hook = serde_json::json!([{
        "matcher": "",
        "hooks": [{ "type": "command", "command": clear_cmd }]
    }]);

    let hooks_obj = hooks.as_object_mut().unwrap();
    hooks_obj
        .entry("Notification")
        .or_insert(notification_hook);
    hooks_obj.entry("Stop").or_insert(stop_hook);

    if wants_diff_review {
        let pre_tool_use_hook = serde_json::json!([{
            "matcher": "Edit|Write",
            "hooks": [{
                "type": "command",
                "command": diff_review_cmd,
                "timeout": 600
            }]
        }]);
        hooks_obj
            .entry("PreToolUse")
            .or_insert(pre_tool_use_hook);
    } else {
        hooks_obj.remove("PreToolUse");
    }

    if wants_diff_review {
        let perms = settings
            .as_object_mut()
            .unwrap()
            .entry("permissions")
            .or_insert_with(|| serde_json::json!({}));
        let allow = perms
            .as_object_mut()
            .unwrap()
            .entry("allow")
            .or_insert_with(|| serde_json::json!([]));
        let arr = allow.as_array_mut().unwrap();
        if !arr.iter().any(|v| v.as_str() == Some("Edit")) {
            arr.push(serde_json::json!("Edit"));
        }
        if !arr.iter().any(|v| v.as_str() == Some("Write"))
        {
            arr.push(serde_json::json!("Write"));
        }
    } else {
        if let Some(arr) = settings
            .pointer_mut("/permissions/allow")
            .and_then(|v| v.as_array_mut())
        {
            arr.retain(|v| {
                v.as_str() != Some("Edit")
                    && v.as_str() != Some("Write")
            });
        }
    }

    let _ = std::fs::create_dir_all(&claude_dir);
    let _ = std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings)
            .unwrap_or_default(),
    );
}

fn detect_repo_path() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    WorktreeManager::repo_root(&cwd)
        .unwrap_or(cwd)
        .to_string_lossy()
        .into_owned()
}

fn detect_branch() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    WorktreeManager::current_branch(&cwd)
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub struct App {
    pub store: ProjectStore,
    pub store_path: PathBuf,
    pub config: AppConfig,
    pub selection: Selection,
    pub mode: AppMode,
    pub message: Option<String>,
    pub should_quit: bool,
    pub should_switch: Option<String>,
    pub pane_content: String,
    pub tmux_cursor: Option<(u16, u16)>,
    pub leader_active: bool,
    pub leader_activated_at: Option<Instant>,
    pub pending_inputs: Vec<PendingInput>,
    pub usage: UsageManager,
    pub scroll_offset: usize,
    pub session_filter: SessionFilter,
    pub throbber_state: throbber_widgets_tui::ThrobberState,
    pub thinking_features: std::collections::HashSet<String>,
}

impl App {
    pub fn new(store_path: PathBuf) -> Result<Self> {
        let store = ProjectStore::load(&store_path)?;
        let config = load_config();
        let zai_monthly = config.zai.get_monthly_limit();
        let zai_weekly = config.zai.get_weekly_limit();
        let zai_five_hour = config.zai.get_five_hour_limit();
        Ok(Self {
            store,
            store_path,
            config,
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            message: None,
            should_quit: false,
            should_switch: None,
            pane_content: String::new(),
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            pending_inputs: Vec::new(),
            usage: UsageManager::new(zai_monthly, zai_weekly, zai_five_hour),
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            thinking_features: std::collections::HashSet::new(),
        })
    }

    pub fn save(&self) -> Result<()> {
        self.store.save(&self.store_path)
    }

    pub fn visible_items(&self) -> Vec<VisibleItem> {
        let mut items = Vec::new();
        for (pi, project) in
            self.store.projects.iter().enumerate()
        {
            items.push(VisibleItem::Project(pi));
            if !project.collapsed {
                for (fi, feature) in
                    project.features.iter().enumerate()
                {
                    items.push(VisibleItem::Feature(pi, fi));
                    if !feature.collapsed {
                        for (si, session) in
                            feature.sessions.iter().enumerate()
                        {
                            if self.session_matches_filter(session) {
                                items.push(VisibleItem::Session(
                                    pi, fi, si,
                                ));
                            }
                        }
                    }
                }
            }
        }
        items
    }

    fn session_matches_filter(&self, session: &FeatureSession) -> bool {
        use crate::project::SessionKind;
        match &self.session_filter {
            SessionFilter::All => true,
            SessionFilter::Claude => session.kind == SessionKind::Claude,
            SessionFilter::Opencode => session.kind == SessionKind::Opencode,
            SessionFilter::Terminal => session.kind == SessionKind::Terminal,
            SessionFilter::Nvim => {
                session.kind == SessionKind::Nvim && session.label != "Memo"
            }
            SessionFilter::Memo => {
                session.kind == SessionKind::Nvim && session.label == "Memo"
            }
        }
    }

    fn selection_index(&self) -> Option<usize> {
        let items = self.visible_items();
        items.iter().position(|item| match (&self.selection, item)
        {
            (
                Selection::Project(a),
                VisibleItem::Project(b),
            ) => a == b,
            (
                Selection::Feature(a1, a2),
                VisibleItem::Feature(b1, b2),
            ) => a1 == b1 && a2 == b2,
            (
                Selection::Session(a1, a2, a3),
                VisibleItem::Session(b1, b2, b3),
            ) => a1 == b1 && a2 == b2 && a3 == b3,
            _ => false,
        })
    }

    pub fn select_next(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        let next = (current + 1) % items.len();
        self.selection = match items[next] {
            VisibleItem::Project(pi) => Selection::Project(pi),
            VisibleItem::Feature(pi, fi) => {
                Selection::Feature(pi, fi)
            }
            VisibleItem::Session(pi, fi, si) => {
                Selection::Session(pi, fi, si)
            }
        };
    }

    pub fn select_prev(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        let prev = if current == 0 {
            items.len() - 1
        } else {
            current - 1
        };
        self.selection = match items[prev] {
            VisibleItem::Project(pi) => Selection::Project(pi),
            VisibleItem::Feature(pi, fi) => {
                Selection::Feature(pi, fi)
            }
            VisibleItem::Session(pi, fi, si) => {
                Selection::Session(pi, fi, si)
            }
        };
    }

    pub fn ensure_selection_visible(&mut self, visible_height: usize) {
        let items = self.visible_items();
        if items.is_empty() || visible_height == 0 {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        if current < self.scroll_offset {
            self.scroll_offset = current;
        } else if current >= self.scroll_offset + visible_height {
            self.scroll_offset = current - visible_height + 1;
        }
    }

    pub fn select_next_feature(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        for offset in 1..=items.len() {
            let idx = (current + offset) % items.len();
            if matches!(items[idx], VisibleItem::Feature(..)) {
                self.selection = match items[idx] {
                    VisibleItem::Feature(pi, fi) => {
                        Selection::Feature(pi, fi)
                    }
                    _ => unreachable!(),
                };
                return;
            }
        }
    }

    pub fn select_prev_feature(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        for offset in 1..=items.len() {
            let idx = if current >= offset {
                current - offset
            } else {
                items.len() - (offset - current)
            };
            if matches!(items[idx], VisibleItem::Feature(..)) {
                self.selection = match items[idx] {
                    VisibleItem::Feature(pi, fi) => {
                        Selection::Feature(pi, fi)
                    }
                    _ => unreachable!(),
                };
                return;
            }
        }
    }

    pub fn sync_statuses(&mut self) {
        let live_sessions =
            TmuxManager::list_sessions().unwrap_or_default();
        for project in &mut self.store.projects {
            for feature in &mut project.features {
                if live_sessions
                    .contains(&feature.tmux_session)
                {
                    if feature.status == ProjectStatus::Stopped
                    {
                        feature.status = ProjectStatus::Idle;
                    }
                } else {
                    feature.status = ProjectStatus::Stopped;
                }
            }
        }
    }

    pub fn sync_thinking_status(&mut self) {
        self.thinking_features.clear();
        for project in &self.store.projects {
            for feature in &project.features {
                if feature.status == ProjectStatus::Stopped {
                    continue;
                }
                let agent_session = feature.sessions.iter().find(|s| {
                    matches!(
                        s.kind,
                        SessionKind::Claude | SessionKind::Opencode
                    )
                });
                let Some(session) = agent_session else {
                    continue;
                };
                if let Ok(content) = TmuxManager::capture_pane(
                    &feature.tmux_session,
                    &session.tmux_window,
                ) {
                    if Self::is_agent_thinking(&content) {
                        self.thinking_features
                            .insert(feature.tmux_session.clone());
                    }
                }
            }
        }
    }

    fn is_agent_thinking(content: &str) -> bool {
        let lower = content.to_lowercase();
        lower.contains("esc interrupt")
    }

    pub fn is_feature_thinking(&self, tmux_session: &str) -> bool {
        self.thinking_features.contains(tmux_session)
    }

    pub fn selected_project(&self) -> Option<&Project> {
        match &self.selection {
            Selection::Project(pi)
            | Selection::Feature(pi, _)
            | Selection::Session(pi, _, _) => {
                self.store.projects.get(*pi)
            }
        }
    }

    pub fn selected_feature(
        &self,
    ) -> Option<(&Project, &Feature)> {
        match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => {
                let project = self.store.projects.get(*pi)?;
                let feature = project.features.get(*fi)?;
                Some((project, feature))
            }
            _ => None,
        }
    }

    pub fn selected_session(
        &self,
    ) -> Option<(&Project, &Feature, &FeatureSession)> {
        match &self.selection {
            Selection::Session(pi, fi, si) => {
                let project = self.store.projects.get(*pi)?;
                let feature = project.features.get(*fi)?;
                let session = feature.sessions.get(*si)?;
                Some((project, feature, session))
            }
            _ => None,
        }
    }

    pub fn toggle_collapse(&mut self) {
        match &self.selection {
            Selection::Project(pi) => {
                let pi = *pi;
                if let Some(project) =
                    self.store.projects.get_mut(pi)
                {
                    project.collapsed = !project.collapsed;
                }
            }
            Selection::Feature(pi, fi) => {
                let pi = *pi;
                let fi = *fi;
                if let Some(feature) = self
                    .store
                    .projects
                    .get_mut(pi)
                    .and_then(|p| p.features.get_mut(fi))
                {
                    feature.collapsed = !feature.collapsed;
                }
            }
            Selection::Session(pi, fi, _) => {
                let pi = *pi;
                let fi = *fi;
                if let Some(feature) = self
                    .store
                    .projects
                    .get_mut(pi)
                    .and_then(|p| p.features.get_mut(fi))
                {
                    feature.collapsed = true;
                }
                self.selection = Selection::Feature(pi, fi);
            }
        }
    }

    pub fn start_create_project(&mut self) {
        self.mode = AppMode::CreatingProject(
            CreateProjectState::auto_detect(),
        );
        self.message = None;
    }

    pub fn cancel_create(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn show_error(&mut self, error: anyhow::Error) {
        self.message = Some(format!("Error: {}", error));
        match &self.mode {
            AppMode::Normal
            | AppMode::Help
            | AppMode::Viewing(_) => {}
            _ => {
                self.mode = AppMode::Normal;
            }
        }
    }

    pub fn start_browse_path(
        &mut self,
        create_state: CreateProjectState,
    ) {
        let mut explorer = match FileExplorer::new() {
            Ok(e) => e,
            Err(_) => {
                self.message =
                    Some("Failed to open file browser".into());
                return;
            }
        };

        let start_dir = PathBuf::from(&create_state.path);
        let start_dir = if start_dir.is_dir() {
            start_dir
        } else {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
        };

        let _ = explorer.set_cwd(start_dir);

        self.mode = AppMode::BrowsingPath(Box::new(
            BrowsePathState {
                explorer,
                create_state,
            },
        ));
        self.message = None;
    }

    pub fn confirm_browse_path(&mut self) {
        let path = match &self.mode {
            AppMode::BrowsingPath(state) => {
                state.explorer.cwd().to_string_lossy().into_owned()
            }
            _ => return,
        };

        let browse = std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        );
        if let AppMode::BrowsingPath(mut state) = browse {
            state.create_state.path = path;
            state.create_state.step = CreateProjectStep::Path;
            self.mode =
                AppMode::CreatingProject(state.create_state);
        }
    }

    pub fn cancel_browse_path(&mut self) {
        let browse = std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        );
        if let AppMode::BrowsingPath(state) = browse {
            self.mode =
                AppMode::CreatingProject(state.create_state);
        }
    }

    pub fn create_project(&mut self) -> Result<()> {
        let state = match &self.mode {
            AppMode::CreatingProject(s) => s,
            _ => return Ok(()),
        };

        let name = state.name.clone();
        let path = PathBuf::from(&state.path);

        if name.is_empty() {
            self.message =
                Some("Error: Project name cannot be empty".into());
            return Ok(());
        }

        if !path.exists() {
            self.message = Some(format!(
                "Error: Path does not exist: {}",
                path.display()
            ));
            return Ok(());
        }

        if self.store.find_project(&name).is_some() {
            self.message = Some(format!(
                "Error: Project '{}' already exists",
                name
            ));
            return Ok(());
        }

        let (project_path, is_git) =
            match WorktreeManager::repo_root(&path) {
                Ok(r) => (r, true),
                Err(_) => (path.clone(), false),
            };
        let project =
            Project::new(name.clone(), project_path, is_git);

        self.store.add_project(project);
        self.save()?;

        let pi =
            self.store.projects.len().saturating_sub(1);
        self.selection = Selection::Project(pi);
        self.mode = AppMode::Normal;
        self.message =
            Some(format!("Created project '{}'", name));

        Ok(())
    }

    pub fn delete_project(&mut self) -> Result<()> {
        let project_name = match &self.mode {
            AppMode::DeletingProject(name) => name.clone(),
            _ => return Ok(()),
        };

        if let Some(project) =
            self.store.find_project(&project_name)
        {
            let features: Vec<(String, PathBuf, bool)> = project
                .features
                .iter()
                .map(|f| {
                    (
                        f.tmux_session.clone(),
                        f.workdir.clone(),
                        f.is_worktree,
                    )
                })
                .collect();
            let repo = project.repo.clone();

            for (session, workdir, is_worktree) in features {
                let _ = TmuxManager::kill_session(&session);
                if is_worktree {
                    let _ =
                        WorktreeManager::remove(&repo, &workdir);
                }
            }
        }

        self.store.remove_project(&project_name);
        self.save()?;

        let items = self.visible_items();
        if items.is_empty() {
            self.selection = Selection::Project(0);
        } else {
            let idx = self.selection_index().unwrap_or(0);
            if idx >= items.len() {
                let last = &items[items.len() - 1];
                self.selection = match last {
                    VisibleItem::Project(pi) => {
                        Selection::Project(*pi)
                    }
                    VisibleItem::Feature(pi, fi) => {
                        Selection::Feature(*pi, *fi)
                    }
                    VisibleItem::Session(pi, fi, si) => {
                        Selection::Session(*pi, *fi, *si)
                    }
                };
            }
        }

        self.mode = AppMode::Normal;
        self.message = Some(format!(
            "Deleted project '{}'",
            project_name
        ));
        Ok(())
    }

    pub fn start_create_feature(&mut self) {
        let (project_name, project_repo, is_first, used_workdirs) =
            match &self.selection {
                Selection::Project(pi)
                | Selection::Feature(pi, _)
                | Selection::Session(pi, _, _) => {
                    if let Some(p) =
                        self.store.projects.get(*pi)
                    {
                        let used: Vec<PathBuf> = p
                            .features
                            .iter()
                            .map(|f| f.workdir.clone())
                            .collect();
                        (
                            p.name.clone(),
                            p.repo.clone(),
                            p.features.is_empty(),
                            used,
                        )
                    } else {
                        return;
                    }
                }
            };

        let worktrees = WorktreeManager::list(&project_repo)
            .unwrap_or_default()
            .into_iter()
            .filter(|wt| {
                wt.path != project_repo
                    && !used_workdirs.contains(&wt.path)
            })
            .collect();

        self.mode = AppMode::CreatingFeature(
            CreateFeatureState::new(
                project_name,
                project_repo,
                worktrees,
                is_first,
            ),
        );
        self.message = None;
    }

    pub fn create_feature(&mut self) -> Result<()> {
        let state = match &self.mode {
            AppMode::CreatingFeature(s) => s,
            _ => return Ok(()),
        };

        let project_name = state.project_name.clone();
        let project_repo = state.project_repo.clone();
        let branch = state.branch.clone();
        let mode = state.mode.clone();
        let use_existing_worktree = state.source_index == 1
            && !state.worktrees.is_empty();
        let selected_worktree = if use_existing_worktree {
            state.worktrees.get(state.worktree_index).cloned()
        } else {
            None
        };
        let use_worktree = state.use_worktree;
        let enable_chrome = state.enable_chrome;
        let enable_notes = state.enable_notes;

        if branch.is_empty() {
            self.message =
                Some("Error: Branch name cannot be empty".into());
            return Ok(());
        }

        let stored_is_git = {
            let project =
                match self.store.find_project(&project_name) {
                    Some(p) => p,
                    None => {
                        self.message = Some(format!(
                            "Error: Project '{}' not found",
                            project_name
                        ));
                        return Ok(());
                    }
                };

            if project
                .features
                .iter()
                .any(|f| f.name == branch)
            {
                self.message = Some(format!(
                    "Error: Feature '{}' already exists in '{}'",
                    branch, project_name
                ));
                return Ok(());
            }

            if !use_worktree
                && selected_worktree.is_none()
                && project
                    .features
                    .iter()
                    .any(|f| !f.is_worktree)
            {
                self.message = Some(
                    "Error: Only one non-worktree feature \
                     allowed per project"
                        .into(),
                );
                return Ok(());
            }

            project.is_git
        };

        let is_git = stored_is_git
            || WorktreeManager::repo_root(&project_repo).is_ok();

        if is_git && !stored_is_git {
            if let Some(p) =
                self.store.find_project_mut(&project_name)
            {
                p.is_git = true;
            }
            self.save()?;
        }

        if (use_worktree || selected_worktree.is_some())
            && !is_git
        {
            self.message = Some(
                "Error: Worktrees require a git repository"
                    .into(),
            );
            return Ok(());
        }

        let (workdir, is_worktree) =
            if let Some(wt) = &selected_worktree {
                (wt.path.clone(), true)
            } else if use_worktree {
                let wt_path = WorktreeManager::create(
                    &project_repo,
                    &branch,
                    &branch,
                )?;
                (wt_path, true)
            } else {
                (project_repo.clone(), false)
            };

        if enable_notes {
            let claude_dir = workdir.join(".claude");
            if !claude_dir.exists() {
                let _ = std::fs::create_dir_all(&claude_dir);
            }
            let notes_path = claude_dir.join("notes.md");
            if !notes_path.exists() {
                let _ = std::fs::write(
                    &notes_path,
                    "# Notes\n\nWrite instructions for Claude here.\n",
                );
            }
        }

        let feature = Feature::new(
            branch.clone(),
            branch.clone(),
            workdir,
            is_worktree,
            mode,
            state.agent.clone(),
            enable_chrome,
            enable_notes,
        );

        self.store.add_feature(&project_name, feature);
        self.save()?;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            let fi = self.store.projects[pi]
                .features
                .len()
                .saturating_sub(1);
            self.store.projects[pi].collapsed = false;
            self.selection = Selection::Feature(pi, fi);
        }

        self.mode = AppMode::Normal;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            let fi = self.store.projects[pi]
                .features
                .len()
                .saturating_sub(1);
            self.ensure_feature_running(pi, fi)?;
            self.save()?;
        }

        self.message = Some(format!(
            "Created and started feature '{}'",
            branch
        ));

        Ok(())
    }

    fn ensure_feature_running(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        let repo =
            self.store.projects[pi].repo.clone();
        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        ensure_notification_hooks(
            &feature.workdir,
            &repo,
            &feature.mode,
            &feature.agent,
        );

        if feature.sessions.is_empty() {
            let session_kind = match feature.agent {
                AgentKind::Claude => SessionKind::Claude,
                AgentKind::Opencode => SessionKind::Opencode,
            };
            feature.add_session(session_kind);
            feature.add_session(SessionKind::Terminal);
            if feature.has_notes {
                let s = feature.add_session(SessionKind::Nvim);
                s.label = "Memo".into();
            }
        }

        if TmuxManager::session_exists(&feature.tmux_session) {
            return Ok(());
        }

        TmuxManager::create_session_with_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
            &feature.workdir,
        )?;

        for session in &feature.sessions[1..] {
            TmuxManager::create_window(
                &feature.tmux_session,
                &session.tmux_window,
                &feature.workdir,
            )?;
        }

        let extra_args: Vec<String> = feature.mode.cli_flags(feature.enable_chrome);
        let extra_args_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
        for session in &feature.sessions {
            match session.kind {
                SessionKind::Claude => {
                    TmuxManager::launch_claude(
                        &feature.tmux_session,
                        &session.tmux_window,
                        session.claude_session_id.as_deref(),
                        &extra_args_refs,
                    )?;
                }
                SessionKind::Opencode => {
                    TmuxManager::launch_opencode(
                        &feature.tmux_session,
                        &session.tmux_window,
                    )?;
                }
                SessionKind::Nvim => {
                    if feature.has_notes {
                        TmuxManager::send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim .claude/notes.md",
                        )?;
                    } else {
                        TmuxManager::send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim",
                        )?;
                    }
                }
                SessionKind::Terminal => {}
            }
        }

        TmuxManager::select_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
        )?;

        feature.status = ProjectStatus::Idle;
        feature.touch();

        Ok(())
    }

    pub fn start_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let status = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.status.clone());

        if status != Some(ProjectStatus::Stopped) {
            if let Some(name) = self
                .store
                .projects
                .get(pi)
                .and_then(|p| p.features.get(fi))
                .map(|f| f.name.clone())
            {
                self.message = Some(format!(
                    "Error: '{}' is already running",
                    name
                ));
            }
            return Ok(());
        }

        self.ensure_feature_running(pi, fi)?;

        let name = self.store.projects[pi].features[fi]
            .name
            .clone();
        self.save()?;
        self.message = Some(format!("Started '{}'", name));

        Ok(())
    }

    pub fn stop_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if feature.status == ProjectStatus::Stopped {
            self.message = Some(format!(
                "Error: '{}' is already stopped",
                feature.name
            ));
            return Ok(());
        }

        TmuxManager::kill_session(&feature.tmux_session)?;
        feature.status = ProjectStatus::Stopped;
        let name = feature.name.clone();
        self.save()?;
        self.message = Some(format!("Stopped '{}'", name));

        Ok(())
    }

    pub fn delete_feature(&mut self) -> Result<()> {
        let (project_name, feature_name) = match &self.mode {
            AppMode::DeletingFeature(pn, fn_) => {
                (pn.clone(), fn_.clone())
            }
            _ => return Ok(()),
        };

        if let Some(project) =
            self.store.find_project(&project_name)
            && let Some(feature) = project
                .features
                .iter()
                .find(|f| f.name == feature_name)
            {
                let _ = TmuxManager::kill_session(
                    &feature.tmux_session,
                );
                if feature.is_worktree {
                    let _ = WorktreeManager::remove(
                        &project.repo,
                        &feature.workdir,
                    );
                }
            }

        self.store
            .remove_feature(&project_name, &feature_name);
        self.save()?;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            self.selection = Selection::Project(pi);
        }

        self.mode = AppMode::Normal;
        self.message = Some(format!(
            "Deleted feature '{}'",
            feature_name
        ));
        Ok(())
    }

    pub fn add_terminal_session(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session)
        {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session =
            feature.add_session(SessionKind::Terminal);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn add_nvim_session(&mut self) -> Result<()> {
        if std::process::Command::new("nvim")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            self.message = Some(
                "Error: nvim is not installed".into(),
            );
            return Ok(());
        }

        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session)
        {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session =
            feature.add_session(SessionKind::Nvim);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;
        TmuxManager::send_keys(
            &tmux_session,
            &window,
            "nvim",
        )?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn create_memo(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if feature.has_notes {
            self.message =
                Some("Memo already exists".into());
            return Ok(());
        }

        let claude_dir = feature.workdir.join(".claude");
        if !claude_dir.exists() {
            let _ = std::fs::create_dir_all(&claude_dir);
        }
        let notes_path = claude_dir.join("notes.md");
        if !notes_path.exists() {
            let _ = std::fs::write(
                &notes_path,
                "# Notes\n\nWrite instructions for Claude here.\n",
            );
        }

        feature.has_notes = true;

        if TmuxManager::session_exists(
            &feature.tmux_session,
        ) {
            let workdir = feature.workdir.clone();
            let tmux_session =
                feature.tmux_session.clone();
            let session =
                feature.add_session(SessionKind::Nvim);
            session.label = "Memo".into();
            let window = session.tmux_window.clone();

            TmuxManager::create_window(
                &tmux_session,
                &window,
                &workdir,
            )?;
            TmuxManager::send_keys(
                &tmux_session,
                &window,
                "nvim .claude/notes.md",
            )?;

            feature.collapsed = false;
        }

        self.save()?;
        self.message = Some("Created memo".into());

        Ok(())
    }

    pub fn add_claude_session(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let repo =
            self.store.projects[pi].repo.clone();

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session)
        {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let mode = feature.mode.clone();
        let extra_args: Vec<String> = feature.mode.cli_flags(feature.enable_chrome);
        let agent = feature.agent.clone();
        ensure_notification_hooks(&workdir, &repo, &mode, &agent);
        let session_kind = match feature.agent {
            AgentKind::Claude => SessionKind::Claude,
            AgentKind::Opencode => SessionKind::Opencode,
        };
        let session = feature.add_session(session_kind);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;
        let extra_refs: Vec<&str> =
            extra_args.iter().map(|s| s.as_str()).collect();
        match feature.agent {
            AgentKind::Claude => {
                TmuxManager::launch_claude(
                    &tmux_session,
                    &window,
                    None,
                    &extra_refs,
                )?;
            }
            AgentKind::Opencode => {
                TmuxManager::launch_opencode(
                    &tmux_session,
                    &window,
                )?;
            }
        }

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn remove_session(&mut self) -> Result<()> {
        let (pi, fi, si) = match &self.selection {
            Selection::Session(pi, fi, si) => {
                (*pi, *fi, *si)
            }
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        let tmux_session = feature.tmux_session.clone();
        let session = match feature.sessions.get(si) {
            Some(s) => s,
            None => return Ok(()),
        };
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        if TmuxManager::session_exists(&tmux_session) {
            let _ = TmuxManager::kill_window(
                &tmux_session,
                &window,
            );
        }

        feature.sessions.remove(si);

        if feature.sessions.is_empty() {
            let _ = TmuxManager::kill_session(&tmux_session);
            feature.status = ProjectStatus::Stopped;
        }

        self.selection = Selection::Feature(pi, fi);
        self.save()?;
        self.message = Some(format!("Removed '{}'", label));

        Ok(())
    }

    pub fn enter_view(&mut self) -> Result<()> {
        let (pi, fi, target_si) = match &self.selection {
            Selection::Session(pi, fi, si) => {
                (*pi, *fi, Some(*si))
            }
            Selection::Feature(pi, fi) => (*pi, *fi, None),
            _ => return Ok(()),
        };

        self.ensure_feature_running(pi, fi)?;

        let (
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
        ) = {
            let project = &self.store.projects[pi];
            let feature = &project.features[fi];

            let si = target_si.unwrap_or_else(|| {
                feature
                    .sessions
                    .iter()
                    .position(|s| {
                        s.kind == SessionKind::Claude
                    })
                    .unwrap_or(0)
            });

            let session = &feature.sessions[si];
            (
                project.name.clone(),
                feature.name.clone(),
                feature.tmux_session.clone(),
                session.tmux_window.clone(),
                session.label.clone(),
                feature.mode.clone(),
            )
        };

        let feature = self.store.projects[pi]
            .features
            .get_mut(fi)
            .unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;

        let view = ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
        );

        self.save()?;
        self.pane_content.clear();

        let feat_name = view.feature_name.clone();
        self.mode = AppMode::Viewing(view);

        for input in &self.pending_inputs {
            if input.notification_type == "diff-review"
                && input.feature_name.as_deref()
                    == Some(&feat_name)
                && let Some(signal_path) =
                    &input.proceed_signal
            {
                let path = Path::new(signal_path);
                if let Some(parent) = path.parent() {
                    let _ =
                        std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(path, "");
            }
        }

        Ok(())
    }

    pub fn exit_view(&mut self) {
        self.mode = AppMode::Normal;
        self.pane_content.clear();
        self.message = Some("Returned to dashboard".into());
    }

    pub fn switch_to_selected(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let window = match &self.selection {
            Selection::Session(_, _, si) => self
                .store
                .projects
                .get(pi)
                .and_then(|p| p.features.get(fi))
                .and_then(|f| f.sessions.get(*si))
                .map(|s| s.tmux_window.clone()),
            _ => None,
        };

        self.ensure_feature_running(pi, fi)?;

        let feature = self.store.projects[pi]
            .features
            .get_mut(fi)
            .unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;
        let session = feature.tmux_session.clone();
        self.save()?;

        if let Some(window) = &window {
            let _ =
                TmuxManager::select_window(&session, window);
        }

        if TmuxManager::is_inside_tmux() {
            TmuxManager::switch_client(&session)?;
            self.message =
                Some("Switched back from project".into());
        } else {
            self.should_switch = Some(session);
        }

        Ok(())
    }

    pub fn activate_leader(&mut self) {
        self.leader_active = true;
        self.leader_activated_at = Some(Instant::now());
    }

    pub fn deactivate_leader(&mut self) {
        self.leader_active = false;
        self.leader_activated_at = None;
    }

    pub fn leader_timed_out(&self) -> bool {
        self.leader_activated_at
            .map(|t| {
                t.elapsed()
                    >= std::time::Duration::from_secs(2)
            })
            .unwrap_or(false)
    }

    pub fn toggle_scroll_mode(&mut self) {
        if let AppMode::Viewing(ref mut view) = self.mode {
            view.scroll_mode = !view.scroll_mode;
            if view.scroll_mode {
                let (content, lines) = TmuxManager::capture_pane_with_history(
                    &view.session,
                    &view.window,
                    10000,
                )
                .unwrap_or((String::new(), 0));
                view.scroll_content = content;
                view.scroll_total_lines = lines;
                view.scroll_offset = 0;
            } else {
                view.scroll_content.clear();
                view.scroll_offset = 0;
            }
        }
    }

    pub fn scroll_up(&mut self, amount: usize) {
        if let AppMode::Viewing(ref mut view) = self.mode
            && view.scroll_mode
        {
            view.scroll_offset = view.scroll_offset.saturating_sub(amount);
        }
    }

    pub fn scroll_down(&mut self, amount: usize, visible_rows: u16) {
        if let AppMode::Viewing(ref mut view) = self.mode
            && view.scroll_mode
        {
            let max_offset = view.scroll_total_lines.saturating_sub(visible_rows as usize);
            view.scroll_offset = (view.scroll_offset + amount).min(max_offset);
        }
    }

    pub fn scroll_to_top(&mut self) {
        if let AppMode::Viewing(ref mut view) = self.mode {
            view.scroll_offset = 0;
        }
    }

    pub fn scroll_to_bottom(&mut self, visible_rows: u16) {
        if let AppMode::Viewing(ref mut view) = self.mode {
            let max_offset = view.scroll_total_lines.saturating_sub(visible_rows as usize);
            view.scroll_offset = max_offset;
        }
    }

    pub fn view_next_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == view.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return Ok(()),
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == view.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return Ok(()),
                };
                (pi, fi)
            }
            _ => return Ok(()),
        };

        let project = &self.store.projects[pi];
        let len = project.features.len();
        if len <= 1 {
            return Ok(());
        }

        for offset in 1..len {
            let candidate = (fi + offset) % len;
            if project.features[candidate].status != ProjectStatus::Stopped {
                return self.switch_view_to_feature(pi, candidate);
            }
        }
        Ok(())
    }

    pub fn view_prev_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == view.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return Ok(()),
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == view.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return Ok(()),
                };
                (pi, fi)
            }
            _ => return Ok(()),
        };

        let project = &self.store.projects[pi];
        let len = project.features.len();
        if len <= 1 {
            return Ok(());
        }

        for offset in 1..len {
            let candidate = (fi + len - offset) % len;
            if project.features[candidate].status != ProjectStatus::Stopped {
                return self.switch_view_to_feature(pi, candidate);
            }
        }
        Ok(())
    }

    fn switch_view_to_feature(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        self.ensure_feature_running(pi, fi)?;

        let project = &self.store.projects[pi];
        let feature = &project.features[fi];
        let project_name = project.name.clone();
        let feature_name = feature.name.clone();
        let tmux_session = feature.tmux_session.clone();
        let vibe_mode = feature.mode.clone();

        let si = feature
            .sessions
            .iter()
            .position(|s| s.kind == SessionKind::Claude)
            .unwrap_or(0);
        let (session_window, session_label) =
            if let Some(s) = feature.sessions.get(si) {
                (s.tmux_window.clone(), s.label.clone())
            } else {
                ("claude".into(), "Claude 1".into())
            };

        let feature = self.store.projects[pi]
            .features
            .get_mut(fi)
            .unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;

        self.selection = Selection::Feature(pi, fi);
        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
        ));
        self.save()?;

        Ok(())
    }

    pub fn view_next_session(&mut self) {
        let (pi, fi, current_window) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == view.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return,
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == view.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return,
                };
                (pi, fi, view.window.clone())
            }
            _ => return,
        };

        let feature = &self.store.projects[pi].features[fi];
        if feature.sessions.len() <= 1 {
            return;
        }

        let current_si = feature
            .sessions
            .iter()
            .position(|s| s.tmux_window == current_window)
            .unwrap_or(0);
        let next_si =
            (current_si + 1) % feature.sessions.len();
        let next = &feature.sessions[next_si];

        if let AppMode::Viewing(ref mut view) = self.mode {
            view.window = next.tmux_window.clone();
            view.session_label = next.label.clone();
        }
        self.pane_content.clear();
    }

    pub fn view_prev_session(&mut self) {
        let (pi, fi, current_window) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == view.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return,
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == view.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return,
                };
                (pi, fi, view.window.clone())
            }
            _ => return,
        };

        let feature = &self.store.projects[pi].features[fi];
        if feature.sessions.len() <= 1 {
            return;
        }

        let current_si = feature
            .sessions
            .iter()
            .position(|s| s.tmux_window == current_window)
            .unwrap_or(0);
        let prev_si = if current_si == 0 {
            feature.sessions.len() - 1
        } else {
            current_si - 1
        };
        let prev = &feature.sessions[prev_si];

        if let AppMode::Viewing(ref mut view) = self.mode {
            view.window = prev.tmux_window.clone();
            view.session_label = prev.label.clone();
        }
        self.pane_content.clear();
    }

    pub fn open_session_switcher(&mut self) {
        let (project_name, feature_name, tmux_session, current_window, current_label, sessions, vibe_mode) =
            match &self.mode {
                AppMode::Viewing(view) => {
                    let pi = self
                        .store
                        .projects
                        .iter()
                        .position(|p| {
                            p.name == view.project_name
                        });
                    let pi = match pi {
                        Some(pi) => pi,
                        None => return,
                    };
                    let fi = self.store.projects[pi]
                        .features
                        .iter()
                        .position(|f| {
                            f.name == view.feature_name
                        });
                    let fi = match fi {
                        Some(fi) => fi,
                        None => return,
                    };
                    let feature =
                        &self.store.projects[pi].features[fi];
                    let entries: Vec<SwitcherEntry> = feature
                        .sessions
                        .iter()
                        .map(|s| SwitcherEntry {
                            tmux_window: s
                                .tmux_window
                                .clone(),
                            kind: s.kind.clone(),
                            label: s.label.clone(),
                        })
                        .collect();
                    (
                        view.project_name.clone(),
                        view.feature_name.clone(),
                        view.session.clone(),
                        view.window.clone(),
                        view.session_label.clone(),
                        entries,
                        view.vibe_mode.clone(),
                    )
                }
                _ => return,
            };

        if sessions.is_empty() {
            return;
        }

        let selected = sessions
            .iter()
            .position(|s| s.tmux_window == current_window)
            .unwrap_or(0);

        self.mode =
            AppMode::SessionSwitcher(SessionSwitcherState {
                project_name,
                feature_name,
                tmux_session,
                sessions,
                selected,
                return_window: current_window,
                return_label: current_label,
                vibe_mode,
            });
    }

    pub fn switch_from_switcher(&mut self) {
        let (
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
            vibe_mode,
        ) = match &self.mode {
            AppMode::SessionSwitcher(state) => {
                let entry =
                    match state.sessions.get(state.selected)
                    {
                        Some(e) => e,
                        None => return,
                    };
                (
                    state.project_name.clone(),
                    state.feature_name.clone(),
                    state.tmux_session.clone(),
                    entry.tmux_window.clone(),
                    entry.label.clone(),
                    state.vibe_mode.clone(),
                )
            }
            _ => return,
        };

        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
            vibe_mode,
        ));
    }

    pub fn cancel_session_switcher(&mut self) {
        let (
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
            vibe_mode,
        ) = match &self.mode {
            AppMode::SessionSwitcher(state) => (
                state.project_name.clone(),
                state.feature_name.clone(),
                state.tmux_session.clone(),
                state.return_window.clone(),
                state.return_label.clone(),
                state.vibe_mode.clone(),
            ),
            _ => return,
        };

        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
            vibe_mode,
        ));
    }

    pub fn scan_notifications(&mut self) {
        let notify_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("claude-super-vibeless")
            .join("notifications");

        let mut inputs = Vec::new();

        let entries = match std::fs::read_dir(&notify_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str())
                != Some("json")
            {
                continue;
            }

            let data = match std::fs::read_to_string(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };

            #[derive(Deserialize)]
            struct NotificationJson {
                session_id: Option<String>,
                cwd: Option<String>,
                message: Option<String>,
                #[serde(alias = "type")]
                notification_type: Option<String>,
                proceed_signal: Option<String>,
            }

            let notif: NotificationJson =
                match serde_json::from_str(&data) {
                    Ok(n) => n,
                    Err(_) => continue,
                };

            let session_id =
                notif.session_id.unwrap_or_default();
            let cwd = notif.cwd.unwrap_or_default();
            let message = notif.message.unwrap_or_default();
            let notification_type =
                notif.notification_type.unwrap_or_default();
            let proceed_signal = notif.proceed_signal;

            let mut project_name = None;
            let mut feature_name = None;
            let mut best_len: usize = 0;
            let cwd_path = PathBuf::from(&cwd);
            for project in &self.store.projects {
                for feature in &project.features {
                    let wlen = feature
                        .workdir
                        .as_os_str()
                        .len();
                    if (cwd_path
                        .starts_with(&feature.workdir)
                        || feature
                            .workdir
                            .starts_with(&cwd_path))
                        && wlen > best_len
                    {
                        project_name =
                            Some(project.name.clone());
                        feature_name =
                            Some(feature.name.clone());
                        best_len = wlen;
                    }
                }
            }

            inputs.push(PendingInput {
                session_id,
                cwd,
                message,
                notification_type,
                file_path: path,
                project_name,
                feature_name,
                proceed_signal,
            });
        }

        self.pending_inputs = inputs;

        if let AppMode::Viewing(ref view) = self.mode {
            let feat_name = view.feature_name.clone();
            for input in &self.pending_inputs {
                if input.notification_type == "diff-review"
                    && input.feature_name.as_deref()
                        == Some(&feat_name)
                    && let Some(signal_path) =
                        &input.proceed_signal
                {
                    let path = Path::new(signal_path);
                    if let Some(parent) = path.parent() {
                        let _ =
                            std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(path, "");
                }
            }
        }
    }

    pub fn handle_notification_select(
        &mut self,
    ) -> Result<()> {
        let idx = match &self.mode {
            AppMode::NotificationPicker(i) => *i,
            _ => return Ok(()),
        };

        let input = match self.pending_inputs.get(idx) {
            Some(i) => i.clone(),
            None => {
                self.mode = AppMode::Normal;
                return Ok(());
            }
        };

        if input.notification_type != "diff-review" {
            let _ = std::fs::remove_file(&input.file_path);
        }

        if let (Some(proj_name), Some(feat_name)) =
            (&input.project_name, &input.feature_name)
        {
            let pi = self
                .store
                .projects
                .iter()
                .position(|p| &p.name == proj_name);
            if let Some(pi) = pi {
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| &f.name == feat_name);
                if let Some(fi) = fi {
                    if input.notification_type == "diff-review"
                        && let Some(signal_path) =
                            &input.proceed_signal
                    {
                        let p = Path::new(signal_path);
                        if let Some(parent) = p.parent() {
                            let _ =
                                std::fs::create_dir_all(
                                    parent,
                                );
                        }
                        let _ = std::fs::write(p, "");
                    }
                    self.selection =
                        Selection::Feature(pi, fi);
                    self.pending_inputs.remove(idx);
                    return self.enter_view();
                }
            }
        }

        self.pending_inputs.remove(idx);
        self.mode = AppMode::Normal;
        self.message = Some(
            "Notification cleared (no matching feature)"
                .into(),
        );
        Ok(())
    }

    pub fn start_rename_session(&mut self) {
        let (pi, fi, si) = match &self.selection {
            Selection::Session(pi, fi, si) => {
                (*pi, *fi, *si)
            }
            _ => return,
        };

        let label = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .and_then(|f| f.sessions.get(si))
        {
            Some(s) => s.label.clone(),
            None => return,
        };

        self.mode =
            AppMode::RenamingSession(RenameSessionState {
                project_idx: pi,
                feature_idx: fi,
                session_idx: si,
                input: label,
                return_to: RenameReturnTo::Dashboard,
            });
    }

    pub fn start_rename_from_switcher(&mut self) {
        let (pi, fi, si, switcher_state) = match &self.mode
        {
            AppMode::SessionSwitcher(state) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == state.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return,
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == state.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return,
                };
                let si = state.selected;
                (pi, fi, si, state)
            }
            _ => return,
        };

        let label = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .and_then(|f| f.sessions.get(si))
        {
            Some(s) => s.label.clone(),
            None => return,
        };

        let saved_switcher = SessionSwitcherState {
            project_name: switcher_state
                .project_name
                .clone(),
            feature_name: switcher_state
                .feature_name
                .clone(),
            tmux_session: switcher_state
                .tmux_session
                .clone(),
            sessions: switcher_state
                .sessions
                .iter()
                .map(|s| SwitcherEntry {
                    tmux_window: s.tmux_window.clone(),
                    kind: s.kind.clone(),
                    label: s.label.clone(),
                })
                .collect(),
            selected: switcher_state.selected,
            return_window: switcher_state
                .return_window
                .clone(),
            return_label: switcher_state
                .return_label
                .clone(),
            vibe_mode: switcher_state.vibe_mode.clone(),
        };

        self.mode =
            AppMode::RenamingSession(RenameSessionState {
                project_idx: pi,
                feature_idx: fi,
                session_idx: si,
                input: label,
                return_to: RenameReturnTo::SessionSwitcher(
                    saved_switcher,
                ),
            });
    }

    pub fn apply_rename_session(&mut self) -> Result<()> {
        let (pi, fi, si, input) = match &self.mode {
            AppMode::RenamingSession(state) => (
                state.project_idx,
                state.feature_idx,
                state.session_idx,
                state.input.clone(),
            ),
            _ => return Ok(()),
        };

        if input.is_empty() {
            self.message =
                Some("Name cannot be empty".into());
            return Ok(());
        }

        if let Some(session) = self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
            .and_then(|f| f.sessions.get_mut(si))
        {
            session.label = input.clone();
        }
        self.save()?;

        let old_mode = std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        );
        if let AppMode::RenamingSession(rename_state) =
            old_mode
        {
            match rename_state.return_to {
                RenameReturnTo::Dashboard => {
                    self.mode = AppMode::Normal;
                }
                RenameReturnTo::SessionSwitcher(
                    mut switcher,
                ) => {
                    let feature = &self.store.projects[pi]
                        .features[fi];
                    switcher.sessions = feature
                        .sessions
                        .iter()
                        .map(|s| SwitcherEntry {
                            tmux_window: s
                                .tmux_window
                                .clone(),
                            kind: s.kind.clone(),
                            label: s.label.clone(),
                        })
                        .collect();
                    self.mode =
                        AppMode::SessionSwitcher(switcher);
                }
            }
        }

        self.message =
            Some(format!("Renamed to '{}'", input));
        Ok(())
    }

    pub fn cancel_rename_session(&mut self) {
        let old_mode = std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        );
        if let AppMode::RenamingSession(state) = old_mode {
            match state.return_to {
                RenameReturnTo::Dashboard => {
                    self.mode = AppMode::Normal;
                }
                RenameReturnTo::SessionSwitcher(
                    switcher,
                ) => {
                    self.mode =
                        AppMode::SessionSwitcher(switcher);
                }
            }
        }
    }

    pub fn open_command_picker(
        &mut self,
        from_view: Option<ViewState>,
    ) {
        let (repo, workdir) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => {
                let p = self.store.projects.get(*pi);
                (
                    p.map(|p| p.repo.clone()),
                    p.and_then(|p| {
                        p.features
                            .get(*fi)
                            .map(|f| f.workdir.clone())
                    }),
                )
            }
            Selection::Project(pi) => {
                (
                    self.store
                        .projects
                        .get(*pi)
                        .map(|p| p.repo.clone()),
                    None,
                )
            }
        };

        let mut commands = Vec::new();

        if let Some(home) = dirs::home_dir() {
            let global_cmd_dir =
                home.join(".claude").join("commands");
            scan_commands_recursive(
                &global_cmd_dir,
                &global_cmd_dir,
                "Global",
                &mut commands,
            );
        }
        commands.sort_by(|a, b| a.name.cmp(&b.name));

        let mut project_cmds = Vec::new();
        let mut scanned_repo = false;
        if let Some(ref wd) = workdir {
            let workdir_cmd_dir =
                wd.join(".claude").join("commands");
            if workdir_cmd_dir.exists() {
                scan_commands_recursive(
                    &workdir_cmd_dir,
                    &workdir_cmd_dir,
                    "Project",
                    &mut project_cmds,
                );
                scanned_repo = true;
            }
        }

        if !scanned_repo {
            if let Some(ref repo) = repo {
                let project_cmd_dir =
                    repo.join(".claude").join("commands");
                scan_commands_recursive(
                    &project_cmd_dir,
                    &project_cmd_dir,
                    "Project",
                    &mut project_cmds,
                );
            }
        }

        project_cmds.sort_by(|a, b| a.name.cmp(&b.name));
        commands.extend(project_cmds);

        self.mode =
            AppMode::CommandPicker(CommandPickerState {
                commands,
                selected: 0,
                from_view,
            });
    }

    pub fn start_search(&mut self) {
        self.mode = AppMode::Searching(SearchState {
            query: String::new(),
            matches: Vec::new(),
            selected_match: 0,
        });
        self.message = None;
    }

    pub fn perform_search(&mut self) {
        let query = match &self.mode {
            AppMode::Searching(state) => state.query.to_lowercase(),
            _ => return,
        };

        let mut matches = Vec::new();

        for (pi, project) in self.store.projects.iter().enumerate() {
            if project.name.to_lowercase().contains(&query) {
                matches.push(SearchMatch {
                    item: VisibleItem::Project(pi),
                    label: project.name.clone(),
                    context: shorten_path(&project.repo),
                });
            }

            for (fi, feature) in project.features.iter().enumerate() {
                if feature.name.to_lowercase().contains(&query) {
                    matches.push(SearchMatch {
                        item: VisibleItem::Feature(pi, fi),
                        label: feature.name.clone(),
                        context: format!("{} / {}", project.name, shorten_path(&feature.workdir)),
                    });
                }

                for (si, session) in feature.sessions.iter().enumerate() {
                    if session.label.to_lowercase().contains(&query) {
                        matches.push(SearchMatch {
                            item: VisibleItem::Session(pi, fi, si),
                            label: session.label.clone(),
                            context: format!("{} / {}", project.name, feature.name),
                        });
                    }
                }
            }
        }

        if let AppMode::Searching(state) = &mut self.mode {
            state.matches = matches;
            if state.selected_match >= state.matches.len() {
                state.selected_match = 0;
            }
        }
    }

    pub fn jump_to_search_match(&mut self) {
        let match_item = match &self.mode {
            AppMode::Searching(state) => {
                state.matches.get(state.selected_match).cloned()
            }
            _ => return,
        };

        if let Some(m) = match_item {
            self.selection = match m.item {
                VisibleItem::Project(pi) => Selection::Project(pi),
                VisibleItem::Feature(pi, fi) => Selection::Feature(pi, fi),
                VisibleItem::Session(pi, fi, si) => Selection::Session(pi, fi, si),
            };
            self.mode = AppMode::Normal;
            self.message = None;
        }
    }

    pub fn cancel_search(&mut self) {
        self.mode = AppMode::Normal;
        self.message = None;
    }

    pub fn select_next_search_match(&mut self) {
        if let AppMode::Searching(state) = &mut self.mode {
            if !state.matches.is_empty() {
                state.selected_match = (state.selected_match + 1) % state.matches.len();
            }
        }
    }

    pub fn select_prev_search_match(&mut self) {
        if let AppMode::Searching(state) = &mut self.mode {
            if !state.matches.is_empty() {
                state.selected_match = if state.selected_match == 0 {
                    state.matches.len() - 1
                } else {
                    state.selected_match - 1
                };
            }
        }
    }

    pub fn pick_session(&mut self) {
        let workdir = match self.selected_feature() {
            Some((_, feature)) => feature.workdir.clone(),
            None => {
                self.message =
                    Some("Select a feature first".into());
                return;
            }
        };

        let sessions = match fetch_opencode_sessions(&workdir) {
            Ok(s) => s,
            Err(e) => {
                self.message =
                    Some(format!("Failed to fetch sessions: {}", e));
                return;
            }
        };

        if sessions.is_empty() {
            self.message =
                Some("No opencode sessions for this worktree".into());
            return;
        }

        self.mode = AppMode::OpencodeSessionPicker(
            OpencodeSessionPickerState {
                sessions,
                selected: 0,
                workdir,
            },
        );
    }

    pub fn cancel_opencode_session_picker(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn confirm_opencode_session(&mut self) {
        let session_id = match &self.mode {
            AppMode::OpencodeSessionPicker(state) => {
                state
                    .sessions
                    .get(state.selected)
                    .map(|s| s.id.clone())
            }
            _ => return,
        };

        let session_id = match session_id {
            Some(id) => id,
            None => return,
        };

        let feature_running = self.selected_feature().map_or(false, |(_, f)| {
            f.status != ProjectStatus::Stopped
                && TmuxManager::session_exists(&f.tmux_session)
        });

        if feature_running {
            let workdir = match &self.mode {
                AppMode::OpencodeSessionPicker(state) => {
                    state.workdir.clone()
                }
                _ => return,
            };
            self.mode = AppMode::ConfirmingOpencodeSession {
                session_id,
                workdir,
            };
        } else {
            self.mode = AppMode::Normal;
            if let Err(e) = self.restart_feature_with_opencode_session(&session_id) {
                self.message = Some(format!("Error: {}", e));
            }
        }
    }

    pub fn cancel_opencode_session_confirm(&mut self) {
        let workdir = match &self.mode {
            AppMode::ConfirmingOpencodeSession { workdir, .. } => {
                workdir.clone()
            }
            _ => return,
        };

        self.mode = AppMode::OpencodeSessionPicker(
            OpencodeSessionPickerState {
                sessions: match fetch_opencode_sessions(&workdir) {
                    Ok(s) => s,
                    Err(_) => Vec::new(),
                },
                selected: 0,
                workdir,
            },
        );
    }

    pub fn confirm_and_start_opencode(&mut self) -> Result<()> {
        let session_id = match &self.mode {
            AppMode::ConfirmingOpencodeSession { session_id, .. } => {
                session_id.clone()
            }
            _ => return Ok(()),
        };

        self.mode = AppMode::Normal;
        self.restart_feature_with_opencode_session(&session_id)
    }

    fn restart_feature_with_opencode_session(
        &mut self,
        opencode_session_id: &str,
    ) -> Result<()> {
        let (pi, fi) = match self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => (pi, fi),
            _ => return Ok(()),
        };

        let tmux_session = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.tmux_session.clone());

        let tmux_session = match tmux_session {
            Some(s) => s,
            None => return Ok(()),
        };

        if TmuxManager::session_exists(&tmux_session) {
            TmuxManager::kill_session(&tmux_session)?;
        }

        self.ensure_feature_running_with_opencode_session(pi, fi, opencode_session_id)?;

        let (
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
        ) = {
            let project = &self.store.projects[pi];
            let feature = &project.features[fi];

            let si = feature
                .sessions
                .iter()
                .position(|s| s.kind == SessionKind::Opencode)
                .unwrap_or(0);

            let session = &feature.sessions[si];
            self.selection = Selection::Session(pi, fi, si);
            (
                project.name.clone(),
                feature.name.clone(),
                feature.tmux_session.clone(),
                session.tmux_window.clone(),
                session.label.clone(),
                feature.mode.clone(),
            )
        };

        let feature = self.store.projects[pi]
            .features
            .get_mut(fi)
            .unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;

        let view = ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
        );

        self.save()?;
        self.pane_content.clear();
        self.mode = AppMode::Viewing(view);
        self.message = Some("Restored opencode session".into());

        Ok(())
    }

    fn ensure_feature_running_with_opencode_session(
        &mut self,
        pi: usize,
        fi: usize,
        opencode_session_id: &str,
    ) -> Result<()> {
        let repo = self.store.projects[pi].repo.clone();
        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        ensure_notification_hooks(
            &feature.workdir,
            &repo,
            &feature.mode,
            &feature.agent,
        );

        if feature.sessions.is_empty() {
            feature.add_session(SessionKind::Opencode);
            feature.add_session(SessionKind::Terminal);
            if feature.has_notes {
                let s = feature.add_session(SessionKind::Nvim);
                s.label = "Memo".into();
            }
        }

        if TmuxManager::session_exists(&feature.tmux_session) {
            return Ok(());
        }

        TmuxManager::create_session_with_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
            &feature.workdir,
        )?;

        for session in &feature.sessions[1..] {
            TmuxManager::create_window(
                &feature.tmux_session,
                &session.tmux_window,
                &feature.workdir,
            )?;
        }

        for session in &feature.sessions {
            match session.kind {
                SessionKind::Opencode => {
                    TmuxManager::launch_opencode_with_session(
                        &feature.tmux_session,
                        &session.tmux_window,
                        Some(opencode_session_id),
                    )?;
                }
                SessionKind::Claude => {
                    let extra_args: Vec<String> = feature.mode.cli_flags(feature.enable_chrome);
                    let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                    TmuxManager::launch_claude(
                        &feature.tmux_session,
                        &session.tmux_window,
                        session.claude_session_id.as_deref(),
                        &extra_refs,
                    )?;
                }
                SessionKind::Nvim => {
                    if feature.has_notes {
                        TmuxManager::send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim .claude/notes.md",
                        )?;
                    } else {
                        TmuxManager::send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim",
                        )?;
                    }
                }
                SessionKind::Terminal => {}
            }
        }

        TmuxManager::select_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
        )?;

        feature.status = ProjectStatus::Idle;
        feature.touch();

        Ok(())
    }

    pub fn open_terminal(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        let session = feature.tmux_session.clone();
        if TmuxManager::session_exists(&session) {
            if let Some(terminal_session) = feature
                .sessions
                .iter()
                .find(|s| s.kind == SessionKind::Terminal)
            {
                let _ = TmuxManager::select_window(
                    &session,
                    &terminal_session.tmux_window,
                );
            }
            if TmuxManager::is_inside_tmux() {
                TmuxManager::switch_client(&session)?;
                self.message = Some(
                    "Switched back from terminal".into(),
                );
            } else {
                self.should_switch = Some(session);
            }
        }

        Ok(())
    }
}

fn scan_commands_recursive(
    base: &Path,
    dir: &Path,
    source: &str,
    out: &mut Vec<CommandEntry>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_commands_recursive(
                base, &path, source, out,
            );
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            == Some("md")
            && let Some(stem) =
                path.file_stem().and_then(|s| s.to_str())
        {
            let name = if let Ok(rel) = dir.strip_prefix(base)
                && !rel.as_os_str().is_empty()
            {
                format!(
                    "{}:{}",
                    rel.to_string_lossy(),
                    stem,
                )
            } else {
                stem.to_string()
            };

            out.push(CommandEntry {
                name,
                source: source.into(),
                path,
            });
        }
    }
}

fn fetch_opencode_sessions(
    workdir: &PathBuf,
) -> Result<Vec<OpencodeSessionInfo>> {
    use std::process::Command;

    let output = Command::new("opencode")
        .args(["session", "list", "--format", "json"])
        .output()?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "opencode session list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let sessions: Vec<serde_json::Value> =
        serde_json::from_str(&json_str)?;

    let dir_str = workdir.to_string_lossy();
    let filtered: Vec<OpencodeSessionInfo> = sessions
        .into_iter()
        .filter(|s| {
            s.get("directory")
                .and_then(|d| d.as_str())
                .map(|d| d == dir_str)
                .unwrap_or(false)
        })
        .filter_map(|s| {
            let id = s.get("id")?.as_str()?.to_string();
            let title = s
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("Untitled")
                .to_string();
            let updated = s
                .get("updated")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            Some(OpencodeSessionInfo {
                id,
                title,
                updated,
            })
        })
        .collect();

    Ok(filtered)
}
