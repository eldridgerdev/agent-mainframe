use super::*;
use crate::tmux::TmuxManager;

impl App {
    pub fn sync_statuses(&mut self) {
        let live_sessions =
            self.tmux.list_sessions().unwrap_or_default();
        for project in &mut self.store.projects {
            for feature in &mut project.features {
                if live_sessions
                    .contains(&feature.tmux_session)
                {
                    if feature.status == ProjectStatus::Stopped
                    {
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
                    if session.kind
                        != crate::project::SessionKind::Custom
                    {
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
                                let line = content
                                    .lines()
                                    .next()?
                                    .trim()
                                    .to_string();
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
                        let session = feature.sessions.iter().find(
                            |s| s.kind == SessionKind::Claude,
                        );
                        let timer_changed = session
                            .and_then(|s| {
                                TmuxManager::capture_pane(
                                    &feature.tmux_session,
                                    &s.tmux_window,
                                )
                                .ok()
                            })
                            .and_then(|content| {
                                timer_re.find(&content).map(|m| {
                                    let current = m.as_str().to_string();
                                    let prev = self
                                        .last_timer_values
                                        .get(&feature.tmux_session)
                                        .cloned();
                                    self.last_timer_values.insert(
                                        feature.tmux_session.clone(),
                                        current.clone(),
                                    );
                                    prev.map(|p| p != current).unwrap_or(false)
                                })
                            })
                            .unwrap_or(false);
                        timer_changed || Self::is_claude_thinking(&feature.tmux_session)
                    }
                    AgentKind::Opencode => {
                        let session = feature.sessions.iter().find(
                            |s| s.kind == SessionKind::Opencode,
                        );
                        session
                            .and_then(|s| {
                                TmuxManager::capture_pane(
                                    &feature.tmux_session,
                                    &s.tmux_window,
                                )
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
                    self.thinking_features
                        .insert(feature.tmux_session.clone());
                }
            }
        }
    }

    fn is_claude_thinking(tmux_session: &str) -> bool {
        std::path::Path::new(&format!(
            "/tmp/amf-thinking/{}",
            tmux_session
        ))
        .exists()
    }

    pub fn is_feature_thinking(&self, tmux_session: &str) -> bool {
        self.thinking_features.contains(tmux_session)
    }

    pub fn is_feature_waiting_for_input(&self, feature_name: &str) -> bool {
        self.pending_inputs.iter().any(|input| {
            input.feature_name.as_deref() == Some(feature_name)
        })
    }
}
