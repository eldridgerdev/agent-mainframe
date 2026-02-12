use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::project::{Feature, Project, ProjectStatus, ProjectStore};
use crate::tmux::TmuxManager;
use crate::worktree::WorktreeManager;

/// Default contents for `.claude/settings.local.json` when a repo
/// does not already have one.  Ensures the diff-review plugin is
/// enabled for every Claude Code session managed by this tool.
const DEFAULT_CLAUDE_SETTINGS: &str = r#"{
  "enabledPlugins": {
    "diff-review@claude_vibeless": true
  }
}
"#;

/// Ensure `.claude/settings.local.json` exists in `repo`.
/// If missing, creates it with a minimal default that enables
/// the diff-review plugin.  Existing files are left untouched.
fn ensure_claude_settings(repo: &Path) -> Result<()> {
    let settings = repo.join(".claude").join("settings.local.json");
    if !settings.exists() {
        std::fs::create_dir_all(settings.parent().unwrap())?;
        std::fs::write(&settings, DEFAULT_CLAUDE_SETTINGS)?;
    }
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

    let notify_cmd = notify_script.to_string_lossy().to_string();
    let clear_cmd = clear_script.to_string_lossy().to_string();

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

    // Check if hooks already exist — skip write if so
    if let Some(hooks) = settings.get("hooks") {
        if hooks.get("Notification").is_some()
            && hooks.get("Stop").is_some()
        {
            return;
        }
    }

    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    // Build the hook entries we want
    let notification_hook = serde_json::json!([{
        "matcher": "",
        "hooks": [{ "type": "command", "command": notify_cmd }]
    }]);
    let stop_hook = serde_json::json!([{
        "matcher": "",
        "hooks": [{ "type": "command", "command": clear_cmd }]
    }]);

    let hooks_obj = hooks.as_object_mut().unwrap();

    // Only set if not already configured
    hooks_obj
        .entry("Notification")
        .or_insert(notification_hook);
    hooks_obj.entry("Stop").or_insert(stop_hook);

    // Write back
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
}

