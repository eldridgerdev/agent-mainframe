use std::sync::Mutex;

use super::super::{AppState, GuiError, emit_workspace_changed};
use agent_mainframe::gui_contract::SessionTarget;
use agent_mainframe::gui_contract::fresh_context::{
    FreshContextPreview, FreshContextRequest, FreshContextResponse,
};
use agent_mainframe::gui_contract::session_sidebar::{
    CompleteSidebarTodo, SessionPlanView, SessionSidebarView,
};
use agent_mainframe::gui_contract::sidebar::SidebarClock;
use tauri::State;

pub(crate) struct SidebarSources(pub(crate) Mutex<SidebarClock>);

impl SidebarSources {
    pub(crate) fn new() -> Self {
        Self(Mutex::new(SidebarClock::default()))
    }
}

/// The agent sidebar for one agent tab. Drives the same background sources
/// the dashboard sidebar does, plus this feature's sidebar load and the
/// usage windows, then answers from this process's caches.

#[tauri::command]
pub(crate) fn session_sidebar(
    state: State<AppState>,
    sources: State<SidebarSources>,
    target: SessionTarget,
) -> Result<SessionSidebarView, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    gui.drive_sidebar_sources(&mut sources.0.lock().expect("sidebar clock mutex poisoned"));
    gui.refresh_session_sidebar_sources(&target)?;
    gui.refresh_usage_windows();
    gui.session_sidebar(&target)
}

#[tauri::command]
pub(crate) fn fresh_context_preview(
    state: State<AppState>,
    target: SessionTarget,
) -> Result<FreshContextPreview, GuiError> {
    state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .fresh_context_preview(&target)
}

#[tauri::command]
pub(crate) fn fresh_context_start(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: SessionTarget,
    request: FreshContextRequest,
) -> Result<FreshContextResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.fresh_context_start(&target, request)?;
    emit_workspace_changed(&app, &gui.broadcast_snapshot());
    Ok(response)
}

#[tauri::command]
pub(crate) fn session_sidebar_plan(
    state: State<AppState>,
    target: SessionTarget,
) -> Result<SessionPlanView, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    gui.session_sidebar_plan(&target)
}

#[tauri::command]
pub(crate) fn session_sidebar_complete_todo(
    state: State<AppState>,
    target: SessionTarget,
    request: CompleteSidebarTodo,
) -> Result<String, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    gui.session_sidebar_complete_todo(&target, request)
}
