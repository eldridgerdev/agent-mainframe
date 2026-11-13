//! D1 of the PR fix destinations: a dedicated "PR Triage" agent session in the
//! feature a PR workflow was opened from.
//!
//! The TUI creates (or reuses) this session implicitly while sending a fix. The
//! GUI splits it into explicit steps so nothing starts by accident:
//!
//! 1. [`catalog`] lists the harnesses the project allows and the default name.
//! 2. [`preview`] is read-only. It says what would be created, or which existing
//!    labelled session would be reused, and whether the soft resource gate would
//!    ask for approval. Its `revision` binds the answer.
//! 3. [`prepare`] repeats every check, refuses a stale `revision`, and creates
//!    the session **stopped**: a saved row with no tmux window and no harness
//!    process. It never starts the agent, sends a prompt or writes to GitHub.
//!
//! The created session is then an ordinary fix target (#814). Starting it is the
//! person's explicit `Start` in the session tab.
//!
//! Identity rules mirror `app::pr_review::domain`: the label is the session's
//! identity, so a label already used by another harness is a conflict, never a
//! silent reuse; two sessions with the label are refused rather than guessed
//! between; a session created elsewhere between preview and prepare changes the
//! revision and so reads as stale. The caller also echoes the PR number, head SHA
//! and comment-set (Triage) or AI-result (Workspace) identity it was looking at;
//! any mismatch is a conflict that names what changed.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::ReapplyOutcome;
use crate::app::pr_review::TRIAGE_SESSION_LABEL;
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult, SessionTarget};
use crate::project::{AgentKind, Feature, FeatureSession, Project, SessionKind};

/// The label sessions had before "PR Triage"; honoured only for the default name.
const LEGACY_LABEL: &str = "PR Review";
const MAX_NAME_CHARS: usize = 60;

/// Which PR workflow is asking, and its id.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrDestinationWorkflow {
    Triage { workflow_id: String },
    Workspace { workflow_id: String },
}

impl PrDestinationWorkflow {
    pub(crate) fn key(&self) -> String {
        match self {
            Self::Triage { workflow_id } => format!("triage:{workflow_id}"),
            Self::Workspace { workflow_id } => format!("workspace:{workflow_id}"),
        }
    }
}

/// What the person was looking at. `set_digest` is the comment set (Triage) or
/// the AI result revision (Workspace); `None` when no AI result is open.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PrIdentity {
    pub pr_number: u32,
    pub head_sha: String,
    pub set_digest: Option<String>,
}

/// The destination being created.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrDestinationChoice {
    DedicatedSession {
        agent: AgentKind,
        /// Custom session name; `None` or blank is the default "PR Triage".
        name: Option<String>,
    },
    /// A new triage companion feature seeded from the PR head (see
    /// `gui_companion_creation`). Created stopped, by a background operation.
    CompanionFeature {
        settings: crate::gui_review::destinations::CompanionSettings,
        /// The answer to the `on_worktree_created` hook's prompt, if it has one.
        hook_choice: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize)]
