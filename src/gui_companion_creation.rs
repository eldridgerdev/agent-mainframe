//! D2 of the PR fix destinations: a NEW triage companion feature, created
//! stopped, seeded from the PR head.
//!
//! The TUI creates the companion and starts its agent in one step. The GUI keeps
//! creation and use apart, in the order Q2/Q4 decided:
//!
//! 1. [`catalog`] lists what the creation form needs (harnesses, modes, the
//!    suggested branch, the setup hook).
//! 2. [`preview_read`] / [`PreviewRead::run`] is read-only. It names the branch,
//!    the checkout path, the base (the PR head), the hook that would run, the
//!    resource warning and whether the PR branch exists on the remote. Its
//!    `revision` binds everything that creation depends on *locally*; the remote
//!    probe is a fact shown to the person and is not part of it.
//! 3. [`start`] repeats every check, refuses a stale `revision`, then hands the
//!    work to a cancellable background operation and returns at once. The
//!    operation fetches the PR head if this repository does not have it, creates
//!    the worktree on a brand-new branch, runs the previewed hook and registers
//!    the feature **stopped** through [`App::register_created_triage_companion`].
//!    It never starts an agent, sends a prompt, pushes or writes to GitHub.
//! 4. [`status`] and [`cancel`] observe and cooperatively stop the operation.
//!
//! Safety rules: a branch is never force-created or reset (a name collision with
//! a feature, a local branch or a worktree path is a refusal); the checkout, once
//! it exists, is retained on every later failure and the message names its path;
//! a failing hook keeps the companion. The GUI lock is held only for the local
//! decisions (store reads, a few quick local `git` reads in `plan`, and the
//! final registration), never across the network, the fetch, the worktree
//! creation or the hook.
//!
//! The hook is the project's own script, but it runs *inside a checkout of the
//! PR head*, so files the PR's author wrote can execute. The preview says so,
//! and when the PR branch cannot be confirmed on `origin` (a fork, or an
//! unreachable remote) creation also needs the person's explicit approval.
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::app::App;
use crate::app::triage_feature::{
    BaseSeed, FetchPolicy, TriageCompanionPlan, TriageCompanionSpec, TriagePrFacts,
};
use crate::gui_contract::{GuiError, GuiHandle, GuiResult, SessionTarget};
use crate::gui_dedicated_session::{
    DedicatedAgent, OperationStart, PrDestinationChoice, PrDestinationPrepareRequest,
    PrDestinationPrepared, PrDestinationPreview, PrDestinationRequest, PrDestinationWorkflow,
    PrIdentity, PrepareStatus, begin_operation, check_identity, conflict, current_identity, digest,
};
use crate::gui_pr_workspace::DestinationKind;
use crate::gui_review::destinations::CompanionSettings;
use crate::project::{AgentKind, Feature, Project, VibeMode, normalized_feature_name};

const FETCH_TIMEOUT: Duration = Duration::from_secs(180);
const REMOTE_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BRANCH_CHARS: usize = 100;
/// Namespace of this module's operation slot (see `begin_operation`).
const COMPANION_OPERATIONS: &str = "companion";
/// How long a fetch gets to exit after SIGTERM before it is killed.
const FETCH_TERM_GRACE: Duration = Duration::from_secs(1);
/// Finished operations kept so a late `status` still answers.
const FINISHED_KEPT: usize = 32;

// ------------------------------------------------------------------ views

/// What the creation form needs before any preview.
#[derive(Debug, Clone, Serialize)]
pub struct CompanionCatalog {
    pub identity: PrIdentity,
    pub source_name: String,
    pub source_branch: String,
    pub project_name: String,
    /// The PR's head branch name as GitHub reports it (may be empty for an old row).
    pub pr_branch: String,
    pub suggested_branch: String,
    pub agents: Vec<DedicatedAgent>,
    pub preferred: AgentKind,
    pub default_mode: VibeMode,
    pub modes: Vec<ModeChoice>,
    pub hook: Option<HookPreview>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModeChoice {
    pub mode: VibeMode,
    pub label: String,
    pub description: String,
    /// Harnesses that cannot run this mode as a feature's primary agent.
    pub unsupported_agents: Vec<AgentKind>,
}

/// The project's `on_worktree_created` hook exactly as it would run.
#[derive(Debug, Clone, Serialize)]
pub struct HookPreview {
    pub script: String,
    pub prompt_title: Option<String>,
    pub options: Vec<String>,
    /// The choice that will be passed to it, if one was given.
    pub choice: Option<String>,
    /// The hook asks a question and none was answered yet.
    pub choice_required: bool,
}

/// The companion-specific half of a preview.
#[derive(Debug, Clone, Serialize)]
pub struct CompanionPreview {
    pub branch: String,
    pub destination_path: String,
    pub source_path: String,
    pub head_sha: String,
    pub base_sha: String,
    pub base_short: String,
    pub pr_branch: String,
    /// `Some(false)`: the PR branch was not found on `origin` (for example a PR
    /// from a fork). `None`: not checked or the check failed (see the note).
    pub pr_branch_on_origin: Option<bool>,
    pub pr_branch_note: Option<String>,
    /// The PR head is not in this repository, so creation will fetch it.
    pub will_fetch: bool,
    pub agent: AgentKind,
    pub mode: VibeMode,
    pub review: bool,
    pub enable_chrome: bool,
    pub hook: Option<HookPreview>,
    /// Existing companions for this PR on an older head (left untouched).
    pub older_companions: Vec<String>,
}

// ---------------------------------------------------------------- binding

struct Context {
    opened_feature_id: String,
    identity: PrIdentity,
    head_ref: String,
    project: Project,
    source: Feature,
}

fn head_ref_of(gui: &mut GuiHandle, workflow: &PrDestinationWorkflow) -> GuiResult<String> {
    match workflow {
        PrDestinationWorkflow::Triage { workflow_id } => {
            crate::gui_pr_triage::destination_head_ref(gui, workflow_id)
        }
        PrDestinationWorkflow::Workspace { workflow_id } => {
            crate::gui_pr_workspace::destination_head_ref(gui, workflow_id)
        }
    }
}

fn bind(gui: &mut GuiHandle, workflow: &PrDestinationWorkflow) -> GuiResult<Context> {
    gui.refresh_store()?;
    let (opened, identity) = current_identity(gui, workflow)?;
    let head_ref = head_ref_of(gui, workflow)?;
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&opened.project_id), &opened.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature this PR was opened from was deleted"))?;
    let project = app.store.projects[pi].clone();
    let source = project.features[fi].clone();
    if source.pending_worktree_script {
        return Err(conflict(
            "Finish the feature's pending worktree setup before creating a companion from it",
        ));
    }
    if !project.is_git {
        return Err(conflict(
            "A triage companion needs a Git repository (it runs in its own worktree)",
        ));
    }
    Ok(Context {
        opened_feature_id: opened.feature_id,
        identity,
        head_ref,
        project,
        source,
    })
}

