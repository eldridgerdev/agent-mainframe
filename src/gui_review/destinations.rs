//! Explicit feedback destinations. Reads and previews are side-effect free;
//! preparation names every new resource and returns an identity pinned for finish.
use super::*;
use crate::app::{review::FINAL_REVIEW_SESSION_LABEL, review_destination::ReviewCompanionSpec};
use crate::gui_contract::SessionTarget;
use crate::project::{AgentKind, Feature, FeatureSession, Project, VibeMode};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn conflict(error: anyhow::Error) -> GuiError {
    GuiError::conflict(error.to_string())
}
fn hash(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn directory(path: &Path) -> Result<Value> {
    let resolved = path.canonicalize()?;
    let metadata = resolved.metadata()?;
    ensure!(metadata.is_dir(), "Checkout is no longer a directory");
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = metadata.created().ok();
    Ok(json!([
        resolved
            .to_str()
            .context("Checkout path is not valid UTF-8")?,
        identity
    ]))
}
fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "Cannot observe checkout Git identity"
    );
    Ok(String::from_utf8(output.stdout)?.trim().into())
}
#[cfg(test)]
thread_local! {
    // Scoped test documents avoid changing process-global XDG variables or
    // reading/writing the operator's actual configuration in parallel tests.
    static TEST_GLOBAL_DOCUMENT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}