pub struct PrDestinationRequest {
    pub workflow: PrDestinationWorkflow,
    pub identity: PrIdentity,
    pub choice: PrDestinationChoice,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PrDestinationPrepareRequest {
    pub workflow: PrDestinationWorkflow,
    pub identity: PrIdentity,
    pub choice: PrDestinationChoice,
    /// The `revision` of the preview the person confirmed.
    pub preview_revision: String,
    /// The person accepted the resource warning.
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DedicatedAgent {
    pub agent: AgentKind,
    pub label: String,
    pub unavailable: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DedicatedCatalog {
    pub identity: PrIdentity,
    pub feature_name: String,
    pub default_name: String,
    pub agents: Vec<DedicatedAgent>,
    pub preferred: AgentKind,
}

#[derive(Debug, Serialize)]
pub struct PrDestinationPreview {
    pub revision: String,
    pub identity: PrIdentity,
    pub label: String,
    pub agent: AgentKind,
    pub feature_name: String,
    pub workdir: String,
    pub reuses_existing: bool,
    pub existing_target: Option<SessionTarget>,
    pub creates_session: bool,
    /// Whether the agent is stopped now (a reused session may already run).
    pub stopped: bool,
    pub resource_warning: Option<String>,
    pub consequences: Vec<String>,
    /// Present for a companion feature choice only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub companion: Option<crate::gui_companion_creation::CompanionPreview>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrepareStatus {
    Created,
    Reused,
    /// Another prepare for this workflow is running; nothing new was started.
    InProgress,
    /// A background operation was started; poll `pr_destination_status`.
    Started,
}

#[derive(Clone, Debug, Serialize)]
pub struct PrDestinationPrepared {
    pub status: PrepareStatus,
    pub operation_id: String,
    pub created_target: Option<SessionTarget>,
    pub destination_revision: Option<String>,
    pub destination_kind: crate::gui_pr_workspace::DestinationKind,
    pub stopped: bool,
    pub message: String,
}

pub(crate) fn conflict(message: impl Into<String>) -> GuiError {
    GuiError::conflict(message)
}

pub(crate) fn digest(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("serializable revision"))
    )
}

struct Bound {
    opened: FeatureTarget,
    identity: PrIdentity,
    project: Project,
    feature: Feature,
}

pub(crate) fn current_identity(
    gui: &mut GuiHandle,
    workflow: &PrDestinationWorkflow,
) -> GuiResult<(FeatureTarget, PrIdentity)> {
    match workflow {
        PrDestinationWorkflow::Triage { workflow_id } => {
            let (opened, pr_number, head_sha, set) =
                crate::gui_pr_triage::destination_binding(gui, workflow_id)?;
            Ok((
                opened,
                PrIdentity {
                    pr_number,
                    head_sha,
                    set_digest: Some(set),
                },
            ))
        }
        PrDestinationWorkflow::Workspace { workflow_id } => {
            let (opened, pr_number, head_sha, set) =
                crate::gui_pr_workspace::destination_binding(gui, workflow_id)?;
            Ok((
                opened,
                PrIdentity {
                    pr_number,
                    head_sha,
                    set_digest: set,
                },
            ))
        }
    }
}

/// Resolve the open workflow and its feature. Fails closed on anything it
/// cannot establish.
fn bind(gui: &mut GuiHandle, workflow: &PrDestinationWorkflow) -> GuiResult<Bound> {
    gui.refresh_store()?;
    let (opened, identity) = current_identity(gui, workflow)?;
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&opened.project_id), &opened.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature this PR was opened from was deleted"))?;
    let project = app.store.projects[pi].clone();
    let feature = project.features[fi].clone();
    if feature.pending_worktree_script {
        return Err(conflict(
            "Finish the feature's pending worktree setup before adding a session",
        ));
    }
    if app.feature_tmux_session_is_shared(pi, fi) {
        return Err(conflict(
            "This feature shares a background session with another feature; a triage session cannot be added to it",
        ));
    }
    Ok(Bound {
        opened,
        identity,
        project,
        feature,
    })
}

/// The caller's echoed identity must equal the live one.
pub(crate) fn check_identity(live: &PrIdentity, echoed: &PrIdentity) -> GuiResult<()> {
    if echoed.pr_number != live.pr_number {
        return Err(conflict(format!(
            "The open PR changed from #{} to #{}; reopen this form",
            echoed.pr_number, live.pr_number
        )));
    }
    if echoed.head_sha != live.head_sha {
        return Err(conflict(
            "The PR has new commits since this form opened; refresh and reopen it",
        ));
    }
    if echoed.set_digest != live.set_digest {
        return Err(conflict(
            "The comments or AI findings changed since this form opened; refresh and reopen it",
        ));
    }
    Ok(())
}

