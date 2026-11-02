//! The **companion triage feature**: PR Triage's `New feature…` fix target.
//!
//! The two in-feature fix targets (`ExistingLive`, `DedicatedReview`) both run
//! the triage agent inside the feature the PR was implemented in. That feature
//! carries its own vibe mode and launch flags, and the triage agent writes into
//! the same worktree — the same `.claude/settings.local.json`, the same
//! permissions. If the PR was built in SuperVibe and you want review fixes
//! applied under Vibeless supervision, there was no way to get it.
//!
//! This module adds a third target: an isolated, worktree-backed AMF feature
//! created on demand for one PR, with its harness, vibe mode, and other
//! settings chosen independently of the source feature (optionally from a
//! configured feature preset).
//!
//! Two constraints shape the design:
//!
//! 1. **Git can't check out one branch in two worktrees.** The companion sits
//!    on its own branch, seeded from the PR head. Branch-based PR
//!    auto-detection therefore can't find its way back, so the link to the PR
//!    and the source feature is persisted explicitly on the feature
//!    ([`crate::project::TriageSource`]) and is what
//!    `App::triage_feature_indices` matches on.
//! 2. **Fixes still have to land on the PR.** Because the work happens off the
//!    PR branch, integration is an explicit, visible step (`I`) with two
//!    non-destructive options — push the triage branch onto the PR branch, or
//!    cherry-pick into the source worktree — and the cherry-pick is refused
//!    outright while the source worktree is dirty.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use crate::app::pr_review::{FixTarget, TRIAGE_SESSION_LABEL};
use crate::app::state::{
    AppMode, TriageFeatureSetupState, TriageIntegrateState, TriageIntegration,
};
use crate::app::{App, StartIntent};
use crate::extension::{HookConfig, merge_project_extension_config};
use crate::project::{
    AgentKind, Feature, ProjectStatus, TriageSource, VibeMode, normalized_feature_name,
    worktree_name,
};

/// Most commits worth previewing in the integration overlay. A triage branch
/// with more than this has almost certainly drifted well past "apply the
/// review comments", and the list is a preview, not an audit log.
const INTEGRATE_COMMIT_PREVIEW: usize = 20;

/// Suffix appended to the source branch to name the companion branch. Kept
/// short and recognizable so `git branch` output reads obviously.
const TRIAGE_BRANCH_SUFFIX: &str = "-triage";

/// The facts about a PR a companion is planned from. Explicit so a plan does
/// not depend on `AppMode::PrReview` being open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriagePrFacts {
    pub pr_number: u32,
    pub head_sha: String,
    /// The PR's head branch; empty for a pre-`head_ref` cached PR row.
    pub head_ref: String,
}

/// What the companion feature is to be created with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TriageCompanionSpec {
    pub branch: String,
    pub agent: AgentKind,
    pub mode: VibeMode,
    pub review: bool,
    pub enable_chrome: bool,
}

/// Whether planning may `git fetch origin <head sha>` when the PR head is not
/// present locally. A fetch adds objects to the object database, so a strictly
/// read-only preview uses [`FetchPolicy::Never`].
// `Never` and the plan's preview-only fields are for the GUI destination job
// (a later change); the TUI only needs `IfMissing` and the registration inputs.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchPolicy {
    Never,
    IfMissing,
}

/// A companion for the same `(PR, source feature)` that already exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExistingTriageCompanion {
    pub feature_id: String,
    pub name: String,
}

/// Everything decided about a companion before anything is created. Holds the
/// exact hook snapshot and choice so registration runs what was previewed.
#[allow(dead_code)] // preview fields are read by the GUI caller, not the TUI
#[derive(Debug, Clone)]
pub(crate) struct TriageCompanionPlan {
    pub project_id: String,
    pub project_name: String,
    pub repo: PathBuf,
    pub source_feature_id: String,
    pub source_workdir: PathBuf,
    pub pr_number: u32,
    /// The ref integration pushes onto.
    pub pr_branch: String,
    /// Spec with the branch trimmed.
    pub spec: TriageCompanionSpec,
    pub worktree_name: String,
    /// What `git worktree add` branches from (a branch name or the head sha).
    pub base: String,
    /// The resolved commit recorded as `TriageSource::base_sha`.
    pub base_sha: String,
    /// A local git branch already has the companion's name (git would refuse).
    pub local_branch_exists: bool,
    /// `base_sha` names a commit this repository actually has. `rev-parse
    /// --verify` accepts any well-formed full id, so the TUI's base can
    /// "resolve" without the object existing; a caller that must fail closed
    /// checks this before creating anything.
    pub base_commit_present: bool,
    pub existing_companion: Option<ExistingTriageCompanion>,
    pub hook: Option<HookConfig>,
    pub hook_choice: Option<String>,
    /// The hook asks the user to choose; a caller must supply a choice.
    pub hook_choice_required: bool,
}

/// Stable identities of a registered (stopped) companion.
#[allow(dead_code)] // `session_id` is for the GUI caller
#[derive(Debug, Clone)]
pub(crate) struct CreatedTriageCompanion {
    pub project_id: String,
    pub feature_id: String,
    pub session_id: String,
    pub name: String,
    /// Set when the worktree hook failed; the companion is kept.
    pub hook_warning: Option<String>,
}

impl App {
    // ---------------------------------------------------------------- setup

