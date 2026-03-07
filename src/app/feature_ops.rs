use anyhow::Result;
use std::path::PathBuf;

use super::setup::{ensure_notification_hooks, ensure_review_claude_md};
use super::*;
use crate::extension::{load_global_extension_config, merge_project_extension_config};
use crate::tmux::TmuxManager;
use crate::worktree::WorktreeManager;
use state::{BackgroundDeletion, DeleteStage, ForkFeatureState, ForkFeatureStep};

impl App {
    pub fn toggle_feature_ready(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => (*pi, *fi),
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

        feature.ready = !feature.ready;
        let name = feature.name.clone();
        let ready = feature.ready;
        self.save()?;
        self.message = Some(if ready {
            format!("Marked '{}' as ready", name)
        } else {
            format!("Marked '{}' as not ready", name)
        });

        Ok(())
    }

    pub fn start_create_feature(&mut self) {
        let (project_name, project_repo, is_first, used_workdirs) = match &self.selection {
            Selection::Project(pi) | Selection::Feature(pi, _) | Selection::Session(pi, _, _) => {
                if let Some(p) = self.store.projects.get(*pi) {
                    let used: Vec<PathBuf> = p.features.iter().map(|f| f.workdir.clone()).collect();
                    (p.name.clone(), p.repo.clone(), p.features.is_empty(), used)
                } else {
                    return;
                }
            }
        };

        let worktrees = WorktreeManager::list(&project_repo)
            .unwrap_or_default()
            .into_iter()
            .filter(|wt| wt.path != project_repo && !used_workdirs.contains(&wt.path))
            .collect();

        self.mode = AppMode::CreatingFeature(CreateFeatureState::new(
            project_name,
            project_repo,
            worktrees,
            is_first,
        ));
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
        let use_existing_worktree = state.source_index == 1 && !state.worktrees.is_empty();
        let selected_worktree = if use_existing_worktree {
            state.worktrees.get(state.worktree_index).cloned()
        } else {
            None
        };
        let use_worktree = state.use_worktree;
        let enable_chrome = state.enable_chrome;
        let enable_notes = state.enable_notes;

        if branch.is_empty() {
            self.message = Some("Error: Branch name cannot be empty".into());
            return Ok(());
        }

        let stored_is_git = {
            let project = match self.store.find_project(&project_name) {
                Some(p) => p,
                None => {
                    self.message = Some(format!("Error: Project '{}' not found", project_name));
                    return Ok(());
                }
            };

            if project.features.iter().any(|f| f.name == branch) {
                self.message = Some(format!(
                    "Error: Feature '{}' already exists in '{}'",
                    branch, project_name
                ));
                return Ok(());
            }

            if !use_worktree
                && selected_worktree.is_none()
                && project.features.iter().any(|f| !f.is_worktree)
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

        let is_git = stored_is_git || self.worktree.repo_root(&project_repo).is_ok();

        if is_git && !stored_is_git {
            if let Some(p) = self.store.find_project_mut(&project_name) {
                p.is_git = true;
            }
            self.save()?;
        }

        if (use_worktree || selected_worktree.is_some()) && !is_git {
            self.message = Some("Error: Worktrees require a git repository".into());
            return Ok(());
        }

        let (workdir, is_worktree) = if let Some(wt) = &selected_worktree {
            (wt.path.clone(), true)
        } else if use_worktree {
            let wt_path = self.worktree.create(&project_repo, &branch, &branch)?;

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
            let fi = self.store.projects[pi].features.len().saturating_sub(1);
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
            let fi = self.store.projects[pi].features.len().saturating_sub(1);
            self.ensure_feature_running(pi, fi)?;
            self.save()?;
        }

        self.message = Some(format!("Created and started feature '{}'", branch));

        Ok(())
    }

    pub(crate) fn ensure_feature_running(&mut self, pi: usize, fi: usize) -> Result<()> {
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
            feature.is_worktree,
        );
        ensure_review_claude_md(&feature.workdir, feature.review);

        if feature.sessions.is_empty() {
            let session_kind = match feature.agent {
                AgentKind::Claude => SessionKind::Claude,
                AgentKind::Opencode => SessionKind::Opencode,
                AgentKind::Codex => SessionKind::Codex,
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
        self.tmux
            .set_session_env(&feature.tmux_session, "AMF_SESSION", &feature.tmux_session)?;

        for session in &feature.sessions[1..] {
            self.tmux.create_window(
                &feature.tmux_session,
                &session.tmux_window,
                &feature.workdir,
            )?;
        }

        let extra_args: Vec<String> = feature.mode.cli_flags(feature.enable_chrome);
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
                    self.tmux
                        .launch_opencode(&feature.tmux_session, &session.tmux_window)?;
                }
                SessionKind::Codex => {
                    self.tmux
                        .launch_codex(&feature.tmux_session, &session.tmux_window)?;
                }
                SessionKind::Nvim => {
                    if feature.has_notes {
                        self.tmux.send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim .claude/notes.md",
                        )?;
                    } else {
                        self.tmux
                            .send_keys(&feature.tmux_session, &session.tmux_window, "nvim")?;
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
                    // Run pre_check before launching
                    if let Some(ref check) = session.pre_check {
                        if !check.is_empty() {
                            let ok = std::process::Command::new("bash")
                                .arg("-c")
                                .arg(check)
                                .current_dir(&feature.workdir)
                                .output()
                                .map(|o| o.status.success())
                                .unwrap_or(false);
                            if !ok {
                                // Skip this session silently
                                // on restart; the tmux window
                                // will show a shell prompt.
                                continue;
                            }
                        }
                    }
                    let status_dir = feature.workdir.join(".amf").join("session-status");
                    let _ = std::fs::create_dir_all(&status_dir);
                    let env_prefix = format!(
                        "AMF_SESSION_ID='{}' AMF_STATUS_DIR='{}'",
                        session.id,
                        status_dir.display(),
                    );
                    let shell_cmd = if let Some(ref cmd) = session.command {
                        format!(
                            "env {} bash -c '{}'",
                            env_prefix,
                            cmd.replace('\'', "'\\''"),
                        )
                    } else {
                        format!("env {}", env_prefix)
                    };
                    self.tmux.send_literal(
                        &feature.tmux_session,
                        &session.tmux_window,
                        &shell_cmd,
                    )?;
                    self.tmux.send_key_name(
                        &feature.tmux_session,
                        &session.tmux_window,
                        "Enter",
                    )?;
                }
            }
        }