fn global_snapshot(gui: &mut GuiHandle) -> GuiResult<(String, crate::extension::ExtensionConfig)> {
    #[cfg(test)]
    let path = TEST_GLOBAL_DOCUMENT.with(|path| path.borrow().clone());
    #[cfg(not(test))]
    let path: Option<PathBuf> = None;
    if path.is_none() && gui.app_for_workflow().store_path.as_os_str().is_empty() {
        let config = &gui.app_for_workflow().config;
        return Ok((hash(config).map_err(conflict)?, config.extension.clone()));
    }
    let path = path.unwrap_or_else(|| crate::project::amf_config_dir().join("config.json"));
    let (_, config, revision) = crate::gui_settings::read_document(&path)?;
    // Read the current soft-gate policy before an explicit preparation; no
    // settings file is written and no other runtime setting is changed.
    gui.app_for_workflow().config.max_concurrent_agents = config.max_concurrent_agents;
    gui.app_for_workflow().config.low_memory_warn_mb = config.low_memory_warn_mb;
    Ok((revision, config.extension))
}
#[derive(Clone)]
struct BoundFeature {
    project: Project,
    feature: Feature,
    identity: Value,
    config: String,
    global_extension: crate::extension::ExtensionConfig,
}
fn bind(gui: &mut GuiHandle, target: &FeatureTarget) -> GuiResult<BoundFeature> {
    gui.refresh_store()?;
    let (global, global_extension) = global_snapshot(gui)?;
    let settings = crate::gui_project_settings::load(gui, &target.project_id)?;
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .filter(|(pi, _)| app.store.projects[*pi].id == target.project_id)
        .ok_or_else(|| GuiError::not_found("The feedback feature was deleted or moved"))?;
    let project = app.store.projects[pi].clone();
    let feature = project.features[fi].clone();
    if feature.pending_worktree_script || app.paused_plan_interview_belongs_to_feature(&feature.id)
    {
        return Err(GuiError::conflict(
            "Finish the target's pending worktree setup or parked plan first",
        ));
    }
    if app.feature_tmux_session_is_shared(pi, fi) {
        return Err(GuiError::conflict(
            "The target shares a tmux session identity with another feature",
        ));
    }
    let git_identity = if project.is_git {
        let dir = git(&feature.workdir, &["rev-parse", "--absolute-git-dir"]).map_err(conflict)?;
        Some(json!([
            directory(Path::new(&dir)).map_err(conflict)?,
            git(&feature.workdir, &["rev-parse", "--verify", "HEAD"]).map_err(conflict)?,
            git(&feature.workdir, &["rev-parse", "--abbrev-ref", "HEAD"]).map_err(conflict)?
        ]))
    } else {
        None
    };
    let identity = json!([
        &project.id,
        &project.name,
        &project.repo,
        project.is_git,
        directory(&project.repo).map_err(conflict)?,
        &feature.id,
        &feature.name,
        &feature.branch,
        &feature.workdir,
        directory(&feature.workdir).map_err(conflict)?,
        feature.is_worktree,
        &feature.tmux_session,
        &feature.agent,
        &feature.mode,
        feature.review,
        feature.enable_chrome,
        feature.remote_control,
        git_identity
    ]);
    Ok(BoundFeature {
        project,
        feature,
        identity,
        config: hash(&(global, settings.revision)).map_err(conflict)?,
        global_extension,
    })
}
fn companion_hook(source: &BoundFeature) -> Option<crate::extension::HookConfig> {
    crate::extension::merge_project_extension_config(&source.global_extension, &source.project.repo)
        .lifecycle_hooks
        .on_worktree_created
}
fn present_hook(
    hook: Option<&crate::extension::HookConfig>,
) -> Option<crate::automation::AutomationHookPrompt> {
    hook.and_then(|hook| hook.prompt())
        .map(|prompt| crate::automation::AutomationHookPrompt {
            title: prompt.title.clone(),
            options: prompt.options.clone(),
        })
}
fn review_target(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
) -> GuiResult<FeatureTarget> {
    let context = gui
        .review_context
        .as_ref()
        .filter(|context| context.id == workflow_id && context.revision == revision)
        .ok_or_else(|| {
            GuiError::conflict("The review changed. Refresh the destination preview.")
        })?;
    if context.completion.is_some() {
        return Err(GuiError::conflict(
            "Wait for or cancel completion before choosing a destination",
        ));
    }
    let target = context.target.clone();
    let state = ai::state(&gui.app_for_workflow().mode)?;
    if !state.summary_open || state.review_history.is_some() {
        return Err(GuiError::conflict("Open the pre-finish summary first"));
    }
    ai::fresh(state)?;
    Ok(target)
}
fn source(gui: &mut GuiHandle) -> GuiResult<BoundFeature> {
    let target = gui
        .review_context
        .as_ref()
        .ok_or_else(|| GuiError::conflict("Review closed"))?
        .target
        .clone();
    let bound = bind(gui, &target)?;
    if ai::state(&gui.app_for_workflow().mode)?.workdir != bound.feature.workdir {
        return Err(GuiError::conflict("The reviewed checkout changed"));
    }
    Ok(bound)
}
fn validate_agent(
    gui: &mut GuiHandle,
    bound: &BoundFeature,
    agent: &AgentKind,
    mode: &VibeMode,
    primary: bool,
) -> GuiResult<()> {
    let app = gui.app_for_workflow();
    if !app
        .allowed_agents_for_repo(&bound.project.repo)
        .contains(agent)
    {
        return Err(GuiError::conflict(
            "This harness is no longer allowed or installed for the project",
        ));
    }
    if !(if primary {
        mode.supports_agent(agent)
    } else {
        bound.feature.mode.supports_agent(&bound.feature.agent)
    }) {
        return Err(GuiError::conflict(
            "The selected harness does not support this mode",
        ));
    }
    app.tmux.check_harness_available(agent)?;
    Ok(())
}
fn session(gui: &mut GuiHandle, bound: &BoundFeature, id: &str) -> GuiResult<FeatureSession> {
    let row = bound
        .feature
        .sessions
        .iter()
        .find(|row| row.id == id)
        .ok_or_else(|| GuiError::conflict("The feedback session was removed"))?
        .clone();
    let agent = crate::app::session_ops::agent_for_session_kind(&row.kind)
        .ok_or_else(|| GuiError::conflict("Choose an agent session"))?;
    validate_agent(gui, bound, &agent, &bound.feature.mode, false)?;
    Ok(row)
}
fn target_bound(
    gui: &mut GuiHandle,
    target: &SessionTarget,
) -> GuiResult<(BoundFeature, FeatureSession)> {
    let bound = bind(
        gui,
        &FeatureTarget {
            project_id: target.project_id.clone(),
            feature_id: target.feature_id.clone(),
        },
    )?;
    let row = session(gui, &bound, &target.session_id)?;
    Ok((bound, row))
}
fn target_revision(
    gui: &mut GuiHandle,
    source: &BoundFeature,
    target: &SessionTarget,
) -> GuiResult<(String, BoundFeature, FeatureSession)> {
    let (bound, row) = target_bound(gui, target)?;
    let app = gui.app_for_workflow();
    let live = app.tmux.session_exists(&bound.feature.tmux_session)
        && app
            .tmux
            .window_exists(&bound.feature.tmux_session, &row.tmux_window);
    let mut panes = app
        .tmux
        .list_panes()
        .into_iter()
        .filter(|(session, window, _)| {
            session == &bound.feature.tmux_session && window == &row.tmux_window
        })
        .collect::<Vec<_>>();
    panes.sort();
    if live && panes.is_empty() {
        return Err(GuiError::conflict(
            "Cannot observe the feedback session's pane identity; refresh before selecting it",
        ));
    }
    let revision = hash(&(
        &source.identity,
        &source.config,
        &bound.identity,
        &bound.config,
        &row,
        live,
        panes,
    ))
    .map_err(conflict)?;
    Ok((revision, bound, row))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CompanionSettings {
    pub branch: String,
    pub agent: AgentKind,
    pub mode: VibeMode,
    pub review: bool,
    pub enable_chrome: bool,
}
impl CompanionSettings {
    fn spec(&self) -> ReviewCompanionSpec {
        ReviewCompanionSpec {
            branch: self.branch.clone(),
            agent: self.agent.clone(),
            mode: self.mode.clone(),
            review: self.review,
            enable_chrome: self.enable_chrome,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DestinationChoice {
    Existing {
        target: SessionTarget,
    },
    Dedicated {
        agent: AgentKind,
    },
    Companion {
        settings: CompanionSettings,
        hook_choice: Option<String>,
    },
}
#[derive(Serialize)]
pub struct DestinationSession {
    pub target: SessionTarget,
    pub label: String,
    pub current: bool,
    pub stopped: bool,
    pub unavailable: Option<String>,
}
#[derive(Serialize)]
pub struct DestinationAgent {
    pub agent: AgentKind,
    pub label: String,
    pub unavailable: Option<String>,
}
#[derive(Serialize)]
pub struct DestinationCatalog {
    pub sessions: Vec<DestinationSession>,
    pub agents: Vec<DestinationAgent>,
    pub source_path: String,
    pub source_branch: String,
    pub source_head: String,
    pub companion_available: bool,
    pub default_branch: String,
    pub hook_prompt: Option<crate::automation::AutomationHookPrompt>,
    pub selected: Option<SessionTarget>,
}
#[derive(Debug, Serialize)]
pub struct DestinationPreview {
    pub revision: String,
    pub label: String,
    pub source_path: String,
    pub destination_path: Option<String>,
    pub base_branch: Option<String>,
    pub base_sha: Option<String>,
    pub resource_warning: Option<String>,
    pub creates_session: bool,
    pub creates_companion: bool,
    pub hook_prompt: Option<crate::automation::AutomationHookPrompt>,
}
struct Plan {
    view: DestinationPreview,
    source: BoundFeature,
    target: Option<SessionTarget>,
    hook: Option<crate::extension::HookConfig>,
    copy_revision: Option<String>,
}
fn plan(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
    choice: &DestinationChoice,
) -> GuiResult<Plan> {
    review_target(gui, workflow_id, revision)?;
    let source = source(gui)?;
    let mut target = None;
    let mut target_stamp = None;
    let mut base_sha = None;
    let mut hook = None;
    let mut copy_revision = None;
    let (label, destination_path, creates_session, creates_companion) = match choice {
        DestinationChoice::Existing { target: chosen } => {
            let (stamp, bound, row) = target_revision(gui, &source, chosen)?;
            target = Some(chosen.clone());
            target_stamp = Some(stamp);
            (
                format!(
                    "{} / {} / {}",
                    bound.project.name, bound.feature.name, row.label
                ),
                Some(bound.feature.workdir.display().to_string()),
                false,
                false,
            )
        }
        DestinationChoice::Dedicated { agent } => {
            validate_agent(gui, &source, agent, &source.feature.mode, false)?;
            let rows = source
                .feature
                .sessions
                .iter()
                .filter(|row| {
                    row.kind.is_agent_harness() && row.label == FINAL_REVIEW_SESSION_LABEL
                })
                .collect::<Vec<_>>();
            if rows.len() > 1 {
                return Err(GuiError::conflict(
                    "Multiple Final Review sessions exist; choose one explicitly",
                ));
            }
            if let Some(row) = rows.first() {
                if crate::app::session_ops::agent_for_session_kind(&row.kind).as_ref()
                    != Some(agent)
                {
                    return Err(GuiError::conflict(
                        "Final Review already uses another harness. Choose that harness or an explicit existing session.",
                    ));
                }
                let chosen = SessionTarget {
                    project_id: source.project.id.clone(),
                    feature_id: source.feature.id.clone(),
                    session_id: row.id.clone(),
                };
                target_stamp = Some(target_revision(gui, &source, &chosen)?.0);
                target = Some(chosen);
            }
            (
                format!(
                    "{} / Final Review ({})",
                    source.feature.name,
                    agent.display_name()
                ),
                Some(source.feature.workdir.display().to_string()),
                target.is_none(),
                false,
            )
        }
        DestinationChoice::Companion { settings, .. } => {
            validate_agent(gui, &source, &settings.agent, &settings.mode, true)?;
            let branch = settings.branch.trim();
            if branch.is_empty() {
                return Err(GuiError::conflict("Choose a companion branch name"));
            }
            let app = gui.app_for_workflow();
            if source.project.features.iter().any(|feature| {
                crate::project::normalized_feature_name(&feature.name)
                    == crate::project::normalized_feature_name(branch)
            }) || crate::app::triage_feature::branch_exists(&source.feature.workdir, branch)
            {
                return Err(GuiError::conflict(
                    "That companion feature or branch already exists",
                ));
            }
            let head = git(
                &source.feature.workdir,
                &["rev-parse", "--verify", &source.feature.branch],
            )
            .map_err(conflict)?;
            if head
                != git(&source.feature.workdir, &["rev-parse", "--verify", "HEAD"])
                    .map_err(conflict)?
                || git(
                    &source.feature.workdir,
                    &["symbolic-ref", "--quiet", "--short", "HEAD"],
                )
                .map_err(conflict)?
                    != source.feature.branch
            {
                return Err(GuiError::conflict(
                    "The feature's recorded branch is not its checked-out branch. Refresh its identity before creating a companion.",
                ));
            }
            copy_revision = Some(
                crate::app::review_destination::companion_copy_revision(
                    &source.project.repo,
                    &head,
                )
                .map_err(conflict)?,
            );
            base_sha = Some(head);
            hook = companion_hook(&source);
            (
                format!(
                    "New companion {} ({}, {})",
                    branch,
                    settings.agent.display_name(),
                    settings.mode.display_name()
                ),
                None,
                true,
                true,
            )
        }
    };
    let resource_warning = if creates_session {
        gui.require_start_approval(&format!("Preparing {label}"))
            .err()
            .map(|error| error.message)
    } else {
        None
    };
    let view = DestinationPreview {
        revision: hash(&(
            workflow_id,
            revision,
            choice,
            &source.identity,
            &source.config,
            target_stamp,
            &base_sha,
            &hook,
            &copy_revision,
            resource_warning.is_some(),
        ))
        .map_err(conflict)?,
        label,
        source_path: source.feature.workdir.display().to_string(),
        destination_path,
        base_branch: base_sha.as_ref().map(|_| source.feature.branch.clone()),
        base_sha,
        resource_warning,
        creates_session,
        creates_companion,
        hook_prompt: present_hook(hook.as_ref()),
    };
    Ok(Plan {
        view,
        source,
        target,
        hook,
        copy_revision,
    })
}
pub fn catalog(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
) -> GuiResult<DestinationCatalog> {
    review_target(gui, workflow_id, revision)?;
    let source = source(gui)?;
    let projects = gui.app_for_workflow().store.projects.clone();
    let mut sessions = Vec::new();
    for project in projects {
        for feature in &project.features {
            for row in feature
                .sessions
                .iter()
                .filter(|row| row.kind.is_agent_harness())
            {
                let target = SessionTarget {
                    project_id: project.id.clone(),
                    feature_id: feature.id.clone(),
                    session_id: row.id.clone(),
                };
                let unavailable = target_bound(gui, &target).err().map(|error| error.message);
                sessions.push(DestinationSession {
                    label: format!("{} / {} / {}", project.name, feature.name, row.label),
                    current: feature.id == source.feature.id,
                    stopped: row.stopped
                        || feature.status == crate::project::ProjectStatus::Stopped,
                    target,
                    unavailable,
                });
            }
        }
    }
    let agents = gui
        .app_for_workflow()
        .allowed_agents_for_repo(&source.project.repo)
        .into_iter()
        .map(|agent| {
            let unavailable = gui
                .app_for_workflow()
                .tmux
                .check_harness_available(&agent)
                .err()
                .map(|error| error.to_string());
            DestinationAgent {
                label: agent.display_name().into(),
                agent,
                unavailable,
            }
        })
        .collect();
    let default_branch = gui
        .app_for_workflow()
        .unique_review_branch(&source.feature.workdir, &source.feature.branch);
    let hook_prompt = present_hook(companion_hook(&source).as_ref());
    Ok(DestinationCatalog {
        sessions,
        agents,
        source_path: source.feature.workdir.display().to_string(),
        source_branch: source.feature.branch.clone(),
        source_head: if source.project.is_git {
            git(
                &source.feature.workdir,
                &["rev-parse", "--verify", &source.feature.branch],
            )
            .map_err(conflict)?
        } else {
            String::new()
        },
        companion_available: source.project.is_git,
        default_branch,
        hook_prompt,
        selected: gui
            .review_context
            .as_ref()
            .and_then(|context| context.destination.as_ref())
            .map(|destination| destination.target.clone()),
    })
}
pub fn preview(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
    choice: DestinationChoice,
) -> GuiResult<DestinationPreview> {
    Ok(plan(gui, workflow_id, revision, &choice)?.view)
}

#[derive(Clone)]
pub(super) struct PreparedFeedbackDestination {
    pub target: SessionTarget,
    pub revision: String,
    pub kind: String,
    pub label: String,
    pub warning: Option<String>,
    request_key: String,
}
#[derive(Clone)]
pub(super) struct ResolvedDestination {
    pub target: SessionTarget,
    pub revision: String,
    pub label: String,
    pub stopped: bool,
    pub source_path: PathBuf,
    pub workdir: PathBuf,
    pub kind: String,
    pub warning: Option<String>,
}
pub(super) fn resolved(gui: &mut GuiHandle) -> GuiResult<Option<ResolvedDestination>> {
    let source = source(gui)?;
    let prepared = gui
        .review_context
        .as_ref()
        .and_then(|context| context.destination.clone());
    let (target, expected, kind, warning) = if let Some(prepared) = prepared {
        (
            prepared.target,
            Some(prepared.revision),
            prepared.kind,
            prepared.warning,
        )
    } else {
        let Some(si) = crate::app::pr_review::fix_session_index(
            &source.feature,
            crate::app::pr_review::FixTarget::ExistingLive,
            FINAL_REVIEW_SESSION_LABEL,
        ) else {
            return Ok(None);
        };
        (
            SessionTarget {
                project_id: source.project.id.clone(),
                feature_id: source.feature.id.clone(),
                session_id: source.feature.sessions[si].id.clone(),
            },
            None,
            "current".into(),
            None,
        )
    };
    let (revision, bound, row) = target_revision(gui, &source, &target)?;
    if expected
        .as_ref()
        .is_some_and(|expected| expected != &revision)
    {
        return Err(GuiError::conflict(
            "The prepared feedback destination changed. Select it again before completing.",
        ));
    }
    let app = gui.app_for_workflow();
    let stopped = row.stopped
        || bound.feature.status == crate::project::ProjectStatus::Stopped
        || !app.tmux.session_exists(&bound.feature.tmux_session)
        || !app
            .tmux
            .window_exists(&bound.feature.tmux_session, &row.tmux_window);
    Ok(Some(ResolvedDestination {
        target,
        revision,
        label: if kind == "current" {
            row.label.clone()
        } else {
            format!(
                "{} / {} / {}",
                bound.project.name, bound.feature.name, row.label
            )
        },
        stopped,
        source_path: source.feature.workdir,
        workdir: bound.feature.workdir,
        kind,
        warning,
    }))
}

fn remember(
    gui: &mut GuiHandle,
    target: SessionTarget,
    kind: String,
    label: String,
    request_key: String,
    warning: Option<String>,
) -> GuiResult<()> {
    // Store stable IDs first, even when a hook or launch has invalidated the
    // reviewed source. A refused continuation must not lose a created resource.
    gui.review_context.as_mut().unwrap().destination = Some(PreparedFeedbackDestination {
        target: target.clone(),
        revision: String::new(),
        kind,
        label,
        warning,
        request_key,
    });
    let source = source(gui)?;
    let revision = target_revision(gui, &source, &target)?.0;
    gui.review_context
        .as_mut()
        .unwrap()
        .destination
        .as_mut()
        .unwrap()
        .revision = revision;
    Ok(())
}
pub(super) fn prepare(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
    choice: DestinationChoice,
    expected: &str,
    approved: bool,
) -> GuiResult<ReviewView> {
    review_target(gui, workflow_id, revision)?;
    let context = gui.review_context.as_ref().unwrap();
    if context.save_error.is_some() || context.ready_comment.is_some() {
        return Err(GuiError::conflict(
            "Save review progress and resolve generated drafts before preparing a destination",
        ));
    }
    let app = gui.app_for_workflow();
    let state = ai::state(&app.mode)?;
    if ai::view(app)?.running || state.questions.draft.is_some() {
        return Err(GuiError::conflict(
            "Wait for AI work and resolve its draft first",
        ));
    }
    let request_key = hash(&choice).map_err(conflict)?;
    // Replayed preparation after a successful create (including a refused
    // start) selects its exact session again; it never creates another one.
    if let Some(previous) = gui
        .review_context
        .as_ref()
        .unwrap()
        .destination
        .clone()
        .filter(|previous| {
            previous.request_key == request_key
                && matches!(choice, DestinationChoice::Companion { .. })
        })
    {
        let current_source = source(gui)?;
        if target_revision(gui, &current_source, &previous.target)?.0 != previous.revision {
            return Err(GuiError::conflict(
                "The retained companion changed. Select its existing session explicitly before continuing.",
            ));
        }
        remember(
            gui,
            previous.target,
            previous.kind,
            previous.label,
            request_key,
            previous.warning,
        )?;
        gui.review_context.as_mut().unwrap().revision += 1;
        return snapshot(gui);
    }
    let plan = plan(gui, workflow_id, revision, &choice)?;
    if plan.view.revision != expected {
        return Err(GuiError::conflict(
            "Destination, configuration or branch changed. Refresh its preview before preparing it.",
        ));
    }
    if plan.view.creates_session && !approved {
        gui.require_start_approval(&format!("Preparing {}", plan.view.label))?;
    }
    let warning = None;
    let (target, kind) = match &choice {
        DestinationChoice::Existing { target } => (target.clone(), "existing"),
        DestinationChoice::Dedicated { agent } => {
            if let Some(target) = plan.target.clone() {
                (target, "dedicated")
            } else {
                gui.app_for_workflow()
                    .preflight_feedback_agent_setup(&plan.source.feature.id, agent)?;
                let created = gui.add_session(
                    FeatureTarget {
                        project_id: plan.source.project.id.clone(),
                        feature_id: plan.source.feature.id.clone(),
                    },
                    crate::app::session_kind_for_agent(agent),
                    Some(FINAL_REVIEW_SESSION_LABEL.into()),
                    approved,
                )?;
                (created.target, "dedicated")
            }
        }
        DestinationChoice::Companion {
            settings,
            hook_choice,
        } => {
            let registration = gui.app_for_workflow().plan_review_companion(
                &plan.source.feature.id,
                &settings.spec(),
                plan.hook.as_ref(),
                hook_choice.as_deref(),
                true,
                plan.view.base_sha.as_deref(),
            )?;
            if plan.copy_revision.as_ref() != Some(&registration.copy_revision) {
                return Err(GuiError::conflict(
                    "Repository settings changed. Refresh the companion preview before creating it.",
                ));
            }
            let worktree = gui.app_for_workflow().worktree.clone();
            let worker_plan = registration.clone();
            let receiver = spawn_job(move || {
                ensure!(
                    crate::app::review_destination::companion_copy_revision(
                        &worker_plan.repo,
                        &worker_plan.base_sha
                    )? == worker_plan.copy_revision,
                    "Repository settings changed before companion creation. Refresh its preview."
                );
                worktree
                    .create_from(
                        &worker_plan.repo,
                        &crate::project::worktree_name(
                            &worker_plan.project_name,
                            &worker_plan.spec.branch,
                        ),
                        &worker_plan.spec.branch,
                        &worker_plan.base_sha,
                    )
                    .map(WorkerResult::Checkout)
            })?;
            let context = gui.review_context.as_mut().unwrap();
            context.revision += 1;
            context.preparation_error = None;
            context.preparation = Some(Preparation {
                id: uuid::Uuid::new_v4().to_string(),
                receiver,
                registration,
                source: plan.source,
                hook: plan.hook,
                hook_choice: hook_choice.clone(),
                target: None,
                target_revision: None,
                cancelled: false,
                approved,
                label: plan.view.label,
                request_key,
                expected_revision: context.revision,
            });
            return snapshot(gui);
        }
    };
    remember(
        gui,
        target,
        kind.into(),
        plan.view.label,
        request_key,
        warning,
    )?;
    let context = gui.review_context.as_mut().unwrap();
    context.preparation_error = None;
    context.revision += 1;
    snapshot(gui)
}

enum WorkerResult {
    Checkout(PathBuf),
    Hook,
}
fn spawn_job(
    job: impl FnOnce() -> anyhow::Result<WorkerResult> + Send + 'static,
) -> GuiResult<std::sync::mpsc::Receiver<anyhow::Result<WorkerResult>>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("review-destination-setup".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Destination setup worker panicked")));
            let _ = sender.send(outcome);
        })
        .map_err(anyhow::Error::from)?;
    Ok(receiver)
}
pub(super) struct Preparation {
    id: String,
    receiver: std::sync::mpsc::Receiver<anyhow::Result<WorkerResult>>,
    registration: crate::app::review_destination::ReviewCompanionRegistration,
    source: BoundFeature,
    hook: Option<crate::extension::HookConfig>,
    hook_choice: Option<String>,
    target: Option<SessionTarget>,
    target_revision: Option<String>,
    cancelled: bool,
    approved: bool,
    label: String,
    request_key: String,
    expected_revision: u64,
}
impl Preparation {
    pub(super) fn matches_id(&self, id: &str) -> bool {
        self.id == id
    }
}
#[derive(Debug, Serialize)]
pub struct PreparationView {
    pub operation_id: String,
    pub label: String,
    pub cancelled: bool,
    pub created_target: Option<SessionTarget>,
}
pub(super) fn preparation_view(gui: &GuiHandle) -> Option<PreparationView> {
    gui.review_context
        .as_ref()?
        .preparation
        .as_ref()
        .map(|operation| PreparationView {
            operation_id: operation.id.clone(),
            label: operation.label.clone(),
            cancelled: operation.cancelled,
            created_target: operation.target.clone(),
        })
}
pub(super) fn cancel_preparation(gui: &mut GuiHandle, operation_id: &str) -> GuiResult<()> {
    let operation = gui
        .review_context
        .as_mut()
        .and_then(|context| context.preparation.as_mut())
        .filter(|operation| operation.id == operation_id)
        .ok_or_else(|| GuiError::conflict("Destination setup operation changed"))?;
    operation.cancelled = true;
    Ok(())
}
fn validate_continuation(gui: &mut GuiHandle, operation: &Preparation) -> GuiResult<()> {
    let context = gui
        .review_context
        .as_ref()
        .ok_or_else(|| GuiError::conflict("Review closed"))?;
    if operation.cancelled || context.revision != operation.expected_revision {
        return Err(GuiError::conflict(
            "Pending launch was cancelled; created resources are retained",
        ));
    }
    let current = source(gui)?;
    if current.identity != operation.source.identity || current.config != operation.source.config {
        return Err(GuiError::conflict(
            "Source checkout, branch or configuration changed during setup",
        ));
    }
    let state = ai::state(&gui.app_for_workflow().mode)?;
    ai::fresh(state)?;
    if progress_bytes(&state.workdir)? != gui.review_context.as_ref().unwrap().progress {
        return Err(GuiError::conflict(
            "Saved review changed during destination setup",
        ));
    }
    if crate::app::review_destination::companion_copy_revision(
        &operation.registration.repo,
        &operation.registration.base_sha,
    )
    .map_err(conflict)?
        != operation.registration.copy_revision
    {
        return Err(GuiError::conflict(
            "Repository settings copy inputs changed during companion setup",
        ));
    }
    if let Some(target) = &operation.target {
        let (revision, bound, row) = target_revision(gui, &current, target)?;
        if operation
            .target_revision
            .as_ref()
            .is_some_and(|expected| expected != &revision)
        {
            return Err(GuiError::conflict(
                "Created companion checkout, configuration or session changed during setup",
            ));
        }
        if row.label != FINAL_REVIEW_SESSION_LABEL
            || row.kind != crate::app::session_kind_for_agent(&operation.registration.spec.agent)
            || bound.feature.review_source.as_ref().is_none_or(|link| {
                link.source_feature_id != operation.registration.source_id
                    || link.base_sha != operation.registration.base_sha
            })
        {
            return Err(GuiError::conflict(
                "Created companion identity changed during setup",
            ));
        }
    }
    Ok(())
}
/// Poll only owned work; no worktree process or hook runs on the coordinator.
pub(super) fn poll_preparation(gui: &mut GuiHandle) -> GuiResult<bool> {
    let Some(mut operation) = gui
        .review_context
        .as_mut()
        .and_then(|context| context.preparation.take())
    else {
        return Ok(false);
    };
    let outcome = match operation.receiver.try_recv() {
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            gui.review_context.as_mut().unwrap().preparation = Some(operation);
            return Ok(true);
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!(
            "Destination setup worker exited without a result"
        )),
        Ok(outcome) => outcome,
    };
    let continuation = (|| -> GuiResult<bool> {
        match outcome? {
            WorkerResult::Checkout(workdir) => {
                // Registration is deliberately completed even after cancellation:
                // the owned checkout remains visible and recoverable by stable ID.
                let created = gui
                    .app_for_workflow()
                    .register_created_review_companion(&operation.registration, workdir.clone())?;
                let target = SessionTarget {
                    project_id: created.project_id,
                    feature_id: created.feature_id,
                    session_id: created.session_id,
                };
                operation.target = Some(target.clone());
                let _ = remember(
                    gui,
                    target,
                    "companion".into(),
                    operation.label.clone(),
                    operation.request_key.clone(),
                    None,
                );
                validate_continuation(gui, &operation)?;
                operation.target_revision = Some(
                    target_revision(gui, &operation.source, operation.target.as_ref().unwrap())?.0,
                );
                if let Some(hook) = &operation.hook {
                    let script = hook.script().to_string();
                    let choice = operation.hook_choice.clone();
                    operation.receiver = spawn_job(move || {
                        let (ok, detail) = crate::app::App::run_worktree_hook_sync(
                            &script,
                            &workdir,
                            choice.as_deref(),
                        );
                        ensure!(
                            ok,
                            "Worktree hook failed: {}",
                            detail.unwrap_or_else(|| "unknown error".into())
                        );
                        Ok(WorkerResult::Hook)
                    })?;
                    return Ok(true);
                }
            }
            WorkerResult::Hook => {}
        }
        validate_continuation(gui, &operation)?;
        let target = operation
            .target
            .clone()
            .ok_or_else(|| GuiError::conflict("Companion registration is missing"))?;
        gui.app_for_workflow().preflight_feedback_agent_setup(
            &target.feature_id,
            &operation.registration.spec.agent,
        )?;
        gui.start_session(target.clone(), operation.approved)?;
        remember(
            gui,
            target,
            "companion".into(),
            operation.label.clone(),
            operation.request_key.clone(),
            None,
        )?;
        Ok(false)
    })();
    match continuation {
        Ok(true) => {
            let context = gui.review_context.as_mut().unwrap();
            context.revision += 1;
            operation.expected_revision = context.revision;
            context.preparation = Some(operation);
        }
        outcome => {
            let warning = outcome.err().map(|error| format!("{}: {}. Any created companion checkout/feature is retained for recovery; launch was not completed.", operation.label, error.message));
            if let Some(target) = operation.target {
                let _ = remember(
                    gui,
                    target,
                    "companion".into(),
                    operation.label,
                    operation.request_key,
                    warning.clone(),
                );
            }
            let context = gui.review_context.as_mut().unwrap();
            context.preparation_error = warning;
            context.revision += 1;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{ProjectStore, SessionKind};
    use crate::traits::MockWorktreeOps;
    use std::time::{Duration, Instant};

    fn review() -> (tempfile::TempDir, GuiHandle, ReviewView) {
        let (dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let app = gui.app_for_workflow();
        app.store.projects[0].features[0].add_session(SessionKind::Claude);
        app.save().unwrap();
        let view = begin(&mut gui, target).unwrap();
        let view = act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            ReviewAction::General {
                text: "Fix invoice rounding.".into(),
            },
        )
        .unwrap()
        .unwrap();
        let view = act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            ReviewAction::SummaryOpen,
        )
        .unwrap()
        .unwrap();
        (dir, gui, view)
    }
    fn companion(branch: &str) -> DestinationChoice {
        DestinationChoice::Companion {
            settings: CompanionSettings {
                branch: branch.into(),
                agent: AgentKind::Claude,
                mode: VibeMode::SuperVibe,
                review: false,
                enable_chrome: false,
            },
            hook_choice: None,
        }
    }
    fn refresh_store(dir: &tempfile::TempDir, edit: impl FnOnce(&mut ProjectStore)) {
        let db = crate::db::AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = db.load_store().unwrap();
        edit(&mut store);
        db.save_store(&store).unwrap();
    }
    fn selected(view: &ReviewView) -> DestinationChoice {
        DestinationChoice::Existing {
            target: view
                .finish
                .as_ref()
                .unwrap()
                .handoff
                .as_ref()
                .unwrap()
                .target
                .clone(),
        }
    }
    fn prepare_choice(
        gui: &mut GuiHandle,
        view: &ReviewView,
        choice: DestinationChoice,
    ) -> GuiResult<ReviewView> {
        let preview = preview(gui, &view.workflow_id, view.revision, choice.clone())?;
        prepare(
            gui,
            &view.workflow_id,
            view.revision,
            choice,
            &preview.revision,
            false,
        )
    }
    fn wait_setup(gui: &mut GuiHandle, workflow_id: &str) -> ReviewView {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let view = poll_open(gui, workflow_id).unwrap().unwrap();
            if view.preparation.is_none() {
                return view;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn stale_selected_session_is_refused_without_fallback_or_feedback_write() {
        let (dir, mut gui, view) = review();
        let chosen = prepare_choice(&mut gui, &view, selected(&view)).unwrap();
        refresh_store(&dir, |store| {
            let f = &mut store.projects[0].features[0];
            f.sessions.clear();
            f.add_session(SessionKind::Claude);
        });
        let changed = snapshot(&mut gui).unwrap();
        assert!(changed.finish.as_ref().unwrap().handoff.is_none());
        assert!(changed.finish.as_ref().unwrap().handoff_error.is_some());
        let target = chosen.finish.unwrap().handoff.unwrap();
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                chosen.revision,
                ReviewAction::Complete {
                    check_command: None,
                    apply_suggestions: 0,
                    handoff_session: Some(target.session_id),
                    handoff_revision: Some(target.revision),
                    deliver: true
                }
            )
            .is_err()
        );
        assert!(
            !dir.path()
                .join("repo/.claude/final-review-feedback.md")
                .exists()
        );
    }
    #[test]
    fn companion_refuses_another_checked_out_branch_even_at_the_same_sha() {
        let (dir, mut gui, view) = review();
        let repo = dir.path().join("repo");
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["checkout", "-b", "other"])
                .output()
                .unwrap()
                .status
                .success()
        );
        let error = preview(
            &mut gui,
            &view.workflow_id,
            view.revision,
            companion("fixes"),
        )
        .unwrap_err();
        assert!(error.message.contains("recorded branch"));
        assert_eq!(gui.app_for_workflow().store.projects[0].features.len(), 1);
    }
    #[test]
    fn changed_preview_and_dedicated_label_harness_collision_make_no_resources() {
        let (dir, mut gui, view) = review();
        let choice = DestinationChoice::Dedicated {
            agent: AgentKind::Opencode,
        };
        let before = preview(&mut gui, &view.workflow_id, view.revision, choice.clone()).unwrap();
        refresh_store(&dir, |store| {
            store.projects[0].features[0]
                .add_session_named(SessionKind::Claude, FINAL_REVIEW_SESSION_LABEL.into());
        });
        assert!(
            prepare(
                &mut gui,
                &view.workflow_id,
                view.revision,
                choice,
                &before.revision,
                true
            )
            .unwrap_err()
            .message
            .contains("another harness")
        );
        assert_eq!(
            gui.app_for_workflow().store.projects[0].features[0]
                .sessions
                .len(),
            2
        );
    }
    #[test]
    fn resource_warning_is_explicit_and_unapproved_preparation_starts_no_worker() {
        let (_dir, mut gui, view) = review();
        gui.app_for_workflow().config.low_memory_warn_mb = u64::MAX;
        let choice = companion("gated");
        let shown = preview(&mut gui, &view.workflow_id, view.revision, choice.clone()).unwrap();
        assert!(shown.resource_warning.is_some());
        assert_eq!(
            prepare(
                &mut gui,
                &view.workflow_id,
                view.revision,
                choice,
                &shown.revision,
                false
            )
            .unwrap_err()
            .kind,
            crate::gui_contract::GuiErrorKind::NeedsApproval
        );
        assert!(gui.review_context.as_ref().unwrap().preparation.is_none());
    }
    #[test]
    fn slow_worktree_worker_does_not_block_poll_and_cancel_retains_created_ids() {
        let (dir, mut gui, view) = review();
        let checkout = dir.path().join("companion");
        // Use a real git checkout in the injected fake, never a real harness.
        let repo = dir.path().join("repo");
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Mutex::new(release_rx);
        let mut worktree = MockWorktreeOps::new();
        let captured_checkout = checkout.clone();
        worktree
            .expect_create_from()
            .times(1)
            .returning(move |_, _, branch, base| {
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                let status = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(["worktree", "add", "-b", branch])
                    .arg(&captured_checkout)
                    .arg(base)
                    .output()?;
                ensure!(status.status.success(), "git fixture worktree failed");
                Ok(captured_checkout.clone())
            });
        gui.app_for_workflow().worktree = std::sync::Arc::new(worktree);
        let started = prepare_choice(&mut gui, &view, companion("cancelled-fixes")).unwrap();
        let operation = started.preparation.unwrap();
        let time = Instant::now();
        let polling = poll_open(&mut gui, &view.workflow_id).unwrap().unwrap();
        assert!(polling.preparation.is_some());
        assert!(time.elapsed() < Duration::from_secs(1));
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                started.revision,
                ReviewAction::Pause
            )
            .is_err()
        );
        cancel_preparation(&mut gui, &operation.operation_id).unwrap();
        release_tx.send(()).unwrap();
        let done = wait_setup(&mut gui, &view.workflow_id);
        assert!(
            done.preparation_error
                .as_ref()
                .unwrap()
                .contains("cancelled")
        );
        let retained = gui
            .review_context
            .as_ref()
            .unwrap()
            .destination
            .as_ref()
            .unwrap();
        let id = retained.target.feature_id.clone();
        let app = gui.app_for_workflow();
        let (pi, fi) = app.store.locate_feature_by_id(None, &id).unwrap();
        assert_eq!(
            app.store.projects[pi].features[fi].status,
            crate::project::ProjectStatus::Stopped
        );
        assert!(checkout.exists());
        assert!(
            !dir.path()
                .join("repo/.claude/final-review-feedback.md")
                .exists()
        );
    }
    #[test]
    fn slow_confirmed_hook_allows_polling_and_stale_source_never_starts_agent() {
        for scenario in ["source_edit", "stale_view_cancel"] {
            let (dir, mut gui, view) = review();
            let checkout = dir.path().join("hook-companion");
            let repo = dir.path().join("repo");
            let gate = dir.path().join("release");
            let marker = dir.path().join("hook-ready");
            gui.app_for_workflow()
                .config
                .extension
                .lifecycle_hooks
                .on_worktree_created = Some(crate::extension::HookConfig::WithPrompt {
                script: format!(
                    "printf '%s' \"$AMF_HOOK_CHOICE\" > '{}'; n=0; while [ ! -e '{}' ] && [ $n -lt 250 ]; do n=$((n+1)); sleep 0.02; done; test -e '{}'",
                    marker.display(),
                    gate.display(),
                    gate.display()
                ),
                prompt: crate::extension::HookPrompt {
                    title: "Setup".into(),
                    options: vec!["offline".into(), "full".into()],
                },
            });
            let mut worktree = MockWorktreeOps::new();
            worktree
                .expect_create_from()
                .times(1)
                .returning(move |_, _, branch, base| {
                    let output = std::process::Command::new("git")
                        .arg("-C")
                        .arg(&repo)
                        .args(["worktree", "add", "-b", branch])
                        .arg(&checkout)
                        .arg(base)
                        .output()?;
                    ensure!(output.status.success(), "private fixture checkout failed");
                    Ok(checkout.clone())
                });
            gui.app_for_workflow().worktree = std::sync::Arc::new(worktree);
            let refused = prepare_choice(&mut gui, &view, companion("hook-fixes")).unwrap_err();
            assert!(refused.message.contains("choice"));
            assert!(gui.review_context.as_ref().unwrap().preparation.is_none());
            let mut choice = companion("hook-fixes");
            if let DestinationChoice::Companion { hook_choice, .. } = &mut choice {
                *hook_choice = Some("offline".into());
            }
            let started = prepare_choice(&mut gui, &view, choice).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !marker.exists() {
                poll_open(&mut gui, &view.workflow_id).unwrap();
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(std::fs::read_to_string(&marker).unwrap(), "offline");
            let now = Instant::now();
            assert!(
                poll_open(&mut gui, &view.workflow_id)
                    .unwrap()
                    .unwrap()
                    .preparation
                    .is_some()
            );
            assert!(now.elapsed() < Duration::from_secs(1));
            // An external settings writer invalidates the confirmed launch, even
            // though it does not affect the reviewed source patch.
            if scenario == "source_edit" {
                std::fs::write(dir.path().join("repo/amf.json"), "{\"future\":true}").unwrap();
            } else {
                // Hook-stage polling advanced the view revision; cancelling still
                // addresses this exact operation rather than unrelated state.
                let operation_id = started.preparation.as_ref().unwrap().operation_id.clone();
                assert!(gui.review_context.as_ref().unwrap().revision > started.revision);
                act(
                    &mut gui,
                    &started.workflow_id,
                    started.revision,
                    ReviewAction::CancelPreparation { operation_id },
                )
                .unwrap();
            }
            std::fs::write(gate, "").unwrap();
            let done = wait_setup(&mut gui, &started.workflow_id);
            assert!(
                done.preparation_error
                    .unwrap()
                    .contains(if scenario == "source_edit" {
                        "changed"
                    } else {
                        "cancelled"
                    })
            );
            let retained = gui
                .review_context
                .as_ref()
                .unwrap()
                .destination
                .as_ref()
                .unwrap()
                .target
                .clone();
            let app = gui.app_for_workflow();
            let (pi, fi) = app
                .store
                .locate_feature_by_id(None, &retained.feature_id)
                .unwrap();
            assert_eq!(
                app.store.projects[pi].features[fi].status,
                crate::project::ProjectStatus::Stopped
            );
        }
    }
    #[test]
    fn worker_panic_reaches_a_terminal_recoverable_review() {
        let (_dir, mut gui, view) = review();
        let mut worktree = MockWorktreeOps::new();
        worktree
            .expect_create_from()
            .times(1)
            .returning(|_, _, _, _| panic!("injected worker panic"));
        gui.app_for_workflow().worktree = std::sync::Arc::new(worktree);
        let started = prepare_choice(&mut gui, &view, companion("panic-fixes")).unwrap();
        let settled = wait_setup(&mut gui, &started.workflow_id);
        assert!(settled.preparation_error.unwrap().contains("panicked"));
        assert!(gui.review_context.as_ref().unwrap().preparation.is_none());
        assert!(
            act(
                &mut gui,
                &settled.workflow_id,
                settled.revision,
                ReviewAction::Pause
            )
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn another_feature_handoff_uses_original_feedback_file_and_never_submits() {
        let (dir, mut gui, view) = review();
        let checkout = dir.path().join("other");
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path().join("repo"))
            .args(["worktree", "add", "-b", "other"])
            .arg(&checkout)
            .arg("HEAD")
            .output()
            .unwrap();
        assert!(status.status.success());
        let app = gui.app_for_workflow();
        let mut feature = crate::project::Feature::new_for_project(
            "Demo",
            "Other".into(),
            "other".into(),
            checkout.clone(),
            true,
            VibeMode::SuperVibe,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        feature.status = crate::project::ProjectStatus::Stopped;
        let session_id = feature.add_session(SessionKind::Claude).id.clone();
        let target = SessionTarget {
            project_id: app.store.projects[0].id.clone(),
            feature_id: feature.id.clone(),
            session_id,
        };
        app.store.projects[0].features.push(feature);
        app.save().unwrap();
        app.config.final_review_submit_prompt = true;
        let selected = prepare_choice(
            &mut gui,
            &view,
            DestinationChoice::Existing {
                target: target.clone(),
            },
        )
        .unwrap();
        let shown = selected.finish.as_ref().unwrap().handoff.as_ref().unwrap();
        assert!(
            act(
                &mut gui,
                &selected.workflow_id,
                selected.revision,
                ReviewAction::Complete {
                    check_command: None,
                    apply_suggestions: 0,
                    handoff_session: Some(shown.session_id.clone()),
                    handoff_revision: Some(shown.revision.clone()),
                    deliver: true
                }
            )
            .unwrap()
            .is_none()
        );
        let done = take_completion(&mut gui, &selected.workflow_id).unwrap();
        let handoff = done.handoff.unwrap();
        assert_eq!(handoff.target.feature_id, target.feature_id);
        let draft = handoff.draft_prompt.unwrap();
        assert!(
            draft.contains(
                &dir.path()
                    .join("repo/.claude/final-review-feedback.md")
                    .display()
                    .to_string()
            )
        );
        assert!(draft.contains(&checkout.display().to_string()));
        assert!(
            dir.path()
                .join("repo/.claude/final-review-feedback.md")
                .exists()
        );
        assert!(!checkout.join(".claude/final-review-feedback.md").exists());
        assert!(gui.review_context.is_none());
    }

    #[test]
    fn mode_restrictions_apply_to_companion_primary_not_existing_secondary_agents() {
        let (_dir, mut gui, view) = review();
        let app = gui.app_for_workflow();
        app.store.projects[0].features[0].mode = VibeMode::Vibeless;
        app.save().unwrap();
        assert!(
            preview(
                &mut gui,
                &view.workflow_id,
                view.revision,
                DestinationChoice::Dedicated {
                    agent: AgentKind::Codex
                }
            )
            .is_ok()
        );
        let mut choice = companion("primary-codex");
        if let DestinationChoice::Companion { settings, .. } = &mut choice {
            settings.agent = AgentKind::Codex;
            settings.mode = VibeMode::Vibeless;
        }
        assert!(
            preview(&mut gui, &view.workflow_id, view.revision, choice)
                .unwrap_err()
                .message
                .contains("mode")
        );
    }

    #[test]
    fn reselecting_prepared_existing_target_still_checks_its_new_preview() {
        let (dir, mut gui, view) = review();
        let choice = selected(&view);
        let selected_view = prepare_choice(&mut gui, &view, choice.clone()).unwrap();
        let shown = preview(
            &mut gui,
            &selected_view.workflow_id,
            selected_view.revision,
            choice.clone(),
        )
        .unwrap();
        refresh_store(&dir, |store| {
            store.projects[0].features[0].sessions[0].label = "Externally renamed".into()
        });
        assert!(
            prepare(
                &mut gui,
                &selected_view.workflow_id,
                selected_view.revision,
                choice.clone(),
                &shown.revision,
                false
            )
            .unwrap_err()
            .message
            .contains("changed")
        );
        // An explicit new preview can accept that same stable ID's new state.
        assert!(prepare_choice(&mut gui, &selected_view, choice).is_ok());
    }
    #[test]
    fn companion_preview_uses_the_current_strict_global_hook_without_mutating_startup_settings() {
        struct DocumentGuard(Option<PathBuf>);
        impl Drop for DocumentGuard {
            fn drop(&mut self) {
                TEST_GLOBAL_DOCUMENT.with(|path| *path.borrow_mut() = self.0.take());
            }
        }
        fn config(title: &str) -> crate::app::AppConfig {
            let mut config = crate::app::AppConfig::default();
            config.extension.lifecycle_hooks.on_worktree_created =
                Some(crate::extension::HookConfig::WithPrompt {
                    script: "exit 0".into(),
                    prompt: crate::extension::HookPrompt {
                        title: title.into(),
                        options: vec!["offline".into()],
                    },
                });
            config
        }
        let (dir, mut gui, view) = review();
        gui.app_for_workflow().config.extension = config("Startup setup").extension;
        let startup = serde_json::to_value(&gui.app_for_workflow().config.extension).unwrap();
        let path = dir.path().join("private-global-config.json");
        std::fs::write(&path, serde_json::to_vec(&config("Current setup")).unwrap()).unwrap();
        let _guard =
            DocumentGuard(TEST_GLOBAL_DOCUMENT.with(|slot| slot.replace(Some(path.clone()))));
        let mut choice = companion("fresh-hook");
        if let DestinationChoice::Companion { hook_choice, .. } = &mut choice {
            *hook_choice = Some("offline".into());
        }
        let catalog = catalog(&mut gui, &view.workflow_id, view.revision).unwrap();
        assert_eq!(catalog.hook_prompt.unwrap().title, "Current setup");
        let shown = preview(&mut gui, &view.workflow_id, view.revision, choice.clone()).unwrap();
        assert_eq!(shown.hook_prompt.unwrap().title, "Current setup");
        assert_eq!(
            serde_json::to_value(&gui.app_for_workflow().config.extension).unwrap(),
            startup
        );
        std::fs::write(&path, serde_json::to_vec(&config("Updated setup")).unwrap()).unwrap();
        let error = prepare(
            &mut gui,
            &view.workflow_id,
            view.revision,
            choice.clone(),
            &shown.revision,
            false,
        )
        .unwrap_err();
        assert!(error.message.contains("changed"), "{}", error.message);
        assert!(gui.review_context.as_ref().unwrap().preparation.is_none());
        let refreshed =
            preview(&mut gui, &view.workflow_id, view.revision, choice.clone()).unwrap();
        assert_eq!(refreshed.hook_prompt.unwrap().title, "Updated setup");
        std::fs::write(&path, "{malformed global settings").unwrap();
        assert!(preview(&mut gui, &view.workflow_id, view.revision, choice).is_err());
        assert_eq!(
            serde_json::to_value(&gui.app_for_workflow().config.extension).unwrap(),
            startup
        );
    }

    #[test]
    fn malformed_pinned_base_refuses_creation_despite_a_valid_working_copy() {
        let (dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let repo = dir.path().join("repo");
        let source = dir.path().join("separate-source-checkout");
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["worktree", "add", "-b", "source-feature"])
                .arg(&source)
                .arg("HEAD")
                .output()
                .unwrap()
                .status
                .success()
        );
        std::fs::create_dir(repo.join(".claude")).unwrap();
        let repository_settings = repo.join(".claude/settings.local.json");
        std::fs::write(&repository_settings, "{\"repo_only\":true}").unwrap();
        std::fs::create_dir(source.join(".claude")).unwrap();
        let settings = source.join(".claude/settings.local.json");
        std::fs::write(&settings, "{bad base").unwrap();
        for args in [
            vec!["add", "-f", ".claude/settings.local.json"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "-m",
                "malformed base",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&source)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        std::fs::write(&settings, "{\"fixed_locally\":true}").unwrap();
        let app = gui.app_for_workflow();
        let feature = &mut app.store.projects[0].features[0];
        feature.workdir = source;
        feature.branch = "source-feature".into();
        feature.is_worktree = true;
        app.save().unwrap();
        let opened = begin(&mut gui, target).unwrap();
        let view = act(
            &mut gui,
            &opened.workflow_id,
            opened.revision,
            ReviewAction::SummaryOpen,
        )
        .unwrap()
        .unwrap();
        let error = prepare_choice(&mut gui, &view, companion("must-not-create")).unwrap_err();
        assert!(
            error.message.contains("pinned companion base"),
            "{}",
            error.message
        );
        assert!(!repo.join(".worktrees").exists());
        assert!(gui.review_context.as_ref().unwrap().preparation.is_none());
        assert_eq!(gui.app_for_workflow().store.projects[0].features.len(), 1);
        assert_eq!(
            std::fs::read_to_string(settings).unwrap(),
            "{\"fixed_locally\":true}"
        );
        assert_eq!(
            std::fs::read_to_string(repository_settings).unwrap(),
            "{\"repo_only\":true}"
        );
    }
}
