//! Project-scoped manual PR review, driven by the TUI's list, revision fetch,
//! draft persistence and confirmed GitHub submission engines.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::app::{AppMode, DiffScope, PrReviewListLoad, PrSubmitStatus};
use crate::diff::DiffLineLocation;
use crate::gui_contract::{GuiError, GuiHandle, GuiResult};
use crate::gui_diff::{DiffFileView, file_view_budgeted};
use crate::gui_review::Severity;
use crate::gui_syntax::HighlightBudget;

pub(crate) struct PrReviewContext {
    id: String,
    project_id: String,
    workdir: PathBuf,
    revision: u64,
    draft: Option<crate::db::pr_review_drafts::PrReviewDraft>,
    /// Set when PR Workspace's Files tab owns this review: it opens exactly
    /// one PR at exactly one head and only edits comments.
    embedded: Option<EmbeddedPin>,
}

/// The PR and head the workspace's Files tab was reading when it asked to
/// author comments. The review it opens must be that PR at that head.
#[derive(Clone)]
struct EmbeddedPin {
    number: u32,
    head_sha: String,
    /// The open has been issued; do not issue it twice.
    requested: bool,
    /// Why the review cannot be edited from Files. Terminal for this review.
    error: Option<String>,
}

impl PrReviewContext {
    pub(crate) fn workflow_id(&self) -> String {
        self.id.clone()
    }
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
    line_comments: Vec<PrReviewLineComment>,
    /// The lines of the PR's own diff, each with the index of its original
    /// hunk. Expanded context is absent, so the client can refuse a selection
    /// GitHub would reject (outside the diff, or spanning two hunks).
    pr_lines: Vec<PrDiffLine>,
}

#[derive(Serialize)]
pub struct PrDiffLine {
    old_line: Option<usize>,
    new_line: Option<usize>,
    hunk: usize,
}

/// The file as GitHub's own diff has it, free of expanded context.
fn original_file(file: &crate::diff::DiffFile) -> Option<crate::diff::DiffFile> {
    crate::diff::parse_unified_diff(&file.patch)
        .ok()
        .and_then(|files| files.into_iter().next())
}

fn pr_lines(file: &crate::diff::DiffFile) -> Vec<PrDiffLine> {
    let Some(original) = original_file(file) else {
        return Vec::new();
    };
    original
        .hunks
        .iter()
        .enumerate()
        .flat_map(|(hunk, h)| {
            crate::diff::addressable_lines_in(std::slice::from_ref(h))
                .into_iter()
                .map(move |l| PrDiffLine {
                    old_line: l.old_line,
                    new_line: l.new_line,
                    hunk,
                })
        })
        .collect()
}

/// A saved line thread, in the shared draft's own terms.
#[derive(Serialize)]
pub struct PrReviewLineComment {
    start: DiffLineLocation,
    end: DiffLineLocation,
    /// GitHub side of the end line (`RIGHT` for any line still in the file).
    side: &'static str,
    start_side: Option<&'static str>,
    editable: bool,
    anchor: String,
    text: String,
    severity: Severity,
    resolved: bool,
    anchor_lost: bool,
    suggestion: Option<String>,
    /// Whether an open thread posts inline; `note` says where it goes if not.
    inline: bool,
    note: Option<String>,
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
    /// The PR head every comment in this review is pinned to.
    head_sha: Option<String>,
    stage: &'static str,
    /// Opened from PR Workspace's Files tab: line, range, suggestion, thread
    /// and file comments only; summary, preview and posting stay in the
    /// PR Review dialog.
    embedded: bool,
    /// Why this Files review is refused (PR not open for review, or its head
    /// moved since the workspace loaded it).
    pin_error: Option<String>,
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
    Open {
        number: u32,
    },
    FileComment {
        path: String,
        text: String,
    },
    LineComment {
        path: String,
        start: DiffLineLocation,
        end: DiffLineLocation,
        text: String,
        severity: Severity,
    },
    Suggestion {
        path: String,
        start: DiffLineLocation,
        end: DiffLineLocation,
        text: String,
    },
    ToggleLineResolved {
        path: String,
        start: DiffLineLocation,
        end: DiffLineLocation,
    },
    Summary {
        text: String,
    },
    Preview {
        event: String,
    },
    CancelSubmit,
    ConfirmSubmit,
    Reopen,
    Back,
    Close,
}

pub fn begin(gui: &mut GuiHandle, project_id: String) -> GuiResult<PrReviewView> {
    gui.refresh_store()?;
    if let Some(context) = &gui.pr_review_context {
        if context.embedded.is_some() {
            return Err(GuiError::conflict(
                "PR Workspace's Files tab is editing a PR review; leave that tab before opening PR Review",
            ));
        }
        if context.project_id == project_id {
            return poll(gui, &context.id.clone());
        }
        return Err(GuiError::conflict(
            "Close PR Review before opening another project",
        ));
    }
    start(gui, project_id, None)
}

/// Open the review for PR Workspace's Files tab: the one PR the workspace is
/// reading, at the head it read. The caller parks any other workflow first.
pub(crate) fn begin_embedded(
    gui: &mut GuiHandle,
    project_id: String,
    number: u32,
    head_sha: String,
) -> GuiResult<PrReviewView> {
    gui.refresh_store()?;
    if gui.pr_review_context.is_some() {
        return Err(GuiError::conflict(
            "Close PR Review before editing comments in PR Workspace",
        ));
    }
    start(
        gui,
        project_id,
        Some(EmbeddedPin {
            number,
            head_sha,
            requested: false,
            error: None,
        }),
    )
}

