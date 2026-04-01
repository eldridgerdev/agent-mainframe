use super::*;
use crate::project::{AgentKind, SessionKind};
use crate::summary::SummaryManager;
use crate::tmux::TmuxManager;
use crate::token_tracking::{
    SessionTokenTracker, TokenUsageProvider, TokenUsageSource, format_token_usage,
    provider_for_session_kind,
};

use chrono::Utc;

pub(super) fn pane_shows_thinking_hint(content: &str) -> bool {
    let lower = content.to_lowercase();
    ["esc interrupt", "esc to interrupt", "ctrl+c to interrupt"]
        .iter()
        .any(|marker| lower.contains(marker))
}

fn opencode_sidebar_thinking_state(
    sidebar: &crate::app::opencode_storage::OpencodeSidebarData,
) -> Option<bool> {
    if sidebar
        .pending_permission
        .as_deref()
        .is_some_and(|permission| !permission.trim().is_empty())
    {
        return Some(false);
    }

    let status = sidebar.status.as_deref()?.trim().to_ascii_lowercase();
    if status.is_empty() {
        return None;
    }

    Some(!matches!(
        status.as_str(),
        "idle"
            | "ready"
            | "done"
            | "completed"
            | "closed"
            | "cancelled"
            | "canceled"
            | "stopped"
            | "waiting"
    ))
}

impl App {
    pub fn sync_statuses(&mut self) {
        let live_sessions = self.tmux.list_sessions().unwrap_or_default();
        let mut stopped_sessions = Vec::new();
        for project in &mut self.store.projects {
            for feature in &mut project.features {
                if live_sessions.contains(&feature.tmux_session) {
                    if feature.status == ProjectStatus::Stopped {
                        feature.status = ProjectStatus::Idle;
                    }
                } else {
                    if feature.status != ProjectStatus::Stopped {
                        stopped_sessions.push(feature.tmux_session.clone());
                    }
                    feature.status = ProjectStatus::Stopped;
                }
            }
        }
        for tmux_session in stopped_sessions {
            self.clear_sidebar_state_for_session(&tmux_session);
        }
    }

    pub fn sync_session_status(&mut self) {
        let mut tracker = std::mem::take(&mut self.token_tracker);
        self.sync_session_status_with_tracker(&mut tracker);
        self.token_tracker = tracker;
    }

    pub(crate) fn sync_session_status_with_tracker(&mut self, tracker: &mut SessionTokenTracker) {
        let pricing = self.config.token_pricing.clone();
        let mut discovered_sources = false;

        for project in &mut self.store.projects {
            for feature in &mut project.features {
                for session in &mut feature.sessions {
                    if session.kind == crate::project::SessionKind::Custom {
                        let status_path = feature
                            .workdir
                            .join(".amf")
                            .join("session-status")
                            .join(format!("{}.txt", session.id));
                        session.status_text =
                            std::fs::read_to_string(&status_path)
                                .ok()
                                .and_then(|content| {
                                    let line = content.lines().next()?.trim().to_string();
                                    if line.is_empty() { None } else { Some(line) }
                                });
                        continue;
                    }

                    let Some(expected_provider) = provider_for_session_kind(&session.kind) else {
                        session.status_text = None;
                        continue;
                    };

                    if session.token_usage_source.is_none()
                        && matches!(session.kind, SessionKind::Claude)
                        && session.claude_session_id.is_some()
                    {
                        if let Some(id) = session.claude_session_id.as_ref() {
                            session.set_token_usage_source_exact(TokenUsageSource {
                                provider: TokenUsageProvider::Claude,
                                id: id.clone(),
                            });
                            discovered_sources = true;
                        }
                    }

                    if session
                        .token_usage_source
                        .as_ref()
                        .is_some_and(|source| source.provider != expected_provider)
                    {
                        session.clear_token_usage_source();
                        discovered_sources = true;
                    }

                    if session.token_usage_source.is_none() {
                        if let Some(source) = tracker.discover_source(
                            &session.kind,
                            &feature.workdir,
                            session.created_at,
                        ) {
                            session.set_token_usage_source_inferred(source);
                            discovered_sources = true;
                        }
                    }

                    session.status_text = session
                        .token_usage_source
                        .as_ref()
                        .and_then(|source| tracker.read_usage(source, &feature.workdir))
                        .map(|usage| format_token_usage(&usage, &pricing));
                }
            }
        }

        if self.has_active_sidebar() {
            self.refresh_sidebar_for_current_view();
        } else if !matches!(self.mode, AppMode::Viewing(_)) {
            self.schedule_sidebar_loads_for_all_features();
        }

        if discovered_sources && let Err(err) = self.save() {
            self.log_warn(
                "usage",
                format!("Failed to persist discovered token tracking sources: {err}"),
            );
        }
    }

