//! GUI PR Triage adapter: pick a pull request, browse its review comments and
//! threads, run read-only investigations, reply and resolve threads.
//!
//! Every step drives the shared PR Triage engines (`app::pr_review`) on the
//! GUI's private App, addressed by a stable workflow id and revision. GitHub
//! writes and headless calls are two-step: a request records exactly what
//! would be sent, and only an explicit confirmation of that pending step —
//! rechecked against GitHub and the open review — performs it.
//!
//! Each step's `gh` reads run without the GUI lock: [`plan_begin`],
//! [`plan_poll`] and [`plan_act`] name them under the lock,
//! [`PrTriageReads::run`] performs them without it, and the matching
//! `*_prefetched` call applies the results under the lock again. GitHub
//! writes stay under the lock, after their rechecks, so nothing can change
//! between the last check and the write.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::app::pr_review::github_access::TriageGithub;
use crate::app::pr_review::{
    CommentKind, INVESTIGATION_CONTEXT_MAX_LEN, PrComment, PrFetchFailure, PrInvestigationStatus,
    PrSortMode, ReplyKind, ReplyTarget, investigation_prompt_from_meta, strip_bot_boilerplate,
};
use crate::app::toast::ToastKind;
use crate::app::{App, AppMode, PendingFollowUp, PrPickerState, PrReviewState};
use crate::editor::TextEditor;
use crate::github::{PrListEntry, PrMeta, PrRef, PrResolution, ReviewThread};
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};
use crate::gui_plans::PrecallView;
use crate::project::AgentKind;

/// The title the pre-call notice shows for a PR investigation.
const INVESTIGATION_CALL_TITLE: &str = "PR Triage: read-only investigation";

pub(crate) struct PrTriageContext {
    id: String,
    target: FeatureTarget,
    revision: u64,
    workdir: PathBuf,
    branch_pr: Option<u32>,
    current_user: Option<String>,
    /// The pull request list's last filter, kept so returning to the list
    /// (back, or after a failed comment fetch) shows the same pull requests.
    include_closed: bool,
    /// The list is open but not read yet; the next poll reads it.
    list_pending: bool,
    error: Option<String>,
    notice: Option<String>,
    investigation: Option<PendingInvestigation>,
    write: Option<PendingWrite>,
}

/// The GitHub reads one step needs, named under the GUI lock and performed by
/// [`Self::run`] without it.
pub struct PrTriageReads {
    github: Option<Arc<dyn TriageGithub>>,
    workdir: PathBuf,
    plan: ReadPlan,
}

#[derive(Default)]
struct ReadPlan {
    branch_pr: bool,
    current_user: bool,
    list: Option<bool>,
    pr: Option<u32>,
    meta: Option<u32>,
    threads: Option<PrRef>,
}

impl PrTriageReads {
    fn none() -> Self {
        Self {
            github: None,
            workdir: PathBuf::new(),
            plan: ReadPlan::default(),
        }
    }

    fn new(app: &App, workdir: PathBuf, plan: ReadPlan) -> Self {
        Self {
            github: Some(app.pr_review_work.github()),
            workdir,
            plan,
        }
    }

    /// Perform the planned reads. Blocking: call it without the GUI lock.
    pub fn run(self) -> PrTriagePrefetch {
        let Some(github) = self.github else {
            return PrTriagePrefetch::default();
        };
        let (workdir, plan) = (&self.workdir, self.plan);
        PrTriagePrefetch {
            github: None,
            branch_pr: plan.branch_pr.then(|| github.resolve_pr(workdir)),
            current_user: plan.current_user.then(|| github.current_user(workdir)),
            list: plan.list.map(|c| (c, github.list_prs(workdir, c))),
            pr: plan.pr.map(|n| (n, github.fetch_pr_by_number(workdir, n))),
            meta: plan.meta.map(|n| (n, github.pr_meta(workdir, n))),
            threads: plan
                .threads
                .map(|pr| (pr.number, github.review_threads(workdir, &pr))),
            workdir: Some(self.workdir),
        }
    }
}

/// The results of [`PrTriageReads::run`]. A step takes each result it
/// planned; anything it reads that was not prefetched (or was read for
/// another checkout or pull request) is read live, so a plan that misses a
/// read costs a blocking call, never a stale answer.
#[derive(Default)]
pub struct PrTriagePrefetch {
    github: Option<Arc<dyn TriageGithub>>,
    workdir: Option<PathBuf>,
    branch_pr: Option<anyhow::Result<PrResolution>>,
    current_user: Option<anyhow::Result<String>>,
    list: Option<(bool, anyhow::Result<Vec<PrListEntry>>)>,
    pr: Option<(u32, anyhow::Result<PrRef>)>,
    meta: Option<(u32, anyhow::Result<PrMeta>)>,
    threads: Option<(u32, anyhow::Result<Vec<ReviewThread>>)>,
}

fn take_for<K: PartialEq, T>(slot: &mut Option<(K, T)>, key: K) -> Option<T> {
    slot.take().filter(|(k, _)| *k == key).map(|(_, v)| v)
}

impl PrTriagePrefetch {
    /// Attach the App's GitHub for live reads; every `*_prefetched` entry
    /// point calls this first.
    fn attach(&mut self, gui: &mut GuiHandle) {
        self.github = Some(gui.app_for_workflow().pr_review_work.github());
    }

    fn live(&self) -> &dyn TriageGithub {
        self.github
            .as_deref()
            .expect("prefetch attached to the App")
    }

    fn fresh(&self, workdir: &Path) -> bool {
        self.workdir.as_deref() == Some(workdir)
    }

    fn resolve_pr(&mut self, workdir: &Path) -> anyhow::Result<PrResolution> {
        match self.branch_pr.take().filter(|_| self.fresh(workdir)) {
            Some(result) => result,
            None => self.live().resolve_pr(workdir),
        }
    }

    fn current_user(&mut self, workdir: &Path) -> anyhow::Result<String> {
        match self.current_user.take().filter(|_| self.fresh(workdir)) {
            Some(result) => result,
            None => self.live().current_user(workdir),
        }
    }

    /// The prefetched list only: `None` when it was not read.
    fn prefetched_list(
        &mut self,
        workdir: &Path,
        include_closed: bool,
    ) -> Option<anyhow::Result<Vec<PrListEntry>>> {
        take_for(&mut self.list, include_closed).filter(|_| self.fresh(workdir))
    }

    fn list_prs(
        &mut self,
        workdir: &Path,
        include_closed: bool,
    ) -> anyhow::Result<Vec<PrListEntry>> {
        match self.prefetched_list(workdir, include_closed) {
            Some(result) => result,
            None => self.live().list_prs(workdir, include_closed),
        }
    }

    fn fetch_pr_by_number(&mut self, workdir: &Path, number: u32) -> anyhow::Result<PrRef> {
        match take_for(&mut self.pr, number).filter(|_| self.fresh(workdir)) {
            Some(result) => result,
            None => self.live().fetch_pr_by_number(workdir, number),
        }
    }

    fn pr_meta(&mut self, workdir: &Path, number: u32) -> anyhow::Result<PrMeta> {
        match take_for(&mut self.meta, number).filter(|_| self.fresh(workdir)) {
            Some(result) => result,
            None => self.live().pr_meta(workdir, number),
        }
    }

    fn review_threads(&mut self, workdir: &Path, pr: &PrRef) -> anyhow::Result<Vec<ReviewThread>> {
        match take_for(&mut self.threads, pr.number).filter(|_| self.fresh(workdir)) {
            Some(result) => result,
            None => self.live().review_threads(workdir, pr),
        }
    }
}

/// An investigation waiting for its pre-call confirmation. `prompt` is the
/// exact text the run will send.
struct PendingInvestigation {
    comment_id: u64,
    harness: AgentKind,
    follow_up: Option<String>,
    note: Option<String>,
    head_sha: String,
    prompt: String,
    viewing: bool,
}

