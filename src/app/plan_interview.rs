//! App-level lifecycle for plan interviews and deferred feature launches.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::mpsc;

use anyhow::{Context, Result};

use super::pr_review::estimate_tokens;
use super::{
    App, AppMode, PlanInterviewPhase, PlanInterviewState, PreparedFeatureLaunch, Selection,
};
use crate::db::plan_interviews::PlanInterviewRecord;
use crate::headless::HeadlessRunner;
use crate::plan_interview::{self, PlanQuestion};

const PLAN_FILE_NAME: &str = "plan.md";

/// Composer seed offered after an accepted interview. Deliberately short and
/// editable: the plan itself carries the detail, and the instruction block
/// already told the agent to treat its decisions as settled.
const PLAN_KICKOFF_PROMPT: &str = "\
Read `.claude/plan.md`. It is the plan I approved for this feature — its \
decisions are settled unless I say otherwise.

Start with the first unchecked task, and keep the task checkboxes current as \
you go.";

impl App {
    pub(crate) fn start_plan_interview(&mut self, prepared: PreparedFeatureLaunch) {
        let questions = self
            .store
            .find_project(&prepared.project_name)
            .map(|project| {
                self.extension_for_repo(&project.repo)
                    .plan_interview_questions()
            })
            .unwrap_or_else(crate::plan_interview::builtin_questions);
        let mut state = PlanInterviewState::for_feature_creation(prepared, questions);
        if let Some(draft) = self.load_plan_interview_draft(&state.interview_key) {
            state.offer_resume(draft);
        }
        self.mode = AppMode::PlanInterview(state);
        self.message = None;
    }

    /// The saved draft for `interview_key`, or `None` when there is nothing to
    /// resume.
    ///
    /// An unreadable row is treated as "no draft" so a corrupt or
    /// older-format record cannot block the interview; the reason lands in the
    /// debug log rather than in the user's way.
    fn load_plan_interview_draft(&mut self, interview_key: &str) -> Option<PlanInterviewRecord> {
        match self.db.as_ref()?.plan_interview_draft(interview_key) {
            Ok(draft) => draft,
            Err(e) => {
                self.log_warn(
                    "plan_interview",
                    format!("ignoring unreadable saved draft for {interview_key}: {e}"),
                );
                None
            }
        }
    }

    /// Save the interview as it stands so abandoning the mode — or losing the
    /// process — does not cost the user their answers.
    ///
    /// Called after every action that records an answer. Deliberately silent on
    /// failure: persistence is a convenience layered under a flow that works
    /// entirely from memory, and a DB error must not interrupt the interview.
    /// Nothing is saved before the brief is entered, since an interview with no
    /// brief has nothing worth resuming into.
    pub(crate) fn persist_plan_interview_draft(&mut self) {
        let record = match &self.mode {
            AppMode::PlanInterview(state) if !state.brief.trim().is_empty() => {
                state.to_draft_record()
            }
            _ => return,
        };
        let Some(db) = self.db.as_ref() else {
            return;
        };
        if let Err(e) = db.save_plan_interview(&record) {
            self.log_warn(
                "plan_interview",
                format!("failed to save plan interview draft: {e}"),
            );
        }
    }

    /// Resume the offered draft, restoring answers and any plan already
    /// generated for it.
    pub(crate) fn resume_plan_interview_draft(&mut self) -> Result<()> {
        let resumed = match &mut self.mode {
            AppMode::PlanInterview(state) => state.resume_from_draft(),
            _ => false,
        };
        if !resumed {
            return Ok(());
        }
        self.message = None;
        // Resuming can land directly on `Done` when the draft answered
        // everything and had already opted into adaptive rounds.
        self.continue_plan_interview_after_done()
    }

    /// Discard the offered draft and start over from a blank brief.
    pub(crate) fn discard_plan_interview_draft(&mut self) {
        let discarded = match &mut self.mode {
            AppMode::PlanInterview(state) => {
                state.discard_draft().then(|| state.interview_key.clone())
            }
            _ => None,
        };
        let Some(interview_key) = discarded else {
            return;
        };
        if let Some(db) = self.db.as_ref()
            && let Err(e) = db.delete_plan_interview_draft(&interview_key)
        {
            self.log_warn(
                "plan_interview",
                format!("failed to discard saved plan interview draft: {e}"),
            );
        }
        self.message = None;
    }

