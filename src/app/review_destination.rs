//! The final review's **destination choice**: where a finished review's
//! "address the feedback" prompt is dispatched.
//!
//! This mirrors PR Triage's fix-target picker (`src/app/pr_review.rs` +
//! `src/app/triage_feature.rs`), one layer over from a PR pane to the native
//! diff-viewer review. Four destinations:
//!
//! 1. **This feature's live session** — the shipped behaviour.
//! 2. **A dedicated review session** on a harness the reviewer picks — a fresh
//!    `Final Review` window in the reviewed feature.
//! 3. **Another existing feature** — route the fixes into an unrelated
//!    feature's agent session. (PR Triage has no equivalent; a PR's fixes have
//!    no reason to land in an unrelated feature.)
//! 4. **A new companion feature** — an isolated worktree branched from the
//!    reviewed feature's branch head, with its own harness / vibe mode, and an
//!    explicit integration step (push / cherry-pick) to land the fixes back on
//!    the source branch. Directly analogous to PR Triage's `New feature…`
//!    target, minus the PR: integration targets the source feature's own
//!    branch, recorded on [`crate::project::ReviewSource`].

use anyhow::Result;

use crate::app::pr_review::FixTarget;
use crate::app::review::FINAL_REVIEW_SESSION_LABEL;
use crate::app::state::{
    AppMode, ReviewDestinationPickState, ReviewDestinationRow, TriageFeatureSetupState,
    TriageIntegrateState, TriageIntegration,
};
use crate::app::triage_feature::{
    branch_exists, cherry_pick_range, commits_since, push_branch, rev_parse, worktree_is_dirty,
};
use crate::app::{App, StartIntent};
use crate::extension::merge_project_extension_config;
use crate::project::{
    Feature, ProjectStatus, ReviewSource, VibeMode, normalized_feature_name, worktree_name,
};

/// Most commits worth previewing in the integration overlay — the same cap PR
/// Triage uses. The list is a preview, not an audit log.
const INTEGRATE_COMMIT_PREVIEW: usize = 20;

/// Suffix appended to the source branch to name the companion branch.
const REVIEW_BRANCH_SUFFIX: &str = "-review-fixes";

/// Settings shared by companion creation adapters; no picker state is needed.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReviewCompanionSpec {
    pub branch: String,
    pub agent: crate::project::AgentKind,
    pub mode: VibeMode,
    pub review: bool,
    pub enable_chrome: bool,
}

pub(crate) struct CreatedReviewCompanion {
    pub project_id: String,
    pub feature_id: String,
    pub session_id: String,
    pub name: String,
    pub hook_warning: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ReviewCompanionRegistration {
    pub project_id: String,
    pub project_name: String,
    pub repo: std::path::PathBuf,
    pub source_id: String,
    pub source_branch: String,
    pub source_workdir: std::path::PathBuf,
    pub base_sha: String,
    pub spec: ReviewCompanionSpec,
}

impl App {
    // ------------------------------------------------------------ picker

    /// The reviewed feature's `(project, feature)` indices, resolved from the
    /// open diff viewer's workdir.
    fn review_source_indices(&self) -> Option<(usize, usize)> {
        let AppMode::DiffViewer(state) = &self.mode else {
            return None;
        };
        self.feature_indices_for_workdir(&state.workdir)
    }

