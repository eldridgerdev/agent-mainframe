//! Drafts of manual PR reviews, saved to the database (never the checkout).
//!
//! The draft body is the final review's own [`ReviewProgress`], so a PR review
//! and a feature review share one notion of "what a reviewer has done so far".
//! What differs is where it lives and what happens when the code moves:
//!
//! - A draft is saved at the revision it was made against (`head_oid`), with
//!   each file's diff fingerprint. When the PR's head has moved by the time it
//!   is reopened, the files whose fingerprint changed are flagged for another
//!   look (`changed_since_last`, the final review's own re-review marker) and
//!   lose their verdicts — an approval of code that has since changed is not
//!   an approval. Files that did not change keep theirs. Every comment is
//!   kept and re-anchored; one whose code is gone is marked `anchor_lost` and
//!   shown as outdated in the notes panel.
//! - Comments on a file that is no longer in the PR's diff are held aside in
//!   `pr_detached_*` rather than dropped, and saved back every time.

use std::collections::HashMap;

use super::preparation::{ReviewProgress, file_fingerprint};
use crate::app::{App, AppMode, DiffScope};
use crate::db::pr_review_drafts::{PrReviewDraft, PrReviewDraftStatus};

/// How many comments a saved draft holds (line + whole-file), for the Review
/// tab's badge. An unreadable draft counts as none rather than failing the list.
pub(crate) fn draft_comment_count(progress_json: &str) -> usize {
    serde_json::from_str::<ReviewProgress>(progress_json)
        .map(|p| p.line_comments.values().map(Vec::len).sum::<usize>() + p.file_comments.len())
        .unwrap_or(0)
}

/// What a draft save did, so a caller's message can say exactly that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DraftSave {
    Saved,
    /// Nothing to keep: no row (any old one was removed).
    Empty,
    NoDatabase,
    Failed(String),
    NotAPrReview,
}

fn progress_is_empty(progress: &ReviewProgress) -> bool {
    progress.decisions.is_empty()
        && progress.line_comments.values().all(Vec::is_empty)
        && progress.file_comments.is_empty()
        && progress.general_feedback.trim().is_empty()
}

impl App {
    /// Save the open PR review's draft, returning whether it is safely stored.
    /// An untouched review saves nothing (and clears a draft emptied by hand),
    /// so merely opening a PR never leaves a "draft" badge behind.
    pub(crate) fn persist_pr_review_draft(&mut self) -> DraftSave {
        let AppMode::DiffViewer(state) = &self.mode else {
            return DraftSave::NotAPrReview;
        };
        let DiffScope::PullRequest(target) = &state.scope else {
            return DraftSave::NotAPrReview;
        };
        let Some(db) = &self.db else {
            return DraftSave::NoDatabase;
        };
        let mut progress = ReviewProgress::of(state);
        for (path, comments) in &state.pr_detached_line_comments {
            progress
                .line_comments
                .entry(path.clone())
                .or_default()
                .extend(comments.iter().cloned());
        }
        for (path, comment) in &state.pr_detached_file_comments {
            progress
                .file_comments
                .entry(path.clone())
                .or_insert_with(|| comment.clone());
        }

        let empty = progress_is_empty(&progress);
        let result = if empty {
            db.delete_pr_review_draft(&target.repo, target.pr.number)
        } else {
            serde_json::to_string(&progress)
                .map_err(anyhow::Error::from)
                .and_then(|progress| {
                    db.upsert_pr_review_draft(&PrReviewDraft {
                        repo_key: target.repo.clone(),
                        pr_number: target.pr.number,
                        base_oid: target.pr.base_oid.clone(),
                        head_oid: target.pr.head_oid.clone(),
                        merge_base_oid: target.merge_base_oid.clone(),
                        status: PrReviewDraftStatus::Draft,
                        progress,
                        file_fingerprints: state
                            .files
                            .iter()
                            .map(|file| (file.path.clone(), file_fingerprint(file)))
                            .collect(),
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    })
                })
        };
        match result {
            Ok(()) if empty => DraftSave::Empty,
            Ok(()) => DraftSave::Saved,
            Err(err) => {
                let err = format!("{err:#}");
                self.log_warn("review", format!("failed to save PR review draft: {err}"));
                self.message = Some(format!("Couldn't save the PR review draft: {err}"));
                DraftSave::Failed(err)
            }
        }
    }

    /// After a PR review's diff (re)loads: restore the saved draft (on first
    /// load), re-anchor every comment against this diff, and say in one message
    /// what happened — including comments whose code is gone (outdated).
    pub(crate) fn resume_pr_review(&mut self) {
        let before = self.message.clone();
        self.restore_pr_review_draft();
        let restored = (self.message != before)
            .then(|| self.message.clone())
            .flatten();
        self.reanchor_line_comments();
        let AppMode::DiffViewer(state) = &self.mode else {
            return;
        };
        let outdated = state
            .line_comments
            .values()
            .flatten()
            .filter(|c| c.anchor_lost)
            .count();
        let outdated_note = (outdated > 0).then(|| {
            format!("{outdated} comment(s) are outdated — their code is gone (see the notes panel)")
        });
        self.message = match (restored, outdated_note) {
            (Some(restored), Some(outdated)) => Some(format!("{restored}; {outdated}")),
            (Some(restored), None) => Some(restored),
            // A refresh with nothing restored keeps the re-anchor message.
            (None, _) => self.message.take(),
        };
    }