    /// Called once the interview's question flow or AI consent step reaches
    /// `Done`. Starts the next opted-in adaptive round, then synthesizes the
    /// collected interview before completing. No-op unless the mode is
    /// actually `Done`.
    pub(crate) fn continue_plan_interview_after_done(&mut self) -> Result<()> {
        let (is_done, should_start_next_round, synthesis_allowed, synthesis_attempted) =
            match &self.mode {
                AppMode::PlanInterview(state) => (
                    state.phase == PlanInterviewPhase::Done,
                    state.ai_followups_opted_in
                        && !state.skip_ai_rounds
                        && state.ai_rounds_completed < plan_interview::MAX_AI_ROUNDS,
                    state.ai_followups_opted_in || state.synthesis_requested,
                    state.synthesis_attempted,
                ),
                _ => (false, false, false, false),
            };
        if !is_done {
            return Ok(());
        }
        if should_start_next_round {
            self.start_next_plan_interview_ai_round()
        } else if synthesis_attempted {
            // Defensive only: both writers of `synthesis_attempted`
            // (`begin_synthesis`/`apply_synthesis`) leave the phase at
            // `SynthesisLoading`/`Review`, and nothing transitions back to
            // `Done` from there, so no current call site reaches this arm. It
            // stays as a backstop: were such a transition ever added, this
            // re-opens the plan that was already paid for rather than silently
            // spending tokens on a second synthesis pass.
            self.open_plan_interview_review(None);
            Ok(())
        } else if synthesis_allowed {
            self.start_plan_interview_synthesis()
        } else {
            // Enter/skip from the consent screen is the zero-token path.
            self.open_plan_interview_review(None);
            Ok(())
        }
    }

    /// Resolve the interview engine (lazily, once) and spawn one AI-adaptive
    /// round off the UI thread. Falls straight through to completion when no
    /// headless-capable harness is available — AI rounds are best-effort.
    fn start_next_plan_interview_ai_round(&mut self) -> Result<()> {
        let (
            preferred_harness,
            resolved_harness,
            round,
            feature_name,
            brief,
            questions,
            answers,
            workdir,
        ) = match &self.mode {
            AppMode::PlanInterview(state) => (
                state.preferred_harness.clone(),
                state.ai_harness.clone(),
                state.ai_rounds_completed + 1,
                state.feature_name.clone(),
                state.brief.clone(),
                state.questions.clone(),
                state.answers.clone(),
                state
                    .pending_launch
                    .as_ref()
                    .map(|prepared| prepared.workdir.clone())
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
            ),
            _ => return Ok(()),
        };

        let harness = match resolved_harness {
            Some(resolved) => resolved,
            None => HeadlessRunner::select_for_interview(&preferred_harness),
        };
        // Cache the resolution (even `None`) so later rounds don't re-probe.
        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.ai_harness = Some(harness.clone());
        }

        let Some(harness) = harness else {
            self.log_info(
                "plan_interview",
                "no headless-capable harness available; skipping AI rounds".to_string(),
            );
            return self.start_plan_interview_synthesis();
        };

        let context = plan_interview::gather_repository_context(&workdir);
        let prompt = plan_interview::build_interviewer_prompt(
            &feature_name,
            &brief,
            &questions,
            &answers,
            &context,
            round,
        );
        let token_estimate = estimate_tokens(&prompt);

        self.log_info(
            "plan_interview",
            format!(
                "starting AI round {round} with {} (~{token_estimate} tokens)",
                harness.display_name()
            ),
        );

