use anyhow::Result;
use ratatui_explorer::FileExplorer;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::project::{
    Feature, FeatureSession, Project, ProjectStatus,
    ProjectStore, SessionKind,
};

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
}
use crate::tmux::TmuxManager;
use crate::worktree::WorktreeManager;

/// Ensure `.claude/settings.local.json` in `repo` has the
/// diff-review plugin enabled.  Merges with existing settings
/// rather than overwriting.
fn ensure_claude_settings(repo: &Path) -> Result<()> {
    let settings_path =
        repo.join(".claude").join("settings.local.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        std::fs::read_to_string(&settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Check if already enabled
    if settings
        .get("enabledPlugins")
        .and_then(|p| p.get("diff-review@claude_vibeless"))
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        return Ok(());
    }

    let plugins = settings
        .as_object_mut()
        .unwrap()
        .entry("enabledPlugins")
        .or_insert_with(|| serde_json::json!({}));
    plugins
        .as_object_mut()
        .unwrap()
        .insert(
            "diff-review@claude_vibeless".into(),
            serde_json::json!(true),
        );

    std::fs::create_dir_all(settings_path.parent().unwrap())?;
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings)? + "\n",
    )?;
    Ok(())
}

/// Ensure `.claude/settings.json` in the given workdir has the
/// notification hooks configured. Merges with existing settings
/// rather than overwriting.
pub fn ensure_notification_hooks(workdir: &Path) {
    let claude_dir = workdir.join(".claude");
    let settings_path = claude_dir.join("settings.json");

    let notify_script = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude-super-vibeless")
        .join("notify.sh");
    let clear_script = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude-super-vibeless")
        .join("clear-notify.sh");

    let notify_cmd =
        notify_script.to_string_lossy().to_string();
    let clear_cmd =
        clear_script.to_string_lossy().to_string();

    // Read existing settings or start fresh
    let mut settings: serde_json::Value =
        if settings_path.exists() {
            std::fs::read_to_string(&settings_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

    // Check if hooks already exist - skip write if so
    if let Some(hooks) = settings.get("hooks")
        && hooks.get("Notification").is_some()
            && hooks.get("Stop").is_some()
        {
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

    let _ = std::fs::create_dir_all(&claude_dir);
    let _ = std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings)
            .unwrap_or_default(),
    );
}

/// Try to detect the git repo root from cwd, falling back to cwd itself.
fn detect_repo_path() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    WorktreeManager::repo_root(&cwd)
        .unwrap_or(cwd)
        .to_string_lossy()
        .into_owned()
}

/// Try to detect the current git branch from cwd.
fn detect_branch() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    WorktreeManager::current_branch(&cwd)
        .ok()
        .flatten()
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
pub enum Selection {
    Project(usize),
    Feature(usize, usize),
    Session(usize, usize, usize),
}

pub struct ViewState {
    pub project_name: String,
    pub feature_name: String,
    pub session: String,
    pub window: String,
    pub session_label: String,
}

#[derive(Debug, Clone)]
pub struct PendingInput {
    pub session_id: String,
    pub cwd: String,
    pub message: String,
    pub notification_type: String,
    pub file_path: PathBuf,
    /// Resolved project name (if matched)
    pub project_name: Option<String>,
    /// Resolved feature name (if matched)
    pub feature_name: Option<String>,
}

#[derive(Deserialize)]
struct NotificationJson {
    session_id: Option<String>,
    cwd: Option<String>,
    message: Option<String>,
    #[serde(alias = "type")]
    notification_type: Option<String>,
}

pub enum RenameReturnTo {
    Dashboard,
    SessionSwitcher(SessionSwitcherState),
}

pub struct RenameSessionState {
    pub project_idx: usize,
    pub feature_idx: usize,
    pub session_idx: usize,
    pub input: String,
    pub return_to: RenameReturnTo,
}

