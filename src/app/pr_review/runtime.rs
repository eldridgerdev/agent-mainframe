//! In-flight PR work. Closing a pane and cancelling a worker are separate actions.

use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};

use super::{InvestigationOutcome, PrReview};
use crate::app::ai_review::AiReviewProgress;
use crate::app::{AiReviewRunProgress, AiReviewState};

/// One fetch and one investigation slot, preserving the original independent
/// cancellation and receiver-drop behavior. AppMode remains the result target.
///
/// The Review tab's list load has a third, independent slot. Its results
/// carry the request id they were started under, because (unlike the other
/// two) a retry can start a new load while the old one is still in flight.
pub(crate) struct PrReviewWork {
    fetch: Option<Receiver<anyhow::Result<PrReview>>>,
    investigation: Option<Receiver<InvestigationOutcome>>,
    review_list: Option<Receiver<ReviewListFetch>>,
    review_list_seq: u64,
    review_list_loader: ReviewListLoader,
    review_open: Option<Receiver<ReviewOpenFetch>>,
    review_open_seq: u64,
    review_opener: ReviewOpener,
    review_post: Option<Receiver<ReviewPostFetch>>,
    review_post_seq: u64,
    review_poster: ReviewPoster,
}

/// Everything needed to post one PR review. `pr.head_sha` is the head the
/// review was made against: inline comments are pinned to it.
#[derive(Debug, Clone)]
pub(crate) struct PrPostRequest {
    pub(crate) workdir: std::path::PathBuf,
    pub(crate) repo_key: String,
    pub(crate) pr: crate::github::PrRef,
    pub(crate) event: crate::app::PrReviewEvent,
    pub(crate) body: String,
    pub(crate) comments: Vec<crate::github::PrReviewComment>,
    pub(crate) file_comments: Vec<crate::github::PrFileComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrPostOutcome {
    /// The review is on GitHub. Whole-file comments are posted one by one
    /// after it, so each that failed is listed (`path: error`).
    Posted { file_comment_failures: Vec<String> },
    /// Not posted: the PR's head is no longer the reviewed one.
    HeadMoved { current_head: String },
    /// Not posted.
    Failed(String),
}

pub(crate) struct ReviewPostFetch {
    pub(crate) request_id: u64,
    pub(crate) request: PrPostRequest,
    pub(crate) outcome: PrPostOutcome,
}

/// Blocking post run on the worker thread. A plain `fn` so tests can
/// substitute one without `gh`.
pub(crate) type ReviewPoster = fn(&PrPostRequest) -> PrPostOutcome;

fn gh_review_poster(request: &PrPostRequest) -> PrPostOutcome {
    use crate::github::GhCli;
    let workdir = &request.workdir;
    let number = request.pr.number;
    // Re-check right before posting: the PR may have moved since it opened.
    match GhCli::pr_revisions(workdir, number) {
        Ok(current) if current.pr.head_oid != request.pr.head_sha => {
            return PrPostOutcome::HeadMoved {
                current_head: current.pr.head_oid,
            };
        }
        Ok(current) if !current.state.eq_ignore_ascii_case("open") => {
            return PrPostOutcome::Failed(format!(
                "PR #{number} is {} — GitHub doesn't take reviews on it",
                current.state.to_ascii_lowercase()
            ));
        }
        Ok(_) => {}
        Err(err) => {
            return PrPostOutcome::Failed(format!("Couldn't re-check PR #{number}: {err:#}"));
        }
    }
    if let Err(err) = GhCli::create_review(
        workdir,
        &request.pr,
        &request.body,
        request.event.api_name(),
        &request.comments,
    ) {
        let message = format!("{err:#}");
        // `create_review`'s 422 text is written for the AI review's flow;
        // say what it means here instead.
        let message = if message.contains("GitHub rejected the review (422)") {
            format!(
                "GitHub rejected the review (422). An inline comment may not match the \
                 PR's diff, or the event isn't allowed on this PR. {message}"
            )
        } else {
            message
        };
        return PrPostOutcome::Failed(message);
    }
    let file_comment_failures = request
        .file_comments
        .iter()
        .filter_map(|comment| {
            GhCli::create_file_comment(workdir, &request.pr, &comment.path, &comment.body)
                .err()
                .map(|err| format!("{}: {err:#}", comment.path))
        })
        .collect();
    PrPostOutcome::Posted {
        file_comment_failures,
    }
}

/// What the worker opening a PR for review sends back.
pub(crate) struct ReviewOpenFetch {
    pub(crate) request_id: u64,
    pub(crate) target: anyhow::Result<crate::app::PrDiffTarget>,
}

/// Blocking "make this PR reviewable" run on the worker thread: fetch its
/// revisions into private refs and pin them. A plain `fn` so tests can
/// substitute one without `gh` or a network.
pub(crate) type ReviewOpener =
    fn(&Path, &crate::github::ReviewablePr) -> anyhow::Result<crate::app::PrDiffTarget>;

fn gh_review_opener(
    workdir: &Path,
    pr: &crate::github::ReviewablePr,
) -> anyhow::Result<crate::app::PrDiffTarget> {
    use super::revisions;
    use crate::github::GhCli;

    // Refs a crash left behind. Safe with a review open elsewhere, which
    // reads by OID (see `prune_review_refs`).
    let _ = revisions::prune_review_refs(workdir);
    let url = GhCli::base_repo_url(workdir)?;
    let revisions = revisions::materialize(workdir, &url, pr, || {
        GhCli::pr_revisions(workdir, pr.number).map(|current| current.pr)
    })?;
    // `materialize` may have refreshed a PR that moved: pin the row to what
    // was actually fetched and verified.
    let mut pinned = pr.clone();
    pinned.base_oid = revisions.base_oid;
    pinned.head_oid = revisions.head_oid;
    Ok(crate::app::PrDiffTarget {
        repo: pr_review_repo_key(&url),
        pr: pinned,
        merge_base_oid: revisions.merge_base_oid,
    })
}

/// What the Review tab's worker sends back.
pub(crate) struct ReviewListFetch {
    pub(crate) request_id: u64,
    pub(crate) loaded: ReviewListLoaded,
}

/// A list load's results.
pub(crate) struct ReviewListLoaded {
    pub(crate) prs: anyhow::Result<Vec<crate::github::ReviewablePr>>,
    /// The `gh` user, when the worker was asked to resolve it.
    pub(crate) current_user: Option<String>,
    /// The repository's draft key ([`pr_review_repo_key`]), for finding
    /// saved drafts. `None` when it couldn't be resolved: no badges.
    pub(crate) repo_key: Option<String>,
}

/// Blocking list load run on the worker thread: `(workdir, resolve_user)`.
/// A plain `fn` so tests can substitute one without a `gh` binary.
pub(crate) type ReviewListLoader = fn(&Path, bool) -> ReviewListLoaded;

/// The key PR review drafts are filed under: the base repository `gh`
/// resolves, as `host/owner/name`, or the raw URL when that can't be parsed.
/// The list and the opener both use this, so a badge always finds its draft.
pub(crate) fn pr_review_repo_key(base_repo_url: &str) -> String {
    crate::github::GithubRepository::from_remote_url(base_repo_url)
        .map(|repo| repo.canonical())
        .unwrap_or_else(|| base_repo_url.to_string())
}

fn gh_review_list_loader(workdir: &Path, resolve_user: bool) -> ReviewListLoaded {
    use crate::github::GhCli;
    let prs = GhCli::list_reviewable_prs(workdir);
    // Only worth more calls when the list itself worked: an auth or network
    // failure would fail these too, and the list error is the one to show.
    let (current_user, repo_key) = if prs.is_ok() {
        (
            resolve_user
                .then(|| GhCli::current_user(workdir).ok())
                .flatten(),
            GhCli::base_repo_url(workdir)
                .ok()
                .map(|url| pr_review_repo_key(&url)),
        )
    } else {
        (None, None)
    };
    ReviewListLoaded {
        prs,
        current_user,
        repo_key,
    }
}

impl Default for PrReviewWork {
    fn default() -> Self {
        Self {
            fetch: None,
            investigation: None,
            review_list: None,
            review_list_seq: 0,
            review_list_loader: gh_review_list_loader,
            review_open: None,
            review_open_seq: 0,
            review_opener: gh_review_opener,
            review_post: None,
            review_post_seq: 0,
            review_poster: gh_review_poster,
        }
    }
}

impl PrReviewWork {
    /// Start a Review-tab list load on a worker thread and return its request
    /// id. Any load already in flight is abandoned: its receiver is dropped,
    /// so its result goes nowhere even if the thread finishes later.
    pub(crate) fn begin_review_list(&mut self, workdir: &Path, resolve_user: bool) -> u64 {
        self.review_list_seq += 1;
        let request_id = self.review_list_seq;
        let (tx, rx) = std::sync::mpsc::channel();
        self.review_list = Some(rx);
        let loader = self.review_list_loader;
        let workdir = workdir.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(ReviewListFetch {
                request_id,
                loaded: loader(&workdir, resolve_user),
            });
        });
        request_id
    }

    pub(crate) fn review_list_pending(&self) -> bool {
        self.review_list.is_some()
    }

    pub(crate) fn poll_review_list(&self) -> Option<Result<ReviewListFetch, TryRecvError>> {
        self.review_list.as_ref().map(Receiver::try_recv)
    }

    pub(crate) fn cancel_review_list(&mut self) {
        self.review_list = None;
    }

    /// Start opening `pr` for review on a worker thread and return the
    /// request id. Abandons any open already in flight.
    pub(crate) fn begin_review_open(
        &mut self,
        workdir: &Path,
        pr: &crate::github::ReviewablePr,
    ) -> u64 {
        self.review_open_seq += 1;
        let request_id = self.review_open_seq;
        let (tx, rx) = std::sync::mpsc::channel();
        self.review_open = Some(rx);
        let opener = self.review_opener;
        let workdir = workdir.to_path_buf();
        let pr = pr.clone();
        std::thread::spawn(move || {
            let _ = tx.send(ReviewOpenFetch {
                request_id,
                target: opener(&workdir, &pr),
            });
        });
        request_id
    }

    pub(crate) fn review_open_pending(&self) -> bool {
        self.review_open.is_some()
    }

    pub(crate) fn poll_review_open(&self) -> Option<Result<ReviewOpenFetch, TryRecvError>> {
        self.review_open.as_ref().map(Receiver::try_recv)
    }

    pub(crate) fn cancel_review_open(&mut self) {
        self.review_open = None;
    }

    /// Post a PR review on a worker thread, returning its request id.
    pub(crate) fn begin_review_post(&mut self, request: PrPostRequest) -> u64 {
        self.review_post_seq += 1;
        let request_id = self.review_post_seq;
        let (tx, rx) = std::sync::mpsc::channel();
        self.review_post = Some(rx);
        let poster = self.review_poster;
        std::thread::spawn(move || {
            let outcome = poster(&request);
            let _ = tx.send(ReviewPostFetch {
                request_id,
                request,
                outcome,
            });
        });
        request_id
    }

    pub(crate) fn review_post_pending(&self) -> bool {
        self.review_post.is_some()
    }

    pub(crate) fn poll_review_post(&self) -> Option<Result<ReviewPostFetch, TryRecvError>> {
        self.review_post.as_ref().map(Receiver::try_recv)
    }

    pub(crate) fn finish_review_post(&mut self) {
        self.review_post = None;
    }

    #[cfg(test)]
    pub(crate) fn set_review_poster_for_test(&mut self, poster: ReviewPoster) {
        self.review_poster = poster;
    }

    #[cfg(test)]
    pub(crate) fn set_review_opener_for_test(&mut self, opener: ReviewOpener) {
        self.review_opener = opener;
    }

    #[cfg(test)]
    pub(crate) fn set_review_list_loader_for_test(&mut self, loader: ReviewListLoader) {
        self.review_list_loader = loader;
    }

    pub(crate) fn begin_fetch(&mut self, receiver: Receiver<anyhow::Result<PrReview>>) {
        self.fetch = Some(receiver);
    }

    pub(crate) fn fetch_pending(&self) -> bool {
        self.fetch.is_some()
    }

    pub(crate) fn poll_fetch(&self) -> Option<Result<anyhow::Result<PrReview>, TryRecvError>> {
        self.fetch.as_ref().map(Receiver::try_recv)
    }

    pub(crate) fn cancel_fetch(&mut self) {
        self.fetch = None;
    }

    pub(crate) fn begin_investigation(&mut self, receiver: Receiver<InvestigationOutcome>) {
        self.investigation = Some(receiver);
    }

    pub(crate) fn investigation_pending(&self) -> bool {
        self.investigation.is_some()
    }

    pub(crate) fn poll_investigation(&self) -> Option<Result<InvestigationOutcome, TryRecvError>> {
        self.investigation.as_ref().map(Receiver::try_recv)
    }

    pub(crate) fn cancel_investigation(&mut self) {
        self.investigation = None;
    }
}