    /// Open the compact triage-feature setup overlay. Called when the user
    /// picks the `New feature…` row in the fix-target picker and no companion
    /// feature exists for this PR yet.
    ///
    /// Deliberately a single settings list rather than a re-run of the
    /// multi-step feature wizard: the user is mid-triage, and everything the
    /// wizard asks that doesn't change how the *triage agent* behaves (source
    /// worktree, existing-worktree reuse, session naming, task prompt) has an
    /// obvious answer here.
    pub(crate) fn pr_review_open_triage_feature_setup(&mut self, pending_batch: bool) {
        let Some((state_workdir, source_branch)) = (match &self.mode {
            AppMode::PrReview(state) => Some((
                state.workdir.clone(),
                // Prefer the PR's own head branch: it names what the fixes are
                // for, even when the workdir has some other branch checked out.
                state.review.pr.head_ref.clone(),
            )),
            _ => None,
        }) else {
            return;
        };

        let agents = self.allowed_agents_for_project_path(&state_workdir);
        if agents.is_empty() {
            self.push_toast_warning("No agent harness is allowed for this workspace");
            return;
        }
        let preferred = self
            .feature_indices_for_workdir(&state_workdir)
            .map(|(pi, _)| self.store.projects[pi].preferred_agent.clone());
        let agent_index = preferred
            .and_then(|p| agents.iter().position(|a| *a == p))
            .unwrap_or(0);
        let presets = self.active_extension.allowed_feature_presets();
        let branch = self.unique_triage_branch(&state_workdir, &source_branch);

        if let AppMode::PrReview(state) = &mut self.mode {
            state.new_feature_setup = Some(TriageFeatureSetupState {
                presets,
                preset_index: 0,
                agents,
                agent_index,
                mode: VibeMode::default(),
                review: false,
                enable_chrome: false,
                branch,
                row: 0,
                error: None,
                pending_batch,
            });
        }
    }

    /// Whether the triage-feature setup overlay is open (so key handling can
    /// route to it before the pane's own keys).
    pub fn pr_review_triage_setup_open(&self) -> bool {
        matches!(&self.mode, AppMode::PrReview(state) if state.new_feature_setup.is_some())
    }

