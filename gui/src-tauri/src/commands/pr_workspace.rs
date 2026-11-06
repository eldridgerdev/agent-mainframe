use super::super::{AppState, GuiError, pr_workspace_blocking};
use agent_mainframe::gui_dedicated_session::{self, DedicatedCatalog, PrDestinationWorkflow};
use agent_mainframe::gui_pr_review::PrReviewView;
use agent_mainframe::gui_pr_triage;
use agent_mainframe::gui_pr_workspace::{
    self, AiAction as PrWorkspaceAiAction, PrWorkspaceAiView, PrWorkspaceFiles,
};
use tauri::State;

#[tauri::command]
pub(crate) async fn pr_workspace_begin(
    app: tauri::AppHandle,
    target: agent_mainframe::gui_contract::FeatureTarget,
) -> Result<String, GuiError> {
    pr_workspace_blocking(app, move |gui| {
        gui_pr_workspace::begin(&mut gui.lock().expect("gui handle mutex poisoned"), target)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pr_workspace_select_pr(
    app: tauri::AppHandle,
    workflow_id: String,
    number: u32,
) -> Result<(), GuiError> {
    pr_workspace_blocking(app, move |gui| {
        gui_pr_workspace::select_pr(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &workflow_id,
            number,
        )
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pr_workspace_switch_ai(
    app: tauri::AppHandle,
    workflow_id: String,
    number: u32,
) -> Result<(), GuiError> {
    pr_workspace_blocking(app, move |gui| {
        let started = std::time::Instant::now();
        let result = gui_pr_workspace::switch_to_ai(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &workflow_id,
            number,
        );
        if let Ok(mut handle) = gui.lock() {
            gui_pr_workspace::log_ai_tab_switch(
                &mut handle,
                number,
                started.elapsed(),
                result.is_err(),
            );
        }
        result
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pr_workspace_switch_threads(
    app: tauri::AppHandle,
    workflow_id: String,
) -> Result<(), GuiError> {
    pr_workspace_blocking(app, move |gui| {
        gui_pr_workspace::switch_to_threads(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &workflow_id,
        )
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pr_workspace_close(
    app: tauri::AppHandle,
    workflow_id: String,
) -> Result<(), GuiError> {
    pr_workspace_blocking(app, move |gui| {
        gui_pr_workspace::close(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &workflow_id,
        )
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pr_workspace_ai_snapshot(
    app: tauri::AppHandle,
    workflow_id: String,
) -> Result<PrWorkspaceAiView, GuiError> {
    pr_workspace_blocking(app, move |gui| {
        let started = std::time::Instant::now();
        let result = gui_pr_workspace::ai_snapshot(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &workflow_id,
        );
        if let Ok(mut handle) = gui.lock() {
            gui_pr_workspace::log_slow_ai_snapshot(&mut handle, started.elapsed());
        }
        result
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pr_workspace_ai_act(
    app: tauri::AppHandle,
    workflow_id: String,
    action: PrWorkspaceAiAction,
) -> Result<PrWorkspaceAiView, GuiError> {
    let action_name = format!("{action:?}");
    pr_workspace_blocking(app, move |gui| {
        let started = std::time::Instant::now();
        {
            let mut handle = gui.lock().expect("gui handle mutex poisoned");
            gui_pr_workspace::log_ai_action_start(&mut handle, &action);
        }
        let result = gui_pr_workspace::ai_act(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &workflow_id,
            action,
        );
        let elapsed = started.elapsed();
        let error_message = result.as_ref().err().map(|error| error.message.clone());
        if let Ok(mut handle) = gui.lock() {
            gui_pr_workspace::log_ai_action_finish(
                &mut handle,
                &action_name,
                elapsed,
                error_message.as_deref(),
            );
        }
        result
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pr_workspace_fix_targets(
    app: tauri::AppHandle,
    workflow_id: String,
) -> Result<Vec<gui_pr_triage::PrFixTargetView>, GuiError> {
    pr_workspace_blocking(app, move |gui| {
        gui_pr_workspace::fix_targets(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &workflow_id,
        )
    })
    .await?
}

// Dedicated PR triage session (D1): catalog -> preview -> prepare. All three
// run on a blocking thread; prepare additionally claims the workflow's single
// destination slot before taking the GUI lock, so a duplicate submit gets the
// running operation back instead of queueing a second creation.
#[tauri::command]
pub(crate) async fn pr_dedicated_catalog(
    app: tauri::AppHandle,
    workflow: PrDestinationWorkflow,
) -> Result<DedicatedCatalog, GuiError> {
    pr_workspace_blocking(app, move |gui| {
        gui_dedicated_session::catalog(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &workflow,
        )
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pr_workspace_prepare_fix(
    app: tauri::AppHandle,
    request: gui_pr_workspace::FixHandoffRequest,
) -> Result<gui_pr_triage::PrFixHandoff, GuiError> {
    pr_workspace_blocking(app, move |gui| {
        let read = gui_pr_workspace::prepare_fix_read(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            request,
        )?;
        let current = gui_pr_workspace::fetch_fix_pr(&read).map_err(GuiError::from)?;
        gui_pr_workspace::finish_fix_read(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            read,
            current,
        )
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pr_workspace_review_open(
    state: State<'_, AppState>,
    workflow_id: String,
    number: u32,
) -> Result<PrReviewView, GuiError> {
    gui_pr_workspace::review_open(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        number,
    )
}

#[tauri::command]
pub(crate) async fn pr_workspace_review_close(
    state: State<'_, AppState>,
    workflow_id: String,
) -> Result<(), GuiError> {
    gui_pr_workspace::review_close(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
    )
}

#[tauri::command]
pub(crate) async fn pr_workspace_files(
    state: State<'_, AppState>,
    workflow_id: String,
) -> Result<PrWorkspaceFiles, GuiError> {
    let (workdir, number, sha) = gui_pr_workspace::files_request(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
    )?;
    let read_workdir = workdir.clone();
    let patch = tauri::async_runtime::spawn_blocking(move || {
        gui_pr_workspace::read_pr_diff(&read_workdir, number)
    })
    .await
    .map_err(|e| GuiError::from(anyhow::anyhow!("PR diff read stopped: {e}")))?
    .map_err(GuiError::from)?;
    let (current_dir, current_number, current_sha) = gui_pr_workspace::files_request(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
    )?;
    if current_dir != workdir || current_number != number || current_sha != sha {
        return Err(GuiError::from(anyhow::anyhow!(
            "The selected PR changed while files were loading; retry"
        )));
    }
    gui_pr_workspace::files_result(&workflow_id, number, sha, patch)
}
