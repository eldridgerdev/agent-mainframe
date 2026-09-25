//! Submitting a PR review to GitHub (`q` in a PR review).
//!
//! The payload is the final review's own GitHub mapping ([`build_pr_review`]):
//! line comments inline (ranges as `start_line`/`start_side`, suggestions as
//! ```` ```suggestion ```` blocks), rejections and file comments as whole-file
//! comments, general feedback as the summary. What differs is what happens to
//! a comment GitHub can't take inline — outside the diff, outdated, or on a
//! file no longer in the PR: a feature review keeps those in its local
//! feedback file, but a PR review has nothing else, so they are folded into
//! the summary rather than dropped.
//!
//! Posting runs on a worker thread and re-checks the PR's head first: a PR
//! that moved since the review opened is not posted to (see
//! [`crate::app::PrSubmitStatus::HeadMoved`]).

use super::headless::{build_pr_review, inline_position, pr_postable_lines};
use crate::app::pr_review::runtime::{PrPostOutcome, PrPostRequest};
use crate::app::{
    App, AppMode, DiffScope, DiffViewerState, FileComment, LineComment, PrDiffTarget,
    PrReviewEvent, PrSubmitState, PrSubmitStatus, ReviewDecision,
};
use crate::db::pr_review_drafts::PrReviewDraftStatus;

/// What would be posted right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrSubmission {
    pub(crate) body: String,
    pub(crate) comments: Vec<crate::github::PrReviewComment>,
    pub(crate) file_comments: Vec<crate::github::PrFileComment>,
    /// Comments that couldn't go on the diff and were folded into `body`.
    pub(crate) in_summary: usize,
}

/// Build the review a PR review's current state would post.
pub(crate) fn build_pr_submission(state: &DiffViewerState) -> PrSubmission {
    let mut rejected = Vec::new();
    let mut file_comment_sections: Vec<(String, FileComment)> = Vec::new();
    let mut line_comment_sections: Vec<(String, Vec<LineComment>)> = Vec::new();
    for file in &state.files {
        if let Some(ReviewDecision::Reject { feedback, severity }) = state.decisions.get(&file.path)
        {
            rejected.push((file.path.clone(), feedback.clone(), *severity));
        }
        if let Some(comment) = state
            .file_comments
            .get(&file.path)
            .filter(|c| c.is_open_thread())
        {
            file_comment_sections.push((file.path.clone(), comment.clone()));
        }
        // Unaccepted AI drafts and resolved threads are never posted.
        let open: Vec<LineComment> = state
            .line_comments
            .get(&file.path)
            .into_iter()
            .flatten()
            .filter(|c| c.is_open_thread())
            .cloned()
            .collect();
        if !open.is_empty() {
            line_comment_sections.push((file.path.clone(), open));
        }
    }

    let postable = pr_postable_lines(&state.files);
    let (summary, comments, file_comments) = build_pr_review(
        &rejected,
        &file_comment_sections,
        &line_comment_sections,
        &state.general_feedback,
        &postable,
    );

    let mut folded = Vec::new();
    for (path, file_comments) in &line_comment_sections {
        for comment in file_comments {
            if inline_position(comment, postable.get(path)).is_none() {
                let why = if comment.anchor_lost {
                    "code no longer in the PR"
                } else {
                    "outside the diff"
                };
                folded.push(folded_line(path, comment, why));
            }
        }
    }
    let mut detached: Vec<(&String, &Vec<LineComment>)> =
        state.pr_detached_line_comments.iter().collect();
    detached.sort_by_key(|(path, _)| *path);
    for (path, file_comments) in detached {
        for comment in file_comments.iter().filter(|c| c.is_open_thread()) {
            folded.push(folded_line(path, comment, "file no longer in the PR"));
        }
    }
    let mut detached_files: Vec<(&String, &FileComment)> = state
        .pr_detached_file_comments
        .iter()
        .filter(|(_, c)| c.is_open_thread())
        .collect();
    detached_files.sort_by_key(|(path, _)| *path);
    for (path, comment) in detached_files {
        folded.push(format!(
            "- `{path}` (whole file; file no longer in the PR): **[{}]** {}",
            comment.severity.label(),
            comment.text.trim()
        ));
    }

    let in_summary = folded.len();
    let mut body = summary;
    if !folded.is_empty() {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str("**Comments that couldn't be placed on the diff**\n\n");
        body.push_str(&folded.join("\n"));
    }
    PrSubmission {
        body,
        comments,
        file_comments,
        in_summary,
    }
}