fn start(
    gui: &mut GuiHandle,
    project_id: String,
    embedded: Option<EmbeddedPin>,
) -> GuiResult<PrReviewView> {
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
        embedded,
    });
    let id = gui.pr_review_context.as_ref().unwrap().id.clone();
    poll(gui, &id)
}

/// Close a Files-tab review. The comments are already saved by each action;
/// this saves once more (unless another interface changed the draft, which
/// it must not overwrite) and refuses while a post is in flight.
pub(crate) fn close_embedded(gui: &mut GuiHandle) -> GuiResult<()> {
    if !gui
        .pr_review_context
        .as_ref()
        .is_some_and(|c| c.embedded.is_some())
    {
        return Ok(());
    }
    let app = gui.app_for_workflow();
    if matches!(&app.mode, AppMode::DiffViewer(s)
        if s.pr_submit.as_ref().is_some_and(|s| matches!(s.status, PrSubmitStatus::Posting { .. })))
    {
        return Err(GuiError::conflict(
            "Wait for GitHub to answer before leaving",
        ));
    }
    if matches!(app.mode, AppMode::DiffViewer(_)) {
        let draft_changed =
            current_draft(gui.app_for_workflow())? != gui.pr_review_context.as_ref().unwrap().draft;
        if !draft_changed {
            save(gui)?;
        }
    }
    close(gui);
    Ok(())
}

