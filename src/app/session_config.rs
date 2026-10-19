use anyhow::Result;
use chrono::Utc;
use std::collections::HashSet;

use super::setup::{
    cleanup_agent_injected_files, ensure_notification_hooks, ensure_plan_mode_instructions,
    ensure_review_claude_md,
};
use super::*;
use crate::project::{FeatureSession, ProjectStatus, SessionKind};

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
        let Some(project) = self.store.projects.get(pi) else {
            return Ok(());
        };
        let Some(feature) = project.features.get(fi) else {
            return Ok(());
        };

        if !feature.is_worktree {
            self.message = Some("Session config is only available for worktree features".into());
            self.mode = AppMode::Normal;
            return Ok(());
        }

        if !self.allows_agent_for_repo(&project.repo, &next_agent) {
            self.message = Some(format!(
                "Error: Harness '{}' is not allowed for this workspace",
                next_agent.display_name()
            ));
            self.mode = AppMode::Normal;
            return Ok(());
        }
        if let Err(err) = self.ensure_agent_mode_supported(&next_agent, &feature.mode) {
            self.message = Some(format!("Error: {}", err));
            self.mode = AppMode::Normal;
            return Ok(());
        }

        let project_name = project.name.clone();
        let feature_name = feature.name.clone();
        let repo = project.repo.clone();
        let workdir = feature.workdir.clone();
        let mode = feature.mode.clone();
        let review = feature.review;
        let plan_mode = feature.plan_mode;
        let is_worktree = feature.is_worktree;
        let tmux_session = feature.tmux_session.clone();
        let old_agent = feature.agent.clone();
        let was_running =
            feature.status != ProjectStatus::Stopped || self.tmux.session_exists(&tmux_session);

        if old_agent == next_agent {
            self.mode = AppMode::Normal;
            self.message = Some(format!(
                "'{}' is already using {}",
                feature_name,
                next_agent.display_name()
            ));
            return Ok(());
        }

        if was_running {
            self.do_stop_feature(pi, fi)?;
        }

        if old_agent != next_agent {
            let tmux_session = self.store.projects[pi].features[fi].tmux_session.clone();
            self.clear_sidebar_state_for_session(&tmux_session);
        }

        if let Some(feature) = self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            sync_feature_agent_sessions(feature, &next_agent);
            feature.agent = next_agent.clone();
        }

        cleanup_agent_injected_files(&workdir, &old_agent);
        ensure_plan_mode_instructions(&workdir, &old_agent, false);
        ensure_notification_hooks(&workdir, &repo, &mode, &next_agent, is_worktree);
        ensure_review_claude_md(&workdir, review);
        ensure_plan_mode_instructions(&workdir, &next_agent, plan_mode);

        if was_running {
            // Restarting what was already running under a different harness:
            // the agent it replaces was counted a moment ago, so this is not a
            // new claim on the machine.
            self.ensure_feature_running(pi, fi, StartIntent::Approved)?;
        }

        self.mode = AppMode::Normal;
        self.save()?;
        self.message = Some(format!(
            "Updated '{}' in '{}' to {}",
            feature_name,
            project_name,
            next_agent.display_name()
        ));

        Ok(())
    }
}

fn is_agent_session(kind: &SessionKind) -> bool {
    matches!(
        kind,
        SessionKind::Claude | SessionKind::Opencode | SessionKind::Codex | SessionKind::Pi
    )
}

fn session_kind_for_agent(agent: &AgentKind) -> SessionKind {
    match agent {
        AgentKind::Claude => SessionKind::Claude,
        AgentKind::Opencode => SessionKind::Opencode,
        AgentKind::Codex => SessionKind::Codex,
        AgentKind::Pi => SessionKind::Pi,
    }
}

fn session_label_base(agent: &AgentKind) -> &'static str {
    match agent {
        AgentKind::Claude => "Claude",
        AgentKind::Opencode => "Opencode",
        AgentKind::Codex => "Codex",
        AgentKind::Pi => "Pi",
    }
}

fn session_window_prefix(agent: &AgentKind) -> &'static str {
    match agent {
        AgentKind::Claude => "claude",
        AgentKind::Opencode => "opencode",
        AgentKind::Codex => "codex",
        AgentKind::Pi => "pi",
    }
}

fn next_agent_window_name(
    used_windows: &mut HashSet<String>,
    prefix: &str,
    next_index: &mut usize,
) -> String {
    loop {
        let candidate = if *next_index == 1 {
            prefix.to_string()
        } else {
            format!("{prefix}-{}", *next_index)
        };
        *next_index += 1;
        if used_windows.insert(candidate.clone()) {
            return candidate;
        }
    }
}

fn sync_feature_agent_sessions(feature: &mut Feature, next_agent: &AgentKind) {
    let next_kind = session_kind_for_agent(next_agent);
    let label_base = session_label_base(next_agent);
    let window_prefix = session_window_prefix(next_agent);

    let agent_session_count = feature
        .sessions
        .iter()
        .filter(|session| is_agent_session(&session.kind))
        .count();

    if agent_session_count == 0 {
        let session = FeatureSession {
            id: uuid::Uuid::new_v4().to_string(),
            kind: next_kind,
            label: format!("{label_base} 1"),
            tmux_window: window_prefix.to_string(),
            claude_session_id: None,
            todo_reference: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
            stopped: false,
        };
        feature.sessions.insert(0, session);
        return;
    }

    let mut used_windows: HashSet<String> = feature
        .sessions
        .iter()
        .filter(|session| !is_agent_session(&session.kind))
        .map(|session| session.tmux_window.clone())
        .collect();
    let mut next_window_index = 1usize;
    let mut agent_index = 1usize;

    for session in &mut feature.sessions {
        if !is_agent_session(&session.kind) {
            continue;
        }

        session.kind = next_kind.clone();
        session.label = format!("{label_base} {agent_index}");
        session.tmux_window =
            next_agent_window_name(&mut used_windows, window_prefix, &mut next_window_index);
        session.claude_session_id = None;
        session.clear_token_usage_source();
        agent_index += 1;
    }
}
