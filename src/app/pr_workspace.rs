//! Experimental navigation shell over the existing PR engines.
//!
//! AppMode stays the handler/worker boundary. Exactly one engine is installed
//! there; inactive engines are owned by this visit, never cloned into a second
//! source of truth. Polling temporarily installs parked engines and restores
//! the visible mode before returning to the event loop.

use std::{collections::HashMap, path::PathBuf};

use super::{App, AppMode, DiffScope, DiffViewerState, Selection};
use crate::{diff::DiffSide, github::PrRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum WorkspaceTab {
    Files,
    Threads,
    Ai,
}

impl WorkspaceTab {
    pub(crate) const ALL: [Self; 3] = [Self::Files, Self::Threads, Self::Ai];
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Files => 0,
            Self::Threads => 1,
            Self::Ai => 2,
        }
    }
    pub(crate) fn for_mode(mode: &AppMode) -> Option<Self> {
        match mode {
            AppMode::DiffViewer(_) | AppMode::DiffViewerLoading(_) => Some(Self::Files),
            AppMode::PrReview(_)
            | AppMode::PrReviewLoading(_)
            | AppMode::PrInvestigationLoading(_) => Some(Self::Threads),
            AppMode::AiReview(_) | AppMode::AiReviewRunning(_) => Some(Self::Ai),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum WorkspacePane {
    Navigator,
    Diff,
    Context,
}

impl WorkspacePane {
    pub(crate) fn next(self, reverse: bool) -> Self {
        match (self, reverse) {
            (Self::Navigator, false) | (Self::Context, true) => Self::Diff,
            (Self::Diff, false) | (Self::Navigator, true) => Self::Context,
            _ => Self::Navigator,
        }
    }
}

pub(crate) struct PrWorkspace {
    pub(crate) workdir: PathBuf,
    pub(crate) pr: Option<PrRef>,
    pub(crate) tab: WorkspaceTab,
    pub(crate) focus: WorkspacePane,
    pub(crate) expanded: bool,
    pub(crate) help: bool,
    pub(crate) help_scroll: u16,
    pub(crate) errors: [Option<String>; 3],
    pub(crate) anchor_message: Option<String>,
    pub(crate) parked: [Option<Box<AppMode>>; 3],
    origin: Box<AppMode>,
    // Manual reading position is independent of feedback jumps into the diff.
    reading_position: Option<(usize, usize, Option<usize>)>,
    last_anchor: Option<(WorkspaceTab, String)>,
    file_scroll: HashMap<String, usize>,
}

impl PrWorkspace {
    pub(crate) fn new(workdir: PathBuf, origin: AppMode) -> Self {
        Self {
            workdir,
            pr: None,
            tab: WorkspaceTab::Files,
            focus: WorkspacePane::Navigator,
            expanded: false,
            help: false,
            help_scroll: 0,
            errors: Default::default(),
            parked: Default::default(),
            origin: Box::new(origin),
            reading_position: None,
            last_anchor: None,
            anchor_message: None,
            file_scroll: HashMap::new(),
        }
    }

    pub(crate) fn mode<'a>(
        &'a self,
        tab: WorkspaceTab,
        active: &'a AppMode,
    ) -> Option<&'a AppMode> {
        if WorkspaceTab::for_mode(active) == Some(tab) {
            Some(active)
        } else {
            self.parked[tab.index()].as_deref()
        }
    }

    pub(crate) fn mode_mut<'a>(
        &'a mut self,
        tab: WorkspaceTab,
        active: &'a mut AppMode,
    ) -> Option<&'a mut AppMode> {
        if WorkspaceTab::for_mode(active) == Some(tab) {
            Some(active)
        } else {
            self.parked[tab.index()].as_deref_mut()
        }
    }

    pub(crate) fn files<'a>(&'a self, active: &'a AppMode) -> Option<&'a DiffViewerState> {
        match self.mode(WorkspaceTab::Files, active)? {
            AppMode::DiffViewer(state) | AppMode::DiffViewerLoading(state) => Some(state),
            _ => None,
        }
    }

    /// A suspended workspace must never wrap an unrelated legacy review.
    pub(crate) fn owns_mode(&self, mode: &AppMode) -> bool {
        let Some(expected) = &self.pr else {
            return true;
        };
        let matches = |workdir: &std::path::Path, pr: &PrRef| {
            workdir == self.workdir
                && pr.number == expected.number
                && pr.url.eq_ignore_ascii_case(&expected.url)
        };
        match mode {
            AppMode::DiffViewer(state) | AppMode::DiffViewerLoading(state) => match &state.scope {
                DiffScope::PullRequest(target) => {
                    super::review::pr_ref_for(target).is_some_and(|pr| matches(&state.workdir, &pr))
                }
                _ => false,
            },
            AppMode::PrReview(state) => matches(&state.workdir, &state.review.pr),
            AppMode::PrReviewLoading(state) => matches(&state.workdir, &state.pr),
            AppMode::PrInvestigationLoading(state) => {
                matches(&state.review.workdir, &state.review.review.pr)
            }
            AppMode::AiReview(state) => matches(&state.workdir, &state.pr),
            AppMode::AiReviewRunning(state) => matches(&state.origin.workdir, &state.origin.pr),
            _ => true,
        }
    }
    pub(crate) fn observed_head_moved(&self, active: &AppMode) -> bool {
        match (&self.pr, self.mode(WorkspaceTab::Threads, active)) {
            (Some(pr), Some(AppMode::PrReview(state))) => state.review.pr.head_sha != pr.head_sha,
            _ => false,
        }
    }
}

