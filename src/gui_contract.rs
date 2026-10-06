//! The GUI's contract layer (`AMF_PLAN.md` Task 4: "Implement shared
//! first-slice operations and GUI contracts"): request/response DTOs, a
//! workspace snapshot for resynchronization, and [`GuiHandle`] — a facade
//! that owns a private [`App`] so nothing in `app`'s still-partly-TUI-coupled
//! surface (see `src/lib.rs`'s module-visibility note) needs to become part
//! of this crate's public API. Every method here calls the same App engine
//! methods the TUI drives (`create_project_from_request`,
//! `ensure_feature_running`, `do_stop_feature`, ...), addressed by stable
//! entity id instead of dashboard selection/cursor position, so GUI and TUI
//! share one behavior instead of two implementations of it.

use serde::{Deserialize, Serialize};

use crate::app::resource_gate::{StartPreconditions, describe_tripped};
use crate::app::session_ops::SessionStop;
use crate::app::{
    App, AppMode, DeleteStage, ReapplyOutcome, Selection, StartIntent, TodoDeleteDisposition,
    TodoPlanOrigin,
};
use crate::automation::{
    AutomationHookPrompt, CreateFeatureRequest, CreateFeatureResponse, CreateProjectRequest,
    CreateProjectResponse,
};
use crate::project::{AgentKind, Project, ProjectStatus, SessionKind, TodoSessionReference};

pub mod sidebar;

/// A structured, serializable error every GUI-facing operation returns
/// instead of a bare string, so the frontend can branch on `kind` (e.g. a
/// "this was deleted elsewhere, refresh?" affordance for `NotFound`) instead
/// of pattern-matching message text.
#[derive(Debug, Clone, Serialize)]
pub struct GuiError {
    pub kind: GuiErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuiErrorKind {
    /// The referenced project/feature id no longer exists: a stale target,
    /// most likely deleted elsewhere since the caller's last snapshot.
    NotFound,
    /// The request conflicts with current state in a way the caller should
    /// react to (e.g. creating a feature whose name already exists). Starting
    /// an already-running feature is deliberately *not* this — see
    /// [`GuiHandle::start_feature`] — that path is idempotent, not a
    /// conflict.
    Conflict,
    /// The start would exceed AMF's soft resource limits; the caller must
    /// present the warning and submit an explicit approval to proceed.
    NeedsApproval,
    /// Anything else: validation failures, I/O, tmux, or git errors.
    Internal,
}

impl GuiError {
    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            kind: GuiErrorKind::NotFound,
            message: message.into(),
        }
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self {
            kind: GuiErrorKind::Conflict,
            message: message.into(),
        }
    }

    pub(crate) fn needs_approval(message: impl Into<String>) -> Self {
        Self {
            kind: GuiErrorKind::NeedsApproval,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for GuiError {
    fn from(err: anyhow::Error) -> Self {
        // `create_project_from_request` / `create_feature_from_request` call
        // `App::save` internally, so a cross-process save conflict
        // (`AMF_PLAN.md` Task 5) can surface through either of them, not
        // just through `start_feature`/`stop_feature`'s own direct saves.
        // Exact-matching `App::SAVE_CONFLICT_MESSAGE` (one shared constant,
        // not a duplicated literal) anywhere in the error's cause chain
        // reclassifies that one specific failure as `Conflict` instead of
        // `Internal` -- the chain, not just the outermost message, so a
        // caller adding `.context(...)` on the way up does not hide it.
        //
        // A missing or unknown worktree-hook answer is the user's to fix, and
        // is typed (`WorktreeHookChoiceError`) so it lands as `Conflict` from
        // every path that checks it.
        //
        // Every other `bail!` in those two calls -- validation failures
        // ("Project name cannot be empty") and name/branch conflicts
        // ("Project '...' already exists") alike -- has no typed error on
        // the App side to match against, so distinguishing them here would
        // mean parsing message text; they all land as `Internal` until
        // those calls grow typed errors of their own.
        if err
            .chain()
            .any(|cause| cause.to_string() == crate::app::SAVE_CONFLICT_MESSAGE)
        {
            return Self::conflict(err.to_string());
        }
        if err
            .chain()
            .any(|cause| cause.is::<crate::app::WorktreeHookChoiceError>())
        {
            return Self::conflict(err.to_string());
        }
        Self {
            kind: GuiErrorKind::Internal,
            message: err.to_string(),
        }
    }
}

pub type GuiResult<T> = Result<T, GuiError>;

/// The GUI's view of workspace state: sent on load/reconnect
/// ("resynchronization") and after every mutation. Reuses the persisted
/// `Project`/`Feature` shape directly rather than a parallel DTO -- it is
/// already the exact serde shape the store round-trips, and a second shape
/// would only be one more place to keep in sync as fields are added.
/// `snapshot_at` is a cheap staleness signal, not a real revision counter;
/// per-entity revisioning (`AMF_PLAN.md`'s "revisioned updates") is deferred
/// past this first slice.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSnapshot {
    pub projects: Vec<Project>,
    pub snapshot_at: chrono::DateTime<chrono::Utc>,
    /// Sessions of a running feature whose tmux window is gone: stopped on
    /// their own (the TUI's `x`) or exited. Only a live snapshot fills this.
    pub stopped_session_ids: Vec<String>,
    /// What the TUI dashboard tree derives at render time, keyed by id
    /// (`gui_contract::sidebar`).
    pub sidebar: sidebar::SidebarSnapshot,
}

/// Addresses one feature by stable id rather than dashboard selection or a
/// `(project_index, feature_index)` pair, which shift under concurrent
/// mutation. `project_id` is included even though `feature_id` alone
/// (a UUID) is already unambiguous: it catches a caller acting on a stale
/// snapshot from the wrong project with `NotFound` instead of silently
/// hitting an unrelated project's feature of the same id-typo class.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FeatureTarget {
    pub project_id: String,
    pub feature_id: String,
}

/// Addresses one agent/terminal session within a feature, for the terminal
/// transport (`AMF_PLAN.md` Task 6) -- a session has no identity on its own
/// outside its owning feature, the same reasoning `FeatureTarget` documents
/// for `project_id`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionTarget {
    pub project_id: String,
    pub feature_id: String,
    pub session_id: String,
}

/// What `TerminalHandle::attach` (`gui_terminal`) actually needs to name a
/// tmux pane -- resolved from a `SessionTarget` once here rather than at
/// every terminal-transport call site, so a session that no longer exists
/// (deleted, or a stale snapshot) is reported the same `NotFound` way
/// `FeatureTarget` resolution already is.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalTarget {
    pub tmux_session: String,
    pub tmux_window: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StartFeatureResponse {
    pub feature_id: String,
    /// `true` when the feature was already running and this call was a
    /// no-op reconnect rather than a fresh start -- the repeated-submission
    /// case `AMF_PLAN.md` Task 4 asks to test.
    pub already_running: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StopFeatureResponse {
    pub feature_id: String,
    pub already_stopped: bool,
    pub message: String,
}

/// The GUI's answer to the TUI's delete-time TODO prompt. *Cancel* has no
/// variant: the GUI cancels by not sending the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoDeleteChoice {
    MoveToProject,
    MoveToGlobal,
    Delete,
}