    pub fn sync_thinking_status(&mut self) {
        let old_thinking = self.thinking_features.clone();
        self.thinking_features.clear();
        let ipc_mode = self.ipc.is_some();
        for project in &self.store.projects {
            for feature in &project.features {
                if feature.status == ProjectStatus::Stopped {
                    continue;
                }
                let thinking = match feature.agent {
                    AgentKind::Claude => {
                        if ipc_mode {
                            self.ipc_thinking_sessions.contains(&feature.tmux_session)
                                || self.ipc_tool_sessions.contains(&feature.tmux_session)
                        } else {
                            Self::is_session_marked_thinking(&feature.tmux_session)
                        }
                    }
                    AgentKind::Opencode => {
                        self.opencode_sidebar_cache
                            .get(&feature.tmux_session)
                            .and_then(opencode_sidebar_thinking_state)
                            .or_else(|| {
                                feature
                                    .sessions
                                    .iter()
                                    .find(|s| s.kind == SessionKind::Opencode)
                                    .and_then(|s| {
                                        TmuxManager::capture_pane(
                                            &feature.tmux_session,
                                            &s.tmux_window,
                                        )
                                        .ok()
                                    })
                                    .map(|content| pane_shows_thinking_hint(&content))
                            })
                            .unwrap_or(false)
                    }
                    AgentKind::Codex => {
                        if ipc_mode {
                            self.ipc_thinking_sessions.contains(&feature.tmux_session)
                        } else {
                            // Fallback when IPC is unavailable.
                            Self::is_session_marked_thinking(&feature.tmux_session)
                        }
                    }
                };
                if thinking {
                    self.thinking_features.insert(feature.tmux_session.clone());
                }
            }
        }

        // Agent-agnostic fallback: if a feature transitions from
        // thinking to not-thinking, treat it as waiting for user
        // input unless another pending notification already exists.
        let active_features: Vec<(String, String, String, String, AgentKind)> = self
            .store
            .projects
            .iter()
            .flat_map(|project| {
                project.features.iter().filter_map(|feature| {
                    if feature.status == ProjectStatus::Stopped {
                        return None;
                    }
                    Some((
                        project.name.clone(),
                        feature.name.clone(),
                        feature.tmux_session.clone(),
                        feature.workdir.to_string_lossy().into_owned(),
                        feature.agent.clone(),
                    ))
                })
            })
            .collect();

        for (project_name, feature_name, sid, cwd, agent) in active_features {
            let was_thinking = old_thinking.contains(&sid);
            let is_thinking = self.thinking_features.contains(&sid);

            if is_thinking {
                let before = self.pending_inputs.len();
                self.pending_inputs.retain(|p| {
                    !(p.notification_type == "input-request"
                        && p.project_name.as_deref() == Some(&project_name)
                        && p.feature_name.as_deref() == Some(&feature_name))
                });
                let removed = before.saturating_sub(self.pending_inputs.len());
                if removed > 0 {
                    self.log_debug(
                        "sync",
                        format!(
                            "Cleared {removed} input notification(s) for {} (agent={}, session={})",
                            feature_name,
                            agent.display_name(),
                            sid
                        ),
                    );
                }
                continue;
            }

            if was_thinking && !is_thinking {
                let any_pending_for_feature = self.pending_inputs.iter().any(|p| {
                    p.project_name.as_deref() == Some(&project_name)
                        && p.feature_name.as_deref() == Some(&feature_name)
                });
                if !any_pending_for_feature {
                    self.pending_inputs.push(PendingInput {
                        session_id: sid.clone(),
                        cwd,
                        message: "Agent finished and is waiting for input".to_string(),
                        notification_type: "input-request".to_string(),
                        file_path: std::path::PathBuf::new(),
                        target_file_path: None,
                        relative_path: None,
                        change_id: None,
                        tool: None,
                        old_snippet: None,
                        new_snippet: None,
                        original_file: None,
                        proposed_file: None,
                        is_new_file: None,
                        reason: None,
                        response_file: None,
                        project_name: Some(project_name),
                        feature_name: Some(feature_name.clone()),
                        proceed_signal: None,
                        request_id: None,
                        reply_socket: None,
                    });
                    self.log_info(
                        "sync",
                        format!(
                            "Detected waiting-for-input for {} (agent={}, session={})",
                            feature_name,
                            agent.display_name(),
                            sid
                        ),
                    );
                }
            }
        }
    }

