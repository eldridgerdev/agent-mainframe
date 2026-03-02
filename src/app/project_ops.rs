use anyhow::Result;
use std::path::PathBuf;

use ratatui_explorer::FileExplorer;

use super::*;
use crate::tmux::TmuxManager;
use crate::worktree::WorktreeManager;

impl App {
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

    pub fn open_settings_project(&mut self) -> Result<()> {
        let settings_dir = crate::project::amf_config_dir();

        if !settings_dir.exists() {
            std::fs::create_dir_all(&settings_dir)?;
        }

        if let Some((pi, _)) = self.store.projects.iter().enumerate().find(|(_, p)| p.repo == settings_dir) {
            self.selection = Selection::Project(pi);
            self.store.projects[pi].collapsed = false;
            self.message = Some("Opened AMF settings project".into());
            return Ok(());
        }

        let project = Project::new("amf-settings".into(), settings_dir.clone(), false);
        self.store.add_project(project);
        self.save()?;

        let pi = self.store.projects.len().saturating_sub(1);
        self.selection = Selection::Project(pi);
        self.message = Some("Created AMF settings project".into());

        Ok(())
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
                new_folder_name: String::new(),
                creating_folder: false,
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

    pub fn create_folder_in_browse(&mut self) -> Result<()> {
        let (cwd, folder_name) = match &self.mode {
            AppMode::BrowsingPath(state) => {
                (state.explorer.cwd().to_path_buf(), state.new_folder_name.clone())
            }
            _ => return Ok(()),
        };

        if folder_name.is_empty() {
            self.message = Some("Folder name cannot be empty".into());
            return Ok(());
        }

        let new_path = cwd.join(&folder_name);
        if let Err(e) = std::fs::create_dir_all(&new_path) {
            self.message = Some(format!(
                "Error: Failed to create folder: {}",
                e
            ));
            return Ok(());
        }

        if let AppMode::BrowsingPath(state) = &mut self.mode {
            state.creating_folder = false;
            state.new_folder_name.clear();
            let _ = state.explorer.set_cwd(new_path);
            state.create_state.path = state.explorer.cwd().to_string_lossy().into_owned();
        }

        Ok(())
    }

    pub fn create_project(&mut self) -> Result<()> {
        let state = match &self.mode {
            AppMode::CreatingProject(s) => s.clone(),
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
                "Error: Path does not exist: {} (press Ctrl+B to browse and create folder)",
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
}