fn is_object_id(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    crate::gui_pr_destinations::git(dir, args)
}

/// A usable new branch name: non-empty, bounded, and accepted by Git itself.
fn validate_branch(workdir: &Path, branch: &str) -> GuiResult<()> {
    if branch.is_empty() {
        return Err(conflict("Choose a name for the companion feature"));
    }
    if branch.chars().count() > MAX_BRANCH_CHARS {
        return Err(conflict(format!(
            "Branch names are limited to {MAX_BRANCH_CHARS} characters"
        )));
    }
    if branch.chars().any(char::is_control) {
        return Err(conflict("Branch names cannot contain control characters"));
    }
    // `--branch` also expands things like `@{-1}` to a different name, so the
    // output must be the input verbatim.
    match git(workdir, &["check-ref-format", "--branch", branch]) {
        // A lone `@` passes the check but is not a usable branch name.
        Ok(normalized) if normalized.trim() == branch && branch != "@" => Ok(()),
        _ => Err(conflict(format!(
            "'{branch}' is not a valid Git branch name; choose another"
        ))),
    }
}

fn hook_of(app: &App, project: &Project) -> Option<crate::extension::HookConfig> {
    crate::extension::merge_project_extension_config(&app.config.extension, &project.repo)
        .lifecycle_hooks
        .on_worktree_created
}

fn hook_preview(
    hook: Option<&crate::extension::HookConfig>,
    choice: Option<&str>,
) -> Option<HookPreview> {
    hook.map(|hook| {
        let prompt = hook.prompt();
        HookPreview {
            script: hook.script().to_string(),
            prompt_title: prompt.map(|p| p.title.clone()),
            options: prompt.map(|p| p.options.clone()).unwrap_or_default(),
            choice: choice.map(str::to_string),
            choice_required: prompt.is_some() && choice.is_none(),
        }
    })
}

fn check_hook_choice(
    hook: Option<&crate::extension::HookConfig>,
    choice: Option<&str>,
) -> GuiResult<()> {
    match (hook.and_then(|h| h.prompt()), choice) {
        (Some(prompt), Some(choice)) if !prompt.options.iter().any(|o| o == choice) => {
            Err(conflict(format!(
                "'{choice}' is not one of the setup script's options; choose again"
            )))
        }
        (None, Some(_)) => Err(conflict(
            "The setup script no longer asks a question; reopen the form",
        )),
        _ => Ok(()),
    }
}

fn agent_unavailable(gui: &mut GuiHandle, repo: &Path, agent: &AgentKind) -> Option<String> {
    let app = gui.app_for_workflow();
    if !app.allowed_agents_for_repo(repo).contains(agent) {
        return Some("This agent is not allowed or installed for the project".into());
    }
    app.tmux
        .check_harness_available(agent)
        .err()
        .map(|error| error.to_string())
}

fn destination_path(plan: &TriageCompanionPlan) -> PathBuf {
    plan.repo.join(".worktrees").join(&plan.worktree_name)
}

/// `<pr-branch>-triage`, then `-triage-2`, `-triage-3`, ... skipping any name a
/// feature, a local branch or a worktree path already uses (the TUI's naming,
/// plus the path check creation itself enforces).
fn suggest_branch(app: &App, ctx: &Context) -> String {
    let stem = if ctx.head_ref.is_empty() {
        ctx.source.branch.as_str()
    } else {
        ctx.head_ref.as_str()
    };
    let base = format!("{stem}-triage");
    let taken = |candidate: &str| {
        let normalized = normalized_feature_name(candidate);
        ctx.project
            .features
            .iter()
            .any(|f| normalized_feature_name(&f.name) == normalized)
            || crate::app::triage_feature::branch_exists(&ctx.source.workdir, candidate)
            || ctx
                .project
                .repo
                .join(".worktrees")
                .join(crate::project::worktree_name(&ctx.project.name, candidate))
                .exists()
    };
    let _ = app;
    if !taken(&base) {
        return base;
    }
    (2..100)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or(base)
}