    /// Open the destination picker over the review viewer (`t`). No-op outside
    /// a final review.
    ///
    /// The reviewed feature may not resolve from the store (an ad-hoc diff of a
    /// path AMF doesn't track); the picker still opens, just without the
    /// per-other-feature rows and with the machine's harness list rather than a
    /// per-repo one.
    pub(crate) fn open_review_destination_picker(&mut self) {
        if !matches!(&self.mode, AppMode::DiffViewer(state) if state.review) {
            return;
        }
        let source = self.review_source_indices();

        // The dedicated-session harness list, same source as
        // `ReviewHarnessPick` uses: the machine's enabled harnesses, or the
        // reviewed project's preferred agent when none are configured.
        let agents = if !self.store.available_harnesses.is_empty() {
            self.store.available_harnesses.clone()
        } else {
            match source {
                Some((pi, _)) => vec![self.store.projects[pi].preferred_agent.clone()],
                None => vec![crate::project::AgentKind::default()],
            }
        };

        let live_label = source.and_then(|(pi, fi)| {
            self.store.projects[pi].features[fi]
                .sessions
                .iter()
                .find(|s| s.kind.is_agent_harness())
                .map(|s| s.label.clone())
        });

        let reviewed_id = source.map(|(pi, fi)| self.store.projects[pi].features[fi].id.clone());
        let mut other_features: Vec<ReviewDestinationRow> = Vec::new();
        for project in &self.store.projects {
            for feature in &project.features {
                if Some(&feature.id) == reviewed_id.as_ref() {
                    continue;
                }
                other_features.push(ReviewDestinationRow::ExistingFeature {
                    feature_id: feature.id.clone(),
                    label: format!("{} / {}", project.name, feature.name),
                });
            }
        }

        let mut rows = vec![ReviewDestinationRow::ExistingLive(live_label)];
        rows.extend(agents.into_iter().map(ReviewDestinationRow::Dedicated));
        rows.extend(other_features);
        rows.push(ReviewDestinationRow::NewFeature);

        // Pre-highlight whatever the viewer is currently pointed at, so opening
        // the picker and pressing Enter is a no-op rather than a silent reset.
        let selected = if let AppMode::DiffViewer(state) = &self.mode {
            rows.iter()
                .position(|row| match (row, state.fix_target) {
                    (ReviewDestinationRow::ExistingLive(_), FixTarget::ExistingLive) => true,
                    (ReviewDestinationRow::Dedicated(a), FixTarget::DedicatedReview) => {
                        state.review_harness.as_ref() == Some(a)
                    }
                    (
                        ReviewDestinationRow::ExistingFeature { feature_id, .. },
                        FixTarget::ExistingFeature,
                    ) => state.fix_target_feature_id.as_deref() == Some(feature_id.as_str()),
                    (ReviewDestinationRow::NewFeature, FixTarget::NewFeature) => true,
                    _ => false,
                })
                .unwrap_or(0)
        } else {
            0
        };

        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.destination_pick = Some(ReviewDestinationPickState { rows, selected });
        }
    }

    /// Whether the destination picker is open (so key handling routes to it
    /// before the viewer's own keys).
    pub fn review_destination_pick_open(&self) -> bool {
        matches!(&self.mode, AppMode::DiffViewer(state) if state.destination_pick.is_some())
    }