impl From<TodoDeleteChoice> for TodoDeleteDisposition {
    fn from(choice: TodoDeleteChoice) -> Self {
        match choice {
            TodoDeleteChoice::MoveToProject => TodoDeleteDisposition::MoveToProject,
            TodoDeleteChoice::MoveToGlobal => TodoDeleteDisposition::MoveToGlobal,
            TodoDeleteChoice::Delete => TodoDeleteDisposition::Delete,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TodoHostCandidate {
    pub feature_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodoHostPrompt {
    pub list_id: String,
    pub todo_count: usize,
    pub candidates: Vec<TodoHostCandidate>,
}

/// A null feature ID explicitly deletes the project list and its items.
#[derive(Debug, Clone, Deserialize)]
pub struct TodoHostChoice {
    pub list_id: String,
    pub feature_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeleteFeatureResponse {
    Deleted {
        feature_id: String,
        message: String,
    },
    /// Nothing was touched: the worktree's list holds `unfinished` open
    /// TODOs, and the caller must resend with a [`TodoDeleteChoice`].
    NeedsTodoDisposition {
        unfinished: usize,
    },
    /// Nothing was touched: choose a surviving host or delete the project list.
    NeedsTodoHost {
        prompt: TodoHostPrompt,
    },
}

/// The outcome of starting one session of a feature (`GuiHandle::start_session`).
#[derive(Debug, Clone, Serialize)]
pub struct StartSessionResponse {
    pub session_id: String,
    /// The session's terminal was already up; nothing was launched.
    pub already_running: bool,
    pub message: String,
}

/// The outcome of stopping one session of a feature (`GuiHandle::stop_session`).
#[derive(Debug, Clone, Serialize)]
pub struct StopSessionResponse {
    pub session_id: String,
    pub already_stopped: bool,
    /// It was the feature's last running session, so the whole feature
    /// stopped with it (the session itself is left unflagged, so starting
    /// the feature brings it back).
    pub feature_stopped: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoveSessionResponse {
    pub session_id: String,
    /// Removing a feature's last running session stops the feature.
    pub feature_stopped: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionRecoveryOption {
    pub harness: String,
    pub saved_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SavedAgentSession {
    pub id: String,
    pub title: String,
    pub updated: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewSessionOption {
    pub kind: SessionKind,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddSessionResponse {
    pub target: SessionTarget,
    pub label: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRecoveryChoice {
    Resume,
    Clear,
    Pick,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodoAgentLaunchResponse {
    pub target: SessionTarget,
    /// An editable prompt for the GUI composer. Launching the agent must not
    /// automatically send the TODO text before the user has reviewed it.
    pub draft_prompt: String,
    pub reused_session: bool,
}

/// Facade over a private [`App`]. See the module doc for why `App` itself
/// never appears in this module's public signatures.
pub struct GuiHandle {
    app: App,
    pub(crate) learning_context: Option<crate::gui_learning::LearningContext>,
    pub(crate) review_context: Option<crate::gui_review::ReviewContext>,
    /// A completed review's result, until the interface takes it once.
    pub(crate) review_completion: Option<crate::gui_review::ReviewCompletion>,
}

impl GuiHandle {
    pub fn new(db_path: std::path::PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            app: App::new(db_path)?,
            learning_context: None,
            review_context: None,
            review_completion: None,
        })
    }

    /// Wrap an already-constructed `App` (typically `App::new_for_test`
    /// plus a real `AmfDb` attached by hand) -- for sibling modules' own
    /// test fixtures (e.g. `gui_todos`'s), which cannot build `GuiHandle`
    /// via a struct literal since `app` has no visibility modifier and they
    /// are not descendants of this module.
    #[cfg(test)]
    pub(crate) fn from_app(app: App) -> Self {
        Self {
            app,
            learning_context: None,
            review_context: None,
            review_completion: None,
        }
    }

    pub fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            projects: self.app.store.projects.clone(),
            snapshot_at: chrono::Utc::now(),
            stopped_session_ids: Vec::new(),
            sidebar: sidebar::project_sidebar(&self.app, &self.app.store.projects),
        }
    }

    /// Refresh the GUI's in-memory workspace when another AMF process has
    /// committed a newer store version. Tauri events only cover writes made
    /// by this GUI process; an open TUI has its own process and cannot emit
    /// those events into this window.
    pub fn refresh_snapshot(&mut self) -> GuiResult<WorkspaceSnapshot> {
        self.refresh_store()?;
        Ok(self.snapshot())
    }

    /// [`Self::refresh_snapshot`] without building a snapshot, for callers
    /// that only need the store current: the sidebar it would build reads
    /// notification directories, the DB, and `amf.json`.
    pub fn refresh_store(&mut self) -> GuiResult<()> {
        self.app.poll_learning_answers_bg();
        if let Some(db) = &self.app.db {
            let current = db.current_store_version().map_err(GuiError::from)?;
            if self.app.store_version != Some(current) {
                let (mut store, version) = db.load_store_versioned().map_err(GuiError::from)?;
                sidebar::carry_runtime_session_fields(&self.app.store, &mut store);
                self.app.store = store;
                self.app.store_version = Some(version);
            }
        }
        Ok(())
    }

    /// Reconcile the displayed status with live tmux sessions without
    /// changing the persisted store. A session can disappear without a DB
    /// write (for example after tmux exits), and the GUI does not run the
    /// TUI's background status synchronizer.
    pub fn refresh_live_snapshot(&mut self) -> GuiResult<WorkspaceSnapshot> {
        self.refresh_store()?;
        // The sidebar is built once, below, after the status correction.
        let mut snapshot = WorkspaceSnapshot {
            projects: self.app.store.projects.clone(),
            snapshot_at: chrono::Utc::now(),
            stopped_session_ids: Vec::new(),
            sidebar: sidebar::SidebarSnapshot::default(),
        };
        let live_sessions: std::collections::HashSet<String> = self
            .app
            .tmux
            .list_sessions()
            .map_err(GuiError::from)?
            .into_iter()
            .collect();
        let mut name_counts = std::collections::HashMap::<String, usize>::new();
        for feature in snapshot
            .projects
            .iter()
            .flat_map(|project| &project.features)
        {
            *name_counts.entry(feature.tmux_session.clone()).or_default() += 1;
        }
        for project in &mut snapshot.projects {
            for feature in &mut project.features {
                // A legacy store can assign the same tmux name to multiple
                // features. A live session cannot identify its owner, so do
                // not present every sibling as running or offer attachment.
                if name_counts.get(feature.tmux_session.as_str()) == Some(&1)
                    && live_sessions.contains(&feature.tmux_session)
                {
                    if feature.status == ProjectStatus::Stopped {
                        feature.status = ProjectStatus::Idle;
                    }
                } else {
                    feature.status = ProjectStatus::Stopped;
                }
            }
        }

        let running = |feature: &crate::project::Feature| {
            feature.status != ProjectStatus::Stopped
                && feature
                    .sessions
                    .iter()
                    .any(|session| session.kind.is_tmux_backed())
        };
        let any_running = snapshot
            .projects
            .iter()
            .flat_map(|project| &project.features)
            .any(running);
        if any_running {
            let live_windows: std::collections::HashSet<(String, String)> = self
                .app
                .tmux
                .window_activity()
                .into_iter()
                .map(|(session, window, _)| (session, window))
                .collect();
            // tmux unreachable: say nothing rather than call everything stopped.
            if !live_windows.is_empty() {
                for feature in snapshot
                    .projects
                    .iter()
                    .flat_map(|project| &project.features)
                    .filter(|feature| running(feature))
                {
                    for session in &feature.sessions {
                        if session.kind.is_tmux_backed()
                            && !live_windows.contains(&(
                                feature.tmux_session.clone(),
                                session.tmux_window.clone(),
                            ))
                        {
                            snapshot.stopped_session_ids.push(session.id.clone());
                        }
                    }
                }
            }
        }
        // Statuses were just corrected against tmux: a stopped feature
        // cannot be thinking, whatever its marker file says.
        snapshot.sidebar = sidebar::project_sidebar(&self.app, &snapshot.projects);
        Ok(snapshot)
    }

    /// The snapshot mutations broadcast: live when tmux answers, so a
    /// broadcast never contradicts the next poll.
    pub fn broadcast_snapshot(&mut self) -> WorkspaceSnapshot {
        self.refresh_live_snapshot()
            .unwrap_or_else(|_| self.snapshot())
    }

    pub fn create_project(
        &mut self,
        request: CreateProjectRequest,
    ) -> GuiResult<CreateProjectResponse> {
        Ok(self.app.create_project_from_request(&request)?)
    }

    pub fn create_feature(
        &mut self,
        request: CreateFeatureRequest,
    ) -> GuiResult<CreateFeatureResponse> {
        self.refresh_store()?;
        Ok(self.app.create_feature_from_request(&request)?)
    }

    /// Preview the project-configured prompt without creating a worktree.
    pub fn worktree_hook_prompt(
        &mut self,
        project_id: &str,
    ) -> GuiResult<Option<AutomationHookPrompt>> {
        self.refresh_store()?;
        let project = self
            .app
            .store
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| GuiError::not_found("Project was deleted; refresh and retry"))?;
        Ok(project
            .is_git
            .then(|| self.app.worktree_hook_prompt_for_repo(&project.repo))
            .flatten())
    }

    fn locate(&self, target: &FeatureTarget) -> GuiResult<(usize, usize)> {
        self.app
            .store
            .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
            .ok_or_else(|| {
                GuiError::not_found(format!(
                    "Feature '{}' was not found in project '{}' (it may have been \
                     deleted, or you're looking at a stale snapshot -- refresh and retry)",
                    target.feature_id, target.project_id
                ))
            })
    }

    fn locate_session(&self, target: &SessionTarget) -> GuiResult<(usize, usize, usize)> {
        let (pi, fi) = self.locate(&FeatureTarget {
            project_id: target.project_id.clone(),
            feature_id: target.feature_id.clone(),
        })?;
        let si = self.app.store.projects[pi].features[fi]
            .sessions
            .iter()
            .position(|session| session.id == target.session_id)
            .ok_or_else(|| GuiError::not_found("The selected session no longer exists"))?;
        Ok((pi, fi, si))
    }

    fn reject_ambiguous_live_session(&self, pi: usize, fi: usize) -> GuiResult<()> {
        let feature = &self.app.store.projects[pi].features[fi];
        if self.app.feature_tmux_session_is_shared(pi, fi)
            && self.app.tmux.session_exists(&feature.tmux_session)
        {
            return Err(GuiError::conflict(
                "Multiple features share this live tmux session. Stop that tmux session before starting or attaching a feature",
            ));
        }
        Ok(())
    }

    /// Match the TUI's per-repository harness picker, plus GUI-viewable
    /// terminal/editor panes. External VS Code windows and configured custom
    /// sessions need separate GUI workflows.
    pub fn new_session_options(
        &mut self,
        target: &FeatureTarget,
    ) -> GuiResult<Vec<NewSessionOption>> {
        self.refresh_store()?;
        let (pi, _) = self.locate(target)?;
        let project = &self.app.store.projects[pi];
        let mut options = self
            .app
            .allowed_agents_for_repo(&project.repo)
            .into_iter()
            .map(|agent| {
                let label = agent.display_name().to_string();
                let kind = crate::app::session_ops::session_kind_for_agent(&agent);
                NewSessionOption { kind, label }
            })
            .collect::<Vec<_>>();
        options.extend([
            NewSessionOption {
                kind: SessionKind::Terminal,
                label: "Terminal".to_string(),
            },
            NewSessionOption {
                kind: SessionKind::Nvim,
                label: "Neovim".to_string(),
            },
        ]);
        Ok(options)
    }

    pub fn add_session(
        &mut self,
        target: FeatureTarget,
        kind: SessionKind,
        label: Option<String>,
        approved: bool,
    ) -> GuiResult<AddSessionResponse> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let project_repo = self.app.store.projects[pi].repo.clone();
        let agent = match kind {
            SessionKind::Claude => Some(AgentKind::Claude),
            SessionKind::Codex => Some(AgentKind::Codex),
            SessionKind::Opencode => Some(AgentKind::Opencode),
            SessionKind::Pi => Some(AgentKind::Pi),
            SessionKind::Terminal | SessionKind::Nvim => None,
            _ => {
                return Err(GuiError::conflict(
                    "This session type is not available in the GUI",
                ));
            }
        };
        if let Some(agent) = &agent
            && !self
                .app
                .allowed_agents_for_repo(&project_repo)
                .contains(agent)
        {
            return Err(GuiError::conflict(
                "This agent is not allowed for this project",
            ));
        }
        if self.app.block_if_feature_pending_worktree_script(pi, fi) {
            return Err(GuiError::conflict(
                "Wait for the feature's worktree setup to finish before adding a session",
            ));
        }
        let feature = &self.app.store.projects[pi].features[fi];
        let label = label
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| feature.next_label(&kind));
        let starts_feature = self.app.add_would_start_feature(pi, fi, &kind);
        if !approved && (agent.is_some() || starts_feature) {
            self.require_start_approval(&format!("Adding '{label}'"))?;
        }

        let session_id = if let Some(agent) = agent {
            let (_, session_id, _) = self
                .app
                .create_agent_session_labeled_identified(
                    pi,
                    fi,
                    &label,
                    Some(agent),
                    StartIntent::Approved,
                )
                .map_err(GuiError::from)?;
            session_id
        } else {
            self.app
                .add_builtin_tmux_session_identified(pi, fi, kind, label.clone())
                .map_err(GuiError::from)?
        };
        Ok(AddSessionResponse {
            target: SessionTarget {
                project_id: target.project_id,
                feature_id: target.feature_id,
                session_id,
            },
            label,
        })
    }

    /// Offer the same saved-session decision as the TUI when tmux disappears.
    pub fn session_recovery_option(
        &mut self,
        target: &SessionTarget,
    ) -> GuiResult<Option<SessionRecoveryOption>> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(target)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let session = &feature.sessions[si];
        let harness = match session.kind {
            SessionKind::Claude => "Claude",
            SessionKind::Codex => "Codex",
            SessionKind::Opencode => "OpenCode",
            _ => return Ok(None),
        };
        let Some(saved_id) = crate::app::session_ops::persisted_resume_id(session) else {
            return Ok(None);
        };
        if self.app.tmux.session_exists(&feature.tmux_session) {
            // A running feature: only this session's own window can be
            // brought back, and only when it is gone.
            if self
                .app
                .tmux
                .window_exists(&feature.tmux_session, &session.tmux_window)
            {
                return Ok(None);
            }
        }
        // Unlike the TUI, a feature stopped on purpose still asks: the TUI's
        // silent restart only resumes Claude, so a Codex or OpenCode
        // conversation would otherwise start over without a word.
        Ok(Some(SessionRecoveryOption {
            harness: harness.to_string(),
            saved_id,
        }))
    }

    pub fn saved_agent_sessions(
        &mut self,
        target: &SessionTarget,
    ) -> GuiResult<Vec<SavedAgentSession>> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(target)?;
        let feature = &self.app.store.projects[pi].features[fi];
        crate::app::session_ops::saved_sessions_for_kind(
            &feature.sessions[si].kind,
            &feature.workdir,
        )
        .map(|sessions| {
            sessions
                .into_iter()
                .map(|(id, title, updated)| SavedAgentSession { id, title, updated })
                .collect()
        })
        .map_err(GuiError::from)
    }

    pub fn recover_session(
        &mut self,
        target: SessionTarget,
        choice: SessionRecoveryChoice,
        picked_id: Option<String>,
        approved: bool,
    ) -> GuiResult<StartFeatureResponse> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let session = &feature.sessions[si];
        let (agent, provider) = match session.kind {
            SessionKind::Claude => (
                AgentKind::Claude,
                crate::token_tracking::TokenUsageProvider::Claude,
            ),
            SessionKind::Codex => (
                AgentKind::Codex,
                crate::token_tracking::TokenUsageProvider::Codex,
            ),
            SessionKind::Opencode => (
                AgentKind::Opencode,
                crate::token_tracking::TokenUsageProvider::Opencode,
            ),
            _ => return Err(GuiError::conflict("This session cannot be resumed")),
        };
        let feature_running = self.app.tmux.session_exists(&feature.tmux_session);
        if feature_running
            && self
                .app
                .tmux
                .window_exists(&feature.tmux_session, &session.tmux_window)
        {
            return Ok(StartFeatureResponse {
                feature_id: target.feature_id,
                already_running: true,
                message: format!("'{}' is already running", session.label),
            });
        }
        let resume_id = match choice {
            SessionRecoveryChoice::Resume => Some(
                crate::app::session_ops::persisted_resume_id(session).ok_or_else(|| {
                    GuiError::conflict("The saved session ID is no longer available")
                })?,
            ),
            SessionRecoveryChoice::Clear => None,
            SessionRecoveryChoice::Pick => {
                let id = picked_id.ok_or_else(|| GuiError::conflict("Choose a saved session"))?;
                let sessions = crate::app::session_ops::saved_sessions_for_kind(
                    &session.kind,
                    &feature.workdir,
                )
                .map_err(GuiError::from)?;
                if !sessions.iter().any(|(candidate, _, _)| candidate == &id) {
                    return Err(GuiError::conflict(
                        "The chosen session is no longer available",
                    ));
                }
                Some(id)
            }
        };
        if !approved {
            let what = if feature_running {
                &session.label
            } else {
                &feature.name
            };
            self.require_start_approval(&format!("Recovering '{what}'"))?;
        }
        self.app
            .tmux
            .check_harness_available(&agent)
            .map_err(GuiError::from)?;

        if feature_running {
            return self.recover_session_window(target, si, choice, resume_id, &provider);
        }

        let mut created_session = false;
        if let Err(error) = self.app.ensure_feature_running_for_recovery(
            pi,
            fi,
            crate::app::feature_ops::RecoveryLaunch {
                session_id: target.session_id.clone(),
                resume_id: resume_id.clone(),
                // Per tab, like the TUI's session row: the feature's other
                // agent sessions stay stopped, each with its own resume choice.
                only_this_agent: true,
            },
            &mut created_session,
            StartIntent::Approved,
        ) {
            if created_session {
                let tmux_session = &self.app.store.projects[pi].features[fi].tmux_session;
                let _ = self.app.tmux.kill_session(tmux_session);
            }
            if created_session {
                self.app.store.projects[pi].features[fi].status = ProjectStatus::Stopped;
            }
            return Err(GuiError::from(error));
        }

        // Another process may have started the same tmux session between our
        // first existence check and the launch helper's own check. In that
        // case it used its own resume choice; do not record ours over it.
        if !created_session {
            return Ok(StartFeatureResponse {
                feature_id: target.feature_id,
                already_running: true,
                message: "The feature is already running".to_string(),
            });
        }

        let tmux_session = self.app.store.projects[pi].features[fi]
            .tmux_session
            .clone();
        let name = self.app.store.projects[pi].features[fi].name.clone();

        let session = &mut self.app.store.projects[pi].features[fi].sessions[si];
        Self::apply_recovery_choice(session, choice, resume_id.as_deref(), &provider);
        let session_kind = session.kind.clone();
        let outcome =
            self.app.save_reapplying(|store| {
                let Some((pi, fi)) =
                    store.locate_feature_by_id(Some(&target.project_id), &target.feature_id)
                else {
                    return false;
                };
                let feature = &mut store.projects[pi].features[fi];
                if feature.tmux_session != tmux_session {
                    return false;
                }
                let Some(session) = feature.sessions.iter_mut().find(|session| {
                    session.id == target.session_id && session.kind == session_kind
                }) else {
                    return false;
                };
                Self::apply_recovery_choice(session, choice, resume_id.as_deref(), &provider);
                feature.status = ProjectStatus::Idle;
                feature.touch();
                true
            });
        match outcome {
            Ok(ReapplyOutcome::Saved) => {}
            Ok(ReapplyOutcome::TargetGone | ReapplyOutcome::Conflict) => {
                self.stop_failed_recovery(&tmux_session)?;
                return Err(GuiError::conflict(
                    "Workspace changed elsewhere while recovery started; the new session was stopped. Refresh and retry",
                ));
            }
            Err(error) => {
                self.stop_failed_recovery(&tmux_session)?;
                return Err(GuiError::from(error));
            }
        }
        Ok(StartFeatureResponse {
            feature_id: target.feature_id,
            already_running: false,
            message: format!("Started '{name}'"),
        })
    }

    fn apply_recovery_choice(
        session: &mut crate::project::FeatureSession,
        choice: SessionRecoveryChoice,
        resume_id: Option<&str>,
        provider: &crate::token_tracking::TokenUsageProvider,
    ) {
        match choice {
            SessionRecoveryChoice::Clear => {
                session.claude_session_id = None;
                session.clear_token_usage_source();
            }
            SessionRecoveryChoice::Pick => {
                let id = resume_id
                    .expect("picked session ID was validated above")
                    .to_string();
                if session.kind == SessionKind::Claude {
                    session.claude_session_id = Some(id.clone());
                }
                session.set_token_usage_source_exact(crate::token_tracking::TokenUsageSource {
                    provider: provider.clone(),
                    id,
                });
            }
            SessionRecoveryChoice::Resume => {}
        }
    }

    fn stop_failed_recovery(&mut self, tmux_session: &str) -> GuiResult<()> {
        self.app.tmux.kill_session(tmux_session).map_err(|error| GuiError {
            kind: GuiErrorKind::Internal,
            message: format!(
                "Recovery could not be saved, and the newly started tmux session '{tmux_session}' could not be stopped: {error}"
            ),
        })?;
        // A failed DB write leaves the attempted recovery in memory. Restore
        // the last committed state so a later GUI action cannot save it.
        if let Some(db) = &self.app.db
            && let Ok((store, version)) = db.load_store_versioned()
        {
            self.app.adopt_store_from_disk(store, version);
        }
        Ok(())
    }

    /// The `(project_id, workdir)` pair that keys a feature's worktree TODO
    /// scope (`db::todos::TodoScope::Worktree`) -- exposed so `gui_todos`
    /// (a sibling module, not a descendant of this one) can resolve a
    /// feature-relative TODO scope without `App` itself needing to leave
    /// this module. See the module doc for why `App` never appears directly
    /// in a public signature here.
    pub fn worktree_scope_for_feature(
        &self,
        target: &FeatureTarget,
    ) -> GuiResult<(String, String)> {
        let (pi, fi) = self.locate(target)?;
        let feature = &self.app.store.projects[pi].features[fi];
        Ok((
            self.app.store.projects[pi].id.clone(),
            feature.workdir.to_string_lossy().into_owned(),
        ))
    }

    /// Access to the attached database for `gui_todos`, which is not
    /// coupled to `App`'s project/feature store at all (TODO lists live
    /// outside it -- see `db::todos`'s own module doc) and so has no other
    /// reason to reach into `App`.
    pub(crate) fn db(&self) -> GuiResult<&crate::db::AmfDb> {
        self.app.db.as_ref().ok_or_else(|| GuiError {
            kind: GuiErrorKind::Internal,
            message: "No database attached to this session".to_string(),
        })
    }

    /// Narrow crate-internal bridge for GUI workflow adapters. The App type
    /// remains absent from every public GUI contract signature.
    pub(crate) fn app_for_workflow(&mut self) -> &mut App {
        &mut self.app
    }

    pub fn start_feature(&mut self, target: FeatureTarget) -> GuiResult<StartFeatureResponse> {
        self.start_feature_with_approval(target, false)
    }

    pub fn start_feature_with_approval(
        &mut self,
        target: FeatureTarget,
        approved: bool,
    ) -> GuiResult<StartFeatureResponse> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let already_running = self.app.tmux.session_exists(&feature.tmux_session);
        if !already_running && !approved {
            self.require_start_approval(&format!("Starting '{}'", feature.name))?;
        }

        // The GUI has either passed the precondition check or obtained an
        // explicit approval. `ensure_feature_running` still checks tmux's
        // real session state and no-ops on a repeated submission.
        self.app
            .ensure_feature_running(pi, fi, StartIntent::Approved)?;
        // `save_reporting_conflict` rather than `save`: this call is entirely
        // ours (unlike `stop_feature`'s, which goes through `do_stop_feature`'s
        // own internal save), so it can report `GuiErrorKind::Conflict`
        // directly instead of relying on `From<anyhow::Error>`'s message match.
        if !self.app.save_reporting_conflict()? {
            return Err(GuiError::conflict(
                "Workspace changed elsewhere (another AMF window or process) before this start \
                 was saved. The view has been refreshed with the current state -- please retry.",
            ));
        }

        let name = self.app.store.projects[pi].features[fi].name.clone();
        Ok(StartFeatureResponse {
            feature_id: target.feature_id,
            already_running,
            message: if already_running {
                format!("'{name}' is already running")
            } else {
                format!("Started '{name}'")
            },
        })
    }

    fn require_start_approval(&self, action: &str) -> GuiResult<()> {
        if let StartPreconditions::NeedsConfirm {
            over_limit,
            low_memory,
        } = self.app.check_start_preconditions()
        {
            let reason = describe_tripped(over_limit, low_memory);
            return Err(GuiError::needs_approval(format!(
                "{action} would exceed AMF's resource warning: {reason}. Start anyway?"
            )));
        }
        Ok(())
    }

    /// Launch a TODO's agent in an explicit feature, using the same session
    /// creation engine as the TUI. The TODO reservation is a conditional SQL
    /// update, so a second GUI/TUI process cannot claim the same item while
    /// this process starts the harness. The prompt is returned unsent.
    pub fn launch_todo_agent(
        &mut self,
        todo_id: &str,
        target: FeatureTarget,
        approved: bool,
    ) -> GuiResult<TodoAgentLaunchResponse> {
        use crate::db::todos::{TodoScope, TodoStatus};

        self.refresh_store()?;
        let (pi, fi) = self.locate(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let resolved = self
            .db()?
            .resolve_todo_by_id(todo_id)
            .map_err(GuiError::from)?
            .ok_or_else(|| GuiError::not_found(format!("TODO '{todo_id}' was not found")))?;
        let feature = &self.app.store.projects[pi].features[fi];
        let scope_matches = match &resolved.list.scope {
            TodoScope::Worktree {
                project_id,
                workdir,
            } => project_id == &target.project_id && feature.workdir.to_string_lossy() == *workdir,
            TodoScope::Project { project_id } => project_id == &target.project_id,
            TodoScope::Global => true,
        };
        if !scope_matches {
            return Err(GuiError::conflict(
                "Choose a feature belonging to this TODO's current scope",
            ));
        }
        if resolved.todo.work.status != TodoStatus::NotStarted {
            return Err(GuiError::conflict(
                "This TODO is already in progress or completed; no new agent was launched",
            ));
        }
        let prompt = App::todo_spawn_prompt(&resolved.todo);

        // A manually reset TODO may still point at a live session. Reuse it
        // as the TUI does, rather than launching a duplicate harness.
        if let Some(existing_id) = resolved.todo.work.agent_session_id.as_deref()
            && let Some((epi, efi, esi)) = self.app.session_indices_by_id(existing_id)
        {
            let existing_feature = &self.app.store.projects[epi].features[efi];
            let existing_session = &existing_feature.sessions[esi];
            if self.app.tmux.window_exists(
                &existing_feature.tmux_session,
                &existing_session.tmux_window,
            ) {
                if !self
                    .db()?
                    .reserve_todo_agent_launch(todo_id)
                    .map_err(GuiError::from)?
                {
                    return Err(GuiError::conflict(
                        "TODO changed elsewhere; refresh and retry",
                    ));
                }
                if !self
                    .db()?
                    .associate_reserved_todo_agent_session(todo_id, existing_id)
                    .map_err(GuiError::from)?
                {
                    return Err(GuiError::conflict("TODO changed during session reuse"));
                }
                return Ok(TodoAgentLaunchResponse {
                    target: SessionTarget {
                        project_id: self.app.store.projects[epi].id.clone(),
                        feature_id: existing_feature.id.clone(),
                        session_id: existing_id.to_string(),
                    },
                    draft_prompt: prompt,
                    reused_session: true,
                });
            }
        }

        if !approved {
            self.require_start_approval(&format!(
                "Starting an agent for '{}'",
                resolved.todo.title
            ))?;
        }
        if !self
            .db()?
            .reserve_todo_agent_launch(todo_id)
            .map_err(GuiError::from)?
        {
            return Err(GuiError::conflict(
                "TODO changed elsewhere; refresh and retry",
            ));
        }

        let tmux_session = feature.tmux_session.clone();
        let agent = feature.agent.clone();
        let label = App::todo_session_label(&resolved.todo.title);
        let created = self.app.create_agent_session_labeled_identified(
            pi,
            fi,
            &label,
            Some(agent),
            StartIntent::Approved,
        );
        let (_index, session_id, window) = match created {
            Ok(created) => created,
            Err(err) => {
                let _ = self
                    .db()?
                    .rollback_reserved_todo_agent_launch(todo_id, None);
                return Err(GuiError::from(err));
            }
        };

        // `create_agent_session_labeled_identified` retains a live agent when
        // its save conflicts, but its App store is refreshed in that case.
        // Detect the missing record by stable id and clean up only our new
        // window before returning an actionable conflict.
        let Some((current_pi, current_fi)) = self
            .app
            .store
            .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        else {
            self.abort_created_todo_session(todo_id, &target, &session_id, &tmux_session, &window);
            return Err(GuiError::conflict(
                "Workspace changed during TODO launch; retry",
            ));
        };
        let Some(session) = self.app.store.projects[current_pi].features[current_fi]
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        else {
            self.abort_created_todo_session(todo_id, &target, &session_id, &tmux_session, &window);
            return Err(GuiError::conflict(
                "Workspace changed during TODO launch; retry",
            ));
        };
        session.todo_reference = Some(TodoSessionReference {
            todo_id: todo_id.to_string(),
            launched_from_todo_menu: true,
        });
        match self.app.save_reporting_conflict() {
            Ok(true) => {}
            Ok(false) => {
                self.abort_created_todo_session(
                    todo_id,
                    &target,
                    &session_id,
                    &tmux_session,
                    &window,
                );
                return Err(GuiError::conflict(
                    "Workspace changed during TODO launch; retry",
                ));
            }
            Err(err) => {
                self.abort_created_todo_session(
                    todo_id,
                    &target,
                    &session_id,
                    &tmux_session,
                    &window,
                );
                return Err(GuiError::from(err));
            }
        }

        if !self
            .db()?
            .associate_reserved_todo_agent_session(todo_id, &session_id)
            .map_err(GuiError::from)?
        {
            self.abort_created_todo_session(todo_id, &target, &session_id, &tmux_session, &window);
            return Err(GuiError::conflict(
                "TODO changed while its agent started; the new session was stopped",
            ));
        }

        Ok(TodoAgentLaunchResponse {
            target: SessionTarget {
                project_id: target.project_id,
                feature_id: target.feature_id,
                session_id,
            },
            draft_prompt: prompt,
            reused_session: false,
        })
    }

    /// Create a git worktree feature for a TODO and seed its initial agent.
    /// The existing feature-creation and TODO handoff engines do the work;
    /// this adapter supplies explicit GUI targets and returns the composer
    /// seed without sending it to the agent.
    pub fn launch_todo_in_new_feature(
        &mut self,
        todo_id: &str,
        request: CreateFeatureRequest,
        approved: bool,
    ) -> GuiResult<TodoAgentLaunchResponse> {
        use crate::db::todos::{TodoScope, TodoStatus};

        self.refresh_store()?;
        if !matches!(self.app.mode, AppMode::Normal) {
            return Err(GuiError::conflict("Finish the current workflow first"));
        }
        if request.dry_run || request.plan_mode || request.use_worktree != Some(true) {
            return Err(GuiError::conflict(
                "Starting a TODO in a new feature requires a git worktree without planning",
            ));
        }
        let project = self
            .app
            .store
            .find_project(&request.project_name)
            .ok_or_else(|| GuiError::not_found("Project was deleted; refresh and retry"))?;
        if !project.is_git {
            return Err(GuiError::conflict(
                "New TODO features require a git repository",
            ));
        }
        self.app
            .validate_worktree_hook_choice(&project.repo, request.hook_choice.as_deref())
            .map_err(GuiError::from)?;
        let project_id = project.id.clone();
        let resolved = self
            .db()?
            .resolve_todo_by_id(todo_id)
            .map_err(GuiError::from)?
            .ok_or_else(|| GuiError::not_found("TODO was deleted; refresh and retry"))?;
        let scope_matches = match &resolved.list.scope {
            TodoScope::Worktree { project_id: id, .. } | TodoScope::Project { project_id: id } => {
                id == &project_id
            }
            TodoScope::Global => true,
        };
        if !scope_matches {
            return Err(GuiError::conflict("Choose the TODO's project"));
        }
        if resolved.todo.work.status != TodoStatus::NotStarted {
            return Err(GuiError::conflict(
                "This TODO is already in progress or completed",
            ));
        }
        if !approved {
            self.require_start_approval(&format!(
                "Starting an agent for '{}'",
                resolved.todo.title
            ))?;
        }
        if !self
            .db()?
            .reserve_todo_agent_launch(todo_id)
            .map_err(GuiError::from)?
        {
            return Err(GuiError::conflict(
                "TODO changed elsewhere; refresh and retry",
            ));
        }
        let prompt = App::todo_spawn_prompt(&resolved.todo);
        let origin = TodoPlanOrigin {
            todo_id: todo_id.to_string(),
            list_id: resolved.list.id,
            todo_title: resolved.todo.title,
            host_feature_id: resolved.list.feature_id.unwrap_or_default(),
        };
        let created = self.app.create_feature_from_request(&request);
        let created = match created {
            Ok(created) => created,
            Err(err) => {
                let _ = self
                    .db()?
                    .rollback_reserved_todo_agent_launch(todo_id, None);
                return Err(GuiError::from(err));
            }
        };
        let target = FeatureTarget {
            project_id: created
                .project_id
                .ok_or_else(|| GuiError::conflict("Feature was not created"))?,
            feature_id: created
                .feature_id
                .ok_or_else(|| GuiError::conflict("Feature was not created"))?,
        };
        let (pi, fi) = self.locate(&target)?;
        if !created.started {
            if !approved
                && self
                    .require_start_approval("Starting the new feature's agent")
                    .is_err()
            {
                let _ = self
                    .db()?
                    .rollback_reserved_todo_agent_launch(todo_id, None);
                return Err(GuiError::conflict(
                    "Feature was created, but the resource limit changed before its agent could start. Refresh and start the feature after approving the warning.",
                ));
            }
            if let Err(err) = self
                .app
                .ensure_feature_running(pi, fi, StartIntent::Approved)
            {
                let _ = self
                    .db()?
                    .rollback_reserved_todo_agent_launch(todo_id, None);
                return Err(GuiError::from(err));
            }
            if !self.app.save_reporting_conflict()? {
                let _ = self
                    .db()?
                    .rollback_reserved_todo_agent_launch(todo_id, None);
                return Err(GuiError::conflict(
                    "Feature was created, but the workspace changed before its agent start was saved; refresh",
                ));
            }
        }
        if let Err(err) = self
            .app
            .finish_todo_spawn_in_new_feature(&origin, pi, fi, prompt)
        {
            let _ = self
                .db()?
                .rollback_reserved_todo_agent_launch(todo_id, None);
            return Err(GuiError::from(err));
        }
        let session_id = self
            .db()?
            .find_todo_by_id(todo_id)
            .map_err(GuiError::from)?
            .and_then(|todo| todo.work.agent_session_id);
        let Some(session_id) = session_id else {
            let _ = self
                .db()?
                .rollback_reserved_todo_agent_launch(todo_id, None);
            return Err(GuiError::conflict(
                "Feature was created, but the TODO agent could not be linked; refresh",
            ));
        };
        let draft_prompt = match &self.app.mode {
            AppMode::Compose(state) => state.editor.text().to_string(),
            _ => String::new(),
        };
        self.app.exit_view_without_resuming_plan_interview();
        Ok(TodoAgentLaunchResponse {
            target: SessionTarget {
                project_id: target.project_id,
                feature_id: target.feature_id,
                session_id,
            },
            draft_prompt,
            reused_session: false,
        })
    }

    fn abort_created_todo_session(
        &mut self,
        todo_id: &str,
        target: &FeatureTarget,
        session_id: &str,
        tmux_session: &str,
        window: &str,
    ) {
        if let Err(err) = self.app.tmux.kill_window(tmux_session, window) {
            self.app
                .log_warn("todos", format!("could not stop failed TODO window: {err}"));
        }
        if let Some((pi, fi)) = self
            .app
            .store
            .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        {
            let sessions = &mut self.app.store.projects[pi].features[fi].sessions;
            let before = sessions.len();
            sessions.retain(|session| session.id != session_id);
            if sessions.len() != before {
                let _ = self.app.save_reporting_conflict();
            }
        }
        if let Some(db) = &self.app.db {
            let _ = db.rollback_reserved_todo_agent_launch(todo_id, None);
        }
    }

    pub fn stop_feature(&mut self, target: FeatureTarget) -> GuiResult<StopFeatureResponse> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let name = self.app.store.projects[pi].features[fi].name.clone();

        if self.app.store.projects[pi].features[fi].status == ProjectStatus::Stopped {
            return Ok(StopFeatureResponse {
                feature_id: target.feature_id,
                already_stopped: true,
                message: format!("'{name}' is already stopped"),
            });
        }

        let editors = self.app.stop_feature_reporting(pi, fi)?;
        // Carry the editor cleanup's outcome (a window left running because
        // AMF did not open it, say) instead of dropping it with the status line.
        let message = match editors.as_ref().and_then(|report| report.summary()) {
            Some(detail) => format!("Stopped '{name}' - {detail}"),
            None => format!("Stopped '{name}'"),
        };
        Ok(StopFeatureResponse {
            feature_id: target.feature_id,
            already_stopped: false,
            message,
        })
    }

    /// Recreate one stopped session's window inside a running feature (the
    /// TUI's per-session restart) with the recovery choice applied.
    fn recover_session_window(
        &mut self,
        target: SessionTarget,
        si: usize,
        choice: SessionRecoveryChoice,
        resume_id: Option<String>,
        provider: &crate::token_tracking::TokenUsageProvider,
    ) -> GuiResult<StartFeatureResponse> {
        let (pi, fi) = self.locate(&FeatureTarget {
            project_id: target.project_id.clone(),
            feature_id: target.feature_id.clone(),
        })?;
        let session = &mut self.app.store.projects[pi].features[fi].sessions[si];
        let original_session = session.clone();
        Self::apply_recovery_choice(session, choice, resume_id.as_deref(), provider);
        let label = session.label.clone();
        self.app.message = None;
        // `restart_*` saves the record, carrying the choice with it.
        let restart = self
            .app
            .restart_stopped_session_window_resuming(pi, fi, si, Some(resume_id))
            .map_err(GuiError::from)
            .and_then(|started| {
                if started {
                    Ok(())
                } else {
                    Err(GuiError::conflict(format!("'{label}' cannot be restarted")))
                }
            });
        if let Err(error) = restart {
            // A no-op restart never saved the choice. Restore it before a
            // later save can persist it, even without a database attached.
            if let Ok((pi, fi, si)) = self.locate_session(&target) {
                self.app.store.projects[pi].features[fi].sessions[si] = original_session;
            }
            if let Some(db) = &self.app.db
                && let Ok((store, version)) = db.load_store_versioned()
            {
                self.app.adopt_store_from_disk(store, version);
            }
            return Err(error);
        }
        self.app.message = None;
        Ok(StartFeatureResponse {
            feature_id: target.feature_id,
            already_running: false,
            message: format!("Started '{label}'"),
        })
    }

    /// Stop one session and keep it listed: the GUI counterpart of the TUI's
    /// `x` on a session row, sharing its core (`App::stop_session_at`). The
    /// stop is persisted, so the session stays down when its feature next
    /// starts. Stopping the feature's last running session — the last live
    /// tmux window — stops the feature the way `stop_feature` does.
    pub fn stop_session(&mut self, target: SessionTarget) -> GuiResult<StopSessionResponse> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        self.reconcile_feature_status(pi, fi);

        let response = |already_stopped, feature_stopped, message| StopSessionResponse {
            session_id: target.session_id.clone(),
            already_stopped,
            feature_stopped,
            message,
        };

        let feature = &self.app.store.projects[pi].features[fi];
        let session = &feature.sessions[si];
        // A tab recovered on its own leaves the feature's other agents
        // windowless without flagging them stopped, so `stop_session_at` would
        // count them as running and kill this, the last live window -- taking
        // the tmux session down without the feature-stop bookkeeping.
        // With no other runnable session it already stops the feature.
        if feature.status != ProjectStatus::Stopped
            && !session.stopped
            && session.kind.is_tmux_backed()
            && feature
                .sessions
                .iter()
                .enumerate()
                .any(|(index, other)| index != si && other.runs_with_feature())
            && self
                .app
                .tmux
                .window_exists(&feature.tmux_session, &session.tmux_window)
            && !self.another_window_is_live(pi, fi, si)
        {
            let (label, name) = (session.label.clone(), feature.name.clone());
            self.app.do_stop_feature(pi, fi)?;
            return Ok(response(
                false,
                true,
                format!("Stopped '{name}': '{label}' was its last running session"),
            ));
        }
        match self.app.stop_session_at(pi, fi, si)? {
            SessionStop::StopsFeature => {
                let feature = &self.app.store.projects[pi].features[fi];
                let label = feature.sessions[si].label.clone();
                let name = feature.name.clone();
                self.app.do_stop_feature(pi, fi)?;
                Ok(response(
                    false,
                    true,
                    format!("Stopped '{name}': '{label}' was its last running session"),
                ))
            }
            SessionStop::NotStoppable => {
                Err(GuiError::conflict("This session has no terminal to stop"))
            }
            outcome @ SessionStop::OnlyRunnable(_) => {
                Err(GuiError::conflict(outcome.message().unwrap_or_default()))
            }
            outcome @ SessionStop::AlreadyStopped(_) => {
                Ok(response(true, false, outcome.message().unwrap_or_default()))
            }
            outcome @ (SessionStop::Recorded(_) | SessionStop::Stopped(_)) => Ok(response(
                false,
                false,
                outcome.message().unwrap_or_default(),
            )),
        }
    }