/// One folded comment: where it was, why it isn't inline, and what it says.
/// A suggestion is kept as a plain code block (a `suggestion` block only
/// works on a diff line).
fn folded_line(path: &str, comment: &LineComment, why: &str) -> String {
    let at = comment
        .location
        .new_line
        .or(comment.location.old_line)
        .map(|line| format!("L{line}, "))
        .unwrap_or_default();
    let mut line = format!(
        "- `{path}` ({at}{why}): **[{}]** {}",
        comment.severity.label(),
        comment.text.trim()
    );
    if let Some(suggestion) = &comment.suggestion {
        line.push_str("\n\n  Suggested:\n\n  ```\n");
        for code in suggestion.lines() {
            line.push_str("  ");
            line.push_str(code);
            line.push('\n');
        }
        line.push_str("  ```\n");
    }
    line
}

/// Why `event` can't be posted with `submission`, if it can't. Checked
/// before posting, so the answer is AMF's plain one rather than GitHub's 422.
pub(crate) fn submission_problem(
    event: PrReviewEvent,
    submission: &PrSubmission,
    own_pr: Option<bool>,
) -> Option<&'static str> {
    if own_pr == Some(true) && event != PrReviewEvent::Comment {
        return Some(
            "GitHub doesn't allow approving or requesting changes on your own PR — choose Comment",
        );
    }
    let has_body = !submission.body.trim().is_empty();
    match event {
        PrReviewEvent::RequestChanges if !has_body => {
            Some("Request changes needs a summary — press e to write one")
        }
        PrReviewEvent::Comment
            if !has_body
                && submission.comments.is_empty()
                && submission.file_comments.is_empty() =>
        {
            Some("Nothing to post — add a comment, or press e to write a summary")
        }
        _ => None,
    }
}

/// The `PrRef` a review of `target` posts against, pinned to the reviewed
/// head. `None` when the repository key isn't `host/owner/name`.
fn pr_ref_for(target: &PrDiffTarget) -> Option<crate::github::PrRef> {
    let mut parts = target.repo.splitn(3, '/');
    let (host, owner, repo) = (parts.next()?, parts.next()?, parts.next()?);
    if host.is_empty() || owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(crate::github::PrRef {
        number: target.pr.number,
        head_sha: target.pr.head_oid.clone(),
        url: format!("https://{host}/{owner}/{repo}/pull/{}", target.pr.number),
        owner: owner.to_string(),
        repo: repo.to_string(),
        head_ref: target.pr.head_ref.clone(),
    })
}

fn pr_submit_mut(mode: &mut AppMode) -> Option<(&mut PrSubmitState, &PrDiffTarget)> {
    let AppMode::DiffViewer(state) = mode else {
        return None;
    };
    let DiffScope::PullRequest(target) = &state.scope else {
        return None;
    };
    Some((state.pr_submit.as_mut()?, target))
}

impl App {
    pub fn pr_submit_open(&self) -> bool {
        matches!(&self.mode, AppMode::DiffViewer(state) if state.pr_submit.is_some())
    }

    /// `q` in a PR review: open the submit dialog.
    pub(crate) fn open_pr_submit(&mut self) {
        let own_pr = match (&self.gh_current_user, &self.mode) {
            (Some(Some(me)), AppMode::DiffViewer(state)) => match &state.scope {
                DiffScope::PullRequest(target) => Some(target.pr.author.eq_ignore_ascii_case(me)),
                _ => None,
            },
            _ => None,
        };
        if let AppMode::DiffViewer(state) = &mut self.mode
            && state.is_pr_review()
        {
            state.pr_submit = Some(PrSubmitState {
                event: PrReviewEvent::Comment,
                status: PrSubmitStatus::Ready,
                own_pr,
            });
        }
    }

