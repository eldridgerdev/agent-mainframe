//! Desktop fork orchestration: fork a feature (optionally from one of its
//! sessions) into a new worktree that inherits the source's *working* content.
//!
//! The risky parts are the shared foundation's, not this module's: the
//! working-change snapshot and transfer ([`crate::fork_snapshot`]), the exact
//! commit checkout ([`crate::traits::WorktreeOps::create_fork_base`]), the
//! exact-session transcript capture ([`crate::transcript::CapturedContext`])
//! and the settings/context population shared with the TUI
//! ([`crate::fork_snapshot::populate`]). This module only sequences them for a
//! window that cannot answer a prompt in the middle of an operation:
//!
//! 1. [`prepare`] resolves the source by stable ids, captures the snapshot and
//!    (optionally) the transcript outside the GUI lock and stores them under a
//!    random token. The returned [`ForkPreview`] states what will be inherited,
//!    what will not (staging), and exactly why a transcript is or is not
//!    available.
//! 2. [`confirm`] takes the token plus an echo of the identities the user saw
//!    (project, feature, session, branch, HEAD). Anything that differs, or a
//!    source that moved on, is refused, never silently retargeted. The work
//!    runs off the lock in the order check -> create base -> populate ->
//!    register stopped -> hook -> (explicitly) start.
//! 3. A token is single use. A repeat confirm returns the recorded outcome and
//!    never forks twice; [`cancel`] before creation leaves no checkout, and
//!    after creation the outcome names the retained checkout and feature.
//!
//! Nothing here launches an agent unless the caller asks for it, after the
//! shared availability and resource checks have passed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::app::{App, StartIntent};
use crate::automation::AutomationHookPrompt;
use crate::fork_snapshot::{self, Entry, ProjectSettings, Snapshot};
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};
use crate::project::{AgentKind, Feature, ProjectStatus, ProjectStore, SessionKind, VibeMode};
use crate::transcript::CapturedContext;

/// Shown with every preview: the index is observed for freshness only.
pub const STAGING_NOTE: &str = "Working-tree content is copied relative to the source HEAD. \
Git staging choices are not copied: the new checkout starts with an empty index at that HEAD. \
Ignored files are not copied. Symbolic links are copied as links, not as their targets.";

const MAX_RECORDS: usize = 8;
const MAX_LISTED_CHANGES: usize = 200;

type ContextCapture = Arc<dyn Fn(&Path, &str) -> anyhow::Result<CapturedContext> + Send + Sync>;

/// Server-side memory of previews and runs. Lives in [`GuiHandle`].
pub struct ForkState {
    records: HashMap<String, Record>,
    /// Insertion order, oldest first, so eviction never drops a running fork.
    order: Vec<String>,
    capture_context: ContextCapture,
    next: u64,
}

impl Default for ForkState {
    fn default() -> Self {
        Self {
            records: HashMap::new(),
            order: Vec::new(),
            capture_context: Arc::new(CapturedContext::capture),
            next: 0,
        }
    }
}

impl ForkState {
    fn insert(&mut self, token: String, record: Record) {
        self.order.retain(|existing| existing != &token);
        self.order.push(token.clone());
        self.records.insert(token, record);
        while self.order.len() > MAX_RECORDS {
            let Some(evict) = self
                .order
                .iter()
                .position(|t| !matches!(self.records.get(t), Some(Record::Running(_))))
            else {
                break;
            };
            let token = self.order.remove(evict);
            self.records.remove(&token);
        }
    }

    fn drop_record(&mut self, token: &str) {
        self.records.remove(token);
        self.order.retain(|existing| existing != token);
    }

    fn new_token(&mut self) -> String {
        self.next += 1;
        format!("fork-{}-{}", uuid::Uuid::new_v4().simple(), self.next)
    }

    /// Test seam: the real transcript root is the user's home directory.
    #[cfg(test)]
    pub(crate) fn set_context_capture(&mut self, capture: ContextCapture) {
        self.capture_context = capture;
    }
}

enum Record {
    Prepared(Box<Prepared>),
    Running(Arc<Run>),
    Done(Box<ForkOutcome>),
}

struct Run {
    cancel: AtomicBool,
    phase: Mutex<ForkPhase>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkPhase {
    Checking,
    CreatingCheckout,
    Transferring,
    Registering,
    Hook,
    Starting,
}

/// The identities a fork is bound to. Everything the TUI compares at confirm
/// time (`create_forked_feature`), by stable ids rather than indices.
#[derive(Clone)]
struct SourceBinding {
    project_id: String,
    project_name: String,
    project_repo: PathBuf,
    feature_id: String,
    feature_name: String,
    workdir: PathBuf,
    branch: String,
    agent: AgentKind,
    tmux_session: String,
    is_worktree: bool,
    stopped: bool,
    mode: VibeMode,
    review: bool,
    enable_chrome: bool,
    remote_control: bool,
    session: Option<SessionBinding>,
}

#[derive(Clone)]
struct SessionBinding {
    id: String,
    label: String,
    kind: SessionKind,
    tmux_window: String,
    stopped: bool,
    claude_session_id: Option<String>,
}

impl SourceBinding {
    fn resolve(
        store: &ProjectStore,
        target: &FeatureTarget,
        session_id: Option<&str>,
    ) -> GuiResult<Self> {
        let project = store
            .projects
            .iter()
            .find(|p| p.id == target.project_id)
            .ok_or_else(|| GuiError::not_found("Fork source project no longer exists"))?;
        let feature = project
            .features
            .iter()
            .find(|f| f.id == target.feature_id)
            .ok_or_else(|| GuiError::not_found("Fork source feature no longer exists"))?;
        let session = match session_id {
            Some(id) => Some(
                feature
                    .sessions
                    .iter()
                    .find(|s| s.id == id)
                    .ok_or_else(|| GuiError::not_found("Fork source session no longer exists"))?,
            ),
            // Same default as the TUI's F on a feature row.
            None => feature
                .sessions
                .iter()
                .find(|s| s.kind == SessionKind::Claude),
        };
        Ok(Self {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            project_repo: project.repo.clone(),
            feature_id: feature.id.clone(),
            feature_name: feature.name.clone(),
            workdir: feature.workdir.clone(),
            branch: feature.branch.clone(),
            agent: feature.agent.clone(),
            tmux_session: feature.tmux_session.clone(),
            is_worktree: feature.is_worktree,
            stopped: feature.status == ProjectStatus::Stopped,
            mode: feature.mode.clone(),
            review: feature.review,
            enable_chrome: feature.enable_chrome,
            remote_control: feature.remote_control,
            session: session.map(|s| SessionBinding {
                id: s.id.clone(),
                label: s.label.clone(),
                kind: s.kind.clone(),
                tmux_window: s.tmux_window.clone(),
                stopped: s.stopped,
                claude_session_id: (s.kind == SessionKind::Claude)
                    .then(|| s.claude_session_id.clone())
                    .flatten(),
            }),
        })
    }

