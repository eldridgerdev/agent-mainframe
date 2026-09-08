use super::{
    BATCH_COMBINED_COMMENT_WARN, BATCH_COMBINED_TOKEN_WARN, FixTarget, FixTargetPickRow, PrComment,
    ReplyDraftRequest, TRIAGE_SESSION_LABEL, TriageState, combined_fix_prompt, estimate_tokens,
    investigation_findings_for_prompt, new_fix_confirm, pr_triage_session_index,
    pr_triage_session_index_named_for_harness, with_reply_draft_handoff,
};
use crate::app::StartIntent;
use crate::app::{App, AppMode, Feature, HarnessPickState, PrReviewReturn, Selection, SessionKind};
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
impl App {
    /// Set `fix_target`, marking the fix-target picker resolved for the rest
    /// of this pane visit, and snapshot the newly-targeted session's current
    /// usage as a baseline if it doesn't already have one — so the "this
    /// visit" tally starts from zero for the just-selected target rather than
    /// including whatever that session had accrued before this pane opened.
    pub(super) fn pr_review_set_fix_target(&mut self, target: FixTarget) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.fix_target = target;
            state.fix_target_picked = true;
        } else {
            return;
        }
        // Read the baseline *after* the target is set: for the companion
        // (`New feature…`) target the usage lookup has to resolve against the
        // triage feature, which only `fix_target` identifies.
        let baseline = self.pr_review_fix_session_usage();
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(usage) = baseline
        {
            state
                .usage_baselines
                .entry(usage.source.clone())
                .or_insert(usage);
        }
    }

    /// Open the fix confirm/edit dialog for the selected comment. Assembles the
    /// minimal fix prompt and shows it for review (with a `~N tokens` preview)
    /// before anything reaches the agent — nothing is injected until the user
    /// confirms. Editing is opt-in (`e`) from the dialog.
    ///
    /// The first fix/batch of a pane visit first opens the fix-target picker
    /// (existing live session, or a dedicated session on a chosen harness) —
    /// the fix confirm follows once the user picks. Subsequent fixes (target
    /// already chosen, or a dedicated session already exists) go straight to
    /// the dialog.
    pub fn pr_review_open_fix_confirm(&mut self) {
        let selected_actionable = match &self.mode {
            AppMode::PrReview(state) => state.selected_comment().map(PrComment::is_actionable),
            _ => return,
        };
        match selected_actionable {
            Some(true) => {}
            Some(false) => {
                self.message =
                    Some("AMF follow-up replies cannot be sent back as fixes".to_string());
                return;
            }
            None => {
                self.message = Some("No comment selected".into());
                return;
            }
        }
        if self.pr_review_needs_harness_pick() {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.pending_batch = false;
            }
            self.pr_review_open_harness_pick();
            return;
        }
        self.pr_review_show_fix_confirm();
    }

    /// Build and open the single-comment fix confirm dialog for the selected
    /// comment. Assumes any harness pick has already happened (callers gate it),
    /// so it never re-opens the picker.
    pub(super) fn pr_review_show_fix_confirm(&mut self) {
        let AppMode::PrReview(state) = &mut self.mode else {
            return;
        };
        state.pending_batch = false;
        let Some(comment) = state.selected_comment() else {
            self.message = Some("No comment selected".into());
            return;
        };
        if !comment.is_actionable() {
            self.message = Some("AMF follow-up replies cannot be sent back as fixes".to_string());
            return;
        }
        let request = ReplyDraftRequest::new(comment.id, &state.review.pr.head_sha);
        let mut base = comment.fix_prompt();
        // If a read-only investigation of this comment already finished, hand
        // its findings to the fixing agent as a starting point.
        if let Some(findings) = state
            .investigations
            .iter()
            .find(|r| r.comment_id == comment.id)
            .and_then(investigation_findings_for_prompt)
        {
            base.push_str(
                "\n\n--- A read-only investigation of this comment already ran. Use its \
                 findings as a starting point, but verify them: ---\n",
            );
            base.push_str(&findings);
        }
        let prompt =
            with_reply_draft_handoff(base, state.review.pr.number, std::slice::from_ref(&request));
        let vim = state.fix_vim_enabled;
        state.fix_confirm = Some(new_fix_confirm(prompt, vim, None, vec![request]));
    }

    /// Open the **combined-batch** confirm dialog (`B`): assemble one numbered
    /// prompt from every marked, not-yet-resolved comment and show it (with a
    /// `~N tokens` preview and editing) before injecting it once into the
    /// dedicated triage session — the "fix all of these, then I'll come back"
    /// flow. Requires a non-empty marked set (`space` to mark); like a single
    /// fix, the first fix of a dedicated-review PR picks the harness first.
    pub fn pr_review_open_batch_confirm(&mut self) {
        // A marked, not-all-resolved set is required before we touch the harness
        // picker or build anything.
        let valid = match &self.mode {
            AppMode::PrReview(state) => {
                if state.marked.is_empty() {
                    self.message = Some("No comments marked — press space to mark".into());
                    return;
                }
                state
                    .review
                    .comments
                    .iter()
                    .filter(|c| state.marked.contains(&c.id) && c.is_actionable() && !c.is_resolved)
                    .count()
            }
            _ => return,
        };
        if valid == 0 {
            self.message = Some(
                "Marked comments are all resolved or AMF follow-up replies — nothing to batch"
                    .into(),
            );
            return;
        }
        // The dedicated-review target picks a harness before the first fix, same
        // as a single fix; route the picker's continuation back to the batch.
        if self.pr_review_needs_harness_pick() {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.pending_batch = true;
            }
            self.pr_review_open_harness_pick();
            return;
        }
        self.pr_review_show_batch_confirm();
    }

    /// Build and open the combined-batch confirm dialog. Assumes the marked set
    /// was already validated and any harness pick has happened.
    pub(super) fn pr_review_show_batch_confirm(&mut self) {
        let built = match &self.mode {
            AppMode::PrReview(state) => {
                let selected: Vec<&PrComment> = state
                    .review
                    .comments
                    .iter()
                    .filter(|c| state.marked.contains(&c.id) && c.is_actionable() && !c.is_resolved)
                    .collect();
                (!selected.is_empty()).then(|| {
                    let ids: Vec<u64> = selected.iter().map(|c| c.id).collect();
                    let requests: Vec<ReplyDraftRequest> = ids
                        .iter()
                        .copied()
                        .map(|id| ReplyDraftRequest::new(id, &state.review.pr.head_sha))
                        .collect();
                    let mut base = combined_fix_prompt(&selected);
                    // Append the findings of any completed investigation, tagged
                    // with the comment number they belong to.
                    let appendix: String = selected
                        .iter()
                        .enumerate()
                        .filter_map(|(i, c)| {
                            state
                                .investigations
                                .iter()
                                .find(|r| r.comment_id == c.id)
                                .and_then(investigation_findings_for_prompt)
                                .map(|f| {
                                    format!(
                                        "\n\nInvestigation already run for comment {}:\n{f}",
                                        i + 1
                                    )
                                })
                        })
                        .collect();
                    if !appendix.is_empty() {
                        base.push_str(
                            "\n\n--- Read-only investigations already ran for some of these \
                             comments. Use them as starting points, but verify: ---",
                        );
                        base.push_str(&appendix);
                    }
                    let prompt = with_reply_draft_handoff(base, state.review.pr.number, &requests);
                    (prompt, ids, requests)
                })
            }
            _ => return,
        };
        let Some((prompt, ids, requests)) = built else {
            self.message = Some(
                "Marked comments are all resolved or AMF follow-up replies — nothing to batch"
                    .into(),
            );
            return;
        };

        // Keep the set bounded: warn (but don't block) past the soft ceilings so
        // the user knows a single prompt this large may exceed the context window.
        let count = ids.len();
        let tokens = estimate_tokens(&prompt);
        if count > BATCH_COMBINED_COMMENT_WARN || tokens > BATCH_COMBINED_TOKEN_WARN {
            self.push_toast_warning(format!(
                "Large batch: {count} comments (~{tokens} tokens) in one prompt — may exceed the agent's context window"
            ));
        }

        if let AppMode::PrReview(state) = &mut self.mode {
            state.pending_batch = false;
            let vim = state.fix_vim_enabled;
            state.fix_confirm = Some(new_fix_confirm(prompt, vim, Some(ids), requests));
        }
    }

    /// Whether the first `f`/`B` of this pane visit should pick a fix target
    /// before injecting. Every new pane visit asks once even when the default
    /// dedicated session already exists, because the user may choose a
    /// different name to run another triage session alongside it.
    pub(super) fn pr_review_needs_harness_pick(&self) -> bool {
        let AppMode::PrReview(state) = &self.mode else {
            return false;
        };
        !state.fix_target_picked
            && state.review_harness.is_none()
            && self.feature_indices_for_workdir(&state.workdir).is_some()
    }

    /// Un-resolve the pane's fix target so the next `f`/`B` re-opens the
    /// picker. Clears every field [`Self::pr_review_needs_harness_pick`]
    /// short-circuits on, not just `fix_target_picked` — leaving
    /// `review_harness` set would keep the picker shut just as effectively.
    pub(crate) fn pr_review_clear_fix_target(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.fix_target = FixTarget::default();
            state.fix_target_picked = false;
            state.review_harness = None;
            state.dedicated_session_label = TRIAGE_SESSION_LABEL.to_string();
        }
    }

    /// Open the single-select fix-target picker: an "existing live session"
    /// row plus one "dedicated session" row per allowed harness. The
    /// highlighted row reflects the *current* `fix_target` (and, for a
    /// dedicated target, `review_harness`), so a picker reopened from a
    /// confirm dialog to inspect the destination can be confirmed without
    /// silently moving the fix; on a first open, where `fix_target` is still
    /// `FixTarget::default()` with no pinned harness, this lands on the
    /// project's preferred agent's dedicated row. No-op if no harnesses are
    /// available for a dedicated session — falls back to the
    /// existing-live-less default (dedicated, no explicit harness) and skips
    /// straight to the confirm dialog, since there'd be nothing to choose
    /// between anyway.
    pub(super) fn pr_review_open_harness_pick(&mut self) {
        let (workdir, current_target, current_harness) = match &self.mode {
            AppMode::PrReview(state) => (
                state.workdir.clone(),
                state.fix_target,
                state.review_harness.clone(),
            ),
            _ => return,
        };
        let agents = self.allowed_agents_for_project_path(&workdir);
        if agents.is_empty() {
            return self.pr_review_skip_harness_pick();
        }
        let preferred = self
            .feature_indices_for_workdir(&workdir)
            .map(|(pi, _)| self.store.projects[pi].preferred_agent.clone());
        let dedicated_default = preferred
            .and_then(|p| agents.iter().position(|a| *a == p))
            .unwrap_or(0);
        let existing_live_label =
            self.feature_indices_for_workdir(&workdir)
                .and_then(|(pi, fi)| {
                    let feature = &self.store.projects[pi].features[fi];
                    pr_triage_session_index(feature, FixTarget::ExistingLive)
                        .map(|idx| feature.sessions[idx].label.clone())
                });
        let mut rows = vec![FixTargetPickRow::ExistingLive(existing_live_label)];
        rows.extend(agents.into_iter().map(FixTargetPickRow::Dedicated));
        // The isolated option goes last: it costs a worktree and an explicit
        // integration step, so it reads as the deliberate choice rather than
        // the one the cursor lands on.
        rows.push(FixTargetPickRow::NewFeature);
        // +1: rows[0] is the ExistingLive row, so the dedicated default shifts by one.
        let dedicated_selected = dedicated_default + 1;
        // Highlight the row the fix currently points at. PR Triage never offers
        // the `ExistingFeature` row and treats it like `ExistingLive`, so both
        // map to rows[0]. A dedicated target prefers the row for its pinned
        // harness, falling back to the project default when none is set yet.
        let selected = match current_target {
            FixTarget::ExistingLive | FixTarget::ExistingFeature => 0,
            FixTarget::NewFeature => rows.len() - 1,
            FixTarget::DedicatedReview => current_harness
                .and_then(|h| {
                    rows.iter()
                        .position(|r| matches!(r, FixTargetPickRow::Dedicated(a) if *a == h))
                })
                .unwrap_or(dedicated_selected),
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.harness_pick = Some(HarnessPickState {
                rows,
                selected,
                session_name: None,
            });
        }
    }

    /// Skip the fix-target picker (e.g. no harnesses available): continue
    /// straight to the confirm dialog, leaving `fix_target`/`review_harness`
    /// at their defaults, but still marking the pick resolved so it isn't
    /// re-offered on the next fix.
    pub(super) fn pr_review_skip_harness_pick(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.harness_pick = None;
            state.fix_target_picked = true;
        }
        self.pr_review_continue_after_harness();
    }

    /// After the fix target is chosen (or skipped), open the dialog the
    /// pending action wanted: the combined-batch confirm for the `B` flow,
    /// otherwise the single-comment fix confirm. Neither re-checks the pick,
    /// so this can't loop back into the picker.
    pub(crate) fn pr_review_continue_after_harness(&mut self) {
        // A target change from an already-open confirmation dialog must retain
        // the exact prompt the user reviewed (and possibly edited). The picker
        // is only changing where it will be delivered, not what will be sent.
        if matches!(&self.mode, AppMode::PrReview(state) if state.fix_confirm.is_some()) {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.pending_batch = false;
            }
            return;
        }
        let batch = matches!(&self.mode, AppMode::PrReview(state) if state.pending_batch);
        if batch {
            self.pr_review_show_batch_confirm();
        } else {
            self.pr_review_show_fix_confirm();
        }
    }

    /// Whether the fix-target picker is currently open over PR Triage.
    pub fn pr_review_harness_picking(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.harness_pick.is_some()
        )
    }

    /// Move the fix-target-picker highlight (`+1`/`-1`, wrapping).
    pub fn pr_review_harness_pick_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(pick) = &mut state.harness_pick
            && !pick.rows.is_empty()
        {
            let len = pick.rows.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Confirm the fix-target picker: remember the choice (and, for a
    /// dedicated row, the harness) for the rest of this pane visit, and
    /// continue into the fix confirm dialog.
    pub fn pr_review_harness_pick_confirm(&mut self) {
        let (chosen, session_name) = match &self.mode {
            AppMode::PrReview(state) => state
                .harness_pick
                .as_ref()
                .map(|p| (p.rows.get(p.selected).cloned(), p.session_name.clone()))
                .unwrap_or((None, None)),
            _ => return,
        };
        let Some(row) = chosen else {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.harness_pick = None;
            }
            return;
        };
        match &row {
            FixTargetPickRow::ExistingLive(_) => {
                self.pr_review_set_fix_target(FixTarget::ExistingLive);
                if let AppMode::PrReview(state) = &mut self.mode {
                    state.harness_pick = None;
                    state.review_harness = None;
                }
                self.push_toast_success("Fixes target the existing live session".to_string());
            }
            FixTargetPickRow::Dedicated(agent) => {
                // Choosing a harness advances to the optional-name step. A
                // second Enter accepts the default; typing first creates (or
                // reuses) the exact named session instead.
                let Some(name) = session_name else {
                    if let AppMode::PrReview(state) = &mut self.mode
                        && let Some(pick) = &mut state.harness_pick
                    {
                        pick.session_name = Some(String::new());
                    }
                    return;
                };
                let label = match name.trim() {
                    "" => TRIAGE_SESSION_LABEL.to_string(),
                    custom => custom.to_string(),
                };
                // A name already owned by another harness cannot satisfy this
                // choice. Keep the picker open so the user can choose the
                // existing harness or edit the name instead of claiming one
                // harness will run while routing to another.
                let existing = if let Some((pi, fi)) =
                    self.feature_indices_for_workdir(match &self.mode {
                        AppMode::PrReview(state) => &state.workdir,
                        _ => return,
                    }) {
                    let feature = &self.store.projects[pi].features[fi];
                    match pr_triage_session_index_named_for_harness(
                        feature,
                        FixTarget::DedicatedReview,
                        &label,
                        Some(agent),
                    ) {
                        Ok(si) => si.map(|si| feature.sessions[si].label.clone()),
                        Err(e) => {
                            self.push_toast_error(e.to_string());
                            return;
                        }
                    }
                } else {
                    None
                };
                if let AppMode::PrReview(state) = &mut self.mode {
                    state.review_harness = Some(agent.clone());
                    state.dedicated_session_label = label.clone();
                    state.harness_pick = None;
                }
                self.pr_review_set_fix_target(FixTarget::DedicatedReview);
                let message = match existing {
                    Some(existing_label) => format!(
                        "Triage session '{existing_label}' will be reused with {}",
                        agent.display_name(),
                    ),
                    None => format!("Triage session '{label}' will run {}", agent.display_name(),),
                };
                self.push_toast_success(message);
            }
            FixTargetPickRow::NewFeature => {
                // The companion feature's settings aren't a single choice, so
                // this row hands off to the setup overlay instead of resolving
                // the target here; the overlay's confirm sets the target and
                // continues into the same fix dialog.
                let pending_batch =
                    matches!(&self.mode, AppMode::PrReview(state) if state.pending_batch);
                if let AppMode::PrReview(state) = &mut self.mode {
                    state.harness_pick = None;
                }
                self.pr_review_open_triage_feature_setup(pending_batch);
                return;
            }
        }
        // Continue into the dialog the pending action wanted (single or batch).
        self.pr_review_continue_after_harness();
    }

    /// Cancel the fix-target picker without choosing. During initial setup this
    /// aborts the pending fix; when opened from an existing confirmation dialog
    /// it simply returns to that dialog with its target unchanged.
    pub fn pr_review_harness_pick_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.harness_pick = None;
            state.pending_batch = false;
        }
    }

    /// Re-open the fix-target picker from a confirmation dialog. The dialog
    /// remains intact behind the picker, so cancelling keeps the current
    /// target and confirming a new one preserves any prompt edits.
    pub fn pr_review_change_fix_target(&mut self) {
        if !matches!(&self.mode, AppMode::PrReview(state) if state.fix_confirm.is_some()) {
            return;
        }
        self.pr_review_open_harness_pick();
    }

    /// Whether the fix-target picker is accepting the dedicated session name.
    pub fn pr_review_harness_pick_naming(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state)
                if state
                    .harness_pick
                    .as_ref()
                    .is_some_and(|pick| pick.session_name.is_some())
        )
    }

    /// Return from the optional-name step to the fix-target list.
    pub fn pr_review_harness_pick_name_back(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(pick) = &mut state.harness_pick
        {
            pick.session_name = None;
        }
    }

    pub fn pr_review_harness_pick_name_push(&mut self, c: char) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(name) = state
                .harness_pick
                .as_mut()
                .and_then(|pick| pick.session_name.as_mut())
        {
            name.push(c);
        }
    }

    pub fn pr_review_harness_pick_name_backspace(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(name) = state
                .harness_pick
                .as_mut()
                .and_then(|pick| pick.session_name.as_mut())
        {
            name.pop();
        }
    }

    pub fn pr_review_harness_pick_name_clear(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(name) = state
                .harness_pick
                .as_mut()
                .and_then(|pick| pick.session_name.as_mut())
        {
            name.clear();
        }
    }

    /// Confirm the dialog: inject the (possibly edited) prompt into the chosen
    /// agent session and switch the user into that session to watch it (no
    /// auto-advance). The dedicated triage session is spun up on first use and
    /// reused thereafter; the existing-live target reuses the feature's running
    /// agent session. Delivery goes through the shared compose / prompt-library
    /// seam: pasted without sending so the user reviews before it runs.
    ///
    /// Handles both a single-comment fix and a **combined batch** (the `B` flow,
    /// `FixConfirmState::batch`): the batch injects one numbered prompt and marks
    /// every included comment `Fixing`, then clears the marked set.
    pub fn pr_review_inject_fix(&mut self) -> Result<()> {
        let selected_is_amf_followup = matches!(
            &self.mode,
            AppMode::PrReview(state)
                if state.fix_confirm.is_none()
                    && state
                        .selected_comment()
                        .is_some_and(PrComment::is_amf_followup_reply)
        );
        if selected_is_amf_followup {
            self.message = Some("AMF follow-up replies cannot be sent back as fixes".into());
            return Ok(());
        }

        let (prompt, pr_number, head_sha, fixing_ids, is_batch, reply_draft_requests) =
            match &self.mode {
                AppMode::PrReview(state) => {
                    let pr_number = state.review.pr.number;
                    let head_sha = state.review.pr.head_sha.clone();
                    let (prompt, ids, is_batch, reply_draft_requests) = match &state.fix_confirm {
                        // Confirming the open dialog uses its edited buffer. The
                        // target ids come from the dialog's own reply-draft
                        // requests — captured when the dialog was built — rather
                        // than the *current* selection, which a PR Triage refresh
                        // received while the dialog sat open can have moved onto
                        // an unrelated comment the injected prompt never mentions.
                        Some(confirm) => {
                            let prompt = confirm.editor.text().trim().to_string();
                            let ids: Vec<u64> = confirm
                                .reply_draft_requests
                                .iter()
                                .map(|r| r.comment_id)
                                .collect();
                            (
                                prompt,
                                ids,
                                confirm.batch.is_some(),
                                confirm.reply_draft_requests.clone(),
                            )
                        }
                        // No dialog open (e.g. empty pane): fall back to the selection.
                        None => match state.selected_comment() {
                            Some(c) => {
                                let request = ReplyDraftRequest::new(c.id, &head_sha);
                                let mut base = c.fix_prompt();
                                if let Some(findings) = state
                                    .investigations
                                    .iter()
                                    .find(|r| r.comment_id == c.id)
                                    .and_then(investigation_findings_for_prompt)
                                {
                                    base.push_str(
                                        "\n\n--- A read-only investigation of this comment already \
                                         ran. Use its findings as a starting point, but verify \
                                         them: ---\n",
                                    );
                                    base.push_str(&findings);
                                }
                                let prompt = with_reply_draft_handoff(
                                    base,
                                    pr_number,
                                    std::slice::from_ref(&request),
                                );
                                (prompt, vec![c.id], false, vec![request])
                            }
                            None => {
                                self.message = Some("No comment selected".into());
                                return Ok(());
                            }
                        },
                    };
                    (
                        prompt,
                        pr_number,
                        head_sha,
                        ids,
                        is_batch,
                        reply_draft_requests,
                    )
                }
                _ => return Ok(()),
            };

        if prompt.is_empty() {
            self.message = Some("Nothing to inject — the prompt is empty".into());
            return Ok(());
        }

        let (pi, fi, si) = match self.resolve_fix_session() {
            Ok(target) => target,
            Err(e) => {
                // Every one of these errors tells the user to press a key *in
                // PR Triage* to recover ("press f and pick a target again",
                // "switch to the dedicated target (t)"), so report it over the
                // pane rather than through `show_error`, which would drop them
                // back to the dashboard and make that advice unfollowable.
                self.log_error("pr_triage", format!("Fix target: {e}"));
                self.push_toast_error(e.to_string());
                return Ok(());
            }
        };

        // Captured from the session the prompt is about to be delivered to —
        // the only point where "which agent wrote this draft" is knowable
        // without guessing.
        let provenance = self.reply_draft_provenance(pi, fi, si);
        self.begin_reply_draft_requests(pr_number, &reply_draft_requests, provenance.as_ref());

        // A combined batch (`B`) gets one shared id, stamped on every comment it
        // resolves so the single run's fix cost can be attributed to the whole
        // batch, sibling rows can be highlighted, and the posted GitHub replies
        // can note the batch. A single-comment fix carries no batch id. A UUID
        // keeps it unique across projects in the global SQLite store.
        let batch_id = is_batch.then(|| uuid::Uuid::new_v4().to_string());

        // The fix is committed: mark every targeted comment `Fixing` and persist
        // before we leave the pane, so re-opening the review (cache hit) shows
        // the state.
        for id in &fixing_ids {
            if let AppMode::PrReview(state) = &mut self.mode
                && let Some(c) = state.review.comments.iter_mut().find(|c| c.id == *id)
            {
                c.triage = TriageState::Fixing;
                // Only a real batch stamps membership. On the single-comment
                // (`f`) path `batch_id` is `None`; assigning it here would
                // clear the in-memory `batch_id` of a comment that was already
                // part of an earlier batch (the DB value survives via the
                // `COALESCE` stickiness, but the `⧉` marker and `[`/`]` jump
                // would stop working for it until the pane is re-entered).
                if is_batch {
                    c.batch_id = batch_id.clone();
                }
            }
            self.persist_triage(
                pr_number,
                &head_sha,
                *id,
                TriageState::Fixing,
                None,
                batch_id.as_deref(),
            );
        }
        // A batch consumes the marked set once it's committed.
        if is_batch {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.marked.clear();
            }
            self.push_toast_success(format!(
                "Injected a combined fix for {} comments",
                fixing_ids.len()
            ));
        }

        // Stash the pane's exact state so leader+P can jump straight back to
        // it without re-fetching — the same mechanism the `P` toggle uses.
        // Leaving the pane is still intentional (the user watches the agent),
        // but the round trip back to triage the next comment no longer has to
        // go through the dashboard and a re-resolve. The confirm dialog has
        // already served its purpose (the prompt above was read from it), so
        // clear it before stashing — otherwise returning would reopen the
        // same "inject fix" dialog instead of the plain comment list.
        let AppMode::PrReview(mut state) = std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            return Ok(());
        };
        state.fix_confirm = None;
        let feature = &self.store.projects[pi].features[fi];
        self.pr_review_return = Some(PrReviewReturn {
            session: feature.tmux_session.clone(),
            window: feature.sessions[si].tmux_window.clone(),
            state,
        });

        // Switch into the target session, then deliver the prompt via the
        // shared seam (seeds the compose box when interception is on, else
        // pastes without sending).
        self.selection = Selection::Session(pi, fi, si);
        self.enter_view_without_auto_compose()?;
        let AppMode::Viewing(view) = &self.mode else {
            return Ok(());
        };
        let view = view.clone();
        self.deliver_prompt(prompt, Some(view))
    }

    /// Jump from PR Triage straight into the linked fix session (`P`),
    /// stashing the pane's exact state (selection, scroll, open dialogs) so
    /// `pr_review_return_to_pane` can pop back to it without re-fetching.
    /// Unlike `f`, this never spins up the dedicated session — it only jumps
    /// to one that already exists, so a quick "peek at the agent" doesn't
    /// have the side effect of starting a triage session on its own.
    pub fn pr_review_toggle_to_session(&mut self) -> Result<()> {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::PrReview(state) => state,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        // Resolves to the companion triage feature under the `New feature…`
        // target, so `P` peeks at the session `f` actually targets rather than
        // the source feature's.
        let Some((pi, fi)) = self.pr_review_feature_for_target(&state) else {
            self.mode = AppMode::PrReview(state);
            self.push_toast_warning("Could not find the feature for this PR");
            return Ok(());
        };
        let feature = &self.store.projects[pi].features[fi];
        let si = match pr_triage_session_index_named_for_harness(
            feature,
            state.fix_target,
            &state.dedicated_session_label,
            state.review_harness.as_ref(),
        ) {
            Ok(Some(si)) => si,
            Ok(None) => {
                self.mode = AppMode::PrReview(state);
                self.push_toast_warning("No triage session yet — press f to start one");
                return Ok(());
            }
            Err(e) => {
                self.mode = AppMode::PrReview(state);
                self.push_toast_error(e.to_string());
                return Ok(());
            }
        };
        let session = feature.tmux_session.clone();
        let window = feature.sessions[si].tmux_window.clone();

        self.selection = Selection::Session(pi, fi, si);
        self.pr_review_return = Some(PrReviewReturn {
            session,
            window,
            state,
        });
        self.enter_view_without_auto_compose()
    }

    /// Jump back from a Viewing session to the review pane stashed by
    /// `pr_review_toggle_to_session` (`leader+P`), restoring the exact prior
    /// state — no re-fetch. Only restores when the current session is the one
    /// the stash was jumped from; a stash left behind after navigating
    /// elsewhere is not popped into an unrelated session's view.
    pub fn pr_review_return_to_pane(&mut self) {
        let Some(stash) = &self.pr_review_return else {
            self.push_toast_warning("No PR Triage pane to return to");
            return;
        };
        let matches_current = matches!(
            &self.mode,
            AppMode::Viewing(view) if view.session == stash.session && view.window == stash.window
        );
        if !matches_current {
            self.push_toast_warning("No PR Triage pane linked to this session");
            return;
        }
        if let Some(stash) = self.pr_review_return.take() {
            self.mode = AppMode::PrReview(stash.state);
        }
    }

    /// Resolve (and, for the dedicated strategy, lazily create) the agent
    /// window that fix prompts target. Returns `(project, feature, session)`
    /// indices. Ensures the feature's tmux session is running first.
    pub(super) fn resolve_fix_session(&mut self) -> Result<(usize, usize, usize)> {
        let (target, harness, dedicated_label) = match &self.mode {
            AppMode::PrReview(state) => (
                state.fix_target,
                state.review_harness.clone(),
                state.dedicated_session_label.clone(),
            ),
            _ => anyhow::bail!("not reviewing a PR"),
        };
        let (pi, fi) = match self.pr_review_target_feature() {
            Some(found) => found,
            // The companion feature was deleted out from under the pane. Drop
            // the resolved pick so `f` really does re-open the picker the way
            // the message promises — otherwise every later fix this visit
            // resolves against the same dead target.
            None if target.is_companion_feature() => {
                self.pr_review_clear_fix_target();
                anyhow::bail!(
                    "the triage feature for this PR no longer exists — press f and pick a target again"
                )
            }
            None => anyhow::bail!("could not find the feature for this PR"),
        };

        // The fix hand-off is already in motion and its target lives in the
        // pane's state, so a modal here would strand it: warn instead.
        self.ensure_feature_running_for_new_session(
            pi,
            fi,
            StartIntent::Warn("the PR triage agent"),
        )?;

        let feature = &self.store.projects[pi].features[fi];
        if let Some(si) = pr_triage_session_index_named_for_harness(
            feature,
            target,
            &dedicated_label,
            harness.as_ref(),
        )? {
            return Ok((pi, fi, si));
        }

        match target {
            // The companion feature is created with its triage session already
            // in place, but a user who removed that window still gets a
            // working `f` rather than a dead end.
            FixTarget::DedicatedReview | FixTarget::NewFeature => {
                let si = self.create_dedicated_review_session(
                    pi,
                    fi,
                    &dedicated_label,
                    harness,
                    StartIntent::Warn("the PR triage agent"),
                )?;
                Ok((pi, fi, si))
            }
            // `ExistingFeature` never reaches PR Triage's resolver (its picker
            // doesn't offer the row); handled alongside `ExistingLive` for
            // exhaustiveness.
            FixTarget::ExistingLive | FixTarget::ExistingFeature => {
                anyhow::bail!("no live agent session to reuse — switch to the dedicated target (t)")
            }
        }
    }

    /// The `(project, feature)` whose sessions the pane's current fix target
    /// resolves against: the **companion triage feature** for
    /// [`FixTarget::NewFeature`], otherwise the feature PR Triage was opened
    /// from. `None` when the feature can't be resolved — for the companion
    /// case, that means one hasn't been created for this PR yet (or was
    /// deleted). Read-only: never creates anything.
    pub(crate) fn pr_review_target_feature(&self) -> Option<(usize, usize)> {
        let AppMode::PrReview(state) = &self.mode else {
            return None;
        };
        self.pr_review_feature_for_target(state)
    }

    /// [`Self::pr_review_target_feature`] against an explicit state, so callers
    /// that already hold the pane state (or a stashed one) don't have to go
    /// through `self.mode`.
    pub(crate) fn pr_review_feature_for_target(
        &self,
        state: &crate::app::PrReviewState,
    ) -> Option<(usize, usize)> {
        if state.fix_target.is_companion_feature() {
            self.triage_feature_indices(state)
        } else {
            self.feature_indices_for_workdir(&state.workdir)
        }
    }

    /// Find the companion triage feature created for this pane's PR, in the
    /// same project as the source feature. Matched on the persisted
    /// [`TriageSource`] link — not on branch, which deliberately differs from
    /// the PR's own branch so both can be checked out at once — so re-opening
    /// the PR after a restart finds and reuses the same feature.
    pub(crate) fn triage_feature_indices(
        &self,
        state: &crate::app::PrReviewState,
    ) -> Option<(usize, usize)> {
        let (pi, source_fi) = self.feature_indices_for_workdir(&state.workdir)?;
        let source_id = self.store.projects[pi].features[source_fi].id.clone();
        let pr_number = state.review.pr.number;
        let fi = self.store.projects[pi].features.iter().position(|f| {
            f.triage_source.as_ref().is_some_and(|link| {
                link.pr_number == pr_number && link.source_feature_id == source_id
            })
        })?;
        Some((pi, fi))
    }

    /// Find the `(project, feature)` indices of the feature whose workdir
    /// matches `workdir`.
    pub(crate) fn feature_indices_for_workdir(&self, workdir: &Path) -> Option<(usize, usize)> {
        self.store.projects.iter().enumerate().find_map(|(pi, p)| {
            p.features
                .iter()
                .position(|f| f.workdir == workdir)
                .map(|fi| (pi, fi))
        })
    }

    pub(super) fn fix_session_usage_for(
        &self,
        workdir: &Path,
        target: FixTarget,
    ) -> Option<crate::token_tracking::SessionTokenUsage> {
        let (pi, fi) = self.feature_indices_for_workdir(workdir)?;
        let feature = &self.store.projects[pi].features[fi];
        let si = pr_triage_session_index(feature, target)?;
        feature.sessions[si].token_usage.clone()
    }

    pub(super) fn pr_review_initial_usage_baselines(
        &self,
        workdir: &Path,
    ) -> HashMap<crate::token_tracking::TokenUsageSource, crate::token_tracking::SessionTokenUsage>
    {
        self.fix_session_usage_for(workdir, FixTarget::default())
            .map(|usage| [(usage.source.clone(), usage)].into_iter().collect())
            .unwrap_or_default()
    }

    /// Token usage for the session the pane's current fix target resolves to,
    /// for a header display. Read-only — unlike [`App::resolve_fix_session`] it
    /// never creates a session, so this is safe to call on every frame just to
    /// render a number. `None` before any fix has spun up the target session.
    pub(crate) fn pr_review_fix_session_usage(
        &self,
    ) -> Option<crate::token_tracking::SessionTokenUsage> {
        let AppMode::PrReview(state) = &self.mode else {
            return None;
        };
        // Resolves against the companion triage feature for the `New feature…`
        // target, so the header reports what that feature's agent spent rather
        // than the source feature's unrelated session.
        let (pi, fi) = self.pr_review_feature_for_target(state)?;
        let feature = &self.store.projects[pi].features[fi];
        let si = pr_triage_session_index_named_for_harness(
            feature,
            state.fix_target,
            &state.dedicated_session_label,
            state.review_harness.as_ref(),
        )
        .ok()??;
        feature.sessions[si].token_usage.clone()
    }

    /// Whether the dedicated PR-triage session exists and is actively
    /// thinking or running a tool. Returns `None` for the existing-live target
    /// so callers never label that session's activity as dedicated. Claude and
    /// Codex activity is keyed by the AMF feature-session ID supplied by
    /// hooks/plugins; OpenCode and Pi reuse their existing sidebar and
    /// marker-based activity signals.
    pub(crate) fn pr_review_dedicated_session_working(&self) -> Option<bool> {
        let AppMode::PrReview(state) = &self.mode else {
            return None;
        };
        if state.fix_target == FixTarget::ExistingLive {
            return None;
        }
        let (pi, fi) = self.pr_review_feature_for_target(state)?;
        let feature = &self.store.projects[pi].features[fi];
        let si = pr_triage_session_index_named_for_harness(
            feature,
            state.fix_target,
            &state.dedicated_session_label,
            state.review_harness.as_ref(),
        )
        .ok()??;
        self.review_session_working(feature, si)
    }

    /// Same as [`Self::pr_review_dedicated_session_working`] but for an
    /// arbitrary feature workdir rather than the currently open PR Triage
    /// pane — used by the ambient status badge shown while `Viewing` a
    /// session whose feature has an active PR.
    pub(crate) fn dedicated_review_session_working_for_workdir(
        &self,
        workdir: &Path,
    ) -> Option<bool> {
        let (pi, fi) = self.feature_indices_for_workdir(workdir)?;
        let feature = &self.store.projects[pi].features[fi];
        let si = pr_triage_session_index(feature, FixTarget::DedicatedReview)?;
        self.review_session_working(feature, si)
    }

    pub(super) fn review_session_working(&self, feature: &Feature, si: usize) -> Option<bool> {
        let session = &feature.sessions[si];
        Some(match session.kind {
            SessionKind::Opencode => self
                .opencode_sidebar_cache
                .get(&feature.tmux_session)
                .filter(|sidebar| {
                    session
                        .token_usage_source
                        .as_ref()
                        .filter(|source| {
                            source.provider == crate::token_tracking::TokenUsageProvider::Opencode
                        })
                        .is_none_or(|source| source.id == sidebar.session_id)
                })
                .and_then(crate::app::sync::opencode_sidebar_thinking_state)
                .unwrap_or(false),
            SessionKind::Pi => Self::is_session_marked_thinking(&feature.tmux_session),
            _ => {
                self.ipc_thinking_feature_sessions.contains(&session.id)
                    || self.ipc_tool_feature_sessions.contains(&session.id)
            }
        })
    }

    /// Usage added to the selected fix target since this visit to the PR pane
    /// began. Existing sessions are snapshotted on entry (or when selected via
    /// `t`); a dedicated session created by the first fix starts from zero.
    pub(crate) fn pr_review_triage_session_usage(
        &self,
    ) -> Option<crate::token_tracking::SessionTokenUsage> {
        let AppMode::PrReview(state) = &self.mode else {
            return None;
        };
        let current = self.pr_review_fix_session_usage()?;
        let delta = state
            .usage_baselines
            .get(&current.source)
            .map(|baseline| crate::token_tracking::token_usage_delta(&current, baseline))
            .unwrap_or(current);
        (delta.input_tokens > 0
            || delta.output_tokens > 0
            || delta.cache_read_tokens > 0
            || delta.cache_write_tokens > 0
            || delta.reasoning_tokens > 0
            || delta.total_tokens > 0)
            .then_some(delta)
    }
}
