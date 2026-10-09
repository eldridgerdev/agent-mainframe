//! Project-scoped manual PR review, driven by the TUI's list, revision fetch,
//! draft persistence and confirmed GitHub submission engines.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::app::{AppMode, DiffScope, PrReviewListLoad, PrSubmitStatus};
use crate::gui_contract::{GuiError, GuiHandle, GuiResult};
use crate::gui_diff::{DiffFileView, file_view_budgeted};
use crate::gui_syntax::HighlightBudget;

pub(crate) struct PrReviewContext {
    id: String,
    project_id: String,
    workdir: PathBuf,
    revision: u64,
    draft: Option<crate::db::pr_review_drafts::PrReviewDraft>,
}

#[derive(Serialize)]
pub struct PrReviewEntry {
    number: u32,
    title: String,
    author: String,
    head_ref: String,
    is_draft: bool,
    has_draft: bool,
    updated: bool,
}

#[derive(Serialize)]
pub struct PrReviewFile {
    diff: DiffFileView,
    comment: String,
}

#[derive(Serialize)]
pub struct PrReviewSubmission {
    event: &'static str,
    body: String,
    comments: Vec<crate::github::PrReviewComment>,
    file_comments: Vec<crate::github::PrFileComment>,
    posting: bool,
    error: Option<String>,
    head_moved: bool,
}

#[derive(Serialize)]
pub struct PrReviewView {
    pub workflow_id: String,
    pub revision: u64,
    project_name: String,
    stage: &'static str,
    entries: Vec<PrReviewEntry>,
    loading: bool,
    number: Option<u32>,
    title: Option<String>,
    files: Vec<PrReviewFile>,
    summary: String,
    submission: Option<PrReviewSubmission>,
    error: Option<String>,
    notice: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrReviewAction {
    Retry,
    Open { number: u32 },
    FileComment { path: String, text: String },
    Summary { text: String },
    Preview { event: String },
    CancelSubmit,
    ConfirmSubmit,
    Reopen,
    Back,
    Close,
}

pub fn begin(gui: &mut GuiHandle, project_id: String) -> GuiResult<PrReviewView> {
    gui.refresh_store()?;
    if let Some(context) = &gui.pr_review_context {
        if context.project_id == project_id {
            return poll(gui, &context.id.clone());
        }
        return Err(GuiError::conflict(
            "Close PR Review before opening another project",
        ));
    }
    let app = gui.app_for_workflow();
    if !matches!(app.mode, AppMode::Normal) || app.paused_plan_interview.is_some() {
        return Err(GuiError::conflict(
            "Finish the current workflow before opening PR Review",
        ));
    }
    let project = app
        .store
        .projects
        .iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| GuiError::not_found("The project was deleted"))?;
    if !project.is_git {
        return Err(GuiError::conflict("PR Review requires a Git repository"));
    }
    let workdir = project.repo.clone();
    app.open_pr_review_list(workdir.clone(), None);
    gui.pr_review_context = Some(PrReviewContext {
        id: uuid::Uuid::new_v4().to_string(),
        project_id,
        workdir,
        revision: 0,
        draft: None,
    });
    view(gui)
}

fn check(gui: &mut GuiHandle, id: &str) -> GuiResult<()> {
    gui.refresh_store()?;
    let context = gui
        .pr_review_context
        .as_ref()
        .filter(|c| c.id == id)
        .ok_or_else(|| GuiError::conflict("PR Review is no longer open"))?;
    let (project_id, workdir) = (context.project_id.clone(), context.workdir.clone());
    // A submitted request may already be on GitHub. Keep polling its receipt
    // even if the project is removed while the worker is posting.
    let posting = matches!(&gui.app_for_workflow().mode, AppMode::DiffViewer(state)
        if state.pr_submit.as_ref().is_some_and(|submit| matches!(submit.status, PrSubmitStatus::Posting { .. })));
    if !posting
        && !gui
            .app_for_workflow()
            .store
            .projects
            .iter()
            .any(|p| p.id == project_id && p.repo == workdir && p.is_git)
    {
        close(gui);
        return Err(GuiError::conflict(
            "The project changed or was deleted; PR Review closed",
        ));
    }
    Ok(())
}