    /// Load the saved draft, if any, into a PR review that has just opened.
    /// Runs once: a refresh (`r`) of a review already holding work keeps it.
    fn restore_pr_review_draft(&mut self) {
        let AppMode::DiffViewer(state) = &mut self.mode else {
            return;
        };
        let DiffScope::PullRequest(target) = &state.scope else {
            return;
        };
        if !state.decisions.is_empty()
            || !state.line_comments.is_empty()
            || !state.file_comments.is_empty()
            || !state.general_feedback.is_empty()
            || !state.pr_detached_line_comments.is_empty()
            || !state.pr_detached_file_comments.is_empty()
        {
            return;
        }
        let Some(db) = &self.db else {
            return;
        };
        let number = target.pr.number;
        let head = target.pr.head_oid.clone();
        let draft = match db.load_pr_review_draft(&target.repo, number) {
            Ok(Some(draft)) => draft,
            Ok(None) => return,
            Err(err) => {
                self.log_warn("review", format!("failed to load PR review draft: {err}"));
                self.message = Some(format!("Couldn't load your saved draft: {err}"));
                return;
            }
        };
        if draft.status == PrReviewDraftStatus::Posted {
            // That review is on GitHub; this is a new one.
            self.message = Some(format!(
                "You already posted a review of PR #{number} — this starts a new one"
            ));
            return;
        }
        let progress: ReviewProgress = match serde_json::from_str(&draft.progress) {
            Ok(progress) => progress,
            Err(err) => {
                self.log_warn("review", format!("unreadable PR review draft: {err}"));
                self.message = Some(format!(
                    "Your saved draft for PR #{number} couldn't be read ({err}); starting fresh"
                ));
                return;
            }
        };

        let AppMode::DiffViewer(state) = &mut self.mode else {
            return;
        };
        let known: std::collections::HashSet<String> =
            state.files.iter().map(|f| f.path.clone()).collect();
        let (line_comments, detached_lines): (HashMap<_, _>, HashMap<_, _>) = progress
            .line_comments
            .into_iter()
            .filter(|(_, comments)| !comments.is_empty())
            .partition(|(path, _)| known.contains(path));
        let (file_comments, detached_files): (HashMap<_, _>, HashMap<_, _>) = progress
            .file_comments
            .into_iter()
            .partition(|(path, _)| known.contains(path));
        let detached = detached_lines.values().map(Vec::len).sum::<usize>() + detached_files.len();
        state.line_comments = line_comments;
        state.file_comments = file_comments;
        state.pr_detached_line_comments = detached_lines;
        state.pr_detached_file_comments = detached_files;
        state.general_feedback = progress.general_feedback;

        let same_head = draft.head_oid == head;
        // A file is unchanged only if its diff fingerprint matches the one
        // saved with the draft; a file new to the PR has none and is changed.
        let changed: std::collections::HashSet<String> = if same_head {
            Default::default()
        } else {
            state
                .files
                .iter()
                .filter(|file| {
                    draft.file_fingerprints.get(&file.path) != Some(&file_fingerprint(file))
                })
                .map(|file| file.path.clone())
                .collect()
        };
        let keeps_verdict = |path: &String| known.contains(path) && !changed.contains(path);
        let cleared = progress
            .decisions
            .keys()
            .filter(|path| changed.contains(*path))
            .count();
        state.decisions = progress
            .decisions
            .into_iter()
            .filter(|(path, _)| keeps_verdict(path))
            .collect();
        state.auto_rejected = progress
            .auto_rejected
            .into_iter()
            .filter(|path| keeps_verdict(path))
            .collect();
        state.has_prior_review = !same_head;
        state.changed_since_last = changed;

        if !state.files.is_empty() {
            let changed_count = state.changed_since_last.len();
            if !same_head && changed_count > 0 && changed_count < state.files.len() {
                // Only some files moved: show just those, starting at the
                // first, as the final review's re-review does.
                state.file_filter = crate::app::FileFilter::Changed;
                state.selected_file = state
                    .files
                    .iter()
                    .position(|f| state.changed_since_last.contains(&f.path))
                    .unwrap_or(0);
            } else {
                state.selected_file = progress.selected_file.min(state.files.len() - 1);
            }
            state.on_file_changed();
        }

        let mut notes = Vec::new();
        if !same_head {
            let changed_count = state.changed_since_last.len();
            let total = state.files.len();
            let mut note = match changed_count {
                0 => "the PR has new commits, but none of its files' changes differ".to_string(),
                n if n == total => format!("all {total} file(s) changed since your draft"),
                n => format!("{n} of {total} file(s) changed since your draft (showing those)"),
            };
            if cleared > 0 {
                note.push_str(&format!(", {cleared} verdict(s) on them cleared"));
            }
            notes.push(note);
        }
        if detached > 0 {
            notes.push(format!(
                "{detached} comment(s) are on files no longer in the PR (kept in the draft)"
            ));
        }
        self.message = Some(if notes.is_empty() {
            format!("Resumed your draft review of PR #{number}")
        } else {
            format!("Resumed PR #{number}: {}", notes.join("; "))
        });
    }
}
