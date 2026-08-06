//! App-level lifecycle for plan interviews and deferred feature launches.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::mpsc;

use anyhow::{Context, Result, bail};

use super::pr_review::estimate_tokens;
use super::{
    App, AppMode, PlanInterviewPhase, PlanInterviewState, PlanKickoffTarget, PreparedFeatureLaunch,
    Selection,
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

    /// Run the interview on demand against the currently selected feature.
    ///
    /// Unlike the feature-creation trigger there is no launch to defer: the
    /// feature already exists, so accepting simply rewrites its plan file. The
    /// interview is keyed by the feature's id, so a saved draft or an earlier
    /// accepted transcript for this feature is picked up on entry.
    pub(crate) fn start_plan_interview_for_selected_feature(&mut self) {
        let Some((project, feature)) = self.selected_feature() else {
            self.message = Some("Select a feature to plan".into());
            return;
        };
        let (repo, feature_name, feature_id, workdir, agent) = (
            project.repo.clone(),
            feature.name.clone(),
            feature.id.clone(),
            feature.workdir.clone(),
            feature.agent.clone(),
        );

        let questions = self.extension_for_repo(&repo).plan_interview_questions();
        let mut state = PlanInterviewState::for_feature(
            feature_name,
            feature_id.clone(),
            questions,
            workdir,
            agent,
        );

        // A re-run starts from the plan the user already accepted: prior answers
        // are pre-filled so keeping one is `Enter` and only what changed needs
        // typing. Applied before the draft prompt so discarding a stale draft
        // falls back to the accepted answers rather than to a blank interview.
        let mut notice = None;
        if let Some(transcript) = self.load_plan_interview_transcript(&feature_id)
            && state.apply_previous_transcript(&transcript)
        {
            notice = Some("Previous answers pre-filled: Enter keeps one, Ctrl+R restores a change");
        }
        if let Some(draft) = self.load_plan_interview_draft(&feature_id) {
            state.offer_resume(draft);
            // The draft prompt has its own explanation on screen; a pre-fill
            // notice here would describe a state the user has not reached yet.
            notice = None;
        }

        self.mode = AppMode::PlanInterview(state);
        self.message = notice.map(Into::into);
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

    /// The accepted transcript for `interview_key`, or `None` when this feature
    /// has never had a plan accepted.
    ///
    /// Unreadable rows are treated as "no transcript" for the same reason as
    /// drafts: a re-run must still be possible, just without the pre-fill.
    fn load_plan_interview_transcript(
        &mut self,
        interview_key: &str,
    ) -> Option<PlanInterviewRecord> {
        match self.db.as_ref()?.plan_interview_final(interview_key) {
            Ok(transcript) => transcript,
            Err(e) => {
                self.log_warn(
                    "plan_interview",
                    format!("ignoring unreadable saved transcript for {interview_key}: {e}"),
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

    /// Discard the offered draft and start the interview over from its baseline
    /// — a blank brief, or the accepted transcript's answers on a re-run.
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
                state.context_workdir(),
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
                state.context_workdir(),
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
                    state.context_workdir(),
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

    /// Run a free-form review-gate instruction through the planning agent.
    /// Unlike synthesis and critique, this pass may inspect the feature
    /// workdir, so it uses the runner's read-only tool contract.
    pub(crate) fn start_plan_interview_directed_feedback(&mut self) -> Result<()> {
        if self.plan_interview_directed_feedback_bg.is_some() {
            self.message = Some("A directed plan revision is already running".into());
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
            instruction,
        ) = match &self.mode {
            AppMode::PlanInterview(state)
                if state.phase == PlanInterviewPhase::DirectedFeedback =>
            {
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
                    state.context_workdir(),
                    plan,
                    state.editor.text().trim().to_string(),
                )
            }
            _ => return Ok(()),
        };

        if instruction.is_empty() {
            self.message = Some("Describe how the plan should change".into());
            return Ok(());
        }

        let harness = match resolved_harness {
            Some(resolved) => resolved,
            None => HeadlessRunner::select_for_interview(&preferred_harness),
        };
        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.ai_harness = Some(harness.clone());
        }
        let Some(harness) = harness else {
            self.message =
                Some("No headless-capable harness available; the plan is unchanged".into());
            return Ok(());
        };

        let prompt = plan_interview::build_directed_revision_prompt(
            &feature_name,
            &plan,
            &instruction,
            &brief,
            &questions,
            &answers,
        );
        let token_estimate = estimate_tokens(&prompt);
        let started = match &mut self.mode {
            AppMode::PlanInterview(state) => state.begin_directed_feedback_loading(token_estimate),
            _ => false,
        };
        if !started {
            return Ok(());
        }

        self.log_info(
            "plan_interview",
            format!(
                "starting read-only directed plan revision with {} (~{token_estimate} tokens)",
                harness.display_name()
            ),
        );
        let (tx, rx) = mpsc::channel();
        self.plan_interview_directed_feedback_bg = Some(rx);
        std::thread::spawn(move || {
            let result = HeadlessRunner::run_read_only(&harness, &workdir, &prompt, None);
            let _ = tx.send(result);
        });
        self.message = None;
        Ok(())
    }

    /// Apply a completed directed revision only while its loading frame is
    /// still active. If the user backed out, the late result is discarded and
    /// can never overwrite a plan they chose to keep.
    pub fn poll_plan_interview_directed_feedback_bg(&mut self) -> bool {
        let Some(rx) = self.plan_interview_directed_feedback_bg.as_ref() else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.plan_interview_directed_feedback_bg = None;
                if let AppMode::PlanInterview(state) = &mut self.mode {
                    state.fail_directed_feedback();
                }
                self.message = Some("Directed plan revision failed; the plan is unchanged".into());
                return true;
            }
        };
        self.plan_interview_directed_feedback_bg = None;

        let is_loading = matches!(
            &self.mode,
            AppMode::PlanInterview(state)
                if state.phase == PlanInterviewPhase::DirectedFeedbackLoading
        );
        if !is_loading {
            self.log_info(
                "plan_interview",
                "discarded a directed revision after the user returned to the plan".to_string(),
            );
            return true;
        }

        match result {
            Ok(response) => match plan_interview::parse_synthesized_plan(&response) {
                Some(plan) => {
                    if let AppMode::PlanInterview(state) = &mut self.mode {
                        state.apply_synthesis(plan);
                    }
                    self.persist_plan_interview_draft();
                    self.message =
                        Some("Plan revised from your feedback; review the changes".into());
                }
                None => {
                    self.log_warn(
                        "plan_interview",
                        format!(
                            "directed plan revision returned invalid markdown: {}",
                            truncate_for_log(&response)
                        ),
                    );
                    if let AppMode::PlanInterview(state) = &mut self.mode {
                        state.fail_directed_feedback();
                    }
                    self.message = Some(
                        "Directed revision returned no usable plan; your instruction is preserved"
                            .into(),
                    );
                }
            },
            Err(error) => {
                self.log_warn(
                    "plan_interview",
                    format!("directed plan revision failed: {error}"),
                );
                if let AppMode::PlanInterview(state) = &mut self.mode {
                    state.fail_directed_feedback();
                }
                self.message =
                    Some("Directed plan revision failed; your instruction is preserved".into());
            }
        }
        true
    }

    /// Run focused repository research in fresh read-only contexts, then hand
    /// only the bounded findings to a separate no-tools planning context for
    /// merging. This keeps exploration transcripts out of both the interview
    /// state and the implementation session that will eventually receive the
    /// accepted plan.
    pub(crate) fn start_plan_interview_investigation(&mut self) -> Result<()> {
        if self.plan_interview_investigation_bg.is_some() {
            self.message = Some("An isolated plan investigation is already running".into());
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
            focuses,
        ) = match &self.mode {
            AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Investigation => {
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
                    state.context_workdir(),
                    plan,
                    plan_interview::investigation_focuses(state.editor.text()),
                )
            }
            _ => return Ok(()),
        };

        if focuses.is_empty() {
            self.message = Some("Describe what the investigators should research".into());
            return Ok(());
        }
        if focuses.len() > plan_interview::MAX_INVESTIGATION_FOCUSES {
            self.message = Some(format!(
                "Use at most {} research focuses; separate them with blank lines",
                plan_interview::MAX_INVESTIGATION_FOCUSES
            ));
            return Ok(());
        }

        let harness = match resolved_harness {
            Some(resolved) => resolved,
            None => HeadlessRunner::select_for_interview(&preferred_harness),
        };
        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.ai_harness = Some(harness.clone());
        }
        let Some(harness) = harness else {
            self.message =
                Some("No headless-capable harness available; the plan is unchanged".into());
            return Ok(());
        };

        let investigation_prompts: Vec<(String, String)> = focuses
            .iter()
            .map(|focus| {
                (
                    focus.clone(),
                    plan_interview::build_investigation_prompt(
                        &feature_name,
                        &plan,
                        focus,
                        &brief,
                        &questions,
                        &answers,
                    ),
                )
            })
            .collect();
        // The merge prompt carries the investigators' real reports, each bounded
        // at `INVESTIGATION_FINDINGS_MAX_CHARS`. Sizing it with a short
        // placeholder would omit that payload entirely and understate a paid
        // call by roughly 3k tokens per focus, so the estimate uses the bound
        // and comes out a ceiling.
        let placeholder_findings: Vec<plan_interview::PlanInvestigationFinding> = focuses
            .iter()
            .map(|focus| plan_interview::PlanInvestigationFinding {
                focus: focus.clone(),
                findings: plan_interview::investigation_findings_size_placeholder(),
            })
            .collect();
        let merge_estimate = estimate_tokens(&plan_interview::build_investigation_merge_prompt(
            &feature_name,
            &plan,
            &brief,
            &questions,
            &answers,
            &placeholder_findings,
        ));
        let token_estimate = investigation_prompts
            .iter()
            .fold(merge_estimate, |total, (_, prompt)| {
                total.saturating_add(estimate_tokens(prompt))
            });
        let started = match &mut self.mode {
            AppMode::PlanInterview(state) => state.begin_investigation_loading(token_estimate),
            _ => false,
        };
        if !started {
            return Ok(());
        }

        self.log_info(
            "plan_interview",
            format!(
                "starting {} isolated plan investigation(s) with {} (~{token_estimate} prompt tokens including merge)",
                investigation_prompts.len(),
                harness.display_name()
            ),
        );
        let (tx, rx) = mpsc::channel();
        self.plan_interview_investigation_bg = Some(rx);
        std::thread::spawn(move || {
            let result = (|| -> Result<plan_interview::PlanInvestigationOutcome> {
                let total = investigation_prompts.len();
                let mut findings = Vec::with_capacity(total);
                let mut failed_focuses = Vec::new();
                for (index, (focus, prompt)) in investigation_prompts.into_iter().enumerate() {
                    // One investigator's failure costs only its own focus. The
                    // runs that already completed are paid for, so the gap is
                    // recorded for the merge pass and the batch continues
                    // instead of forcing a retry that re-runs everything.
                    let findings_markdown =
                        HeadlessRunner::run_read_only(&harness, &workdir, &prompt, None)
                            .with_context(|| format!("isolated investigator {} failed", index + 1))
                            .and_then(|response| {
                                plan_interview::parse_investigation_findings(&response)
                                    .with_context(|| {
                                        format!(
                                            "isolated investigator {} returned no usable findings",
                                            index + 1
                                        )
                                    })
                            });
                    let findings_markdown = match findings_markdown {
                        Ok(findings_markdown) => findings_markdown,
                        Err(error) => {
                            crate::debug::log_to_file(
                                crate::debug::LogLevel::Warn,
                                "plan_interview",
                                &format!("{error:#}"),
                            );
                            failed_focuses.push(focus.clone());
                            plan_interview::FAILED_INVESTIGATION_FINDINGS.to_string()
                        }
                    };
                    findings.push(plan_interview::PlanInvestigationFinding {
                        focus,
                        findings: findings_markdown,
                    });
                }
                if failed_focuses.len() == total {
                    bail!("every isolated investigator failed");
                }

                // A new headless invocation is the context boundary: the
                // planner sees only these findings, never tool traces or the
                // investigators' repository-exploration context.
                let merge_prompt = plan_interview::build_investigation_merge_prompt(
                    &feature_name,
                    &plan,
                    &brief,
                    &questions,
                    &answers,
                    &findings,
                );
                let merge_response =
                    HeadlessRunner::run(&harness, &workdir, &merge_prompt, None, true)
                        .context("isolated investigation merge failed")?;
                Ok(plan_interview::PlanInvestigationOutcome {
                    merge_response,
                    failed_focuses,
                })
            })();
            let _ = tx.send(result);
        });
        self.message = None;
        Ok(())
    }

    /// Apply the isolated investigation only while its loading frame remains
    /// active. Dismissing it makes any late merge result inert, exactly like a
    /// dismissed directed revision.
    pub fn poll_plan_interview_investigation_bg(&mut self) -> bool {
        let Some(rx) = self.plan_interview_investigation_bg.as_ref() else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.plan_interview_investigation_bg = None;
                if let AppMode::PlanInterview(state) = &mut self.mode {
                    state.fail_investigation();
                }
                self.message = Some("Isolated investigation failed; the plan is unchanged".into());
                return true;
            }
        };
        self.plan_interview_investigation_bg = None;

        let is_loading = matches!(
            &self.mode,
            AppMode::PlanInterview(state)
                if state.phase == PlanInterviewPhase::InvestigationLoading
        );
        if !is_loading {
            self.log_info(
                "plan_interview",
                "discarded isolated investigation after the user returned to the plan".to_string(),
            );
            return true;
        }

        match result {
            Ok(outcome) => match plan_interview::parse_synthesized_plan(&outcome.merge_response) {
                Some(plan) => {
                    if let AppMode::PlanInterview(state) = &mut self.mode {
                        state.apply_investigation_revision(plan);
                    }
                    self.persist_plan_interview_draft();
                    // A partly failed batch still merges what completed, so the
                    // notice names the gap rather than implying every focus was
                    // researched.
                    self.message = Some(match outcome.failed_focuses.len() {
                        0 => "Investigation findings merged into the draft; review the changes"
                            .to_string(),
                        failed => format!(
                            "Investigation merged with {failed} focus(es) unresearched; review the changes"
                        ),
                    });
                }
                None => {
                    self.log_warn(
                        "plan_interview",
                        format!(
                            "isolated investigation merge returned invalid markdown: {}",
                            truncate_for_log(&outcome.merge_response)
                        ),
                    );
                    if let AppMode::PlanInterview(state) = &mut self.mode {
                        state.fail_investigation();
                    }
                    self.message = Some(
                        "Investigation returned no usable revised plan; your research request is preserved"
                            .into(),
                    );
                }
            },
            Err(error) => {
                self.log_warn(
                    "plan_interview",
                    format!("isolated plan investigation failed: {error}"),
                );
                if let AppMode::PlanInterview(state) = &mut self.mode {
                    state.fail_investigation();
                }
                self.message = Some(
                    "Isolated investigation failed; your research request is preserved".into(),
                );
            }
        }
        true
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
            AppMode::PlanInterview(state) => (
                state.workdir.clone(),
                state.synthesized_plan.clone().unwrap_or_else(|| {
                    render_static_plan(
                        &state.feature_name,
                        &state.brief,
                        &state.questions,
                        &state.answers,
                    )
                }),
                state.interview_key.clone(),
            ),
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

        if let Some(prepared) = pending {
            self.mode = AppMode::Normal;
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
            // On-demand: the feature already exists and is possibly already
            // running, so accepting rewrites its plan rather than launching
            // anything. `interview_key` is the feature's own id here, so the
            // transcript is already filed where a re-run will look for it.
            self.apply_on_demand_plan(&interview_key, &workdir);
            self.finalize_plan_interview_transcript(&interview_key, "", "", &plan);

            let plan_path = workdir.join(".claude").join(PLAN_FILE_NAME);
            // A running agent has no reason to re-read its instruction file, so
            // a plan written underneath it goes unnoticed until something says
            // so. Offer the handoff rather than sending it: the session may be
            // mid-task, and typing into it is the user's call.
            match self.live_agent_session_for_feature(&interview_key) {
                Some((session_id, session_label)) => {
                    if let AppMode::PlanInterview(state) = &mut self.mode {
                        state.offer_kickoff_handoff(PlanKickoffTarget {
                            session_id,
                            session_label,
                            plan_path,
                        });
                    }
                    self.message = None;
                }
                None => {
                    self.mode = AppMode::Normal;
                    self.message = Some(format!("Plan written to {}", plan_path.display()));
                }
            }
            Ok(())
        }
    }

    /// Locate a live agent session for the feature an on-demand plan was just
    /// accepted for, so the kickoff prompt has somewhere to go.
    ///
    /// "Live" means all three: the feature is not stopped, tmux still has the
    /// session, *and* tmux still has the harness's own window. Any of them alone
    /// is stale — AMF's status is only reconciled every few seconds, and a
    /// harness that exited leaves its feature's tmux session up for as long as
    /// the terminal window (or a dev server, or a second harness) outlives it.
    ///
    /// The window check also disambiguates a feature carrying several harnesses:
    /// only a running one can be offered, and the selected session wins over the
    /// first-configured one when both are running.
    ///
    /// Returns the session's id and display label.
    fn live_agent_session_for_feature(&self, feature_id: &str) -> Option<(String, String)> {
        let feature = self
            .store
            .projects
            .iter()
            .flat_map(|project| project.features.iter())
            .find(|feature| feature.id == feature_id)?;

        if feature.status == crate::project::ProjectStatus::Stopped
            || !self.tmux.session_exists(&feature.tmux_session)
        {
            return None;
        }

        // What the user was looking at when they opened the interview is the
        // better guess than declaration order, but only if it is itself running.
        let selected_id = match self.selection {
            Selection::Session(pi, fi, si) => self
                .store
                .projects
                .get(pi)
                .and_then(|project| project.features.get(fi))
                .filter(|candidate| candidate.id == feature_id)
                .and_then(|candidate| candidate.sessions.get(si))
                .map(|session| session.id.as_str()),
            _ => None,
        };

        let session = selected_id
            .and_then(|id| {
                feature.sessions.iter().find(|session| {
                    session.id == id && self.session_is_live_agent(feature, session)
                })
            })
            .or_else(|| {
                feature
                    .sessions
                    .iter()
                    .find(|session| self.session_is_live_agent(feature, session))
            })?;
        Some((session.id.clone(), session.label.clone()))
    }

    /// Whether a session is an agent harness that tmux is still running, checked
    /// against its own window rather than the feature's session as a whole.
    fn session_is_live_agent(
        &self,
        feature: &crate::project::Feature,
        session: &crate::project::FeatureSession,
    ) -> bool {
        session.kind.is_agent_harness()
            && session.kind.is_tmux_backed()
            && self
                .tmux
                .window_exists(&feature.tmux_session, &session.tmux_window)
    }

    /// Hand the accepted plan to the live session: open it and seed its composer
    /// with the same kickoff prompt a freshly launched session gets.
    ///
    /// The prompt is left editable and unsubmitted, like every other compose
    /// seed — the session may be mid-task, and the user decides when it lands.
    pub(crate) fn send_plan_kickoff_to_live_session(&mut self) -> Result<()> {
        let Some(target) = (match &mut self.mode {
            AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::KickoffHandoff => {
                state.kickoff_handoff.take()
            }
            _ => None,
        }) else {
            return Ok(());
        };
        self.mode = AppMode::Normal;

        // Resolved by id rather than reusing indices from the offer: the accept
        // saved the store in between, and seeding the wrong session's composer
        // would be worse than skipping the handoff.
        let Some((pi, fi, si)) =
            self.store
                .projects
                .iter()
                .enumerate()
                .find_map(|(pi, project)| {
                    project
                        .features
                        .iter()
                        .enumerate()
                        .find_map(|(fi, feature)| {
                            feature
                                .sessions
                                .iter()
                                .position(|session| session.id == target.session_id)
                                .map(|si| (pi, fi, si))
                        })
                })
        else {
            self.message = Some(format!(
                "Plan written to {}; its session is gone",
                target.plan_path.display()
            ));
            return Ok(());
        };

        // Re-checked here and not just at the offer: the prompt sits on screen
        // for as long as the user takes to answer it, and the harness can exit
        // in that window. Entering a dead session would recreate it, which is a
        // much bigger action than the one being offered.
        let feature = &self.store.projects[pi].features[fi];
        if !self.session_is_live_agent(feature, &feature.sessions[si]) {
            self.message = Some(format!(
                "Plan written to {}; '{}' is no longer running",
                target.plan_path.display(),
                target.session_label
            ));
            return Ok(());
        }

        self.selection = Selection::Session(pi, fi, si);
        if let Err(e) = self.enter_view_without_auto_compose() {
            self.report_logged_error(
                "plan_interview",
                format!(
                    "could not open '{}' for the plan kickoff prompt ({e}) — the plan is written \
                     to {}",
                    target.session_label,
                    target.plan_path.display()
                ),
            );
            return Ok(());
        }
        // The seed is the whole point of saying yes, so a failure here has to be
        // visible: the session is open but its composer is empty, and silence
        // would read as "the agent has the plan".
        if let Err(e) = self.open_compose_seeded(PLAN_KICKOFF_PROMPT.to_string()) {
            self.report_logged_error(
                "plan_interview",
                format!(
                    "could not seed the plan kickoff prompt ({e}) — tell '{}' to read {} yourself",
                    target.session_label,
                    target.plan_path.display()
                ),
            );
        }
        Ok(())
    }

    /// Decline the handoff: the plan is already written and the instruction
    /// block already points at it, so nothing is lost by leaving the running
    /// session alone.
    pub(crate) fn dismiss_plan_kickoff_handoff(&mut self) {
        let path = match &mut self.mode {
            AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::KickoffHandoff => {
                state.kickoff_handoff.take().map(|target| target.plan_path)
            }
            _ => return,
        };
        self.mode = AppMode::Normal;
        self.message = path.map(|path| format!("Plan written to {}", path.display()));
    }

    /// Make an on-demand plan effective for the feature it was written for.
    ///
    /// Writing `.claude/plan.md` is not enough on its own: unless the harness's
    /// instruction file points at it, the agent is never told the plan exists.
    /// Running the interview is also taken as turning plan mode on, so a later
    /// restart keeps injecting the block instead of silently dropping it.
    ///
    /// Best-effort: the plan file is already on disk by this point, so a store
    /// that will not save must not fail the accept.
    fn apply_on_demand_plan(&mut self, feature_id: &str, workdir: &Path) {
        let Some((pi, fi)) = self
            .store
            .projects
            .iter()
            .enumerate()
            .find_map(|(pi, project)| {
                project
                    .features
                    .iter()
                    .position(|feature| feature.id == feature_id)
                    .map(|fi| (pi, fi))
            })
        else {
            return;
        };

        let feature = &mut self.store.projects[pi].features[fi];
        let agent = feature.agent.clone();
        let already_on = feature.plan_mode;
        feature.plan_mode = true;

        crate::app::setup::ensure_plan_mode_instructions(workdir, &agent, true);

        if !already_on && let Err(e) = self.save() {
            self.log_warn(
                "plan_interview",
                format!("failed to record plan mode for '{feature_id}': {e}"),
            );
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
    fn static_plan_preserves_a_giant_answer_losslessly() {
        let questions = vec![free_text_question("details", "What are the details?")];
        let answer = format!(
            "{}FULL_ANSWER_TAIL",
            "🧰".repeat(crate::plan_interview::MODEL_INPUT_FIELD_MAX_CHARS + 1)
        );

        let plan = render_static_plan(
            "large-input",
            "Keep the complete interview locally.",
            &questions,
            &[Some(answer.clone())],
        );

        assert!(plan.contains(&answer));
        assert!(plan.ends_with("FULL_ANSWER_TAIL\n"));
        assert!(!plan.contains("truncated for model input"));
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
