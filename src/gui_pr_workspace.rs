//! Experimental GUI shell that combines PR files, existing PR Triage threads,
//! and AMF's existing AI PR review runner in one feature-scoped visit.
use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::{
    AppMode,
    ai_review::{AiReviewFinding, AiReviewStage},
};
use crate::github::PrRef;
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};

#[derive(Debug, Serialize)]
pub struct PrWorkspaceFiles {
    pub workflow_id: String,
    pub pr_number: u32,
    pub head_sha: String,
    pub files: Vec<crate::gui_diff::DiffFileView>,
}

pub fn files_request(gui: &GuiHandle, id: &str) -> GuiResult<(PathBuf, u32, String)> {
    check(gui, id)?;
    let context = gui.pr_workspace_context.as_ref().unwrap();
    let pr = context
        .pr
        .as_ref()
        .ok_or_else(|| GuiError::conflict("Select a PR in Threads first"))?;
    Ok((context.workdir.clone(), pr.number, pr.head_sha.clone()))
}

pub fn read_pr_diff(workdir: &Path, number: u32) -> anyhow::Result<String> {
    crate::github::GhCli::pr_diff(workdir, number)
}

pub fn files_result(
    id: &str,
    number: u32,
    sha: String,
    patch: String,
) -> GuiResult<PrWorkspaceFiles> {
    let files = crate::diff::parse_unified_diff(&patch)?
        .into_iter()
        .map(|file| crate::gui_diff::file_view(file, 3))
        .collect();
    Ok(PrWorkspaceFiles {
        workflow_id: id.into(),
        pr_number: number,
        head_sha: sha,
        files,
    })
}

