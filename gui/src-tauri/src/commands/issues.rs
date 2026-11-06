use std::sync::Mutex;

use super::super::{AppState, GuiError, GuiHandle, emit_workspace_changed};
use agent_mainframe::gui_issues::{
    self, IssueFixRequest, IssueFixResponse, IssueListView, IssuePreview,
};
use tauri::Manager;

// `gh` subprocesses block for seconds. They run on a blocking thread so the
// async runtime workers stay free; the GUI lock is held only briefly inside.
async fn issue_blocking<T: Send + 'static>(
    app: tauri::AppHandle,
    work: impl FnOnce(&Mutex<GuiHandle>) -> T + Send + 'static,
) -> Result<T, GuiError> {
    tauri::async_runtime::spawn_blocking(move || work(&app.state::<AppState>().0))
        .await
        .map_err(|e| GuiError::from(anyhow::anyhow!("GitHub issue operation stopped: {e}")))
}

/// Open issues of a project's GitHub repository (read-only `gh api`), on a
/// blocking thread; the GUI lock is not held during the `gh` calls.
#[tauri::command]
pub(crate) async fn issue_browser_list(
    app: tauri::AppHandle,
    project_id: String,
    page: u32,
) -> Result<IssueListView, GuiError> {
    issue_blocking(app, move |gui| gui_issues::list(gui, &project_id, page)).await?
}

#[tauri::command]
pub(crate) async fn issue_fix_preview(
    app: tauri::AppHandle,
    project_id: String,
    number: u32,
) -> Result<IssuePreview, GuiError> {
    issue_blocking(app, move |gui| {
        gui_issues::preview(gui, &project_id, number)
    })
    .await?
}

/// Creates the issue-fixing feature after the user confirmed a preview.
#[tauri::command]
pub(crate) async fn issue_fix_create(
    app: tauri::AppHandle,
    request: IssueFixRequest,
) -> Result<IssueFixResponse, GuiError> {
    let (response, snapshot) = issue_blocking(app.clone(), move |gui| {
        let response = gui_issues::create(gui, request)?;
        let snapshot = gui
            .lock()
            .expect("gui handle mutex poisoned")
            .broadcast_snapshot();
        Ok::<_, GuiError>((response, snapshot))
    })
    .await??;
    emit_workspace_changed(&app, &snapshot);
    Ok(response)
}