        let (tx, rx) = mpsc::channel();
        self.plan_interview_ai_bg = Some(rx);
        let thread_harness = harness;
        let thread_workdir = workdir;
        std::thread::spawn(move || {
            let result = HeadlessRunner::run(&thread_harness, &thread_workdir, &prompt, None, true);
            let _ = tx.send((round, result));
        });

        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.begin_ai_round(token_estimate);
        }
        Ok(())
    }

    /// Spawn the final plan-synthesis pass off the UI thread. A missing
    /// headless engine is not fatal: completion retains Epic 1's raw-Q&A plan
    /// as a deterministic fallback.
    pub(crate) fn start_plan_interview_synthesis(&mut self) -> Result<()> {
        let (
            preferred_harness,
            resolved_harness,
            feature_name,
            brief,
            questions,
            answers,
            workdir,
            revision_critique,
        ) = match &mut self.mode {
            AppMode::PlanInterview(state) => (
                state.preferred_harness.clone(),
                state.ai_harness.clone(),
                state.feature_name.clone(),
                state.brief.clone(),
                state.questions.clone(),
                state.answers.clone(),
                state
                    .pending_launch
                    .as_ref()
                    .map(|prepared| prepared.workdir.clone())
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
                // Read, not taken: a revision that cannot run must leave the
                // feedback staged rather than spend it on a pass that never
                // happens.
                state.staged_revision_critique().map(str::to_string),
            ),
            _ => return Ok(()),
        };

        let harness = match resolved_harness {
            Some(resolved) => resolved,
            None => HeadlessRunner::select_for_interview(&preferred_harness),
        };
        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.ai_harness = Some(harness.clone());
        }

        let Some(harness) = harness else {
            // Regenerating from the review gate is the only way to get here
            // with a plan already on screen. The fallback re-renders that same
            // plan, so without an explicit message the keypress would look
            // unbound rather than deliberately declined. The staged feedback is
            // deliberately left untaken: the review is still on `a`, and `r`
            // retries the revision if a harness turns up.
            let regenerating = matches!(
                &self.mode,
                AppMode::PlanInterview(state) if state.synthesized_plan.is_some()
            );
            self.log_info(
                "plan_interview",
                if revision_critique.is_some() {
                    "no headless-capable harness available; keeping current plan and its review"
                        .to_string()
                } else if regenerating {
                    "no headless-capable harness available; keeping current plan".to_string()
                } else {
                    "no headless-capable harness available; using raw Q&A plan".to_string()
                },
            );
            self.open_plan_interview_review(None);
            self.message = Some(if revision_critique.is_some() {
                "No headless-capable harness available; the plan and its review are unchanged"
                    .into()
            } else if regenerating {
                "No headless-capable harness available; keeping current plan".into()
            } else {
                "No headless-capable harness available; using the raw Q&A plan".into()
            });
            return Ok(());
        };

        // Committed to the pass now, so the staged feedback is spent.
        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.take_revision_critique();
        }

        let context = plan_interview::gather_repository_context(&workdir);
        let prompt = plan_interview::build_synthesis_prompt(
            &feature_name,
            &brief,
            &questions,
            &answers,
            &context,
            revision_critique.as_deref(),
        );
        let token_estimate = estimate_tokens(&prompt);

        self.log_info(
            "plan_interview",
            format!(
                "starting plan {} with {} (~{token_estimate} tokens)",
                if revision_critique.is_some() {
                    "revision"
                } else {
                    "synthesis"
                },
                harness.display_name()
            ),
        );

        let (tx, rx) = mpsc::channel();
        self.plan_interview_synthesis_bg = Some(rx);
        let thread_harness = harness;
        std::thread::spawn(move || {
            let result = HeadlessRunner::run(&thread_harness, &workdir, &prompt, None, true);
            let _ = tx.send(result);
        });

        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.begin_synthesis(token_estimate);
        }
        Ok(())
    }

    /// Spawn the optional agent review of the draft plan off the UI thread.
    ///
    /// Purely advisory: the plan on screen is never modified by the result.
    /// A missing headless engine leaves the user at the review gate with a
    /// notice rather than blocking acceptance.
    pub(crate) fn start_plan_interview_critique(&mut self) -> Result<()> {
        // A review already held for this plan is re-opened rather than paid
        // for again; it is dropped whenever the plan changes, so one that
        // survives still describes what is on screen.
        let reopened = match &mut self.mode {
            AppMode::PlanInterview(state) => state.reopen_critique(),
            _ => false,
        };
        if reopened {
            self.message = None;
            return Ok(());
        }

        // A dismissed review leaves its worker running. Starting a second call
        // would pay twice for the same analysis and drop the first result.
        if self.plan_interview_critique_bg.is_some() {
            self.message = Some("Plan review still running".into());
            return Ok(());
        }

        let (
            preferred_harness,
            resolved_harness,
            feature_name,
            brief,
            questions,
            answers,
            workdir,
            plan,
        ) = match &self.mode {
            AppMode::PlanInterview(state) => {
                let Some(plan) = state.synthesized_plan.clone() else {
                    return Ok(());
                };
                (
                    state.preferred_harness.clone(),
                    state.ai_harness.clone(),
                    state.feature_name.clone(),
                    state.brief.clone(),
                    state.questions.clone(),
                    state.answers.clone(),
                    state
                        .pending_launch
                        .as_ref()
                        .map(|prepared| prepared.workdir.clone())
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
                    plan,
                )
            }
            _ => return Ok(()),
        };

        let harness = match resolved_harness {
            Some(resolved) => resolved,
            None => HeadlessRunner::select_for_interview(&preferred_harness),
        };
        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.ai_harness = Some(harness.clone());
        }

        let Some(harness) = harness else {
            self.log_info(
                "plan_interview",
                "no headless-capable harness available; skipping plan review".to_string(),
            );
            self.message = Some("No headless-capable harness available to review the plan".into());
            return Ok(());
        };

        let context = plan_interview::gather_repository_context(&workdir);
        let prompt = plan_interview::build_critique_prompt(
            &feature_name,
            &plan,
            &brief,
            &questions,
            &answers,
            &context,
        );
        let token_estimate = estimate_tokens(&prompt);

        // Claim the loading phase before spawning so a keypress in the same
        // tick cannot start a second paid review.
        let started = match &mut self.mode {
            AppMode::PlanInterview(state) => state.begin_critique(token_estimate),
            _ => false,
        };
        if !started {
            return Ok(());
        }

        self.log_info(
            "plan_interview",
            format!(
                "starting plan review with {} (~{token_estimate} tokens)",
                harness.display_name()
            ),
        );

        let (tx, rx) = mpsc::channel();
        self.plan_interview_critique_bg = Some(rx);
        std::thread::spawn(move || {
            let result = HeadlessRunner::run(&harness, &workdir, &prompt, None, true);
            let _ = tx.send(result);
        });
        self.message = None;
        Ok(())
    }

    /// Poll the in-flight agent review. A failure, or output that does not
    /// match the advisory contract, returns the user to the unchanged plan
    /// with a notice — there is nothing to fall back to and nothing to lose.
    pub fn poll_plan_interview_critique_bg(&mut self) -> bool {
        let Some(rx) = self.plan_interview_critique_bg.as_ref() else {
            return false;
        };
        let confirming_abort = matches!(
            &self.mode,
            AppMode::PlanInterview(state) if state.abort_confirmation
        );
        if confirming_abort {
            return false;
        }

        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.plan_interview_critique_bg = None;
                self.log_warn(
                    "plan_interview",
                    "plan review worker thread ended unexpectedly".to_string(),
                );
                if self.close_plan_interview_critique_loading() {
                    self.message = Some("Plan review failed; the plan is unchanged".into());
                }
                return true;
            }
        };
        self.plan_interview_critique_bg = None;

        let is_loading = matches!(
            &self.mode,
            AppMode::PlanInterview(state)
                if state.phase == PlanInterviewPhase::CritiqueLoading
        );
        if !is_loading {
            // The user dismissed the review or moved on. Don't pull them back
            // into it — but the call was paid for, so keep a usable result
            // where `a` can re-open it instead of dropping it on the floor.
            let stashed = match (result, &mut self.mode) {
                (Ok(response), AppMode::PlanInterview(state)) => {
                    match plan_interview::parse_plan_critique(&response) {
                        Some(critique) => state.stash_critique(critique),
                        None => false,
                    }
                }
                _ => false,
            };
            if stashed {
                self.log_info(
                    "plan_interview",
                    "dismissed plan review finished; kept for re-open".to_string(),
                );
            }
            return stashed;
        }

        // A call that never ran and a call that answered off-contract are
        // different problems with different fixes, so they get different
        // messages rather than one catch-all.
        let (critique, failure) = match result {
            Ok(response) => match plan_interview::parse_plan_critique(&response) {
                Some(critique) => (Some(critique), None),
                None => {
                    self.log_warn(
                        "plan_interview",
                        format!(
                            "plan review returned output that does not match the review contract: {}",
                            truncate_for_log(&response)
                        ),
                    );
                    (None, Some("Plan review returned no usable analysis"))
                }
            },
            Err(e) => {
                self.log_warn("plan_interview", format!("plan review failed: {e}"));
                (None, Some("Plan review failed; the plan is unchanged"))
            }
        };

        match critique {
            Some(critique) => {
                if let AppMode::PlanInterview(state) = &mut self.mode {
                    state.apply_critique(critique);
                }
                self.message = None;
            }
            None => {
                self.close_plan_interview_critique_loading();
                self.message = failure.map(Into::into);
            }
        }
        true
    }

    /// Return from an unresolved review to the plan. Returns whether the
    /// caller is the one that ended the loading phase.
    fn close_plan_interview_critique_loading(&mut self) -> bool {
        match &mut self.mode {
            AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::CritiqueLoading => {
                state.close_critique()
            }
            _ => false,
        }
    }

    /// Poll the in-flight AI-adaptive round. Returns `true` when a redraw is
    /// warranted.
    pub fn poll_plan_interview_ai_bg(&mut self) -> bool {
        let Some(rx) = self.plan_interview_ai_bg.as_ref() else {
            return false;
        };
        let confirming_abort = matches!(
            &self.mode,
            AppMode::PlanInterview(state) if state.abort_confirmation
        );
        if confirming_abort {
            // Leave the round result unread until the user resolves the
            // pending abort choice — draining it here could start another
            // paid round or launch the feature out from under them. Esc
            // (resume) clears `abort_confirmation` and the next poll tick
            // picks the result back up normally; y/n move `self.mode` out of
            // `PlanInterview`, so the next tick falls through to the
            // "navigated away, nothing to apply" path below instead.
            return false;
        }
        let (round, result) = match rx.try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.plan_interview_ai_bg = None;
                self.log_warn(
                    "plan_interview",
                    "AI round worker thread ended unexpectedly".to_string(),
                );
                let was_loading = matches!(
                    &self.mode,
                    AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::AiLoading
                );
                if was_loading {
                    if let AppMode::PlanInterview(state) = &mut self.mode {
                        state.ai_round_started_at = None;
                        state.ai_rounds_completed = plan_interview::MAX_AI_ROUNDS;
                        state.phase = PlanInterviewPhase::Done;
                    }
                    if let Err(e) = self.continue_plan_interview_after_done() {
                        self.report_logged_error(
                            "plan_interview",
                            format!("Failed to continue plan interview: {e}"),
                        );
                    }
                }
                return true;
            }
        };
        self.plan_interview_ai_bg = None;

        let is_loading = matches!(
            &self.mode,
            AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::AiLoading
        );
        if !is_loading {
            // The user navigated or aborted away from the loading screen;
            // the background call already finished, nothing to apply.
            return false;
        }

        let existing_ids: Vec<String> = match &self.mode {
            AppMode::PlanInterview(state) => state.questions.iter().map(|q| q.id.clone()).collect(),
            _ => return false,
        };

        let new_questions = match result {
            Ok(response) => {
                let parsed = plan_interview::parse_ai_questions(&response, &existing_ids, round);
                if parsed.is_empty() {
                    self.log_debug(
                        "plan_interview",
                        format!(
                            "AI round {round} returned no usable follow-up questions: {}",
                            truncate_for_log(&response)
                        ),
                    );
                }
                parsed
            }
            Err(e) => {
                self.log_warn("plan_interview", format!("AI round {round} failed: {e}"));
                Vec::new()
            }
        };

        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.apply_ai_round(round, new_questions);
        }
        // A spent round is recorded even when it produced nothing usable, so the
        // draft has to capture it: resuming must not hand back a paid round.
        self.persist_plan_interview_draft();

        let reached_done = matches!(
            &self.mode,
            AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Done
        );
        if reached_done && let Err(e) = self.continue_plan_interview_after_done() {
            self.report_logged_error(
                "plan_interview",
                format!("Failed to continue plan interview: {e}"),
            );
        }
        true
    }

    /// Poll the in-flight synthesis pass. A failed, disconnected, empty, or
    /// structurally incomplete response falls back to the raw interview plan.
    pub fn poll_plan_interview_synthesis_bg(&mut self) -> bool {
        let Some(rx) = self.plan_interview_synthesis_bg.as_ref() else {
            return false;
        };
        let confirming_abort = matches!(
            &self.mode,
            AppMode::PlanInterview(state) if state.abort_confirmation
        );
        if confirming_abort {
            return false;
        }

        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.plan_interview_synthesis_bg = None;
                self.log_warn(
                    "plan_interview",
                    "plan synthesis worker thread ended unexpectedly; using raw Q&A plan"
                        .to_string(),
                );
                let was_loading = matches!(
                    &self.mode,
                    AppMode::PlanInterview(state)
                        if state.phase == PlanInterviewPhase::SynthesisLoading
                );
                if was_loading {
                    self.open_plan_interview_review(None);
                }
                return true;
            }
        };
        self.plan_interview_synthesis_bg = None;

        let is_loading = matches!(
            &self.mode,
            AppMode::PlanInterview(state)
                if state.phase == PlanInterviewPhase::SynthesisLoading
        );
        if !is_loading {
            // The user navigated or aborted away from the synthesis screen;
            // discard the late result without touching the launch.
            return false;
        }

        let plan = match result {
            Ok(response) => {
                let plan = plan_interview::parse_synthesized_plan(&response);
                if plan.is_none() {
                    self.log_warn(
                        "plan_interview",
                        format!(
                            "plan synthesis returned incomplete markdown; using raw Q&A plan: {}",
                            truncate_for_log(&response)
                        ),
                    );
                }
                plan
            }
            Err(e) => {
                self.log_warn(
                    "plan_interview",
                    format!("plan synthesis failed; using raw Q&A plan: {e}"),
                );
                None
            }
        };

        self.open_plan_interview_review(plan);
        true
    }

    /// Resolve a synthesis result into the exact markdown shown at the review
    /// gate. A failed first pass uses the raw-Q&A fallback; a failed
    /// regeneration preserves the plan the user was already reviewing.
    fn open_plan_interview_review(&mut self, generated: Option<String>) {
        let plan = match &self.mode {
            AppMode::PlanInterview(state) => generated
                .or_else(|| state.synthesized_plan.clone())
                .unwrap_or_else(|| {
                    render_static_plan(
                        &state.feature_name,
                        &state.brief,
                        &state.questions,
                        &state.answers,
                    )
                }),
            _ => return,
        };
        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.apply_synthesis(plan);
        }
        // The plan at the review gate was paid for. Persisting it means an
        // abandoned review resumes straight back to it instead of synthesizing
        // again.
        self.persist_plan_interview_draft();
        self.message = None;
    }

    /// Accept the reviewed plan and execute the launch it has been holding.
    pub(crate) fn complete_plan_interview(&mut self) -> Result<()> {
        let (workdir, plan, interview_key) = match &self.mode {
            AppMode::PlanInterview(state) => {
                let Some(prepared) = state.pending_launch.as_ref() else {
                    return Ok(());
                };
                (
                    prepared.workdir.clone(),
                    state.synthesized_plan.clone().unwrap_or_else(|| {
                        render_static_plan(
                            &state.feature_name,
                            &state.brief,
                            &state.questions,
                            &state.answers,
                        )
                    }),
                    state.interview_key.clone(),
                )
            }
            _ => return Ok(()),
        };

        // Keep the interview open if either write fails so the user can retry
        // or abort without losing the answers they just entered.
        write_plan_file(&workdir, &plan)?;

        // The draft has to exist for the accept to finalize it, and it only
        // exists if something was saved during the interview. Save the accepted
        // state first so a plan reached without a persisted draft — a DB that
        // appeared mid-interview, or a flow that skipped straight to synthesis —
        // still leaves a transcript behind.
        self.persist_plan_interview_draft();

        let pending = match &mut self.mode {
            AppMode::PlanInterview(state) => state.pending_launch.take(),
            _ => return Ok(()),
        };
        self.mode = AppMode::Normal;

        if let Some(prepared) = pending {
            let project_name = prepared.project_name.clone();
            let branch = prepared.branch.clone();
            // The launch injects the plan-mode instruction block into the
            // harness's instruction file (via `ensure_feature_running`), so the
            // agent already knows the plan is user-approved before it reads the
            // kickoff prompt.
            self.finish_feature_launch_without_interview(prepared)?;
            self.finalize_plan_interview_transcript(&interview_key, &project_name, &branch, &plan);
            self.seed_plan_kickoff_prompt(&project_name, &branch);
            Ok(())
        } else {
            self.finalize_plan_interview_transcript(&interview_key, "", "", &plan);
            self.message = Some("Plan interview complete".into());
            Ok(())
        }
    }

    /// Promote the accepted interview's draft into the feature's transcript.
    ///
    /// A feature-creation interview saved its draft under a pending key, because
    /// the feature had no id yet; the transcript is re-filed under the id the
    /// launch just created so a later re-run on that feature finds it. Called
    /// after the launch for exactly that reason.
    ///
    /// Best-effort: the plan file is already written and the session already
    /// running by this point, so nothing here is allowed to fail the accept.
    fn finalize_plan_interview_transcript(
        &mut self,
        interview_key: &str,
        project_name: &str,
        branch: &str,
        plan: &str,
    ) {
        let feature_id = self
            .store
            .find_project(project_name)
            .and_then(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.name == branch)
            })
            .map(|feature| feature.id.clone())
            // No feature to re-file under (an on-demand interview, or a launch
            // that did not produce one) leaves the transcript on its own key.
            .unwrap_or_else(|| interview_key.to_string());

        let Some(db) = self.db.as_ref() else {
            return;
        };
        match db.finalize_plan_interview_draft(interview_key, &feature_id, plan) {
            Ok(true) => {}
            Ok(false) => self.log_debug(
                "plan_interview",
                format!("no saved draft to finalize for {interview_key}"),
            ),
            Err(e) => self.log_warn(
                "plan_interview",
                format!("failed to save the accepted plan interview transcript: {e}"),
            ),
        }
    }

    /// Open the freshly launched agent session with its composer seeded with a
    /// kickoff prompt pointing at the accepted plan.
    ///
    /// Best-effort by design: this runs *after* the feature is created and
    /// started, so nothing here is allowed to fail the accept. If the launch
    /// took the user somewhere else (the startup steering prompt) or the
    /// feature has no tmux-backed agent session, the plan file and instruction
    /// block are already in place and the seed is simply skipped.
    fn seed_plan_kickoff_prompt(&mut self, project_name: &str, branch: &str) {
        if !matches!(self.mode, AppMode::Normal) {
            return;
        }

        let Some((pi, fi, si)) = self
            .store
            .projects
            .iter()
            .position(|project| project.name == project_name)
            .and_then(|pi| {
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|feature| feature.name == branch)?;
                let si = self.store.projects[pi].features[fi]
                    .sessions
                    .iter()
                    .position(|session| {
                        session.kind.is_agent_harness() && session.kind.is_tmux_backed()
                    })?;
                Some((pi, fi, si))
            })
        else {
            return;
        };

        self.selection = Selection::Session(pi, fi, si);
        if let Err(e) = self.enter_view_without_auto_compose() {
            self.log_warn(
                "plan_interview",
                format!("failed to open the new session for the plan kickoff prompt: {e}"),
            );
            return;
        }
        if let Err(e) = self.open_compose_seeded(PLAN_KICKOFF_PROMPT.to_string()) {
            self.log_warn(
                "plan_interview",
                format!("failed to seed the plan kickoff prompt: {e}"),
            );
        }
    }

    /// Drop a deleted feature's stored interviews.
    ///
    /// `plan_interviews.feature_id` carries no foreign key (the store's
    /// full-replace save would cascade-wipe the rows), so deletion is explicit.
    /// Two keys are cleared: the feature's id, holding anything saved once the
    /// feature existed, and the pending key from
    /// [`crate::plan_interview::pending_interview_key`], which still holds a
    /// draft if the feature was created after its interview was abandoned.
    ///
    /// Best-effort: the feature is going away regardless, and a leftover row is
    /// inert — it is only ever read by an interview for this same feature.
    pub(crate) fn delete_plan_interviews_for_deleted_feature(
        &mut self,
        project_name: &str,
        feature_name: &str,
        feature_id: &Option<String>,
    ) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let mut keys = vec![plan_interview::pending_interview_key(
            project_name,
            feature_name,
        )];
        keys.extend(feature_id.clone());

        let failures: Vec<String> = keys
            .iter()
            .filter_map(|key| {
                db.delete_plan_interviews_for_feature(key)
                    .err()
                    .map(|e| format!("{key}: {e}"))
            })
            .collect();
        if !failures.is_empty() {
            self.log_warn(
                "plan_interview",
                format!(
                    "failed to delete stored interviews for '{feature_name}': {}",
                    failures.join("; ")
                ),
            );
        }
    }

    /// Abort discovery but keep creating the feature, explicitly without plan
    /// mode so the legacy plan-file behavior is not triggered.
    pub(crate) fn launch_plan_interview_without_plan(&mut self) -> Result<()> {
        let pending = match &mut self.mode {
            AppMode::PlanInterview(state) => state.pending_launch.take(),
            _ => return Ok(()),
        };
        self.mode = AppMode::Normal;

        if let Some(mut prepared) = pending {
            prepared.plan_mode = false;
            self.finish_feature_launch_without_interview(prepared)
        } else {
            self.message = Some("Plan interview cancelled".into());
            Ok(())
        }
    }

    /// Cancel the pending feature launch. A worktree may already have been
    /// created (and may contain hook changes), so keep it rather than removing
    /// user data. Pending placeholder features created for hooks are removed.
    pub(crate) fn cancel_plan_interview_feature(&mut self) -> Result<()> {
        let pending = match &mut self.mode {
            AppMode::PlanInterview(state) => state.pending_launch.take(),
            _ => return Ok(()),
        };
        self.mode = AppMode::Normal;

        let Some(prepared) = pending else {
            self.message = Some("Plan interview cancelled".into());
            return Ok(());
        };

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|project| project.name == prepared.project_name)
        {
            self.store.projects[pi].features.retain(|feature| {
                !(feature.name == prepared.branch && feature.pending_worktree_script)
            });
            self.selection = Selection::Project(pi);
            self.save()?;
        }

        self.message = Some(if prepared.is_worktree {
            format!(
                "Feature creation cancelled; worktree kept at {}",
                prepared.workdir.display()
            )
        } else {
            "Feature creation cancelled".into()
        });
        Ok(())
    }
}

