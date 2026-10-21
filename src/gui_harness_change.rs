//! Confirmed primary-harness changes with fresh checkout and process identity.
use crate::app::AppMode;
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};
use crate::project::{AgentKind, ProjectStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct HarnessChoice {
    pub agent: AgentKind,
    pub name: String,
    pub unavailable: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HarnessChangeView {
    pub target: FeatureTarget,
    pub feature_name: String,
    pub current_agent: AgentKind,
    pub running: bool,
    pub agent_rows: usize,
    pub choices: Vec<HarnessChoice>,
    pub revision: String,
}

#[derive(Debug, Deserialize)]
pub struct HarnessChangeRequest {
    pub target: FeatureTarget,
    pub revision: String,
    pub agent: AgentKind,
}

fn directory_identity(path: &Path) -> GuiResult<String> {
    let canonical = path.canonicalize().map_err(anyhow::Error::from)?;
    let metadata = canonical.metadata().map_err(anyhow::Error::from)?;
    if !metadata.is_dir() {
        return Err(GuiError::conflict("Checkout is no longer a directory"));
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = format!("{:?}", metadata.created().ok());
    Ok(format!("{}:{identity}", canonical.display()))
}

pub fn load(gui: &mut GuiHandle, target: FeatureTarget) -> GuiResult<HarnessChangeView> {
    gui.refresh_store()?;
    if gui
        .custom_session_adds
        .iter()
        .any(|(feature, _)| feature == &target.feature_id)
    {
        return Err(GuiError::conflict(
            "Wait for the configured session pre-check before changing harness",
        ));
    }
    // Strictly validate configuration before using the shared extension resolver,
    // whose fallback is useful at startup but unsuitable for a delayed mutation.
    let global = crate::gui_settings::agents_load()?;
    let project_settings = crate::gui_project_settings::load(gui, &target.project_id)?;
    let app = gui.app_for_workflow();
    if !matches!(app.mode, AppMode::Normal) {
        return Err(GuiError::conflict(
            "Finish or close the active workflow before changing harness",
        ));
    }
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .filter(|(pi, _)| app.store.projects[*pi].id == target.project_id)
        .ok_or_else(|| {
            GuiError::not_found("Feature was deleted or moved. Refresh the workspace.")
        })?;
    let project = &app.store.projects[pi];
    let feature = &project.features[fi];
    if !feature.is_worktree {
        return Err(GuiError::conflict(
            "Harness changes require a worktree feature",
        ));
    }
    if feature.pending_worktree_script || app.paused_plan_interview_belongs_to_feature(&feature.id)
    {
        return Err(GuiError::conflict(
            "Finish the feature's pending worktree script or parked plan before changing harness",
        ));
    }
    if app.feature_tmux_session_is_shared(pi, fi) {
        return Err(GuiError::conflict(
            "The feature shares its tmux session name. Resolve the ambiguity before changing harness.",
        ));
    }
    let repo_identity = directory_identity(&project.repo)?;
    let checkout_identity = directory_identity(&feature.workdir)?;
    // A real worktree has a gitdir file. Bind both that file and the directory
    // it names, so replacing/repointing a checkout invalidates confirmation.
    let git_file =
        std::fs::read_to_string(feature.workdir.join(".git")).map_err(anyhow::Error::from)?;
    let git_path = git_file
        .trim()
        .strip_prefix("gitdir: ")
        .ok_or_else(|| GuiError::conflict("The checkout no longer has a worktree Git identity"))?;
    let git_identity = directory_identity(&feature.workdir.join(git_path))?;
    let git_head = std::process::Command::new("git")
        .arg("-C")
        .arg(&feature.workdir)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(anyhow::Error::from)?;
    if !git_head.status.success() {
        return Err(GuiError::conflict("Cannot observe the checkout's Git HEAD"));
    }
    let git_branch = std::process::Command::new("git")
        .arg("-C")
        .arg(&feature.workdir)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .output()
        .map_err(anyhow::Error::from)?;
    let head_identity = (
        git_head.stdout,
        git_branch.status.success(),
        git_branch.stdout,
    );
    let exists = app.tmux.session_exists(&feature.tmux_session);
    let mut panes: Vec<_> = app
        .tmux
        .list_panes()
        .into_iter()
        .filter(|(session, _, _)| session == &feature.tmux_session)
        .collect();
    panes.sort();
    if exists && panes.is_empty() {
        return Err(GuiError::conflict(
            "Cannot observe the running feature's panes. Refresh before changing harness.",
        ));
    }
    let mut choices = Vec::new();
    for agent in app.extension_for_repo(&project.repo).allowed_agents() {
        let unavailable = if !feature.mode.supports_agent(&agent) {
            Some(format!(
                "Unavailable in {} mode",
                feature.mode.display_name()
            ))
        } else {
            app.tmux
                .check_harness_available(&agent)
                .err()
                .map(|error| format!("Not installed or unavailable: {error}"))
        };
        choices.push(HarnessChoice {
            name: agent.display_name().to_string(),
            agent,
            unavailable,
        });
    }
    // Bind only what the change acts on. Status sync, hook IPC (last access,
    // token-source bindings) and summaries rewrite other fields on a running
    // feature, and hashing those would make confirmation fail over and over.
    let sessions: Vec<_> = feature
        .sessions
        .iter()
        .map(|session| {
            (
                &session.id,
                &session.kind,
                &session.tmux_window,
                &session.claude_session_id,
                session.stopped,
                &session.command,
                &session.on_stop,
                &session.pre_check,
            )
        })
        .collect();
    let feature_binding = (
        &feature.id,
        &feature.workdir,
        &feature.tmux_session,
        &feature.agent,
        &feature.mode,
        feature.is_worktree,
        feature.pending_worktree_script,
        feature.status == ProjectStatus::Stopped,
        &sessions,
    );
    let bytes = serde_json::to_vec(&(
        &target,
        &feature_binding,
        &repo_identity,
        &checkout_identity,
        &git_file,
        &git_identity,
        &head_identity,
        exists,
        &panes,
        &choices,
        &global.revision,
        &project_settings.revision,
    ))
    .map_err(anyhow::Error::from)?;
    Ok(HarnessChangeView {
        target,
        feature_name: feature.name.clone(),
        current_agent: feature.agent.clone(),
        running: exists || feature.status != ProjectStatus::Stopped,
        agent_rows: feature
            .sessions
            .iter()
            .filter(|session| session.kind.is_agent_harness())
            .count(),
        choices,
        revision: format!("{:x}", Sha256::digest(bytes)),
    })
}

pub fn apply(gui: &mut GuiHandle, request: HarnessChangeRequest) -> GuiResult<()> {
    let current = load(gui, request.target.clone())?;
    if current.revision != request.revision {
        return Err(GuiError::conflict(
            "The feature, checkout, processes or harness choices changed. Reload and confirm again; nothing was stopped.",
        ));
    }
    let choice = current
        .choices
        .iter()
        .find(|choice| choice.agent == request.agent)
        .ok_or_else(|| GuiError::conflict("The replacement harness is no longer allowed"))?;
    if let Some(reason) = &choice.unavailable {
        return Err(GuiError::conflict(reason));
    }
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&request.target.project_id), &request.target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature no longer exists"))?;
    app.change_feature_harness(pi, fi, request.agent)
        .map_err(|error| {
            let message = format!("{error:#}");
            let mut response = GuiError::from(error);
            response.message = message;
            response
        })?;
    Ok(())
}
