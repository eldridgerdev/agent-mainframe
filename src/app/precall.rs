//! The blocking pre-call notice shown before a **user-initiated** headless AI
//! run (Editable Headless Prompts). It announces the prompt ID and target
//! harness and offers view / edit / continue / cancel — there is no "don't
//! ask again" (a deliberate product decision).
//!
//! Automated headless runs (the Learning Mode answer queue, session
//! summaries) deliberately do **not** gate here — a modal with nobody
//! watching would stall the queue. They push a non-blocking toast instead
//! (`App::announce_headless_run`).
//!
//! ## How the gate resumes
//!
//! A gated call site resolves its prompt, then calls [`App::precall_gate`]
//! right before it would spawn its worker thread. If the run is not yet
//! cleared, the gate stashes the *originating mode* and swaps in
//! [`crate::app::AppMode::PromptPrecall`]; the caller returns without
//! spawning. On **continue**, [`App::precall_confirm`] restores the mode,
//! marks the action cleared, and re-invokes the same `start_*` method — which
//! re-resolves the prompt (so a just-saved override applies) and this time
//! sails past the gate and spawns.

use anyhow::Result;

use crate::app::{App, AppMode};
use crate::project::AgentKind;
use crate::prompts::PromptId;

/// Which gated headless call a pre-call notice belongs to. One variant per
/// user-initiated call site; [`App::dispatch_precall`] maps each back to the
/// method that starts it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrecallAction {
    PlanRound,
    PlanSynthesis,
    PlanCritique,
    PlanDirectedRevision,
    PlanInvestigation,
    ReviewWalkthrough,
    ReviewCoReview,
    ReviewChangesetOverview,
    ReviewDiffExplain,
    PrReviewAiReview,
    ReviewMemoryBootstrap,
    ReviewMemoryCompact,
}

impl PrecallAction {
    pub fn prompt_id(self) -> PromptId {
        match self {
            PrecallAction::PlanRound => PromptId::PlanInterviewRound,
            PrecallAction::PlanSynthesis => PromptId::PlanInterviewSynthesis,
            PrecallAction::PlanCritique => PromptId::PlanInterviewCritique,
            PrecallAction::PlanDirectedRevision => PromptId::PlanInterviewDirectedRevision,
            PrecallAction::PlanInvestigation => PromptId::PlanInterviewInvestigation,
            PrecallAction::ReviewWalkthrough => PromptId::ReviewWalkthrough,
            PrecallAction::ReviewCoReview => PromptId::ReviewCoReview,
            PrecallAction::ReviewChangesetOverview => PromptId::ReviewChangesetOverview,
            PrecallAction::ReviewDiffExplain => PromptId::ReviewDiffExplain,
            PrecallAction::PrReviewAiReview => PromptId::PrReviewAiReview,
            PrecallAction::ReviewMemoryBootstrap => PromptId::ReviewMemoryBootstrap,
            PrecallAction::ReviewMemoryCompact => PromptId::ReviewMemoryCompact,
        }
    }
}

/// The stashed pre-call notice.
pub struct PendingPrecall {
    pub action: PrecallAction,
    pub prompt_id: PromptId,
    pub harness: AgentKind,
    /// The rendered prompt, shown when the user presses `v`.
    pub preview: String,
    /// Whether the prompt preview is currently expanded.
    pub viewing: bool,
    pub scroll: usize,
    /// The mode the run was initiated from, restored before the run is
    /// re-dispatched (or on cancel).
    pub prior_mode: Box<AppMode>,
}

impl App {
    /// Called by a gated call site immediately before it would spawn its
    /// worker. Returns `true` when the run is cleared to proceed (the user
    /// already confirmed); returns `false` after opening the notice — the
    /// caller must then `return` without spawning.
    pub(crate) fn precall_gate(
        &mut self,
        action: PrecallAction,
        harness: &AgentKind,
        rendered_prompt: &str,
    ) -> bool {
        if self.precall_cleared == Some(action) {
            self.precall_cleared = None;
            return true;
        }
        let prior = std::mem::replace(&mut self.mode, AppMode::Normal);
        self.mode = AppMode::PromptPrecall(Box::new(PendingPrecall {
            action,
            prompt_id: action.prompt_id(),
            harness: harness.clone(),
            preview: rendered_prompt.to_string(),
            viewing: false,
            scroll: 0,
            prior_mode: Box::new(prior),
        }));
        self.message = None;
        false
    }

