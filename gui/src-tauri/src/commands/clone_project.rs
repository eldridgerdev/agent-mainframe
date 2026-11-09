//! "New project from a git URL": thin commands over `gui_clone_project`.
//!
//! The clone runs on background threads owned by the shared `App`. Starting a
//! step spawns a short-lived watcher that polls the handle and emits each
//! changed snapshot as `clone_project_changed`, until nothing is running.
use std::path::PathBuf;
use std::time::Duration;

use agent_mainframe::gui_clone_project::{self, CloneProjectSnapshot, DerivedCloneTarget};
use tauri::{AppHandle, Emitter, Manager, State};

use super::super::{AppState, GuiError, emit_workspace_changed};

const EVENT: &str = "clone_project_changed";
const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn emit_snapshot(app: &AppHandle, snapshot: &CloneProjectSnapshot) {
    if let Err(err) = app.emit(EVENT, snapshot) {
        eprintln!("amf-gui: failed to emit {EVENT}: {err}");
    }
}

/// Polls until the flow stops running, emitting every change. Registration
/// adds a project, so a success also refreshes the workspace.
fn watch(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("clone-project-watch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(POLL_INTERVAL);
                let state = app.state::<AppState>();
                let (changed, active, workspace) = {
                    let mut gui = state.0.lock().unwrap_or_else(|p| p.into_inner());
                    let changed = gui_clone_project::poll(&mut gui);
                    let current = changed
                        .clone()
                        .unwrap_or_else(|| gui_clone_project::snapshot(&mut gui));
                    let workspace = changed
                        .as_ref()
                        .is_some_and(|s| {
                            matches!(s.phase, gui_clone_project::ClonePhase::Succeeded { .. })
                        })
                        .then(|| gui.broadcast_snapshot());
                    (changed, gui_clone_project::is_active(&current), workspace)
                };
                if let Some(snapshot) = &changed {
                    emit_snapshot(&app, snapshot);
                }
                if let Some(workspace) = &workspace {
                    emit_workspace_changed(&app, workspace);
                }
                if !active {
                    break;
                }
            }
        });
    if let Err(err) = spawned {
        eprintln!("amf-gui: could not start the clone watcher: {err}");
    }
}

#[tauri::command]
pub(crate) fn clone_project_derive(
    state: State<AppState>,
    url: String,
) -> Result<DerivedCloneTarget, GuiError> {
    gui_clone_project::derive(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &url,
    )
}

#[tauri::command]
pub(crate) fn clone_project_validate(
    state: State<AppState>,
    url: String,
    dest: PathBuf,
    name: String,
) -> Result<(), GuiError> {
    gui_clone_project::validate(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &url,
        &dest,
        &name,
    )
}

#[tauri::command]
pub(crate) fn clone_project_preflight(
    app: AppHandle,
    state: State<AppState>,
    url: String,
    dest: PathBuf,
    name: String,
) -> Result<CloneProjectSnapshot, GuiError> {
    let snapshot = gui_clone_project::start_preflight(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &url,
        &dest,
        &name,
    )?;
    emit_snapshot(&app, &snapshot);
    watch(app);
    Ok(snapshot)
}

#[tauri::command]
pub(crate) fn clone_project_start(
    app: AppHandle,
    state: State<AppState>,
    url: String,
    dest: PathBuf,
    name: String,
) -> Result<CloneProjectSnapshot, GuiError> {
    let snapshot = gui_clone_project::start(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &url,
        &dest,
        &name,
    )?;
    emit_snapshot(&app, &snapshot);
    watch(app);
    Ok(snapshot)
}

#[tauri::command]
pub(crate) fn clone_project_cancel(app: AppHandle, state: State<AppState>) -> CloneProjectSnapshot {
    let snapshot =
        gui_clone_project::cancel(&mut state.0.lock().expect("gui handle mutex poisoned"));
    emit_snapshot(&app, &snapshot);
    snapshot
}

#[tauri::command]
pub(crate) fn clone_project_reset(
    app: AppHandle,
    state: State<AppState>,
) -> Result<CloneProjectSnapshot, GuiError> {
    let snapshot =
        gui_clone_project::reset(&mut state.0.lock().expect("gui handle mutex poisoned"))?;
    emit_snapshot(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
pub(crate) fn clone_project_snapshot(state: State<AppState>) -> CloneProjectSnapshot {
    gui_clone_project::snapshot(&mut state.0.lock().expect("gui handle mutex poisoned"))
}
