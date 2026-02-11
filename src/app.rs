use anyhow::Result;
use std::path::PathBuf;

use crate::project::{Feature, Project, ProjectStatus, ProjectStore};
use crate::tmux::TmuxManager;
use crate::worktree::WorktreeManager;

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

pub enum AppMode {
    Normal,
    CreatingProject(CreateProjectState),
    CreatingFeature(CreateFeatureState),
    DeletingProject(String),
    DeletingFeature(String, String),
    Viewing(ViewState),
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
