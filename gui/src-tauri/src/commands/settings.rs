use std::sync::Mutex;

use super::super::{
    AppState, DeleteFeatureResponse, GuiError, GuiHandle, TodoDeleteChoice, TodoHostChoice,
    emit_workspace_changed,
};
use agent_mainframe::gui_project_preference::{
    self, ProjectHarnessChoices, ProjectHarnessPreference,
};
use agent_mainframe::gui_project_settings::{
    self, ProjectAgentAvailability, ProjectAgentAvailabilityView,
};
use agent_mainframe::gui_settings::{
    self, AgentAvailability, AgentAvailabilityView, ContextSettingsInput, ContextSettingsView,
    DormancySettings, DormancySettingsView,
};
use tauri::{Manager, State};

#[tauri::command]
pub(crate) async fn feature_presets_load()
-> Result<agent_mainframe::gui_presets::PresetsView, GuiError> {
    agent_mainframe::gui_presets::load()
}

#[tauri::command]
pub(crate) async fn feature_presets_save(
    state: State<'_, AppState>,
    revision: String,
    rows: Vec<agent_mainframe::gui_presets::PresetRow>,
) -> Result<agent_mainframe::gui_presets::PresetsView, GuiError> {
    agent_mainframe::gui_presets::save(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &revision,
        rows,
    )
}

#[tauri::command]
pub(crate) async fn feature_presets_choices(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<agent_mainframe::gui_presets::FeaturePreset>, GuiError> {
    agent_mainframe::gui_presets::creation_choices(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &project_id,
    )
}

#[tauri::command]
pub(crate) async fn context_settings_load() -> Result<ContextSettingsView, GuiError> {
    gui_settings::context_load()
}

#[tauri::command]
pub(crate) async fn context_settings_save(
    state: State<'_, AppState>,
    revision: String,
    settings: ContextSettingsInput,
) -> Result<ContextSettingsView, GuiError> {
    gui_settings::context_save(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &revision,
        settings,
    )
}

#[tauri::command]
pub(crate) async fn agent_availability_load() -> Result<AgentAvailabilityView, GuiError> {
    gui_settings::agents_load()
}

#[tauri::command]
pub(crate) async fn agent_availability_save(
    state: State<'_, AppState>,
    revision: String,
    settings: AgentAvailability,
) -> Result<AgentAvailabilityView, GuiError> {
    gui_settings::agents_save(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &revision,
        settings,
    )
}

#[tauri::command]
pub(crate) async fn project_agent_availability_load(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ProjectAgentAvailabilityView, GuiError> {
    gui_project_settings::load(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &project_id,
    )
}

// Harness probes spawn one `--version` process per harness. They run on a
// blocking thread, not an async runtime worker, and (inside
// `gui_project_preference`) outside the handle lock. A sync command would not
// do: Tauri runs those on the main thread, which would freeze the window.
async fn project_preference_blocking<T: Send + 'static>(
    app: tauri::AppHandle,
    work: impl FnOnce(&Mutex<GuiHandle>) -> T + Send + 'static,
) -> Result<T, GuiError> {
    tauri::async_runtime::spawn_blocking(move || work(&app.state::<AppState>().0))
        .await
        .map_err(|e| GuiError::from(anyhow::anyhow!("Project harness check stopped: {e}")))
}

#[tauri::command]
pub(crate) async fn project_harness_choices(
    app: tauri::AppHandle,
    project_id: String,
) -> Result<ProjectHarnessChoices, GuiError> {
    project_preference_blocking(app, move |gui| {
        gui_project_preference::load(gui, &project_id)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn project_preference_save(
    app: tauri::AppHandle,
    project_id: String,
    revision: String,
    settings: ProjectHarnessPreference,
) -> Result<ProjectHarnessChoices, GuiError> {
    let emit_app = app.clone();
    project_preference_blocking(app, move |gui| {
        let result =
            gui_project_preference::save(gui, &project_id, &revision, settings.preferred_agent);
        // Take the lock again only to broadcast.
        let snapshot = gui
            .lock()
            .expect("gui handle mutex poisoned")
            .broadcast_snapshot();
        emit_workspace_changed(&emit_app, &snapshot);
        result
    })
    .await?
}

#[tauri::command]
pub(crate) async fn project_agent_availability_save(
    state: State<'_, AppState>,
    project_id: String,
    revision: String,
    settings: ProjectAgentAvailability,
) -> Result<ProjectAgentAvailabilityView, GuiError> {
    gui_project_settings::save(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &project_id,
        &revision,
        settings,
    )
}
use agent_mainframe::gui_dormancy::{
    self, DormancyStopResult, DormancyView, DormantDeleteRequest, DormantObservation,
};

#[tauri::command]
pub(crate) async fn diff_layout_save(
    state: State<'_, AppState>,
    layout: gui_settings::DiffViewerLayout,
) -> Result<(), GuiError> {
    gui_settings::diff_layout_save(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        layout,
    )
}

#[tauri::command]
pub(crate) async fn dormancy_settings_load() -> Result<DormancySettingsView, GuiError> {
    gui_settings::load()
}

#[tauri::command]
pub(crate) async fn dormancy_settings_save(
    state: State<'_, AppState>,
    revision: String,
    settings: DormancySettings,
) -> Result<DormancySettingsView, GuiError> {
    gui_settings::save(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &revision,
        settings,
    )
}

#[tauri::command]
pub(crate) async fn dormancy_load(state: State<'_, AppState>) -> Result<DormancyView, GuiError> {
    gui_dormancy::load(&mut state.0.lock().expect("gui handle mutex poisoned"))
}

/// Async so editor cleanup's grace period runs off the main thread. The handle
/// is locked per feature inside `stop`, never for the whole batch, so other
/// commands (terminal input included) wait for at most one feature's stop.
#[tauri::command]
pub(crate) async fn dormancy_stop(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    selection: Vec<DormantObservation>,
) -> Result<Vec<DormancyStopResult>, GuiError> {
    let results = gui_dormancy::stop(&state.0, selection)?;
    let snapshot = state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .broadcast_snapshot();
    emit_workspace_changed(&app, &snapshot);
    Ok(results)
}

/// Deletion can wait for worktree/editor cleanup; keep it off the UI thread.
#[tauri::command]
pub(crate) async fn dormancy_delete(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: DormantDeleteRequest,
    todos: Option<TodoDeleteChoice>,
    todo_host: Option<TodoHostChoice>,
) -> Result<DeleteFeatureResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui_dormancy::delete(&mut gui, request, todos, todo_host);
    // On an error too: validation may have adopted another process's store
    // changes, and deletion can fail after the feature is already gone.
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    response
}
