use super::super::{AppState, GuiError};
use agent_mainframe::gui_screenshots::{
    self, EvidenceListing, EvidenceOwner, EvidenceSelection, ImageData,
};
use tauri::State;

#[tauri::command]
pub(crate) async fn screenshots_list(
    state: State<'_, AppState>,
    selection: EvidenceSelection,
) -> Result<EvidenceListing, GuiError> {
    let read = gui_screenshots::plan(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        selection,
    )?;
    let listing = tauri::async_runtime::spawn_blocking(move || read.run())
        .await
        .map_err(|e| GuiError::from(anyhow::anyhow!("Evidence scan stopped: {e}")))?;
    gui_screenshots::finish(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        listing,
    )
}

#[tauri::command]
pub(crate) async fn screenshots_image(
    state: State<'_, AppState>,
    scope_id: String,
    image_id: String,
    hash: String,
    thumbnail: bool,
) -> Result<ImageData, GuiError> {
    let read = gui_screenshots::plan_image(
        &state.0.lock().expect("gui handle mutex poisoned"),
        scope_id,
        image_id,
        hash,
        thumbnail,
    )?;
    let (read, result) = tauri::async_runtime::spawn_blocking(move || {
        let result = read.run();
        (read, result)
    })
    .await
    .map_err(|e| GuiError::from(anyhow::anyhow!("Evidence decode stopped: {e}")))?;
    gui_screenshots::finish_image(&state.0.lock().expect("gui handle mutex poisoned"), &read)?;
    result
}

#[tauri::command]
pub(crate) fn screenshots_changed(state: State<AppState>) -> bool {
    gui_screenshots::changed(&mut state.0.lock().expect("gui handle mutex poisoned"))
}

#[tauri::command]
pub(crate) fn screenshots_cleanup_scopes(
    state: State<AppState>,
) -> Result<Vec<(EvidenceOwner, bool)>, GuiError> {
    gui_screenshots::cleanup_scopes(&state.0.lock().expect("gui handle mutex poisoned"))
}

#[tauri::command]
pub(crate) async fn screenshots_cleanup(
    state: State<'_, AppState>,
    scope_id: String,
) -> Result<(), GuiError> {
    let read = gui_screenshots::plan_cleanup(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &scope_id,
    )?;
    tauri::async_runtime::spawn_blocking(move || read.run())
        .await
        .map_err(|e| GuiError::from(anyhow::anyhow!("Evidence cleanup stopped: {e}")))?
}

#[tauri::command]
pub(crate) async fn screenshots_remote_list(
    state: State<'_, AppState>,
    workflow_id: String,
    selected_run: Option<u64>,
    run_page: u32,
    request_id: String,
) -> Result<gui_screenshots::RemoteListing, GuiError> {
    let read = gui_screenshots::plan_remote(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        selected_run,
        run_page,
        request_id,
    )?;
    let result = tauri::async_runtime::spawn_blocking(move || read.run())
        .await
        .map_err(|e| GuiError::from(anyhow::anyhow!("Screenshot retrieval stopped: {e}")))??;
    gui_screenshots::finish_remote(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        result,
    )
}
#[tauri::command]
pub(crate) async fn screenshots_remote_image(
    state: State<'_, AppState>,
    request_id: String,
    key: String,
    thumbnail: bool,
) -> Result<ImageData, GuiError> {
    let read = gui_screenshots::plan_remote_image(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        request_id,
        &key,
        thumbnail,
    )?;
    let (read, result) = tauri::async_runtime::spawn_blocking(move || {
        let result = read.run();
        (read, result)
    })
    .await
    .map_err(|e| GuiError::from(anyhow::anyhow!("Screenshot retrieval stopped: {e}")))?;
    gui_screenshots::finish_remote_image(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &read,
    )?;
    result
}
#[tauri::command]
pub(crate) fn screenshots_remote_close(state: State<AppState>, request_id: String) {
    gui_screenshots::close_remote(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &request_id,
    );
}
#[tauri::command]
pub(crate) fn screenshots_open_browser(url: String) -> Result<(), GuiError> {
    gui_screenshots::open_gallery_browser(&url)
}

#[tauri::command]
pub(crate) async fn screenshots_pr_document(
    state: State<'_, AppState>,
    workflow_id: String,
) -> Result<String, GuiError> {
    let read = gui_screenshots::plan_pr_document(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
    )?;
    let (read, result) = tauri::async_runtime::spawn_blocking(move || {
        let result = read.run();
        (read, result)
    })
    .await
    .map_err(|e| GuiError::from(anyhow::anyhow!("PR description read stopped: {e}")))?;
    gui_screenshots::finish_pr_document(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &read,
    )?;
    result
}

#[tauri::command]
pub(crate) async fn screenshots_check_access(
    state: State<'_, AppState>,
    workflow_id: String,
    source: String,
) -> Result<Vec<gui_screenshots::GithubAccessCheck>, GuiError> {
    let read = gui_screenshots::plan_inline_image(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        source,
    )?;
    let (read, result) = tauri::async_runtime::spawn_blocking(move || {
        let result = read.check_access();
        (read, result)
    })
    .await
    .map_err(|e| GuiError::from(anyhow::anyhow!("GitHub access check stopped: {e}")))?;
    gui_screenshots::finish_inline_image(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &read,
    )?;
    result
}

#[tauri::command]
pub(crate) async fn screenshots_inline_image(
    state: State<'_, AppState>,
    workflow_id: String,
    source: String,
) -> Result<ImageData, GuiError> {
    let read = gui_screenshots::plan_inline_image(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        source,
    )?;
    let (read, result) = tauri::async_runtime::spawn_blocking(move || {
        let result = read.run();
        (read, result)
    })
    .await
    .map_err(|e| GuiError::from(anyhow::anyhow!("Inline image read stopped: {e}")))?;
    gui_screenshots::finish_inline_image(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &read,
    )?;
    result
}
