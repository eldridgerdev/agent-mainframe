//! The PR picker's **Review** tab: list every open PR in the repository so one
//! can be opened for a manual review. Opening and reviewing are wired in
//! later; this module owns the list, its background load, and switching
//! between the Triage and Review tabs.
//!
//! Nothing here runs `gh` on the UI thread. The Triage tab keeps its existing
//! (synchronous) behavior; this tab only ever hands it back.

use std::path::PathBuf;

use crate::app::state::{
    AppMode, DiffScope, DiffViewerState, PrPickerState, PrReviewDraftBadge, PrReviewListError,
    PrReviewListLoad, PrReviewListState, PrReviewOpening, ViewState,
};
use crate::app::{App, Selection};
use crate::db::pr_review_drafts::PrReviewDraftStatus;
use crate::project::{SessionKind, VibeMode};

impl App {
    /// Open the Review tab and start loading its list. `triage` is the Triage
    /// tab being switched away from, if any, kept so `Tab` can restore it.
    pub(crate) fn open_pr_review_list(&mut self, workdir: PathBuf, triage: Option<PrPickerState>) {
        let current_user = self.gh_current_user.clone().flatten();
        let resolve_user = self.gh_current_user.is_none();
        let request_id = self
            .pr_review_work
            .begin_review_list(&workdir, resolve_user);
        self.mode = AppMode::PrReviewList(Box::new(PrReviewListState {
            workdir,
            load: PrReviewListLoad::Loading,
            selected: 0,
            reloading: false,
            request_id,
            triage,
            current_user,
            opening: None,
            open_error: None,
            drafts: Default::default(),
        }));
    }

    /// Dashboard `G` on a project row: there is no feature, so there is no
    /// branch PR to triage. Go straight to the Review tab for the project's
    /// repository.
    pub(crate) fn open_pr_review_list_for_selected_project(&mut self) -> bool {
        let Selection::Project(pi) = self.selection else {
            return false;
        };
        let Some(repo) = self.store.projects.get(pi).map(|p| p.repo.clone()) else {
            return false;
        };
        self.open_pr_review_list(repo, None);
        true
    }