/// Issue the pinned open once the PR list has loaded, and refuse a review
/// that is not the PR and head the Files tab was reading.
fn advance_pin(gui: &mut GuiHandle) -> bool {
    let Some(pin) = gui
        .pr_review_context
        .as_ref()
        .and_then(|c| c.embedded.clone())
    else {
        return false;
    };
    if pin.error.is_some() {
        return false;
    }
    let mut next = pin.clone();
    let mut open = false;
    let app = gui.app_for_workflow();
    match &mut app.mode {
        AppMode::PrReviewList(state) if state.opening.is_none() && !state.reloading => {
            if !pin.requested {
                if let PrReviewListLoad::Loaded(entries) = &state.load {
                    match entries.iter().position(|p| p.number == pin.number) {
                        Some(index) => {
                            state.selected = index;
                            next.requested = true;
                            open = true;
                        }
                        None => {
                            next.error = Some(format!(
                                "PR #{} is not in the open pull requests PR Review lists (closed, or not reviewable); the read-only diff is shown instead",
                                pin.number
                            ))
                        }
                    }
                }
            } else if let Some(error) = state.open_error.clone() {
                next.error = Some(format!("PR #{} could not be opened: {error}", pin.number));
            }
        }
        AppMode::DiffViewer(state) => {
            if let DiffScope::PullRequest(target) = &state.scope
                && (target.pr.number != pin.number
                    || !target.pr.head_oid.eq_ignore_ascii_case(&pin.head_sha))
            {
                next.error = Some(format!(
                    "PR #{} is now at {}, but PR Workspace loaded {}. Comments are pinned to one head, so editing is refused; reselect the PR in Threads to refresh, then reopen Files",
                    target.pr.number,
                    target.pr.head_oid.chars().take(8).collect::<String>(),
                    pin.head_sha.chars().take(8).collect::<String>(),
                ));
            }
        }
        _ => {}
    }
    if open {
        app.pr_review_list_open_selected();
    }
    let changed = next.requested != pin.requested || next.error != pin.error;
    if changed {
        gui.pr_review_context.as_mut().unwrap().embedded = Some(next);
    }
    changed
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
    changed |= advance_pin(gui);
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

/// The thread state a line action may change, so a failed draft save can put
/// it back: the view must never show a thread that is not in storage.
struct ThreadState {
    comments: std::collections::HashMap<String, Vec<crate::app::LineComment>>,
    decisions: std::collections::HashMap<String, crate::app::ReviewDecision>,
    auto_rejected: std::collections::HashSet<String>,
    // The shared viewer state a line action also moves; a TUI attached to the
    // same state must not be left on the rolled-back action's file or editor.
    selected_file: usize,
    comment_anchor: Option<usize>,
    comment_cursor: Option<usize>,
    comment_severity: crate::app::Severity,
    feedback_editor: crate::editor::TextEditor,
    editing_line_comment: bool,
    editing_suggestion: bool,
}

fn thread_state(app: &mut crate::app::App) -> Option<ThreadState> {
    match &app.mode {
        AppMode::DiffViewer(s) => Some(ThreadState {
            comments: s.line_comments.clone(),
            decisions: s.decisions.clone(),
            auto_rejected: s.auto_rejected.clone(),
            selected_file: s.selected_file,
            comment_anchor: s.comment_anchor,
            comment_cursor: s.comment_cursor,
            comment_severity: s.comment_severity,
            feedback_editor: s.feedback_editor.clone(),
            editing_line_comment: s.editing_line_comment,
            editing_suggestion: s.editing_suggestion,
        }),
        _ => None,
    }
}

/// Save the draft after a line action; on failure restore `before` and report.
fn save_threads(gui: &mut GuiHandle, before: Option<ThreadState>) -> GuiResult<()> {
    let result = save(gui);
    if result.is_err()
        && let (Some(before), AppMode::DiffViewer(s)) = (before, &mut gui.app_for_workflow().mode)
    {
        s.line_comments = before.comments;
        s.decisions = before.decisions;
        s.auto_rejected = before.auto_rejected;
        s.selected_file = before.selected_file;
        s.comment_anchor = before.comment_anchor;
        s.comment_cursor = before.comment_cursor;
        s.comment_severity = before.comment_severity;
        s.feedback_editor = before.feedback_editor;
        s.editing_line_comment = before.editing_line_comment;
        s.editing_suggestion = before.editing_suggestion;
    }
    result
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
    advance_pin(gui);
    if let Some(pin) = &gui.pr_review_context.as_ref().unwrap().embedded {
        if !matches!(
            action,
            PrReviewAction::FileComment { .. }
                | PrReviewAction::LineComment { .. }
                | PrReviewAction::Suggestion { .. }
                | PrReviewAction::ToggleLineResolved { .. }
        ) {
            return Err(GuiError::conflict(
                "The Files tab edits line and file comments only; use PR Review to edit the summary, preview and post",
            ));
        }
        if let Some(error) = &pin.error {
            return Err(GuiError::conflict(error.clone()));
        }
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
        PrReviewAction::LineComment {
            path,
            start,
            end,
            text,
            severity,
        } => {
            let existing = prepare_span(app, &path, start, end, None)?;
            if text.trim().is_empty() && !existing {
                return Err(GuiError::conflict("Write a comment before saving"));
            }
            let before = thread_state(app);
            app.defer_review_progress_persist = true;
            app.diff_review_start_line_comment();
            if let AppMode::DiffViewer(state) = &mut app.mode {
                state.reset_feedback_editor(text);
                state.comment_severity = severity;
            }
            app.diff_review_submit_line_comment();
            app.defer_review_progress_persist = false;
            save_threads(gui, before)?;
        }
        PrReviewAction::Suggestion {
            path,
            start,
            end,
            text,
        } => {
            let existing = prepare_span(app, &path, start, end, Some(&text))?;
            if text.trim().is_empty() && !existing {
                return Err(GuiError::conflict("Write a replacement before saving"));
            }
            let before = thread_state(app);
            app.defer_review_progress_persist = true;
            app.diff_review_start_suggestion();
            if let AppMode::DiffViewer(state) = &mut app.mode {
                state.reset_feedback_editor(text);
            }
            app.diff_review_submit_suggestion();
            app.defer_review_progress_persist = false;
            save_threads(gui, before)?;
        }
        PrReviewAction::ToggleLineResolved { path, start, end } => {
            let AppMode::DiffViewer(state) = &mut app.mode else {
                return Err(GuiError::conflict("Open a PR first"));
            };
            state.selected_file = state
                .files
                .iter()
                .position(|f| f.path == path)
                .ok_or_else(|| GuiError::not_found("File is no longer in this PR"))?;
            let before = thread_state(app);
            app.defer_review_progress_persist = true;
            let toggled = app.diff_review_toggle_thread_resolved(start, end);
            app.defer_review_progress_persist = false;
            if !toggled {
                return Err(GuiError::conflict(
                    "That thread is no longer at this anchor, is lost, or is an unaccepted draft",
                ));
            }
            save_threads(gui, before)?;
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

fn line_comment_views(
    state: &crate::app::DiffViewerState,
    file: &crate::diff::DiffFile,
    inline: Option<&Vec<bool>>,
) -> Vec<PrReviewLineComment> {
    // Editable only where `prepare_span` will accept it: the PR's own lines.
    let locations = original_file(file)
        .map(|f| f.addressable_lines())
        .unwrap_or_default();
    let side = |l: DiffLineLocation| {
        if l.new_line.is_some() {
            "RIGHT"
        } else {
            "LEFT"
        }
    };
    let label = |l: DiffLineLocation| match (l.old_line, l.new_line) {
        (_, Some(line)) => format!("line {line}"),
        (Some(line), None) => format!("base line {line}"),
        _ => "lost anchor".to_string(),
    };
    state
        .line_comments
        .get(&file.path)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, c)| {
            let first = c.start.unwrap_or(c.location);
            let posts_inline = inline.and_then(|f| f.get(i)).copied().unwrap_or(false);
            let note = if c.resolved {
                Some("Resolved threads are kept locally and not posted".to_string())
            } else if !posts_inline {
                Some(
                    if c.anchor_lost {
                        "Its code is no longer in the PR; it will be added to the review summary"
                    } else {
                        "It is outside the PR's diff; it will be added to the review summary"
                    }
                    .to_string(),
                )
            } else {
                None
            };
            PrReviewLineComment {
                start: first,
                end: c.location,
                side: side(c.location),
                start_side: c.start.map(side),
                editable: !c.anchor_lost
                    && locations.contains(&c.location)
                    && c.start.is_none_or(|s| locations.contains(&s)),
                anchor: match c.start {
                    Some(start) => format!("{} – {}", label(start), label(c.location)),
                    None => label(c.location),
                },
                text: c.text.clone(),
                severity: c.severity,
                resolved: c.resolved,
                anchor_lost: c.anchor_lost,
                suggestion: c.suggestion.clone(),
                inline: posts_inline && !c.resolved,
                note,
            }
        })
        .collect()
}

/// Validate a line span against the PR's own diff and aim the shared line
/// editor at it. Nothing is changed when this refuses. Returns whether a saved
/// thread already sits at exactly this span (so an empty save may remove it).
///
/// GitHub 422s a range that crosses hunks, and a suggestion replaces lines of
/// the current file, so a suggestion must be one run of current-side lines.
fn prepare_span(
    app: &mut crate::app::App,
    path: &str,
    start: DiffLineLocation,
    end: DiffLineLocation,
    suggestion: Option<&str>,
) -> GuiResult<bool> {
    let AppMode::DiffViewer(state) = &mut app.mode else {
        return Err(GuiError::conflict("Open a PR first"));
    };
    if !state.is_pr_review() {
        return Err(GuiError::conflict("Open a PR first"));
    }
    let index = state
        .files
        .iter()
        .position(|f| f.path == path)
        .ok_or_else(|| GuiError::not_found("File is no longer in this PR"))?;
    state.selected_file = index;
    let file = &state.files[index];
    let locs = file.addressable_lines();
    let locate = |location: DiffLineLocation| {
        locs.iter().position(|l| *l == location).ok_or_else(|| {
            GuiError::conflict("That line is not in this PR's diff; reopen the PR and select again")
        })
    };
    let (lo, hi) = (locate(start)?, locate(end)?);
    if lo > hi {
        return Err(GuiError::conflict("Select the range in diff order"));
    }
    // Only lines of the PR's own diff can carry an inline comment.
    let original_file = original_file(file);
    let original = original_file
        .as_ref()
        .map(|f| f.addressable_lines())
        .unwrap_or_default();
    if !(original.contains(&start) && original.contains(&end)) {
        return Err(GuiError::conflict(
            "Those lines were only reached by expanding context; GitHub takes comments on the PR's own diff lines",
        ));
    }
    let hunk_of = |location: DiffLineLocation| {
        // The PR's own hunks: expanded context may have merged neighbours.
        original_file
            .iter()
            .flat_map(|f| f.hunks.iter())
            .position(|h| {
                crate::diff::addressable_lines_in(std::slice::from_ref(h)).contains(&location)
            })
    };
    if hunk_of(start) != hunk_of(end) {
        return Err(GuiError::conflict(
            "GitHub only accepts a range inside one hunk; select lines within a single hunk",
        ));
    }
    if let Some(text) = suggestion {
        if locs[lo..=hi].iter().any(|l| l.new_line.is_none()) {
            return Err(GuiError::conflict(
                "A suggestion replaces current lines; the selection includes a removed (base-only) line",
            ));
        }
        if text.lines().any(|l| l.trim_start().starts_with("```")) {
            return Err(GuiError::conflict(
                "A suggestion cannot contain a ``` fence; it would end the suggestion block early",
            ));
        }
    }
    let mut existing = false;
    for comment in state.line_comments.get(path).into_iter().flatten() {
        let Some(range) = comment.covered_indices(&locs) else {
            continue;
        };
        if *range.start() > hi || *range.end() < lo {
            continue;
        }
        if comment.start.unwrap_or(comment.location) == start && comment.location == end {
            existing = true;
        } else {
            return Err(GuiError::conflict(
                "The selection overlaps another saved thread, and saving would replace it; select that thread's own lines, or lines clear of saved threads",
            ));
        }
    }
    crate::gui_review::set_span(state, start, end)?;
    Ok(existing)
}

fn view(gui: &mut GuiHandle) -> GuiResult<PrReviewView> {
    let context = gui.pr_review_context.as_ref().unwrap();
    let (id, revision, project_id) = (
        context.id.clone(),
        context.revision,
        context.project_id.clone(),
    );
    let embedded = context.embedded.clone();
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
        head_sha: None,
        stage: "pick",
        embedded: embedded.is_some(),
        pin_error: embedded.as_ref().and_then(|pin| pin.error.clone()),
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
                view.head_sha = Some(target.pr.head_oid.clone());
            }
            view.summary = state.general_feedback.clone();
            view.error = state.error.clone();
            let mut budget = HighlightBudget::view();
            let inline = crate::app::review::pr_inline_flags(state);
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
                    line_comments: line_comment_views(state, f, inline.get(&f.path)),
                    pr_lines: pr_lines(f),
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

    // The fixture PR replaces base line 8 with "committed" inside the hunk
    // that shows lines 5-11.
    fn at(old: Option<usize>, new: Option<usize>) -> DiffLineLocation {
        DiffLineLocation {
            old_line: old,
            new_line: new,
        }
    }
    fn ctx(n: usize) -> DiffLineLocation {
        at(Some(n), Some(n))
    }
    fn added(n: usize) -> DiffLineLocation {
        at(None, Some(n))
    }
    fn removed(n: usize) -> DiffLineLocation {
        at(Some(n), None)
    }
    fn line_comment(
        start: DiffLineLocation,
        end: DiffLineLocation,
        text: &str,
        severity: Severity,
    ) -> PrReviewAction {
        PrReviewAction::LineComment {
            path: "code.txt".into(),
            start,
            end,
            text: text.into(),
            severity,
        }
    }
    fn suggestion(start: DiffLineLocation, end: DiffLineLocation, text: &str) -> PrReviewAction {
        PrReviewAction::Suggestion {
            path: "code.txt".into(),
            start,
            end,
            text: text.into(),
        }
    }
    fn refused(gui: &mut GuiHandle, view: &PrReviewView, action: PrReviewAction) -> String {
        super::act(gui, &view.workflow_id, view.revision, action)
            .err()
            .expect("the action must be refused")
            .message
    }
    /// A feature branch with a second change far from the first, so the PR
    /// has two hunks.
    fn two_hunk_fixture() -> (tempfile::TempDir, GuiHandle, String) {
        let (dir, gui, project_id) = fixture();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        git(&repo, &["worktree", "add", wt.to_str().unwrap(), "feature"]);
        let text = std::fs::read_to_string(wt.join("code.txt")).unwrap();
        std::fs::write(
            wt.join("code.txt"),
            text.replace("line 19\n", "late change\n"),
        )
        .unwrap();
        git(
            &wt,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-am",
                "second hunk",
            ],
        );
        git(
            &repo,
            &["worktree", "remove", "--force", wt.to_str().unwrap()],
        );
        (dir, gui, project_id)
    }

    static POSTED: std::sync::Mutex<Option<PrPostRequest>> = std::sync::Mutex::new(None);
    fn capture(request: &PrPostRequest) -> PrPostOutcome {
        *POSTED.lock().unwrap() = Some(request.clone());
        PrPostOutcome::Posted {
            file_comment_failures: vec![],
        }
    }

    #[test]
    fn line_range_side_and_suggestion_comments_build_the_exact_submission() {
        let (_dir, mut gui, project_id) = fixture();
        gui.app_for_workflow()
            .pr_review_work
            .set_review_poster_for_test(capture);
        let view = opened(&mut gui, project_id);
        let head = git(
            &gui.app_for_workflow().store.projects[0].repo.clone(),
            &["rev-parse", "feature"],
        );
        assert_eq!(view.head_sha.as_deref(), Some(head.as_str()));
        let view = action(
            &mut gui,
            &view,
            line_comment(added(8), added(8), "Single", Severity::Blocker),
        );
        let view = action(
            &mut gui,
            &view,
            line_comment(ctx(5), ctx(6), "Range", Severity::Nit),
        );
        let view = action(
            &mut gui,
            &view,
            line_comment(removed(8), removed(8), "Base side", Severity::Question),
        );
        let view = action(&mut gui, &view, suggestion(ctx(10), ctx(11), "ten\neleven"));
        let threads = &view.files[0].line_comments;
        assert_eq!(threads.len(), 4);
        assert_eq!(
            threads.iter().map(|t| t.side).collect::<Vec<_>>(),
            ["RIGHT", "RIGHT", "LEFT", "RIGHT"]
        );
        assert!(threads.iter().all(|t| t.inline && t.note.is_none()));
        assert_eq!(threads[0].anchor, "line 5 – line 6");
        assert_eq!(threads[2].anchor, "base line 8");
        assert_eq!(threads[3].suggestion.as_deref(), Some("ten\neleven"));

        let view = action(
            &mut gui,
            &view,
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
        );
        let comment =
            |line, side, start_line, start_side, body: &str| crate::github::PrReviewComment {
                path: "code.txt".into(),
                line,
                side,
                start_line,
                start_side,
                body: body.into(),
            };
        let expected = vec![
            comment(6, "RIGHT", Some(5), Some("RIGHT"), "**[nit]** Range"),
            comment(8, "RIGHT", None, None, "**[blocker]** Single"),
            comment(8, "LEFT", None, None, "**[question]** Base side"),
            comment(
                11,
                "RIGHT",
                Some(10),
                Some("RIGHT"),
                "**[suggestion]**\n\n```suggestion\nten\neleven\n```",
            ),
        ];
        let submission = view.submission.as_ref().unwrap();
        assert_eq!(submission.comments, expected);
        assert!(submission.file_comments.is_empty());
        assert_eq!(submission.body, "");

        // Posting sends that same payload, pinned to the reviewed head.
        let view = action(&mut gui, &view, PrReviewAction::ConfirmSubmit);
        settle(&mut gui, view);
        let posted = POSTED.lock().unwrap().take().unwrap();
        assert_eq!(posted.comments, expected);
        assert_eq!(posted.pr.head_sha, head);
    }

    #[test]
    fn line_threads_are_the_shared_draft_and_resolve_locally() {
        let (_dir, mut gui, project_id) = fixture();
        let view = opened(&mut gui, project_id);
        let view = action(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(9), "Check this", Severity::Suggestion),
        );
        // The TUI reads the same row: the saved progress holds the thread.
        let db = gui.app_for_workflow().db.as_ref().unwrap();
        let draft = db.load_pr_review_draft(REPO, 9).unwrap().unwrap();
        let progress: serde_json::Value = serde_json::from_str(&draft.progress).unwrap();
        let saved = progress["line_comments"]["code.txt"].as_array().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0]["text"], "Check this");
        assert_eq!(saved[0]["location"]["new_line"], 9);

        let resolve = PrReviewAction::ToggleLineResolved {
            path: "code.txt".into(),
            start: ctx(9),
            end: ctx(9),
        };
        let view = action(&mut gui, &view, resolve);
        assert!(view.files[0].line_comments[0].resolved);
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
        );
        // A resolved thread is kept locally and never posted.
        assert!(view.submission.as_ref().unwrap().comments.is_empty());
        let view = action(&mut gui, &view, PrReviewAction::CancelSubmit);
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::ToggleLineResolved {
                path: "code.txt".into(),
                start: ctx(9),
                end: ctx(9),
            },
        );
        assert!(!view.files[0].line_comments[0].resolved);
        let view = action(&mut gui, &view, PrReviewAction::Back);
        let view = action(&mut gui, &view, PrReviewAction::Open { number: 9 });
        let view = settle(&mut gui, view);
        assert_eq!(view.files[0].line_comments[0].text, "Check this");
        // An empty save at the thread's own span removes the prose (and thread).
        let view = action(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(9), "", Severity::Suggestion),
        );
        assert!(view.files[0].line_comments.is_empty());
    }

    #[test]
    fn invalid_locations_suggestions_and_overlaps_are_refused_with_a_reason() {
        let (_dir, mut gui, project_id) = two_hunk_fixture();
        let view = opened(&mut gui, project_id);
        assert_eq!(view.files[0].diff.hunks.len(), 2);
        let late = view.files[0].diff.hunks[1]
            .lines
            .iter()
            .find(|l| l.new_line == Some(19))
            .map(|_| added(19))
            .unwrap();

        // A location that is not in the diff at all.
        let error = refused(
            &mut gui,
            &view,
            line_comment(ctx(1), ctx(1), "x", Severity::Nit),
        );
        assert!(error.contains("not in this PR's diff"), "{error}");
        // A range given backwards.
        let error = refused(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(6), "x", Severity::Nit),
        );
        assert!(error.contains("diff order"), "{error}");
        // A range across hunks.
        let error = refused(
            &mut gui,
            &view,
            line_comment(ctx(9), late, "x", Severity::Nit),
        );
        assert!(error.contains("single hunk"), "{error}");
        let error = refused(&mut gui, &view, suggestion(ctx(9), late, "x"));
        assert!(error.contains("single hunk"), "{error}");
        // A suggestion on removed-only lines, or a range that includes one.
        let error = refused(&mut gui, &view, suggestion(removed(8), removed(8), "x"));
        assert!(error.contains("removed"), "{error}");
        let error = refused(&mut gui, &view, suggestion(ctx(7), added(8), "x"));
        assert!(error.contains("removed"), "{error}");
        // A suggestion that would close its own fence early.
        let error = refused(&mut gui, &view, suggestion(ctx(9), ctx(9), "a\n```\nb"));
        assert!(error.contains("fence"), "{error}");
        // Unknown file, and empty saves with nothing to remove.
        let error = refused(
            &mut gui,
            &view,
            PrReviewAction::LineComment {
                path: "gone.txt".into(),
                start: ctx(9),
                end: ctx(9),
                text: "x".into(),
                severity: Severity::Nit,
            },
        );
        assert!(error.contains("no longer in this PR"), "{error}");
        let error = refused(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(9), "  ", Severity::Nit),
        );
        assert!(error.contains("Write a comment"), "{error}");
        let error = refused(&mut gui, &view, suggestion(ctx(9), ctx(9), ""));
        assert!(error.contains("Write a replacement"), "{error}");
        // Resolving a thread that does not exist.
        let error = refused(
            &mut gui,
            &view,
            PrReviewAction::ToggleLineResolved {
                path: "code.txt".into(),
                start: ctx(9),
                end: ctx(9),
            },
        );
        assert!(error.contains("no longer at this anchor"), "{error}");

        // Nothing above changed the draft or the submission.
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
        );
        assert!(
            gui.app_for_workflow()
                .db
                .as_ref()
                .unwrap()
                .load_pr_review_draft(REPO, 9)
                .unwrap()
                .is_none()
        );
        assert!(view.submission.as_ref().unwrap().comments.is_empty());
        let view = action(&mut gui, &view, PrReviewAction::CancelSubmit);

        // Overlapping an existing thread would silently replace it.
        let view = action(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(10), "Kept", Severity::Nit),
        );
        let error = refused(
            &mut gui,
            &view,
            line_comment(ctx(10), ctx(11), "x", Severity::Nit),
        );
        assert!(error.contains("overlaps another saved thread"), "{error}");
        let error = refused(&mut gui, &view, suggestion(ctx(9), ctx(9), "x"));
        assert!(error.contains("overlaps another saved thread"), "{error}");
        assert_eq!(view.files[0].line_comments[0].text, "Kept");
        // The thread's own span still edits it, keeping any suggestion.
        let view = action(&mut gui, &view, suggestion(ctx(9), ctx(10), "nine\nten"));
        let view = action(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(10), "Edited", Severity::Blocker),
        );
        let thread = &view.files[0].line_comments[0];
        assert_eq!(thread.text, "Edited");
        assert_eq!(thread.suggestion.as_deref(), Some("nine\nten"));
        assert_eq!(view.files[0].line_comments.len(), 1);
    }

    #[test]
    fn actions_are_bound_to_the_revision_the_draft_and_the_open_preview() {
        let (_dir, mut gui, project_id) = fixture();
        let initial = opened(&mut gui, project_id);
        let view = action(
            &mut gui,
            &initial,
            line_comment(ctx(9), ctx(9), "First", Severity::Nit),
        );
        // A stale view (older revision) cannot add a thread.
        let error = super::act(
            &mut gui,
            &initial.workflow_id,
            initial.revision,
            line_comment(ctx(6), ctx(6), "Stale", Severity::Nit),
        )
        .err()
        .unwrap();
        assert!(error.message.contains("changed"), "{}", error.message);
        // Another interface saved the draft: line actions refuse until reopen.
        let db = gui.app_for_workflow().db.as_ref().unwrap();
        let mut draft = db.load_pr_review_draft(REPO, 9).unwrap().unwrap();
        draft.progress = draft.progress.replace("First", "Elsewhere");
        db.upsert_pr_review_draft(&draft).unwrap();
        for next in [
            line_comment(ctx(6), ctx(6), "x", Severity::Nit),
            suggestion(ctx(6), ctx(6), "x"),
            PrReviewAction::ToggleLineResolved {
                path: "code.txt".into(),
                start: ctx(9),
                end: ctx(9),
            },
        ] {
            let error = refused(&mut gui, &view, next);
            assert!(error.contains("another interface"), "{error}");
        }
        let view = action(&mut gui, &view, PrReviewAction::Back);
        let view = action(&mut gui, &view, PrReviewAction::Open { number: 9 });
        let view = settle(&mut gui, view);
        assert_eq!(view.files[0].line_comments[0].text, "Elsewhere");

        // While the submission preview is open, the comments are frozen.
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
        );
        let error = refused(
            &mut gui,
            &view,
            line_comment(ctx(6), ctx(6), "x", Severity::Nit),
        );
        assert!(error.contains("submission preview"), "{error}");
        let error = refused(
            &mut gui,
            &view,
            PrReviewAction::ToggleLineResolved {
                path: "code.txt".into(),
                start: ctx(9),
                end: ctx(9),
            },
        );
        assert!(error.contains("submission preview"), "{error}");
    }

    #[test]
    fn a_moved_head_still_blocks_posting_a_review_with_line_comments() {
        let (_dir, mut gui, project_id) = fixture();
        gui.app_for_workflow()
            .pr_review_work
            .set_review_poster_for_test(moved);
        let view = opened(&mut gui, project_id);
        let view = action(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(10), "Range", Severity::Blocker),
        );
        let view = action(
            &mut gui,
            &view,
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
        );
        let view = action(&mut gui, &view, PrReviewAction::ConfirmSubmit);
        let view = settle(&mut gui, view);
        let submission = view.submission.as_ref().unwrap();
        assert!(submission.head_moved);
        assert_eq!(submission.comments.len(), 1);
        assert_eq!(submission.comments[0].start_line, Some(9));
        // Confirming again is refused instead of posting at the old head.
        let view = action(&mut gui, &view, PrReviewAction::ConfirmSubmit);
        assert!(view.submission.as_ref().unwrap().head_moved);
    }

    #[test]
    fn a_failed_draft_save_is_reported_and_the_thread_is_not_stored() {
        let (dir, mut gui, project_id) = fixture();
        let view = opened(&mut gui, project_id);
        let conn = rusqlite::Connection::open(dir.path().join("amf.db")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER block_draft BEFORE INSERT ON pr_review_drafts
             BEGIN SELECT RAISE(ABORT, 'drafts are read-only'); END;",
        )
        .unwrap();
        let error = refused(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(9), "Lost?", Severity::Nit),
        );
        assert!(error.contains("drafts are read-only"), "{error}");
        // The view does not show a thread that is not in storage.
        let snapshot = poll(&mut gui, &view.workflow_id).unwrap();
        assert!(snapshot.files[0].line_comments.is_empty());
        assert!(
            gui.app_for_workflow()
                .db
                .as_ref()
                .unwrap()
                .load_pr_review_draft(REPO, 9)
                .unwrap()
                .is_none()
        );
        // Once storage works again, the same action saves.
        conn.execute_batch("DROP TRIGGER block_draft;").unwrap();
        let snapshot = poll(&mut gui, &view.workflow_id).unwrap();
        let saved = action(
            &mut gui,
            &snapshot,
            line_comment(ctx(9), ctx(9), "Lost?", Severity::Nit),
        );
        assert_eq!(saved.files[0].line_comments.len(), 1);
        assert!(
            gui.app_for_workflow()
                .db
                .as_ref()
                .unwrap()
                .load_pr_review_draft(REPO, 9)
                .unwrap()
                .is_some()
        );
    }

    // --- PR Workspace Files tab (embedded) ---------------------------------

    fn workspace(head: Option<&str>) -> (tempfile::TempDir, GuiHandle, String, String) {
        let (dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let project_id = target.project_id.clone();
        let app = gui.app_for_workflow();
        app.pr_review_work.set_review_list_loader_for_test(listed);
        app.pr_review_work.set_review_opener_for_test(opener);
        app.pr_review_work.set_review_poster_for_test(poster);
        let repo = app.store.projects[0].repo.clone();
        let real = git(&repo, &["rev-parse", "feature"]);
        let id = crate::gui_pr_workspace::begin(&mut gui, target).unwrap();
        gui.pr_workspace_context.as_mut().unwrap().pr = Some(crate::github::PrRef {
            number: 9,
            head_sha: head.map(String::from).unwrap_or(real),
            url: "https://example.test/pull/9".into(),
            owner: "demo".into(),
            repo: "repo".into(),
            head_ref: "feature".into(),
        });
        (dir, gui, id, project_id)
    }
    fn files_open(gui: &mut GuiHandle, id: &str) -> PrReviewView {
        let view = crate::gui_pr_workspace::review_open(gui, id, 9).unwrap();
        settle(gui, view)
    }
    fn pinned_error(view: &PrReviewView) -> String {
        view.pin_error.clone().expect("the review must be refused")
    }

    #[test]
    fn files_tab_opens_the_pinned_pr_and_shares_its_draft_with_the_dialog() {
        let (_dir, mut gui, id, project_id) = workspace(None);
        let view = files_open(&mut gui, &id);
        assert!(view.embedded);
        assert_eq!(view.stage, "review");
        assert_eq!(view.number, Some(9));
        assert!(view.pin_error.is_none());
        {
            let ws = gui.pr_workspace_context.as_ref().unwrap();
            assert!(ws.files_review && ws.files_prior.is_some());
        }
        let view = action(
            &mut gui,
            &view,
            line_comment(ctx(5), ctx(6), "From Files", Severity::Nit),
        );
        let view = action(&mut gui, &view, suggestion(ctx(10), ctx(11), "ten\neleven"));
        assert_eq!(view.files[0].line_comments.len(), 2);
        // Posting and the summary are not Files actions.
        for refused in [
            PrReviewAction::Summary { text: "x".into() },
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
            PrReviewAction::ConfirmSubmit,
            PrReviewAction::Reopen,
            PrReviewAction::Back,
            PrReviewAction::Close,
        ] {
            let error = super::act(&mut gui, &view.workflow_id, view.revision, refused)
                .err()
                .unwrap();
            assert!(error.message.contains("PR Review"), "{}", error.message);
        }
        // The dialog cannot open while Files owns the review.
        let error = begin(&mut gui, project_id.clone()).err().unwrap();
        assert!(error.message.contains("Files tab"), "{}", error.message);

        // Leaving Files saves, returns the Threads mode, and frees the dialog.
        crate::gui_pr_workspace::switch_to_threads(&mut gui, &id).unwrap();
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
        {
            let ws = gui.pr_workspace_context.as_ref().unwrap();
            assert!(!ws.files_review && ws.files_prior.is_none());
        }
        assert!(gui.pr_review_context.is_none());
        let dialog = opened(&mut gui, project_id);
        let threads = &dialog.files[0].line_comments;
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].text, "From Files");
        assert_eq!(threads[1].suggestion.as_deref(), Some("ten\neleven"));
        // And the dialog's preview is the submit path for those comments.
        let dialog = action(
            &mut gui,
            &dialog,
            PrReviewAction::Preview {
                event: "COMMENT".into(),
            },
        );
        assert_eq!(dialog.submission.unwrap().comments.len(), 2);
    }

    #[test]
    fn files_tab_reuses_the_dialogs_validation() {
        let (_dir, mut gui, id, _) = workspace(None);
        let view = files_open(&mut gui, &id);
        let error = refused(
            &mut gui,
            &view,
            line_comment(ctx(1), ctx(1), "x", Severity::Nit),
        );
        assert!(error.contains("not in this PR's diff"), "{error}");
        let error = refused(&mut gui, &view, suggestion(removed(8), removed(8), "x"));
        assert!(error.contains("removed"), "{error}");
        let error = refused(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(6), "x", Severity::Nit),
        );
        assert!(error.contains("diff order"), "{error}");
        // A draft saved by another interface is not overwritten.
        let view = action(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(9), "mine", Severity::Nit),
        );
        let db = gui.app_for_workflow().db.as_ref().unwrap();
        let mut draft = db.load_pr_review_draft(REPO, 9).unwrap().unwrap();
        draft.progress = draft.progress.replace("mine", "theirs");
        db.upsert_pr_review_draft(&draft).unwrap();
        let error = refused(
            &mut gui,
            &view,
            line_comment(ctx(10), ctx(10), "later", Severity::Nit),
        );
        assert!(error.contains("another interface"), "{error}");
        crate::gui_pr_workspace::review_close(&mut gui, &id).unwrap();
        let saved = gui
            .app_for_workflow()
            .db
            .as_ref()
            .unwrap()
            .load_pr_review_draft(REPO, 9)
            .unwrap()
            .unwrap();
        assert!(saved.progress.contains("theirs") && !saved.progress.contains("later"));
    }

    #[test]
    fn files_tab_refuses_a_head_that_differs_from_the_workspace() {
        let (_dir, mut gui, id, _) = workspace(Some("0000000000000000000000000000000000000000"));
        let view = files_open(&mut gui, &id);
        let error = pinned_error(&view);
        assert!(error.contains("pinned to one head"), "{error}");
        for edit in [
            line_comment(ctx(9), ctx(9), "x", Severity::Nit),
            suggestion(ctx(9), ctx(9), "x"),
            PrReviewAction::FileComment {
                path: "code.txt".into(),
                text: "x".into(),
            },
        ] {
            let refusal = refused(&mut gui, &view, edit);
            assert!(refusal.contains("pinned to one head"), "{refusal}");
        }
        assert!(
            gui.app_for_workflow()
                .db
                .as_ref()
                .unwrap()
                .load_pr_review_draft(REPO, 9)
                .unwrap()
                .is_none()
        );
        crate::gui_pr_workspace::review_close(&mut gui, &id).unwrap();
        assert!(gui.pr_review_context.is_none());
    }

    #[test]
    fn files_tab_refuses_a_pr_the_dialog_does_not_list() {
        let (_dir, mut gui, id, _) = workspace(None);
        gui.pr_workspace_context
            .as_mut()
            .unwrap()
            .pr
            .as_mut()
            .unwrap()
            .number = 77;
        let view = crate::gui_pr_workspace::review_open(&mut gui, &id, 77).unwrap();
        let view = settle(&mut gui, view);
        assert!(pinned_error(&view).contains("PR #77 is not in the open pull requests"));
    }

    #[test]
    fn files_tab_and_other_tabs_never_own_the_mode_together() {
        let (_dir, mut gui, id, _) = workspace(None);
        // A different PR cannot be opened than the one selected.
        assert!(crate::gui_pr_workspace::review_open(&mut gui, &id, 5).is_err());
        assert!(!gui.pr_workspace_context.as_ref().unwrap().files_review);
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
        let view = files_open(&mut gui, &id);
        let again = crate::gui_pr_workspace::review_open(&mut gui, &id, 9).unwrap();
        assert_eq!(again.workflow_id, view.workflow_id);
        // Closing the workspace closes the review, saves, and frees the mode.
        let view = action(
            &mut gui,
            &view,
            line_comment(ctx(9), ctx(9), "kept", Severity::Nit),
        );
        let _ = view;
        crate::gui_pr_workspace::close(&mut gui, &id).unwrap();
        assert!(gui.pr_review_context.is_none());
        assert!(gui.pr_workspace_context.is_none());
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
        assert!(
            gui.app_for_workflow()
                .db
                .as_ref()
                .unwrap()
                .load_pr_review_draft(REPO, 9)
                .unwrap()
                .unwrap()
                .progress
                .contains("kept")
        );
    }

    #[test]
    fn the_dialog_blocks_the_files_review_for_the_same_project() {
        let (_dir, mut gui, id, project_id) = workspace(None);
        let dialog = begin(&mut gui, project_id).unwrap();
        let _ = dialog;
        let error = crate::gui_pr_workspace::review_open(&mut gui, &id, 9)
            .err()
            .unwrap();
        assert!(error.message.contains("Close PR Review") || error.message.contains("Finish"));
        // A failed open puts the displaced mode back.
        assert!(!gui.pr_workspace_context.as_ref().unwrap().files_review);
        assert!(
            gui.pr_workspace_context
                .as_ref()
                .unwrap()
                .files_prior
                .is_none()
        );
    }
}
