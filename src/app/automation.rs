use std::path::Path;

use anyhow::{Result, bail};

use super::*;
use crate::automation::{
    BatchFeatureAutomationResult, CreateBatchFeaturesRequest, CreateBatchFeaturesResponse,
    CreateProjectRequest, CreateProjectResponse,
};

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
