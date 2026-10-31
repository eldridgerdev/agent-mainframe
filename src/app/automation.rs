use std::path::Path;

use anyhow::{Context, Result, bail};

use super::*;
use crate::automation::{
    AutomationHookPrompt, BatchFeatureAutomationResult, CreateBatchFeaturesRequest,
    CreateBatchFeaturesResponse, CreateFeatureRequest, CreateFeatureResponse, CreateProjectRequest,
    CreateProjectResponse, CreateSessionRequest, CreateSessionResponse, SeedAiReviewRequest,
    SeedAiReviewResponse,
};
use crate::extension::{HookPrompt, merge_project_extension_config};
use crate::project::{normalized_feature_name, tmux_session_name, worktree_name};

/// A missing or unknown answer to the project's `on_worktree_created`
/// prompt. Typed so callers can report it as the user's to fix
/// (`GuiError::conflict`) rather than as an internal failure.
#[derive(Debug)]
pub(crate) struct WorktreeHookChoiceError(String);

impl std::fmt::Display for WorktreeHookChoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WorktreeHookChoiceError {}

/// The one place the hook-choice rule lives: a prompted hook needs one of its
/// options; an unprompted hook (or none) takes no choice, so `None`.
pub(super) fn checked_hook_choice<'a>(
    prompt: Option<&HookPrompt>,
    choice: Option<&'a str>,
) -> std::result::Result<Option<&'a str>, WorktreeHookChoiceError> {
    let Some(prompt) = prompt else {
        return Ok(None);
    };
    let options = prompt.options.join(", ");
    let choice = choice.ok_or_else(|| {
        WorktreeHookChoiceError(format!(
            "Worktree hook requires a choice; choose from [{options}]"
        ))
    })?;
    if !prompt.options.iter().any(|option| option == choice) {
        return Err(WorktreeHookChoiceError(format!(
            "Invalid hook_choice '{choice}'; expected one of [{options}]"
        )));
    }
    Ok(Some(choice))
}

pub(crate) struct PendingAutomationPrompt {
    session_id: String,
    tmux_session: String,
    window: String,
    text: String,
    submit: bool,
    queued_at: std::time::Instant,
}

fn automation_prompt_ready(kind: &SessionKind, screen: &str) -> bool {
    // Startup selectors can use the same cursor glyph as an editor. Do not
    // mistake their highlighted choice for an input prompt.
    let lower = screen.to_lowercase();
    if [
        "do you trust",
        "trust this folder",
        "trust the files",
        "sign in with",
        "select login",
        "choose the text style",
        "select the text style",
    ]
    .iter()
    .any(|blocker| lower.contains(blocker))
    {
        return false;
    }
    let editor_marker = |line: &str, marker: char| {
        line.strip_prefix(marker).is_some_and(|rest| {
            !rest
                .trim_start()
                .starts_with(|ch: char| ch.is_ascii_digit())
        })
    };
    let lines: Vec<_> = screen.lines().rev().take(12).map(str::trim).collect();
    match kind {
        SessionKind::Claude => lines
            .iter()
            .any(|line| editor_marker(line, '❯') || *line == ">" || line.starts_with("> Try ")),
        SessionKind::Codex => lines.iter().any(|line| editor_marker(line, '›')),
        SessionKind::Opencode => screen.contains("Ask anything"),
        SessionKind::Pi => screen.contains("%/") && lines.iter().any(|line| line.contains("───")),
        _ => false,
    }
}