pub enum AppMode {
    Normal,
    CreatingProject(CreateProjectState),
    CreatingFeature(CreateFeatureState),
    DeletingProject(String),
    DeletingFeature(String, String),
    Viewing(ViewState),
    Help,
    NotificationPicker(usize),
    SessionSwitcher(SessionSwitcherState),
    RenamingSession(RenameSessionState),
    BrowsingPath(Box<BrowsePathState>),
}

pub struct BrowsePathState {
    pub explorer: FileExplorer,
    pub create_state: CreateProjectState,
}

#[derive(Clone)]
pub struct CreateProjectState {
    pub step: CreateProjectStep,
    pub name: String,
    pub path: String,
}

#[derive(Clone)]
pub enum CreateProjectStep {
    Name,
    Path,
}

impl CreateProjectState {
    pub fn auto_detect() -> Self {
        Self {
            step: CreateProjectStep::Name,
            name: String::new(),
            path: detect_repo_path(),
        }
    }
}

pub struct CreateFeatureState {
    pub project_name: String,
    pub project_repo: PathBuf,
    pub branch: String,
}

impl CreateFeatureState {
    pub fn new(
        project_name: String,
        project_repo: PathBuf,
    ) -> Self {
        Self {
            project_name,
            project_repo,
            branch: detect_branch(),
        }
    }
}

/// A visible item in the flattened tree view.
#[derive(Debug, Clone)]
pub enum VisibleItem {
    Project(usize),
    Feature(usize, usize),
    Session(usize, usize, usize),
}

pub struct App {
    pub store: ProjectStore,
    pub store_path: PathBuf,
    pub selection: Selection,
    pub mode: AppMode,
    pub message: Option<String>,
    pub should_quit: bool,
    pub should_switch: Option<String>,
    pub pane_content: String,
    pub leader_active: bool,
    pub leader_activated_at: Option<Instant>,
    pub pending_inputs: Vec<PendingInput>,
}

impl App {
    pub fn new(store_path: PathBuf) -> Result<Self> {
        let store = ProjectStore::load(&store_path)?;
        Ok(Self {
            store,
            store_path,
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            message: None,
            should_quit: false,
            should_switch: None,
            pane_content: String::new(),
            leader_active: false,
            leader_activated_at: None,
            pending_inputs: Vec::new(),
        })
    }

    pub fn save(&self) -> Result<()> {
        self.store.save(&self.store_path)
    }