    /// Start one session: the GUI counterpart of the TUI's `c` on a session
    /// row. On a running feature only that session's terminal is recreated;
    /// on a stopped one the feature starts with that session included (and
    /// without the others stopped individually). Launching an agent asks for
    /// approval past the resource warning, like every GUI start.
    pub fn start_session(
        &mut self,
        target: SessionTarget,
        approved: bool,
    ) -> GuiResult<StartSessionResponse> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        if self.app.block_if_feature_pending_worktree_script(pi, fi) {
            return Err(GuiError::conflict(
                "Wait for the feature's worktree setup to finish before starting a session",
            ));
        }

        let feature = &self.app.store.projects[pi].features[fi];
        let session = &feature.sessions[si];
        if !session.kind.is_tmux_backed() {
            return Err(GuiError::conflict("This session has no terminal to start"));
        }
        let label = session.label.clone();
        let is_agent = session.kind.is_agent_harness();
        let was_flagged = session.stopped;
        let feature_live = self.app.tmux.session_exists(&feature.tmux_session);
        let window_live = feature_live
            && self
                .app
                .tmux
                .window_exists(&feature.tmux_session, &session.tmux_window);
        let response = |already_running, message| StartSessionResponse {
            session_id: target.session_id.clone(),
            already_running,
            message,
        };

        if window_live {
            // Up after all (recreated outside the GUI): the flag is what is
            // stale.
            if was_flagged {
                self.app.set_session_stopped(pi, fi, si, false);
                self.save_or_conflict()?;
            }
            return Ok(response(true, format!("'{label}' is already running")));
        }

        if !approved && (is_agent || !feature_live) {
            self.require_start_approval(&format!("Starting '{label}'"))?;
        }

