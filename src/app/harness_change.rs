//! One ordered harness replacement operation for the desktop and TUI adapters.
use super::feature_ops::{ensure_workdir_present, feature_harnesses};
use super::*;
use crate::project::{FeatureSession, ProjectStatus, SessionKind};
use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashSet;

impl App {
    pub(crate) fn change_feature_harness(
        &mut self,
        pi: usize,
        fi: usize,
        next: AgentKind,
    ) -> Result<()> {
        let project = self
            .store
            .projects
            .get(pi)
            .context("Project no longer exists")?;
        let feature = project
            .features
            .get(fi)
            .context("Feature no longer exists")?;
        anyhow::ensure!(
            feature.is_worktree,
            "Session config is only available for worktree features"
        );
        anyhow::ensure!(
            !feature.pending_worktree_script,
            "The feature's worktree script is still running"
        );
        anyhow::ensure!(
            !self.paused_plan_interview_belongs_to_feature(&feature.id),
            "Resume or finish the parked plan before changing harness"
        );
        anyhow::ensure!(
            !self.feature_tmux_session_is_shared(pi, fi),
            "The feature shares a tmux session name; resolve it before changing harness"
        );
        ensure_workdir_present(feature)?;
        let repo = project.repo.clone();
        anyhow::ensure!(
            self.extension_for_repo(&repo)
                .allowed_agents()
                .contains(&next),
            "Harness '{}' is not allowed for this workspace",
            next.display_name()
        );
        self.ensure_agent_mode_supported(&next, &feature.mode)?;
        if feature.agent == next {
            self.message = Some(format!(
                "'{}' is already using {}",
                feature.name,
                next.display_name()
            ));
            return Ok(());
        }
        // Availability and input validation happen before any process is stopped.
        self.tmux
            .check_harness_available(&next)
            .context("Replacement harness is unavailable; nothing was stopped")?;
        let old_harnesses = feature_harnesses(feature);
        setup::preflight_harness_change_setup(&feature.workdir, &repo, &old_harnesses, &next)
            .context("Cannot prepare replacement hooks; nothing was stopped")?;
        let project_id = project.id.clone();
        let original = feature.clone();
        let was_running = feature.status != ProjectStatus::Stopped
            || self.tmux.session_exists(&feature.tmux_session);
        if was_running && let Err(error) = self.do_stop_feature(pi, fi) {
            return Err(error.context("Harness change stopped during shutdown; the original harness bindings remain. Refresh to inspect process state"));
        }
        // Stop only changes feature process state; each row's explicit stopped
        // flag remains the user's choice for the subsequent restart.
        if let Err(error) = setup::change_harness_setup(&original, &repo, &old_harnesses, &next) {
            return Err(error.context("Harness change could not finish hook setup. Original bindings remain; the feature was not restarted and generated files may have changed"));
        }
        let feature = &mut self.store.projects[pi].features[fi];
        sync_feature_agent_sessions(feature, &next);
        feature.agent = next.clone();
        feature.status = ProjectStatus::Stopped;
        let version = self.store_version;
        if let Err(error) = self.save() {
            // A CAS conflict already adopted the newer store. For ordinary I/O
            // failure restore only the unsaved bindings, never start a process.
            if self.store_version == version {
                let feature = &mut self.store.projects[pi].features[fi];
                *feature = original.clone();
                feature.status = ProjectStatus::Stopped;
            }
            return Err(error.context("Replacement bindings could not be saved; no replacement was launched. Generated files may have changed"));
        }
        self.clear_sidebar_state_for_session(&original.tmux_session);
        // Ordinary Start intentionally revives an all-stopped list. A harness
        // change is not consent to override those individual stop choices.
        let has_runnable_rows = self.store.projects[pi].features[fi]
            .sessions
            .iter()
            .any(FeatureSession::runs_with_feature);
        if was_running
            && has_runnable_rows
            && let Err(error) = self.ensure_feature_running(pi, fi, StartIntent::Approved)
        {
            let running = self.tmux.session_exists(&original.tmux_session);
            self.store.projects[pi].features[fi].status = if running {
                ProjectStatus::Idle
            } else {
                ProjectStatus::Stopped
            };
            let save = self.save();
            return Err(anyhow::anyhow!(
                "Replacement harness bindings were saved, but restart failed: {error:#}. A replacement tmux session {}. Process-state save: {}. Refresh before retrying",
                if running {
                    "exists and may be partially started"
                } else {
                    "does not exist"
                },
                save.map_or_else(|error| format!("failed: {error:#}"), |_| "completed".into())
            ));
        }
        let expected_sessions =
            serde_json::to_value(&self.store.projects[pi].features[fi].sessions)?;
        if let Err(error) = self.save() {
            let running = self.tmux.session_exists(&original.tmux_session);
            // CAS failure may have adopted a store with moved/deleted/reordered
            // targets. Never use the pre-save indices or save over sibling edits.
            let location = self
                .store
                .locate_feature_by_id(Some(&project_id), &original.id);
            let unchanged_bindings = location.is_some_and(|(pi, fi)| {
                let project = &self.store.projects[pi];
                let feature = &project.features[fi];
                project.repo == repo
                    && feature.workdir == original.workdir
                    && feature.tmux_session == original.tmux_session
                    && feature.agent == next
                    && !self.feature_tmux_session_is_shared(pi, fi)
                    && serde_json::to_value(&feature.sessions).ok().as_ref()
                        == Some(&expected_sessions)
            });
            if unchanged_bindings && let Some((pi, fi)) = location {
                self.store.projects[pi].features[fi].status = if running {
                    ProjectStatus::Idle
                } else {
                    ProjectStatus::Stopped
                };
            }
            return Err(error.context(format!(
                "Replacement harness bindings were saved, but final process state could not be saved. Replacement tmux session {}. {} Refresh before retrying",
                if running { "exists" } else { "does not exist" },
                if unchanged_bindings { "Process state was reconciled in memory; external edits were preserved." }
                    else { "The target changed, moved or was deleted; no workspace row was reconciled or overwritten." }
            )));
        }
        self.message = Some(format!(
            "Updated '{}' to {}",
            original.name,
            next.display_name()
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
        let mut used_windows = feature
            .sessions
            .iter()
            .map(|session| session.tmux_window.clone())
            .collect();
        let window = next_agent_window_name(&mut used_windows, window_prefix, &mut 1);
        let session = FeatureSession {
            id: uuid::Uuid::new_v4().to_string(),
            kind: next_kind,
            label: format!("{label_base} 1"),
            tmux_window: window,
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