impl App {
    pub fn create_session_from_request(
        &mut self,
        request: &CreateSessionRequest,
    ) -> Result<CreateSessionResponse> {
        if request.project_name.trim().is_empty() || request.feature.trim().is_empty() {
            bail!("project_name and feature are required");
        }
        if request
            .label
            .as_ref()
            .is_some_and(|label| label.trim().is_empty())
        {
            bail!("Session label cannot be empty");
        }
        let pi = self
            .store
            .projects
            .iter()
            .position(|p| p.name == request.project_name)
            .ok_or_else(|| anyhow::anyhow!("Project '{}' not found", request.project_name))?;
        let fi = self.store.projects[pi]
            .features
            .iter()
            .position(|f| f.name == request.feature)
            .ok_or_else(|| anyhow::anyhow!("Feature '{}' not found", request.feature))?;
        let agent = match request.kind.as_str() {
            "claude" => Some(AgentKind::Claude),
            "codex" => Some(AgentKind::Codex),
            "opencode" => Some(AgentKind::Opencode),
            "pi" => Some(AgentKind::Pi),
            _ => None,
        };
        let repo = &self.store.projects[pi].repo;
        let custom = if agent.is_none() && request.kind != "terminal" {
            Some(
                merge_project_extension_config(&self.config.extension, repo)
                    .custom_sessions
                    .into_iter()
                    .find(|config| config.name == request.kind)
                    .ok_or_else(|| anyhow::anyhow!("Unknown session kind '{}'", request.kind))?,
            )
        } else {
            None
        };
        Self::validate_automation_prompt(request.initial_prompt.as_deref(), request.submit_prompt)?;
        if request.initial_prompt.is_some() && agent.is_none() {
            bail!("initial_prompt requires an agent harness session");
        }
        if let Some(agent) = &agent
            && !self.allows_agent_for_repo(repo, agent)
        {
            bail!(
                "Harness '{}' is not allowed for this workspace",
                agent.display_name()
            );
        }
        let label = request
            .label
            .clone()
            .unwrap_or_else(|| request.kind.clone());
        let feature_id = self.store.projects[pi].features[fi].id.clone();
        let mut response = CreateSessionResponse {
            msg_type: crate::automation::AUTOMATION_RESULT_TYPE,
            action: crate::automation::CREATE_SESSION_ACTION,
            ok: true,
            dry_run: request.dry_run,
            tmux_session: self.store.projects[pi].features[fi].tmux_session.clone(),
            tmux_window: None,
            session_id: None,
            message: format!(
                "Dry run: would create '{}' session in '{}'",
                label, request.feature
            ),
        };
        if request.dry_run {
            return Ok(response);
        }
        let kind = agent
            .as_ref()
            .map(session_kind_for_agent)
            .unwrap_or(SessionKind::Terminal);
        if (agent.is_some() || self.add_would_start_feature(pi, fi, &kind))
            && !matches!(
                self.check_start_preconditions(),
                super::resource_gate::StartPreconditions::Ok
            )
        {
            bail!("Session not created: the machine is at its agent/memory limit");
        }
        if let Some(config) = &custom {
            let workdir = &self.store.projects[pi].features[fi].workdir;
            let check_dir = config
                .working_dir
                .as_ref()
                .map(|dir| workdir.join(dir))
                .unwrap_or_else(|| workdir.clone());
            config
                .run_pre_check(&check_dir)
                .map_err(anyhow::Error::msg)?;
        }
        if let Some(agent) = &agent {
            self.tmux.check_harness_available(agent)?;
        }
        let id = if let Some(agent) = agent {
            self.with_feature_started_for_add(pi, fi, |app| {
                app.create_agent_session_labeled_identified(
                    pi,
                    fi,
                    &label,
                    Some(agent),
                    StartIntent::Approved,
                )
                .map(|(_, id, _)| id)
            })?
        } else if let Some(config) = custom {
            self.add_custom_session_identified(pi, fi, &config, label)?
        } else {
            self.add_builtin_tmux_session_identified(pi, fi, SessionKind::Terminal, label)?
        };
        let (pi, fi) = self
            .store
            .locate_feature_by_id(None, &feature_id)
            .ok_or_else(|| anyhow::anyhow!("Feature removed while creating session"))?;
        let feature = &self.store.projects[pi].features[fi];
        let session = feature
            .sessions
            .iter()
            .find(|session| session.id == id)
            .ok_or_else(|| anyhow::anyhow!("Session removed while creating it"))?;
        response.tmux_session = feature.tmux_session.clone();
        response.tmux_window = Some(session.tmux_window.clone());
        response.session_id = Some(id.clone());
        response.message = if request.initial_prompt.is_some() {
            if request.submit_prompt {
                "Created session; initial prompt queued for submission".to_string()
            } else {
                "Created session; initial prompt queued as an unsent draft".to_string()
            }
        } else {
            "Created session".to_string()
        };
        self.queue_automation_prompt(
            &id,
            request.initial_prompt.as_deref(),
            request.submit_prompt,
        );
        Ok(response)
    }

    fn validate_automation_prompt(prompt: Option<&str>, submit: bool) -> Result<()> {
        if submit && prompt.is_none() {
            bail!("submit_prompt requires initial_prompt");
        }
        if prompt.is_some_and(|text| text.trim().is_empty() || text.contains('\0')) {
            bail!("initial_prompt must be nonempty and contain no NUL bytes");
        }
        Ok(())
    }

    pub(crate) fn queue_automation_prompt(
        &mut self,
        session_id: &str,
        prompt: Option<&str>,
        submit: bool,
    ) {
        let Some(text) = prompt else {
            return;
        };
        for project in &self.store.projects {
            for feature in &project.features {
                if let Some(session) = feature.sessions.iter().find(|s| s.id == session_id) {
                    self.pending_automation_prompts
                        .push(PendingAutomationPrompt {
                            session_id: session_id.to_string(),
                            tmux_session: feature.tmux_session.clone(),
                            window: session.tmux_window.clone(),
                            text: text.to_string(),
                            submit,
                            queued_at: std::time::Instant::now(),
                        });
                    return;
                }
            }
        }
    }

