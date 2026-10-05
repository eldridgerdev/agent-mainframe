//! Manual final review over the TUI's authoritative review state and saved
//! progress. No agent or running tmux session is required.
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::app::{AppMode, DiffViewerState, ReviewDecision, ViewState};
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};
use crate::gui_diff::{DiffFileView, file_view};
use crate::project::SessionKind;

mod ai;
mod checks;
mod summary;
pub use ai::{
    ReviewAiView, ReviewCommentDraftView, ReviewCommentEditorView, ReviewDraftDestination,
    ReviewQuestionView,
};
pub use checks::{ReviewCheckStatus, ReviewCheckView, poll};
pub use summary::{ReviewSummaryRow, ReviewSummaryView};

pub use crate::app::review::state::{FileComment, Severity};
pub use crate::diff::DiffLineLocation;

pub(crate) struct ReviewContext {
    id: String,
    target: FeatureTarget,
    revision: u64,
    progress: Option<Vec<u8>>,
    save_error: Option<String>,
    ready_comment: Option<ReviewCommentEditorView>,
    check_run: Option<crate::app::review::ReviewCheckRun>,
    check: Option<ReviewCheckView>,
}

#[derive(Debug, Serialize)]
pub struct ReviewFileView {
    pub diff: DiffFileView,
    pub verdict: &'static str,
    pub feedback: String,
    pub severity: Severity,
    pub comment: Option<FileComment>,
    /// Line threads share the TUI anchors and progress format.
    pub line_comments: Vec<ReviewLineCommentView>,
    pub notes: Option<String>,
    pub walkthrough: Option<String>,
    pub changed_since_last: bool,
}

