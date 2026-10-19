use anyhow::Result;

use super::*;

impl App {
    pub fn start_project_agent_config(&mut self) -> Result<()> {
        let pi = match self.selection {
            Selection::Project(pi) => pi,
            _ => {
                self.message = Some("Select a project first".into());
                return Ok(());
            }
        };

        let Some(project) = self.store.projects.get(pi) else {
            return Ok(());
        };

        let allowed_agents = self.allowed_agents_for_repo(&project.repo);
        let selected_agent = AgentKind::index_in(&allowed_agents, &project.preferred_agent);

        self.mode = AppMode::ProjectAgentConfig(ProjectAgentConfigState {
            project_idx: pi,
            project_name: project.name.clone(),
            current_agent: project.preferred_agent.clone(),
            allowed_agents,
            selected_agent,
        });
        self.message = None;

        Ok(())
    }

    pub fn start_session_config(&mut self) -> Result<()> {
        let (pi, fi) = match self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => (pi, fi),
            _ => {
                self.message = Some("Select a worktree feature first".into());
                return Ok(());
            }
        };

        let Some(project) = self.store.projects.get(pi) else {
            return Ok(());
        };
        let Some(feature) = project.features.get(fi) else {
            return Ok(());
        };

        if feature.pending_worktree_script {
            self.message = Some(format!(
                "'{}' is still running its worktree script",
                feature.name
            ));
            return Ok(());
        }

        if !feature.is_worktree {
            self.message = Some("Session config is only available for worktree features".into());
            return Ok(());
        }

        let allowed_agents = self.allowed_agents_for_repo(&project.repo);
        let selected_agent = AgentKind::index_in(&allowed_agents, &feature.agent);

        self.mode = AppMode::SessionConfig(SessionConfigState {
            project_idx: pi,
            feature_idx: fi,
            project_name: project.name.clone(),
            feature_name: feature.name.clone(),
            current_agent: feature.agent.clone(),
            allowed_agents,
            selected_agent,
        });
        self.message = None;

        Ok(())
    }

    pub fn cancel_session_config(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn apply_session_config(&mut self) -> Result<()> {
        if let AppMode::ProjectAgentConfig(_) = &self.mode {
            return self.apply_project_agent_config();
        }

        let (pi, fi, next_agent) = match &self.mode {
            AppMode::SessionConfig(state) => {
                let next = state
                    .allowed_agents
                    .get(state.selected_agent)
                    .cloned()
                    .unwrap_or_else(|| state.current_agent.clone());
                (state.project_idx, state.feature_idx, next)
            }
            _ => return Ok(()),
        };

        self.apply_feature_agent_change(pi, fi, next_agent)
    }

    fn apply_project_agent_config(&mut self) -> Result<()> {
        let (pi, next_agent) = match &self.mode {
            AppMode::ProjectAgentConfig(state) => {
                let next = state
                    .allowed_agents
                    .get(state.selected_agent)
                    .cloned()
                    .unwrap_or_else(|| state.current_agent.clone());
                (state.project_idx, next)
            }
            _ => return Ok(()),
        };

        let Some(project) = self.store.projects.get(pi) else {
            return Ok(());
        };
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        self.mode = AppMode::Normal;
        let changed = self.set_project_preferred_harness(&project_id, next_agent.clone())?;
        self.message = Some(format!(
            "{} '{}' preferred harness to {}",
            if changed { "Updated" } else { "Kept" },
            project_name,
            next_agent.display_name()
        ));

        Ok(())
    }

    /// Persist only the project default. Existing features, sessions and
    /// processes are untouched. Both front ends use this validation/save path.
    pub(crate) fn set_project_preferred_harness(
        &mut self,
        project_id: &str,
        next: AgentKind,
    ) -> Result<bool> {
        let project = self
            .store
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| anyhow::anyhow!("Project was deleted or moved"))?;
        // Re-selecting the current preference changes nothing, so it must not
        // spawn a probe or fail because the harness has since gone missing.
        if project.preferred_agent == next {
            return Ok(false);
        }
        if !self.allows_agent_for_repo(&project.repo, &next) {
            anyhow::bail!(
                "Harness '{}' is not allowed for this workspace",
                next.display_name()
            );
        }
        self.tmux.check_harness_available(&next)?;
        let previous = project.preferred_agent.clone();
        let version = self.store_version;
        self.store
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
            .expect("resolved project")
            .preferred_agent = next.clone();
        if let Err(error) = self.save() {
            // CAS failure adopts the sibling's store. An I/O failure without
            // reload must not leave an unsaved preference displayed as saved.
            if self.store_version == version
                && let Some(project) = self
                    .store
                    .projects
                    .iter_mut()
                    .find(|project| project.id == project_id)
                && project.preferred_agent == next
            {
                project.preferred_agent = previous;
            }
            return Err(error);
        }
        Ok(true)
    }

    fn apply_feature_agent_change(
        &mut self,
        pi: usize,
        fi: usize,
        next_agent: AgentKind,
    ) -> Result<()> {
        let result = self.change_feature_harness(pi, fi, next_agent);
        self.mode = AppMode::Normal;
        if let Err(error) = &result {
            self.message = Some(format!("Error: {error:#}"));
        }
        result
    }
}