    /// The source must still be exactly what was previewed.
    fn verify(&self, store: &ProjectStore) -> GuiResult<()> {
        let changed = |what: &str| {
            GuiError::conflict(format!(
                "Fork source {what} changed since the preview; prepare the fork again"
            ))
        };
        let current = Self::resolve(
            store,
            &FeatureTarget {
                project_id: self.project_id.clone(),
                feature_id: self.feature_id.clone(),
            },
            self.session.as_ref().map(|s| s.id.as_str()),
        )
        .map_err(|err| GuiError::conflict(format!("{}; prepare the fork again", err.message)))?;
        if current.project_name != self.project_name || current.project_repo != self.project_repo {
            return Err(changed("project"));
        }
        // Idle/Active flip as the agent works; only liveness matters.
        if current.workdir != self.workdir
            || current.branch != self.branch
            || current.agent != self.agent
            || current.tmux_session != self.tmux_session
            || current.is_worktree != self.is_worktree
            || current.stopped != self.stopped
            || current.mode != self.mode
            || current.review != self.review
            || current.enable_chrome != self.enable_chrome
            || current.remote_control != self.remote_control
        {
            return Err(changed("feature"));
        }
        match (&self.session, &current.session) {
            (None, _) => Ok(()),
            (Some(a), Some(b))
                if a.kind == b.kind
                    && a.tmux_window == b.tmux_window
                    && a.stopped == b.stopped
                    && a.claude_session_id == b.claude_session_id =>
            {
                Ok(())
            }
            _ => Err(changed("session")),
        }
    }
}

struct Prepared {
    binding: SourceBinding,
    snapshot: Snapshot,
    /// `Some` only when the exact session's transcript was read and exported.
    context: Option<CapturedContext>,
    context_view: ContextAvailability,
}

// ---- wire types -----------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ForkPrepareRequest {
    pub target: FeatureTarget,
    /// The session the fork is made from. `None` means the feature's first
    /// Claude session, if any, exactly as the TUI's `F` on a feature row.
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForkPreview {
    pub token: String,
    pub source: ForkSourceView,
    pub suggested_branch: String,
    /// The agent the fork defaults to (the source's, normalized to what the
    /// project allows).
    pub default_agent: String,
    pub mode: String,
    pub changes: ForkChanges,
    pub staging_note: &'static str,
    pub context: ContextAvailability,
    pub hook: Option<AutomationHookPrompt>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForkSourceView {
    pub project_id: String,
    pub project_name: String,
    pub feature_id: String,
    pub feature_name: String,
    pub branch: String,
    pub workdir: String,
    pub head: String,
    pub session: Option<ForkSessionView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForkSessionView {
    pub id: String,
    pub label: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForkChanges {
    pub total: usize,
    pub listed: Vec<ForkChange>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForkChange {
    /// Lossy display only; the transfer uses the exact bytes.
    pub path: String,
    pub kind: ForkChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkChangeKind {
    File,
    Executable,
    Symlink,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextStatus {
    Available,
    NoSession,
    /// Only Claude Code sessions have a transcript exporter.
    UnsupportedHarness,
    /// A Claude session that has no recorded conversation id yet.
    NotBound,
    Unreadable,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextAvailability {
    pub status: ContextStatus,
    pub message: String,
    pub exported_chars: Option<usize>,
}

impl ContextAvailability {
    pub fn available(&self) -> bool {
        self.status == ContextStatus::Available
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ForkExpected {
    pub project_id: String,
    pub feature_id: String,
    pub session_id: Option<String>,
    pub branch: String,
    pub head: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ForkConfirmRequest {
    pub token: String,
    pub expected: ForkExpected,
    pub new_branch: String,
    pub agent: AgentKind,
    pub include_context: bool,
    pub hook_choice: Option<String>,
    /// Start the harness after the fork is registered. `false` leaves the new
    /// feature stopped.
    pub start_agent: bool,
    pub approve_resources: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForkRetained {
    pub workdir: String,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForkHookReport {
    pub succeeded: bool,
    pub cancelled: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ForkOutcome {
    /// The fork exists and is registered. `started` says whether its agent
    /// was launched; `notes` carries anything the user should still know.
    Created {
        feature: FeatureTarget,
        branch: String,
        workdir: String,
        started: bool,
        transferred: usize,
        context_written: bool,
        hook: Option<ForkHookReport>,
        notes: Vec<String>,
        message: String,
    },
    /// Cancelled before a checkout existed.
    Cancelled { message: String },
    /// A checkout may exist but the fork is not complete. No agent started.
    Failed {
        phase: ForkPhase,
        retained: Option<ForkRetained>,
        feature: Option<FeatureTarget>,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkRecordState {
    Prepared,
    Running,
    Done,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForkStatus {
    pub state: ForkRecordState,
    pub phase: Option<ForkPhase>,
    pub cancel_requested: bool,
    pub outcome: Option<ForkOutcome>,
}

fn kind_name(kind: &SessionKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{kind:?}").to_lowercase())
}

// ---- prepare --------------------------------------------------------------

struct PrepareInputs {
    binding: SourceBinding,
    capture_context: ContextCapture,
    hook_prompt: Option<AutomationHookPrompt>,
    suggested_branch: String,
    default_agent: AgentKind,
}

fn suggested_branch(binding: &SourceBinding, store: &ProjectStore) -> String {
    let taken = |name: &str| {
        store
            .projects
            .iter()
            .find(|p| p.id == binding.project_id)
            .is_some_and(|p| p.features.iter().any(|f| f.name == name))
    };
    let base = format!("{}-fork", binding.branch);
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !taken(name))
        .expect("an unused suffix exists")
}

pub fn prepare(gui: &Mutex<GuiHandle>, request: ForkPrepareRequest) -> GuiResult<ForkPreview> {
    // Names the exact source under the lock; the heavy reads run without it.
    let inputs = {
        let mut gui = gui.lock().expect("gui handle mutex poisoned");
        gui.refresh_store()?;
        let capture_context = gui.fork.capture_context.clone();
        let app = gui.app_for_workflow();
        let binding =
            SourceBinding::resolve(&app.store, &request.target, request.session_id.as_deref())?;
        let project = app
            .store
            .projects
            .iter()
            .find(|p| p.id == binding.project_id)
            .expect("resolved above");
        if !project.is_git {
            return Err(GuiError::conflict(
                "Forking needs a Git project: the fork is a new worktree at the source's HEAD",
            ));
        }
        let hook_prompt = app.worktree_hook_prompt_for_repo(&binding.project_repo);
        let suggested = suggested_branch(&binding, &app.store);
        let (default_agent, _) =
            app.normalize_agent_for_repo(&binding.project_repo, &binding.agent);
        PrepareInputs {
            binding,
            capture_context,
            hook_prompt,
            suggested_branch: suggested,
            default_agent,
        }
    };

    let binding = &inputs.binding;
    let snapshot = Snapshot::capture(&binding.workdir)
        .map_err(|err| GuiError::conflict(format!("Cannot prepare fork: {err:#}")))?;
    if !snapshot.is_branch(&binding.branch) {
        return Err(GuiError::conflict(format!(
            "Fork source checkout is not on its recorded branch {:?}; check the source before forking",
            binding.branch
        )));
    }
    let (context, context_view) = capture_context(binding, &inputs.capture_context);

    let mut gui = gui.lock().expect("gui handle mutex poisoned");
    gui.refresh_store()?;
    // The source may have been edited, stopped or deleted while we read it.
    inputs.binding.verify(&gui.app_for_workflow().store)?;
    let token = gui.fork.new_token();
    let changes = changes_view(&snapshot);
    let preview = ForkPreview {
        token: token.clone(),
        source: ForkSourceView {
            project_id: binding.project_id.clone(),
            project_name: binding.project_name.clone(),
            feature_id: binding.feature_id.clone(),
            feature_name: binding.feature_name.clone(),
            branch: binding.branch.clone(),
            workdir: binding.workdir.display().to_string(),
            head: snapshot.head.clone(),
            session: binding.session.as_ref().map(|s| ForkSessionView {
                id: s.id.clone(),
                label: s.label.clone(),
                kind: kind_name(&s.kind),
            }),
        },
        suggested_branch: inputs.suggested_branch.clone(),
        default_agent: inputs.default_agent.slug().to_string(),
        mode: format!("{:?}", binding.mode).to_lowercase(),
        changes,
        staging_note: STAGING_NOTE,
        context: context_view.clone(),
        hook: inputs.hook_prompt.clone(),
    };
    let PrepareInputs { binding, .. } = inputs;
    gui.fork.insert(
        token,
        Record::Prepared(Box::new(Prepared {
            binding,
            snapshot,
            context,
            context_view,
        })),
    );
    Ok(preview)
}

/// Decide, and say why, whether this exact session has a transcript to carry.
fn capture_context(
    binding: &SourceBinding,
    capture: &ContextCapture,
) -> (Option<CapturedContext>, ContextAvailability) {
    let unavailable = |status, message: String| {
        (
            None,
            ContextAvailability {
                status,
                message,
                exported_chars: None,
            },
        )
    };
    let Some(session) = &binding.session else {
        return unavailable(
            ContextStatus::NoSession,
            "This feature has no Claude session to take a conversation from, so the fork starts without context."
                .into(),
        );
    };
    if session.kind != SessionKind::Claude {
        return unavailable(
            ContextStatus::UnsupportedHarness,
            format!(
                "Conversation export is only available for Claude Code sessions; \"{}\" is a {} session, so it has no exportable transcript. The fork still inherits the working changes.",
                session.label,
                kind_name(&session.kind)
            ),
        );
    }
    let Some(claude_id) = &session.claude_session_id else {
        return unavailable(
            ContextStatus::NotBound,
            format!(
                "\"{}\" has no recorded Claude conversation yet, so there is nothing to export.",
                session.label
            ),
        );
    };
    match capture(&binding.workdir, claude_id) {
        Ok(context) => {
            let chars = context.markdown.chars().count();
            (
                Some(context),
                ContextAvailability {
                    status: ContextStatus::Available,
                    message: format!(
                        "The conversation of \"{}\" will be written to .claude/context.md in the fork ({chars} characters, fixed at this preview).",
                        session.label
                    ),
                    exported_chars: Some(chars),
                },
            )
        }
        Err(err) => unavailable(
            ContextStatus::Unreadable,
            format!(
                "The transcript of \"{}\" could not be read: {err:#}. Continue without context, or fix the transcript and prepare again.",
                session.label
            ),
        ),
    }
}

fn changes_view(snapshot: &Snapshot) -> ForkChanges {
    let kind = |entry: &Entry| match entry {
        Entry::Missing => ForkChangeKind::Deleted,
        Entry::Regular {
            executable: true, ..
        } => ForkChangeKind::Executable,
        Entry::Regular { .. } => ForkChangeKind::File,
        Entry::Symlink(_) => ForkChangeKind::Symlink,
    };
    ForkChanges {
        total: snapshot.entries.len(),
        listed: snapshot
            .entries
            .iter()
            .take(MAX_LISTED_CHANGES)
            .map(|(path, entry)| ForkChange {
                path: path.to_string_lossy().into_owned(),
                kind: kind(entry),
            })
            .collect(),
        truncated: snapshot.entries.len() > MAX_LISTED_CHANGES,
    }
}

// ---- cancel / status ------------------------------------------------------

pub fn cancel(gui: &Mutex<GuiHandle>, token: &str) -> GuiResult<ForkStatus> {
    let mut gui = gui.lock().expect("gui handle mutex poisoned");
    match gui.fork.records.get(token) {
        None => Err(GuiError::not_found(
            "That fork preview is gone; nothing was created",
        )),
        Some(Record::Prepared(_)) => {
            gui.fork.drop_record(token);
            Ok(ForkStatus {
                state: ForkRecordState::Done,
                phase: None,
                cancel_requested: true,
                outcome: Some(ForkOutcome::Cancelled {
                    message: "Fork cancelled before anything was created".into(),
                }),
            })
        }
        Some(Record::Running(run)) => {
            run.cancel.store(true, Ordering::SeqCst);
            Ok(status_of(gui.fork.records.get(token).expect("present")))
        }
        Some(done) => Ok(status_of(done)),
    }
}

pub fn status(gui: &Mutex<GuiHandle>, token: &str) -> GuiResult<ForkStatus> {
    let gui = gui.lock().expect("gui handle mutex poisoned");
    gui.fork
        .records
        .get(token)
        .map(status_of)
        .ok_or_else(|| GuiError::not_found("That fork preview is gone; prepare again"))
}

fn status_of(record: &Record) -> ForkStatus {
    match record {
        Record::Prepared(_) => ForkStatus {
            state: ForkRecordState::Prepared,
            phase: None,
            cancel_requested: false,
            outcome: None,
        },
        Record::Running(run) => ForkStatus {
            state: ForkRecordState::Running,
            phase: Some(*run.phase.lock().expect("fork phase poisoned")),
            cancel_requested: run.cancel.load(Ordering::SeqCst),
            outcome: None,
        },
        Record::Done(outcome) => ForkStatus {
            state: ForkRecordState::Done,
            phase: None,
            cancel_requested: false,
            outcome: Some((**outcome).clone()),
        },
    }
}

// ---- confirm --------------------------------------------------------------

/// What the lock-held validation hands to the unlocked worker.
struct Plan {
    prepared: Box<Prepared>,
    run: Arc<Run>,
    new_branch: String,
    agent: AgentKind,
    include_context: bool,
    hook: Option<HookPlan>,
    start_agent: bool,
    worktree: Arc<dyn crate::traits::WorktreeOps>,
}

struct HookPlan {
    script: String,
    choice: Option<String>,
}

pub fn confirm(gui: &Mutex<GuiHandle>, request: ForkConfirmRequest) -> GuiResult<ForkOutcome> {
    let plan = {
        let mut gui = gui.lock().expect("gui handle mutex poisoned");
        match begin(&mut gui, &request)? {
            Begin::Repeat(outcome) => return Ok(outcome),
            Begin::Run(plan) => plan,
        }
    };
    let token = request.token.clone();
    let (prepared, result) = execute(gui, plan);
    let mut gui = gui.lock().expect("gui handle mutex poisoned");
    match result {
        Step::Finished(outcome) => {
            gui.fork
                .insert(token, Record::Done(Box::new(outcome.clone())));
            Ok(outcome)
        }
        Step::Refused { error, reusable } => {
            if reusable && let Some(prepared) = prepared {
                gui.fork.insert(token, Record::Prepared(prepared));
            } else {
                gui.fork.drop_record(&token);
            }
            Err(error)
        }
    }
}

enum Begin {
    Repeat(ForkOutcome),
    Run(Plan),
}

fn begin(gui: &mut GuiHandle, request: &ForkConfirmRequest) -> GuiResult<Begin> {
    gui.refresh_store()?;
    match gui.fork.records.get(&request.token) {
        None => {
            return Err(GuiError::not_found(
                "That fork preview expired or was already used; prepare the fork again",
            ));
        }
        Some(Record::Running(_)) => {
            return Err(GuiError::conflict(
                "This fork is already in progress; wait for its result",
            ));
        }
        Some(Record::Done(outcome)) => return Ok(Begin::Repeat((**outcome).clone())),
        Some(Record::Prepared(_)) => (),
    }
    let Some(Record::Prepared(prepared)) = gui.fork.records.remove(&request.token) else {
        unreachable!("checked above");
    };
    // From here the record is out of the map; every exit puts something back
    // or deliberately drops the token.
    let restore = |gui: &mut GuiHandle, prepared: Box<Prepared>| {
        gui.fork
            .records
            .insert(request.token.clone(), Record::Prepared(prepared));
    };
    let expected = &request.expected;
    let binding = &prepared.binding;
    let session_id = binding.session.as_ref().map(|s| s.id.as_str());
    if expected.project_id != binding.project_id
        || expected.feature_id != binding.feature_id
        || expected.session_id.as_deref() != session_id
        || expected.branch != binding.branch
        || expected.head != prepared.snapshot.head
    {
        restore(gui, prepared);
        return Err(GuiError::conflict(
            "The confirmation does not match the preview it was made from (source, session, branch or HEAD differ); prepare the fork again",
        ));
    }
    if let Err(err) = binding.verify(&gui.app_for_workflow().store) {
        gui.fork.drop_record(&request.token);
        return Err(err);
    }
    let new_branch = request.new_branch.trim().to_string();
    let refuse = |gui: &mut GuiHandle, prepared: Box<Prepared>, error: GuiError| {
        restore(gui, prepared);
        Err(error)
    };
    if new_branch.is_empty() {
        return refuse(
            gui,
            prepared,
            GuiError::conflict("Branch name cannot be empty"),
        );
    }
    if request.include_context && !prepared.context_view.available() {
        let message = format!(
            "Context cannot be included: {} Turn context off to fork without it.",
            prepared.context_view.message
        );
        return refuse(gui, prepared, GuiError::conflict(message));
    }
    let hook_plan = match validate_request(gui.app_for_workflow(), &prepared, request, &new_branch)
    {
        Ok(plan) => plan,
        Err(error) => return refuse(gui, prepared, error),
    };
    if request.start_agent
        && !request.approve_resources
        && let Err(error) = gui.require_start_approval(&format!("Starting fork '{new_branch}'"))
    {
        return refuse(gui, prepared, error);
    }
    let run = Arc::new(Run {
        cancel: AtomicBool::new(false),
        phase: Mutex::new(ForkPhase::Checking),
    });
    gui.fork
        .records
        .insert(request.token.clone(), Record::Running(run.clone()));
    let worktree = gui.app_for_workflow().worktree.clone();
    Ok(Begin::Run(Plan {
        prepared,
        run,
        new_branch,
        agent: request.agent.clone(),
        include_context: request.include_context,
        hook: hook_plan,
        start_agent: request.start_agent,
        worktree,
    }))
}

/// Checks that need `App` but not the checkout: names, policy, hook choice and
/// harness availability. Everything here runs before anything is created.
fn validate_request(
    app: &mut App,
    prepared: &Prepared,
    request: &ForkConfirmRequest,
    new_branch: &str,
) -> GuiResult<Option<HookPlan>> {
    let repo = prepared.binding.project_repo.clone();
    let project = app
        .store
        .projects
        .iter()
        .find(|p| p.id == prepared.binding.project_id)
        .ok_or_else(|| GuiError::not_found("Fork source project no longer exists"))?;
    if project.features.iter().any(|f| f.name == new_branch) {
        return Err(GuiError::conflict(format!(
            "Feature '{new_branch}' already exists"
        )));
    }
    if !app.allows_agent_for_repo(&repo, &request.agent) {
        return Err(GuiError::conflict(format!(
            "Harness '{}' is not allowed for this workspace",
            request.agent.display_name()
        )));
    }
    app.ensure_agent_mode_supported(&request.agent, &prepared.binding.mode)
        .map_err(|err| GuiError::conflict(format!("{err:#}")))?;
    app.validate_worktree_hook_choice(&repo, request.hook_choice.as_deref())?;
    if request.start_agent {
        app.tmux
            .check_harness_available(&request.agent)
            .map_err(|err| {
                GuiError::conflict(format!(
                    "Cannot start {}: {err:#}. Turn \"Start the agent\" off to create the fork stopped.",
                    request.agent.display_name()
                ))
            })?;
    }
    Ok(
        crate::extension::merge_project_extension_config(&app.config.extension, &repo)
            .lifecycle_hooks
            .on_worktree_created
            .map(|hook| HookPlan {
                script: hook.script().to_string(),
                choice: request.hook_choice.clone(),
            }),
    )
}

enum Step {
    Finished(ForkOutcome),
    Refused { error: GuiError, reusable: bool },
}

fn set_phase(run: &Run, phase: ForkPhase) {
    *run.phase.lock().expect("fork phase poisoned") = phase;
}

fn refused(error: impl Into<GuiError>, reusable: bool) -> Step {
    Step::Refused {
        error: error.into(),
        reusable,
    }
}

fn execute(gui: &Mutex<GuiHandle>, plan: Plan) -> (Option<Box<Prepared>>, Step) {
    let Plan {
        prepared,
        run,
        new_branch,
        agent,
        include_context,
        hook,
        start_agent,
        worktree,
    } = plan;
    let cancelled = || run.cancel.load(Ordering::SeqCst);
    let binding = &prepared.binding;
    let repo = binding.project_repo.clone();

    // ---- everything below runs without the GUI lock ----
    set_phase(&run, ForkPhase::Checking);
    if cancelled() {
        return (
            None,
            Step::Finished(ForkOutcome::Cancelled {
                message: "Fork cancelled before anything was created".into(),
            }),
        );
    }
    if let Err(err) = check_branch_name(&repo, &new_branch) {
        return (Some(prepared), refused(err, true));
    }
    let context = if include_context {
        prepared.context.as_ref()
    } else {
        None
    };
    let pre = || -> anyhow::Result<ProjectSettings> {
        prepared.snapshot.validate()?;
        if context.is_some() {
            fork_snapshot::ensure_context_does_not_collide(&prepared.snapshot)?;
        }
        let settings = ProjectSettings::read(&repo)?;
        fork_snapshot::ensure_no_generated_collisions(
            &prepared.snapshot,
            &settings,
            &binding.workdir,
            &repo,
        )?;
        if let Some(context) = context {
            context.validate()?;
        }
        settings.verify()?;
        prepared.snapshot.validate()?;
        Ok(settings)
    };
    let settings = match pre() {
        Ok(settings) => settings,
        Err(err) => {
            return (
                None,
                refused(
                    GuiError::conflict(format!(
                        "{err:#}. Nothing was created; prepare the fork again."
                    )),
                    false,
                ),
            );
        }
    };
    if cancelled() {
        return (
            None,
            Step::Finished(ForkOutcome::Cancelled {
                message: "Fork cancelled before anything was created".into(),
            }),
        );
    }

    set_phase(&run, ForkPhase::CreatingCheckout);
    let workdir = match worktree.create_fork_base(
        &repo,
        &new_branch,
        &new_branch,
        &prepared.snapshot.head,
    ) {
        Ok(workdir) => workdir,
        Err(err) => {
            return (
                Some(prepared),
                refused(
                    GuiError::conflict(format!(
                        "Could not create the fork checkout: {err:#}. If a checkout or branch named '{new_branch}' was left behind, it is yours to inspect."
                    )),
                    true,
                ),
            );
        }
    };
    let retained = || ForkRetained {
        workdir: workdir.display().to_string(),
        branch: new_branch.clone(),
    };

    set_phase(&run, ForkPhase::Transferring);
    if let Err(err) = fork_snapshot::populate(&workdir, &prepared.snapshot, &settings, context) {
        if !fork_snapshot::is_source_changed(&err) {
            return (
                None,
                Step::Finished(ForkOutcome::Failed {
                    phase: ForkPhase::Transferring,
                    retained: Some(retained()),
                    feature: None,
                    message: format!(
                        "Fork setup failed: {err:#}. The checkout was kept for inspection and is not registered; no agent started."
                    ),
                }),
            );
        }
        // The source moved on mid-fork; a stale copy is worth nothing and would
        // block retrying under the same branch.
        let removed =
            fork_snapshot::remove_checkout(worktree.as_ref(), &repo, &workdir, &new_branch);
        return (
            None,
            Step::Finished(match removed {
                Ok(()) => ForkOutcome::Failed {
                    phase: ForkPhase::Transferring,
                    retained: None,
                    feature: None,
                    message: format!(
                        "The source changed during the fork: {err:#}. The partial checkout and branch were removed; no agent started. Prepare the fork again."
                    ),
                },
                Err(cleanup) => ForkOutcome::Failed {
                    phase: ForkPhase::Transferring,
                    retained: Some(retained()),
                    feature: None,
                    message: format!(
                        "The source changed during the fork: {err:#}. Removing the partial checkout failed ({cleanup:#}); it was kept; no agent started."
                    ),
                },
            }),
        );
    }
    let transferred = prepared.snapshot.entries.len();
    let context_written = context.is_some();

    // ---- register stopped, so every later failure leaves a findable feature ----
    set_phase(&run, ForkPhase::Registering);
    let target = {
        let mut gui = gui.lock().expect("gui handle mutex poisoned");
        match register(&mut gui, binding, &new_branch, &workdir, &agent) {
            Ok(target) => target,
            Err(message) => {
                return (
                    None,
                    Step::Finished(ForkOutcome::Failed {
                        phase: ForkPhase::Registering,
                        retained: Some(retained()),
                        feature: None,
                        message: format!(
                            "{message}. The checkout is complete but not registered; no agent started."
                        ),
                    }),
                );
            }
        }
    };
    let mut notes = Vec::new();
    let created =
        |started: bool, hook: Option<ForkHookReport>, notes: Vec<String>, message: String| {
            Step::Finished(ForkOutcome::Created {
                feature: target.clone(),
                branch: new_branch.clone(),
                workdir: workdir.display().to_string(),
                started,
                transferred,
                context_written,
                hook,
                notes,
                message,
            })
        };

    // ---- hook ----
    let mut hook_report = None;
    if let Some(hook) = &hook {
        set_phase(&run, ForkPhase::Hook);
        let (ok, detail) = App::run_worktree_hook_cancellable(
            &hook.script,
            &workdir,
            hook.choice.as_deref(),
            Some(&run.cancel),
        );
        let was_cancelled = cancelled() && !ok;
        hook_report = Some(ForkHookReport {
            succeeded: ok,
            cancelled: was_cancelled,
            detail: detail.clone(),
        });
        if was_cancelled {
            return (
                None,
                created(
                    false,
                    hook_report,
                    vec!["The worktree hook was cancelled and its process tree stopped.".into()],
                    format!(
                        "Forked '{new_branch}' and kept it stopped: cancelled during the worktree hook."
                    ),
                ),
            );
        }
        if !ok {
            notes.push(format!(
                "The worktree hook failed ({}); the fork was created anyway.",
                detail.unwrap_or_else(|| "no detail".into())
            ));
        }
    }
    if cancelled() || !start_agent {
        let why = if cancelled() {
            notes.push("Cancelled before the agent started.".into());
            "cancelled before start"
        } else {
            "not started as requested"
        };
        return (
            None,
            created(
                false,
                hook_report,
                notes,
                format!("Forked '{new_branch}' and kept it stopped ({why})."),
            ),
        );
    }

    // ---- start, explicitly, under the shared launch primitive ----
    set_phase(&run, ForkPhase::Starting);
    let started = {
        let mut gui = gui.lock().expect("gui handle mutex poisoned");
        start(&mut gui, &target)
    };
    match started {
        Ok(()) => (
            None,
            created(
                true,
                hook_report,
                notes,
                format!(
                    "Forked '{new_branch}' and started {}.",
                    agent.display_name()
                ),
            ),
        ),
        Err(message) => {
            notes.push(format!("The agent did not start: {message}"));
            (
                None,
                created(
                    false,
                    hook_report,
                    notes,
                    format!(
                        "Forked '{new_branch}' but the agent did not start; it is stopped and can be started from the sidebar."
                    ),
                ),
            )
        }
    }
}

fn check_branch_name(repo: &Path, branch: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !branch.starts_with('-'),
        "'{branch}' is not a valid branch name"
    );
    let out = std::process::Command::new("git")
        .args(["check-ref-format", "--branch", branch])
        .current_dir(repo)
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "'{branch}' is not a valid Git branch name"
    );
    // Also the worktree directory: a plain relative name below .worktrees.
    anyhow::ensure!(
        Path::new(branch)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_))),
        "'{branch}' cannot be used as a checkout name"
    );
    Ok(())
}

fn register(
    gui: &mut GuiHandle,
    binding: &SourceBinding,
    new_branch: &str,
    workdir: &Path,
    agent: &AgentKind,
) -> Result<FeatureTarget, String> {
    if let Err(err) = gui.refresh_store() {
        return Err(format!(
            "Could not refresh before registering: {}",
            err.message
        ));
    }
    let app = gui.app_for_workflow();
    let mut feature = Feature::new_for_project(
        &binding.project_name,
        new_branch.to_string(),
        new_branch.to_string(),
        workdir.to_path_buf(),
        true,
        binding.mode.clone(),
        binding.review,
        false,
        agent.clone(),
        binding.enable_chrome,
        binding.remote_control,
    );
    App::initialize_feature_sessions(&mut feature, false, None);
    let feature_id = feature.id.clone();
    if !app.store.add_feature(&binding.project_name, feature) {
        return Err("The source project no longer exists".into());
    }
    match app.save_reporting_conflict() {
        Ok(true) => (),
        Ok(false) => return Err("Registration conflicted with another AMF change".into()),
        Err(err) => return Err(format!("Registration failed: {err:#}")),
    }
    app.store
        .locate_feature_by_id(Some(&binding.project_id), &feature_id)
        .map(|_| FeatureTarget {
            project_id: binding.project_id.clone(),
            feature_id,
        })
        .ok_or_else(|| "The registered feature disappeared".to_string())
}

fn start(gui: &mut GuiHandle, target: &FeatureTarget) -> Result<(), String> {
    gui.refresh_store().map_err(|e| e.message)?;
    let (pi, fi) = gui.locate(target).map_err(|e| e.message)?;
    let app = gui.app_for_workflow();
    app.ensure_feature_running(pi, fi, StartIntent::Approved)
        .map_err(|err| format!("{err:#}"))?;
    match app.save_reporting_conflict() {
        Ok(true) => Ok(()),
        Ok(false) => Err("saving the started state conflicted with another AMF change".into()),
        Err(err) => Err(format!("{err:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::gui_contract::GuiErrorKind;
    use crate::project::Project;
    use crate::traits::{MockTmuxOps, MockWorktreeOps, WorktreeOps};
    use crate::worktree::WorktreeManager;
    use std::process::Command;
    use tempfile::TempDir;

    const PROJECT: &str = "p1";
    const FEATURE: &str = "f1";
    const CLAUDE: &str = "s-claude";
    const CODEX: &str = "s-codex";

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    struct Fixture {
        repo: TempDir,
        _transcripts: TempDir,
        gui: Arc<Mutex<GuiHandle>>,
    }

    fn repo(hook: Option<&str>) -> TempDir {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q"]);
        git(repo.path(), &["checkout", "-qb", "my-feat"]);
        std::fs::write(repo.path().join("file"), b"head").unwrap();
        let config = match hook {
            Some(script) => serde_json::json!({
                "allowed_agents": ["claude"],
                "lifecycle_hooks": {"on_worktree_created": {
                    "script": script,
                    "prompt": {"title": "Choose setup", "options": ["Continue", "Other"]}
                }}
            }),
            None => serde_json::json!({"allowed_agents": ["claude"]}),
        };
        std::fs::write(repo.path().join("amf.json"), config.to_string()).unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "initial"]);
        repo
    }

    fn fixture_with(
        hook: Option<&str>,
        tmux: MockTmuxOps,
        worktree: Box<dyn WorktreeOps>,
        edit: impl FnOnce(&mut Project),
    ) -> Fixture {
        let repo = repo(hook);
        let mut project = Project::new(
            "demo".into(),
            repo.path().to_path_buf(),
            true,
            AgentKind::Claude,
        );
        project.id = PROJECT.into();
        let mut feature = Feature::new_for_project(
            "demo",
            "my-feat".into(),
            "my-feat".into(),
            repo.path().to_path_buf(),
            false,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        feature.id = FEATURE.into();
        let claude = feature.add_session(SessionKind::Claude);
        claude.id = CLAUDE.into();
        claude.claude_session_id = Some("claude-conversation".into());
        feature.add_session(SessionKind::Codex).id = CODEX.into();
        project.features.push(feature);
        edit(&mut project);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let mut app = App::new_for_test(store, Box::new(tmux), worktree);
        app.store_path = repo.path().join("store.json");
        let mut gui = GuiHandle::from_app(app);
        // The transcript root is the real home directory; read a fixture file.
        let transcripts = TempDir::new().unwrap();
        let transcript = transcripts.path().join("conversation.jsonl");
        std::fs::write(
            &transcript,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"selected request\"}}\n",
        )
        .unwrap();
        gui.fork.set_context_capture(Arc::new(move |_, id| {
            anyhow::ensure!(id == "claude-conversation", "unexpected session {id}");
            CapturedContext::from_path(transcript.clone())
        }));
        Fixture {
            repo,
            _transcripts: transcripts,
            gui: Arc::new(Mutex::new(gui)),
        }
    }

    fn fixture() -> Fixture {
        fixture_with(None, MockTmuxOps::new(), Box::new(WorktreeManager), |_| ())
    }

    fn target() -> FeatureTarget {
        FeatureTarget {
            project_id: PROJECT.into(),
            feature_id: FEATURE.into(),
        }
    }

    fn prepare_for(fx: &Fixture, session: Option<&str>) -> GuiResult<ForkPreview> {
        prepare(
            &fx.gui,
            ForkPrepareRequest {
                target: target(),
                session_id: session.map(str::to_string),
            },
        )
    }

    fn request(preview: &ForkPreview) -> ForkConfirmRequest {
        ForkConfirmRequest {
            token: preview.token.clone(),
            expected: ForkExpected {
                project_id: preview.source.project_id.clone(),
                feature_id: preview.source.feature_id.clone(),
                session_id: preview.source.session.as_ref().map(|s| s.id.clone()),
                branch: preview.source.branch.clone(),
                head: preview.source.head.clone(),
            },
            new_branch: preview.suggested_branch.clone(),
            agent: AgentKind::Claude,
            include_context: false,
            hook_choice: None,
            start_agent: false,
            approve_resources: false,
        }
    }

    fn fork_dir(fx: &Fixture) -> PathBuf {
        fx.repo
            .path()
            .canonicalize()
            .unwrap()
            .join(".worktrees/my-feat-fork")
    }

    fn feature_count(fx: &Fixture) -> usize {
        fx.gui.lock().unwrap().app_for_workflow().store.projects[0]
            .features
            .len()
    }

    #[test]
    fn preview_binds_head_session_and_working_changes() {
        let fx = fixture();
        std::fs::write(fx.repo.path().join("file"), b"working").unwrap();
        std::fs::write(fx.repo.path().join("new.txt"), b"untracked").unwrap();
        let head = git(fx.repo.path(), &["rev-parse", "HEAD"]);

        let preview = prepare_for(&fx, None).unwrap();

        assert_eq!(preview.source.head, head.trim());
        assert_eq!(preview.source.branch, "my-feat");
        assert_eq!(preview.source.session.as_ref().unwrap().id, CLAUDE);
        assert_eq!(preview.suggested_branch, "my-feat-fork");
        assert_eq!(preview.changes.total, 2);
        let paths: Vec<_> = preview
            .changes
            .listed
            .iter()
            .map(|c| c.path.as_str())
            .collect();
        assert_eq!(paths, ["file", "new.txt"]);
        assert!(
            preview
                .staging_note
                .contains("staging choices are not copied")
        );
        assert_eq!(preview.context.status, ContextStatus::Available);
        assert!(preview.context.exported_chars.unwrap() > 0);
        assert!(preview.hook.is_none());
        // Reading the preview created nothing.
        assert!(!fork_dir(&fx).exists());
        assert_eq!(feature_count(&fx), 1);
    }

    #[test]
    fn context_availability_is_disclosed_for_every_unavailable_case() {
        let fx = fixture();
        let codex = prepare_for(&fx, Some(CODEX)).unwrap();
        assert_eq!(codex.context.status, ContextStatus::UnsupportedHarness);
        assert!(
            codex
                .context
                .message
                .contains("only available for Claude Code")
        );

        let fx = fixture_with(None, MockTmuxOps::new(), Box::new(WorktreeManager), |p| {
            p.features[0].sessions[0].claude_session_id = None;
        });
        let unbound = prepare_for(&fx, Some(CLAUDE)).unwrap();
        assert_eq!(unbound.context.status, ContextStatus::NotBound);

        let fx = fixture_with(None, MockTmuxOps::new(), Box::new(WorktreeManager), |p| {
            p.features[0].sessions.clear();
        });
        let none = prepare_for(&fx, None).unwrap();
        assert_eq!(none.context.status, ContextStatus::NoSession);
        assert!(none.source.session.is_none());

        let fx = fixture();
        fx.gui
            .lock()
            .unwrap()
            .fork
            .set_context_capture(Arc::new(|_, _| anyhow::bail!("transcript is gone")));
        let unreadable = prepare_for(&fx, None).unwrap();
        assert_eq!(unreadable.context.status, ContextStatus::Unreadable);
        assert!(unreadable.context.message.contains("transcript is gone"));
    }

    #[test]
    fn unavailable_context_is_refused_not_silently_dropped() {
        let fx = fixture();
        let preview = prepare_for(&fx, Some(CODEX)).unwrap();
        let mut asked = request(&preview);
        asked.include_context = true;

        let error = confirm(&fx.gui, asked).unwrap_err();

        assert_eq!(error.kind, GuiErrorKind::Conflict);
        assert!(
            error.message.contains("Context cannot be included"),
            "{}",
            error.message
        );
        assert!(!fork_dir(&fx).exists());
        // The preview stays usable once context is turned off.
        let outcome = confirm(&fx.gui, request(&preview)).unwrap();
        assert!(matches!(
            outcome,
            ForkOutcome::Created {
                context_written: false,
                ..
            }
        ));
    }

    #[test]
    fn confirm_creates_a_stopped_fork_with_working_content_and_context() {
        // No expectation on this tmux: any harness launch would panic.
        let fx = fixture();
        std::fs::write(fx.repo.path().join("file"), b"working").unwrap();
        std::fs::write(fx.repo.path().join("new.txt"), b"untracked").unwrap();
        let preview = prepare_for(&fx, None).unwrap();
        let mut asked = request(&preview);
        asked.include_context = true;

        let outcome = confirm(&fx.gui, asked.clone()).unwrap();

        let ForkOutcome::Created {
            feature,
            started,
            transferred,
            context_written,
            workdir,
            ..
        } = &outcome
        else {
            panic!("{outcome:?}");
        };
        assert!(!started);
        assert_eq!(*transferred, 2);
        assert!(context_written);
        let dest = PathBuf::from(workdir);
        assert_eq!(std::fs::read(dest.join("file")).unwrap(), b"working");
        assert_eq!(std::fs::read(dest.join("new.txt")).unwrap(), b"untracked");
        let context = std::fs::read_to_string(dest.join(".claude/context.md")).unwrap();
        assert!(context.contains("selected request"));
        // The destination index is at the source HEAD: nothing is staged.
        assert!(git(&dest, &["diff", "--cached", "--name-only"]).is_empty());
        {
            let mut gui = fx.gui.lock().unwrap();
            let app = gui.app_for_workflow();
            let fork = app.store.projects[0]
                .features
                .iter()
                .find(|f| f.id == feature.feature_id)
                .unwrap();
            assert_eq!(fork.status, ProjectStatus::Stopped);
            assert_eq!(fork.name, "my-feat-fork");
            assert!(fork.is_worktree);
            assert!(!fork.sessions.is_empty());
        }
        // A duplicate confirm returns the recorded outcome and forks nothing.
        let again = confirm(&fx.gui, asked).unwrap();
        assert!(
            matches!(again, ForkOutcome::Created { feature: ref f, .. } if f.feature_id == feature.feature_id)
        );
        assert_eq!(feature_count(&fx), 2);
        assert!(matches!(
            status(&fx.gui, &preview.token).unwrap().outcome,
            Some(ForkOutcome::Created { .. })
        ));
    }

    #[test]
    fn a_mismatched_confirmation_is_refused_without_retargeting() {
        let fx = fixture();
        let preview = prepare_for(&fx, None).unwrap();
        let mut wrong_head = request(&preview);
        wrong_head.expected.head = "0".repeat(40);
        let mut wrong_session = request(&preview);
        wrong_session.expected.session_id = Some(CODEX.into());
        let mut wrong_feature = request(&preview);
        wrong_feature.expected.feature_id = "another".into();

        for asked in [wrong_head, wrong_session, wrong_feature] {
            let error = confirm(&fx.gui, asked).unwrap_err();
            assert_eq!(error.kind, GuiErrorKind::Conflict);
            assert!(error.message.contains("does not match the preview"));
        }
        assert!(!fork_dir(&fx).exists());
        assert_eq!(feature_count(&fx), 1);
        // The honest confirmation still works.
        assert!(matches!(
            confirm(&fx.gui, request(&preview)).unwrap(),
            ForkOutcome::Created { .. }
        ));
    }

    #[test]
    fn a_source_edit_after_preview_invalidates_the_token() {
        let fx = fixture();
        let preview = prepare_for(&fx, None).unwrap();
        std::fs::write(fx.repo.path().join("file"), b"edited after preview").unwrap();

        let error = confirm(&fx.gui, request(&preview)).unwrap_err();

        assert_eq!(error.kind, GuiErrorKind::Conflict);
        assert!(
            error.message.contains("prepare the fork again"),
            "{}",
            error.message
        );
        assert!(!fork_dir(&fx).exists());
        assert_eq!(feature_count(&fx), 1);
        let gone = confirm(&fx.gui, request(&preview)).unwrap_err();
        assert_eq!(gone.kind, GuiErrorKind::NotFound);
    }

    #[test]
    fn a_transcript_that_changes_after_preview_is_not_silently_re_read() {
        let fx = fixture();
        let preview = prepare_for(&fx, None).unwrap();
        let mut asked = request(&preview);
        asked.include_context = true;
        let transcript = fx._transcripts.path().join("conversation.jsonl");
        std::fs::write(
            &transcript,
            b"{\"type\":\"user\",\"message\":{\"content\":\"newer\"}}\n",
        )
        .unwrap();

        let error = confirm(&fx.gui, asked).unwrap_err();

        assert!(
            error.message.contains("transcript changed"),
            "{}",
            error.message
        );
        assert!(!fork_dir(&fx).exists());
    }

    #[test]
    fn a_new_commit_or_changed_session_after_preview_is_refused() {
        let fx = fixture();
        let preview = prepare_for(&fx, None).unwrap();
        git(
            fx.repo.path(),
            &["commit", "-q", "--allow-empty", "-m", "moved"],
        );
        assert!(confirm(&fx.gui, request(&preview)).is_err());
        assert!(!fork_dir(&fx).exists());

        let fx = fixture();
        let preview = prepare_for(&fx, None).unwrap();
        {
            let mut gui = fx.gui.lock().unwrap();
            gui.app_for_workflow().store.projects[0].features[0].sessions[0].claude_session_id =
                Some("a-newer-conversation".into());
        }
        let error = confirm(&fx.gui, request(&preview)).unwrap_err();
        assert!(
            error.message.contains("session changed"),
            "{}",
            error.message
        );
        assert!(!fork_dir(&fx).exists());
    }

    #[test]
    fn a_fork_needs_a_git_project_on_its_recorded_branch() {
        let fx = fixture_with(None, MockTmuxOps::new(), Box::new(WorktreeManager), |p| {
            p.is_git = false;
        });
        let error = prepare_for(&fx, None).unwrap_err();
        assert!(error.message.contains("Git project"));

        let fx = fixture();
        git(fx.repo.path(), &["checkout", "-qb", "elsewhere"]);
        let error = prepare_for(&fx, None).unwrap_err();
        assert!(
            error.message.contains("not on its recorded branch"),
            "{}",
            error.message
        );
    }

    #[test]
    fn invalid_names_policy_and_duplicates_refuse_before_creation() {
        let fx = fixture();
        let preview = prepare_for(&fx, None).unwrap();
        for (branch, expect) in [
            ("", "cannot be empty"),
            ("my-feat", "already exists"),
            ("bad name", "not a valid Git branch name"),
            ("../escape", "not a valid Git branch name"),
        ] {
            let mut asked = request(&preview);
            asked.new_branch = branch.into();
            let error = confirm(&fx.gui, asked).unwrap_err();
            assert!(
                error.message.contains(expect),
                "{branch}: {}",
                error.message
            );
        }
        let mut codex = request(&preview);
        codex.agent = AgentKind::Codex;
        assert!(
            confirm(&fx.gui, codex)
                .unwrap_err()
                .message
                .contains("not allowed")
        );
        assert!(!fx.repo.path().join(".worktrees").exists());
        // Each refusal left the preview usable.
        assert!(confirm(&fx.gui, request(&preview)).is_ok());
    }

    #[test]
    fn cancelling_a_preview_creates_nothing_and_ends_the_token() {
        let fx = fixture();
        let preview = prepare_for(&fx, None).unwrap();
        let status = cancel(&fx.gui, &preview.token).unwrap();
        assert!(matches!(
            status.outcome,
            Some(ForkOutcome::Cancelled { .. })
        ));
        let error = confirm(&fx.gui, request(&preview)).unwrap_err();
        assert_eq!(error.kind, GuiErrorKind::NotFound);
        assert!(!fx.repo.path().join(".worktrees").exists());
        assert_eq!(feature_count(&fx), 1);
    }

    #[test]
    fn starting_requires_an_available_harness_before_anything_is_created() {
        let mut tmux = MockTmuxOps::new();
        tmux.expect_check_harness_available()
            .returning(|_| Err(anyhow::anyhow!("claude is not installed")));
        let fx = fixture_with(None, tmux, Box::new(WorktreeManager), |_| ());
        let preview = prepare_for(&fx, None).unwrap();
        let mut asked = request(&preview);
        asked.start_agent = true;

        let error = confirm(&fx.gui, asked).unwrap_err();

        assert!(error.message.contains("claude is not installed"));
        assert!(error.message.contains("create the fork stopped"));
        assert!(!fork_dir(&fx).exists());
    }

    #[test]
    fn start_goes_through_the_shared_launch_with_a_mocked_harness() {
        let mut tmux = MockTmuxOps::new();
        tmux.expect_check_harness_available().returning(|_| Ok(()));
        tmux.expect_session_exists().return_const(false);
        tmux.expect_create_session_with_window()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_set_session_env()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_select_window()
            .times(1)
            .returning(|_, _| Ok(()));
        let fx = fixture_with(None, tmux, Box::new(WorktreeManager), |_| ());
        let preview = prepare_for(&fx, None).unwrap();
        let mut asked = request(&preview);
        asked.start_agent = true;
        asked.approve_resources = true;

        let outcome = confirm(&fx.gui, asked).unwrap();

        let ForkOutcome::Created {
            started, feature, ..
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert!(started);
        let mut gui = fx.gui.lock().unwrap();
        let fork = gui.app_for_workflow().store.projects[0]
            .features
            .iter()
            .find(|f| f.id == feature.feature_id)
            .unwrap()
            .status
            .clone();
        assert_ne!(fork, ProjectStatus::Stopped);
    }

    #[test]
    fn the_hook_choice_is_required_validated_and_run_after_registration() {
        let fx = fixture_with(
            Some("echo \"$AMF_HOOK_CHOICE\" > hook-ran"),
            MockTmuxOps::new(),
            Box::new(WorktreeManager),
            |_| (),
        );
        let preview = prepare_for(&fx, None).unwrap();
        assert_eq!(
            preview.hook.as_ref().unwrap().options,
            ["Continue", "Other"]
        );

        let missing = confirm(&fx.gui, request(&preview)).unwrap_err();
        assert!(missing.message.contains("requires a choice"));
        let mut invalid = request(&preview);
        invalid.hook_choice = Some("Nope".into());
        assert!(
            confirm(&fx.gui, invalid)
                .unwrap_err()
                .message
                .contains("Invalid hook_choice")
        );
        assert!(!fork_dir(&fx).exists());

        let mut asked = request(&preview);
        asked.hook_choice = Some("Other".into());
        let outcome = confirm(&fx.gui, asked).unwrap();
        let ForkOutcome::Created {
            hook,
            started,
            workdir,
            ..
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert!(!started);
        assert!(hook.unwrap().succeeded);
        assert_eq!(
            std::fs::read_to_string(PathBuf::from(workdir).join("hook-ran")).unwrap(),
            "Other\n"
        );
    }

    #[test]
    fn a_failing_hook_is_reported_and_does_not_hide_the_fork() {
        let fx = fixture_with(
            Some("echo broken >&2; exit 3"),
            MockTmuxOps::new(),
            Box::new(WorktreeManager),
            |_| (),
        );
        let preview = prepare_for(&fx, None).unwrap();
        let mut asked = request(&preview);
        asked.hook_choice = Some("Continue".into());

        let outcome = confirm(&fx.gui, asked).unwrap();

        let ForkOutcome::Created {
            hook,
            notes,
            started,
            ..
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        let hook = hook.unwrap();
        assert!(!hook.succeeded && !hook.cancelled);
        assert_eq!(hook.detail.as_deref(), Some("broken"));
        assert!(notes.iter().any(|n| n.contains("hook failed")));
        assert!(!started);
    }

    #[test]
    fn cancelling_during_the_hook_stops_it_and_keeps_the_stopped_fork() {
        let fx = fixture_with(
            Some("sleep 60"),
            MockTmuxOps::new(),
            Box::new(WorktreeManager),
            |_| (),
        );
        let preview = prepare_for(&fx, None).unwrap();
        let mut asked = request(&preview);
        asked.hook_choice = Some("Continue".into());
        asked.start_agent = false;
        let gui = fx.gui.clone();
        let worker = std::thread::spawn(move || confirm(&gui, asked));

        let began = std::time::Instant::now();
        loop {
            let current = status(&fx.gui, &preview.token).unwrap();
            if current.phase == Some(ForkPhase::Hook) {
                break;
            }
            assert!(
                began.elapsed() < std::time::Duration::from_secs(30),
                "never reached the hook"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // A second confirm while running is refused rather than forking twice.
        let busy = confirm(&fx.gui, request(&preview)).unwrap_err();
        assert!(busy.message.contains("already in progress"));
        let cancelling = cancel(&fx.gui, &preview.token).unwrap();
        assert!(cancelling.cancel_requested);

        let outcome = worker.join().unwrap().unwrap();
        let ForkOutcome::Created { hook, started, .. } = outcome else {
            panic!("{outcome:?}");
        };
        assert!(hook.unwrap().cancelled);
        assert!(!started);
        assert!(began.elapsed() < std::time::Duration::from_secs(30));
        assert_eq!(feature_count(&fx), 2);
    }

    #[test]
    fn a_transfer_failure_retains_the_checkout_without_registering() {
        let mut worktree = MockWorktreeOps::new();
        worktree
            .expect_create_fork_base()
            .times(1)
            .returning(|repo, name, branch, commit| {
                let path = WorktreeManager::create_fork_base(repo, name, branch, commit)?;
                std::fs::write(path.join("unexpected"), b"user content")?;
                Ok(path)
            });
        let fx = fixture_with(None, MockTmuxOps::new(), Box::new(worktree), |_| ());
        std::fs::write(fx.repo.path().join("file"), b"working").unwrap();
        let preview = prepare_for(&fx, None).unwrap();

        let outcome = confirm(&fx.gui, request(&preview)).unwrap();

        let ForkOutcome::Failed {
            phase,
            retained,
            feature,
            message,
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert_eq!(phase, ForkPhase::Transferring);
        assert!(feature.is_none());
        let retained = retained.unwrap();
        assert_eq!(retained.branch, "my-feat-fork");
        assert_eq!(
            std::fs::read(PathBuf::from(&retained.workdir).join("unexpected")).unwrap(),
            b"user content"
        );
        assert!(message.contains("no agent started"));
        assert_eq!(feature_count(&fx), 1);
        // Retrying the same token cannot reapply into the kept checkout.
        assert!(matches!(
            confirm(&fx.gui, request(&preview)).unwrap(),
            ForkOutcome::Failed { .. }
        ));
    }

    #[test]
    fn a_source_that_moves_mid_fork_removes_the_partial_checkout() {
        let mut worktree = MockWorktreeOps::new();
        let source = Arc::new(Mutex::new(PathBuf::new()));
        let edit = source.clone();
        worktree
            .expect_create_fork_base()
            .times(1)
            .returning(move |repo, name, branch, commit| {
                let path = WorktreeManager::create_fork_base(repo, name, branch, commit)?;
                std::fs::write(edit.lock().unwrap().join("edited-by-live-agent"), b"new")?;
                Ok(path)
            });
        worktree
            .expect_remove()
            .times(1)
            .returning(WorktreeManager::remove);
        let fx = fixture_with(None, MockTmuxOps::new(), Box::new(worktree), |_| ());
        *source.lock().unwrap() = fx.repo.path().to_path_buf();
        let preview = prepare_for(&fx, None).unwrap();

        let outcome = confirm(&fx.gui, request(&preview)).unwrap();

        let ForkOutcome::Failed {
            retained, message, ..
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert!(retained.is_none(), "{message}");
        assert!(message.contains("source changed"));
        assert!(!fork_dir(&fx).exists());
        assert!(git(fx.repo.path(), &["branch", "--list", "my-feat-fork"]).is_empty());
        assert_eq!(feature_count(&fx), 1);
    }

    #[test]
    fn a_failed_checkout_creation_leaves_the_preview_usable() {
        let mut worktree = MockWorktreeOps::new();
        worktree
            .expect_create_fork_base()
            .times(1)
            .returning(|_, _, _, _| Err(anyhow::anyhow!("disk full")));
        let fx = fixture_with(None, MockTmuxOps::new(), Box::new(worktree), |_| ());
        let preview = prepare_for(&fx, None).unwrap();

        let error = confirm(&fx.gui, request(&preview)).unwrap_err();

        assert!(error.message.contains("disk full"));
        assert_eq!(feature_count(&fx), 1);
        assert!(matches!(
            status(&fx.gui, &preview.token).unwrap().state,
            ForkRecordState::Prepared
        ));
    }

    #[test]
    fn the_suggested_branch_avoids_an_existing_feature_name() {
        let fx = fixture_with(None, MockTmuxOps::new(), Box::new(WorktreeManager), |p| {
            let mut taken = p.features[0].clone();
            taken.id = "other".into();
            taken.name = "my-feat-fork".into();
            p.features.push(taken);
        });
        assert_eq!(
            prepare_for(&fx, None).unwrap().suggested_branch,
            "my-feat-fork-2"
        );
    }

    #[test]
    fn old_previews_are_evicted_but_a_running_fork_never_is() {
        let mut state = ForkState::default();
        let run = Arc::new(Run {
            cancel: AtomicBool::new(false),
            phase: Mutex::new(ForkPhase::Checking),
        });
        state.insert("running".into(), Record::Running(run));
        for n in 0..(MAX_RECORDS + 3) {
            state.insert(
                format!("done-{n}"),
                Record::Done(Box::new(ForkOutcome::Cancelled {
                    message: "x".into(),
                })),
            );
        }
        assert!(state.records.contains_key("running"));
        assert_eq!(state.records.len(), MAX_RECORDS);
        assert!(!state.records.contains_key("done-0"));
    }

    #[test]
    fn requests_and_outcomes_have_the_wire_shape_the_frontend_uses() {
        let prepare: ForkPrepareRequest = serde_json::from_value(serde_json::json!({
            "target": {"project_id": "p", "feature_id": "f"}, "session_id": null
        }))
        .unwrap();
        assert!(prepare.session_id.is_none());
        let confirm: ForkConfirmRequest = serde_json::from_value(serde_json::json!({
            "token": "t",
            "expected": {"project_id": "p", "feature_id": "f", "session_id": "s", "branch": "b", "head": "h"},
            "new_branch": "n", "agent": "claude", "include_context": true,
            "hook_choice": null, "start_agent": false, "approve_resources": false
        }))
        .unwrap();
        assert_eq!(confirm.agent, AgentKind::Claude);
        let json = serde_json::to_value(ForkOutcome::Failed {
            phase: ForkPhase::CreatingCheckout,
            retained: None,
            feature: None,
            message: "m".into(),
        })
        .unwrap();
        assert_eq!(json["status"], "failed");
        assert_eq!(json["phase"], "creating_checkout");
        assert_eq!(
            serde_json::to_value(ContextStatus::UnsupportedHarness).unwrap(),
            "unsupported_harness"
        );
    }
}
