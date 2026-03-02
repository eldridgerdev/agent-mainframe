use anyhow::Result;
use std::path::PathBuf;

use super::*;
use super::setup::{ensure_notification_hooks, ensure_review_claude_md};
use crate::extension::{load_global_extension_config, merge_project_extension_config};
use crate::tmux::TmuxManager;
use crate::worktree::WorktreeManager;

impl App {
    pub fn start_create_feature(&mut self) {
        let (project_name, project_repo, is_first, used_workdirs) =
            match &self.selection {
                Selection::Project(pi)
                | Selection::Feature(pi, _)
                | Selection::Session(pi, _, _) => {
                    if let Some(p) =
                        self.store.projects.get(*pi)
                    {
                        let used: Vec<PathBuf> = p
                            .features
                            .iter()
                            .map(|f| f.workdir.clone())
                            .collect();
                        (
                            p.name.clone(),
                            p.repo.clone(),
                            p.features.is_empty(),
                            used,
                        )
                    } else {
                        return;
                    }
                }
            };

        let worktrees = WorktreeManager::list(&project_repo)
            .unwrap_or_default()
            .into_iter()
            .filter(|wt| {
                wt.path != project_repo
                    && !used_workdirs.contains(&wt.path)
            })
            .collect();

        self.mode = AppMode::CreatingFeature(
            CreateFeatureState::new(
                project_name,
                project_repo,
                worktrees,
                is_first,
            ),
        );
        self.message = None;
    }

    pub fn create_feature(&mut self) -> Result<()> {
        let state = match &self.mode {
            AppMode::CreatingFeature(s) => s,
            _ => return Ok(()),
        };

        let project_name = state.project_name.clone();
        let project_repo = state.project_repo.clone();
        let branch = state.branch.clone();
        let mode = state.mode.clone();
        let review = state.review;
        let use_existing_worktree = state.source_index == 1
            && !state.worktrees.is_empty();
        let selected_worktree = if use_existing_worktree {
            state.worktrees.get(state.worktree_index).cloned()
        } else {
            None
        };
        let use_worktree = state.use_worktree;
        let enable_chrome = state.enable_chrome;
        let enable_notes = state.enable_notes;

        if branch.is_empty() {
            self.message =
                Some("Error: Branch name cannot be empty".into());
            return Ok(());
        }

        let stored_is_git = {
            let project =
                match self.store.find_project(&project_name) {
                    Some(p) => p,
                    None => {
                        self.message = Some(format!(
                            "Error: Project '{}' not found",
                            project_name
                        ));
                        return Ok(());
                    }
                };

            if project
                .features
                .iter()
                .any(|f| f.name == branch)
            {
                self.message = Some(format!(
                    "Error: Feature '{}' already exists in '{}'",
                    branch, project_name
                ));
                return Ok(());
            }

            if !use_worktree
                && selected_worktree.is_none()
                && project
                    .features
                    .iter()
                    .any(|f| !f.is_worktree)
            {
                self.message = Some(
                    "Error: Only one non-worktree feature \
                     allowed per project"
                        .into(),
                );
                return Ok(());
            }

            project.is_git
        };

        let is_git = stored_is_git
            || self.worktree.repo_root(&project_repo).is_ok();

        if is_git && !stored_is_git {
            if let Some(p) =
                self.store.find_project_mut(&project_name)
            {
                p.is_git = true;
            }
            self.save()?;
        }

        if (use_worktree || selected_worktree.is_some())
            && !is_git
        {
            self.message = Some(
                "Error: Worktrees require a git repository"
                    .into(),
            );
            return Ok(());
        }

        let (workdir, is_worktree) =
            if let Some(wt) = &selected_worktree {
                (wt.path.clone(), true)
            } else if use_worktree {
                let wt_path = self.worktree.create(
                    &project_repo,
                    &branch,
                    &branch,
                )?;

                let global_ext = load_global_extension_config();
                let ext = merge_project_extension_config(&global_ext, &project_repo);

                if let Some(ref hook_cfg) = ext.lifecycle_hooks.on_worktree_created {
                    if let Some(prompt) = hook_cfg.prompt() {
                        self.start_hook_prompt(
                            hook_cfg.script().to_string(),
                            wt_path.clone(),
                            prompt.title.clone(),
                            prompt.options.clone(),
                            HookNext::WorktreeCreated {
                                project_name,
                                branch,
                                mode,
                                review,
                                agent: state.agent.clone(),
                                enable_chrome,
                                enable_notes,
                            },
                        );
                    } else {
                        self.start_worktree_hook(
                            hook_cfg.script(),
                            wt_path.clone(),
                            project_name,
                            branch,
                            mode,
                            review,
                            state.agent.clone(),
                            enable_chrome,
                            enable_notes,
                            None,
                        );
                    }
                    return Ok(());
                }

                (wt_path, true)
            } else {
                (project_repo.clone(), false)
            };

        if enable_notes {
            let claude_dir = workdir.join(".claude");
            if !claude_dir.exists() {
                let _ = std::fs::create_dir_all(&claude_dir);
            }
            let notes_path = claude_dir.join("notes.md");
            if !notes_path.exists() {
                let _ = std::fs::write(
                    &notes_path,
                    "# Notes\n\nWrite instructions for Claude here.\n",
                );
            }
        }

        let feature = Feature::new(
            branch.clone(),
            branch.clone(),
            workdir,
            is_worktree,
            mode,
            review,
            state.agent.clone(),
            enable_chrome,
            enable_notes,
        );

        self.store.add_feature(&project_name, feature);
        self.save()?;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            let fi = self.store.projects[pi]
                .features
                .len()
                .saturating_sub(1);
            self.store.projects[pi].collapsed = false;
            self.selection = Selection::Feature(pi, fi);
        }