fn clean_label(name: &Option<String>) -> GuiResult<String> {
    let trimmed = name.as_deref().map(str::trim).unwrap_or_default();
    if trimmed.is_empty() {
        return Ok(TRIAGE_SESSION_LABEL.to_string());
    }
    if trimmed.chars().count() > MAX_NAME_CHARS {
        return Err(conflict(format!(
            "Session names are limited to {MAX_NAME_CHARS} characters"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(conflict("Session names cannot contain control characters"));
    }
    Ok(trimmed.to_string())
}

fn agent_of(session: &FeatureSession) -> Option<AgentKind> {
    crate::app::session_ops::agent_for_session_kind(&session.kind)
}

/// The labelled session this choice would reuse. Mirrors the TUI's resolver,
/// but a label held by two sessions is refused instead of resolving to the
/// first. The label is the session's identity whatever its kind and however it
/// is capitalised, so a terminal or TODO session already called "pr triage"
/// blocks the name too rather than sitting beside a second, look-alike row.
fn labelled_session<'a>(
    feature: &'a Feature,
    label: &str,
    agent: &AgentKind,
) -> GuiResult<Option<&'a FeatureSession>> {
    let rows_for = |wanted: &str| {
        feature
            .sessions
            .iter()
            .filter(|s| s.label.trim().eq_ignore_ascii_case(wanted.trim()))
            .collect::<Vec<_>>()
    };
    let mut rows = rows_for(label);
    if rows.is_empty() && label.eq_ignore_ascii_case(TRIAGE_SESSION_LABEL) {
        rows = rows_for(LEGACY_LABEL);
    }
    match rows.as_slice() {
        [] => Ok(None),
        [row] => {
            if !row.kind.is_agent_harness() {
                return Err(conflict(format!(
                    "A non-agent session is already named '{}'; choose another session name",
                    row.label
                )));
            }
            if agent_of(row).as_ref() != Some(agent) {
                let existing = agent_of(row).map_or("another agent", |a| match a {
                    AgentKind::Claude => "Claude",
                    AgentKind::Opencode => "Opencode",
                    AgentKind::Codex => "Codex",
                    AgentKind::Pi => "Pi",
                });
                return Err(conflict(format!(
                    "Session '{}' already runs {existing}; choose {existing} or another session name",
                    row.label
                )));
            }
            Ok(Some(*row))
        }
        _ => Err(conflict(format!(
            "More than one session is labelled '{}'; choose a different name or remove the extras so AMF does not have to guess",
            rows[0].label
        ))),
    }
}

fn agent_unavailable(gui: &mut GuiHandle, bound: &Bound, agent: &AgentKind) -> Option<String> {
    let app = gui.app_for_workflow();
    if !app
        .allowed_agents_for_repo(&bound.project.repo)
        .contains(agent)
    {
        return Some("This agent is not allowed or installed for the project".into());
    }
    app.tmux
        .check_harness_available(agent)
        .err()
        .map(|error| error.to_string())
}

pub fn catalog(
    gui: &mut GuiHandle,
    workflow: &PrDestinationWorkflow,
) -> GuiResult<DedicatedCatalog> {
    let bound = bind(gui, workflow)?;
    let allowed = gui
        .app_for_workflow()
        .allowed_agents_for_repo(&bound.project.repo);
    let agents = allowed
        .into_iter()
        .map(|agent| DedicatedAgent {
            label: agent.display_name().into(),
            unavailable: agent_unavailable(gui, &bound, &agent),
            agent,
        })
        .collect();
    Ok(DedicatedCatalog {
        identity: bound.identity,
        feature_name: bound.feature.name,
        default_name: TRIAGE_SESSION_LABEL.into(),
        agents,
        preferred: bound.project.preferred_agent,
    })
}

struct Plan {
    preview: PrDestinationPreview,
    bound: Bound,
    label: String,
    agent: AgentKind,
    existing: Option<FeatureSession>,
}

/// Runs under the GUI handle's lock, like every PR command: the harness probe,
/// the checkout identity read and the tmux window check are short local reads
/// whose answers are bound into the revision, so they are taken from the same
/// state the rest of the plan sees rather than before the lock.
fn plan(gui: &mut GuiHandle, request: &PrDestinationRequest) -> GuiResult<Plan> {
    let bound = bind(gui, &request.workflow)?;
    check_identity(&bound.identity, &request.identity)?;
    let PrDestinationChoice::DedicatedSession { agent, name } = &request.choice else {
        return Err(conflict(
            "A companion feature is created by its own background operation, not as a session",
        ));
    };
    let label = clean_label(name)?;
    if let Some(reason) = agent_unavailable(gui, &bound, agent) {
        return Err(conflict(reason));
    }
    let existing = labelled_session(&bound.feature, &label, agent)?.cloned();
    let feature = &bound.feature;
    let mut consequences = Vec::new();
    let mut resource_warning = None;
    let stopped = match &existing {
        Some(row) => {
            consequences.push(format!(
                "Reuses the existing '{}' session in {}. Nothing is created.",
                row.label, feature.name
            ));
            row.stopped
                || feature.status == crate::project::ProjectStatus::Stopped
                || !gui
                    .app_for_workflow()
                    .tmux
                    .window_exists(&feature.tmux_session, &row.tmux_window)
        }
        None => {
            gui.app_for_workflow()
                .preflight_feedback_agent_setup(&feature.id, agent)
                .map_err(|error| conflict(error.to_string()))?;
            consequences.push(format!(
                "Adds a '{label}' {} session to {}.",
                agent.display_name(),
                feature.name
            ));
            consequences.push(
                "The agent is created stopped: no process runs and no prompt is sent. Start it from the session tab (Start) when you are ready."
                    .into(),
            );
            consequences.push(format!(
                "After the session is saved, writes AMF's local agent notification settings for {} in the feature checkout, as every new session does.",
                agent.display_name()
            ));
            if feature.review {
                consequences.push(
                    "This feature has review enabled, so AMF's review instructions are also kept up to date in the checkout's CLAUDE.local.md."
                        .into(),
                );
            }
            resource_warning = gui
                .require_start_approval(&format!("Adding '{label}'"))
                .err()
                .map(|error| error.message);
            true
        }
    };
    consequences.push(format!(
        "Bound to PR #{} at head {}, as last loaded by AMF; commits pushed since are only noticed after you refresh.",
        bound.identity.pr_number,
        bound.identity.head_sha.chars().take(12).collect::<String>()
    ));
    consequences.push("Nothing is posted to GitHub.".into());
    if existing.is_some() && !stopped {
        consequences
            .push("It is already running; nothing is sent until you confirm a send.".into());
    }
    let revision = digest(&(
        request.workflow.key(),
        &bound.identity,
        &request.choice,
        &label,
        &feature.id,
        &feature.workdir,
        crate::gui_pr_workspace::checkout_identity(&feature.workdir),
        &feature.branch,
        &feature.tmux_session,
        feature.status == crate::project::ProjectStatus::Stopped,
        existing.as_ref().map(|row| {
            (
                &row.id,
                &row.kind,
                &row.label,
                &row.tmux_window,
                &row.created_at,
                row.stopped,
            )
        }),
        resource_warning.is_some(),
    ));
    let existing_target = existing.as_ref().map(|row| SessionTarget {
        project_id: bound.opened.project_id.clone(),
        feature_id: feature.id.clone(),
        session_id: row.id.clone(),
    });
    let preview = PrDestinationPreview {
        revision,
        identity: bound.identity.clone(),
        label: label.clone(),
        agent: agent.clone(),
        feature_name: feature.name.clone(),
        workdir: feature.workdir.display().to_string(),
        reuses_existing: existing.is_some(),
        existing_target,
        creates_session: existing.is_none(),
        stopped,
        resource_warning,
        consequences,
        companion: None,
    };
    Ok(Plan {
        preview,
        bound,
        label,
        agent: agent.clone(),
        existing,
    })
}

/// Read-only: nothing is created, started or written.
pub fn preview(
    gui: &mut GuiHandle,
    request: &PrDestinationRequest,
) -> GuiResult<PrDestinationPreview> {
    if matches!(request.choice, PrDestinationChoice::CompanionFeature { .. }) {
        return crate::gui_companion_creation::preview_read(gui, request).map(|read| read.run());
    }
    plan(gui, request).map(|plan| plan.preview)
}

/// Create (or confirm reuse of) the dedicated session, stopped.
pub fn prepare(
    gui: &mut GuiHandle,
    request: PrDestinationPrepareRequest,
    operation_id: &str,
) -> GuiResult<PrDestinationPrepared> {
    if matches!(request.choice, PrDestinationChoice::CompanionFeature { .. }) {
        return Err(conflict(
            "A companion feature is created by a background operation; start it with the companion creation command",
        ));
    }
    let plain = PrDestinationRequest {
        workflow: request.workflow.clone(),
        identity: request.identity.clone(),
        choice: request.choice.clone(),
    };
    let plan = plan(gui, &plain)?;
    if plan.preview.revision != request.preview_revision {
        return Err(conflict(
            "The feature, session names or PR changed since the preview. Review the preview again before creating the session.",
        ));
    }
    let opened = plan.bound.opened.clone();
    let kind = crate::gui_pr_workspace::DestinationKind::OpenedFeature;
    if let Some(row) = &plan.existing {
        let revision = {
            let feature = &plan.bound.feature;
            crate::gui_pr_workspace::destination_revision(feature, row)
        };
        return Ok(PrDestinationPrepared {
            status: PrepareStatus::Reused,
            operation_id: operation_id.into(),
            created_target: plan.preview.existing_target.clone(),
            destination_revision: Some(revision),
            destination_kind: kind,
            stopped: plan.preview.stopped,
            message: format!("Using the existing '{}' session.", row.label),
        });
    }
    if !request.approved {
        gui.require_start_approval(&format!("Adding '{}'", plan.label))?;
    }
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&opened.project_id), &opened.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature was deleted"))?;
    let (repo, workdir, mode, is_worktree, review_feature, containerized) = {
        let project = &app.store.projects[pi];
        let feature = &project.features[fi];
        (
            project.repo.clone(),
            feature.workdir.clone(),
            feature.mode.clone(),
            feature.is_worktree,
            feature.clone(),
            project.containerized,
        )
    };
    let session_kind: SessionKind = crate::app::session_kind_for_agent(&plan.agent);
    let session = {
        let feature = &mut app.store.projects[pi].features[fi];
        let session = feature.add_session_named(session_kind, plan.label.clone());
        session.stopped = true;
        let session = session.clone();
        feature.collapsed = false;
        session
    };
    // Re-applied by id onto whatever another writer saved meanwhile, so an
    // unrelated concurrent store write is not a hard conflict.
    let (project_id, feature_id) = (opened.project_id.clone(), opened.feature_id.clone());
    let outcome = app.save_reapplying(|store| {
        let Some((pi, fi)) = store.locate_feature_by_id(Some(&project_id), &feature_id) else {
            return false;
        };
        let feature = &mut store.projects[pi].features[fi];
        if !feature.sessions.iter().any(|s| s.id == session.id) {
            feature.sessions.push(session.clone());
        }
        feature.collapsed = false;
        true
    });
    match outcome {
        Ok(ReapplyOutcome::Saved) => {}
        Ok(ReapplyOutcome::TargetGone) => {
            return Err(GuiError::not_found(
                "The feature was deleted before the session was saved. Nothing was created.",
            ));
        }
        Ok(ReapplyOutcome::Conflict) => {
            return Err(conflict(
                "Workspace changed elsewhere (another AMF window or process) before the session was saved. The view was refreshed; review the preview and retry.",
            ));
        }
        Err(error) => {
            // The unsaved row is still in memory; take it back out by id (the
            // indices may not survive a reload).
            if let Some((pi, fi)) = app
                .store
                .locate_feature_by_id(Some(&project_id), &feature_id)
            {
                app.store.projects[pi].features[fi]
                    .sessions
                    .retain(|s| s.id != session.id);
            }
            return Err(GuiError::from(error));
        }
    }
    // Only now touch the checkout. The session is stopped, so nothing is
    // reading these files, and a failed or conflicting save leaves no trace.
    crate::app::setup::ensure_notification_hooks(&workdir, &repo, &mode, &plan.agent, is_worktree);
    crate::app::setup::ensure_feature_review_instructions(&workdir, &review_feature, containerized);
    let Some((pi, fi)) = app
        .store
        .locate_feature_by_id(Some(&project_id), &feature_id)
    else {
        return Err(GuiError::not_found("The feature was deleted"));
    };
    let feature = &app.store.projects[pi].features[fi];
    let revision = crate::gui_pr_workspace::destination_revision(feature, &session);
    Ok(PrDestinationPrepared {
        status: PrepareStatus::Created,
        operation_id: operation_id.into(),
        created_target: Some(SessionTarget {
            project_id: opened.project_id,
            feature_id: opened.feature_id,
            session_id: session.id.clone(),
        }),
        destination_revision: Some(revision),
        destination_kind: kind,
        stopped: true,
        message: format!(
            "Created '{}' ({}), stopped. Nothing is running. Start it from the {} feature's session tab when you are ready.",
            plan.label,
            plan.agent.display_name(),
            plan.bound.feature.name
        ),
    })
}

