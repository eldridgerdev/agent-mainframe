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
    TriageIntegrateState, TriageIntegration, TriageSetupRow,
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
                    .is_some_and(|s| s.focused_row() == TriageSetupRow::Branch)
        )
    }

    /// Move the setup overlay's focused row (`+1`/`-1`, wrapping).
    pub fn review_feature_setup_move(&mut self, delta: isize) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(setup) = &mut state.review_feature_setup
        {
            let len = TriageSetupRow::ALL.len() as isize;
            setup.row = ((setup.row as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Change the focused row's value (`+1`/`-1`, wrapping). Reuses the exact
    /// preset-application rules from PR Triage's `pr_review_triage_setup_adjust`.
    pub fn review_feature_setup_adjust(&mut self, delta: isize) {
        let Some(applied_preset) = (match &mut self.mode {
            AppMode::DiffViewer(state) => state.review_feature_setup.as_mut().map(|setup| {
                setup.error = None;
                match setup.focused_row() {
                    TriageSetupRow::Preset => {
                        let len = setup.presets.len() as isize + 1;
                        setup.preset_index =
                            ((setup.preset_index as isize + delta).rem_euclid(len)) as usize;
                        setup.selected_preset().cloned()
                    }
                    TriageSetupRow::Harness => {
                        let len = setup.agents.len().max(1) as isize;
                        setup.agent_index =
                            ((setup.agent_index as isize + delta).rem_euclid(len)) as usize;
                        None
                    }
                    TriageSetupRow::Mode => {
                        let all = VibeMode::ALL;
                        let current =
                            all.iter().position(|m| *m == setup.mode).unwrap_or(0) as isize;
                        let next = (current + delta).rem_euclid(all.len() as isize) as usize;
                        setup.mode = all[next].clone();
                        None
                    }
                    TriageSetupRow::Review => {
                        setup.review = !setup.review;
                        None
                    }
                    TriageSetupRow::Chrome => {
                        setup.enable_chrome = !setup.enable_chrome;
                        None
                    }
                    TriageSetupRow::Branch => None,
                }
            }),
            _ => None,
        }) else {
            return;
        };

        if let Some(preset) = applied_preset
            && let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(setup) = &mut state.review_feature_setup
        {
            if let Some(idx) = setup.agents.iter().position(|a| *a == preset.agent) {
                setup.agent_index = idx;
            }
            setup.mode = preset.mode.clone();
            setup.review = preset.review;
            setup.enable_chrome = preset.enable_chrome;
            if let Some(prefix) = &preset.branch_prefix {
                let base = setup
                    .branch
                    .rsplit('/')
                    .next()
                    .unwrap_or(&setup.branch)
                    .to_string();
                setup.branch = format!("{prefix}{base}");
            }
        }
    }

    /// Append a character to the branch row (only when it's focused).
    pub fn review_feature_setup_branch_push(&mut self, c: char) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(setup) = &mut state.review_feature_setup
            && setup.focused_row() == TriageSetupRow::Branch
        {
            setup.error = None;
            setup.branch.push(c);
        }
    }

    /// Delete the last character of the branch row (only when it's focused).
    pub fn review_feature_setup_branch_backspace(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(setup) = &mut state.review_feature_setup
            && setup.focused_row() == TriageSetupRow::Branch
        {
            setup.error = None;
            setup.branch.pop();
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
        let branch = setup.branch.trim().to_string();
        if branch.is_empty() {
            anyhow::bail!("Branch name cannot be empty");
        }

        let (pi, source_fi) = self
            .review_source_indices()
            .ok_or_else(|| anyhow::anyhow!("could not resolve the feature under review"))?;

        let project_name = self.store.projects[pi].name.clone();
        let project_repo = self.store.projects[pi].repo.clone();
        let source = &self.store.projects[pi].features[source_fi];
        let source_feature_id = source.id.clone();
        let source_branch = source.branch.clone();
        let source_workdir = source.workdir.clone();

        if !self.store.projects[pi].is_git && self.worktree.repo_root(&project_repo).is_err() {
            anyhow::bail!(
                "A companion review feature needs a git repository (it runs in its own worktree)"
            );
        }
        if source_branch.trim().is_empty() {
            anyhow::bail!("the feature under review has no branch to seed the companion from");
        }

        let agent = setup.agent();
        if !self.allows_agent_for_repo(&project_repo, &agent) {
            anyhow::bail!(
                "Harness '{}' is not allowed for this workspace",
                agent.display_name()
            );
        }
        self.ensure_agent_mode_supported(&agent, &setup.mode)?;

        let normalized = normalized_feature_name(&branch);
        if self.store.projects[pi]
            .features
            .iter()
            .any(|f| normalized_feature_name(&f.name) == normalized)
        {
            anyhow::bail!("Feature '{branch}' already exists in '{project_name}'");
        }

        // Seed from the reviewed feature's branch head.
        let base = source_branch.clone();
        if !branch_exists(&source_workdir, &base) {
            anyhow::bail!("branch '{base}' not found to seed the companion from");
        }
        let base_sha = rev_parse(&source_workdir, &base).unwrap_or_else(|| base.clone());

        let wt_name = worktree_name(&project_name, &branch);
        let workdir = self
            .worktree
            .create_from(&project_repo, &wt_name, &branch, &base)?;

        // Run the on-worktree-created hook synchronously — the interactive
        // prompt flow would have to unwind the review viewer.
        let ext = merge_project_extension_config(&self.config.extension, &project_repo);
        if let Some(hook) = ext.lifecycle_hooks.on_worktree_created.as_ref() {
            let (ok, detail) = Self::run_worktree_hook_sync(hook.script(), &workdir, None);
            if !ok {
                self.log_warn(
                    "review",
                    format!(
                        "Worktree hook failed for companion review feature '{branch}': {}",
                        detail.unwrap_or_else(|| "unknown error".to_string())
                    ),
                );
                self.push_toast_warning(
                    "Companion worktree created, but its on_worktree_created hook failed",
                );
            }
        }

        let mut feature = Feature::new_for_project(
            &project_name,
            branch.clone(),
            branch.clone(),
            workdir,
            true,
            setup.mode.clone(),
            setup.review,
            // Plan mode would defer the launch into a planning interview; this
            // feature exists to apply review feedback that already says what to do.
            false,
            agent.clone(),
            setup.enable_chrome,
            false,
        );
        // The primary session carries the `Final Review` label, so
        // `fix_session_index(.., FINAL_REVIEW_SESSION_LABEL)` finds it inside
        // the companion the same way it does for the in-feature dedicated target.
        feature.add_session_named(
            crate::app::session_kind_for_agent(&agent),
            FINAL_REVIEW_SESSION_LABEL.to_string(),
        );
        feature.status = ProjectStatus::Stopped;
        feature.review_source = Some(ReviewSource {
            source_feature_id,
            target_branch: source_branch,
            base_sha,
        });
        let feature_name = feature.name.clone();
        let feature_id = feature.id.clone();

        self.store.add_feature(&project_name, feature);
        self.save()?;

        let fi = self.store.projects[pi]
            .features
            .iter()
            .position(|f| f.id == feature_id)
            .ok_or_else(|| anyhow::anyhow!("companion feature missing after add"))?;
        self.store.projects[pi].collapsed = false;
        self.ensure_feature_running(pi, fi, StartIntent::Warn("the review agent"))?;
        self.save()?;
        self.log_info(
            "review",
            format!(
                "Created companion review feature '{feature_name}' from '{base}' \
                 ({} / {})",
                agent.display_name(),
                setup.mode.display_name()
            ),
        );
        Ok((feature_name, feature_id))
    }

    /// A companion branch name not already taken by a feature or a git branch:
    /// `<source>-review-fixes`, then `-2`, `-3`, …
    fn unique_review_branch(
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
}
