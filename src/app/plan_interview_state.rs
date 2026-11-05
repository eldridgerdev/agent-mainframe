use super::*;
use crate::db::plan_interviews::{PlanInterviewRecord, PlanInterviewStage};
use crate::editor::TextEditor;
use crate::plan_interview::{
    CUSTOM_ANSWER_MAX_LEN, PlanClarificationQuestion, PlanQuestion, PlanQuestionKind,
    QuestionSource, serialize_choice_answer, split_choice_answer,
};
use crate::project::AgentKind;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// An accepted plan's exact deferred feature launch.
///
/// The resource confirmation owns this after the interview has written the
/// plan but before the feature exists. Keeping the prepared launch and the
/// accepted markdown together lets confirmation resume without rebuilding the
/// wizard state, regenerating the plan, or losing the kickoff prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingPlanLaunch {
    pub prepared: PreparedFeatureLaunch,
    pub interview_key: String,
    pub plan: String,
}

/// Which interview this [`PlanInterviewState`] is running.
///
/// The two share every phase, the round/synthesis machinery, and the Q&A UI
/// — `kind` only steers which [`crate::prompts::PromptId`] is dispatched and
/// how the synthesis response is interpreted (a single markdown plan for
/// `Full`, a three-way outcome for `Quick`). Escalation
/// (`App::escalate_quick_plan_to_full`) flips a live `Quick` interview to
/// `Full` in place — nothing about the state is reset, so the brief and any
/// answers already given carry forward into the full round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanInterviewMode {
    Full,
    Quick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanInterviewPhase {
    /// A saved draft for this interview was found on entry and the user must
    /// choose to resume or discard it before any questions are shown.
    ResumePrompt,
    Brief,
    StaticQuestions,
    /// The static question flow is complete and the user must explicitly
    /// choose whether to spend agent tokens on adaptive follow-ups.
    AiConsent,
    /// A background AI-adaptive round is in flight (`App::poll_plan_interview_ai_bg`).
    /// Question navigation is frozen; `current_question()` returns `None`.
    AiLoading,
    /// The completed interview is being synthesized into structured markdown
    /// by a background headless call.
    SynthesisLoading,
    /// The proposed plan is rendered as markdown and awaits an explicit
    /// accept, edit, regenerate, review, or abort action.
    Review,
    /// The proposed plan is open as raw markdown in the shared text editor.
    Editing,
    /// The user is composing a free-form instruction for the planning agent.
    DirectedFeedback,
    /// A repository-aware, read-only revision from that instruction is in flight.
    DirectedFeedbackLoading,
    /// The user is identifying one or more plan questions that need a focused,
    /// context-isolated repository investigation.
    Investigation,
    /// Fresh read-only investigator contexts are gathering findings, after
    /// which a separate no-tools planning context merges them into the draft.
    InvestigationLoading,
    /// A background headless call is reviewing the draft plan.
    CritiqueLoading,
    /// An agent's advisory review of the draft plan is on screen. The plan
    /// itself is untouched unless the user asks for a revision from here.
    Critique,
    /// An on-demand plan was accepted for a feature whose agent session is
    /// already running, and the user is choosing whether to hand the kickoff
    /// prompt to that live session. The plan is already written by this point,
    /// so both answers are safe — only the handoff is in question.
    KickoffHandoff,
    /// Transient question-flow completion used while app-level code decides
    /// whether to run another adaptive round, synthesize, or use the fallback.
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanInterviewAdvanceError {
    BriefRequired,
    AnswerRequired,
}

/// The live agent session an accepted on-demand plan can be handed off to.
///
/// Identified by session **id** rather than by index: the accept saves the
/// store before the prompt is answered, and resolving the id again at send time
/// means a store that moved underneath cannot seed the wrong session's composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanKickoffTarget {
    pub session_id: String,
    /// The session's display label, so the prompt can name what it will type into.
    pub session_label: String,
    /// Where the plan was just written, shown alongside the offer and reused as
    /// the confirmation message when the handoff is declined.
    pub plan_path: PathBuf,
}

/// How the step on screen compares with the same step's answer in the feature's
/// last accepted interview. Only meaningful on a re-run, which pre-fills those
/// answers so keeping one is the default and changing it is deliberate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorAnswerState {
    /// Still the previously accepted answer, verbatim.
    Kept,
    /// Edited this run; the previous answer is still restorable.
    Changed,
    /// Emptied this run, which records as a skip unless it is restored.
    Cleared,
}

/// Transient return step for a cancelled GUI headless-call notice.
pub(crate) struct PlanPrecallCheckpoint {
    phase: PlanInterviewPhase,
    ai_followups_opted_in: bool,
    skip_ai_rounds: bool,
    synthesis_requested: bool,
}