/// Text inputs and existing overlays own all keys before workspace shortcuts.
pub(crate) fn engine_modal(mode: &AppMode) -> bool {
    match mode {
        AppMode::DiffViewer(s) => {
            s.questions.open
                || s.editing_base_ref
                || s.editing_search
                || s.feedback_editing
                || s.editing_general
                || s.editing_line_comment
                || s.editing_file_comment
                || s.editing_suggestion
                || s.pr_submit.is_some()
                || s.help_open
                || s.review_history.is_some()
                || s.interdiff_open
                || s.changeset_overview_open
                || s.summary_open
                || s.destination_pick.is_some()
                || s.review_feature_setup.is_some()
                || s.finish_confirm
        }
        AppMode::PrReview(s) => {
            s.harness_pick.is_some()
                || s.new_feature_setup.is_some()
                || s.integrate.is_some()
                || s.fix_confirm.is_some()
                || s.mark_pick.is_some()
                || s.reply_kind_pick.is_some()
                || s.reply.is_some()
                || s.memory_add.is_some()
                || s.investigation_harness_pick.is_some()
                || s.investigation_action_pick.is_some()
                || s.investigation_follow_up.is_some()
                || s.investigation_context.editor.is_some()
        }
        AppMode::AiReview(s) => {
            s.questions.open
                || s.harness_pick.is_some()
                || s.model_pick.is_some()
                || s.finding_editor.is_some()
                || s.post_confirm.is_some()
        }
        _ => false,
    }
}

impl App {
    /// Default G / leader+G entry; leader+U keeps the legacy workflow available.
    pub(crate) fn open_pr_workspace(&mut self) {
        if let Some(workspace) = &mut self.pr_workspace
            && workspace.pr.is_some()
            && matches!(self.mode, AppMode::Viewing(_))
            && let Some(files) = workspace.parked[WorkspaceTab::Files.index()].take()
        {
            if self.pr_review_return.as_ref().is_some_and(|stash| {
                stash.state.workdir == workspace.workdir
                    && workspace.pr.as_ref().is_some_and(|pr| {
                        stash.state.review.pr.number == pr.number
                            && stash.state.review.pr.url.eq_ignore_ascii_case(&pr.url)
                    })
            }) && let Some(stash) = self.pr_review_return.take()
            {
                workspace.parked[WorkspaceTab::Threads.index()] =
                    Some(Box::new(AppMode::PrReview(stash.state)));
            }
            self.mode = *files;
            workspace.tab = WorkspaceTab::Files;
            workspace.focus = WorkspacePane::Navigator;
            return;
        }
        if self.pr_workspace.is_some() && !self.close_pr_workspace() {
            return;
        }
        let workdir = match &self.mode {
            AppMode::Viewing(view) => self.feature_for_view(view).map(|f| f.workdir.clone()),
            AppMode::Normal => match self.selection {
                Selection::Project(pi) => self.store.projects.get(pi).map(|p| p.repo.clone()),
                _ => self.selected_feature().map(|(_, f)| f.workdir.clone()),
            },
            _ => None,
        };
        let Some(workdir) = workdir else {
            self.message = Some("Select a project or feature to open PR Workspace".into());
            return;
        };
        let origin = std::mem::replace(&mut self.mode, AppMode::Normal);
        self.pr_workspace = Some(PrWorkspace::new(workdir.clone(), origin));
        self.open_pr_review_list(workdir, None);
    }