    pub(crate) fn poll_automation_prompts(&mut self) {
        for pending in std::mem::take(&mut self.pending_automation_prompts) {
            // The persisted identity must still own this exact target. Never deliver
            // into a window recreated for a different session.
            let kind = self
                .store
                .projects
                .iter()
                .flat_map(|p| &p.features)
                .filter(|f| {
                    f.tmux_session == pending.tmux_session && f.status != ProjectStatus::Stopped
                })
                .flat_map(|f| &f.sessions)
                .find(|s| {
                    s.id == pending.session_id && s.tmux_window == pending.window && !s.stopped
                })
                .map(|s| s.kind.clone());
            let Some(kind) = kind else {
                continue;
            };
            if pending.queued_at.elapsed() > std::time::Duration::from_secs(120) {
                self.log_warn(
                    "automation",
                    format!(
                        "Initial prompt for session {} timed out waiting for agent input",
                        pending.session_id
                    ),
                );
                continue;
            }
            let ready = self
                .tmux
                .window_exists(&pending.tmux_session, &pending.window)
                && self
                    .tmux
                    .capture_pane(&pending.tmux_session, &pending.window)
                    .is_ok_and(|screen| automation_prompt_ready(&kind, &screen));
            if !ready {
                self.pending_automation_prompts.push(pending);
                continue;
            }
            let result = self
                .tmux
                .paste_text(&pending.tmux_session, &pending.window, &pending.text)
                .and_then(|()| {
                    if pending.submit {
                        self.tmux
                            .send_key_name(&pending.tmux_session, &pending.window, "Enter")
                    } else {
                        Ok(())
                    }
                });
            if let Err(err) = result {
                self.log_error(
                    "automation",
                    format!(
                        "Could not deliver initial prompt to {}: {err}",
                        pending.session_id
                    ),
                );
            }
        }
    }

    fn planned_batch_feature_results(
        request: &CreateBatchFeaturesRequest,
        project_repo: &Path,
    ) -> Vec<BatchFeatureAutomationResult> {
        (1..=request.feature_count)
            .map(|i| {
                let branch = format!("{}{}", request.feature_prefix, i);
                BatchFeatureAutomationResult {
                    name: branch.clone(),
                    branch: branch.clone(),
                    workdir: project_repo
                        .join(".worktrees")
                        .join(worktree_name(&request.project_name, &branch)),
                    tmux_session: tmux_session_name(&request.project_name, &branch),
                    started: !request.dry_run,
                }
            })
            .collect()
    }

    fn resolve_sync_hook_script(script: &str) -> String {
        if script.starts_with("~/") {
            dirs::home_dir()
                .map(|h| format!("{}/{}", h.display(), &script[2..]))
                .unwrap_or_else(|| script.to_string())
        } else {
            script.to_string()
        }
    }

    pub(crate) fn run_worktree_hook_sync(
        script: &str,
        workdir: &Path,
        choice: Option<&str>,
    ) -> (bool, Option<String>) {
        Self::run_worktree_hook_cancellable(script, workdir, choice, None)
    }