    fn is_session_marked_thinking(tmux_session: &str) -> bool {
        let path_str = format!("/tmp/amf-thinking/{}", tmux_session);
        let path = std::path::Path::new(&path_str);
        if !path.exists() {
            return false;
        }

        match std::fs::metadata(path) {
            Ok(metadata) => match metadata.modified() {
                Ok(modified) => match modified.elapsed() {
                    Ok(elapsed) => elapsed < std::time::Duration::from_secs(2),
                    Err(_) => false,
                },
                Err(_) => false,
            },
            Err(_) => false,
        }
    }

    pub fn cleanup_stale_thinking_files() {
        let Ok(entries) = std::fs::read_dir("/tmp/amf-thinking") else {
            return;
        };

        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(elapsed) = modified.elapsed() {
                        if elapsed > std::time::Duration::from_secs(10) {
                            let _ = std::fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
    }

    pub fn is_feature_thinking(&self, tmux_session: &str) -> bool {
        self.thinking_features.contains(tmux_session)
    }

    pub(crate) fn note_codex_prompt_submit(&mut self, tmux_session: &str, tmux_window: &str) {
        let mut matched: Option<(String, String)> = None;
        for project in &self.store.projects {
            for feature in &project.features {
                if feature.tmux_session != tmux_session || feature.agent != AgentKind::Codex {
                    continue;
                }
                let has_codex_window = feature
                    .sessions
                    .iter()
                    .any(|s| s.kind == SessionKind::Codex && s.tmux_window == tmux_window);
                if has_codex_window {
                    matched = Some((project.name.clone(), feature.name.clone()));
                    break;
                }
            }
            if matched.is_some() {
                break;
            }
        }

        if let Some((project_name, feature_name)) = matched {
            self.ipc_thinking_sessions.insert(tmux_session.to_string());
            self.pending_inputs.retain(|p| {
                !(p.notification_type == "input-request"
                    && p.project_name.as_deref() == Some(&project_name)
                    && p.feature_name.as_deref() == Some(&feature_name))
            });
            self.log_debug(
                "ipc",
                format!(
                    "Codex prompt submit captured locally; marked thinking (session={}, feature={})",
                    tmux_session, feature_name
                ),
            );
        } else {
            self.log_debug(
                "ipc",
                format!(
                    "Ignored codex prompt submit marker for non-worktree/non-codex window (session={}, window={})",
                    tmux_session, tmux_window
                ),
            );
        }
    }

    pub fn is_feature_waiting_for_input(&self, feature_name: &str) -> bool {
        self.pending_inputs
            .iter()
            .any(|input| input.feature_name.as_deref() == Some(feature_name))
    }

    pub fn trigger_summary_for_selected(&mut self) -> Result<()> {
        let selected = match &self.selection {
            Selection::Feature(pi, fi) => {
                let feature = &self.store.projects[*pi].features[*fi];
                Some((
                    feature.tmux_session.clone(),
                    feature.workdir.clone(),
                    feature.agent.clone(),
                ))
            }
            Selection::Session(pi, fi, _si) => {
                let feature = &self.store.projects[*pi].features[*fi];
                Some((
                    feature.tmux_session.clone(),
                    feature.workdir.clone(),
                    feature.agent.clone(),
                ))
            }
            _ => None,
        };

        if let Some((tmux_session, workdir, agent)) = selected {
            if self.summary_state.generating.contains(&tmux_session) {
                self.message = Some("Summary already generating...".into());
                return Ok(());
            }

            let window = self.get_window_for_session(&tmux_session, &agent);
            if let Some(w) = window {
                self.summary_state.generating.insert(tmux_session.clone());
                self.message = Some("Generating summary...".into());

                let (tx, rx) = std::sync::mpsc::channel();
                self.summary_rx = Some(rx);

                let tmux_session_clone = tmux_session.clone();
                std::thread::spawn(move || {
                    let result =
                        SummaryManager::generate_summary(&tmux_session_clone, &w, &workdir, agent);
                    let _ = tx.send((tmux_session_clone, result));
                });
            } else {
                self.message = Some("No agent window found".into());
            }
        } else {
            self.message = Some("Select a feature to summarize".into());
        }

        Ok(())
    }

    pub fn poll_summary_result(&mut self) -> Result<()> {
        if let Some(ref rx) = self.summary_rx {
            match rx.try_recv() {
                Ok((tmux_session, result)) => {
                    self.summary_rx = None;
                    self.summary_state.generating.remove(&tmux_session);

                    match result {
                        Ok(summary) => {
                            for project in &mut self.store.projects {
                                for feature in &mut project.features {
                                    if feature.tmux_session == tmux_session {
                                        feature.summary = Some(summary.clone());
                                        feature.summary_updated_at = Some(Utc::now());
                                        break;
                                    }
                                }
                            }
                            self.save()?;
                            self.message = Some(format!("Summary: {}", summary));
                        }
                        Err(e) => {
                            self.message = Some(format!("Failed to generate summary: {}", e));
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.summary_rx = None;
                }
            }
        }
        Ok(())
    }

    pub fn poll_codex_sidebar_metadata(&mut self) {
        while let Ok(result) = self.codex_sidebar_metadata_rx.try_recv() {
            self.codex_sidebar_metadata_inflight
                .remove(&result.cache_key);
            self.codex_session_title_cache
                .insert(result.cache_key.clone(), result.title);
            self.codex_session_prompt_cache
                .insert(result.cache_key, result.prompt);
        }
    }

    fn get_window_for_session(&self, tmux_session: &str, _agent: &AgentKind) -> Option<String> {
        for project in &self.store.projects {
            for feature in &project.features {
                if feature.tmux_session == tmux_session {
                    return Self::get_agent_window(feature);
                }
            }
        }
        None
    }

    fn get_agent_window(feature: &Feature) -> Option<String> {
        let target_kind = match feature.agent {
            AgentKind::Claude => SessionKind::Claude,
            AgentKind::Opencode => SessionKind::Opencode,
            AgentKind::Codex => SessionKind::Codex,
        };
        feature
            .sessions
            .iter()
            .find(|s| s.kind == target_kind)
            .map(|s| s.tmux_window.clone())
    }
}

pub fn cleanup_stale_thinking_files() {
    let Ok(entries) = std::fs::read_dir("/tmp/amf-thinking") else {
        return;
    };

    for entry in entries.flatten() {
        if let Ok(metadata) = entry.metadata() {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed > std::time::Duration::from_secs(10) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}
