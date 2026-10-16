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

        if !self.allows_agent_for_repo(&project.repo, &next_agent) {
            self.message = Some(format!(
                "Error: Harness '{}' is not allowed for this workspace",
                next_agent.display_name()
            ));
            self.mode = AppMode::Normal;
            return Ok(());
        }

        if project.preferred_agent == next_agent {
            self.mode = AppMode::Normal;
            self.message = Some(format!(
                "'{}' already prefers {}",
                project.name,
                next_agent.display_name()
            ));
            return Ok(());
        }

        let project_name = project.name.clone();
        if let Some(project) = self.store.projects.get_mut(pi) {
            project.preferred_agent = next_agent.clone();
        }

        self.mode = AppMode::Normal;
        self.save()?;
        self.message = Some(format!(
            "Updated '{}' preferred harness to {}",
            project_name,
            next_agent.display_name()
        ));

        Ok(())
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