pub fn catalog(
    gui: &mut GuiHandle,
    workflow: &PrDestinationWorkflow,
) -> GuiResult<CompanionCatalog> {
    let ctx = bind(gui, workflow)?;
    let allowed = gui
        .app_for_workflow()
        .allowed_agents_for_repo(&ctx.project.repo);
    let agents = allowed
        .into_iter()
        .map(|agent| DedicatedAgent {
            label: agent.display_name().into(),
            unavailable: agent_unavailable(gui, &ctx.project.repo, &agent),
            agent,
        })
        .collect();
    let app = gui.app_for_workflow();
    let hook = hook_of(app, &ctx.project);
    let modes = [VibeMode::Vibeless, VibeMode::Vibe, VibeMode::SuperVibe]
        .into_iter()
        .map(|mode| ModeChoice {
            label: mode.display_name().into(),
            description: mode.permission_summary().into(),
            unsupported_agents: [
                AgentKind::Claude,
                AgentKind::Opencode,
                AgentKind::Codex,
                AgentKind::Pi,
            ]
            .into_iter()
            .filter(|agent| !mode.supports_agent(agent))
            .collect(),
            mode,
        })
        .collect();
    let suggested_branch = suggest_branch(app, &ctx);
    Ok(CompanionCatalog {
        identity: ctx.identity,
        source_name: ctx.source.name,
        source_branch: ctx.source.branch,
        project_name: ctx.project.name,
        pr_branch: ctx.head_ref,
        suggested_branch,
        agents,
        preferred: ctx.project.preferred_agent,
        default_mode: VibeMode::default(),
        modes,
        hook: hook_preview(hook.as_ref(), None),
    })
}

// ------------------------------------------------------------------- plan

struct Plan {
    ctx: Context,
    triage: TriageCompanionPlan,
    needs_fetch: bool,
    revision: String,
    resource_warning: Option<String>,
    preview: PrDestinationPreview,
    probe_head_ref: String,
}