/// In-memory state for one plan-mode discovery interview.
///
/// `pending_launch` is optional so the same state can also support on-demand
/// interviews for existing features.
pub struct PlanInterviewState {
    pub feature_name: String,
    /// The key this interview's draft and transcript are filed under in the
    /// `plan_interviews` table: the feature's id for an on-demand interview,
    /// or [`crate::plan_interview::pending_interview_key`] while the feature it
    /// plans does not exist yet. Unread for a `Quick` interview: Quick Plan
    /// skips the `plan_interviews` table entirely (see [`Self::kind`]).
    pub interview_key: String,
    /// Full Plan-mode interview or Quick Plan. See [`PlanInterviewMode`].
    pub kind: PlanInterviewMode,
    pub phase: PlanInterviewPhase,
    /// The editable step before a GUI action entered `Done` and opened a
    /// headless-call notice. Continue retains the prepared call state;
    /// cancellation restores this step and its previous token consent. `None`
    /// for a notice no action opened (a poll, or resuming a draft), which
    /// cancels through [`Self::return_to_ai_consent`] instead.
    pub(crate) precall_checkpoint: Option<PlanPrecallCheckpoint>,
    pub questions: Vec<PlanQuestion>,
    pub question_index: usize,
    pub brief: String,
    pub answers: Vec<Option<String>>,
    pub editor: TextEditor,
    /// The highlighted/picked option for the current choice question, or `None`
    /// when nothing is picked — the state a user is in when they answer purely
    /// with custom text. Parallel `selected_option`/`editor` are transient
    /// scratch for the question on screen; the durable record is `answers` (the
    /// serialized combined string) and `custom_answers` (the raw custom text).
    pub selected_option: Option<usize>,
    /// Raw free-text custom answer per question, positionally paired with
    /// `questions`. Empty for a question with no custom text and for every
    /// free-text question. Kept alongside `answers` so revisiting a choice
    /// question restores the radio selection *and* the custom text rather than
    /// a flat editable string, and persisted so a resumed/re-run interview can
    /// do the same.
    pub custom_answers: Vec<String>,
    /// Whether the inline custom-answer editor (the `editor` buffer, reused for
    /// a choice question) currently has focus. `e` opens it; committing or
    /// cancelling returns focus to the option list without submitting.
    pub custom_answer_focused: bool,
    /// The custom-answer buffer captured when the editor was opened, restored
    /// verbatim if the edit is cancelled with `Esc`.
    pub custom_answer_backup: Option<String>,
    /// Where the accepted plan is written (`<workdir>/AMF_PLAN.md`). Held
    /// separately from `pending_launch` because an on-demand interview has an
    /// existing feature's workdir and no launch at all.
    pub workdir: PathBuf,
    pub pending_launch: Option<PreparedFeatureLaunch>,
    pub abort_confirmation: bool,
    /// The feature's configured agent, preferred as the AI-adaptive
    /// interviewer/synthesis engine before
    /// `HeadlessRunner::select_for_interview` falls back to another installed
    /// harness.
    pub preferred_harness: AgentKind,
    /// Resolved lazily on the first AI round attempt. `None` = not yet
    /// resolved; `Some(None)` = resolution was attempted and no
    /// headless-capable harness is available, so remaining AI work falls back
    /// to the raw Q&A plan; `Some(Some(harness))` = the engine powering AI
    /// work for the rest of this interview.
    pub ai_harness: Option<Option<AgentKind>>,
    /// Number of AI rounds that have finished (successfully or not),
    /// checked against [`crate::plan_interview::MAX_AI_ROUNDS`].
    pub ai_rounds_completed: usize,
    /// True only after the user explicitly accepts the token-use prompt.
    /// App-level round dispatch also checks this so no headless call can
    /// start from an accidental `Done` transition.
    pub ai_followups_opted_in: bool,
    /// Set when the user finishes early or declines the token-use prompt so
    /// the `Done` transition skips any remaining AI rounds.
    pub skip_ai_rounds: bool,
    /// Set by the explicit "draft plan now" action. This permits the final
    /// headless pass without opting into adaptive rounds while preserving the
    /// consent screen's guarantee that ordinary completion spends no tokens.
    pub synthesis_requested: bool,
    /// When set, an AI round is in flight; used to render elapsed time on
    /// the `AiLoading` frame. `None` outside `AiLoading`.
    pub ai_round_started_at: Option<std::time::Instant>,
    /// Cheap token estimate for the in-flight round's prompt, shown on the
    /// `AiLoading` frame (`app::pr_review::estimate_tokens` is a chars/4
    /// heuristic, not a harness-reported count — no headless call currently
    /// surfaces real usage).
    pub ai_round_token_estimate: usize,
    /// True once plan synthesis has been started or deliberately bypassed
    /// because no headless engine is available. Prevents a failed plan-file
    /// write from spending tokens again when the user retries completion.
    /// Cleared when a Quick Plan escalates: that pass was triage and left no
    /// plan, so the full interview's synthesis has not been attempted yet.
    pub synthesis_attempted: bool,
    /// The plan currently displayed at the review gate. App-level completion
    /// fills this with either valid synthesized markdown or the raw-Q&A
    /// fallback, and edits replace it before acceptance.
    pub synthesized_plan: Option<String>,
    /// Start time and prompt-size estimate for the synthesis loading frame.
    pub synthesis_started_at: Option<std::time::Instant>,
    pub synthesis_token_estimate: usize,
    /// Cached markdown-viewer layout for the review gate.
    pub review_scroll_offset: usize,
    pub review_rendered_width: u16,
    pub review_rendered_lines: Vec<ratatui::text::Line<'static>>,
    pub review_diagrams: crate::mermaid::DiagramDocument,
    pub edit_scroll_offset: usize,
    pub edit_sync_to_cursor: bool,
    /// Start time and prompt-size estimate for a directed revision. The
    /// instruction itself remains in `editor` while loading so a failed call
    /// can return it intact for retrying or adjustment.
    pub directed_feedback_started_at: Option<std::time::Instant>,
    pub directed_feedback_token_estimate: usize,
    /// Start time and aggregate prompt-size estimate for the isolated
    /// investigation plus its separate no-tools merge pass.
    pub investigation_started_at: Option<std::time::Instant>,
    pub investigation_token_estimate: usize,
    /// An agent's advisory review of the plan currently at the review gate.
    /// Cleared whenever the plan changes, since the findings describe the
    /// draft they were written against.
    pub critique: Option<String>,
    /// Explicit frontier model chosen for this plan's Expert review.
    pub expert_model: Option<String>,
    /// Reasoning level picked with `expert_model`. Not persisted with the draft
    /// (that would need a schema change on the shared `amf.db`), so a resumed
    /// interview falls back to `review_reasoning_for(PlanPreflight)`.
    pub expert_reasoning: Option<crate::headless::ReasoningLevel>,
    /// The picker was confirmed this session, so `expert_reasoning` is
    /// authoritative even when `None` (an explicit "Default"). False for a
    /// resumed draft, where the config level applies instead.
    pub expert_reasoning_picked: bool,
    /// Single-select model picker shown before the Expert pre-call gate.
    pub expert_model_pick: Option<AiModelPickState>,
    /// Durable lifecycle state for the explicitly requested Expert review.
    pub critique_status: Option<String>,
    /// SHA-256 fingerprint of the plan the persisted preflight reviewed.
    pub preflight_fingerprint: Option<String>,
    /// Start time and prompt-size estimate for the agent-review loading frame.
    pub critique_started_at: Option<std::time::Instant>,
    pub critique_token_estimate: usize,
    /// Cached markdown-viewer layout for the advisory review.
    pub critique_scroll_offset: usize,
    pub critique_rendered_width: u16,
    pub critique_rendered_lines: Vec<ratatui::text::Line<'static>>,
    /// Questions extracted from the structured expert preflight response.
    pub critique_questions: Vec<PlanClarificationQuestion>,
    /// User answers paired with `critique_questions`; empty means unanswered.
    pub critique_answers: Vec<String>,
    pub critique_question_index: usize,
    pub critique_answering: bool,
    /// The single clarification follow-up is consumed once it starts.
    pub critique_followup_used: bool,
    /// Advisory review staged as input for the next synthesis pass by the
    /// review's "revise" action. Consumed once that pass actually starts, so a
    /// revision that cannot run leaves the feedback recoverable.
    pub revision_critique: Option<String>,
    /// Bumped whenever `synthesized_plan` changes. A review is written against
    /// one revision, so a result that lands after the plan moved on can be
    /// recognized as stale without keeping a second copy of the plan.
    pub plan_revision: u64,
    /// The `plan_revision` the in-flight or displayed review describes.
    pub critique_plan_revision: Option<u64>,
    /// A saved draft found on entry, held while the user decides whether to
    /// resume or discard it. Taken by [`Self::resume_from_draft`]; dropped by
    /// [`Self::discard_draft`].
    pub resume_draft: Option<PlanInterviewRecord>,
    /// The brief from the feature's last accepted interview, pre-filled as this
    /// run's starting point. `None` unless this is a re-run of a feature that
    /// has an accepted transcript.
    pub prior_brief: Option<String>,
    /// Answers from that transcript, keyed by question id — the stable slug
    /// that survives a config change to the question bank. Kept after
    /// pre-filling so each question can say whether its answer is still the
    /// previous one ([`Self::prior_answer_state`]) and
    /// [`Self::restore_prior_answer`] can put it back.
    pub prior_answers: HashMap<String, String>,
    /// The custom-text half of a re-run's pre-filled choice answers, keyed by
    /// question id, so [`Self::restore_prior_answer`] can put back both the
    /// selection and the elaboration the previous interview accepted.
    pub prior_custom_answers: HashMap<String, String>,
    /// The live session an accepted on-demand plan is being offered to. Only
    /// set in [`PlanInterviewPhase::KickoffHandoff`], which is only reached
    /// after the plan file is already on disk.
    pub kickoff_handoff: Option<PlanKickoffTarget>,
    /// The TODO this interview was started from, if any. Carried so accepting
    /// the plan can record the result on that row — and so the header can say
    /// which TODO is being planned.
    pub todo_origin: Option<TodoPlanOrigin>,
    /// Reference documents the feature owner attached on the brief step, as
    /// canonical absolute paths. Empty is the norm; a non-empty list is the
    /// explicit, opt-in trigger that switches the round / synthesis / critique
    /// passes from a no-tools run to a read-only one so the interviewer can
    /// read those documents (and the surrounding codebase). The files are read
    /// at dispatch time, never here, so an edit between attaching and running
    /// is picked up. Persisted in the interview draft and accepted record.
    pub attached_docs: Vec<PathBuf>,
}

impl PlanInterviewState {
    pub(crate) fn precall_checkpoint(&self) -> PlanPrecallCheckpoint {
        PlanPrecallCheckpoint {
            phase: self.phase,
            ai_followups_opted_in: self.ai_followups_opted_in,
            skip_ai_rounds: self.skip_ai_rounds,
            synthesis_requested: self.synthesis_requested,
        }
    }

    pub(crate) fn restore_precall_checkpoint(&mut self) {
        if let Some(checkpoint) = self.precall_checkpoint.take() {
            self.phase = checkpoint.phase;
            self.ai_followups_opted_in = checkpoint.ai_followups_opted_in;
            self.skip_ai_rounds = checkpoint.skip_ai_rounds;
            self.synthesis_requested = checkpoint.synthesis_requested;
        }
    }

    /// Leave `Done` for the AI consent step with no token consent held.
    ///
    /// The GUI's fallback for a cancelled headless-call notice that no action
    /// checkpointed: one opened by a poll (a round with no usable follow-ups,
    /// a Quick Plan escalation) or by resuming a fully answered draft. There
    /// is no editable step to return to, so this lands on the choice that
    /// follows the answers — the same sibling of `Done` that
    /// [`Self::resume_from_draft`] picks — and withdraws the consent the
    /// cancelled call was acting on, so `Next` there is a real zero-token
    /// path. Spent rounds stay counted. Returns `false` outside `Done`.
    pub(crate) fn return_to_ai_consent(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Done {
            return false;
        }
        self.phase = PlanInterviewPhase::AiConsent;
        self.ai_followups_opted_in = false;
        self.skip_ai_rounds = false;
        self.synthesis_requested = false;
        true
    }

