use super::super::{AppState, GuiError, GuiHandle, emit_workspace_changed, pr_workspace_blocking};
use agent_mainframe::gui_dedicated_session::{
    self, OperationStart, PrDestinationPrepareRequest, PrDestinationPrepared, PrDestinationPreview,
    PrDestinationRequest, PrDestinationWorkflow,
};
use tauri::Manager;

#[tauri::command]
pub(crate) async fn pr_destination_preview(
    app: tauri::AppHandle,
    request: PrDestinationRequest,
) -> Result<PrDestinationPreview, GuiError> {
    if matches!(
        request.choice,
        agent_mainframe::gui_dedicated_session::PrDestinationChoice::CompanionFeature { .. }
    ) {
        // The remote probe (`git ls-remote`) runs after the lock is released.
        let read = pr_workspace_blocking(app, move |gui| {
            agent_mainframe::gui_companion_creation::preview_read(
                &mut gui.lock().expect("gui handle mutex poisoned"),
                &request,
            )
        })
        .await??;
        return tauri::async_runtime::spawn_blocking(move || read.run())
            .await
            .map_err(|e| GuiError::from(anyhow::anyhow!("Destination preview stopped: {e}")));
    }
    pr_workspace_blocking(app, move |gui| {
        gui_dedicated_session::preview(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &request,
        )
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pr_destination_prepare(
    app: tauri::AppHandle,
    request: PrDestinationPrepareRequest,
) -> Result<PrDestinationPrepared, GuiError> {
    // A companion feature is a cancellable background operation: this returns as
    // soon as the checks pass and the work is running; the panel then polls
    // `pr_destination_status`.
    if matches!(
        request.choice,
        agent_mainframe::gui_dedicated_session::PrDestinationChoice::CompanionFeature { .. }
    ) {
        return pr_companion_start(app, request).await;
    }
    let guard = match gui_dedicated_session::begin_operation(
        &request.workflow,
        gui_dedicated_session::SESSION_OPERATIONS,
    ) {
        OperationStart::Existing(running) => return Ok(running),
        OperationStart::Started(guard) => guard,
    };
    pr_workspace_blocking(app, move |gui| {
        let result = gui_dedicated_session::prepare(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            request,
            &guard.operation_id,
        );
        drop(guard);
        result
    })
    .await?
}

/// Locks the one shared GUI handle on behalf of a background operation.
struct AppLocker(tauri::AppHandle);

impl agent_mainframe::gui_companion_creation::GuiLocker for AppLocker {
    fn lock_gui(&self) -> std::sync::MutexGuard<'_, GuiHandle> {
        self.0
            .state::<AppState>()
            .inner()
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

async fn pr_companion_start(
    app: tauri::AppHandle,
    request: PrDestinationPrepareRequest,
) -> Result<PrDestinationPrepared, GuiError> {
    let locker = std::sync::Arc::new(AppLocker(app.clone()));
    let finished = app.clone();
    let on_finish: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
        let state = finished.state::<AppState>();
        if let Ok(mut handle) = state.inner().0.lock() {
            emit_workspace_changed(&finished, &handle.broadcast_snapshot());
        }
    });
    pr_workspace_blocking(app, move |_| {
        agent_mainframe::gui_companion_creation::start(locker, request, on_finish)
    })
    .await?
}

/// The creation form's lists: harnesses, modes, the suggested name and the hook.
#[tauri::command]
pub(crate) async fn pr_companion_catalog(
    app: tauri::AppHandle,
    workflow: PrDestinationWorkflow,
) -> Result<agent_mainframe::gui_companion_creation::CompanionCatalog, GuiError> {
    pr_workspace_blocking(app, move |gui| {
        agent_mainframe::gui_companion_creation::catalog(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &workflow,
        )
    })
    .await?
}

/// Progress of a companion creation. Never takes the GUI lock.
#[tauri::command]
pub(crate) async fn pr_destination_status(
    operation_id: String,
) -> Result<agent_mainframe::gui_companion_creation::PrDestinationOperation, GuiError> {
    agent_mainframe::gui_companion_creation::status(&operation_id)
}

/// Ask a running companion creation to stop (cooperatively; a checkout that
/// already exists is kept).
#[tauri::command]
pub(crate) async fn pr_destination_cancel(
    operation_id: String,
) -> Result<agent_mainframe::gui_companion_creation::PrDestinationOperation, GuiError> {
    agent_mainframe::gui_companion_creation::cancel(&operation_id)
}

/// Read-only: where a PR fix may go (opened feature + verified companions).
/// The state read holds the GUI mutex only briefly; the Git reads run after it
/// is released.
#[tauri::command]
pub(crate) async fn pr_destination_catalog(
    app: tauri::AppHandle,
    request: agent_mainframe::gui_pr_destinations::DestinationCatalogRequest,
) -> Result<agent_mainframe::gui_pr_destinations::DestinationCatalog, GuiError> {
    let read = pr_workspace_blocking(app, move |gui| {
        agent_mainframe::gui_pr_destinations::catalog_read(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            request,
        )
    })
    .await??;
    tauri::async_runtime::spawn_blocking(move || read.run())
        .await
        .map_err(|e| GuiError::from(anyhow::anyhow!("Destination catalog stopped: {e}")))
}
