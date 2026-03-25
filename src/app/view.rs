use anyhow::Result;

use super::*;
use crate::tmux::TmuxManager;
use crate::worktree::WorktreeManager;

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
            session_kind,
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
                session.kind.clone(),
                feature.mode.clone(),
                feature.review,
            )
        };

        let feature = self.store.projects[pi].features.get_mut(fi).unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;
        self.refresh_latest_prompt_for_feature(pi, fi);
        self.refresh_sidebar_plan_for_feature(pi, fi);
        self.request_codex_sidebar_metadata_for_view(
            &project_name,
            &feature_name,
            &session_window,
            &session_kind,
        );

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

        let pending_project_name = project_name.clone();
        let pending_feature_name = feature_name.clone();
        let view = ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            session_kind,
            vibe_mode,
            review,
        );

        self.save()?;
        self.pane_content.clear();

        self.mode = AppMode::Viewing(view);

        if self.use_custom_diff_review_viewer()
            && let Some(idx) = self.pending_inputs.iter().position(|input| {
                let is_structured_diff_review = input.notification_type == "change-reason"
                    || input.notification_type == "diff-review";
                is_structured_diff_review
                    && input.project_name.as_deref() == Some(&pending_project_name)
                    && input.feature_name.as_deref() == Some(&pending_feature_name)
            })
        {
            let input = self.pending_inputs.remove(idx);
            self.open_diff_review_prompt(&input);
            let _ = std::fs::remove_file(&input.file_path);
        }

        Ok(())
    }

    pub fn exit_view(&mut self) {
        self.mode = AppMode::Normal;
        self.pane_content.clear();
        self.tmux_cursor = None;
        self.message = Some("Returned to dashboard".into());
    }

    pub fn open_latest_prompt_from_view(&mut self) {
        let view = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::Viewing(view) => view,
            other => {
                self.mode = other;
                return;
            }
        };

        let feature_prompt_context = self
            .store
            .projects
            .iter()
            .find(|project| project.name == view.project_name)
            .and_then(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.name == view.feature_name)
            })
            .map(|feature| {
                let prompt_session_id = feature
                    .sessions
                    .iter()
                    .find(|session| session.tmux_window == view.window)
                    .and_then(|session| {
                        session
                            .token_usage_source
                            .as_ref()
                            .filter(|source| {
                                view.session_kind == SessionKind::Codex
                                    && source.provider
                                        == crate::token_tracking::TokenUsageProvider::Codex
                            })
                            .map(|source| source.id.clone())
                    });
                (feature.workdir.clone(), prompt_session_id)
            });

        let Some((workdir, prompt_session_id)) = feature_prompt_context else {
            self.mode = AppMode::Viewing(view);
            self.message = Some("Error: Could not resolve feature workdir".into());
            return;
        };

        let prompts = crate::app::util::read_all_prompts_for_session(
            &workdir,
            &view.session_kind,
            prompt_session_id.as_deref(),
        );
        self.mode = AppMode::LatestPrompt(LatestPromptState {
            prompts,
            selected: 0,
            view,
        });
        self.message = None;
    }

    pub fn inject_latest_prompt(&mut self) -> Result<()> {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::LatestPrompt(state) => state,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let prompt = state
            .prompts
            .get(state.selected)
            .map(|e| e.text.trim().to_string())
            .filter(|p| !p.is_empty());

        let Some(prompt) = prompt else {
            self.mode = AppMode::LatestPrompt(state);
            self.message = Some("No saved prompt to inject".into());
            return Ok(());
        };

        self.tmux
            .paste_text(&state.view.session, &state.view.window, &prompt)?;
        self.tmux
            .send_key_name(&state.view.session, &state.view.window, "Enter")?;

        self.mode = AppMode::Viewing(state.view);
        self.message = Some("Injected prompt".into());
        Ok(())
    }

    pub fn copy_selected_prompt_to_clipboard(&mut self) -> Result<()> {
        let text = match &self.mode {
            AppMode::LatestPrompt(state) => state
                .prompts
                .get(state.selected)
                .map(|e| e.text.clone())
                .filter(|t| !t.trim().is_empty()),
            _ => return Ok(()),
        };

        let Some(text) = text else {
            self.message = Some("No prompt to copy".into());
            return Ok(());
        };

        match crate::app::util::copy_to_clipboard(&text) {
            Ok(()) => self.message = Some("Copied to clipboard".into()),
            Err(e) => self.message = Some(format!("Clipboard error: {e}")),
        }
        Ok(())
    }

    pub fn latest_prompt_select_next(&mut self) {
        if let AppMode::LatestPrompt(state) = &mut self.mode {
            if !state.prompts.is_empty() && state.selected + 1 < state.prompts.len() {
                state.selected += 1;
            }
        }
    }

    pub fn latest_prompt_select_prev(&mut self) {
        if let AppMode::LatestPrompt(state) = &mut self.mode {
            if state.selected > 0 {
                state.selected -= 1;
            }
        }
    }

    pub fn open_markdown_viewer_from_view(&mut self) -> Result<()> {
        let view = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::Viewing(view) => view,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let workdir = self
            .store
            .projects
            .iter()
            .find(|project| project.name == view.project_name)
            .and_then(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.name == view.feature_name)
            })
            .map(|feature| feature.workdir.clone());

        let Some(workdir) = workdir else {
            self.mode = AppMode::Viewing(view);
            self.message = Some("Error: Could not resolve feature workdir".into());
            return Ok(());
        };

        let repo_root = WorktreeManager::is_worktree(&workdir)
            .then(|| WorktreeManager::primary_worktree_root(&workdir).ok())
            .flatten()
            .filter(|root| root != &workdir);

        let files = crate::markdown::collect_markdown_view_paths(&workdir, repo_root.as_deref());
        if files.is_empty() {
            self.mode = AppMode::Viewing(view);
            self.message = Some(
                "Error: No markdown file found (.claude/*.md or top-level *.md in the worktree/repo root)"
                    .into(),
            );
            return Ok(());
        }

        if files.len() == 1 {
            return self.open_markdown_viewer_path(
                files[0].clone(),
                workdir,
                repo_root,
                view,
                None,
            );
        }

        self.mode = AppMode::MarkdownFilePicker(crate::app::MarkdownFilePickerState {
            files,
            selected: 0,
            plan_only: true,
            workdir,
            repo_root,
            from_view: Some(view),
        });
        self.message = None;
        Ok(())
    }

    pub fn open_markdown_viewer_path(
        &mut self,
        path: PathBuf,
        workdir: PathBuf,
        repo_root: Option<PathBuf>,
        view: ViewState,
        return_to_picker: Option<crate::app::MarkdownFilePickerState>,
    ) -> Result<()> {
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => {
                self.mode = AppMode::Viewing(view);
                self.message = Some(format!("Error: Failed to read {}: {err}", path.display()));
                return Ok(());
            }
        };

        let title = crate::markdown::markdown_view_label(&path, &workdir, repo_root.as_deref());

        self.mode = AppMode::MarkdownViewer(crate::app::MarkdownViewerState {
            title,
            source_path: path,
            content,
            scroll_offset: 0,
            rendered_width: 0,
            rendered_lines: Vec::new(),
            return_to_picker,
            from_view: Some(view),
        });
        self.message = None;
        Ok(())
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
        let (session_window, session_label, session_kind) =
            if let Some(s) = feature.sessions.get(si) {
                (s.tmux_window.clone(), s.label.clone(), s.kind.clone())
            } else {
                (
                    "terminal".into(),
                    "Terminal 1".into(),
                    SessionKind::Terminal,
                )
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
            session_kind,
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
            view.session_kind = next.kind.clone();
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
            view.session_kind = prev.kind.clone();
        }
        self.pane_content.clear();
    }
}
