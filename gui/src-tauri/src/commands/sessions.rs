use super::super::{
    AppState, DeleteFeatureResponse, FeatureTarget, GuiError, RemoveSessionResponse, SessionTarget,
    StartFeatureResponse, StartSessionResponse, StopFeatureResponse, StopSessionResponse,
    TodoDeleteChoice, TodoHostChoice, VibeMode, emit_workspace_changed,
};
use agent_mainframe::gui_contract::AddSessionResponse;
use agent_mainframe::gui_sessions::{
    self, AddCustomSessionRequest, AddCustomSessionResponse, CloseEditorsResponse,
    NewSessionOptions, OpenVscodeResponse,
};
use agent_mainframe::project::SessionKind;
use tauri::State;

#[tauri::command]
pub(crate) fn session_recovery_option(
    state: State<AppState>,
    target: SessionTarget,
) -> Result<Option<agent_mainframe::gui_contract::SessionRecoveryOption>, GuiError> {
    state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .session_recovery_option(&target)
}

#[tauri::command]
pub(crate) fn saved_agent_sessions(
    state: State<AppState>,
    target: SessionTarget,
) -> Result<Vec<agent_mainframe::gui_contract::SavedAgentSession>, GuiError> {
    state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .saved_agent_sessions(&target)
}

#[tauri::command]
pub(crate) fn recover_session(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: SessionTarget,
    choice: agent_mainframe::gui_contract::SessionRecoveryChoice,
    picked_id: Option<String>,
    approved: bool,
) -> Result<StartFeatureResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.recover_session(target, choice, picked_id, approved)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) fn stop_feature(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: FeatureTarget,
) -> Result<StopFeatureResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.stop_feature(target)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) fn start_session(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: SessionTarget,
    approved: bool,
) -> Result<StartSessionResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.start_session(target, approved)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) fn stop_session(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: SessionTarget,
) -> Result<StopSessionResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.stop_session(target)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) fn remove_session(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: SessionTarget,
) -> Result<RemoveSessionResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.remove_session(target)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) fn delete_feature(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: FeatureTarget,
    todos: Option<TodoDeleteChoice>,
    todo_host: Option<TodoHostChoice>,
) -> Result<DeleteFeatureResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.delete_feature(target, todos, todo_host)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) fn set_feature_vibe_mode(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: FeatureTarget,
    mode: VibeMode,
) -> Result<(), GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    gui.set_feature_vibe_mode(target, mode)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(())
}

#[tauri::command]
pub(crate) fn start_feature(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: FeatureTarget,
    approved: bool,
) -> Result<StartFeatureResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.start_feature_with_approval(target, approved)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) async fn new_session_options(
    state: State<'_, AppState>,
    target: FeatureTarget,
) -> Result<NewSessionOptions, GuiError> {
    gui_sessions::new_session_options(&state.0, &target)
}

#[tauri::command]
pub(crate) fn add_session(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: FeatureTarget,
    kind: SessionKind,
    label: Option<String>,
    approved: bool,
) -> Result<AddSessionResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.add_session(target, kind, label, approved)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

/// Async (off the main thread): the custom session's `pre_check` and the
/// `code` CLI can take a while.
#[tauri::command]
pub(crate) async fn add_custom_session(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: AddCustomSessionRequest,
) -> Result<AddCustomSessionResponse, GuiError> {
    let response = gui_sessions::add_custom_session(&state.0, request)?;
    let snapshot = state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .broadcast_snapshot();
    emit_workspace_changed(&app, &snapshot);
    Ok(response)
}

#[tauri::command]
pub(crate) async fn open_vscode(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    target: FeatureTarget,
    approved: bool,
) -> Result<OpenVscodeResponse, GuiError> {
    let response = gui_sessions::open_vscode(&state.0, target, approved)?;
    let snapshot = state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .broadcast_snapshot();
    emit_workspace_changed(&app, &snapshot);
    Ok(response)
}

#[tauri::command]
pub(crate) async fn close_editors(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    target: FeatureTarget,
    seen: Vec<String>,
) -> Result<CloseEditorsResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.close_editors(target, seen)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}