pub(crate) struct PrWorkspaceContext {
    pub(crate) id: String,
    pub(crate) target: FeatureTarget,
    pub(crate) workdir: PathBuf,
    pub(crate) pr: Option<PrRef>,
    pub(crate) parked_threads: Option<Box<AppMode>>,
    pub(crate) parked_ai: Option<Box<AppMode>>,
    pub(crate) ai_active: bool,
    result_epoch: String,
    checkout_identity: CheckoutIdentity,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrWorkspaceAiView {
    pub workflow_id: String,
    pub pr_number: u32,
    pub head_sha: String,
    pub running: bool,
    pub stage: Option<String>,
    pub activity: Option<String>,
    pub elapsed_seconds: u64,
    pub token_estimate: Option<usize>,
    pub findings: Vec<AiReviewFinding>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub precall: Option<crate::gui_plans::PrecallView>,
    pub can_cancel: bool,
    pub result_revision: Option<String>,
}

pub fn begin(gui: &mut GuiHandle, target: FeatureTarget) -> GuiResult<String> {
    gui.refresh_store()?;
    if gui.pr_workspace_context.is_some() {
        let (id, existing_target, existing_workdir) = {
            let context = gui.pr_workspace_context.as_ref().unwrap();
            (
                context.id.clone(),
                context.target.clone(),
                context.workdir.clone(),
            )
        };
        if existing_target.project_id == target.project_id
            && existing_target.feature_id == target.feature_id
            && context_for_feature(gui.app_for_workflow(), &target)? == existing_workdir
        {
            let context = gui.pr_workspace_context.as_ref().unwrap();
            if checkout_identity(&context.workdir).as_ref() != Some(&context.checkout_identity) {
                return Err(GuiError::conflict(
                    "Feature checkout changed; close and reopen PR Workspace",
                ));
            }
            // React StrictMode replays mount effects in development. Treat a
            // second open for the same feature as the same workspace instead
            // of leaving its first initialization orphaned.
            return Ok(id);
        }
        // A stale frontend mount can leave its context behind. Retire it when
        // it is safe to do so before opening another feature; close still
        // refuses while a review runner or confirmation is active.
        close(gui, &id)?;
    }
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted"))?;
    if !app.store.projects[pi].is_git {
        return Err(GuiError::conflict("PR Workspace requires a Git feature"));
    }
    let workdir = app.store.projects[pi].features[fi].workdir.clone();
    let identity = checkout_identity(&workdir).ok_or_else(|| {
        GuiError::conflict("Feature checkout is unavailable or is not a Git checkout")
    })?;
    let id = uuid::Uuid::new_v4().to_string();
    gui.pr_workspace_context = Some(PrWorkspaceContext {
        id: id.clone(),
        target,
        workdir,
        pr: None,
        parked_threads: None,
        parked_ai: None,
        ai_active: false,
        result_epoch: uuid::Uuid::new_v4().to_string(),
        checkout_identity: identity,
    });
    Ok(id)
}

fn check(gui: &GuiHandle, id: &str) -> GuiResult<()> {
    if gui
        .pr_workspace_context
        .as_ref()
        .is_some_and(|c| c.id == id)
    {
        Ok(())
    } else {
        Err(GuiError::conflict("PR Workspace is no longer open"))
    }
}

pub fn select_pr(gui: &mut GuiHandle, id: &str, number: u32) -> GuiResult<()> {
    check(gui, id)?;
    if number == 0 {
        gui.pr_workspace_context.as_mut().unwrap().pr = None;
        return Ok(());
    }
    let (app, workspace) = gui.app_and_pr_workspace();
    let context = workspace.as_ref().unwrap();
    let target = context.target.clone();
    let context_workdir = context.workdir.clone();
    // Once a PR has been selected, its identity remains valid while the AI
    // pane is active or its background run finishes. The embedded triage
    // panel can re-emit the same selection during that transition; keep the
    // cached ref instead of requiring AppMode::PrReview to be foregrounded.
    if !matches!(&app.mode, AppMode::PrReview(state) if state.review.pr.number == number)
        && context
            .pr
            .as_ref()
            .is_some_and(|selected| selected.number == number)
    {
        if context_workdir != context_for_feature(app, &target)? {
            return Err(GuiError::conflict(
                "Feature checkout changed; reopen PR Workspace",
            ));
        }
        return Ok(());
    }
    let pr = match &app.mode {
        AppMode::PrReview(state) if state.review.pr.number == number => state.review.pr.clone(),
        _ => {
            return Err(GuiError::conflict(
                "Open the PR in Threads before loading its workspace",
            ));
        }
    };
    if context_workdir != context_for_feature(app, &target)? {
        return Err(GuiError::conflict(
            "Feature checkout changed; reopen PR Workspace",
        ));
    }
    workspace.as_mut().unwrap().pr = Some(pr);
    Ok(())
}

fn context_for_feature(app: &crate::app::App, target: &FeatureTarget) -> GuiResult<PathBuf> {
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted"))?;
    Ok(app.store.projects[pi].features[fi].workdir.clone())
}

fn mode_matches_pr(mode: &AppMode, workdir: &Path, pr: &PrRef) -> bool {
    match mode {
        AppMode::AiReview(state) => {
            state.workdir == workdir
                && state.pr.number == pr.number
                && state.pr.head_sha == pr.head_sha
        }
        AppMode::AiReviewRunning(state) => {
            state.origin.workdir == workdir
                && state.origin.pr.number == pr.number
                && state.origin.pr.head_sha == pr.head_sha
        }
        AppMode::PromptPrecall(pending) => mode_matches_pr(&pending.prior_mode, workdir, pr),
        _ => false,
    }
}

pub fn switch_to_ai(gui: &mut GuiHandle, id: &str, number: u32) -> GuiResult<()> {
    check(gui, id)?;
    let (workdir, pr, already_ai) = {
        let context = gui.pr_workspace_context.as_ref().unwrap();
        (
            context.workdir.clone(),
            context.pr.clone().filter(|pr| pr.number == number),
            context.ai_active,
        )
    };
    let pr = pr.ok_or_else(|| GuiError::conflict("Select a PR in Threads first"))?;
    let (app, workspace) = gui.app_and_pr_workspace();
    let Some(context) = workspace.as_mut() else {
        return Err(GuiError::conflict("PR Workspace is no longer open"));
    };
    if already_ai {
        if mode_matches_pr(&app.mode, &workdir, &pr) || app.ai_review_run.is_pending() {
            return Ok(());
        }
        context.parked_ai = Some(Box::new(std::mem::replace(&mut app.mode, AppMode::Normal)));
        app.open_ai_review_for_pr(workdir, pr);
        return Ok(());
    }
    let old = std::mem::replace(&mut app.mode, AppMode::Normal);
    context.parked_threads = Some(Box::new(old));
    let saved = context.parked_ai.take();
    if saved
        .as_deref()
        .is_some_and(|mode| mode_matches_pr(mode, &workdir, &pr))
    {
        app.mode = *saved.unwrap();
    } else if saved.is_some() && app.ai_review_run.is_pending() {
        // The shared runner can own only one PR at a time. Restore its pending
        // pane so the user can see progress or cancel it after a PR switch.
        app.mode = *saved.unwrap();
    } else {
        app.open_ai_review_for_pr(workdir, pr);
    }
    context.ai_active = true;
    Ok(())
}

pub fn switch_to_threads(gui: &mut GuiHandle, id: &str) -> GuiResult<()> {
    check(gui, id)?;
    let (app, workspace) = gui.app_and_pr_workspace();
    if let Some(context) = workspace.as_mut()
        && context.ai_active
    {
        let ai = std::mem::replace(&mut app.mode, AppMode::Normal);
        context.parked_ai = Some(Box::new(ai));
        app.mode = *context
            .parked_threads
            .take()
            .ok_or_else(|| GuiError::conflict("Threads view expired"))?;
        context.ai_active = false;
    }
    Ok(())
}

pub fn close(gui: &mut GuiHandle, id: &str) -> GuiResult<()> {
    check(gui, id)?;
    let (app, workspace) = gui.app_and_pr_workspace();
    let waiting_for_user = |mode: &AppMode| {
        matches!(mode, AppMode::PromptPrecall(pending)
        if matches!(pending.prior_mode.as_ref(), AppMode::AiReview(_) | AppMode::AiReviewRunning(_)))
    };
    if app.ai_review_run.is_pending()
        || waiting_for_user(&app.mode)
        || workspace
            .as_ref()
            .is_some_and(|c| c.parked_ai.as_deref().is_some_and(waiting_for_user))
    {
        return Err(GuiError::conflict(
            "Continue or cancel the AI review before closing PR Workspace",
        ));
    }
    let context = workspace.take().unwrap();
    if context.ai_active {
        app.mode = *context
            .parked_threads
            .ok_or_else(|| GuiError::conflict("Threads view expired"))?;
    }
    Ok(())
}

pub fn ai_snapshot(gui: &mut GuiHandle, id: &str) -> GuiResult<PrWorkspaceAiView> {
    check(gui, id)?;
    let context = gui.pr_workspace_context.as_ref().unwrap();
    let identity = checkout_identity(&context.workdir);
    if identity.as_ref() != Some(&context.checkout_identity) {
        return Err(GuiError::conflict(
            "Feature checkout changed; reopen PR Workspace",
        ));
    }
    let (app, workspace) = gui.app_and_pr_workspace();
    let context = workspace.as_mut().unwrap();
    // A snapshot may arrive while Threads is visible (the frontend can still
    // have an AI poll in flight). Borrow the AI mode for the snapshot, but
    // preserve the live Threads mode in its own slot and restore it below.
    let swap = !context.ai_active && context.parked_ai.is_some();
    if swap {
        let ai = *context.parked_ai.take().unwrap();
        context.parked_threads = Some(Box::new(std::mem::replace(&mut app.mode, ai)));
    }
    app.poll_ai_pr_review_bg();
    let selected_pr = context.pr.clone();
    let result_revision = match &app.mode {
        AppMode::AiReview(state) => Some(result_revision(&context.result_epoch, state, &identity)),
        _ => None,
    };
    let (findings, summary, error) = match &app.mode {
        AppMode::AiReview(state) => (
            state.findings.clone(),
            state.summary.clone(),
            state
                .post_confirm
                .as_ref()
                .and_then(|p| p.error.clone())
                .or_else(|| {
                    state.last_run.as_ref().and_then(|r| match &r.outcome {
                        crate::app::ai_review::AiReviewRunOutcome::Error(e) => Some(e.clone()),
                        _ => None,
                    })
                }),
        ),
        AppMode::AiReviewRunning(state) => (
            state.origin.findings.clone(),
            state.origin.summary.clone(),
            None,
        ),
        _ => (Vec::new(), None, None),
    };
    let ai_identity = match &app.mode {
        AppMode::AiReview(state) => Some((state.pr.number, state.pr.head_sha.clone())),
        AppMode::AiReviewRunning(state) => {
            Some((state.origin.pr.number, state.origin.pr.head_sha.clone()))
        }
        AppMode::PromptPrecall(pending) => match pending.prior_mode.as_ref() {
            AppMode::AiReview(state) => Some((state.pr.number, state.pr.head_sha.clone())),
            _ => None,
        },
        _ => None,
    };
    let selected_identity = ai_identity.or_else(|| selected_pr.map(|pr| (pr.number, pr.head_sha)));
    let pre = match &app.mode {
        AppMode::PromptPrecall(pending)
            if matches!(pending.prior_mode.as_ref(), AppMode::AiReview(_)) =>
        {
            Some(crate::gui_plans::PrecallView {
                title: pending.prompt_id.spec().title.to_string(),
                harness: pending.harness.display_name().into(),
                preview: pending.preview.clone(),
                viewing: pending.viewing,
            })
        }
        _ => None,
    };
    let (stage, activity, token_estimate, running) = match &app.mode {
        AppMode::AiReviewRunning(state) => match state.progress.stage {
            AiReviewStage::PreparingDiff => (
                Some("Fetching PR diff".into()),
                state.progress.activity.clone(),
                None,
                true,
            ),
            AiReviewStage::Reviewing { token_estimate } => (
                Some("Reviewing code".into()),
                state.progress.activity.clone(),
                Some(token_estimate),
                true,
            ),
        },
        _ => (None, None, None, false),
    };
    let elapsed_seconds = app
        .ai_review_run
        .progress()
        .as_ref()
        .map(|progress| progress.started_at.elapsed().as_secs())
        .unwrap_or_default();
    if swap {
        context.parked_ai = Some(Box::new(std::mem::replace(&mut app.mode, AppMode::Normal)));
        if let Some(threads) = context.parked_threads.take() {
            app.mode = *threads;
        }
    }
    let (pr_number, head_sha) =
        selected_identity.ok_or_else(|| GuiError::conflict("Select a PR in Threads first"))?;
    let can_cancel = running || pre.is_some();
    Ok(PrWorkspaceAiView {
        workflow_id: id.into(),
        pr_number,
        head_sha,
        running,
        stage,
        activity,
        elapsed_seconds,
        token_estimate,
        findings,
        summary,
        error,
        precall: pre,
        can_cancel,
        result_revision,
    })
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AiAction {
    Start,
    Cancel,
    ConfirmPrecall,
    CancelPrecall,
    Post,
    /// Skip (or restore) one finding so it is left out of Post and of a fix
    /// handoff. Bound to the result and PR head the person was looking at.
    ToggleSkip {
        index: usize,
        result_revision: String,
        head_sha: String,
    },
    /// Replace one finding's body before posting.
    EditBody {
        index: usize,
        body: String,
        result_revision: String,
        head_sha: String,
    },
}

/// Longest finding body an edit may save (characters).
pub const MAX_FINDING_BODY_CHARS: usize = 8000;

/// Refuse a skip or edit unless it targets the exact result, PR head and
/// finding the person was looking at. Nothing is retargeted: a changed result
/// (new run, other edit, other skip) must be reviewed again first.
fn finding_edit_guard(
    gui: &mut GuiHandle,
    result_revision_seen: &str,
    head_sha: &str,
    index: usize,
) -> GuiResult<()> {
    let context = gui.pr_workspace_context.as_ref().unwrap();
    let identity = checkout_identity(&context.workdir);
    if identity.as_ref() != Some(&context.checkout_identity) {
        return Err(GuiError::conflict(
            "Feature checkout changed; reopen PR Workspace",
        ));
    }
    let epoch = context.result_epoch.clone();
    let app = gui.app_for_workflow();
    let AppMode::AiReview(state) = &app.mode else {
        return Err(GuiError::conflict(
            "Wait for the AI review to finish before changing its findings",
        ));
    };
    if result_revision(&epoch, state, &identity) != result_revision_seen {
        return Err(GuiError::conflict(
            "The AI result changed since you opened it; review the current findings and try again",
        ));
    }
    if state.pr.head_sha != head_sha {
        return Err(GuiError::conflict(
            "The AI result is for a different PR head; review the current findings and try again",
        ));
    }
    let finding = state.findings.get(index).ok_or_else(|| {
        GuiError::conflict("That finding is no longer in the result; review the current findings")
    })?;
    if finding.published {
        return Err(GuiError::conflict(
            "That finding was already posted to GitHub; follow it up in PR Triage",
        ));
    }
    Ok(())
}

pub fn log_ai_action_start(gui: &mut GuiHandle, action: &AiAction) {
    gui.app_for_workflow().log_info(
        "pr_workspace",
        format!("AI review action started: {action:?}"),
    );
}

pub fn log_ai_action_finish(
    gui: &mut GuiHandle,
    action: &str,
    elapsed: std::time::Duration,
    error: Option<&str>,
) {
    let message = format!(
        "AI review action {action} {} in {:.1}s{}",
        if error.is_some() {
            "failed"
        } else {
            "completed"
        },
        elapsed.as_secs_f32(),
        error.map(|e| format!(": {e}")).unwrap_or_default()
    );
    if error.is_some() {
        gui.app_for_workflow().log_warn("pr_workspace", message);
    } else {
        gui.app_for_workflow().log_debug("pr_workspace", message);
    }
}

pub fn log_slow_ai_snapshot(gui: &mut GuiHandle, elapsed: std::time::Duration) {
    if elapsed >= std::time::Duration::from_secs(2) {
        gui.app_for_workflow().log_warn(
            "pr_workspace",
            format!("AI review snapshot took {:.1}s", elapsed.as_secs_f32()),
        );
    }
}

pub fn log_ai_tab_switch(
    gui: &mut GuiHandle,
    number: u32,
    elapsed: std::time::Duration,
    failed: bool,
) {
    let message = format!(
        "Switch to AI review for PR #{number} {} in {:.1}ms",
        if failed { "failed" } else { "completed" },
        elapsed.as_secs_f64() * 1000.0
    );
    if failed {
        gui.app_for_workflow().log_warn("pr_workspace", message);
    } else {
        gui.app_for_workflow().log_debug("pr_workspace", message);
    }
}

pub fn ai_act(gui: &mut GuiHandle, id: &str, action: AiAction) -> GuiResult<PrWorkspaceAiView> {
    check(gui, id)?;
    let (ai_active, target, workdir, pr_number) = {
        let context = gui.pr_workspace_context.as_ref().unwrap();
        (
            context.ai_active,
            context.target.clone(),
            context.workdir.clone(),
            context.pr.as_ref().map(|pr| pr.number),
        )
    };
    if !ai_active {
        let number = pr_number.ok_or_else(|| GuiError::conflict("Select a PR in Threads first"))?;
        switch_to_ai(gui, id, number)?;
    }
    match &action {
        AiAction::ToggleSkip {
            index,
            result_revision,
            head_sha,
        } => finding_edit_guard(gui, result_revision, head_sha, *index)?,
        AiAction::EditBody {
            index,
            body,
            result_revision,
            head_sha,
        } => {
            if body.trim().is_empty() {
                return Err(GuiError::conflict(
                    "A finding needs a body; skip it instead to leave it out",
                ));
            }
            if body.chars().count() > MAX_FINDING_BODY_CHARS {
                return Err(GuiError::conflict(format!(
                    "A finding body can be at most {MAX_FINDING_BODY_CHARS} characters"
                )));
            }
            finding_edit_guard(gui, result_revision, head_sha, *index)?
        }
        _ => {}
    }
    if matches!(action, AiAction::Start) {
        gui.pr_workspace_context.as_mut().unwrap().result_epoch = uuid::Uuid::new_v4().to_string();
    }
    let app = gui.app_for_workflow();
    match action {
        AiAction::Start => {
            let (pi, _) = app
                .store
                .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
                .ok_or_else(|| GuiError::not_found("Feature was deleted"))?;
            let allowed = app.allowed_agents_for_project_path(&workdir);
            let preferred = app.store.projects[pi].preferred_agent.clone();
            let default_harness = allowed
                .iter()
                .find(|agent| **agent == preferred)
                .cloned()
                .or_else(|| allowed.first().cloned());
            if default_harness.is_none() {
                return Err(GuiError::conflict(
                    "No AI harness is enabled for this project",
                ));
            }
            if let AppMode::AiReview(state) = &mut app.mode {
                if state.harness.is_none() {
                    state.harness = default_harness;
                }
                state.model_picked = true;
            }
            app.start_ai_pr_review();
        }
        AiAction::Cancel => {
            if let Some(origin) = app.ai_review_run.cancel() {
                if matches!(app.mode, AppMode::AiReviewRunning(_)) {
                    app.mode = AppMode::AiReview(origin);
                } else if let Some(workspace) = gui.pr_workspace_context.as_mut() {
                    // A run can be parked while another pane is foregrounded.
                    // Keep its cancelled origin in the AI slot so restoring
                    // that pane cannot resurrect a runnerless "running" view.
                    workspace.parked_ai = Some(Box::new(AppMode::AiReview(origin)));
                }
            }
        }
        AiAction::ConfirmPrecall => app.precall_confirm().map_err(GuiError::from)?,
        AiAction::CancelPrecall => app.precall_cancel(),
        AiAction::ToggleSkip { index, .. } => {
            if let AppMode::AiReview(state) = &mut app.mode {
                state.selected = index;
            }
            app.ai_review_toggle_skip();
        }
        AiAction::EditBody { index, body, .. } => {
            if let AppMode::AiReview(state) = &mut app.mode {
                state.selected = index;
            }
            app.ai_review_edit_finding();
            if let AppMode::AiReview(state) = &mut app.mode {
                state.finding_editor = Some(crate::editor::TextEditor::new(body));
            }
            app.ai_review_stop_edit_finding();
        }
        AiAction::Post => {
            if !matches!(app.mode, AppMode::AiReview(_)) {
                return Err(GuiError::conflict(
                    "Wait for the AI review to finish before posting findings",
                ));
            }
            app.ai_review_open_post_confirm();
            if app.ai_review_post_confirm_view().is_none() {
                return Err(GuiError::conflict(
                    "There are no unpublished findings to post",
                ));
            }
            app.ai_review_post().map_err(GuiError::from)?;
        }
    }
    ai_snapshot(gui, id)
}

fn digest(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("serializable revision"))
    )
}