/// A GitHub write waiting for explicit confirmation.
enum PendingWrite {
    Reply {
        comment_id: u64,
        kind: ReplyKind,
        body: String,
        posted: String,
        head_sha: String,
    },
    Resolve {
        comment_id: u64,
        thread_id: String,
        resolve: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct PrEntryView {
    pub number: u32,
    pub title: String,
    pub author: String,
    pub head_ref: String,
    pub updated_at: String,
    pub is_draft: bool,
    pub state: String,
    pub mine: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrPickerView {
    pub entries: Vec<PrEntryView>,
    pub include_closed: bool,
    pub error: Option<String>,
    pub branch_pr: Option<u32>,
    /// The list is still being read; poll for it.
    pub loading: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrThreadReplyView {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub via_amf: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrInvestigationTurnView {
    pub question: String,
    pub answer: String,
    pub harness: AgentKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrInvestigationView {
    pub status: &'static str,
    pub harness: AgentKind,
    pub answer: Option<String>,
    pub error: Option<String>,
    pub follow_ups: Vec<PrInvestigationTurnView>,
    /// The PR head moved since this investigation ran.
    pub stale_head: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrCommentView {
    pub id: u64,
    /// `inline`, `review_summary` or `conversation`.
    pub kind: &'static str,
    pub review_state: Option<String>,
    pub author: String,
    pub is_bot: bool,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub side: Option<String>,
    pub outdated: bool,
    pub file_level: bool,
    pub body: String,
    pub snippet: String,
    pub hunk: Option<String>,
    pub resolved: bool,
    pub can_resolve: bool,
    pub triage: &'static str,
    pub local_note: Option<String>,
    pub actionable: bool,
    pub local_finding: bool,
    pub replies: Vec<PrThreadReplyView>,
    pub investigation: Option<PrInvestigationView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrReviewView {
    pub number: u32,
    pub url: String,
    pub head_sha: String,
    pub head_ref: String,
    pub branch_mismatch: Option<String>,
    pub fetched_at: String,
    pub open_count: usize,
    pub total: usize,
    pub hide_resolved: bool,
    pub sort: &'static str,
    pub hidden_resolved: usize,
    /// Index into `comments` where the conversation section starts.
    pub conversation_start: Option<usize>,
    pub comments: Vec<PrCommentView>,
    /// The comment whose investigation is running, if any.
    pub investigating: Option<u64>,
    pub investigating_harness: Option<AgentKind>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrReplyDraftView {
    pub comment_id: u64,
    pub kind: &'static str,
    pub seed: String,
    pub agent_drafted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrWriteConfirmView {
    /// `reply`, `resolve` or `reopen`.
    pub kind: &'static str,
    pub comment_id: u64,
    pub destination: String,
    /// The exact text a reply posts, attribution included.
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrTriageView {
    pub workflow_id: String,
    pub revision: u64,
    pub target: FeatureTarget,
    pub feature_name: String,
    pub branch: String,
    /// `pick`, `loading` or `review`.
    pub stage: &'static str,
    pub picker: Option<PrPickerView>,
    pub loading_pr: Option<u32>,
    pub review: Option<PrReviewView>,
    pub precall: Option<PrecallView>,
    pub reply: Option<PrReplyDraftView>,
    pub write_confirm: Option<PrWriteConfirmView>,
    pub harnesses: Vec<AgentKind>,
    pub default_harness: Option<AgentKind>,
    pub error: Option<String>,
    pub notice: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrTriageAction {
    ToggleClosed,
    Open {
        number: u32,
    },
    BackToList,
    Refresh,
    View {
        hide_resolved: bool,
        sort: String,
    },
    ToggleDone {
        comment_id: u64,
    },
    ToggleSkipped {
        comment_id: u64,
    },
    Investigate {
        comment_id: u64,
        harness: AgentKind,
        note: Option<String>,
        follow_up: Option<String>,
    },
    PrecallToggleView,
    PrecallCancel,
    PrecallConfirm,
    CancelInvestigation,
    DismissInvestigation {
        comment_id: u64,
    },
    StartReply {
        comment_id: u64,
        reply: String,
    },
    DiscardReply,
    PrepareReply {
        comment_id: u64,
        body: String,
    },
    RequestResolve {
        comment_id: u64,
    },
    CancelWrite,
    ConfirmWrite,
    Close,
}

const SORTS: [(PrSortMode, &str); 5] = [
    (PrSortMode::FetchOrder, "fetch_order"),
    (PrSortMode::ByFile, "by_file"),
    (PrSortMode::ByAuthor, "by_author"),
    (PrSortMode::HumansFirst, "humans_first"),
    (PrSortMode::Conversations, "conversations"),
];

fn sort_key(mode: PrSortMode) -> &'static str {
    SORTS
        .iter()
        .find(|(m, _)| *m == mode)
        .map_or("fetch_order", |(_, k)| k)
}

fn reply_kind_key(kind: ReplyKind) -> &'static str {
    match kind {
        ReplyKind::Done => "done",
        ReplyKind::NotNeeded => "not_needed",
        ReplyKind::Investigation => "investigation",
    }
}

fn is_triage_mode(mode: &AppMode) -> bool {
    matches!(
        mode,
        AppMode::PrPicker(_)
            | AppMode::PrReviewLoading(_)
            | AppMode::PrReview(_)
            | AppMode::PrInvestigationLoading(_)
    )
}

/// The open review, including while an investigation holds it.
fn review_state(mode: &AppMode) -> Option<&PrReviewState> {
    match mode {
        AppMode::PrReview(state) => Some(state),
        AppMode::PrInvestigationLoading(load) => Some(&load.review),
        _ => None,
    }
}

fn review_mut(app: &mut App) -> GuiResult<&mut PrReviewState> {
    match &mut app.mode {
        AppMode::PrReview(state) => Ok(state),
        AppMode::PrInvestigationLoading(_) => Err(GuiError::conflict(
            "Wait for or cancel the running investigation first",
        )),
        _ => Err(GuiError::conflict("Open a pull request first")),
    }
}

/// Point the shared engines' selection at `comment_id`.
fn select(app: &mut App, comment_id: u64) -> GuiResult<PrComment> {
    let state = review_mut(app)?;
    let index = state
        .review
        .comments
        .iter()
        .position(|c| c.id == comment_id)
        .ok_or_else(|| {
            GuiError::not_found("That comment is no longer in this pull request; refresh")
        })?;
    state.selected = index;
    Ok(state.review.comments[index].clone())
}

fn harnesses(app: &App, workdir: &std::path::Path) -> Vec<AgentKind> {
    let allowed = app.allowed_agents_for_project_path(workdir);
    if allowed.is_empty() {
        app.store.available_harnesses.clone()
    } else {
        allowed
    }
}

/// Collect what a shared engine reported through the TUI's message and
/// toasts, so the GUI can show it instead of a dashboard status line.
fn take_feedback(app: &mut App, toasts_before: usize) -> (Option<String>, Option<String>) {
    let mut error = None;
    let mut notice = app.message.take();
    for toast in app.toasts.drain(toasts_before.min(app.toasts.len())..) {
        match toast.kind {
            ToastKind::Error | ToastKind::Warning => error = Some(toast.message),
            _ => notice = Some(toast.message),
        }
    }
    (error, notice)
}

/// Show the pull request list. `entries` is `None` when the list is still to
/// be read: the next poll reads it without the GUI lock.
fn open_picker(
    gui: &mut GuiHandle,
    include_closed: bool,
    entries: Option<anyhow::Result<Vec<PrListEntry>>>,
    error: Option<String>,
) -> GuiResult<()> {
    let context = gui
        .pr_triage_context
        .as_ref()
        .ok_or_else(|| GuiError::conflict("PR Triage is no longer open"))?;
    let (workdir, branch_pr, current_user) = (
        context.workdir.clone(),
        context.branch_pr,
        context.current_user.clone(),
    );
    let list_pending = entries.is_none();
    let (entries, list_error) = match entries {
        Some(Ok(entries)) => (entries, None),
        Some(Err(e)) => (Vec::new(), Some(e.to_string())),
        None => (Vec::new(), None),
    };
    let selected = branch_pr
        .and_then(|n| entries.iter().position(|e| e.number == n))
        .unwrap_or(0);
    let app = gui.app_for_workflow();
    app.pr_review_work.cancel_fetch();
    app.mode = AppMode::PrPicker(PrPickerState {
        workdir,
        entries,
        selected,
        include_closed,
        error: list_error,
        bootstrap_pick: None,
        current_user,
    });
    if let Some(context) = gui.pr_triage_context.as_mut() {
        context.error = error;
        context.include_closed = include_closed;
        context.list_pending = list_pending;
    }
    Ok(())
}

enum Opening {
    /// This feature's triage is already open.
    Existing,
    /// A new triage for the feature checked out here.
    New(PathBuf),
}

fn opening(gui: &mut GuiHandle, target: &FeatureTarget) -> GuiResult<Opening> {
    gui.refresh_snapshot()?;
    let open_target = gui.pr_triage_context.as_ref().map(|c| c.target.clone());
    if let Some(open) = open_target {
        if is_triage_mode(&gui.app_for_workflow().mode) {
            if open.project_id == target.project_id && open.feature_id == target.feature_id {
                return Ok(Opening::Existing);
            }
            return Err(GuiError::conflict(
                "Close PR Triage before opening another feature",
            ));
        }
        gui.pr_triage_context = None;
    }
    let app = gui.app_for_workflow();
    if !matches!(app.mode, AppMode::Normal) || app.paused_plan_interview.is_some() {
        return Err(GuiError::conflict(
            "Finish the current workflow before opening PR Triage",
        ));
    }
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted; refresh and retry"))?;
    if !app.store.projects[pi].is_git {
        return Err(GuiError::conflict("PR Triage requires a Git repository"));
    }
    Ok(Opening::New(
        app.store.projects[pi].features[fi].workdir.clone(),
    ))
}

/// Open PR Triage for `target`, reading GitHub in place. The Tauri command
/// uses [`plan_begin`] and [`begin_prefetched`] to read without the lock.
pub fn begin(gui: &mut GuiHandle, target: FeatureTarget) -> GuiResult<PrTriageView> {
    let reads = plan_begin(gui, &target)?;
    begin_prefetched(gui, target, reads.run())
}

pub fn plan_begin(gui: &mut GuiHandle, target: &FeatureTarget) -> GuiResult<PrTriageReads> {
    Ok(match opening(gui, target)? {
        Opening::Existing => poll_reads(gui),
        Opening::New(workdir) => PrTriageReads::new(
            gui.app_for_workflow(),
            workdir,
            ReadPlan {
                branch_pr: true,
                current_user: true,
                list: Some(false),
                ..ReadPlan::default()
            },
        ),
    })
}

pub fn begin_prefetched(
    gui: &mut GuiHandle,
    target: FeatureTarget,
    mut reads: PrTriagePrefetch,
) -> GuiResult<PrTriageView> {
    reads.attach(gui);
    let workdir = match opening(gui, &target)? {
        Opening::Existing => return snapshot(gui, &mut reads),
        Opening::New(workdir) => workdir,
    };
    let (branch_pr, resolve_error) = match reads.resolve_pr(&workdir) {
        Ok(PrResolution::Found(pr)) => (Some(pr.number), None),
        Ok(PrResolution::NoPrForBranch) => (None, None),
        Err(e) => (None, Some(e.to_string())),
    };
    let current_user = reads.current_user(&workdir).ok();
    let entries = reads.list_prs(&workdir, false);
    gui.pr_triage_context = Some(PrTriageContext {
        id: uuid::Uuid::new_v4().to_string(),
        target,
        revision: 0,
        workdir,
        branch_pr,
        current_user,
        include_closed: false,
        list_pending: false,
        error: None,
        notice: None,
        investigation: None,
        write: None,
    });
    open_picker(gui, false, Some(entries), resolve_error)?;
    snapshot(gui, &mut reads)
}

/// Apply finished background work, then describe the current state.
pub fn poll(gui: &mut GuiHandle, workflow_id: &str) -> GuiResult<PrTriageView> {
    let reads = plan_poll(gui, workflow_id)?;
    poll_prefetched(gui, workflow_id, reads.run())
}

fn check_workflow(gui: &GuiHandle, workflow_id: &str) -> GuiResult<()> {
    if !gui
        .pr_triage_context
        .as_ref()
        .is_some_and(|c| c.id == workflow_id)
    {
        return Err(GuiError::conflict("PR Triage is no longer open"));
    }
    Ok(())
}

pub fn plan_poll(gui: &mut GuiHandle, workflow_id: &str) -> GuiResult<PrTriageReads> {
    check_workflow(gui, workflow_id)?;
    Ok(poll_reads(gui))
}

/// A poll reads only a pull request list that is open but not read yet.
fn poll_reads(gui: &mut GuiHandle) -> PrTriageReads {
    let Some(context) = gui.pr_triage_context.as_ref() else {
        return PrTriageReads::none();
    };
    let (workdir, include_closed) = (context.workdir.clone(), context.include_closed);
    if !context.list_pending {
        return PrTriageReads::none();
    }
    let app = gui.app_for_workflow();
    if !matches!(app.mode, AppMode::PrPicker(_)) {
        return PrTriageReads::none();
    }
    PrTriageReads::new(
        app,
        workdir,
        ReadPlan {
            list: Some(include_closed),
            ..ReadPlan::default()
        },
    )
}

pub fn poll_prefetched(
    gui: &mut GuiHandle,
    workflow_id: &str,
    mut reads: PrTriagePrefetch,
) -> GuiResult<PrTriageView> {
    check_workflow(gui, workflow_id)?;
    reads.attach(gui);
    snapshot(gui, &mut reads)
}

fn check_target(gui: &mut GuiHandle) -> GuiResult<()> {
    gui.refresh_snapshot()?;
    let context = gui
        .pr_triage_context
        .as_ref()
        .ok_or_else(|| GuiError::conflict("PR Triage is no longer open"))?;
    let (target, workdir) = (context.target.clone(), context.workdir.clone());
    let app = gui.app_for_workflow();
    if !is_triage_mode(&app.mode) {
        gui.pr_triage_context = None;
        return Err(GuiError::conflict("PR Triage is no longer open"));
    }
    let located = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .map(|(pi, fi)| app.store.projects[pi].features[fi].workdir.clone());
    let problem = match located {
        None => Some(GuiError::not_found(
            "The feature was deleted; PR Triage closed",
        )),
        Some(current) if current != workdir => Some(GuiError::conflict(
            "The feature's checkout changed; PR Triage closed",
        )),
        Some(_) => None,
    };
    if let Some(problem) = problem {
        close(gui);
        return Err(problem);
    }
    Ok(())
}

fn snapshot(gui: &mut GuiHandle, reads: &mut PrTriagePrefetch) -> GuiResult<PrTriageView> {
    check_target(gui)?;
    let context = gui.pr_triage_context.as_ref().unwrap();
    let (workdir, include_closed, list_pending) = (
        context.workdir.clone(),
        context.include_closed,
        context.list_pending,
    );
    let mut changed = false;
    if list_pending {
        if !matches!(gui.app_for_workflow().mode, AppMode::PrPicker(_)) {
            gui.pr_triage_context.as_mut().unwrap().list_pending = false;
        } else if let Some(entries) = reads.prefetched_list(&workdir, include_closed) {
            let error = gui.pr_triage_context.as_mut().unwrap().error.take();
            open_picker(gui, include_closed, Some(entries), error)?;
            changed = true;
        }
    }
    let app = gui.app_for_workflow();
    let toasts = app.toasts.len();
    let mut fetch_failure = None;
    if matches!(app.mode, AppMode::PrReviewLoading(_)) {
        match app.poll_pr_review_fetch() {
            Some(Ok(())) => changed = true,
            Some(Err(failure)) => {
                let number = match &app.mode {
                    AppMode::PrReviewLoading(load) => load.pr.number,
                    _ => 0,
                };
                fetch_failure = Some(match failure {
                    PrFetchFailure::Failed(e) => {
                        format!("Could not load PR #{number} comments: {e}")
                    }
                    PrFetchFailure::Disconnected => {
                        format!("Loading PR #{number} comments failed unexpectedly")
                    }
                });
            }
            None => {}
        }
    }
    if app.pr_review_work.investigation_pending() && app.poll_pr_investigation_bg() {
        changed = true;
    }
    let (error, notice) = take_feedback(app, toasts);
    if let Some(message) = fetch_failure {
        // Back to the list the user opened it from; it is read on the next
        // poll rather than here, under the lock.
        open_picker(gui, include_closed, None, Some(message))?;
        changed = true;
    }
    let context = gui.pr_triage_context.as_mut().unwrap();
    if changed {
        context.revision += 1;
    }
    if error.is_some() {
        context.error = error;
    }
    if notice.is_some() {
        context.notice = notice;
    }
    view(gui)
}

fn view(gui: &mut GuiHandle) -> GuiResult<PrTriageView> {
    let context = gui.pr_triage_context.as_ref().unwrap();
    let (workflow_id, revision, target, workdir) = (
        context.id.clone(),
        context.revision,
        context.target.clone(),
        context.workdir.clone(),
    );
    let branch_pr = context.branch_pr;
    let list_pending = context.list_pending;
    let current_user = context.current_user.clone();
    let error = context.error.clone();
    let notice = context.notice.clone();
    let precall = context.investigation.as_ref().map(|p| PrecallView {
        title: INVESTIGATION_CALL_TITLE.into(),
        harness: p.harness.display_name().into(),
        preview: p.prompt.clone(),
        viewing: p.viewing,
    });
    let pending_write = context.write.as_ref().map(|w| match w {
        PendingWrite::Reply {
            comment_id, posted, ..
        } => (*comment_id, "reply", Some(posted.clone())),
        PendingWrite::Resolve {
            comment_id,
            resolve,
            ..
        } => (
            *comment_id,
            if *resolve { "resolve" } else { "reopen" },
            None,
        ),
    });
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature was deleted"))?;
    let feature = &app.store.projects[pi].features[fi];
    let (feature_name, branch) = (feature.name.clone(), feature.branch.clone());
    let preferred = app.store.projects[pi].preferred_agent.clone();
    let harnesses = harnesses(app, &workdir);
    let default_harness = harnesses
        .iter()
        .find(|h| **h == preferred)
        .or(harnesses.first())
        .cloned();
    let stage = match &app.mode {
        AppMode::PrPicker(_) => "pick",
        AppMode::PrReviewLoading(_) => "loading",
        _ => "review",
    };
    let picker = match &app.mode {
        AppMode::PrPicker(state) => Some(PrPickerView {
            entries: state
                .entries
                .iter()
                .map(|e| PrEntryView {
                    number: e.number,
                    title: e.title.clone(),
                    author: e.author.clone(),
                    head_ref: e.head_ref.clone(),
                    updated_at: e.updated_at.clone(),
                    is_draft: e.is_draft,
                    state: e.state.clone(),
                    mine: current_user
                        .as_deref()
                        .is_some_and(|u| u.eq_ignore_ascii_case(&e.author)),
                })
                .collect(),
            include_closed: state.include_closed,
            error: state.error.clone(),
            branch_pr,
            loading: list_pending,
        }),
        _ => None,
    };
    let loading_pr = match &app.mode {
        AppMode::PrReviewLoading(load) => Some(load.pr.number),
        _ => None,
    };
    let investigating = match &app.mode {
        AppMode::PrInvestigationLoading(load) => Some((load.comment_id, load.harness.clone())),
        _ => None,
    };
    let review = review_state(&app.mode).map(|state| review_view(state, investigating));
    let reply = review_state(&app.mode)
        .and_then(|s| s.reply.as_ref())
        .map(|r| PrReplyDraftView {
            comment_id: r.comment_id,
            kind: reply_kind_key(r.kind),
            seed: r.original_seed.clone(),
            agent_drafted: r.agent_drafted,
        });
    let write_confirm = pending_write.map(|(comment_id, kind, body)| {
        let state = review_state(&app.mode);
        let comment = state.and_then(|s| s.review.comments.iter().find(|c| c.id == comment_id));
        let number = state.map_or(0, |s| s.review.pr.number);
        let destination = match (kind, comment.map(PrComment::reply_target)) {
            ("reply", Some(ReplyTarget::InlineThread { .. })) => {
                format!("Reply in the inline review thread on PR #{number}")
            }
            ("reply", _) => format!("New conversation comment on PR #{number}"),
            ("resolve", _) => format!("Resolve the review thread on PR #{number}"),
            _ => format!("Reopen the review thread on PR #{number}"),
        };
        PrWriteConfirmView {
            kind,
            comment_id,
            destination,
            body,
        }
    });
    Ok(PrTriageView {
        workflow_id,
        revision,
        target,
        feature_name,
        branch,
        stage,
        picker,
        loading_pr,
        review,
        precall,
        reply,
        write_confirm,
        harnesses,
        default_harness,
        error,
        notice,
    })
}

fn review_view(state: &PrReviewState, investigating: Option<(u64, AgentKind)>) -> PrReviewView {
    let all = &state.review.comments;
    let head = &state.review.pr.head_sha;
    let comment_view = |c: &PrComment| PrCommentView {
        id: c.id,
        kind: match c.kind {
            CommentKind::Inline => "inline",
            CommentKind::ReviewSummary { .. } => "review_summary",
            CommentKind::Conversation => "conversation",
        },
        review_state: match &c.kind {
            CommentKind::ReviewSummary { state } => Some(state.clone()),
            _ => None,
        },
        author: c.author.clone(),
        is_bot: c.is_bot,
        path: c.path.clone(),
        line: c.line,
        side: c.side.clone(),
        outdated: c.outdated,
        file_level: c.file_level,
        body: if c.is_bot {
            strip_bot_boilerplate(&c.body)
        } else {
            c.body.clone()
        },
        snippet: c.snippet.clone(),
        hunk: c.prompt_hunk().map(|h| h.into_owned()),
        resolved: c.is_resolved,
        can_resolve: c.thread_id.is_some() && !c.is_local_finding(),
        triage: c.triage.as_db_str(),
        local_note: c.local_note.clone(),
        actionable: c.is_actionable(),
        local_finding: c.is_local_finding(),
        replies: c
            .replies_in(all)
            .into_iter()
            .map(|r| PrThreadReplyView {
                id: r.id,
                author: r.author.clone(),
                body: r.body.clone(),
                via_amf: r.is_amf_followup_reply(),
            })
            .collect(),
        investigation: state
            .investigations
            .iter()
            .find(|r| r.comment_id == c.id)
            .map(|r| PrInvestigationView {
                status: r.status.as_db_str(),
                harness: r.harness.clone(),
                answer: r.answer.clone(),
                error: r.error.clone(),
                follow_ups: r
                    .follow_ups
                    .iter()
                    .map(|t| PrInvestigationTurnView {
                        question: t.question.clone(),
                        answer: t.answer.clone(),
                        harness: t.harness.clone(),
                    })
                    .collect(),
                stale_head: !r.head_sha.is_empty() && &r.head_sha != head,
            }),
    };
    PrReviewView {
        number: state.review.pr.number,
        url: state.review.pr.url.clone(),
        head_sha: state.review.pr.head_sha.clone(),
        head_ref: state.review.pr.head_ref.clone(),
        branch_mismatch: state.branch_mismatch().map(str::to_string),
        fetched_at: state.review.fetched_at.format("%Y-%m-%d %H:%M").to_string(),
        open_count: state.review.open_count(),
        // Collated AMF replies render under their root, not as rows.
        total: all
            .iter()
            .filter(|c| !state.review.is_collated_amf_reply(c))
            .count(),
        hide_resolved: state.hide_resolved,
        sort: sort_key(state.sort_mode),
        hidden_resolved: state.hidden_resolved_count(),
        conversation_start: state.conversation_section_start(),
        comments: state
            .visible_indices()
            .into_iter()
            .map(|i| comment_view(&all[i]))
            .collect(),
        investigating: investigating.as_ref().map(|(id, _)| *id),
        investigating_harness: investigating.map(|(_, h)| h),
    }
}

fn close(gui: &mut GuiHandle) {
    let app = gui.app_for_workflow();
    if matches!(app.mode, AppMode::PrInvestigationLoading(_)) {
        app.pr_investigation_cancel();
    }
    if is_triage_mode(&app.mode) {
        app.close_pr_review();
    }
    gui.pr_triage_context = None;
}

/// Apply one action, reading GitHub in place. The Tauri command uses
/// [`plan_act`] and [`act_prefetched`] to read without the lock.
pub fn act(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
    action: PrTriageAction,
) -> GuiResult<Option<PrTriageView>> {
    let reads = plan_act(gui, workflow_id, revision, &action)?;
    act_prefetched(gui, workflow_id, revision, action, reads.run())
}

/// Refuse an action for a stale view or one a pending step blocks.
fn gate<'a>(
    gui: &'a GuiHandle,
    workflow_id: &str,
    revision: u64,
    action: &PrTriageAction,
) -> GuiResult<&'a PrTriageContext> {
    use PrTriageAction as A;
    let context = gui
        .pr_triage_context
        .as_ref()
        .filter(|c| c.id == workflow_id && c.revision == revision)
        .ok_or_else(|| GuiError::conflict("PR Triage changed; retry from the current view"))?;
    if context.investigation.is_some()
        && !matches!(
            action,
            A::PrecallToggleView | A::PrecallCancel | A::PrecallConfirm | A::Close
        )
    {
        return Err(GuiError::conflict(
            "Continue or cancel the pending AI call first",
        ));
    }
    if context.write.is_some() && !matches!(action, A::ConfirmWrite | A::CancelWrite | A::Close) {
        return Err(GuiError::conflict(
            "Confirm or cancel the pending GitHub write first",
        ));
    }
    Ok(context)
}

/// Name the GitHub reads `action` will make. Only a guess at what it needs:
/// [`act_prefetched`] re-checks everything and reads live what was missed.
pub fn plan_act(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
    action: &PrTriageAction,
) -> GuiResult<PrTriageReads> {
    use PrTriageAction as A;
    let context = gate(gui, workflow_id, revision, action)?;
    let (workdir, include_closed) = (context.workdir.clone(), context.include_closed);
    let resolving = matches!(context.write, Some(PendingWrite::Resolve { .. }));
    let replying = matches!(context.write, Some(PendingWrite::Reply { .. }));
    let app = gui.app_for_workflow();
    let pr = review_state(&app.mode).map(|s| s.review.pr.clone());
    let number = pr.as_ref().map(|pr| pr.number);
    let plan = match action {
        A::ToggleClosed => match &app.mode {
            AppMode::PrPicker(state) => ReadPlan {
                list: Some(!state.include_closed),
                ..ReadPlan::default()
            },
            _ => ReadPlan::default(),
        },
        A::BackToList => ReadPlan {
            list: Some(include_closed),
            ..ReadPlan::default()
        },
        A::Open { number } => ReadPlan {
            pr: Some(*number),
            ..ReadPlan::default()
        },
        A::Refresh => ReadPlan {
            pr: number,
            ..ReadPlan::default()
        },
        A::Investigate { .. } | A::PrecallConfirm => ReadPlan {
            meta: number,
            ..ReadPlan::default()
        },
        A::ConfirmWrite if replying => ReadPlan {
            pr: number,
            ..ReadPlan::default()
        },
        A::ConfirmWrite if resolving => ReadPlan {
            threads: pr,
            ..ReadPlan::default()
        },
        _ => return Ok(PrTriageReads::none()),
    };
    Ok(PrTriageReads::new(app, workdir, plan))
}

pub fn act_prefetched(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
    action: PrTriageAction,
    mut reads: PrTriagePrefetch,
) -> GuiResult<Option<PrTriageView>> {
    use PrTriageAction as A;
    gate(gui, workflow_id, revision, &action)?;
    reads.attach(gui);
    if matches!(action, A::Close) {
        close(gui);
        return Ok(None);
    }
    check_target(gui)?;
    let result = apply(gui, action, &mut reads);
    let Some(context) = gui.pr_triage_context.as_mut() else {
        return result.map(|_| None);
    };
    context.revision += 1;
    if result.is_ok() {
        context.error = None;
    }
    result?;
    snapshot(gui, &mut reads).map(Some)
}

fn apply(
    gui: &mut GuiHandle,
    action: PrTriageAction,
    reads: &mut PrTriagePrefetch,
) -> GuiResult<()> {
    use PrTriageAction as A;
    let context = gui.pr_triage_context.as_ref().unwrap();
    let (workdir, include_closed) = (context.workdir.clone(), context.include_closed);
    {
        let context = gui.pr_triage_context.as_mut().unwrap();
        context.notice = None;
    }
    match action {
        A::Close => unreachable!(),
        A::ToggleClosed => {
            let AppMode::PrPicker(state) = &gui.app_for_workflow().mode else {
                return Err(GuiError::conflict("The pull request list is not open"));
            };
            let include = !state.include_closed;
            let entries = reads.list_prs(&workdir, include);
            open_picker(gui, include, Some(entries), None)
        }
        A::Open { number } => {
            let app = gui.app_for_workflow();
            if !matches!(app.mode, AppMode::PrPicker(_)) {
                return Err(GuiError::conflict("The pull request list is not open"));
            }
            let pr = reads
                .fetch_pr_by_number(&workdir, number)
                .map_err(GuiError::from)?;
            app.enter_pr_review(workdir, pr);
            Ok(())
        }
        A::BackToList => {
            let app = gui.app_for_workflow();
            if let Some(state) = review_state(&app.mode)
                && state.reply.is_some()
            {
                return Err(GuiError::conflict(
                    "Post or discard the open reply before leaving this pull request",
                ));
            }
            if matches!(app.mode, AppMode::PrInvestigationLoading(_)) {
                return Err(GuiError::conflict(
                    "Wait for or cancel the running investigation first",
                ));
            }
            let entries = reads.list_prs(&workdir, include_closed);
            open_picker(gui, include_closed, Some(entries), None)
        }
        A::Refresh => {
            let app = gui.app_for_workflow();
            let state = review_mut(app)?;
            if state.reply.is_some() {
                return Err(GuiError::conflict(
                    "Post or discard the open reply before refreshing",
                ));
            }
            let number = state.review.pr.number;
            let pr = reads
                .fetch_pr_by_number(&workdir, number)
                .map_err(GuiError::from)?;
            app.start_pr_review_fetch(workdir, pr);
            Ok(())
        }
        A::View {
            hide_resolved,
            sort,
        } => {
            let mode = SORTS
                .iter()
                .find(|(_, k)| *k == sort)
                .map(|(m, _)| *m)
                .ok_or_else(|| GuiError::conflict(format!("Unknown sort order {sort}")))?;
            let state = match &mut gui.app_for_workflow().mode {
                AppMode::PrReview(state) => state,
                AppMode::PrInvestigationLoading(load) => &mut load.review,
                _ => return Err(GuiError::conflict("Open a pull request first")),
            };
            state.hide_resolved = hide_resolved;
            state.sort_mode = mode;
            state.snap_selection_to_visible();
            Ok(())
        }
        A::ToggleDone { comment_id } | A::ToggleSkipped { comment_id } => {
            let done = matches!(action, A::ToggleDone { .. });
            let app = gui.app_for_workflow();
            let comment = select(app, comment_id)?;
            if !comment.is_actionable() {
                return Err(GuiError::conflict(
                    "AMF follow-up replies are shown for context only",
                ));
            }
            let toasts = app.toasts.len();
            if done {
                app.pr_review_mark_done();
            } else {
                app.pr_review_skip();
            }
            set_feedback(gui, toasts);
            Ok(())
        }
        A::Investigate {
            comment_id,
            harness,
            note,
            follow_up,
        } => request_investigation(gui, reads, comment_id, harness, note, follow_up),
        A::PrecallToggleView => {
            let pending = gui
                .pr_triage_context
                .as_mut()
                .unwrap()
                .investigation
                .as_mut()
                .ok_or_else(|| GuiError::conflict("No AI call is waiting"))?;
            pending.viewing = !pending.viewing;
            Ok(())
        }
        A::PrecallCancel => {
            let context = gui.pr_triage_context.as_mut().unwrap();
            context
                .investigation
                .take()
                .ok_or_else(|| GuiError::conflict("No AI call is waiting"))?;
            context.notice = Some("Headless AI call cancelled".into());
            Ok(())
        }
        A::PrecallConfirm => confirm_investigation(gui, reads),
        A::CancelInvestigation => {
            let app = gui.app_for_workflow();
            if !matches!(app.mode, AppMode::PrInvestigationLoading(_)) {
                return Err(GuiError::conflict("No investigation is running"));
            }
            let toasts = app.toasts.len();
            app.pr_investigation_cancel();
            set_feedback(gui, toasts);
            Ok(())
        }
        A::DismissInvestigation { comment_id } => {
            let app = gui.app_for_workflow();
            select(app, comment_id)?;
            let state = review_mut(app)?;
            let finished = state
                .investigations
                .iter()
                .find(|r| r.comment_id == comment_id)
                .is_some_and(|r| r.status != PrInvestigationStatus::Running);
            if !finished {
                return Err(GuiError::conflict(
                    "This comment has no finished investigation",
                ));
            }
            let toasts = app.toasts.len();
            app.pr_investigation_dismiss();
            set_feedback(gui, toasts);
            Ok(())
        }
        A::StartReply { comment_id, reply } => start_reply(gui, comment_id, &reply),
        A::DiscardReply => {
            review_mut(gui.app_for_workflow())?.reply = None;
            Ok(())
        }
        A::PrepareReply { comment_id, body } => {
            let app = gui.app_for_workflow();
            let state = review_mut(app)?;
            let head_sha = state.review.pr.head_sha.clone();
            let kind = match &state.reply {
                Some(reply) if reply.comment_id == comment_id => reply.kind,
                _ => {
                    return Err(GuiError::conflict(
                        "That reply is no longer open; start it again",
                    ));
                }
            };
            if body.trim().is_empty() {
                return Err(GuiError::conflict(match kind {
                    ReplyKind::NotNeeded => "Explain why a fix isn't needed before posting",
                    _ => "The reply is empty",
                }));
            }
            let posted = app
                .pr_review_reply_posted_body(&body)
                .ok_or_else(|| GuiError::conflict("That reply is no longer open"))?;
            gui.pr_triage_context.as_mut().unwrap().write = Some(PendingWrite::Reply {
                comment_id,
                kind,
                body,
                posted,
                head_sha,
            });
            Ok(())
        }
        A::RequestResolve { comment_id } => {
            let comment = select(gui.app_for_workflow(), comment_id)?;
            let thread_id = comment
                .thread_id
                .clone()
                .filter(|_| !comment.is_local_finding())
                .ok_or_else(|| {
                    GuiError::conflict("This comment has no resolvable review thread")
                })?;
            gui.pr_triage_context.as_mut().unwrap().write = Some(PendingWrite::Resolve {
                comment_id,
                thread_id,
                resolve: !comment.is_resolved,
            });
            Ok(())
        }
        A::CancelWrite => {
            gui.pr_triage_context
                .as_mut()
                .unwrap()
                .write
                .take()
                .ok_or_else(|| GuiError::conflict("No GitHub write is waiting"))?;
            Ok(())
        }
        A::ConfirmWrite => confirm_write(gui, reads),
    }
}

fn set_feedback(gui: &mut GuiHandle, toasts: usize) {
    let (error, notice) = take_feedback(gui.app_for_workflow(), toasts);
    let context = gui.pr_triage_context.as_mut().unwrap();
    if notice.is_some() {
        context.notice = notice;
    }
    if error.is_some() {
        context.error = error;
    }
}

fn request_investigation(
    gui: &mut GuiHandle,
    reads: &mut PrTriagePrefetch,
    comment_id: u64,
    harness: AgentKind,
    note: Option<String>,
    follow_up: Option<String>,
) -> GuiResult<()> {
    let workdir = gui.pr_triage_context.as_ref().unwrap().workdir.clone();
    let app = gui.app_for_workflow();
    if app.pr_review_work.investigation_pending() {
        return Err(GuiError::conflict("An investigation is already running"));
    }
    if !harnesses(app, &workdir).contains(&harness) {
        return Err(GuiError::conflict(format!(
            "{} is not allowed for this project",
            harness.display_name()
        )));
    }
    let comment = select(app, comment_id)?;
    if !comment.is_actionable() {
        return Err(GuiError::conflict(
            "AMF follow-up replies cannot be investigated",
        ));
    }
    let note = note
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty() && follow_up.is_none());
    if note
        .as_ref()
        .is_some_and(|n| n.chars().count() > INVESTIGATION_CONTEXT_MAX_LEN)
    {
        return Err(GuiError::conflict(format!(
            "Keep the investigation note under {INVESTIGATION_CONTEXT_MAX_LEN} characters"
        )));
    }
    let follow_up = follow_up.map(|q| q.trim().to_string());
    if follow_up.as_ref().is_some_and(String::is_empty) {
        return Err(GuiError::conflict("Type a follow-up question first"));
    }
    let prompt = build_prompt(
        app,
        reads,
        &workdir,
        &comment,
        follow_up.as_deref(),
        note.as_deref(),
    )?;
    let head_sha = review_mut(app)?.review.pr.head_sha.clone();
    gui.pr_triage_context.as_mut().unwrap().investigation = Some(PendingInvestigation {
        comment_id,
        harness,
        follow_up,
        note,
        head_sha,
        prompt,
        viewing: false,
    });
    Ok(())
}

/// Build the prompt the run will send, from a fresh read of the PR.
fn build_prompt(
    app: &mut App,
    reads: &mut PrTriagePrefetch,
    workdir: &Path,
    comment: &PrComment,
    follow_up: Option<&str>,
    note: Option<&str>,
) -> GuiResult<String> {
    let state = review_mut(app)?;
    let number = state.review.pr.number;
    let prior = match follow_up {
        None => None,
        Some(question) => {
            let row = state
                .investigations
                .iter()
                .find(|r| r.comment_id == comment.id)
                .filter(|r| {
                    r.status == PrInvestigationStatus::Complete
                        || r.status == PrInvestigationStatus::Dismissed
                })
                .filter(|r| r.answer.as_deref().is_some_and(|a| !a.trim().is_empty()))
                .ok_or_else(|| GuiError::conflict("No completed investigation to follow up on"))?;
            Some((
                row.answer.clone().unwrap_or_default(),
                row.follow_ups.clone(),
                question.to_string(),
            ))
        }
    };
    let meta = reads
        .pr_meta(workdir, number)
        .map_err(|e| GuiError::from(e.context(format!("Couldn't load PR #{number}"))))?;
    Ok(investigation_prompt_from_meta(
        comment,
        number,
        &meta,
        prior.as_ref().map(|(answer, turns, question)| {
            (answer.as_str(), turns.as_slice(), question.as_str())
        }),
        note,
    ))
}

fn confirm_investigation(gui: &mut GuiHandle, reads: &mut PrTriagePrefetch) -> GuiResult<()> {
    let workdir = gui.pr_triage_context.as_ref().unwrap().workdir.clone();
    let pending = gui
        .pr_triage_context
        .as_mut()
        .unwrap()
        .investigation
        .take()
        .ok_or_else(|| GuiError::conflict("No AI call is waiting"))?;
    let app = gui.app_for_workflow();
    if app.pr_review_work.investigation_pending() {
        gui.pr_triage_context.as_mut().unwrap().investigation = Some(pending);
        return Err(GuiError::conflict("An investigation is already running"));
    }
    let comment = select(app, pending.comment_id)?;
    if review_mut(app)?.review.pr.head_sha != pending.head_sha {
        return Err(GuiError::conflict(
            "The pull request was refreshed after the preview; start the investigation again",
        ));
    }
    // The PR's description or file list may have changed on GitHub since the
    // preview. Never send a prompt the operator has not seen.
    let prompt = match build_prompt(
        app,
        reads,
        &workdir,
        &comment,
        pending.follow_up.as_deref(),
        pending.note.as_deref(),
    ) {
        Ok(prompt) => prompt,
        Err(e) => {
            // A failed re-read changes nothing: keep the notice to retry.
            gui.pr_triage_context.as_mut().unwrap().investigation = Some(pending);
            return Err(e);
        }
    };
    if prompt != pending.prompt {
        gui.pr_triage_context.as_mut().unwrap().investigation = Some(PendingInvestigation {
            prompt,
            viewing: true,
            ..pending
        });
        return Err(GuiError::conflict(
            "The pull request changed after the preview; review the updated prompt, then continue",
        ));
    }
    let follow_up = pending.follow_up.map(|question| PendingFollowUp {
        comment_id: pending.comment_id,
        question,
    });
    let toasts = app.toasts.len();
    app.pr_review_launch_investigation(pending.harness, follow_up, Some(pending.prompt));
    let started = matches!(app.mode, AppMode::PrInvestigationLoading(_));
    let (error, _) = take_feedback(app, toasts);
    if !started {
        return Err(GuiError::conflict(
            error.unwrap_or_else(|| "The investigation could not start".into()),
        ));
    }
    Ok(())
}

fn start_reply(gui: &mut GuiHandle, comment_id: u64, reply: &str) -> GuiResult<()> {
    let kind = match reply {
        "done" => ReplyKind::Done,
        "not_needed" => ReplyKind::NotNeeded,
        "investigation" => ReplyKind::Investigation,
        other => return Err(GuiError::conflict(format!("Unknown reply kind {other}"))),
    };
    let app = gui.app_for_workflow();
    if let Some(open) = &review_mut(app)?.reply {
        if open.comment_id == comment_id && open.kind == kind {
            return Ok(());
        }
        return Err(GuiError::conflict(
            "Post or discard the open reply before starting another",
        ));
    }
    let comment = select(app, comment_id)?;
    if comment.is_local_finding() {
        return Err(GuiError::conflict(
            "This AI finding is not posted to GitHub; there is nothing to reply to",
        ));
    }
    if !comment.is_actionable() {
        return Err(GuiError::conflict(
            "AMF follow-up replies are shown for context only",
        ));
    }
    let toasts = app.toasts.len();
    match kind {
        ReplyKind::Done => app.pr_review_open_reply_done(),
        ReplyKind::NotNeeded => app.pr_review_open_reply_not_needed(),
        ReplyKind::Investigation => app.pr_investigation_post_reply(),
    }
    let opened = review_mut(app)?.reply.is_some();
    let (error, notice) = take_feedback(app, toasts);
    if !opened {
        return Err(GuiError::conflict(
            error
                .or(notice)
                .unwrap_or_else(|| "The reply could not be opened".into()),
        ));
    }
    Ok(())
}

fn confirm_write(gui: &mut GuiHandle, reads: &mut PrTriagePrefetch) -> GuiResult<()> {
    let workdir = gui.pr_triage_context.as_ref().unwrap().workdir.clone();
    let write = gui
        .pr_triage_context
        .as_mut()
        .unwrap()
        .write
        .take()
        .ok_or_else(|| GuiError::conflict("No GitHub write is waiting"))?;
    let app = gui.app_for_workflow();
    match write {
        PendingWrite::Reply {
            comment_id,
            kind,
            body,
            posted,
            head_sha,
        } => {
            select(app, comment_id)?;
            let state = review_mut(app)?;
            if !state
                .reply
                .as_ref()
                .is_some_and(|r| r.comment_id == comment_id && r.kind == kind)
            {
                return Err(GuiError::conflict(
                    "That reply is no longer open; nothing was posted",
                ));
            }
            let pr = state.review.pr.clone();
            if pr.head_sha != head_sha {
                return Err(GuiError::conflict(
                    "The pull request was refreshed after you confirmed; nothing was posted",
                ));
            }
            let current = reads
                .fetch_pr_by_number(&workdir, pr.number)
                .map_err(GuiError::from)?;
            if current.head_sha != pr.head_sha {
                return Err(GuiError::conflict(format!(
                    "PR #{} has new commits since these comments were loaded. Refresh before replying; nothing was posted and your draft is kept",
                    pr.number
                )));
            }
            if let Some(reply) = &mut review_mut(app)?.reply {
                reply.editor = TextEditor::new(body.clone());
            }
            if app.pr_review_reply_posted_body(&body).as_deref() != Some(posted.as_str()) {
                return Err(GuiError::conflict(
                    "The reply changed after confirmation; nothing was posted",
                ));
            }
            let toasts = app.toasts.len();
            let posted = app.try_pr_review_post_reply();
            let (error, notice) = take_feedback(app, toasts);
            match posted {
                Ok(true) => {
                    gui.pr_triage_context.as_mut().unwrap().notice = notice;
                    Ok(())
                }
                Ok(false) => Err(GuiError::conflict(
                    error
                        .or(notice)
                        .unwrap_or_else(|| "Nothing was posted".into()),
                )),
                Err(e) => Err(GuiError::from(
                    e.context("Could not post the reply; your draft is kept"),
                )),
            }
        }
        PendingWrite::Resolve {
            comment_id,
            thread_id,
            resolve,
        } => {
            select(app, comment_id)?;
            let pr = review_mut(app)?.review.pr.clone();
            let threads = reads
                .review_threads(&workdir, &pr)
                .map_err(GuiError::from)?;
            let Some(current) = threads.iter().find(|t| t.id == thread_id) else {
                return Err(GuiError::not_found(
                    "That review thread no longer exists on GitHub; refresh",
                ));
            };
            if current.is_resolved == resolve {
                // Someone else already changed it: show the real state.
                let now = current.is_resolved;
                for c in &mut review_mut(app)?.review.comments {
                    if c.thread_id.as_deref() == Some(thread_id.as_str()) {
                        c.is_resolved = now;
                    }
                }
                return Err(GuiError::conflict(if now {
                    "The thread was already resolved on GitHub; the list now shows its current state"
                } else {
                    "The thread was already reopened on GitHub; the list now shows its current state"
                }));
            }
            let toasts = app.toasts.len();
            let result = app.try_pr_review_toggle_resolve();
            let (_, notice) = take_feedback(app, toasts);
            match result {
                Ok(Some(_)) => {
                    gui.pr_triage_context.as_mut().unwrap().notice = notice;
                    Ok(())
                }
                Ok(None) => Err(GuiError::conflict(
                    notice.unwrap_or_else(|| "Nothing was changed".into()),
                )),
                Err(e) => Err(GuiError::from(
                    e.context("Could not change the thread on GitHub"),
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::pr_review::PrReview;
    use crate::app::pr_review::github_access::fake::FakeGithub;
    use crate::github::{PrListEntry, PrMeta};
    use std::time::{Duration, Instant};

    const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn runner(harness: &AgentKind, _: &std::path::Path, prompt: &str) -> anyhow::Result<String> {
        let kind = if prompt.contains("--- Their follow-up question ---") {
            "Follow-up answer"
        } else {
            "Initial answer"
        };
        Ok(format!("{kind} from {}", harness.display_name()))
    }

    fn comment(value: serde_json::Value) -> PrComment {
        let mut base = serde_json::json!({
            "kind": "Inline", "author": "aria", "is_bot": false, "path": "code.txt",
            "line": 8, "side": "RIGHT", "outdated": false, "file_level": false,
            "diff_hunk": "@@ -6,3 +6,3 @@\n line 6\n line 7\n-line 8\n+committed",
            "snippet": "", "in_reply_to": null, "thread_id": null, "is_resolved": false,
            "triage": "Untriaged", "local_note": null,
        });
        for (key, v) in value.as_object().unwrap() {
            base[key] = v.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    fn github() -> FakeGithub {
        let fake = FakeGithub::default();
        {
            let mut state = fake.state();
            state.head_sha = HEAD.into();
            state.branch_pr = Some(7);
            state.prs = [(7, "OPEN", "Feature work"), (5, "MERGED", "Older work")]
                .into_iter()
                .map(|(number, state, title)| PrListEntry {
                    number,
                    title: title.into(),
                    author: "reviewer".into(),
                    head_ref: "feature".into(),
                    updated_at: "2026-10-05T12:00:00Z".into(),
                    is_draft: false,
                    state: state.into(),
                })
                .collect();
            state.meta = PrMeta {
                title: "Feature work".into(),
                body: "Rounds totals.".into(),
                files: Vec::new(),
            };
            state.threads = vec![
                ("THREAD_A".into(), false, vec![101, 103]),
                ("THREAD_B".into(), true, vec![102]),
            ];
            state.review = Some(PrReview {
                pr: fake_pr(),
                fetched_at: chrono::Local::now(),
                comments: vec![
                    comment(
                        serde_json::json!({"id": 101, "body": "Does line 8 handle an empty input?"}),
                    ),
                    comment(
                        serde_json::json!({"id": 102, "body": "Rename this variable.", "line": 9}),
                    ),
                    comment(
                        serde_json::json!({"id": 103, "author": "dev", "in_reply_to": 101,
                        "body": "Checking.\n\n— posted via AMF"}),
                    ),
                    comment(
                        serde_json::json!({"id": 104, "kind": "Conversation", "path": null,
                        "line": null, "diff_hunk": null, "body": "Thanks for the PR!"}),
                    ),
                ],
            });
        }
        fake
    }

    fn fake_pr() -> crate::github::PrRef {
        crate::github::PrRef {
            number: 7,
            head_sha: HEAD.into(),
            url: String::new(),
            owner: "demo".into(),
            repo: "repo".into(),
            head_ref: "feature".into(),
        }
    }

    fn fixture() -> (tempfile::TempDir, GuiHandle, FeatureTarget) {
        let (dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let mut worktree = crate::traits::MockWorktreeOps::new();
        worktree
            .expect_repo_root()
            .returning(|path| Ok(path.to_path_buf()));
        gui.app_for_workflow().worktree = Box::new(worktree);
        (dir, gui, target)
    }

    fn opened() -> (tempfile::TempDir, GuiHandle, FakeGithub, PrTriageView) {
        let (dir, mut gui, target) = fixture();
        let fake = github();
        let app = gui.app_for_workflow();
        app.store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];
        app.pr_review_work
            .set_github_for_test(std::sync::Arc::new(fake.clone()));
        app.pr_review_work.set_investigation_runner_for_test(runner);
        let picked = begin(&mut gui, target).unwrap();
        assert_eq!(picked.stage, "pick");
        let loading = act(&mut gui, &picked, PrTriageAction::Open { number: 7 });
        let view = settle(&mut gui, loading);
        assert_eq!(view.stage, "review");
        (dir, gui, fake, view)
    }

    fn act(gui: &mut GuiHandle, view: &PrTriageView, action: PrTriageAction) -> PrTriageView {
        super::act(gui, &view.workflow_id, view.revision, action)
            .unwrap()
            .unwrap()
    }

    fn try_act(
        gui: &mut GuiHandle,
        view: &PrTriageView,
        action: PrTriageAction,
    ) -> GuiResult<Option<PrTriageView>> {
        super::act(gui, &view.workflow_id, view.revision, action)
    }

    /// Poll until no fetch, list read or investigation is in flight.
    fn settle(gui: &mut GuiHandle, mut view: PrTriageView) -> PrTriageView {
        let until = Instant::now() + Duration::from_secs(10);
        while view.stage == "loading"
            || view.picker.as_ref().is_some_and(|p| p.loading)
            || view
                .review
                .as_ref()
                .is_some_and(|r| r.investigating.is_some())
        {
            assert!(Instant::now() < until, "background work never finished");
            std::thread::sleep(Duration::from_millis(10));
            view = poll(gui, &view.workflow_id).unwrap();
        }
        view
    }

    fn comment_view(view: &PrTriageView, id: u64) -> &PrCommentView {
        view.review
            .as_ref()
            .unwrap()
            .comments
            .iter()
            .find(|c| c.id == id)
            .unwrap()
    }

    #[test]
    fn picks_a_pr_and_browses_threads_through_the_shared_fetch() {
        let (dir, mut gui, target) = fixture();
        let fake = github();
        gui.app_for_workflow()
            .pr_review_work
            .set_github_for_test(std::sync::Arc::new(fake.clone()));
        let picked = begin(&mut gui, target.clone()).unwrap();
        let picker = picked.picker.as_ref().unwrap();
        assert_eq!(picker.branch_pr, Some(7));
        assert_eq!(
            picker.entries.len(),
            1,
            "open pull requests only by default"
        );
        assert!(picker.entries[0].mine);
        let all = act(&mut gui, &picked, PrTriageAction::ToggleClosed);
        assert_eq!(all.picker.as_ref().unwrap().entries.len(), 2);
        // Reopening the same feature returns the same workflow.
        assert_eq!(
            begin(&mut gui, target).unwrap().workflow_id,
            picked.workflow_id
        );

        let loading = act(&mut gui, &all, PrTriageAction::Open { number: 7 });
        assert_eq!(loading.stage, "loading");
        assert_eq!(loading.loading_pr, Some(7));
        let view = settle(&mut gui, loading);
        let review = view.review.as_ref().unwrap();
        assert_eq!(review.number, 7);
        assert_eq!(
            review.open_count, 2,
            "resolved threads and AMF replies are not open work"
        );
        assert_eq!(review.total, 3, "collated AMF replies are not rows");
        // The AMF follow-up reply is collated under its root, not a row.
        assert_eq!(
            review.comments.iter().map(|c| c.id).collect::<Vec<_>>(),
            [101, 102, 104]
        );
        let root = comment_view(&view, 101);
        assert_eq!(root.replies.len(), 1);
        assert!(root.replies[0].via_amf);
        assert!(root.hunk.as_deref().unwrap().contains("+committed"));
        assert!(root.can_resolve && !root.resolved);
        assert!(comment_view(&view, 102).resolved);
        assert!(!comment_view(&view, 104).can_resolve);

        let filtered = act(
            &mut gui,
            &view,
            PrTriageAction::View {
                hide_resolved: true,
                sort: "conversations".into(),
            },
        );
        let review = filtered.review.as_ref().unwrap();
        assert_eq!(review.hidden_resolved, 1);
        assert_eq!(
            review.comments.iter().map(|c| c.id).collect::<Vec<_>>(),
            [101, 104]
        );
        assert_eq!(review.conversation_start, Some(1));

        // Local triage is bookkeeping only.
        let done = act(
            &mut gui,
            &filtered,
            PrTriageAction::ToggleDone { comment_id: 101 },
        );
        assert_eq!(comment_view(&done, 101).triage, "done");
        // A stale revision is refused without acting.
        assert_eq!(
            try_act(
                &mut gui,
                &filtered,
                PrTriageAction::ToggleDone { comment_id: 101 }
            )
            .unwrap_err()
            .kind,
            crate::gui_contract::GuiErrorKind::Conflict
        );
        // The second open is a cache hit: no second comment fetch.
        let listed = act(&mut gui, &done, PrTriageAction::BackToList);
        let reopened = act(&mut gui, &listed, PrTriageAction::Open { number: 7 });
        assert_eq!(reopened.stage, "review");
        assert_eq!(fake.state().fetches, 1);
        assert_eq!(comment_view(&reopened, 101).triage, "done");
        assert!(fake.state().writes.is_empty());
        drop(dir);
    }

    #[test]
    fn a_failed_fetch_returns_to_the_list_with_the_error() {
        let (_dir, mut gui, target) = fixture();
        let fake = github();
        fake.state().fail_fetch = Some("rate limited".into());
        gui.app_for_workflow()
            .pr_review_work
            .set_github_for_test(std::sync::Arc::new(fake.clone()));
        let picked = begin(&mut gui, target).unwrap();
        let all = act(&mut gui, &picked, PrTriageAction::ToggleClosed);
        let loading = act(&mut gui, &all, PrTriageAction::Open { number: 5 });
        // The failure is reported at once; the list is read by a later poll,
        // never inside the poll that saw the failure.
        let mut failed = loading;
        while failed.stage == "loading" {
            std::thread::sleep(Duration::from_millis(10));
            failed = poll(&mut gui, &failed.workflow_id).unwrap();
        }
        assert_eq!(failed.stage, "pick");
        assert!(failed.picker.as_ref().unwrap().loading);
        let view = settle(&mut gui, failed);
        assert!(view.error.as_deref().unwrap().contains("rate limited"));
        // Back on the list it was opened from, closed and merged included.
        let picker = view.picker.as_ref().unwrap();
        assert!(picker.include_closed);
        assert!(picker.entries.iter().any(|e| e.number == 5));

        fake.state().fail_fetch = None;
        let loading = act(&mut gui, &view, PrTriageAction::Open { number: 7 });
        let opened = settle(&mut gui, loading);
        let listed = act(&mut gui, &opened, PrTriageAction::BackToList);
        let picker = listed.picker.as_ref().unwrap();
        assert!(picker.include_closed && picker.entries.len() == 2);
    }

    #[test]
    fn each_step_uses_the_reads_made_before_it_took_the_lock() {
        let (_dir, mut gui, target) = fixture();
        let fake = github();
        let app = gui.app_for_workflow();
        app.store.available_harnesses = vec![AgentKind::Claude];
        app.pr_review_work
            .set_github_for_test(std::sync::Arc::new(fake.clone()));
        app.pr_review_work.set_investigation_runner_for_test(runner);
        // Every step reads first, then GitHub goes away: whatever it applies
        // under the lock must come from those reads, not a fresh `gh` call.
        let reads = plan_begin(&mut gui, &target).unwrap().run();
        fake.state().fail_reads = true;
        let picked = begin_prefetched(&mut gui, target, reads).unwrap();
        let picker = picked.picker.as_ref().unwrap();
        assert_eq!(picker.branch_pr, Some(7));
        assert!(picker.error.is_none() && picker.entries[0].mine);

        let step = |gui: &mut GuiHandle, view: &PrTriageView, action: PrTriageAction| {
            fake.state().fail_reads = false;
            let reads = plan_act(gui, &view.workflow_id, view.revision, &action)
                .unwrap()
                .run();
            fake.state().fail_reads = true;
            super::act_prefetched(gui, &view.workflow_id, view.revision, action, reads)
                .unwrap()
                .unwrap()
        };
        let all = step(&mut gui, &picked, PrTriageAction::ToggleClosed);
        assert_eq!(all.picker.as_ref().unwrap().entries.len(), 2);
        let loading = step(&mut gui, &all, PrTriageAction::Open { number: 7 });
        let view = settle(&mut gui, loading);
        let investigate = PrTriageAction::Investigate {
            comment_id: 101,
            harness: AgentKind::Claude,
            note: None,
            follow_up: None,
        };
        let pending = step(&mut gui, &view, investigate);
        assert!(pending.precall.is_some());
        let running = step(&mut gui, &pending, PrTriageAction::PrecallConfirm);
        let done = settle(&mut gui, running);
        assert_eq!(
            comment_view(&done, 101)
                .investigation
                .as_ref()
                .unwrap()
                .status,
            "complete"
        );
        let resolve = step(
            &mut gui,
            &done,
            PrTriageAction::RequestResolve { comment_id: 101 },
        );
        let resolved = step(&mut gui, &resolve, PrTriageAction::ConfirmWrite);
        assert!(comment_view(&resolved, 101).resolved);
        assert_eq!(fake.state().writes, ["resolve THREAD_A: true"]);
    }

    #[test]
    fn a_failed_reread_keeps_the_precall_notice() {
        let (_dir, mut gui, fake, view) = opened();
        let pending = act(
            &mut gui,
            &view,
            PrTriageAction::Investigate {
                comment_id: 101,
                harness: AgentKind::Claude,
                note: Some("I suspect the empty case".into()),
                follow_up: None,
            },
        );
        fake.state().fail_reads = true;
        let refused = try_act(&mut gui, &pending, PrTriageAction::PrecallConfirm).unwrap_err();
        assert!(
            refused.message.contains("Couldn't load PR #7"),
            "{}",
            refused.message
        );
        let kept = poll(&mut gui, &pending.workflow_id).unwrap();
        assert_eq!(
            kept.precall.as_ref().map(|p| &p.preview),
            pending.precall.as_ref().map(|p| &p.preview)
        );
        assert!(comment_view(&kept, 101).investigation.is_none());

        fake.state().fail_reads = false;
        let running = act(&mut gui, &kept, PrTriageAction::PrecallConfirm);
        let done = settle(&mut gui, running);
        assert_eq!(
            comment_view(&done, 101)
                .investigation
                .as_ref()
                .unwrap()
                .status,
            "complete"
        );
    }

    #[test]
    fn investigations_wait_for_the_precall_and_refuse_a_changed_prompt() {
        let (_dir, mut gui, fake, view) = opened();
        let pending = act(
            &mut gui,
            &view,
            PrTriageAction::Investigate {
                comment_id: 101,
                harness: AgentKind::Codex,
                note: Some("I suspect the empty case".into()),
                follow_up: None,
            },
        );
        let precall = pending.precall.as_ref().unwrap();
        assert_eq!(precall.harness, "Codex");
        assert!(precall.preview.contains("PR #7: Feature work"));
        assert!(precall.preview.contains("I suspect the empty case"));
        assert!(comment_view(&pending, 101).investigation.is_none());
        // Nothing else can happen while the notice waits.
        assert!(try_act(&mut gui, &pending, PrTriageAction::Refresh).is_err());
        let pending = poll(&mut gui, &pending.workflow_id).unwrap();
        let cancelled = act(&mut gui, &pending, PrTriageAction::PrecallCancel);
        assert!(cancelled.precall.is_none());
        assert!(comment_view(&cancelled, 101).investigation.is_none());

        let pending = act(
            &mut gui,
            &cancelled,
            PrTriageAction::Investigate {
                comment_id: 101,
                harness: AgentKind::Claude,
                note: None,
                follow_up: None,
            },
        );
        fake.state().meta.title = "Feature work, renamed".into();
        let refused = try_act(&mut gui, &pending, PrTriageAction::PrecallConfirm).unwrap_err();
        assert!(refused.message.contains("changed after the preview"));
        let refreshed = poll(&mut gui, &pending.workflow_id).unwrap();
        let precall = refreshed.precall.as_ref().unwrap();
        assert!(precall.viewing);
        assert!(precall.preview.contains("Feature work, renamed"));
        assert!(comment_view(&refreshed, 101).investigation.is_none());

        let running = act(&mut gui, &refreshed, PrTriageAction::PrecallConfirm);
        assert!(running.precall.is_none());
        let done = settle(&mut gui, running);
        let investigation = comment_view(&done, 101).investigation.clone().unwrap();
        assert_eq!(investigation.status, "complete");
        assert_eq!(
            investigation.answer.as_deref(),
            Some("Initial answer from Claude")
        );

        let follow = act(
            &mut gui,
            &done,
            PrTriageAction::Investigate {
                comment_id: 101,
                harness: AgentKind::Codex,
                note: None,
                follow_up: Some("Is there a test?".into()),
            },
        );
        assert!(
            follow
                .precall
                .as_ref()
                .unwrap()
                .preview
                .contains("Is there a test?")
        );
        let running = act(&mut gui, &follow, PrTriageAction::PrecallConfirm);
        let finished = settle(&mut gui, running);
        let investigation = comment_view(&finished, 101).investigation.clone().unwrap();
        assert_eq!(investigation.follow_ups.len(), 1);
        assert_eq!(
            investigation.follow_ups[0].answer,
            "Follow-up answer from Codex"
        );
        assert_eq!(
            investigation.answer.as_deref(),
            Some("Initial answer from Claude")
        );

        let dismissed = act(
            &mut gui,
            &finished,
            PrTriageAction::DismissInvestigation { comment_id: 101 },
        );
        assert_eq!(
            comment_view(&dismissed, 101)
                .investigation
                .as_ref()
                .unwrap()
                .status,
            "dismissed"
        );
        assert!(fake.state().writes.is_empty());
    }

    fn slow_runner(_: &AgentKind, _: &std::path::Path, _: &str) -> anyhow::Result<String> {
        std::thread::sleep(Duration::from_millis(400));
        Ok("Late answer".into())
    }

    #[test]
    fn a_running_investigation_holds_the_review_until_cancelled() {
        let (_dir, mut gui, _fake, view) = opened();
        gui.app_for_workflow()
            .pr_review_work
            .set_investigation_runner_for_test(slow_runner);
        let pending = act(
            &mut gui,
            &view,
            PrTriageAction::Investigate {
                comment_id: 101,
                harness: AgentKind::Claude,
                note: None,
                follow_up: None,
            },
        );
        let running = act(&mut gui, &pending, PrTriageAction::PrecallConfirm);
        let review = running.review.as_ref().unwrap();
        assert_eq!(review.investigating, Some(101));
        assert_eq!(
            comment_view(&running, 101)
                .investigation
                .as_ref()
                .unwrap()
                .status,
            "running"
        );
        // Writes and another call wait for the run.
        assert!(
            try_act(
                &mut gui,
                &running,
                PrTriageAction::RequestResolve { comment_id: 101 }
            )
            .is_err()
        );
        let running = poll(&mut gui, &running.workflow_id).unwrap();
        let cancelled = act(&mut gui, &running, PrTriageAction::CancelInvestigation);
        assert!(cancelled.review.as_ref().unwrap().investigating.is_none());
        let row = comment_view(&cancelled, 101).investigation.clone().unwrap();
        assert_eq!(row.status, "failed");
        // The late answer is discarded rather than reviving the row.
        std::thread::sleep(Duration::from_millis(600));
        let later = poll(&mut gui, &cancelled.workflow_id).unwrap();
        assert_eq!(
            comment_view(&later, 101)
                .investigation
                .as_ref()
                .unwrap()
                .status,
            "failed"
        );
    }

    #[test]
    fn replies_post_once_after_confirmation_and_refuse_a_moved_head() {
        let (_dir, mut gui, fake, view) = opened();
        let reply = act(
            &mut gui,
            &view,
            PrTriageAction::StartReply {
                comment_id: 101,
                reply: "not_needed".into(),
            },
        );
        assert_eq!(reply.reply.as_ref().unwrap().kind, "not_needed");
        assert!(
            try_act(
                &mut gui,
                &reply,
                PrTriageAction::PrepareReply {
                    comment_id: 101,
                    body: "  ".into()
                }
            )
            .is_err()
        );
        let reply = poll(&mut gui, &reply.workflow_id).unwrap();
        let confirm = act(
            &mut gui,
            &reply,
            PrTriageAction::PrepareReply {
                comment_id: 101,
                body: "The caller already guards empty input.".into(),
            },
        );
        let pending = confirm.write_confirm.as_ref().unwrap();
        assert_eq!(pending.kind, "reply");
        assert!(pending.destination.contains("inline review thread"));
        assert_eq!(
            pending.body.as_deref(),
            Some("The caller already guards empty input.\n\n— posted via AMF")
        );
        assert!(fake.state().writes.is_empty(), "preparing never writes");

        // GitHub moved on: refuse, keep the draft, write nothing.
        fake.state().head_sha = "b".repeat(40);
        let refused = try_act(&mut gui, &confirm, PrTriageAction::ConfirmWrite).unwrap_err();
        assert!(refused.message.contains("new commits"));
        let after = poll(&mut gui, &confirm.workflow_id).unwrap();
        assert!(after.write_confirm.is_none());
        assert!(after.reply.is_some());
        assert!(fake.state().writes.is_empty());

        // A failed write keeps the pane and the draft.
        fake.state().head_sha = HEAD.into();
        fake.state().fail_writes = true;
        let confirm = act(
            &mut gui,
            &after,
            PrTriageAction::PrepareReply {
                comment_id: 101,
                body: "The caller already guards empty input.".into(),
            },
        );
        assert!(try_act(&mut gui, &confirm, PrTriageAction::ConfirmWrite).is_err());
        let after = poll(&mut gui, &confirm.workflow_id).unwrap();
        assert_eq!(after.stage, "review");
        assert!(after.reply.is_some());

        fake.state().fail_writes = false;
        let confirm = act(
            &mut gui,
            &after,
            PrTriageAction::PrepareReply {
                comment_id: 101,
                body: "The caller already guards empty input.".into(),
            },
        );
        let posted = act(&mut gui, &confirm, PrTriageAction::ConfirmWrite);
        assert_eq!(
            fake.state().writes,
            ["reply 101: The caller already guards empty input.\n\n— posted via AMF"]
        );
        assert!(posted.reply.is_none());
        let row = comment_view(&posted, 101);
        assert_eq!(row.triage, "skipped");
        assert_eq!(
            row.local_note.as_deref(),
            Some("The caller already guards empty input.")
        );
    }

    #[test]
    fn resolving_rechecks_the_thread_and_writes_only_on_confirmation() {
        let (_dir, mut gui, fake, view) = opened();
        let pending = act(
            &mut gui,
            &view,
            PrTriageAction::RequestResolve { comment_id: 101 },
        );
        assert_eq!(pending.write_confirm.as_ref().unwrap().kind, "resolve");
        let cancelled = act(&mut gui, &pending, PrTriageAction::CancelWrite);
        assert!(cancelled.write_confirm.is_none());

        let pending = act(
            &mut gui,
            &cancelled,
            PrTriageAction::RequestResolve { comment_id: 101 },
        );
        // Someone resolved it on GitHub meanwhile.
        fake.state().threads[0].1 = true;
        let refused = try_act(&mut gui, &pending, PrTriageAction::ConfirmWrite).unwrap_err();
        assert!(refused.message.contains("already resolved"));
        let current = poll(&mut gui, &pending.workflow_id).unwrap();
        assert!(comment_view(&current, 101).resolved);
        assert!(fake.state().writes.is_empty());

        let reopen = act(
            &mut gui,
            &current,
            PrTriageAction::RequestResolve { comment_id: 101 },
        );
        assert_eq!(reopen.write_confirm.as_ref().unwrap().kind, "reopen");
        let reopened = act(&mut gui, &reopen, PrTriageAction::ConfirmWrite);
        assert!(!comment_view(&reopened, 101).resolved);
        assert_eq!(fake.state().writes, ["resolve THREAD_A: false"]);
    }

    #[test]
    fn deleted_features_and_other_workflows_close_or_refuse_triage() {
        let (_dir, mut gui, _fake, view) = opened();
        let other = FeatureTarget {
            project_id: view.target.project_id.clone(),
            feature_id: "other".into(),
        };
        assert!(begin(&mut gui, other).is_err());
        assert!(crate::gui_learning::begin(&mut gui, view.target.clone()).is_err());
        let app = gui.app_for_workflow();
        app.store.projects[0].features.clear();
        if let Some(db) = &app.db {
            db.save_store(&app.store).unwrap();
        }
        app.store_version = None;
        let error = poll(&mut gui, &view.workflow_id).unwrap_err();
        assert_eq!(error.kind, crate::gui_contract::GuiErrorKind::NotFound);
        assert!(gui.pr_triage_context.is_none());
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
    }
}