/// Bound an unexpected harness reply before it reaches the debug log, so a
/// contract mismatch is diagnosable without dumping a whole response into it.
fn truncate_for_log(response: &str) -> String {
    const MAX_CHARS: usize = 300;
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return "<empty response>".to_string();
    }
    let truncated: String = trimmed.chars().take(MAX_CHARS).collect();
    if truncated.chars().count() < trimmed.chars().count() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

/// Deterministic plan used whenever synthesis is unavailable or fails.
///
/// Skipped and blank answers are omitted rather than listed as "_Skipped._":
/// this file is what the implementing agent reads, and a wall of unanswered
/// prompts is context it has to spend attention discarding. An interview where
/// nothing was answered degrades to the brief alone, which is still the whole
/// of what the user said.
fn render_static_plan(
    feature_name: &str,
    brief: &str,
    questions: &[PlanQuestion],
    answers: &[Option<String>],
) -> String {
    let mut plan = format!("# Plan: {feature_name}\n\n## Feature brief\n\n{brief}\n");

    let answered = questions
        .iter()
        .enumerate()
        .filter_map(|(index, question)| {
            answers
                .get(index)
                .and_then(|answer| answer.as_deref())
                .filter(|answer| !answer.trim().is_empty())
                .map(|answer| (question, answer))
        });

    let mut wrote_heading = false;
    for (question, answer) in answered {
        if !wrote_heading {
            plan.push_str("\n## Q&A\n");
            wrote_heading = true;
        }
        plan.push_str("\n### ");
        plan.push_str(&question.text);
        plan.push_str("\n\n");
        plan.push_str(answer);
        plan.push('\n');
    }

    plan
}