    /// Move the setup overlay's focused row (`+1`/`-1`, wrapping).
    pub fn pr_review_triage_setup_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(setup) = &mut state.new_feature_setup
        {
            setup.move_row(delta);
        }
    }

    /// Change the focused row's value (`+1`/`-1`, wrapping). A no-op on the
    /// branch row, which is typed rather than cycled.
    ///
    /// Choosing a preset immediately applies its harness/mode/review/chrome to
    /// the rows below it, so the overlay always shows the settings that will
    /// actually be used — and those rows stay editable afterwards, so a preset
    /// is a starting point rather than a lock.
    pub fn pr_review_triage_setup_adjust(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(setup) = &mut state.new_feature_setup
        {
            setup.adjust(delta);
        }
    }

    /// Whether the setup overlay's focused row is the free-text branch field,
    /// so key handling can send bare characters to it instead of treating them
    /// as movement/adjust verbs.
    pub fn pr_review_triage_setup_on_branch_row(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state)
                if state
                    .new_feature_setup
                    .as_ref()
                    .is_some_and(|setup| setup.on_branch_row())
        )
    }

    /// Append a character to the branch row (only when it's focused).
    pub fn pr_review_triage_setup_branch_push(&mut self, c: char) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(setup) = &mut state.new_feature_setup
        {
            setup.branch_push(c);
        }
    }

    /// Delete the last character of the branch row (only when it's focused).
    pub fn pr_review_triage_setup_branch_backspace(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(setup) = &mut state.new_feature_setup
        {
            setup.branch_backspace();
        }
    }

    /// Abandon the setup overlay — and with it this fix, since no target was
    /// resolved. The fix-target pick is left unresolved so the next `f`
    /// re-offers every option rather than silently falling back to a target
    /// the user didn't choose.
    pub fn pr_review_triage_setup_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.new_feature_setup = None;
            state.pending_batch = false;
        }
        self.pr_review_clear_fix_target();
    }

    /// Create the companion feature from the overlay's settings and continue
    /// into the fix confirm dialog the pending action wanted. Validation
    /// failures (empty/duplicate branch, worktree already present) stay inline
    /// in the overlay so the user can correct them without losing the pane.
    pub fn pr_review_triage_setup_confirm(&mut self) -> Result<()> {
        let Some(setup) = (match &self.mode {
            AppMode::PrReview(state) => state.new_feature_setup.clone(),
            _ => return Ok(()),
        }) else {
            return Ok(());
        };

        match self.create_triage_feature(&setup) {
            Ok(feature_name) => {
                if let AppMode::PrReview(state) = &mut self.mode {
                    state.new_feature_setup = None;
                    state.pending_batch = setup.pending_batch;
                    state.fix_target = FixTarget::NewFeature;
                    state.fix_target_picked = true;
                    state.review_harness = Some(setup.agent());
                }
                self.push_toast_success(format!(
                    "Triage feature '{feature_name}' created ({}, {})",
                    setup.agent().display_name(),
                    setup.mode.display_name()
                ));
                self.pr_review_continue_after_harness();
                Ok(())
            }
            Err(err) => {
                if let AppMode::PrReview(state) = &mut self.mode
                    && let Some(setup) = &mut state.new_feature_setup
                {
                    setup.error = Some(err.to_string());
                }
                Ok(())
            }
        }
    }

    /// Create the isolated worktree, register the feature with its
    /// [`TriageSource`] link, and start its triage agent session. Returns the
    /// new feature's name.
    ///
    /// A thin TUI caller of [`Self::plan_triage_companion`] and
    /// [`Self::register_triage_companion`]: it reads the PR facts off the open
    /// pane, then plans, registers, and starts the agent.
    ///
    /// Errors are returned rather than shown so the caller can keep them
    /// inline in the setup overlay.
    fn create_triage_feature(&mut self, setup: &TriageFeatureSetupState) -> Result<String> {
        let branch = setup.branch.trim().to_string();
        if branch.is_empty() {
            anyhow::bail!("Branch name cannot be empty");
        }

        let (state_workdir, facts) = match &self.mode {
            AppMode::PrReview(state) => (
                state.workdir.clone(),
                TriagePrFacts {
                    pr_number: state.review.pr.number,
                    head_sha: state.review.pr.head_sha.clone(),
                    head_ref: state.review.pr.head_ref.trim().to_string(),
                },
            ),
            _ => anyhow::bail!("not reviewing a PR"),
        };
        let (pi, source_fi) = self
            .feature_indices_for_workdir(&state_workdir)
            .ok_or_else(|| anyhow::anyhow!("could not find the feature for this PR"))?;
        let source_feature_id = self.store.projects[pi].features[source_fi].id.clone();
        let project_repo = self.store.projects[pi].repo.clone();
        let hook = merge_project_extension_config(&self.config.extension, &project_repo)
            .lifecycle_hooks
            .on_worktree_created;

        let agent = setup.agent();
        let plan = self.plan_triage_companion(
            &source_feature_id,
            &facts,
            &TriageCompanionSpec {
                branch,
                agent: agent.clone(),
                mode: setup.mode.clone(),
                review: setup.review,
                enable_chrome: setup.enable_chrome,
            },
            hook.as_ref(),
            // The TUI runs the hook choice-less (the interactive prompt would
            // have to unwind the PR pane), exactly as before the split.
            None,
            false,
            FetchPolicy::IfMissing,
        )?;
        let created = self.register_triage_companion(&plan)?;
        let feature_name = created.name;

        let (pi, fi) = self
            .store
            .locate_feature_by_id(Some(&created.project_id), &created.feature_id)
            .ok_or_else(|| anyhow::anyhow!("triage feature missing after add"))?;
        self.store.projects[pi].collapsed = false;
        // Mid-flow: the companion feature has already been created and
        // recorded, so a modal here would strand the triage hand-off.
        self.ensure_feature_running(pi, fi, StartIntent::Warn("the PR triage agent"))?;
        self.save()?;
        self.log_info(
            "pr_triage",
            format!(
                "Created triage feature '{feature_name}' for PR #{} \
                 ({} / {})",
                facts.pr_number,
                agent.display_name(),
                setup.mode.display_name()
            ),
        );
        Ok(feature_name)
    }

    // ------------------------------------------------- plan / register split

    /// Decide everything a triage companion needs, without changing anything.
    ///
    /// Reads the store and git (`rev-parse`, `show-ref`, `merge-base`) and
    /// nothing else: no worktree, branch, hook, toast or store write. With
    /// [`FetchPolicy::IfMissing`] it may also `git fetch origin <head sha>`,
    /// which adds objects to the object database (the TUI's historic
    /// behavior); [`FetchPolicy::Never`] keeps it strictly read-only, and a
    /// head that is not present locally then fails closed.
    ///
    /// Inputs are explicit (the PR facts, the source feature id, the exact
    /// hook config snapshot and choice), so a caller that is not the PR pane
    /// can preview and later register the same plan.
    ///
    /// Check order matches the historic `create_triage_feature`, so the first
    /// failure a user sees is unchanged.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn plan_triage_companion(
        &self,
        source_feature_id: &str,
        facts: &TriagePrFacts,
        spec: &TriageCompanionSpec,
        hook: Option<&HookConfig>,
        hook_choice: Option<&str>,
        require_hook_choice: bool,
        fetch: FetchPolicy,
    ) -> Result<TriageCompanionPlan> {
        let branch = spec.branch.trim().to_string();
        if branch.is_empty() {
            anyhow::bail!("Branch name cannot be empty");
        }
        let (pi, source_fi) = self
            .store
            .locate_feature_by_id(None, source_feature_id)
            .ok_or_else(|| anyhow::anyhow!("could not find the feature for this PR"))?;
        let project = &self.store.projects[pi];
        let source = &project.features[source_fi];
        let source_workdir = source.workdir.clone();
        let head_ref = facts.head_ref.trim();
        // The branch the fixes have to land on is the *PR's* head branch, which
        // is not necessarily what the source feature has checked out — triaging
        // a PR picked from the "other PR" list is exactly the case where they
        // differ. This value is the push destination in `push_branch`, so
        // taking the feature's branch here would push the triage commits onto
        // an unrelated remote branch. The setup overlay pre-fills the companion
        // branch name from the same source (`head_ref`).
        //
        // An empty `head_ref` means a pre-`head_ref` cached PR row; the
        // checked-out branch is then the only thing we know.
        let pr_branch = if head_ref.is_empty() {
            source.branch.clone()
        } else {
            head_ref.to_string()
        };

        if !project.is_git && self.worktree.repo_root(&project.repo).is_err() {
            anyhow::bail!("A triage feature needs a git repository (it runs in its own worktree)");
        }

        if !self.allows_agent_for_repo(&project.repo, &spec.agent) {
            anyhow::bail!(
                "Harness '{}' is not allowed for this workspace",
                spec.agent.display_name()
            );
        }
        self.ensure_agent_mode_supported(&spec.agent, &spec.mode)?;

        let normalized = normalized_feature_name(&branch);
        if project
            .features
            .iter()
            .any(|f| normalized_feature_name(&f.name) == normalized)
        {
            anyhow::bail!("Feature '{branch}' already exists in '{}'", project.name);
        }

        if require_hook_choice {
            super::automation::checked_hook_choice(hook.and_then(|h| h.prompt()), hook_choice)?;
        }

        // Seed from the PR head. `triage_base_with` prefers the local branch
        // when it already contains the PR head (so unpushed work isn't dropped)
        // and falls back to the head SHA itself.
        let base = triage_base_with(&source_workdir, &pr_branch, &facts.head_sha, fetch)
            .ok_or_else(|| anyhow::anyhow!("could not resolve a base commit for the PR head"))?;
        let base_sha = rev_parse(&source_workdir, &base).unwrap_or_else(|| base.clone());

        let existing_companion = project.features.iter().find_map(|f| {
            f.triage_source
                .as_ref()
                .filter(|link| {
                    link.pr_number == facts.pr_number && link.source_feature_id == source.id
                })
                .map(|_| ExistingTriageCompanion {
                    feature_id: f.id.clone(),
                    name: f.name.clone(),
                })
        });

        Ok(TriageCompanionPlan {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            repo: project.repo.clone(),
            source_feature_id: source.id.clone(),
            source_workdir: source_workdir.clone(),
            pr_number: facts.pr_number,
            pr_branch,
            worktree_name: worktree_name(&project.name, &branch),
            local_branch_exists: branch_exists(&source_workdir, &branch),
            base_commit_present: commit_exists(&source_workdir, &base_sha),
            base,
            base_sha,
            existing_companion,
            hook: hook.cloned(),
            hook_choice: hook_choice.map(str::to_string),
            hook_choice_required: hook.is_some_and(|h| h.prompt().is_some()),
            spec: TriageCompanionSpec {
                branch,
                ..spec.clone()
            },
        })
    }

    /// Execute a plan: create the worktree and branch, run the planned hook
    /// exactly as planned, and register the companion **stopped**. Starting the
    /// agent is the caller's decision.
    ///
    /// The checkout is retained when a later step fails (it may hold the
    /// user's hook output); the error names it.
    pub(crate) fn register_triage_companion(
        &mut self,
        plan: &TriageCompanionPlan,
    ) -> Result<CreatedTriageCompanion> {
        // Re-validate before creating anything: the store may have changed
        // since the plan, and a refusal here must leave no checkout or hook
        // side effects behind. The post-create check stays as the final guard.
        self.triage_registration_target(plan, "")
            .map_err(|e| anyhow::anyhow!(e))?;
        let workdir = self.worktree.create_from(
            &plan.repo,
            &plan.worktree_name,
            &plan.spec.branch,
            &plan.base,
        )?;

        // The triage feature gets its own worktree precisely so hooks and
        // permissions it writes can't touch the source feature. Run the
        // on-worktree-created hook synchronously — the interactive prompt flow
        // would have to unwind the PR pane, and the choice-less path is what
        // the automation API already does here.
        let mut hook_warning = None;
        if let Some(hook) = plan.hook.as_ref() {
            let (ok, detail) =
                Self::run_worktree_hook_sync(hook.script(), &workdir, plan.hook_choice.as_deref());
            if !ok {
                let detail = detail.unwrap_or_else(|| "unknown error".to_string());
                self.log_warn(
                    "pr_triage",
                    format!(
                        "Worktree hook failed for triage feature '{}': {detail}",
                        plan.spec.branch
                    ),
                );
                self.push_toast_warning(
                    "Triage worktree created, but its on_worktree_created hook failed",
                );
                hook_warning = Some(detail);
            }
        }

        let mut created = self.register_created_triage_companion(plan, workdir)?;
        created.hook_warning = hook_warning;
        Ok(created)
    }

    /// The project index to register into, or why the plan no longer applies
    /// (source removed, or the companion name taken). Creates nothing.
    fn triage_registration_target(
        &self,
        plan: &TriageCompanionPlan,
        suffix: &str,
    ) -> std::result::Result<usize, String> {
        let Some((pi, _)) = self
            .store
            .locate_feature_by_id(Some(&plan.project_id), &plan.source_feature_id)
        else {
            return Err(format!("Source feature removed{suffix}"));
        };
        let normalized = normalized_feature_name(&plan.spec.branch);
        if self.store.projects[pi]
            .features
            .iter()
            .any(|f| normalized_feature_name(&f.name) == normalized)
        {
            return Err(format!(
                "Feature '{}' already exists in '{}'{suffix}",
                plan.spec.branch, plan.project_name
            ));
        }
        Ok(pi)
    }

    /// Register an already-created checkout as a stopped companion. Re-checks
    /// that the source and project still exist and that the name is still free
    /// (the store may have changed since the plan), and removes the in-memory
    /// row again if the save fails, so a failed registration leaves no
    /// half-registered feature behind.
    pub(crate) fn register_created_triage_companion(
        &mut self,
        plan: &TriageCompanionPlan,
        workdir: PathBuf,
    ) -> Result<CreatedTriageCompanion> {
        let retained = format!("Triage checkout retained at {}", workdir.display());
        let pi = self
            .triage_registration_target(plan, &format!(". {retained}"))
            .map_err(|e| anyhow::anyhow!(e))?;
        let spec = &plan.spec;
        let mut feature = Feature::new_for_project(
            &plan.project_name,
            spec.branch.clone(),
            spec.branch.clone(),
            workdir.clone(),
            true,
            spec.mode.clone(),
            spec.review,
            // Plan mode would defer the launch into a planning interview; a
            // triage feature exists to apply comments that already say what to do.
            false,
            spec.agent.clone(),
            spec.enable_chrome,
            false,
        );
        // The primary session carries the triage label, so the same
        // `pr_triage_session_index` lookup the in-feature dedicated target uses
        // finds it inside the companion feature.
        let session_id = feature
            .add_session_named(
                crate::app::session_kind_for_agent(&spec.agent),
                TRIAGE_SESSION_LABEL.to_string(),
            )
            .id
            .clone();
        feature.status = ProjectStatus::Stopped;
        feature.triage_source = Some(TriageSource {
            pr_number: plan.pr_number,
            source_feature_id: plan.source_feature_id.clone(),
            pr_branch: plan.pr_branch.clone(),
            base_sha: plan.base_sha.clone(),
        });
        let feature_id = feature.id.clone();
        let name = feature.name.clone();

        self.store.projects[pi].features.push(feature);
        if let Err(err) = self.save() {
            for project in &mut self.store.projects {
                project.features.retain(|f| f.id != feature_id);
            }
            // The row is rolled back but the checkout and branch stay, so a
            // bare retry would fail in git. Say what to clear first; the path
            // stays last so it can still be read off the end.
            anyhow::bail!(
                "{err}. Before retrying, delete the retained checkout and its branch '{}' \
                 (git worktree remove {path}; git branch -D {}). {retained}",
                spec.branch,
                spec.branch,
                path = workdir.display(),
            );
        }
        Ok(CreatedTriageCompanion {
            project_id: plan.project_id.clone(),
            feature_id,
            session_id,
            name,
            hook_warning: None,
        })
    }

    /// A companion branch name not already taken by a feature or a git branch.
    /// `<pr-branch>-triage`, then `-triage-2`, `-triage-3`, … so re-triaging a
    /// PR after deleting the first companion doesn't collide.
    fn unique_triage_branch(&self, workdir: &Path, source_branch: &str) -> String {
        let base = format!("{source_branch}{TRIAGE_BRANCH_SUFFIX}");
        let taken = |candidate: &str| -> bool {
            let normalized = normalized_feature_name(candidate);
            let name_taken = self
                .feature_indices_for_workdir(workdir)
                .map(|(pi, _)| {
                    self.store.projects[pi]
                        .features
                        .iter()
                        .any(|f| normalized_feature_name(&f.name) == normalized)
                })
                .unwrap_or(false);
            name_taken || branch_exists(workdir, candidate)
        };
        if !taken(&base) {
            return base;
        }
        (2..100)
            .map(|n| format!("{base}-{n}"))
            .find(|candidate| !taken(candidate))
            .unwrap_or(base)
    }

    /// On entering PR Triage, adopt a companion triage feature that already
    /// exists for this PR: point the fix target at it and treat the target as
    /// resolved, so every fix in the PR reuses the same feature across pane
    /// re-opens and restarts without re-asking.
    ///
    /// Mirrors the existing "a dedicated session already exists, so don't
    /// re-offer the picker" rule, one level up.
    pub(crate) fn adopt_existing_triage_feature(&mut self) {
        let Some((pi, fi)) = (match &self.mode {
            AppMode::PrReview(state) => self.triage_feature_indices(state),
            _ => None,
        }) else {
            return;
        };
        let harness = self.store.projects[pi].features[fi].agent.clone();
        if let AppMode::PrReview(state) = &mut self.mode {
            state.fix_target = FixTarget::NewFeature;
            state.fix_target_picked = true;
            state.review_harness = Some(harness);
        }
    }

    // ---------------------------------------------------------- integration

    /// Open the integration overlay (`I`): what the companion triage feature
    /// has committed since branching, and the two ways to land it on the PR.
    /// Only meaningful for the companion target — the other two targets commit
    /// straight onto the PR branch and need no integration step.
    pub fn pr_review_open_integrate(&mut self) {
        let is_companion = matches!(
            &self.mode,
            AppMode::PrReview(state) if state.fix_target.is_companion_feature()
        );
        if !is_companion {
            self.push_toast_warning(
                "Integration applies to the `New feature…` target — other targets commit on the PR branch already",
            );
            return;
        }
        let Some((pi, fi)) = self.pr_review_target_feature() else {
            self.push_toast_warning("No triage feature for this PR yet — press f to create one");
            return;
        };
        let feature = &self.store.projects[pi].features[fi];
        let Some(link) = feature.triage_source.clone() else {
            self.push_toast_warning("That feature isn't linked to a PR");
            return;
        };
        let triage_workdir = feature.workdir.clone();
        let triage_branch = feature.branch.clone();
        let source_workdir = match &self.mode {
            AppMode::PrReview(state) => state.workdir.clone(),
            _ => return,
        };

        let commits = commits_since(
            &triage_workdir,
            &link.base_sha,
            Some(INTEGRATE_COMMIT_PREVIEW),
        );
        // Names the worktree, not a branch: the cherry-pick lands in whatever
        // the source feature has checked out, which isn't necessarily the PR's
        // branch.
        let source_dirty = worktree_is_dirty(&source_workdir).then(|| {
            format!(
                "the source worktree has uncommitted changes — commit or stash them in `{}` first",
                crate::app::util::shorten_path(&source_workdir)
            )
        });
        let triage_dirty = worktree_is_dirty(&triage_workdir);

        if let AppMode::PrReview(state) = &mut self.mode {
            state.integrate = Some(TriageIntegrateState {
                triage_branch,
                pr_branch: link.pr_branch,
                commits,
                source_dirty,
                triage_dirty,
                selected: 0,
                error: None,
                done: None,
                companion_feature_id: None,
            });
        }
    }

    /// Whether the integration overlay is open.
    pub fn pr_review_integrate_open(&self) -> bool {
        matches!(&self.mode, AppMode::PrReview(state) if state.integrate.is_some())
    }

    /// Move the integration overlay's highlight (`+1`/`-1`, wrapping).
    pub fn pr_review_integrate_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(integrate) = &mut state.integrate
        {
            let len = TriageIntegration::ALL.len() as isize;
            integrate.selected = ((integrate.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Close the integration overlay.
    pub fn pr_review_integrate_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.integrate = None;
        }
    }

    /// Run the highlighted integration. Both paths are non-destructive: the
    /// push is never forced (a diverged PR branch is reported, not
    /// overwritten), and the cherry-pick refuses to run against a dirty source
    /// worktree. Results — success or failure — stay in the overlay.
    pub fn pr_review_integrate_confirm(&mut self) -> Result<()> {
        let Some((choice, triage_branch, pr_branch, blocked)) = (match &self.mode {
            AppMode::PrReview(state) => state.integrate.as_ref().map(|i| {
                (
                    i.focused(),
                    i.triage_branch.clone(),
                    i.pr_branch.clone(),
                    i.source_dirty.clone(),
                )
            }),
            _ => None,
        }) else {
            return Ok(());
        };

        if choice == TriageIntegration::CherryPick
            && let Some(reason) = blocked
        {
            self.set_integrate_error(format!("Cherry-pick refused: {reason}"));
            return Ok(());
        }

        let Some((pi, fi)) = self.pr_review_target_feature() else {
            self.set_integrate_error("The triage feature no longer exists".to_string());
            return Ok(());
        };
        let feature = &self.store.projects[pi].features[fi];
        let triage_workdir = feature.workdir.clone();
        let base_sha = feature
            .triage_source
            .as_ref()
            .map(|link| link.base_sha.clone())
            .unwrap_or_default();
        let source_workdir = match &self.mode {
            AppMode::PrReview(state) => state.workdir.clone(),
            _ => return Ok(()),
        };

        let outcome = match choice {
            TriageIntegration::Push => push_branch(&triage_workdir, &triage_branch, &pr_branch),
            TriageIntegration::CherryPick => {
                cherry_pick_range(&source_workdir, &triage_workdir, &base_sha, &triage_branch)
            }
        };

        match outcome {
            Ok(summary) => {
                if let AppMode::PrReview(state) = &mut self.mode
                    && let Some(integrate) = &mut state.integrate
                {
                    integrate.error = None;
                    integrate.done = Some(summary.clone());
                }
                self.log_info("pr_triage", format!("Integration: {summary}"));
                self.push_toast_success(summary);
            }
            Err(err) => self.set_integrate_error(err.to_string()),
        }
        Ok(())
    }

    fn set_integrate_error(&mut self, message: String) {
        self.log_warn("pr_triage", format!("Integration failed: {message}"));
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(integrate) = &mut state.integrate
        {
            integrate.error = Some(message);
        }
    }

    /// Short "feature (harness, mode)" description of where a fix will land,
    /// for the fix confirm dialog. `None` for the in-feature targets, whose
    /// existing "will inject into the …" line already says everything.
    pub(crate) fn pr_review_triage_feature_summary(&self) -> Option<String> {
        let AppMode::PrReview(state) = &self.mode else {
            return None;
        };
        if !state.fix_target.is_companion_feature() {
            return None;
        }
        let (pi, fi) = self.pr_review_feature_for_target(state)?;
        let feature = &self.store.projects[pi].features[fi];
        Some(format!(
            "{} · {} · {}",
            feature.name,
            feature.agent.display_name(),
            feature.mode.display_name()
        ))
    }
}

