use super::super::{
    AppState, AutomationHookPrompt, CreateFeatureRequest, CreateFeatureResponse,
    CreateProjectRequest, CreateProjectResponse, FeatureTarget, GuiError, emit_workspace_changed,
};
use tauri::State;

#[tauri::command]
pub(crate) fn worktree_hook_prompt(
    state: State<AppState>,
    project_id: String,
) -> Result<Option<AutomationHookPrompt>, GuiError> {
    state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .worktree_hook_prompt(&project_id)
}

#[tauri::command]
pub(crate) fn create_project(
    app: tauri::AppHandle,
    state: State<AppState>,
    request: CreateProjectRequest,
) -> Result<CreateProjectResponse, GuiError> {
    let dry_run = request.dry_run;
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.create_project(request)?;
    // A dry run (the Add project form checks the path on blur) changes
    // nothing, so it must not trigger a workspace-wide refresh.
    if !dry_run {
        emit_workspace_changed(&app, &gui.broadcast_snapshot());
    }
    Ok(response)
}

/// The steering coach checklist for a draft prompt. Pure and local (no
/// harness call, no GUI mutex); the digest binds the result to the text. It
/// runs on a blocking thread so scanning a large draft never stalls the UI.

#[tauri::command]
pub(crate) fn create_feature(
    app: tauri::AppHandle,
    state: State<AppState>,
    request: CreateFeatureRequest,
) -> Result<CreateFeatureResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.create_feature(request)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) fn feature_harness_load(
    state: State<AppState>,
    target: FeatureTarget,
) -> Result<agent_mainframe::gui_harness_change::HarnessChangeView, GuiError> {
    agent_mainframe::gui_harness_change::load(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
    )
}

#[tauri::command]
pub(crate) fn feature_harness_apply(
    app: tauri::AppHandle,
    state: State<AppState>,
    request: agent_mainframe::gui_harness_change::HarnessChangeRequest,
) -> Result<(), GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let result = agent_mainframe::gui_harness_change::apply(&mut gui, request);
    // Shutdown, hooks or restart may have partially completed even on error.
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    result
}
