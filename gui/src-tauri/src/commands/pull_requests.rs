use super::super::{AppState, GuiError};
use agent_mainframe::gui_pr_review::{self, PrReviewAction, PrReviewView};
use agent_mainframe::gui_pr_triage::{
    self, PrTriageAction, PrTriagePrefetch, PrTriageReads, PrTriageTarget, PrTriageView,
};
use tauri::State;

// PR Triage reads GitHub through `gh`, which can be slow or hang (network,
// auth prompt, rate limit). Each step names its reads under the handle lock,
// performs them on a blocking thread without it, then applies the results
// under the lock again, so the other commands never queue behind `gh`.
async fn pr_triage_read(reads: PrTriageReads) -> Result<PrTriagePrefetch, GuiError> {
    tauri::async_runtime::spawn_blocking(move || reads.run())
        .await
        .map_err(|e| GuiError::from(anyhow::anyhow!("PR Triage's GitHub read stopped: {e}")))
}

#[tauri::command]
pub(crate) async fn pr_review_begin(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<PrReviewView, GuiError> {
    gui_pr_review::begin(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        project_id,
    )
}

#[tauri::command]
pub(crate) async fn pr_review_snapshot(
    state: State<'_, AppState>,
    workflow_id: String,
) -> Result<PrReviewView, GuiError> {
    gui_pr_review::poll(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
    )
}

#[tauri::command]
pub(crate) async fn pr_review_act(
    state: State<'_, AppState>,
    workflow_id: String,
    revision: u64,
    action: PrReviewAction,
) -> Result<Option<PrReviewView>, GuiError> {
    gui_pr_review::act(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        revision,
        action,
    )
}

#[tauri::command]
pub(crate) async fn pr_triage_begin(
    state: State<'_, AppState>,
    target: PrTriageTarget,
) -> Result<PrTriageView, GuiError> {
    let reads = gui_pr_triage::plan_begin(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &target,
    )?;
    let prefetch = pr_triage_read(reads).await?;
    gui_pr_triage::begin_prefetched(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
        prefetch,
    )
}

#[tauri::command]
pub(crate) async fn pr_triage_snapshot(
    state: State<'_, AppState>,
    workflow_id: String,
) -> Result<PrTriageView, GuiError> {
    let reads = gui_pr_triage::plan_poll(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
    )?;
    let prefetch = pr_triage_read(reads).await?;
    gui_pr_triage::poll_prefetched(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        prefetch,
    )
}

#[tauri::command]
pub(crate) async fn pr_triage_act(
    state: State<'_, AppState>,
    workflow_id: String,
    revision: u64,
    action: PrTriageAction,
) -> Result<Option<PrTriageView>, GuiError> {
    let reads = gui_pr_triage::plan_act(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        revision,
        &action,
    )?;
    let prefetch = pr_triage_read(reads).await?;
    gui_pr_triage::act_prefetched(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &workflow_id,
        revision,
        action,
        prefetch,
    )
}