// Bind the checkout directory, Git marker and worktree gitdir to detect
// replacement at an otherwise unchanged path.
#[derive(Serialize, PartialEq, Eq)]
pub(crate) struct CheckoutIdentity {
    directory: (PathBuf, u64, u64),
    marker: (PathBuf, u64, u64),
    marker_contents: Option<Vec<u8>>,
    gitdir: (PathBuf, u64, u64),
}

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

// No portable device/inode pair off Unix: identity falls back to the
// canonical paths and the `.git` pointer contents.
#[cfg(not(unix))]
fn file_id(_: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

pub(crate) fn checkout_identity(path: &Path) -> Option<CheckoutIdentity> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let directory = std::fs::metadata(&canonical).ok()?;
    if !directory.is_dir() {
        return None;
    }
    let marker = std::fs::canonicalize(canonical.join(".git")).ok()?;
    let meta = std::fs::metadata(&marker).ok()?;
    let (contents, gitdir) = if meta.is_file() {
        let bytes = std::fs::read(&marker).ok()?;
        let pointer = std::str::from_utf8(&bytes)
            .ok()?
            .trim_end_matches(['\r', '\n'])
            .strip_prefix("gitdir: ")?;
        let target = Path::new(pointer);
        let target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            canonical.join(target)
        };
        (Some(bytes), std::fs::canonicalize(target).ok()?)
    } else if meta.is_dir() {
        (None, marker.clone())
    } else {
        return None;
    };
    let gitdir_meta = std::fs::metadata(&gitdir).ok()?;
    if !gitdir_meta.is_dir() {
        return None;
    }
    Some(CheckoutIdentity {
        directory: (canonical, file_id(&directory).0, file_id(&directory).1),
        marker: (marker, file_id(&meta).0, file_id(&meta).1),
        marker_contents: contents,
        gitdir: (gitdir, file_id(&gitdir_meta).0, file_id(&gitdir_meta).1),
    })
}