pub fn poll(gui: &mut GuiHandle, id: &str) -> GuiResult<PrReviewView> {
    check(gui, id)?;
    let app = gui.app_for_workflow();
    let mut changed = app.poll_pr_review_list_bg();
    changed |= app.poll_pr_review_open_bg();
    if matches!(app.mode, AppMode::DiffViewerLoading(_)) {
        let draft = current_draft(app)?;
        app.complete_diff_viewer_loading();
        gui.pr_review_context.as_mut().unwrap().draft = draft;
        changed = true;
    }
    changed |= gui.app_for_workflow().poll_pr_review_post_bg();
    if changed {
        gui.pr_review_context.as_mut().unwrap().revision += 1;
    }
    view(gui)
}

fn current_draft(
    app: &crate::app::App,
) -> GuiResult<Option<crate::db::pr_review_drafts::PrReviewDraft>> {
    let state = match &app.mode {
        AppMode::DiffViewer(state) | AppMode::DiffViewerLoading(state) => state,
        _ => return Ok(None),
    };
    let DiffScope::PullRequest(target) = &state.scope else {
        return Ok(None);
    };
    app.db
        .as_ref()
        .ok_or_else(|| GuiError::conflict("No database for PR review drafts"))?
        .load_pr_review_draft(&target.repo, target.pr.number)
        .map_err(GuiError::from)
}

fn save(gui: &mut GuiHandle) -> GuiResult<()> {
    // Use the fallible draft result: closing must never silently lose comments.
    gui.app_for_workflow().recapture_anchor_contexts();
    let result = match gui.app_for_workflow().persist_pr_review_draft() {
        crate::app::review::DraftSave::Saved | crate::app::review::DraftSave::Empty => Ok(()),
        crate::app::review::DraftSave::Failed(error) => Err(GuiError::from(anyhow::anyhow!(error))),
        _ => Err(GuiError::conflict("The PR review draft could not be saved")),
    };
    result?;
    let draft = current_draft(gui.app_for_workflow())?;
    gui.pr_review_context.as_mut().unwrap().draft = draft;
    Ok(())
}

fn back(gui: &mut GuiHandle) {
    let app = gui.app_for_workflow();
    if !matches!(&app.mode, AppMode::DiffViewer(state) if state.is_pr_review()) {
        return;
    }
    if let AppMode::DiffViewer(state) = std::mem::replace(&mut app.mode, AppMode::Normal) {
        app.exit_pr_review(state);
    }
}

fn close(gui: &mut GuiHandle) {
    back_if_review(gui);
    gui.app_for_workflow().close_pr_review_list();
    // A second close abandons the list when the first cancelled an open.
    gui.app_for_workflow().close_pr_review_list();
    gui.pr_review_context = None;
}

fn back_if_review(gui: &mut GuiHandle) {
    if matches!(gui.app_for_workflow().mode, AppMode::DiffViewer(_)) {
        back(gui);
    }
}

