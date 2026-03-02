use anyhow::Result;

use super::*;
use super::setup::{ensure_notification_hooks, ensure_review_claude_md};
use crate::tmux::TmuxManager;

impl App {
    pub fn pick_session(&mut self) {
        let workdir = match &self.selection {
            Selection::Feature(pi, fi) => {
                self.store
                    .projects
                    .get(*pi)
                    .and_then(|p| p.features.get(*fi))
                    .map(|f| f.workdir.clone())
            }
            Selection::Session(pi, fi, _) => {
                self.store
                    .projects
                    .get(*pi)
                    .and_then(|p| p.features.get(*fi))
                    .map(|f| f.workdir.clone())
            }
            _ => None,
        };
        let workdir = match workdir {
            Some(w) => w,
            None => {
                self.message =
                    Some("Select a feature or session first".into());
                return;
            }
        };

        let sessions = match fetch_opencode_sessions(&workdir) {
            Ok(s) => s,
            Err(e) => {
                self.message =
                    Some(format!("Failed to fetch sessions: {}", e));
                return;
            }
        };

        if sessions.is_empty() {
            self.message =
                Some("No opencode sessions for this worktree".into());
            return;
        }

        self.mode = AppMode::OpencodeSessionPicker(
            OpencodeSessionPickerState {
                sessions,
                selected: 0,
                workdir,
            },
        );
    }

    pub fn cancel_opencode_session_picker(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn confirm_opencode_session(&mut self) {
        let session_id = match &self.mode {
            AppMode::OpencodeSessionPicker(state) => {
                state
                    .sessions
                    .get(state.selected)
                    .map(|s| s.id.clone())
            }
            _ => return,
        };

        let session_id = match session_id {
            Some(id) => id,
            None => return,
        };

        let feature_running = self.selected_feature().is_some_and(|(_, f)| {
            f.status != ProjectStatus::Stopped
                && TmuxManager::session_exists(&f.tmux_session)
        });

        if feature_running {
            let workdir = match &self.mode {
                AppMode::OpencodeSessionPicker(state) => {
                    state.workdir.clone()
                }
                _ => return,
            };
            self.mode = AppMode::ConfirmingOpencodeSession {
                session_id,
                workdir,
            };
        } else {
            self.mode = AppMode::Normal;
            if let Err(e) = self.restart_feature_with_opencode_session(&session_id) {
                self.message = Some(format!("Error: {}", e));
            }
        }
    }

    pub fn cancel_opencode_session_confirm(&mut self) {
        let workdir = match &self.mode {
            AppMode::ConfirmingOpencodeSession { workdir, .. } => {
                workdir.clone()
            }
            _ => return,
        };

        self.mode = AppMode::OpencodeSessionPicker(
            OpencodeSessionPickerState {
                sessions: fetch_opencode_sessions(&workdir).unwrap_or_default(),
                selected: 0,
                workdir,
            },
        );
    }

    pub fn confirm_and_start_opencode(&mut self) -> Result<()> {
        let session_id = match &self.mode {
            AppMode::ConfirmingOpencodeSession { session_id, .. } => {
                session_id.clone()
            }
            _ => return Ok(()),
        };

        self.mode = AppMode::Normal;
        self.restart_feature_with_opencode_session(&session_id)
    }

    fn restart_feature_with_opencode_session(
        &mut self,
        opencode_session_id: &str,
    ) -> Result<()> {
        let (pi, fi) = match self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => (pi, fi),
            _ => return Ok(()),
        };

        let tmux_session = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.tmux_session.clone());

        let tmux_session = match tmux_session {
            Some(s) => s,
            None => return Ok(()),
        };

        if TmuxManager::session_exists(&tmux_session) {
            TmuxManager::kill_session(&tmux_session)?;
        }

        self.ensure_feature_running_with_opencode_session(pi, fi, opencode_session_id)?;

