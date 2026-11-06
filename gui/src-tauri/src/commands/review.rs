use super::super::{AppState, FeatureTarget, GuiError};
use agent_mainframe::gui_review::{self, ReviewAction, ReviewView};
use tauri::State;

#[tauri::command]
pub(crate) async fn review_begin(
    state: State<'_, AppState>,
    target: FeatureTarget,
) -> Result<ReviewView, GuiError> {
    gui_review::begin(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
    )
}

#[tauri::command]
pub(crate) async fn review_snapshot(
    state: State<'_, AppState>,
    workflow_id: String,
) -> Result<Option<ReviewView>, GuiError> {
    gui_review::poll_open(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
    )
}

#[tauri::command]
pub(crate) async fn review_destination_catalog(
    state: State<'_, AppState>,
    workflow_id: String,
    revision: u64,
) -> Result<gui_review::destinations::DestinationCatalog, GuiError> {
    gui_review::destinations::catalog(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        revision,
    )
}
#[tauri::command]
pub(crate) async fn review_destination_preview(
    state: State<'_, AppState>,
    workflow_id: String,
    revision: u64,
    choice: gui_review::destinations::DestinationChoice,
) -> Result<gui_review::destinations::DestinationPreview, GuiError> {
    gui_review::destinations::preview(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        revision,
        choice,
    )
}

#[tauri::command]
pub(crate) fn review_take_completion(
    state: State<AppState>,
    workflow_id: String,
) -> Option<gui_review::ReviewCompletion> {
    gui_review::take_completion(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
    )
}

#[tauri::command]
pub(crate) async fn review_act(
    state: State<'_, AppState>,
    workflow_id: String,
    revision: u64,
    action: ReviewAction,
) -> Result<Option<ReviewView>, GuiError> {
    gui_review::act(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        revision,
        action,
    )
}
