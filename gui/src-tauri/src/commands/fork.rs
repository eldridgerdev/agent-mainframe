use std::sync::Mutex;

use super::super::{AppState, GuiError, GuiHandle, emit_workspace_changed};
use agent_mainframe::gui_fork::{
    self, ForkConfirmRequest, ForkOutcome, ForkPrepareRequest, ForkPreview, ForkStatus,
};
use tauri::Manager;

// Forking reads a working-tree snapshot, creates a worktree and may run the
// project's worktree hook, so every step runs on a blocking thread and the
// module itself holds the handle lock only for short checks and registration.
async fn fork_blocking<T: Send + 'static>(
    app: tauri::AppHandle,
    work: impl FnOnce(&Mutex<GuiHandle>) -> T + Send + 'static,
) -> Result<T, GuiError> {
    tauri::async_runtime::spawn_blocking(move || work(&app.state::<AppState>().0))
        .await
        .map_err(|e| GuiError::from(anyhow::anyhow!("Fork operation stopped: {e}")))
}

#[tauri::command]
pub(crate) async fn fork_prepare(
    app: tauri::AppHandle,
    request: ForkPrepareRequest,
) -> Result<ForkPreview, GuiError> {
    fork_blocking(app, move |gui| gui_fork::prepare(gui, request)).await?
}

#[tauri::command]
pub(crate) async fn fork_confirm(
    app: tauri::AppHandle,
    request: ForkConfirmRequest,
) -> Result<ForkOutcome, GuiError> {
    let result = fork_blocking(app.clone(), move |gui| gui_fork::confirm(gui, request)).await?;
    // On an error too: validation may have adopted another process's changes.
    let snapshot = app
        .state::<AppState>()
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .broadcast_snapshot();
    emit_workspace_changed(&app, &snapshot);
    result
}

#[tauri::command]
pub(crate) async fn fork_cancel(
    app: tauri::AppHandle,
    token: String,
) -> Result<ForkStatus, GuiError> {
    fork_blocking(app, move |gui| gui_fork::cancel(gui, &token)).await?
}

#[tauri::command]
pub(crate) async fn fork_status(
    app: tauri::AppHandle,
    token: String,
) -> Result<ForkStatus, GuiError> {
    fork_blocking(app, move |gui| gui_fork::status(gui, &token)).await?
}
