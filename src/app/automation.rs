use std::path::Path;

use anyhow::{Result, bail};

use super::*;
use crate::automation::{
    AutomationHookPrompt, BatchFeatureAutomationResult, CreateBatchFeaturesRequest,
    CreateBatchFeaturesResponse, CreateFeatureRequest, CreateFeatureResponse, CreateProjectRequest,
    CreateProjectResponse,
};
use crate::extension::merge_project_extension_config;

impl App {
    fn ensure_notes_file(workdir: &Path) {
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
                    workdir: project_repo.join(".worktrees").join(&branch),
                    tmux_session: format!("amf-{}", branch),
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

    fn run_worktree_hook_sync(
        script: &str,
        workdir: &Path,
        choice: Option<&str>,
    ) -> (bool, Option<String>) {
        let expanded = Self::resolve_sync_hook_script(script);
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&expanded)
            .current_dir(workdir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        if let Some(c) = choice {
            cmd.env("AMF_HOOK_CHOICE", c);
        }

        match cmd.output() {
            Ok(output) if output.status.success() => (true, None),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let detail = stderr
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| line.trim().to_string())
                    .or_else(|| output.status.code().map(|code| format!("exit code {code}")));
                (false, detail)
            }
            Err(err) => (false, Some(err.to_string())),
        }
    }

    fn worktree_hook_prompt_for_repo(&self, project_repo: &Path) -> Option<AutomationHookPrompt> {
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

        if self.store.find_project(&request.project_name).is_some() {
            bail!("Project '{}' already exists", request.project_name);
        }

        let (project_path, is_git) = match self.worktree.repo_root(&request.path) {
            Ok(repo) => (repo, true),
            Err(_) => (request.path.clone(), false),
        };

        if request.dry_run {
            let message = format!("Dry run: would create project '{}'", request.project_name);
            return Ok(CreateProjectResponse::success(
                request,
                project_path,
                is_git,
                message,
            ));
        }

        let project = Project::new(request.project_name.clone(), project_path.clone(), is_git);
        self.store.add_project(project);
        self.save()?;

        let message = format!("Created project '{}'", request.project_name);
        Ok(CreateProjectResponse::success(
            request,
            project_path,
            is_git,
            message,
        ))
    }

    pub fn create_feature_from_request(
        &mut self,
        request: &CreateFeatureRequest,
    ) -> Result<CreateFeatureResponse> {
        if request.project_name.trim().is_empty() {
            bail!("Project name cannot be empty");
        }

        if request.branch.trim().is_empty() {
            bail!("Branch name cannot be empty");
        }

        if request.mode == VibeMode::Review {
            bail!("Use `review: true` with a non-review mode; `mode: review` is not supported");
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

            if project.features.iter().any(|f| f.name == request.branch) {
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
                "Agent '{}' is not allowed for this workspace",
                request.agent.display_name()
            );
        }

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

        if !use_worktree && has_non_worktree_feature {
            bail!("Only one non-worktree feature allowed per project");
        }

        let hook_prompt = if use_worktree {
            self.worktree_hook_prompt_for_repo(&project_repo)
        } else {
            None
        };
        let mut hook_ran = false;
        let mut hook_succeeded = None;
        let workdir = if use_worktree {
            let planned_workdir = project_repo.join(".worktrees").join(&request.branch);
            if planned_workdir.exists() {
                bail!("Worktree path already exists: {}", planned_workdir.display());
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
                format!("amf-{}", request.branch),
                false,
                hook_ran,
                hook_succeeded,
                hook_prompt.clone(),
                message,
            ));
        }

        let final_workdir = if use_worktree {
            let workdir = self
                .worktree
                .create(&project_repo, &request.branch, &request.branch)?;

            let ext = merge_project_extension_config(&self.config.extension, &project_repo);
            if let Some(ref hook_cfg) = ext.lifecycle_hooks.on_worktree_created {
                hook_ran = true;
                let prompt = hook_cfg.prompt();
                if let Some(prompt_cfg) = prompt {
                    let choice = request.hook_choice.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Worktree hook requires a choice; provide `hook_choice` from [{}]",
                            prompt_cfg.options.join(", ")
                        )
                    })?;
                    if !prompt_cfg.options.iter().any(|option| option == choice) {
                        bail!(
                            "Invalid hook_choice '{}'; expected one of [{}]",
                            choice,
                            prompt_cfg.options.join(", ")
                        );
                    }
                    let (success, detail) =
                        Self::run_worktree_hook_sync(hook_cfg.script(), &workdir, Some(choice));
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
                } else {
                    let (success, detail) =
                        Self::run_worktree_hook_sync(hook_cfg.script(), &workdir, None);
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
            }

            workdir
        } else {
            project_repo.clone()
        };

        if request.enable_notes {
            Self::ensure_notes_file(&final_workdir);
        }

        let feature = Feature::new(
            request.branch.clone(),
            request.branch.clone(),
            final_workdir.clone(),
            use_worktree,
            request.mode.clone(),
            request.review,
            request.plan_mode,
            request.agent.clone(),
            request.enable_chrome,
            request.enable_notes,
        );

        self.store.add_feature(&request.project_name, feature);
        self.save()?;

        let pi = self
            .store
            .projects
            .iter()
            .position(|p| p.name == request.project_name)
            .ok_or_else(|| anyhow::anyhow!("Project '{}' missing after feature add", request.project_name))?;
        let fi = self.store.projects[pi].features.len().saturating_sub(1);
        self.store.projects[pi].collapsed = project_collapsed;
        self.ensure_feature_running(pi, fi)?;
        self.save()?;

        let message = match hook_succeeded {
            Some(true) => format!("Created and started feature '{}' (hook succeeded)", request.branch),
            Some(false) => format!("Created and started feature '{}' (hook failed)", request.branch),
            None => format!("Created and started feature '{}'", request.branch),
        };

        Ok(CreateFeatureResponse::success(
            request,
            final_workdir,
            use_worktree,
            format!("amf-{}", request.branch),
            true,
            hook_ran,
            hook_succeeded,
            hook_prompt,
            message,
        ))
    }

    pub fn create_batch_features_from_request(
        &mut self,
        request: &CreateBatchFeaturesRequest,
    ) -> Result<CreateBatchFeaturesResponse> {
        if request.workspace_path.as_os_str().is_empty() || !request.workspace_path.exists() {
            bail!("Workspace path is invalid");
        }

        if request.project_name.trim().is_empty() {
            bail!("Project name cannot be empty");
        }

        if self.store.find_project(&request.project_name).is_some() {
            bail!("Project '{}' already exists", request.project_name);
        }

        if request.feature_count == 0 {
            bail!("Feature count must be at least 1");
        }

        if request.feature_prefix.trim().is_empty() {
            bail!("Feature prefix cannot be empty");
        }

        if request.mode == VibeMode::Review {
            bail!("Use `review: true` with a non-review mode; `mode: review` is not supported");
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
                "Agent '{}' is not allowed for this workspace",
                request.agent.display_name()
            );
        }

        let planned_features = Self::planned_batch_feature_results(request, &project_repo);
        for feature in &planned_features {
            if feature.workdir.exists() {
                bail!("Worktree path already exists: {}", feature.workdir.display());
            }
        }

        if request.dry_run {
            let message = format!(
                "Dry run: would create project '{}' with {} features",
                request.project_name,
                request.feature_count
            );
            return Ok(CreateBatchFeaturesResponse::success(
                request,
                project_repo,
                planned_features,
                message,
            ));
        }

        let project = Project::new(request.project_name.clone(), project_repo.clone(), is_git);
        self.store.add_project(project);
        self.save()?;

        let pi = self.store.projects.len().saturating_sub(1);
        let mut created_feature_names = Vec::new();
        let mut response_features = Vec::new();

        for planned in planned_features {
            let workdir = self
                .worktree
                .create(&project_repo, &planned.branch, &planned.branch)?;

            if request.enable_notes {
                Self::ensure_notes_file(&workdir);
            }

            let feature = Feature::new(
                planned.name.clone(),
                planned.branch.clone(),
                workdir.clone(),
                true,
                request.mode.clone(),
                request.review,
                false,
                request.agent.clone(),
                request.enable_chrome,
                request.enable_notes,
            );

            self.store.add_feature(&request.project_name, feature);
            created_feature_names.push(planned.name.clone());
            response_features.push(BatchFeatureAutomationResult {
                workdir,
                started: true,
                ..planned
            });
            self.save()?;
        }

        for branch in &created_feature_names {
            let fi = self.store.projects[pi]
                .features
                .iter()
                .position(|f| f.name == *branch)
                .ok_or_else(|| anyhow::anyhow!("Feature '{}' missing after creation", branch))?;

            self.ensure_feature_running(pi, fi)?;
            self.save()?;
        }

        let message = format!(
            "Created project '{}' with {} features",
            request.project_name, request.feature_count
        );

        Ok(CreateBatchFeaturesResponse::success(
            request,
            project_repo,
            response_features,
            message,
        ))
    }
}
