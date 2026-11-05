use super::super::{AppState, FeatureTarget, GuiError, emit_workspace_changed};
use agent_mainframe::gui_learning::{self, LearningAction, LearningHandoff, LearningView};
use tauri::State;

#[tauri::command]
pub(crate) fn learning_begin(
    state: State<AppState>,
    target: FeatureTarget,
) -> Result<LearningView, GuiError> {
    gui_learning::begin(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
    )
}

#[tauri::command]
pub(crate) fn learning_snapshot(state: State<AppState>) -> Result<Option<LearningView>, GuiError> {
    gui_learning::snapshot(&mut state.0.lock().expect("gui handle mutex poisoned"))
}

#[tauri::command]
pub(crate) fn learning_act(
    state: State<AppState>,
    workflow_id: String,
    revision: u64,
    action: LearningAction,
) -> Result<Option<LearningView>, GuiError> {
    gui_learning::act(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        revision,
        action,
    )
}

#[tauri::command]
pub(crate) fn learning_launch_agent(
    app: tauri::AppHandle,
    state: State<AppState>,
    workflow_id: String,
    revision: u64,
    qa_id: String,
    approved: bool,
) -> Result<LearningHandoff, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui_learning::launch_agent(&mut gui, &workflow_id, revision, &qa_id, approved)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}