    pub(crate) fn open_pr_workspace_for_workdir(&mut self, workdir: PathBuf) {
        let origin = std::mem::replace(&mut self.mode, AppMode::Normal);
        self.pr_workspace = Some(PrWorkspace::new(workdir.clone(), origin));
        self.open_pr_review_list(workdir, None);
    }

    /// Called after transitions as well as before background polling. Seeding
    /// uses the fetched PR identity, never the checkout's current branch.
    pub(crate) fn reconcile_pr_workspace(&mut self) -> bool {
        let Some(mut workspace) = self.pr_workspace.take() else {
            return false;
        };
        let mut changed = false;
        if !workspace.owns_mode(&self.mode) {
            self.pr_workspace = Some(workspace);
            return self.leave_pr_workspace_preserving_mode();
        }
        if workspace.pr.is_some() && matches!(self.mode, AppMode::Normal) {
            // Leaving the temporary fix-session view for the dashboard ends
            // this visit. A subsequent G/W must retain the legacy workflow.
            workspace.origin = Box::new(AppMode::Normal);
            self.pr_workspace = Some(workspace);
            return self.close_pr_workspace();
        }
        if workspace.pr.is_none()
            && let AppMode::DiffViewer(state) = &self.mode
            && let DiffScope::PullRequest(target) = &state.scope
            && let Some(pr) = super::review::pr_ref_for(target)
        {
            workspace.pr = Some(pr.clone());
            workspace.workdir = state.workdir.clone();
            let visible = std::mem::replace(&mut self.mode, AppMode::Normal);
            self.open_ai_review_for_pr(workspace.workdir.clone(), pr.clone());
            workspace.parked[WorkspaceTab::Ai.index()] =
                Some(Box::new(std::mem::replace(&mut self.mode, AppMode::Normal)));
            self.enter_pr_review(workspace.workdir.clone(), pr);
            workspace.parked[WorkspaceTab::Threads.index()] =
                Some(Box::new(std::mem::replace(&mut self.mode, visible)));
            changed = true;
        }
        if let Some(tab) = WorkspaceTab::for_mode(&self.mode) {
            workspace.tab = tab;
        }
        // A legacy close from an editor/run may return to the PR picker. Keep
        // the shell, but release the previous PR views before opening another.
        if workspace.pr.is_some() && matches!(self.mode, AppMode::PrReviewList(_)) {
            release_workspace_refs(&workspace, &self.mode);
            workspace.pr = None;
            workspace.parked = Default::default();
            workspace.last_anchor = None;
            workspace.reading_position = None;
            workspace.errors = Default::default();
            changed = true;
        }
        self.pr_workspace = Some(workspace);
        changed
    }

    /// Switch active engine without rebuilding it or disturbing its workers.
    pub(crate) fn switch_pr_workspace(&mut self, tab: WorkspaceTab) -> bool {
        let Some(workspace) = &mut self.pr_workspace else {
            return false;
        };
        if workspace.pr.is_none() || engine_modal(&self.mode) {
            return false;
        }
        if WorkspaceTab::for_mode(&self.mode) == Some(tab) {
            workspace.tab = tab;
            return true;
        }
        let Some(next) = workspace.parked[tab.index()].take() else {
            return false;
        };
        let previous = std::mem::replace(&mut self.mode, *next);
        if let Some(previous_tab) = WorkspaceTab::for_mode(&previous) {
            workspace.parked[previous_tab.index()] = Some(Box::new(previous));
        } else {
            // Suspended sessions/modal origins must not be silently discarded.
            workspace.parked[tab.index()] =
                Some(Box::new(std::mem::replace(&mut self.mode, previous)));
            return false;
        }
        workspace.tab = tab;
        true
    }