/// The pending origin and live progress outlive the running dialog. Finishing
/// or invalidating a run clears all three together; merely closing a pane does not.
#[derive(Default)]
pub(crate) struct AiReviewRun {
    receiver: Option<Receiver<AiReviewProgress>>,
    origin: Option<AiReviewState>,
    progress: Option<AiReviewRunProgress>,
}

impl AiReviewRun {
    pub(crate) fn begin(&mut self, receiver: Receiver<AiReviewProgress>, origin: AiReviewState) {
        self.receiver = Some(receiver);
        self.origin = Some(origin);
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.receiver.is_some()
    }

    pub(crate) fn poll(&self) -> Result<AiReviewProgress, TryRecvError> {
        self.receiver
            .as_ref()
            .map_or(Err(TryRecvError::Disconnected), Receiver::try_recv)
    }

    pub(crate) fn origin(&self) -> &Option<AiReviewState> {
        &self.origin
    }

    pub(crate) fn progress(&self) -> &Option<AiReviewRunProgress> {
        &self.progress
    }

    pub(crate) fn progress_mut(&mut self) -> Option<&mut AiReviewRunProgress> {
        self.progress.as_mut()
    }

    pub(crate) fn show_progress(&mut self, progress: AiReviewRunProgress) {
        self.progress = Some(progress);
    }

    pub(crate) fn finish(&mut self) -> Option<AiReviewState> {
        self.receiver = None;
        self.progress = None;
        self.origin.take()
    }

    // A few existing regressions intentionally seed incomplete/disconnected
    // runs. Keep that injection test-only, outside the production lifecycle API.
    #[cfg(test)]
    pub(crate) fn set_receiver_for_test(&mut self, receiver: Option<Receiver<AiReviewProgress>>) {
        self.receiver = receiver;
    }

    #[cfg(test)]
    pub(crate) fn set_origin_for_test(&mut self, origin: Option<AiReviewState>) {
        self.origin = origin;
    }

    #[cfg(test)]
    pub(crate) fn set_progress_for_test(&mut self, progress: Option<AiReviewRunProgress>) {
        self.progress = progress;
    }
}