        if feature_live {
            // Saves on its own, like the TUI's restart.
            self.app
                .restart_stopped_session_window_unchecked(pi, fi, si)?;
        } else {
            if let Err(error) = self.app.ensure_feature_running_with_target(
                pi,
                fi,
                Some(&target.session_id),
                StartIntent::Approved,
            ) {
                self.app.set_session_stopped(pi, fi, si, was_flagged);
                return Err(GuiError::from(error));
            }
            self.save_or_conflict()?;
        }
        Ok(response(false, format!("Started '{label}'")))
    }

    /// The GUI runs no background status sync, so a feature whose tmux
    /// session vanished can still read as running in the store. Settle that
    /// before deciding what a session stop means.
    fn reconcile_feature_status(&mut self, pi: usize, fi: usize) {
        let feature = &self.app.store.projects[pi].features[fi];
        if feature.status != ProjectStatus::Stopped
            && !self.app.tmux.session_exists(&feature.tmux_session)
        {
            self.app.store.projects[pi].features[fi].status = ProjectStatus::Stopped;
        }
    }

    fn save_or_conflict(&mut self) -> GuiResult<()> {
        if !self.app.save_reporting_conflict()? {
            return Err(GuiError::conflict(
                "Workspace changed elsewhere (another AMF window or process) before this change \
                 was saved. The view has been refreshed with the current state -- please retry.",
            ));
        }
        Ok(())
    }

    /// Whether a tmux-backed session other than `si` still has its window, so
    /// killing `si`'s window would leave the feature's tmux session alive.
    fn another_window_is_live(&self, pi: usize, fi: usize, si: usize) -> bool {
        let feature = &self.app.store.projects[pi].features[fi];
        feature.sessions.iter().enumerate().any(|(index, session)| {
            index != si
                && session.kind.is_tmux_backed()
                && self
                    .app
                    .tmux
                    .window_exists(&feature.tmux_session, &session.tmux_window)
        })
    }

    /// The TUI's `d` on a session row: kill its tmux window and drop the
    /// record. When that takes the feature down with it — its only session,
    /// or its last live window — the feature is stopped first, with the same
    /// bookkeeping as [`Self::stop_feature`].
    pub fn remove_session(&mut self, target: SessionTarget) -> GuiResult<RemoveSessionResponse> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let session = &feature.sessions[si];
        let label = session.label.clone();
        let was_running = feature.status != ProjectStatus::Stopped;
        let stops_feature = self.app.tmux.session_exists(&feature.tmux_session)
            && (feature.sessions.len() == 1
                || (session.kind.is_tmux_backed()
                    && self
                        .app
                        .tmux
                        .window_exists(&feature.tmux_session, &session.tmux_window)
                    && !self.another_window_is_live(pi, fi, si)));

        if stops_feature {
            self.app.message = None;
            // Runs every custom session's `on_stop`, this one's included, so
            // the record is dropped directly rather than through
            // `App::remove_session`, which would run it a second time.
            self.app.do_stop_feature(pi, fi)?;
            let (pi, fi, si) = self.locate_session(&target)?;
            self.app.store.projects[pi].features[fi].sessions.remove(si);
            self.app.selection = Selection::Feature(pi, fi);
            self.app.save()?;
            self.app.message = None;
            return Ok(RemoveSessionResponse {
                session_id: target.session_id,
                feature_stopped: true,
                message: format!("Removed '{label}' and stopped its feature"),
            });
        }

        self.app.selection = Selection::Session(pi, fi, si);
        self.app.message = None;
        self.app.remove_session()?;

        let feature_stopped = was_running
            && self
                .app
                .store
                .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
                .is_some_and(|(pi, fi)| {
                    self.app.store.projects[pi].features[fi].status == ProjectStatus::Stopped
                });
        Ok(RemoveSessionResponse {
            session_id: target.session_id,
            feature_stopped,
            message: self
                .app
                .message
                .take()
                .unwrap_or_else(|| format!("Removed '{label}'")),
        })
    }

    /// The TUI's feature delete: kill the tmux session, remove the worktree
    /// (`--force`; the branch stays), and drop the feature. A worktree list
    /// with open TODOs is settled first, and nothing is touched until the
    /// caller has chosen what happens to them. If this feature hosts the
    /// project list, also collect its surviving host or delete-list choice
    /// before applying the worktree disposition or starting deletion.
    ///
    /// Runs the TUI's staged deletion to completion before returning, the
    /// way `create_feature` runs worktree creation.
    pub fn delete_feature(
        &mut self,
        target: FeatureTarget,
        todos: Option<TodoDeleteChoice>,
        todo_host: Option<TodoHostChoice>,
    ) -> GuiResult<DeleteFeatureResponse> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        // The deletion is driven through `app.mode`, which is also where an
        // open GUI plan lives.
        if !matches!(self.app.mode, AppMode::Normal) {
            return Err(GuiError::conflict(
                "Finish or close the open plan before deleting a feature",
            ));
        }
        let project_name = self.app.store.projects[pi].name.clone();
        let feature = &self.app.store.projects[pi].features[fi];
        let feature_name = feature.name.clone();
        if self
            .app
            .paused_plan_interview_belongs_to_feature(&feature.id)
        {
            return Err(GuiError::conflict(
                "Resume or finish the parked plan interview before deleting its feature",
            ));
        }

        let disposition = self
            .app
            .pending_todo_disposition(&project_name, &feature_name);
        if let Some(disposition) = &disposition
            && todos.is_none()
        {
            return Ok(DeleteFeatureResponse::NeedsTodoDisposition {
                unfinished: disposition.unfinished,
            });
        }
        let mut host_prompt = self.todo_host_prompt(pi, fi)?;
        if todos == Some(TodoDeleteChoice::MoveToProject)
            && let (Some(prompt), Some(disposition)) = (&mut host_prompt, &disposition)
        {
            prompt.todo_count += disposition.unfinished;
        }
        match (&host_prompt, &todo_host) {
            (Some(prompt), None) => {
                return Ok(DeleteFeatureResponse::NeedsTodoHost {
                    prompt: prompt.clone(),
                });
            }
            (Some(prompt), Some(choice))
                if choice.list_id == prompt.list_id
                    && choice.feature_id.as_ref().is_none_or(|id| {
                        prompt
                            .candidates
                            .iter()
                            .any(|candidate| &candidate.feature_id == id)
                    }) => {}
            (None, None) => {}
            _ => {
                return Err(GuiError::conflict(
                    "The project TODO list or its destination changed. Reopen Delete feature to choose again",
                ));
            }
        }
        // Collect both answers before moving or deleting any TODOs.
        if let (Some(disposition), Some(choice)) = (disposition, todos) {
            self.app
                .apply_todo_disposition(&disposition, choice.into())?;
        }

        self.app.message = None;
        self.app.mode = AppMode::DeletingFeature(project_name, feature_name.clone());
        let result = self.run_feature_deletion(&feature_name, todo_host.as_ref());
        // Never leave the shared engine parked in a TUI dialog.
        self.app.mode = AppMode::Normal;
        let message = result?;
        Ok(DeleteFeatureResponse::Deleted {
            feature_id: target.feature_id,
            message,
        })
    }

    fn todo_host_prompt(&self, pi: usize, fi: usize) -> GuiResult<Option<TodoHostPrompt>> {
        let Some(db) = &self.app.db else {
            return Ok(None);
        };
        let project = &self.app.store.projects[pi];
        let scope = crate::db::todos::TodoScope::Project {
            project_id: project.id.clone(),
        };
        let Some(list) = db.todo_list(&scope)? else {
            return Ok(None);
        };
        if list.feature_id.as_deref() != Some(&project.features[fi].id) {
            return Ok(None);
        }
        let candidates: Vec<_> = project
            .features
            .iter()
            .filter(|feature| feature.id != project.features[fi].id)
            .map(|feature| TodoHostCandidate {
                feature_id: feature.id.clone(),
                name: feature.name.clone(),
            })
            .collect();
        // The TUI drops an orphaned project list when no features survive.
        if candidates.is_empty() {
            return Ok(None);
        }
        Ok(Some(TodoHostPrompt {
            todo_count: db.todos(&list.id)?.len(),
            list_id: list.id,
            candidates,
        }))
    }

    fn run_feature_deletion(
        &mut self,
        feature_name: &str,
        todo_host: Option<&TodoHostChoice>,
    ) -> GuiResult<String> {
        self.app.delete_feature()?;
        loop {
            match &self.app.mode {
                AppMode::DeletingFeatureInProgress(state)
                    if state.stage == DeleteStage::Completed && state.child.is_none() =>
                {
                    break;
                }
                AppMode::DeletingFeatureInProgress(_) => {}
                // `delete_feature` declined without starting; it says why.
                _ => {
                    return Err(GuiError::conflict(
                        self.app
                            .message
                            .take()
                            .unwrap_or_else(|| format!("'{feature_name}' was not deleted")),
                    ));
                }
            }
            self.app.poll_deleting_feature()?;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let error = match &self.app.mode {
            AppMode::DeletingFeatureInProgress(state) => state.error.clone(),
            _ => None,
        };
        self.app.complete_deleting_feature()?;
        if let Some(error) = error {
            self.app.message = None;
            return Err(GuiError::from(anyhow::anyhow!(
                "Could not delete '{feature_name}': {error}"
            )));
        }
        let mut message = self
            .app
            .message
            .take()
            .unwrap_or_else(|| format!("Deleted feature '{feature_name}'"));
        if let AppMode::TodosHostReassign(state) = &mut self.app.mode {
            let choice = todo_host.filter(|choice| choice.list_id == state.list_id)
                .ok_or_else(|| GuiError::conflict("Feature deleted; the project TODO list changed during deletion. Its TODOs were kept"))?;
            state.selected = match &choice.feature_id {
                Some(id) => state.candidates.iter().position(|(_, feature_id)| feature_id == id)
                    .ok_or_else(|| GuiError::conflict("Feature deleted; the chosen TODO host disappeared. Its TODOs were kept"))?,
                None => state.candidates.len(),
            };
            self.app.confirm_todos_host_reassign()?;
            if let Some(rehomed) = self.app.message.take() {
                message = format!("Deleted feature '{feature_name}'. {rehomed}");
            }
        }
        Ok(message)
    }

    /// Resolve a `SessionTarget` to the tmux session/window
    /// `gui_terminal::TerminalHandle::attach` needs. Does not check that the
    /// feature is actually running (a stopped feature has no live tmux
    /// session for `attach` to find) -- callers already have `already_running`
    /// from `start_feature`/a snapshot's `feature.status` to check first, and
    /// duplicating that check here would just be a second place for the two
    /// to disagree.
    pub fn resolve_session_target(&mut self, target: &SessionTarget) -> GuiResult<TerminalTarget> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(&FeatureTarget {
            project_id: target.project_id.clone(),
            feature_id: target.feature_id.clone(),
        })?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let session = feature
            .sessions
            .iter()
            .find(|session| session.id == target.session_id)
            .ok_or_else(|| {
                GuiError::not_found(format!(
                    "Session '{}' was not found on feature '{}'",
                    target.session_id, target.feature_id
                ))
            })?;
        Ok(TerminalTarget {
            tmux_session: feature.tmux_session.clone(),
            tmux_window: session.tmux_window.clone(),
        })
    }
}

/// Point the sessions this process launches at the `amf` CLI rather than at
/// the GUI itself. Hook scripts run `"${AMF_BIN:-amf}" notify ...` on every
/// agent event, and `AMF_BIN` defaults to the launching executable -- which,
/// in the GUI, opens another window per event. Resolves `amf` beside the GUI
/// binary (a dev `target/` dir, or an install that ships both) and then on
/// `PATH`; finding neither leaves `AMF_BIN` unset so the scripts fall back to
/// `amf` on the session's own `PATH`. Call once at startup, before any
/// session starts.
pub fn use_cli_for_session_hooks() {
    let cli = std::env::current_exe()
        .ok()
        .and_then(|exe| find_cli_binary(&exe, std::env::var_os("PATH").as_deref()));
    crate::tmux::TmuxManager::set_cli_binary(cli);
}

/// Pick the same tmux server the TUI uses. `configure_control_mode` decides
/// between the shared socket and the dedicated `managed-tmux.sock`, and its
/// built-in default (dedicated) disagrees with the config default (shared):
/// a GUI that skipped this watched an empty server and showed every session
/// the TUI started as stopped. Call once at startup, before any tmux call --
/// the resolved socket is cached for the life of the process.
pub fn use_configured_tmux_server() {
    let config = crate::app::load_config();
    crate::tmux::TmuxManager::configure_control_mode(config.tmux_control_mode);
}

