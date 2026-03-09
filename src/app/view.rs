use anyhow::Result;

use super::*;
use crate::tmux::TmuxManager;

impl App {
    pub fn enter_view(&mut self) -> Result<()> {
        let (pi, fi, target_si) = match &self.selection {
            Selection::Session(pi, fi, si) => (*pi, *fi, Some(*si)),
            Selection::Feature(pi, fi) => (*pi, *fi, None),
            _ => return Ok(()),
        };

        if self.block_if_feature_pending_worktree_script(pi, fi) {
            return Ok(());
        }

        self.ensure_feature_running(pi, fi)?;

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

            let si = target_si.unwrap_or_else(|| {
                feature
                    .sessions
                    .iter()
                    .position(|s| {
                        matches!(
                            s.kind,
                            SessionKind::Claude | SessionKind::Opencode | SessionKind::Codex
                        )
                    })
                    .unwrap_or(0)
            });

            let session = &feature.sessions[si];
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

        let feature = self.store.projects[pi].features.get_mut(fi).unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;

        // Clear pending input notifications for this feature
        self.pending_inputs.retain(|input| {
            if input.project_name.as_deref() == Some(&project_name)
                && input.feature_name.as_deref() == Some(&feature_name)
                && input.notification_type != "diff-review"
            {
                let _ = std::fs::remove_file(&input.file_path);
                false
            } else {
                true
            }
        });

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

        Ok(())
    }

    pub fn exit_view(&mut self) {
        self.mode = AppMode::Normal;
        self.pane_content.clear();
        self.tmux_cursor = None;
        self.message = Some("Returned to dashboard".into());
    }

    pub fn activate_leader(&mut self) {
        self.leader_active = true;
        self.leader_activated_at = Some(std::time::Instant::now());
    }

    pub fn deactivate_leader(&mut self) {
        self.leader_active = false;
        self.leader_activated_at = None;
    }

    pub fn leader_timed_out(&self) -> bool {
        let timeout_secs = self.config.leader_timeout_seconds.max(1);
        self.leader_activated_at
            .map(|t| t.elapsed() >= std::time::Duration::from_secs(timeout_secs))
            .unwrap_or(false)
    }

    pub fn toggle_scroll_mode(&mut self, visible_rows: u16) {
        if let AppMode::Viewing(ref mut view) = self.mode {
            view.scroll_mode = !view.scroll_mode;
            if view.scroll_mode {
                let is_alternate = TmuxManager::is_alternate_screen(&view.session, &view.window);
                view.scroll_passthrough = is_alternate;

                if !is_alternate {
                    let (content, lines) =
                        TmuxManager::capture_pane_with_history(&view.session, &view.window, 10000)
                            .unwrap_or((String::new(), 0));
                    view.scroll_content = content;
                    view.scroll_total_lines = lines;
                    let max_offset = lines.saturating_sub(visible_rows as usize);
                    view.scroll_offset = max_offset;
                } else {
                    view.scroll_content.clear();
                    view.scroll_total_lines = 0;
                    view.scroll_offset = 0;
                }
            } else {
                view.scroll_content.clear();
                view.scroll_offset = 0;
            }
        }
    }

    pub fn scroll_up(&mut self, amount: usize) {
        if let AppMode::Viewing(ref mut view) = self.mode
            && view.scroll_mode
            && !view.scroll_passthrough
        {
            view.scroll_offset = view.scroll_offset.saturating_sub(amount);
        }
    }

    pub fn scroll_down(&mut self, amount: usize, visible_rows: u16) {
        if let AppMode::Viewing(ref mut view) = self.mode
            && view.scroll_mode
            && !view.scroll_passthrough
        {
            let max_offset = view
                .scroll_total_lines
                .saturating_sub(visible_rows as usize);
            view.scroll_offset = (view.scroll_offset + amount).min(max_offset);
        }
    }

    pub fn scroll_to_top(&mut self) {
        if let AppMode::Viewing(ref mut view) = self.mode
            && view.scroll_mode
            && !view.scroll_passthrough
        {
            view.scroll_offset = 0;
        }
    }

    pub fn scroll_to_bottom(&mut self, visible_rows: u16) {
        if let AppMode::Viewing(ref mut view) = self.mode
            && view.scroll_mode
            && !view.scroll_passthrough
        {
            let max_offset = view
                .scroll_total_lines
                .saturating_sub(visible_rows as usize);
            view.scroll_offset = max_offset;
        }
    }