    pub fn for_feature_creation(
        pending_launch: PreparedFeatureLaunch,
        questions: Vec<PlanQuestion>,
    ) -> Self {
        let feature_name = pending_launch
            .feature_name
            .clone()
            .unwrap_or_else(|| pending_launch.branch.clone());
        let interview_key = crate::plan_interview::pending_interview_key(
            &pending_launch.project_name,
            &feature_name,
        );
        Self::new(feature_name, interview_key, questions, Some(pending_launch))
    }

    /// The Quick Plan sibling of [`Self::for_feature_creation`]: always an
    /// empty static question bank (Quick Plan has no built-in question bank;
    /// every question comes from the dynamically-sized adaptive round) and
    /// `kind: PlanInterviewMode::Quick`.
    pub fn for_feature_creation_quick(pending_launch: PreparedFeatureLaunch) -> Self {
        let mut state = Self::for_feature_creation(pending_launch, Vec::new());
        state.kind = PlanInterviewMode::Quick;
        state
    }

    /// An on-demand interview for a feature that already exists: no launch to
    /// defer, and the plan is written into the workdir the feature is already
    /// checked out in. Keyed by the feature's id, which is where an accepted
    /// transcript is filed, so a re-run finds the previous one.
    pub fn for_feature(
        feature_name: String,
        feature_id: String,
        questions: Vec<PlanQuestion>,
        workdir: PathBuf,
        agent: AgentKind,
    ) -> Self {
        let mut state = Self::new(feature_name, feature_id, questions, None);
        state.workdir = workdir;
        state.preferred_harness = agent;
        state
    }

    /// The Quick Plan sibling of [`Self::for_feature`]: always an empty
    /// static question bank and `kind: PlanInterviewMode::Quick`.
    pub fn for_feature_quick(
        feature_name: String,
        feature_id: String,
        workdir: PathBuf,
        agent: AgentKind,
    ) -> Self {
        let mut state = Self::for_feature(feature_name, feature_id, Vec::new(), workdir, agent);
        state.kind = PlanInterviewMode::Quick;
        state
    }

    /// An interview started from a TODO that plans work into the TODO's
    /// **host feature**, which already exists.
    ///
    /// Keyed by the TODO rather than the feature
    /// ([`crate::plan_interview::todo_interview_key`]): the host feature has
    /// its own `P` draft and accepted transcript, and planning a TODO against
    /// it must not overwrite them or be pre-filled from them.
    ///
    /// The brief is pre-filled and the interview opens on it, so the composed
    /// text is something the user edits rather than something they are handed.
    pub fn for_todo(
        feature_name: String,
        origin: TodoPlanOrigin,
        questions: Vec<PlanQuestion>,
        workdir: PathBuf,
        agent: AgentKind,
        brief: String,
    ) -> Self {
        let interview_key = crate::plan_interview::todo_interview_key(&origin.todo_id);
        let mut state = Self::new(feature_name, interview_key, questions, None);
        state.workdir = workdir;
        state.preferred_harness = agent;
        state.editor = TextEditor::new(brief);
        state.todo_origin = Some(origin);
        state
    }

    pub fn new(
        feature_name: String,
        interview_key: String,
        questions: Vec<PlanQuestion>,
        pending_launch: Option<PreparedFeatureLaunch>,
    ) -> Self {
        let answer_count = questions.len();
        let preferred_harness = pending_launch
            .as_ref()
            .map(|prepared| prepared.agent.clone())
            .unwrap_or_default();
        let workdir = pending_launch
            .as_ref()
            .map(|prepared| prepared.workdir.clone())
            .unwrap_or_default();
        Self {
            feature_name,
            interview_key,
            kind: PlanInterviewMode::Full,
            phase: PlanInterviewPhase::Brief,
            precall_checkpoint: None,
            questions,
            question_index: 0,
            brief: String::new(),
            answers: vec![None; answer_count],
            editor: TextEditor::new(String::new()),
            selected_option: None,
            custom_answers: vec![String::new(); answer_count],
            custom_answer_focused: false,
            custom_answer_backup: None,
            workdir,
            pending_launch,
            abort_confirmation: false,
            preferred_harness,
            ai_harness: None,
            ai_rounds_completed: 0,
            ai_followups_opted_in: false,
            skip_ai_rounds: false,
            synthesis_requested: false,
            ai_round_started_at: None,
            ai_round_token_estimate: 0,
            synthesis_attempted: false,
            synthesized_plan: None,
            synthesis_started_at: None,
            synthesis_token_estimate: 0,
            review_scroll_offset: 0,
            review_rendered_width: 0,
            review_rendered_lines: Vec::new(),
            review_diagrams: crate::mermaid::DiagramDocument::default(),
            edit_scroll_offset: 0,
            edit_sync_to_cursor: false,
            directed_feedback_started_at: None,
            directed_feedback_token_estimate: 0,
            investigation_started_at: None,
            investigation_token_estimate: 0,
            critique: None,
            expert_model: None,
            expert_reasoning: None,
            expert_reasoning_picked: false,
            expert_model_pick: None,
            critique_status: None,
            preflight_fingerprint: None,
            critique_started_at: None,
            critique_token_estimate: 0,
            critique_scroll_offset: 0,
            critique_rendered_width: 0,
            critique_rendered_lines: Vec::new(),
            critique_questions: Vec::new(),
            critique_answers: Vec::new(),
            critique_question_index: 0,
            critique_answering: false,
            critique_followup_used: false,
            revision_critique: None,
            plan_revision: 0,
            critique_plan_revision: None,
            resume_draft: None,
            prior_brief: None,
            prior_answers: HashMap::new(),
            prior_custom_answers: HashMap::new(),
            kickoff_handoff: None,
            todo_origin: None,
            attached_docs: Vec::new(),
        }
    }

    /// Ask whether the accepted plan should be handed to the feature's already
    /// running agent session.
    ///
    /// Only reached from an accepted on-demand interview: a feature-creation
    /// interview seeds the session it just launched without asking, and an
    /// on-demand accept with no live session has nothing to hand off to.
    pub fn offer_kickoff_handoff(&mut self, target: PlanKickoffTarget) {
        self.kickoff_handoff = Some(target);
        self.phase = PlanInterviewPhase::KickoffHandoff;
    }