        self.mode = AppMode::Normal;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            let fi = self.store.projects[pi]
                .features
                .len()
                .saturating_sub(1);
            self.ensure_feature_running(pi, fi)?;
            self.save()?;
        }

        self.message = Some(format!(
            "Created and started feature '{}'",
            branch
        ));

        Ok(())
    }

    pub(crate) fn ensure_feature_running(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        let repo =
            self.store.projects[pi].repo.clone();
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
            let session_kind = match feature.agent {
                AgentKind::Claude => SessionKind::Claude,
                AgentKind::Opencode => SessionKind::Opencode,
            };
            feature.add_session(session_kind);
            feature.add_session(SessionKind::Terminal);
            if feature.has_notes {
                let s = feature.add_session(SessionKind::Nvim);
                s.label = "Memo".into();
            }
        }

        if self.tmux.session_exists(&feature.tmux_session) {
            return Ok(());
        }

        self.tmux.create_session_with_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
            &feature.workdir,
        )?;
        self.tmux.set_session_env(
            &feature.tmux_session,
            "AMF_SESSION",
            &feature.tmux_session,
        )?;

        for session in &feature.sessions[1..] {
            self.tmux.create_window(
                &feature.tmux_session,
                &session.tmux_window,
                &feature.workdir,
            )?;
        }

        let extra_args: Vec<String> =
            feature.mode.cli_flags(feature.enable_chrome);
        for session in &feature.sessions {
            match session.kind {
                SessionKind::Claude => {
                    self.tmux.launch_claude(
                        &feature.tmux_session,
                        &session.tmux_window,
                        session.claude_session_id.clone(),
                        extra_args.clone(),
                    )?;
                }
                SessionKind::Opencode => {
                    self.tmux.launch_opencode(
                        &feature.tmux_session,
                        &session.tmux_window,
                    )?;
                }
                SessionKind::Nvim => {
                    if feature.has_notes {
                        self.tmux.send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim .claude/notes.md",
                        )?;
                    } else {
                        self.tmux.send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim",
                        )?;
                    }
                }
                SessionKind::Terminal => {}
                SessionKind::Vscode => {
                    self.tmux.send_keys(
                        &feature.tmux_session,
                        &session.tmux_window,
                        &format!("code {}", feature.workdir.display()),
                    )?;
                }
                SessionKind::Custom => {
                    let status_dir = feature
                        .workdir
                        .join(".amf")
                        .join("session-status");
                    let _ = std::fs::create_dir_all(
                        &status_dir,
                    );
                    let export_cmd = format!(
                        "export AMF_SESSION_ID='{}' \
                         AMF_STATUS_DIR='{}'",
                        session.id,
                        status_dir.display(),
                    );
                    self.tmux.send_literal(
                        &feature.tmux_session,
                        &session.tmux_window,
                        &export_cmd,
                    )?;
                    self.tmux.send_key_name(
                        &feature.tmux_session,
                        &session.tmux_window,
                        "Enter",
                    )?;
                    if let Some(ref cmd) = session.command {
                        self.tmux.send_literal(
                            &feature.tmux_session,
                            &session.tmux_window,
                            cmd,
                        )?;
                        self.tmux.send_key_name(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "Enter",
                        )?;
                    }
                }
            }
        }

        self.tmux.select_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
        )?;

        feature.status = ProjectStatus::Idle;
        feature.touch();

        Ok(())
    }

    pub fn start_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let status = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.status.clone());

        if status != Some(ProjectStatus::Stopped) {
            if let Some(name) = self
                .store
                .projects
                .get(pi)
                .and_then(|p| p.features.get(fi))
                .map(|f| f.name.clone())
            {
                self.message = Some(format!(
                    "Error: '{}' is already running",
                    name
                ));
            }
            return Ok(());
        }

        // If on_start has a prompt, show the picker first.
        let on_start =
            self.active_extension.lifecycle_hooks.on_start.clone();
        if let Some(ref cfg) = on_start {
            if let Some(prompt) = cfg.prompt() {
                let workdir = self
                    .store
                    .projects
                    .get(pi)
                    .and_then(|p| p.features.get(fi))
                    .map(|f| f.workdir.clone())
                    .unwrap_or_default();
                self.start_hook_prompt(
                    cfg.script().to_string(),
                    workdir,
                    prompt.title.clone(),
                    prompt.options.clone(),
                    HookNext::StartFeature { pi, fi },
                );
                return Ok(());
            }
        }

        self.ensure_feature_running(pi, fi)?;

        // Fire on_start lifecycle hook (plain script) if configured.
        if let Some(ref cfg) = on_start {
            let workdir = self
                .store
                .projects
                .get(pi)
                .and_then(|p| p.features.get(fi))
                .map(|f| f.workdir.clone())
                .unwrap_or_default();
            self.run_lifecycle_hook(cfg.script(), &workdir, None);
        }

        let name = self.store.projects[pi].features[fi]
            .name
            .clone();
        self.save()?;
        self.message = Some(format!("Started '{}'", name));

        Ok(())
    }

    /// Inner start logic called after a hook prompt is confirmed.
    pub fn do_start_feature(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        self.ensure_feature_running(pi, fi)?;
        let name = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.name.clone())
            .unwrap_or_default();
        self.save()?;
        self.message = Some(format!("Started '{}'", name));
        Ok(())
    }

    pub fn stop_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if feature.status == ProjectStatus::Stopped {
            self.message = Some(format!(
                "Error: '{}' is already stopped",
                feature.name
            ));
            return Ok(());
        }

        // Fire on_stop lifecycle hook before killing session.
        // Clone hook and workdir data before mutable borrow.
        let on_stop_hook =
            self.active_extension.lifecycle_hooks.on_stop.clone();
        let workdir_for_hook = feature.workdir.clone();

        // If on_stop has a prompt, show the picker first.
        if let Some(ref cfg) = on_stop_hook {
            if let Some(prompt) = cfg.prompt() {
                self.start_hook_prompt(
                    cfg.script().to_string(),
                    workdir_for_hook,
                    prompt.title.clone(),
                    prompt.options.clone(),
                    HookNext::StopFeature { pi, fi },
                );
                return Ok(());
            }
        }

        if let Some(ref cfg) = on_stop_hook {
            self.run_lifecycle_hook(
                cfg.script(),
                &workdir_for_hook,
                None,
            );
        }

        self.do_stop_feature(pi, fi)?;

        Ok(())
    }

    /// Inner stop logic called after a hook prompt is confirmed.
    pub fn do_stop_feature(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        // Run on_stop for custom sessions before killing tmux.
        if let Some(feature) = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
        {
            Self::run_custom_session_on_stop(feature);
        }

        let tmux_session = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
        {
            Some(f) => f.tmux_session.clone(),
            None => return Ok(()),
        };

        self.tmux.kill_session(&tmux_session)?;

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };
        feature.status = ProjectStatus::Stopped;
        let name = feature.name.clone();
        self.save()?;
        self.message = Some(format!("Stopped '{}'", name));

        Ok(())
    }

    /// Run on_stop commands for all custom sessions in a
    /// feature and clean up their status files. Fire-and-forget.
    fn run_custom_session_on_stop(feature: &Feature) {
        use crate::project::SessionKind;

        let status_dir = feature
            .workdir
            .join(".amf")
            .join("session-status");

        for session in &feature.sessions {
            if session.kind != SessionKind::Custom {
                continue;
            }
            if let Some(ref cmd) = session.on_stop {
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .current_dir(&feature.workdir)
                    .env("AMF_SESSION_ID", &session.id)
                    .env("AMF_STATUS_DIR", &status_dir)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
            let _ = std::fs::remove_file(
                status_dir.join(format!("{}.txt", session.id)),
            );
        }
    }

    pub fn delete_feature(&mut self) -> Result<()> {
        let (project_name, feature_name) = match &self.mode {
            AppMode::DeletingFeature(pn, fn_) => {
                (pn.clone(), fn_.clone())
            }
            _ => return Ok(()),
        };

        let (tmux_session, is_worktree, repo, workdir) =
            if let Some(project) =
                self.store.find_project(&project_name)
                && let Some(feature) = project
                    .features
                    .iter()
                    .find(|f| f.name == feature_name)
            {
                // Run on_stop for custom sessions before killing.
                Self::run_custom_session_on_stop(feature);
                (
                    feature.tmux_session.clone(),
                    feature.is_worktree,
                    project.repo.clone(),
                    feature.workdir.clone(),
                )
            } else {
                return Ok(());
            };

        let child = TmuxManager::spawn_kill_session(&tmux_session)?;

        self.mode = AppMode::DeletingFeatureInProgress(DeletingFeatureState {
            project_name,
            feature_name,
            tmux_session,
            is_worktree,
            repo,
            workdir,
            stage: DeleteStage::KillingTmux,
            child,
            error: None,
        });

        Ok(())
    }

    pub fn poll_deleting_feature(&mut self) -> Result<()> {
        let state = match &mut self.mode {
            AppMode::DeletingFeatureInProgress(s) => s,
            _ => return Ok(()),
        };

        if let Some(ref mut child) = state.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        state.error = Some(format!(
                            "Command failed with code: {:?}",
                            status.code()
                        ));
                    }
                    state.child = None;
                }
                Ok(None) => return Ok(()),
                Err(e) => {
                    state.error = Some(e.to_string());
                    state.child = None;
                }
            }
        }

        match state.stage {
            DeleteStage::KillingTmux => {
                if state.is_worktree {
                    match WorktreeManager::spawn_remove(
                        &state.repo,
                        &state.workdir,
                    ) {
                        Ok(child) => {
                            state.child = Some(child);
                            state.stage = DeleteStage::RemovingWorktree;
                        }
                        Err(e) => {
                            state.error = Some(e.to_string());
                        }
                    }
                } else {
                    state.stage = DeleteStage::Completed;
                }
            }
            DeleteStage::RemovingWorktree => {
                state.stage = DeleteStage::Completed;
            }
            DeleteStage::Completed => {}
        }

        Ok(())
    }

    pub fn complete_deleting_feature(&mut self) -> Result<()> {
        let (project_name, feature_name, had_error, error_msg) = {
            match &self.mode {
                AppMode::DeletingFeatureInProgress(s) => (
                    s.project_name.clone(),
                    s.feature_name.clone(),
                    s.error.is_some(),
                    s.error.clone(),
                ),
                _ => return Ok(()),
            }
        };

        if had_error {
            self.mode = AppMode::Normal;
            self.message = Some(format!(
                "Error deleting feature '{}': {}",
                feature_name,
                error_msg.unwrap_or_else(|| "Unknown error".to_string())
            ));
            return Ok(());
        }

        self.store
            .remove_feature(&project_name, &feature_name);
        self.save()?;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            self.selection = Selection::Project(pi);
        }

        self.mode = AppMode::Normal;
        self.message = Some(format!(
            "Deleted feature '{}'",
            feature_name
        ));
        Ok(())
    }

    pub fn cancel_deleting_feature(&mut self) {
        self.mode = AppMode::Normal;
    }
}