    pub fn view_next_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| p.name == view.project_name);
                let pi = match pi {
                    Some(pi) => pi,
                    None => return Ok(()),
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| f.name == view.feature_name);
                let fi = match fi {
                    Some(fi) => fi,
                    None => return Ok(()),
                };
                (pi, fi)
            }
            _ => return Ok(()),
        };

        let project = &self.store.projects[pi];
        let len = project.features.len();
        if len <= 1 {
            return Ok(());
        }

        for offset in 1..len {
            let candidate = (fi + offset) % len;
            if project.features[candidate].status != ProjectStatus::Stopped {
                return self.switch_view_to_feature(pi, candidate);
            }
        }
        Ok(())
    }

    pub fn view_prev_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| p.name == view.project_name);
                let pi = match pi {
                    Some(pi) => pi,
                    None => return Ok(()),
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| f.name == view.feature_name);
                let fi = match fi {
                    Some(fi) => fi,
                    None => return Ok(()),
                };
                (pi, fi)
            }
            _ => return Ok(()),
        };

        let project = &self.store.projects[pi];
        let len = project.features.len();
        if len <= 1 {
            return Ok(());
        }

        for offset in 1..len {
            let candidate = (fi + len - offset) % len;
            if project.features[candidate].status != ProjectStatus::Stopped {
                return self.switch_view_to_feature(pi, candidate);
            }
        }
        Ok(())
    }

    pub(crate) fn switch_view_to_feature(&mut self, pi: usize, fi: usize) -> Result<()> {
        self.ensure_feature_running(pi, fi)?;

        let project = &self.store.projects[pi];
        let feature = &project.features[fi];
        let project_name = project.name.clone();
        let feature_name = feature.name.clone();
        let tmux_session = feature.tmux_session.clone();
        let vibe_mode = feature.mode.clone();
        let review = feature.review;

        let si = feature
            .sessions
            .iter()
            .position(|s| {
                matches!(
                    s.kind,
                    SessionKind::Claude | SessionKind::Opencode | SessionKind::Codex
                )
            })
            .unwrap_or(0);
        let (session_window, session_label) = if let Some(s) = feature.sessions.get(si) {
            (s.tmux_window.clone(), s.label.clone())
        } else {
            ("terminal".into(), "Terminal 1".into())
        };

        let feature = self.store.projects[pi].features.get_mut(fi).unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;

        self.selection = Selection::Feature(pi, fi);
        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
            review,
        ));
        self.save()?;

        Ok(())
    }

    pub fn view_next_session(&mut self) {
        let (pi, fi, current_window) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| p.name == view.project_name);
                let pi = match pi {
                    Some(pi) => pi,
                    None => return,
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| f.name == view.feature_name);
                let fi = match fi {
                    Some(fi) => fi,
                    None => return,
                };
                (pi, fi, view.window.clone())
            }
            _ => return,
        };

        let feature = &self.store.projects[pi].features[fi];
        if feature.sessions.len() <= 1 {
            return;
        }

        let current_si = feature
            .sessions
            .iter()
            .position(|s| s.tmux_window == current_window)
            .unwrap_or(0);
        let next_si = (current_si + 1) % feature.sessions.len();
        let next = &feature.sessions[next_si];

        if let AppMode::Viewing(ref mut view) = self.mode {
            view.window = next.tmux_window.clone();
            view.session_label = next.label.clone();
        }
        self.pane_content.clear();
    }

    pub fn view_prev_session(&mut self) {
        let (pi, fi, current_window) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| p.name == view.project_name);
                let pi = match pi {
                    Some(pi) => pi,
                    None => return,
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| f.name == view.feature_name);
                let fi = match fi {
                    Some(fi) => fi,
                    None => return,
                };
                (pi, fi, view.window.clone())
            }
            _ => return,
        };

        let feature = &self.store.projects[pi].features[fi];
        if feature.sessions.len() <= 1 {
            return;
        }

        let current_si = feature
            .sessions
            .iter()
            .position(|s| s.tmux_window == current_window)
            .unwrap_or(0);
        let prev_si = if current_si == 0 {
            feature.sessions.len() - 1
        } else {
            current_si - 1
        };
        let prev = &feature.sessions[prev_si];

        if let AppMode::Viewing(ref mut view) = self.mode {
            view.window = prev.tmux_window.clone();
            view.session_label = prev.label.clone();
        }
        self.pane_content.clear();
    }
}