pub fn act(
    gui: &mut GuiHandle,
    id: &str,
    revision: u64,
    action: PrReviewAction,
) -> GuiResult<Option<PrReviewView>> {
    if let Err(error) = check(gui, id) {
        // A target invalidation closes the shared workflow. Let the panel
        // dismiss too, including when it was showing a submission preview.
        if gui.pr_review_context.is_none() {
            return Ok(None);
        }
        return Err(error);
    }
    if gui.pr_review_context.as_ref().unwrap().revision != revision {
        return Err(GuiError::conflict("PR Review changed; refresh and retry"));
    }
    let draft = current_draft(gui.app_for_workflow())?;
    let draft_changed = matches!(gui.app_for_workflow().mode, AppMode::DiffViewer(_))
        && draft != gui.pr_review_context.as_ref().unwrap().draft;
    if draft_changed
        && !matches!(
            action,
            PrReviewAction::Back | PrReviewAction::Close | PrReviewAction::CancelSubmit
        )
    {
        return Err(GuiError::conflict(
            "The saved PR review changed in another interface; return to the PR list and reopen it before editing or posting",
        ));
    }
    let app = gui.app_for_workflow();
    let submitting = matches!(&app.mode, AppMode::DiffViewer(s) if s.pr_submit.is_some());
    let posting = matches!(&app.mode, AppMode::DiffViewer(s) if s.pr_submit.as_ref().is_some_and(|s| matches!(s.status, PrSubmitStatus::Posting { .. })));
    if posting {
        return Err(GuiError::conflict(
            "Wait for GitHub to answer before continuing",
        ));
    }
    if submitting
        && !matches!(
            action,
            PrReviewAction::CancelSubmit | PrReviewAction::ConfirmSubmit | PrReviewAction::Reopen
        )
    {
        return Err(GuiError::conflict(
            "Close the submission preview before editing or leaving",
        ));
    }
    match action {
        PrReviewAction::Retry => {
            if !matches!(app.mode, AppMode::PrReviewList(_)) {
                return Err(GuiError::conflict("Return to the PR list first"));
            }
            app.pr_review_list_retry();
        }
        PrReviewAction::Open { number } => {
            let AppMode::PrReviewList(state) = &mut app.mode else {
                return Err(GuiError::conflict("Return to the PR list first"));
            };
            let PrReviewListLoad::Loaded(entries) = &state.load else {
                return Err(GuiError::conflict("Wait for the PR list"));
            };
            state.selected = entries
                .iter()
                .position(|p| p.number == number)
                .ok_or_else(|| GuiError::not_found("PR is no longer in the list"))?;
            app.pr_review_list_open_selected();
        }
        PrReviewAction::FileComment { path, text } => {
            let AppMode::DiffViewer(state) = &mut app.mode else {
                return Err(GuiError::conflict("Open a PR first"));
            };
            state.selected_file = state
                .files
                .iter()
                .position(|f| f.path == path)
                .ok_or_else(|| GuiError::not_found("File is no longer in this PR"))?;
            app.defer_review_progress_persist = true;
            app.diff_review_start_file_comment();
            if let AppMode::DiffViewer(state) = &mut app.mode {
                state.reset_feedback_editor(text);
            }
            app.diff_review_submit_file_comment();
            app.defer_review_progress_persist = false;
            save(gui)?;
        }
        PrReviewAction::Summary { text } => {
            let AppMode::DiffViewer(state) = &mut app.mode else {
                return Err(GuiError::conflict("Open a PR first"));
            };
            state.general_feedback = text;
            save(gui)?;
        }
        PrReviewAction::Preview { event } => {
            let event = match event.as_str() {
                "COMMENT" => crate::app::PrReviewEvent::Comment,
                "APPROVE" => crate::app::PrReviewEvent::Approve,
                "REQUEST_CHANGES" => crate::app::PrReviewEvent::RequestChanges,
                _ => return Err(GuiError::conflict("Unknown review event")),
            };
            save(gui)?;
            let app = gui.app_for_workflow();
            app.open_pr_submit();
            if let AppMode::DiffViewer(state) = &mut app.mode
                && let Some(submit) = &mut state.pr_submit
            {
                submit.event = event;
            }
        }
        PrReviewAction::CancelSubmit => app.pr_submit_close(),
        PrReviewAction::ConfirmSubmit => {
            if !submitting {
                return Err(GuiError::conflict("Preview the review before posting"));
            }
            app.pr_submit_post();
            let draft = current_draft(gui.app_for_workflow())?;
            gui.pr_review_context.as_mut().unwrap().draft = draft;
        }
        PrReviewAction::Reopen => app.pr_submit_reopen_at_new_head(),
        PrReviewAction::Back => {
            if !draft_changed {
                save(gui)?;
            }
            back(gui);
        }
        PrReviewAction::Close => {
            if matches!(app.mode, AppMode::DiffViewer(_)) && !draft_changed {
                save(gui)?;
            }
            close(gui);
            return Ok(None);
        }
    }
    gui.pr_review_context.as_mut().unwrap().revision += 1;
    view(gui).map(Some)
}