#[derive(Debug, Serialize)]
pub struct ReviewLineCommentView {
    pub start: DiffLineLocation,
    pub end: DiffLineLocation,
    pub editable: bool,
    pub anchor: String,
    pub text: String,
    pub severity: Severity,
    pub resolved: bool,
    pub draft: bool,
    pub anchor_lost: bool,
    pub suggestion: Option<String>,
    /// A shared-engine explanation when this thread cannot be written locally.
    pub apply_blocked: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReviewView {
    pub workflow_id: String,
    pub revision: u64,
    pub target: FeatureTarget,
    pub feature_name: String,
    pub branch: String,
    pub base_ref: String,
    pub files: Vec<ReviewFileView>,
    pub selected_path: Option<String>,
    pub general_feedback: String,
    pub has_prior_review: bool,
    pub error: Option<String>,
    pub save_error: Option<String>,
    pub applied_suggestions: Vec<String>,
    pub history: Option<ReviewHistoryView>,
    pub summary: Option<ReviewSummaryView>,
    pub check_command: Option<String>,
    pub check: Option<ReviewCheckView>,
    pub ai: ReviewAiView,
}

#[derive(Debug, Serialize)]
pub struct ReviewHistoryView {
    /// Current is index zero; completed rounds are newest first.
    pub selected: usize,
    pub rounds: Vec<ReviewHistoryRoundView>,
    pub markdown: String,
    pub current_unresolved: usize,
    pub archive_available: bool,
    pub archive_loaded: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReviewHistoryRoundView {
    pub title: String,
    pub carried_unresolved: usize,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewAction {
    SummaryOpen,
    SummaryClose,
    ApplyFinishSuggestions,
    RunCheck {
        command: String,
    },
    CancelCheck,
    HistoryOpen,
    HistorySelect {
        round: usize,
    },
    HistoryLoadOlder,
    HistoryClose,
    Walkthrough {
        path: String,
    },
    CoReview {
        path: String,
    },
    Overview,
    Ask {
        path: String,
        start: Option<DiffLineLocation>,
        end: Option<DiffLineLocation>,
        question: String,
        harness: crate::project::AgentKind,
    },
    DraftQuestion {
        turn: usize,
        destination: ReviewDraftDestination,
    },
    TransferQuestionDraft {
        request: u64,
        text: String,
    },
    DiscardQuestionDraft,
    AcceptDraft {
        path: String,
        start: DiffLineLocation,
        end: DiffLineLocation,
    },
    DismissDraft {
        path: String,
        start: DiffLineLocation,
        end: DiffLineLocation,
    },
    CancelAi,
    PrecallConfirm,
    PrecallCancel,
    PrecallToggleView,
    Select {
        path: String,
    },
    Approve {
        path: String,
    },
    Skip {
        path: String,
    },
    Reject {
        path: String,
        feedback: String,
        severity: Severity,
    },
    Comment {
        path: String,
        text: String,
        severity: Severity,
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
    ApplySuggestion {
        path: String,
        start: DiffLineLocation,
        end: DiffLineLocation,
    },
    ToggleLineResolved {
        path: String,
        start: DiffLineLocation,
        end: DiffLineLocation,
    },
    ToggleResolved {
        path: String,
    },
    General {
        text: String,
    },
    Undo,
    Refresh,
    /// Explicitly discard this view and reload the saved review after an
    /// external edit. Kept separate from refreshing the diff.
    Reload,
    RetrySave,
    Pause,
    /// Close without saving, abandoning any edits a failed save left unsaved.
    /// The way out when saving keeps failing or the feature is gone.
    Discard,
}

fn progress_bytes(workdir: &Path) -> GuiResult<Option<Vec<u8>>> {
    match std::fs::read(crate::app::review::review_progress_path(workdir)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(err) => Err(anyhow::Error::from(err).into()),
    }
}

/// Open the review over the feature's checkout and return the saved progress
/// as it was read *before* the shared loader restored it, which is the
/// baseline for detecting a save by another interface: if one lands between
/// this read and the restore, the next action reports a conflict instead of
/// overwriting it.
fn open_state(gui: &mut GuiHandle, target: &FeatureTarget) -> GuiResult<Option<Vec<u8>>> {
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted; close this review"))?;
    let project = &app.store.projects[pi];
    if !project.is_git {
        return Err(GuiError::conflict("Final Review requires a Git checkout"));
    }
    let feature = &project.features[fi];
    let progress = crate::app::review::checked_review_progress(&feature.workdir)?;
    let from_view = ViewState::new(
        project.name.clone(),
        feature.name.clone(),
        feature.tmux_session.clone(),
        String::new(),
        String::new(),
        SessionKind::Terminal,
        feature.mode.clone(),
        feature.review,
    );
    let mut state = DiffViewerState::new(from_view, feature.workdir.clone());
    state.review = true;
    let previous_mode = std::mem::replace(&mut app.mode, AppMode::DiffViewerLoading(state));
    app.complete_diff_viewer_loading();
    if let AppMode::DiffViewer(state) = &app.mode
        && let Some(error) = &state.error
    {
        let error = GuiError::from(anyhow::anyhow!("Could not load review changes: {error}"));
        app.mode = previous_mode;
        return Err(error);
    }
    Ok(progress)
}

pub fn begin(gui: &mut GuiHandle, target: FeatureTarget) -> GuiResult<ReviewView> {
    gui.refresh_snapshot()?;
    if let Some(context) = &gui.review_context {
        if context.target.project_id == target.project_id
            && context.target.feature_id == target.feature_id
        {
            return snapshot(gui);
        }
        return Err(GuiError::conflict(
            "Pause Final Review before opening another feature",
        ));
    }
    let app = gui.app_for_workflow();
    if !matches!(app.mode, AppMode::Normal) || app.paused_plan_interview.is_some() {
        return Err(GuiError::conflict(
            "Finish the current workflow before opening Final Review",
        ));
    }
    let progress = open_state(gui, &target)?;
    gui.review_context = Some(ReviewContext {
        id: uuid::Uuid::new_v4().to_string(),
        target,
        revision: 0,
        progress,
        save_error: None,
        ready_comment: None,
        check_run: None,
        check: None,
    });
    snapshot(gui)
}

pub fn snapshot(gui: &mut GuiHandle) -> GuiResult<ReviewView> {
    let context = gui
        .review_context
        .as_ref()
        .ok_or_else(|| GuiError::conflict("Final Review is no longer open"))?;
    let (workflow_id, revision, target, save_error) = (
        context.id.clone(),
        context.revision,
        context.target.clone(),
        context.save_error.clone(),
    );
    let ready_comment = context.ready_comment.clone();
    let check = context.check.clone();
    let check_command = checks::command(gui);
    let app = gui.app_for_workflow();
    let state = ai::state(&app.mode)?;
    let mut ai = ai::view(app)?;
    ai.ready_comment = ready_comment;
    let files = state
        .files
        .iter()
        .map(|file| {
            let locations = file.addressable_lines();
            let (verdict, feedback, severity) = match state.decisions.get(&file.path) {
                Some(ReviewDecision::Approve) => ("approved", String::new(), Severity::default()),
                Some(ReviewDecision::Reject { feedback, severity }) => {
                    ("rejected", feedback.clone(), *severity)
                }
                None => ("undecided", String::new(), Severity::default()),
            };
            ReviewFileView {
                diff: file_view(file.clone(), 3),
                verdict,
                feedback,
                severity,
                comment: state.file_comments.get(&file.path).cloned(),
                line_comments: state
                    .line_comments
                    .get(&file.path)
                    .into_iter()
                    .flatten()
                    .map(|comment| ReviewLineCommentView {
                        start: comment.start.unwrap_or(comment.location),
                        end: comment.location,
                        editable: !comment.anchor_lost
                            && locations.contains(&comment.location)
                            && comment.start.is_none_or(|start| locations.contains(&start)),
                        anchor: {
                            let label = |loc: DiffLineLocation| match (loc.old_line, loc.new_line) {
                                (_, Some(line)) => format!("line {line}"),
                                (Some(line), None) => format!("base line {line}"),
                                _ => "lost anchor".to_string(),
                            };
                            match comment.start {
                                Some(start) => {
                                    format!("{} – {}", label(start), label(comment.location))
                                }
                                None => label(comment.location),
                            }
                        },
                        text: comment.text.clone(),
                        severity: comment.severity,
                        resolved: comment.resolved,
                        draft: comment.draft,
                        anchor_lost: comment.anchor_lost,
                        suggestion: comment.suggestion.clone(),
                        apply_blocked: crate::app::review::local_suggestion_blocker(file, comment),
                    })
                    .collect(),
                notes: state.review_notes.get(&file.path).cloned(),
                walkthrough: state.generated_notes.get(&file.path).cloned(),
                changed_since_last: state.changed_since_last.contains(&file.path),
            }
        })
        .collect();
    Ok(ReviewView {
        workflow_id,
        revision,
        target,
        save_error,
        files,
        feature_name: state.from_view.feature_name.clone(),
        branch: state.branch.clone(),
        base_ref: state.base_ref.clone(),
        selected_path: state.files.get(state.selected_file).map(|f| f.path.clone()),
        general_feedback: state.general_feedback.clone(),
        has_prior_review: state.has_prior_review,
        error: state.error.clone(),
        applied_suggestions: state.applied_suggestions.clone(),
        summary: summary::view(state),
        check_command,
        check,
        history: state
            .review_history
            .as_ref()
            .map(|history| ReviewHistoryView {
                selected: history.selected,
                rounds: history
                    .rounds
                    .iter()
                    .map(|round| ReviewHistoryRoundView {
                        title: round.title.clone(),
                        carried_unresolved: round.carried_unresolved,
                    })
                    .collect(),
                markdown: history
                    .selected
                    .checked_sub(1)
                    .and_then(|index| history.rounds.get(index))
                    .map(|round| round.markdown.clone())
                    .unwrap_or_else(|| crate::app::review::current_review_history_markdown(state)),
                current_unresolved: state.unresolved_thread_count(),
                archive_available: history.archive_available,
                archive_loaded: history.archive_loaded,
                error: history.error_message(),
            }),
        ai,
    })
}

pub fn act(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
    action: ReviewAction,
) -> GuiResult<Option<ReviewView>> {
    let preserve_draft = matches!(
        action,
        ReviewAction::DraftQuestion { .. }
            | ReviewAction::TransferQuestionDraft { .. }
            | ReviewAction::PrecallConfirm
            | ReviewAction::PrecallCancel
            | ReviewAction::PrecallToggleView
            | ReviewAction::CancelAi
            | ReviewAction::RetrySave
            | ReviewAction::SummaryOpen
            | ReviewAction::SummaryClose
    );
    let pause = matches!(action, ReviewAction::Pause);
    let reload = matches!(action, ReviewAction::Reload);
    let refresh = matches!(action, ReviewAction::Refresh);
    let context = gui
        .review_context
        .as_ref()
        .filter(|c| c.id == workflow_id && c.revision == revision)
        .ok_or_else(|| GuiError::conflict("Review changed; retry from the current view"))?;
    if context.ready_comment.is_some()
        && matches!(
            action,
            ReviewAction::Ask { .. }
                | ReviewAction::DraftQuestion { .. }
                | ReviewAction::ApplyFinishSuggestions
        )
    {
        return Err(GuiError::conflict(
            "Save or discard the transferred comment draft first",
        ));
    }
    if context.check_run.is_some()
        && !matches!(
            action,
            ReviewAction::SummaryOpen
                | ReviewAction::SummaryClose
                | ReviewAction::HistoryOpen
                | ReviewAction::HistorySelect { .. }
                | ReviewAction::HistoryLoadOlder
                | ReviewAction::HistoryClose
                | ReviewAction::CancelCheck
                | ReviewAction::Pause
                | ReviewAction::Discard
        )
    {
        return Err(GuiError::conflict(
            "Wait for or cancel the running check before changing this review",
        ));
    }
    if matches!(action, ReviewAction::CancelCheck) {
        if context.check_run.is_none() {
            return Err(GuiError::conflict("No review check is running"));
        }
        checks::cancel(
            gui,
            ReviewCheckStatus::Cancelled,
            "Cancelled by reviewer".into(),
        );
        gui.review_context.as_mut().unwrap().revision += 1;
        return snapshot(gui).map(Some);
    }
    let target = context.target.clone();
    let unsaved = context.save_error.is_some();
    let app = gui.app_for_workflow();
    let in_precall = matches!(app.mode, AppMode::PromptPrecall(_));
    if in_precall
        && !matches!(
            action,
            ReviewAction::PrecallConfirm
                | ReviewAction::PrecallCancel
                | ReviewAction::PrecallToggleView
                | ReviewAction::CancelAi
                | ReviewAction::Pause
                | ReviewAction::Discard
        )
    {
        return Err(GuiError::conflict(
            "Continue or cancel the pending AI call first",
        ));
    }
    if matches!(
        action,
        ReviewAction::PrecallCancel
            | ReviewAction::PrecallToggleView
            | ReviewAction::CancelAi
            | ReviewAction::DiscardQuestionDraft
            | ReviewAction::HistoryClose
            | ReviewAction::SummaryClose
    ) {
        if matches!(action, ReviewAction::SummaryClose) {
            app.close_review_summary();
        } else if matches!(action, ReviewAction::HistoryClose) {
            // Returning to the editor must remain possible after the target
            // disappears, so the reviewer can pause or discard their edits.
            app.close_review_history();
        } else {
            ai::apply(app, &action)?;
        }
        let context = gui.review_context.as_mut().unwrap();
        if matches!(action, ReviewAction::DiscardQuestionDraft) {
            context.ready_comment = None;
        }
        context.revision += 1;
        return snapshot(gui).map(Some);
    }
    let context = gui.review_context.as_ref().unwrap();
    if unsaved
        && matches!(
            action,
            ReviewAction::ApplySuggestion { .. } | ReviewAction::ApplyFinishSuggestions
        )
    {
        return Err(GuiError::conflict(
            "Save review progress successfully before applying a suggestion",
        ));
    }
    // Pausing an already-saved review requires no writes, including after its
    // feature was deleted or another interface updated the saved progress.
    // Discarding never writes: it is the exit when a save cannot succeed.
    let close_now = match action {
        ReviewAction::Pause => context.save_error.is_none(),
        ReviewAction::Discard => true,
        _ => false,
    };
    if close_now {
        ai::cancel(gui.app_for_workflow());
        gui.app_for_workflow().mode = AppMode::Normal;
        gui.review_context = None;
        return Ok(None);
    }
    gui.refresh_snapshot()?;
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| {
            GuiError::not_found(if unsaved {
                "Feature was deleted; discard the unsaved progress to close this review"
            } else {
                "Feature was deleted; pause this review"
            })
        })?;
    let workdir = app.store.projects[pi].features[fi].workdir.clone();
    let state = ai::state(&app.mode)?;
    if state.workdir != workdir {
        return Err(GuiError::conflict(
            "Feature checkout changed; pause and reopen Final Review",
        ));
    }
    // History navigation only reads shared state/files. It must never save
    // progress, discard drafts, cancel workers or hide an outstanding save
    // error, even when the saved review has changed externally. (A changed
    // checkout is still refused above: history reads the review's workdir.)
    if matches!(
        action,
        ReviewAction::HistoryOpen
            | ReviewAction::HistorySelect { .. }
            | ReviewAction::HistoryLoadOlder
            | ReviewAction::SummaryOpen
    ) {
        match action {
            ReviewAction::SummaryOpen => {
                app.close_review_history();
                app.open_review_summary();
            }
            ReviewAction::HistoryOpen => app.open_review_history(),
            ReviewAction::HistorySelect { round } => {
                let history = state
                    .review_history
                    .as_ref()
                    .ok_or_else(|| GuiError::conflict("Open review history first"))?;
                if round > history.rounds.len() {
                    return Err(GuiError::conflict("Review round is not loaded"));
                }
                let delta = round as isize - history.selected as isize;
                app.review_history_move(delta);
            }
            ReviewAction::HistoryLoadOlder => {
                if state.review_history.is_none() {
                    return Err(GuiError::conflict("Open review history first"));
                }
                app.load_review_history_archive();
            }
            _ => unreachable!(),
        }
        gui.review_context.as_mut().unwrap().revision += 1;
        return snapshot(gui).map(Some);
    }
    if state.error.is_some() && !reload && !refresh {
        return Err(GuiError::conflict(
            "Refresh changes successfully before editing this review",
        ));
    }
    if !reload && progress_bytes(&workdir)? != gui.review_context.as_ref().unwrap().progress {
        return Err(GuiError::conflict(
            "Saved review changed in another interface; reload the saved review before editing",
        ));
    }
    if let ReviewAction::RunCheck { command } = &action {
        checks::start(gui, command)?;
        gui.review_context.as_mut().unwrap().revision += 1;
        return snapshot(gui).map(Some);
    }
    let invalidate_check = matches!(
        action,
        ReviewAction::Refresh
            | ReviewAction::Reload
            | ReviewAction::ApplySuggestion { .. }
            | ReviewAction::ApplyFinishSuggestions
    );
    // Set when the action itself establishes what the saved file holds.
    let mut baseline = None;
    if reload {
        ai::cancel(gui.app_for_workflow());
        baseline = Some(open_state(gui, &target)?);
    } else {
        let app = gui.app_for_workflow();
        if matches!(action, ReviewAction::PrecallConfirm) {
            ai::apply(app, &action)?;
        } else {
            // Only Pause gets here with a notice open (an unsaved review must
            // retry its save first): drop the unsent call before saving.
            if matches!(app.mode, AppMode::PromptPrecall(_)) {
                app.precall_cancel();
            }
            let AppMode::DiffViewer(state) = &mut app.mode else {
                unreachable!()
            };
            let path = match &action {
                ReviewAction::Select { path }
                | ReviewAction::Approve { path }
                | ReviewAction::Skip { path }
                | ReviewAction::Reject { path, .. }
                | ReviewAction::Comment { path, .. }
                | ReviewAction::ToggleResolved { path }
                | ReviewAction::LineComment { path, .. }
                | ReviewAction::Suggestion { path, .. }
                | ReviewAction::ApplySuggestion { path, .. }
                | ReviewAction::ToggleLineResolved { path, .. } => Some(path.clone()),
                ReviewAction::Walkthrough { path }
                | ReviewAction::CoReview { path }
                | ReviewAction::Ask { path, .. }
                | ReviewAction::AcceptDraft { path, .. }
                | ReviewAction::DismissDraft { path, .. } => Some(path.clone()),
                ReviewAction::Undo => state.verdict_undo.last().map(|entry| entry.path.clone()),
                _ => None,
            };
            if let Some(path) = path {
                let index = state
                    .files
                    .iter()
                    .position(|f| f.path == path)
                    .ok_or_else(|| {
                        GuiError::conflict("File is no longer in this review; refresh changes")
                    })?;
                // A verdict or comment must apply to the patch the reviewer saw.
                if !matches!(action, ReviewAction::Select { .. }) {
                    let fresh = crate::diff::load_snapshot(
                        &workdir,
                        state.override_base_ref.as_deref(),
                        state.ignore_whitespace,
                    )?;
                    if !fresh.files.iter().any(|f| {
                        f.path == path
                            && f.patch == state.files[index].patch
                            && f.status == state.files[index].status
                    }) {
                        return Err(GuiError::conflict(
                            "File changed since you opened it; refresh changes before reviewing it",
                        ));
                    }
                }
                state.selected_file = index;
            }
            // Resolve canonical source coordinates against the unchanged patch,
            // never treating a marker/header or an out-of-date anchor as a line.
            match &action {
                ReviewAction::LineComment { start, end, .. }
                | ReviewAction::Suggestion { start, end, .. }
                | ReviewAction::ApplySuggestion { start, end, .. }
                | ReviewAction::ToggleLineResolved { start, end, .. } => {
                    set_span(state, *start, *end)?;
                }
                ReviewAction::AcceptDraft { start, end, .. }
                | ReviewAction::DismissDraft { start, end, .. }
                | ReviewAction::Ask {
                    start: Some(start),
                    end: Some(end),
                    ..
                } => {
                    set_span(state, *start, *end)?;
                }
                _ => {}
            }
            // The shared actions save on their own, best-effort; this interface
            // saves once below instead, and reports whether that save succeeded.
            app.defer_review_progress_persist = true;
            let outcome = (|| -> GuiResult<()> {
                if ai::apply(app, &action)? {
                    return Ok(());
                }
                match action {
                    ReviewAction::ApplyFinishSuggestions => summary::apply(app)?,
                    ReviewAction::Approve { .. } => app.diff_review_approve_current(),
                    ReviewAction::Skip { .. } => app.diff_review_skip_current(),
                    ReviewAction::Reject {
                        feedback, severity, ..
                    } => {
                        app.diff_review_start_feedback();
                        if let AppMode::DiffViewer(s) = &mut app.mode {
                            s.reset_feedback_editor(feedback);
                            s.comment_severity = severity;
                        }
                        app.diff_review_submit_feedback();
                    }
                    ReviewAction::Comment { text, severity, .. } => {
                        app.diff_review_start_file_comment();
                        if let AppMode::DiffViewer(s) = &mut app.mode {
                            s.reset_feedback_editor(text);
                            s.comment_severity = severity;
                        }
                        app.diff_review_submit_file_comment();
                    }
                    ReviewAction::LineComment { text, severity, .. } => {
                        app.diff_review_start_line_comment();
                        if let AppMode::DiffViewer(s) = &mut app.mode {
                            s.reset_feedback_editor(text);
                            s.comment_severity = severity;
                        }
                        app.diff_review_submit_line_comment();
                    }
                    ReviewAction::Suggestion { text, .. } => {
                        app.diff_review_start_suggestion();
                        if let AppMode::DiffViewer(s) = &mut app.mode {
                            s.reset_feedback_editor(text);
                        }
                        app.diff_review_submit_suggestion();
                    }
                    ReviewAction::ApplySuggestion { path, start, end } => {
                        let AppMode::DiffViewer(s) = &app.mode else {
                            unreachable!()
                        };
                        // Match the whole thread, never another suggestion whose
                        // span happens to overlap the requested line cursor.
                        let comments = s.line_comments.get(&path).ok_or_else(|| {
                            GuiError::conflict("Kept suggestion is no longer at this anchor")
                        })?;
                        let mut matching = comments.iter().enumerate().filter(|(_, c)| {
                            c.start.unwrap_or(c.location) == start && c.location == end
                        });
                        let (index, comment) = matching.next().ok_or_else(|| {
                            GuiError::conflict("Kept suggestion is no longer at this anchor")
                        })?;
                        if matching.next().is_some() {
                            return Err(GuiError::conflict(
                                "Multiple threads share this anchor; edit the thread before applying",
                            ));
                        }
                        if let Some(reason) = crate::app::review::local_suggestion_blocker(
                            &s.files[s.selected_file],
                            comment,
                        ) {
                            return Err(GuiError::conflict(reason));
                        }
                        app.diff_review_apply_suggestion(&path, index)
                            .map_err(GuiError::conflict)?;
                    }
                    ReviewAction::ToggleLineResolved { start, end, .. } => {
                        if !app.diff_review_toggle_thread_resolved(start, end) {
                            return Err(GuiError::conflict(
                                "Kept thread is no longer at this anchor",
                            ));
                        }
                    }
                    ReviewAction::ToggleResolved { .. } => {
                        app.diff_review_toggle_file_comment_resolved();
                    }
                    ReviewAction::General { text } => {
                        app.diff_review_start_general_feedback();
                        if let AppMode::DiffViewer(s) = &mut app.mode {
                            s.reset_feedback_editor(text);
                        }
                        app.diff_review_submit_general_feedback();
                    }
                    ReviewAction::Undo => app.diff_review_undo_verdict(),
                    ReviewAction::Refresh => {
                        // A failed Git load must not replace an editable review with
                        // an empty file list and subsequently drop its saved threads.
                        let previous_files = if let AppMode::DiffViewer(s) = &app.mode {
                            crate::diff::load_snapshot(
                                &workdir,
                                s.override_base_ref.as_deref(),
                                s.ignore_whitespace,
                            )?;
                            s.files.clone()
                        } else {
                            unreachable!()
                        };
                        ai::cancel(app);
                        app.refresh_diff_viewer();
                        if let AppMode::DiffViewerLoading(s) = &mut app.mode {
                            s.generated_notes.clear();
                            s.changeset_overview = None;
                        }
                        app.complete_diff_viewer_loading();
                        if let AppMode::DiffViewer(s) = &mut app.mode {
                            s.forget_verdicts_for_changed_patches(&previous_files);
                        }
                    }
                    _ => {}
                }
                Ok(())
            })();
            app.defer_review_progress_persist = false;
            outcome?;
        }
    }
    if invalidate_check && gui.review_context.as_ref().unwrap().check.is_some() {
        checks::cancel(
            gui,
            ReviewCheckStatus::Stale,
            "Reviewed changes were refreshed or suggestions applied; run the check again".into(),
        );
    }
    if !preserve_draft {
        gui.review_context.as_mut().unwrap().ready_comment = None;
        if let Some(q) = gui.app_for_workflow().review_questions_mut() {
            q.draft = None;
        }
    }
    if reload {
        let context = gui.review_context.as_mut().unwrap();
        context.progress = baseline.unwrap();
        context.save_error = None;
    } else if !matches!(gui.app_for_workflow().mode, AppMode::PromptPrecall(_)) {
        save(gui);
    }
    let context = gui.review_context.as_mut().unwrap();
    context.revision += 1;
    if pause && context.save_error.is_none() {
        ai::cancel(gui.app_for_workflow());
        gui.app_for_workflow().mode = AppMode::Normal;
        gui.review_context = None;
        return Ok(None);
    }
    snapshot(gui).map(Some)
}

fn set_span(
    state: &mut DiffViewerState,
    start: DiffLineLocation,
    end: DiffLineLocation,
) -> GuiResult<()> {
    let locs = state.files[state.selected_file].addressable_lines();
    let locate = |location| {
        locs.iter().position(|l| *l == location).ok_or_else(|| {
            GuiError::conflict("Line anchor is no longer in this diff; refresh changes")
        })
    };
    let lo = locate(start)?;
    let hi = locate(end)?;
    if lo > hi {
        return Err(GuiError::conflict("Select the range in diff order"));
    }
    if state
        .line_comments
        .get(&state.files[state.selected_file].path)
        .is_some_and(|comments| {
            comments.iter().any(|c| {
                c.anchor_lost
                    && c.covered_indices(&locs)
                        .is_some_and(|range| *range.start() <= hi && *range.end() >= lo)
            })
        })
    {
        return Err(GuiError::conflict(
            "Thread anchor was lost; refresh changes before editing this span",
        ));
    }
    state.comment_anchor = Some(lo);
    state.comment_cursor = Some(hi);

    Ok(())
}

fn save(gui: &mut GuiHandle) {
    let outcome = gui.app_for_workflow().try_persist_review_progress();
    let context = gui.review_context.as_mut().unwrap();
    match outcome {
        Ok(written) => {
            if let Some(written) = written {
                context.progress = Some(written);
            }
            context.save_error = None;
        }
        Err(error) => context.save_error = Some(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_contract::GuiErrorKind;
    use crate::gui_diff::tests::{fixture, git};

    fn action(gui: &mut GuiHandle, view: &ReviewView, action: ReviewAction) -> ReviewView {
        act(gui, &view.workflow_id, view.revision, action)
            .unwrap()
            .unwrap()
    }

    fn span(view: &ReviewView, start: usize, end: usize) -> (DiffLineLocation, DiffLineLocation) {
        let find = |number| {
            let line = view.files[0]
                .diff
                .hunks
                .iter()
                .flat_map(|h| &h.lines)
                .find(|l| l.new_line == Some(number))
                .unwrap();
            DiffLineLocation {
                old_line: line.old_line,
                new_line: line.new_line,
            }
        };
        (find(start), find(end))
    }

    fn suggested(gui: &mut GuiHandle, view: &ReviewView, from: usize, to: usize) -> ReviewView {
        let (start, end) = span(view, from, to);
        action(
            gui,
            view,
            ReviewAction::Suggestion {
                path: "code.txt".into(),
                start,
                end,
                text: "replacement\nsecond\nthird".into(),
            },
        )
    }

    fn apply(start: DiffLineLocation, end: DiffLineLocation) -> ReviewAction {
        ReviewAction::ApplySuggestion {
            path: "code.txt".into(),
            start,
            end,
        }
    }

    #[test]
    fn summary_reads_all_verdicts_and_only_open_kept_threads_without_saving_or_discarding_drafts() {
        let (dir, mut gui, target) = fixture();
        let initial = begin(&mut gui, target).unwrap();
        let view = suggested(&mut gui, &initial, 8, 8);
        let view = action(
            &mut gui,
            &view,
            ReviewAction::General {
                text: "Overall feedback".into(),
            },
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Comment {
                path: "code.txt".into(),
                text: "File feedback".into(),
                severity: Severity::Question,
            },
        );
        let repo = dir.path().join("repo");
        let before = progress_bytes(&repo).unwrap();
        let source = std::fs::read(repo.join("code.txt")).unwrap();
        if let AppMode::DiffViewer(s) = &mut gui.app_for_workflow().mode {
            let comments = s.line_comments.get_mut("code.txt").unwrap();
            let mut draft = comments[0].clone();
            draft.draft = true;
            draft.text = "AI draft excluded".into();
            comments.push(draft);
            let mut resolved = comments[0].clone();
            resolved.resolved = true;
            resolved.text = "Resolved excluded".into();
            comments.push(resolved);
        }
        gui.review_context.as_mut().unwrap().save_error = Some("Retain save failure".into());
        gui.review_context.as_mut().unwrap().ready_comment = Some(ReviewCommentEditorView {
            request: 1,
            original: "".into(),
            text: "Retain draft".into(),
            path: None,
            start: None,
            end: None,
            severity: Severity::Nit,
        });
        let view = action(&mut gui, &view, ReviewAction::SummaryOpen);
        let summary = view.summary.as_ref().unwrap();
        assert_eq!(summary.rows.len(), 4);
        // The shared TUI engine marks an undecided file as needing work when
        // its first kept suggestion is saved; the summary preserves that.
        assert!(summary.rows[0].title.contains("needs work"));
        assert_eq!(summary.pending_suggestions, 1);
        assert_eq!(summary.undecided, 0);
        assert_eq!(
            summary.rows[1].suggestion.as_deref(),
            Some("replacement\nsecond\nthird")
        );
        assert_eq!(summary.rows[2].text, "File feedback");
        assert_eq!(summary.rows[3].text, "Overall feedback");
        assert_eq!(view.ai.ready_comment.as_ref().unwrap().text, "Retain draft");
        let view = action(&mut gui, &view, ReviewAction::SummaryClose);
        assert!(view.summary.is_none());
        assert!(view.save_error.is_some());
        assert!(view.ai.ready_comment.is_some());
        assert_eq!(progress_bytes(&repo).unwrap(), before);
        assert_eq!(std::fs::read(repo.join("code.txt")).unwrap(), source);
        assert!(!repo.join(".claude/final-review-feedback.md").exists());
    }

    #[test]
    fn finish_preparation_batches_bottom_up_consumes_opt_in_and_resumes_without_finishing() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target.clone()).unwrap();
        let view = suggested(&mut gui, &view, 8, 9);
        let (start, end) = span(&view, 10, 10);
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Suggestion {
                path: "code.txt".into(),
                start,
                end,
                text: "BOTTOM".into(),
            },
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        let repo = dir.path().join("repo");
        let source = std::fs::read_to_string(repo.join("code.txt")).unwrap();
        let view = action(&mut gui, &view, ReviewAction::SummaryOpen);
        assert_eq!(view.summary.as_ref().unwrap().pending_suggestions, 2);
        assert_eq!(
            std::fs::read_to_string(repo.join("code.txt")).unwrap(),
            source
        );
        let applied = action(&mut gui, &view, ReviewAction::ApplyFinishSuggestions);
        assert_eq!(
            std::fs::read_to_string(repo.join("code.txt")).unwrap(),
            source.replace(
                "committed 🦀\nline 9\nline 10\n",
                "replacement\nsecond\nthird\nBOTTOM\n"
            )
        );
        assert_eq!(applied.applied_suggestions.len(), 2);
        assert_eq!(applied.summary.as_ref().unwrap().pending_suggestions, 0);
        assert!(applied.summary.as_ref().unwrap().failures.is_empty());
        assert_eq!(applied.files[0].verdict, "undecided");
        let s = ai::state(&gui.app_for_workflow().mode).unwrap();
        assert!(!s.apply_suggestions_on_finish);
        assert!(s.verdict_undo.is_empty());
        assert!(s.finish_check_child.is_none());
        assert!(!repo.join(".claude/final-review-feedback.md").exists());
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::ApplyFinishSuggestions
            )
            .unwrap_err()
            .message
            .contains("Review changed")
        );
        assert!(
            act(
                &mut gui,
                &applied.workflow_id,
                applied.revision,
                ReviewAction::ApplyFinishSuggestions
            )
            .unwrap_err()
            .message
            .contains("No open suggestions")
        );
        act(
            &mut gui,
            &applied.workflow_id,
            applied.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        let resumed = begin(&mut gui, target).unwrap();
        assert_eq!(resumed.applied_suggestions, applied.applied_suggestions);
        assert!(resumed.files[0].line_comments.is_empty());
    }

    #[test]
    fn finish_preparation_reports_blocked_jobs_and_keeps_their_replacements() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let view = suggested(&mut gui, &view, 8, 8);
        let view = suggested(&mut gui, &view, 10, 10);
        if let AppMode::DiffViewer(s) = &mut gui.app_for_workflow().mode {
            s.line_comments.get_mut("code.txt").unwrap()[1].anchor_lost = true;
        }
        let view = action(&mut gui, &view, ReviewAction::RetrySave);
        let view = action(&mut gui, &view, ReviewAction::SummaryOpen);
        let applied = action(&mut gui, &view, ReviewAction::ApplyFinishSuggestions);
        let summary = applied.summary.unwrap();
        assert_eq!(summary.pending_suggestions, 1);
        assert_eq!(summary.failures.len(), 1);
        assert!(summary.failures[0].contains("anchor"));
        assert_eq!(applied.applied_suggestions.len(), 1);
        assert!(
            applied.files[0]
                .line_comments
                .iter()
                .any(|c| c.suggestion.is_some())
        );
        let text = std::fs::read_to_string(dir.path().join("repo/code.txt")).unwrap();
        assert!(text.contains("replacement\nsecond\nthird\nline 9\nline 10\n"));
        assert!(
            !ai::state(&gui.app_for_workflow().mode)
                .unwrap()
                .apply_suggestions_on_finish
        );
    }

