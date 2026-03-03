use super::*;
use crate::project::{AgentKind, SessionKind};
use crate::summary::SummaryManager;
use crate::tmux::TmuxManager;

use chrono::Utc;

impl App {
    pub fn sync_statuses(&mut self) {
        let live_sessions = self.tmux.list_sessions().unwrap_or_default();
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

    pub fn sync_session_status(&mut self) {
        for project in &mut self.store.projects {
            for feature in &mut project.features {
                for session in &mut feature.sessions {
                    if session.kind != crate::project::SessionKind::Custom {
                        continue;
                    }
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
                                if line.is_empty() {
                                    None
                                } else {
                                    Some(line)
                                }
                            });
                }
            }
        }
    }

    pub fn sync_thinking_status(&mut self) {
        use regex::Regex;
        let timer_re = Regex::new(r"\((\d+m\s+)?\d+s\)").unwrap();

        self.thinking_features.clear();
        for project in &self.store.projects {
            for feature in &project.features {
                if feature.status == ProjectStatus::Stopped {
                    continue;
                }
                let thinking = match feature.agent {
                    AgentKind::Claude => {
                        let session = feature
                            .sessions
                            .iter()
                            .find(|s| s.kind == SessionKind::Claude);
                        let timer_changed = session
                            .and_then(|s| {
                                TmuxManager::capture_pane(&feature.tmux_session, &s.tmux_window)
                                    .ok()
                            })
                            .and_then(|content| {
                                timer_re.find(&content).map(|m| {
                                    let current = m.as_str().to_string();
                                    let prev =
                                        self.last_timer_values.get(&feature.tmux_session).cloned();
                                    self.last_timer_values
                                        .insert(feature.tmux_session.clone(), current.clone());
                                    prev.map(|p| p != current).unwrap_or(false)
                                })
                            })
                            .unwrap_or(false);
                        timer_changed || Self::is_claude_thinking(&feature.tmux_session)
                    }
                    AgentKind::Opencode => {
                        let session = feature
                            .sessions
                            .iter()
                            .find(|s| s.kind == SessionKind::Opencode);
                        session
                            .and_then(|s| {
                                TmuxManager::capture_pane(&feature.tmux_session, &s.tmux_window)
                                    .ok()
                            })
                            .map(|content| {
                                let lower = content.to_lowercase();
                                lower.contains("esc interrupt")
                            })
                            .unwrap_or(false)
                    }
                };
                if thinking {
                    self.thinking_features.insert(feature.tmux_session.clone());
                }
            }
        }
    }

    fn is_claude_thinking(tmux_session: &str) -> bool {
        std::path::Path::new(&format!("/tmp/amf-thinking/{}", tmux_session)).exists()
    }

    pub fn is_feature_thinking(&self, tmux_session: &str) -> bool {
        self.thinking_features.contains(tmux_session)
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
        };
        feature
            .sessions
            .iter()
            .find(|s| s.kind == target_kind)
            .map(|s| s.tmux_window.clone())
    }
}
