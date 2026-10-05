//! Pre-finish projection and source-write preparation. Finishing/checks/dispatch
//! remain separate from this explicit, repeat-safe batch action.
use serde::Serialize;

use super::{GuiError, GuiResult, Severity, ai};
use crate::app::{App, AppMode, DiffViewerState, ReviewDecision, SummaryItem};

#[derive(Debug, Serialize)]
pub struct ReviewSummaryView {
    pub rows: Vec<ReviewSummaryRow>,
    pub undecided: usize,
    pub pending_suggestions: usize,
    pub failures: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ReviewSummaryRow {
    pub path: Option<String>,
    pub title: String,
    pub text: String,
    pub severity: Option<Severity>,
    pub suggestion: Option<String>,
    pub apply_blocked: Option<String>,
}

pub(super) fn view(s: &DiffViewerState) -> Option<ReviewSummaryView> {
    if !s.summary_open {
        return None;
    }
    let rows = s
        .summary_items()
        .into_iter()
        .map(|item| {
            let mut row = ReviewSummaryRow {
                path: None,
                title: String::new(),
                text: String::new(),
                severity: None,
                suggestion: None,
                apply_blocked: None,
            };
            match item {
                SummaryItem::File { file_idx } => {
                    let path = &s.files[file_idx].path;
                    let verdict = match s.decisions.get(path) {
                        Some(ReviewDecision::Approve) => "approved",
                        Some(ReviewDecision::Reject { feedback, severity }) => {
                            row.text = feedback.clone();
                            row.severity = Some(*severity);
                            "needs work"
                        }
                        None => "no verdict",
                    };
                    row.title = format!("{path} — {verdict}");
                    row.path = Some(path.clone());
                }
                SummaryItem::LineComment {
                    file_idx,
                    comment_idx,
                } => {
                    let file = &s.files[file_idx];
                    let comment = &s.line_comments[&file.path][comment_idx];
                    row.path = Some(file.path.clone());
                    row.title = crate::app::review::comment_anchor_label(&file.path, comment);
                    row.text = comment.text.clone();
                    row.severity = Some(comment.severity);
                    row.suggestion = comment.suggestion.clone();
                    row.apply_blocked = crate::app::review::local_suggestion_blocker(file, comment);
                }
                SummaryItem::FileComment { file_idx } => {
                    let path = &s.files[file_idx].path;
                    let comment = &s.file_comments[path];
                    row.path = Some(path.clone());
                    row.title = format!("{path} — file comment");
                    row.text = comment.text.clone();
                    row.severity = Some(comment.severity);
                }
                SummaryItem::General => {
                    row.title = "Overall feedback".into();
                    row.text = s.general_feedback.clone();
                }
            }
            row
        })
        .collect();
    Some(ReviewSummaryView {
        rows,
        undecided: s
            .files
            .iter()
            .filter(|f| !s.decisions.contains_key(&f.path))
            .count(),
        pending_suggestions: s.pending_suggestion_count(),
        failures: s.open_suggestion_apply_failures(),
    })
}

pub(super) fn apply(app: &mut App) -> GuiResult<()> {
    let s = ai::state(&app.mode)?;
    if !s.summary_open || s.review_history.is_some() {
        return Err(GuiError::conflict(
            "Open the pre-finish summary before applying the batch",
        ));
    }
    if ai::view(app)?.running {
        return Err(GuiError::conflict(
            "Wait for or cancel the running AI request before applying suggestions",
        ));
    }
    if s.questions.draft.is_some() {
        return Err(GuiError::conflict(
            "Transfer or discard the generated comment draft first",
        ));
    }
    if s.pending_suggestion_count() == 0 {
        return Err(GuiError::conflict("No open suggestions to apply"));
    }
    // Check the complete reviewed changeset before any write, including files
    // without suggestions. Per-file source/path/overlap checks remain shared.
    ai::fresh(s)?;
    if let AppMode::DiffViewer(s) = &mut app.mode {
        s.apply_suggestions_on_finish = true;
    }
    app.prepare_final_review_suggestions();
    // Refresh may rebuild presentation state; keep the last-look page open.
    app.open_review_summary();
    Ok(())
}