    /// The directory headless interview calls run in and gather repo context
    /// from. Falls back to the process's cwd only when the interview was built
    /// without a workdir, which outside tests means neither a launch nor a
    /// feature was available to take one from.
    pub fn context_workdir(&self) -> PathBuf {
        if self.workdir.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_default()
        } else {
            self.workdir.clone()
        }
    }

    /// Validate a candidate reference document and, on success, add it to
    /// [`Self::attached_docs`], returning its basename for the confirmation
    /// message. Rejections carry the reason.
    pub fn attach_doc(
        &mut self,
        path: &std::path::Path,
    ) -> Result<String, crate::plan_interview::AttachError> {
        let canonical = crate::plan_interview::validate_attachment(path, &self.attached_docs)?;
        let label = canonical
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| canonical.to_string_lossy().into_owned());
        self.attached_docs.push(canonical);
        Ok(label)
    }

    /// Drop the most recently attached reference document, returning its
    /// basename if there was one to drop.
    pub fn remove_last_attached_doc(&mut self) -> Option<String> {
        let path = self.attached_docs.pop()?;
        Some(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        )
    }

    /// Hold a saved draft and ask the user whether to resume it, before any
    /// question is shown. Called on interview entry only, so the answers the
    /// draft would restore cannot overwrite answers given in this session.
    pub fn offer_resume(&mut self, draft: PlanInterviewRecord) {
        self.resume_draft = Some(draft);
        self.phase = PlanInterviewPhase::ResumePrompt;
    }

    /// Restore the held draft's brief, answers, and spent AI rounds, then land
    /// on the first question still unanswered.
    ///
    /// Answers are matched by question id, not position: the built-in bank and
    /// the project's `plan_questions` config may both have changed since the
    /// draft was saved, so anything the current bank no longer asks is dropped
    /// rather than mapped onto the wrong question. Stored AI-generated questions
    /// are appended instead, since those rounds were paid for and the current
    /// bank cannot contain them.
    pub fn resume_from_draft(&mut self) -> bool {
        let Some(draft) = self.resume_draft.take() else {
            return false;
        };

        self.brief = draft.brief.clone();
        self.adopt_recorded_answers(&draft);
        // Restore the attached reference documents verbatim. The paths are only
        // read at headless-dispatch time, where `prepare_attached_docs` drops
        // and reports any that have since moved — re-checking here would just
        // duplicate that, and a still-listed missing file is itself a useful
        // signal on the brief step.
        self.attached_docs = draft
            .attached_docs
            .iter()
            .map(std::path::PathBuf::from)
            .collect();

        self.ai_rounds_completed = draft.ai_rounds_completed;
        // Rounds only run after an explicit opt-in, so a draft that spent one
        // carries that consent forward rather than re-asking for it.
        self.ai_followups_opted_in = draft.ai_rounds_completed > 0;

        // A draft abandoned at the review gate already has a paid-for plan.
        // Resume there rather than walking the questions again and synthesizing
        // a second time.
        if let Some(plan) = draft.plan {
            self.synthesized_plan = Some(plan);
            self.critique = draft.expert_brief;
            self.expert_model = draft.preflight_model;
            self.preflight_fingerprint = draft.preflight_fingerprint;
            self.critique_status = draft.preflight_status;
            self.critique_token_estimate = draft.preflight_token_estimate;
            self.synthesis_attempted = true;
            self.phase = PlanInterviewPhase::Review;
            return true;
        }

        match self
            .answers
            .iter()
            .position(|answer| answer.as_deref().unwrap_or_default().trim().is_empty())
        {
            Some(index) => {
                self.phase = PlanInterviewPhase::StaticQuestions;
                self.question_index = index;
                self.load_current_answer();
            }
            // Every question already has an answer, so there is nothing to
            // resume *into*; go straight to the choice that follows them.
            None if self.ai_followups_opted_in => self.phase = PlanInterviewPhase::Done,
            None => self.phase = PlanInterviewPhase::AiConsent,
        }
        true
    }

    /// Fill this interview's answers from a stored record, matching by question
    /// **id** rather than position: the built-in bank and the project's
    /// `plan_questions` config may both have changed since the record was
    /// written, so anything the current bank no longer asks is dropped rather
    /// than mapped onto the wrong question. The record's AI-generated questions
    /// are appended instead of dropped — those rounds were paid for and the
    /// current bank cannot contain them.
    ///
    /// Matching by id is not enough on its own for a select question: config can
    /// rewrite the same id's options, leaving a stored answer that names a choice
    /// the question no longer offers. That part of the answer is dropped rather
    /// than pre-filled, because it is unselectable in the UI and would otherwise
    /// reach the AI rounds and synthesis attached to the current question text.
    /// A choice answer's custom-text half survives an option rewrite; only the
    /// selection is re-validated.
    fn adopt_recorded_answers(&mut self, record: &PlanInterviewRecord) {
        // Carried AI questions are appended first so the structured pass below
        // covers them too — the record still holds their options, answer, and
        // custom text under the same id.
        let known: HashSet<&str> = self.questions.iter().map(|q| q.id.as_str()).collect();
        let carried: Vec<PlanQuestion> = record
            .questions
            .iter()
            .filter(|question| {
                matches!(question.source, QuestionSource::Ai { .. })
                    && !known.contains(question.id.as_str())
            })
            .cloned()
            .collect();
        self.questions.extend(carried);

        let adopted: Vec<(Option<String>, String)> = self
            .questions
            .iter()
            .map(|question| {
                let Some(raw) = record.answer_for(&question.id) else {
                    return (None, String::new());
                };
                match &question.kind {
                    PlanQuestionKind::FreeText => (Some(raw.to_string()), String::new()),
                    PlanQuestionKind::Select(options) => {
                        let stored_custom = record.custom_answer_for(&question.id);
                        let (indices, custom) = split_choice_answer(raw, stored_custom, options);
                        let labels: Vec<&str> = indices
                            .iter()
                            .filter_map(|&index| options.get(index))
                            .map(String::as_str)
                            .collect();
                        (serialize_choice_answer(&labels, &custom), custom)
                    }
                }
            })
            .collect();
        self.answers = adopted.iter().map(|(answer, _)| answer.clone()).collect();
        self.custom_answers = adopted.into_iter().map(|(_, custom)| custom).collect();
    }

    /// Adopt the feature's last accepted interview as this run's starting point,
    /// so a re-run asks the same questions with the previous answers already in
    /// place: `Enter` keeps one, typing changes it, and
    /// [`Self::restore_prior_answer`] puts a changed one back.
    ///
    /// Returns whether anything was pre-filled, so the caller can say so rather
    /// than announcing a re-run that restored nothing.
    ///
    /// Spent AI rounds are deliberately *not* carried over: this is a new
    /// interview, so it gets its own consent step and its own round budget. The
    /// previous run's AI questions are still asked again (with their answers),
    /// since what the user told the interviewer about this feature is exactly
    /// the context the re-run should start from.
    pub fn apply_previous_transcript(&mut self, record: &PlanInterviewRecord) -> bool {
        self.adopt_recorded_answers(record);
        self.prior_brief = (!record.brief.trim().is_empty()).then(|| record.brief.clone());
        // Read back off the adopted answers rather than the record, so carried AI
        // questions are covered and answers the current bank rejected are not
        // remembered as the baseline: the keep/change note would call an untouched
        // question "changed", and `Ctrl+R` would offer to restore a value that
        // cannot be selected.
        self.prior_answers = self
            .questions
            .iter()
            .zip(self.answers.iter())
            .filter_map(|(question, answer)| {
                answer.clone().map(|answer| (question.id.clone(), answer))
            })
            .collect();
        // The custom-text half of a pre-filled choice answer, so Ctrl+R can put
        // back the elaboration as well as the selection.
        self.prior_custom_answers = self
            .questions
            .iter()
            .zip(self.custom_answers.iter())
            .filter(|(_, custom)| !custom.trim().is_empty())
            .map(|(question, custom)| (question.id.clone(), custom.clone()))
            .collect();

        self.brief = self.prior_brief.clone().unwrap_or_default();
        self.editor = TextEditor::new(self.brief.clone());
        // Carry the previously attached reference documents as this run's
        // starting set, the same way the brief and answers are pre-filled; the
        // user can drop any on the brief step before the first pass.
        self.attached_docs = record
            .attached_docs
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        self.prior_brief.is_some() || !self.prior_answers.is_empty()
    }

    /// Whether a previously accepted interview was pre-filled into this run.
    pub fn has_prior_answers(&self) -> bool {
        self.prior_brief.is_some() || !self.prior_answers.is_empty()
    }

    /// Drop the held draft and start the interview over. The caller deletes the
    /// stored row.
    ///
    /// Resets the collected brief and answers rather than only the held record:
    /// "discard and start over" has to mean the interview begins from its
    /// baseline, whatever the state held when the draft was offered. On a re-run
    /// that baseline is the previously accepted transcript, not a blank
    /// interview — discarding a stale draft must not also throw away the
    /// accepted answers it was revising.
    pub fn discard_draft(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::ResumePrompt {
            return false;
        }
        self.resume_draft = None;
        self.brief = self.prior_brief.clone().unwrap_or_default();
        let baseline = self
            .questions
            .iter()
            .map(|question| self.prior_answers.get(&question.id).cloned())
            .collect();
        self.answers = baseline;
        self.custom_answers = self
            .questions
            .iter()
            .map(|question| {
                self.prior_custom_answers
                    .get(&question.id)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        self.question_index = 0;
        self.selected_option = None;
        self.custom_answer_focused = false;
        self.custom_answer_backup = None;
        self.phase = PlanInterviewPhase::Brief;
        self.editor = TextEditor::new(self.brief.clone());
        true
    }

    /// How the step on screen compares with the previously accepted answer for
    /// it, or `None` when there is no previous answer to compare against.
    pub fn prior_answer_state(&self) -> Option<PriorAnswerState> {
        let (prior, current): (String, String) = match self.phase {
            PlanInterviewPhase::Brief => (
                self.prior_brief.as_deref()?.to_string(),
                self.editor.text().to_string(),
            ),
            PlanInterviewPhase::StaticQuestions => {
                let question = self.questions.get(self.question_index)?;
                let prior = self.prior_answers.get(&question.id)?.clone();
                match &question.kind {
                    PlanQuestionKind::FreeText => (prior, self.editor.text().to_string()),
                    // The whole choice answer — selection plus custom text —
                    // compared as the one serialized string it is stored as, so
                    // adding an elaboration to a kept option reads as "changed".
                    PlanQuestionKind::Select(options) => {
                        let labels: Vec<&str> = self
                            .selected_option
                            .and_then(|index| options.get(index))
                            .map(String::as_str)
                            .into_iter()
                            .collect();
                        let current = serialize_choice_answer(&labels, self.editor.text())
                            .unwrap_or_default();
                        (prior, current)
                    }
                }
            }
            _ => return None,
        };

        Some(if current.trim() == prior.trim() {
            PriorAnswerState::Kept
        } else if current.trim().is_empty() {
            PriorAnswerState::Cleared
        } else {
            PriorAnswerState::Changed
        })
    }

    /// Put the previously accepted answer for the current step back, undoing an
    /// edit made this run. Returns false when there is nothing stored for this
    /// step, so the caller can say so rather than appearing to do nothing.
    pub fn restore_prior_answer(&mut self) -> bool {
        match self.phase {
            PlanInterviewPhase::Brief => {
                let Some(brief) = self.prior_brief.clone() else {
                    return false;
                };
                self.editor = TextEditor::new(brief);
                true
            }
            PlanInterviewPhase::StaticQuestions => {
                let Some((id, kind)) = self
                    .questions
                    .get(self.question_index)
                    .map(|question| (question.id.clone(), question.kind.clone()))
                else {
                    return false;
                };
                let Some(prior) = self.prior_answers.get(&id).cloned() else {
                    return false;
                };
                match &kind {
                    PlanQuestionKind::FreeText => {
                        self.editor = TextEditor::new(prior);
                        true
                    }
                    // Restore both halves the previous interview accepted: the
                    // radio selection and the custom-text elaboration. Adoption
                    // keeps only a selection the question still offers, so an
                    // answer that survives as neither is reported as "nothing
                    // restored" rather than moving the highlight to option 0.
                    PlanQuestionKind::Select(options) => {
                        let prior_custom = self.prior_custom_answers.get(&id).map(String::as_str);
                        let (indices, custom) = split_choice_answer(&prior, prior_custom, options);
                        if indices.is_empty() && custom.trim().is_empty() {
                            return false;
                        }
                        self.selected_option = indices.first().copied();
                        self.editor = TextEditor::new(custom);
                        self.custom_answer_focused = false;
                        self.custom_answer_backup = None;
                        true
                    }
                }
            }
            _ => false,
        }
    }

    /// Snapshot the interview as a draft record for persistence.
    ///
    /// Deliberately a plain snapshot of what has been collected: the caller
    /// decides when a save is worth making, and re-saving the same state is
    /// harmless because the row is keyed by `(feature_id, stage)`.
    pub fn to_draft_record(&self) -> PlanInterviewRecord {
        PlanInterviewRecord {
            feature_id: self.interview_key.clone(),
            stage: PlanInterviewStage::Draft,
            feature_name: self.feature_name.clone(),
            brief: self.brief.clone(),
            questions: self.questions.clone(),
            answers: self.answers.clone(),
            // The custom-text half of every choice answer, so a resumed or
            // re-run interview can restore the selection and the elaboration
            // together. Blank entries persist as `None`.
            custom_answers: self
                .custom_answers
                .iter()
                .map(|custom| (!custom.trim().is_empty()).then(|| custom.clone()))
                .collect(),
            // A draft holds the plan only once one has been generated, so
            // resuming after synthesis does not silently re-spend those tokens.
            plan: self.synthesized_plan.clone(),
            ai_rounds_completed: self.ai_rounds_completed,
            attached_docs: self
                .attached_docs
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            expert_brief: self.critique.clone(),
            preflight_fingerprint: self
                .synthesized_plan
                .as_deref()
                .map(|plan| format!("{:x}", Sha256::digest(plan.as_bytes()))),
            preflight_status: self.critique_status.clone(),
            preflight_model: self.expert_model.clone(),
            preflight_token_estimate: self.critique_token_estimate,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// The round cap `continue_plan_interview_after_done` checks
    /// `ai_rounds_completed` against — [`crate::plan_interview::MAX_QUICK_AI_ROUNDS`]
    /// while `kind` is `Quick`, [`crate::plan_interview::MAX_AI_ROUNDS`] once
    /// escalated (or for a `Full` interview from the start).
    pub fn max_ai_rounds(&self) -> usize {
        match self.kind {
            PlanInterviewMode::Quick => crate::plan_interview::MAX_QUICK_AI_ROUNDS,
            PlanInterviewMode::Full => crate::plan_interview::MAX_AI_ROUNDS,
        }
    }

    /// Move into the `AiLoading` phase while a background round runs.
    pub fn begin_ai_round(&mut self, token_estimate: usize) {
        self.phase = PlanInterviewPhase::AiLoading;
        self.ai_round_started_at = Some(std::time::Instant::now());
        self.ai_round_token_estimate = token_estimate;
    }

    /// Move into the synthesis loading phase while the final plan is
    /// generated off the UI thread.
    pub fn begin_synthesis(&mut self, token_estimate: usize) {
        self.synthesis_attempted = true;
        self.phase = PlanInterviewPhase::SynthesisLoading;
        self.synthesis_started_at = Some(std::time::Instant::now());
        self.synthesis_token_estimate = token_estimate;
    }

    /// Store the synthesized or fallback plan and stop at the review gate.
    ///
    /// A pass that returns the plan already on screen — the "keep the current
    /// plan" path taken when no headless engine is available — is not a plan
    /// change, so any review of that plan stays valid.
    pub fn apply_synthesis(&mut self, plan: String) {
        let changed = self.synthesized_plan.as_deref() != Some(plan.as_str());
        self.synthesis_attempted = true;
        self.synthesized_plan = Some(plan);
        self.synthesis_started_at = None;
        if changed {
            self.mark_plan_changed();
        }
        self.phase = PlanInterviewPhase::Review;
    }

    /// The scroll offset of whatever pane the current phase puts on screen, if
    /// that pane scrolls at all.
    ///
    /// Mouse-wheel events route through here so the wheel moves the plan (or
    /// the advisory review, or an instruction editor) rather than the dashboard
    /// list behind the dialog. `None` is a phase whose body always fits — the
    /// caller still swallows the event so the hidden selection cannot drift.
    /// Every one of these offsets is clamped by the renderer against the
    /// laid-out content, so this only ever has to move it.
    pub fn scroll_offset_mut(&mut self) -> Option<&mut usize> {
        if self.abort_confirmation {
            return None;
        }
        match self.phase {
            PlanInterviewPhase::Review => Some(&mut self.review_scroll_offset),
            PlanInterviewPhase::Critique => Some(&mut self.critique_scroll_offset),
            PlanInterviewPhase::Editing
            | PlanInterviewPhase::DirectedFeedback
            | PlanInterviewPhase::Investigation => Some(&mut self.edit_scroll_offset),
            _ => None,
        }
    }

    /// Open a blank multi-line instruction editor from the review gate.
    pub fn begin_directed_feedback(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Review || self.synthesized_plan.is_none() {
            return false;
        }
        self.editor = TextEditor::new(String::new());
        self.edit_scroll_offset = 0;
        self.edit_sync_to_cursor = true;
        self.phase = PlanInterviewPhase::DirectedFeedback;
        true
    }

    /// Freeze the directed-feedback editor while a read-only agent pass runs.
    pub fn begin_directed_feedback_loading(&mut self, token_estimate: usize) -> bool {
        if self.phase != PlanInterviewPhase::DirectedFeedback
            || self.synthesized_plan.is_none()
            || self.editor.text().trim().is_empty()
        {
            return false;
        }
        self.phase = PlanInterviewPhase::DirectedFeedbackLoading;
        self.directed_feedback_started_at = Some(std::time::Instant::now());
        self.directed_feedback_token_estimate = token_estimate;
        true
    }

    /// Return to the instruction editor after a failed revision, preserving
    /// what the user wrote so retrying does not require retyping it.
    pub fn fail_directed_feedback(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::DirectedFeedbackLoading {
            return false;
        }
        self.directed_feedback_started_at = None;
        self.phase = PlanInterviewPhase::DirectedFeedback;
        true
    }

    /// Leave directed feedback without changing the draft plan.
    pub fn cancel_directed_feedback(&mut self) -> bool {
        if !matches!(
            self.phase,
            PlanInterviewPhase::DirectedFeedback | PlanInterviewPhase::DirectedFeedbackLoading
        ) {
            return false;
        }
        self.directed_feedback_started_at = None;
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// Open a blank editor for research questions or plan sections that need
    /// repository evidence before the plan is accepted.
    pub fn begin_investigation(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Review || self.synthesized_plan.is_none() {
            return false;
        }
        self.editor = TextEditor::new(String::new());
        self.edit_scroll_offset = 0;
        self.edit_sync_to_cursor = true;
        self.phase = PlanInterviewPhase::Investigation;
        true
    }

    /// Freeze the research-focus editor while isolated investigators and the
    /// final no-tools merge pass run in the background.
    pub fn begin_investigation_loading(&mut self, token_estimate: usize) -> bool {
        if self.phase != PlanInterviewPhase::Investigation
            || self.synthesized_plan.is_none()
            || self.editor.text().trim().is_empty()
        {
            return false;
        }
        self.phase = PlanInterviewPhase::InvestigationLoading;
        self.investigation_started_at = Some(std::time::Instant::now());
        self.investigation_token_estimate = token_estimate;
        true
    }

    /// Return to the research-focus editor after any investigator or merge
    /// failure, preserving the request so the user can retry or narrow it.
    pub fn fail_investigation(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::InvestigationLoading {
            return false;
        }
        self.investigation_started_at = None;
        self.phase = PlanInterviewPhase::Investigation;
        true
    }

    /// Leave the optional investigation without changing the draft plan.
    pub fn cancel_investigation(&mut self) -> bool {
        if !matches!(
            self.phase,
            PlanInterviewPhase::Investigation | PlanInterviewPhase::InvestigationLoading
        ) {
            return false;
        }
        self.investigation_started_at = None;
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// Apply the context-isolated merge result and return to the review gate.
    pub fn apply_investigation_revision(&mut self, plan: String) {
        self.investigation_started_at = None;
        self.apply_synthesis(plan);
    }

    /// Move into the agent-review loading phase. Returns false outside the
    /// review gate so a stray keypress cannot start a paid call from a phase
    /// that has no plan to review.
    pub fn begin_critique(&mut self, token_estimate: usize) -> bool {
        if self.phase != PlanInterviewPhase::Review || self.synthesized_plan.is_none() {
            return false;
        }
        self.phase = PlanInterviewPhase::CritiqueLoading;
        self.critique_status = Some("running".into());
        self.critique_followup_used = false;
        self.critique_started_at = Some(std::time::Instant::now());
        self.critique_token_estimate = token_estimate;
        self.critique_plan_revision = Some(self.plan_revision);
        true
    }

    /// Show a finished advisory review. The plan is deliberately untouched.
    pub fn apply_critique(&mut self, critique: String, questions: Vec<PlanClarificationQuestion>) {
        self.critique = Some(critique);
        self.critique_status = Some("completed".into());
        self.critique_answers = vec![String::new(); questions.len()];
        self.critique_question_index = 0;
        self.critique_answering = false;
        self.critique_questions = questions;
        self.critique_started_at = None;
        self.critique_scroll_offset = 0;
        self.critique_rendered_width = 0;
        self.critique_rendered_lines.clear();
        self.critique_plan_revision = Some(self.plan_revision);
        self.phase = PlanInterviewPhase::Critique;
    }

    /// Keep a review that finished after the user dismissed it, without
    /// pulling them back into it. Returns false when there is nothing to keep
    /// or the plan moved on while the review was in flight, since the findings
    /// then describe a draft that is gone.
    pub fn stash_critique(
        &mut self,
        critique: String,
        questions: Vec<PlanClarificationQuestion>,
    ) -> bool {
        if self.phase != PlanInterviewPhase::Review
            || self.critique.is_some()
            || self.critique_plan_revision != Some(self.plan_revision)
        {
            return false;
        }
        self.critique = Some(critique);
        self.critique_answers = vec![String::new(); questions.len()];
        self.critique_question_index = 0;
        self.critique_answering = false;
        self.critique_questions = questions;
        self.critique_started_at = None;
        self.critique_scroll_offset = 0;
        self.critique_rendered_width = 0;
        self.critique_rendered_lines.clear();
        true
    }

    /// Re-open the review already held for the current plan. This is what
    /// makes a dismissed review recoverable instead of leaving the user to pay
    /// for an identical second call.
    pub fn reopen_critique(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Review || self.critique.is_none() {
            return false;
        }
        self.phase = PlanInterviewPhase::Critique;
        true
    }

    /// Begin collecting answers to the bounded expert clarification set.
    pub fn begin_critique_answers(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Critique
            || self.critique_questions.is_empty()
            || self.critique_followup_used
        {
            return false;
        }
        self.critique_question_index = 0;
        self.editor = TextEditor::new(self.critique_answers[0].clone());
        self.critique_answering = true;
        true
    }

    /// Save the current clarification answer and advance through the bounded set.
    pub fn save_critique_answer(&mut self) -> bool {
        if !self.critique_answering {
            return false;
        }
        self.critique_answers[self.critique_question_index] = self.editor.text().to_string();
        if self.critique_question_index + 1 >= self.critique_questions.len() {
            self.critique_answering = false;
        } else {
            self.critique_question_index += 1;
            self.editor =
                TextEditor::new(self.critique_answers[self.critique_question_index].clone());
        }
        true
    }

    pub fn begin_critique_followup(&mut self, token_estimate: usize) -> bool {
        if self.phase != PlanInterviewPhase::Critique
            || self.critique_questions.is_empty()
            || self.critique_followup_used
            || !self
                .critique_answers
                .iter()
                .any(|answer| !answer.trim().is_empty())
        {
            return false;
        }
        self.critique_followup_used = true;
        self.phase = PlanInterviewPhase::CritiqueLoading;
        self.critique_started_at = Some(std::time::Instant::now());
        self.critique_token_estimate = token_estimate;
        self.critique_plan_revision = Some(self.plan_revision);
        true
    }

    /// Return to the plan from the advisory review, or from a review still in
    /// flight — a result that arrives after this is stashed rather than shown.
    pub fn close_critique(&mut self) -> bool {
        if !matches!(
            self.phase,
            PlanInterviewPhase::Critique | PlanInterviewPhase::CritiqueLoading
        ) {
            return false;
        }
        self.critique_started_at = None;
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// Stage the advisory review as input for the next synthesis pass. The
    /// caller starts that pass; until it lands the plan is unchanged.
    pub fn revise_from_critique(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Critique {
            return false;
        }
        let Some(critique) = self.critique.clone() else {
            return false;
        };
        self.revision_critique = Some(critique);
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// The advisory review staged for the next synthesis pass, if any. Read
    /// without consuming so a pass that turns out to be impossible leaves the
    /// feedback where the user can still reach it.
    pub fn staged_revision_critique(&self) -> Option<&str> {
        self.revision_critique.as_deref()
    }

    /// Take the staged revision feedback, leaving none behind so a later
    /// regenerate is a clean pass rather than a repeat of the same revision.
    /// Called only once the revision pass has actually started.
    pub fn take_revision_critique(&mut self) -> Option<String> {
        self.revision_critique.take()
    }

    /// Record that the plan on screen is a different plan: reset its rendered
    /// layout and drop an advisory review that no longer describes it.
    fn mark_plan_changed(&mut self) {
        self.plan_revision = self.plan_revision.wrapping_add(1);
        self.review_scroll_offset = 0;
        self.review_rendered_width = 0;
        self.review_rendered_lines.clear();
        self.clear_critique();
    }

    /// Drop an advisory review that no longer describes the current plan.
    fn clear_critique(&mut self) {
        self.critique = None;
        self.expert_model = None;
        self.expert_reasoning = None;
        self.expert_reasoning_picked = false;
        self.expert_model_pick = None;
        self.critique_status = None;
        self.preflight_fingerprint = None;
        self.critique_questions.clear();
        self.critique_answers.clear();
        self.critique_question_index = 0;
        self.critique_answering = false;
        self.critique_followup_used = false;
        self.critique_started_at = None;
        self.critique_scroll_offset = 0;
        self.critique_rendered_width = 0;
        self.critique_rendered_lines.clear();
        self.critique_plan_revision = None;
    }

    /// Open the reviewed plan as raw markdown without changing the staged
    /// plan until the user explicitly saves the edit.
    pub fn begin_plan_edit(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Review {
            return false;
        }
        let Some(plan) = self.synthesized_plan.clone() else {
            return false;
        };
        self.editor = TextEditor::new(plan);
        self.edit_scroll_offset = 0;
        self.edit_sync_to_cursor = true;
        self.phase = PlanInterviewPhase::Editing;
        true
    }

    /// Save the raw markdown edit and return to the rendered preview.
    /// Empty plans are rejected so acceptance can never write a blank file.
    pub fn save_plan_edit(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Editing || self.editor.text().trim().is_empty() {
            return false;
        }
        let mut plan = self.editor.text().to_string();
        if !plan.ends_with('\n') {
            plan.push('\n');
        }
        let changed = self.synthesized_plan.as_deref() != Some(plan.as_str());
        self.synthesized_plan = Some(plan);
        if changed {
            self.mark_plan_changed();
        }
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// Discard the editor buffer and return to the last rendered plan.
    pub fn cancel_plan_edit(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Editing {
            return false;
        }
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// Explicitly accept the optional token-spending AI follow-up stage.
    /// Returns false outside the consent screen so callers cannot opt in
    /// accidentally from an ordinary answer editor.
    pub fn opt_in_ai_followups(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::AiConsent {
            return false;
        }
        self.ai_followups_opted_in = true;
        self.skip_ai_rounds = false;
        self.phase = PlanInterviewPhase::Done;
        true
    }

    /// Apply a finished AI round's parsed follow-up questions.
    ///
    /// With no usable follow-ups, moves straight back to `Done` so the
    /// caller can decide whether to try another round or complete. With
    /// follow-ups, appends them to the question list and resumes the
    /// question flow at the first new one.
    pub fn apply_ai_round(&mut self, round: usize, new_questions: Vec<PlanQuestion>) {
        self.ai_round_started_at = None;
        self.ai_rounds_completed = round;
        if new_questions.is_empty() {
            self.phase = PlanInterviewPhase::Done;
            return;
        }
        let first_new_index = self.questions.len();
        self.answers.extend(new_questions.iter().map(|_| None));
        self.custom_answers
            .extend(new_questions.iter().map(|_| String::new()));
        self.questions.extend(new_questions);
        self.phase = PlanInterviewPhase::StaticQuestions;
        self.question_index = first_new_index;
        self.load_current_answer();
    }

    pub fn current_question(&self) -> Option<&PlanQuestion> {
        if self.phase == PlanInterviewPhase::StaticQuestions {
            self.questions.get(self.question_index)
        } else {
            None
        }
    }

    pub fn fail_critique(&mut self) {
        self.critique_status = Some("failed".into());
        self.critique_started_at = None;
        if self.phase == PlanInterviewPhase::CritiqueLoading {
            self.phase = PlanInterviewPhase::Review;
        }
    }

    fn current_option_count(&self) -> usize {
        self.current_question()
            .and_then(|question| match &question.kind {
                PlanQuestionKind::Select(options) => Some(options.len()),
                PlanQuestionKind::FreeText => None,
            })
            .unwrap_or(0)
    }

    /// Move the option highlight up, wrapping. From "nothing picked" this lands
    /// on the last option — the arrows always settle on a real choice; leaving
    /// the options untouched is how a user answers with custom text alone.
    pub fn select_previous_option(&mut self) {
        let option_count = self.current_option_count();
        if option_count == 0 {
            return;
        }
        self.selected_option = Some(match self.selected_option {
            None | Some(0) => option_count - 1,
            Some(index) => index - 1,
        });
    }

    pub fn select_next_option(&mut self) {
        let option_count = self.current_option_count();
        if option_count == 0 {
            return;
        }
        self.selected_option = Some(match self.selected_option {
            None => 0,
            Some(index) => (index + 1) % option_count,
        });
    }

    /// Return a choice question to the "nothing picked" state. Because the arrow
    /// keys only ever move between real options, this is the sole way back once
    /// a pick has been made — and "nothing picked" is a real answer: it is how a
    /// user submits with custom text alone. Returns `false` when the current
    /// question is not a choice or nothing was picked.
    pub fn clear_option_selection(&mut self) -> bool {
        if self.current_option_count() == 0 || self.selected_option.is_none() {
            return false;
        }
        self.selected_option = None;
        true
    }

    /// Save the current input and move to the next interview step.
    pub fn advance(&mut self) -> Result<(), PlanInterviewAdvanceError> {
        match self.phase {
            PlanInterviewPhase::Brief => {
                if self.editor.text().trim().is_empty() {
                    return Err(PlanInterviewAdvanceError::BriefRequired);
                }
                self.brief = self.editor.text().to_string();
                if self.questions.is_empty() {
                    self.phase = PlanInterviewPhase::AiConsent;
                } else {
                    self.phase = PlanInterviewPhase::StaticQuestions;
                    self.question_index = 0;
                    self.load_current_answer();
                }
            }
            PlanInterviewPhase::StaticQuestions => {
                self.save_current_answer(false)?;
                self.move_after_current_question();
            }
            PlanInterviewPhase::AiConsent => {
                // Enter is deliberately the no-token default. Opting in uses
                // the dedicated `a` action and `opt_in_ai_followups`.
                self.skip_ai_rounds = true;
                self.phase = PlanInterviewPhase::Done;
            }
            // The resume choice has its own dedicated keys; Enter must not fall
            // through to a question flow whose answers are not loaded yet. The
            // handoff prompt is past acceptance entirely.
            PlanInterviewPhase::ResumePrompt
            | PlanInterviewPhase::AiLoading
            | PlanInterviewPhase::SynthesisLoading
            | PlanInterviewPhase::Review
            | PlanInterviewPhase::Editing
            | PlanInterviewPhase::DirectedFeedback
            | PlanInterviewPhase::DirectedFeedbackLoading
            | PlanInterviewPhase::Investigation
            | PlanInterviewPhase::InvestigationLoading
            | PlanInterviewPhase::CritiqueLoading
            | PlanInterviewPhase::Critique
            | PlanInterviewPhase::KickoffHandoff
            | PlanInterviewPhase::Done => {}
        }
        Ok(())
    }

    /// Skip an optional question, or decline the optional AI follow-up stage.
    pub fn skip(&mut self) -> Result<(), PlanInterviewAdvanceError> {
        if self.phase == PlanInterviewPhase::AiConsent {
            self.skip_ai_rounds = true;
            self.phase = PlanInterviewPhase::Done;
            return Ok(());
        }
        let Some(question) = self.current_question() else {
            return Ok(());
        };
        if !question.optional {
            return Err(PlanInterviewAdvanceError::AnswerRequired);
        }
        self.answers[self.question_index] = None;
        self.move_after_current_question();
        Ok(())
    }

    /// Return to the previous step, restoring its draft answer into the editor.
    pub fn back(&mut self) -> bool {
        match self.phase {
            PlanInterviewPhase::Brief => false,
            PlanInterviewPhase::StaticQuestions if self.question_index == 0 => {
                self.save_current_draft();
                self.phase = PlanInterviewPhase::Brief;
                self.editor = TextEditor::new(self.brief.clone());
                self.selected_option = None;
                self.custom_answer_focused = false;
                self.custom_answer_backup = None;
                true
            }
            PlanInterviewPhase::StaticQuestions => {
                self.save_current_draft();
                self.question_index -= 1;
                self.load_current_answer();
                true
            }
            PlanInterviewPhase::AiConsent if !self.questions.is_empty() => {
                self.phase = PlanInterviewPhase::StaticQuestions;
                self.question_index = self.questions.len() - 1;
                self.load_current_answer();
                true
            }
            PlanInterviewPhase::AiConsent => {
                self.phase = PlanInterviewPhase::Brief;
                self.editor = TextEditor::new(self.brief.clone());
                true
            }
            PlanInterviewPhase::Done if !self.questions.is_empty() => {
                self.phase = PlanInterviewPhase::StaticQuestions;
                self.question_index = self.questions.len() - 1;
                self.load_current_answer();
                true
            }
            PlanInterviewPhase::Done => {
                self.phase = PlanInterviewPhase::Brief;
                self.editor = TextEditor::new(self.brief.clone());
                true
            }
            // Loading is a transient App-driven state; there is nothing to
            // navigate back to until it resolves. The review-gate phases have
            // their own dedicated navigation, the resume choice is the first
            // screen of the interview, and the handoff prompt comes after an
            // accept that already wrote the plan.
            PlanInterviewPhase::ResumePrompt
            | PlanInterviewPhase::AiLoading
            | PlanInterviewPhase::SynthesisLoading
            | PlanInterviewPhase::Review
            | PlanInterviewPhase::Editing
            | PlanInterviewPhase::DirectedFeedback
            | PlanInterviewPhase::DirectedFeedbackLoading
            | PlanInterviewPhase::Investigation
            | PlanInterviewPhase::InvestigationLoading
            | PlanInterviewPhase::CritiqueLoading
            | PlanInterviewPhase::Critique
            | PlanInterviewPhase::KickoffHandoff => false,
        }
    }

    /// End questioning with the answers collected so far, skip any remaining
    /// adaptive rounds, and explicitly request plan synthesis.
    pub fn finish_early(&mut self) -> Result<(), PlanInterviewAdvanceError> {
        match self.phase {
            PlanInterviewPhase::Brief => {
                if self.editor.text().trim().is_empty() {
                    return Err(PlanInterviewAdvanceError::BriefRequired);
                }
                self.brief = self.editor.text().to_string();
            }
            PlanInterviewPhase::StaticQuestions => self.save_current_draft(),
            PlanInterviewPhase::AiConsent => {}
            // Do not overlap paid calls or mutate a retryable completed
            // synthesis. The UI does not advertise this action while loading,
            // nor at the resume choice, which has no brief to synthesize yet,
            // nor at the handoff prompt, whose plan is already accepted.
            PlanInterviewPhase::ResumePrompt
            | PlanInterviewPhase::AiLoading
            | PlanInterviewPhase::SynthesisLoading
            | PlanInterviewPhase::Review
            | PlanInterviewPhase::Editing
            | PlanInterviewPhase::DirectedFeedback
            | PlanInterviewPhase::DirectedFeedbackLoading
            | PlanInterviewPhase::Investigation
            | PlanInterviewPhase::InvestigationLoading
            | PlanInterviewPhase::CritiqueLoading
            | PlanInterviewPhase::Critique
            | PlanInterviewPhase::KickoffHandoff
            | PlanInterviewPhase::Done => return Ok(()),
        }
        self.ai_round_started_at = None;
        self.skip_ai_rounds = true;
        self.synthesis_requested = true;
        self.phase = PlanInterviewPhase::Done;
        Ok(())
    }

    fn save_current_answer(
        &mut self,
        allow_empty_optional: bool,
    ) -> Result<(), PlanInterviewAdvanceError> {
        let Some(question) = self.questions.get(self.question_index) else {
            return Ok(());
        };
        let answer = match &question.kind {
            PlanQuestionKind::FreeText => {
                let text = self.editor.text();
                if text.trim().is_empty() {
                    None
                } else {
                    Some(text.to_string())
                }
            }
            // A choice answer is the picked option label(s) and the trimmed
            // custom text, combined into one plain string. Nothing picked and
            // blank custom text records as no answer — which the gate below
            // blocks for a required question, exactly as for free text.
            PlanQuestionKind::Select(options) => {
                let custom = self.editor.text().trim().to_string();
                if let Some(slot) = self.custom_answers.get_mut(self.question_index) {
                    *slot = custom.clone();
                }
                let labels: Vec<&str> = self
                    .selected_option
                    .and_then(|index| options.get(index))
                    .map(String::as_str)
                    .into_iter()
                    .collect();
                serialize_choice_answer(&labels, &custom)
            }
        };
        if answer.is_none() && !question.optional && !allow_empty_optional {
            return Err(PlanInterviewAdvanceError::AnswerRequired);
        }
        self.answers[self.question_index] = answer;
        Ok(())
    }

    fn save_current_draft(&mut self) {
        let _ = self.save_current_answer(true);
    }

    fn move_after_current_question(&mut self) {
        if self.question_index + 1 >= self.questions.len() {
            self.phase = if self.ai_followups_opted_in {
                PlanInterviewPhase::Done
            } else {
                PlanInterviewPhase::AiConsent
            };
        } else {
            self.question_index += 1;
            self.load_current_answer();
        }
    }

    /// Load the answer stored for the question now on screen into the transient
    /// `selected_option` / `editor` scratch. For a choice question this rebuilds
    /// the structured control from the serialized string and the stored custom
    /// text, so revisiting an answered question shows the radio selection and
    /// the custom-text box — never a flat editable string.
    pub(super) fn load_current_answer(&mut self) {
        let existing = self
            .answers
            .get(self.question_index)
            .and_then(|answer| answer.clone());
        let stored_custom = self
            .custom_answers
            .get(self.question_index)
            .cloned()
            .unwrap_or_default();
        self.custom_answer_focused = false;
        self.custom_answer_backup = None;
        match self.questions.get(self.question_index).map(|q| &q.kind) {
            Some(PlanQuestionKind::FreeText) => {
                self.editor = TextEditor::new(existing.unwrap_or_default());
                self.selected_option = None;
            }
            Some(PlanQuestionKind::Select(options)) => {
                let stored = (!stored_custom.is_empty()).then_some(stored_custom.as_str());
                let (indices, custom) = match existing.as_deref() {
                    Some(combined) => split_choice_answer(combined, stored, options),
                    None => (Vec::new(), stored_custom.clone()),
                };
                self.selected_option = indices.first().copied();
                self.editor = TextEditor::new(custom);
            }
            None => {}
        }
    }

    /// Open the inline custom-answer editor for the choice question on screen.
    /// The current buffer is stashed so `Esc` can restore it; `false` when the
    /// phase or question is wrong or the editor is already focused.
    pub fn open_custom_answer_editor(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::StaticQuestions || self.custom_answer_focused {
            return false;
        }
        if !matches!(
            self.current_question().map(|q| &q.kind),
            Some(PlanQuestionKind::Select(_))
        ) {
            return false;
        }
        self.custom_answer_backup = Some(self.editor.text().to_string());
        self.custom_answer_focused = true;
        true
    }

    /// Commit the custom-answer edit: trim the buffer, record it, and return
    /// focus to the option list without submitting the question. The serialized
    /// answer is refreshed too, so a draft persisted right after a commit
    /// round-trips even though the question has not been advanced through.
    pub fn commit_custom_answer(&mut self) {
        if !self.custom_answer_focused {
            return;
        }
        let trimmed = self.editor.text().trim().to_string();
        self.editor = TextEditor::new(trimmed.clone());
        if let Some(slot) = self.custom_answers.get_mut(self.question_index) {
            *slot = trimmed.clone();
        }
        let recorded = match self.questions.get(self.question_index).map(|q| &q.kind) {
            Some(PlanQuestionKind::Select(options)) => {
                let labels: Vec<&str> = self
                    .selected_option
                    .and_then(|index| options.get(index))
                    .map(String::as_str)
                    .into_iter()
                    .collect();
                Some(serialize_choice_answer(&labels, &trimmed))
            }
            _ => None,
        };
        if let Some(answer) = recorded
            && let Some(slot) = self.answers.get_mut(self.question_index)
        {
            *slot = answer;
        }
        self.custom_answer_focused = false;
        self.custom_answer_backup = None;
    }

    /// Abandon the custom-answer edit, restoring the buffer captured when it was
    /// opened.
    pub fn cancel_custom_answer(&mut self) {
        if !self.custom_answer_focused {
            return;
        }
        let restore = self.custom_answer_backup.take().unwrap_or_default();
        self.editor = TextEditor::new(restore);
        self.custom_answer_focused = false;
    }

    /// Forward a key to the focused custom-answer editor, enforcing the
    /// character cap by reverting any edit that would exceed it (a paste, or a
    /// keystroke at the limit).
    pub fn custom_answer_handle_key(&mut self, key: crossterm::event::KeyEvent) {
        if !self.custom_answer_focused {
            return;
        }
        let before = self.editor.text().to_string();
        self.editor.handle_key(key);
        if self.editor.text().chars().count() > CUSTOM_ANSWER_MAX_LEN {
            self.editor = TextEditor::new(before);
        }
    }

    /// Whether the question on screen is a choice question (so the custom-answer
    /// box is shown and `e` is bound).
    pub fn current_question_is_choice(&self) -> bool {
        matches!(
            self.current_question().map(|q| &q.kind),
            Some(PlanQuestionKind::Select(_))
        )
    }
}