        self.tmux
            .select_window(&feature.tmux_session, &feature.sessions[0].tmux_window)?;

        feature.status = ProjectStatus::Idle;
        feature.touch();

        Ok(())
    }

    pub fn start_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => (*pi, *fi),
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
                self.message = Some(format!("Error: '{}' is already running", name));
            }
            return Ok(());
        }

        // If on_start has a prompt, show the picker first.
        let on_start = self.active_extension.lifecycle_hooks.on_start.clone();
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

        let name = self.store.projects[pi].features[fi].name.clone();
        self.save()?;
        self.message = Some(format!("Started '{}'", name));

        Ok(())
    }

    /// Inner start logic called after a hook prompt is confirmed.
    pub fn do_start_feature(&mut self, pi: usize, fi: usize) -> Result<()> {
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
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => (*pi, *fi),
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
            self.message = Some(format!("Error: '{}' is already stopped", feature.name));
            return Ok(());
        }

        // Fire on_stop lifecycle hook before killing session.
        // Clone hook and workdir data before mutable borrow.
        let on_stop_hook = self.active_extension.lifecycle_hooks.on_stop.clone();
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
            self.run_lifecycle_hook(cfg.script(), &workdir_for_hook, None);
        }

        self.do_stop_feature(pi, fi)?;

        Ok(())
    }

    /// Inner stop logic called after a hook prompt is confirmed.
    pub fn do_stop_feature(&mut self, pi: usize, fi: usize) -> Result<()> {
        // Run on_stop for custom sessions before killing tmux.
        if let Some(feature) = self.store.projects.get(pi).and_then(|p| p.features.get(fi)) {
            Self::run_custom_session_on_stop(feature);
        }

        let tmux_session = match self.store.projects.get(pi).and_then(|p| p.features.get(fi)) {
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

        let status_dir = feature.workdir.join(".amf").join("session-status");

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
            let _ = std::fs::remove_file(status_dir.join(format!("{}.txt", session.id)));
        }
    }

    pub fn delete_feature(&mut self) -> Result<()> {
        let (project_name, feature_name) = match &self.mode {
            AppMode::DeletingFeature(pn, fn_) => (pn.clone(), fn_.clone()),
            _ => return Ok(()),
        };

        let (tmux_session, is_worktree, repo, workdir) = if let Some(project) =
            self.store.find_project(&project_name)
            && let Some(feature) = project.features.iter().find(|f| f.name == feature_name)
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
                        state.error =
                            Some(format!("Command failed with code: {:?}", status.code()));
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
                    match WorktreeManager::spawn_remove(&state.repo, &state.workdir) {
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

        self.store.remove_feature(&project_name, &feature_name);
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
        self.message = Some(format!("Deleted feature '{}'", feature_name));
        Ok(())
    }

    pub fn cancel_deleting_feature(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn hide_deleting_feature(&mut self) {
        if let AppMode::DeletingFeatureInProgress(state) =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        {
            let key = state.key();
            let bg = BackgroundDeletion::from_deleting_state(state);
            self.background_deletions.insert(key, bg);
            self.message = Some("Deletion moved to background".to_string());
        }
    }

    pub fn is_feature_being_deleted(&self, project_name: &str, feature_name: &str) -> bool {
        let key = format!("{}/{}", project_name, feature_name);
        self.background_deletions.contains_key(&key)
    }

    pub fn poll_background_deletions(&mut self) -> Result<()> {
        let mut completed = Vec::new();

        for (key, deletion) in self.background_deletions.iter_mut() {
            if let Some(ref mut child) = deletion.child {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            deletion.error =
                                Some(format!("Command failed with code: {:?}", status.code()));
                        }
                        deletion.child = None;
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        deletion.error = Some(e.to_string());
                        deletion.child = None;
                    }
                }
            }

            match deletion.stage {
                DeleteStage::KillingTmux => {
                    if deletion.child.is_none() {
                        if deletion.is_worktree {
                            match WorktreeManager::spawn_remove(&deletion.repo, &deletion.workdir) {
                                Ok(child) => {
                                    deletion.child = Some(child);
                                    deletion.stage = DeleteStage::RemovingWorktree;
                                }
                                Err(e) => {
                                    deletion.error = Some(e.to_string());
                                }
                            }
                        } else {
                            deletion.stage = DeleteStage::Completed;
                        }
                    }
                }
                DeleteStage::RemovingWorktree => {
                    if deletion.child.is_none() {
                        deletion.stage = DeleteStage::Completed;
                    }
                }
                DeleteStage::Completed => {}
            }

            if deletion.stage == DeleteStage::Completed && deletion.child.is_none() {
                completed.push(key.clone());
            }
        }

        for key in completed {
            if let Some(deletion) = self.background_deletions.remove(&key) {
                if deletion.error.is_some() {
                    self.message = Some(format!(
                        "Error deleting feature '{}': {}",
                        deletion.feature_name,
                        deletion
                            .error
                            .unwrap_or_else(|| "Unknown error".to_string())
                    ));
                } else {
                    self.store
                        .remove_feature(&deletion.project_name, &deletion.feature_name);
                    let _ = self.save();
                    self.message = Some(format!("Deleted feature '{}'", deletion.feature_name));
                }
            }
        }

        Ok(())
    }

    pub fn start_fork_feature(&mut self) {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi) => (*pi, *fi),
            Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return,
        };

        let (project_name, project_repo, feature) = match self.store.projects.get(pi) {
            Some(p) => match p.features.get(fi) {
                Some(f) => (p.name.clone(), p.repo.clone(), f),
                None => return,
            },
            None => return,
        };

        let agent_index = AgentKind::ALL
            .iter()
            .position(|a| *a == feature.agent)
            .unwrap_or(0);

        let state = ForkFeatureState {
            source_pi: pi,
            source_fi: fi,
            project_name,
            project_repo,
            source_branch: feature.branch.clone(),
            new_branch: format!("{}-fork", feature.branch),
            step: ForkFeatureStep::Branch,
            agent: feature.agent.clone(),
            agent_index,
            mode: feature.mode.clone(),
            review: feature.review,
            enable_chrome: feature.enable_chrome,
            enable_notes: feature.has_notes,
            include_context: true,
        };

        self.mode = AppMode::ForkingFeature(state);
        self.message = None;
    }

    pub fn create_forked_feature(&mut self) -> Result<()> {
        let state = match &self.mode {
            AppMode::ForkingFeature(s) => s,
            _ => return Ok(()),
        };

        let project_name = state.project_name.clone();
        let project_repo = state.project_repo.clone();
        let source_branch = state.source_branch.clone();
        let new_branch = state.new_branch.clone();
        let mode = state.mode.clone();
        let review = state.review;
        let agent = state.agent.clone();
        let enable_chrome = state.enable_chrome;
        let enable_notes = state.enable_notes;
        let include_context = state.include_context;
        let source_workdir = self
            .store
            .projects
            .get(state.source_pi)
            .and_then(|p| p.features.get(state.source_fi))
            .map(|f| f.workdir.clone());

        if new_branch.is_empty() {
            self.message = Some("Error: Branch name cannot be empty".into());
            return Ok(());
        }

        // Check for duplicate feature name
        if let Some(project) = self.store.find_project(&project_name) {
            if project.features.iter().any(|f| f.name == new_branch) {
                self.message = Some(format!("Error: Feature '{}' already exists", new_branch));
                return Ok(());
            }
        } else {
            self.message = Some(format!("Error: Project '{}' not found", project_name));
            return Ok(());
        }

        // Create worktree from source branch
        let workdir =
            self.worktree
                .create_from(&project_repo, &new_branch, &new_branch, &source_branch)?;

        // Copy uncommitted changes from source worktree
        if let Some(ref src_wd) = source_workdir {
            let _ = WorktreeManager::copy_uncommitted_changes(src_wd, &workdir);
        }

        // Export transcript context from source session
        if include_context
            && let Some(ref src_wd) = source_workdir
            && let Some(jsonl) = crate::transcript::find_latest_transcript(src_wd)
            && let Ok(md) = crate::transcript::export_transcript_markdown(&jsonl)
        {
            let claude_dir = workdir.join(".claude");
            let _ = std::fs::create_dir_all(&claude_dir);
            let _ = std::fs::write(claude_dir.join("context.md"), md);
        }

        // Check for lifecycle hooks
        let global_ext = load_global_extension_config();
        let ext = merge_project_extension_config(&global_ext, &project_repo);

        if let Some(ref hook_cfg) = ext.lifecycle_hooks.on_worktree_created {
            if let Some(prompt) = hook_cfg.prompt() {
                self.start_hook_prompt(
                    hook_cfg.script().to_string(),
                    workdir.clone(),
                    prompt.title.clone(),
                    prompt.options.clone(),
                    HookNext::WorktreeCreated {
                        project_name,
                        branch: new_branch,
                        mode,
                        review,
                        agent,
                        enable_chrome,
                        enable_notes,
                    },
                );
                return Ok(());
            }

            self.start_worktree_hook(
                hook_cfg.script(),
                workdir.clone(),
                project_name.clone(),
                new_branch.clone(),
                mode.clone(),
                review,
                agent.clone(),
                enable_chrome,
                enable_notes,
                None,
            );
            return Ok(());
        }

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
            new_branch.clone(),
            new_branch.clone(),
            workdir,
            true,
            mode,
            review,
            agent,
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
            let fi = self.store.projects[pi].features.len().saturating_sub(1);
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
            let fi = self.store.projects[pi].features.len().saturating_sub(1);
            self.ensure_feature_running(pi, fi)?;
            self.save()?;
        }

        self.message = Some(format!("Forked '{}' -> '{}'", source_branch, new_branch));

        Ok(())
    }

    pub fn start_rename_feature(&mut self) {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi) => (*pi, *fi),
            _ => return,
        };

        let current_nickname = match self.store.projects.get(pi).and_then(|p| p.features.get(fi)) {
            Some(f) => f.nickname.clone().unwrap_or_default(),
            None => return,
        };

        self.mode = AppMode::RenamingFeature(state::RenameFeatureState {
            project_idx: pi,
            feature_idx: fi,
            input: current_nickname,
        });
    }

    pub fn apply_rename_feature(&mut self) -> Result<()> {
        let (pi, fi, input) = match &self.mode {
            AppMode::RenamingFeature(state) => {
                (state.project_idx, state.feature_idx, state.input.clone())
            }
            _ => return Ok(()),
        };

        if let Some(feature) = self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            if input.is_empty() {
                feature.nickname = None;
            } else {
                feature.nickname = Some(input.clone());
            }
        }

        self.save()?;
        self.mode = AppMode::Normal;

        self.message = if input.is_empty() {
            Some("Nickname cleared".into())
        } else {
            Some(format!("Renamed to '{}'", input))
        };

        Ok(())
    }

    pub fn cancel_rename_feature(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn create_batch_features(&mut self) -> Result<()> {
        let state = match &self.mode {
            AppMode::CreatingBatchFeatures(s) => s.clone(),
            _ => return Ok(()),
        };

        let workspace_path = if state.workspace_path.is_empty() {
            PathBuf::new()
        } else {
            PathBuf::from(&state.workspace_path)
        };
        let project_name = state.project_name.clone();
        let feature_count = state.feature_count;
        let feature_prefix = state.feature_prefix.clone();
        let mode = state.mode.clone();
        let review = state.review;
        let agent = state.agent.clone();
        let enable_chrome = state.enable_chrome;
        let enable_notes = state.enable_notes;

        if state.workspace_path.is_empty() || !workspace_path.exists() {
            self.message = Some("Error: Workspace path is invalid".into());
            return Ok(());
        }

        if project_name.is_empty() {
            self.message = Some("Error: Project name cannot be empty".into());
            return Ok(());
        }

        if self.store.find_project(&project_name).is_some() {
            self.message = Some(format!("Error: Project '{}' already exists", project_name));
            return Ok(());
        }

        if feature_count == 0 {
            self.message = Some("Error: Feature count must be at least 1".into());
            return Ok(());
        }

        if feature_prefix.is_empty() {
            self.message = Some("Error: Feature prefix cannot be empty".into());
            return Ok(());
        }

        let (project_repo, is_git) = match WorktreeManager::repo_root(&workspace_path) {
            Ok(r) => (r, true),
            Err(_) => (workspace_path.clone(), false),
        };

        if !is_git {
            self.message = Some("Error: Batch features require a git repository".into());
            return Ok(());
        }

        let project = Project::new(project_name.clone(), project_repo.clone(), is_git);
        self.store.add_project(project);
        self.save()?;

        let pi = self.store.projects.len().saturating_sub(1);
        self.store.projects[pi].collapsed = false;
        self.selection = Selection::Project(pi);
        self.mode = AppMode::Normal;

        let mut created_features = Vec::new();
        for i in 1..=feature_count {
            let branch = format!("{}{}", feature_prefix, i);

            if self.store.projects[pi]
                .features
                .iter()
                .any(|f| f.name == branch)
            {
                self.message = Some(format!("Error: Feature '{}' already exists", branch));
                return Ok(());
            }

            let workdir = self.worktree.create(&project_repo, &branch, &branch)?;

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
                true,
                mode.clone(),
                review,
                agent.clone(),
                enable_chrome,
                enable_notes,
            );

            self.store.add_feature(&project_name, feature);
            created_features.push(branch);
            self.save()?;
        }

        for (_idx, branch) in created_features.iter().enumerate() {
            let fi = self.store.projects[pi]
                .features
                .iter()
                .position(|f| f.name == *branch)
                .unwrap();

            self.ensure_feature_running(pi, fi)?;
            self.save()?;
        }

        self.message = Some(format!(
            "Created project '{}' with {} features",
            project_name, feature_count
        ));

        Ok(())
    }
}
