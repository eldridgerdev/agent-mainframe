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
use crate::app::{App, AppMode, StartIntent, TodoPlanOrigin};
use crate::automation::{
    CreateFeatureRequest, CreateFeatureResponse, CreateProjectRequest, CreateProjectResponse,
};
use crate::project::{Project, ProjectStatus, TodoSessionReference};

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
}

/// Addresses one feature by stable id rather than dashboard selection or a
/// `(project_index, feature_index)` pair, which shift under concurrent
/// mutation. `project_id` is included even though `feature_id` alone
/// (a UUID) is already unambiguous: it catches a caller acting on a stale
/// snapshot from the wrong project with `NotFound` instead of silently
/// hitting an unrelated project's feature of the same id-typo class.
#[derive(Debug, Clone, Deserialize)]
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
}

impl GuiHandle {
    pub fn new(db_path: std::path::PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            app: App::new(db_path)?,
        })
    }

    /// Wrap an already-constructed `App` (typically `App::new_for_test`
    /// plus a real `AmfDb` attached by hand) -- for sibling modules' own
    /// test fixtures (e.g. `gui_todos`'s), which cannot build `GuiHandle`
    /// via a struct literal since `app` has no visibility modifier and they
    /// are not descendants of this module.
    #[cfg(test)]
    pub(crate) fn from_app(app: App) -> Self {
        Self { app }
    }

    pub fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            projects: self.app.store.projects.clone(),
            snapshot_at: chrono::Utc::now(),
        }
    }

    /// Refresh the GUI's in-memory workspace when another AMF process has
    /// committed a newer store version. Tauri events only cover writes made
    /// by this GUI process; an open TUI has its own process and cannot emit
    /// those events into this window.
    pub fn refresh_snapshot(&mut self) -> GuiResult<WorkspaceSnapshot> {
        if let Some(db) = &self.app.db {
            let current = db.current_store_version().map_err(GuiError::from)?;
            if self.app.store_version != Some(current) {
                let (store, version) = db.load_store_versioned().map_err(GuiError::from)?;
                self.app.store = store;
                self.app.store_version = Some(version);
            }
        }
        Ok(self.snapshot())
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
        self.refresh_snapshot()?;
        if let Some(project) = self.app.store.find_project(&request.project_name) {
            let use_worktree = request.use_worktree.unwrap_or(!project.features.is_empty());
            if use_worktree
                && crate::extension::merge_project_extension_config(
                    &self.app.config.extension,
                    &project.repo,
                )
                .lifecycle_hooks
                .on_worktree_created
                .as_ref()
                .and_then(|hook| hook.prompt())
                .is_some()
            {
                // The automation method can ask for a hook choice only after
                // it has created the worktree. Until the GUI supports that
                // detour, reject before a failed request leaves one behind.
                return Err(GuiError::conflict(
                    "This project's worktree hook needs the TUI creation wizard for now",
                ));
            }
        }
        Ok(self.app.create_feature_from_request(&request)?)
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

    /// Narrow crate-internal bridge for the GUI plan adapter. The App type
    /// remains absent from every public GUI contract signature.
    pub(crate) fn app_for_plan(&mut self) -> &mut App {
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
        self.refresh_snapshot()?;
        let (pi, fi) = self.locate(&target)?;
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

        self.refresh_snapshot()?;
        let (pi, fi) = self.locate(&target)?;
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

        self.refresh_snapshot()?;
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
        // Automation checks a prompted hook's choice after creating the
        // worktree. Reject before that side effect until the GUI can ask.
        if crate::extension::merge_project_extension_config(
            &self.app.config.extension,
            &project.repo,
        )
        .lifecycle_hooks
        .on_worktree_created
        .as_ref()
        .and_then(|hook| hook.prompt())
        .is_some()
        {
            return Err(GuiError::conflict(
                "This project's worktree hook needs the TUI creation wizard for now",
            ));
        }
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
        self.refresh_snapshot()?;
        let (pi, fi) = self.locate(&target)?;
        let name = self.app.store.projects[pi].features[fi].name.clone();

        if self.app.store.projects[pi].features[fi].status == ProjectStatus::Stopped {
            return Ok(StopFeatureResponse {
                feature_id: target.feature_id,
                already_stopped: true,
                message: format!("'{name}' is already stopped"),
            });
        }

        self.app.do_stop_feature(pi, fi)?;
        Ok(StopFeatureResponse {
            feature_id: target.feature_id,
            already_stopped: false,
            message: format!("Stopped '{name}'"),
        })
    }

    /// Resolve a `SessionTarget` to the tmux session/window
    /// `gui_terminal::TerminalHandle::attach` needs. Does not check that the
    /// feature is actually running (a stopped feature has no live tmux
    /// session for `attach` to find) -- callers already have `already_running`
    /// from `start_feature`/a snapshot's `feature.status` to check first, and
    /// duplicating that check here would just be a second place for the two
    /// to disagree.
    pub fn resolve_session_target(&self, target: &SessionTarget) -> GuiResult<TerminalTarget> {
        let (pi, fi) = self.locate(&FeatureTarget {
            project_id: target.project_id.clone(),
            feature_id: target.feature_id.clone(),
        })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{ExtensionConfig, HookConfig, HookPrompt, LifecycleHooks};
    use crate::project::{AgentKind, Feature, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::path::PathBuf;

    const PROJECT_ID: &str = "proj-1";
    const FEATURE_ID: &str = "feat-1";

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
        GuiHandle { app }
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
        let gui = handle(store, MockTmuxOps::new());

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
        let gui = handle(store, MockTmuxOps::new());

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
                std::ptr::null(),
                std::ptr::null(),
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
        GuiHandle { app }
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
            GuiHandle { app },
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
        assert!(error.message.contains("TUI creation wizard"));
        assert!(gui.app.store.projects[0].features.is_empty());
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