fn find_cli_binary(
    gui_exe: &std::path::Path,
    path_var: Option<&std::ffi::OsStr>,
) -> Option<std::path::PathBuf> {
    let name = format!("amf{}", std::env::consts::EXE_SUFFIX);
    let sibling = gui_exe.parent().map(|dir| dir.join(&name));
    let on_path = path_var
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(&name));
    sibling
        .into_iter()
        .chain(on_path)
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{ExtensionConfig, HookConfig, HookPrompt, LifecycleHooks};
    use crate::project::{AgentKind, Feature, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::path::PathBuf;

    const PROJECT_ID: &str = "proj-1";
    const FEATURE_ID: &str = "feat-1";

    fn touch(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn session_hooks_prefer_the_amf_beside_the_gui_then_path() {
        let tmp = tempfile::tempdir().unwrap();
        let gui_dir = tmp.path().join("target/debug");
        let gui = gui_dir.join("amf-gui");
        let path_dir = tmp.path().join("bin");
        let path_amf = path_dir.join("amf");
        let path_var = std::env::join_paths([tmp.path().join("empty"), path_dir]).unwrap();
        touch(&gui);

        assert_eq!(super::find_cli_binary(&gui, Some(&path_var)), None);

        touch(&path_amf);
        assert_eq!(
            super::find_cli_binary(&gui, Some(&path_var)),
            Some(path_amf.clone())
        );

        let sibling = gui_dir.join("amf");
        touch(&sibling);
        assert_eq!(super::find_cli_binary(&gui, Some(&path_var)), Some(sibling));
    }

    #[test]
    fn session_hooks_never_resolve_to_a_directory_named_amf() {
        let tmp = tempfile::tempdir().unwrap();
        let gui = tmp.path().join("amf-gui");
        touch(&gui);
        std::fs::create_dir(tmp.path().join("amf")).unwrap();

        assert_eq!(super::find_cli_binary(&gui, None), None);
    }

    #[test]
    fn a_save_conflict_is_classified_even_under_added_context() {
        let bare = anyhow::anyhow!(crate::app::SAVE_CONFLICT_MESSAGE);
        assert_eq!(GuiError::from(bare).kind, GuiErrorKind::Conflict);

        let wrapped = anyhow::anyhow!(crate::app::SAVE_CONFLICT_MESSAGE)
            .context("feature was not saved")
            .context("creating 'new-work'");
        let error = GuiError::from(wrapped);
        assert_eq!(error.kind, GuiErrorKind::Conflict);
        assert_eq!(error.message, "creating 'new-work'");

        let other = anyhow::anyhow!("disk full").context("feature was not saved");
        assert_eq!(GuiError::from(other).kind, GuiErrorKind::Internal);
    }

    fn store_with_one_feature(status: ProjectStatus) -> ProjectStore {
        let mut feature = Feature::new_for_project(
            "demo",
            "my-feat".to_string(),
            "my-feat".to_string(),
            PathBuf::from("/tmp/test-workdir"),
            false,
            VibeMode::default(),
            false,
            false,
            AgentKind::default(),
            false,
            false,
        );
        feature.id = FEATURE_ID.to_string();
        feature.status = status;

        let mut project = Project::new(
            "demo".to_string(),
            PathBuf::from("/tmp/test-repo"),
            false,
            AgentKind::default(),
        );
        project.id = PROJECT_ID.to_string();
        project.features.push(feature);

        let mut store = ProjectStore::empty();
        store.projects.push(project);
        store
    }

    fn handle(store: ProjectStore, tmux: MockTmuxOps) -> GuiHandle {
        GuiHandle {
            app: App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new())),
            learning_context: None,
            review_context: None,
            review_completion: None,
        }
    }

    /// A `GuiHandle` backed by a *real* `AmfDb` connection, standing in for
    /// one process's `App`/`GuiHandle` in a GUI+TUI overlap test: opening the
    /// same on-disk path from two of these is exactly what two live AMF
    /// processes (or a GUI window and a TUI, or two TUI instances) do. Mocks
    /// still cover tmux/worktree, since those aren't what Task 5 is testing
    /// here -- `App::new_for_test` plus manually attaching a real `db` is the
    /// same pattern several existing `app::tests` suites already use for
    /// DB-backed coverage without a real `App::new` (see `App::save`'s
    /// `store_version: None` handling, added for exactly this pattern).
    fn handle_loading_from(db: crate::db::AmfDb, tmux: MockTmuxOps) -> GuiHandle {
        let (store, version) = db.load_store_versioned().unwrap();
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.db = Some(db);
        app.store_version = Some(version);
        GuiHandle {
            app,
            learning_context: None,
            review_context: None,
            review_completion: None,
        }
    }

    fn prepend_project_elsewhere(db_path: &std::path::Path) {
        let db = crate::db::AmfDb::open(db_path).unwrap();
        let mut store = db.load_store().unwrap();
        let mut project = Project::new(
            "other".to_string(),
            PathBuf::from("/tmp/other-repo"),
            false,
            AgentKind::default(),
        );
        project.id = "other-project".to_string();
        store.projects.insert(0, project);
        db.save_store(&store).unwrap();
    }

    fn stale_target() -> FeatureTarget {
        FeatureTarget {
            project_id: PROJECT_ID.to_string(),
            feature_id: "does-not-exist".to_string(),
        }
    }

    fn target() -> FeatureTarget {
        FeatureTarget {
            project_id: PROJECT_ID.to_string(),
            feature_id: FEATURE_ID.to_string(),
        }
    }

    #[test]
    fn start_feature_reports_not_found_for_a_stale_target() {
        let store = store_with_one_feature(ProjectStatus::Stopped);
        let mut gui = handle(store, MockTmuxOps::new());

        let err = gui.start_feature(stale_target()).unwrap_err();

        assert_eq!(err.kind, GuiErrorKind::NotFound);
    }

    #[test]
    fn stop_feature_reports_not_found_for_a_stale_target() {
        let store = store_with_one_feature(ProjectStatus::Idle);
        let mut gui = handle(store, MockTmuxOps::new());

        let err = gui.stop_feature(stale_target()).unwrap_err();

        assert_eq!(err.kind, GuiErrorKind::NotFound);
    }

    /// The fixture feature with two Claude sessions, the second one stopped
    /// individually when `second_stopped`.
    fn store_with_two_sessions(status: ProjectStatus, second_stopped: bool) -> ProjectStore {
        let mut store = store_with_one_feature(status);
        let feature = &mut store.projects[0].features[0];
        feature.add_session_named(SessionKind::Claude, "Claude 1".into());
        feature
            .add_session_named(SessionKind::Claude, "Claude 2".into())
            .stopped = second_stopped;
        store
    }

    fn session_target(gui: &GuiHandle, si: usize) -> SessionTarget {
        SessionTarget {
            project_id: PROJECT_ID.to_string(),
            feature_id: FEATURE_ID.to_string(),
            session_id: gui.app.store.projects[0].features[0].sessions[si]
                .id
                .clone(),
        }
    }

    fn second_session_window(store: &ProjectStore) -> String {
        store.projects[0].features[0].sessions[1]
            .tmux_window
            .clone()
    }

    #[test]
    fn stop_session_kills_only_that_window_and_keeps_it_listed() {
        let store = store_with_two_sessions(ProjectStatus::Idle, false);
        let window = second_session_window(&store);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_window_exists().return_const(true);
        tmux.expect_kill_window()
            .withf(move |_, w| w == window)
            .times(1)
            .returning(|_, _| Ok(()));
        tmux.expect_kill_session().times(0);
        let mut gui = handle(store, tmux);
        let target = session_target(&gui, 1);

        let response = gui.stop_session(target).unwrap();

        assert!(!response.already_stopped);
        assert!(!response.feature_stopped);
        let feature = &gui.snapshot().projects[0].features[0];
        assert_eq!(feature.sessions.len(), 2);
        assert!(feature.sessions[1].stopped);
        assert!(!feature.sessions[0].stopped);
    }

    #[test]
    fn stopping_the_last_running_session_stops_the_feature() {
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_kill_session().times(1).returning(|_| Ok(()));
        tmux.expect_kill_window().times(0);
        let mut gui = handle(store_with_two_sessions(ProjectStatus::Idle, true), tmux);
        let target = session_target(&gui, 0);

        let response = gui.stop_session(target).unwrap();

        assert!(response.feature_stopped);
        let feature = &gui.snapshot().projects[0].features[0];
        assert_eq!(feature.status, ProjectStatus::Stopped);
        // Left unflagged, so starting the feature brings it back.
        assert!(!feature.sessions[0].stopped);
        assert!(feature.sessions[1].stopped);
    }

    #[test]
    fn stopping_a_stopped_session_reports_already_stopped() {
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        let mut gui = handle(store_with_two_sessions(ProjectStatus::Idle, true), tmux);
        let target = session_target(&gui, 1);

        let response = gui.stop_session(target).unwrap();

        assert!(response.already_stopped);
    }

    #[test]
    fn stop_session_reports_not_found_for_a_stale_session() {
        let mut gui = handle(
            store_with_two_sessions(ProjectStatus::Idle, false),
            MockTmuxOps::new(),
        );
        let mut target = session_target(&gui, 0);
        target.session_id = "gone".to_string();

        assert_eq!(
            gui.stop_session(target).unwrap_err().kind,
            GuiErrorKind::NotFound
        );
    }

    #[test]
    fn start_session_on_a_running_feature_recreates_only_that_window() {
        let store = store_with_two_sessions(ProjectStatus::Idle, true);
        let window = second_session_window(&store);
        let create_window = window.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_window_exists().return_const(false);
        tmux.expect_create_window()
            .withf(move |_, w, _| w == create_window)
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .withf(move |_, w, _, _, _| w == window)
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_create_session_with_window().times(0);
        let mut gui = handle(store, tmux);
        let target = session_target(&gui, 1);

        let response = gui.start_session(target, true).unwrap();

        assert!(!response.already_running);
        assert!(!gui.snapshot().projects[0].features[0].sessions[1].stopped);
    }

    #[test]
    fn start_session_on_a_stopped_feature_starts_it_with_that_session() {
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(false);
        tmux.expect_create_session_with_window()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
        tmux.expect_create_window()
            .times(1)
            .returning(|_, _, _| Ok(()));
        // Both launch under the default one-agent autostart cap: the session
        // asked for is not the one the cap skips.
        tmux.expect_launch_claude()
            .times(2)
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_select_window().returning(|_, _| Ok(()));
        let mut gui = handle(store_with_two_sessions(ProjectStatus::Stopped, true), tmux);
        assert_eq!(gui.app.config.max_agent_autostart_sessions, 1);
        let target = session_target(&gui, 1);

        let response = gui.start_session(target, true).unwrap();

        assert!(!response.already_running);
        let feature = &gui.snapshot().projects[0].features[0];
        assert_ne!(feature.status, ProjectStatus::Stopped);
        assert!(!feature.sessions[1].stopped);
    }

    #[test]
    fn start_session_on_a_live_window_is_a_no_op() {
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_window_exists().return_const(true);
        tmux.expect_create_window().times(0);
        let mut gui = handle(store_with_two_sessions(ProjectStatus::Idle, false), tmux);
        let target = session_target(&gui, 0);

        let response = gui.start_session(target, false).unwrap();

        assert!(response.already_running);
    }

    #[test]
    fn a_mismatched_project_id_is_also_not_found() {
        let store = store_with_one_feature(ProjectStatus::Idle);
        let mut gui = handle(store, MockTmuxOps::new());
        let wrong_project = FeatureTarget {
            project_id: "some-other-project".to_string(),
            feature_id: FEATURE_ID.to_string(),
        };

        let err = gui.start_feature(wrong_project).unwrap_err();

        assert_eq!(err.kind, GuiErrorKind::NotFound);
    }

    /// Repeated-submission case: a second `start_feature` on a feature whose
    /// tmux session is already up must not attempt a second launch. Setting
    /// up only `expect_session_exists` (no `create_session_with_window` /
    /// `launch_claude` expectations at all) means mockall's own default
    /// behavior -- panicking on an unexpected call -- is what proves no
    /// double launch happens, not just an assertion on the response.
    #[test]
    fn starting_an_already_running_feature_is_idempotent() {
        let store = store_with_one_feature(ProjectStatus::Idle);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        let mut gui = handle(store, tmux);

        let response = gui.start_feature(target()).unwrap();

        assert!(response.already_running);
        assert_eq!(response.feature_id, FEATURE_ID);
    }

    /// Cold-start happy path: the feature has no live tmux session yet, so
    /// this exercises the full launch sequence `ensure_feature_running`
    /// drives for a fresh Claude session (the default `AgentKind`).
    #[test]
    fn starting_a_stopped_feature_launches_it_once() {
        let store = store_with_one_feature(ProjectStatus::Stopped);
        let mut gui = handle(store, claude_cold_start_tmux());

        let response = gui.start_feature(target()).unwrap();

        assert!(!response.already_running);
        assert_eq!(
            gui.app.store.projects[0].features[0].status,
            ProjectStatus::Idle
        );
    }

    #[test]
    fn a_feature_left_running_in_the_database_can_restart_after_tmux_exits() {
        let mut tmux = claude_cold_start_tmux();
        tmux.expect_list_sessions()
            .times(1)
            .returning(|| Ok(vec![]));
        let mut gui = handle(store_with_one_feature(ProjectStatus::Active), tmux);

        let snapshot = gui.refresh_live_snapshot().unwrap();
        assert_eq!(
            snapshot.projects[0].features[0].status,
            ProjectStatus::Stopped
        );

        let response = gui.start_feature(target()).unwrap();
        assert!(!response.already_running);
        assert_eq!(
            gui.app.store.projects[0].features[0].status,
            ProjectStatus::Idle
        );
    }

    #[test]
    fn gui_adds_a_second_agent_session_with_a_stable_target() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        store.projects[0].repo = dir.path().to_path_buf();
        store.projects[0].features[0].workdir = dir.path().to_path_buf();
        store.projects[0].features[0].add_session(SessionKind::Claude);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_create_window()
            .withf(|_, window, _| window == "claude-2")
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .withf(|_, window, _, resume_id, _| window == "claude-2" && resume_id.is_none())
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        let mut gui = handle(store, tmux);

        let added = gui
            .add_session(target(), SessionKind::Claude, None, true)
            .unwrap();

        assert_eq!(added.label, "Claude 2");
        assert_eq!(added.target.project_id, PROJECT_ID);
        assert_eq!(added.target.feature_id, FEATURE_ID);
        assert_eq!(
            gui.app.store.projects[0].features[0].sessions[1].id,
            added.target.session_id
        );
    }

    #[test]
    fn gui_adds_a_named_terminal_session_without_launching_an_agent() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        store.projects[0].repo = dir.path().to_path_buf();
        store.projects[0].features[0].workdir = dir.path().to_path_buf();
        store.projects[0].features[0].add_session(SessionKind::Claude);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_create_window()
            .withf(|_, window, _| window == "terminal")
            .times(1)
            .returning(|_, _, _| Ok(()));
        let mut gui = handle(store, tmux);

        let added = gui
            .add_session(
                target(),
                SessionKind::Terminal,
                Some("  Build shell  ".into()),
                false,
            )
            .unwrap();

        assert_eq!(added.label, "Build shell");
        assert_eq!(
            gui.app.store.projects[0].features[0].sessions[1].id,
            added.target.session_id
        );
    }

    #[test]
    fn terminal_session_survives_a_concurrent_store_write_with_its_record() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        store.projects[0].features[0].add_session(SessionKind::Terminal);
        let writer = crate::db::AmfDb::open(db_file.path()).unwrap();
        writer.save_store(&store).unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        let path = db_file.path().to_path_buf();
        tmux.expect_create_window()
            .times(1)
            .returning(move |_, _, _| {
                prepend_project_elsewhere(&path);
                Ok(())
            });
        tmux.expect_kill_window().never();
        let mut gui = handle_loading_from(crate::db::AmfDb::open(db_file.path()).unwrap(), tmux);

        let added = gui
            .add_session(target(), SessionKind::Terminal, Some("Shell".into()), true)
            .unwrap();

        let on_disk = writer.load_store().unwrap();
        assert_eq!(on_disk.projects.len(), 2);
        assert!(
            on_disk.projects[1].features[0]
                .sessions
                .iter()
                .any(|session| session.id == added.target.session_id)
        );
    }

    #[test]
    fn terminal_window_is_closed_if_its_feature_is_deleted_during_save() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        store.projects[0].features[0].add_session(SessionKind::Terminal);
        let writer = crate::db::AmfDb::open(db_file.path()).unwrap();
        writer.save_store(&store).unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        let path = db_file.path().to_path_buf();
        tmux.expect_create_window()
            .times(1)
            .returning(move |_, _, _| {
                let db = crate::db::AmfDb::open(&path).unwrap();
                db.save_store(&ProjectStore::empty()).unwrap();
                Ok(())
            });
        tmux.expect_kill_window().times(1).returning(|_, _| Ok(()));
        let mut gui = handle_loading_from(crate::db::AmfDb::open(db_file.path()).unwrap(), tmux);

        let error = gui
            .add_session(target(), SessionKind::Terminal, None, true)
            .unwrap_err();

        assert!(error.message.contains("removed elsewhere"));
        assert!(writer.load_store().unwrap().projects.is_empty());
    }

    #[test]
    fn failed_terminal_add_stops_the_feature_it_started() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let mut store = store_with_one_feature(ProjectStatus::Stopped);
        store.projects[0].features[0].add_session(SessionKind::Terminal);
        let writer = crate::db::AmfDb::open(db_file.path()).unwrap();
        writer.save_store(&store).unwrap();
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists()
            .returning(move |_| checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 3);
        tmux.expect_create_session_with_window()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_set_session_env()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_select_window()
            .times(1)
            .returning(|_, _| Ok(()));
        let path = db_file.path().to_path_buf();
        tmux.expect_create_window()
            .times(1)
            .returning(move |_, _, _| {
                let db = crate::db::AmfDb::open(&path).unwrap();
                db.save_store(&ProjectStore::empty()).unwrap();
                Ok(())
            });
        tmux.expect_kill_window().times(1).returning(|_, _| Ok(()));
        tmux.expect_kill_session().times(1).returning(|_| Ok(()));
        let mut gui = handle_loading_from(crate::db::AmfDb::open(db_file.path()).unwrap(), tmux);

        let error = gui
            .add_session(target(), SessionKind::Terminal, None, true)
            .unwrap_err();

        assert!(error.message.contains("removed elsewhere"));
        assert!(writer.load_store().unwrap().projects.is_empty());
    }

    #[test]
    fn failed_save_after_starting_feature_stops_its_new_tmux_session() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let mut store = store_with_one_feature(ProjectStatus::Stopped);
        store.projects[0].features[0].add_session(SessionKind::Terminal);
        let writer = crate::db::AmfDb::open(db_file.path()).unwrap();
        writer.save_store(&store).unwrap();
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists()
            .returning(move |_| checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 3);
        tmux.expect_create_session_with_window()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_set_session_env()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let path = db_file.path().to_path_buf();
        tmux.expect_select_window().times(1).returning(move |_, _| {
            prepend_project_elsewhere(&path);
            Ok(())
        });
        tmux.expect_kill_session().times(1).returning(|_| Ok(()));
        tmux.expect_create_window().never();
        let mut gui = handle_loading_from(crate::db::AmfDb::open(db_file.path()).unwrap(), tmux);

        let error = gui
            .add_session(target(), SessionKind::Terminal, None, true)
            .unwrap_err();

        assert_eq!(error.kind, GuiErrorKind::Conflict);
        assert_eq!(writer.load_store().unwrap().projects.len(), 2);
    }

    fn recoverable_claude_feature() -> (ProjectStore, SessionTarget) {
        let mut store = store_with_one_feature(ProjectStatus::Active);
        let session = store.projects[0].features[0].add_session(SessionKind::Claude);
        session.claude_session_id = Some("saved-claude-id".to_string());
        let target = SessionTarget {
            project_id: PROJECT_ID.to_string(),
            feature_id: FEATURE_ID.to_string(),
            session_id: session.id.clone(),
        };
        (store, target)
    }

    fn claude_recovery_tmux_with_select(
        expected_resume_id: Option<&'static str>,
        on_select: impl Fn(&str, &str) -> anyhow::Result<()> + Send + Sync + 'static,
    ) -> MockTmuxOps {
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(false);
        tmux.expect_check_harness_available()
            .times(1)
            .returning(|_| Ok(()));
        tmux.expect_create_session_with_window()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_set_session_env()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .withf(move |_, _, _, resume_id, _| resume_id.as_deref() == expected_resume_id)
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_select_window().times(1).returning(on_select);
        tmux
    }

    fn claude_recovery_tmux(expected_resume_id: Option<&'static str>) -> MockTmuxOps {
        claude_recovery_tmux_with_select(expected_resume_id, |_, _| Ok(()))
    }

    #[test]
    fn resuming_one_tab_of_a_stopped_feature_leaves_its_other_agents_stopped() {
        let mut store = store_with_one_feature(ProjectStatus::Stopped);
        let feature = &mut store.projects[0].features[0];
        // First in order, yet held back: the tmux session opens on the
        // recovered tab instead, and the Codex tab never gets a window.
        let other_window = feature.add_session(SessionKind::Codex).tmux_window.clone();
        let claude = feature.add_session(SessionKind::Claude);
        claude.claude_session_id = Some("saved-claude-id".to_string());
        let (claude_id, claude_window) = (claude.id.clone(), claude.tmux_window.clone());
        let terminal_window = feature
            .add_session(SessionKind::Terminal)
            .tmux_window
            .clone();

        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(false);
        tmux.expect_check_harness_available().returning(|_| Ok(()));
        let first = claude_window.clone();
        tmux.expect_create_session_with_window()
            .withf(move |_, window, _| window == first)
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
        tmux.expect_create_window()
            .withf(move |_, window, _| window == terminal_window)
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_create_window()
            .withf(move |_, window, _| window == other_window)
            .never();
        tmux.expect_kill_window().never();
        tmux.expect_launch_codex().never();
        tmux.expect_launch_claude()
            .withf(|_, _, _, resume, _| resume.as_deref() == Some("saved-claude-id"))
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_select_window()
            .withf(move |_, window| window == claude_window)
            .times(1)
            .returning(|_, _| Ok(()));
        let mut gui = handle(store, tmux);

        gui.recover_session(
            session_target_by_id(&claude_id),
            SessionRecoveryChoice::Resume,
            None,
            true,
        )
        .unwrap();

        assert_eq!(gui.app.store.projects[0].features[0].sessions.len(), 3);
    }

    #[test]
    fn a_feature_stopped_on_purpose_still_offers_to_resume_a_tab() {
        let (store, target) = recoverable_claude_feature();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(false);
        let mut gui = handle(store, tmux);
        gui.app.user_stopped_features.insert(FEATURE_ID.to_string());

        let option = gui.session_recovery_option(&target).unwrap();

        assert_eq!(
            option.map(|option| option.saved_id).as_deref(),
            Some("saved-claude-id")
        );
    }

    #[test]
    fn gui_recovery_resumes_the_saved_claude_session() {
        let (store, target) = recoverable_claude_feature();
        let mut gui = handle(store, claude_recovery_tmux(Some("saved-claude-id")));

        let option = gui.session_recovery_option(&target).unwrap().unwrap();
        assert_eq!(option.harness, "Claude");
        assert_eq!(option.saved_id, "saved-claude-id");

        let response = gui
            .recover_session(target, SessionRecoveryChoice::Resume, None, true)
            .unwrap();
        assert!(!response.already_running);
        assert_eq!(
            gui.app.store.projects[0].features[0].status,
            ProjectStatus::Idle
        );
    }

    #[test]
    fn gui_recovery_can_start_fresh_and_clear_the_saved_id() {
        let (store, target) = recoverable_claude_feature();
        let mut gui = handle(store, claude_recovery_tmux(None));

        gui.recover_session(target, SessionRecoveryChoice::Clear, None, true)
            .unwrap();

        let session = &gui.app.store.projects[0].features[0].sessions[0];
        assert_eq!(session.claude_session_id, None);
        assert_eq!(session.token_usage_source, None);
    }

    #[test]
    fn recovery_does_not_save_a_choice_when_another_process_started_first() {
        let (store, target) = recoverable_claude_feature();
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists()
            .times(2)
            .returning(move |_| checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) != 0);
        tmux.expect_check_harness_available()
            .times(1)
            .returning(|_| Ok(()));
        let mut gui = handle(store, tmux);

        let response = gui
            .recover_session(target, SessionRecoveryChoice::Clear, None, true)
            .unwrap();

        assert!(response.already_running);
        let session = &gui.app.store.projects[0].features[0].sessions[0];
        assert_eq!(
            session.claude_session_id.as_deref(),
            Some("saved-claude-id")
        );
    }

    #[test]
    fn recovery_reapplies_the_choice_after_an_unrelated_store_write() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let (store, target) = recoverable_claude_feature();
        let writer = crate::db::AmfDb::open(db_file.path()).unwrap();
        writer.save_store(&store).unwrap();
        let path = db_file.path().to_path_buf();
        let mut tmux = claude_recovery_tmux_with_select(None, move |_, _| {
            prepend_project_elsewhere(&path);
            Ok(())
        });
        tmux.expect_kill_session().never();
        let mut gui = handle_loading_from(crate::db::AmfDb::open(db_file.path()).unwrap(), tmux);

        let response = gui
            .recover_session(target, SessionRecoveryChoice::Clear, None, true)
            .unwrap();

        assert!(!response.already_running);
        let on_disk = writer.load_store().unwrap();
        assert_eq!(on_disk.projects.len(), 2);
        assert_eq!(
            on_disk.projects[1].features[0].sessions[0].claude_session_id,
            None
        );
        assert_eq!(on_disk.projects[1].features[0].status, ProjectStatus::Idle);
    }

    #[test]
    fn recovery_stops_its_new_tmux_session_if_target_was_deleted() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let (store, target) = recoverable_claude_feature();
        let writer = crate::db::AmfDb::open(db_file.path()).unwrap();
        writer.save_store(&store).unwrap();
        let path = db_file.path().to_path_buf();
        let mut tmux = claude_recovery_tmux_with_select(None, move |_, _| {
            let db = crate::db::AmfDb::open(&path).unwrap();
            db.save_store(&ProjectStore::empty()).unwrap();
            Ok(())
        });
        tmux.expect_kill_session().times(1).returning(|_| Ok(()));
        let mut gui = handle_loading_from(crate::db::AmfDb::open(db_file.path()).unwrap(), tmux);

        let error = gui
            .recover_session(target, SessionRecoveryChoice::Clear, None, true)
            .unwrap_err();

        assert_eq!(error.kind, GuiErrorKind::Conflict);
        assert!(writer.load_store().unwrap().projects.is_empty());
    }

    #[test]
    fn over_limit_start_waits_for_explicit_gui_approval() {
        let _lease_lock = crate::resources::limits::lock_lease_tests();
        assert_eq!(crate::resources::limits::wait_for_in_flight(0), 0);
        let _lease = crate::resources::limits::HeadlessLease::acquire();

        let store = store_with_one_feature(ProjectStatus::Stopped);
        let mut tmux = claude_cold_start_tmux();
        tmux.expect_list_panes().returning(Vec::new);
        let mut gui = handle(store, tmux);
        gui.app.config.max_concurrent_agents = 1;
        gui.app.config.low_memory_warn_mb = 0;

        let warning = gui.start_feature(target()).unwrap_err();
        assert_eq!(warning.kind, GuiErrorKind::NeedsApproval);
        assert!(warning.message.contains("1 agent already running"));
        assert_eq!(
            gui.app.store.projects[0].features[0].status,
            ProjectStatus::Stopped,
            "a refused start must leave the feature stopped"
        );

        let response = gui.start_feature_with_approval(target(), true).unwrap();
        assert!(!response.already_running);
        assert_eq!(
            gui.app.store.projects[0].features[0].status,
            ProjectStatus::Idle
        );
    }

    #[test]
    fn todo_launch_associates_one_session_and_returns_an_unsent_draft() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("amf.db");
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        store.projects[0].repo = dir.path().to_path_buf();
        store.projects[0].features[0].workdir = dir.path().to_path_buf();
        let seed = crate::db::AmfDb::open(&db_path).unwrap();
        seed.save_store(&store).unwrap();
        let list = seed
            .create_todo_list(
                &crate::db::todos::TodoScope::Project {
                    project_id: PROJECT_ID.into(),
                },
                Some(FEATURE_ID),
            )
            .unwrap();
        let todo = seed
            .add_todo(
                &list.id,
                "Fix the issue",
                Some("Check the logs"),
                crate::db::todos::TodoPriority::Med,
            )
            .unwrap();
        drop(seed);

        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_create_window()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        let mut gui = handle_loading_from(crate::db::AmfDb::open(&db_path).unwrap(), tmux);

        let launched = gui.launch_todo_agent(&todo.id, target(), false).unwrap();
        assert!(!launched.reused_session);
        assert_eq!(launched.target.feature_id, FEATURE_ID);
        assert!(launched.draft_prompt.contains("Fix the issue"));
        assert!(launched.draft_prompt.contains("Check the logs"));
        let linked = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo.id)
            .unwrap()
            .unwrap();
        assert_eq!(linked.work.status, crate::db::todos::TodoStatus::InProgress);
        assert_eq!(
            linked.work.agent_session_id.as_deref(),
            Some(launched.target.session_id.as_str())
        );
        let session = gui.app.store.projects[0].features[0]
            .sessions
            .iter()
            .find(|session| session.id == launched.target.session_id)
            .unwrap();
        assert_eq!(session.todo_reference.as_ref().unwrap().todo_id, todo.id);
        let repeated = gui
            .launch_todo_agent(&todo.id, target(), false)
            .unwrap_err();
        assert_eq!(repeated.kind, GuiErrorKind::Conflict);
    }

    #[test]
    fn failed_todo_agent_launch_rolls_back_its_reservation() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("amf.db");
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        store.projects[0].repo = dir.path().to_path_buf();
        store.projects[0].features[0].workdir = dir.path().to_path_buf();
        let seed = crate::db::AmfDb::open(&db_path).unwrap();
        seed.save_store(&store).unwrap();
        let list = seed
            .create_todo_list(
                &crate::db::todos::TodoScope::Project {
                    project_id: PROJECT_ID.into(),
                },
                Some(FEATURE_ID),
            )
            .unwrap();
        let todo = seed
            .add_todo(
                &list.id,
                "Failing work",
                None,
                crate::db::todos::TodoPriority::Med,
            )
            .unwrap();
        drop(seed);

        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_create_window()
            .times(1)
            .returning(|_, _, _| anyhow::bail!("window failed"));
        tmux.expect_kill_window().times(1).returning(|_, _| Ok(()));
        let mut gui = handle_loading_from(crate::db::AmfDb::open(&db_path).unwrap(), tmux);

        let err = gui
            .launch_todo_agent(&todo.id, target(), false)
            .unwrap_err();
        assert!(err.message.contains("window failed"));
        let restored = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            restored.work.status,
            crate::db::todos::TodoStatus::NotStarted
        );
        assert!(restored.work.agent_session_id.is_none());
    }

    /// Repeated-submission case for stop: a second `stop_feature` on an
    /// already-stopped feature must not touch tmux at all. `MockTmuxOps::new()`
    /// with no expectations means any call at all fails the test.
    #[test]
    fn stopping_an_already_stopped_feature_is_idempotent() {
        let store = store_with_one_feature(ProjectStatus::Stopped);
        let mut gui = handle(store, MockTmuxOps::new());

        let response = gui.stop_feature(target()).unwrap();

        assert!(response.already_stopped);
    }

    #[test]
    fn stopping_a_running_feature_kills_its_tmux_session() {
        let store = store_with_one_feature(ProjectStatus::Idle);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_kill_session().times(1).returning(|_| Ok(()));
        let mut gui = handle(store, tmux);

        let response = gui.stop_feature(target()).unwrap();

        assert!(!response.already_stopped);
        assert_eq!(
            gui.app.store.projects[0].features[0].status,
            ProjectStatus::Stopped
        );
    }

    fn session_target_by_id(session_id: &str) -> SessionTarget {
        SessionTarget {
            project_id: PROJECT_ID.to_string(),
            feature_id: FEATURE_ID.to_string(),
            session_id: session_id.to_string(),
        }
    }

    #[test]
    fn removing_one_of_several_sessions_kills_only_its_window() {
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        let feature = &mut store.projects[0].features[0];
        feature.add_session(SessionKind::Terminal);
        let doomed = feature.add_session(SessionKind::Terminal).id.clone();
        let doomed_window = feature.sessions[1].tmux_window.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_window_exists().return_const(true);
        tmux.expect_kill_window()
            .withf(move |_, window| window == doomed_window)
            .times(1)
            .returning(|_, _| Ok(()));
        tmux.expect_kill_session().never();
        let mut gui = handle(store, tmux);

        let response = gui.remove_session(session_target_by_id(&doomed)).unwrap();

        assert!(!response.feature_stopped);
        let feature = &gui.app.store.projects[0].features[0];
        assert_eq!(feature.sessions.len(), 1);
        assert!(feature.sessions.iter().all(|session| session.id != doomed));
        assert_eq!(feature.status, ProjectStatus::Idle);
    }

    #[test]
    fn removing_the_last_session_stops_the_feature() {
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        let only = store.projects[0].features[0]
            .add_session(SessionKind::Terminal)
            .id
            .clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_kill_session().times(1).returning(|_| Ok(()));
        let mut gui = handle(store, tmux);

        let response = gui.remove_session(session_target_by_id(&only)).unwrap();

        assert!(response.feature_stopped);
        let feature = &gui.app.store.projects[0].features[0];
        assert!(feature.sessions.is_empty());
        assert_eq!(feature.status, ProjectStatus::Stopped);
    }

    #[test]
    fn removing_the_last_live_session_runs_feature_stop_bookkeeping() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        let feature = &mut store.projects[0].features[0];
        let live = feature.add_session(SessionKind::Terminal).clone();
        let stopped = feature.add_session(SessionKind::Codex).id.clone();
        let list = feature.add_session(SessionKind::Todos).id.clone();
        let writer = crate::db::AmfDb::open(db_file.path()).unwrap();
        writer.save_store(&store).unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        let live_window = live.tmux_window.clone();
        tmux.expect_window_exists()
            .returning(move |_, window| window == live_window);
        tmux.expect_kill_window().never();
        tmux.expect_kill_session().times(1).returning(|_| Ok(()));
        let mut gui = handle_loading_from(crate::db::AmfDb::open(db_file.path()).unwrap(), tmux);

        let response = gui.remove_session(session_target_by_id(&live.id)).unwrap();

        assert!(response.feature_stopped);
        assert!(gui.app.user_stopped_features.contains(FEATURE_ID));
        let persisted = writer.load_store().unwrap();
        let feature = &persisted.projects[0].features[0];
        assert_eq!(feature.status, ProjectStatus::Stopped);
        let ids: Vec<_> = feature.sessions.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, vec![stopped, list]);
    }

    #[test]
    fn removing_a_stopped_session_beside_a_live_one_keeps_the_feature_running() {
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        let feature = &mut store.projects[0].features[0];
        let live_window = feature
            .add_session(SessionKind::Terminal)
            .tmux_window
            .clone();
        let stopped = feature.add_session(SessionKind::Terminal).id.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_window_exists()
            .returning(move |_, window| window == live_window);
        tmux.expect_kill_window().never();
        tmux.expect_kill_session().never();
        let mut gui = handle(store, tmux);

        let response = gui.remove_session(session_target_by_id(&stopped)).unwrap();

        assert!(!response.feature_stopped);
        let feature = &gui.app.store.projects[0].features[0];
        assert_eq!(feature.sessions.len(), 1);
        assert_eq!(feature.status, ProjectStatus::Idle);
    }

    #[test]
    fn stopping_one_of_several_sessions_keeps_its_record() {
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        let feature = &mut store.projects[0].features[0];
        feature.add_session(SessionKind::Terminal);
        let stopped = feature.add_session(SessionKind::Terminal).id.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_window_exists().return_const(true);
        tmux.expect_kill_window().times(1).returning(|_, _| Ok(()));
        tmux.expect_kill_session().never();
        let mut gui = handle(store, tmux);

        let response = gui.stop_session(session_target_by_id(&stopped)).unwrap();

        assert!(!response.feature_stopped);
        let feature = &gui.app.store.projects[0].features[0];
        assert_eq!(feature.sessions.len(), 2);
        assert_eq!(feature.status, ProjectStatus::Idle);
    }

    #[test]
    fn stopping_the_only_session_stops_the_feature_and_keeps_it() {
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        let only = store.projects[0].features[0]
            .add_session(SessionKind::Terminal)
            .id
            .clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_window_exists().return_const(true);
        tmux.expect_kill_session().times(1).returning(|_| Ok(()));
        let mut gui = handle(store, tmux);

        let response = gui.stop_session(session_target_by_id(&only)).unwrap();

        assert!(response.feature_stopped);
        let feature = &gui.app.store.projects[0].features[0];
        assert_eq!(feature.sessions.len(), 1);
        assert_eq!(feature.status, ProjectStatus::Stopped);
    }

    #[test]
    fn stopping_the_last_live_session_runs_feature_stop_bookkeeping() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        let feature = &mut store.projects[0].features[0];
        let live = feature.add_session(SessionKind::Terminal).clone();
        // Already stopped, and a session kind with no window at all: neither
        // keeps the tmux session alive once `live`'s window is gone.
        feature.add_session(SessionKind::Codex);
        feature.add_session(SessionKind::Todos);
        let writer = crate::db::AmfDb::open(db_file.path()).unwrap();
        writer.save_store(&store).unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        let live_window = live.tmux_window.clone();
        tmux.expect_window_exists()
            .returning(move |_, window| window == live_window);
        tmux.expect_kill_window().never();
        tmux.expect_kill_session().times(1).returning(|_| Ok(()));
        let mut gui = handle_loading_from(crate::db::AmfDb::open(db_file.path()).unwrap(), tmux);

        let response = gui.stop_session(session_target_by_id(&live.id)).unwrap();

        assert!(response.feature_stopped);
        assert!(gui.app.user_stopped_features.contains(FEATURE_ID));
        let persisted = writer.load_store().unwrap();
        let feature = &persisted.projects[0].features[0];
        assert_eq!(feature.status, ProjectStatus::Stopped);
        assert_eq!(feature.sessions.len(), 3);
    }

    #[test]
    fn live_snapshot_lists_a_session_whose_window_is_gone() {
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        let feature = &mut store.projects[0].features[0];
        let tmux_session = feature.tmux_session.clone();
        let live = feature.add_session(SessionKind::Terminal).clone();
        let gone = feature.add_session(SessionKind::Terminal).id.clone();
        let mut tmux = MockTmuxOps::new();
        let listed = tmux_session.clone();
        tmux.expect_list_sessions()
            .returning(move || Ok(vec![listed.clone()]));
        tmux.expect_window_activity()
            .times(1)
            .returning(move || vec![(tmux_session.clone(), live.tmux_window.clone(), 0)]);
        let mut gui = handle(store, tmux);

        let snapshot = gui.refresh_live_snapshot().unwrap();

        assert_eq!(snapshot.stopped_session_ids, vec![gone]);
    }

    #[test]
    fn recovering_a_stopped_codex_tab_resumes_it_in_its_own_window() {
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        let feature = &mut store.projects[0].features[0];
        feature.add_session(SessionKind::Terminal);
        let codex = feature.add_session(SessionKind::Codex);
        codex.set_token_usage_source_exact(crate::token_tracking::TokenUsageSource {
            provider: crate::token_tracking::TokenUsageProvider::Codex,
            id: "saved-codex".to_string(),
        });
        let codex_id = codex.id.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_window_exists().return_const(false);
        tmux.expect_check_harness_available().returning(|_| Ok(()));
        tmux.expect_create_session_with_window().never();
        tmux.expect_create_window()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_launch_codex()
            .withf(|_, _, _, resume, _| resume.as_deref() == Some("saved-codex"))
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        let mut gui = handle(store, tmux);
        let target = session_target_by_id(&codex_id);

        let option = gui.session_recovery_option(&target).unwrap();
        assert_eq!(
            option.map(|option| option.saved_id).as_deref(),
            Some("saved-codex")
        );
        let response = gui
            .recover_session(target, SessionRecoveryChoice::Resume, None, true)
            .unwrap();

        assert!(!response.already_running);
        assert_eq!(gui.app.store.projects[0].features[0].sessions.len(), 2);
    }

    #[test]
    fn recovery_no_op_reports_conflict_and_restores_the_saved_conversation() {
        // The restart backs out either because the tmux session went down or
        // because the window came back after the recovery check.
        for window_returned in [false, true] {
            let db_file = tempfile::NamedTempFile::new().unwrap();
            let (mut store, target) = recoverable_claude_feature();
            let session = &mut store.projects[0].features[0].sessions[0];
            session.set_token_usage_source_exact(crate::token_tracking::TokenUsageSource {
                provider: crate::token_tracking::TokenUsageProvider::Claude,
                id: "saved-claude-id".to_string(),
            });
            let original_session = serde_json::to_value(session).unwrap();
            let writer = crate::db::AmfDb::open(db_file.path()).unwrap();
            writer.save_store(&store).unwrap();
            let original_version = writer.current_store_version().unwrap();
            let mut tmux = MockTmuxOps::new();
            tmux.expect_session_exists().times(1).return_const(true);
            tmux.expect_session_exists()
                .times(1)
                .return_const(window_returned);
            tmux.expect_window_exists().times(1).return_const(false);
            if window_returned {
                tmux.expect_window_exists().times(1).return_const(true);
            }
            tmux.expect_check_harness_available().returning(|_| Ok(()));
            tmux.expect_create_window().never();
            tmux.expect_launch_claude().never();
            let mut gui =
                handle_loading_from(crate::db::AmfDb::open(db_file.path()).unwrap(), tmux);

            let error = gui
                .recover_session(target, SessionRecoveryChoice::Clear, None, true)
                .unwrap_err();

            assert_eq!(error.kind, GuiErrorKind::Conflict);
            let restored = &gui.app.store.projects[0].features[0].sessions[0];
            assert_eq!(serde_json::to_value(restored).unwrap(), original_session);
            assert_eq!(writer.current_store_version().unwrap(), original_version);
            // Saving unrelated work must not carry the failed Clear choice.
            gui.app.save().unwrap();
            let persisted = writer.load_store().unwrap();
            assert_eq!(
                serde_json::to_value(&persisted.projects[0].features[0].sessions[0]).unwrap(),
                original_session
            );
        }
    }

    #[test]
    fn recovery_no_op_without_a_database_restores_the_session_in_memory() {
        let (store, target) = recoverable_claude_feature();
        let original_session =
            serde_json::to_value(&store.projects[0].features[0].sessions[0]).unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().times(1).return_const(true);
        tmux.expect_session_exists().times(1).return_const(false);
        tmux.expect_window_exists().times(1).return_const(false);
        tmux.expect_check_harness_available().returning(|_| Ok(()));
        let mut gui = handle(store, tmux);

        let error = gui
            .recover_session(target, SessionRecoveryChoice::Clear, None, true)
            .unwrap_err();

        assert_eq!(error.kind, GuiErrorKind::Conflict);
        let restored = &gui.app.store.projects[0].features[0].sessions[0];
        assert_eq!(serde_json::to_value(restored).unwrap(), original_session);
    }

    #[test]
    fn removing_a_stale_session_is_not_found() {
        let store = store_with_one_feature(ProjectStatus::Idle);
        let mut gui = handle(store, MockTmuxOps::new());

        let err = gui
            .remove_session(session_target_by_id("gone"))
            .unwrap_err();

        assert_eq!(err.kind, GuiErrorKind::NotFound);
    }

    #[test]
    fn deleting_a_stopped_non_worktree_feature_removes_it() {
        let mut store = store_with_one_feature(ProjectStatus::Stopped);
        // `spawn_kill_session` asks real tmux; a name nothing uses makes it a
        // no-op, as it is for any stopped feature.
        store.projects[0].features[0].tmux_session =
            "amf-gui-contract-test-no-such-session".to_string();
        let mut gui = handle(store, MockTmuxOps::new());

        let response = gui.delete_feature(target(), None, None).unwrap();

        assert!(matches!(response, DeleteFeatureResponse::Deleted { .. }));
        assert!(gui.app.store.projects[0].features.is_empty());
        assert!(matches!(gui.app.mode, AppMode::Normal));
    }

    #[test]
    fn deleting_a_worktree_with_open_todos_asks_first_and_touches_nothing() {
        use crate::db::todos::{TodoPriority, TodoScope};

        let db_file = tempfile::NamedTempFile::new().unwrap();
        let mut store = store_with_one_feature(ProjectStatus::Stopped);
        store.projects[0].features[0].is_worktree = true;
        let db = crate::db::AmfDb::open(db_file.path()).unwrap();
        db.save_store(&store).unwrap();
        let list = db
            .load_or_create_todo_list(
                &TodoScope::Worktree {
                    project_id: PROJECT_ID.to_string(),
                    workdir: "/tmp/test-workdir".to_string(),
                },
                Some(FEATURE_ID),
            )
            .unwrap();
        db.add_todo(&list.id, "unfinished", None, TodoPriority::Med)
            .unwrap();
        let mut gui = handle_loading_from(db, MockTmuxOps::new());

        let response = gui.delete_feature(target(), None, None).unwrap();

        assert!(matches!(
            response,
            DeleteFeatureResponse::NeedsTodoDisposition { unfinished: 1 }
        ));
        assert_eq!(gui.app.store.projects[0].features.len(), 1);
        let db = gui.app.db.as_ref().unwrap();
        assert_eq!(db.todos(&list.id).unwrap().len(), 1);
    }

    fn deletion_host_fixture(db: crate::db::AmfDb) -> (GuiHandle, String) {
        use crate::db::todos::{TodoPriority, TodoScope};

        let mut store = store_with_one_feature(ProjectStatus::Stopped);
        store.projects[0].features[0].tmux_session =
            format!("amf-gui-delete-test-{}", uuid::Uuid::new_v4());
        for (id, name) in [("first-host", "First"), ("second-host", "Second")] {
            let mut survivor = store.projects[0].features[0].clone();
            survivor.id = id.to_string();
            survivor.name = name.to_string();
            survivor.tmux_session = format!("amf-gui-delete-test-{}", uuid::Uuid::new_v4());
            store.projects[0].features.push(survivor);
        }
        db.save_store(&store).unwrap();
        let list = db
            .load_or_create_todo_list(
                &TodoScope::Project {
                    project_id: PROJECT_ID.to_string(),
                },
                Some(FEATURE_ID),
            )
            .unwrap();
        db.add_todo(&list.id, "Keep this", Some("notes"), TodoPriority::High)
            .unwrap();
        (handle_loading_from(db, MockTmuxOps::new()), list.id)
    }

    #[test]
    fn deleting_a_project_todo_host_collects_a_choice_without_mutation() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let (mut gui, list_id) =
            deletion_host_fixture(crate::db::AmfDb::open(file.path()).unwrap());
        let version = gui.app.store_version;
        let response = gui.delete_feature(target(), None, None).unwrap();
        let DeleteFeatureResponse::NeedsTodoHost { prompt } = response else {
            panic!("expected host choice");
        };
        assert_eq!(prompt.list_id, list_id);
        assert_eq!(prompt.todo_count, 1);
        assert_eq!(
            prompt
                .candidates
                .iter()
                .map(|candidate| candidate.feature_id.as_str())
                .collect::<Vec<_>>(),
            ["first-host", "second-host"]
        );
        assert_eq!(gui.app.store.projects[0].features.len(), 3);
        assert_eq!(gui.app.store_version, version);
        assert!(matches!(gui.app.mode, AppMode::Normal));
        assert_eq!(
            gui.app.db.as_ref().unwrap().todos(&list_id).unwrap().len(),
            1
        );
    }

    #[test]
    fn deleting_a_todo_host_applies_the_explicit_keep_or_delete_choice() {
        use crate::db::todos::TodoScope;
        for feature_id in [Some("second-host"), None] {
            let file = tempfile::NamedTempFile::new().unwrap();
            let (mut gui, list_id) =
                deletion_host_fixture(crate::db::AmfDb::open(file.path()).unwrap());
            let response = gui
                .delete_feature(
                    target(),
                    None,
                    Some(TodoHostChoice {
                        list_id: list_id.clone(),
                        feature_id: feature_id.map(str::to_string),
                    }),
                )
                .unwrap();
            assert!(matches!(response, DeleteFeatureResponse::Deleted { .. }));
            assert_eq!(gui.app.store.projects[0].features.len(), 2);
            assert!(matches!(gui.app.mode, AppMode::Normal));
            let db = gui.app.db.as_ref().unwrap();
            let list = db
                .todo_list(&TodoScope::Project {
                    project_id: PROJECT_ID.to_string(),
                })
                .unwrap();
            if let Some(id) = feature_id {
                assert_eq!(list.unwrap().feature_id.as_deref(), Some(id));
                let todos = db.todos(&list_id).unwrap();
                assert_eq!(todos.len(), 1);
                assert_eq!(todos[0].body.as_deref(), Some("notes"));
            } else {
                assert!(list.is_none());
                assert!(db.todos(&list_id).unwrap().is_empty());
            }
        }
    }

    #[test]
    fn deleting_with_a_stale_todo_host_choice_touches_nothing() {
        use crate::db::todos::TodoScope;
        for stale in ["destination", "list", "host"] {
            let file = tempfile::NamedTempFile::new().unwrap();
            let (mut gui, list_id) =
                deletion_host_fixture(crate::db::AmfDb::open(file.path()).unwrap());
            let writer = crate::db::AmfDb::open(file.path()).unwrap();
            let mut choice = TodoHostChoice {
                list_id: list_id.clone(),
                feature_id: Some("second-host".to_string()),
            };
            match stale {
                "destination" => {
                    let mut store = writer.load_store().unwrap();
                    store.projects[0]
                        .features
                        .retain(|feature| feature.id != "second-host");
                    writer.save_store(&store).unwrap();
                }
                "list" => {
                    writer.delete_todo_list(&list_id).unwrap();
                    writer
                        .create_todo_list(
                            &TodoScope::Project {
                                project_id: PROJECT_ID.to_string(),
                            },
                            Some(FEATURE_ID),
                        )
                        .unwrap();
                    choice.feature_id = None; // stale delete must not remove the new list
                }
                "host" => {
                    writer
                        .set_todo_list_host_feature(&list_id, "first-host")
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let error = gui
                .delete_feature(target(), None, Some(choice))
                .unwrap_err();
            assert_eq!(error.kind, GuiErrorKind::Conflict);
            assert!(
                gui.app.store.projects[0]
                    .features
                    .iter()
                    .any(|feature| feature.id == FEATURE_ID)
            );
            assert!(
                writer
                    .todo_list(&TodoScope::Project {
                        project_id: PROJECT_ID.to_string()
                    })
                    .unwrap()
                    .is_some()
            );
            assert!(matches!(gui.app.mode, AppMode::Normal));
        }
    }

    #[test]
    fn todo_host_choice_follows_ids_after_an_external_reorder_and_rename() {
        use crate::db::todos::TodoScope;
        let file = tempfile::NamedTempFile::new().unwrap();
        let (mut gui, list_id) =
            deletion_host_fixture(crate::db::AmfDb::open(file.path()).unwrap());
        assert!(matches!(
            gui.delete_feature(target(), None, None).unwrap(),
            DeleteFeatureResponse::NeedsTodoHost { .. }
        ));
        let writer = crate::db::AmfDb::open(file.path()).unwrap();
        let mut store = writer.load_store().unwrap();
        store.projects[0].features[2].name = "Renamed second".to_string();
        store.projects[0].features.swap(1, 2);
        writer.save_store(&store).unwrap();
        gui.delete_feature(
            target(),
            None,
            Some(TodoHostChoice {
                list_id,
                feature_id: Some("second-host".to_string()),
            }),
        )
        .unwrap();
        let list = writer
            .todo_list(&TodoScope::Project {
                project_id: PROJECT_ID.to_string(),
            })
            .unwrap()
            .unwrap();
        assert_eq!(list.feature_id.as_deref(), Some("second-host"));
    }

    #[test]
    fn deleting_the_last_feature_or_a_non_host_needs_no_host_choice() {
        use crate::db::todos::TodoScope;
        for last_feature in [false, true] {
            let file = tempfile::NamedTempFile::new().unwrap();
            let (mut gui, list_id) =
                deletion_host_fixture(crate::db::AmfDb::open(file.path()).unwrap());
            if last_feature {
                gui.app.store.projects[0].features.truncate(1);
                gui.app.save().unwrap();
            } else {
                gui.app
                    .db
                    .as_ref()
                    .unwrap()
                    .set_todo_list_host_feature(&list_id, "second-host")
                    .unwrap();
            }
            assert!(matches!(
                gui.delete_feature(target(), None, None).unwrap(),
                DeleteFeatureResponse::Deleted { .. }
            ));
            let db = gui.app.db.as_ref().unwrap();
            let list = db
                .todo_list(&TodoScope::Project {
                    project_id: PROJECT_ID.to_string(),
                })
                .unwrap();
            assert_eq!(list.is_none(), last_feature);
            if let Some(list) = list {
                assert_eq!(list.feature_id.as_deref(), Some("second-host"));
                assert_eq!(db.todos(&list_id).unwrap().len(), 1);
            }
        }
    }

    #[test]
    fn host_choice_is_collected_before_worktree_todos_are_moved() {
        use crate::db::todos::{TodoPriority, TodoScope};
        let file = tempfile::NamedTempFile::new().unwrap();
        let (mut gui, project_list_id) =
            deletion_host_fixture(crate::db::AmfDb::open(file.path()).unwrap());
        gui.app.store.projects[0].features[0].is_worktree = true;
        gui.app.save().unwrap();
        let db = gui.app.db.as_ref().unwrap();
        let worktree_list = db
            .load_or_create_todo_list(
                &TodoScope::Worktree {
                    project_id: PROJECT_ID.to_string(),
                    workdir: "/tmp/test-workdir".to_string(),
                },
                Some(FEATURE_ID),
            )
            .unwrap();
        db.add_todo(&worktree_list.id, "Worktree task", None, TodoPriority::Med)
            .unwrap();

        assert!(matches!(
            gui.delete_feature(target(), None, None).unwrap(),
            DeleteFeatureResponse::NeedsTodoDisposition { unfinished: 1 }
        ));
        let response = gui
            .delete_feature(target(), Some(TodoDeleteChoice::MoveToProject), None)
            .unwrap();
        let DeleteFeatureResponse::NeedsTodoHost { prompt } = response else {
            panic!("expected host choice");
        };
        assert_eq!(prompt.todo_count, 2);
        let db = gui.app.db.as_ref().unwrap();
        assert_eq!(db.todos(&worktree_list.id).unwrap().len(), 1);
        assert_eq!(db.todos(&project_list_id).unwrap().len(), 1);
        assert_eq!(gui.app.store.projects[0].features.len(), 3);
    }

    #[test]
    fn create_project_validation_failure_becomes_a_structured_error() {
        let store = ProjectStore::empty();
        let mut gui = handle(store, MockTmuxOps::new());

        let err = gui
            .create_project(CreateProjectRequest {
                path: PathBuf::from("/tmp"),
                project_name: String::new(),
                preferred_agent: None,
                dry_run: false,
            })
            .unwrap_err();

        assert_eq!(err.kind, GuiErrorKind::Internal);
        assert!(err.message.contains("cannot be empty"));
    }

    #[test]
    fn snapshot_reflects_the_current_store() {
        let store = store_with_one_feature(ProjectStatus::Idle);
        let gui = handle(store, MockTmuxOps::new());

        let snapshot = gui.snapshot();

        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].features[0].id, FEATURE_ID);
    }

    #[test]
    fn live_snapshot_shows_a_disappeared_tmux_session_as_stopped() {
        let mut tmux = MockTmuxOps::new();
        tmux.expect_list_sessions()
            .times(1)
            .returning(|| Ok(vec![]));
        let mut gui = handle(store_with_one_feature(ProjectStatus::Active), tmux);

        let snapshot = gui.refresh_live_snapshot().unwrap();

        assert_eq!(
            snapshot.projects[0].features[0].status,
            ProjectStatus::Stopped
        );
        assert_eq!(
            gui.app.store.projects[0].features[0].status,
            ProjectStatus::Active
        );
    }

    #[test]
    fn live_snapshot_shows_a_restarted_tmux_session_as_idle() {
        let store = store_with_one_feature(ProjectStatus::Stopped);
        let session = store.projects[0].features[0].tmux_session.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_list_sessions()
            .times(1)
            .return_once(move || Ok(vec![session]));
        let mut gui = handle(store, tmux);

        let snapshot = gui.refresh_live_snapshot().unwrap();

        assert_eq!(snapshot.projects[0].features[0].status, ProjectStatus::Idle);
        assert_eq!(
            gui.app.store.projects[0].features[0].status,
            ProjectStatus::Stopped
        );
    }

    #[test]
    fn live_legacy_tmux_name_collision_is_not_shown_or_attached_as_running() {
        let mut store = store_with_one_feature(ProjectStatus::Active);
        let first_session = store.projects[0].features[0]
            .add_session(SessionKind::Terminal)
            .id
            .clone();
        let mut second = store.projects[0].features[0].clone();
        second.id = "other-feature".to_string();
        second.status = ProjectStatus::Stopped;
        store.projects[0].features.push(second);
        let live_name = store.projects[0].features[0].tmux_session.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_list_sessions()
            .times(1)
            .return_once(move || Ok(vec![live_name]));
        tmux.expect_session_exists().return_const(true);
        let mut gui = handle(store, tmux);

        let snapshot = gui.refresh_live_snapshot().unwrap();

        assert!(
            snapshot.projects[0]
                .features
                .iter()
                .all(|feature| feature.status == ProjectStatus::Stopped)
        );
        let attach = gui
            .resolve_session_target(&SessionTarget {
                project_id: PROJECT_ID.to_string(),
                feature_id: FEATURE_ID.to_string(),
                session_id: first_session,
            })
            .unwrap_err();
        assert_eq!(attach.kind, GuiErrorKind::Conflict);
        assert_eq!(
            gui.start_feature(target()).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        assert_eq!(
            gui.stop_feature(target()).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
    }

    #[test]
    fn refresh_snapshot_sees_another_processs_committed_change() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let writer = crate::db::AmfDb::open(tmp.path()).unwrap();
        writer.save_store(&ProjectStore::empty()).unwrap();
        let mut reader = handle_loading_from(
            crate::db::AmfDb::open(tmp.path()).unwrap(),
            MockTmuxOps::new(),
        );
        assert!(reader.refresh_snapshot().unwrap().projects.is_empty());

        writer
            .save_store(&store_with_one_feature(ProjectStatus::Stopped))
            .unwrap();

        let refreshed = reader.refresh_snapshot().unwrap();
        assert_eq!(refreshed.projects[0].features[0].id, FEATURE_ID);
        assert_eq!(
            reader.app.store_version,
            Some(writer.current_store_version().unwrap())
        );
    }

    #[test]
    fn resolve_session_target_finds_the_sessions_tmux_window() {
        let mut store = store_with_one_feature(ProjectStatus::Idle);
        store.projects[0].features[0].add_session(crate::project::SessionKind::Claude);
        let session_id = store.projects[0].features[0].sessions[0].id.clone();
        let mut gui = handle(store, MockTmuxOps::new());

        let resolved = gui
            .resolve_session_target(&SessionTarget {
                project_id: PROJECT_ID.to_string(),
                feature_id: FEATURE_ID.to_string(),
                session_id,
            })
            .unwrap();

        assert_eq!(resolved.tmux_session, "amf-demo-my-feat");
        assert_eq!(resolved.tmux_window, "claude");
    }

    #[test]
    fn resolve_session_target_reports_not_found_for_an_unknown_session() {
        let store = store_with_one_feature(ProjectStatus::Idle);
        let mut gui = handle(store, MockTmuxOps::new());

        let err = gui
            .resolve_session_target(&SessionTarget {
                project_id: PROJECT_ID.to_string(),
                feature_id: FEATURE_ID.to_string(),
                session_id: "does-not-exist".to_string(),
            })
            .unwrap_err();

        assert_eq!(err.kind, GuiErrorKind::NotFound);
    }

    // ── GUI/TUI overlap (AMF_PLAN.md Task 5) ────────────────────────
    //
    // These simulate two live AMF processes -- concretely, two `AmfDb`
    // connections to the same on-disk file, which is exactly what a GUI
    // window and a TUI (or two TUI instances) each have -- one process
    // saving out from under the other's already-loaded, now-stale view.

    #[test]
    fn a_stale_stop_refreshes_and_reuses_the_stopped_feature() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let seed_db = crate::db::AmfDb::open(tmp.path()).unwrap();
        seed_db
            .save_store(&store_with_one_feature(ProjectStatus::Idle))
            .unwrap();
        drop(seed_db);

        // Both "processes" load the same initial snapshot (feature Idle).
        let mut gui_a = handle_loading_from(crate::db::AmfDb::open(tmp.path()).unwrap(), {
            let mut tmux = MockTmuxOps::new();
            tmux.expect_kill_session().times(1).returning(|_| Ok(()));
            tmux
        });
        let mut gui_b = handle_loading_from(crate::db::AmfDb::open(tmp.path()).unwrap(), {
            // B refreshes from the DB before acting, so no second tmux kill
            // is attempted for the already-stopped feature.
            MockTmuxOps::new()
        });

        // A stops the feature first and saves successfully.
        let response_a = gui_a.stop_feature(target()).unwrap();
        assert!(!response_a.already_stopped);

        // B's stale in-memory view is refreshed before the stop decision.
        let response_b = gui_b.stop_feature(target()).unwrap();
        assert!(response_b.already_stopped);

        // B's next read already reflects A's committed change.
        assert_eq!(
            gui_b.snapshot().projects[0].features[0].status,
            ProjectStatus::Stopped
        );
    }

    #[test]
    fn a_stale_start_refreshes_and_reuses_the_running_feature() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let seed_db = crate::db::AmfDb::open(tmp.path()).unwrap();
        seed_db
            .save_store(&store_with_one_feature(ProjectStatus::Stopped))
            .unwrap();
        drop(seed_db);

        let mut gui_a = handle_loading_from(
            crate::db::AmfDb::open(tmp.path()).unwrap(),
            claude_cold_start_tmux(),
        );
        let mut tmux_b = MockTmuxOps::new();
        tmux_b.expect_session_exists().return_const(true);
        let mut gui_b = handle_loading_from(crate::db::AmfDb::open(tmp.path()).unwrap(), tmux_b);

        let response_a = gui_a.start_feature(target()).unwrap();
        assert!(!response_a.already_running);

        // B reloads A's commit before acting and sees the real tmux session,
        // so it neither creates another agent nor reports a false conflict.
        let response_b = gui_b.start_feature(target()).unwrap();
        assert!(response_b.already_running);

        assert_eq!(
            gui_b.snapshot().projects[0].features[0].status,
            ProjectStatus::Idle
        );
    }

    /// The mock chain for one cold Claude start (`ensure_feature_running`'s
    /// full launch sequence for a fresh, sessionless feature), shared by the
    /// single-process happy-path test and the overlap tests, where both
    /// simulated processes attempt (and, for the second, lose) the same
    /// launch.
    fn claude_cold_start_tmux() -> MockTmuxOps {
        let mut tmux = MockTmuxOps::new();
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
        tmux
    }

    // ── the vertical slice, end to end (AMF_PLAN.md Task 7) ─────────
    //
    // Everything above uses `MockTmuxOps`, appropriately for testing
    // `GuiHandle`'s own logic in isolation. This one test instead uses the
    // real `TmuxManager` (a real tmux session, no mocking) to walk the
    // literal sequence Task 7 asks to verify: create -> start -> attach a
    // terminal and interact with it -> restart without a duplicate agent ->
    // "close the GUI" without killing the session -> the same persisted
    // session still controllable afterward (standing in for "usable from
    // the TUI": same DB, same tmux primitives, no GUI-only side channel).
    //
    // The feature's only session is `SessionKind::Terminal`, whose launch
    // arm (`feature_ops.rs`) is a deliberate no-op -- this proves the real
    // start/attach/stop path without spawning a real `claude`/`codex`/...
    // process.

    /// See `gui_terminal::tests::ensure_process_stdin_is_a_pty` for why this
    /// is needed at all. Duplicated rather than shared across the two test
    /// modules: it is ten lines, each copy's own `Once` is independently
    /// idempotent, and a second harmless `dup2` if both happen to run in the
    /// same test binary costs nothing.
    fn ensure_process_stdin_is_a_pty() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| unsafe {
            let mut master: i32 = -1;
            let mut slave: i32 = -1;
            let ok = libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                // `null_mut` for both: macOS declares these `*mut`, Linux
                // `*const`, and `*mut` coerces to `*const` but not back.
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            assert_eq!(ok, 0, "failed to open a pty for the test process's stdin");
            assert_ne!(
                libc::dup2(slave, libc::STDIN_FILENO),
                -1,
                "failed to dup2 the pty slave onto stdin"
            );
            libc::close(slave);
        });
    }

    /// A `GuiHandle` backed by the real `TmuxManager` (not `MockTmuxOps`)
    /// plus the given real `AmfDb` connection -- for the one test that needs
    /// to actually drive a real tmux session rather than assert on mock
    /// call counts. `store_version: None` establishes a baseline on this
    /// handle's first save (see `App::save`'s doc comment); there is no
    /// concurrent writer in this test to race against.
    fn real_handle(db: crate::db::AmfDb, store: ProjectStore) -> GuiHandle {
        let mut app = App::new_for_test(
            store,
            Box::new(crate::tmux::TmuxManager),
            Box::new(MockWorktreeOps::new()),
        );
        app.db = Some(db);
        app.store_version = None;
        GuiHandle {
            app,
            learning_context: None,
            review_context: None,
            review_completion: None,
        }
    }

    #[test]
    fn the_vertical_slice_create_start_attach_restart_close_and_reuse() {
        use crate::gui_terminal::TerminalHandle;
        use crate::project::SessionKind;
        use crate::tmux::TmuxManager;
        use std::time::Duration;

        ensure_process_stdin_is_a_pty();

        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let tmux_session = format!("amf-gui-vertical-slice-{}", uuid::Uuid::new_v4());

        let mut feature = crate::project::Feature::new_for_project(
            "vslice",
            "vslice-feat".to_string(),
            "vslice-feat".to_string(),
            std::env::temp_dir(),
            false,
            crate::project::VibeMode::default(),
            false,
            false,
            AgentKind::default(),
            false,
            false,
        );
        feature.tmux_session = tmux_session.clone();
        feature.add_session(SessionKind::Terminal);
        let session_id = feature.sessions[0].id.clone();
        let tmux_window = feature.sessions[0].tmux_window.clone();
        let feature_id = feature.id.clone();

        let mut project = Project::new(
            "vslice".to_string(),
            PathBuf::from("/tmp/vslice-repo"),
            false,
            AgentKind::default(),
        );
        let project_id = project.id.clone();
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);

        let seed_db = crate::db::AmfDb::open(tmp_db.path()).unwrap();
        seed_db.save_store(&store).unwrap();
        drop(seed_db);

        // Guarantee the real tmux session gets cleaned up even if an
        // assertion below fails partway through.
        struct KillOnDrop(String);
        impl Drop for KillOnDrop {
            fn drop(&mut self) {
                let _ = TmuxManager::kill_session(&self.0);
            }
        }
        let _cleanup = KillOnDrop(tmux_session.clone());

        let target = FeatureTarget {
            project_id: project_id.clone(),
            feature_id: feature_id.clone(),
        };
        let session_target = SessionTarget {
            project_id: project_id.clone(),
            feature_id: feature_id.clone(),
            session_id: session_id.clone(),
        };

        // 1. Create + start, from a fresh handle -- as the GUI's first
        //    launch of this session would.
        let mut gui = real_handle(
            crate::db::AmfDb::open(tmp_db.path()).unwrap(),
            store.clone(),
        );

        let start_response = gui.start_feature(target.clone()).unwrap();
        assert!(!start_response.already_running);
        assert!(TmuxManager::session_exists(&tmux_session));

        // 2. Attach a terminal and actually interact with it -- proving the
        //    started session is real and controllable, not just recorded as
        //    "started" in the store. Resolved through `resolve_session_target`
        //    (the same call `attach_terminal`'s Tauri command makes) rather
        //    than reading `tmux_session`/`tmux_window` off the local
        //    `feature` value, so this exercises that resolution too.
        let terminal_target = gui.resolve_session_target(&session_target).unwrap();
        let (terminal, _initial) = TerminalHandle::attach(
            &terminal_target.tmux_session,
            &terminal_target.tmux_window,
            80,
            24,
            |_| {},
        )
        .unwrap();
        terminal.send_input("echo vertical-slice-marker\r").unwrap();
        let saw_marker = (0..100).find_map(|_| {
            std::thread::sleep(Duration::from_millis(20));
            TmuxManager::capture_pane(&tmux_session, &tmux_window)
                .ok()
                .filter(|content| content.contains("vertical-slice-marker"))
        });
        assert!(
            saw_marker.is_some(),
            "the attached terminal never showed output from input it sent"
        );

        // 3. Restart/reattachment without a duplicated agent: starting the
        //    same feature again must be a no-op, not a second launch.
        let restart_response = gui.start_feature(target.clone()).unwrap();
        assert!(restart_response.already_running);

        // 4. "Close the GUI": drop the terminal attachment and the handle
        //    holding the DB connection -- must not touch the tmux session.
        drop(terminal);
        drop(gui);
        assert!(
            TmuxManager::session_exists(&tmux_session),
            "closing the GUI must not terminate the session it was attached to"
        );

        // 5. The same persisted project/session, reloaded fresh (standing in
        //    for "usable from the TUI": this reload uses nothing the GUI
        //    didn't -- the same DB file and the same `TmuxManager`
        //    primitives) can still see and control it.
        let (reloaded_store, _version) = crate::db::AmfDb::open(tmp_db.path())
            .unwrap()
            .load_store_versioned()
            .unwrap();
        assert_eq!(
            reloaded_store.projects[0].features[0].status,
            ProjectStatus::Idle,
            "the reload must see the running status this same test already saved"
        );
        let mut reloaded = real_handle(
            crate::db::AmfDb::open(tmp_db.path()).unwrap(),
            reloaded_store,
        );

        let stop_response = reloaded.stop_feature(target).unwrap();
        assert!(!stop_response.already_stopped);
        assert!(
            !TmuxManager::session_exists(&tmux_session),
            "stopping from the reloaded handle must actually kill the real session"
        );
    }

    fn todo_new_feature_fixture(
        root: &std::path::Path,
        tmux: MockTmuxOps,
        worktree_create_succeeds: Option<bool>,
    ) -> (GuiHandle, CreateFeatureRequest, String) {
        use crate::db::todos::{TodoPriority, TodoScope};

        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut project = Project::new("demo".into(), repo.clone(), true, AgentKind::default());
        project.id = PROJECT_ID.into();
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let mut worktree = MockWorktreeOps::new();
        let workdir = repo.join(".worktrees").join("todo-work");
        if let Some(worktree_create_succeeds) = worktree_create_succeeds {
            worktree.expect_create().times(1).returning(move |_, _, _| {
                if worktree_create_succeeds {
                    Ok(workdir.clone())
                } else {
                    anyhow::bail!("worktree creation failed")
                }
            });
        }
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(worktree));
        let db = crate::db::AmfDb::open(&root.join("amf.db")).unwrap();
        db.save_store(&app.store).unwrap();
        let list = db
            .create_todo_list(
                &TodoScope::Project {
                    project_id: PROJECT_ID.into(),
                },
                None,
            )
            .unwrap();
        let todo = db
            .add_todo(&list.id, "Improve the API", None, TodoPriority::Med)
            .unwrap();
        app.store_version = Some(db.current_store_version().unwrap());
        app.db = Some(db);
        app.config.max_concurrent_agents = 8;
        app.config.low_memory_warn_mb = 0;
        (
            GuiHandle {
                app,
                learning_context: None,
                review_context: None,
                review_completion: None,
            },
            CreateFeatureRequest {
                project_name: "demo".into(),
                branch: "todo-work".into(),
                agent: AgentKind::default(),
                mode: VibeMode::default(),
                review: false,
                plan_mode: false,
                create_terminal: false,
                use_worktree: Some(true),
                enable_chrome: false,
                hook_choice: None,
                dry_run: false,
            },
            todo.id,
        )
    }

    #[test]
    fn todo_new_feature_starts_one_agent_and_returns_an_unsent_draft() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let root = tempfile::tempdir().unwrap();
        let created = Arc::new(AtomicBool::new(false));
        let seen = created.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists()
            .returning(move |_| seen.load(Ordering::SeqCst));
        tmux.expect_create_session_with_window()
            .times(1)
            .returning(move |_, _, _| {
                created.store(true, Ordering::SeqCst);
                Ok(())
            });
        tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_select_window().returning(|_, _| Ok(()));
        tmux.expect_resize_pane().returning(|_, _, _, _| Ok(()));
        tmux.expect_list_sessions().returning(|| Ok(vec![]));
        let (mut gui, request, todo_id) = todo_new_feature_fixture(root.path(), tmux, Some(true));

        let launched = gui
            .launch_todo_in_new_feature(&todo_id, request, false)
            .unwrap();
        assert!(launched.draft_prompt.contains("Improve the API"));
        assert!(!launched.reused_session);
        assert_eq!(gui.app.store.projects[0].features.len(), 1);
        let todo = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(todo.linked_feature_id, Some(launched.target.feature_id));
        assert_eq!(todo.work.agent_session_id, Some(launched.target.session_id));
        assert_eq!(todo.work.status, crate::db::todos::TodoStatus::InProgress);
        assert!(matches!(gui.app.mode, AppMode::Normal));
    }

    #[test]
    fn failed_todo_new_feature_creation_releases_its_reservation() {
        let root = tempfile::tempdir().unwrap();
        let (mut gui, request, todo_id) =
            todo_new_feature_fixture(root.path(), MockTmuxOps::new(), Some(false));
        let error = gui
            .launch_todo_in_new_feature(&todo_id, request, false)
            .unwrap_err();
        assert!(error.message.contains("worktree creation failed"));
        let todo = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(todo.work.status, crate::db::todos::TodoStatus::NotStarted);
        assert!(gui.app.store.projects[0].features.is_empty());
    }

    #[test]
    fn feature_hook_choice_is_rejected_before_a_worktree_is_created() {
        let root = tempfile::tempdir().unwrap();
        let mut project = Project::new(
            "demo".into(),
            root.path().to_path_buf(),
            true,
            AgentKind::default(),
        );
        project.id = PROJECT_ID.into();
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        // No worktree expectation: an attempt to create one would fail the
        // test before the user had a chance to choose the hook option.
        let mut gui = handle(store, MockTmuxOps::new());
        gui.app.config.extension = ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_worktree_created: Some(HookConfig::WithPrompt {
                    script: "setup.sh".into(),
                    prompt: HookPrompt {
                        title: "Choose stack".into(),
                        options: vec!["rust".into()],
                    },
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let request = CreateFeatureRequest {
            project_name: "demo".into(),
            branch: "new-work".into(),
            use_worktree: Some(true),
            ..Default::default()
        };

        let error = gui.create_feature(request).unwrap_err();
        assert_eq!(error.kind, GuiErrorKind::Conflict);
        assert!(error.message.contains("requires a choice"));
        assert!(gui.app.store.projects[0].features.is_empty());
    }

    fn prompting_worktree_hook(script: &str) -> ExtensionConfig {
        ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_worktree_created: Some(HookConfig::WithPrompt {
                    script: script.into(),
                    prompt: HookPrompt {
                        title: "Choose stack".into(),
                        options: vec!["node".into(), "rust & tools".into()],
                    },
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn worktree_hook_preview_uses_project_overrides_and_rejects_stale_projects() {
        let root = tempfile::tempdir().unwrap();
        let (mut gui, _, _) = todo_new_feature_fixture(root.path(), MockTmuxOps::new(), None);
        gui.app.config.extension = prompting_worktree_hook("exit 0");
        let repo = &gui.app.store.projects[0].repo;
        std::fs::write(
            repo.join("amf.json"),
            serde_json::json!({
                "lifecycle_hooks": { "on_worktree_created": {
                    "script": "exit 0", "prompt": { "title": "Local choice", "options": ["local"] }
                } }
            })
            .to_string(),
        )
        .unwrap();
        let prompt = gui.worktree_hook_prompt(PROJECT_ID).unwrap().unwrap();
        assert_eq!(prompt.title, "Local choice");
        assert_eq!(prompt.options, ["local"]);
        assert_eq!(
            gui.worktree_hook_prompt("deleted-project")
                .unwrap_err()
                .kind,
            GuiErrorKind::NotFound
        );
        assert!(gui.app.store.projects[0].features.is_empty());
    }

    #[test]
    fn todo_hook_choice_is_validated_before_reservation_or_creation() {
        for choice in [None, Some("removed option")] {
            let root = tempfile::tempdir().unwrap();
            let (mut gui, mut request, todo_id) =
                todo_new_feature_fixture(root.path(), MockTmuxOps::new(), None);
            gui.app.config.extension = prompting_worktree_hook("exit 0");
            request.hook_choice = choice.map(str::to_string);
            let error = gui
                .launch_todo_in_new_feature(&todo_id, request, true)
                .unwrap_err();
            assert_eq!(error.kind, GuiErrorKind::Conflict);
            let todo = gui
                .db()
                .unwrap()
                .find_todo_by_id(&todo_id)
                .unwrap()
                .unwrap();
            assert_eq!(todo.work.status, crate::db::todos::TodoStatus::NotStarted);
            assert!(todo.work.agent_session_id.is_none());
            assert!(gui.app.store.projects[0].features.is_empty());
        }
    }

    fn new_hook_agent_tmux() -> MockTmuxOps {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let created = Arc::new(AtomicBool::new(false));
        let seen = created.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists()
            .returning(move |_| seen.load(Ordering::SeqCst));
        tmux.expect_create_session_with_window()
            .times(1)
            .returning(move |_, _, _| {
                created.store(true, Ordering::SeqCst);
                Ok(())
            });
        tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_select_window().returning(|_, _| Ok(()));
        tmux.expect_resize_pane().returning(|_, _, _, _| Ok(()));
        tmux.expect_list_sessions().returning(|| Ok(vec![]));
        tmux
    }

    #[test]
    fn ordinary_creation_runs_a_prompting_hook_with_the_exact_choice() {
        let root = tempfile::tempdir().unwrap();
        let (mut gui, mut request, _) =
            todo_new_feature_fixture(root.path(), new_hook_agent_tmux(), Some(true));
        let repo = gui.app.store.projects[0].repo.clone();
        let workdir = repo.join(".worktrees/todo-work");
        std::fs::create_dir_all(&workdir).unwrap();
        gui.app.config.extension =
            prompting_worktree_hook("printf '%s' \"$AMF_HOOK_CHOICE\" > hook-choice");
        request.hook_choice = Some("rust & tools".into());
        // The real shell hook runs; the agent launch is mocked.
        let response = gui.create_feature(request).unwrap();
        assert_eq!(response.worktree_hook_succeeded, Some(true));
        assert_eq!(
            std::fs::read_to_string(workdir.join("hook-choice")).unwrap(),
            "rust & tools"
        );
        assert_eq!(gui.app.store.projects[0].features.len(), 1);
    }

    #[test]
    fn todo_creation_runs_the_chosen_hook_and_keeps_the_launch_association() {
        let root = tempfile::tempdir().unwrap();
        let (mut gui, mut request, todo_id) =
            todo_new_feature_fixture(root.path(), new_hook_agent_tmux(), Some(true));
        let workdir = gui.app.store.projects[0].repo.join(".worktrees/todo-work");
        std::fs::create_dir_all(&workdir).unwrap();
        gui.app.config.extension =
            prompting_worktree_hook("printf '%s' \"$AMF_HOOK_CHOICE\" > hook-choice");
        request.hook_choice = Some("rust & tools".into());
        let response = gui
            .launch_todo_in_new_feature(&todo_id, request, false)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(workdir.join("hook-choice")).unwrap(),
            "rust & tools"
        );
        let todo = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            todo.work.agent_session_id.as_deref(),
            Some(response.target.session_id.as_str())
        );
        assert_eq!(
            todo.linked_feature_id.as_deref(),
            Some(response.target.feature_id.as_str())
        );
        assert!(response.draft_prompt.contains("Improve the API"));
    }

    #[test]
    fn todo_new_feature_resource_gate_precedes_worktree_creation_and_reservation() {
        let _lease_lock = crate::resources::limits::lock_lease_tests();
        assert_eq!(crate::resources::limits::wait_for_in_flight(0), 0);
        let _lease = crate::resources::limits::HeadlessLease::acquire();
        let root = tempfile::tempdir().unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_list_panes().returning(Vec::new);
        let (mut gui, request, todo_id) = todo_new_feature_fixture(root.path(), tmux, None);
        gui.app.config.max_concurrent_agents = 1;

        let error = gui
            .launch_todo_in_new_feature(&todo_id, request, false)
            .unwrap_err();
        assert_eq!(error.kind, GuiErrorKind::NeedsApproval);
        assert!(gui.app.store.projects[0].features.is_empty());
        let todo = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(todo.work.status, crate::db::todos::TodoStatus::NotStarted);
    }
}