/// Every local decision. Strict (`for_prepare`) additionally requires the hook's
/// choice. Runs under the GUI lock; nothing here changes the repository.
fn plan(
    gui: &mut GuiHandle,
    workflow: &PrDestinationWorkflow,
    identity: &PrIdentity,
    settings: &CompanionSettings,
    hook_choice: Option<&str>,
    for_prepare: bool,
) -> GuiResult<Plan> {
    let ctx = bind(gui, workflow)?;
    check_identity(&ctx.identity, identity)?;
    if !is_object_id(&ctx.identity.head_sha) {
        return Err(conflict(
            "The PR head is not a full commit ID; refresh the PR and reopen this form",
        ));
    }
    let branch = settings.branch.trim().to_string();
    // The trimmed form is what is hashed, planned and created.
    let trimmed = CompanionSettings {
        branch: branch.clone(),
        ..settings.clone()
    };
    let settings = &trimmed;
    // Establish the checkout is a readable work tree before any Git fact is
    // trusted: the planning helpers treat a Git error as "not found".
    git(&ctx.source.workdir, &["rev-parse", "--is-inside-work-tree"]).map_err(|_| {
        conflict("The feature's checkout is missing or is not a readable Git work tree")
    })?;
    validate_branch(&ctx.source.workdir, &branch)?;
    if let Some(reason) = agent_unavailable(gui, &ctx.project.repo, &settings.agent) {
        return Err(conflict(reason));
    }
    let app = gui.app_for_workflow();
    let hook = hook_of(app, &ctx.project);
    check_hook_choice(hook.as_ref(), hook_choice)?;
    let facts = TriagePrFacts {
        pr_number: ctx.identity.pr_number,
        // Lowercase so the planned base (and the revision digested from it) is
        // the same before and after the fetch, whatever case GitHub reported.
        head_sha: ctx.identity.head_sha.to_ascii_lowercase(),
        head_ref: ctx.head_ref.clone(),
    };
    let triage = app
        .plan_triage_companion_seeded(
            &ctx.opened_feature_id,
            &facts,
            &TriageCompanionSpec {
                branch: branch.clone(),
                agent: settings.agent.clone(),
                mode: settings.mode.clone(),
                review: settings.review,
                enable_chrome: settings.enable_chrome,
            },
            hook.as_ref(),
            hook_choice,
            for_prepare,
            FetchPolicy::Never,
            BaseSeed::PrHead,
        )
        .map_err(|error| conflict(error.to_string()))?;
    if triage.local_branch_exists {
        return Err(conflict(format!(
            "A local branch named '{branch}' already exists. AMF never resets or reuses an existing branch; choose another name"
        )));
    }
    let dest = destination_path(&triage);
    if dest.exists() {
        return Err(conflict(format!(
            "The checkout path {} already exists; choose another name",
            dest.display()
        )));
    }
    let head = ctx.identity.head_sha.to_ascii_lowercase();
    let mut older = Vec::new();
    for feature in &ctx.project.features {
        let Some(link) = feature.triage_source.as_ref().filter(|link| {
            link.pr_number == ctx.identity.pr_number
                && link.source_feature_id == ctx.opened_feature_id
        }) else {
            continue;
        };
        if link.base_sha.eq_ignore_ascii_case(&head) {
            return Err(conflict(format!(
                "A triage companion for this PR head already exists: '{}'. Choose it from the destination list instead of creating another",
                feature.name
            )));
        }
        older.push(feature.name.clone());
    }
    if feature_has_both_links(&ctx.source) {
        return Err(conflict(
            "This feature is linked as both a review and a triage companion; resolve that before creating another companion",
        ));
    }
    let needs_fetch = !triage.base_commit_present;
    let resource_warning = gui
        .require_start_approval(&format!("Creating '{branch}'"))
        .err()
        .map(|error| error.message);
    let app = gui.app_for_workflow();
    let hook_view = hook_preview(triage.hook.as_ref(), hook_choice);
    // The revision binds what creation depends on locally. `needs_fetch` and the
    // remote probe are deliberately outside it: fetching the head is the step
    // that turns the first into the second's opposite.
    let revision = digest(&(
        workflow.key(),
        &ctx.identity,
        settings,
        hook_choice,
        &triage.hook,
        &ctx.source.id,
        &ctx.source.workdir,
        crate::gui_pr_workspace::checkout_identity(&ctx.source.workdir),
        &ctx.source.branch,
        &triage.repo,
        &triage.base_sha,
        &triage.pr_branch,
        &triage.worktree_name,
        &dest,
        &older,
    ));
    let _ = app;
    let mut consequences = Vec::new();
    consequences.push(format!(
        "Creates a new Git worktree at {} on a new branch '{branch}', started from PR #{} head {}. The branch is new: no existing branch is reset or reused.",
        dest.display(),
        ctx.identity.pr_number,
        short(&triage.base_sha)
    ));
    if needs_fetch {
        consequences.push(format!(
            "The PR head is not in this repository yet, so creation first fetches it from the remote (git fetch origin {}). That only reads from the remote, and you can cancel while it runs.",
            short(&triage.base_sha)
        ));
    }
    if let Some(hook) = &hook_view {
        consequences.push(match (&hook.choice, &hook.prompt_title) {
            (Some(choice), Some(title)) => format!(
                "Runs the project's setup script (on_worktree_created) once in the new checkout, with {title}: {choice}. Script: {}",
                hook.script
            ),
            _ => format!(
                "Runs the project's setup script (on_worktree_created) once in the new checkout. Script: {}",
                hook.script
            ),
        });
        consequences.push(
            "The setup script executes files from the PR checkout, which is code the PR's author wrote (it may be a fork). Only continue if you trust this PR's contents."
                .into(),
        );
        consequences.push(
            "If the setup script fails, the companion is still kept and no agent is started."
                .into(),
        );
    }
    consequences.push(format!(
        "Registers a feature '{branch}' ({}, {}) linked to PR #{} and to {}. Plan mode is off.",
        settings.agent.display_name(),
        settings.mode.display_name(),
        ctx.identity.pr_number,
        ctx.source.name
    ));
    consequences.push(
        "The agent is stopped: nothing runs and no prompt is sent. Start it from its session tab when you are ready."
            .into(),
    );
    consequences.push(format!(
        "Nothing is pushed and nothing is posted to GitHub. Fixes only reach the PR branch '{}' (on the remote) if you later use Integrate, which asks separately.",
        triage.pr_branch
    ));
    consequences.push(
        "If anything fails after the checkout exists, the checkout is kept and its path is shown; AMF never deletes it automatically."
            .into(),
    );
    if !older.is_empty() {
        consequences.push(format!(
            "Existing companion(s) for an older PR head stay as they are: {}.",
            older.join(", ")
        ));
    }
    consequences.push(format!(
        "Bound to PR #{} at head {}, as last loaded by AMF; commits pushed since are only noticed after you refresh.",
        ctx.identity.pr_number,
        short(&ctx.identity.head_sha)
    ));
    let preview = PrDestinationPreview {
        revision: revision.clone(),
        identity: ctx.identity.clone(),
        label: format!(
            "New triage feature {branch} ({}, {})",
            settings.agent.display_name(),
            settings.mode.display_name()
        ),
        agent: settings.agent.clone(),
        feature_name: ctx.source.name.clone(),
        workdir: dest.display().to_string(),
        reuses_existing: false,
        existing_target: None,
        creates_session: true,
        stopped: true,
        resource_warning: resource_warning.clone(),
        consequences,
        companion: Some(CompanionPreview {
            branch: branch.clone(),
            destination_path: dest.display().to_string(),
            source_path: ctx.source.workdir.display().to_string(),
            head_sha: ctx.identity.head_sha.clone(),
            base_sha: triage.base_sha.clone(),
            base_short: short(&triage.base_sha),
            pr_branch: triage.pr_branch.clone(),
            pr_branch_on_origin: None,
            pr_branch_note: Some("Not checked yet".into()),
            will_fetch: needs_fetch,
            agent: settings.agent.clone(),
            mode: settings.mode.clone(),
            review: settings.review,
            enable_chrome: settings.enable_chrome,
            hook: hook_view,
            older_companions: older,
        }),
    };
    Ok(Plan {
        probe_head_ref: ctx.head_ref.clone(),
        ctx,
        triage,
        needs_fetch,
        revision,
        resource_warning,
        preview,
    })
}

fn feature_has_both_links(feature: &Feature) -> bool {
    feature.triage_source.is_some() && feature.review_source.is_some()
}

// ---------------------------------------------------------------- preview

/// The locked half of a preview. [`PreviewRead::run`] adds the remote probe and
/// must be called without the GUI lock.
pub struct PreviewRead {
    preview: PrDestinationPreview,
    workdir: PathBuf,
    head_ref: String,
    head_sha: String,
}