// ------------------------------------------------------------------- git

/// Base commit for the companion worktree.
///
/// Prefers the local branch of the PR's head when it already contains the PR
/// head commit — that's the PR head plus any local commits not yet pushed, and
/// branching from the SHA instead would silently drop them. Otherwise use the
/// PR head itself (the local branch is behind, e.g. someone else pushed, or
/// this checkout has never had that branch at all), fetching it first if it
/// isn't present locally.
#[cfg(test)]
fn triage_base(workdir: &Path, pr_branch: &str, head_sha: &str) -> Option<String> {
    triage_base_with(workdir, pr_branch, head_sha, FetchPolicy::IfMissing)
}

/// [`triage_base`] with an explicit fetch policy; `Never` performs no git
/// writes at all.
fn triage_base_with(
    workdir: &Path,
    pr_branch: &str,
    head_sha: &str,
    fetch: FetchPolicy,
) -> Option<String> {
    let head_sha = head_sha.trim();
    if !pr_branch.is_empty()
        && branch_exists(workdir, pr_branch)
        && (head_sha.is_empty() || contains_commit(workdir, pr_branch, head_sha))
    {
        return Some(pr_branch.to_string());
    }
    if head_sha.is_empty() {
        return branch_exists(workdir, pr_branch).then(|| pr_branch.to_string());
    }
    if fetch == FetchPolicy::IfMissing && rev_parse(workdir, head_sha).is_none() {
        // Best effort: the head may simply not have been fetched yet.
        let _ = Command::new("git")
            .args(["fetch", "--quiet", "origin", head_sha])
            .current_dir(workdir)
            .status();
    }
    if rev_parse(workdir, head_sha).is_some() {
        Some(head_sha.to_string())
    } else if branch_exists(workdir, pr_branch) {
        Some(pr_branch.to_string())
    } else {
        None
    }
}