    /// Move the picker highlight (`+1`/`-1`, wrapping).
    pub fn review_destination_pick_move(&mut self, delta: isize) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(pick) = &mut state.destination_pick
        {
            let n = pick.rows.len();
            if n == 0 {
                return;
            }
            pick.selected = ((pick.selected as isize + delta).rem_euclid(n as isize)) as usize;
        }
    }

    /// Close the picker without changing the destination.
    pub fn review_destination_pick_cancel(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.destination_pick = None;
        }
    }

    /// Apply the highlighted row. For every row but `New feature…` this resolves
    /// the destination and closes the picker; `New feature…` opens the compact
    /// companion-feature setup overlay instead.
    pub fn review_destination_pick_confirm(&mut self) -> Result<()> {
        let Some(row) = (match &self.mode {
            AppMode::DiffViewer(state) => state
                .destination_pick
                .as_ref()
                .and_then(|pick| pick.rows.get(pick.selected).cloned()),
            _ => None,
        }) else {
            return Ok(());
        };

        match row {
            ReviewDestinationRow::ExistingLive(_) => {
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.fix_target = FixTarget::ExistingLive;
                    state.fix_target_feature_id = None;
                    state.review_harness = None;
                    state.destination_pick = None;
                }
                self.message =
                    Some("Fixes go to this feature's existing agent session".to_string());
            }
            ReviewDestinationRow::Dedicated(agent) => {
                let name = agent.display_name().to_string();
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.fix_target = FixTarget::DedicatedReview;
                    state.fix_target_feature_id = None;
                    state.review_harness = Some(agent);
                    state.destination_pick = None;
                }
                self.message = Some(format!(
                    "Fixes will run in a fresh dedicated review session ({name})"
                ));
            }
            ReviewDestinationRow::ExistingFeature { feature_id, label } => {
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.fix_target = FixTarget::ExistingFeature;
                    state.fix_target_feature_id = Some(feature_id);
                    state.review_harness = None;
                    state.destination_pick = None;
                }
                self.message = Some(format!("Fixes go to {label}"));
            }
            ReviewDestinationRow::NewFeature => {
                self.open_review_feature_setup();
            }
        }
        Ok(())
    }

    // ----------------------------------------------- companion-feature setup

    /// Open the compact companion-feature setup overlay (from the `New feature…`
    /// row). Deliberately a single settings list, not a re-run of the feature
    /// wizard — mirrors `pr_review_open_triage_feature_setup`.
    fn open_review_feature_setup(&mut self) {
        let Some((pi, fi)) = self.review_source_indices() else {
            self.message = Some("Can't resolve the feature under review".to_string());
            return;
        };
        let preferred = self.store.projects[pi].preferred_agent.clone();
        // Same harness list as the destination picker — the machine's enabled
        // harnesses, or the reviewed project's preferred agent when none are
        // configured.
        let agents = if self.store.available_harnesses.is_empty() {
            vec![preferred.clone()]
        } else {
            self.store.available_harnesses.clone()
        };
        let agent_index = agents.iter().position(|a| *a == preferred).unwrap_or(0);
        let presets = self.active_extension.allowed_feature_presets();

        let source_workdir = self.store.projects[pi].features[fi].workdir.clone();
        let source_branch = self.store.projects[pi].features[fi].branch.clone();
        let branch = self.unique_review_branch(&source_workdir, &source_branch);

        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.destination_pick = None;
            state.review_feature_setup = Some(TriageFeatureSetupState {
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
                pending_batch: false,
            });
        }
    }

    /// Whether the companion-feature setup overlay is open.
    pub fn review_feature_setup_open(&self) -> bool {
        matches!(&self.mode, AppMode::DiffViewer(state) if state.review_feature_setup.is_some())
    }

    /// Whether the setup overlay's focused row is the free-text branch field.
    pub fn review_feature_setup_on_branch_row(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::DiffViewer(state)
                if state
                    .review_feature_setup
                    .as_ref()
                    .is_some_and(|s| s.on_branch_row())
        )
    }

    /// Move the setup overlay's focused row (`+1`/`-1`, wrapping).
    pub fn review_feature_setup_move(&mut self, delta: isize) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(setup) = &mut state.review_feature_setup
        {
            setup.move_row(delta);
        }
    }

    /// Change the focused row's value (`+1`/`-1`, wrapping). Reuses the exact
    /// preset-application rules from PR Triage's `pr_review_triage_setup_adjust`.
    pub fn review_feature_setup_adjust(&mut self, delta: isize) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(setup) = &mut state.review_feature_setup
        {
            setup.adjust(delta);
        }
    }

    /// Append a character to the branch row (only when it's focused).
    pub fn review_feature_setup_branch_push(&mut self, c: char) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(setup) = &mut state.review_feature_setup
        {
            setup.branch_push(c);
        }
    }

    /// Delete the last character of the branch row (only when it's focused).
    pub fn review_feature_setup_branch_backspace(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(setup) = &mut state.review_feature_setup
        {
            setup.branch_backspace();
        }
    }

    /// Abandon the setup overlay. The destination is left unresolved (the picker
    /// closed on the way in), so the footer still shows whatever was chosen
    /// before, and the next `t` re-offers every option.
    pub fn review_feature_setup_cancel(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.review_feature_setup = None;
        }
    }

    /// Create the companion feature from the overlay's settings and point the
    /// destination at it. Validation failures stay inline in the overlay.
    pub fn review_feature_setup_confirm(&mut self) -> Result<()> {
        let Some(setup) = (match &self.mode {
            AppMode::DiffViewer(state) => state.review_feature_setup.clone(),
            _ => return Ok(()),
        }) else {
            return Ok(());
        };

        match self.create_review_companion_feature(&setup) {
            Ok((feature_name, feature_id)) => {
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.review_feature_setup = None;
                    state.fix_target = FixTarget::NewFeature;
                    state.fix_target_feature_id = Some(feature_id);
                    state.review_harness = Some(setup.agent());
                }
                self.push_toast_success(format!(
                    "Companion feature '{feature_name}' created ({}, {}) — fixes will run there",
                    setup.agent().display_name(),
                    setup.mode.display_name()
                ));
                Ok(())
            }
            Err(err) => {
                if let AppMode::DiffViewer(state) = &mut self.mode
                    && let Some(setup) = &mut state.review_feature_setup
                {
                    setup.error = Some(err.to_string());
                }
                Ok(())
            }
        }
    }

    /// Create the isolated worktree, register the feature with its
    /// [`ReviewSource`] link, start its `Final Review` agent session, and return
    /// `(name, id)`. Errors are returned (not shown) so the caller keeps them
    /// inline in the overlay. Mirrors `create_triage_feature`.
    fn create_review_companion_feature(
        &mut self,
        setup: &TriageFeatureSetupState,
    ) -> Result<(String, String)> {
        let (pi, fi) = self
            .review_source_indices()
            .ok_or_else(|| anyhow::anyhow!("could not resolve the feature under review"))?;
        let source_id = self.store.projects[pi].features[fi].id.clone();
        let repo = self.store.projects[pi].repo.clone();
        let hook = merge_project_extension_config(&self.config.extension, &repo)
            .lifecycle_hooks
            .on_worktree_created;
        let created = self.register_review_companion(
            &source_id,
            &ReviewCompanionSpec {
                branch: setup.branch.clone(),
                agent: setup.agent(),
                mode: setup.mode.clone(),
                review: setup.review,
                enable_chrome: setup.enable_chrome,
            },
            hook.as_ref(),
            None,
            false,
            None,
        )?;
        let (pi, fi) = self
            .store
            .locate_feature_by_id(Some(&created.project_id), &created.feature_id)
            .ok_or_else(|| anyhow::anyhow!("companion feature was removed"))?;
        self.ensure_feature_running(pi, fi, StartIntent::Warn("the review agent"))?;
        self.save()?;
        Ok((created.name, created.feature_id))
    }

    /// Create and register a stopped companion, retaining its stable identities
    /// before an adapter chooses whether to start it. The exact hook is supplied
    /// by the caller's confirmed config snapshot, rather than read again after
    /// validation. The TUI retains its existing noninteractive hook policy.
    pub(crate) fn register_review_companion(
        &mut self,
        source_id: &str,
        spec: &ReviewCompanionSpec,
        hook: Option<&crate::extension::HookConfig>,
        hook_choice: Option<&str>,
        require_hook_choice: bool,
        expected_base_sha: Option<&str>,
    ) -> Result<CreatedReviewCompanion> {
        let plan = self.plan_review_companion(
            source_id,
            spec,
            hook,
            hook_choice,
            require_hook_choice,
            expected_base_sha,
        )?;
        let workdir = self.worktree.create_from(
            &plan.repo,
            &worktree_name(&plan.project_name, &plan.spec.branch),
            &plan.spec.branch,
            &plan.base_sha,
        )?;
        let mut created = self.register_created_review_companion(&plan, workdir.clone())?;
        created.hook_warning = hook.and_then(|hook| {
            let (ok, detail) = Self::run_worktree_hook_sync(hook.script(), &workdir, hook_choice);
            (!ok).then(|| format!("Companion created, but its worktree hook failed: {}. The companion is kept for recovery.", detail.unwrap_or_else(|| "unknown error".into())))
        });
        if let Some(warning) = &created.hook_warning {
            self.log_warn("review", warning.clone());
            self.push_toast_warning(warning.clone());
        }
        Ok(created)
    }

    pub(crate) fn plan_review_companion(
        &self,
        source_id: &str,
        spec: &ReviewCompanionSpec,
        hook: Option<&crate::extension::HookConfig>,
        hook_choice: Option<&str>,
        require_hook_choice: bool,
        expected_base_sha: Option<&str>,
    ) -> Result<ReviewCompanionRegistration> {
        let branch = spec.branch.trim();
        if branch.is_empty() {
            anyhow::bail!("Branch name cannot be empty");
        }
        let (pi, fi) = self
            .store
            .locate_feature_by_id(None, source_id)
            .ok_or_else(|| anyhow::anyhow!("The reviewed feature was deleted"))?;
        let project = &self.store.projects[pi];
        let source = &project.features[fi];
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        let repo = project.repo.clone();
        let source_branch = source.branch.clone();
        let source_workdir = source.workdir.clone();
        if !project.is_git && self.worktree.repo_root(&repo).is_err() {
            anyhow::bail!("A companion review feature needs a git repository");
        }
        if source_branch.trim().is_empty() || !branch_exists(&source_workdir, &source_branch) {
            anyhow::bail!("The reviewed branch no longer exists");
        }
        if !self.allows_agent_for_repo(&repo, &spec.agent) {
            anyhow::bail!(
                "Harness '{}' is not allowed for this workspace",
                spec.agent.display_name()
            );
        }
        self.ensure_agent_mode_supported(&spec.agent, &spec.mode)?;
        if project.features.iter().any(|feature| {
            normalized_feature_name(&feature.name) == normalized_feature_name(branch)
        }) || branch_exists(&source_workdir, branch)
        {
            anyhow::bail!("Feature or branch '{branch}' already exists");
        }
        if require_hook_choice {
            super::automation::checked_hook_choice(
                hook.and_then(|hook| hook.prompt()),
                hook_choice,
            )?;
        }
        let base_sha = rev_parse(&source_workdir, &source_branch)
            .ok_or_else(|| anyhow::anyhow!("Cannot resolve the reviewed branch head"))?;
        if expected_base_sha.is_some_and(|expected| expected != base_sha) {
            anyhow::bail!(
                "The reviewed branch head changed. Refresh the companion preview before creating it"
            );
        }
        Ok(ReviewCompanionRegistration {
            project_id,
            project_name,
            repo,
            source_id: source_id.into(),
            source_branch,
            source_workdir,
            base_sha,
            spec: ReviewCompanionSpec {
                branch: branch.into(),
                ..spec.clone()
            },
        })
    }

    pub(crate) fn register_created_review_companion(
        &mut self,
        plan: &ReviewCompanionRegistration,
        workdir: std::path::PathBuf,
    ) -> Result<CreatedReviewCompanion> {
        let ReviewCompanionRegistration {
            project_id,
            project_name,
            repo,
            source_id,
            source_branch,
            source_workdir,
            spec,
            ..
        } = plan;
        let branch = spec.branch.as_str();
        let (pi, _) = self
            .store
            .locate_feature_by_id(Some(project_id), source_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Source removed; companion checkout retained at {}",
                    workdir.display()
                )
            })?;
        let mut feature = Feature::new_for_project(
            &project_name,
            branch.to_string(),
            branch.to_string(),
            workdir.clone(),
            true,
            spec.mode.clone(),
            spec.review,
            false,
            spec.agent.clone(),
            spec.enable_chrome,
            false,
        );
        let session_id = feature
            .add_session_named(
                crate::app::session_kind_for_agent(&spec.agent),
                FINAL_REVIEW_SESSION_LABEL.into(),
            )
            .id
            .clone();
        feature.status = ProjectStatus::Stopped;
        feature.review_source = Some(ReviewSource {
            source_feature_id: source_id.into(),
            target_branch: source_branch.clone(),
            base_sha: plan.base_sha.clone(),
        });
        let feature_id = feature.id.clone();
        self.store.projects[pi].features.push(feature.clone());
        let saved = self.save_reapplying(|store| {
            let Some((pi, fi)) = store.locate_feature_by_id(Some(&project_id), source_id) else {
                return false;
            };
            let project = &mut store.projects[pi];
            if &project.repo != repo
                || &project.features[fi].workdir != source_workdir
                || &project.features[fi].branch != source_branch
            {
                return false;
            }
            if project.features.iter().any(|existing| {
                existing.id != feature_id
                    && normalized_feature_name(&existing.name) == normalized_feature_name(branch)
            }) {
                return false;
            }
            if !project
                .features
                .iter()
                .any(|existing| existing.id == feature_id)
            {
                project.features.push(feature.clone());
            }
            true
        });
        if !matches!(saved, Ok(crate::app::ReapplyOutcome::Saved)) {
            for project in &mut self.store.projects {
                project.features.retain(|feature| feature.id != feature_id);
            }
            let reason = saved
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "Workspace changed before registration".into());
            anyhow::bail!(
                "{reason}. Created companion checkout retained at {} for recovery; no agent started",
                workdir.display()
            );
        }
        Ok(CreatedReviewCompanion {
            project_id: project_id.clone(),
            feature_id,
            session_id,
            name: branch.into(),
            hook_warning: None,
        })
    }

    /// Validate every setup document a destination start can read, including
    /// existing feature agents brought up by the shared session-start engine.
    pub(crate) fn preflight_feedback_agent_setup(
        &self,
        feature_id: &str,
        next: &crate::project::AgentKind,
    ) -> Result<()> {
        let (pi, fi) = self
            .store
            .locate_feature_by_id(None, feature_id)
            .ok_or_else(|| anyhow::anyhow!("Destination feature was deleted"))?;
        let project = &self.store.projects[pi];
        let feature = &project.features[fi];
        let mut agents = feature
            .sessions
            .iter()
            .filter_map(|session| crate::app::session_ops::agent_for_session_kind(&session.kind))
            .collect::<Vec<_>>();
        if !agents.contains(next) {
            agents.push(next.clone());
        }
        for agent in agents {
            super::setup::preflight_harness_change_setup(
                &feature.workdir,
                &project.repo,
                &[],
                &agent,
            )?;
        }
        Ok(())
    }

    /// A companion branch name not already taken by a feature or a git branch:
    /// `<source>-review-fixes`, then `-2`, `-3`, …
    pub(crate) fn unique_review_branch(
        &self,
        source_workdir: &std::path::Path,
        source_branch: &str,
    ) -> String {
        let base = format!("{source_branch}{REVIEW_BRANCH_SUFFIX}");
        let taken = |candidate: &str| -> bool {
            let normalized = normalized_feature_name(candidate);
            let name_taken = self
                .feature_indices_for_workdir(source_workdir)
                .map(|(pi, _)| {
                    self.store.projects[pi]
                        .features
                        .iter()
                        .any(|f| normalized_feature_name(&f.name) == normalized)
                })
                .unwrap_or(false);
            name_taken || branch_exists(source_workdir, candidate)
        };
        if !taken(&base) {
            return base;
        }
        (2..100)
            .map(|n| format!("{base}-{n}"))
            .find(|candidate| !taken(candidate))
            .unwrap_or(base)
    }

    // ------------------------------------------------------- integration

    /// Open the integration overlay for the selected companion review feature
    /// (dashboard `t`). Only meaningful for a feature carrying a
    /// [`ReviewSource`] link; the other destinations commit straight onto the
    /// feature's own branch and need no integration step.
    pub fn open_review_integrate(&mut self) {
        let (pi, fi) = match self.selection {
            crate::app::Selection::Feature(pi, fi) | crate::app::Selection::Session(pi, fi, _) => {
                (pi, fi)
            }
            crate::app::Selection::Project(_) => return,
        };
        let Some(feature) = self.store.projects.get(pi).and_then(|p| p.features.get(fi)) else {
            return;
        };
        // Silent no-op for an ordinary feature: `t` is a common key and only
        // means "integrate" for a companion the final review created.
        let Some(link) = feature.review_source.clone() else {
            return;
        };
        let companion_feature_id = feature.id.clone();
        let companion_workdir = feature.workdir.clone();
        let companion_branch = feature.branch.clone();

        // The source worktree the cherry-pick would land in.
        let source_workdir = self
            .feature_indices_by_id(&link.source_feature_id)
            .map(|(spi, sfi)| self.store.projects[spi].features[sfi].workdir.clone());

        let commits = commits_since(
            &companion_workdir,
            &link.base_sha,
            Some(INTEGRATE_COMMIT_PREVIEW),
        );
        let source_dirty = source_workdir.as_ref().and_then(|wd| {
            worktree_is_dirty(wd).then(|| {
                format!(
                    "the source worktree has uncommitted changes — commit or stash them in `{}` first",
                    crate::app::util::shorten_path(wd)
                )
            })
        });
        let source_missing = source_workdir.is_none();
        let triage_dirty = worktree_is_dirty(&companion_workdir);

        self.mode = AppMode::ReviewIntegrate(TriageIntegrateState {
            triage_branch: companion_branch,
            pr_branch: link.target_branch,
            commits,
            source_dirty: source_dirty.or_else(|| {
                source_missing.then(|| "the source feature no longer exists".to_string())
            }),
            triage_dirty,
            selected: 0,
            error: None,
            done: None,
            companion_feature_id: Some(companion_feature_id),
        });
    }

    /// Move the integration overlay's highlight (`+1`/`-1`, wrapping).
    pub fn review_integrate_move(&mut self, delta: isize) {
        if let AppMode::ReviewIntegrate(state) = &mut self.mode {
            let len = TriageIntegration::ALL.len() as isize;
            state.selected = ((state.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Close the integration overlay.
    pub fn review_integrate_cancel(&mut self) {
        if matches!(&self.mode, AppMode::ReviewIntegrate(_)) {
            self.mode = AppMode::Normal;
        }
    }

    /// Run the highlighted integration. Both paths are non-destructive: the push
    /// is never forced, and the cherry-pick refuses a dirty source worktree.
    /// Results stay in the overlay.
    pub fn review_integrate_confirm(&mut self) -> Result<()> {
        let Some((choice, companion_branch, target_branch, blocked, companion_feature_id)) =
            (match &self.mode {
                AppMode::ReviewIntegrate(state) => Some((
                    state.focused(),
                    state.triage_branch.clone(),
                    state.pr_branch.clone(),
                    state.source_dirty.clone(),
                    state.companion_feature_id.clone(),
                )),
                _ => None,
            })
        else {
            return Ok(());
        };

        if choice == TriageIntegration::CherryPick
            && let Some(reason) = blocked
        {
            self.set_review_integrate_error(format!("Cherry-pick refused: {reason}"));
            return Ok(());
        }

        // Re-resolve the companion by the feature id captured when the overlay
        // opened — never by a branch-name scan. Two projects can each review a
        // feature of the same name, producing companion branches with the same
        // generated name (`<branch>-review-fixes`); a scan could then match the
        // wrong project's feature and run the push / cherry-pick against the
        // wrong repo.
        let Some((pi, fi)) = companion_feature_id
            .as_deref()
            .and_then(|id| self.feature_indices_by_id(id))
        else {
            self.set_review_integrate_error("The companion feature no longer exists".to_string());
            return Ok(());
        };
        let (companion_workdir, base_sha, source_id) = {
            let feature = &self.store.projects[pi].features[fi];
            (
                feature.workdir.clone(),
                feature
                    .review_source
                    .as_ref()
                    .map(|l| l.base_sha.clone())
                    .unwrap_or_default(),
                feature
                    .review_source
                    .as_ref()
                    .map(|l| l.source_feature_id.clone())
                    .unwrap_or_default(),
            )
        };
        let source_workdir = self
            .feature_indices_by_id(&source_id)
            .map(|(spi, sfi)| self.store.projects[spi].features[sfi].workdir.clone());

        let outcome = match choice {
            TriageIntegration::Push => {
                push_branch(&companion_workdir, &companion_branch, &target_branch)
            }
            TriageIntegration::CherryPick => match source_workdir {
                Some(sw) => {
                    cherry_pick_range(&sw, &companion_workdir, &base_sha, &companion_branch)
                }
                None => Err(anyhow::anyhow!("the source feature no longer exists")),
            },
        };

        match outcome {
            Ok(summary) => {
                if let AppMode::ReviewIntegrate(state) = &mut self.mode {
                    state.error = None;
                    state.done = Some(summary.clone());
                }
                self.log_info("review", format!("Companion integration: {summary}"));
                self.push_toast_success(summary);
            }
            Err(err) => self.set_review_integrate_error(err.to_string()),
        }
        Ok(())
    }

    fn set_review_integrate_error(&mut self, message: String) {
        self.log_warn("review", format!("Companion integration failed: {message}"));
        if let AppMode::ReviewIntegrate(state) = &mut self.mode {
            state.error = Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Selection;
    use crate::app::state::TriageIntegration;
    use crate::project::{AgentKind, Project, ProjectStore};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::collections::HashMap;

    fn companion(project: &str, branch: &str, source_id: &str) -> Feature {
        let mut f = Feature::new(
            branch.to_string(),
            branch.to_string(),
            std::path::PathBuf::from(format!("/tmp/{project}/{branch}")),
            true,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        f.review_source = Some(ReviewSource {
            source_feature_id: source_id.to_string(),
            target_branch: "feat".to_string(),
            base_sha: "deadbeef".to_string(),
        });
        f
    }

    /// The integration overlay must re-resolve its companion by the feature id
    /// captured when it opened — never by a branch-name scan across every
    /// project, which collides when two repos each review a same-named feature
    /// (both companion branches are then called `feat-review-fixes`).
    #[test]
    fn review_integrate_resolves_the_companion_by_id_not_a_cross_project_branch_scan() {
        let mut p0 = Project::new("p0".into(), "/tmp/p0".into(), true, AgentKind::Claude);
        p0.features
            .push(companion("p0", "feat-review-fixes", "p0-src"));
        let mut p1 = Project::new("p1".into(), "/tmp/p1".into(), true, AgentKind::Claude);
        p1.features
            .push(companion("p1", "feat-review-fixes", "p1-src"));

        let store = ProjectStore {
            version: 2,
            projects: vec![p0, p1],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        };
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );

        // Open the overlay for *project 1's* companion.
        app.selection = Selection::Feature(1, 0);
        app.open_review_integrate();
        let p1_companion_id = app.store.projects[1].features[0].id.clone();
        match &app.mode {
            AppMode::ReviewIntegrate(state) => assert_eq!(
                state.companion_feature_id.as_deref(),
                Some(p1_companion_id.as_str())
            ),
            _ => panic!("integration overlay should be open"),
        }

        // Project 1's companion is deleted out from under the open overlay.
        app.store.projects[1].features.remove(0);

        // Confirming must fail cleanly, not fall through to project 0's
        // identically-named companion branch.
        if let AppMode::ReviewIntegrate(state) = &mut app.mode {
            state.selected = TriageIntegration::ALL
                .iter()
                .position(|o| *o == TriageIntegration::Push)
                .unwrap();
        }
        app.review_integrate_confirm().unwrap();
        match &app.mode {
            AppMode::ReviewIntegrate(state) => {
                assert!(
                    state
                        .error
                        .as_deref()
                        .is_some_and(|e| e.contains("no longer exists")),
                    "got {:?}",
                    state.error
                );
                assert!(state.done.is_none());
            }
            _ => panic!("still in the overlay"),
        }
    }
    #[test]
    fn tui_companion_registration_keeps_failed_hook_warning_visible_and_exact_base() {
        let (dir, mut gui, source) = crate::gui_diff::tests::fixture();
        let repo = dir.path().join("repo");
        let expected = rev_parse(&repo, "feature").unwrap();
        let checkout = dir.path().join("companion");
        let mut worktree = MockWorktreeOps::new();
        let expected_worker = expected.clone();
        worktree
            .expect_create_from()
            .times(1)
            .returning(move |_, _, branch, base| {
                assert_eq!(base, expected_worker);
                let output = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(["worktree", "add", "-b", branch])
                    .arg(&checkout)
                    .arg(base)
                    .output()?;
                anyhow::ensure!(output.status.success(), "fixture checkout failed");
                Ok(checkout.clone())
            });
        let app = gui.app_for_workflow();
        app.worktree = std::sync::Arc::new(worktree);
        let created = app
            .register_review_companion(
                &source.feature_id,
                &ReviewCompanionSpec {
                    branch: "tui-review-fixes".into(),
                    agent: AgentKind::Claude,
                    mode: VibeMode::SuperVibe,
                    review: false,
                    enable_chrome: false,
                },
                Some(&crate::extension::HookConfig::Script(
                    "printf 'offline setup failed' >&2; exit 9".into(),
                )),
                None,
                false,
                None,
            )
            .unwrap();
        assert!(
            created
                .hook_warning
                .as_deref()
                .unwrap()
                .contains("offline setup failed")
        );
        assert!(
            app.toasts
                .iter()
                .any(|toast| toast.message.contains("offline setup failed")
                    && matches!(toast.kind, crate::app::ToastKind::Warning))
        );
        let (pi, fi) = app
            .store
            .locate_feature_by_id(None, &created.feature_id)
            .unwrap();
        let feature = &app.store.projects[pi].features[fi];
        assert_eq!(feature.status, ProjectStatus::Stopped);
        assert_eq!(feature.review_source.as_ref().unwrap().base_sha, expected);
    }
}