    /// Non-blocking announcement for an **automated** headless run.
    pub(crate) fn announce_headless_run(&mut self, id: PromptId, harness: &AgentKind) {
        self.push_toast_info(format!(
            "Headless AI call: {} · {}",
            id.spec().title,
            harness.display_name()
        ));
    }

    fn take_pending_precall(&mut self) -> Option<Box<PendingPrecall>> {
        match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::PromptPrecall(pending) => Some(pending),
            other => {
                self.mode = other;
                None
            }
        }
    }

    pub(crate) fn precall_toggle_view(&mut self) {
        if let AppMode::PromptPrecall(pending) = &mut self.mode {
            pending.viewing = !pending.viewing;
            pending.scroll = 0;
        }
    }

    pub(crate) fn precall_scroll(&mut self, delta: isize) {
        if let AppMode::PromptPrecall(pending) = &mut self.mode
            && pending.viewing
        {
            let max = pending.preview.lines().count().saturating_sub(1);
            pending.scroll = (pending.scroll as isize + delta).clamp(0, max as isize) as usize;
        }
    }

    /// Continue: restore the originating mode, mark the action cleared, and
    /// re-dispatch it. The re-run re-resolves the prompt, so an override the
    /// user just saved from `e` takes effect on this run.
    pub(crate) fn precall_confirm(&mut self) -> Result<()> {
        let Some(pending) = self.take_pending_precall() else {
            return Ok(());
        };
        self.mode = *pending.prior_mode;
        self.precall_cleared = Some(pending.action);
        let result = self.dispatch_precall(pending.action);
        // The re-dispatched method's gate runs synchronously inside
        // `dispatch_precall`. Whether it consumed the clearance (normal path)
        // or bailed before reaching its gate (e.g. no harness → falls through
        // to a *different* gated call), the flag must not survive this call —
        // a stale `precall_cleared` would silently skip a later notice.
        self.precall_cleared = None;
        result
    }

    /// Cancel: restore the originating mode, run nothing.
    pub(crate) fn precall_cancel(&mut self) {
        if let Some(pending) = self.take_pending_precall() {
            self.mode = *pending.prior_mode;
            self.push_toast_info("Headless AI call cancelled");
        }
    }

    /// Edit: open the override manager focused on this prompt. The notice is
    /// stashed and restored when the manager closes.
    pub(crate) fn precall_edit(&mut self) {
        let Some(pending) = self.take_pending_precall() else {
            return;
        };
        let id = pending.prompt_id;
        self.precall_return = Some(pending);
        self.open_prompt_overrides_focused(None, Some(id));
    }

    fn dispatch_precall(&mut self, action: PrecallAction) -> Result<()> {
        match action {
            PrecallAction::PlanRound => self.start_next_plan_interview_ai_round(),
            PrecallAction::PlanSynthesis => self.start_plan_interview_synthesis(),
            PrecallAction::PlanCritique => self.start_plan_interview_critique(),
            PrecallAction::PlanDirectedRevision => self.start_plan_interview_directed_feedback(),
            PrecallAction::PlanInvestigation => self.start_plan_interview_investigation(),
            PrecallAction::ReviewWalkthrough => {
                self.generate_review_walkthrough();
                Ok(())
            }
            PrecallAction::ReviewCoReview => {
                self.generate_co_review();
                Ok(())
            }
            PrecallAction::ReviewChangesetOverview => {
                self.generate_changeset_overview();
                Ok(())
            }
            PrecallAction::ReviewDiffExplain => {
                crate::handlers::diff_review::generate_diff_review_explanation(self);
                Ok(())
            }
            PrecallAction::PrReviewAiReview => {
                self.begin_ai_pr_review();
                Ok(())
            }
            PrecallAction::ReviewMemoryBootstrap => {
                self.review_memory_bootstrap_pick_confirm();
                Ok(())
            }
            PrecallAction::ReviewMemoryCompact => {
                self.review_memory_compact_confirm_run();
                Ok(())
            }
        }
    }
}
