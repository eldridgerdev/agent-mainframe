//! Experimental GUI shell that combines PR files, existing PR Triage threads,
//! and AMF's existing AI PR review runner in one feature-scoped visit.
use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
    let id = uuid::Uuid::new_v4().to_string();
    gui.pr_workspace_context = Some(PrWorkspaceContext {
        id: id.clone(),
        target,
        workdir,
        pr: None,
        parked_threads: None,
        parked_ai: None,
        ai_active: false,
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
        app.worktree = Box::new(worktree);
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