    /// Prepare a batch where the line-10 suggestion is blocked by a lost
    /// anchor and the line-8 one applies.
    fn blocked_batch(gui: &mut GuiHandle, target: FeatureTarget) -> ReviewView {
        let view = begin(gui, target).unwrap();
        let view = suggested(gui, &view, 8, 8);
        let view = suggested(gui, &view, 10, 10);
        if let AppMode::DiffViewer(s) = &mut gui.app_for_workflow().mode {
            let comment = &mut s.line_comments.get_mut("code.txt").unwrap()[1];
            comment.anchor_lost = true;
            comment.text = "blocked one".into();
        }
        let view = action(gui, &view, ReviewAction::RetrySave);
        let view = action(gui, &view, ReviewAction::SummaryOpen);
        let applied = action(gui, &view, ReviewAction::ApplyFinishSuggestions);
        assert_eq!(applied.summary.as_ref().unwrap().failures.len(), 1);
        applied
    }

    #[test]
    fn finish_preparation_failures_survive_a_pause_into_the_finished_round() {
        let (dir, mut gui, target) = fixture();
        let applied = blocked_batch(&mut gui, target.clone());
        let reason = applied.summary.as_ref().unwrap().failures[0].clone();
        act(
            &mut gui,
            &applied.workflow_id,
            applied.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        let repo = dir.path().join("repo");
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(repo.join(".claude/final-review-progress.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            saved["suggestion_apply_failures"].as_array().unwrap().len(),
            1
        );

        // Resume and finish through the shared (TUI) finish path.
        let resumed = begin(&mut gui, target).unwrap();
        assert_eq!(
            ai::state(&gui.app_for_workflow().mode)
                .unwrap()
                .open_suggestion_apply_failures()
                .len(),
            1
        );
        act(
            &mut gui,
            &resumed.workflow_id,
            resumed.revision,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        )
        .unwrap();
        gui.app_for_workflow().finish_final_review().unwrap();
        let feedback =
            std::fs::read_to_string(repo.join(".claude/final-review-feedback.md")).unwrap();
        let reason = reason.split_once(": ").unwrap().1;
        assert!(
            feedback.contains("1 not applied") && feedback.contains(reason),
            "{feedback}"
        );
    }

    #[test]
    fn finish_preparation_failure_lapses_once_its_suggestion_is_settled() {
        let (_dir, mut gui, target) = fixture();
        let applied = blocked_batch(&mut gui, target);
        let comment = applied.files[0]
            .line_comments
            .iter()
            .find(|c| c.suggestion.is_some())
            .unwrap();
        let resolved = action(
            &mut gui,
            &applied,
            ReviewAction::ToggleLineResolved {
                path: "code.txt".into(),
                start: comment.start,
                end: comment.end,
            },
        );
        let summary = resolved.summary.as_ref().unwrap();
        assert_eq!(summary.pending_suggestions, 0);
        assert!(summary.failures.is_empty(), "{:?}", summary.failures);
        // Not saved either, so a resumed review cannot bring it back.
        assert!(
            ai::state(&gui.app_for_workflow().mode)
                .unwrap()
                .live_suggestion_apply_failures()
                .is_empty()
        );
    }

    #[test]
    fn finish_preparation_refuses_changed_patches_progress_checkouts_and_deleted_targets_before_writing()
     {
        for change in ["patch", "progress", "checkout", "deleted"] {
            let (dir, mut gui, target) = fixture();
            let view = begin(&mut gui, target).unwrap();
            let view = suggested(&mut gui, &view, 8, 9);
            let view = action(&mut gui, &view, ReviewAction::SummaryOpen);
            let repo = dir.path().join("repo");
            match change {
                "patch" => std::fs::write(repo.join("code.txt"), "External edit\n").unwrap(),
                "progress" => {
                    std::fs::write(crate::app::review::review_progress_path(&repo), "{}").unwrap()
                }
                _ => {
                    let db = gui.app_for_workflow().db.as_ref().unwrap();
                    let mut store = db.load_store().unwrap();
                    if change == "deleted" {
                        store.projects[0].features.clear();
                    } else {
                        store.projects[0].features[0].workdir = dir.path().join("other");
                    }
                    db.save_store(&store).unwrap();
                }
            }
            let before = std::fs::read(repo.join("code.txt")).unwrap();
            assert!(
                act(
                    &mut gui,
                    &view.workflow_id,
                    view.revision,
                    ReviewAction::ApplyFinishSuggestions
                )
                .is_err(),
                "{change}"
            );
            assert_eq!(
                std::fs::read(repo.join("code.txt")).unwrap(),
                before,
                "{change}"
            );
            assert!(snapshot(&mut gui).unwrap().applied_suggestions.is_empty());
            let closed = action(&mut gui, &view, ReviewAction::SummaryClose);
            assert!(closed.summary.is_none());
        }
    }

    #[test]
    fn finish_preparation_requires_summary_and_refuses_running_ai_or_transferred_drafts() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let view = suggested(&mut gui, &view, 8, 9);
        let source = dir.path().join("repo/code.txt");
        let before = std::fs::read(&source).unwrap();
        let err = act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            ReviewAction::ApplyFinishSuggestions,
        )
        .unwrap_err();
        assert!(err.message.contains("Open the pre-finish summary"));
        let view = action(&mut gui, &view, ReviewAction::SummaryOpen);
        let (_sender, receiver) = std::sync::mpsc::channel();
        if let AppMode::DiffViewer(s) = &mut gui.app_for_workflow().mode {
            s.co_review_bg = Some(receiver);
        }
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::ApplyFinishSuggestions
            )
            .unwrap_err()
            .message
            .contains("running AI request")
        );
        ai::cancel(gui.app_for_workflow());
        gui.review_context.as_mut().unwrap().ready_comment = Some(ReviewCommentEditorView {
            request: 1,
            original: "".into(),
            text: "Draft".into(),
            path: None,
            start: None,
            end: None,
            severity: Severity::Nit,
        });
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::ApplyFinishSuggestions
            )
            .unwrap_err()
            .message
            .contains("transferred comment draft")
        );
        assert!(snapshot(&mut gui).unwrap().ai.ready_comment.is_some());
        assert_eq!(std::fs::read(source).unwrap(), before);
    }

    #[test]
    fn finish_preparation_retains_applied_source_after_save_failure_and_requires_retry() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let view = suggested(&mut gui, &view, 8, 9);
        let view = action(&mut gui, &view, ReviewAction::SummaryOpen);
        let claude = dir.path().join("repo/.claude");
        let permissions = std::fs::metadata(&claude).unwrap().permissions();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            ReviewAction::ApplyFinishSuggestions,
        );
        std::fs::set_permissions(&claude, permissions).unwrap();
        let applied = result.unwrap().unwrap();
        assert!(applied.save_error.is_some());
        assert!(applied.summary.is_some());
        assert_eq!(applied.applied_suggestions.len(), 1);
        assert!(
            std::fs::read_to_string(dir.path().join("repo/code.txt"))
                .unwrap()
                .contains("replacement\nsecond\nthird\n")
        );
        assert!(
            act(
                &mut gui,
                &applied.workflow_id,
                applied.revision,
                ReviewAction::ApplyFinishSuggestions
            )
            .unwrap_err()
            .message
            .contains("Save review progress successfully")
        );
        let saved = action(&mut gui, &applied, ReviewAction::RetrySave);
        assert!(saved.save_error.is_none());
        assert!(saved.summary.is_some());
    }

    #[test]
    fn history_loads_live_rounds_then_archive_newest_first_without_writes() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let claude = repo.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let live = "# Final Review Feedback\n\n## Review — newest\n\n(unresolved from a previous round)\n```suggestion\nreplacement\n```\n**Agent:** fixed\n\n## Review — previous\n\n**Check:** passed\n";
        let archive = "# Final Review Feedback Archive\n\n## Review — oldest\n\nold\n\n## Review — middle\n\nnewer\n";
        std::fs::write(claude.join("final-review-feedback.md"), live).unwrap();
        std::fs::write(claude.join("final-review-feedback-archive.md"), archive).unwrap();
        let source = std::fs::read(repo.join("code.txt")).unwrap();
        let progress = progress_bytes(&repo).unwrap();
        let view = begin(&mut gui, target).unwrap();
        let view = action(&mut gui, &view, ReviewAction::HistoryOpen);
        let history = view.history.as_ref().unwrap();
        assert_eq!(history.selected, 0);
        assert_eq!(history.rounds.len(), 2);
        assert_eq!(history.rounds[0].title, "Review — newest");
        assert_eq!(history.rounds[0].carried_unresolved, 1);
        assert!(history.archive_available);
        assert!(!history.archive_loaded);
        let view = action(&mut gui, &view, ReviewAction::HistorySelect { round: 1 });
        let markdown = &view.history.as_ref().unwrap().markdown;
        assert!(markdown.contains("```suggestion\nreplacement"));
        assert!(markdown.contains("**Agent:** fixed"));
        let view = action(&mut gui, &view, ReviewAction::HistoryLoadOlder);
        let history = view.history.as_ref().unwrap();
        assert!(history.archive_loaded);
        assert_eq!(history.selected, 1);
        assert_eq!(history.rounds[2].title, "Review — middle");
        assert_eq!(history.rounds[3].title, "Review — oldest");
        let view = action(&mut gui, &view, ReviewAction::HistoryLoadOlder);
        assert_eq!(view.history.as_ref().unwrap().rounds.len(), 4);
        let view = action(&mut gui, &view, ReviewAction::HistorySelect { round: 4 });
        assert!(view.history.as_ref().unwrap().markdown.contains("old\n"));
        let view = action(&mut gui, &view, ReviewAction::HistoryClose);
        assert!(view.history.is_none());
        assert_eq!(progress_bytes(&repo).unwrap(), progress);
        assert_eq!(std::fs::read(repo.join("code.txt")).unwrap(), source);
        assert_eq!(
            std::fs::read_to_string(claude.join("final-review-feedback.md")).unwrap(),
            live
        );
        assert_eq!(
            std::fs::read_to_string(claude.join("final-review-feedback-archive.md")).unwrap(),
            archive
        );
    }

    #[test]
    fn current_history_projects_verdicts_resolved_and_draft_threads_and_agent_replies() {
        use crate::app::review::state::{AgentResponse, LineComment};
        let (_dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let (start, end) = span(&view, 8, 9);
        let state = match &mut gui.app_for_workflow().mode {
            AppMode::DiffViewer(state) => state,
            _ => unreachable!(),
        };
        state.decisions.insert(
            "code.txt".into(),
            ReviewDecision::Reject {
                feedback: "Must fix".into(),
                severity: Severity::Blocker,
            },
        );
        state.general_feedback = "Overall feedback".into();
        state.line_comments.insert(
            "code.txt".into(),
            vec![
                LineComment {
                    start: Some(start),
                    location: end,
                    text: "Resolved thread".into(),
                    suggestion: Some("replacement".into()),
                    resolved: true,
                    draft: false,
                    severity: Severity::Nit,
                    anchor_context: None,
                    start_anchor_context: None,
                    anchor_lost: false,
                    carried: false,
                },
                LineComment {
                    start: None,
                    location: start,
                    text: "Unaccepted finding".into(),
                    suggestion: None,
                    resolved: false,
                    draft: true,
                    severity: Severity::Question,
                    anchor_context: None,
                    start_anchor_context: None,
                    anchor_lost: false,
                    carried: false,
                },
            ],
        );
        state.prior_agent_responses.insert(
            "code.txt".into(),
            vec![AgentResponse {
                anchor: "code.txt:8".into(),
                response: "Agent explanation".into(),
            }],
        );
        let view = action(&mut gui, &view, ReviewAction::HistoryOpen);
        let history = view.history.unwrap();
        assert!(history.rounds.is_empty());
        assert_eq!(history.current_unresolved, 0);
        for text in [
            "Overall feedback",
            "Must fix",
            "needs work [blocker]",
            "L8-9",
            "nit · resolved",
            "replacement",
            "AI draft",
            "Unaccepted finding",
            "Agent explanation",
        ] {
            assert!(history.markdown.contains(text), "Missing {text}");
        }
    }

    #[test]
    fn history_archive_read_errors_can_be_retried_without_reopening() {
        let (dir, mut gui, target) = fixture();
        let claude = dir.path().join("repo/.claude");
        std::fs::create_dir_all(&claude).unwrap();
        let archive = claude.join("final-review-feedback-archive.md");
        std::fs::write(&archive, [0xff]).unwrap();
        let view = begin(&mut gui, target).unwrap();
        let view = action(&mut gui, &view, ReviewAction::HistoryOpen);
        assert!(view.history.as_ref().unwrap().error.is_none());
        let view = action(&mut gui, &view, ReviewAction::HistoryLoadOlder);
        let history = view.history.as_ref().unwrap();
        // A failed read keeps "Load older rounds" offered and says so.
        assert!(!history.archive_loaded);
        let error = history.error.as_ref().unwrap();
        assert!(error.contains("archived review history"));
        assert!(error.contains("Load older rounds again to retry"));
        std::fs::write(&archive, "## Review — repaired\n\nbody\n").unwrap();
        let view = action(&mut gui, &view, ReviewAction::HistoryLoadOlder);
        let history = view.history.as_ref().unwrap();
        assert!(history.archive_loaded);
        assert!(history.error.is_none());
        assert_eq!(history.rounds[0].title, "Review — repaired");
    }

    #[test]
    fn history_preserves_unsaved_progress_and_transferred_drafts_after_external_save() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let view = begin(&mut gui, target).unwrap();
        let context = gui.review_context.as_mut().unwrap();
        context.save_error = Some("Retry this save".into());
        context.ready_comment = Some(ReviewCommentEditorView {
            request: 4,
            original: String::new(),
            path: None,
            start: None,
            end: None,
            text: "Transferred draft".into(),
            severity: Severity::Nit,
        });
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        let external = b"{\"general_feedback\":\"external\"}";
        std::fs::write(crate::app::review::review_progress_path(&repo), external).unwrap();
        let view = action(&mut gui, &view, ReviewAction::HistoryOpen);
        let view = action(&mut gui, &view, ReviewAction::HistoryClose);
        assert_eq!(view.save_error.as_deref(), Some("Retry this save"));
        assert_eq!(view.ai.ready_comment.unwrap().text, "Transferred draft");
        assert_eq!(progress_bytes(&repo).unwrap().unwrap(), external);
        assert!(gui.review_context.as_ref().unwrap().progress.is_none());
    }

    #[test]
    fn history_rejects_stale_unloaded_and_deleted_targets() {
        let (dir, mut gui, target) = fixture();
        let original = begin(&mut gui, target).unwrap();
        assert!(
            act(
                &mut gui,
                &original.workflow_id,
                original.revision,
                ReviewAction::HistorySelect { round: 0 }
            )
            .is_err()
        );
        let view = action(&mut gui, &original, ReviewAction::HistoryOpen);
        assert!(
            act(
                &mut gui,
                &original.workflow_id,
                original.revision,
                ReviewAction::HistoryClose
            )
            .is_err()
        );
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::HistorySelect { round: usize::MAX }
            )
            .is_err()
        );
        let db = crate::db::AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = db.load_store().unwrap();
        store.projects[0].features.clear();
        db.save_store(&store).unwrap();
        assert_eq!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::HistoryLoadOlder
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::NotFound
        );
        let view = action(&mut gui, &view, ReviewAction::HistoryClose);
        assert!(view.history.is_none());
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::Pause
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn tui_history_navigation_loads_archive_only_after_the_live_tail() {
        let (dir, mut gui, target) = fixture();
        let claude = dir.path().join("repo/.claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("final-review-feedback.md"),
            "## Review — live\n\nbody\n",
        )
        .unwrap();
        std::fs::write(
            claude.join("final-review-feedback-archive.md"),
            "## Review — archived\n\nbody\n",
        )
        .unwrap();
        begin(&mut gui, target).unwrap();
        let app = gui.app_for_workflow();
        app.open_review_history();
        app.review_history_move(1);
        let history = ai::state(&app.mode)
            .unwrap()
            .review_history
            .as_ref()
            .unwrap();
        assert_eq!(history.selected, 1);
        assert!(!history.archive_loaded);
        app.review_history_move(1);
        let history = ai::state(&app.mode)
            .unwrap()
            .review_history
            .as_ref()
            .unwrap();
        assert_eq!(history.selected, 2);
        assert!(history.archive_loaded);
        assert_eq!(history.rounds[1].title, "Review — archived");
        app.review_history_move(-2);
        assert_eq!(
            ai::state(&app.mode)
                .unwrap()
                .review_history
                .as_ref()
                .unwrap()
                .selected,
            0
        );
        app.close_review_history();
        assert!(ai::state(&app.mode).unwrap().review_history.is_none());
    }

    #[test]
    fn failed_archive_read_keeps_live_error_and_can_be_retried() {
        let (dir, mut gui, target) = fixture();
        let claude = dir.path().join("repo/.claude");
        std::fs::create_dir_all(&claude).unwrap();
        // A directory makes the live log unreadable; invalid UTF-8 makes the
        // archive unreadable while it still counts as available.
        std::fs::create_dir_all(claude.join("final-review-feedback.md")).unwrap();
        let archive = claude.join("final-review-feedback-archive.md");
        std::fs::write(&archive, [0xff, 0xfe, 0xfd]).unwrap();
        begin(&mut gui, target).unwrap();
        let app = gui.app_for_workflow();
        app.open_review_history();
        app.load_review_history_archive();
        let history = ai::state(&app.mode)
            .unwrap()
            .review_history
            .as_ref()
            .unwrap();
        assert!(!history.archive_loaded);
        let message = history.error_message().unwrap();
        assert!(message.contains("Could not read review history"));
        assert!(message.contains("Could not read archived review history"));
        assert!(message.contains("Load older rounds again to retry"));

        std::fs::write(&archive, "## Review — archived\n\nbody\n").unwrap();
        app.load_review_history_archive();
        let history = ai::state(&app.mode)
            .unwrap()
            .review_history
            .as_ref()
            .unwrap();
        assert!(history.archive_loaded);
        assert_eq!(history.rounds[0].title, "Review — archived");
        assert!(history.archive_error.is_none());
        assert!(history.error.is_some());
    }

    #[test]
    fn local_application_writes_the_range_settles_prose_and_reanchors_other_threads() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target.clone()).unwrap();
        let (start, end) = span(&view, 8, 9);
        let (other, _) = span(&view, 10, 10);
        let view = action(
            &mut gui,
            &view,
            ReviewAction::LineComment {
                path: "code.txt".into(),
                start: other,
                end: other,
                text: "Keep this neighbour".into(),
                severity: Severity::Nit,
            },
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::LineComment {
                path: "code.txt".into(),
                start,
                end,
                text: "Explain the fix".into(),
                severity: Severity::Nit,
            },
        );
        let view = suggested(&mut gui, &view, 8, 9);
        let source = dir.path().join("repo/code.txt");
        let before = std::fs::read_to_string(&source).unwrap();
        assert!(
            view.files[0]
                .line_comments
                .iter()
                .find(|c| c.suggestion.is_some())
                .unwrap()
                .apply_blocked
                .is_none()
        );
        let applied = action(&mut gui, &view, apply(start, end));
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            before.replace("committed 🦀\nline 9\n", "replacement\nsecond\nthird\n")
        );
        assert!(applied.files[0].diff.patch.contains("+replacement"));
        let prose = applied.files[0]
            .line_comments
            .iter()
            .find(|c| c.text == "Explain the fix")
            .unwrap();
        assert!(prose.resolved && prose.suggestion.is_none());
        let neighbour = applied.files[0]
            .line_comments
            .iter()
            .find(|c| c.text == "Keep this neighbour")
            .unwrap();
        assert_eq!(neighbour.end.new_line, Some(11));
        assert!(!neighbour.anchor_lost);
        assert_eq!(applied.applied_suggestions.len(), 1);
        assert!(applied.save_error.is_none());
        action(&mut gui, &applied, ReviewAction::Undo);
        let latest = snapshot(&mut gui).unwrap();
        act(
            &mut gui,
            &latest.workflow_id,
            latest.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        let resumed = begin(&mut gui, target).unwrap();
        assert_eq!(resumed.applied_suggestions, applied.applied_suggestions);
        assert!(
            resumed.files[0]
                .line_comments
                .iter()
                .all(|c| c.suggestion.is_none())
        );
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(crate::app::review::review_progress_path(
                &dir.path().join("repo"),
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(saved["applied_suggestions"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn applying_invalidates_approvals_and_undo_and_refuses_a_repeated_submission() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let (start, end) = span(&view, 8, 8);
        let view = suggested(&mut gui, &view, 8, 8);
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        assert_eq!(view.files[0].verdict, "approved");
        let applied = action(&mut gui, &view, apply(start, end));
        assert_eq!(applied.files[0].verdict, "undecided");
        assert!(applied.files[0].line_comments.is_empty());
        let source = std::fs::read_to_string(dir.path().join("repo/code.txt")).unwrap();
        // Resubmit against the current revision, so the refusal has to come
        // from the consumed thread rather than the stale-revision guard.
        let repeated = act(
            &mut gui,
            &applied.workflow_id,
            applied.revision,
            apply(start, end),
        )
        .unwrap_err();
        assert_eq!(repeated.kind, GuiErrorKind::Conflict);
        assert_eq!(
            repeated.message,
            "Kept suggestion is no longer at this anchor"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("repo/code.txt")).unwrap(),
            source
        );
        let undone = action(&mut gui, &applied, ReviewAction::Undo);
        assert_eq!(undone.files[0].verdict, "undecided");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("repo/code.txt")).unwrap(),
            source
        );
    }

    #[test]
    fn applying_also_invalidates_approvals_of_files_changed_outside_amf() {
        let (dir, mut gui, target) = fixture();
        std::fs::write(dir.path().join("repo/old.txt"), "other change\n").unwrap();
        let initial = begin(&mut gui, target).unwrap();
        let (start, end) = span(&initial, 8, 8);
        let view = suggested(&mut gui, &initial, 8, 8);
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "old.txt".into(),
            },
        );
        // Another file moves after it was approved; applying a suggestion to
        // `code.txt` reloads its new patch, so the old approval must go too.
        std::fs::write(dir.path().join("repo/old.txt"), "edited elsewhere\n").unwrap();
        let applied = action(&mut gui, &view, apply(start, end));
        let old = applied
            .files
            .iter()
            .find(|f| f.diff.path == "old.txt")
            .unwrap();
        assert_eq!(old.verdict, "undecided");
        assert!(old.diff.patch.contains("+edited elsewhere"));
        let AppMode::DiffViewer(state) = &gui.app_for_workflow().mode else {
            panic!("review should remain open")
        };
        assert!(state.verdict_undo.is_empty());
    }

    #[test]
    fn tui_cursor_application_keeps_unrelated_approvals_and_invalidates_only_changed_undo() {
        let (dir, mut gui, target) = fixture();
        std::fs::write(dir.path().join("repo/old.txt"), "other change\n").unwrap();
        let initial = begin(&mut gui, target).unwrap();
        let view = suggested(&mut gui, &initial, 8, 8);
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "old.txt".into(),
            },
        );
        action(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        let app = gui.app_for_workflow();
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.selected_file = state
                .files
                .iter()
                .position(|f| f.path == "code.txt")
                .unwrap();
            state.comment_cursor = state.files[state.selected_file]
                .addressable_lines()
                .iter()
                .position(|l| l.new_line == Some(8));
        }
        app.diff_review_apply_suggestion_under_cursor();
        assert!(
            app.message
                .as_deref()
                .unwrap()
                .starts_with("Applied suggestion locally:")
        );
        let AppMode::DiffViewer(state) = &app.mode else {
            panic!("review should remain open")
        };
        assert_eq!(
            state.decisions.get("old.txt"),
            Some(&ReviewDecision::Approve)
        );
        assert!(!state.decisions.contains_key("code.txt"));
        assert!(
            state
                .verdict_undo
                .iter()
                .all(|entry| entry.path == "old.txt")
        );
        assert_eq!(state.applied_suggestions.len(), 1);
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(crate::app::review::review_progress_path(
                &dir.path().join("repo"),
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(saved["applied_suggestions"].as_array().unwrap().len(), 1);
        assert!(saved["line_comments"]["code.txt"].is_null());
    }

    #[test]
    fn application_requires_the_exact_whole_thread_and_a_kept_applicable_replacement() {
        for case in [
            "partial",
            "resolved",
            "draft",
            "lost",
            "mixed",
            "no replacement",
        ] {
            let (dir, mut gui, target) = fixture();
            let initial = begin(&mut gui, target).unwrap();
            let view = suggested(&mut gui, &initial, 8, 9);
            let (mut start, mut end) = span(&view, 8, 9);
            if let AppMode::DiffViewer(state) = &mut gui.app_for_workflow().mode {
                let comment = &mut state.line_comments.get_mut("code.txt").unwrap()[0];
                match case {
                    "partial" => start = end,
                    "resolved" => comment.resolved = true,
                    "draft" => comment.draft = true,
                    "lost" => comment.anchor_lost = true,
                    "mixed" => {
                        start = DiffLineLocation {
                            old_line: Some(8),
                            new_line: None,
                        };
                        comment.start = Some(start);
                    }
                    "no replacement" => comment.suggestion = None,
                    _ => unreachable!(),
                }
                end = comment.location;
            }
            let source = dir.path().join("repo/code.txt");
            let before = std::fs::read_to_string(&source).unwrap();
            let err = act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                apply(start, end),
            )
            .unwrap_err();
            assert_eq!(err.kind, GuiErrorKind::Conflict, "{case}: {err:?}");
            assert_eq!(std::fs::read_to_string(&source).unwrap(), before, "{case}");
            assert!(snapshot(&mut gui).unwrap().applied_suggestions.is_empty());
            assert!(!gui.app_for_workflow().defer_review_progress_persist);
        }
    }

    #[test]
    fn application_refuses_changed_source_or_external_progress_without_consuming_the_suggestion() {
        for external_progress in [false, true] {
            let (dir, mut gui, target) = fixture();
            let initial = begin(&mut gui, target).unwrap();
            let view = suggested(&mut gui, &initial, 8, 9);
            let (start, end) = span(&view, 8, 9);
            let source = dir.path().join("repo/code.txt");
            if external_progress {
                std::fs::write(
                    crate::app::review::review_progress_path(&dir.path().join("repo")),
                    "{}",
                )
                .unwrap();
            } else {
                std::fs::write(&source, "edited elsewhere\n").unwrap();
            }
            let before = std::fs::read_to_string(&source).unwrap();
            let err = act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                apply(start, end),
            )
            .unwrap_err();
            assert_eq!(err.kind, GuiErrorKind::Conflict);
            assert!(err.message.contains(if external_progress {
                "Saved review changed"
            } else {
                "File changed"
            }));
            assert_eq!(std::fs::read_to_string(&source).unwrap(), before);
            assert!(
                snapshot(&mut gui).unwrap().files[0].line_comments[0]
                    .suggestion
                    .is_some()
            );
        }
    }

    #[test]
    fn application_refuses_a_feature_checkout_reassigned_by_another_process() {
        let (dir, mut gui, target) = fixture();
        let initial = begin(&mut gui, target).unwrap();
        let view = suggested(&mut gui, &initial, 8, 9);
        let (start, end) = span(&view, 8, 9);
        let source = dir.path().join("repo/code.txt");
        let before = std::fs::read_to_string(&source).unwrap();
        let other = dir.path().join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("code.txt"), "other checkout\n").unwrap();
        let db = crate::db::AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = db.load_store().unwrap();
        store.projects[0].features[0].workdir = other.clone();
        db.save_store(&store).unwrap();
        let err = act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            apply(start, end),
        )
        .unwrap_err();
        assert!(err.message.contains("Feature checkout changed"), "{err:?}");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(other.join("code.txt")).unwrap(),
            "other checkout\n"
        );
        assert!(!gui.app_for_workflow().defer_review_progress_persist);
    }

    #[test]
    fn an_application_write_failure_keeps_the_suggestion_and_source_intact() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut gui, target) = fixture();
        let initial = begin(&mut gui, target).unwrap();
        let view = suggested(&mut gui, &initial, 8, 9);
        let (start, end) = span(&view, 8, 9);
        let source = dir.path().join("repo/code.txt");
        let before = std::fs::read_to_string(&source).unwrap();
        let permissions = std::fs::metadata(&source).unwrap().permissions();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o444)).unwrap();
        let outcome = act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            apply(start, end),
        );
        std::fs::set_permissions(&source, permissions).unwrap();
        let err = outcome.unwrap_err();
        assert!(err.message.contains("could not write file"), "{err:?}");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), before);
        assert!(
            snapshot(&mut gui).unwrap().files[0].line_comments[0]
                .suggestion
                .is_some()
        );
    }

    #[test]
    fn a_failed_progress_save_after_application_retains_the_write_and_offers_retry() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut gui, target) = fixture();
        let initial = begin(&mut gui, target.clone()).unwrap();
        let view = suggested(&mut gui, &initial, 8, 9);
        let (start, end) = span(&view, 8, 9);
        let claude = dir.path().join("repo/.claude");
        let permissions = std::fs::metadata(&claude).unwrap().permissions();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o555)).unwrap();
        let outcome = act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            apply(start, end),
        );
        std::fs::set_permissions(&claude, permissions).unwrap();
        let applied = outcome.unwrap().unwrap();
        assert!(applied.save_error.is_some());
        assert_eq!(applied.applied_suggestions.len(), 1);
        assert!(applied.files[0].line_comments.is_empty());
        assert!(
            std::fs::read_to_string(dir.path().join("repo/code.txt"))
                .unwrap()
                .contains("replacement\nsecond\nthird\n")
        );
        let err = act(
            &mut gui,
            &applied.workflow_id,
            applied.revision,
            apply(start, end),
        )
        .unwrap_err();
        assert!(err.message.contains("Save review progress successfully"));
        let saved = action(&mut gui, &applied, ReviewAction::RetrySave);
        assert!(saved.save_error.is_none());
        act(
            &mut gui,
            &saved.workflow_id,
            saved.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        assert_eq!(
            begin(&mut gui, target).unwrap().applied_suggestions,
            applied.applied_suggestions
        );
    }

    #[test]
    fn range_comments_suggestions_and_resolution_resume_in_the_tui_progress_format() {
        let (dir, mut gui, target) = fixture();
        let initial = begin(&mut gui, target.clone()).unwrap();
        let (start, end) = span(&initial, 8, 9);
        let view = action(
            &mut gui,
            &initial,
            ReviewAction::LineComment {
                path: "code.txt".into(),
                start,
                end,
                text: "Range question".into(),
                severity: Severity::Question,
            },
        );
        assert_eq!(view.files[0].verdict, "rejected");
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Suggestion {
                path: "code.txt".into(),
                start,
                end,
                text: "  replacement 🦀\n    next\n".into(),
            },
        );
        let comment = &view.files[0].line_comments[0];
        assert_eq!(comment.start, start);
        assert_eq!(comment.end, end);
        assert_eq!(comment.anchor, "line 8 – line 9");
        assert!(comment.editable);
        assert_eq!(comment.text, "Range question");
        assert_eq!(comment.severity, Severity::Question);
        assert_eq!(
            comment.suggestion.as_deref(),
            Some("  replacement 🦀\n    next")
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::ToggleLineResolved {
                path: "code.txt".into(),
                start,
                end,
            },
        );
        assert!(view.files[0].line_comments[0].resolved);
        assert_eq!(view.files[0].verdict, "undecided");
        act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        let resumed = begin(&mut gui, target).unwrap();
        assert!(resumed.files[0].line_comments[0].resolved);
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(crate::app::review::review_progress_path(
                &dir.path().join("repo"),
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            saved["line_comments"]["code.txt"][0]["start"]["new_line"],
            8
        );
        assert_eq!(
            saved["line_comments"]["code.txt"][0]["location"]["new_line"],
            9
        );
        assert!(saved["line_comments"]["code.txt"][0]["anchor_context"].is_object());
        let reopened = action(
            &mut gui,
            &resumed,
            ReviewAction::ToggleLineResolved {
                path: "code.txt".into(),
                start,
                end,
            },
        );
        assert!(!reopened.files[0].line_comments[0].resolved);
        assert_eq!(reopened.files[0].verdict, "rejected");
    }

    #[test]
    fn toggling_a_thread_flips_the_exact_anchor_not_an_overlapping_one() {
        let (_dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let (outer_start, end) = span(&view, 7, 9);
        action(
            &mut gui,
            &view,
            ReviewAction::LineComment {
                path: "code.txt".into(),
                start: end,
                end,
                text: "Inner".into(),
                severity: Severity::Nit,
            },
        );
        // Loaded or carried progress can hold kept threads that overlap; the
        // outer one sorts first and covers the inner thread's line.
        let AppMode::DiffViewer(state) = &mut gui.app_for_workflow().mode else {
            panic!()
        };
        let comments = state.line_comments.get_mut("code.txt").unwrap();
        let mut outer = comments[0].clone();
        outer.start = Some(outer_start);
        outer.text = "Outer".into();
        comments.insert(0, outer);
        let view = snapshot(&mut gui).unwrap();
        let view = action(
            &mut gui,
            &view,
            ReviewAction::ToggleLineResolved {
                path: "code.txt".into(),
                start: end,
                end,
            },
        );
        let resolved = |text: &str| {
            view.files[0]
                .line_comments
                .iter()
                .find(|c| c.text == text)
                .unwrap()
                .resolved
        };
        assert!(resolved("Inner"));
        assert!(!resolved("Outer"));
    }

    #[test]
    fn editing_prose_preserves_suggestion_and_empty_saves_remove_only_the_requested_part() {
        let (_dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let (start, end) = span(&view, 8, 8);
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Suggestion {
                path: "code.txt".into(),
                start,
                end,
                text: "replacement".into(),
            },
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::LineComment {
                path: "code.txt".into(),
                start,
                end,
                text: "Explain".into(),
                severity: Severity::Blocker,
            },
        );
        assert_eq!(
            view.files[0].line_comments[0].suggestion.as_deref(),
            Some("replacement")
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Suggestion {
                path: "code.txt".into(),
                start,
                end,
                text: String::new(),
            },
        );
        assert_eq!(view.files[0].line_comments[0].text, "Explain");
        assert!(view.files[0].line_comments[0].suggestion.is_none());
        let view = action(
            &mut gui,
            &view,
            ReviewAction::LineComment {
                path: "code.txt".into(),
                start,
                end,
                text: String::new(),
                severity: Severity::Nit,
            },
        );
        assert!(view.files[0].line_comments.is_empty());
        assert_eq!(view.files[0].verdict, "undecided");
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Suggestion {
                path: "code.txt".into(),
                start,
                end,
                text: "code only".into(),
            },
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::LineComment {
                path: "code.txt".into(),
                start,
                end,
                text: String::new(),
                severity: Severity::Praise,
            },
        );
        assert_eq!(
            view.files[0].line_comments[0].suggestion.as_deref(),
            Some("code only")
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Suggestion {
                path: "code.txt".into(),
                start,
                end,
                text: String::new(),
            },
        );
        assert!(view.files[0].line_comments.is_empty());
    }

    #[test]
    fn invalid_reversed_and_stale_line_anchors_do_not_mutate_or_save() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let (start, end) = span(&view, 8, 9);
        for (start, end) in [
            (end, start),
            (
                DiffLineLocation {
                    old_line: None,
                    new_line: None,
                },
                end,
            ),
            (
                start,
                DiffLineLocation {
                    old_line: None,
                    new_line: Some(999),
                },
            ),
        ] {
            assert_eq!(
                act(
                    &mut gui,
                    &view.workflow_id,
                    view.revision,
                    ReviewAction::LineComment {
                        path: "code.txt".into(),
                        start,
                        end,
                        text: "Must not save".into(),
                        severity: Severity::Nit,
                    }
                )
                .unwrap_err()
                .kind,
                GuiErrorKind::Conflict
            );
        }
        assert!(
            snapshot(&mut gui).unwrap().files[0]
                .line_comments
                .is_empty()
        );
        assert_eq!(snapshot(&mut gui).unwrap().revision, view.revision);
        assert!(!crate::app::review::review_progress_path(&dir.path().join("repo")).exists());
        std::fs::write(dir.path().join("repo/code.txt"), "different code\n").unwrap();
        assert_eq!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::Suggestion {
                    path: "code.txt".into(),
                    start,
                    end,
                    text: "stale replacement".into(),
                }
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::Conflict
        );
    }

    #[test]
    fn deletion_only_comments_and_no_newline_markers_use_canonical_base_anchors() {
        let (dir, mut gui, target) = fixture();
        std::fs::remove_file(dir.path().join("repo/delete.txt")).unwrap();
        std::fs::write(dir.path().join("repo/new.txt"), "no newline").unwrap();
        let view = begin(&mut gui, target).unwrap();
        let base = DiffLineLocation {
            old_line: Some(1),
            new_line: None,
        };
        let view = action(
            &mut gui,
            &view,
            ReviewAction::LineComment {
                path: "delete.txt".into(),
                start: base,
                end: base,
                text: "Keep this?".into(),
                severity: Severity::Question,
            },
        );
        let comment = &view
            .files
            .iter()
            .find(|f| f.diff.path == "delete.txt")
            .unwrap()
            .line_comments[0];
        assert_eq!(comment.anchor, "base line 1");
        assert!(comment.editable);
        let loc = DiffLineLocation {
            old_line: None,
            new_line: Some(1),
        };
        let view = action(
            &mut gui,
            &view,
            ReviewAction::LineComment {
                path: "new.txt".into(),
                start: loc,
                end: loc,
                text: "Add newline".into(),
                severity: Severity::Nit,
            },
        );
        assert_eq!(
            view.files
                .iter()
                .find(|f| f.diff.path == "new.txt")
                .unwrap()
                .line_comments[0]
                .end,
            loc
        );
    }

    #[test]
    fn editing_a_carried_draft_keeps_its_origin_and_reopens_it_as_a_human_thread() {
        let (dir, mut gui, target) = fixture();
        let path = crate::app::review::review_progress_path(&dir.path().join("repo"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"line_comments":{"code.txt":[{"location":{"old_line":null,"new_line":8},"text":"AI idea","draft":true,"carried":true,"resolved":true,"severity":"nit"}]}}"#).unwrap();
        let view = begin(&mut gui, target).unwrap();
        let (start, end) = span(&view, 8, 8);
        assert_eq!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::ToggleLineResolved {
                    path: "code.txt".into(),
                    start,
                    end,
                }
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::Conflict
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::LineComment {
                path: "code.txt".into(),
                start,
                end,
                text: "Human comment".into(),
                severity: Severity::Nit,
            },
        );
        assert!(!view.files[0].line_comments[0].draft);
        assert!(!view.files[0].line_comments[0].resolved);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["line_comments"]["code.txt"][0]["carried"], true);
        let AppMode::DiffViewer(state) = &mut gui.app_for_workflow().mode else {
            panic!()
        };
        state.line_comments.get_mut("code.txt").unwrap()[0].anchor_lost = true;
        assert!(!snapshot(&mut gui).unwrap().files[0].line_comments[0].editable);
        assert_eq!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::Suggestion {
                    path: "code.txt".into(),
                    start,
                    end,
                    text: "must not replace a lost thread".into(),
                }
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::Conflict
        );
    }

    #[test]
    fn manual_review_saves_and_resumes_through_the_tui_state_without_starting_an_agent() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target.clone()).unwrap();
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Comment {
                path: "code.txt".into(),
                text: "Explain this".into(),
                severity: Severity::Question,
            },
        );
        assert_eq!(view.files[0].verdict, "undecided");
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Reject {
                path: "code.txt".into(),
                feedback: "Fix this".into(),
                severity: Severity::Blocker,
            },
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::General {
                text: "Overall feedback".into(),
            },
        );
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::Pause
            )
            .unwrap()
            .is_none()
        );
        // Restore with the TUI's loader, proving identical persistence semantics.
        let app = gui.app_for_workflow();
        let mut state = DiffViewerState::new(
            ViewState::new(
                "Demo".into(),
                "Feature".into(),
                String::new(),
                String::new(),
                String::new(),
                SessionKind::Terminal,
                Default::default(),
                false,
            ),
            dir.path().join("repo"),
        );
        state.review = true;
        app.mode = AppMode::DiffViewerLoading(state);
        app.complete_diff_viewer_loading();
        let AppMode::DiffViewer(state) = &app.mode else {
            panic!()
        };
        assert_eq!(state.general_feedback, "Overall feedback");
        assert_eq!(state.file_comments["code.txt"].text, "Explain this");
        assert!(matches!(
            state.decisions["code.txt"],
            ReviewDecision::Reject {
                severity: Severity::Blocker,
                ..
            }
        ));
        assert!(
            !state
                .files
                .iter()
                .any(|file| file.path.ends_with("final-review-progress.json"))
        );
        app.mode = AppMode::Normal;
        let resumed = begin(&mut gui, target).unwrap();
        assert_ne!(view.workflow_id, resumed.workflow_id);
        assert_eq!(resumed.files[0].feedback, "Fix this");
    }

    #[test]
    fn repeated_open_and_delayed_actions_use_workflow_and_revision_identity() {
        let (_dir, mut gui, target) = fixture();
        let original = begin(&mut gui, target.clone()).unwrap();
        assert_eq!(
            begin(&mut gui, target.clone()).unwrap().workflow_id,
            original.workflow_id
        );
        let current = action(
            &mut gui,
            &original,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        assert_eq!(
            act(
                &mut gui,
                &original.workflow_id,
                original.revision,
                ReviewAction::Skip {
                    path: "code.txt".into()
                }
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::Conflict
        );
        assert_eq!(current.files[0].verdict, "approved");
        act(
            &mut gui,
            &current.workflow_id,
            current.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        let resumed = begin(&mut gui, target).unwrap();
        assert_eq!(
            act(
                &mut gui,
                &current.workflow_id,
                current.revision,
                ReviewAction::Pause
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::Conflict
        );
        assert_eq!(snapshot(&mut gui).unwrap().workflow_id, resumed.workflow_id);
    }

    #[test]
    fn verdict_undo_skip_and_comment_resolution_use_shared_review_actions() {
        let (_dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        let view = action(&mut gui, &view, ReviewAction::Undo);
        assert_eq!(view.files[0].verdict, "undecided");
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Comment {
                path: "code.txt".into(),
                text: "Nice".into(),
                severity: Severity::Praise,
            },
        );
        let view = action(
            &mut gui,
            &view,
            ReviewAction::ToggleResolved {
                path: "code.txt".into(),
            },
        );
        assert!(view.files[0].comment.as_ref().unwrap().resolved);
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Comment {
                path: "code.txt".into(),
                text: "Edited".into(),
                severity: Severity::Nit,
            },
        );
        assert!(!view.files[0].comment.as_ref().unwrap().resolved);
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Comment {
                path: "code.txt".into(),
                text: String::new(),
                severity: Severity::Nit,
            },
        );
        assert!(view.files[0].comment.is_none());
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Skip {
                path: "code.txt".into(),
            },
        );
        assert_eq!(view.files[0].verdict, "undecided");
    }

    #[test]
    fn external_saved_review_changes_are_not_overwritten_and_can_be_reloaded() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let path = crate::app::review::review_progress_path(&dir.path().join("repo"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json = r#"{"general_feedback":"Saved from TUI"}"#;
        std::fs::write(&path, json).unwrap();
        assert_eq!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::General {
                    text: "Overwrite".into()
                }
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::Conflict
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), json);
        let reloaded = action(&mut gui, &view, ReviewAction::Reload);
        assert_eq!(reloaded.general_feedback, "Saved from TUI");
        assert!(
            !reloaded
                .files
                .iter()
                .any(|f| f.diff.path.ends_with("final-review-progress.json"))
        );
    }

    #[test]
    fn changed_patch_requires_refresh_before_a_verdict_or_comment() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let path = dir.path().join("repo/code.txt");
        std::fs::write(&path, "new changes\n").unwrap();
        assert_eq!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::Approve {
                    path: "code.txt".into()
                }
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::Conflict
        );
        let view = action(&mut gui, &view, ReviewAction::Refresh);
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        assert_eq!(view.files[0].verdict, "approved");
    }

    #[test]
    fn refresh_clears_changed_approvals_and_cannot_undo_them_back() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target).unwrap();
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        let skipped = action(
            &mut gui,
            &view,
            ReviewAction::Skip {
                path: "code.txt".into(),
            },
        );
        std::fs::write(dir.path().join("repo/code.txt"), "different patch\n").unwrap();
        assert_eq!(
            act(
                &mut gui,
                &skipped.workflow_id,
                skipped.revision,
                ReviewAction::Undo
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::Conflict
        );
        // Restore the already-approved view's state, then refresh the changed
        // patch: the approval and its undo history must be invalidated.
        let AppMode::DiffViewer(state) = &mut gui.app_for_workflow().mode else {
            panic!()
        };
        state
            .decisions
            .insert("code.txt".into(), ReviewDecision::Approve);
        let refreshed = action(&mut gui, &skipped, ReviewAction::Refresh);
        assert_eq!(refreshed.files[0].verdict, "undecided");
        let undone = action(&mut gui, &refreshed, ReviewAction::Undo);
        assert_eq!(undone.files[0].verdict, "undecided");
        let AppMode::DiffViewer(state) = &mut gui.app_for_workflow().mode else {
            panic!()
        };
        state.decisions.clear();
        gui.app_for_workflow().restore_review_progress();
        assert_eq!(snapshot(&mut gui).unwrap().files[0].verdict, "undecided");
    }

    #[test]
    fn saved_tui_line_threads_and_suggestions_survive_gui_file_actions() {
        let (dir, mut gui, target) = fixture();
        let path = crate::app::review::review_progress_path(&dir.path().join("repo"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"line_comments":{"code.txt":[{"location":{"old_line":null,"new_line":8},"text":"TUI thread","suggestion":"replacement","severity":"nit"}]},"apply_suggestions_on_finish":true,"applied_suggestions":["prior change"]}"#).unwrap();
        let view = begin(&mut gui, target).unwrap();
        assert_eq!(view.files[0].line_comments[0].text, "TUI thread");
        assert_eq!(
            view.files[0].line_comments[0].suggestion.as_deref(),
            Some("replacement")
        );
        let _view = action(
            &mut gui,
            &view,
            ReviewAction::Comment {
                path: "code.txt".into(),
                text: "GUI file comment".into(),
                severity: Severity::Praise,
            },
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            saved["line_comments"]["code.txt"][0]["suggestion"],
            "replacement"
        );
        assert_eq!(saved["apply_suggestions_on_finish"], true);
        assert_eq!(saved["applied_suggestions"][0], "prior change");
    }

    #[test]
    fn save_failure_keeps_edits_and_pause_waits_for_a_successful_retry() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target.clone()).unwrap();
        // The parent becomes unwritable without changing the progress file.
        let claude = dir.path().join("repo/.claude");
        std::fs::write(&claude, "parent is a file").unwrap();
        let view = action(
            &mut gui,
            &view,
            ReviewAction::General {
                text: "Keep me".into(),
            },
        );
        assert!(view.save_error.is_some());
        assert_eq!(view.general_feedback, "Keep me");
        let view = action(&mut gui, &view, ReviewAction::Pause);
        assert!(view.save_error.is_some());
        std::fs::remove_file(claude).unwrap();
        let view = action(&mut gui, &view, ReviewAction::RetrySave);
        assert!(view.save_error.is_none());
        act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        assert_eq!(begin(&mut gui, target).unwrap().general_feedback, "Keep me");
    }

    #[test]
    fn discard_closes_when_saving_keeps_failing_without_writing() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target.clone()).unwrap();
        let claude = dir.path().join("repo/.claude");
        std::fs::write(&claude, "parent is a file").unwrap();
        let view = action(
            &mut gui,
            &view,
            ReviewAction::General {
                text: "Unsaveable".into(),
            },
        );
        assert!(view.save_error.is_some());
        assert!(!gui.app_for_workflow().defer_review_progress_persist);
        // The feature disappears too: Pause can no longer succeed at all.
        let db = crate::db::AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = db.load_store().unwrap();
        store.projects[0].features.clear();
        db.save_store(&store).unwrap();
        let err = act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            ReviewAction::Pause,
        )
        .unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::NotFound);
        assert!(err.message.contains("discard"));
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::Discard
            )
            .unwrap()
            .is_none()
        );
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
        assert!(gui.review_context.is_none());
        assert_eq!(std::fs::read_to_string(claude).unwrap(), "parent is a file");
    }

    #[test]
    fn an_approval_is_dropped_on_reopen_when_its_patch_changed_while_paused() {
        let (dir, mut gui, target) = fixture();
        let view = begin(&mut gui, target.clone()).unwrap();
        let view = action(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        // Unchanged: the approval resumes.
        let view = begin(&mut gui, target.clone()).unwrap();
        assert_eq!(view.files[0].verdict, "approved");
        act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        std::fs::write(dir.path().join("repo/code.txt"), "edited while paused\n").unwrap();
        let view = begin(&mut gui, target).unwrap();
        assert_eq!(view.files[0].verdict, "undecided");
    }

    #[test]
    fn a_claude_file_and_stray_staging_files_are_not_review_progress() {
        let (dir, mut gui, target) = fixture();
        let claude = dir.path().join("repo/.claude");
        std::fs::write(&claude, "parent is a file").unwrap();
        assert!(
            begin(&mut gui, target.clone())
                .unwrap()
                .save_error
                .is_none()
        );
        let view = snapshot(&mut gui).unwrap();
        act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        std::fs::remove_file(&claude).unwrap();
        std::fs::create_dir(&claude).unwrap();
        // What a crash mid-save leaves behind is never a file to review.
        std::fs::write(claude.join(".final-review-progress.json.a1B2c3.tmp"), "{").unwrap();
        let view = begin(&mut gui, target).unwrap();
        assert!(
            view.files
                .iter()
                .all(|f| !f.diff.path.contains("final-review-progress"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn saving_progress_keeps_the_existing_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut gui, target) = fixture();
        let path = crate::app::review::review_progress_path(&dir.path().join("repo"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
        let view = begin(&mut gui, target).unwrap();
        action(
            &mut gui,
            &view,
            ReviewAction::General {
                text: "Saved".into(),
            },
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o664);
        assert!(std::fs::read_to_string(&path).unwrap().contains("Saved"));
    }

    #[test]
    fn stale_targets_and_non_git_projects_are_rejected_and_deleted_reviews_can_pause() {
        let (dir, mut gui, target) = fixture();
        let wrong = FeatureTarget {
            project_id: "wrong".into(),
            ..target.clone()
        };
        assert_eq!(
            begin(&mut gui, wrong).unwrap_err().kind,
            GuiErrorKind::NotFound
        );
        gui.app_for_workflow().store.projects[0].is_git = false;
        assert_eq!(
            begin(&mut gui, target.clone()).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        gui.app_for_workflow().store.projects[0].is_git = true;
        let view = begin(&mut gui, target).unwrap();
        let db = crate::db::AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = db.load_store().unwrap();
        store.projects[0].features.clear();
        db.save_store(&store).unwrap();
        assert_eq!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::General { text: "No".into() }
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::NotFound
        );
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::Pause
            )
            .unwrap()
            .is_none()
        );
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
    }

    #[test]
    fn malformed_progress_is_preserved_and_another_workflow_is_not_replaced() {
        let (dir, mut gui, target) = fixture();
        let path = crate::app::review::review_progress_path(&dir.path().join("repo"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "bad json").unwrap();
        assert!(begin(&mut gui, target.clone()).is_err());
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "bad json");
        std::fs::remove_file(path).unwrap();
        crate::gui_learning::begin(&mut gui, target.clone()).unwrap();
        assert_eq!(
            begin(&mut gui, target).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Learning(_)));
        // The fixture's harness is mocked, and the repo remains usable.
        assert!(!git(&dir.path().join("repo"), &["rev-parse", "HEAD"]).is_empty());
    }
}