    /// `Tab` on the Triage tab.
    pub fn pr_picker_switch_to_review_tab(&mut self) {
        let picker = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::PrPicker(picker) => picker,
            other => {
                self.mode = other;
                return;
            }
        };
        let workdir = picker.workdir.clone();
        self.open_pr_review_list(workdir, Some(picker));
    }

    /// `Tab` on the Review tab. Restores the Triage picker it came from, or
    /// opens Triage fresh (its usual synchronous load) when there was none.
    pub fn pr_review_list_switch_to_triage_tab(&mut self) {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::PrReviewList(state) => state,
            other => {
                self.mode = other;
                return;
            }
        };
        self.pr_review_work.cancel_review_list();
        match state.triage {
            Some(picker) => self.mode = AppMode::PrPicker(picker),
            None => self.open_pr_picker(state.workdir, None),
        }
    }

    /// `r`: reload the list. Allowed from any state, so a stuck load can be
    /// restarted too; the abandoned load's result is dropped by request id.
    pub fn pr_review_list_retry(&mut self) {
        let AppMode::PrReviewList(state) = &self.mode else {
            return;
        };
        let workdir = state.workdir.clone();
        let resolve_user = self.gh_current_user.is_none();
        let request_id = self
            .pr_review_work
            .begin_review_list(&workdir, resolve_user);
        if let AppMode::PrReviewList(state) = &mut self.mode {
            state.request_id = request_id;
            if matches!(state.load, PrReviewListLoad::Loaded(_)) {
                state.reloading = true;
            } else {
                state.load = PrReviewListLoad::Loading;
            }
        }
    }

    /// `Esc`/`q`: abandon a PR that is still opening, else close the tab.
    pub fn close_pr_review_list(&mut self) {
        if let AppMode::PrReviewList(state) = &mut self.mode
            && let Some(opening) = state.opening.take()
        {
            self.pr_review_work.cancel_review_open();
            self.message = Some(format!("Stopped opening PR #{}", opening.number));
            return;
        }
        self.pr_review_work.cancel_review_list();
        self.pr_review_work.cancel_review_open();
        self.mode = AppMode::Normal;
    }

    /// `Enter`: fetch the highlighted PR's revisions on a worker thread. The
    /// tab stays usable meanwhile; the viewer opens when the fetch lands.
    pub fn pr_review_list_open_selected(&mut self) {
        let AppMode::PrReviewList(state) = &self.mode else {
            return;
        };
        if state.opening.is_some() {
            return;
        }
        let PrReviewListLoad::Loaded(prs) = &state.load else {
            return;
        };
        let Some(pr) = prs.get(state.selected).cloned() else {
            return;
        };
        let workdir = state.workdir.clone();
        let request_id = self.pr_review_work.begin_review_open(&workdir, &pr);
        if let AppMode::PrReviewList(state) = &mut self.mode {
            state.open_error = None;
            state.opening = Some(PrReviewOpening {
                request_id,
                number: pr.number,
            });
        }
    }

    /// Apply a finished open. On success the Review tab, cursor and all, is
    /// stashed in the viewer and restored verbatim when the review closes.
    pub fn poll_pr_review_open_bg(&mut self) -> bool {
        let Some(polled) = self.pr_review_work.poll_review_open() else {
            return false;
        };
        let fetch = match polled {
            Ok(fetch) => fetch,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pr_review_work.cancel_review_open();
                let AppMode::PrReviewList(state) = &mut self.mode else {
                    return false;
                };
                if state.opening.take().is_none() {
                    return false;
                }
                state.open_error = Some("the worker opening the PR stopped unexpectedly".into());
                return true;
            }
        };
        self.pr_review_work.cancel_review_open();

        let AppMode::PrReviewList(state) = &mut self.mode else {
            return false;
        };
        if state.opening.as_ref().map(|o| o.request_id) != Some(fetch.request_id) {
            return false;
        }
        state.opening = None;
        let target = match fetch.target {
            Ok(target) => target,
            Err(err) => {
                let message = format!("{err:#}");
                state.open_error = Some(message.clone());
                self.log_warn(
                    "pr_review",
                    format!("could not open PR for review: {message}"),
                );
                return true;
            }
        };

        let origin = std::mem::replace(&mut self.mode, AppMode::Normal);
        let AppMode::PrReviewList(list) = &origin else {
            unreachable!("checked above");
        };
        let mut viewer = DiffViewerState::new(pr_review_placeholder_view(), list.workdir.clone());
        viewer.scope = DiffScope::PullRequest(Box::new(target));
        viewer.review = true;
        viewer.layout = self.preferred_diff_viewer_layout();
        viewer.return_to = Some(Box::new(origin));
        self.mode = AppMode::DiffViewerLoading(viewer);
        true
    }

    /// Leave a PR review for the mode it was opened from, dropping its
    /// private refs off the UI thread. Reopening fetches them again, which
    /// only transfers what changed.
    pub(crate) fn exit_pr_review(&mut self, state: DiffViewerState) {
        if let DiffScope::PullRequest(target) = &state.scope {
            let workdir = state.workdir.clone();
            let number = target.pr.number;
            std::thread::spawn(move || {
                let _ = super::revisions::remove_review_refs(&workdir, number);
            });
        }
        self.mode = match state.return_to {
            Some(origin) => *origin,
            None => AppMode::Normal,
        };
    }

    pub fn pr_review_list_select_next(&mut self) {
        if let AppMode::PrReviewList(state) = &mut self.mode
            && let PrReviewListLoad::Loaded(prs) = &state.load
            && !prs.is_empty()
        {
            state.selected = (state.selected + 1).min(prs.len() - 1);
        }
    }

    pub fn pr_review_list_select_prev(&mut self) {
        if let AppMode::PrReviewList(state) = &mut self.mode {
            state.selected = state.selected.saturating_sub(1);
        }
    }

    /// Apply a finished list load. Returns `true` when the screen changed.
    pub fn poll_pr_review_list_bg(&mut self) -> bool {
        let Some(polled) = self.pr_review_work.poll_review_list() else {
            return false;
        };
        let fetch = match polled {
            Ok(fetch) => fetch,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pr_review_work.cancel_review_list();
                return self.fail_pending_pr_review_list_load(
                    "the PR list worker stopped unexpectedly".to_string(),
                );
            }
        };
        self.pr_review_work.cancel_review_list();
        let loaded = fetch.loaded;

        if loaded.prs.is_ok() && self.gh_current_user.is_none() {
            // Cached either way, as `resolve_gh_current_user` does, so a
            // failed lookup isn't repeated on every load.
            self.gh_current_user = Some(loaded.current_user.clone());
        }
        let current_user = self.gh_current_user.clone().flatten();
        let drafts = self.pr_review_draft_badges(loaded.repo_key.as_deref());

        let AppMode::PrReviewList(state) = &mut self.mode else {
            return false;
        };
        if state.request_id != fetch.request_id {
            return false;
        }
        state.reloading = false;
        match loaded.prs {
            Ok(prs) => {
                let previous = match &state.load {
                    PrReviewListLoad::Loaded(old) => old.get(state.selected).map(|p| p.number),
                    _ => None,
                };
                // Keep the highlight on the same PR across a reload.
                state.selected = previous
                    .and_then(|n| prs.iter().position(|p| p.number == n))
                    .unwrap_or(0);
                state.load = PrReviewListLoad::Loaded(prs);
                state.current_user = current_user;
                state.drafts = drafts;
            }
            Err(err) => {
                let detail = err.to_string();
                self.log_warn("pr_review", format!("review list failed: {detail}"));
                if let AppMode::PrReviewList(state) = &mut self.mode {
                    state.load = PrReviewListLoad::Failed(PrReviewListError::classify(detail));
                }
            }
        }
        true
    }

    /// Badges for every draft saved in `repo_key`'s repository. Posted
    /// reviews get none: there is nothing left to resume.
    fn pr_review_draft_badges(
        &mut self,
        repo_key: Option<&str>,
    ) -> std::collections::HashMap<u32, PrReviewDraftBadge> {
        let (Some(db), Some(repo_key)) = (&self.db, repo_key) else {
            return Default::default();
        };
        match db.load_pr_review_drafts_for_repo(repo_key) {
            Ok(drafts) => drafts
                .into_iter()
                .filter(|d| d.status == PrReviewDraftStatus::Draft)
                .map(|d| {
                    (
                        d.pr_number,
                        PrReviewDraftBadge {
                            comments: crate::app::review::draft_comment_count(&d.progress),
                            head_oid: d.head_oid,
                        },
                    )
                })
                .collect(),
            Err(err) => {
                self.log_warn("pr_review", format!("could not load review drafts: {err}"));
                Default::default()
            }
        }
    }

    /// Show a failure on the tab if it is still waiting on a load, so a dead
    /// worker never leaves it on "Loading…" forever.
    fn fail_pending_pr_review_list_load(&mut self, detail: String) -> bool {
        let AppMode::PrReviewList(state) = &mut self.mode else {
            return false;
        };
        if !matches!(state.load, PrReviewListLoad::Loading) {
            return false;
        }
        state.load = PrReviewListLoad::Failed(PrReviewListError::classify(detail));
        true
    }
}

/// The `from_view` a PR review's viewer carries without a session behind it.
/// Never read: see [`DiffViewerState::from_view`].
fn pr_review_placeholder_view() -> ViewState {
    ViewState::new(
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        "PR review".to_string(),
        SessionKind::Terminal,
        VibeMode::default(),
        true,
    )
}