    /// Move the event choice by `delta`, wrapping.
    pub fn pr_submit_cycle_event(&mut self, delta: isize) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(submit) = &mut state.pr_submit
            && !matches!(submit.status, PrSubmitStatus::Posting { .. })
        {
            let all = PrReviewEvent::ALL;
            let index = all.iter().position(|e| *e == submit.event).unwrap_or(0) as isize;
            let next = (index + delta).rem_euclid(all.len() as isize) as usize;
            submit.event = all[next];
        }
    }

    /// `Esc`: close the dialog, keeping the draft. Not while posting: a post
    /// abandoned halfway may still land, and its answer must be seen.
    pub fn pr_submit_close(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(submit) = &state.pr_submit
        {
            if matches!(submit.status, PrSubmitStatus::Posting { .. }) {
                self.message = Some("Posting to GitHub — wait for its answer".to_string());
                return;
            }
            state.pr_submit = None;
        }
    }

    /// `e`: edit the summary, which is the review's general feedback.
    pub fn pr_submit_edit_summary(&mut self) {
        let posting = matches!(
            &self.mode,
            AppMode::DiffViewer(state)
                if matches!(state.pr_submit.as_ref().map(|s| &s.status), Some(PrSubmitStatus::Posting { .. }))
        );
        if !posting {
            self.diff_review_start_general_feedback();
        }
    }

    /// `Enter`: post (or retry). Saves the draft first, so nothing is lost
    /// whatever GitHub answers.
    pub fn pr_submit_post(&mut self) {
        let AppMode::DiffViewer(state) = &self.mode else {
            return;
        };
        let (Some(submit), DiffScope::PullRequest(target)) = (&state.pr_submit, &state.scope)
        else {
            return;
        };
        match &submit.status {
            PrSubmitStatus::Posting { .. } => return,
            PrSubmitStatus::HeadMoved { .. } => {
                self.message =
                    Some("The PR has new commits — press o to reopen it at the new head".into());
                return;
            }
            PrSubmitStatus::Ready | PrSubmitStatus::Failed(_) => {}
        }
        let submission = build_pr_submission(state);
        let problem = submission_problem(submit.event, &submission, submit.own_pr);
        let pr = pr_ref_for(target);
        let repo_key = target.repo.clone();
        let workdir = state.workdir.clone();
        let event = submit.event;

        let failure = match (problem, pr) {
            (Some(problem), _) => Err(problem.to_string()),
            (None, None) => Err(format!(
                "Can't tell which GitHub repository `{repo_key}` is, so there's nowhere to post"
            )),
            (None, Some(pr)) => Ok(pr),
        };
        let pr = match failure {
            Ok(pr) => pr,
            Err(problem) => {
                if let Some((submit, _)) = pr_submit_mut(&mut self.mode) {
                    submit.status = PrSubmitStatus::Failed(problem);
                }
                return;
            }
        };

        let _ = self.persist_pr_review_draft();
        let request_id = self.pr_review_work.begin_review_post(PrPostRequest {
            workdir,
            repo_key,
            pr,
            event,
            body: submission.body,
            comments: submission.comments,
            file_comments: submission.file_comments,
        });
        if let Some((submit, _)) = pr_submit_mut(&mut self.mode) {
            submit.status = PrSubmitStatus::Posting { request_id };
        }
    }

    /// `o` after a blocked post: leave (saving the draft at the head it was
    /// made against) and reopen the PR, which fetches the new head and flags
    /// what changed.
    pub fn pr_submit_reopen_at_new_head(&mut self) {
        let head_moved = matches!(
            &self.mode,
            AppMode::DiffViewer(state)
                if matches!(state.pr_submit.as_ref().map(|s| &s.status), Some(PrSubmitStatus::HeadMoved { .. }))
        );
        if !head_moved {
            return;
        }
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.pr_submit = None;
        }
        self.pause_final_review();
        if matches!(self.mode, AppMode::PrReviewList(_)) {
            self.pr_review_list_open_selected();
        }
    }

    /// Apply a finished post. A post that succeeded is recorded even if the
    /// viewer is somehow gone, since it is on GitHub regardless.
    pub fn poll_pr_review_post_bg(&mut self) -> bool {
        let Some(polled) = self.pr_review_work.poll_review_post() else {
            return false;
        };
        let fetch = match polled {
            Ok(fetch) => fetch,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pr_review_work.finish_review_post();
                if let Some((submit, _)) = pr_submit_mut(&mut self.mode)
                    && matches!(submit.status, PrSubmitStatus::Posting { .. })
                {
                    submit.status = PrSubmitStatus::Failed(
                        "the posting worker stopped without an answer — check the PR on GitHub \
                         before retrying"
                            .to_string(),
                    );
                    return true;
                }
                return false;
            }
        };
        self.pr_review_work.finish_review_post();
        let number = fetch.request.pr.number;

        if let PrPostOutcome::Posted { .. } = &fetch.outcome {
            self.mark_pr_review_posted(&fetch.request.repo_key, number);
        }

        let waiting = matches!(
            pr_submit_mut(&mut self.mode),
            Some((submit, _)) if submit.status == PrSubmitStatus::Posting { request_id: fetch.request_id }
        );
        match fetch.outcome {
            PrPostOutcome::Posted {
                file_comment_failures,
            } => {
                let mut message = format!(
                    "Posted a {} review to PR #{number}",
                    fetch.request.event.label()
                );
                let inline = fetch.request.comments.len();
                if inline > 0 {
                    message.push_str(&format!(" with {inline} inline comment(s)"));
                }
                if !file_comment_failures.is_empty() {
                    message.push_str(&format!(
                        " — but {} file comment(s) failed: {}",
                        file_comment_failures.len(),
                        file_comment_failures.join("; ")
                    ));
                }
                if waiting {
                    // Done: back to the list, the review's refs removed. The
                    // row's draft badge goes with it — the draft is posted.
                    // (`waiting` means the mode is this PR's viewer.)
                    if let AppMode::DiffViewer(state) =
                        std::mem::replace(&mut self.mode, AppMode::Normal)
                    {
                        self.exit_pr_review(state);
                    }
                    if let AppMode::PrReviewList(list) = &mut self.mode {
                        list.drafts.remove(&number);
                    }
                }
                self.message = Some(message);
            }
            PrPostOutcome::HeadMoved { current_head } => {
                if waiting && let Some((submit, _)) = pr_submit_mut(&mut self.mode) {
                    submit.status = PrSubmitStatus::HeadMoved { current_head };
                } else {
                    self.message = Some(format!(
                        "Not posted: PR #{number} has new commits since your review"
                    ));
                }
            }
            PrPostOutcome::Failed(err) => {
                self.log_warn("review", format!("PR #{number} review post failed: {err}"));
                if waiting && let Some((submit, _)) = pr_submit_mut(&mut self.mode) {
                    submit.status = PrSubmitStatus::Failed(err);
                } else {
                    self.message = Some(format!("PR #{number} review not posted: {err}"));
                }
            }
        }
        true
    }

    /// Record that the draft for `number` was posted, so it gets no badge and
    /// the next review of the PR starts fresh.
    fn mark_pr_review_posted(&mut self, repo_key: &str, number: u32) {
        let Some(db) = &self.db else {
            return;
        };
        let result = db.load_pr_review_draft(repo_key, number).and_then(|draft| {
            let Some(mut draft) = draft else {
                return Ok(());
            };
            draft.status = PrReviewDraftStatus::Posted;
            draft.updated_at = chrono::Utc::now().to_rfc3339();
            db.upsert_pr_review_draft(&draft)
        });
        if let Err(err) = result {
            self.log_warn(
                "review",
                format!("posted PR #{number}, but couldn't mark its draft posted: {err}"),
            );
        }
    }
}

/// Counts the submit dialog shows: inline comments, whole-file comments, and
/// comments folded into the summary.
pub(crate) fn submission_counts(state: &DiffViewerState) -> (usize, usize, usize) {
    let submission = build_pr_submission(state);
    (
        submission.comments.len(),
        submission.file_comments.len(),
        submission.in_summary,
    )
}
