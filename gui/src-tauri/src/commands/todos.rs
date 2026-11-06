use super::super::{
    AppState, CreateFeatureRequest, FeatureTarget, GuiError, TodoAgentLaunchResponse,
    emit_workspace_changed,
};
use agent_mainframe::gui_todos::{
    self, ScratchpadEditRequest, ScratchpadOutcome, TodoEditOutcome, TodoEditRequest, TodoListView,
    TodoPriority, TodoScopeRequest, TodoStatus,
};
use tauri::State;

#[tauri::command]
pub(crate) fn todo_list(
    state: State<AppState>,
    request: TodoScopeRequest,
) -> Result<Option<TodoListView>, GuiError> {
    gui_todos::load(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &request,
    )
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) fn todo_add(
    state: State<AppState>,
    request: TodoScopeRequest,
    host_feature_id: Option<String>,
    title: String,
    body: Option<String>,
    priority: TodoPriority,
) -> Result<TodoListView, GuiError> {
    gui_todos::add(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &request,
        host_feature_id.as_deref(),
        &title,
        body.as_deref(),
        priority,
    )
}

#[tauri::command]
pub(crate) fn todo_update(
    state: State<AppState>,
    request: TodoEditRequest,
) -> Result<TodoEditOutcome, GuiError> {
    gui_todos::update(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &request,
    )
}

#[tauri::command]
pub(crate) fn todo_update_scratchpad(
    state: State<AppState>,
    request: ScratchpadEditRequest,
) -> Result<ScratchpadOutcome, GuiError> {
    gui_todos::update_scratchpad(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &request,
    )
}

#[tauri::command]
pub(crate) fn todo_next_preview(
    state: State<AppState>,
    request: agent_mainframe::gui_contract::NextTodoRequest,
) -> Result<agent_mainframe::gui_contract::NextTodoPreview, GuiError> {
    state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .preview_next_todo(&request)
}

#[tauri::command]
pub(crate) fn todo_quick_capture_target(
    state: State<AppState>,
    target: FeatureTarget,
) -> Result<String, GuiError> {
    state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .quick_capture_target(&target)
}

#[tauri::command]
pub(crate) fn todo_quick_capture(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: FeatureTarget,
    title: String,
) -> Result<agent_mainframe::gui_contract::QuickCaptureResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.quick_capture_todo(&target, &title)?;
    // A first capture can add the feature's TODOs session.
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) fn todo_work_target(
    state: State<AppState>,
    todo_id: String,
) -> Result<agent_mainframe::gui_contract::TodoWorkTarget, GuiError> {
    state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .todo_work_target(&todo_id)
}

#[tauri::command]
pub(crate) fn todo_set_status(
    state: State<AppState>,
    todo_id: String,
    status: TodoStatus,
) -> Result<(), GuiError> {
    gui_todos::set_status(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
        status,
    )
}

#[tauri::command]
pub(crate) fn todo_delete(state: State<AppState>, todo_id: String) -> Result<(), GuiError> {
    gui_todos::delete(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
    )
}

#[tauri::command]
pub(crate) fn todo_move(
    state: State<AppState>,
    todo_id: String,
    target: TodoScopeRequest,
) -> Result<TodoListView, GuiError> {
    gui_todos::move_to(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
        &target,
    )
}

#[tauri::command]
pub(crate) fn todo_copy(
    state: State<AppState>,
    todo_id: String,
    target: TodoScopeRequest,
) -> Result<TodoListView, GuiError> {
    gui_todos::copy_to(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
        &target,
    )
}

#[tauri::command]
pub(crate) fn todo_reorder(
    state: State<AppState>,
    ordered_ids: Vec<String>,
) -> Result<(), GuiError> {
    gui_todos::reorder(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &ordered_ids,
    )
}

#[tauri::command]
pub(crate) fn todo_launch_agent(
    app: tauri::AppHandle,
    state: State<AppState>,
    todo_id: String,
    target: FeatureTarget,
    approved: bool,
) -> Result<TodoAgentLaunchResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.launch_todo_agent(&todo_id, target, approved)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) fn todo_launch_new_feature(
    app: tauri::AppHandle,
    state: State<AppState>,
    todo_id: String,
    request: CreateFeatureRequest,
    approved: bool,
) -> Result<TodoAgentLaunchResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.launch_todo_in_new_feature(&todo_id, request, approved)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}