fn view(gui: &mut GuiHandle) -> GuiResult<PrReviewView> {
    let context = gui.pr_review_context.as_ref().unwrap();
    let (id, revision, project_id) = (
        context.id.clone(),
        context.revision,
        context.project_id.clone(),
    );
    let app = gui.app_for_workflow();
    let project_name = app
        .store
        .projects
        .iter()
        .find(|p| p.id == project_id)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let mut view = PrReviewView {
        workflow_id: id,
        revision,
        project_name,
        stage: "pick",
        entries: vec![],
        loading: false,
        number: None,
        title: None,
        files: vec![],
        summary: String::new(),
        submission: None,
        error: None,
        notice: app.message.clone(),
    };
    match &app.mode {
        AppMode::PrReviewList(state) => {
            view.loading = matches!(state.load, PrReviewListLoad::Loading)
                || state.reloading
                || state.opening.is_some();
            view.error = state.open_error.clone();
            match &state.load {
                PrReviewListLoad::Loaded(entries) => {
                    view.entries = entries
                        .iter()
                        .map(|p| PrReviewEntry {
                            number: p.number,
                            title: p.title.clone(),
                            author: p.author.clone(),
                            head_ref: p.head_ref.clone(),
                            is_draft: p.is_draft,
                            has_draft: state.drafts.contains_key(&p.number),
                            updated: state
                                .drafts
                                .get(&p.number)
                                .is_some_and(|d| d.head_oid != p.head_oid),
                        })
                        .collect()
                }
                PrReviewListLoad::Failed(error) => view.error = Some(error.detail.clone()),
                _ => {}
            }
        }
        AppMode::DiffViewer(state) if state.is_pr_review() => {
            view.stage = "review";
            if let DiffScope::PullRequest(target) = &state.scope {
                view.number = Some(target.pr.number);
                view.title = Some(target.pr.title.clone());
            }
            view.summary = state.general_feedback.clone();
            view.error = state.error.clone();
            let mut budget = HighlightBudget::view();
            view.files = state
                .files
                .iter()
                .map(|f| PrReviewFile {
                    diff: file_view_budgeted(f.clone(), 3, &mut budget),
                    comment: state
                        .file_comments
                        .get(&f.path)
                        .map(|c| c.text.clone())
                        .unwrap_or_default(),
                })
                .collect();
            if let Some(submit) = &state.pr_submit {
                let submission = crate::app::review::build_pr_submission(state);
                let (error, head_moved) = match &submit.status {
                    PrSubmitStatus::Failed(e) => (Some(e.clone()), false),
                    PrSubmitStatus::HeadMoved { .. } => (
                        Some("The PR has new commits. Reopen it before posting.".into()),
                        true,
                    ),
                    _ => (None, false),
                };
                view.submission = Some(PrReviewSubmission {
                    event: submit.event.api_name(),
                    body: submission.body,
                    comments: submission.comments,
                    file_comments: submission.file_comments,
                    posting: matches!(submit.status, PrSubmitStatus::Posting { .. }),
                    error,
                    head_moved,
                });
            }
        }
        _ => return Err(GuiError::conflict("PR Review is no longer open")),
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PrDiffTarget;
    use crate::app::pr_review::runtime::{PrPostOutcome, PrPostRequest, ReviewListLoaded};
    use crate::github::ReviewablePr;
    use std::path::Path;
    use std::time::{Duration, Instant};

    const REPO: &str = "github.com/demo/repo";
    fn git(path: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    fn listed(path: &Path, _: bool) -> ReviewListLoaded {
        ReviewListLoaded {
            prs: Ok(vec![ReviewablePr {
                number: 9,
                title: "A different branch".into(),
                author: "alice".into(),
                is_draft: false,
                updated_at: String::new(),
                base_ref: "main".into(),
                base_oid: String::new(),
                head_ref: "feature".into(),
                head_oid: git(path, &["rev-parse", "feature"]),
                is_cross_repository: false,
                head_owner: "demo".into(),
            }]),
            current_user: Some("bob".into()),
            repo_key: Some(REPO.into()),
        }
    }
    fn opener(path: &Path, pr: &ReviewablePr) -> anyhow::Result<PrDiffTarget> {
        let mut pr = pr.clone();
        pr.base_oid = git(path, &["rev-parse", "main"]);
        let merge_base_oid = git(path, &["merge-base", &pr.base_oid, &pr.head_oid]);
        Ok(PrDiffTarget {
            repo: REPO.into(),
            pr,
            merge_base_oid,
        })
    }
    fn poster(request: &PrPostRequest) -> PrPostOutcome {
        assert!(request.body.contains("Summary from GUI"));
        assert_eq!(request.pr.number, 9);
        PrPostOutcome::Posted {
            file_comment_failures: vec![],
        }
    }
    fn moved(_: &PrPostRequest) -> PrPostOutcome {
        PrPostOutcome::HeadMoved {
            current_head: "new-head".into(),
        }
    }
    fn fixture() -> (tempfile::TempDir, GuiHandle, String) {
        let (dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let app = gui.app_for_workflow();
        let repo = app.store.projects[0].repo.clone();
        // Run the workflow with no feature, on a dirty checkout of main.
        git(&repo, &["checkout", "main"]);
        std::fs::write(repo.join("code.txt"), "local changes\n").unwrap();
        app.store.projects[0].features.clear();
        app.db.as_ref().unwrap().save_store(&app.store).unwrap();
        app.pr_review_work.set_review_list_loader_for_test(listed);
        app.pr_review_work.set_review_opener_for_test(opener);
        app.pr_review_work.set_review_poster_for_test(poster);
        (dir, gui, target.project_id)
    }
    fn settle(gui: &mut GuiHandle, mut view: PrReviewView) -> PrReviewView {
        let deadline = Instant::now() + Duration::from_secs(10);
        while view.loading || view.submission.as_ref().is_some_and(|s| s.posting) {
            assert!(Instant::now() < deadline, "PR worker did not finish");
            std::thread::sleep(Duration::from_millis(5));
            view = poll(gui, &view.workflow_id).unwrap();
        }
        view
    }
    fn action(gui: &mut GuiHandle, view: &PrReviewView, action: PrReviewAction) -> PrReviewView {
        super::act(gui, &view.workflow_id, view.revision, action)
            .unwrap()
            .unwrap()
    }
    fn opened(gui: &mut GuiHandle, project_id: String) -> PrReviewView {
        let view = begin(gui, project_id).unwrap();
        assert_eq!(view.stage, "pick");
        let view = settle(gui, view);
        let view = action(gui, &view, PrReviewAction::Open { number: 9 });
        let view = settle(gui, view);
        assert_eq!(view.stage, "review");
        view
    }

    #[test]
    fn project_review_uses_pr_revisions_keeps_checkout_and_resumes_draft() {
        let (_dir, mut gui, project_id) = fixture();
        let view = opened(&mut gui, project_id.clone());
        let file = &view.files[0];
        assert!(file.diff.patch.contains("committed"));
        assert!(!file.diff.patch.contains("local changes"));
        let path = file.diff.path.clone();
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::FileComment {
                path,
                text: "Keep this comment".into(),
            },
        );
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::Summary {
                text: "Summary from GUI".into(),
            },
        );
        let view = action(&mut gui, &view, PrReviewAction::Back);
        assert_eq!(view.stage, "pick");
        let view = action(&mut gui, &view, PrReviewAction::Open { number: 9 });
        let view = settle(&mut gui, view);
        assert_eq!(view.summary, "Summary from GUI");
        assert_eq!(view.files[0].comment, "Keep this comment");
        let repo = gui.app_for_workflow().store.projects[0].repo.clone();
        assert_eq!(git(&repo, &["branch", "--show-current"]), "main");
        assert_eq!(
            std::fs::read_to_string(repo.join("code.txt")).unwrap(),
            "local changes\n"
        );
        assert!(!crate::app::review::review_progress_path(&repo).exists());
        assert!(
            super::act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                PrReviewAction::ConfirmSubmit
            )
            .is_err()
        );
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
        );
        let submission = view.submission.as_ref().unwrap();
        assert!(!submission.posting);
        assert!(submission.body.contains("Summary from GUI"));
        assert!(
            submission.file_comments[0]
                .body
                .contains("Keep this comment")
        );
        let view = action(&mut gui, &view, PrReviewAction::ConfirmSubmit);
        assert!(view.submission.as_ref().unwrap().posting);
        let view = settle(&mut gui, view);
        assert_eq!(view.stage, "pick");
        assert!(view.notice.unwrap().contains("Posted"));
    }

    #[test]
    fn stale_actions_external_drafts_and_moved_heads_cannot_post() {
        let (_dir, mut gui, project_id) = fixture();
        let initial = opened(&mut gui, project_id);
        let view = action(
            &mut gui,
            &initial,
            PrReviewAction::Summary {
                text: "Summary from GUI".into(),
            },
        );
        assert!(
            super::act(
                &mut gui,
                &view.workflow_id,
                initial.revision,
                PrReviewAction::Close
            )
            .is_err()
        );
        gui.app_for_workflow()
            .pr_review_work
            .set_review_poster_for_test(moved);
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
        );
        let view = action(&mut gui, &view, PrReviewAction::ConfirmSubmit);
        let view = settle(&mut gui, view);
        assert!(view.submission.as_ref().unwrap().head_moved);
        let view = action(&mut gui, &view, PrReviewAction::CancelSubmit);
        let db = gui.app_for_workflow().db.as_ref().unwrap();
        let mut draft = db.load_pr_review_draft(REPO, 9).unwrap().unwrap();
        draft.progress = draft
            .progress
            .replace("Summary from GUI", "Saved elsewhere");
        db.upsert_pr_review_draft(&draft).unwrap();
        let error = super::act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
        )
        .err()
        .unwrap();
        assert!(error.message.contains("another interface"));
        let view = action(&mut gui, &view, PrReviewAction::Back);
        let view = action(&mut gui, &view, PrReviewAction::Open { number: 9 });
        let view = settle(&mut gui, view);
        assert_eq!(view.summary, "Saved elsewhere");
    }

    #[test]
    fn a_post_receipt_is_recorded_even_when_its_project_is_deleted() {
        let (_dir, mut gui, project_id) = fixture();
        let view = opened(&mut gui, project_id);
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::Summary {
                text: "Summary from GUI".into(),
            },
        );
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
        );
        let view = action(&mut gui, &view, PrReviewAction::ConfirmSubmit);
        let app = gui.app_for_workflow();
        app.store.projects.clear();
        app.db.as_ref().unwrap().save_store(&app.store).unwrap();
        let view = settle(&mut gui, view);
        let draft = gui
            .app_for_workflow()
            .db
            .as_ref()
            .unwrap()
            .load_pr_review_draft(REPO, 9)
            .unwrap()
            .unwrap();
        assert_eq!(
            draft.status,
            crate::db::pr_review_drafts::PrReviewDraftStatus::Posted
        );
        assert!(
            super::act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                PrReviewAction::Close
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn deleted_projects_close_and_retired_workflow_ids_are_refused() {
        let (_dir, mut gui, project_id) = fixture();
        let initial = begin(&mut gui, project_id).unwrap();
        let id = initial.workflow_id.clone();
        super::act(&mut gui, &id, initial.revision, PrReviewAction::Close).unwrap();
        assert!(poll(&mut gui, &id).is_err());
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
        let project_id = gui.app_for_workflow().store.projects[0].id.clone();
        let view = opened(&mut gui, project_id);
        let app = gui.app_for_workflow();
        app.store.projects.clear();
        app.db.as_ref().unwrap().save_store(&app.store).unwrap();
        assert!(poll(&mut gui, &view.workflow_id).is_err());
        assert!(gui.pr_review_context.is_none());
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
    }
}