pub fn preview_read(gui: &mut GuiHandle, request: &PrDestinationRequest) -> GuiResult<PreviewRead> {
    let PrDestinationChoice::CompanionFeature {
        settings,
        hook_choice,
    } = &request.choice
    else {
        return Err(conflict("Not a companion feature choice"));
    };
    let plan = plan(
        gui,
        &request.workflow,
        &request.identity,
        settings,
        hook_choice.as_deref(),
        false,
    )?;
    Ok(PreviewRead {
        workdir: plan.ctx.source.workdir.clone(),
        head_ref: plan.probe_head_ref,
        head_sha: plan.ctx.identity.head_sha,
        preview: plan.preview,
    })
}

impl PreviewRead {
    /// Whether the PR branch exists on `origin`. A read of the remote only, with
    /// a timeout; any failure reads as "unknown", never as present or absent.
    pub fn run(mut self) -> PrDestinationPreview {
        let (found, note) = probe_remote_branch(&self.workdir, &self.head_ref, &self.head_sha);
        if let Some(companion) = self.preview.companion.as_mut() {
            companion.pr_branch_on_origin = found;
            companion.pr_branch_note = note;
        }
        self.preview
    }
}

/// The approval text when a hook would run files from a PR whose branch is not
/// confirmed on `origin`.
fn hook_ack_message(found: Option<bool>, head_ref: &str) -> String {
    let why = match found {
        Some(false) => {
            format!("the PR branch '{head_ref}' is not on the remote 'origin' (for example a fork)")
        }
        _ => "AMF could not confirm the PR branch on the remote 'origin'".to_string(),
    };
    format!(
        "The setup script would run files from this PR's checkout, and {why}. That code is not from your repository. Run it anyway?"
    )
}

fn probe_remote_branch(
    workdir: &Path,
    head_ref: &str,
    head_sha: &str,
) -> (Option<bool>, Option<String>) {
    if head_ref.is_empty() {
        return (
            None,
            Some("GitHub did not report the PR's branch name, so it cannot be looked up on the remote.".into()),
        );
    }
    let wanted = format!("refs/heads/{head_ref}");
    let output = crate::gui_pr_destinations::git_limited(
        workdir,
        &["ls-remote", "--heads", "origin", &wanted],
        REMOTE_PROBE_TIMEOUT,
        65_536,
    );
    match output {
        Err(error) => (
            None,
            Some(format!(
                "Could not check whether the PR branch '{head_ref}' exists on the remote ({error}). Creating the companion does not need it; integrating later does."
            )),
        ),
        Ok(text) => {
            let tip = text.lines().find_map(|line| {
                let (sha, name) = line.split_once('\t')?;
                (name == wanted).then(|| sha.to_string())
            });
            match tip {
                None => (
                    Some(false),
                    Some(format!(
                        "The PR branch '{head_ref}' was not found on the remote 'origin' (for example the PR comes from a fork). A companion can still be created, but Integrate will not be able to push its fixes to the PR branch."
                    )),
                ),
                Some(tip) if tip.eq_ignore_ascii_case(head_sha) => (Some(true), None),
                Some(_) => (
                    Some(true),
                    Some(format!(
                        "The PR branch '{head_ref}' on the remote has moved past this PR head; refresh the PR if you want the newer commits."
                    )),
                ),
            }
        }
    }
}

// ---------------------------------------------------------------- operation

/// How the background operation reaches the GUI state. The application holds
/// the real handle in a `Mutex`; tests do too.
pub trait GuiLocker: Send + Sync + 'static {
    fn lock_gui(&self) -> MutexGuard<'_, GuiHandle>;
}