fn write_plan_file(workdir: &Path, contents: &str) -> Result<()> {
    let claude_dir = workdir.join(".claude");
    fs::create_dir_all(&claude_dir)
        .with_context(|| format!("failed to create plan directory {}", claude_dir.display()))?;

    ensure_gitignore_entry(&claude_dir.join(".gitignore"), PLAN_FILE_NAME)?;

    let plan_path = claude_dir.join(PLAN_FILE_NAME);
    fs::write(&plan_path, contents)
        .with_context(|| format!("failed to write plan file {}", plan_path.display()))
}

fn ensure_gitignore_entry(path: &Path, entry: &str) -> Result<()> {
    let current = fs::read_to_string(path).unwrap_or_default();
    if current.lines().any(|line| line == entry) {
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open gitignore {}", path.display()))?;
    if !current.is_empty() && !current.ends_with('\n') {
        writeln!(file).with_context(|| format!("failed to update gitignore {}", path.display()))?;
    }
    writeln!(file, "{entry}")
        .with_context(|| format!("failed to update gitignore {}", path.display()))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::markdown::{preferred_plan_markdown_path, read_plan_preview};
    use crate::plan_interview::{PlanQuestionKind, QuestionSource};

    fn free_text_question(id: &str, text: &str) -> PlanQuestion {
        PlanQuestion {
            id: id.into(),
            text: text.into(),
            kind: PlanQuestionKind::FreeText,
            source: QuestionSource::Builtin,
            optional: true,
        }
    }

    #[test]
    fn static_plan_keeps_brief_and_answers_but_omits_unanswered_questions() {
        let questions = vec![
            free_text_question("scope", "What is in scope?"),
            free_text_question("risks", "What are the risks?"),
            free_text_question("ui", "What is the UI surface?"),
        ];

        let plan = render_static_plan(
            "guided-plans",
            "Collect a brief\nbefore launch.",
            &questions,
            &[
                Some("Native TUI only.\nNo AI yet.".into()),
                None,
                // Blank answers are skips too — a config-authored select option
                // can be an empty string.
                Some("   ".into()),
            ],
        );

        assert!(plan.starts_with("# Plan: guided-plans\n\n## Feature brief"));
        assert!(plan.contains("Collect a brief\nbefore launch."));
        assert!(plan.contains("### What is in scope?\n\nNative TUI only.\nNo AI yet."));
        assert!(!plan.contains("What are the risks?"));
        assert!(!plan.contains("What is the UI surface?"));
        assert!(!plan.contains("_Skipped._"));
    }

    #[test]
    fn static_plan_without_any_answers_is_brief_only() {
        let questions = vec![free_text_question("scope", "What is in scope?")];

        let plan = render_static_plan("guided-plans", "Ship it.", &questions, &[None]);

        assert_eq!(
            plan,
            "# Plan: guided-plans\n\n## Feature brief\n\nShip it.\n"
        );
        assert!(!plan.contains("## Q&A"));
    }

    #[test]
    fn writing_plan_creates_claude_dir_and_idempotent_ignore_entry() {
        let workdir = TempDir::new().unwrap();
        fs::create_dir(workdir.path().join(".claude")).unwrap();
        fs::write(workdir.path().join(".claude/.gitignore"), "notifications/").unwrap();

        write_plan_file(workdir.path(), "# First plan\n").unwrap();
        write_plan_file(workdir.path(), "# Updated plan\n").unwrap();

        assert_eq!(
            fs::read_to_string(workdir.path().join(".claude/plan.md")).unwrap(),
            "# Updated plan\n"
        );
        assert_eq!(
            fs::read_to_string(workdir.path().join(".claude/.gitignore")).unwrap(),
            "notifications/\nplan.md\n"
        );
    }

    #[test]
    fn non_worktree_plan_write_is_the_plan_sidebar_reads() {
        let repo = TempDir::new().unwrap();
        let expected_plan = repo.path().join(".claude/plan.md");

        write_plan_file(
            repo.path(),
            "# Plan: first feature\n\n## Feature brief\n\nShip it.\n",
        )
        .unwrap();

        assert_eq!(
            preferred_plan_markdown_path(repo.path(), Some(repo.path())),
            Some(expected_plan.clone())
        );
        assert_eq!(
            read_plan_preview(repo.path(), Some(repo.path())).as_deref(),
            Some("Plan: first feature\nFeature brief\nShip it.")
        );
        assert!(expected_plan.is_file());
        assert!(!repo.path().join("PLAN.md").exists());
        assert!(!repo.path().join("plan.md").exists());
    }
}