// ------------------------------------------------------------ operations

struct Running {
    operation_id: String,
}

static OPERATIONS: OnceLock<Mutex<HashMap<String, Running>>> = OnceLock::new();

fn operations() -> &'static Mutex<HashMap<String, Running>> {
    OPERATIONS.get_or_init(Default::default)
}

/// Holds a workflow's single destination operation. Dropping it (including by
/// panic) releases the workflow.
pub struct OperationGuard {
    key: String,
    pub operation_id: String,
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = operations().lock() {
            map.remove(&self.key);
        }
    }
}

/// Namespace of the dedicated-session prepare's operation slot.
pub const SESSION_OPERATIONS: &str = "session";

pub enum OperationStart {
    Started(OperationGuard),
    /// A prepare is already running for this workflow; this is its id.
    Existing(PrDestinationPrepared),
}

/// Claim the workflow's single destination slot. A duplicate submit while one
/// is running gets the running operation back instead of starting another.
///
/// `namespace` names the kind of destination operation ([`SESSION_OPERATIONS`],
/// the companion creation's own), so two kinds of work on one workflow neither
/// block each other nor hand back each other's operation ids.
pub fn begin_operation(workflow: &PrDestinationWorkflow, namespace: &str) -> OperationStart {
    let key = format!("{namespace}:{}", workflow.key());
    let mut map = operations().lock().expect("operations mutex poisoned");
    if let Some(running) = map.get(&key) {
        return OperationStart::Existing(PrDestinationPrepared {
            status: PrepareStatus::InProgress,
            operation_id: running.operation_id.clone(),
            created_target: None,
            destination_revision: None,
            destination_kind: crate::gui_pr_workspace::DestinationKind::OpenedFeature,
            stopped: true,
            message: "This session is already being created. Waiting for that operation.".into(),
        });
    }
    let operation_id = uuid::Uuid::new_v4().to_string();
    map.insert(
        key.clone(),
        Running {
            operation_id: operation_id.clone(),
        },
    );
    OperationStart::Started(OperationGuard { key, operation_id })
}

#[cfg(test)]
mod tests;