impl GuiLocker for Mutex<GuiHandle> {
    fn lock_gui(&self) -> MutexGuard<'_, GuiHandle> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Fetches `sha` from `origin` into `workdir`'s repository, stopping promptly
/// once `cancel` is set. Injected so tests can stand in for a slow remote.
pub type Fetcher = Arc<dyn Fn(&Path, &str, &AtomicBool) -> Result<(), String> + Send + Sync>;

pub fn git_fetcher() -> Fetcher {
    Arc::new(fetch_head)
}

fn fetch_head(workdir: &Path, sha: &str, cancel: &AtomicBool) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut stderr = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(workdir)
        .args(["fetch", "--quiet", "origin", sha])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr.try_clone().map_err(|e| e.to_string())?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start Git".to_string())?;
    let started = Instant::now();
    // SIGTERM the group first so Git can drop its lock files, then SIGKILL
    // whatever is left once the grace period is over.
    let stop = |child: &mut std::process::Child| {
        let pgid = child.id() as i32;
        #[cfg(unix)]
        // SAFETY: signalling the process group this function created.
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
        let deadline = Instant::now() + FETCH_TERM_GRACE;
        while Instant::now() < deadline && !matches!(child.try_wait(), Ok(Some(_))) {
            std::thread::sleep(Duration::from_millis(20));
        }
        #[cfg(unix)]
        // SAFETY: as above; a group that already exited returns ESRCH.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => return Err(error.to_string()),
        }
        if cancel.load(Ordering::SeqCst) {
            stop(&mut child);
            return Err("cancelled".into());
        }
        if started.elapsed() > FETCH_TIMEOUT {
            stop(&mut child);
            return Err("the remote did not answer in time".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if status.success() {
        return Ok(());
    }
    let mut text = String::new();
    let _ = stderr.seek(SeekFrom::Start(0));
    let _ = stderr.take(2048).read_to_string(&mut text);
    let text = text.trim();
    Err(if text.is_empty() {
        "git fetch failed".into()
    } else {
        text.lines().last().unwrap_or(text).to_string()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Running,
    Created,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPhase {
    Starting,
    Fetching,
    CreatingWorktree,
    RunningHook,
    Registering,
    Done,
}

/// What `pr_destination_status` and `pr_destination_cancel` return.
#[derive(Debug, Clone, Serialize)]
pub struct PrDestinationOperation {
    pub operation_id: String,
    pub state: OperationState,
    pub phase: OperationPhase,
    pub cancel_requested: bool,
    pub message: String,
    /// The checkout, once it exists. Kept whatever happens next.
    pub retained_path: Option<String>,
    pub created_target: Option<SessionTarget>,
    pub destination_revision: Option<String>,
    pub destination_kind: DestinationKind,
    pub stopped: bool,
    pub hook_warning: Option<String>,
}

struct Inner {
    state: OperationState,
    phase: OperationPhase,
    message: String,
    retained_path: Option<String>,
    created_target: Option<SessionTarget>,
    destination_revision: Option<String>,
    hook_warning: Option<String>,
}

struct OpState {
    id: String,
    cancel: AtomicBool,
    inner: Mutex<Inner>,
}

impl OpState {
    fn new(id: String) -> Self {
        Self {
            id,
            cancel: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                state: OperationState::Running,
                phase: OperationPhase::Starting,
                message: "Starting…".into(),
                retained_path: None,
                created_target: None,
                destination_revision: None,
                hook_warning: None,
            }),
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn phase(&self, phase: OperationPhase, message: impl Into<String>) {
        let mut inner = self.inner();
        inner.phase = phase;
        inner.message = message.into();
    }

    fn finish(&self, state: OperationState, message: impl Into<String>) {
        let mut inner = self.inner();
        inner.state = state;
        inner.phase = OperationPhase::Done;
        inner.message = message.into();
    }

    fn view(&self) -> PrDestinationOperation {
        let inner = self.inner();
        PrDestinationOperation {
            operation_id: self.id.clone(),
            state: inner.state,
            phase: inner.phase,
            cancel_requested: self.cancelled(),
            message: inner.message.clone(),
            retained_path: inner.retained_path.clone(),
            created_target: inner.created_target.clone(),
            destination_revision: inner.destination_revision.clone(),
            destination_kind: DestinationKind::Companion,
            stopped: true,
            hook_warning: inner.hook_warning.clone(),
        }
    }
}

#[derive(Default)]
struct Registry {
    by_id: HashMap<String, Arc<OpState>>,
    order: VecDeque<String>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

fn track(state: &Arc<OpState>) {
    let mut registry = registry().lock().unwrap_or_else(PoisonError::into_inner);
    registry.by_id.insert(state.id.clone(), state.clone());
    registry.order.push_back(state.id.clone());
    // Drop the oldest *finished* entries beyond the cap; a running one stays.
    let mut index = 0;
    while registry.order.len() > FINISHED_KEPT && index < registry.order.len() {
        let id = registry.order[index].clone();
        let finished = registry
            .by_id
            .get(&id)
            .is_none_or(|op| op.inner().state != OperationState::Running);
        if finished {
            registry.order.remove(index);
            registry.by_id.remove(&id);
        } else {
            index += 1;
        }
    }
}

fn lookup(operation_id: &str) -> GuiResult<Arc<OpState>> {
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .by_id
        .get(operation_id)
        .cloned()
        .ok_or_else(|| {
            GuiError::not_found(
                "That companion operation is no longer tracked. Check the feature list for what was created",
            )
        })
}

/// Read-only; never takes the GUI lock, so it answers while the operation holds
/// or waits for it.
pub fn status(operation_id: &str) -> GuiResult<PrDestinationOperation> {
    lookup(operation_id).map(|op| op.view())
}

/// Ask a running operation to stop. Idempotent. The fetch and the hook are
/// stopped promptly; a checkout that already exists is kept and registered so it
/// stays visible and recoverable.
pub fn cancel(operation_id: &str) -> GuiResult<PrDestinationOperation> {
    let op = lookup(operation_id)?;
    if op.inner().state == OperationState::Running {
        op.cancel.store(true, Ordering::SeqCst);
    }
    Ok(op.view())
}

/// Start creating the companion in the background. Returns at once.
pub fn start(
    locker: Arc<dyn GuiLocker>,
    request: PrDestinationPrepareRequest,
    on_finish: Arc<dyn Fn() + Send + Sync>,
) -> GuiResult<PrDestinationPrepared> {
    start_with(locker, request, git_fetcher(), on_finish)
}

pub(crate) fn start_with(
    locker: Arc<dyn GuiLocker>,
    request: PrDestinationPrepareRequest,
    fetcher: Fetcher,
    on_finish: Arc<dyn Fn() + Send + Sync>,
) -> GuiResult<PrDestinationPrepared> {
    let PrDestinationChoice::CompanionFeature {
        settings,
        hook_choice,
    } = request.choice.clone()
    else {
        return Err(conflict("Not a companion feature choice"));
    };
    // One destination operation per workflow. A duplicate submit gets the
    // running one back; the slot is released when the worker drops the guard.
    let guard = match begin_operation(&request.workflow, COMPANION_OPERATIONS) {
        OperationStart::Existing(mut running) => {
            running.message =
                "This companion is already being created. Waiting for that operation.".into();
            running.destination_kind = DestinationKind::Companion;
            return Ok(running);
        }
        OperationStart::Started(guard) => guard,
    };
    let (planned, worktree) = {
        let mut gui = locker.lock_gui();
        let planned = plan(
            &mut gui,
            &request.workflow,
            &request.identity,
            &settings,
            hook_choice.as_deref(),
            true,
        )?;
        if planned.revision != request.preview_revision {
            return Err(conflict(
                "The feature, PR, branch names or setup script changed since the preview. Review the preview again before creating the companion.",
            ));
        }
        if planned.resource_warning.is_some() && !request.approved {
            gui.require_start_approval(&format!("Creating '{}'", settings.branch.trim()))?;
        }
        (planned, gui.app_for_workflow().worktree.clone())
    };
    // A hook executes files from the PR checkout. Without a confirmed `origin`
    // branch (fork PR, unreachable remote) that needs explicit approval, folded
    // into `approved`. The probe is a bounded read of the remote, made without
    // the GUI lock.
    if planned.triage.hook.is_some() && !request.approved {
        let (found, _) = probe_remote_branch(
            &planned.ctx.source.workdir,
            &planned.probe_head_ref,
            &planned.ctx.identity.head_sha,
        );
        if found != Some(true) {
            return Err(GuiError::needs_approval(hook_ack_message(
                found,
                &planned.probe_head_ref,
            )));
        }
    }
    let state = Arc::new(OpState::new(guard.operation_id.clone()));
    track(&state);
    let operation_id = state.id.clone();
    let branch = planned.triage.spec.branch.clone();
    let job = Job {
        locker,
        state: state.clone(),
        fetcher,
        worktree,
        request,
        settings,
        hook_choice,
        plan: planned,
        on_finish: on_finish.clone(),
    };
    let panic_state = state.clone();
    let panic_finish = on_finish.clone();
    std::thread::Builder::new()
        .name("pr-companion-create".into())
        .spawn(move || {
            let _guard = guard;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.run()));
            if outcome.is_err() {
                let retained = panic_state.inner().retained_path.clone();
                panic_state.finish(
                    OperationState::Failed,
                    match retained {
                        Some(path) => format!(
                            "Companion creation stopped unexpectedly. The checkout at {path} was kept; it may not be registered as a feature"
                        ),
                        None => "Companion creation stopped unexpectedly. Check the worktree list for anything it left behind".into(),
                    },
                );
                // The job's own on_finish never ran; tell the UI anyway.
                panic_finish();
            }
        })
        .map_err(|error| {
            state.finish(OperationState::Failed, "Could not start the worker thread");
            GuiError::from(anyhow::Error::from(error))
        })?;
    Ok(PrDestinationPrepared {
        status: PrepareStatus::Started,
        operation_id,
        created_target: None,
        destination_revision: None,
        destination_kind: DestinationKind::Companion,
        stopped: true,
        message: format!(
            "Creating '{branch}' in the background. Nothing is started; you can cancel while it runs."
        ),
    })
}

struct Job {
    locker: Arc<dyn GuiLocker>,
    state: Arc<OpState>,
    fetcher: Fetcher,
    worktree: Arc<dyn crate::traits::WorktreeOps>,
    request: PrDestinationPrepareRequest,
    settings: CompanionSettings,
    hook_choice: Option<String>,
    plan: Plan,
    on_finish: Arc<dyn Fn() + Send + Sync>,
}

impl Job {
    fn run(self) {
        let on_finish = self.on_finish.clone();
        self.execute();
        on_finish();
    }

    fn fail(&self, message: impl Into<String>) {
        self.state.finish(OperationState::Failed, message);
    }

    fn execute(self) {
        let state = self.state.clone();
        let mut triage = self.plan.triage.clone();
        let branch = triage.spec.branch.clone();
        let dest = destination_path(&triage);

        if self.plan.needs_fetch {
            if state.cancelled() {
                return state.finish(
                    OperationState::Cancelled,
                    "Cancelled before anything was created. Nothing was created.",
                );
            }
            state.phase(
                OperationPhase::Fetching,
                format!(
                    "Fetching PR head {} from the remote…",
                    short(&triage.base_sha)
                ),
            );
            let fetched = (self.fetcher)(&triage.source_workdir, &triage.base_sha, &state.cancel);
            if state.cancelled() {
                return state.finish(
                    OperationState::Cancelled,
                    "Cancelled while fetching the PR head. Nothing was created.",
                );
            }
            if let Err(error) = fetched {
                return self.fail(format!(
                    "Could not fetch PR head {} from the remote: {error}. Nothing was created.",
                    short(&triage.base_sha)
                ));
            }
            // Fetching can take a while: plan again against the live state and
            // refuse anything that moved, and require the commit to be present.
            let again = {
                let mut gui = self.locker.lock_gui();
                plan(
                    &mut gui,
                    &self.request.workflow,
                    &self.request.identity,
                    &self.settings,
                    self.hook_choice.as_deref(),
                    true,
                )
            };
            let again = match again {
                Ok(again) => again,
                Err(error) => {
                    return self.fail(format!(
                        "{} Nothing was created.",
                        ensure_period(&error.message)
                    ));
                }
            };
            if again.revision != self.request.preview_revision {
                return self.fail(
                    "The feature, PR, branch names or setup script changed while the PR head was being fetched. Nothing was created. Review the preview again.",
                );
            }
            if !again.triage.base_commit_present {
                return self.fail(format!(
                    "The PR head {} is still not in this repository after fetching, so it cannot be used as a base. Nothing was created.",
                    short(&triage.base_sha)
                ));
            }
            triage = again.triage;
        } else if !triage.base_commit_present {
            return self.fail("The PR head commit is not in this repository. Nothing was created.");
        }

        if state.cancelled() {
            return state.finish(
                OperationState::Cancelled,
                "Cancelled before the worktree was created. Nothing was created.",
            );
        }
        state.phase(
            OperationPhase::CreatingWorktree,
            format!("Creating the worktree for '{branch}'…"),
        );
        let workdir = match self.worktree.create_from(
            &triage.repo,
            &triage.worktree_name,
            &triage.spec.branch,
            &triage.base,
        ) {
            Ok(workdir) => workdir,
            Err(error) => {
                let leftover = if dest.exists() {
                    format!(
                        " Something may have been left at {}; AMF did not remove it",
                        dest.display()
                    )
                } else {
                    String::new()
                };
                return self.fail(format!(
                    "Could not create the worktree: {error}.{leftover} No feature was registered."
                ));
            }
        };
        state.inner().retained_path = Some(workdir.display().to_string());

        let mut hook_warning = None;
        if let Some(hook) = triage.hook.as_ref()
            && !state.cancelled()
        {
            state.phase(
                OperationPhase::RunningHook,
                "Running the setup script in the new checkout…",
            );
            let (ok, detail) = App::run_worktree_hook_cancellable(
                hook.script(),
                &workdir,
                triage.hook_choice.as_deref(),
                Some(&state.cancel),
            );
            if !ok {
                hook_warning = Some(detail.unwrap_or_else(|| "unknown error".into()));
            }
        }

        state.phase(
            OperationPhase::Registering,
            format!("Registering '{branch}' as a stopped feature…"),
        );
        let registered = {
            let mut gui = self.locker.lock_gui();
            match gui.refresh_store() {
                Err(error) => Err(format!(
                    "{}. Triage checkout retained at {}",
                    error.message,
                    workdir.display()
                )),
                Ok(()) => {
                    let app = gui.app_for_workflow();
                    if let Some(name) = existing_companion_for_head(app, &triage) {
                        Err(format!(
                            "A triage companion for this PR head ('{name}') was created while this one was being set up"
                        ))
                    } else {
                        app.register_created_triage_companion(&triage, workdir.clone())
                            .map(|created| {
                                let revision = app
                                    .store
                                    .locate_feature_by_id(
                                        Some(&created.project_id),
                                        &created.feature_id,
                                    )
                                    .and_then(|(pi, fi)| {
                                        let feature = &app.store.projects[pi].features[fi];
                                        feature
                                            .sessions
                                            .iter()
                                            .find(|s| s.id == created.session_id)
                                            .map(|s| {
                                                crate::gui_pr_workspace::destination_revision(
                                                    feature, s,
                                                )
                                            })
                                    });
                                (created, revision)
                            })
                            .map_err(|error| error.to_string())
                    }
                }
            }
        };
        let (created, revision) = match registered {
            Ok(ok) => ok,
            Err(message) => {
                return self.fail(format!(
                    "{} No feature was registered; the checkout was kept for you to inspect or remove.",
                    ensure_period(&message)
                ));
            }
        };
        let target = SessionTarget {
            project_id: created.project_id.clone(),
            feature_id: created.feature_id.clone(),
            session_id: created.session_id.clone(),
        };
        {
            let mut inner = state.inner();
            inner.created_target = Some(target);
            inner.destination_revision = revision;
            inner.hook_warning = hook_warning.clone();
        }
        let path = workdir.display();
        if state.cancelled() {
            return state.finish(
                OperationState::Cancelled,
                format!(
                    "Cancelled after the worktree was created. The stopped feature '{}' was kept at {path}; no agent was started.",
                    created.name
                ),
            );
        }
        let mut message = format!(
            "Created '{}' ({}, {}) at {path}, stopped. Nothing is running. Start it from its session tab when you are ready.",
            created.name,
            self.settings.agent.display_name(),
            self.settings.mode.display_name()
        );
        if let Some(warning) = &hook_warning {
            message.push_str(&format!(
                " Its setup script failed ({warning}); the companion was kept and no agent was started."
            ));
        }
        state.finish(OperationState::Created, message);
    }
}

/// A companion already registered for the same (PR, source feature, base
/// commit). `plan` checks this too, but two operations on different workflows
/// can both pass `plan` before either registers, so the registration repeats it
/// under the same lock hold that registers.
fn existing_companion_for_head(app: &App, triage: &TriageCompanionPlan) -> Option<String> {
    let (pi, _) = app
        .store
        .locate_feature_by_id(Some(&triage.project_id), &triage.source_feature_id)?;
    app.store.projects[pi]
        .features
        .iter()
        .find(|feature| {
            feature.triage_source.as_ref().is_some_and(|link| {
                link.pr_number == triage.pr_number
                    && link.source_feature_id == triage.source_feature_id
                    && link.base_sha.eq_ignore_ascii_case(&triage.base_sha)
            })
        })
        .map(|feature| feature.name.clone())
}

fn ensure_period(message: &str) -> String {
    let message = message.trim_end();
    if message.ends_with(['.', '!', '?']) {
        message.to_string()
    } else {
        format!("{message}.")
    }
}

#[cfg(test)]
mod tests;