    pub(crate) fn browse_pr_workspace(&mut self, tab: WorkspaceTab) {
        let previous = self.pr_workspace.as_ref().map(|w| w.tab);
        if previous == Some(WorkspaceTab::Files)
            && tab != WorkspaceTab::Files
            && let (Some(workspace), AppMode::DiffViewer(state)) =
                (&mut self.pr_workspace, &self.mode)
        {
            workspace.reading_position = Some((
                state.selected_file,
                state.patch_scroll,
                state.comment_cursor,
            ));
        }
        if !self.switch_pr_workspace(tab) {
            return;
        }
        let workspace = self
            .pr_workspace
            .as_mut()
            .expect("switch requires workspace");
        workspace.focus = WorkspacePane::Navigator;
        workspace.last_anchor = None;
        if tab == WorkspaceTab::Files
            && let (Some((file, scroll, cursor)), AppMode::DiffViewer(state)) =
                (workspace.reading_position.take(), &mut self.mode)
        {
            state.selected_file = file.min(state.files.len().saturating_sub(1));
            state.patch_scroll = scroll;
            state.comment_cursor = cursor;
            state.cursor_sync_to_view = false;
        }
        if tab == WorkspaceTab::Files {
            workspace.anchor_message = None;
        }
        self.sync_pr_workspace_anchor();
    }

    /// Point the pinned diff at feedback's exact side/line. General/outdated
    /// feedback never borrows the previously selected item's code location.
    pub(crate) fn sync_pr_workspace_anchor(&mut self) {
        let Some(mut workspace) = self.pr_workspace.take() else {
            return;
        };
        let anchor = match &self.mode {
            AppMode::PrReview(state) => state.selected_comment().map(|c| {
                (
                    c.id.to_string(),
                    c.path.clone(),
                    c.line,
                    if c.side.as_deref() == Some("LEFT") {
                        Some(DiffSide::Old)
                    } else {
                        Some(DiffSide::New)
                    },
                    c.outdated,
                    state.review.pr.head_sha.clone(),
                )
            }),
            AppMode::AiReview(state) => state.findings.get(state.selected).map(|f| {
                (
                    state.selected.to_string(),
                    f.path.clone(),
                    f.line,
                    f.side,
                    false,
                    state.pr.head_sha.clone(),
                )
            }),
            _ => None,
        };
        if let Some((id, path, line, side, outdated, head)) = anchor {
            let key = (
                workspace.tab,
                format!("{id}:{path:?}:{line:?}:{side:?}:{outdated}:{head}"),
            );
            let head_moved = workspace.pr.as_ref().is_some_and(|pr| pr.head_sha != head);
            if workspace.last_anchor.as_ref() != Some(&key) {
                workspace.last_anchor = Some(key);
                workspace.anchor_message = if head_moved {
                    Some("Feedback is from a different PR head. Press F5 to reopen this PR at its latest head before comparing code.".into())
                } else if outdated {
                    Some("Outdated thread · its original location is no longer in this diff. Press 1 to browse the current code.".into())
                } else if path.is_none() {
                    Some(
                        "General PR feedback · no file location. Press 1 to browse changes.".into(),
                    )
                } else {
                    Some("This feedback's file is outside the pinned diff. Press 1 to browse changes.".into())
                };
                if let Some(files) = workspace.files(&self.mode)
                    && let Some(file) = files.files.get(files.selected_file)
                {
                    let (path, scroll) = (file.path.clone(), files.patch_scroll);
                    workspace.file_scroll.insert(path, scroll);
                }
                if let Some(AppMode::DiffViewer(files)) =
                    workspace.parked[WorkspaceTab::Files.index()].as_deref_mut()
                {
                    files.comment_cursor = None;
                    files.comment_anchor = None;
                    if !outdated
                        && !head_moved
                        && let Some(path) = path
                        && let Some(index) = files
                            .files
                            .iter()
                            .position(|f| f.path == path || f.old_path.as_ref() == Some(&path))
                    {
                        files.selected_file = index;
                        files.tree_cursor_dir = None;
                        files.reveal_selected_file();
                        files.patch_scroll = workspace
                            .file_scroll
                            .get(&files.files[index].path)
                            .copied()
                            .unwrap_or(0);
                        files.comment_cursor = line.and_then(|line| {
                            files.files[index]
                                .addressable_lines()
                                .iter()
                                .position(|loc| match side {
                                    Some(DiffSide::Old) => loc.old_line == Some(line as usize),
                                    _ => loc.new_line == Some(line as usize),
                                })
                        });
                        files.cursor_sync_to_view = files.comment_cursor.is_some();
                        workspace.anchor_message = if line.is_some()
                            && files.comment_cursor.is_none()
                        {
                            Some("The feedback's line is outside the pinned diff. Press 1 to browse the current file.".into())
                        } else {
                            None
                        };
                    }
                }
            }
        }
        self.pr_workspace = Some(workspace);
    }

