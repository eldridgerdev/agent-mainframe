use super::super::{AppState, EditExplanationState, FeatureTarget, GuiError};
use agent_mainframe::gui_diff;
use agent_mainframe::gui_supervised_edits::{
    self, PendingEditCount, SupervisedEditDecision, SupervisedEditOutcome, SupervisedEditsView,
};
use tauri::State;

#[tauri::command]
pub(crate) fn supervised_edit_explanation_prepare(
    state: State<AppState>,
    explanations: State<EditExplanationState>,
    target: FeatureTarget,
    edit_id: String,
    revision: String,
) -> Result<gui_supervised_edits::explanations::ExplanationView, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    explanations
        .0
        .lock()
        .expect("explanation mutex poisoned")
        .prepare(&mut gui, target, &edit_id, &revision)
}

#[tauri::command]
pub(crate) fn supervised_edit_explanation_start(
    state: State<AppState>,
    explanations: State<EditExplanationState>,
    owner: String,
    approved: bool,
) -> Result<gui_supervised_edits::explanations::ExplanationView, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    explanations
        .0
        .lock()
        .expect("explanation mutex poisoned")
        .start(&mut gui, &owner, approved)
}

#[tauri::command]
pub(crate) fn supervised_edit_explanation_poll(
    state: State<AppState>,
    explanations: State<EditExplanationState>,
    owner: String,
    request: u64,
) -> Result<gui_supervised_edits::explanations::ExplanationView, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    explanations
        .0
        .lock()
        .expect("explanation mutex poisoned")
        .poll(&mut gui, &owner, request)
}

#[tauri::command]
pub(crate) fn supervised_edit_explanation_cancel(
    explanations: State<EditExplanationState>,
    owner: String,
) -> gui_supervised_edits::explanations::ExplanationView {
    explanations
        .0
        .lock()
        .expect("explanation mutex poisoned")
        .cancel(&owner)
}

#[tauri::command]
pub(crate) async fn supervised_edits_load(
    state: State<'_, AppState>,
    target: FeatureTarget,
    context: gui_diff::DiffContext,
) -> Result<SupervisedEditsView, GuiError> {
    gui_supervised_edits::load(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
        context,
    )
}

#[tauri::command]
pub(crate) async fn supervised_edit_counts(
    state: State<'_, AppState>,
) -> Result<Vec<PendingEditCount>, GuiError> {
    gui_supervised_edits::pending_counts(&mut state.0.lock().expect("gui handle mutex poisoned"))
}

/// Answers an agent waiting on a Vibeless edit. The frontend sends this only
/// after the reviewer explicitly confirms; the backend refuses a changed,
/// already-answered or abandoned edit.
#[tauri::command]
pub(crate) async fn supervised_edit_respond(
    state: State<'_, AppState>,
    target: FeatureTarget,
    edit_id: String,
    revision: String,
    decision: SupervisedEditDecision,
) -> Result<SupervisedEditOutcome, GuiError> {
    gui_supervised_edits::respond(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
        &edit_id,
        &revision,
        decision,
    )
}
