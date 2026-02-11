use anyhow::Result;
use std::path::PathBuf;

use crate::project::{Project, ProjectStatus, ProjectStore};
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

pub struct ViewState {
    pub project_name: String,
    pub session: String,
    pub window: String,
}

pub enum AppMode {
    Normal,
    Creating(CreateState),
    Deleting(String),
    Viewing(ViewState),
}

pub struct CreateState {
    pub step: CreateStep,
    pub name: String,
    pub path: String,
    pub branch: String,
}

pub enum CreateStep {
    Name,
    Path,
    Branch,
}

impl CreateState {
    pub fn auto_detect() -> Self {
        Self {
            step: CreateStep::Name,
            name: String::new(),
            path: detect_repo_path(),
            branch: detect_branch(),
        }
    }
}

pub struct App {
    pub store: ProjectStore,
    pub store_path: PathBuf,
    pub selected: usize,
    pub mode: AppMode,
    pub message: Option<String>,
    pub should_quit: bool,
    pub should_switch: Option<String>,
    pub pane_content: String,
}

impl App {
    pub fn new(store_path: PathBuf) -> Result<Self> {
        let store = ProjectStore::load(&store_path)?;
        Ok(Self {
            store,
            store_path,
            selected: 0,
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

    /// Sync project statuses with actual tmux session state
    pub fn sync_statuses(&mut self) {
        let live_sessions = TmuxManager::list_sessions().unwrap_or_default();
        for project in &mut self.store.projects {
            if live_sessions.contains(&project.tmux_session) {
                if project.status == ProjectStatus::Stopped {
                    project.status = ProjectStatus::Idle;
                }
            } else {
                project.status = ProjectStatus::Stopped;
            }
        }
    }

    pub fn project_count(&self) -> usize {
        self.store.projects.len()
    }

    pub fn selected_project(&self) -> Option<&Project> {
        self.store.projects.get(self.selected)
    }

    pub fn select_next(&mut self) {
        if self.project_count() > 0 {
            self.selected = (self.selected + 1) % self.project_count();
        }
    }

    pub fn select_prev(&mut self) {
        if self.project_count() > 0 {
            if self.selected == 0 {
                self.selected = self.project_count() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    pub fn start_create(&mut self) {
        self.mode = AppMode::Creating(CreateState::auto_detect());
        self.message = None;
    }

    pub fn cancel_create(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn create_project(&mut self) -> Result<()> {
        let state = match &self.mode {
            AppMode::Creating(s) => s,
            _ => return Ok(()),
        };

        let name = state.name.clone();
        let path = PathBuf::from(&state.path);
        let branch = if state.branch.is_empty() {
            None
        } else {
            Some(state.branch.clone())
        };

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

        if self.store.find(&name).is_some() {
            self.message = Some(format!(
                "Project '{}' already exists",
                name
            ));
            return Ok(());
        }

        // Determine if we need a worktree
        let repo_root = WorktreeManager::repo_root(&path)?;
        let needs_worktree =
            self.store.has_project_in_repo(&repo_root) && branch.is_some();

        let (workdir, is_worktree) = if needs_worktree {
            let branch_name = branch.as_deref().unwrap();
            let wt_path =
                WorktreeManager::create(&repo_root, &name, branch_name)?;
            (wt_path, true)
        } else {
            (path.clone(), false)
        };

        let mut project =
            Project::new(name.clone(), repo_root, workdir.clone(), branch, is_worktree);

        // Create tmux session
        TmuxManager::create_session(&project.tmux_session, &workdir)?;
        TmuxManager::launch_claude(&project.tmux_session, None)?;
        project.status = ProjectStatus::Idle;

        self.store.add(project);
        self.save()?;

        self.mode = AppMode::Normal;
        self.message = Some(format!("Created project '{}'", name));
        self.selected = self.project_count().saturating_sub(1);

        Ok(())
    }

    pub fn delete_selected(&mut self) -> Result<()> {
        if let Some(project) = self.selected_project().cloned() {
            // Kill tmux session
            TmuxManager::kill_session(&project.tmux_session)?;

            // Remove worktree if applicable
            if project.is_worktree {
                let _ = WorktreeManager::remove(&project.repo, &project.workdir);
            }

            let name = project.name.clone();
            self.store.remove(&name);
            self.save()?;

            if self.selected >= self.project_count() && self.selected > 0 {
                self.selected -= 1;
            }

            self.message = Some(format!("Deleted project '{}'", name));
        }
        self.mode = AppMode::Normal;
        Ok(())
    }

    pub fn switch_to_selected(&mut self) -> Result<()> {
        if let Some(project) = self.store.projects.get_mut(self.selected) {
            // If the session is stopped, recreate it
            if !TmuxManager::session_exists(&project.tmux_session) {
                TmuxManager::create_session(&project.tmux_session, &project.workdir)?;
                TmuxManager::launch_claude(&project.tmux_session, None)?;
            }

            project.touch();
            project.status = ProjectStatus::Active;
            let session = project.tmux_session.clone();
            self.save()?;

            if TmuxManager::is_inside_tmux() {
                // Switch client inline — manager TUI stays alive
                TmuxManager::switch_client(&session)?;
                self.message = Some("Switched back from project".into());
            } else {
                // Not inside tmux — exit and attach
                self.should_switch = Some(session);
            }
        }
        Ok(())
    }

    pub fn open_terminal(&mut self) -> Result<()> {
        if let Some(project) = self.selected_project() {
            let session = project.tmux_session.clone();
            if TmuxManager::session_exists(&session) {
                if TmuxManager::is_inside_tmux() {
                    TmuxManager::switch_client(&session)?;
                    self.message = Some("Switched back from terminal".into());
                } else {
                    self.should_switch = Some(session);
                }
            }
        }
        Ok(())
    }

    pub fn enter_view(&mut self) -> Result<()> {
        if let Some(project) = self.store.projects.get_mut(self.selected) {
            // Ensure session exists
            if !TmuxManager::session_exists(&project.tmux_session) {
                TmuxManager::create_session(&project.tmux_session, &project.workdir)?;
                TmuxManager::launch_claude(&project.tmux_session, None)?;
            }

            project.touch();
            project.status = ProjectStatus::Active;

            let view = ViewState {
                project_name: project.name.clone(),
                session: project.tmux_session.clone(),
                window: "claude".into(),
            };

            self.save()?;
            self.pane_content.clear();
            self.mode = AppMode::Viewing(view);
        }
        Ok(())
    }

    pub fn exit_view(&mut self) {
        self.mode = AppMode::Normal;
        self.pane_content.clear();
        self.message = Some("Returned to dashboard".into());
    }

    pub fn stop_selected(&mut self) -> Result<()> {
        if let Some(project) = self.store.projects.get_mut(self.selected) {
            if project.status == ProjectStatus::Stopped {
                self.message = Some(format!("'{}' is already stopped", project.name));
                return Ok(());
            }
            TmuxManager::kill_session(&project.tmux_session)?;
            project.status = ProjectStatus::Stopped;
            let name = project.name.clone();
            self.save()?;
            self.message = Some(format!("Stopped '{}'", name));
        }
        Ok(())
    }
}