    /// Reopen the same selected PR at fresh revisions, preserving its durable
    /// manual draft and using the existing opener's revision verification.
    pub(crate) fn reload_pr_workspace(&mut self) {
        if !self.switch_pr_workspace(WorkspaceTab::Files) {
            return;
        }
        if matches!(
            self.persist_pr_review_draft(),
            super::review::DraftSave::Failed(_)
        ) {
            return;
        }
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::DiffViewer(state) => state,
            other => {
                self.mode = other;
                return;
            }
        };
        self.exit_pr_review(state);
        if let Some(workspace) = &mut self.pr_workspace {
            workspace.pr = None;
            workspace.parked = Default::default();
            workspace.errors = Default::default();
            workspace.anchor_message = None;
            workspace.reading_position = None;
            workspace.last_anchor = None;
        }
        self.pr_review_work.cancel_fetch();
        self.pr_review_list_open_selected();
    }

    /// Poll every owned engine, including hidden views, before the legacy
    /// event-loop polls. No result can land in a different PR's visible mode.
    pub(crate) fn poll_pr_workspace_bg(&mut self) -> bool {
        let mut changed = self.reconcile_pr_workspace();
        let Some(mut workspace) = self.pr_workspace.take() else {
            return changed;
        };
        if workspace.pr.is_none() {
            self.pr_workspace = Some(workspace);
            return changed;
        }
        for tab in WorkspaceTab::ALL {
            let parked = if WorkspaceTab::for_mode(&self.mode) != Some(tab) {
                let Some(mode) = workspace.parked[tab.index()].take() else {
                    continue;
                };
                Some(std::mem::replace(&mut self.mode, *mode))
            } else {
                None
            };
            match tab {
                WorkspaceTab::Files => {
                    if self.diff_viewer_loading() {
                        self.complete_diff_viewer_loading();
                        changed = true;
                    }
                    if self.pr_review_work.review_post_pending() {
                        changed |= self.poll_pr_review_post_bg();
                    }
                    if let Err(error) = self
                        .poll_review_walkthrough()
                        .and_then(|_| self.poll_co_review())
                        .and_then(|_| self.poll_changeset_overview())
                    {
                        self.show_error(error);
                        changed = true;
                    }
                }
                WorkspaceTab::Threads => {
                    if self.pr_review_work.fetch_pending()
                        && let Some(result) = self.poll_pr_review_fetch()
                    {
                        workspace.errors[tab.index()] = result.err().map(|error| match error {
                            super::pr_review::PrFetchFailure::Failed(error) => format!("{error:#}"),
                            super::pr_review::PrFetchFailure::Disconnected => {
                                "Comment worker stopped unexpectedly".into()
                            }
                        });
                        changed = true;
                    }
                    if self.pr_review_work.investigation_pending() {
                        changed |= self.poll_pr_investigation_bg();
                    }
                    if self.ai_review_triage_refresh_bg.is_some() {
                        changed |= self.poll_ai_review_triage_refresh_bg();
                    }
                    if self.memory_ai_summary_bg.is_some() {
                        changed |= self.poll_memory_ai_summary_bg();
                    }
                }
                WorkspaceTab::Ai => {
                    if self.ai_review_run.is_pending() {
                        changed |= self.poll_ai_pr_review_bg();
                    }
                    if self.codex_models_cli_bg.is_some() {
                        changed |= self.poll_codex_models_cli_bg();
                    }
                }
            }
            let owns_question = self.review_question_work.job.as_ref().is_none_or(|job| {
                self.review_questions()
                    .is_some_and(|q| q.owner == job.owner && q.request == Some(job.request))
            });
            if owns_question {
                changed |= self.poll_review_questions();
            }
            if let Some(visible) = parked {
                workspace.parked[tab.index()] =
                    Some(Box::new(std::mem::replace(&mut self.mode, visible)));
            }
        }
        self.pr_workspace = Some(workspace);
        changed |= self.reconcile_pr_workspace();
        if changed {
            self.sync_pr_workspace_anchor();
        }
        changed
    }

    pub(crate) fn pr_workspace_owns_engines(&self) -> bool {
        self.pr_workspace
            .as_ref()
            .is_some_and(|workspace| workspace.pr.is_some())
    }

    pub(crate) fn retry_workspace_comments(&mut self) {
        let Some(workspace) = &mut self.pr_workspace else {
            return;
        };
        let Some(pr) = workspace.pr.clone() else {
            return;
        };
        workspace.errors[WorkspaceTab::Threads.index()] = None;
        let workdir = workspace.workdir.clone();
        self.start_pr_review_fetch(workdir, pr);
    }

    /// Persist manual drafts before leaving. A failed save keeps the workspace
    /// open, so an unsuccessful save never silently discards the draft.
    pub(crate) fn close_pr_workspace(&mut self) -> bool {
        let Some(mut workspace) = self.pr_workspace.take() else {
            return true;
        };
        if let Some(files) = workspace.parked[WorkspaceTab::Files.index()].take() {
            let visible = std::mem::replace(&mut self.mode, *files);
            let save = self.persist_workspace_draft();
            workspace.parked[WorkspaceTab::Files.index()] =
                Some(Box::new(std::mem::replace(&mut self.mode, visible)));
            if matches!(save, super::review::DraftSave::Failed(_)) {
                self.pr_workspace = Some(workspace);
                return false;
            }
        } else if matches!(
            self.mode,
            AppMode::DiffViewer(_) | AppMode::DiffViewerLoading(_)
        ) && matches!(
            self.persist_workspace_draft(),
            super::review::DraftSave::Failed(_)
        ) {
            self.pr_workspace = Some(workspace);
            return false;
        }
        release_workspace_refs(&workspace, &self.mode);
        self.pr_review_work.cancel_review_list();
        self.pr_review_work.cancel_review_open();
        self.pr_review_work.cancel_fetch();
        self.mode = *workspace.origin;
        true
    }

    fn persist_workspace_draft(&mut self) -> super::review::DraftSave {
        let loading = matches!(self.mode, AppMode::DiffViewerLoading(_));
        if loading {
            let AppMode::DiffViewerLoading(state) =
                std::mem::replace(&mut self.mode, AppMode::Normal)
            else {
                unreachable!()
            };
            self.mode = AppMode::DiffViewer(state);
        }
        let saved = self.persist_pr_review_draft();
        if loading {
            let AppMode::DiffViewer(state) = std::mem::replace(&mut self.mode, AppMode::Normal)
            else {
                unreachable!()
            };
            self.mode = AppMode::DiffViewerLoading(state);
        }
        saved
    }

    pub(crate) fn leave_pr_workspace_preserving_mode(&mut self) -> bool {
        let visible = std::mem::replace(&mut self.mode, AppMode::Normal);
        let closed = self.close_pr_workspace();
        self.mode = visible;
        closed
    }
}

fn release_workspace_refs(workspace: &PrWorkspace, active: &AppMode) {
    if let Some(files) = workspace.files(active)
        && let DiffScope::PullRequest(target) = &files.scope
    {
        super::pr_review::revisions::remove_review_refs_in_background(
            files.workdir.clone(),
            target.pr.number,
        );
    }
}
