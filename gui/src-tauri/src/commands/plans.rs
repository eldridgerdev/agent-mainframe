use super::super::{
    AppState, CreateFeatureRequest, FeatureTarget, GuiError, emit_workspace_changed,
};
use agent_mainframe::gui_plans::{self, PlanAction, PlanInput, PlanStatus};
use tauri::State;

#[tauri::command]
pub(crate) fn plan_begin(
    state: State<AppState>,
    target: FeatureTarget,
    quick: bool,
) -> Result<PlanStatus, GuiError> {
    gui_plans::begin(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &target,
        quick,
    )
}

#[tauri::command]
pub(crate) fn plan_begin_todo_host(
    state: State<AppState>,
    todo_id: String,
    target: FeatureTarget,
) -> Result<PlanStatus, GuiError> {
    gui_plans::begin_todo_in_host(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
        &target,
    )
}

#[tauri::command]
pub(crate) fn plan_begin_creation(
    state: State<AppState>,
    request: CreateFeatureRequest,
    quick: bool,
) -> Result<PlanStatus, GuiError> {
    gui_plans::begin_feature_creation(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &request,
        quick,
    )
}

#[tauri::command]
pub(crate) fn plan_begin_todo_new(
    state: State<AppState>,
    todo_id: String,
    request: CreateFeatureRequest,
    quick: Option<bool>,
) -> Result<PlanStatus, GuiError> {
    gui_plans::begin_todo_in_new_feature(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
        &request,
        quick.unwrap_or(false),
    )
}

#[tauri::command]
pub(crate) fn plan_snapshot(state: State<AppState>) -> PlanStatus {
    gui_plans::poll(&mut state.0.lock().expect("gui handle mutex poisoned"))
}

#[tauri::command]
pub(crate) fn plan_act(
    app: tauri::AppHandle,
    state: State<AppState>,
    expected_step: String,
    action: PlanAction,
    input: Option<PlanInput>,
) -> Result<PlanStatus, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let result = gui_plans::act(&mut gui, &expected_step, action, input)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(result)
}