pub(crate) fn rev_parse(workdir: &Path, rev: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", rev])
        .current_dir(workdir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub(crate) fn branch_exists(workdir: &Path, branch: &str) -> bool {
    if branch.is_empty() {
        return false;
    }
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(workdir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn commit_exists(workdir: &Path, rev: &str) -> bool {
    Command::new("git")
        .args(["cat-file", "-e", &format!("{rev}^{{commit}}")])
        .current_dir(workdir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn contains_commit(workdir: &Path, branch: &str, sha: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", sha, branch])
        .current_dir(workdir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Whether a worktree has uncommitted changes (tracked or untracked).
pub(crate) fn worktree_is_dirty(workdir: &Path) -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(workdir)
        .output()
        .map(|out| out.status.success() && !out.stdout.is_empty())
        .unwrap_or(false)
}

/// One-line summaries of commits on `HEAD` after `base`, newest first.
/// `limit` caps the preview; `None` means every commit in the range (what the
/// cherry-pick path needs, since it acts on all of them).
pub(crate) fn commits_since(workdir: &Path, base: &str, limit: Option<usize>) -> Vec<String> {
    if base.trim().is_empty() {
        return Vec::new();
    }
    let mut args = vec![
        "log".to_string(),
        "--oneline".to_string(),
        "--no-decorate".to_string(),
    ];
    if let Some(limit) = limit {
        args.push(format!("-{limit}"));
    }
    args.push(format!("{base}..HEAD"));
    Command::new("git")
        .args(&args)
        .current_dir(workdir)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// `git push origin <triage>:<pr-branch>` — a plain fast-forward push. Never
/// `--force`: if the PR branch moved on independently, that's reported so the
/// user can rebase deliberately rather than silently losing the other commits.
pub(crate) fn push_branch(
    triage_workdir: &Path,
    triage_branch: &str,
    pr_branch: &str,
) -> Result<String> {
    let out = Command::new("git")
        .args(["push", "origin", &format!("{triage_branch}:{pr_branch}")])
        .current_dir(triage_workdir)
        .output()?;
    if out.status.success() {
        return Ok(format!(
            "Pushed `{triage_branch}` onto the PR branch `{pr_branch}`"
        ));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let detail = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("git push failed")
        .trim()
        .to_string();
    if stderr.contains("non-fast-forward") || stderr.contains("fetch first") {
        anyhow::bail!(
            "the PR branch has moved on — pull `{pr_branch}` into `{triage_branch}` and try again ({detail})"
        );
    }
    anyhow::bail!("{detail}");
}

/// Cherry-pick the triage commits into the source worktree. Guarded by a
/// dirty check at the call site *and* here, and aborts the pick on conflict so
/// the source worktree is never left mid-cherry-pick.
pub(crate) fn cherry_pick_range(
    source_workdir: &Path,
    triage_workdir: &Path,
    base_sha: &str,
    triage_branch: &str,
) -> Result<String> {
    if worktree_is_dirty(source_workdir) {
        anyhow::bail!("the source worktree has uncommitted changes");
    }
    let commits = commits_since(triage_workdir, base_sha, None);
    if commits.is_empty() {
        anyhow::bail!("`{triage_branch}` has no commits to integrate yet");
    }
    let count = commits.len();
    let out = Command::new("git")
        .args(["cherry-pick", &format!("{base_sha}..{triage_branch}")])
        .current_dir(source_workdir)
        .output()?;
    if out.status.success() {
        return Ok(format!(
            "Cherry-picked {count} commit{} from `{triage_branch}` into the source worktree",
            if count == 1 { "" } else { "s" }
        ));
    }
    // Leave the source worktree exactly as it was found.
    let _ = Command::new("git")
        .args(["cherry-pick", "--abort"])
        .current_dir(source_workdir)
        .status();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let detail = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("git cherry-pick failed")
        .trim()
        .to_string();
    anyhow::bail!("cherry-pick aborted, source worktree left unchanged ({detail})");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo on `main` with two commits, returning `(dir, first_sha, head_sha)`.
    fn repo_with_two_commits() -> (tempfile::TempDir, String, String) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        git(&path, &["init", "-q", "-b", "main"]);
        git(&path, &["config", "user.email", "t@example.com"]);
        git(&path, &["config", "user.name", "T"]);
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-qm", "first"]);
        let first = rev_parse(&path, "HEAD").unwrap();
        std::fs::write(path.join("a.txt"), "one\ntwo\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-qm", "second"]);
        let head = rev_parse(&path, "HEAD").unwrap();
        (dir, first, head)
    }

    #[test]
    fn triage_base_prefers_the_local_branch_when_it_contains_the_pr_head() {
        // The local branch is at or ahead of the PR head: branching from the
        // SHA would silently drop the unpushed commits, so the branch wins.
        let (dir, first, _head) = repo_with_two_commits();
        assert_eq!(
            triage_base(dir.path(), "main", &first).as_deref(),
            Some("main")
        );
    }

    #[test]
    fn triage_base_falls_back_to_the_head_sha_when_the_branch_is_behind() {
        // The PR head isn't reachable from the local branch (someone else
        // pushed, or we're on a stale checkout) — seed from the head itself.
        let (dir, first, head) = repo_with_two_commits();
        git(dir.path(), &["checkout", "-q", "-B", "main", &first]);
        assert_eq!(
            triage_base(dir.path(), "main", &head).as_deref(),
            Some(head.as_str())
        );
    }

    #[test]
    fn triage_base_is_none_when_nothing_resolves() {
        let dir = tempfile::TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        assert_eq!(triage_base(dir.path(), "nope", "deadbeef"), None);
    }

    #[test]
    fn commits_since_lists_only_what_came_after_the_base() {
        let (dir, first, _head) = repo_with_two_commits();
        let commits = commits_since(dir.path(), &first, None);
        assert_eq!(commits.len(), 1);
        assert!(commits[0].contains("second"));

        // An empty base is "unknown", not "everything".
        assert!(commits_since(dir.path(), "", None).is_empty());
    }

    #[test]
    fn worktree_is_dirty_tracks_uncommitted_work() {
        let (dir, _first, _head) = repo_with_two_commits();
        assert!(!worktree_is_dirty(dir.path()));
        std::fs::write(dir.path().join("b.txt"), "wip\n").unwrap();
        assert!(
            worktree_is_dirty(dir.path()),
            "untracked files count — a cherry-pick could still collide with them"
        );
    }

    #[test]
    fn cherry_pick_refuses_a_dirty_source_even_when_called_directly() {
        // The overlay disables the option, but the guard is repeated at the
        // git layer so no future caller can bypass it.
        let (dir, first, _head) = repo_with_two_commits();
        std::fs::write(dir.path().join("wip.txt"), "wip\n").unwrap();
        let err = cherry_pick_range(dir.path(), dir.path(), &first, "main").unwrap_err();
        assert!(err.to_string().contains("uncommitted changes"), "{err}");
    }

    #[test]
    fn cherry_pick_reports_an_empty_range_rather_than_running() {
        let (dir, _first, head) = repo_with_two_commits();
        let err = cherry_pick_range(dir.path(), dir.path(), &head, "main").unwrap_err();
        assert!(err.to_string().contains("no commits to integrate"), "{err}");
    }

    #[test]
    fn cherry_pick_conflict_leaves_the_source_worktree_clean() {
        // Both branches edit the same line, so the pick must conflict; the
        // source worktree has to come back exactly as it was found.
        let (dir, first, _head) = repo_with_two_commits();
        let source = dir.path().to_path_buf();
        // A path of its own, outside the repo, so the worktree doesn't show up
        // as untracked noise in the source's dirty check.
        let outside = tempfile::TempDir::new().unwrap();
        let triage = outside.path().join("triage-wt");
        git(
            &source,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "main-triage",
                triage.to_str().unwrap(),
                &first,
            ],
        );
        std::fs::write(triage.join("a.txt"), "one\nconflicting\n").unwrap();
        git(&triage, &["add", "-A"]);
        git(&triage, &["commit", "-qm", "conflicting change"]);

        let err = cherry_pick_range(&source, &triage, &first, "main-triage").unwrap_err();

        assert!(err.to_string().contains("left unchanged"), "{err}");
        assert!(!worktree_is_dirty(&source));
        assert_eq!(
            std::fs::read_to_string(source.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
    }

    #[test]
    fn branch_exists_distinguishes_real_branches() {
        let (dir, _first, _head) = repo_with_two_commits();
        assert!(branch_exists(dir.path(), "main"));
        assert!(!branch_exists(dir.path(), "main-triage"));
        assert!(!branch_exists(dir.path(), ""));
    }

    #[test]
    fn rev_parse_resolves_only_real_revisions() {
        let (dir, _first, head) = repo_with_two_commits();
        assert_eq!(
            rev_parse(dir.path(), "HEAD").as_deref(),
            Some(head.as_str())
        );
        assert_eq!(rev_parse(dir.path(), "no-such-ref"), None);
        // A non-repo path resolves nothing rather than panicking.
        assert_eq!(rev_parse(&PathBuf::from("/"), "HEAD"), None);
    }
}