pub struct ViewState {
    pub project_name: String,
    pub feature_name: String,
    pub session: String,
    pub window: String,
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

pub enum AppMode {
    Normal,
    CreatingProject(CreateProjectState),
    CreatingFeature(CreateFeatureState),
    DeletingProject(String),
    DeletingFeature(String, String),
    Viewing(ViewState),
    Help,
    NotificationPicker(usize),
}

pub struct CreateProjectState {
    pub step: CreateProjectStep,
    pub name: String,
    pub path: String,
}

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
    pub fn new(project_name: String, project_repo: PathBuf) -> Self {
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
        let selection = if store.projects.is_empty() {
            Selection::Project(0)
        } else {
            Selection::Project(0)
        };
        Ok(Self {
            store,
            store_path,
            selection,
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

    /// Compute the flattened list of visible items respecting collapse.
    pub fn visible_items(&self) -> Vec<VisibleItem> {
        let mut items = Vec::new();
        for (pi, project) in self.store.projects.iter().enumerate() {
            items.push(VisibleItem::Project(pi));
            if !project.collapsed {
                for (fi, _feature) in project.features.iter().enumerate() {
                    items.push(VisibleItem::Feature(pi, fi));
                }
            }
        }
        items
    }

    /// Find the index of the current selection in the visible items list.
    fn selection_index(&self) -> Option<usize> {
        let items = self.visible_items();
        items.iter().position(|item| match (&self.selection, item) {
            (Selection::Project(a), VisibleItem::Project(b)) => a == b,
            (Selection::Feature(a1, a2), VisibleItem::Feature(b1, b2)) => {
                a1 == b1 && a2 == b2
            }
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
            VisibleItem::Feature(pi, fi) => Selection::Feature(pi, fi),
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
            VisibleItem::Feature(pi, fi) => Selection::Feature(pi, fi),
        };
    }

    /// Sync feature statuses with actual tmux session state.
    pub fn sync_statuses(&mut self) {
        let live_sessions = TmuxManager::list_sessions().unwrap_or_default();
        for project in &mut self.store.projects {
            for feature in &mut project.features {
                if live_sessions.contains(&feature.tmux_session) {
                    if feature.status == ProjectStatus::Stopped {
                        feature.status = ProjectStatus::Idle;
                    }
                } else {
                    feature.status = ProjectStatus::Stopped;
                }
            }
        }
    }

    /// Get the currently selected project (if selection is on a project row).
    pub fn selected_project(&self) -> Option<&Project> {
        match &self.selection {
            Selection::Project(pi) => self.store.projects.get(*pi),
            Selection::Feature(pi, _) => self.store.projects.get(*pi),
        }
    }

    /// Get the currently selected feature (if selection is on a feature row).
    pub fn selected_feature(&self) -> Option<(&Project, &Feature)> {
        match &self.selection {
            Selection::Feature(pi, fi) => {
                if let Some(project) = self.store.projects.get(*pi) {
                    if let Some(feature) = project.features.get(*fi) {
                        return Some((project, feature));
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Toggle collapse on the currently selected project.
    pub fn toggle_collapse(&mut self) {
        let pi = match &self.selection {
            Selection::Project(pi) => *pi,
            Selection::Feature(pi, _) => *pi,
        };
        if let Some(project) = self.store.projects.get_mut(pi) {
            project.collapsed = !project.collapsed;
            // If collapsing and we were on a feature, move to project
            if project.collapsed {
                if matches!(self.selection, Selection::Feature(_, _)) {
                    self.selection = Selection::Project(pi);
                }
            }
        }
    }

    // --- Project CRUD ---

    pub fn start_create_project(&mut self) {
        self.mode = AppMode::CreatingProject(CreateProjectState::auto_detect());
        self.message = None;
    }

    pub fn cancel_create(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn create_project(&mut self) -> Result<()> {
        let state = match &self.mode {
            AppMode::CreatingProject(s) => s,
            _ => return Ok(()),
        };

        let name = state.name.clone();
        let path = PathBuf::from(&state.path);

        if name.is_empty() {
            self.message = Some("Project name cannot be empty".into());
            return Ok(());
        }

        if !path.exists() {
            self.message = Some(format!(
                "Path does not exist: {}",
                path.display()
            ));
            return Ok(());
        }

        if self.store.find_project(&name).is_some() {
            self.message =
                Some(format!("Project '{}' already exists", name));
            return Ok(());
        }

        let repo_root = WorktreeManager::repo_root(&path)?;
        let project = Project::new(name.clone(), repo_root);

        self.store.add_project(project);
        self.save()?;

        let pi = self.store.projects.len().saturating_sub(1);
        self.selection = Selection::Project(pi);
        self.mode = AppMode::Normal;
        self.message = Some(format!("Created project '{}'", name));

        Ok(())
    }

    pub fn delete_project(&mut self) -> Result<()> {
        let project_name = match &self.mode {
            AppMode::DeletingProject(name) => name.clone(),
            _ => return Ok(()),
        };

        // Stop all features first
        if let Some(project) = self.store.find_project(&project_name) {
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
                    let _ = WorktreeManager::remove(&repo, &workdir);
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
            // Clamp to last visible item
            let idx = self.selection_index().unwrap_or(0);
            if idx >= items.len() {
                let last = &items[items.len() - 1];
                self.selection = match last {
                    VisibleItem::Project(pi) => Selection::Project(*pi),
                    VisibleItem::Feature(pi, fi) => {
                        Selection::Feature(*pi, *fi)
                    }
                };
            }
        }

        self.mode = AppMode::Normal;
        self.message =
            Some(format!("Deleted project '{}'", project_name));
        Ok(())
    }

    // --- Feature CRUD ---

    pub fn start_create_feature(&mut self) {
        let (project_name, project_repo) = match &self.selection {
            Selection::Project(pi) => {
                if let Some(p) = self.store.projects.get(*pi) {
                    (p.name.clone(), p.repo.clone())
                } else {
                    return;
                }
            }
            Selection::Feature(pi, _) => {
                if let Some(p) = self.store.projects.get(*pi) {
                    (p.name.clone(), p.repo.clone())
                } else {
                    return;
                }
            }
        };

        self.mode = AppMode::CreatingFeature(CreateFeatureState::new(
            project_name,
            project_repo,
        ));
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
            self.message = Some("Branch name cannot be empty".into());
            return Ok(());
        }

        let project = match self.store.find_project(&project_name) {
            Some(p) => p,
            None => {
                self.message =
                    Some(format!("Project '{}' not found", project_name));
                return Ok(());
            }
        };

        // Check for duplicate feature name
        if project.features.iter().any(|f| f.name == branch) {
            self.message = Some(format!(
                "Feature '{}' already exists in '{}'",
                branch, project_name
            ));
            return Ok(());
        }

        let is_first = project.features.is_empty();

        // Ensure the repo has .claude/settings.local.json so that
        // worktrees inherit it and the diff-review plugin is enabled.
        ensure_claude_settings(&project_repo)?;

        let (workdir, is_worktree) = if is_first {
            // First feature uses the repo dir directly
            (project_repo.clone(), false)
        } else {
            // Subsequent features get a worktree
            let wt_path =
                WorktreeManager::create(&project_repo, &branch, &branch)?;
            (wt_path, true)
        };

        let feature =
            Feature::new(branch.clone(), branch.clone(), workdir, is_worktree);

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

    pub fn start_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi) => (*pi, *fi),
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

        if feature.status != ProjectStatus::Stopped {
            self.message = Some(format!(
                "'{}' is already running",
                feature.name
            ));
            return Ok(());
        }

        // Create tmux session + launch claude
        ensure_notification_hooks(&feature.workdir);
        TmuxManager::create_session(
            &feature.tmux_session,
            &feature.workdir,
        )?;
        TmuxManager::launch_claude(&feature.tmux_session, None)?;
        feature.status = ProjectStatus::Idle;
        feature.touch();

        let name = feature.name.clone();
        self.save()?;
        self.message = Some(format!("Started '{}'", name));

        Ok(())
    }

    pub fn stop_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi) => (*pi, *fi),
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
                "'{}' is already stopped",
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
        if let Some(project) = self.store.find_project(&project_name) {
            if let Some(feature) =
                project.features.iter().find(|f| f.name == feature_name)
            {
                let _ = TmuxManager::kill_session(&feature.tmux_session);
                if feature.is_worktree {
                    let _ = WorktreeManager::remove(
                        &project.repo,
                        &feature.workdir,
                    );
                }
            }
        }

        self.store.remove_feature(&project_name, &feature_name);
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
        self.message =
            Some(format!("Deleted feature '{}'", feature_name));
        Ok(())
    }

    // --- View / Switch ---

    pub fn enter_view(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi) => (*pi, *fi),
            _ => return Ok(()),
        };

        // Read names/paths before mutable borrow
        let (project_name, feature_name, tmux_session, workdir) = {
            let project = match self.store.projects.get(pi) {
                Some(p) => p,
                None => return Ok(()),
            };
            let feature = match project.features.get(fi) {
                Some(f) => f,
                None => return Ok(()),
            };
            (
                project.name.clone(),
                feature.name.clone(),
                feature.tmux_session.clone(),
                feature.workdir.clone(),
            )
        };

        // Ensure session exists
        if !TmuxManager::session_exists(&tmux_session) {
            ensure_notification_hooks(&workdir);
            TmuxManager::create_session(&tmux_session, &workdir)?;
            TmuxManager::launch_claude(&tmux_session, None)?;
        }

        // Now mutate
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
            window: "claude".into(),
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
            Selection::Feature(pi, fi) => (*pi, *fi),
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

        if !TmuxManager::session_exists(&feature.tmux_session) {
            ensure_notification_hooks(&feature.workdir);
            TmuxManager::create_session(
                &feature.tmux_session,
                &feature.workdir,
            )?;
            TmuxManager::launch_claude(&feature.tmux_session, None)?;
        }

        feature.touch();
        feature.status = ProjectStatus::Active;
        let session = feature.tmux_session.clone();
        self.save()?;

        if TmuxManager::is_inside_tmux() {
            TmuxManager::switch_client(&session)?;
            self.message = Some("Switched back from project".into());
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
            .map(|t| t.elapsed() >= std::time::Duration::from_secs(2))
            .unwrap_or(false)
    }

    /// Cycle to the next feature within the same project while
    /// staying in Viewing mode.
    pub fn view_next_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.mode {
            AppMode::Viewing(view) => {
                // Find current project/feature indices
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| p.name == view.project_name);
                let pi = match pi {
                    Some(pi) => pi,
                    None => return Ok(()),
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| f.name == view.feature_name);
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
                    .position(|p| p.name == view.project_name);
                let pi = match pi {
                    Some(pi) => pi,
                    None => return Ok(()),
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| f.name == view.feature_name);
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

    /// Switch the current view to a different feature.
    fn switch_view_to_feature(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        let project = &self.store.projects[pi];
        let feature = &project.features[fi];
        let project_name = project.name.clone();
        let feature_name = feature.name.clone();
        let tmux_session = feature.tmux_session.clone();
        let workdir = feature.workdir.clone();

        if !TmuxManager::session_exists(&tmux_session) {
            ensure_notification_hooks(&workdir);
            TmuxManager::create_session(&tmux_session, &workdir)?;
            TmuxManager::launch_claude(&tmux_session, None)?;
        }

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
            window: "claude".into(),
        });
        self.save()?;

        Ok(())
    }

    /// Scan the notifications directory for pending input requests
    /// and match them to known features by cwd.
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
            if path.extension().and_then(|e| e.to_str()) != Some("json")
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
                    if cwd_path.starts_with(&feature.workdir)
                        || feature.workdir.starts_with(&cwd_path)
                    {
                        project_name = Some(project.name.clone());
                        feature_name = Some(feature.name.clone());
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
    /// Enters view mode for the matched feature and removes
    /// the notification file.
    pub fn handle_notification_select(&mut self) -> Result<()> {
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
            // Find indices
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
                    self.selection = Selection::Feature(pi, fi);
                    // Remove the item from pending_inputs
                    self.pending_inputs.remove(idx);
                    return self.enter_view();
                }
            }
        }

        // No match found, just close picker
        self.pending_inputs.remove(idx);
        self.mode = AppMode::Normal;
        self.message =
            Some("Notification cleared (no matching feature)".into());
        Ok(())
    }

    pub fn open_terminal(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi) => (*pi, *fi),
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
            if TmuxManager::is_inside_tmux() {
                TmuxManager::switch_client(&session)?;
                self.message =
                    Some("Switched back from terminal".into());
            } else {
                self.should_switch = Some(session);
            }
        }

        Ok(())
    }
}