    /// Like [`Self::run_worktree_hook_sync`], but terminates the hook's whole
    /// process tree once `cancel` is set, so a hook that never exits cannot
    /// pin its caller.
    pub(crate) fn run_worktree_hook_cancellable(
        script: &str,
        workdir: &Path,
        choice: Option<&str>,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> (bool, Option<String>) {
        use std::io::Read;
        use std::os::unix::process::CommandExt;
        use std::sync::atomic::Ordering;
        let expanded = Self::resolve_sync_hook_script(script);
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&expanded)
            .current_dir(workdir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .process_group(0);
        if let Some(c) = choice {
            cmd.env("AMF_HOOK_CHOICE", c);
        }
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => return (false, Some(err.to_string())),
        };
        // Drain stderr off-thread so a chatty hook cannot block on a full pipe.
        let mut stderr = child.stderr.take();
        let reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(stderr) = stderr.as_mut() {
                let _ = stderr.read_to_end(&mut buf);
            }
            buf
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {}
                Err(err) => break Err(err),
            }
            if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                crate::resources::procs::terminate_tree(
                    i64::from(child.id()),
                    std::time::Duration::from_secs(2),
                );
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return (false, Some("cancelled; hook process was terminated".into()));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let stderr = reader.join().unwrap_or_default();
        match status {
            Ok(status) if status.success() => (true, None),
            Ok(status) => {
                let stderr = String::from_utf8_lossy(&stderr);
                let detail = stderr
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| line.trim().to_string())
                    .or_else(|| status.code().map(|code| format!("exit code {code}")));
                (false, detail)
            }
            Err(err) => (false, Some(err.to_string())),
        }
    }

    pub(crate) fn worktree_hook_prompt_for_repo(
        &self,
        project_repo: &Path,
    ) -> Option<AutomationHookPrompt> {
        let ext = merge_project_extension_config(&self.config.extension, project_repo);
        ext.lifecycle_hooks
            .on_worktree_created
            .as_ref()
            .and_then(|hook| hook.prompt())
            .map(|prompt| AutomationHookPrompt {
                title: prompt.title.clone(),
                options: prompt.options.clone(),
            })
    }

    /// Validate before creating a checkout or reserving a TODO. Dry-run
    /// callers can still discover the prompt without supplying an answer.
    pub(crate) fn validate_worktree_hook_choice(
        &self,
        project_repo: &Path,
        choice: Option<&str>,
    ) -> Result<()> {
        let ext = merge_project_extension_config(&self.config.extension, project_repo);
        let hook = ext.lifecycle_hooks.on_worktree_created.as_ref();
        checked_hook_choice(hook.and_then(|hook| hook.prompt()), choice)?;
        Ok(())
    }

    pub fn create_project_from_request(
        &mut self,
        request: &CreateProjectRequest,
    ) -> Result<CreateProjectResponse> {
        if request.project_name.trim().is_empty() {
            bail!("Project name cannot be empty");
        }

        if request.path.as_os_str().is_empty() {
            bail!("Path cannot be empty");
        }

        if !request.path.exists() {
            bail!("Path does not exist: {}", request.path.display());
        }

        if !request.path.is_dir() {
            bail!("Path is not a directory: {}", request.path.display());
        }

        if self.store.find_project(&request.project_name).is_some() {
            bail!("Project '{}' already exists", request.project_name);
        }

        let (project_path, is_git) = match self.worktree.repo_root(&request.path) {
            Ok(repo) => (repo, true),
            Err(_) => (request.path.clone(), false),
        };
        let preferred_agent = request
            .preferred_agent
            .clone()
            .unwrap_or_else(|| self.default_project_preferred_agent());
        let (preferred_agent, _) =
            self.normalize_agent_for_project_path(&project_path, &preferred_agent);

        if request.dry_run {
            let message = format!("Dry run: would create project '{}'", request.project_name);
            return Ok(CreateProjectResponse::success(
                request,
                project_path,
                is_git,
                message,
                None,
            ));
        }

        let project = Project::new(
            request.project_name.clone(),
            project_path.clone(),
            is_git,
            preferred_agent,
        );
        let project_id = project.id.clone();
        self.store.add_project(project);
        self.save()?;

        let message = format!("Created project '{}'", request.project_name);
        Ok(CreateProjectResponse::success(
            request,
            project_path,
            is_git,
            message,
            Some(project_id),
        ))
    }

    /// Match `path` against the repo's registered worktrees, refusing the repo
    /// root and any checkout another feature already owns. Mirrors the
    /// wizard's "existing worktree" list.
    fn resolve_existing_worktree(
        &self,
        project_repo: &Path,
        path: &Path,
    ) -> Result<crate::worktree::WorktreeInfo> {
        let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let wanted = canonical(&if path.is_relative() {
            project_repo.join(path)
        } else {
            path.to_path_buf()
        });
        let repo = canonical(project_repo);
        let listed = crate::worktree::WorktreeManager::list(project_repo)
            .context("Failed to list git worktrees for this repo")?;
        let Some(info) = listed
            .iter()
            .find(|wt| canonical(&wt.path) == wanted && canonical(&wt.path) != repo)
        else {
            let available = listed
                .iter()
                .filter(|wt| canonical(&wt.path) != repo)
                .map(|wt| wt.path.display().to_string())
                .collect::<Vec<_>>();
            bail!(
                "'{}' is not an existing git worktree of this repo (available: [{}])",
                path.display(),
                available.join(", ")
            );
        };
        let owner = self.store.projects.iter().find_map(|p| {
            p.features
                .iter()
                .find(|f| canonical(&f.workdir) == wanted)
                .map(|f| (p.name.clone(), f.name.clone()))
        });
        if let Some((project, feature)) = owner {
            bail!(
                "Worktree {} is already used by feature '{}' in '{}'",
                info.path.display(),
                feature,
                project
            );
        }
        if info.branch.is_none() {
            bail!(
                "Worktree {} has a detached HEAD; check out a branch there before attaching it",
                info.path.display()
            );
        }
        Ok(info.clone())
    }

    pub fn create_feature_from_request(
        &mut self,
        request: &CreateFeatureRequest,
    ) -> Result<CreateFeatureResponse> {
        Self::validate_automation_prompt(request.initial_prompt.as_deref(), request.submit_prompt)?;
        if request.no_start && (request.initial_prompt.is_some() || request.submit_prompt) {
            bail!("initial_prompt and submit_prompt require starting the feature");
        }
        if request.project_name.trim().is_empty() {
            bail!("Project name cannot be empty");
        }

        if request.branch.trim().is_empty() {
            bail!("Branch name cannot be empty");
        }

        let (
            project_repo,
            stored_is_git,
            has_non_worktree_feature,
            has_any_features,
            project_collapsed,
        ) = {
            let project = self
                .store
                .find_project(&request.project_name)
                .ok_or_else(|| anyhow::anyhow!("Project '{}' not found", request.project_name))?;

            let normalized_branch = normalized_feature_name(&request.branch);
            if project
                .features
                .iter()
                .any(|f| normalized_feature_name(&f.name) == normalized_branch)
            {
                bail!(
                    "Feature '{}' already exists in '{}'",
                    request.branch,
                    request.project_name
                );
            }

            (
                project.repo.clone(),
                project.is_git,
                project.features.iter().any(|f| !f.is_worktree),
                !project.features.is_empty(),
                project.collapsed,
            )
        };

        if !self.allows_agent_for_repo(&project_repo, &request.agent) {
            bail!(
                "Harness '{}' is not allowed for this workspace",
                request.agent.display_name()
            );
        }
        self.ensure_agent_mode_supported(&request.agent, &request.mode)?;

        let use_worktree = request.use_worktree.unwrap_or(has_any_features);
        let is_git = stored_is_git || self.worktree.repo_root(&project_repo).is_ok();

        if is_git && !stored_is_git {
            if let Some(project) = self.store.find_project_mut(&request.project_name) {
                project.is_git = true;
            }
            self.save()?;
        }

        if use_worktree && !is_git {
            bail!("Worktrees require a git repository");
        }

        let existing_worktree = match request.existing_worktree.as_deref() {
            Some(path) => {
                if request.use_worktree == Some(false) {
                    bail!("existing_worktree cannot be combined with use_worktree: false");
                }
                if !is_git {
                    bail!("Worktrees require a git repository");
                }
                Some(self.resolve_existing_worktree(&project_repo, path)?)
            }
            None => None,
        };
        let use_worktree = use_worktree || existing_worktree.is_some();
        if !use_worktree && has_non_worktree_feature {
            bail!("Only one non-worktree feature allowed per project");
        }

        // An attached worktree already exists, so its creation hook is moot.
        let hook_prompt = if use_worktree && existing_worktree.is_none() {
            self.worktree_hook_prompt_for_repo(&project_repo)
        } else {
            None
        };
        let mut hook_ran = false;
        let mut hook_succeeded = None;
        let workdir = if let Some(info) = &existing_worktree {
            info.path.clone()
        } else if use_worktree {
            let planned_workdir = project_repo
                .join(".worktrees")
                .join(worktree_name(&request.project_name, &request.branch));
            if planned_workdir.exists() {
                let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
                let wanted = canonical(&planned_workdir);
                let registered = crate::worktree::WorktreeManager::list(&project_repo)
                    .map(|list| list.iter().any(|wt| canonical(&wt.path) == wanted))
                    .unwrap_or(false);
                if registered {
                    bail!(
                        "Worktree path already exists: {path}. To create the feature from that \
                         existing worktree, resend the request with \"existing_worktree\": \
                         \"{path}\" (or pick a different branch name)",
                        path = planned_workdir.display()
                    );
                }
                bail!(
                    "Worktree path already exists: {path}, but it is not a registered git \
                     worktree. Remove the stale directory or pick a different branch name",
                    path = planned_workdir.display()
                );
            }
            planned_workdir
        } else {
            project_repo.clone()
        };

        if request.dry_run {
            let message = format!("Dry run: would create feature '{}'", request.branch);
            return Ok(CreateFeatureResponse::success(
                request,
                workdir,
                use_worktree,
                tmux_session_name(&request.project_name, &request.branch),
                false,
                hook_ran,
                hook_succeeded,
                hook_prompt.clone(),
                message,
                None,
            ));
        }

        // Read the hook once and validate the choice against it before the
        // checkout exists; the hook that runs below is this same one, so the
        // choice cannot go stale between the check and the run.
        let worktree_hook = if use_worktree && existing_worktree.is_none() {
            merge_project_extension_config(&self.config.extension, &project_repo)
                .lifecycle_hooks
                .on_worktree_created
        } else {
            None
        };
        let hook_choice = checked_hook_choice(
            worktree_hook.as_ref().and_then(|hook| hook.prompt()),
            request.hook_choice.as_deref(),
        )?;
        let final_workdir = if let Some(info) = &existing_worktree {
            info.path.clone()
        } else if use_worktree {
            let worktree_name = worktree_name(&request.project_name, &request.branch);
            let workdir = self
                .worktree
                .create(&project_repo, &worktree_name, &request.branch)?;

            if let Some(ref hook_cfg) = worktree_hook {
                hook_ran = true;
                let (success, detail) =
                    Self::run_worktree_hook_sync(hook_cfg.script(), &workdir, hook_choice);
                hook_succeeded = Some(success);
                if !success {
                    self.log_warn(
                        "automation",
                        format!(
                            "Worktree hook failed for feature '{}': {}",
                            request.branch,
                            detail.unwrap_or_else(|| "unknown error".to_string())
                        ),
                    );
                }
            }

            workdir
        } else {
            project_repo.clone()
        };

        let feature = Feature::new_for_project(
            &request.project_name,
            request.branch.clone(),
            existing_worktree
                .as_ref()
                .and_then(|info| info.branch.clone())
                .unwrap_or_else(|| request.branch.clone()),
            final_workdir.clone(),
            use_worktree,
            request.mode.clone(),
            request.review,
            request.plan_mode,
            request.agent.clone(),
            request.enable_chrome,
            false, // remote_control not supported via automation API yet
        );
        let mut feature = feature;
        Self::initialize_feature_sessions(&mut feature, request.create_terminal, None);

        self.store.add_feature(&request.project_name, feature);
        self.save()?;

        let pi = self
            .store
            .projects
            .iter()
            .position(|p| p.name == request.project_name)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Project '{}' missing after feature add",
                    request.project_name
                )
            })?;
        let fi = self.store.projects[pi].features.len().saturating_sub(1);
        let ids = (
            self.store.projects[pi].id.clone(),
            self.store.projects[pi].features[fi].id.clone(),
        );
        self.store.projects[pi].collapsed = project_collapsed;
        // No caller to answer a confirmation dialog here, so a tripped gate
        // creates the feature and leaves it stopped; `started: false` in the
        // response says so.
        let started = !request.no_start && self.autostart_allowed(&request.branch);
        if started {
            self.ensure_feature_running(pi, fi, StartIntent::Approved)?;
            if let Some(id) = self.store.projects[pi].features[fi]
                .sessions
                .iter()
                .find(|s| s.kind.is_agent_harness())
                .map(|s| s.id.clone())
            {
                self.queue_automation_prompt(
                    &id,
                    request.initial_prompt.as_deref(),
                    request.submit_prompt,
                );
            }
        }
        self.save()?;

        let message = match (started, hook_succeeded) {
            (false, _) if request.no_start => {
                format!(
                    "Created feature '{}' without starting sessions",
                    request.branch
                )
            }
            (false, _) => format!(
                "Created feature '{}' but did not start it: the machine is at its agent/memory limit",
                request.branch
            ),
            (true, Some(true)) => format!(
                "Created and started feature '{}' (hook succeeded)",
                request.branch
            ),
            (true, Some(false)) => format!(
                "Created and started feature '{}' (hook failed)",
                request.branch
            ),
            (true, None) => format!("Created and started feature '{}'", request.branch),
        };

        Ok(CreateFeatureResponse::success(
            request,
            final_workdir,
            use_worktree,
            tmux_session_name(&request.project_name, &request.branch),
            started,
            hook_ran,
            hook_succeeded,
            hook_prompt,
            message,
            Some(ids),
        ))
    }

    pub fn create_batch_features_from_request(
        &mut self,
        request: &CreateBatchFeaturesRequest,
    ) -> Result<CreateBatchFeaturesResponse> {
        self.create_batch_features_inner(request, false)
    }

    /// The dashboard's `B` on a selected project: the same batch, added to
    /// that existing project instead of a new one. Kept off the automation
    /// API, whose callers rely on "project already exists" being an error.
    pub(crate) fn add_batch_features_to_project(
        &mut self,
        request: &CreateBatchFeaturesRequest,
    ) -> Result<CreateBatchFeaturesResponse> {
        self.create_batch_features_inner(request, true)
    }

    fn create_batch_features_inner(
        &mut self,
        request: &CreateBatchFeaturesRequest,
        into_existing: bool,
    ) -> Result<CreateBatchFeaturesResponse> {
        Self::validate_automation_prompt(request.initial_prompt.as_deref(), request.submit_prompt)?;
        if request.workspace_path.as_os_str().is_empty() || !request.workspace_path.exists() {
            bail!("Workspace path is invalid");
        }

        if request.project_name.trim().is_empty() {
            bail!("Project name cannot be empty");
        }

        let existing = self.store.find_project(&request.project_name);
        match (existing, into_existing) {
            (Some(_), false) => bail!("Project '{}' already exists", request.project_name),
            (None, true) => bail!("Project '{}' not found", request.project_name),
            _ => {}
        }

        if request.feature_count == 0 {
            bail!("Feature count must be at least 1");
        }

        if request.feature_prefix.trim().is_empty() {
            bail!("Feature prefix cannot be empty");
        }

        let (project_repo, is_git) = match self.worktree.repo_root(&request.workspace_path) {
            Ok(repo) => (repo, true),
            Err(_) => (request.workspace_path.clone(), false),
        };

        if !is_git {
            bail!("Batch features require a git repository");
        }

        if !self.allows_agent_for_repo(&project_repo, &request.agent) {
            bail!(
                "Harness '{}' is not allowed for this workspace",
                request.agent.display_name()
            );
        }
        self.ensure_agent_mode_supported(&request.agent, &request.mode)?;

        let planned_features = Self::planned_batch_feature_results(request, &project_repo);
        if into_existing && let Some(project) = self.store.find_project(&request.project_name) {
            if project.repo != project_repo {
                bail!(
                    "Project '{}' is at {}, not {}",
                    request.project_name,
                    project.repo.display(),
                    project_repo.display()
                );
            }
            for feature in &planned_features {
                let normalized = normalized_feature_name(&feature.name);
                if project
                    .features
                    .iter()
                    .any(|f| normalized_feature_name(&f.name) == normalized)
                {
                    bail!(
                        "Feature '{}' already exists in '{}'; choose another base name",
                        feature.name,
                        request.project_name
                    );
                }
            }
        }
        for feature in &planned_features {
            if feature.workdir.exists() {
                bail!(
                    "Worktree path already exists: {}",
                    feature.workdir.display()
                );
            }
        }

        if request.dry_run {
            let message = format!(
                "Dry run: would create project '{}' with {} features",
                request.project_name, request.feature_count
            );
            return Ok(CreateBatchFeaturesResponse::success(
                request,
                project_repo,
                planned_features,
                message,
            ));
        }

        let pi = if into_existing {
            self.store
                .projects
                .iter()
                .position(|p| p.name == request.project_name)
                .ok_or_else(|| anyhow::anyhow!("Project '{}' not found", request.project_name))?
        } else {
            let project = Project::new(
                request.project_name.clone(),
                project_repo.clone(),
                is_git,
                request.agent.clone(),
            );
            self.store.add_project(project);
            self.save()?;
            self.store.projects.len().saturating_sub(1)
        };
        let mut created_feature_names = Vec::new();
        let mut response_features = Vec::new();

        for planned in planned_features {
            let planned_name = planned.name.clone();
            let worktree_name = worktree_name(&request.project_name, &planned.branch);
            let workdir = self
                .worktree
                .create(&project_repo, &worktree_name, &planned.branch)?;

            let feature = Feature::new_for_project(
                &request.project_name,
                planned.name.clone(),
                planned.branch.clone(),
                workdir.clone(),
                true,
                request.mode.clone(),
                request.review,
                false,
                request.agent.clone(),
                request.enable_chrome,
                false, // remote_control not supported via automation API yet
            );

            self.store.add_feature(&request.project_name, feature);
            created_feature_names.push(planned.name.clone());
            response_features.push(BatchFeatureAutomationResult {
                workdir,
                started: true,
                ..planned
            });
            self.save()?;

            if let Some(feature) =
                self.store
                    .find_project(&request.project_name)
                    .and_then(|project| {
                        project
                            .features
                            .iter()
                            .find(|feature| feature.name == planned_name)
                    })
            {
                self.log_info(
                    "feature_create",
                    format!(
                        "Created feature: {}",
                        App::feature_audit_details(&request.project_name, feature)
                    ),
                );
            }
        }

        for branch in &created_feature_names {
            let fi = self.store.projects[pi]
                .features
                .iter()
                .position(|f| f.name == *branch)
                .ok_or_else(|| anyhow::anyhow!("Feature '{}' missing after creation", branch))?;

            // Re-checked per feature: a batch big enough to blow past the cap
            // starts what fits and leaves the rest created-but-stopped, rather
            // than queueing a confirmation dialog for every one of them.
            let started = self.autostart_allowed(branch);
            if started {
                self.ensure_feature_running(pi, fi, StartIntent::Approved)?;
                if let Some(id) = self.store.projects[pi].features[fi]
                    .sessions
                    .iter()
                    .find(|s| s.kind.is_agent_harness())
                    .map(|s| s.id.clone())
                {
                    self.queue_automation_prompt(
                        &id,
                        request.initial_prompt.as_deref(),
                        request.submit_prompt,
                    );
                }
            }
            if let Some(result) = response_features
                .iter_mut()
                .find(|result| result.name == *branch)
            {
                result.started = started;
            }
            self.save()?;
        }

        let message = if into_existing {
            format!(
                "Added {} features to project '{}'",
                request.feature_count, request.project_name
            )
        } else {
            format!(
                "Created project '{}' with {} features",
                request.project_name, request.feature_count
            )
        };

        Ok(CreateBatchFeaturesResponse::success(
            request,
            project_repo,
            response_features,
            message,
        ))
    }

    pub fn seed_ai_review_from_request(
        &mut self,
        request: &SeedAiReviewRequest,
    ) -> Result<SeedAiReviewResponse> {
        if request.pr_number == 0 || request.head_sha.trim().is_empty() {
            bail!("pr_number and head_sha are required")
        }
        if request.summary.trim().is_empty() {
            bail!("summary is required")
        }
        if request
            .findings
            .iter()
            .any(|finding| finding.body.trim().is_empty())
        {
            bail!("every fixture finding needs a body")
        }
        let (workdir, owner, repo) = if request.open {
            let workdir = request
                .workdir
                .as_ref()
                .filter(|path| path.exists())
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("open fixtures require an existing workdir"))?;
            let repository = request
                .repository
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("open fixtures require repository as owner/name"))?;
            let (owner, repo) = repository
                .split_once('/')
                .filter(|(owner, repo)| {
                    !owner.is_empty() && !repo.is_empty() && !repo.contains('/')
                })
                .ok_or_else(|| anyhow::anyhow!("repository must be owner/name"))?;
            (
                Some(workdir),
                Some(owner.to_string()),
                Some(repo.to_string()),
            )
        } else {
            (None, None, None)
        };
        let findings = request
            .findings
            .iter()
            .map(|finding| crate::app::ai_review::AiReviewFinding {
                path: finding.path.clone(),
                line: finding.line,
                side: finding.side,
                body: finding.body.clone(),
                diff_hunk: finding.diff_hunk.clone(),
                skipped: finding.skipped,
                published: finding.published,
            })
            .collect::<Vec<_>>();
        let count = findings.len();
        let entry = crate::app::ai_review::AiReviewCacheEntry {
            findings,
            last_run: Some(crate::app::ai_review::AiReviewRun {
                ran_at: chrono::Local::now(),
                outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(count),
            }),
            summary: Some(request.summary.clone()),
            // Without a fixture attribution the seeded row renders the bare
            // `— AI review via AMF` marker with no model/token/cost disclosure
            // line and no last-run memory line.
            attribution: request.attribution.clone(),
        };
        self.db
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("AI review cache database is unavailable"))?
            .save_ai_review_cache(request.pr_number, &request.head_sha, &entry)?;
        if let (Some(workdir), Some(owner), Some(repo)) = (workdir, owner, repo) {
            self.open_ai_review_for_pr(
                workdir,
                crate::github::PrRef {
                    number: request.pr_number,
                    head_sha: request.head_sha.clone(),
                    url: format!(
                        "https://github.com/{owner}/{repo}/pull/{}",
                        request.pr_number
                    ),
                    owner,
                    repo,
                    head_ref: request.head_ref.clone().unwrap_or_default(),
                },
            );
        }
        Ok(SeedAiReviewResponse {
            msg_type: crate::automation::AUTOMATION_RESULT_TYPE,
            action: crate::automation::SEED_AI_REVIEW_ACTION,
            ok: true,
            pr_number: request.pr_number,
            head_sha: request.head_sha.clone(),
            finding_count: count,
            opened: request.open,
            message: format!("Seeded {count} deterministic AI-review finding(s)"),
        })
    }
}

#[cfg(test)]
mod prompt_readiness_tests {
    use super::*;

    #[test]
    fn only_default_harness_input_surfaces_are_ready() {
        for (kind, screen) in [
            (
                SessionKind::Claude,
                "Claude Code\n❯ Try fixing a bug\n? for shortcuts",
            ),
            (
                SessionKind::Codex,
                "OpenAI Codex\n› Describe a task\n100% context left",
            ),
            (
                SessionKind::Opencode,
                "OpenCode\nAsk anything...\ntab switch agent",
            ),
            (
                SessionKind::Pi,
                "────────────────────\n\n────────────────────\n/project\n0.0%/200k (auto)",
            ),
        ] {
            assert!(automation_prompt_ready(&kind, screen), "{kind:?}");
            for not_ready in [
                "",
                "Starting…",
                "Do you trust this folder?",
                "Sign in to continue",
                "Do you trust this folder?\n❯ Yes",
                "Sign in with ChatGPT\n› Continue",
                "› 1. Yes, continue",
            ] {
                assert!(
                    !automation_prompt_ready(&kind, not_ready),
                    "{kind:?}: {not_ready}"
                );
            }
        }
    }
}
