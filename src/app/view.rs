use anyhow::Result;

use super::*;
use crate::tmux::TmuxManager;

/// Result of the background scan started by
/// [`App::open_latest_prompt_from_view`], delivered through
/// `App::latest_prompt_menu_bg`.
pub(crate) struct LatestPromptScanResult {
    view: ViewState,
    prompts: Vec<crate::app::util::PromptEntry>,
}

impl App {
    fn feature_workdir_for_view(&self, view: &ViewState) -> Option<PathBuf> {
        self.store
            .projects
            .iter()
            .find(|project| project.name == view.project_name)
            .and_then(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.name == view.feature_name)
            })
            .map(|feature| feature.workdir.clone())
    }

    fn feature_markdown_context(
        &self,
        from_view: Option<&ViewState>,
    ) -> Option<(PathBuf, Option<PathBuf>)> {
        let workdir = if let Some(view) = from_view {
            self.feature_workdir_for_view(view)?
        } else {
            self.selected_feature()
                .map(|(_, feature)| feature.workdir.clone())?
        };

        let repo_root = self
            .worktree
            .repo_root(&workdir)
            .ok()
            .filter(|root| root != &workdir);

        Some((workdir, repo_root))
    }

    pub fn enter_view(&mut self) -> Result<()> {
        self.enter_view_with_options(true)
    }

    pub(crate) fn enter_view_without_auto_compose(&mut self) -> Result<()> {
        self.enter_view_with_options(false)
    }

    fn enter_view_with_options(&mut self, auto_compose: bool) -> Result<()> {
        // Opening a stopped feature starts it, which launches its saved
        // agents — the same claim on the machine that `c` makes, so it asks
        // the same question.
        self.enter_view_gated(
            auto_compose,
            StartIntent::Ask(PendingStart::EnterView { auto_compose }),
        )
    }

    /// Replay of [`Self::enter_view_with_options`] after the user answered the
    /// resource confirmation.
    pub(crate) fn enter_view_approved(&mut self, auto_compose: bool) -> Result<()> {
        self.enter_view_gated(auto_compose, StartIntent::Approved)
    }

    fn enter_view_gated(&mut self, auto_compose: bool, intent: StartIntent) -> Result<()> {
        let (pi, fi, target_si) = match &self.selection {
            Selection::Session(pi, fi, si) => (*pi, *fi, Some(*si)),
            Selection::Feature(pi, fi) => (*pi, *fi, None),
            _ => return Ok(()),
        };

        // TODOs sessions open a native overlay rather than a tmux pane (there
        // is no tmux window to attach to).
        let is_todos = target_si
            .and_then(|si| {
                self.store
                    .projects
                    .get(pi)
                    .and_then(|p| p.features.get(fi))
                    .and_then(|f| f.sessions.get(si))
            })
            .is_some_and(|s| s.kind == SessionKind::Todos);
        if is_todos {
            return self.open_todos_view(pi, fi);
        }

        if self.block_if_feature_pending_worktree_script(pi, fi) {
            return Ok(());
        }

        let feature_was_stopped = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .is_some_and(|feature| feature.status == ProjectStatus::Stopped);

        if self.ensure_feature_running(pi, fi, intent)? == Started::Parked {
            // The confirmation dialog owns the screen now; it replays this
            // call if the user says yes.
            return Ok(());
        }

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
                            SessionKind::Claude
                                | SessionKind::Opencode
                                | SessionKind::Codex
                                | SessionKind::Pi
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

        // Clear-on-open, for harnesses that will never tell us they resumed.
        // Harnesses AMF can observe thinking for clear themselves in
        // `sync_thinking_status` when output actually resumes, which is the
        // more accurate signal — opening a session is not the same as dealing
        // with it, so we only fall back to it where nothing better exists.
        if self.store.projects[pi]
            .features
            .get(fi)
            .map(|feature| crate::app::attention::HarnessCapabilities::for_agent(&feature.agent))
            .is_some_and(|capabilities| capabilities.clears_on_open())
        {
            let session = tmux_session.clone();
            self.clear_attention(&session);
        }

        let pending_project_name = project_name.clone();
        let pending_feature_name = feature_name.clone();
        let mut view = ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            session_kind,
            vibe_mode,
            review,
        );
        if feature_was_stopped && view.session_kind.is_agent_harness() {
            view.show_startup_mask();
        }

        self.save()?;
        self.pane_content.clear();

        self.mode = AppMode::Viewing(view);
        self.refresh_sidebar_for_current_view();

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

        if auto_compose
            && let AppMode::Viewing(view) = &self.mode
            && self.compose_intercept_active(view)
        {
            self.open_compose_from_view(None)?;
        }

        Ok(())
    }

    pub(crate) fn exit_view_without_resuming_plan_interview(&mut self) {
        self.pane_content.clear();
        self.tmux_cursor = None;
        self.mode = AppMode::Normal;
        self.message = Some("Returned to dashboard".into());
    }

    pub fn exit_view(&mut self) {
        self.exit_view_without_resuming_plan_interview();
        self.resume_paused_plan_interview();
    }

    pub fn open_latest_prompt_from_view(&mut self) {
        if self.latest_prompt_menu_bg.is_some() {
            self.message = Some("Already loading latest prompts...".into());
            return;
        }

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
                // The session this window is actually running, not just any
                // session sharing the workdir — two harness windows in the
                // same worktree write their transcripts into the same
                // on-disk directory, and without this the scan below could
                // surface (and let the user inject) another session's
                // prompts.
                let preferred_session_id = feature
                    .sessions
                    .iter()
                    .find(|session| session.tmux_window == view.window)
                    .and_then(super::session_ops::persisted_resume_id);

                (
                    feature.workdir.clone(),
                    view.session_kind.clone(),
                    preferred_session_id,
                )
            });

        let Some((workdir, session_kind, preferred_session_id)) = feature_prompt_context else {
            self.mode = AppMode::Viewing(view);
            self.message = Some("Error: Could not resolve feature workdir".into());
            return;
        };

        let (tx, rx) = std::sync::mpsc::channel();
        self.latest_prompt_menu_bg = Some(rx);
        let result_view = view.clone();
        std::thread::spawn(move || {
            // Reading and parsing every transcript file for a session can be
            // slow (large or numerous histories), so it happens off the UI
            // thread; the result is still the complete list, just delivered
            // once `poll_latest_prompt_menu_bg` picks it up.
            let prompts = crate::app::util::read_all_prompts_for_session(
                &workdir,
                Some(&session_kind),
                preferred_session_id.as_deref(),
            );
            let _ = tx.send(LatestPromptScanResult {
                view: result_view,
                prompts,
            });
        });
        self.mode = AppMode::Viewing(view);
        self.message = Some("Loading latest prompts...".into());
    }

    /// Drain the background "all prompts" scan started by
    /// [`Self::open_latest_prompt_from_view`]. Called from the main loop
    /// beside the other `poll_*_bg` calls; returns true when the UI should
    /// redraw.
    pub fn poll_latest_prompt_menu_bg(&mut self) -> bool {
        let Some(rx) = self.latest_prompt_menu_bg.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.latest_prompt_menu_bg = None;
                // The user may have exited this view (or switched windows)
                // while the scan was running; only land the menu if they're
                // still looking at the same one.
                let still_here = matches!(
                    &self.mode,
                    AppMode::Viewing(current)
                        if current.session == result.view.session
                            && current.window == result.view.window
                );
                if still_here {
                    self.mode = AppMode::LatestPrompt(LatestPromptState {
                        prompts: result.prompts,
                        selected: 0,
                        view: result.view,
                    });
                    self.message = None;
                } else {
                    self.log_debug(
                        "latest_prompt",
                        "discarding latest-prompt scan result: view changed during load"
                            .to_string(),
                    );
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.latest_prompt_menu_bg = None;
                false
            }
        }
    }

    pub fn toggle_expanded_todos_in_view(&mut self) {
        if let AppMode::Viewing(view) = &mut self.mode {
            view.todos_expanded = !view.todos_expanded;
            self.message = Some(if view.todos_expanded {
                "Expanded todos".into()
            } else {
                "Collapsed todos".into()
            });
        }
    }

    pub fn toggle_sidebar_in_view(&mut self) {
        let mut sidebar_shown = false;
        if let AppMode::Viewing(view) = &mut self.mode {
            view.sidebar_visible = !view.sidebar_visible;
            if !view.sidebar_visible {
                view.todos_expanded = false;
            } else {
                sidebar_shown = true;
            }
            self.message = Some(if view.sidebar_visible {
                "Showed sidebar".into()
            } else {
                "Hid sidebar".into()
            });
        }
        if sidebar_shown {
            self.refresh_sidebar_for_current_view();
        }
    }

    /// Target for the periodic re-anchor bounce: `(session, window,
    /// content_cols, content_rows)` for the live pane, but only for
    /// Claude sessions, whose incremental renderer drifts its input-box
    /// anchor and leaves stale cells in the real tmux grid. Other
    /// harnesses fully repaint and never need this, so they are excluded
    /// to avoid the bounce's flicker. Returns `None` unless a Claude pane
    /// is live and sized.
    pub fn reanchor_bounce_target(&self) -> Option<(String, String, u16, u16)> {
        let view = match &self.mode {
            AppMode::Viewing(view) => view,
            AppMode::Compose(state) => &state.view,
            _ => return None,
        };
        if view.session_kind != crate::project::SessionKind::Claude {
            return None;
        }
        let content_cols = crate::ui::viewing_main_width(view, self.viewport_cols);
        let content_rows = self.viewport_total_rows.saturating_sub(1);
        if content_cols == 0 || content_rows <= 1 {
            return None;
        }
        Some((
            view.session.clone(),
            view.window.clone(),
            content_cols,
            content_rows,
        ))
    }

    pub fn refresh_view_sizing(&mut self) -> Result<()> {
        // Must match the sizing in the main loop: view content area is
        // the full terminal minus the 1-row header.
        let viewport_cols = self.viewport_cols;
        let content_rows = self.viewport_total_rows.saturating_sub(1);
        if viewport_cols == 0 || content_rows == 0 {
            self.push_toast_warning("Sizing unavailable");
            return Ok(());
        }

        let (session, window, content_cols) = match &self.mode {
            AppMode::Viewing(view) => (
                view.session.clone(),
                view.window.clone(),
                crate::ui::viewing_main_width(view, viewport_cols),
            ),
            _ => return Ok(()),
        };

        // Bounce the pane height by one row and back: the agent gets a
        // SIGWINCH pair and performs a full repaint. This recovers from
        // agent-side renderer desync (incremental updates drawn at a
        // stale anchor row), which AMF cannot fix by re-capturing — the
        // corruption is in the pane grid itself.
        self.tmux.resize_pane(
            &session,
            &window,
            content_cols,
            content_rows.saturating_sub(1).max(1),
        )?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.tmux
            .resize_pane(&session, &window, content_cols, content_rows)?;
        self.push_toast_success("Repainted agent pane");
        Ok(())
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

    /// Open the prompt editor seeded with the selected past prompt, to save
    /// it as a new `User` template. Returns to the session view on save or
    /// cancel.
    pub fn save_latest_prompt_as_template(&mut self) {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::LatestPrompt(state) => state,
            other => {
                self.mode = other;
                return;
            }
        };

        let text = state
            .prompts
            .get(state.selected)
            .map(|e| e.text.trim().to_string())
            .filter(|p| !p.is_empty());

        let Some(text) = text else {
            self.mode = AppMode::LatestPrompt(state);
            self.message = Some("No prompt to save".into());
            return;
        };

        let dest_path = self.template_source_path(crate::prompt_library::PromptSource::User, None);
        self.mode = AppMode::PromptEditor(crate::app::PromptEditorState {
            editing_id: None,
            editing_source: crate::prompt_library::PromptSource::User,
            original_template: None,
            name: String::new(),
            tags: String::new(),
            focus: crate::app::PromptEditorFocus::Name,
            editor: crate::editor::TextEditor::with_vim(text),
            return_to: Box::new(AppMode::Viewing(state.view)),
            dest_path,
        });
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

    /// Resolve the Remote Control session URL from the current Claude pane,
    /// or set a helpful message and return `None`. Only meaningful while
    /// viewing a Claude session.
    fn remote_control_url_from_view(&mut self) -> Option<String> {
        match &self.mode {
            AppMode::Viewing(view) if view.session_kind == SessionKind::Claude => {}
            _ => {
                self.push_toast_warning("Remote Control: not a Claude session");
                return None;
            }
        }
        let status = crate::app::remote_control::detect_remote_control(&self.pane_content);
        match status.url {
            Some(url) => Some(url),
            None if status.active => {
                self.push_toast_warning(
                    "Remote Control active — open the /rc link in the pane footer",
                );
                None
            }
            None => {
                self.push_toast_warning("Remote Control not active in this session");
                None
            }
        }
    }

    pub fn copy_remote_control_url(&mut self) -> Result<()> {
        let Some(url) = self.remote_control_url_from_view() else {
            return Ok(());
        };
        match crate::app::util::copy_to_clipboard(&url) {
            Ok(()) => self.push_toast_success("Copied Remote Control URL"),
            Err(e) => self.push_toast_error(format!("Clipboard error: {e}")),
        }
        Ok(())
    }

    pub fn open_remote_control_url(&mut self) -> Result<()> {
        let Some(url) = self.remote_control_url_from_view() else {
            return Ok(());
        };
        match crate::app::util::open_in_browser(&url) {
            Ok(()) => self.push_toast_success("Opened Remote Control URL"),
            Err(e) => self.push_toast_error(format!("Open failed: {e}")),
        }
        Ok(())
    }

    /// Toggle Remote Control on the focused Claude session by sending `/rc`.
    /// Sent straight to the tmux pane (bypassing the AMF composer), mirroring
    /// how the compose path submits slash commands.
    pub fn toggle_remote_control_in_view(&mut self) -> Result<()> {
        let (session, window) = match &self.mode {
            AppMode::Viewing(v) if v.session_kind == SessionKind::Claude => {
                (v.session.clone(), v.window.clone())
            }
            AppMode::Viewing(_) => {
                self.push_toast_warning("Remote Control: not a Claude session");
                return Ok(());
            }
            _ => return Ok(()),
        };

        if let Some(reason) =
            crate::claude::ClaudeLauncher::remote_control_block_reason(self.config.zai.is_some())
        {
            self.push_toast_warning(format!("Remote Control {reason}"));
            return Ok(());
        }

        // Clear any leftover input so `/rc` cannot merge with typed text.
        self.tmux.send_key_name(&session, &window, "C-u")?;
        self.tmux.send_literal(&session, &window, "/rc")?;
        self.tmux.send_key_name(&session, &window, "Enter")?;
        self.push_toast_info("Sent /rc to toggle Remote Control");
        Ok(())
    }

    pub fn latest_prompt_select_next(&mut self) {
        if let AppMode::LatestPrompt(state) = &mut self.mode
            && !state.prompts.is_empty()
            && state.selected + 1 < state.prompts.len()
        {
            state.selected += 1;
        }
    }

    pub fn latest_prompt_select_prev(&mut self) {
        if let AppMode::LatestPrompt(state) = &mut self.mode
            && state.selected > 0
        {
            state.selected -= 1;
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

        self.mode = AppMode::MarkdownLoading(crate::app::MarkdownLoadingState {
            title: "Finding markdown files...".into(),
            from_view: Some(view.clone()),
            operation: crate::app::MarkdownLoadingOperation::DiscoverFromView { view },
        });
        self.message = None;
        Ok(())
    }

    /// Open this feature's effective plan, or choose one from Markdown files
    /// contained by its worktree when neither the conventional nor persisted
    /// path is currently valid.
    pub fn open_current_plan_from_view(&mut self) -> Result<()> {
        let view = match &self.mode {
            AppMode::Viewing(view) if view.session_kind.is_agent_harness() => view.clone(),
            AppMode::Viewing(_) => {
                self.push_toast_warning("Current plans are available in agent sessions");
                return Ok(());
            }
            _ => return Ok(()),
        };

        let Some((feature_id, workdir, effective_plan)) =
            self.store.projects.iter().find_map(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.tmux_session == view.session)
                    .map(|feature| {
                        (
                            feature.id.clone(),
                            feature.workdir.clone(),
                            crate::app::plan::resolve_effective_plan(feature),
                        )
                    })
            })
        else {
            self.push_toast_warning("Could not resolve the current feature");
            return Ok(());
        };

        if let Some(plan) = effective_plan {
            return self.open_markdown_viewer_path(
                plan.path().to_path_buf(),
                workdir,
                None,
                view,
                None,
                true,
            );
        }

        self.mode = AppMode::MarkdownLoading(crate::app::MarkdownLoadingState {
            title: "Finding worktree plans...".into(),
            from_view: Some(view.clone()),
            operation: crate::app::MarkdownLoadingOperation::DiscoverPlan { view, feature_id },
        });
        self.message = None;
        Ok(())
    }

    pub fn open_markdown_file_picker_from_viewer(&mut self) -> Result<()> {
        let viewer = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::MarkdownViewer(viewer) => viewer,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let Some(view) = viewer.from_view.clone() else {
            self.mode = AppMode::MarkdownViewer(viewer);
            self.message = Some("Error: Could not resolve feature workdir".into());
            return Ok(());
        };

        self.mode = AppMode::MarkdownLoading(crate::app::MarkdownLoadingState {
            title: "Finding markdown files...".into(),
            from_view: Some(view),
            operation: crate::app::MarkdownLoadingOperation::DiscoverFromViewer { viewer },
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
        current_plan: bool,
    ) -> Result<()> {
        self.mode = AppMode::MarkdownLoading(crate::app::MarkdownLoadingState {
            title: format!("Loading {}...", path.display()),
            from_view: Some(view.clone()),
            operation: crate::app::MarkdownLoadingOperation::ReadPath {
                path,
                workdir,
                repo_root,
                view,
                return_to_picker,
                current_plan,
            },
        });
        self.message = None;
        Ok(())
    }

    pub fn markdown_loading(&self) -> bool {
        matches!(self.mode, AppMode::MarkdownLoading(_))
    }

    pub fn complete_markdown_loading(&mut self) {
        let loading = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::MarkdownLoading(loading) => loading,
            other => {
                self.mode = other;
                return;
            }
        };

        match loading.operation {
            crate::app::MarkdownLoadingOperation::DiscoverFromView { view } => {
                self.complete_markdown_discovery_from_view(view);
            }
            crate::app::MarkdownLoadingOperation::DiscoverFromViewer { viewer } => {
                self.complete_markdown_discovery_from_viewer(viewer);
            }
            crate::app::MarkdownLoadingOperation::DiscoverPlan { view, feature_id } => {
                self.complete_plan_markdown_discovery(view, feature_id);
            }
            crate::app::MarkdownLoadingOperation::ReadPath {
                path,
                workdir,
                repo_root,
                view,
                return_to_picker,
                current_plan,
            } => {
                self.complete_markdown_read_path(
                    path,
                    workdir,
                    repo_root,
                    view,
                    return_to_picker,
                    current_plan,
                );
            }
        }
    }

    pub fn cancel_markdown_loading(&mut self) {
        let loading = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::MarkdownLoading(loading) => loading,
            other => {
                self.mode = other;
                return;
            }
        };

        self.mode = match loading.operation {
            crate::app::MarkdownLoadingOperation::DiscoverFromView { view } => {
                AppMode::Viewing(view)
            }
            crate::app::MarkdownLoadingOperation::DiscoverFromViewer { viewer } => {
                AppMode::MarkdownViewer(viewer)
            }
            crate::app::MarkdownLoadingOperation::DiscoverPlan { view, .. } => {
                AppMode::Viewing(view)
            }
            crate::app::MarkdownLoadingOperation::ReadPath {
                view,
                return_to_picker,
                ..
            } => return_to_picker
                .map(AppMode::MarkdownFilePicker)
                .unwrap_or(AppMode::Viewing(view)),
        };
    }

    fn complete_markdown_discovery_from_view(&mut self, view: ViewState) {
        let Some((workdir, repo_root)) = self.feature_markdown_context(Some(&view)) else {
            self.mode = AppMode::Viewing(view);
            self.message = Some("Error: Could not resolve feature workdir".into());
            return;
        };

        let files = crate::markdown::collect_markdown_view_paths(&workdir, repo_root.as_deref());
        if files.is_empty() {
            self.mode = AppMode::Viewing(view);
            self.message = Some(
                "Error: No markdown file found (.claude/*.md or top-level *.md in the worktree/repo root)"
                    .into(),
            );
            return;
        }

        if files.len() == 1 {
            self.complete_markdown_read_path(
                files[0].clone(),
                workdir,
                repo_root,
                view,
                None,
                false,
            );
            return;
        }

        self.mode = AppMode::MarkdownFilePicker(crate::app::MarkdownFilePickerState {
            files,
            selected: 0,
            plan_only: true,
            search_active: false,
            query: String::new(),
            workdir,
            repo_root,
            purpose: crate::app::MarkdownFilePickerPurpose::Browse,
            from_view: Some(view),
        });
        self.message = None;
    }

    fn complete_markdown_discovery_from_viewer(&mut self, viewer: crate::app::MarkdownViewerState) {
        let Some(view) = viewer.from_view.clone() else {
            self.mode = AppMode::MarkdownViewer(viewer);
            self.message = Some("Error: Could not resolve feature workdir".into());
            return;
        };

        let Some((workdir, repo_root)) = self.feature_markdown_context(Some(&view)) else {
            self.mode = AppMode::MarkdownViewer(viewer);
            self.message = Some("Error: Could not resolve feature workdir".into());
            return;
        };

        let files = crate::markdown::collect_markdown_view_paths(&workdir, repo_root.as_deref());
        if files.is_empty() {
            self.mode = AppMode::MarkdownViewer(viewer);
            self.message = Some(
                "Error: No markdown file found (.claude/*.md or top-level *.md in the worktree/repo root)"
                    .into(),
            );
            return;
        }

        let selected = files
            .iter()
            .position(|path| path == &viewer.source_path)
            .unwrap_or(0);

        self.mode = AppMode::MarkdownFilePicker(crate::app::MarkdownFilePickerState {
            files,
            selected,
            plan_only: true,
            search_active: false,
            query: String::new(),
            workdir,
            repo_root,
            purpose: crate::app::MarkdownFilePickerPurpose::Browse,
            from_view: Some(view),
        });
        self.message = None;
    }

    fn complete_plan_markdown_discovery(&mut self, view: ViewState, feature_id: String) {
        let Some(workdir) = self
            .store
            .projects
            .iter()
            .flat_map(|project| &project.features)
            .find(|feature| feature.id == feature_id)
            .map(|feature| feature.workdir.clone())
        else {
            self.mode = AppMode::Viewing(view);
            self.push_toast_warning("Could not resolve the current feature");
            return;
        };

        let mut files = crate::markdown::collect_markdown_view_paths(&workdir, None)
            .into_iter()
            .filter_map(|path| crate::app::plan::validate_selected_plan_path(&workdir, &path).ok())
            .collect::<Vec<_>>();
        files.sort();
        files.dedup();

        if files.is_empty() {
            self.mode = AppMode::Viewing(view);
            self.push_toast_warning("No Markdown plan is available in this worktree");
            return;
        }

        self.mode = AppMode::MarkdownFilePicker(crate::app::MarkdownFilePickerState {
            files,
            selected: 0,
            plan_only: false,
            search_active: false,
            query: String::new(),
            workdir,
            repo_root: None,
            purpose: crate::app::MarkdownFilePickerPurpose::SelectPlan { feature_id },
            from_view: Some(view),
        });
        self.message = None;
    }

    pub(crate) fn persist_selected_plan_path(
        &mut self,
        feature_id: &str,
        workdir: &Path,
        candidate: &Path,
    ) -> Result<PathBuf> {
        let canonical = crate::app::plan::validate_selected_plan_path(workdir, candidate)
            .map_err(|error| anyhow::anyhow!("invalid selected plan: {error:?}"))?;
        let feature = self
            .store
            .projects
            .iter_mut()
            .flat_map(|project| &mut project.features)
            .find(|feature| feature.id == feature_id)
            .ok_or_else(|| anyhow::anyhow!("feature no longer exists"))?;
        feature.selected_plan_path = Some(canonical.clone());
        self.save()?;
        Ok(canonical)
    }

    fn complete_markdown_read_path(
        &mut self,
        path: PathBuf,
        workdir: PathBuf,
        repo_root: Option<PathBuf>,
        view: ViewState,
        return_to_picker: Option<crate::app::MarkdownFilePickerState>,
        current_plan: bool,
    ) {
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => {
                self.mode = AppMode::Viewing(view);
                self.message = Some(format!("Error: Failed to read {}: {err}", path.display()));
                return;
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
            current_plan,
        });
        self.message = None;
    }

    /// Explicit viewer refresh: edits are re-read in place. If the current
    /// plan was moved or deleted, close the stale snapshot, clear a matching
    /// manual selection, and return to the originating agent session.
    pub fn refresh_markdown_viewer(&mut self) -> Result<()> {
        let (path, current_plan) = match &self.mode {
            AppMode::MarkdownViewer(state) => (state.source_path.clone(), state.current_plan),
            _ => return Ok(()),
        };

        match std::fs::read_to_string(&path) {
            Ok(content) => {
                if let AppMode::MarkdownViewer(state) = &mut self.mode {
                    state.content = content;
                    state.rendered_width = 0;
                    state.rendered_lines.clear();
                }
                self.push_toast_success("Markdown refreshed");
            }
            Err(error) if current_plan => {
                let from_view = match std::mem::replace(&mut self.mode, AppMode::Normal) {
                    AppMode::MarkdownViewer(state) => state.from_view,
                    other => {
                        self.mode = other;
                        return Ok(());
                    }
                };
                if let Some(view) = from_view {
                    if let Some(feature) = self
                        .store
                        .projects
                        .iter_mut()
                        .flat_map(|project| &mut project.features)
                        .find(|feature| feature.tmux_session == view.session)
                        && feature.selected_plan_path.as_deref() == Some(path.as_path())
                    {
                        feature.selected_plan_path = None;
                        self.save()?;
                    }
                    self.mode = AppMode::Viewing(view);
                }
                self.push_toast_warning(format!("Current plan is no longer available: {error}"));
            }
            Err(error) => {
                self.push_toast_error(format!("Could not refresh Markdown: {error}"));
            }
        }
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
        // Jumping to a stopped feature starts it. Remember the view being left
        // so cancelling puts the user back where they were instead of on the
        // dashboard.
        let from_view = match &self.mode {
            AppMode::Viewing(view) => Some(view.clone()),
            _ => None,
        };
        let started = self.ensure_feature_running(
            pi,
            fi,
            StartIntent::Ask(PendingStart::SwitchViewToFeature { pi, fi }),
        )?;
        if started == Started::Parked {
            self.set_resource_confirm_return_view(from_view);
            return Ok(());
        }

        self.switch_view_to_feature_started(pi, fi)
    }

    /// Replay of [`Self::switch_view_to_feature`] after the user answered the
    /// resource confirmation.
    pub(crate) fn switch_view_to_feature_approved(&mut self, pi: usize, fi: usize) -> Result<()> {
        self.ensure_feature_running(pi, fi, StartIntent::Approved)?;
        self.switch_view_to_feature_started(pi, fi)
    }

    /// Attach the view to `(pi, fi)`, whose session is already up.
    fn switch_view_to_feature_started(&mut self, pi: usize, fi: usize) -> Result<()> {
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
        self.refresh_sidebar_for_current_view();
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
        // Only cycle tmux-backed sessions; native ones (TODOs) have no pane.
        let tmux_indices: Vec<usize> = feature
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind.is_tmux_backed())
            .map(|(i, _)| i)
            .collect();
        if tmux_indices.len() <= 1 {
            return;
        }

        let current_pos = tmux_indices
            .iter()
            .position(|&i| feature.sessions[i].tmux_window == current_window)
            .unwrap_or(0);
        let next_si = tmux_indices[(current_pos + 1) % tmux_indices.len()];
        let next = &feature.sessions[next_si];

        if let AppMode::Viewing(ref mut view) = self.mode {
            view.window = next.tmux_window.clone();
            view.session_label = next.label.clone();
            view.session_kind = next.kind.clone();
        }
        self.pane_content.clear();
        self.refresh_sidebar_for_current_view();
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
        // Only cycle tmux-backed sessions; native ones (TODOs) have no pane.
        let tmux_indices: Vec<usize> = feature
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind.is_tmux_backed())
            .map(|(i, _)| i)
            .collect();
        if tmux_indices.len() <= 1 {
            return;
        }

        let current_pos = tmux_indices
            .iter()
            .position(|&i| feature.sessions[i].tmux_window == current_window)
            .unwrap_or(0);
        let prev_si = if current_pos == 0 {
            tmux_indices[tmux_indices.len() - 1]
        } else {
            tmux_indices[current_pos - 1]
        };
        let prev = &feature.sessions[prev_si];

        if let AppMode::Viewing(ref mut view) = self.mode {
            view.window = prev.tmux_window.clone();
            view.session_label = prev.label.clone();
            view.session_kind = prev.kind.clone();
        }
        self.pane_content.clear();
        self.refresh_sidebar_for_current_view();
    }
}
