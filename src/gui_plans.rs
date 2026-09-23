//! GUI adapter for the existing Plan Interview state machine. The TUI's
//! handlers and this module call the same `PlanInterviewState` transitions and
//! `App` completion methods; neither interface owns a second plan engine.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::app::TodoPlanOrigin;
use crate::app::resource_gate::{StartPreconditions, describe_tripped};
use crate::app::{App, AppMode, PlanInterviewAdvanceError, PlanInterviewPhase, Selection};
use crate::automation::CreateFeatureRequest;
use crate::db::todos::{TodoScope, TodoStatus};
use crate::editor::TextEditor;
use crate::gui_contract::{
    FeatureTarget, GuiError, GuiErrorKind, GuiHandle, GuiResult, SessionTarget,
};
use crate::plan_interview::{CUSTOM_ANSWER_MAX_LEN, PlanQuestionKind};

#[derive(Debug, Clone, Serialize)]
pub struct PlanQuestionView {
    pub id: String,
    pub text: String,
    pub optional: bool,
    pub options: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanView {
    pub interview_key: String,
    pub feature_name: String,
    pub kind: String,
    pub phase: String,
    /// Rejects an action submitted after navigation or an AI completion has
    /// already moved the interview to a different step.
    pub step_key: String,
    pub question_index: usize,
    pub question_count: usize,
    pub question: Option<PlanQuestionView>,
    pub editor_text: String,
    pub selected_option: Option<usize>,
    pub review_markdown: Option<String>,
    pub critique: Option<String>,
    pub attached_docs: Vec<String>,
    pub kickoff_target: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanStatus {
    pub active: Option<PlanView>,
    pub precall: Option<PrecallView>,
    pub message: Option<String>,
    pub handoff: Option<PlanHandoff>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanHandoff {
    pub target: SessionTarget,
    pub draft_prompt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrecallView {
    pub title: String,
    pub harness: String,
    pub preview: String,
    pub viewing: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanInput {
    pub text: String,
    pub selected_option: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanAction {
    Resume,
    DiscardDraft,
    Next,
    Back,
    Skip,
    FinishEarly,
    OptInAi,
    BeginEdit,
    SaveEdit,
    CancelEdit,
    Regenerate,
    RequestCritique,
    CloseCritique,
    ReviseFromCritique,
    BeginFeedback,
    SubmitFeedback,
    CancelFeedback,
    BeginInvestigation,
    SubmitInvestigation,
    CancelInvestigation,
    RestorePrior,
    AttachDoc,
    RemoveDoc,
    Accept,
    AcceptApproved,
    Cancel,
    KickoffAccept,
    KickoffDecline,
    PrecallConfirm,
    PrecallCancel,
    PrecallToggleView,
}

fn phase_name(phase: PlanInterviewPhase) -> &'static str {
    match phase {
        PlanInterviewPhase::ResumePrompt => "resume_prompt",
        PlanInterviewPhase::Brief => "brief",
        PlanInterviewPhase::StaticQuestions => "static_questions",
        PlanInterviewPhase::AiConsent => "ai_consent",
        PlanInterviewPhase::AiLoading => "ai_loading",
        PlanInterviewPhase::SynthesisLoading => "synthesis_loading",
        PlanInterviewPhase::Review => "review",
        PlanInterviewPhase::Editing => "editing",
        PlanInterviewPhase::DirectedFeedback => "directed_feedback",
        PlanInterviewPhase::DirectedFeedbackLoading => "directed_feedback_loading",
        PlanInterviewPhase::Investigation => "investigation",
        PlanInterviewPhase::InvestigationLoading => "investigation_loading",
        PlanInterviewPhase::CritiqueLoading => "critique_loading",
        PlanInterviewPhase::Critique => "critique",
        PlanInterviewPhase::KickoffHandoff => "kickoff_handoff",
        PlanInterviewPhase::Done => "done",
    }
}

fn status_of(app: &App) -> PlanStatus {
    let plan_mode = match &app.mode {
        AppMode::PlanInterview(_) => Some(&app.mode),
        AppMode::PromptPrecall(pending)
            if matches!(pending.prior_mode.as_ref(), AppMode::PlanInterview(_)) =>
        {
            Some(pending.prior_mode.as_ref())
        }
        _ => None,
    };
    let active = match plan_mode {
        Some(AppMode::PlanInterview(state)) => {
            let question = state.current_question().map(|question| PlanQuestionView {
                id: question.id.clone(),
                text: question.text.clone(),
                optional: question.optional,
                options: match &question.kind {
                    PlanQuestionKind::FreeText => None,
                    PlanQuestionKind::Select(options) => Some(options.clone()),
                },
            });
            let phase = phase_name(state.phase).to_string();
            Some(PlanView {
                interview_key: state.interview_key.clone(),
                feature_name: state.feature_name.clone(),
                kind: match state.kind {
                    crate::app::PlanInterviewMode::Full => "full",
                    crate::app::PlanInterviewMode::Quick => "quick",
                }
                .into(),
                step_key: format!(
                    "{}:{}:{}:{}:docs{}",
                    state.interview_key,
                    phase,
                    state.question_index,
                    state.plan_revision,
                    state.attached_docs.len()
                ),
                phase,
                question_index: state.question_index,
                question_count: state.questions.len(),
                question,
                editor_text: state.editor.text().to_string(),
                selected_option: state.selected_option,
                review_markdown: state.synthesized_plan.clone(),
                critique: state.critique.clone(),
                attached_docs: state
                    .attached_docs
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect(),
                kickoff_target: state
                    .kickoff_handoff
                    .as_ref()
                    .map(|target| target.session_label.clone()),
            })
        }
        _ => None,
    };
    PlanStatus {
        active,
        precall: match &app.mode {
            AppMode::PromptPrecall(pending)
                if matches!(pending.prior_mode.as_ref(), AppMode::PlanInterview(_)) =>
            {
                Some(PrecallView {
                    title: pending.prompt_id.spec().title.to_string(),
                    harness: pending.harness.display_name().to_string(),
                    preview: pending.preview.clone(),
                    viewing: pending.viewing,
                })
            }
            _ => None,
        },
        message: app.message.clone(),
        handoff: None,
    }
}

/// Start an on-demand interview for an existing feature. One live interview
/// is owned by `App`; a second target gets a conflict rather than silently
/// replacing the first interview's unsaved editor state.
pub fn begin(gui: &mut GuiHandle, target: &FeatureTarget, quick: bool) -> GuiResult<PlanStatus> {
    gui.refresh_snapshot()?;
    let app = gui.app_for_plan();
    let existing = match &app.mode {
        AppMode::PlanInterview(state) => Some(state),
        AppMode::PromptPrecall(pending) => match pending.prior_mode.as_ref() {
            AppMode::PlanInterview(state) => Some(state),
            _ => None,
        },
        _ => None,
    };
    if let Some(state) = existing {
        if state.interview_key == target.feature_id {
            return Ok(status_of(app));
        }
        return Err(GuiError::conflict(
            "Finish or cancel the current plan interview before opening another",
        ));
    }
    if !matches!(&app.mode, AppMode::Normal) {
        return Err(GuiError::conflict(
            "Finish the current workflow before opening a plan interview",
        ));
    }
    if app.paused_plan_interview.is_some() {
        return Err(GuiError::conflict(
            "Resume the parked plan interview before opening another",
        ));
    }
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted; refresh and retry"))?;
    app.selection = Selection::Feature(pi, fi);
    if quick {
        app.start_quick_plan_interview_for_selected_feature();
    } else {
        app.start_plan_interview_for_selected_feature();
    }
    Ok(status_of(app))
}

/// Enter the existing feature-creation wizard's deferred launch path with
/// explicit GUI form values. A worktree hook that prompts for a choice is
/// rejected before a worktree is created until that prompt has a GUI adapter;
/// a plain hook runs to completion here (see `finish_worktree_hook`), the
/// same rule `GuiHandle::create_feature` applies.
pub fn begin_feature_creation(
    gui: &mut GuiHandle,
    request: &CreateFeatureRequest,
    quick: bool,
) -> GuiResult<PlanStatus> {
    gui.refresh_snapshot()?;
    begin_feature_creation_core(gui.app_for_plan(), request, quick, None)
}

fn begin_feature_creation_core(
    app: &mut App,
    request: &CreateFeatureRequest,
    quick: bool,
    origin_seed: Option<(TodoPlanOrigin, String)>,
) -> GuiResult<PlanStatus> {
    if !matches!(&app.mode, AppMode::Normal) || app.paused_plan_interview.is_some() {
        return Err(GuiError::conflict(
            "Finish the current workflow before creating a feature",
        ));
    }
    if request.dry_run || request.branch.trim().is_empty() {
        return Err(GuiError::conflict("Enter a feature name to plan"));
    }
    let pi = app
        .store
        .projects
        .iter()
        .position(|project| project.name == request.project_name)
        .ok_or_else(|| GuiError::not_found("Project was deleted; refresh and retry"))?;
    let project = &app.store.projects[pi];
    let use_worktree = request.use_worktree.unwrap_or(!project.features.is_empty());
    if use_worktree
        && crate::extension::merge_project_extension_config(&app.config.extension, &project.repo)
            .lifecycle_hooks
            .on_worktree_created
            .as_ref()
            .and_then(|hook| hook.prompt())
            .is_some()
    {
        return Err(GuiError::conflict(
            "This project's worktree hook needs the TUI creation wizard for now",
        ));
    }
    app.selection = Selection::Project(pi);
    app.start_create_feature();
    let AppMode::CreatingFeature(state) = &mut app.mode else {
        return Err(GuiError::conflict(
            app.message
                .clone()
                .unwrap_or_else(|| "Feature wizard could not open".into()),
        ));
    };
    state.branch = request.branch.clone();
    state.agent = request.agent.clone();
    state.agent_index = state
        .allowed_agents
        .iter()
        .position(|agent| agent == &state.agent)
        .unwrap_or(0);
    state.mode = request.mode.clone();
    state.review = request.review;
    state.plan_mode = true;
    state.quick_plan = quick;
    state.create_terminal = request.create_terminal;
    state.session_name = App::default_session_name_for_agent(&state.agent);
    state.use_worktree = use_worktree;
    state.enable_chrome = request.enable_chrome;
    if let Some((origin, seed)) = origin_seed {
        state.todo_origin = Some(origin);
        app.pending_todo_plan_brief = Some(seed);
    }
    if let Err(error) = app
        .create_feature()
        .and_then(|()| finish_worktree_hook(app))
    {
        app.pending_todo_plan_brief = None;
        if !matches!(&app.mode, AppMode::PlanInterview(_)) {
            app.mode = AppMode::Normal;
        }
        return Err(GuiError::from(error));
    }
    if !matches!(&app.mode, AppMode::PlanInterview(_)) {
        app.pending_todo_plan_brief = None;
        let message = app
            .message
            .clone()
            .or_else(|| match &app.mode {
                AppMode::CreatingFeature(state) => state.branch_error.clone(),
                _ => None,
            })
            .unwrap_or_else(|| "Feature plan could not start".into());
        app.mode = AppMode::Normal;
        return Err(GuiError::conflict(message));
    }
    Ok(status_of(app))
}

/// Run a plain `on_worktree_created` hook that `create_feature` just started
/// to completion, then take the wizard's own continuation into the plan
/// interview. The wizard starts the hook in `AppMode::RunningHook`, which the
/// TUI's event loop polls and the user dismisses; the GUI has neither, so the
/// hook is waited on here -- blocking, as `GuiHandle::create_feature`'s
/// automation path already runs the same hook synchronously. A no-op in any
/// other mode.
fn finish_worktree_hook(app: &mut App) -> anyhow::Result<()> {
    loop {
        app.poll_running_hook()?;
        match &app.mode {
            AppMode::RunningHook(state) if state.child.is_some() => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            AppMode::RunningHook(_) => return app.complete_running_hook(),
            _ => return Ok(()),
        }
    }
}

/// Route a TODO through the same deferred new-feature launch as the TUI.
/// The TODO remains not-started during interview and is reserved atomically
/// at acceptance by `act`, so cancellation leaves it available.
pub fn begin_todo_in_new_feature(
    gui: &mut GuiHandle,
    todo_id: &str,
    request: &CreateFeatureRequest,
) -> GuiResult<PlanStatus> {
    gui.refresh_snapshot()?;
    let resolved = gui
        .db()?
        .resolve_todo_by_id(todo_id)
        .map_err(GuiError::from)?
        .ok_or_else(|| GuiError::not_found("TODO was deleted; refresh and retry"))?;
    let app = gui.app_for_plan();
    let pi = app
        .store
        .projects
        .iter()
        .position(|project| project.name == request.project_name)
        .ok_or_else(|| GuiError::not_found("Project was deleted; refresh and retry"))?;
    let project = &app.store.projects[pi];
    if !project.is_git || request.use_worktree != Some(true) {
        return Err(GuiError::conflict(
            "Planning a TODO into a new feature requires a git worktree",
        ));
    }
    let scope_matches = match &resolved.list.scope {
        TodoScope::Worktree { project_id, .. } | TodoScope::Project { project_id } => {
            project_id == &project.id
        }
        TodoScope::Global => true,
    };
    if !scope_matches {
        return Err(GuiError::conflict("Choose the TODO's project"));
    }
    if resolved.todo.work.status != TodoStatus::NotStarted {
        return Err(GuiError::conflict(
            "This TODO is already in progress or completed",
        ));
    }
    let origin = TodoPlanOrigin {
        todo_id: todo_id.into(),
        list_id: resolved.list.id,
        todo_title: resolved.todo.title.clone(),
        host_feature_id: resolved.list.feature_id.unwrap_or_default(),
    };
    let provenance = app.todo_provenance(pi, 0, &resolved.todo);
    let brief = App::compose_plan_brief(
        &resolved.todo,
        resolved.list.carry_over.as_deref(),
        &provenance,
    );
    begin_feature_creation_core(app, request, false, Some((origin, brief)))
}

/// Plan a TODO into an existing feature. Scope and TODO state are checked
/// against the database before the shared App interview is entered.
pub fn begin_todo_in_host(
    gui: &mut GuiHandle,
    todo_id: &str,
    target: &FeatureTarget,
) -> GuiResult<PlanStatus> {
    gui.refresh_snapshot()?;
    let resolved = gui
        .db()?
        .resolve_todo_by_id(todo_id)
        .map_err(GuiError::from)?
        .ok_or_else(|| GuiError::not_found("TODO was deleted; refresh and retry"))?;
    let app = gui.app_for_plan();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted; refresh and retry"))?;
    let feature = &app.store.projects[pi].features[fi];
    let scope_matches = match &resolved.list.scope {
        TodoScope::Worktree {
            project_id,
            workdir,
        } => project_id == &target.project_id && feature.workdir.to_string_lossy() == *workdir,
        TodoScope::Project { project_id } => project_id == &target.project_id,
        TodoScope::Global => true,
    };
    if !scope_matches {
        return Err(GuiError::conflict(
            "Choose a feature belonging to this TODO's current scope",
        ));
    }
    if resolved.todo.work.status != TodoStatus::NotStarted {
        return Err(GuiError::conflict(
            "This TODO is already in progress or completed",
        ));
    }
    let expected_key = crate::plan_interview::todo_interview_key(todo_id);
    let existing = match &app.mode {
        AppMode::PlanInterview(state) => Some(state),
        AppMode::PromptPrecall(pending) => match pending.prior_mode.as_ref() {
            AppMode::PlanInterview(state) => Some(state),
            _ => None,
        },
        _ => None,
    };
    if let Some(state) = existing {
        if state.interview_key == expected_key {
            return Ok(status_of(app));
        }
        return Err(GuiError::conflict(
            "Finish or cancel the current plan interview first",
        ));
    }
    if !matches!(&app.mode, AppMode::Normal) || app.paused_plan_interview.is_some() {
        return Err(GuiError::conflict(
            "Finish the current workflow before planning a TODO",
        ));
    }
    let origin = TodoPlanOrigin {
        todo_id: todo_id.to_string(),
        list_id: resolved.list.id,
        todo_title: resolved.todo.title.clone(),
        host_feature_id: target.feature_id.clone(),
    };
    app.start_todo_plan_in_host_feature_explicit(
        origin,
        &resolved.todo,
        pi,
        fi,
        resolved.list.carry_over.as_deref(),
    )
    .map_err(GuiError::from)?;
    Ok(status_of(app))
}

/// Poll the same worker completions as the TUI event loop. The frontend
/// requests this only while the Plan Interview panel is mounted; results
/// remain owned by the shared `App` state machine.
pub fn poll(gui: &mut GuiHandle) -> PlanStatus {
    let app = gui.app_for_plan();
    app.poll_plan_interview_ai_bg();
    app.poll_plan_interview_synthesis_bg();
    app.poll_plan_interview_critique_bg();
    app.poll_plan_interview_directed_feedback_bg();
    app.poll_plan_interview_investigation_bg();
    status_of(app)
}

fn advance_error(error: PlanInterviewAdvanceError) -> GuiError {
    let message = match error {
        PlanInterviewAdvanceError::BriefRequired => "Describe the feature before continuing",
        PlanInterviewAdvanceError::AnswerRequired => "This question requires an answer",
    };
    GuiError {
        kind: GuiErrorKind::Internal,
        message: message.into(),
    }
}

fn action_is_valid(action: PlanAction, phase: PlanInterviewPhase) -> bool {
    use PlanAction as A;
    use PlanInterviewPhase as P;
    match action {
        A::Resume | A::DiscardDraft => phase == P::ResumePrompt,
        A::Next | A::Back => matches!(phase, P::Brief | P::StaticQuestions | P::AiConsent),
        A::Skip => matches!(phase, P::StaticQuestions | P::AiConsent),
        A::FinishEarly => matches!(phase, P::Brief | P::StaticQuestions | P::AiConsent),
        A::OptInAi => phase == P::AiConsent,
        A::BeginEdit
        | A::Regenerate
        | A::RequestCritique
        | A::BeginFeedback
        | A::BeginInvestigation
        | A::Accept
        | A::AcceptApproved => phase == P::Review,
        A::SaveEdit | A::CancelEdit => phase == P::Editing,
        A::CloseCritique => matches!(phase, P::Critique | P::CritiqueLoading),
        A::ReviseFromCritique => phase == P::Critique,
        A::SubmitFeedback => phase == P::DirectedFeedback,
        A::CancelFeedback => {
            matches!(phase, P::DirectedFeedback | P::DirectedFeedbackLoading)
        }
        A::SubmitInvestigation => phase == P::Investigation,
        A::CancelInvestigation => matches!(phase, P::Investigation | P::InvestigationLoading),
        A::RestorePrior => phase == P::StaticQuestions,
        A::AttachDoc | A::RemoveDoc => phase == P::Brief,
        A::Cancel => phase != P::KickoffHandoff,
        A::KickoffAccept | A::KickoffDecline => phase == P::KickoffHandoff,
        A::PrecallConfirm | A::PrecallCancel | A::PrecallToggleView => false,
    }
}

fn apply_input(app: &mut App, input: Option<&PlanInput>) -> GuiResult<()> {
    let Some(input) = input else { return Ok(()) };
    let AppMode::PlanInterview(state) = &mut app.mode else {
        return Err(GuiError::conflict("Plan interview is no longer open"));
    };
    if !matches!(
        state.phase,
        PlanInterviewPhase::Brief
            | PlanInterviewPhase::StaticQuestions
            | PlanInterviewPhase::Editing
            | PlanInterviewPhase::DirectedFeedback
            | PlanInterviewPhase::Investigation
    ) {
        return Err(GuiError::conflict("This plan step no longer accepts text"));
    }
    if let Some(question) = state.current_question()
        && let PlanQuestionKind::Select(options) = &question.kind
    {
        if input.text.chars().count() > CUSTOM_ANSWER_MAX_LEN {
            return Err(GuiError::conflict(format!(
                "Custom answer must be at most {CUSTOM_ANSWER_MAX_LEN} characters"
            )));
        }
        if input
            .selected_option
            .is_some_and(|index| index >= options.len())
        {
            return Err(GuiError::conflict("That answer option no longer exists"));
        }
        state.selected_option = input.selected_option;
    }
    state.editor = TextEditor::new(input.text.clone());
    Ok(())
}

pub fn act(
    gui: &mut GuiHandle,
    expected_step: &str,
    action: PlanAction,
    input: Option<PlanInput>,
) -> GuiResult<PlanStatus> {
    let app = gui.app_for_plan();
    let current = status_of(app);
    let Some(view) = current.active else {
        return Err(GuiError::conflict("Plan interview is no longer open"));
    };
    if view.step_key != expected_step {
        return Err(GuiError::conflict(
            "Plan interview changed while this action was pending; refresh and retry",
        ));
    }
    if matches!(app.mode, AppMode::PromptPrecall(_)) {
        match action {
            PlanAction::PrecallConfirm => app.precall_confirm().map_err(GuiError::from)?,
            PlanAction::PrecallCancel => app.precall_cancel(),
            PlanAction::PrecallToggleView => app.precall_toggle_view(),
            _ => return Err(GuiError::conflict("Answer the headless-call notice first")),
        }
        return Ok(status_of(app));
    }
    let phase = match &app.mode {
        AppMode::PlanInterview(state) => state.phase,
        _ => unreachable!(),
    };
    if !action_is_valid(action, phase) {
        return Err(GuiError::conflict(
            "This action is not available at the current plan step",
        ));
    }

    if matches!(
        action,
        PlanAction::Next
            | PlanAction::Back
            | PlanAction::FinishEarly
            | PlanAction::SaveEdit
            | PlanAction::SubmitFeedback
            | PlanAction::SubmitInvestigation
    ) {
        apply_input(app, input.as_ref())?;
    }

    match action {
        PlanAction::Resume => app.resume_plan_interview_draft().map_err(GuiError::from)?,
        PlanAction::DiscardDraft => app.discard_plan_interview_draft(),
        PlanAction::Next => {
            let state = match &mut app.mode {
                AppMode::PlanInterview(state) => state,
                _ => unreachable!(),
            };
            state.advance().map_err(advance_error)?;
            app.persist_plan_interview_draft();
            app.continue_plan_interview_after_done()
                .map_err(GuiError::from)?;
        }
        PlanAction::Back => {
            if let AppMode::PlanInterview(state) = &mut app.mode
                && state.back()
            {
                app.persist_plan_interview_draft();
            }
        }
        PlanAction::Skip => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.skip().map_err(advance_error)?;
            }
            app.persist_plan_interview_draft();
            app.continue_plan_interview_after_done()
                .map_err(GuiError::from)?;
        }
        PlanAction::FinishEarly => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.finish_early().map_err(advance_error)?;
            }
            app.persist_plan_interview_draft();
            app.continue_plan_interview_after_done()
                .map_err(GuiError::from)?;
        }
        PlanAction::OptInAi => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.opt_in_ai_followups();
            }
            app.continue_plan_interview_after_done()
                .map_err(GuiError::from)?;
        }
        PlanAction::BeginEdit => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.begin_plan_edit();
            }
        }
        PlanAction::SaveEdit => {
            if let AppMode::PlanInterview(state) = &mut app.mode
                && !state.save_plan_edit()
            {
                return Err(GuiError {
                    kind: GuiErrorKind::Internal,
                    message: "Plan markdown cannot be empty".into(),
                });
            }
            app.persist_plan_interview_draft();
        }
        PlanAction::CancelEdit => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.cancel_plan_edit();
            }
        }
        PlanAction::Regenerate => app
            .start_plan_interview_synthesis()
            .map_err(GuiError::from)?,
        PlanAction::RequestCritique => app
            .start_plan_interview_critique()
            .map_err(GuiError::from)?,
        PlanAction::CloseCritique => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.close_critique();
            }
        }
        PlanAction::ReviseFromCritique => {
            let revise = match &mut app.mode {
                AppMode::PlanInterview(state) => state.revise_from_critique(),
                _ => false,
            };
            if revise {
                app.start_plan_interview_synthesis()
                    .map_err(GuiError::from)?;
            }
        }
        PlanAction::BeginFeedback => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.begin_directed_feedback();
            }
        }
        PlanAction::SubmitFeedback => app
            .start_plan_interview_directed_feedback()
            .map_err(GuiError::from)?,
        PlanAction::CancelFeedback => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.cancel_directed_feedback();
            }
        }
        PlanAction::BeginInvestigation => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.begin_investigation();
            }
        }
        PlanAction::SubmitInvestigation => app
            .start_plan_interview_investigation()
            .map_err(GuiError::from)?,
        PlanAction::CancelInvestigation => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.cancel_investigation();
            }
        }
        PlanAction::RestorePrior => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.restore_prior_answer();
            }
        }
        PlanAction::AttachDoc => {
            let path = input
                .as_ref()
                .ok_or_else(|| GuiError::conflict("Choose a document"))?;
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state
                    .attach_doc(Path::new(&path.text))
                    .map_err(|error| GuiError {
                        kind: GuiErrorKind::Internal,
                        message: error.to_string(),
                    })?;
            }
            app.persist_plan_interview_draft();
        }
        PlanAction::RemoveDoc => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.remove_last_attached_doc();
            }
            app.persist_plan_interview_draft();
        }
        PlanAction::Accept | PlanAction::AcceptApproved => {
            let (todo_plan_id, pending_feature) = match &app.mode {
                AppMode::PlanInterview(state) => (
                    state
                        .todo_origin
                        .as_ref()
                        .map(|origin| origin.todo_id.clone()),
                    state
                        .pending_launch
                        .as_ref()
                        .map(|pending| (pending.project_name.clone(), pending.branch.clone())),
                ),
                _ => (None, None),
            };
            let launches_feature = pending_feature.is_some();
            if matches!(action, PlanAction::AcceptApproved)
                && todo_plan_id.is_none()
                && !launches_feature
            {
                return Err(GuiError::conflict(
                    "This plan has no agent start to approve",
                ));
            }
            if (todo_plan_id.is_some() || launches_feature)
                && matches!(action, PlanAction::Accept)
                && let StartPreconditions::NeedsConfirm {
                    over_limit,
                    low_memory,
                } = app.check_start_preconditions()
            {
                let kind = if launches_feature { "feature" } else { "TODO" };
                return Err(GuiError::needs_approval(format!(
                    "Accepting this {kind} plan would exceed AMF's resource warning: {}. Start the agent anyway?",
                    describe_tripped(over_limit, low_memory)
                )));
            }
            // The TUI marks a TODO in progress when its plan chooser opens.
            // The GUI keeps it available while the user interviews, then
            // claims it atomically at acceptance before the App launch path
            // reads the row. This blocks a second process from starting the
            // same work during the save/start sequence.
            if let Some(todo_id) = &todo_plan_id {
                let db = app.db.as_ref().ok_or_else(|| GuiError {
                    kind: GuiErrorKind::Internal,
                    message: "No TODO database attached".into(),
                })?;
                if !db
                    .reserve_todo_agent_launch(todo_id)
                    .map_err(GuiError::from)?
                {
                    return Err(GuiError::conflict(
                        "TODO changed while its plan was open; refresh and retry",
                    ));
                }
            }
            let completed = app.complete_plan_interview_with_resource_approval(launches_feature);
            let mut handoff = None;
            if let Some(todo_id) = &todo_plan_id {
                let db = app.db.as_ref().expect("TODO plan has database");
                let associated = db
                    .find_todo_by_id(todo_id)
                    .map_err(GuiError::from)?
                    .and_then(|todo| todo.work.agent_session_id);
                if completed.is_err() {
                    if associated.is_none() {
                        let _ = db.rollback_reserved_todo_agent_launch(todo_id, None);
                    }
                } else {
                    let Some(session_id) = associated else {
                        let _ = db.rollback_reserved_todo_agent_launch(todo_id, None);
                        return Err(GuiError {
                            kind: GuiErrorKind::Internal,
                            message: "Plan saved, but its TODO agent did not start".into(),
                        });
                    };
                    let draft_prompt = match &app.mode {
                        AppMode::Compose(state) => state.editor.text().to_string(),
                        _ => String::new(),
                    };
                    if let Some((pi, fi, _)) = app.session_indices_by_id(&session_id) {
                        handoff = Some(PlanHandoff {
                            target: SessionTarget {
                                project_id: app.store.projects[pi].id.clone(),
                                feature_id: app.store.projects[pi].features[fi].id.clone(),
                                session_id,
                            },
                            draft_prompt,
                        });
                    }
                    // The shared launch opens the TUI composer. The GUI uses
                    // the captured seed in its own terminal composer instead.
                    app.exit_view_without_resuming_plan_interview();
                    app.message = Some("Plan saved; TODO agent started".into());
                }
            }
            completed.map_err(GuiError::from)?;
            if let Some((project_name, branch)) = pending_feature {
                if let AppMode::Compose(state) = &app.mode
                    && let Some(project) =
                        app.store.projects.iter().find(|p| p.name == project_name)
                    && let Some(feature) = project.features.iter().find(|f| f.name == branch)
                    && let Some(session) = feature
                        .sessions
                        .iter()
                        .find(|session| session.tmux_window == state.view.window)
                {
                    handoff = Some(PlanHandoff {
                        target: SessionTarget {
                            project_id: project.id.clone(),
                            feature_id: feature.id.clone(),
                            session_id: session.id.clone(),
                        },
                        draft_prompt: state.editor.text().to_string(),
                    });
                }
                if matches!(&app.mode, AppMode::Compose(_) | AppMode::Viewing(_)) {
                    app.exit_view_without_resuming_plan_interview();
                }
                app.message = Some(if todo_plan_id.is_some() {
                    "Plan saved; TODO feature created".into()
                } else {
                    "Plan saved; feature created".into()
                });
            }
            if let Some(handoff) = handoff {
                let mut status = status_of(app);
                status.handoff = Some(handoff);
                return Ok(status);
            }
        }
        PlanAction::Cancel => {
            let pending = matches!(&app.mode, AppMode::PlanInterview(state) if state.pending_launch.is_some());
            if pending {
                app.cancel_plan_interview_feature()
                    .map_err(GuiError::from)?;
            } else {
                app.launch_plan_interview_without_plan()
                    .map_err(GuiError::from)?;
            }
        }
        PlanAction::KickoffAccept => app
            .send_plan_kickoff_to_live_session()
            .map_err(GuiError::from)?,
        PlanAction::KickoffDecline => app.dismiss_plan_kickoff_handoff(),
        PlanAction::PrecallConfirm | PlanAction::PrecallCancel | PlanAction::PrecallToggleView => {
            unreachable!()
        }
    }
    Ok(status_of(app))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::app::precall::PrecallAction;
    use crate::project::{AgentKind, Feature, Project, ProjectStatus, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::path::Path;

    fn fixture_with_tmux(workdir: &Path, tmux: MockTmuxOps) -> (GuiHandle, FeatureTarget) {
        let mut feature = Feature::new_for_project(
            "demo",
            "planned-work".into(),
            "planned-work".into(),
            workdir.to_path_buf(),
            false,
            VibeMode::default(),
            false,
            false,
            AgentKind::default(),
            false,
            false,
        );
        feature.id = "feature-1".into();
        feature.status = ProjectStatus::Stopped;
        let mut project = Project::new(
            "demo".into(),
            workdir.to_path_buf(),
            false,
            AgentKind::default(),
        );
        project.id = "project-1".into();
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        (
            GuiHandle::from_app(App::new_for_test(
                store,
                Box::new(tmux),
                Box::new(MockWorktreeOps::new()),
            )),
            FeatureTarget {
                project_id: "project-1".into(),
                feature_id: "feature-1".into(),
            },
        )
    }

    fn fixture(workdir: &Path) -> (GuiHandle, FeatureTarget) {
        fixture_with_tmux(workdir, MockTmuxOps::new())
    }

    fn todo_fixture(workdir: &Path, tmux: MockTmuxOps) -> (GuiHandle, FeatureTarget, String) {
        let (mut gui, target) = fixture_with_tmux(workdir, tmux);
        gui.app_for_plan().store.projects[0].features[0].status = ProjectStatus::Active;
        let db = crate::db::AmfDb::open(&workdir.join("amf.db")).unwrap();
        db.save_store(&gui.app_for_plan().store).unwrap();
        let list = db
            .create_todo_list(
                &TodoScope::Project {
                    project_id: target.project_id.clone(),
                },
                Some(&target.feature_id),
            )
            .unwrap();
        let todo = db
            .add_todo(
                &list.id,
                "Fix the API",
                Some("Keep compatibility"),
                crate::db::todos::TodoPriority::Med,
            )
            .unwrap();
        gui.app_for_plan().store_version = Some(db.current_store_version().unwrap());
        gui.app_for_plan().db = Some(db);
        (gui, target, todo.id)
    }

    fn todo_new_feature_fixture(
        root: &Path,
        tmux: MockTmuxOps,
    ) -> (GuiHandle, CreateFeatureRequest, String, std::path::PathBuf) {
        let repo = root.join("repo");
        let worktree_dir = repo.join(".worktrees").join("todo-work");
        std::fs::create_dir_all(&worktree_dir).unwrap();
        let mut project = Project::new("demo".into(), repo, true, AgentKind::default());
        project.id = "project-1".into();
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let mut worktree = MockWorktreeOps::new();
        let created_path = worktree_dir.clone();
        worktree
            .expect_create()
            .times(1)
            .returning(move |_, _, _| Ok(created_path.clone()));
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(worktree));
        let db = crate::db::AmfDb::open(&root.join("amf.db")).unwrap();
        db.save_store(&app.store).unwrap();
        let list = db
            .create_todo_list(
                &TodoScope::Project {
                    project_id: "project-1".into(),
                },
                None,
            )
            .unwrap();
        let todo = db
            .add_todo(
                &list.id,
                "Improve the API",
                Some("Keep existing calls working"),
                crate::db::todos::TodoPriority::Med,
            )
            .unwrap();
        app.store_version = Some(db.current_store_version().unwrap());
        app.db = Some(db);
        let request = CreateFeatureRequest {
            project_name: "demo".into(),
            branch: "todo-work".into(),
            agent: AgentKind::default(),
            mode: VibeMode::default(),
            review: false,
            plan_mode: true,
            create_terminal: false,
            use_worktree: Some(true),
            enable_chrome: false,
            hook_choice: None,
            dry_run: false,
        };
        (GuiHandle::from_app(app), request, todo.id, worktree_dir)
    }

    fn next(gui: &mut GuiHandle, view: PlanView, text: &str) -> PlanView {
        let input = if view.phase == "ai_consent" {
            None
        } else {
            Some(PlanInput {
                text: text.into(),
                selected_option: None,
            })
        };
        act(gui, &view.step_key, PlanAction::Next, input)
            .unwrap()
            .active
            .unwrap()
    }

    #[test]
    fn cancelling_a_gui_interview_writes_no_plan_and_launches_no_agent() {
        let dir = tempfile::tempdir().unwrap();
        let (mut gui, target) = fixture(dir.path());
        let brief = begin(&mut gui, &target, true).unwrap().active.unwrap();
        let consent = next(&mut gui, brief, "Investigate the API surface");
        assert_eq!(consent.phase, "ai_consent");
        let review = next(&mut gui, consent, "");
        assert_eq!(review.phase, "review");

        let ended = act(&mut gui, &review.step_key, PlanAction::Cancel, None).unwrap();
        assert!(ended.active.is_none());
        assert!(!dir.path().join("AMF_PLAN.md").exists());
        assert_eq!(
            gui.app_for_plan().store.projects[0].features[0].status,
            ProjectStatus::Stopped
        );
    }

    #[test]
    fn accepted_gui_interview_writes_the_reviewed_plan_and_rejects_stale_actions() {
        let dir = tempfile::tempdir().unwrap();
        let (mut gui, target) = fixture(dir.path());
        let brief = begin(&mut gui, &target, true).unwrap().active.unwrap();
        let consent = next(&mut gui, brief.clone(), "Implement the API");
        let stale = act(&mut gui, &brief.step_key, PlanAction::Next, None).unwrap_err();
        assert_eq!(stale.kind, GuiErrorKind::Conflict);
        let review = next(&mut gui, consent, "");
        assert!(
            review
                .review_markdown
                .as_deref()
                .unwrap_or("")
                .contains("Implement the API")
        );

        let ended = act(&mut gui, &review.step_key, PlanAction::Accept, None).unwrap();
        assert!(ended.active.is_none());
        let written = std::fs::read_to_string(dir.path().join("AMF_PLAN.md")).unwrap();
        assert!(written.contains("Implement the API"));
    }

    #[test]
    fn precall_notice_keeps_the_interview_visible_and_restores_it_on_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let (mut gui, target) = fixture(dir.path());
        let brief = begin(&mut gui, &target, true).unwrap().active.unwrap();
        let harness = AgentKind::default();
        assert!(!gui.app_for_plan().precall_gate(
            PrecallAction::PlanSynthesis,
            &harness,
            "Preview text",
        ));

        let notice = poll(&mut gui);
        assert_eq!(notice.active.unwrap().step_key, brief.step_key);
        assert_eq!(notice.precall.unwrap().preview, "Preview text");
        let toggled = act(
            &mut gui,
            &brief.step_key,
            PlanAction::PrecallToggleView,
            None,
        )
        .unwrap();
        assert!(toggled.precall.unwrap().viewing);

        let cancelled = act(&mut gui, &brief.step_key, PlanAction::PrecallCancel, None).unwrap();
        assert!(cancelled.precall.is_none());
        assert_eq!(cancelled.active.unwrap().phase, "brief");
    }

    #[test]
    fn cancelling_a_todo_host_interview_leaves_the_todo_unclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let (mut gui, target, todo_id) = todo_fixture(dir.path(), MockTmuxOps::new());
        let view = begin_todo_in_host(&mut gui, &todo_id, &target)
            .unwrap()
            .active
            .unwrap();
        assert!(view.editor_text.contains("Fix the API"));
        assert!(view.editor_text.contains("Keep compatibility"));
        assert_eq!(
            view.interview_key,
            crate::plan_interview::todo_interview_key(&todo_id)
        );

        let ended = act(&mut gui, &view.step_key, PlanAction::Cancel, None).unwrap();
        assert!(ended.active.is_none());
        assert!(!dir.path().join("AMF_PLAN.md").exists());
        let todo = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(todo.work.status, TodoStatus::NotStarted);
        assert!(todo.work.agent_session_id.is_none());
    }

    #[test]
    fn over_limit_todo_plan_accept_waits_before_writing_or_launching() {
        let _lease_lock = crate::resources::limits::lock_lease_tests();
        assert_eq!(crate::resources::limits::wait_for_in_flight(0), 0);
        let _lease = crate::resources::limits::HeadlessLease::acquire();
        let dir = tempfile::tempdir().unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_list_panes().returning(Vec::new);
        let (mut gui, target, todo_id) = todo_fixture(dir.path(), tmux);
        let view = begin_todo_in_host(&mut gui, &todo_id, &target)
            .unwrap()
            .active
            .unwrap();
        let app = gui.app_for_plan();
        app.config.max_concurrent_agents = 1;
        app.config.low_memory_warn_mb = 0;
        let AppMode::PlanInterview(state) = &mut app.mode else {
            panic!("expected interview");
        };
        state.phase = PlanInterviewPhase::Review;
        state.synthesized_plan = Some("# Approved plan".into());
        let step = status_of(app).active.unwrap().step_key;

        let warning = act(&mut gui, &step, PlanAction::Accept, None).unwrap_err();
        assert_eq!(warning.kind, GuiErrorKind::NeedsApproval);
        assert!(!dir.path().join("AMF_PLAN.md").exists());
        let todo = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(todo.work.status, TodoStatus::NotStarted);
        assert_ne!(step, view.step_key);
    }

    #[test]
    fn accepted_todo_host_plan_hands_an_unsent_kickoff_to_the_gui() {
        let dir = tempfile::tempdir().unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_list_sessions().returning(|| Ok(vec![]));
        tmux.expect_list_panes().returning(Vec::new);
        tmux.expect_session_exists().return_const(true);
        tmux.expect_create_window().returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_resize_pane().returning(|_, _, _, _| Ok(()));
        tmux.expect_select_window().returning(|_, _| Ok(()));
        let (mut gui, target, todo_id) = todo_fixture(dir.path(), tmux);
        begin_todo_in_host(&mut gui, &todo_id, &target).unwrap();
        let app = gui.app_for_plan();
        app.config.max_concurrent_agents = 8;
        app.config.low_memory_warn_mb = 0;
        let AppMode::PlanInterview(state) = &mut app.mode else {
            panic!("expected interview");
        };
        state.phase = PlanInterviewPhase::Review;
        state.synthesized_plan = Some("# Implement the API".into());
        let step = status_of(app).active.unwrap().step_key;

        let result = act(&mut gui, &step, PlanAction::Accept, None).unwrap();
        assert!(result.active.is_none());
        let handoff = result.handoff.expect("TODO launch hands a session to GUI");
        assert_eq!(handoff.target.feature_id, target.feature_id);
        assert!(handoff.draft_prompt.contains("first unchecked task"));
        assert!(matches!(gui.app_for_plan().mode, AppMode::Normal));
        let todo = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(todo.work.status, TodoStatus::InProgress);
        assert_eq!(todo.work.agent_session_id, Some(handoff.target.session_id));
        assert!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().starts_with("AMF_PLAN"))
        );
    }

    #[test]
    fn creation_time_full_and_quick_plans_defer_feature_creation_until_acceptance() {
        for quick in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut project = Project::new(
                "demo".into(),
                dir.path().to_path_buf(),
                true,
                AgentKind::default(),
            );
            project.id = "project-1".into();
            let mut store = ProjectStore::empty();
            store.projects.push(project);
            let mut gui = GuiHandle::from_app(App::new_for_test(
                store,
                Box::new(MockTmuxOps::new()),
                Box::new(MockWorktreeOps::new()),
            ));
            let request = CreateFeatureRequest {
                project_name: "demo".into(),
                branch: "planned-work".into(),
                agent: AgentKind::default(),
                mode: VibeMode::default(),
                review: false,
                plan_mode: true,
                create_terminal: false,
                use_worktree: Some(false),
                enable_chrome: false,
                hook_choice: None,
                dry_run: false,
            };
            let view = begin_feature_creation(&mut gui, &request, quick)
                .unwrap()
                .active
                .unwrap();
            assert_eq!(view.kind, if quick { "quick" } else { "full" });
            assert!(gui.app_for_plan().store.projects[0].features.is_empty());

            let cancelled = act(&mut gui, &view.step_key, PlanAction::Cancel, None).unwrap();
            assert!(cancelled.active.is_none());
            assert!(gui.app_for_plan().store.projects[0].features.is_empty());
            assert!(!dir.path().join("AMF_PLAN.md").exists());
        }
    }

    fn worktree_plan_request() -> CreateFeatureRequest {
        CreateFeatureRequest {
            project_name: "demo".into(),
            branch: "planned-work".into(),
            agent: AgentKind::default(),
            mode: VibeMode::default(),
            review: false,
            plan_mode: true,
            create_terminal: false,
            use_worktree: Some(true),
            enable_chrome: false,
            hook_choice: None,
            dry_run: false,
        }
    }

    fn git_project_store(repo: &Path) -> ProjectStore {
        let mut project = Project::new(
            "demo".into(),
            repo.to_path_buf(),
            true,
            AgentKind::default(),
        );
        project.id = "project-1".into();
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        store
    }

    /// A worktree hook that asks nothing runs to completion inside the call,
    /// as it does for the GUI's unplanned `create_feature`, and the wizard's
    /// own continuation then opens the interview.
    #[test]
    fn a_plain_worktree_hook_runs_and_the_planned_creation_continues() {
        use crate::extension::{ExtensionConfig, HookConfig, LifecycleHooks};

        let dir = tempfile::tempdir().unwrap();
        let worktree_dir = dir.path().join("wt");
        std::fs::create_dir_all(&worktree_dir).unwrap();
        let mut worktree = MockWorktreeOps::new();
        let created = worktree_dir.clone();
        worktree
            .expect_create()
            .times(1)
            .returning(move |_, _, _| Ok(created.clone()));
        let mut app = App::new_for_test(
            git_project_store(dir.path()),
            Box::new(MockTmuxOps::new()),
            Box::new(worktree),
        );
        app.config.extension = ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_worktree_created: Some(HookConfig::Script("echo ok > hook-ran".into())),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut gui = GuiHandle::from_app(app);

        let view = begin_feature_creation(&mut gui, &worktree_plan_request(), false)
            .unwrap()
            .active
            .expect("the interview opens once the hook has finished");

        assert_eq!(view.kind, "full");
        assert!(worktree_dir.join("hook-ran").exists());
        assert!(matches!(gui.app_for_plan().mode, AppMode::PlanInterview(_)));
    }

    #[test]
    fn a_prompting_worktree_hook_is_still_rejected_before_a_worktree_exists() {
        use crate::extension::{ExtensionConfig, HookConfig, HookPrompt, LifecycleHooks};

        let dir = tempfile::tempdir().unwrap();
        // No `create` expectation: creating a worktree would fail the test.
        let mut app = App::new_for_test(
            git_project_store(dir.path()),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.config.extension = ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_worktree_created: Some(HookConfig::WithPrompt {
                    script: "setup.sh".into(),
                    prompt: HookPrompt {
                        title: "Choose stack".into(),
                        options: vec!["rust".into()],
                    },
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut gui = GuiHandle::from_app(app);

        let error = begin_feature_creation(&mut gui, &worktree_plan_request(), false).unwrap_err();

        assert_eq!(error.kind, GuiErrorKind::Conflict);
        assert!(error.message.contains("TUI creation wizard"));
    }

    #[test]
    fn accepting_a_creation_time_plan_creates_one_feature_and_returns_its_kickoff() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::new(
            "demo".into(),
            dir.path().to_path_buf(),
            true,
            AgentKind::default(),
        );
        project.id = "project-1".into();
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let created = Arc::new(AtomicBool::new(false));
        let seen = created.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists()
            .returning(move |_| seen.load(Ordering::SeqCst));
        tmux.expect_create_session_with_window()
            .returning(move |_, _, _| {
                created.store(true, Ordering::SeqCst);
                Ok(())
            });
        tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_select_window().returning(|_, _| Ok(()));
        tmux.expect_resize_pane().returning(|_, _, _, _| Ok(()));
        tmux.expect_list_sessions().returning(|| Ok(vec![]));
        let mut gui = GuiHandle::from_app(App::new_for_test(
            store,
            Box::new(tmux),
            Box::new(MockWorktreeOps::new()),
        ));
        gui.app_for_plan().config.max_concurrent_agents = 8;
        gui.app_for_plan().config.low_memory_warn_mb = 0;
        let request = CreateFeatureRequest {
            project_name: "demo".into(),
            branch: "planned-work".into(),
            agent: AgentKind::default(),
            mode: VibeMode::default(),
            review: false,
            plan_mode: true,
            create_terminal: false,
            use_worktree: Some(false),
            enable_chrome: false,
            hook_choice: None,
            dry_run: false,
        };
        begin_feature_creation(&mut gui, &request, false).unwrap();
        let app = gui.app_for_plan();
        let AppMode::PlanInterview(state) = &mut app.mode else {
            panic!("expected interview");
        };
        state.phase = PlanInterviewPhase::Review;
        state.synthesized_plan = Some("# Build the feature".into());
        let step = status_of(app).active.unwrap().step_key;

        let result = act(&mut gui, &step, PlanAction::Accept, None).unwrap();
        assert!(result.active.is_none());
        let handoff = result.handoff.expect("new agent has an editable kickoff");
        assert!(handoff.draft_prompt.contains("first unchecked task"));
        assert_eq!(gui.app_for_plan().store.projects[0].features.len(), 1);
        assert_eq!(
            handoff.target.feature_id,
            gui.app_for_plan().store.projects[0].features[0].id
        );
        assert!(matches!(gui.app_for_plan().mode, AppMode::Normal));
        assert!(
            std::fs::read_to_string(dir.path().join("AMF_PLAN.md"))
                .unwrap()
                .contains("Build the feature")
        );
    }

    #[test]
    fn cancelling_a_todo_new_feature_plan_keeps_the_worktree_but_not_the_feature() {
        let dir = tempfile::tempdir().unwrap();
        let (mut gui, request, todo_id, worktree_dir) =
            todo_new_feature_fixture(dir.path(), MockTmuxOps::new());
        let view = begin_todo_in_new_feature(&mut gui, &todo_id, &request)
            .unwrap()
            .active
            .unwrap();
        assert!(view.editor_text.contains("Improve the API"));
        assert!(view.editor_text.contains("Keep existing calls working"));
        assert!(gui.app_for_plan().store.projects[0].features.is_empty());

        let cancelled = act(&mut gui, &view.step_key, PlanAction::Cancel, None).unwrap();
        assert!(cancelled.active.is_none());
        assert!(worktree_dir.exists());
        assert!(!worktree_dir.join("AMF_PLAN.md").exists());
        assert!(gui.app_for_plan().store.projects[0].features.is_empty());
        let todo = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(todo.work.status, TodoStatus::NotStarted);
    }

    #[test]
    fn accepting_a_todo_new_feature_plan_links_its_started_agent() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let created = Arc::new(AtomicBool::new(false));
        let seen = created.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists()
            .returning(move |_| seen.load(Ordering::SeqCst));
        tmux.expect_create_session_with_window()
            .returning(move |_, _, _| {
                created.store(true, Ordering::SeqCst);
                Ok(())
            });
        tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_select_window().returning(|_, _| Ok(()));
        tmux.expect_resize_pane().returning(|_, _, _, _| Ok(()));
        tmux.expect_list_sessions().returning(|| Ok(vec![]));
        let (mut gui, request, todo_id, worktree_dir) = todo_new_feature_fixture(dir.path(), tmux);
        begin_todo_in_new_feature(&mut gui, &todo_id, &request).unwrap();
        let app = gui.app_for_plan();
        app.config.max_concurrent_agents = 8;
        app.config.low_memory_warn_mb = 0;
        let AppMode::PlanInterview(state) = &mut app.mode else {
            panic!("expected interview");
        };
        state.phase = PlanInterviewPhase::Review;
        state.synthesized_plan = Some("# Build the TODO".into());
        let step = status_of(app).active.unwrap().step_key;

        let result = act(&mut gui, &step, PlanAction::Accept, None).unwrap();
        let handoff = result.handoff.expect("new TODO feature has an agent");
        assert!(handoff.draft_prompt.contains("first unchecked task"));
        let feature_id = gui.app_for_plan().store.projects[0].features[0].id.clone();
        assert_eq!(feature_id, handoff.target.feature_id);
        let todo = gui
            .db()
            .unwrap()
            .find_todo_by_id(&todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(todo.linked_feature_id.as_deref(), Some(feature_id.as_str()));
        assert_eq!(todo.work.agent_session_id, Some(handoff.target.session_id));
        assert!(
            std::fs::read_to_string(worktree_dir.join("AMF_PLAN.md"))
                .unwrap()
                .contains("Build the TODO")
        );
    }

    #[test]
    fn creation_plan_asks_before_over_limit_launch_or_plan_write() {
        let _lease_lock = crate::resources::limits::lock_lease_tests();
        assert_eq!(crate::resources::limits::wait_for_in_flight(0), 0);
        let _lease = crate::resources::limits::HeadlessLease::acquire();
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::new(
            "demo".into(),
            dir.path().to_path_buf(),
            true,
            AgentKind::default(),
        );
        project.id = "project-1".into();
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_list_panes().returning(Vec::new);
        let mut gui = GuiHandle::from_app(App::new_for_test(
            store,
            Box::new(tmux),
            Box::new(MockWorktreeOps::new()),
        ));
        gui.app_for_plan().config.max_concurrent_agents = 1;
        gui.app_for_plan().config.low_memory_warn_mb = 0;
        let request = CreateFeatureRequest {
            project_name: "demo".into(),
            branch: "planned-work".into(),
            agent: AgentKind::default(),
            mode: VibeMode::default(),
            review: false,
            plan_mode: true,
            create_terminal: false,
            use_worktree: Some(false),
            enable_chrome: false,
            hook_choice: None,
            dry_run: false,
        };
        begin_feature_creation(&mut gui, &request, false).unwrap();
        let app = gui.app_for_plan();
        let AppMode::PlanInterview(state) = &mut app.mode else {
            panic!("expected interview");
        };
        state.phase = PlanInterviewPhase::Review;
        state.synthesized_plan = Some("# Pending plan".into());
        let step = status_of(app).active.unwrap().step_key;

        let warning = act(&mut gui, &step, PlanAction::Accept, None).unwrap_err();
        assert_eq!(warning.kind, GuiErrorKind::NeedsApproval);
        assert!(!dir.path().join("AMF_PLAN.md").exists());
        assert!(gui.app_for_plan().store.projects[0].features.is_empty());
    }
}