        let (
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
            review,
        ) = {
            let project = &self.store.projects[pi];
            let feature = &project.features[fi];

            let si = feature
                .sessions
                .iter()
                .position(|s| s.kind == SessionKind::Opencode)
                .unwrap_or(0);

            let session = &feature.sessions[si];
            self.selection = Selection::Session(pi, fi, si);
            (
                project.name.clone(),
                feature.name.clone(),
                feature.tmux_session.clone(),
                session.tmux_window.clone(),
                session.label.clone(),
                feature.mode.clone(),
                feature.review,
            )
        };

        let feature = self.store.projects[pi]
            .features
            .get_mut(fi)
            .unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;

        let view = ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
            review,
        );

        self.save()?;
        self.pane_content.clear();
        self.mode = AppMode::Viewing(view);
        self.message = Some("Restored opencode session".into());

        Ok(())
    }

    fn ensure_feature_running_with_opencode_session(
        &mut self,
        pi: usize,
        fi: usize,
        opencode_session_id: &str,
    ) -> Result<()> {
        let repo = self.store.projects[pi].repo.clone();
        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        ensure_notification_hooks(
            &feature.workdir,
            &repo,
            &feature.mode,
            &feature.agent,
        );
        ensure_review_claude_md(&feature.workdir, feature.review);

        if feature.sessions.is_empty() {
            feature.add_session(SessionKind::Opencode);
            feature.add_session(SessionKind::Terminal);
            if feature.has_notes {
                let s = feature.add_session(SessionKind::Nvim);
                s.label = "Memo".into();
            }
        }

        if TmuxManager::session_exists(&feature.tmux_session) {
            return Ok(());
        }

        TmuxManager::create_session_with_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
            &feature.workdir,
        )?;
        TmuxManager::set_session_env(
            &feature.tmux_session,
            "AMF_SESSION",
            &feature.tmux_session,
        )?;

        for session in &feature.sessions[1..] {
            TmuxManager::create_window(
                &feature.tmux_session,
                &session.tmux_window,
                &feature.workdir,
            )?;
        }

        for session in &feature.sessions {
            match session.kind {
                SessionKind::Opencode => {
                    TmuxManager::launch_opencode_with_session(
                        &feature.tmux_session,
                        &session.tmux_window,
                        Some(opencode_session_id),
                    )?;
                }
                SessionKind::Claude => {
                    let extra_args: Vec<String> = feature.mode.cli_flags(feature.enable_chrome);
                    let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                    TmuxManager::launch_claude(
                        &feature.tmux_session,
                        &session.tmux_window,
                        session.claude_session_id.as_deref(),
                        &extra_refs,
                    )?;
                }
                SessionKind::Nvim => {
                    if feature.has_notes {
                        TmuxManager::send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim .claude/notes.md",
                        )?;
                    } else {
                        TmuxManager::send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim",
                        )?;
                    }
                }
                SessionKind::Terminal => {}
                SessionKind::Custom => {
                    if let Some(ref cmd) = session.command {
                        TmuxManager::send_literal(
                            &feature.tmux_session,
                            &session.tmux_window,
                            cmd,
                        )?;
                        TmuxManager::send_key_name(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "Enter",
                        )?;
                    }
                }
            }
        }

        TmuxManager::select_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
        )?;

        feature.status = ProjectStatus::Idle;
        feature.touch();

        Ok(())
    }
}

pub fn fetch_opencode_sessions(
    workdir: &std::path::Path,
) -> Result<Vec<OpencodeSessionInfo>> {
    use std::process::Command;

    let output = Command::new("opencode")
        .args(["session", "list", "--format", "json"])
        .output()?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "opencode session list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let sessions: Vec<serde_json::Value> =
        serde_json::from_str(&json_str)?;

    let dir_str = workdir.to_string_lossy();
    let filtered: Vec<OpencodeSessionInfo> = sessions
        .into_iter()
        .filter(|s| {
            s.get("directory")
                .and_then(|d| d.as_str())
                .map(|d| d == dir_str)
                .unwrap_or(false)
        })
        .filter_map(|s| {
            let id = s.get("id")?.as_str()?.to_string();
            let title = s
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("Untitled")
                .to_string();
            let updated = s
                .get("updated")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            Some(OpencodeSessionInfo {
                id,
                title,
                updated,
            })
        })
        .collect();

    Ok(filtered)
}