pub(crate) fn destination_revision(
    feature: &crate::project::Feature,
    session: &crate::project::FeatureSession,
) -> String {
    destination_revision_with(feature, session, &checkout_identity(&feature.workdir))
}

/// As `destination_revision`, for callers that already hold the feature's
/// checkout identity (it is filesystem-backed, so compute it once per feature).
pub(crate) fn destination_revision_with(
    feature: &crate::project::Feature,
    session: &crate::project::FeatureSession,
    identity: &Option<CheckoutIdentity>,
) -> String {
    digest(&(
        &feature.id,
        &feature.workdir,
        identity,
        &feature.branch,
        &feature.tmux_session,
        feature.status == crate::project::ProjectStatus::Stopped,
        &session.id,
        &session.kind,
        &session.tmux_window,
        &session.claude_session_id,
        session.stopped,
        &session.created_at,
    ))
}

fn result_revision(
    epoch: &str,
    state: &crate::app::pr_review::state::AiReviewState,
    identity: &Option<CheckoutIdentity>,
) -> String {
    digest(&(
        epoch,
        identity,
        &state.pr,
        &state.findings,
        &state.summary,
        &state.last_run,
    ))
}

pub fn fix_targets(
    gui: &mut GuiHandle,
    id: &str,
) -> GuiResult<Vec<crate::gui_pr_triage::PrFixTargetView>> {
    gui.refresh_store()?;
    check(gui, id)?;
    let (app, workspace) = gui.app_and_pr_workspace();
    let target = &workspace.as_ref().unwrap().target;
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature was deleted"))?;
    let feature = &app.store.projects[pi].features[fi];
    Ok(feature
        .sessions
        .iter()
        .filter_map(|session| {
            let harness = match session.kind {
                crate::project::SessionKind::Claude => crate::project::AgentKind::Claude,
                crate::project::SessionKind::Codex => crate::project::AgentKind::Codex,
                crate::project::SessionKind::Opencode => crate::project::AgentKind::Opencode,
                crate::project::SessionKind::Pi => crate::project::AgentKind::Pi,
                _ => return None,
            };
            Some(crate::gui_pr_triage::PrFixTargetView {
                target: crate::gui_contract::SessionTarget {
                    project_id: target.project_id.clone(),
                    feature_id: feature.id.clone(),
                    session_id: session.id.clone(),
                },
                label: session.label.clone(),
                harness,
                stopped: feature.status == crate::project::ProjectStatus::Stopped
                    || session.stopped
                    || !app
                        .tmux
                        .window_exists(&feature.tmux_session, &session.tmux_window),
                destination_revision: destination_revision(feature, session),
            })
        })
        .collect())
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixHandoffRequest {
    pub workflow_id: String,
    pub result_revision: String,
    pub finding_indices: Vec<usize>,
    pub target: crate::gui_contract::SessionTarget,
    pub destination_revision: String,
}

#[derive(Clone)]
pub struct FixRead {
    workdir: PathBuf,
    pr: PrRef,
    request: FixHandoffRequest,
}

pub fn prepare_fix_read(gui: &mut GuiHandle, request: FixHandoffRequest) -> GuiResult<FixRead> {
    validate_fix(gui, &request)?;
    let context = gui.pr_workspace_context.as_ref().unwrap();
    Ok(FixRead {
        workdir: context.workdir.clone(),
        pr: context.pr.clone().unwrap(),
        request,
    })
}

pub fn fetch_fix_pr(read: &FixRead) -> anyhow::Result<PrRef> {
    crate::github::GhCli::fetch_pr_by_number(&read.workdir, read.pr.number)
}

pub fn finish_fix_read(
    gui: &mut GuiHandle,
    read: FixRead,
    current: PrRef,
) -> GuiResult<crate::gui_pr_triage::PrFixHandoff> {
    if current.number != read.pr.number
        || current.head_sha != read.pr.head_sha
        || current.owner != read.pr.owner
        || current.repo != read.pr.repo
    {
        return Err(GuiError::conflict(
            "The PR changed; refresh before preparing a fix",
        ));
    }
    let context = gui
        .pr_workspace_context
        .as_ref()
        .ok_or_else(|| GuiError::conflict("PR Workspace closed"))?;
    let selected = context
        .pr
        .as_ref()
        .ok_or_else(|| GuiError::conflict("PR selection changed"))?;
    if context.workdir != read.workdir
        || selected.number != read.pr.number
        || selected.head_sha != read.pr.head_sha
        || selected.owner != read.pr.owner
        || selected.repo != read.pr.repo
    {
        return Err(GuiError::conflict(
            "PR selection changed while checking its head",
        ));
    }
    validate_fix(gui, &read.request)
}

fn validate_fix(
    gui: &mut GuiHandle,
    request: &FixHandoffRequest,
) -> GuiResult<crate::gui_pr_triage::PrFixHandoff> {
    gui.refresh_store()?;
    check(gui, &request.workflow_id)?;
    let (app, workspace) = gui.app_and_pr_workspace();
    let context = workspace.as_ref().unwrap();
    let pr = context
        .pr
        .as_ref()
        .ok_or_else(|| GuiError::conflict("Select a PR first"))?;
    let identity = checkout_identity(&context.workdir);
    if context_for_feature(app, &context.target)? != context.workdir
        || identity.as_ref() != Some(&context.checkout_identity)
    {
        return Err(GuiError::conflict(
            "Feature checkout changed or is unavailable; reopen PR Workspace",
        ));
    }
    let state = match &app.mode {
        AppMode::AiReview(state)
            if context.ai_active
                && state.pr == *pr
                && mode_matches_pr(&app.mode, &context.workdir, pr) =>
        {
            state
        }
        _ => {
            return Err(GuiError::conflict(
                "The AI review changed; refresh before preparing a fix",
            ));
        }
    };
    if matches!(
        state.last_run.as_ref().map(|run| &run.outcome),
        Some(crate::app::ai_review::AiReviewRunOutcome::Error(_))
    ) {
        return Err(GuiError::conflict(
            "The latest AI review failed; run it again before preparing a fix",
        ));
    }
    if result_revision(&context.result_epoch, state, &identity) != request.result_revision {
        return Err(GuiError::conflict(
            "The AI findings changed; refresh before preparing a fix",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    if request.finding_indices.is_empty()
        || request.finding_indices.iter().any(|i| !seen.insert(*i))
    {
        return Err(GuiError::conflict("Choose one or more distinct findings"));
    }
    let mut findings = Vec::new();
    for &index in &request.finding_indices {
        let finding = state
            .findings
            .get(index)
            .filter(|f| !f.skipped && !f.published)
            .ok_or_else(|| GuiError::conflict("A finding is no longer available"))?;
        findings.push(finding);
    }
    if request.target.project_id != context.target.project_id
        || request.target.feature_id != context.target.feature_id
    {
        return Err(GuiError::conflict("Choose a session in this feature"));
    }
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&request.target.project_id), &request.target.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature was deleted"))?;
    let feature = &app.store.projects[pi].features[fi];
    let session = feature
        .sessions
        .iter()
        .find(|s| s.id == request.target.session_id && s.kind.is_agent_harness())
        .ok_or_else(|| GuiError::conflict("The selected agent session is unavailable"))?;
    if checkout_identity(&feature.workdir).is_none()
        || app
            .store
            .projects
            .iter()
            .flat_map(|p| &p.features)
            .filter(|f| f.tmux_session == feature.tmux_session)
            .count()
            != 1
        || feature.status == crate::project::ProjectStatus::Stopped
        || session.stopped
        || destination_revision(feature, session) != request.destination_revision
        || !app
            .tmux
            .window_exists(&feature.tmux_session, &session.tmux_window)
    {
        return Err(GuiError::conflict(
            "The selected agent session changed or stopped; choose it again",
        ));
    }
    let label = if findings.len() == 1 {
        "AI review finding".to_string()
    } else {
        format!("{} AI review findings", findings.len())
    };
    let details = findings
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let location = f
                .path
                .as_ref()
                .map(|p| {
                    format!(
                        "{}{}",
                        p,
                        f.line.map(|l| format!(":{l}")).unwrap_or_default()
                    )
                })
                .unwrap_or_else(|| "General finding".into());
            format!(
                "### {}. {}\n{}{}",
                i + 1,
                location,
                f.body,
                f.diff_hunk
                    .as_ref()
                    .map(|h| format!("\n\nRelated diff:\n\n```diff\n{h}\n```"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(crate::gui_pr_triage::PrFixHandoff {
        target: request.target.clone(),
        draft_prompt: format!(
            "Please address the following {label} from PR #{}. Review the relevant code and make the smallest correct changes. Do not post anything to GitHub; leave a concise summary of changes and any remaining concerns.\n\n{details}",
            pr.number
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_the_same_feature_reuses_its_workspace() {
        let (_dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let first = begin(&mut gui, target.clone()).unwrap();
        let second = begin(&mut gui, target).unwrap();
        assert_eq!(first, second);
        close(&mut gui, &first).unwrap();
        assert!(close(&mut gui, &first).is_err());
    }

    #[test]
    fn beginning_another_feature_retires_a_safe_stale_workspace() {
        let (_dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let first = begin(&mut gui, target.clone()).unwrap();
        let other = FeatureTarget {
            project_id: target.project_id,
            feature_id: "deleted-feature".into(),
        };

        assert!(begin(&mut gui, other).is_err());
        assert!(gui.pr_workspace_context.is_none());
        assert!(close(&mut gui, &first).is_err());
    }

    #[test]
    fn file_requests_require_an_opened_thread_pr() {
        let (_dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let id = begin(&mut gui, target).unwrap();
        let error = files_request(&gui, &id).unwrap_err();
        assert!(error.message.contains("Select a PR in Threads first"));
    }

    fn test_pr() -> PrRef {
        PrRef {
            number: 42,
            head_sha: "head-sha".into(),
            url: "https://example.test/pull/42".into(),
            owner: "fixture".into(),
            repo: "repo".into(),
            head_ref: "feature".into(),
        }
    }

    #[test]
    fn ai_snapshot_on_threads_without_an_ai_pane_preserves_the_live_mode() {
        let (_dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let id = begin(&mut gui, target).unwrap();
        gui.pr_workspace_context.as_mut().unwrap().pr = Some(test_pr());

        ai_snapshot(&mut gui, &id).unwrap();

        assert!(matches!(&gui.app_for_workflow().mode, AppMode::Normal));
        let context = gui.pr_workspace_context.as_ref().unwrap();
        assert!(!context.ai_active);
        assert!(context.parked_ai.is_none());
    }

    #[test]
    fn ai_snapshot_temporarily_borrows_parked_ai_without_losing_threads() {
        let (dir, mut gui, target) = crate::gui_diff::tests::fixture();
        // Opening AI Review resolves its memory docs even before a run starts.
        let mut worktree = crate::traits::MockWorktreeOps::new();
        worktree
            .expect_repo_root()
            .returning(crate::worktree::WorktreeManager::repo_root);
        let app = gui.app_for_workflow();
        app.worktree = std::sync::Arc::new(worktree);
        app.config.global_review_memory_path =
            Some(dir.path().join("absent-memory.md").display().to_string());
        let id = begin(&mut gui, target).unwrap();
        let pr = test_pr();
        let workdir = gui.pr_workspace_context.as_ref().unwrap().workdir.clone();
        gui.app_for_workflow()
            .open_ai_review_for_pr(workdir, pr.clone());
        let ai = Box::new(std::mem::replace(
            &mut gui.app_for_workflow().mode,
            AppMode::Normal,
        ));
        let context = gui.pr_workspace_context.as_mut().unwrap();
        context.pr = Some(pr);
        context.parked_ai = Some(ai);

        ai_snapshot(&mut gui, &id).unwrap();

        assert!(matches!(&gui.app_for_workflow().mode, AppMode::Normal));
        let context = gui.pr_workspace_context.as_ref().unwrap();
        assert!(!context.ai_active);
        assert!(matches!(
            context.parked_ai.as_deref(),
            Some(AppMode::AiReview(_))
        ));
        assert!(context.parked_threads.is_none());
    }

    #[test]
    fn reselecting_the_cached_pr_works_while_another_workspace_pane_is_foregrounded() {
        let (_dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let id = begin(&mut gui, target).unwrap();
        let selected = crate::github::PrRef {
            number: 42,
            head_sha: "head-sha".into(),
            url: "https://example.test/pull/42".into(),
            owner: "fixture".into(),
            repo: "repo".into(),
            head_ref: "feature".into(),
        };
        gui.pr_workspace_context.as_mut().unwrap().pr = Some(selected);
        // The normal mode is standing in for the AI pane or its completion
        // transition; the selected PR must remain usable in this state.
        assert!(select_pr(&mut gui, &id, 42).is_ok());
    }

    fn fix_fixture() -> (tempfile::TempDir, GuiHandle, FixHandoffRequest) {
        let (dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let mut tmux = crate::traits::MockTmuxOps::new();
        tmux.expect_window_exists().returning(|_, _| true);
        let mut worktree = crate::traits::MockWorktreeOps::new();
        worktree
            .expect_repo_root()
            .returning(crate::worktree::WorktreeManager::repo_root);
        let app = gui.app_for_workflow();
        app.tmux = Box::new(tmux);
        app.worktree = std::sync::Arc::new(worktree);
        app.config.global_review_memory_path =
            Some(dir.path().join("absent-memory.md").display().to_string());
        let feature = &mut app.store.projects[0].features[0];
        feature.status = crate::project::ProjectStatus::Idle;
        feature.add_session(crate::project::SessionKind::Claude);
        let session = feature.sessions.first().unwrap();
        let target_session = crate::gui_contract::SessionTarget {
            project_id: target.project_id.clone(),
            feature_id: target.feature_id.clone(),
            session_id: session.id.clone(),
        };
        let destination_revision = super::destination_revision(feature, session);
        let id = begin(&mut gui, target).unwrap();
        let pr = test_pr();
        let workdir = gui.pr_workspace_context.as_ref().unwrap().workdir.clone();
        gui.app_for_workflow()
            .open_ai_review_for_pr(workdir, pr.clone());
        let AppMode::AiReview(state) = &mut gui.app_for_workflow().mode else {
            panic!("AI mode")
        };
        state.findings = vec![
            AiReviewFinding {
                path: Some("code.txt".into()),
                line: Some(8),
                side: None,
                body: "Handle empty input".into(),
                diff_hunk: Some("@@ fixture @@".into()),
                skipped: false,
                published: false,
            },
            AiReviewFinding {
                path: None,
                line: None,
                side: None,
                body: "Check another case".into(),
                diff_hunk: None,
                skipped: false,
                published: false,
            },
        ];
        let context = gui.pr_workspace_context.as_mut().unwrap();
        context.pr = Some(pr);
        context.ai_active = true;
        let result_revision = ai_snapshot(&mut gui, &id).unwrap().result_revision.unwrap();
        (
            dir,
            gui,
            FixHandoffRequest {
                workflow_id: id,
                result_revision,
                finding_indices: vec![0, 1],
                target: target_session,
                destination_revision,
            },
        )
    }

    fn skip_action(request: &FixHandoffRequest, index: usize, head: &str) -> AiAction {
        AiAction::ToggleSkip {
            index,
            result_revision: request.result_revision.clone(),
            head_sha: head.into(),
        }
    }

    #[test]
    fn skipping_a_finding_is_reversible_and_invalidates_the_old_revision() {
        let (_dir, mut gui, request) = fix_fixture();
        let id = request.workflow_id.clone();
        let view = ai_act(&mut gui, &id, skip_action(&request, 0, "head-sha")).unwrap();
        assert!(view.findings[0].skipped && !view.findings[1].skipped);
        assert_ne!(
            view.result_revision.as_deref(),
            Some(&*request.result_revision)
        );
        // The old revision can no longer drive another change.
        let err = ai_act(&mut gui, &id, skip_action(&request, 0, "head-sha")).unwrap_err();
        assert!(err.message.contains("AI result changed"), "{}", err.message);
        // A fix handoff prepared from the old revision is refused too, and a
        // fresh one cannot select the skipped finding.
        assert!(prepare_fix_read(&mut gui, request.clone()).is_err());
        // Restore with the fresh revision.
        let again = AiAction::ToggleSkip {
            index: 0,
            result_revision: view.result_revision.clone().unwrap(),
            head_sha: "head-sha".into(),
        };
        let view = ai_act(&mut gui, &id, again).unwrap();
        assert!(!view.findings[0].skipped);
    }

    #[test]
    fn finding_changes_refuse_wrong_head_index_published_and_missing_ai_result() {
        let (_dir, mut gui, request) = fix_fixture();
        let id = request.workflow_id.clone();
        let err = ai_act(&mut gui, &id, skip_action(&request, 0, "other-head")).unwrap_err();
        assert!(err.message.contains("different PR head"));
        let err = ai_act(&mut gui, &id, skip_action(&request, 9, "head-sha")).unwrap_err();
        assert!(err.message.contains("no longer in the result"));
        if let AppMode::AiReview(state) = &mut gui.app_for_workflow().mode {
            state.findings[1].published = true;
        }
        let revision = ai_snapshot(&mut gui, &id).unwrap().result_revision.unwrap();
        let err = ai_act(
            &mut gui,
            &id,
            AiAction::EditBody {
                index: 1,
                body: "new".into(),
                result_revision: revision,
                head_sha: "head-sha".into(),
            },
        )
        .unwrap_err();
        assert!(err.message.contains("already posted"));
    }

    #[test]
    fn editing_a_finding_is_bounded_bound_to_the_result_and_changes_the_revision() {
        let (_dir, mut gui, request) = fix_fixture();
        let id = request.workflow_id.clone();
        let edit = |body: String, revision: &str| AiAction::EditBody {
            index: 0,
            body,
            result_revision: revision.into(),
            head_sha: "head-sha".into(),
        };
        let blank = ai_act(&mut gui, &id, edit("  \n".into(), &request.result_revision));
        assert!(blank.unwrap_err().message.contains("needs a body"));
        let long = "x".repeat(MAX_FINDING_BODY_CHARS + 1);
        let err = ai_act(&mut gui, &id, edit(long, &request.result_revision)).unwrap_err();
        assert!(err.message.contains("at most"));
        let err = ai_act(&mut gui, &id, edit("fine".into(), "stale")).unwrap_err();
        assert!(err.message.contains("AI result changed"));
        // Nothing above changed the finding.
        let unchanged = ai_snapshot(&mut gui, &id).unwrap();
        assert_eq!(unchanged.findings[0].body, "Handle empty input");
        assert_eq!(
            unchanged.result_revision.as_deref(),
            Some(&*request.result_revision)
        );

        let view = ai_act(
            &mut gui,
            &id,
            edit("Rewritten\nbody".into(), &request.result_revision),
        )
        .unwrap();
        assert_eq!(view.findings[0].body, "Rewritten\nbody");
        assert_ne!(
            view.result_revision.as_deref(),
            Some(&*request.result_revision)
        );
        // A handoff prepared against the pre-edit revision is refused, and a
        // fresh one carries the edited text.
        assert!(prepare_fix_read(&mut gui, request.clone()).is_err());
        let mut fresh = request.clone();
        fresh.result_revision = view.result_revision.unwrap();
        let read = prepare_fix_read(&mut gui, fresh).unwrap();
        let result = finish_fix_read(&mut gui, read, test_pr()).unwrap();
        assert!(result.draft_prompt.contains("Rewritten"));
    }

    #[test]
    fn finding_changes_need_a_finished_result_not_a_running_review() {
        let (_dir, mut gui, request) = fix_fixture();
        let id = request.workflow_id.clone();
        gui.app_for_workflow().mode = AppMode::Normal;
        let err = ai_act(&mut gui, &id, skip_action(&request, 0, "head-sha")).unwrap_err();
        assert!(err.message.contains("Wait for the AI review"));
    }

    #[test]
    fn fix_handoff_binds_findings_and_exact_remote_head_without_side_effects() {
        let (_dir, mut gui, request) = fix_fixture();
        let read = prepare_fix_read(&mut gui, request.clone()).unwrap();
        let result = finish_fix_read(&mut gui, read.clone(), test_pr()).unwrap();
        assert!(result.draft_prompt.contains("2 AI review findings"));
        assert!(result.draft_prompt.contains("Handle empty input"));
        assert!(result.draft_prompt.contains("Check another case"));
        let mut moved = test_pr();
        moved.head_sha = "new-head".into();
        assert!(finish_fix_read(&mut gui, read, moved).is_err());
        let mut single = request;
        single.finding_indices = vec![1];
        assert!(
            prepare_fix_read(&mut gui, single)
                .unwrap()
                .request
                .finding_indices
                .len()
                == 1
        );
    }

    #[test]
    fn fix_handoff_refuses_changed_results_destinations_and_delayed_reads() {
        let (_dir, mut gui, request) = fix_fixture();
        let read = prepare_fix_read(&mut gui, request.clone()).unwrap();
        let AppMode::AiReview(state) = &mut gui.app_for_workflow().mode else {
            panic!("AI mode")
        };
        state.findings[0].body = "changed".into();
        assert!(finish_fix_read(&mut gui, read, test_pr()).is_err());
        state_restore(&mut gui);
        let mut fresh = request.clone();
        fresh.result_revision = ai_snapshot(&mut gui, &request.workflow_id)
            .unwrap()
            .result_revision
            .unwrap();
        let read = prepare_fix_read(&mut gui, fresh.clone()).unwrap();
        gui.app_for_workflow().store.projects[0].features[0].sessions[0].claude_session_id =
            Some("new conversation".into());
        assert!(finish_fix_read(&mut gui, read, test_pr()).is_err());
        assert!(prepare_fix_read(&mut gui, fresh.clone()).is_err());
        let feature = &mut gui.app_for_workflow().store.projects[0].features[0];
        feature.sessions[0].claude_session_id = None;
        fresh.destination_revision = destination_revision(feature, &feature.sessions[0]);
        let read = prepare_fix_read(&mut gui, fresh.clone()).unwrap();
        gui.pr_workspace_context.as_mut().unwrap().result_epoch = uuid::Uuid::new_v4().to_string();
        assert!(finish_fix_read(&mut gui, read, test_pr()).is_err());
        fresh.result_revision = ai_snapshot(&mut gui, &fresh.workflow_id)
            .unwrap()
            .result_revision
            .unwrap();
        for indices in [vec![], vec![0, 0], vec![99]] {
            let mut bad = fresh.clone();
            bad.finding_indices = indices;
            assert!(prepare_fix_read(&mut gui, bad).is_err());
        }
        let AppMode::AiReview(state) = &mut gui.app_for_workflow().mode else {
            panic!("AI mode")
        };
        state.findings[0].skipped = true;
        fresh.result_revision = ai_snapshot(&mut gui, &fresh.workflow_id)
            .unwrap()
            .result_revision
            .unwrap();
        assert!(prepare_fix_read(&mut gui, fresh).is_err());
    }

    #[test]
    fn fix_handoff_rejects_destination_identity_and_finding_state_changes() {
        for change in 0..7 {
            let (_dir, mut gui, request) = fix_fixture();
            let read = prepare_fix_read(&mut gui, request.clone()).unwrap();
            let feature = &mut gui.app_for_workflow().store.projects[0].features[0];
            match change {
                0 => feature.workdir.push("other"),
                1 => feature.tmux_session.push_str("-other"),
                2 => feature.sessions[0].tmux_window.push_str("-other"),
                3 => feature.sessions[0].kind = crate::project::SessionKind::Codex,
                4 => feature.sessions[0].stopped = true,
                5 => feature.sessions.clear(),
                6 => feature.status = crate::project::ProjectStatus::Stopped,
                _ => unreachable!(),
            }
            assert!(
                finish_fix_read(&mut gui, read, test_pr()).is_err(),
                "change {change}"
            );
            assert!(
                prepare_fix_read(&mut gui, request.clone()).is_err(),
                "change {change}"
            );
        }
        for published in [false, true] {
            let (_dir, mut gui, mut request) = fix_fixture();
            let AppMode::AiReview(state) = &mut gui.app_for_workflow().mode else {
                panic!("AI mode")
            };
            if published {
                state.findings[0].published = true;
            } else {
                state.findings[0].skipped = true;
            }
            request.result_revision = ai_snapshot(&mut gui, &request.workflow_id)
                .unwrap()
                .result_revision
                .unwrap();
            assert!(prepare_fix_read(&mut gui, request).is_err());
        }
    }

    #[test]
    fn fix_handoff_rejects_replaced_checkout_at_same_path() {
        let (_dir, mut gui, request) = fix_fixture();
        let read = prepare_fix_read(&mut gui, request).unwrap();
        let checkout = gui.pr_workspace_context.as_ref().unwrap().workdir.clone();
        std::fs::rename(&checkout, checkout.with_extension("old")).unwrap();
        std::fs::create_dir(&checkout).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&checkout)
                .status()
                .unwrap()
                .success()
        );
        assert!(finish_fix_read(&mut gui, read.clone(), test_pr()).is_err());
        assert!(ai_snapshot(&mut gui, &read.request.workflow_id).is_err());
    }

    fn state_restore(gui: &mut GuiHandle) {
        let AppMode::AiReview(state) = &mut gui.app_for_workflow().mode else {
            panic!("AI mode")
        };
        state.findings[0].body = "Handle empty input".into();
    }

    #[test]
    fn workspace_files_are_parsed_from_the_selected_pr_diff() {
        let patch = "diff --git a/src/lib.rs b/src/lib.rs\nindex 1111111..2222222 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1,2 @@\n-old\n+new\n+added\n";
        let view = files_result("workspace", 42, "head-sha".into(), patch.into()).unwrap();
        assert_eq!(view.workflow_id, "workspace");
        assert_eq!(view.pr_number, 42);
        assert_eq!(view.head_sha, "head-sha");
        assert_eq!(view.files.len(), 1);
        assert_eq!(view.files[0].path, "src/lib.rs");
        assert_eq!(view.files[0].additions, 2);
        assert_eq!(view.files[0].deletions, 1);
        assert_eq!(view.files[0].hunks[0].lines.len(), 3);
    }
}