    /// Compute the flattened list of visible items respecting
    /// collapse state at both project and feature levels.
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
                        for (si, _session) in
                            feature.sessions.iter().enumerate()
                        {
                            items.push(VisibleItem::Session(
                                pi, fi, si,
                            ));
                        }
                    }
                }
            }
        }
        items
    }

    /// Find the index of the current selection in the visible
    /// items list.
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

    /// Sync feature statuses with actual tmux session state.
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

    /// Get the currently selected project.
    pub fn selected_project(&self) -> Option<&Project> {
        match &self.selection {
            Selection::Project(pi)
            | Selection::Feature(pi, _)
            | Selection::Session(pi, _, _) => {
                self.store.projects.get(*pi)
            }
        }
    }

    /// Get the currently selected feature.
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

    /// Get the currently selected session.
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

    /// Toggle collapse on the currently selected item.
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

    // --- Project CRUD ---

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
        // If we were in a dialog/creation mode, return to
        // normal so the user isn't stuck
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

        // Stop all features first
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

        // Fix selection
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

    // --- Feature CRUD ---

    pub fn start_create_feature(&mut self) {
        let (project_name, project_repo) =
            match &self.selection {
                Selection::Project(pi)
                | Selection::Feature(pi, _)
                | Selection::Session(pi, _, _) => {
                    if let Some(p) =
                        self.store.projects.get(*pi)
                    {
                        (p.name.clone(), p.repo.clone())
                    } else {
                        return;
                    }
                }
            };

        self.mode = AppMode::CreatingFeature(
            CreateFeatureState::new(
                project_name,
                project_repo,
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

        if branch.is_empty() {
            self.message =
                Some("Error: Branch name cannot be empty".into());
            return Ok(());
        }

        let (is_first, stored_is_git) = {
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

            // Check for duplicate feature name
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

            (project.features.is_empty(), project.is_git)
        };

        // Re-check git status if the stored flag says non-git
        let is_git = stored_is_git
            || WorktreeManager::repo_root(&project_repo).is_ok();

        // Update stored flag if we detected git after initial creation
        if is_git && !stored_is_git {
            if let Some(p) =
                self.store.find_project_mut(&project_name)
            {
                p.is_git = true;
            }
            self.save()?;
        }

        if !is_git && !is_first {
            self.message = Some(
                "Error: Non-git projects support only one feature"
                    .into(),
            );
            return Ok(());
        }

        // Ensure the repo has .claude/settings.local.json so that
        // worktrees inherit it and the diff-review plugin is enabled.
        if is_git {
            ensure_claude_settings(&project_repo)?;
        }

        let (workdir, is_worktree) = if is_first {
            (project_repo.clone(), false)
        } else {
            let wt_path = WorktreeManager::create(
                &project_repo,
                &branch,
                &branch,
            )?;
            (wt_path, true)
        };

        let feature = Feature::new(
            branch.clone(),
            branch.clone(),
            workdir,
            is_worktree,
        );

        self.store.add_feature(&project_name, feature);
        self.save()?;

        // Select the newly created feature
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
            // Ensure parent is expanded
            self.store.projects[pi].collapsed = false;
            self.selection = Selection::Feature(pi, fi);
        }

        self.mode = AppMode::Normal;
        self.message = Some(format!(
            "Created feature '{}' (stopped)",
            branch
        ));

        Ok(())
    }

    /// Ensure a feature's tmux session is running with all its
    /// windows. Auto-creates Claude + Terminal sessions if
    /// the feature has none.
    fn ensure_feature_running(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if feature.sessions.is_empty() {
            feature.add_session(SessionKind::Claude);
            feature.add_session(SessionKind::Terminal);
        }

        if TmuxManager::session_exists(&feature.tmux_session) {
            return Ok(());
        }

        ensure_notification_hooks(&feature.workdir);

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
            if session.kind == SessionKind::Claude {
                TmuxManager::launch_claude(
                    &feature.tmux_session,
                    &session.tmux_window,
                    session.claude_session_id.as_deref(),
                )?;
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

        // Get info before removing
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

        // Move selection to parent project
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

    // --- Session CRUD ---

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

    pub fn add_claude_session(&mut self) -> Result<()> {
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
        ensure_notification_hooks(&workdir);
        let session =
            feature.add_session(SessionKind::Claude);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;
        TmuxManager::launch_claude(
            &tmux_session,
            &window,
            None,
        )?;

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

        // Kill the tmux window
        if TmuxManager::session_exists(&tmux_session) {
            let _ = TmuxManager::kill_window(
                &tmux_session,
                &window,
            );
        }

        feature.sessions.remove(si);

        // If no sessions left, kill the tmux session
        if feature.sessions.is_empty() {
            let _ = TmuxManager::kill_session(&tmux_session);
            feature.status = ProjectStatus::Stopped;
        }

        // Move selection to parent feature
        self.selection = Selection::Feature(pi, fi);
        self.save()?;
        self.message = Some(format!("Removed '{}'", label));

        Ok(())
    }

    // --- View / Switch ---

    pub fn enter_view(&mut self) -> Result<()> {
        let (pi, fi, target_si) = match &self.selection {
            Selection::Session(pi, fi, si) => {
                (*pi, *fi, Some(*si))
            }
            Selection::Feature(pi, fi) => (*pi, *fi, None),
            _ => return Ok(()),
        };

        // Ensure the feature is running
        self.ensure_feature_running(pi, fi)?;

        // Pick the session to view
        let (
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
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
            )
        };

        // Update status
        let feature = self.store.projects[pi]
            .features
            .get_mut(fi)
            .unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;

        let view = ViewState {
            project_name,
            feature_name,
            session: tmux_session,
            window: session_window,
            session_label,
        };

        self.save()?;
        self.pane_content.clear();
        self.mode = AppMode::Viewing(view);

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

        // Get window for the session if on a session
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

        // Select the specific window if on a session
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

    // --- Leader key ---

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

    /// Cycle to the next feature within the same project while
    /// staying in Viewing mode.
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
        if project.features.len() <= 1 {
            return Ok(());
        }

        let next_fi = (fi + 1) % project.features.len();
        self.switch_view_to_feature(pi, next_fi)
    }

    /// Cycle to the previous feature within the same project
    /// while staying in Viewing mode.
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
        if project.features.len() <= 1 {
            return Ok(());
        }

        let prev_fi = if fi == 0 {
            project.features.len() - 1
        } else {
            fi - 1
        };
        self.switch_view_to_feature(pi, prev_fi)
    }

    /// Switch the current view to a different feature,
    /// defaulting to its first Claude session.
    fn switch_view_to_feature(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        // Ensure the target feature is running
        self.ensure_feature_running(pi, fi)?;

        let project = &self.store.projects[pi];
        let feature = &project.features[fi];
        let project_name = project.name.clone();
        let feature_name = feature.name.clone();
        let tmux_session = feature.tmux_session.clone();

        // Default to first Claude session
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
        self.mode = AppMode::Viewing(ViewState {
            project_name,
            feature_name,
            session: tmux_session,
            window: session_window,
            session_label,
        });
        self.save()?;

        Ok(())
    }

    /// Cycle to the next session within the current feature
    /// while staying in Viewing mode.
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

    /// Cycle to the previous session within the current
    /// feature while staying in Viewing mode.
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

    /// Open the session switcher overlay from Viewing mode.
    pub fn open_session_switcher(&mut self) {
        let (project_name, feature_name, tmux_session, current_window, current_label, sessions) =
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
            });
    }

    /// Switch to the selected session from the switcher and
    /// return to Viewing mode.
    pub fn switch_from_switcher(&mut self) {
        let (
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
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
                )
            }
            _ => return,
        };

        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState {
            project_name,
            feature_name,
            session: tmux_session,
            window,
            session_label: label,
        });
    }

    /// Cancel the session switcher and return to the original
    /// session in Viewing mode.
    pub fn cancel_session_switcher(&mut self) {
        let (
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
        ) = match &self.mode {
            AppMode::SessionSwitcher(state) => (
                state.project_name.clone(),
                state.feature_name.clone(),
                state.tmux_session.clone(),
                state.return_window.clone(),
                state.return_label.clone(),
            ),
            _ => return,
        };

        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState {
            project_name,
            feature_name,
            session: tmux_session,
            window,
            session_label: label,
        });
    }

    /// Scan the notifications directory for pending input
    /// requests and match them to known features by cwd.
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

            // Try to match cwd to a known feature
            let mut project_name = None;
            let mut feature_name = None;
            let cwd_path = PathBuf::from(&cwd);
            for project in &self.store.projects {
                for feature in &project.features {
                    if cwd_path
                        .starts_with(&feature.workdir)
                        || feature
                            .workdir
                            .starts_with(&cwd_path)
                    {
                        project_name =
                            Some(project.name.clone());
                        feature_name =
                            Some(feature.name.clone());
                        break;
                    }
                }
                if project_name.is_some() {
                    break;
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
            });
        }

        self.pending_inputs = inputs;
    }

    /// Handle selecting a notification from the picker.
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

        // Delete the notification file
        let _ = std::fs::remove_file(&input.file_path);

        // Try to navigate to the matching feature
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
                    self.selection =
                        Selection::Feature(pi, fi);
                    self.pending_inputs.remove(idx);
                    return self.enter_view();
                }
            }
        }

        // No match found, just close picker
        self.pending_inputs.remove(idx);
        self.mode = AppMode::Normal;
        self.message = Some(
            "Notification cleared (no matching feature)"
                .into(),
        );
        Ok(())
    }

    // --- Rename Session ---

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

        // Save the current switcher state to return to
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
        // Validate input before taking ownership
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

        // Update the label in the store
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

        // Take ownership of mode to extract return_to
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
                    // Rebuild entries with updated labels
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
            // Select a terminal window if possible
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
